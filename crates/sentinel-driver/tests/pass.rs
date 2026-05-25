//! Pass tests for the C0.2+ codegen pipeline.
//!
//! Each `.sentinel` file under workspace-root `tests/pass/` is fed
//! through `snc build`, the resulting executable is invoked, and
//! the exit code is asserted. The exit code is the value of the
//! program's top-level expression truncated to i32 (the C0.2
//! "exit-code-is-the-answer" convention; C0.4 will switch to
//! stdout via `print` per ADR 0010 D11). Only non-negative results
//! that fit in a single POSIX exit byte (0..255) are testable
//! under this convention.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .parent()
        .expect("crates/ has a parent")
        .to_path_buf()
}

fn snc_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_snc"))
}

fn build_dir() -> PathBuf {
    workspace_root().join("target/sentinel-pass")
}

/// Build the named fixture from `tests/pass/` into
/// `target/sentinel-pass/`, run the resulting executable, and
/// return its exit code.
fn build_and_run(fixture: &str) -> i32 {
    let src = workspace_root().join("tests/pass").join(fixture);
    let out_dir = build_dir();
    std::fs::create_dir_all(&out_dir).expect("create build dir");
    let exe = out_dir.join(PathBuf::from(fixture).with_extension(""));

    let build = Command::new(snc_binary())
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("snc invocation failed");
    if !build.status.success() {
        panic!(
            "snc build {fixture} failed: status={}, stderr={}",
            build.status,
            String::from_utf8_lossy(&build.stderr)
        );
    }

    let run = Command::new(&exe).status().expect("running compiled binary failed");
    run.code().expect("process killed by signal")
}

#[test]
fn pass_c02_arithmetic() {
    // 6 + 7 == 13
    assert_eq!(build_and_run("c02_arithmetic.sentinel"), 13);
}

#[test]
fn pass_c02_precedence() {
    // 1 + 2 * 3 == 7  (mul binds tighter than add)
    assert_eq!(build_and_run("c02_precedence.sentinel"), 7);
}

#[test]
fn pass_c02_parens() {
    // (5 + 3) * 2 - 1 == 15  (parens force precedence)
    assert_eq!(build_and_run("c02_parens.sentinel"), 15);
}

#[test]
fn pass_c02_unary() {
    // -(-5) == 5  (double unary minus)
    assert_eq!(build_and_run("c02_unary.sentinel"), 5);
}

#[test]
fn pass_c02_division() {
    // 12 / 3 == 4  (signed integer division)
    assert_eq!(build_and_run("c02_division.sentinel"), 4);
}

#[test]
fn pass_c03_simple_let() {
    // let x = 5; x == 5
    assert_eq!(build_and_run("c03_simple_let.sentinel"), 5);
}

#[test]
fn pass_c03_multiple_lets() {
    // let x = 3; let y = 4; x + y == 7
    assert_eq!(build_and_run("c03_multiple_lets.sentinel"), 7);
}

#[test]
fn pass_c03_let_uses_let() {
    // let a = 2; let b = a * 3; b + 1 == 7
    assert_eq!(build_and_run("c03_let_uses_let.sentinel"), 7);
}

#[test]
fn pass_c03_expr_statement() {
    // let x = 1; x + 99; 5 == 5  (expr-stmt computed and discarded)
    assert_eq!(build_and_run("c03_expr_statement.sentinel"), 5);
}
