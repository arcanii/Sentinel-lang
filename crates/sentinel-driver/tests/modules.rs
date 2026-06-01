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

/// Run `snc build <entry>` (output next to the entry) and return
/// (success, stderr).
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

/// Build `entry`, assert it compiled, run the binary, and return its exit
/// code (the program's tail value, the "exit-code-is-the-answer" rule).
fn build_and_run(entry: PathBuf) -> i32 {
    let (ok, stderr) = build(entry.clone());
    assert!(ok, "expected a successful multi-file build; stderr:\n{stderr}");
    let exe = entry.with_extension("");
    let run = Command::new(&exe).output().expect("run compiled binary");
    run.status.code().expect("process exited normally")
}

#[test]
fn cross_module_call_compiles_and_runs() {
    // The payoff: `use util::add;` + a cross-module call compiles to a
    // binary and runs. add(2, 3) -> exit 5.
    let dir = temp_project("multi");
    write(dir.join("util.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { add(2, 3) }\n");
    assert_eq!(build_and_run(dir.join("main.sentinel")), 5);
}

#[test]
fn nested_module_path_compiles_and_runs() {
    // ADR 0037 open point 4: `use util::math::add;` → module `util::math`
    // → file `util/math.sentinel` (the last segment, `add`, is the item).
    let dir = temp_project("nested");
    write(dir.join("util/math.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::math::add;\nfn main() -> i64 { add(10, 5) }\n");
    assert_eq!(build_and_run(dir.join("main.sentinel")), 15);
}

#[test]
fn private_names_across_modules_do_not_collide() {
    // The point of qualification: two modules each declare a private
    // `helper`; they must not clash, and a `pub fn` calls its OWN module's
    // private. util::compute(4) = helper(4)*10 + 1 = 41; main's own
    // (unused) `helper` returns 99.
    let dir = temp_project("collide");
    write(
        dir.join("util.sentinel"),
        "pub fn compute(x: i64) -> i64 { helper(x) + 1 }\nfn helper(x: i64) -> i64 { x * 10 }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::compute;\nfn helper() -> i64 { 99 }\nfn main() -> i64 { compute(4) }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 41);
}

#[test]
fn import_of_private_item_is_rejected() {
    // ADR 0037 D3: importing a non-`pub` item is a visibility error,
    // surfaced before the compilation gate.
    let dir = temp_project("private");
    write(dir.join("util.sentinel"), "fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "a private import should fail the build; stderr:\n{stderr}");
    assert!(
        stderr.contains("private to module `util`"),
        "expected a PrivateItem error naming `util`; stderr:\n{stderr}"
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
