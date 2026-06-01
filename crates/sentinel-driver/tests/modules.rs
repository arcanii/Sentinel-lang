//! Phase D.6 (1/N) / ADR 0037: multi-file module-graph discovery.
//!
//! `snc build <entry>` follows `use` edges from the entry file, mapping
//! each `use a::b::Item;` to module `a::b` → file `<root>/a/b.sentinel`
//! (the source root is the entry's directory; the last path segment is
//! the imported item). These tests drive the real `snc` binary on temp
//! multi-file projects. Multi-module compilation (per-unit resolve +
//! separate codegen + link) is the NEXT D.6 (1/N) increment, so for now
//! a discovered multi-module graph is reported + gated; this verifies the
//! discovery itself — the path→file mapping and the ModuleNotFound edge.

use std::path::PathBuf;
use std::process::Command;

/// A fresh temp project directory (unique per test + process, so the
/// parallel test runner never collides). Best-effort cleared first.
fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_d6_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");
    dir
}

fn write(path: PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, contents).expect("write source file");
}

/// Run `snc build <entry>` and return (success, stderr).
fn build(entry: PathBuf) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(entry.with_extension(""))
        .output()
        .expect("run snc");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn multi_module_build_is_gated_with_discovered_modules() {
    // `use util::add;` → module `util` (file `util.sentinel`); a sibling
    // module is discovered, and multi-module compilation is gated (the
    // next increment wires per-unit resolve + separate codegen).
    let dir = temp_project("multi");
    write(dir.join("util.sentinel"), "fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "multi-module build should be gated (non-zero exit); stderr:\n{stderr}");
    assert!(
        stderr.contains("multi-module compilation is not yet supported"),
        "expected the multi-module gate message; stderr:\n{stderr}"
    );
    assert!(stderr.contains("util"), "should name the discovered module; stderr:\n{stderr}");
}

#[test]
fn nested_module_path_maps_to_subdirectory() {
    // ADR 0037 open point 4: `use util::math::add;` → module `util::math`
    // → file `util/math.sentinel` (the last segment, `add`, is the item).
    let dir = temp_project("nested");
    write(dir.join("util/math.sentinel"), "fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::math::add;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "still gated; stderr:\n{stderr}");
    assert!(
        stderr.contains("util::math"),
        "the nested module path should resolve to util/math.sentinel; stderr:\n{stderr}"
    );
}

#[test]
fn use_of_missing_module_is_module_not_found() {
    // A `use` whose module file does not exist is surfaced at discovery
    // (before resolve) as a clear ModuleNotFound, naming the expected file.
    let dir = temp_project("missing");
    write(dir.join("main.sentinel"), "use absent::thing;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "a missing module should fail the build; stderr:\n{stderr}");
    assert!(
        stderr.contains("module `absent` not found"),
        "expected a ModuleNotFound naming `absent`; stderr:\n{stderr}"
    );
}
