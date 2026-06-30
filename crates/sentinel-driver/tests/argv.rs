//! ADR 0066 M2.4 follow-on: own command-line argument reflection (`arg_count()` /
//! `arg(i)`). A spawned child (or any CLI program) can read its own invocation —
//! symmetric to `process_spawn(path, args)` passing argv to the child. These run the
//! exe WITH arguments, which the examples.rs harness (no-args) can't, so they live as
//! a focused custom test. The programs use only the builtins (no std), so they build
//! standalone. Like examples.rs, the build links (needs the host link toolchain).

use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_argv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn build(src: &str, dir: &Path, stem: &str) -> PathBuf {
    let entry = dir.join(format!("{stem}.sentinel"));
    std::fs::write(&entry, src).expect("write source");
    let exe = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
    let res = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    assert!(
        res.status.success(),
        "build of {stem} failed; stderr:\n{}",
        String::from_utf8_lossy(&res.stderr),
    );
    exe
}

#[test]
fn arg_count_reflects_invocation() {
    let dir = temp_dir();
    let exe = build("fn main() -> i64 { arg_count() }\n", &dir, "argc");
    // No extra args → just argv[0].
    let c0 = Command::new(&exe).output().expect("run").status.code().unwrap();
    assert_eq!(c0, 1, "arg_count() with no extra args should be 1 (argv[0])");
    // Two extra args → argv[0] + 2.
    let c2 = Command::new(&exe)
        .arg("x")
        .arg("y")
        .output()
        .expect("run")
        .status
        .code()
        .unwrap();
    assert_eq!(c2, 3, "arg_count() with 2 extra args should be 3");
}

#[test]
fn arg_reads_the_nth_argument() {
    let dir = temp_dir();
    // Return the first byte of arg(1) (or 99 if absent).
    let src = "fn main() -> i64 {\n    let a: [u8] = arg(1);\n    if len(a) > 0 { u8_to_i64(a[0]) } else { 99 }\n}\n";
    let exe = build(src, &dir, "argbyte");
    // "*" is ASCII 42 → the program returns 42.
    let code = Command::new(&exe)
        .arg("*")
        .output()
        .expect("run")
        .status
        .code()
        .unwrap();
    assert_eq!(code, 42, "arg(1)[0] of \"*\" should be 42 (ASCII '*')");
    // No arg(1) → the empty-buffer branch returns 99.
    let none = Command::new(&exe).output().expect("run").status.code().unwrap();
    assert_eq!(none, 99, "arg(1) absent should hit the empty branch");
}
