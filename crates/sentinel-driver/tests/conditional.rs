//! ADR 0062 — file-level conditional compilation (target-suffixed module files).
//!
//! A module `plat` is provided in three variants — `plat.sentinel` (portable
//! default, `which()=1`), `plat_windows.sentinel` (`=2`), and `plat_unix.sentinel`
//! (`=3`). The resolver selects one per the active target (`--target`, default
//! host), mapping the suffixed file back to module `plat` so the importer always
//! writes `use plat::which`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_cfg_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    // which() returns a distinct value per variant.
    std::fs::write(dir.join("plat.sentinel"), "pub fn which() -> i64 { 1 }\n").unwrap();
    std::fs::write(dir.join("plat_windows.sentinel"), "pub fn which() -> i64 { 2 }\n").unwrap();
    std::fs::write(dir.join("plat_unix.sentinel"), "pub fn which() -> i64 { 3 }\n").unwrap();
    std::fs::write(dir.join("main.sentinel"), "use plat::which;\nfn main() -> i64 { which() }\n").unwrap();
    dir
}

/// The variant `which()` value expected for an OS atom (the resolver's
/// `_<os>` → `_unix` → default precedence).
fn expected_for(os: &str) -> i32 {
    match os {
        "windows" => 2,
        "linux" | "macos" => 3, // the `_unix` family
        _ => 1,                 // no variant → the portable default
    }
}

#[test]
fn the_host_variant_is_selected_by_default() {
    // No `--target`: the resolver picks the host's variant. Verified via `snc
    // merge` (no linker needed) — the merged program carries that variant's body.
    let dir = temp_project("host");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("merge")
        .arg(dir.join("main.sentinel"))
        .output()
        .expect("run snc merge");
    assert!(out.status.success(), "merge failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let merged = String::from_utf8_lossy(&out.stdout);
    let want = format!("which() -> i64 {{ {} }}", expected_for(std::env::consts::OS));
    assert!(merged.contains(&want), "host {}: expected `{want}` in the merge:\n{merged}", std::env::consts::OS);
}

/// Build `main.sentinel` for `target` and run it; returns its exit code, or
/// `None` if no host linker is available (the test then skips).
fn build_and_run(dir: &Path, target: &str) -> Option<i32> {
    let exe = dir.join(format!("app{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .current_dir(dir)
        .arg("build")
        .arg("main.sentinel")
        .arg("-o")
        .arg(&exe)
        .arg("--target")
        .arg(target)
        .output()
        .expect("run snc build");
    if !build.status.success() {
        let err = String::from_utf8_lossy(&build.stderr);
        if err.contains("link") {
            eprintln!("skipping — no host linker:\n{err}");
            return None;
        }
        panic!("snc build --target {target} failed:\n{err}");
    }
    let run = Command::new(&exe).output().expect("run app");
    run.status.code()
}

#[test]
fn target_flag_selects_the_matching_variant() {
    let dir = temp_project("target");
    // The first build doubles as the linker probe: if absent, skip the whole test.
    let Some(linux) = build_and_run(&dir, "linux") else { return };
    assert_eq!(linux, 3, "linux → the `_unix` variant");
    assert_eq!(build_and_run(&dir, "macos").unwrap(), 3, "macos → the `_unix` variant");
    assert_eq!(build_and_run(&dir, "windows").unwrap(), 2, "windows → the `_windows` variant");
    assert_eq!(build_and_run(&dir, "freebsd").unwrap(), 1, "an unknown target → the portable default");
    // The alias `win32` normalizes to `windows`.
    assert_eq!(build_and_run(&dir, "win32").unwrap(), 2, "`win32` aliases `windows`");
}
