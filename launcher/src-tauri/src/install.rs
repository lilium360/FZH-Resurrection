//! Installing the game from PKG files: extraction into one folder.
//! With both PKG files the folder is merged (the base game's files the update does not
//! replace, the update's files, then the unencrypted PKG entries in `sce_sys/`, the update's winning);
//! with one PKG file it holds that half alone and the other half stays a folder the emulator overlays.
//! Resumable: a file already there with the right size is skipped, and a `.part` file continues from
//! the length recorded in `<file>.part.len` (written after each sync), else it starts again.

use crate::dump::{DumpInfo, Source};
use crate::error::{bytes, BackendError, Msg, Res};
use crate::layout::{same_path, Layout, TITLE_ID};
use crate::pfs::{self, Package};
use crate::store::LauncherFile;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const CHECKPOINT: u64 = 1 << 30;

#[derive(Default)]
pub struct Installer {
    pub running: AtomicBool,
    pub cancel: AtomicBool,
}

impl Installer {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// A folder the launcher extracted PKG files into, complete or not. Only these can be deleted from
/// the launcher, and only the files listed here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallRecord {
    pub target: String,
    pub base_pkg: Option<String>,
    pub update_pkg: Option<String>,
    pub files: Vec<String>,
    pub complete: bool,
}

// ---------- Plan ----------

enum From {
    File { pkg: usize, offset: u64 },
    Entry { pkg: usize, offset: u64 },
}

pub struct Item {
    pub rel: String,
    pub size: u64,
    from: From,
}

pub struct Plan {
    pkgs: Vec<Package>,
    pub items: Vec<Item>,
    pub total: u64,
}

fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(['\\', ':'])
        && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
        && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn dest(target: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(target.to_path_buf(), |p, c| p.join(c))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// What extracting `base` and/or `update` writes.
pub fn plan(base: Option<&Path>, update: Option<&Path>) -> io::Result<Plan> {
    let mut pkgs = Vec::new();
    let open = |p: &Path| Package::open(p).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", file_name(p))));
    let base_idx = base.map(|p| -> io::Result<usize> {
        pkgs.push(open(p)?);
        Ok(pkgs.len() - 1)
    });
    let base_idx = base_idx.transpose()?;
    let update_idx = update
        .map(|p| -> io::Result<usize> {
            pkgs.push(open(p)?);
            Ok(pkgs.len() - 1)
        })
        .transpose()?;
    let mut entries: Vec<(String, usize, u64, u64)> = Vec::new();
    for idx in [base_idx, update_idx].into_iter().flatten() {
        for (name, offset, size) in pfs::sce_sys_entries(&pkgs[idx].entries) {
            let rel = format!("sce_sys/{name}");
            entries.retain(|e| e.0 != rel);
            entries.push((rel, idx, offset, size));
        }
    }
    let from_entries: HashSet<&str> = entries.iter().map(|e| e.0.as_str()).collect();
    let update_files = match update_idx {
        Some(i) => pkgs[i].files()?,
        None => Vec::new(),
    };
    let update_paths: HashSet<&str> = update_files.iter().map(|f| f.path.as_str()).collect();
    let mut inner = Vec::new();
    if let Some(i) = base_idx {
        for f in pkgs[i].files()? {
            if !update_paths.contains(f.path.as_str()) && !from_entries.contains(f.path.as_str()) {
                inner.push(Item { rel: f.path, size: f.size, from: From::File { pkg: i, offset: f.offset } });
            }
        }
    }
    if let Some(i) = update_idx {
        for f in &update_files {
            if !from_entries.contains(f.path.as_str()) {
                inner.push(Item { rel: f.path.clone(), size: f.size, from: From::File { pkg: i, offset: f.offset } });
            }
        }
    }
    inner.sort_by_key(|i| i.size);
    let mut items = inner;
    for (rel, pkg, offset, size) in entries {
        items.push(Item { rel, size, from: From::Entry { pkg, offset } });
    }
    if let Some(bad) = items.iter().find(|i| !safe_rel(&i.rel)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe file name in the PKG: {}", bad.rel)));
    }
    let total = items.iter().map(|i| i.size).sum();
    Ok(Plan { pkgs, items, total })
}

fn recorded_len(part: &Path) -> u64 {
    let Ok(len) = std::fs::metadata(part).map(|m| m.len()) else { return 0 };
    std::fs::read_to_string(with_suffix(part, ".len"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n <= len)
        .unwrap_or(0)
}

fn present_len(target: &Path, item: &Item) -> u64 {
    let dst = dest(target, &item.rel);
    match std::fs::metadata(&dst) {
        Ok(m) if m.is_file() && m.len() == item.size => item.size,
        _ => recorded_len(&with_suffix(&dst, ".part")).min(item.size),
    }
}

impl Plan {
    /// Bytes already extracted in `target`: whole files of the right size and recorded `.part` lengths.
    pub fn present(&self, target: &Path) -> u64 {
        self.items.iter().map(|i| present_len(target, i)).sum()
    }

    pub fn rels(&self) -> Vec<String> {
        self.items.iter().map(|i| i.rel.clone()).collect()
    }
}

/// Files in `target` that are neither in `allowed` nor their `.part`/`.part.len` (relative, with `/`).
pub fn foreign_files(target: &Path, allowed: &[String]) -> Vec<String> {
    let mut ok = HashSet::new();
    for r in allowed {
        ok.insert(r.to_lowercase());
        ok.insert(format!("{r}.part").to_lowercase());
        ok.insert(format!("{r}.part.len").to_lowercase());
    }
    let mut out = Vec::new();
    let mut stack = vec![(target.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = format!("{prefix}{name}");
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push((e.path(), format!("{rel}/"))),
                _ if !ok.contains(&rel.to_lowercase()) => out.push(rel),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

// ---------- Extraction ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub file: String,
}

fn save_len(part: &Path, f: &std::fs::File, len: u64) -> io::Result<()> {
    f.sync_all()?;
    std::fs::write(with_suffix(part, ".len"), len.to_string())
}

/// Extracts `plan` into `target`; false when cancelled (what was written stays for the next time).
pub fn extract(
    plan: &Plan,
    target: &Path,
    workers: usize,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(&Progress),
) -> io::Result<bool> {
    let mut p = Progress { done: 0, total: plan.total, file: String::new() };
    std::fs::create_dir_all(target)?;
    for item in &plan.items {
        if cancel.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let dst = dest(target, &item.rel);
        p.file = item.rel.clone();
        if std::fs::metadata(&dst).map(|m| m.is_file() && m.len() == item.size).unwrap_or(false) {
            p.done += item.size;
            on_progress(&p);
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part = with_suffix(&dst, ".part");
        let len_file = with_suffix(&part, ".len");
        match item.from {
            From::Entry { pkg, offset } => {
                let bytes = plan.pkgs[pkg].entry_bytes(offset, item.size)?;
                std::fs::write(&part, &bytes)?;
                p.done += item.size;
            }
            From::File { pkg, offset } => {
                let start = recorded_len(&part).min(item.size);
                let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part)?;
                f.set_len(start)?;
                f.seek(SeekFrom::Start(start))?;
                let _ = std::fs::remove_file(&len_file);
                p.done += start;
                on_progress(&p);
                let mut written = start;
                let mut checkpoint = start;
                let result = plan.pkgs[pkg].stream(offset + start, offset + item.size, workers, cancel, &mut |chunk| {
                    f.write_all(chunk)?;
                    written += chunk.len() as u64;
                    p.done += chunk.len() as u64;
                    if written - checkpoint >= CHECKPOINT {
                        save_len(&part, &f, written)?;
                        checkpoint = written;
                    }
                    on_progress(&p);
                    Ok(())
                });
                match result {
                    Ok(true) if written == item.size => {}
                    Ok(true) => {
                        save_len(&part, &f, written)?;
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            format!("{} ends early: {written} of {} bytes", item.rel, item.size),
                        ));
                    }
                    Ok(false) => {
                        save_len(&part, &f, written)?;
                        return Ok(false);
                    }
                    Err(e) => {
                        let _ = save_len(&part, &f, written);
                        return Err(e);
                    }
                }
                f.flush()?;
                drop(f);
            }
        }
        std::fs::rename(&part, &dst)?;
        let _ = std::fs::remove_file(&len_file);
        on_progress(&p);
    }
    Ok(true)
}

/// Removes the listed files (and their `.part`, `.part.len`) and the folders left empty; how many
/// entries the launcher did not write are left.
pub fn delete(target: &Path, files: &[String]) -> io::Result<usize> {
    for rel in files.iter().filter(|r| safe_rel(r)) {
        let dst = dest(target, rel);
        for p in [dst.clone(), with_suffix(&dst, ".part"), with_suffix(&dst, ".part.len")] {
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    remove_empty_dirs(target);
    if std::fs::remove_dir(target).is_ok() || !target.exists() {
        return Ok(0);
    }
    Ok(foreign_files(target, &[]).len().max(1))
}

fn remove_empty_dirs(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let p = e.path();
            remove_empty_dirs(&p);
            let _ = std::fs::remove_dir(&p);
        }
    }
}

pub fn size_on_disk(target: &Path, files: &[String]) -> u64 {
    files
        .iter()
        .filter(|r| safe_rel(r))
        .map(|r| {
            let dst = dest(target, r);
            std::fs::metadata(&dst)
                .or_else(|_| std::fs::metadata(with_suffix(&dst, ".part")))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum()
}

/// Free bytes on the volume that holds `path` (or its nearest existing parent).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn drive(p: &Path) -> Msg {
    match p.components().next() {
        Some(Component::Prefix(pre)) => Msg::raw(pre.as_os_str().to_string_lossy().into_owned()),
        _ => Msg::new("install.thisDrive", "this drive"),
    }
}

// ---------- Launcher state ----------

/// The folder made next to the PKG files for the extracted game.
pub const INSTALL_DIR: &str = "FZH Resurrection";

fn pkg_path(info: &Option<DumpInfo>) -> Option<String> {
    info.as_ref().filter(|d| d.source == Source::Pkg).map(|d| d.path.clone())
}

/// `<folder of the base PKG file (else of the update's)>\FZH Resurrection`.
pub fn default_location(layout: &Layout, file: &LauncherFile) -> PathBuf {
    pkg_path(&file.base)
        .or_else(|| pkg_path(&file.update))
        .and_then(|p| Path::new(&p).parent().map(|d| d.join(INSTALL_DIR)))
        .unwrap_or_else(|| layout.data.join("install"))
}

pub fn location(layout: &Layout, file: &LauncherFile) -> PathBuf {
    file.install_location.as_ref().map(PathBuf::from).unwrap_or_else(|| default_location(layout, file))
}

/// After a new choice: the place these PKG files were extracted before if the launcher knows one,
/// else the default next to them.
pub fn choose_location(layout: &Layout, file: &mut LauncherFile) {
    let (base, update) = (pkg_path(&file.base), pkg_path(&file.update));
    if base.is_none() && update.is_none() {
        return;
    }
    let known = file
        .installs
        .iter()
        .rev()
        .find(|r| r.base_pkg == base && r.update_pkg == update && Path::new(&r.target).is_dir())
        .and_then(|r| Path::new(&r.target).parent().map(Path::to_path_buf));
    file.install_location = known.filter(|k| *k != default_location(layout, file)).map(|k| k.display().to_string());
}

/// When the target already holds the whole game extracted from the chosen PKG files (a later visit,
/// or a launcher reinstalled), records it as complete instead of extracting again. Only reads files.
pub fn adopt(layout: &Layout, file: &mut LauncherFile) -> bool {
    if complete(layout, file) {
        return false;
    }
    let Some(t) = target(layout, file) else { return false };
    if !t.is_dir() {
        return false;
    }
    let Ok((plan, summary)) = plan_for(layout, file) else { return false };
    if summary.error.is_some() || summary.present != summary.total || plan.items.iter().any(|i| present_len(&t, i) != i.size) {
        return false;
    }
    if plan.items.iter().any(|i| !dest(&t, &i.rel).is_file()) {
        return false;
    }
    remember(file, &t, &plan);
    mark_complete(file, &t);
    true
}

/// The folder the chosen PKG files go into: `<location>\CUSA11182`, or `CUSA11182-patch` for an update alone.
pub fn target(layout: &Layout, file: &LauncherFile) -> Option<PathBuf> {
    let (base, update) = (pkg_path(&file.base), pkg_path(&file.update));
    if base.is_none() && update.is_none() {
        return None;
    }
    let name = if base.is_some() { TITLE_ID.to_string() } else { format!("{TITLE_ID}-patch") };
    Some(location(layout, file).join(name))
}

fn same(a: &str, b: &Path) -> bool {
    Path::new(a) == b || same_path(Path::new(a), b)
}

pub fn record_for<'a>(file: &'a LauncherFile, target: &Path) -> Option<&'a InstallRecord> {
    file.installs.iter().find(|r| same(&r.target, target))
}

/// The record of the chosen PKG files in the current target, if extracted completely.
pub fn complete(layout: &Layout, file: &LauncherFile) -> bool {
    let Some(t) = target(layout, file) else { return false };
    record_for(file, &t).is_some_and(|r| {
        r.complete && r.base_pkg == pkg_path(&file.base) && r.update_pkg == pkg_path(&file.update) && t.is_dir()
    })
}

/// The two folders the emulator uses: a PKG half becomes the folder it was extracted into. None
/// until both halves are chosen and any PKG file is extracted.
pub fn effective(layout: &Layout, file: &LauncherFile) -> Option<(DumpInfo, DumpInfo)> {
    let (base, update) = (file.base.clone()?, file.update.clone()?);
    if base.source == Source::Folder && update.source == Source::Folder {
        return Some((base, update));
    }
    if !complete(layout, file) {
        return None;
    }
    let t = target(layout, file)?.display().to_string();
    let folder = |d: DumpInfo| if d.source == Source::Pkg { DumpInfo { path: t.clone(), source: Source::Folder, ..d } } else { d };
    Some((folder(base), folder(update)))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub location: String,
    pub target: String,
    pub complete: bool,
    pub running: bool,
    /// Bytes of this game already in the target.
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub target: String,
    pub size: u64,
}

pub fn info(layout: &Layout, file: &LauncherFile, running: bool) -> Option<InstallInfo> {
    let t = target(layout, file)?;
    let size = record_for(file, &t).map(|r| size_on_disk(&t, &r.files)).unwrap_or(0);
    Some(InstallInfo {
        location: location(layout, file).display().to_string(),
        target: t.display().to_string(),
        complete: complete(layout, file),
        running,
        size,
    })
}

/// Extracted folders on disk that the current choice does not use.
pub fn leftovers(layout: &Layout, file: &LauncherFile) -> Vec<Leftover> {
    let current = target(layout, file);
    file.installs
        .iter()
        .filter(|r| current.as_ref().is_none_or(|t| !same(&r.target, t)))
        .filter(|r| Path::new(&r.target).exists())
        .map(|r| Leftover { target: r.target.clone(), size: size_on_disk(Path::new(&r.target), &r.files) })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub location: String,
    pub target: String,
    pub total: u64,
    pub present: u64,
    pub required: u64,
    pub free: Option<u64>,
    /// Why the installation cannot start here, else null.
    pub error: Option<Msg>,
}

/// Opens the chosen PKG files and works out what the installation needs at the current location.
pub fn plan_for(layout: &Layout, file: &LauncherFile) -> Res<(Plan, InstallPlan)> {
    let Some(t) = target(layout, file) else {
        return Err(BackendError::new("game_not_ready", "backend.install_no_pkg", "No PKG file is chosen: nothing to install."));
    };
    let (base, update) = (pkg_path(&file.base), pkg_path(&file.update));
    let plan = plan(base.as_deref().map(Path::new), update.as_deref().map(Path::new))
        .map_err(|e| BackendError::new("io", "dump.unreadablePkg", format!("Cannot read the PKG file {e}")).with("error", e.to_string()))?;
    let mut allowed = plan.rels();
    if let Some(r) = record_for(file, &t) {
        allowed.extend(r.files.iter().cloned());
    }
    let present = if t.is_dir() { plan.present(&t) } else { 0 };
    let required = plan.total - present;
    let free = free_space(&t);
    let shown = t.display().to_string();
    let error = if t.exists() && !t.is_dir() {
        Some(Msg::new("backend.install_target_file", format!("{shown} is a file: choose another install location.")).with("path", shown.clone()))
    } else if t.starts_with(layout.game_root()) {
        Some(Msg::new("backend.install_outside", "Choose an install location outside the launcher's own game folder."))
    } else if t.is_dir() && !foreign_files(&t, &allowed).is_empty() {
        Some(
            Msg::new(
                "backend.install_target_foreign",
                format!("{shown} already holds files that are not part of this game: choose another install location."),
            )
            .with("path", shown.clone()),
        )
    } else {
        match free {
            Some(f) if f < required => {
                let d = drive(&t);
                Some(
                    Msg::new(
                        "backend.install_no_space",
                        format!(
                            "Not enough free space on {}: {} needed, {} free. Free some space or choose another install location.",
                            d.message,
                            gb(required),
                            gb(f)
                        ),
                    )
                    .with("drive", d)
                    .with("needed", bytes(required))
                    .with("free", bytes(f)),
                )
            }
            _ => None,
        }
    };
    let summary = InstallPlan {
        location: location(layout, file).display().to_string(),
        target: t.display().to_string(),
        total: plan.total,
        present,
        required,
        free,
        error,
    };
    Ok((plan, summary))
}

/// Adds or updates the record of the current target before extracting into it.
pub fn remember(file: &mut LauncherFile, target: &Path, plan: &Plan) {
    let base = pkg_path(&file.base);
    let update = pkg_path(&file.update);
    let t = target.display().to_string();
    let mut files = plan.rels();
    if let Some(r) = file.installs.iter().find(|r| same(&r.target, target)) {
        for f in &r.files {
            if !files.contains(f) {
                files.push(f.clone());
            }
        }
    }
    file.installs.retain(|r| !same(&r.target, target));
    file.installs.push(InstallRecord { target: t, base_pkg: base, update_pkg: update, files, complete: false });
}

pub fn mark_complete(file: &mut LauncherFile, target: &Path) {
    if let Some(r) = file.installs.iter_mut().find(|r| same(&r.target, target)) {
        r.complete = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pfs::tests::synthetic_pkg;

    const CID: &[u8; 36] = b"EP9000-CUSA11182_00-FIREWALL00000000";

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(tag: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("fzh-install-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }

    fn pkgs(dir: &Path, big: &[u8]) -> (PathBuf, PathBuf) {
        let base = synthetic_pkg(CID, &[(0x1000, b"BASE-SFO"), (0x1200, b"base-icon"), (0x1220, b"pic0")], &[("eboot.bin", b"base eboot"), ("big.pak", big)]);
        let update = synthetic_pkg(CID, &[(0x1000, b"UPDATE-SFO"), (0x1200, b"update-icon")], &[("eboot.bin", b"update eboot!"), ("patch.bin", b"p")]);
        let (b, u) = (dir.join("b.pkg"), dir.join("u.pkg"));
        std::fs::write(&b, base).unwrap();
        std::fs::write(&u, update).unwrap();
        (b, u)
    }

    fn read(target: &Path, rel: &str) -> Vec<u8> {
        std::fs::read(dest(target, rel)).unwrap()
    }

    #[test]
    fn merged_extraction() {
        let t = tmp("merge");
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 241) as u8).collect();
        let (b, u) = pkgs(&t.0, &big);
        let plan = plan(Some(&b), Some(&u)).unwrap();
        let mut rels = plan.rels();
        rels.sort();
        assert_eq!(rels, vec!["d/big.pak", "d/patch.bin", "eboot.bin", "sce_sys/icon0.png", "sce_sys/param.sfo", "sce_sys/pic0.png"]);
        let target = t.0.join("out").join("CUSA11182");
        let cancel = AtomicBool::new(false);
        let mut last = None;
        assert!(extract(&plan, &target, 2, &cancel, &mut |p| last = Some(p.clone())).unwrap());
        let last = last.unwrap();
        assert_eq!((last.done, last.total), (plan.total, plan.total));
        assert_eq!(read(&target, "eboot.bin"), b"update eboot!");
        assert_eq!(read(&target, "d/big.pak"), big);
        assert_eq!(read(&target, "sce_sys/param.sfo"), b"UPDATE-SFO");
        assert_eq!(read(&target, "sce_sys/icon0.png"), b"update-icon");
        assert_eq!(read(&target, "sce_sys/pic0.png"), b"pic0");
        assert!(foreign_files(&target, &plan.rels()).is_empty());
        assert_eq!(plan.present(&target), plan.total);
        assert_eq!(size_on_disk(&target, &plan.rels()), plan.total);

        std::fs::write(target.join("mine.txt"), b"keep").unwrap();
        assert_eq!(foreign_files(&target, &plan.rels()), vec!["mine.txt"]);
        assert_eq!(delete(&target, &plan.rels()).unwrap(), 1);
        assert_eq!(foreign_files(&target, &[]), vec!["mine.txt"]);
        std::fs::remove_file(target.join("mine.txt")).unwrap();
        assert_eq!(delete(&target, &plan.rels()).unwrap(), 0);
        assert!(!target.exists());
    }

    #[test]
    fn cancel_and_resume() {
        let t = tmp("resume");
        let big: Vec<u8> = (0..400_000u32).map(|i| (i.wrapping_mul(97) >> 3) as u8).collect();
        let (b, _) = pkgs(&t.0, &big);
        let plan = plan(Some(&b), None).unwrap();
        let target = t.0.join("CUSA11182");
        let cancel = AtomicBool::new(false);
        let mut seen = 0u64;
        let stopped = extract(&plan, &target, 2, &cancel, &mut |p| {
            seen = p.done;
            if p.file == "d/big.pak" && p.done > 150_000 {
                cancel.store(true, Ordering::SeqCst);
            }
        })
        .unwrap();
        assert!(!stopped);
        let part = with_suffix(&dest(&target, "d/big.pak"), ".part");
        let kept = recorded_len(&part);
        assert!(kept > 0 && kept < big.len() as u64, "{kept}");
        assert_eq!(plan.present(&target), seen);
        cancel.store(false, Ordering::SeqCst);
        let mut first = None;
        assert!(extract(&plan, &target, 3, &cancel, &mut |p| {
            if p.file == "d/big.pak" {
                first.get_or_insert(p.done);
            }
        })
        .unwrap());
        assert!(first.unwrap() >= kept);
        assert_eq!(read(&target, "d/big.pak"), big);
        assert!(!part.exists() && !with_suffix(&part, ".len").exists());

        std::fs::write(&part, b"garbage without a recorded length").unwrap();
        assert_eq!(recorded_len(&part), 0);
    }

    fn sfo(category: &str, app_ver: &str) -> Vec<u8> {
        use crate::sfo::{build, SfoValue::Text};
        build(&[
            ("APP_VER", Text(app_ver.into())),
            ("CATEGORY", Text(category.into())),
            ("CONTENT_ID", Text(String::from_utf8(CID.to_vec()).unwrap())),
            ("TITLE_ID", Text("CUSA11182".into())),
            ("VERSION", Text("01.00".into())),
        ])
    }

    fn layout(data: &Path) -> Layout {
        Layout {
            data: data.to_path_buf(),
            emulator: data.join("e.exe"),
            patches: data.join("p"),
            licences: data.join("l"),
            warmup_script: data.join("w.txt"),
        }
    }

    #[test]
    fn default_location_next_to_the_pkg_files_and_reuse_on_a_later_visit() {
        use crate::dump::{detect, Detection};
        let t = tmp("default");
        let dump = t.0.join("My PKG files");
        std::fs::create_dir_all(&dump).unwrap();
        let big: Vec<u8> = (0..50_000u32).map(|i| (i % 199) as u8).collect();
        let base = synthetic_pkg(CID, &[(0x1000, &sfo("gd", "01.00"))], &[("eboot.bin", b"base"), ("big.pak", &big)]);
        let update = synthetic_pkg(CID, &[(0x1000, &sfo("gp", "01.43"))], &[("eboot.bin", b"update"), ("x", b"y")]);
        std::fs::write(dump.join("game.pkg"), base).unwrap();
        std::fs::write(dump.join("patch.pkg"), update).unwrap();
        let lay = layout(&t.0.join("data"));

        let Detection::Ready(b, u) = detect(&dump, None, None) else { panic!() };
        let mut file = LauncherFile { base: Some(b), update: Some(u), ..Default::default() };
        choose_location(&lay, &mut file);
        let expected = dump.join(INSTALL_DIR).join("CUSA11182");
        assert_eq!(target(&lay, &file), Some(expected.clone()));
        assert!(!adopt(&lay, &mut file), "nothing extracted yet");
        assert!(effective(&lay, &file).is_none());

        let (plan, summary) = plan_for(&lay, &file).unwrap();
        assert_eq!((summary.error, summary.present), (None, 0));
        assert!(extract(&plan, &expected, 1, &AtomicBool::new(false), &mut |_| {}).unwrap());
        drop(plan);

        let Detection::Ready(b, u) = detect(&dump, None, None) else { panic!("the extracted subfolder must not confuse detection") };
        assert_eq!((b.source, u.source), (Source::Pkg, Source::Pkg));
        let mut later = LauncherFile { base: Some(b), update: Some(u), ..Default::default() };
        choose_location(&lay, &mut later);
        assert!(adopt(&lay, &mut later), "the extracted game is reused");
        let (eb, eu) = effective(&lay, &later).unwrap();
        assert_eq!((Path::new(&eb.path), eb.source, eu.version.as_str()), (expected.as_path(), Source::Folder, "01.43"));
        assert!(leftovers(&lay, &later).is_empty());

        let elsewhere = t.0.join("Other drive");
        later.install_location = Some(elsewhere.display().to_string());
        assert_eq!(target(&lay, &later), Some(elsewhere.join("CUSA11182")));
        assert_eq!(leftovers(&lay, &later).len(), 1);
        choose_location(&lay, &mut later);
        assert_eq!(later.install_location, None, "back to the known copy next to the PKG files");
    }

    #[test]
    fn safe_names_only() {
        assert!(safe_rel("firewall/content/paks/firewall-ps4.pak"));
        for bad in ["", "../x", "a/../b", "/abs", "c:x", "a\\b", "a//b", "./a"] {
            assert!(!safe_rel(bad), "{bad}");
        }
    }
}
