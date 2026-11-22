//! Where everything lives. Writable data under the app data folder, read-only parts in the app resources.
//!
//! Development overrides (environment variables win over `<data>\dev.json`):
//! - `FZH_DATA_DIR`: the data folder instead of `%LOCALAPPDATA%\com.fzh-resurrection.launcher`.
//! - `FZH_EMULATOR` / `"emulator"`: a shadps4.exe outside the bundle.
//! - `FZH_PATCHES` / `"patches"`: a folder of emulator patch .xml files instead of the bundled ones.
//! - `FZH_WARMUP_SCRIPT`: the warm-up input script instead of the bundled `input\warmup.txt`.
//!
//! Bundled resources (filled by `scripts/bundle-emulator.mjs`): `emulator\` (shadps4.exe, the Visual
//! C++ runtime DLLs it imports, `commit.txt`), `licences\`, `patches\`, `input\warmup.txt`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const TITLE_ID: &str = "CUSA11182";
pub const MOD_PAK_NAME: &str = "zz-fzh-mod_P.pak";
/// The mod pak's name in earlier versions; the game loads every pak in the folder.
pub const LEGACY_MOD_PAK_NAME: &str = "zz-fzh-casa_P.pak";

#[derive(Debug, Clone)]
pub struct Layout {
    pub data: PathBuf,
    pub emulator: PathBuf,
    pub patches: PathBuf,
    pub licences: PathBuf,
    /// The input script the shader cache warm-up plays (`tools/input/warmup.txt`).
    pub warmup_script: PathBuf,
}

#[derive(Default, Deserialize)]
struct DevConfig {
    emulator: Option<PathBuf>,
    patches: Option<PathBuf>,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The data folder as known before the application exists, for `single.rs`: `FZH_DATA_DIR`, else the
/// identifier's folder in `%LOCALAPPDATA%`, where Tauri puts the application's local data.
pub fn data_dir(identifier: &str) -> PathBuf {
    env_path("FZH_DATA_DIR").unwrap_or_else(|| env_path("LOCALAPPDATA").unwrap_or_default().join(identifier))
}

impl Layout {
    pub fn resolve(default_data: PathBuf, resources: Option<PathBuf>) -> Layout {
        let data = env_path("FZH_DATA_DIR").unwrap_or(default_data);
        let dev: DevConfig = std::fs::read(data.join("dev.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let resources = resources.unwrap_or_else(|| PathBuf::from("."));
        let emulator = env_path("FZH_EMULATOR")
            .or(dev.emulator)
            .unwrap_or_else(|| resources.join("emulator").join("shadps4.exe"));
        let patches = env_path("FZH_PATCHES").or(dev.patches).unwrap_or_else(|| resources.join("patches"));
        let licences = resources.join("licences");
        let warmup_script = env_path("FZH_WARMUP_SCRIPT").unwrap_or_else(|| resources.join("input").join("warmup.txt"));
        Layout { data, emulator, patches, licences, warmup_script }
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data.join("logs")
    }

    pub fn launcher_json(&self) -> PathBuf {
        self.data.join("launcher.json")
    }
    pub fn settings_json(&self) -> PathBuf {
        self.data.join("settings.json")
    }
    /// Launcher-only account preferences: last Online ID, avatar choice. Never the password.
    pub fn account_json(&self) -> PathBuf {
        self.data.join("account.json")
    }
    /// Which account each server was left with (`accounts.rs`). Never the password.
    pub fn accounts_json(&self) -> PathBuf {
        self.data.join("accounts.json")
    }
    /// The player's own avatar picture (256x256 PNG), when they chose one.
    pub fn avatar_png(&self) -> PathBuf {
        self.data.join("avatar.png")
    }
    pub fn users_json(&self) -> PathBuf {
        self.user_dir().join("users.json")
    }
    pub fn run_dir(&self) -> PathBuf {
        self.data.join("run")
    }
    pub fn user_dir(&self) -> PathBuf {
        self.run_dir().join("user")
    }
    pub fn log_file(&self) -> PathBuf {
        self.user_dir().join("log").join("shad_log.txt")
    }
    pub fn shader_cache(&self) -> PathBuf {
        self.user_dir().join("cache").join(TITLE_ID)
    }
    pub fn game_root(&self) -> PathBuf {
        self.data.join("game")
    }
    /// Junction to the base dump; the emulator mounts it as /app0.
    pub fn game_dir(&self) -> PathBuf {
        self.game_root().join(TITLE_ID)
    }
    /// Junction to the update dump; the emulator overlays `<game>-UPDATE` on the game folder by itself.
    pub fn update_link(&self) -> PathBuf {
        self.game_root().join(format!("{TITLE_ID}-UPDATE"))
    }
    /// Real folder; the emulator overlays `<game>-mods` above both.
    pub fn mods_dir(&self) -> PathBuf {
        self.game_root().join(format!("{TITLE_ID}-mods"))
    }
    pub fn mod_pak(&self) -> PathBuf {
        self.mods_dir().join("firewall").join("content").join("paks").join(MOD_PAK_NAME)
    }
    pub fn legacy_mod_pak(&self) -> PathBuf {
        self.mod_pak().with_file_name(LEGACY_MOD_PAK_NAME)
    }
    pub fn eboot(&self) -> PathBuf {
        self.game_dir().join("eboot.bin")
    }
    pub fn emulator_commit(&self) -> String {
        let file = self.emulator.parent().map(|p| p.join("commit.txt"));
        file.and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}
