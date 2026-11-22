use crate::dump::DumpInfo;
use crate::error::{BackendError, Res};
use crate::install::InstallRecord;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// `<data>\launcher.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LauncherFile {
    pub setup_completed: bool,
    pub base: Option<DumpInfo>,
    pub update: Option<DumpInfo>,
    #[serde(alias = "patched")]
    pub prepared: bool,
    /// What the mod pak on disk was built from.
    pub mod_pak_inputs: Option<Vec<String>>,
    /// Where PKG files are extracted (the parent of the game folder); null = `<data>\install`.
    pub install_location: Option<String>,
    pub installs: Vec<InstallRecord>,
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Res<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| BackendError::io_path(parent.display(), e))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| BackendError::io_path(tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| BackendError::io_path(path.display(), e))
}

pub fn load_launcher(path: &Path) -> LauncherFile {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_launcher(path: &Path, state: &LauncherFile) -> Res<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| BackendError::internal(e.to_string()))?;
    write_atomic(path, &bytes)
}

pub fn load_settings(path: &Path) -> Settings {
    match std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        Some(v) => Settings::from_stored(&v),
        None => Settings::default(),
    }
}

pub fn save_settings(path: &Path, settings: &Settings) -> Res<()> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| BackendError::internal(e.to_string()))?;
    write_atomic(path, &bytes)
}
