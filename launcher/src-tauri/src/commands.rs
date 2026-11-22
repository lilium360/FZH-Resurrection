use crate::account::{AccountState, ProfileUpdate, Registration, ServerStatus, Session};
use crate::dump::{self, Detection, DumpInfo, Kind};
use crate::shadnet::friends::FriendsState;
use crate::error::{bytes, seconds, BackendError, Msg, Res};
use crate::game::{self, LaunchPlan, LogTail};
use crate::install::{self, InstallInfo, InstallPlan, Leftover, Progress};
use crate::report::ProblemReport;
use crate::settings::Settings;
use crate::store::{self, LauncherFile};
use crate::warmup::{self, ShaderCacheState};
use crate::{account, audio, env, files, instance, log, report, users, vault, AppState, Running};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    launcher_version: String,
    emulator_commit: String,
    default_server: String,
    /// This build has an update key: false shows the version without update controls.
    updates: bool,
    /// Hosting the services from the launcher is not implemented: the UI disables it.
    hosting: bool,
}

const SOURCE_URL: &str = "https://github.com/lilium360/FZH-Resurrection";
const LICENCE_URL: &str = "https://github.com/lilium360/FZH-Resurrection/blob/main/LICENSE";
/// Where the player attaches the saved problem report.
const ISSUES_URL: &str = "https://github.com/lilium360/FZH-Resurrection/issues/new/choose";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherState {
    setup_completed: bool,
}

#[derive(Serialize)]
pub struct GameFiles {
    base: Option<DumpInfo>,
    update: Option<DumpInfo>,
    prepared: bool,
    /// Where the chosen PKG files are (or go) extracted; null when no PKG file is chosen.
    install: Option<InstallInfo>,
    /// Folders extracted earlier that the current choice does not use.
    leftovers: Vec<Leftover>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum FolderCheck {
    Ready { files: GameFiles },
    NeedUpdate { base: DumpInfo, note: Option<Msg> },
    NeedBase { update: DumpInfo, note: Option<Msg> },
    Rejected { path: String, found: Msg, error: Msg },
}


#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GameState {
    running: bool,
    exit_code: Option<i32>,
}

#[derive(Serialize, Clone)]
struct PrepareProgress {
    stage: &'static str,
    percent: u32,
}

/// A launch to play, or one round of the shader cache warm-up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LaunchMode {
    Play,
    Warmup,
}

fn joined<T: Send + 'static>(r: Result<Res<T>, tauri::Error>) -> Res<T> {
    r.map_err(|e| BackendError::internal(format!("The launcher task failed: {e}")))?
}

fn game_files(state: &AppState, file: &LauncherFile) -> GameFiles {
    let layout = &state.layout;
    GameFiles {
        base: file.base.clone(),
        update: file.update.clone(),
        prepared: files::is_prepared(layout, file),
        install: install::info(layout, file, state.install.is_running()),
        leftovers: install::leftovers(layout, file),
    }
}

pub(crate) fn installing() -> BackendError {
    BackendError::new("busy", "backend.busy_installing", "The game is being installed: wait until it ends or cancel it on the Game screen.")
}

fn not_signed_in() -> BackendError {
    BackendError::new("not_signed_in", "backend.not_signed_in_play", "Sign in to your account before playing.")
}

pub(crate) fn warmup_running() -> BackendError {
    BackendError::new(
        "busy",
        "backend.warmup_running",
        "The shader cache warm-up is running: wait for it to end or cancel it on the Graphics screen.",
    )
}

fn game_running(key: &'static str, english: &str) -> BackendError {
    BackendError::new("game_running", key, english)
}

/// The log line of a failed action: "<what> failed: <reason>".
fn failed(key: &'static str, what: &str, e: &BackendError) -> Msg {
    Msg::new(key, format!("{what} failed: {}", e.message)).with("reason", e.msg())
}

pub(crate) fn own_game_running(state: &AppState) -> bool {
    let mut guard = state.game.lock().unwrap();
    match guard.as_mut() {
        Some(r) => matches!(r.child.try_wait(), Ok(None)),
        None => false,
    }
}

// ---------- App ----------

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> Res<AppInfo> {
    Ok(AppInfo {
        launcher_version: app.package_info().version.to_string(),
        emulator_commit: state.layout.emulator_commit(),
        default_server: crate::settings::default_server().to_string(),
        updates: crate::update::configured(&app),
        hosting: false,
    })
}

#[tauri::command]
pub fn get_launcher_state(state: State<'_, AppState>) -> Res<LauncherState> {
    Ok(LauncherState { setup_completed: store::load_launcher(&state.layout.launcher_json()).setup_completed })
}

#[tauri::command]
pub fn set_setup_completed(state: State<'_, AppState>, done: bool) -> Res<()> {
    let _g = state.files.lock().unwrap();
    let path = state.layout.launcher_json();
    let mut file = store::load_launcher(&path);
    file.setup_completed = done;
    store::save_launcher(&path, &file)
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Res<crate::update::UpdateInfo> {
    crate::update::check(&app).await
}

#[tauri::command]
pub async fn download_update(app: AppHandle) -> Res<()> {
    crate::update::download(&app).await
}

#[tauri::command]
pub fn install_update(app: AppHandle) -> Res<()> {
    crate::update::install(&app)
}

#[tauri::command]
pub fn open_link(app: AppHandle, state: State<'_, AppState>, target: String) -> Res<()> {
    match target.as_str() {
        "source" | "licence" | "issues" => {
            let url = match target.as_str() {
                "source" => SOURCE_URL,
                "licence" => LICENCE_URL,
                _ => ISSUES_URL,
            };
            app.opener().open_url(url, None::<&str>).map_err(|e| BackendError::internal(format!("Cannot open the link: {e}")))
        }
        "licences" => {
            let dir = state.layout.licences.clone();
            if !dir.is_dir() {
                return Err(BackendError::new("not_found", "backend.licence_missing", format!("The licence texts are missing: {}", dir.display()))
                    .with("path", dir.display().to_string()));
            }
            app.opener()
                .open_path(dir.display().to_string(), None::<&str>)
                .map_err(|e| BackendError::internal(format!("Cannot open the licence folder: {e}")))
        }
        _ => Err(BackendError::internal(format!("unknown link {target}"))),
    }
}

/// Asks where to save, then writes one text file (`report.rs`). Null when the player cancels.
/// `title` is the save dialog's title in the launcher language.
#[tauri::command]
pub async fn save_problem_report(app: AppHandle, report: ProblemReport, title: String) -> Res<Option<String>> {
    let name = format!("fzh-report-{}.txt", crate::clock::local_date_time().replace([' ', ':'], "-"));
    let picker = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = picker.dialog().file().set_title(title).set_file_name(&name).add_filter("Text", &["txt"]);
        if let Ok(desktop) = picker.path().desktop_dir() {
            dialog = dialog.set_directory(desktop);
        }
        dialog.blocking_save_file()
    })
    .await
    .map_err(|e| BackendError::internal(format!("The save dialog failed: {e}")))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    blocking(&app, move |a| {
        let state = a.state::<AppState>();
        let text = report::build(&state.layout, &report, &a.package_info().version.to_string());
        std::fs::write(&path, text).map_err(|e| BackendError::io_path(path.display(), e))?;
        log(a, Msg::new("log.event.reportSaved", "Launcher: problem report saved"));
        Ok(Some(path.display().to_string()))
    })
    .await
}

// ---------- Settings ----------

#[tauri::command]
pub fn load_settings(state: State<'_, AppState>) -> Res<Settings> {
    Ok(store::load_settings(&state.layout.settings_json()))
}

/// A change of server also changes the account in use (`Account::change_server`).
#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Res<Settings> {
    let normalized = settings.normalized()?;
    blocking(&app, move |a| {
        let state = a.state::<AppState>();
        let path = state.layout.settings_json();
        let (from, to) = (store::load_settings(&path).server(), normalized.server());
        let write = || store::save_settings(&path, &normalized);
        if vault::server_key(&from) == vault::server_key(&to) {
            write()?;
        } else {
            state.account.change_server(&env(a), &from, &to, write)?;
        }
        state.account.wake();
        Ok(normalized)
    })
    .await
}

#[tauri::command]
pub async fn list_audio_devices(app: AppHandle) -> Res<audio::AudioDevices> {
    blocking(&app, |_| Ok(audio::list())).await
}

// ---------- Game files ----------

#[tauri::command]
pub fn get_game_files(state: State<'_, AppState>) -> Res<GameFiles> {
    let file = store::load_launcher(&state.layout.launcher_json());
    Ok(game_files(&state, &file))
}

/// `title` is the dialog's title in the launcher language (`folderPicker.<kind>`).
#[tauri::command]
pub async fn pick_folder(app: AppHandle, title: String) -> Res<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(title).blocking_pick_folder()).await;
    let picked = picked.map_err(|e| BackendError::internal(format!("The folder dialog failed: {e}")))?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

#[tauri::command]
pub fn select_game_folder(app: AppHandle, state: State<'_, AppState>,path: String, expect: Option<Kind>) -> Res<FolderCheck> {
    if state.install.is_running() {
        return Err(installing());
    }
    let _g = state.files.lock().unwrap();
    let file_path = state.layout.launcher_json();
    let mut file = store::load_launcher(&file_path);
    let stored = match expect {
        Some(Kind::Update) => file.base.clone().filter(|_| file.update.is_none()),
        Some(Kind::Base) => file.update.clone().filter(|_| file.base.is_none()),
        None => None,
    };
    let detection = dump::detect(Path::new(&path), expect, stored.as_ref());
    file.prepared = false;
    match &detection {
        Detection::Ready(b, u) => (file.base, file.update) = (Some(b.clone()), Some(u.clone())),
        Detection::NeedUpdate { base, .. } => (file.base, file.update) = (Some(base.clone()), None),
        Detection::NeedBase { update, .. } => (file.base, file.update) = (None, Some(update.clone())),
        Detection::Rejected(_) if expect.is_none() => (file.base, file.update) = (None, None),
        Detection::Rejected(_) => {}
    }
    if matches!(detection, Detection::Ready(..)) {
        install::choose_location(&state.layout, &mut file);
        if install::adopt(&state.layout, &mut file) {
            log(&app, already_extracted());
        }
    }
    store::save_launcher(&file_path, &file)?;
    Ok(match detection {
        Detection::Ready(..) => FolderCheck::Ready { files: game_files(&state, &file) },
        Detection::NeedUpdate { base, note } => FolderCheck::NeedUpdate { base, note },
        Detection::NeedBase { update, note } => FolderCheck::NeedBase { update, note },
        Detection::Rejected(r) => FolderCheck::Rejected { path, found: r.found, error: r.error },
    })
}

#[tauri::command]
pub fn reset_game_files(state: State<'_, AppState>) -> Res<GameFiles> {
    if state.install.is_running() {
        return Err(installing());
    }
    let _g = state.files.lock().unwrap();
    let path = state.layout.launcher_json();
    let mut file = store::load_launcher(&path);
    file.base = None;
    file.update = None;
    file.prepared = false;
    store::save_launcher(&path, &file)?;
    Ok(game_files(&state, &file))
}

fn already_extracted() -> Msg {
    Msg::new("log.event.alreadyExtracted", "Installing: the game is already extracted there, nothing to do")
}

fn not_ready(file: &LauncherFile) -> BackendError {
    if file.base.is_some() && file.update.is_some() {
        BackendError::new("game_not_ready", "backend.not_ready_install", "Install the game from your PKG files first.")
    } else {
        BackendError::new("game_not_ready", "backend.not_ready_choose", "Choose the game folder first.")
    }
}

fn progress(app: &AppHandle, stage: &'static str, percent: u32) {
    let _ = app.emit("prepare-progress", PrepareProgress { stage, percent });
}

/// Links the folders, writes the mod pak and installs the emulator patches; the caller holds `files`.
fn prepare_files(app: &AppHandle, file: &mut LauncherFile) -> Res<()> {
    let state = app.state::<AppState>();
    let layout = &state.layout;
    let path = layout.launcher_json();
    let Some((base, update)) = install::effective(layout, file) else {
        return Err(not_ready(file));
    };
    for (kind, info) in [(Kind::Base, &base), (Kind::Update, &update)] {
        if let Err(r) = dump::check(kind, Path::new(&info.path)) {
            return Err(BackendError::new("game_not_ready", "backend.folder_problem", format!("{}: {}", info.path, r.error.message))
                .with("path", info.path.clone())
                .with("error", r.error));
        }
    }
    file.prepared = false;
    store::save_launcher(&path, file)?;

    progress(app, "linking", 0);
    files::make_links(layout, &base, &update)?;
    log(
        app,
        if files::separate_update(&base, &update) {
            Msg::new("log.event.linkedSeparate", "Preparing: base game and update linked, nothing copied")
        } else {
            Msg::new("log.event.linkedMerged", "Preparing: merged game folder linked, nothing copied")
        },
    );

    progress(app, "mod", 40);
    let inputs = store::load_settings(&layout.settings_json()).pak_inputs();
    let size = files::build_mod_pak(layout, &base, &update, &inputs)?;
    file.mod_pak_inputs = Some(inputs);
    log(app, Msg::new("log.event.modPakWritten", format!("Preparing: mod pak written ({size} bytes)")).with("size", bytes(size as u64)));

    progress(app, "patches", 75);
    let names = instance::install_patches(&layout.patches, &layout.user_dir(), &store::load_settings(&layout.settings_json()))?;
    log(
        app,
        Msg::new("log.event.patchesInstalled", format!("Preparing: emulator patches installed ({})", names.join(", "))).with("names", names.join(", ")),
    );

    file.prepared = true;
    store::save_launcher(&path, file)?;
    progress(app, "done", 100);
    log(app, Msg::new("log.event.gameReady", "Preparing: game ready"));
    Ok(())
}

fn do_prepare(app: &AppHandle) -> Res<GameFiles> {
    let state = app.state::<AppState>();
    let _g = state.files.lock().unwrap();
    let mut file = store::load_launcher(&state.layout.launcher_json());
    prepare_files(app, &mut file)?;
    Ok(game_files(&state, &file))
}

#[tauri::command]
pub async fn prepare_game(app: AppHandle) -> Res<GameFiles> {
    let state = app.state::<AppState>();
    if state.warmup.is_active() {
        return Err(warmup_running());
    }
    if own_game_running(&state) || game::any_emulator_running() {
        return Err(game_running("backend.game_running_prepare", "Close the game first: the launcher prepares the game files when it is not running."));
    }
    if state.install.is_running() {
        return Err(installing());
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err(BackendError::new("busy", "backend.busy_preparing", "The launcher is already preparing the game."));
    }
    let worker = app.clone();
    let result = joined(tauri::async_runtime::spawn_blocking(move || do_prepare(&worker)).await);
    state.busy.store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        log(&app, failed("log.event.prepareFailed", "Preparing", e));
    }
    result
}

// ---------- Installing from PKG files ----------

#[tauri::command]
pub async fn get_install_plan(app: AppHandle) -> Res<InstallPlan> {
    blocking(&app, |a| {
        let state = a.state::<AppState>();
        let file = store::load_launcher(&state.layout.launcher_json());
        install::plan_for(&state.layout, &file).map(|(_, summary)| summary)
    })
    .await
}

#[tauri::command]
pub async fn set_install_location(app: AppHandle, path: String) -> Res<InstallPlan> {
    blocking(&app, move |a| {
        let state = a.state::<AppState>();
        if state.install.is_running() {
            return Err(installing());
        }
        let dir = PathBuf::from(path.trim());
        if !dir.is_absolute() {
            return Err(BackendError::invalid("backend.choose_local_drive", "Choose a folder on a local drive."));
        }
        let _g = state.files.lock().unwrap();
        let launcher = state.layout.launcher_json();
        let mut file = store::load_launcher(&launcher);
        file.install_location = if install::default_location(&state.layout, &file) == dir { None } else { Some(dir.display().to_string()) };
        file.prepared = false;
        if install::adopt(&state.layout, &mut file) {
            log(a, already_extracted());
        }
        store::save_launcher(&launcher, &file)?;
        install::plan_for(&state.layout, &file).map(|(_, summary)| summary)
    })
    .await
}

fn do_install(app: &AppHandle) -> Res<GameFiles> {
    let state = app.state::<AppState>();
    let layout = &state.layout;
    let launcher = layout.launcher_json();
    let (plan, summary, target) = {
        let _g = state.files.lock().unwrap();
        let mut file = store::load_launcher(&launcher);
        let (plan, summary) = install::plan_for(layout, &file)?;
        if let Some(e) = summary.error {
            return Err(BackendError::from_msg("io", e));
        }
        let target = PathBuf::from(&summary.target);
        install::remember(&mut file, &target, &plan);
        file.prepared = false;
        store::save_launcher(&launcher, &file)?;
        (plan, summary, target)
    };
    log(
        app,
        Msg::new(
            "log.event.installStarted",
            format!(
                "Installing: {} into {} ({} to write, {} already there)",
                install::gb(summary.total),
                target.display(),
                install::gb(summary.required),
                install::gb(summary.present)
            ),
        )
        .with("total", bytes(summary.total))
        .with("target", target.display().to_string())
        .with("required", bytes(summary.required))
        .with("present", bytes(summary.present)),
    );
    let workers = std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(2).clamp(1, 6);
    let started = std::time::Instant::now();
    let mut last: Option<std::time::Instant> = None;
    let result = install::extract(&plan, &target, workers, &state.install.cancel, &mut |p| {
        if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            last = Some(std::time::Instant::now());
            let _ = app.emit("install-progress", p);
        }
    });
    drop(plan);
    let secs = started.elapsed().as_secs_f64().max(0.001);
    let _g = state.files.lock().unwrap();
    let mut file = store::load_launcher(&launcher);
    match result {
        Ok(true) => {
            install::mark_complete(&mut file, &target);
            store::save_launcher(&launcher, &file)?;
            let mb_s = (summary.required as f64 / (1u64 << 20) as f64 / secs).round() as u64;
            let english = format!("Installing: done in {}", duration(secs));
            log(
                app,
                if secs >= 1.0 {
                    Msg::new("log.event.installDoneSpeed", format!("{english} ({mb_s} MB/s)")).with("speed", mb_s)
                } else {
                    Msg::new("log.event.installDone", english)
                }
                .with("duration", seconds(secs.round() as u64)),
            );
            let _ = app.emit("install-progress", Progress { done: summary.total, total: summary.total, file: String::new() });
        }
        Ok(false) => {
            let files = install::record_for(&file, &target).map(|r| r.files.clone()).unwrap_or_default();
            let size = install::size_on_disk(&target, &files);
            log(
                app,
                Msg::new("log.event.installCancelled", format!("Installing: cancelled; the {} extracted so far stay for the next time", install::gb(size)))
                    .with("size", bytes(size)),
            );
        }
        Err(e) => {
            return Err(BackendError::new("io", "backend.install_failed", format!("Installing failed: {e}")).with("detail", e.to_string()))
        }
    }
    Ok(game_files(&state, &file))
}

fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s < 60 { format!("{s} s") } else { format!("{} min {:02} s", s / 60, s % 60) }
}

/// Extracts the chosen PKG files; returns when it ends or is cancelled (`install.complete` false then).
#[tauri::command]
pub async fn install_game(app: AppHandle) -> Res<GameFiles> {
    let state = app.state::<AppState>();
    if state.warmup.is_active() {
        return Err(warmup_running());
    }
    if state.busy.load(Ordering::SeqCst) {
        return Err(BackendError::new("busy", "backend.warmup_prep", "The launcher is preparing the game: try again in a moment."));
    }
    if state.install.running.swap(true, Ordering::SeqCst) {
        return Err(installing());
    }
    state.install.cancel.store(false, Ordering::SeqCst);
    let result = blocking(&app, do_install).await.map(|mut files| {
        if let Some(i) = files.install.as_mut() {
            i.running = false;
        }
        files
    });
    state.install.running.store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        log(&app, failed("log.event.installFailed", "Installing", e));
    }
    result
}

#[tauri::command]
pub fn cancel_install(state: State<'_, AppState>) -> Res<()> {
    if state.install.is_running() {
        state.install.cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// Deletes a game folder the launcher extracted (only the files it wrote); the PKG files stay.
#[tauri::command]
pub async fn delete_install(app: AppHandle, target: String) -> Res<GameFiles> {
    blocking(&app, move |a| {
        let state = a.state::<AppState>();
        if state.install.is_running() {
            return Err(installing());
        }
        if own_game_running(&state) || game::any_emulator_running() || state.warmup.is_active() {
            return Err(game_running("backend.game_running_delete", "Close the game first: it may be using these files."));
        }
        let _g = state.files.lock().unwrap();
        let launcher = state.layout.launcher_json();
        let mut file = store::load_launcher(&launcher);
        let Some(record) = file.installs.iter().find(|r| r.target == target).cloned() else {
            return Err(BackendError::new("not_found", "backend.not_found_install", "The launcher did not install a game there."));
        };
        let dir = PathBuf::from(&record.target);
        let size = install::size_on_disk(&dir, &record.files);
        let left = install::delete(&dir, &record.files).map_err(|e| BackendError::io_path(dir.display(), e))?;
        file.installs.retain(|r| r.target != target);
        if install::target(&state.layout, &file).is_some_and(|t| t == dir) {
            file.prepared = false;
        }
        store::save_launcher(&launcher, &file)?;
        let english = format!("Launcher: installed game deleted from {} ({} freed)", dir.display(), install::gb(size));
        let line = if left == 0 {
            Msg::new("log.event.installDeleted", english)
        } else {
            Msg::new("log.event.installDeletedKept", format!("{english}; {left} other files there were kept")).with("count", left)
        };
        log(a, line.with("target", dir.display().to_string()).with("size", bytes(size)));
        Ok(game_files(&state, &file))
    })
    .await
}

// ---------- Game ----------

#[tauri::command]
pub fn get_game_state(state: State<'_, AppState>) -> Res<GameState> {
    let foreign = !state.warmup.is_active() && game::any_emulator_running();
    Ok(GameState { running: own_game_running(&state) || foreign, exit_code: None })
}

fn write_launch_record(state: &AppState, plan: &LaunchPlan) {
    let mut text = format!("cwd: {}\ncommand: {}\n", plan.cwd.display(), plan.command_line());
    for (k, v) in &plan.env {
        text.push_str(&format!("{k}={v}\n"));
    }
    let dir = state.layout.logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("last-launch.txt"), text);
}

/// Everything before starting the emulator: preparation if needed, the pak for these inputs, the
/// instance configuration, the account, the log rotation. A warm-up round uses the one-player
/// lobby pak, the gamepad, no microphone, no headset and the input script.
pub(crate) fn prepare_launch(app: &AppHandle, mode: LaunchMode) -> Res<LaunchPlan> {
    let state = app.state::<AppState>();
    let layout = &state.layout;
    let _g = state.files.lock().unwrap();
    let path = layout.launcher_json();
    let mut file = store::load_launcher(&path);
    if !layout.emulator.is_file() {
        return Err(BackendError::new("not_found", "backend.emulator_missing", format!("The emulator is missing: {}", layout.emulator.display()))
            .with("path", layout.emulator.display().to_string()));
    }
    if !files::is_prepared(layout, &file) {
        log(app, Msg::new("log.event.preparingNow", "Launcher: the game files are not prepared, preparing them now"));
        prepare_files(app, &mut file)?;
    }
    let Some((base, update)) = install::effective(layout, &file) else {
        return Err(not_ready(&file));
    };
    let settings = store::load_settings(&layout.settings_json());
    let warm = mode == LaunchMode::Warmup;
    let inputs = if warm { settings.warmup_pak_inputs() } else { settings.pak_inputs() };
    if file.mod_pak_inputs.as_ref() != Some(&inputs) || !layout.mod_pak().is_file() {
        files::build_mod_pak(layout, &base, &update, &inputs)?;
        file.mod_pak_inputs = Some(inputs);
        store::save_launcher(&path, &file)?;
        log(
            app,
            if warm {
                Msg::new("log.event.modPakWarmup", "Warm-up: mod pak rebuilt for a one-player lobby")
            } else {
                Msg::new("log.event.modPakRebuilt", "Launcher: mod pak rebuilt for the new settings")
            },
        );
    }
    let run_settings = if warm { settings.for_warmup() } else { settings.clone() };
    let user = layout.user_dir();
    instance::apply(&user, &run_settings)?;
    instance::install_patches(&layout.patches, &user, &run_settings)?;
    let users_json = layout.users_json();
    let creds = account::credentials(&env(app)).ok_or_else(not_signed_in)?;
    instance::write_json(&users_json, &users::set_account(instance::read_json(&users_json), &creds))?;
    let log_file = layout.log_file();
    if log_file.is_file() {
        let _ = std::fs::rename(&log_file, log_file.with_file_name("shad_log.previous.txt"));
    }
    let env = if warm {
        settings.warmup_environment(&layout.warmup_script.display().to_string())
    } else {
        settings.environment()
    };
    let mut plan = LaunchPlan::new(&layout.emulator, &layout.eboot(), &layout.run_dir(), env);
    plan.args.extend(run_settings.extra_args().into_iter().map(Into::into));
    write_launch_record(&state, &plan);
    if !warm {
        let server = settings.server_name();
        log(app, Msg::new("log.event.playingOn", format!("Launcher: playing on the {}", server.message)).with("server", server));
    }
    Ok(plan)
}

/// After a warm-up: the mod pak for the normal lobby again.
pub(crate) fn restore_pak(app: &AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    let layout = &state.layout;
    let _g = state.files.lock().unwrap();
    let path = layout.launcher_json();
    let mut file = store::load_launcher(&path);
    let Some((base, update)) = install::effective(layout, &file) else { return Ok(()) };
    let inputs = store::load_settings(&layout.settings_json()).pak_inputs();
    if file.mod_pak_inputs.as_ref() == Some(&inputs) && layout.mod_pak().is_file() {
        return Ok(());
    }
    files::build_mod_pak(layout, &base, &update, &inputs)?;
    file.mod_pak_inputs = Some(inputs);
    store::save_launcher(&path, &file)
}

fn watch(app: AppHandle, id: u64, log_file: PathBuf) {
    let state = app.state::<AppState>();
    let mut tail = LogTail::new(log_file);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        for line in tail.poll() {
            log(&app, line);
        }
        let mut guard = state.game.lock().unwrap();
        let exited = match guard.as_mut() {
            Some(r) if r.id == id => match r.child.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            },
            _ => return,
        };
        if let Some(code) = exited {
            *guard = None;
            drop(guard);
            for line in tail.poll() {
                log(&app, line);
            }
            log(&app, match code {
                Some(0) => Msg::new("log.event.gameClosed", "Game closed"),
                Some(c) => Msg::new("log.event.gameExitedCode", format!("Game exited (code {c})")).with("code", c),
                None => Msg::new("log.event.gameExited", "Game exited"),
            });
            let _ = app.emit("game-state", GameState { running: false, exit_code: code });
            state.account.resume();
            return;
        }
    }
}

#[tauri::command]
pub async fn launch_game(app: AppHandle) -> Res<Option<Msg>> {
    let state = app.state::<AppState>();
    if state.warmup.is_active() {
        return Err(warmup_running());
    }
    if own_game_running(&state) || game::any_emulator_running() {
        return Err(game_running("backend.game_already_running", "The game is already running."));
    }
    if state.install.is_running() {
        return Err(installing());
    }
    if !state.account.is_signed_in(&env(&app)) {
        return Err(not_signed_in());
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err(BackendError::new("busy", "backend.busy_preparing_game", "The launcher is busy preparing the game."));
    }
    let plan = blocking(&app, |a| {
        let account = &a.state::<AppState>().account;
        let warning = account.play_grant(&env(a))?;
        let plan = prepare_launch(a, LaunchMode::Play)?;
        account.suspend(&env(a));
        Ok((plan, warning))
    })
    .await;
    let result = plan.and_then(|(plan, warning)| {
        let child = plan.spawn().map_err(|e| {
            state.account.resume();
            BackendError::new("io", "backend.cannot_start_emulator", format!("Cannot start the emulator: {e}"))
        })?;
        let pid = child.id();
        let id = state.next_id.fetch_add(1, Ordering::SeqCst);
        *state.game.lock().unwrap() = Some(Running { id, child });
        log(&app, Msg::new("log.event.gameStarted", format!("Launcher: game started (process {pid})")).with("pid", pid));
        let _ = app.emit("game-state", GameState { running: true, exit_code: None });
        let watcher = app.clone();
        let log_file = state.layout.log_file();
        std::thread::spawn(move || watch(watcher, id, log_file));
        Ok(warning)
    });
    state.busy.store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        log(&app, failed("log.event.launchFailed", "Launch", e));
    }
    result
}

#[tauri::command]
pub async fn stop_game(app: AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    let taken = state.game.lock().unwrap().take();
    match taken {
        Some(mut r) => {
            tauri::async_runtime::spawn_blocking(move || {
                let _ = r.child.kill();
                let _ = r.child.wait();
            })
            .await
            .map_err(|e| BackendError::internal(format!("Cannot stop the game: {e}")))?;
            log(&app, Msg::new("log.event.gameStopped", "Launcher: game stopped"));
            let _ = app.emit("game-state", GameState { running: false, exit_code: None });
            state.account.resume();
            Ok(())
        }
        None if state.warmup.is_active() => Err(warmup_running()),
        None if game::any_emulator_running() => Err(game_running(
            "backend.emulator_not_ours",
            "The emulator running now was not started by the launcher: close it from its own window.",
        )),
        None => {
            let _ = app.emit("game-state", GameState { running: false, exit_code: None });
            Ok(())
        }
    }
}

// ---------- Controller check ----------

/// Which buttons of the headset's controllers reached the game in the last session, from the
/// emulator's "VR input:" log lines.
#[derive(Serialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ControllerCheck {
    /// The OpenXR interaction profile of the right controller, "" when the log has none.
    profile: String,
    /// Presses by gamepad button: "cross", "circle", "l1", ...
    presses: std::collections::BTreeMap<String, u32>,
}

const CONTROLLER_BUTTONS: &[&str] =
    &["cross", "square", "circle", "triangle", "options", "l1", "r1", "l2", "r2", "l3", "r3"];

pub fn controller_check(log: &str) -> ControllerCheck {
    let mut check = ControllerCheck::default();
    for line in log.lines() {
        let Some((_, event)) = line.split_once("VR input: ") else { continue };
        let event = event.trim();
        if let Some((_, profile)) = event.split_once("the right controller is taken as ") {
            check.profile = if profile == "none" { String::new() } else { profile.to_string() };
        } else if let Some((_, button)) = event.rsplit_once("= ") {
            let button = button.to_ascii_lowercase();
            if CONTROLLER_BUTTONS.contains(&button.as_str()) {
                *check.presses.entry(button).or_insert(0) += 1;
            }
        }
    }
    check
}

#[tauri::command]
pub fn get_controller_check(state: State<'_, AppState>) -> Res<ControllerCheck> {
    const MAX: u64 = 8 * 1024 * 1024;
    let Ok(bytes) = std::fs::read(state.layout.log_file()) else { return Ok(ControllerCheck::default()) };
    let skip = (bytes.len() as u64).saturating_sub(MAX) as usize;
    Ok(controller_check(&String::from_utf8_lossy(&bytes[skip..])))
}

#[cfg(test)]
mod controller_check_tests {
    use super::controller_check;

    #[test]
    fn counts_presses_and_reads_the_profile() {
        let log = [
            "[Core.Vr] <Info> a.cpp:1: VR input: the left controller is taken as none",
            "[Core.Vr] <Info> a.cpp:1: VR input: the right controller is taken as /interaction_profiles/oculus/touch_controller",
            "[Core.Vr] <Info> a.cpp:2: VR input: left X = Circle",
            "[Core.Vr] <Info> a.cpp:2: VR input: left menu = OPTIONS",
            "[Core.Vr] <Info> a.cpp:2: VR input: left X = Circle",
            "unrelated line = Circle",
        ]
        .join("\n");
        let check = controller_check(&log);
        assert_eq!(check.profile, "/interaction_profiles/oculus/touch_controller");
        assert_eq!(check.presses["circle"], 2);
        assert_eq!(check.presses["options"], 1);
        assert_eq!(check.presses.len(), 2);
    }
}

// ---------- Shader cache ----------

#[tauri::command]
pub fn get_shader_cache(state: State<'_, AppState>) -> Res<ShaderCacheState> {
    Ok(warmup::state(&state.layout, &state.warmup))
}

#[tauri::command]
pub async fn start_shader_warmup(app: AppHandle) -> Res<ShaderCacheState> {
    let result = blocking(&app, warmup::start).await;
    if let Err(e) = &result {
        log(&app, Msg::new("log.event.warmupNotStarted", format!("Warm-up not started: {}", e.message)).with("reason", e.msg()));
    }
    result
}

#[tauri::command]
pub async fn cancel_shader_warmup(app: AppHandle) -> Res<ShaderCacheState> {
    blocking(&app, |a| {
        let state = a.state::<AppState>();
        if state.warmup.is_active() {
            log(a, Msg::new("log.event.warmupCancelling", "Warm-up: cancelling"));
            state.warmup.cancel();
            state.warmup.wait_until_stopped(Duration::from_secs(30));
        }
        Ok(warmup::state(&state.layout, &state.warmup))
    })
    .await
}

#[tauri::command]
pub async fn clear_shader_cache(app: AppHandle) -> Res<ShaderCacheState> {
    blocking(&app, |a| {
        let state = a.state::<AppState>();
        if state.warmup.is_active() {
            return Err(warmup_running());
        }
        if own_game_running(&state) || game::any_emulator_running() {
            return Err(game_running("backend.game_running_cache", "Close the game first: it uses the shader cache while it runs."));
        }
        let dir = state.layout.shader_cache();
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| BackendError::new("io", "backend.cannot_clear_cache", format!("Cannot clear the shader cache: {e}")))?;
        }
        log(a, Msg::new("log.event.cacheCleared", "Launcher: shader cache cleared"));
        Ok(warmup::state(&state.layout, &state.warmup))
    })
    .await
}

// ---------- Account and friends ----------

async fn blocking<T: Send + 'static>(app: &AppHandle, f: impl FnOnce(&AppHandle) -> Res<T> + Send + 'static) -> Res<T> {
    let worker = app.clone();
    joined(tauri::async_runtime::spawn_blocking(move || f(&worker)).await)
}

#[tauri::command]
pub fn get_account(app: AppHandle, state: State<'_, AppState>) -> Res<AccountState> {
    Ok(state.account.get_account(&env(&app)))
}

#[tauri::command]
pub async fn sign_in(app: AppHandle, online_id: String, recovery_code: String) -> Res<Session> {
    blocking(&app, move |a| a.state::<AppState>().account.sign_in(&env(a), &online_id, &recovery_code)).await
}

#[tauri::command]
pub async fn register_account(app: AppHandle, online_id: String) -> Res<Registration> {
    blocking(&app, move |a| a.state::<AppState>().account.register(&env(a), &online_id)).await
}

#[tauri::command]
pub async fn sign_out(app: AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    if let Some(mut r) = state.game.lock().unwrap().take() {
        let _ = r.child.kill();
        let _ = r.child.wait();
        log(&app, Msg::new("log.event.gameStoppedSignOut", "Launcher: game stopped to sign out"));
        let _ = app.emit("game-state", GameState { running: false, exit_code: None });
    }
    blocking(&app, |a| {
        let state = a.state::<AppState>();
        if state.warmup.is_active() {
            log(a, Msg::new("log.event.warmupCancelledSignOut", "Warm-up: cancelled to sign out"));
            state.warmup.cancel();
            state.warmup.wait_until_stopped(Duration::from_secs(30));
        }
        let account = &state.account;
        let result = account.sign_out(&env(a));
        account.resume();
        result
    })
    .await
}

#[tauri::command]
pub fn update_profile(app: AppHandle, state: State<'_, AppState>, profile: ProfileUpdate) -> Res<Session> {
    state.account.update_profile(&env(&app), &profile)
}

#[tauri::command]
pub async fn upload_avatar(app: AppHandle, image: String) -> Res<()> {
    blocking(&app, move |a| a.state::<AppState>().account.upload_avatar(&env(a), &image)).await
}

#[tauri::command]
pub fn get_friends(app: AppHandle, state: State<'_, AppState>) -> Res<FriendsState> {
    state.account.get_friends(&env(&app))
}

#[tauri::command]
pub async fn send_friend_request(app: AppHandle, online_id: String) -> Res<FriendsState> {
    blocking(&app, move |a| a.state::<AppState>().account.send_friend_request(&env(a), &online_id)).await
}

#[tauri::command]
pub async fn accept_friend_request(app: AppHandle, online_id: String) -> Res<FriendsState> {
    blocking(&app, move |a| a.state::<AppState>().account.accept_friend_request(&env(a), &online_id)).await
}

#[tauri::command]
pub async fn decline_friend_request(app: AppHandle, online_id: String) -> Res<FriendsState> {
    blocking(&app, move |a| a.state::<AppState>().account.decline_friend_request(&env(a), &online_id)).await
}

#[tauri::command]
pub async fn remove_friend(app: AppHandle, online_id: String) -> Res<FriendsState> {
    blocking(&app, move |a| a.state::<AppState>().account.remove_friend(&env(a), &online_id)).await
}

#[tauri::command]
pub async fn get_server_status(app: AppHandle) -> Res<ServerStatus> {
    blocking(&app, |a| Ok(a.state::<AppState>().account.server_status(&env(a)))).await
}

/// Commands whose controls the UI keeps disabled (docs/launcher-contract.md, "Status").
macro_rules! not_yet {
    ($($name:ident => $key:literal, $what:literal),* $(,)?) => {
        $(
            #[tauri::command]
            pub fn $name() -> Res<()> {
                Err(BackendError::unsupported($key, concat!($what, " is not available in this version of the launcher yet.")))
            }
        )*
    };
}

not_yet! {
    set_recovery_email => "backend.unsupported_recovery", "Setting a recovery email",
    start_hosting => "backend.unsupported_hosting", "Hosting from the launcher",
    stop_hosting => "backend.unsupported_hosting", "Hosting from the launcher",
}

#[tauri::command]
pub fn get_hosting() -> Res<Value> {
    Ok(json!({ "running": false, "services": { "network": false, "progression": false, "relay": false } }))
}
