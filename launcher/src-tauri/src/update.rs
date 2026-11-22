//! Launcher updates from the GitHub Releases of the project (`plugins.updater` in tauri.conf.json;
//! docs/releasing.md). The player checks, downloads, then restarts into the new version.

use crate::error::{BackendError, Msg, Res};
use crate::{commands, game, log, AppState};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The `pubkey` in tauri.conf.json until the owner sets the real one: updates stay off.
pub const PUBKEY_PLACEHOLDER: &str = "UPDATER-PUBLIC-KEY-NOT-SET";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    available: bool,
    current: String,
    latest: Option<String>,
    notes: Vec<String>,
}

#[derive(Serialize, Clone)]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

enum Stage {
    Idle,
    Found(Update),
    Downloading,
    Ready(Update, Vec<u8>),
}

pub struct Updates(Mutex<Stage>);

impl Default for Updates {
    fn default() -> Self {
        Updates(Mutex::new(Stage::Idle))
    }
}

pub fn configured(app: &AppHandle) -> bool {
    let key = app.config().plugins.0.get("updater").and_then(|u| u.get("pubkey")).and_then(|k| k.as_str());
    key.is_some_and(|k| !k.trim().is_empty() && k != PUBKEY_PLACEHOLDER)
}

/// The release notes as list items: one per non-empty line, without a leading "-" or "*".
pub fn notes(body: Option<&str>) -> Vec<String> {
    body.unwrap_or_default()
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*']).trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn info(app: &AppHandle, update: Option<&Update>) -> UpdateInfo {
    UpdateInfo {
        available: update.is_some(),
        current: app.package_info().version.to_string(),
        latest: update.map(|u| u.version.clone()),
        notes: update.map(|u| notes(u.body.as_deref())).unwrap_or_default(),
    }
}

fn failure(app: &AppHandle, code: &'static str, key: &'static str, what: &str, e: &tauri_plugin_updater::Error) -> BackendError {
    let reason = e.to_string();
    log(app, Msg::new(key, format!("{what} failed: {reason}")).with("reason", reason.as_str()));
    BackendError::new(code, key, format!("{what} failed: {reason}")).with("reason", reason)
}

/// Updating replaces the emulator too, and the installer closes the launcher.
fn refuse_while_busy(app: &AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    if commands::own_game_running(&state) || game::any_emulator_running() {
        return Err(BackendError::new("game_running", "backend.update_game_running", "Close the game before updating the launcher."));
    }
    if state.warmup.is_active() {
        return Err(commands::warmup_running());
    }
    if state.install.is_running() {
        return Err(commands::installing());
    }
    Ok(())
}

pub async fn check(app: &AppHandle) -> Res<UpdateInfo> {
    if !configured(app) {
        let english = "Updates are off: this build has no update key.";
        log(app, Msg::new("log.event.updatesOff", english));
        return Err(BackendError::unsupported("backend.updates_off", english));
    }
    let state = app.state::<AppState>();
    let updates = &state.updates;
    {
        let stage = updates.0.lock().unwrap();
        match &*stage {
            Stage::Downloading => return Err(BackendError::new("busy", "backend.update_downloading", "The update is downloading.")),
            Stage::Ready(u, _) => return Ok(info(app, Some(u))),
            _ => {}
        }
    }
    let found = app.updater().map_err(|e| failure(app, "server_unreachable", "log.event.updateCheckFailed", "Update check", &e))?;
    let found = found.check().await.map_err(|e| failure(app, "server_unreachable", "log.event.updateCheckFailed", "Update check", &e))?;
    if let Some(u) = &found {
        log(app, Msg::new("log.event.updateFound", format!("Launcher {} available", u.version)).with("version", u.version.as_str()));
    }
    let result = info(app, found.as_ref());
    *updates.0.lock().unwrap() = found.map_or(Stage::Idle, Stage::Found);
    Ok(result)
}

pub async fn download(app: &AppHandle) -> Res<()> {
    refuse_while_busy(app)?;
    let state = app.state::<AppState>();
    let updates = &state.updates;
    let update = {
        let mut stage = updates.0.lock().unwrap();
        match std::mem::replace(&mut *stage, Stage::Downloading) {
            Stage::Found(u) => u,
            Stage::Ready(u, bytes) => {
                *stage = Stage::Ready(u, bytes);
                return Ok(());
            }
            other => {
                *stage = other;
                return Err(BackendError::new("invalid_input", "backend.update_none", "No update to download: check for updates first."));
            }
        }
    };
    let (mut downloaded, mut shown) = (0u64, 0u64);
    let result = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                // About one event per percent, or per MiB when the size is unknown.
                let step = total.map_or(1 << 20, |t| (t / 100).max(1));
                if downloaded - shown >= step || total == Some(downloaded) {
                    shown = downloaded;
                    let _ = app.emit("update-progress", UpdateProgress { downloaded, total });
                }
            },
            || {},
        )
        .await;
    let mut stage = updates.0.lock().unwrap();
    match result {
        Ok(bytes) => {
            log(app, Msg::new("log.event.updateDownloaded", format!("Launcher {} downloaded", update.version)).with("version", update.version.as_str()));
            *stage = Stage::Ready(update, bytes);
            Ok(())
        }
        Err(e) => {
            *stage = Stage::Found(update);
            Err(failure(app, "server_unreachable", "log.event.updateDownloadFailed", "Update download", &e))
        }
    }
}

/// Starts the installer of the downloaded update. On Windows the installer closes the launcher
/// and opens the new version when it ends.
pub fn install(app: &AppHandle) -> Res<()> {
    refuse_while_busy(app)?;
    let state = app.state::<AppState>();
    let mut stage = state.updates.0.lock().unwrap();
    let (update, bytes) = match std::mem::replace(&mut *stage, Stage::Idle) {
        Stage::Ready(u, b) => (u, b),
        other => {
            *stage = other;
            return Err(BackendError::new("invalid_input", "backend.update_not_ready", "The update is not downloaded yet."));
        }
    };
    log(app, Msg::new("log.event.updateInstalling", format!("Installing launcher {}", update.version)).with("version", update.version.as_str()));
    match update.install(&bytes) {
        Ok(()) => app.restart(),
        Err(e) => {
            let err = failure(app, "io", "log.event.updateInstallFailed", "Update installation", &e);
            *stage = Stage::Ready(update, bytes);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_become_list_items() {
        assert_eq!(notes(Some("- Faster start\n\n* Fixed the relay\r\nPlain line \n")), ["Faster start", "Fixed the relay", "Plain line"]);
        assert!(notes(None).is_empty());
    }

    #[test]
    fn the_shipped_configuration_is_valid() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];
        assert_eq!(updater["endpoints"], serde_json::json!(["https://github.com/lilium360/FZH-Resurrection/releases/latest/download/latest.json"]));
        assert!(updater["pubkey"].as_str().is_some_and(|k| !k.is_empty()));
        let parsed: tauri_plugin_updater::Config = serde_json::from_value(updater.clone()).unwrap();
        assert!(parsed.require_signed_version);
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    }
}
