//! Extraction of real PKG files with the launcher's own code (`install::plan` + `install::extract`).
//! Ignored by default; the PKG files are only read. Run with:
//!
//! ```text
//! set FZH_TEST_BASE_PKG=<base .pkg>
//! set FZH_TEST_UPDATE_PKG=<update .pkg>
//! set FZH_TEST_REFERENCE=<an extracted copy of the game>    (plan test: file list and sizes)
//! set FZH_TEST_TARGET=<new folder>                           (extract test)
//! set FZH_TEST_CANCEL_AFTER=<seconds>                        (optional: cancel then stop)
//! cargo test --release --test live_pkg -- --ignored --nocapture <plan|extract>
//! ```

use fzh_launcher_lib::install;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn pkgs() -> (PathBuf, PathBuf) {
    (env_path("FZH_TEST_BASE_PKG").expect("FZH_TEST_BASE_PKG"), env_path("FZH_TEST_UPDATE_PKG").expect("FZH_TEST_UPDATE_PKG"))
}

fn listing(dir: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, prefix)) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let rel = format!("{prefix}{}", e.file_name().to_string_lossy());
            if e.file_type().unwrap().is_dir() {
                stack.push((e.path(), format!("{rel}/")));
            } else {
                out.insert(rel, e.metadata().unwrap().len());
            }
        }
    }
    out
}

#[test]
#[ignore]
fn plan() {
    let (base, update) = pkgs();
    let t = Instant::now();
    let plan = install::plan(Some(&base), Some(&update)).unwrap();
    println!("planned {} files, {} bytes, in {:.2} s", plan.items.len(), plan.total, t.elapsed().as_secs_f64());
    let planned: BTreeMap<String, u64> = plan.items.iter().map(|i| (i.rel.clone(), i.size)).collect();
    for (rel, size) in &planned {
        println!("{size:>14}  {rel}");
    }
    if let Some(reference) = env_path("FZH_TEST_REFERENCE") {
        let mut want = listing(&reference);
        for name in [fzh_launcher_lib::layout::MOD_PAK_NAME, fzh_launcher_lib::layout::LEGACY_MOD_PAK_NAME] {
            want.remove(&format!("firewall/content/paks/{name}"));
        }
        assert_eq!(planned, want, "file list and sizes differ from the reference");
        println!("same {} files and sizes as {}", want.len(), reference.display());
    }
}

#[test]
#[ignore]
fn extract() {
    let (base, update) = pkgs();
    let target = env_path("FZH_TEST_TARGET").expect("FZH_TEST_TARGET");
    let plan = install::plan(Some(&base), Some(&update)).unwrap();
    let present = plan.present(&target);
    println!("total {} bytes, already there {present}, free {:?}", plan.total, install::free_space(&target));
    let cancel = AtomicBool::new(false);
    let cancel_after = std::env::var("FZH_TEST_CANCEL_AFTER").ok().and_then(|s| s.parse::<u64>().ok()).map(Duration::from_secs);
    let started = Instant::now();
    let mut last = Instant::now();
    let mut last_file = String::new();
    let mut first_written: Option<(String, u64)> = None;
    let done = install::extract(&plan, &target, 6, &cancel, &mut |p| {
        if first_written.is_none() && p.done > present {
            first_written = Some((p.file.clone(), p.done));
        }
        if p.file != last_file || last.elapsed() >= Duration::from_secs(10) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            println!(
                "{:>7.1} s  {:>6.2}/{:.2} GiB  {:>5.0} MB/s  {}",
                secs,
                p.done as f64 / (1u64 << 30) as f64,
                p.total as f64 / (1u64 << 30) as f64,
                p.done.saturating_sub(present) as f64 / (1u64 << 20) as f64 / secs,
                p.file
            );
            last = Instant::now();
            last_file = p.file.clone();
        }
        if cancel_after.is_some_and(|d| started.elapsed() >= d) {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    let secs = started.elapsed().as_secs_f64();
    let after = plan.present(&target);
    println!(
        "{} after {secs:.1} s: {} bytes written, {:.0} MB/s; present now {after} of {}; first byte written in {:?}",
        if done { "finished" } else { "cancelled" },
        after - present,
        (after - present) as f64 / (1u64 << 20) as f64 / secs.max(0.001),
        plan.total,
        first_written
    );
}
