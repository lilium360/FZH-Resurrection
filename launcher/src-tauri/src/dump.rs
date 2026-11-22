//! Validation of the two folders a player provides: the base game dump and the 1.43 update dump.

use crate::error::Msg;
use crate::{pfs, sfo};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const TITLE_ID: &str = "CUSA11182";
pub const USA_TITLE_ID: &str = "CUSA09831";
pub const UPDATE_VERSION: &str = "01.43";
pub const GAME_PAK: &str = "firewall/content/paks/firewall-ps4.pak";

/// `GAME_PAK` inside `folder`, with the platform's separators.
pub fn game_pak(folder: &Path) -> std::path::PathBuf {
    GAME_PAK.split('/').fold(folder.to_path_buf(), |p, c| p.join(c))
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Base,
    Update,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpInfo {
    pub path: String,
    pub title_id: String,
    pub region: String,
    pub version: String,
    /// A folder used in place, or a PKG file the launcher extracts first.
    #[serde(default)]
    pub source: Source,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Folder,
    Pkg,
}

#[derive(Debug, PartialEq)]
pub struct Rejection {
    pub found: Msg,
    pub error: Msg,
}

fn region(content_id: &str) -> &'static str {
    match content_id.as_bytes().first() {
        Some(b'E') => "Europe",
        Some(b'U') => "USA",
        Some(b'J') => "Japan",
        Some(b'H') => "Asia",
        _ => "unknown region",
    }
}

fn short_version(v: &str) -> String {
    let t = v.trim_start_matches('0');
    if t.is_empty() || t.starts_with('.') { format!("0{t}") } else { t.to_string() }
}

fn reject(found: Msg, error: Msg) -> Result<DumpInfo, Rejection> {
    Err(Rejection { found, error })
}

/// The i18n key of a region name from `region()`.
fn region_key(region: &str) -> &'static str {
    match region {
        "Europe" => "dump.region.europe",
        "USA" => "dump.region.usa",
        "Japan" => "dump.region.japan",
        "Asia" => "dump.region.asia",
        _ => "dump.region.unknown",
    }
}

/// "<name> · <region> · v<version>", what a rejected folder or file was recognised as.
fn found_release(name: &str, region: &'static str, app_ver: &str) -> Msg {
    Msg::new("dump.foundRelease", format!("{name} · {region} · v{app_ver}"))
        .with("name", name)
        .with("region", Msg::new(region_key(region), region))
        .with("version", app_ver)
}

/// "<name> · base game" or "<name> · update".
fn found_kind(name: &str, kind: Kind) -> Msg {
    let (key, english) = match kind {
        Kind::Base => ("file.base", "base game"),
        Kind::Update => ("file.update", "update"),
    };
    Msg::new("dump.foundKind", format!("{name} · {english}")).with("name", name).with("kind", Msg::new(key, english))
}

fn noun(pkg: bool) -> Msg {
    if pkg { Msg::new("dump.noun.pkg", "This PKG file") } else { Msg::new("dump.noun.folder", "This folder") }
}

fn holds_base(pkg: bool) -> Msg {
    let n = noun(pkg);
    Msg::new("dump.holdsBase", format!("{} holds the base game, not the update.", n.message)).with("noun", n)
}

fn holds_update(pkg: bool) -> Msg {
    let n = noun(pkg);
    Msg::new("dump.holdsUpdate", format!("{} holds the update, not the base game.", n.message)).with("noun", n)
}

fn no_folder() -> Msg {
    Msg::new("dump.doesNotExist", "This folder does not exist.")
}

/// The release, category and version rules shared by dump folders and PKG files.
fn rules(kind: Kind, pkg: bool, title: &str, category: &str, app_ver: &str, merged: bool) -> Result<(), Msg> {
    let n = noun(pkg);
    match kind {
        Kind::Base => {
            if title == USA_TITLE_ID {
                return Err(Msg::new(
                    "dump.usaRelease",
                    "CUSA09831 is the USA release: not supported yet. Only the European release CUSA11182 has been tested.",
                ));
            }
            if title != TITLE_ID {
                return Err(Msg::new("dump.wrongTitle", format!("{} holds {title}, not Firewall Zero Hour (CUSA11182).", n.message))
                    .with("noun", n)
                    .with("title", title));
            }
            if category == "gp" && !merged {
                return Err(holds_update(pkg));
            }
            if category != "gd" && !merged {
                return Err(Msg::new("dump.notBaseGame", format!("{} does not hold the base game.", n.message)).with("noun", n));
            }
        }
        Kind::Update => {
            if title == USA_TITLE_ID {
                return Err(Msg::new(
                    "dump.usaUpdate",
                    "This update belongs to the USA release CUSA09831: it does not match your base game.",
                ));
            }
            if title != TITLE_ID {
                return Err(Msg::new(
                    "dump.wrongUpdateTitle",
                    format!("{} holds {title}, not the Firewall Zero Hour update (CUSA11182).", n.message),
                )
                .with("noun", n)
                .with("title", title));
            }
            if category != "gp" {
                return Err(holds_base(pkg));
            }
            if app_ver != UPDATE_VERSION {
                let v = short_version(app_ver);
                return Err(Msg::new("dump.wrongUpdateVersion", format!("Update {v} found: version 1.43 is needed.")).with("version", v));
            }
        }
    }
    Ok(())
}

fn version_of(kind: Kind, sfo: &std::collections::HashMap<String, sfo::SfoValue>) -> String {
    let app_ver = sfo::text(sfo, "APP_VER").to_string();
    let master_ver = sfo::text(sfo, "VERSION").to_string();
    match kind {
        Kind::Base if !master_ver.is_empty() => master_ver,
        _ => app_ver,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Accepts in the base slot also a folder where base and update are already merged (as the
/// launcher's PKG installation makes it): the update's param.sfo next to the base game's disc map.
pub fn check(kind: Kind, folder: &Path) -> Result<DumpInfo, Rejection> {
    let name = display_name(folder);
    if !folder.is_dir() {
        return reject(Msg::default(), no_folder());
    }
    let Ok(raw) = std::fs::read(folder.join("sce_sys").join("param.sfo")) else {
        return reject(
            Msg::raw(name),
            Msg::new("dump.noParamSfo", "This folder does not hold a PS4 game dump: sce_sys/param.sfo is missing."),
        );
    };
    let Ok(sfo) = sfo::parse(&raw) else {
        return reject(Msg::raw(name), Msg::new("dump.unreadableParamSfo", "The param.sfo file of this folder cannot be read."));
    };
    let title = sfo::text(&sfo, "TITLE_ID").to_string();
    let category = sfo::text(&sfo, "CATEGORY");
    let app_ver = sfo::text(&sfo, "APP_VER").to_string();
    let reg = region(sfo::text(&sfo, "CONTENT_ID"));
    let found = found_release(&name, reg, &app_ver);
    let merged = category == "gp" && folder.join("sce_discmap.plt").is_file();
    if let Err(e) = rules(kind, false, &title, category, &app_ver, merged) {
        return reject(found, e);
    }
    match kind {
        Kind::Base if !folder.join("eboot.bin").is_file() || !game_pak(folder).is_file() => {
            return reject(found, Msg::new("dump.incompleteBase", "This dump is incomplete: eboot.bin or firewall-ps4.pak is missing."))
        }
        Kind::Update if !folder.join("eboot.bin").is_file() => {
            return reject(found, Msg::new("dump.incompleteUpdate", "This update dump is incomplete: eboot.bin is missing."))
        }
        _ => {}
    }
    let version = version_of(kind, &sfo);
    Ok(DumpInfo { path: folder.display().to_string(), title_id: title, region: reg.into(), version, source: Source::Folder })
}

fn pkg_sfo(path: &Path) -> Result<std::collections::HashMap<String, sfo::SfoValue>, Rejection> {
    let name = display_name(path);
    let raw = pfs::read_param_sfo(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => Rejection {
            found: Msg::raw(name.clone()),
            error: Msg::new("dump.notPkg", "This file is not a PS4 PKG file, or it is damaged."),
        },
        _ => Rejection {
            found: Msg::raw(name.clone()),
            error: Msg::new("dump.unreadablePkg", format!("This PKG file cannot be read: {e}.")).with("error", e.to_string()),
        },
    })?;
    sfo::parse(&raw).map_err(|_| Rejection {
        found: Msg::raw(name),
        error: Msg::new("dump.pkgParamSfo", "The param.sfo of this PKG file cannot be read."),
    })
}

/// A PKG file, identified by the param.sfo in its own header whatever its name.
pub fn check_pkg(kind: Kind, path: &Path) -> Result<DumpInfo, Rejection> {
    if !path.is_file() {
        return reject(Msg::default(), Msg::new("dump.pkgNotExist", "This file does not exist."));
    }
    let sfo = pkg_sfo(path)?;
    let title = sfo::text(&sfo, "TITLE_ID").to_string();
    let category = sfo::text(&sfo, "CATEGORY");
    let app_ver = sfo::text(&sfo, "APP_VER").to_string();
    let reg = region(sfo::text(&sfo, "CONTENT_ID"));
    let found = found_release(&display_name(path), reg, &app_ver);
    if let Err(e) = rules(kind, true, &title, category, &app_ver, false) {
        return reject(found, e);
    }
    let version = version_of(kind, &sfo);
    Ok(DumpInfo { path: path.display().to_string(), title_id: title, region: reg.into(), version, source: Source::Pkg })
}

/// What one dump folder holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    Merged(DumpInfo, DumpInfo),
    Base(DumpInfo),
    Update(DumpInfo),
}

fn has_sfo(folder: &Path) -> bool {
    folder.join("sce_sys").join("param.sfo").is_file()
}

fn category(folder: &Path) -> String {
    std::fs::read(folder.join("sce_sys").join("param.sfo"))
        .ok()
        .and_then(|raw| sfo::parse(&raw).ok())
        .map(|s| sfo::text(&s, "CATEGORY").to_string())
        .unwrap_or_default()
}

pub fn classify(folder: &Path) -> Result<Found, Rejection> {
    let cat = category(folder);
    if cat == "gp" && folder.join("sce_discmap.plt").is_file() {
        let base = check(Kind::Base, folder)?;
        let update = check(Kind::Update, folder)?;
        Ok(Found::Merged(base, update))
    } else if cat == "gp" {
        check(Kind::Update, folder).map(Found::Update)
    } else {
        check(Kind::Base, folder).map(Found::Base)
    }
}

/// The folder itself and its direct subfolders that hold `sce_sys/param.sfo`, sorted by name.
pub fn candidates(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut list = Vec::new();
    if has_sfo(folder) {
        list.push(folder.to_path_buf());
    }
    let mut subs: Vec<_> = std::fs::read_dir(folder)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir() && has_sfo(p)).collect())
        .unwrap_or_default();
    subs.sort();
    list.extend(subs);
    list
}

fn is_pkg(p: &Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pkg"))
}

fn pkgs_in(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut list: Vec<_> = std::fs::read_dir(folder)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_pkg(p)).collect())
        .unwrap_or_default();
    list.sort();
    list
}

/// The `.pkg` files in the folder, then those in its direct subfolders, sorted by name.
pub fn pkg_candidates(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut list = pkgs_in(folder);
    let mut subs: Vec<_> = std::fs::read_dir(folder)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    subs.sort();
    for s in subs {
        list.extend(pkgs_in(&s));
    }
    list
}

#[derive(Debug, PartialEq)]
pub enum Detection {
    Ready(DumpInfo, DumpInfo),
    NeedUpdate { base: DumpInfo, note: Option<Msg> },
    NeedBase { update: DumpInfo, note: Option<Msg> },
    Rejected(Rejection),
}

/// Works out what a chosen folder holds. `expect` is the half the player was asked for (the second
/// picker); `stored` is the half chosen before, completed when the right counterpart is found.
pub fn detect(folder: &Path, expect: Option<Kind>, stored: Option<&DumpInfo>) -> Detection {
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.display().to_string());
    if !folder.is_dir() {
        return Detection::Rejected(Rejection { found: Msg::default(), error: no_folder() });
    }
    let mut bases = Vec::new();
    let mut updates = Vec::new();
    let mut base_errors = Vec::new();
    let mut update_errors = Vec::new();
    for c in candidates(folder) {
        let is_update = category(&c) == "gp" && !c.join("sce_discmap.plt").is_file();
        match classify(&c) {
            Ok(Found::Merged(b, u)) => return Detection::Ready(b, u),
            Ok(Found::Base(b)) => bases.push(b),
            Ok(Found::Update(u)) => updates.push(u),
            Err(r) if is_update => update_errors.push(r),
            Err(r) => base_errors.push(r),
        }
    }
    for p in pkg_candidates(folder) {
        let is_update = pkg_sfo(&p).map(|s| sfo::text(&s, "CATEGORY") == "gp").unwrap_or(false);
        let kind = if is_update { Kind::Update } else { Kind::Base };
        match (check_pkg(kind, &p), kind) {
            (Ok(u), Kind::Update) => updates.push(u),
            (Ok(b), Kind::Base) => bases.push(b),
            (Err(r), Kind::Update) => update_errors.push(r),
            (Err(r), Kind::Base) => base_errors.push(r),
        }
    }
    let first_error = |a: &mut Vec<Rejection>, b: &mut Vec<Rejection>| {
        if !a.is_empty() { a.remove(0) } else if !b.is_empty() { b.remove(0) } else {
            Rejection {
                found: Msg::raw(name.clone()),
                error: Msg::new(
                    "dump.noPkgNoDump",
                    "No PS4 game here: this folder and its subfolders hold no PKG files and no game dump (sce_sys/param.sfo).",
                ),
            }
        }
    };
    let note = |errors: &[Rejection]| errors.first().map(|r| r.error.clone());
    match expect {
        None => match (bases.into_iter().next(), updates.into_iter().next()) {
            (Some(b), Some(u)) => Detection::Ready(b, u),
            (Some(base), None) => Detection::NeedUpdate { base, note: note(&update_errors) },
            (None, Some(update)) => Detection::NeedBase { update, note: note(&base_errors) },
            (None, None) => Detection::Rejected(first_error(&mut base_errors, &mut update_errors)),
        },
        Some(Kind::Update) => match (updates.into_iter().next(), stored) {
            (Some(u), Some(b)) => Detection::Ready(b.clone(), u),
            (Some(update), None) => Detection::NeedBase { update, note: None },
            (None, _) if !bases.is_empty() => {
                Detection::Rejected(Rejection { found: found_kind(&name, Kind::Base), error: holds_base(false) })
            }
            (None, _) => Detection::Rejected(first_error(&mut update_errors, &mut base_errors)),
        },
        Some(Kind::Base) => match (bases.into_iter().next(), stored) {
            (Some(b), Some(u)) => Detection::Ready(b, u.clone()),
            (Some(base), None) => Detection::NeedUpdate { base, note: None },
            (None, _) if !updates.is_empty() => {
                Detection::Rejected(Rejection { found: found_kind(&name, Kind::Update), error: holds_update(false) })
            }
            (None, _) => Detection::Rejected(first_error(&mut base_errors, &mut update_errors)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfo::{build, SfoValue::Text};
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    fn dump(tag: &str, title: &str, category: &str, app_ver: &str, content: &str, files: &[&str]) -> Dir {
        let root = std::env::temp_dir().join(format!("fzh-dump-{}-{tag}", std::process::id())).join(format!("{title}-{tag}"));
        std::fs::create_dir_all(root.join("sce_sys")).unwrap();
        let sfo = build(&[
            ("APP_VER", Text(app_ver.into())),
            ("CATEGORY", Text(category.into())),
            ("CONTENT_ID", Text(content.into())),
            ("TITLE_ID", Text(title.into())),
            ("VERSION", Text("01.00".into())),
        ]);
        std::fs::write(root.join("sce_sys").join("param.sfo"), sfo).unwrap();
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        Dir(root)
    }

    const EU: &str = "EP9000-CUSA11182_00-FIREWALL00000000";
    const US: &str = "UP9000-CUSA09831_00-FIREWALL00000000";

    #[test]
    fn base_ok() {
        let d = dump("b1", TITLE_ID, "gd", "01.00", EU, &["eboot.bin", GAME_PAK, "sce_discmap.plt"]);
        let info = check(Kind::Base, &d.0).unwrap();
        assert_eq!((info.title_id.as_str(), info.region.as_str(), info.version.as_str()), (TITLE_ID, "Europe", "01.00"));
    }

    #[test]
    fn base_rejections() {
        let d = dump("b2", USA_TITLE_ID, "gd", "01.00", US, &["eboot.bin", GAME_PAK]);
        let r = check(Kind::Base, &d.0).unwrap_err();
        assert!(r.error.message.contains("USA release"));
        assert!(r.found.message.ends_with("· USA · v01.00"));
        let d = dump("b3", TITLE_ID, "gp", "01.43", EU, &["eboot.bin", GAME_PAK]);
        assert_eq!(check(Kind::Base, &d.0).unwrap_err().error.message, "This folder holds the update, not the base game.");
        let d = dump("b4", TITLE_ID, "gd", "01.00", EU, &[GAME_PAK]);
        assert!(check(Kind::Base, &d.0).unwrap_err().error.message.contains("incomplete"));
        let missing = std::env::temp_dir().join("fzh-does-not-exist-anywhere");
        assert_eq!(check(Kind::Base, &missing).unwrap_err().error.message, "This folder does not exist.");
    }

    #[test]
    fn merged_folder_fills_both_slots() {
        let d = dump("m1", TITLE_ID, "gp", "01.43", EU, &["eboot.bin", GAME_PAK, "sce_discmap.plt", "sce_discmap_patch.plt"]);
        assert_eq!(check(Kind::Base, &d.0).unwrap().version, "01.00");
        assert_eq!(check(Kind::Update, &d.0).unwrap().version, "01.43");
    }

    fn write_dump(root: &Path, title: &str, category: &str, app_ver: &str, content: &str, files: &[&str]) {
        std::fs::create_dir_all(root.join("sce_sys")).unwrap();
        let sfo = build(&[
            ("APP_VER", Text(app_ver.into())),
            ("CATEGORY", Text(category.into())),
            ("CONTENT_ID", Text(content.into())),
            ("TITLE_ID", Text(title.into())),
            ("VERSION", Text("01.00".into())),
        ]);
        std::fs::write(root.join("sce_sys").join("param.sfo"), sfo).unwrap();
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
    }

    struct Parent(PathBuf);
    impl Drop for Parent {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parent(tag: &str) -> Parent {
        let p = std::env::temp_dir().join(format!("fzh-detect-{}-{tag}", std::process::id())).join("My dump");
        std::fs::create_dir_all(&p).unwrap();
        Parent(p.parent().unwrap().to_path_buf())
    }

    const BASE_FILES: &[&str] = &["eboot.bin", GAME_PAK, "sce_discmap.plt"];

    #[test]
    fn detect_merged_and_parent_folders() {
        let p = parent("d1");
        let dir = p.0.join("My dump");
        write_dump(&dir, TITLE_ID, "gp", "01.43", EU, &["eboot.bin", GAME_PAK, "sce_discmap.plt", "sce_discmap_patch.plt"]);
        assert!(matches!(detect(&dir, None, None), Detection::Ready(b, u) if b.path == u.path));

        let p = parent("d2");
        let dir = p.0.join("My dump");
        write_dump(&dir.join("anything"), TITLE_ID, "gd", "01.00", EU, BASE_FILES);
        write_dump(&dir.join("other name"), TITLE_ID, "gp", "01.43", EU, &["eboot.bin"]);
        std::fs::create_dir_all(dir.join("not a dump")).unwrap();
        match detect(&dir, None, None) {
            Detection::Ready(b, u) => {
                assert!(b.path.ends_with("anything"));
                assert!(u.path.ends_with("other name"));
                assert_eq!((b.version.as_str(), u.version.as_str()), ("01.00", "01.43"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn detect_one_half_then_the_other() {
        let p = parent("d3");
        let base_dir = p.0.join("My dump");
        write_dump(&base_dir, TITLE_ID, "gd", "01.00", EU, BASE_FILES);
        let Detection::NeedUpdate { base, note: None } = detect(&base_dir, None, None) else { panic!() };
        let upd_dir = p.0.join("upd");
        write_dump(&upd_dir, TITLE_ID, "gp", "01.43", EU, &["eboot.bin"]);
        assert!(matches!(detect(&upd_dir, Some(Kind::Update), Some(&base)), Detection::Ready(..)));
        assert_eq!(
            detect(&base_dir, Some(Kind::Update), Some(&base)),
            Detection::Rejected(Rejection { found: found_kind("My dump", Kind::Base), error: holds_base(false) })
        );
        let Detection::Rejected(r) = detect(&base_dir, Some(Kind::Update), Some(&base)) else { panic!() };
        assert_eq!((r.found.key, r.found.message.as_str()), ("dump.foundKind", "My dump · base game"));
        assert_eq!(r.found.params["kind"]["key"], "file.base");
        assert_eq!((r.error.key, r.error.message.as_str()), ("dump.holdsBase", "This folder holds the base game, not the update."));
        assert_eq!(r.error.params["noun"]["key"], "dump.noun.folder");

        let Detection::NeedBase { update, .. } = detect(&upd_dir, None, None) else { panic!() };
        assert!(matches!(detect(&base_dir, Some(Kind::Base), Some(&update)), Detection::Ready(..)));
        let Detection::Rejected(r) = detect(&upd_dir, Some(Kind::Base), Some(&update)) else { panic!() };
        assert_eq!(r.error.message, "This folder holds the update, not the base game.");
    }

    #[test]
    fn detect_wrong_title_region_and_version() {
        let p = parent("d4");
        let dir = p.0.join("My dump");
        write_dump(&dir, "CUSA00001", "gd", "01.00", EU, BASE_FILES);
        let Detection::Rejected(r) = detect(&dir, None, None) else { panic!() };
        assert_eq!(r.error.message, "This folder holds CUSA00001, not Firewall Zero Hour (CUSA11182).");

        let p = parent("d5");
        let dir = p.0.join("My dump");
        write_dump(&dir.join("base"), USA_TITLE_ID, "gd", "01.00", US, BASE_FILES);
        let Detection::Rejected(r) = detect(&dir, None, None) else { panic!() };
        assert!(r.error.message.starts_with("CUSA09831 is the USA release"));
        assert_eq!(r.found.message, "base · USA · v01.00");

        let p = parent("d6");
        let dir = p.0.join("My dump");
        write_dump(&dir.join("a"), TITLE_ID, "gd", "01.00", EU, BASE_FILES);
        write_dump(&dir.join("b"), TITLE_ID, "gp", "01.40", EU, &["eboot.bin"]);
        let Detection::NeedUpdate { note, .. } = detect(&dir, None, None) else { panic!() };
        assert_eq!(note.as_ref().map(|m| m.message.as_str()), Some("Update 1.40 found: version 1.43 is needed."));

        let p = parent("d7");
        let dir = p.0.join("My dump");
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        let Detection::Rejected(r) = detect(&dir, None, None) else { panic!() };
        assert!(r.error.message.starts_with("No PS4 game here"));
    }

    fn write_pkg(path: &Path, title: &str, category: &str, app_ver: &str, content: &str) {
        let sfo = build(&[
            ("APP_VER", Text(app_ver.into())),
            ("CATEGORY", Text(category.into())),
            ("CONTENT_ID", Text(content.into())),
            ("TITLE_ID", Text(title.into())),
            ("VERSION", Text("01.00".into())),
        ]);
        let cid: &[u8; 36] = content.as_bytes().try_into().unwrap();
        let bytes = crate::pfs::tests::synthetic_pkg(cid, &[(0x1000, &sfo)], &[("eboot.bin", b"x"), ("f", b"y")]);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn detect_pkg_files_by_header_not_name() {
        let p = parent("p1");
        let dir = p.0.join("My dump");
        write_pkg(&dir.join("zzz.pkg"), TITLE_ID, "gd", "01.00", EU);
        write_pkg(&dir.join("aaa.PKG"), TITLE_ID, "gp", "01.43", EU);
        match detect(&dir, None, None) {
            Detection::Ready(b, u) => {
                assert!(b.path.ends_with("zzz.pkg") && u.path.ends_with("aaa.PKG"));
                assert_eq!((b.source, u.source), (Source::Pkg, Source::Pkg));
                assert_eq!((b.version.as_str(), u.version.as_str(), b.region.as_str()), ("01.00", "01.43", "Europe"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn detect_pkg_then_folder_and_wrong_pkgs() {
        let p = parent("p2");
        let dir = p.0.join("My dump");
        write_pkg(&dir.join("sub").join("game.pkg"), TITLE_ID, "gd", "01.00", EU);
        write_pkg(&dir.join("old.pkg"), TITLE_ID, "gp", "01.40", EU);
        let Detection::NeedUpdate { base, note } = detect(&dir, None, None) else { panic!() };
        assert_eq!(base.source, Source::Pkg);
        assert_eq!(note.as_ref().map(|m| m.message.as_str()), Some("Update 1.40 found: version 1.43 is needed."));
        let upd = p.0.join("upd");
        write_dump(&upd, TITLE_ID, "gp", "01.43", EU, &["eboot.bin"]);
        let Detection::Ready(b, u) = detect(&upd, Some(Kind::Update), Some(&base)) else { panic!() };
        assert_eq!((b.source, u.source), (Source::Pkg, Source::Folder));

        let p = parent("p3");
        let dir = p.0.join("My dump");
        write_pkg(&dir.join("us.pkg"), USA_TITLE_ID, "gd", "01.00", US);
        let Detection::Rejected(r) = detect(&dir, None, None) else { panic!() };
        assert_eq!(r.found.message, "us.pkg · USA · v01.00");
        assert!(r.error.message.starts_with("CUSA09831 is the USA release"));
        write_pkg(&dir.join("base.pkg"), TITLE_ID, "gd", "01.00", EU);
        let Detection::Rejected(r) = detect(&dir, Some(Kind::Update), None) else { panic!() };
        assert_eq!(r.error.message, "This folder holds the base game, not the update.");

        let p = parent("p4");
        let dir = p.0.join("My dump");
        std::fs::write(dir.join("broken.pkg"), b"not a package at all").unwrap();
        let Detection::Rejected(r) = detect(&dir, None, None) else { panic!() };
        assert_eq!((r.found.message.as_str(), r.error.key), ("broken.pkg", "dump.notPkg"));
        write_pkg(&dir.join("other.pkg"), "CUSA00001", "gd", "01.00", EU);
        assert_eq!(
            check_pkg(Kind::Base, &dir.join("other.pkg")).unwrap_err().error.message,
            "This PKG file holds CUSA00001, not Firewall Zero Hour (CUSA11182)."
        );
        assert_eq!(check_pkg(Kind::Update, &dir.join("other.pkg")).unwrap_err().error.message, "This PKG file holds CUSA00001, not the Firewall Zero Hour update (CUSA11182).");
    }

    #[test]
    fn update_rules() {
        let d = dump("u1", TITLE_ID, "gp", "01.43", EU, &["eboot.bin"]);
        assert_eq!(check(Kind::Update, &d.0).unwrap().version, "01.43");
        let d = dump("u2", TITLE_ID, "gp", "01.40", EU, &["eboot.bin"]);
        assert_eq!(check(Kind::Update, &d.0).unwrap_err().error.message, "Update 1.40 found: version 1.43 is needed.");
        let d = dump("u3", USA_TITLE_ID, "gp", "01.43", US, &["eboot.bin"]);
        assert!(check(Kind::Update, &d.0).unwrap_err().error.message.contains("does not match your base game"));
        let d = dump("u4", TITLE_ID, "gd", "01.00", EU, &["eboot.bin"]);
        assert_eq!(check(Kind::Update, &d.0).unwrap_err().error.message, "This folder holds the base game, not the update.");
    }
}
