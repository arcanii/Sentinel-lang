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
    // if true { 42 } else { 99 } -> exit 42 (cond true; C1.3 rewrite
    // of the C0 `if 1 { ... }` shape per ADR 0012 D10)
    let r = build_and_run("c04_if_true_branch.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c04_if_false_branch() {
    // if false { 42 } else { 99 } -> exit 99 (C1.3 rewrite)
    assert_eq!(run_exit("c04_if_false_branch.sentinel"), 99);
}

#[test]
fn pass_c04_if_with_var_cond() {
    // let x = 5; if x != 0 { x * 2 } else { 0 } -> exit 10
    // (C1.3 rewrite: `if x` becomes `if x != 0` per ADR 0012 D10)
    assert_eq!(run_exit("c04_if_with_var_cond.sentinel"), 10);
}

#[test]
fn pass_c04_else_if_chain() {
    // let x = 0; if x != 0 { 1 } else if x != 0 { 2 } else { 3 } -> exit 3
    // (C1.3 rewrite)
    assert_eq!(run_exit("c04_else_if_chain.sentinel"), 3);
}

#[test]
fn pass_c04_block_expression() {
    // let r = { let y = 4; y + 1 }; r * 2 -> exit 10
    assert_eq!(run_exit("c04_block_expression.sentinel"), 10);
}

#[test]
fn pass_c04_if_with_print() {
    // let x = 1; if x != 0 { print(100) } else { print(200) }
    // -> stdout "100\n", exit 0 (print's return is the if-value).
    // C1.3 rewrite: `if x` -> `if x != 0` per ADR 0012 D10.
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
    // C1.3 phase-go program per ADR 0012 appendix: double + is_positive
    // + pick(cond: bool, ...) + main with print. Exercises bool flow
    // end-to-end (`x > 0` returns bool, `is_positive(x)` returns bool,
    // `pick(bool, ...)` takes bool, `if cond` requires bool).
    // -> stdout "10\n", exit 0.
    let r = build_and_run("c05_go_no_go.sentinel");
    assert_eq!(r.stdout, "10\n");
    assert_eq!(r.exit, 0);
}

// ----- C1.3 pass-tests: bool / comparisons / logicals / short-circuit -----

#[test]
fn pass_c13_bool_literal() {
    // if true { 7 } else { 99 } -> exit 7
    assert_eq!(run_exit("c13_bool_literal.sentinel"), 7);
}

#[test]
fn pass_c13_comparison() {
    // if 5 > 3 { 12 } else { 0 } -> exit 12
    assert_eq!(run_exit("c13_comparison.sentinel"), 12);
}

#[test]
fn pass_c13_logical_and() {
    // let x = 5; if x > 0 && x < 10 { 42 } else { 0 } -> exit 42
    assert_eq!(run_exit("c13_logical_and.sentinel"), 42);
}

#[test]
fn pass_c13_logical_or() {
    // let x = 7; if x == 0 || x > 5 { 11 } else { 0 } -> exit 11
    assert_eq!(run_exit("c13_logical_or.sentinel"), 11);
}

#[test]
fn pass_c13_unary_not() {
    // if !false { 5 } else { 0 } -> exit 5
    assert_eq!(run_exit("c13_unary_not.sentinel"), 5);
}

#[test]
fn pass_c13_short_circuit_and() {
    // `false && print(99) > 0` must short-circuit: print never runs.
    // -> stdout "", exit 7
    let r = build_and_run("c13_short_circuit_and.sentinel");
    assert_eq!(r.stdout, "", "rhs of && was evaluated despite lhs being false");
    assert_eq!(r.exit, 7);
}

#[test]
fn pass_c13_short_circuit_or() {
    // `true || print(99) > 0` must short-circuit: print never runs.
    // -> stdout "", exit 3
    let r = build_and_run("c13_short_circuit_or.sentinel");
    assert_eq!(r.stdout, "", "rhs of || was evaluated despite lhs being true");
    assert_eq!(r.exit, 3);
}

// ----- C1.4 pass-tests: structs + field access -----

#[test]
fn pass_c14_struct_basic() {
    // struct Box { v: i64 }; Box { v: 42 }.v -> exit 42
    assert_eq!(run_exit("c14_struct_basic.sentinel"), 42);
}

#[test]
fn pass_c14_struct_nested() {
    // Outer { inner: Inner { x: 7 }, y: 3 }.inner.x + .y -> exit 10
    assert_eq!(run_exit("c14_struct_nested.sentinel"), 10);
}

#[test]
fn pass_c14_struct_in_if() {
    // pick(true) returns Pair { a: 1, b: 2 }; sum is 3.
    // Exercises the if-merge alloca for struct-typed results.
    assert_eq!(run_exit("c14_struct_in_if.sentinel"), 3);
}

#[test]
fn pass_c14_struct_bool_field() {
    // Tagged { value: 5, valid: true, count: 9 } — valid is true,
    // so result is 5 + 9 = 14.
    assert_eq!(run_exit("c14_struct_bool_field.sentinel"), 14);
}

#[test]
fn pass_c14_go_no_go() {
    // ADR 0013 D12 phase-go program: Point { x: 3, y: 4 } ->
    // manhattan returns 7 -> print produces stdout "7\n", exit 0.
    let r = build_and_run("c14_go_no_go.sentinel");
    assert_eq!(r.stdout, "7\n");
    assert_eq!(r.exit, 0);
}

// ----- C1.5 pass-tests: ?T + null + unwrap_or / is_some + == null -----

#[test]
fn pass_c15_null_literal() {
    // let x: ?i64 = null; if is_some(x) { 1 } else { 7 } -> exit 7
    assert_eq!(run_exit("c15_null_literal.sentinel"), 7);
}

#[test]
fn pass_c15_widen() {
    // let x: ?i64 = 42; unwrap_or(x, 0) -> exit 42 (i64 widens to ?i64)
    assert_eq!(run_exit("c15_widen.sentinel"), 42);
}

#[test]
fn pass_c15_eq_null() {
    // let x: ?i64 = null; if x == null { 11 } else { 0 } -> exit 11
    assert_eq!(run_exit("c15_eq_null.sentinel"), 11);
}

#[test]
fn pass_c15_nullable_struct_field() {
    // Pair { first: 5, second: 8 }; unwrap_or(p.first, 0) + p.second -> exit 13
    // (the `first` field's `5` literal widens to ?i64 inside the
    // struct literal because the field's declared type is ?i64).
    assert_eq!(run_exit("c15_nullable_struct_field.sentinel"), 13);
}

#[test]
fn pass_c15_maybe_compose() {
    // maybe_double(some 5) = some 10; unwrap_or = 10.
    // maybe_double(null) = null; unwrap_or with default 999 = 999.
    // 10 + 999 = 1009 -> stdout "1009\n", exit 0. (Uses print
    // because the value doesn't fit in i32 exit code range.)
    let r = build_and_run("c15_maybe_compose.sentinel");
    assert_eq!(r.stdout, "1009\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c15_go_no_go() {
    // ADR 0014 phase-go 1 program: find_or(some 42, 0) + find_or(null, 100)
    // = 42 + 100 = 142 -> stdout "142\n", exit 0.
    let r = build_and_run("c15_go_no_go.sentinel");
    assert_eq!(r.stdout, "142\n");
    assert_eq!(r.exit, 0);
}

// ----- C1.6 pass-tests: arrays + len + indexing + recursive struct unlock -----

#[test]
fn pass_c16_array_basic() {
    // [10, 20, 30]; xs[1] + len(xs) = 20 + 3 = 23 -> exit 23
    assert_eq!(run_exit("c16_array_basic.sentinel"), 23);
}

#[test]
fn pass_c16_empty_array() {
    // let xs: [i64] = []; len(xs) -> exit 0
    assert_eq!(run_exit("c16_empty_array.sentinel"), 0);
}

#[test]
fn pass_c16_array_as_arg() {
    // first([7, 8, 9]) -> exit 7 (the [0] element)
    assert_eq!(run_exit("c16_array_as_arg.sentinel"), 7);
}

#[test]
fn pass_c16_array_of_struct() {
    // [Point{1,2}, Point{3,4}]; ps[0].x + ps[1].y = 1 + 4 = 5 -> exit 5
    assert_eq!(run_exit("c16_array_of_struct.sentinel"), 5);
}

#[test]
fn pass_c16_array_in_struct() {
    // Bag { items: [3, 4, 5], owner_id: 7 }; b.items[2] + b.owner_id
    // = 5 + 7 = 12 -> exit 12
    assert_eq!(run_exit("c16_array_in_struct.sentinel"), 12);
}

#[test]
fn pass_c16_linked_list_node() {
    // ADR 0014 D10 unlock via ADR 0015 D11. Node { value: 99,
    // next: null }; head.value -> exit 99.
    assert_eq!(run_exit("c16_linked_list_node.sentinel"), 99);
}

#[test]
fn pass_c16_go_no_go() {
    // ADR 0015 phase-go 1: sum_from([1,2,3,4,5], 0)
    // = 1+2+3+4+5 = 15 -> stdout "15\n", exit 0.
    let r = build_and_run("c16_go_no_go.sentinel");
    assert_eq!(r.stdout, "15\n");
    assert_eq!(r.exit, 0);
}
