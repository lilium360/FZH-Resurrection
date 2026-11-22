//! Preparing the game: the emulator overlays `<game>-UPDATE` and `<game>-mods` on the game folder by
//! itself (shadPS4 `MntPoints::GetHostPath`), so nothing is copied and the dumps are never written to.
//! `<data>\game\CUSA11182` and `CUSA11182-UPDATE` are NTFS junctions to the player's folders and
//! `CUSA11182-mods` holds the generated mod pak.

use crate::dump::{game_pak, DumpInfo};
use crate::error::{BackendError, Msg, Res};
use crate::layout::{same_path, Layout};
use crate::store::{write_atomic, LauncherFile};
use crate::{instance, modpak};
use std::path::{Path, PathBuf};

fn is_junction(path: &Path) -> bool {
    junction::exists(path).unwrap_or(false)
}

/// Points `link` at `target`, replacing an older junction; refuses to touch anything else.
pub fn link(link: &Path, target: &Path) -> Res<()> {
    if std::fs::symlink_metadata(link).is_ok() {
        if is_junction(link) {
            if same_path(link, target) {
                return Ok(());
            }
            std::fs::remove_dir(link).map_err(|e| BackendError::io_path(link.display(), e))?;
        } else {
            return Err(BackendError::new(
                "io",
                "backend.link_foreign",
                format!("{} exists and was not made by the launcher: move it away and try again.", link.display()),
            )
            .with("path", link.display().to_string()));
        }
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| BackendError::io_path(parent.display(), e))?;
    }
    junction::create(target, link).map_err(|e| {
        BackendError::new(
            "io",
            "backend.ntfs_required",
            format!("Cannot link {} (the game folders must be on a local NTFS drive): {e}", target.display()),
        )
        .with("target", target.display().to_string())
    })
}

pub fn unlink(link: &Path) -> Res<()> {
    if is_junction(link) {
        std::fs::remove_dir(link).map_err(|e| BackendError::io_path(link.display(), e))?;
    }
    Ok(())
}

pub fn separate_update(base: &DumpInfo, update: &DumpInfo) -> bool {
    !same_path(Path::new(&base.path), Path::new(&update.path))
}

/// The game pak whose ini files the mod pak starts from: the update's, which replaces the base one.
pub fn source_pak(base: &DumpInfo, update: &DumpInfo) -> PathBuf {
    let from_update = game_pak(Path::new(&update.path));
    if from_update.is_file() { from_update } else { game_pak(Path::new(&base.path)) }
}

pub fn links_ok(layout: &Layout, base: &DumpInfo, update: &DumpInfo) -> bool {
    let game = layout.game_dir();
    if !is_junction(&game) || !same_path(&game, Path::new(&base.path)) {
        return false;
    }
    let upd = layout.update_link();
    if separate_update(base, update) {
        is_junction(&upd) && same_path(&upd, Path::new(&update.path))
    } else {
        std::fs::symlink_metadata(&upd).is_err()
    }
}

pub fn is_prepared(layout: &Layout, state: &LauncherFile) -> bool {
    let Some((base, update)) = crate::install::effective(layout, state) else { return false };
    state.prepared
        && links_ok(layout, &base, &update)
        && layout.mod_pak().is_file()
        && instance::patches_installed(&layout.patches, &layout.user_dir())
}

pub fn make_links(layout: &Layout, base: &DumpInfo, update: &DumpInfo) -> Res<()> {
    link(&layout.game_dir(), Path::new(&base.path))?;
    if separate_update(base, update) {
        link(&layout.update_link(), Path::new(&update.path))
    } else {
        unlink(&layout.update_link())
    }
}

pub fn build_mod_pak(layout: &Layout, base: &DumpInfo, update: &DumpInfo, inputs: &[String]) -> Res<usize> {
    let source = source_pak(base, update);
    let bytes = modpak::build(&source, inputs)
        .map_err(|e| {
            BackendError::from_msg(
                "io",
                Msg::new("backend.io_path", format!("Cannot build the mod pak from {}: {e}", source.display()))
                    .with("path", source.display().to_string())
                    .with("detail", e.to_string()),
            )
        })?;
    write_atomic(&layout.mod_pak(), &bytes)?;
    remove_legacy_mod_pak(layout)?;
    Ok(bytes.len())
}

fn remove_legacy_mod_pak(layout: &Layout) -> Res<()> {
    let legacy = layout.legacy_mod_pak();
    match std::fs::remove_file(&legacy) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(BackendError::io_path(legacy.display(), e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::Source;

    #[test]
    fn building_the_mod_pak_removes_the_one_of_earlier_versions() {
        let root = std::env::temp_dir().join(format!("fzh-modpak-{}", std::process::id()));
        let layout = Layout { data: root.join("data"), emulator: PathBuf::new(), patches: PathBuf::new(), licences: PathBuf::new(), warmup_script: PathBuf::new() };
        let dump = DumpInfo { path: root.join("dump").display().to_string(), title_id: "CUSA11182".into(), region: "EU".into(), version: "01.43".into(), source: Source::Folder };
        let pak = game_pak(Path::new(&dump.path));
        std::fs::create_dir_all(pak.parent().unwrap()).unwrap();
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("reference.pak"), &pak).unwrap();
        std::fs::create_dir_all(layout.mod_pak().parent().unwrap()).unwrap();
        std::fs::write(layout.legacy_mod_pak(), b"old").unwrap();
        build_mod_pak(&layout, &dump, &dump, &[]).unwrap();
        assert!(layout.mod_pak().is_file());
        assert!(!layout.legacy_mod_pak().exists());
        build_mod_pak(&layout, &dump, &dump, &[]).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
