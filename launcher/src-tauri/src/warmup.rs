//! The shader cache warm-up (also `tools/warmup-cache.ps1`): solo TRAINING matches played by an
//! input script in the game window, one emulator at a time, until two rounds in a row find almost
//! no new pipelines.

use crate::commands::{self, LaunchMode};
use crate::error::{list, seconds, BackendError, Msg, Res};
use crate::game::{self, LogTail, Owned};
use crate::layout::Layout;

use crate::{env, log, store, AppState};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const ROUNDS: u32 = 8;
/// The fewest rounds that can end a warm-up: two in a row with fewer than `FEW_NEW` new pipelines.
pub const MIN_ROUNDS: u32 = 2;
const FEW_NEW: usize = 5;

#[derive(Default)]
pub struct Warmup {
    active: AtomicBool,
    cancel: AtomicBool,
    round: AtomicU32,
    child: Mutex<Option<Owned>>,
}

impl Warmup {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Stops the round in progress at once; the warm-up thread then restores the normal pak.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            c.kill();
        }
    }

    pub fn wait_until_stopped(&self, timeout: Duration) {
        let start = Instant::now();
        while self.is_active() && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ShaderCacheState {
    status: &'static str,
    round: u32,
    rounds: u32,
    min_rounds: u32,
    round_seconds: u64,
}

pub fn cache_has_files(layout: &Layout) -> bool {
    std::fs::read_dir(layout.shader_cache()).map(|mut d| d.next().is_some()).unwrap_or(false)
}

pub fn round_seconds(layout: &Layout) -> Option<u64> {
    std::fs::read(&layout.warmup_script).ok().and_then(|b| game::script_seconds(&String::from_utf8_lossy(&b)))
}

pub fn state(layout: &Layout, w: &Warmup) -> ShaderCacheState {
    let status = if w.is_active() {
        "warming"
    } else if cache_has_files(layout) {
        "ready"
    } else {
        "empty"
    };
    ShaderCacheState {
        status,
        round: if w.is_active() { w.round.load(Ordering::SeqCst) } else { 0 },
        rounds: ROUNDS,
        min_rounds: MIN_ROUNDS,
        round_seconds: round_seconds(layout).unwrap_or(0),
    }
}

fn emit_state(app: &AppHandle) {
    let s = app.state::<AppState>();
    let _ = app.emit("cache-progress", state(&s.layout, &s.warmup));
}

fn duration(secs: u64) -> String {
    format!("{} min {:02} s", secs / 60, secs % 60)
}

/// Checks the preconditions and starts the warm-up thread.
pub fn start(app: &AppHandle) -> Res<ShaderCacheState> {
    let state = app.state::<AppState>();
    let layout = &state.layout;
    let w = &state.warmup;
    if w.is_active() {
        return Err(already_running());
    }
    if commands::own_game_running(&state) || game::any_emulator_running() {
        return Err(BackendError::new("game_running", "backend.warmup_game", "Close the game first: the warm-up starts the game by itself."));
    }
    if state.busy.load(Ordering::SeqCst) {
        return Err(BackendError::new("busy", "backend.warmup_prep", "The launcher is preparing the game: try again in a moment."));
    }
    if state.install.is_running() {
        return Err(BackendError::new("busy", "backend.warmup_install", "The game is being installed: wait until it ends."));
    }
    let file = store::load_launcher(&layout.launcher_json());
    if crate::install::effective(layout, &file).is_none() {
        return Err(BackendError::new("game_not_ready", "backend.warmup_not_ready", "Choose and install your game on the Game screen first."));
    }
    if !layout.emulator.is_file() {
        return Err(BackendError::new("not_found", "backend.warmup_no_emulator", format!("The emulator is missing: {}", layout.emulator.display()))
            .with("path", layout.emulator.display().to_string()));
    }
    let Some(secs) = round_seconds(layout) else {
        return Err(BackendError::new("not_found", "backend.warmup_no_script", "The warm-up input script is missing: reinstall the launcher."));
    };
    let e = env(app);
    if !state.account.is_signed_in(&e) {
        return Err(BackendError::new(
            "not_signed_in",
            "backend.warmup_not_signed_in",
            "Sign in first: the warm-up plays online TRAINING matches with your account.",
        ));
    }
    let host = store::load_settings(&layout.settings_json()).server();
    let status = state.account.server_status(&e);
    if !status.network {
        return Err(crate::shadnet::unreachable_server(&host));
    }
    if !status.progression {
        return Err(BackendError::new(
            "server_unreachable",
            "backend.progress_unreachable",
            "The server is reachable, but its progression service is not answering. Try again later.",
        ));
    }
    if w.active.swap(true, Ordering::SeqCst) {
        return Err(already_running());
    }
    w.cancel.store(false, Ordering::SeqCst);
    w.round.store(1, Ordering::SeqCst);
    state.account.play_grant(&e);
    state.account.suspend(&e);
    log(
        app,
        Msg::new(
            "log.event.warmupStarted",
            format!("Warm-up: started, up to {ROUNDS} rounds of {} in the game window; it stops earlier when the cache is full", duration(secs)),
        )
        .with("rounds", ROUNDS)
        .with("duration", seconds(secs)),
    );
    let worker = app.clone();
    std::thread::spawn(move || run(worker, secs));
    Ok(self::state(layout, w))
}

fn already_running() -> BackendError {
    BackendError::new("busy", "backend.warmup_already", "The warm-up is already running.")
}

enum Outcome {
    Ready(u32),
    AllRounds,
    Cancelled(u32),
    Failed(Msg),
}

fn run(app: AppHandle, secs: u64) {
    let outcome = rounds(&app, secs);
    let state = app.state::<AppState>();
    if let Err(e) = commands::restore_pak(&app) {
        log(
            &app,
            Msg::new("log.event.pakRestoreFailed", format!("Warm-up: the mod pak could not be restored ({}); the next launch rebuilds it", e.message))
                .with("reason", e.msg()),
        );
    }
    state.account.resume();
    log(
        &app,
        match outcome {
            Outcome::Ready(n) => {
                Msg::new("log.event.warmupReady", format!("Warm-up: done after {n} rounds, the shader cache is ready")).with("count", n)
            }
            Outcome::AllRounds => Msg::new("log.event.warmupAllRounds", format!("Warm-up: done, {ROUNDS} rounds played")).with("count", ROUNDS),
            Outcome::Cancelled(n) => {
                Msg::new("log.event.warmupCancelled", format!("Warm-up: cancelled after {n} complete rounds")).with("count", n)
            }
            Outcome::Failed(m) => Msg::new("log.event.warmupStopped", format!("Warm-up stopped: {}", m.message)).with("reason", m),
        },
    );
    state.warmup.round.store(0, Ordering::SeqCst);
    state.warmup.active.store(false, Ordering::SeqCst);
    emit_state(&app);
}

fn rounds(app: &AppHandle, secs: u64) -> Outcome {
    let state = app.state::<AppState>();
    let w = &state.warmup;
    let log_file = state.layout.log_file();
    let (mut few, mut misses) = (0, 0);
    for round in 1..=ROUNDS {
        if w.cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled(round - 1);
        }
        w.round.store(round, Ordering::SeqCst);
        emit_state(app);
        let plan = match commands::prepare_launch(app, LaunchMode::Warmup) {
            Ok(p) => p,
            Err(e) => return Outcome::Failed(e.msg()),
        };
        let child = match plan.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Outcome::Failed(
                    Msg::new("log.event.warmupFailEmulator", format!("the emulator could not start ({e})")).with("detail", e.to_string()),
                )
            }
        };
        let pid = child.id();
        *w.child.lock().unwrap() = Some(Owned::new(child));
        log(
            app,
            Msg::new("log.event.warmupRound", format!("Warm-up: round {round} of {ROUNDS} (process {pid}), {}", duration(secs)))
                .with("round", round)
                .with("rounds", ROUNDS)
                .with("pid", pid)
                .with("duration", seconds(secs)),
        );
        let started = Instant::now();
        let mut tail = LogTail::new(log_file.clone());
        loop {
            std::thread::sleep(Duration::from_millis(500));
            for line in tail.poll() {
                log(app, line);
            }
            let mut guard = w.child.lock().unwrap();
            let Some(c) = guard.as_mut() else { break };
            if !matches!(c.child.try_wait(), Ok(None)) {
                break;
            }
            if w.cancel.load(Ordering::SeqCst) || started.elapsed() >= Duration::from_secs(secs) {
                c.kill();
                break;
            }
        }
        let early = started.elapsed() + Duration::from_secs(2) < Duration::from_secs(secs);
        drop(w.child.lock().unwrap().take());
        if w.cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled(round - 1);
        }
        let text = std::fs::read(&log_file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let r = game::read_round(&text);
        let mut notes = vec![if r.matched {
            Msg::new("log.event.warmupMatchPlayed", "match played")
        } else {
            Msg::new("log.event.warmupNoMatch", "no match reached")
        }];
        if r.crashed {
            notes.push(Msg::new("log.event.warmupCrashed", "the game crashed"));
        }
        if early && !r.crashed {
            notes.push(Msg::new("log.event.warmupClosedEarly", "the game closed early"));
        }
        let english = notes.iter().map(|m| m.message.as_str()).collect::<Vec<_>>().join(", ");
        log(
            app,
            Msg::new("log.event.warmupRoundResult", format!("Warm-up: round {round}: {} new pipelines, {english}", r.new_pipelines))
                .with("round", round)
                .with("count", r.new_pipelines)
                .with("notes", list(notes)),
        );
        few = if r.matched && r.new_pipelines < FEW_NEW { few + 1 } else { 0 };
        if few >= 2 {
            return Outcome::Ready(round);
        }
        misses = if r.matched { 0 } else { misses + 1 };
        if misses >= 2 {
            return Outcome::Failed(if r.signed_in {
                Msg::new("log.event.warmupFailNoMatch", "the game did not reach a match twice in a row")
            } else {
                Msg::new("log.event.warmupFailSignIn", "the game could not sign in to the server twice in a row")
            });
        }
    }
    Outcome::AllRounds
}
