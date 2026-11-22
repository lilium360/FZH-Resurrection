pub mod account;
mod accounts;
mod audio;
pub mod avatar;
mod backend_api;
mod clock;
mod commands;
mod dump;
pub mod error;
mod files;
mod game;
pub mod install;
mod instance;
pub mod layout;
mod modpak;
mod pak;
pub mod pfs;
pub mod recovery;
mod report;
mod settings;
mod sfo;
pub mod shadnet;
mod store;
mod update;
pub mod users;
pub mod vault;
mod warmup;

use layout::Layout;
use serde::Serialize;
use std::io::Write;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub struct Running {
    pub id: u64,
    pub child: Child,
}

pub struct AppState {
    pub layout: Layout,
    pub files: Mutex<()>,
    pub busy: AtomicBool,
    pub game: Arc<Mutex<Option<Running>>>,
    pub next_id: AtomicU64,
    pub account: account::Account,
    pub warmup: warmup::Warmup,
    pub install: install::Installer,
    pub updates: update::Updates,
}

/// `line` is the English text (also written to launcher.log); `key` and `params` let the front end
/// translate it. Emulator output has no key and is shown as it is.
#[derive(Serialize, Clone)]
pub struct LogLine {
    pub time: String,
    pub line: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub key: &'static str,
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// Emits a `log` event and appends the English line to `<data>\logs\launcher.log`.
pub fn log(app: &AppHandle, line: impl Into<error::Msg>) {
    let m: error::Msg = line.into();
    let entry = LogLine { time: clock::local_hms(), line: m.message, key: m.key, params: m.params };
    if let Some(state) = app.try_state::<AppState>() {
        let dir = state.layout.data.join("logs");
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("launcher.log")) {
                let _ = writeln!(f, "{} {}", entry.time, entry.line);
            }
        }
    }
    let _ = app.emit("log", entry);
}

pub struct TauriEnv(pub AppHandle);

impl account::Env for TauriEnv {
    fn layout(&self) -> &Layout {
        &self.0.state::<AppState>().inner().layout
    }
    fn account(&self) -> &account::Account {
        &self.0.state::<AppState>().inner().account
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }
    fn log(&self, line: error::Msg) {
        log(&self.0, line);
    }
    fn client_version(&self) -> String {
        format!("FZH-Launcher v{}", self.0.package_info().version)
    }
    fn vault(&self) -> &dyn vault::Vault {
        &vault::System
    }
}

pub fn env(app: &AppHandle) -> account::EnvRef {
    Arc::new(TauriEnv(app.clone()))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data = app.path().app_local_data_dir()?;
            let resources = app.path().resource_dir().ok();
            app.manage(AppState {
                layout: Layout::resolve(data, resources),
                files: Mutex::new(()),
                busy: AtomicBool::new(false),
                game: Arc::new(Mutex::new(None)),
                next_id: AtomicU64::new(1),
                account: account::Account::default(),
                warmup: warmup::Warmup::default(),
                install: install::Installer::default(),
                updates: update::Updates::default(),
            });
            account::Account::start_monitor(env(app.handle()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_launcher_state,
            commands::set_setup_completed,
            commands::check_for_update,
            commands::download_update,
            commands::install_update,
            commands::open_link,
            commands::save_problem_report,
            commands::load_settings,
            commands::save_settings,
            commands::list_audio_devices,
            commands::get_game_files,
            commands::pick_folder,
            commands::select_game_folder,
            commands::reset_game_files,
            commands::prepare_game,
            commands::get_install_plan,
            commands::set_install_location,
            commands::install_game,
            commands::cancel_install,
            commands::delete_install,
            commands::get_game_state,
            commands::launch_game,
            commands::stop_game,
            commands::get_controller_check,
            commands::get_shader_cache,
            commands::start_shader_warmup,
            commands::cancel_shader_warmup,
            commands::clear_shader_cache,
            commands::get_account,
            commands::sign_in,
            commands::register_account,
            commands::sign_out,
            commands::update_profile,
            commands::upload_avatar,
            commands::set_recovery_email,
            commands::get_friends,
            commands::send_friend_request,
            commands::accept_friend_request,
            commands::decline_friend_request,
            commands::remove_friend,
            commands::get_server_status,
            commands::get_hosting,
            commands::start_hosting,
            commands::stop_hosting,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the launcher")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.warmup.cancel();
                }
            }
        });
}
