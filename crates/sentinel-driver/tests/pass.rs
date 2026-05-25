//! Pass tests for the C0.2+ codegen pipeline.
//!
//! Each `.sentinel` file under workspace-root `tests/pass/` is fed
//! through `snc build`; the resulting executable is invoked and the
//! [`RunResult`] (exit code + captured stdout) is asserted. The
//! exit code is the value of the program's trailing expression
//! truncated to i32 — the "exit-code-is-the-answer" convention
//! from C0.2 still holds; C0.4 adds `print(x)` which writes to
//! stdout (so a single test can assert on both stdout and exit).

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

#[derive(Debug)]
struct RunResult {
    exit: i32,
    stdout: String,
}

/// Build the named fixture from `tests/pass/` into
/// `target/sentinel-pass/`, run the resulting executable, and
/// return its exit code and captured stdout.
fn build_and_run(fixture: &str) -> RunResult {
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

    let run = Command::new(&exe).output().expect("running compiled binary failed");
    RunResult {
        exit: run.status.code().expect("process killed by signal"),
        stdout: String::from_utf8(run.stdout).expect("stdout is not valid UTF-8"),
    }
}

/// Shortcut for tests that only care about the exit code.
fn run_exit(fixture: &str) -> i32 {
    build_and_run(fixture).exit
}

#[test]
fn pass_c02_arithmetic() {
    // 6 + 7 == 13
    assert_eq!(run_exit("c02_arithmetic.sentinel"), 13);
}

#[test]
fn pass_c02_precedence() {
    // 1 + 2 * 3 == 7  (mul binds tighter than add)
    assert_eq!(run_exit("c02_precedence.sentinel"), 7);
}

#[test]
fn pass_c02_parens() {
    // (5 + 3) * 2 - 1 == 15  (parens force precedence)
    assert_eq!(run_exit("c02_parens.sentinel"), 15);
}

#[test]
fn pass_c02_unary() {
    // -(-5) == 5  (double unary minus)
    assert_eq!(run_exit("c02_unary.sentinel"), 5);
}

#[test]
fn pass_c02_division() {
    // 12 / 3 == 4  (signed integer division)
    assert_eq!(run_exit("c02_division.sentinel"), 4);
}

#[test]
fn pass_c03_simple_let() {
    // let x = 5; x == 5
    assert_eq!(run_exit("c03_simple_let.sentinel"), 5);
}

#[test]
fn pass_c03_multiple_lets() {
    // let x = 3; let y = 4; x + y == 7
    assert_eq!(run_exit("c03_multiple_lets.sentinel"), 7);
}

#[test]
fn pass_c03_let_uses_let() {
    // let a = 2; let b = a * 3; b + 1 == 7
    assert_eq!(run_exit("c03_let_uses_let.sentinel"), 7);
}

#[test]
fn pass_c03_expr_statement() {
    // let x = 1; x + 99; 5 == 5  (expr-stmt computed and discarded)
    assert_eq!(run_exit("c03_expr_statement.sentinel"), 5);
}

#[test]
fn pass_c04_print_simple() {
    // print(42) -> stdout "42\n", exit 0 (print returns 0)
    let r = build_and_run("c04_print_simple.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c04_print_then_tail() {
    // let x = 7; print(x); print(x * 2); x + x * 2
    // -> stdout "7\n14\n", exit 21
    let r = build_and_run("c04_print_then_tail.sentinel");
    assert_eq!(r.stdout, "7\n14\n");
    assert_eq!(r.exit, 21);
}

#[test]
fn pass_c04_if_true_branch() {
    // if 1 { 42 } else { 99 } -> exit 42 (cond non-zero)
    let r = build_and_run("c04_if_true_branch.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c04_if_false_branch() {
    // if 0 { 42 } else { 99 } -> exit 99 (cond zero)
    assert_eq!(run_exit("c04_if_false_branch.sentinel"), 99);
}

#[test]
fn pass_c04_if_with_var_cond() {
    // let x = 5; if x { x * 2 } else { 0 } -> exit 10
    assert_eq!(run_exit("c04_if_with_var_cond.sentinel"), 10);
}

#[test]
fn pass_c04_else_if_chain() {
    // let x = 0; if x { 1 } else if x { 2 } else { 3 } -> exit 3
    assert_eq!(run_exit("c04_else_if_chain.sentinel"), 3);
}

#[test]
fn pass_c04_block_expression() {
    // let r = { let y = 4; y + 1 }; r * 2 -> exit 10
    assert_eq!(run_exit("c04_block_expression.sentinel"), 10);
}

#[test]
fn pass_c04_if_with_print() {
    // let x = 1; if x { print(100) } else { print(200) }
    // -> stdout "100\n", exit 0 (print's return is the if-value)
    let r = build_and_run("c04_if_with_print.sentinel");
    assert_eq!(r.stdout, "100\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c05_simple_fn() {
    // fn double(x) { x * 2 }; fn main() { double(7) } -> exit 14
    assert_eq!(run_exit("c05_simple_fn.sentinel"), 14);
}

#[test]
fn pass_c05_multi_arg_fn() {
    // fn add(a, b) { a + b }; fn main() { add(5, 6) } -> exit 11
    assert_eq!(run_exit("c05_multi_arg_fn.sentinel"), 11);
}

#[test]
fn pass_c05_forward_ref() {
    // fn main calls triple defined AFTER main -> exit 12
    assert_eq!(run_exit("c05_forward_ref.sentinel"), 12);
}

#[test]
fn pass_c05_call_chain() {
    // quad(3) = double(double(3)) = 12
    assert_eq!(run_exit("c05_call_chain.sentinel"), 12);
}

#[test]
fn pass_c05_go_no_go() {
    // ADR 0010 appendix go/no-go program: double + pick + main with print
    // -> stdout "10\n", exit 0 (print returns 0)
    let r = build_and_run("c05_go_no_go.sentinel");
    assert_eq!(r.stdout, "10\n");
    assert_eq!(r.exit, 0);
}
