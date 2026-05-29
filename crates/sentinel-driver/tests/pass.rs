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

// ---- C1.7.5 / ADR 0016: user-defined generic fns + monomorphization ----

#[test]
fn pass_c17_id() {
    // The minimum-viable generic fn: `id<T>(x: T) -> T { x }`
    // monomorphised to `id__i64` at the i64 call site.
    let r = build_and_run("c17_id.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c17_two_instantiations() {
    // `pick<T>` instantiated for both T = i64 and T = bool.
    // Codegen emits `pick__i64` AND `pick__bool` as separate
    // LLVM fns. 41 + 0 = 41 (pick(false, true, false) = false,
    // to_i64(false) = 0).
    let r = build_and_run("c17_two_instantiations.sentinel");
    assert_eq!(r.stdout, "41\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c17_generic_nullable() {
    // Generic fn with `?T` in its signature, body calls the
    // `unwrap_or` builtin. 99 + 1 = 100.
    let r = build_and_run("c17_generic_nullable.sentinel");
    assert_eq!(r.stdout, "100\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c17_generic_array() {
    // Generic fn with `[T]` param, body calls the `len`
    // builtin. count<i64>([10,20,30,40]) + count<bool>([true,false])
    // = 4 + 2 = 6.
    let r = build_and_run("c17_generic_array.sentinel");
    assert_eq!(r.stdout, "6\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c17_box() {
    // Single-param generic struct. `Box<i64>` is one
    // GenericInstance; `unbox` is non-generic and reads its field.
    let r = build_and_run("c17_box.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c17_go_no_go() {
    // ADR 0016 D12 phase-go: Pair<A, B> + make_pair / fst / snd +
    // pick_int composition. The full generic-struct + generic-fn
    // dance: monomorphises `make_pair<i64, i64>`, `fst<i64, i64>`,
    // `snd<i64, i64>` and emits an LLVM struct type for
    // `Pair<i64, i64>`. Result: 7 + 35 = 42.
    let r = build_and_run("c17_go_no_go.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.0.2 / ADR 0017: refs + mutability + deref + assignment ----

#[test]
fn pass_c20_ref_basic() {
    // `add(&a, &b)` with `&i64` params + `*a + *b` body. 10 + 32 = 42.
    assert_eq!(run_exit("c20_ref_basic.sentinel"), 42);
}

#[test]
fn pass_c20_mut_basic() {
    // `let mut x = 0; x = x + 1; x = x + 1; x * 2` -> exit 4.
    // Exercises let-mut binding + same-name assignment statement.
    assert_eq!(run_exit("c20_mut_basic.sentinel"), 4);
}

#[test]
fn pass_c20_deref_basic() {
    // `increment(&mut a)` mutates `a` in place. After increment,
    // a = 11 and the call returns 11. 11 + 11 = 22.
    assert_eq!(run_exit("c20_deref_basic.sentinel"), 22);
}

#[test]
fn pass_c20_go_no_go() {
    // ADR 0017 D14 phase-go: `add(&a, &b)` (shared) + `increment(&mut a)`
    // (exclusive) + let-mut + deref-assign + print. sum = 42; inc = 11
    // (after `a` advances 10 -> 11); sum + inc = 53.
    let r = build_and_run("c20_go_no_go.sentinel");
    assert_eq!(r.stdout, "53\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.1 / ADR 0017 D6: shared-only lexical borrow checker ----

#[test]
fn pass_c21_borrow_local_ok() {
    // `let r = &x; *r + *r` — borrow used within source's scope.
    // 5 + 5 = 10.
    assert_eq!(run_exit("c21_borrow_local_ok.sentinel"), 10);
}

#[test]
fn pass_c21_pass_through_ref() {
    // `fn pass(r: &i64) -> &i64 { r }` — returning an incoming
    // ref is sound. Caller derefs the result. exit 17.
    assert_eq!(run_exit("c21_pass_through_ref.sentinel"), 17);
}

#[test]
fn pass_c21_reborrow() {
    // `& *r` reborrow shape — source propagates through deref.
    // The result ref's source is Incoming (caller's), so escapable
    // via return. exit 21.
    assert_eq!(run_exit("c21_reborrow.sentinel"), 21);
}

#[test]
fn pass_c21_go_no_go() {
    // ADR 0017 D6 phase-go (shared-only subset): sum_two(&a, &b)
    // + triple(&a) + triple(&b) = 42 + 30 + 96 = 168.
    let r = build_and_run("c21_go_no_go.sentinel");
    assert_eq!(r.stdout, "168\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.2 / ADR 0017 D6: shared-XOR-mutable rule ----

#[test]
fn pass_c22_multi_shared() {
    // Three `&x` borrows of the same place coexist. 5 + 5 + 5 = 15.
    assert_eq!(run_exit("c22_multi_shared.sentinel"), 15);
}

#[test]
fn pass_c22_scoped_mut() {
    // `&mut x` scoped to an inner block; after exit, x is
    // borrow-free. a = 17 (mut deref result); x = 17 (post-write).
    // 17 + 17 = 34.
    assert_eq!(run_exit("c22_scoped_mut.sentinel"), 34);
}

#[test]
fn pass_c22_transient_then_mut() {
    // `add(&x, &x)` transient shared borrows die at stmt end;
    // next statement takes `&mut x` cleanly. 10 + 6 = 16.
    assert_eq!(run_exit("c22_transient_then_mut.sentinel"), 16);
}

#[test]
fn pass_c22_go_no_go() {
    // ADR 0017 D6 XOR phase-go: shared-multi block then mut
    // block. a = 20 (shared reads); b = 15 (after +5); 20 + 15 = 35.
    let r = build_and_run("c22_go_no_go.sentinel");
    assert_eq!(r.stdout, "35\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.3 / ADR 0017 D9: move semantics + use-after-move ----

#[test]
fn pass_c23_move_struct() {
    // Single move of a Point into manhattan. 3 + 4 = 7.
    assert_eq!(run_exit("c23_move_struct.sentinel"), 7);
}

#[test]
fn pass_c23_branch_isolation() {
    // if/else both branches move p (into fst/snd). Branch-aware
    // merge sees p as Moved after the if/else, which is fine
    // since the if/else IS the tail. exit 1.
    assert_eq!(run_exit("c23_branch_isolation.sentinel"), 1);
}

#[test]
fn pass_c23_array_move() {
    // Array passed by value moves; xs[i] (postfix) doesn't
    // consume; len(xs) (runtime builtin) doesn't consume;
    // sum_to_end(xs, ...) (user-defined) does. Sum = 15.
    assert_eq!(run_exit("c23_array_move.sentinel"), 15);
}

#[test]
fn pass_c23_go_no_go() {
    // ADR 0017 D9 phase-go: Account struct, transfer(take_first,
    // src, dst) moves both. balance_of(a or b) returns 100.
    let r = build_and_run("c23_go_no_go.sentinel");
    assert_eq!(r.stdout, "100\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.4 / ADR 0017 D8: RAII / drop + sentinel_free ----

#[test]
fn pass_c24_array_dropped() {
    // Array allocated in main + dropped at fn-return.
    // 7+8+9 = 24.
    assert_eq!(run_exit("c24_array_dropped.sentinel"), 24);
}

#[test]
fn pass_c24_moved_array_no_double_free() {
    // arr moved into consume(arr); the borrow checker records
    // arr in moved_sources, so codegen skips arr's drop at
    // main's scope exit. consume drops xs (its param) instead.
    // No double-free. 11+22+33 = 66.
    assert_eq!(run_exit("c24_moved_array_no_double_free.sentinel"), 66);
}

#[test]
fn pass_c24_nested_blocks_drop() {
    // tmp declared in an inner block; dropped at inner-block
    // exit (not at fn return). 6 + 4 = 10.
    assert_eq!(run_exit("c24_nested_blocks_drop.sentinel"), 10);
}

#[test]
fn pass_c24_go_no_go() {
    // ADR 0017 D8 phase-go: array drop at fn return + array
    // drop at inner-block exit + move-into-consume skipping
    // drop. inner_sum=10, fn_sum=150, total=160.
    let r = build_and_run("c24_go_no_go.sentinel");
    assert_eq!(r.stdout, "160\n");
    assert_eq!(r.exit, 0);
}

// ---- C2.5(a) / ADR 0017 D8 closure: recursive field drops ----

#[test]
fn pass_c25_struct_field_drop() {
    // A struct with an array field. Drop recursion frees the
    // inner array at scope exit. 3+4+5+7 = 19.
    let r = build_and_run("c25_struct_field_drop.sentinel");
    assert_eq!(r.stdout, "19\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c25_nested_struct_drop() {
    // Outer { inner: Inner { items: [i64] } } — drop recurses
    // two levels deep + frees the array. 100+200+300+7+0 = 607.
    let r = build_and_run("c25_nested_struct_drop.sentinel");
    assert_eq!(r.stdout, "607\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c25_generic_struct_array_drop() {
    // Generic-instance case: Holder<[i64]> field-substitutes
    // T = [i64] when descending into fields. 11+22+33 = 66.
    let r = build_and_run("c25_generic_struct_array_drop.sentinel");
    assert_eq!(r.stdout, "66\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c25_go_no_go() {
    // ADR 0017 D14 phase-go: full C2 surface — shared + mut
    // borrows (D1+D3+D6), move (D9), RAII drop (D8) over a
    // struct with a heap-backed array field (C2.5(a) closure).
    // sum_via_shared=45 + (consume=45) + (add_into 100->145)
    // = 190.
    let r = build_and_run("c25_go_no_go.sentinel");
    assert_eq!(r.stdout, "190\n");
    assert_eq!(r.exit, 0);
}

// ---- C3.1 / ADR 0019 D5+D6: secret typing + declassify ----

#[test]
fn pass_c31_secret_typing() {
    // `let stored: secret i64 = 42;` implicit-widens 42 to
    // secret i64; declassify strips it back to i64 inside the
    // `unwrap` fn. 42 + 8 = 50.
    let r = build_and_run("c31_secret_typing.sentinel");
    assert_eq!(r.stdout, "50\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c31_go_no_go() {
    // ADR 0019 D5+D6+D7 phase-go: full secret-typing surface —
    // implicit widening + secret arithmetic + secret comparison
    // + declassify before branching. stored=42, guess=42, sum=84,
    // matched=true, declassify(sum)+16 = 100.
    let r = build_and_run("c31_go_no_go.sentinel");
    assert_eq!(r.stdout, "100\n");
    assert_eq!(r.exit, 0);
}

// ---- C3.2 / ADR 0019 D1+D2+D4+D13: effect rows + effect_check ----

#[test]
fn pass_c32_go_no_go() {
    // ADR 0019 D1+D2+D4+D13 phase-go: effect declarations +
    // annotated fn chain + main has no effects. effect Io declared;
    // check_inner ! { Io } calls check_outer ! { Io } (annotation
    // matches inferred row); main only calls pure_compute (no
    // effects) so D13's "main must be effect-free" is satisfied.
    // pure_compute(12) = 12 + 30 = 42.
    let r = build_and_run("c32_go_no_go.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

// ---- C3.3 / ADR 0019: Phase C3 close-out phase-go ----

#[test]
fn pass_c33_go_no_go() {
    // The Phase C3 close-out fixture: combines effect_decl + an
    // annotated fn that isn't called from main (so D13 is OK)
    // with secret typing + secret arithmetic + secret comparison
    // + declassify before branching. Exercises D1+D4+D5+D6+D13.
    // stored = guess = 42 (secret); doubled = 84 (secret); matched
    // = true (secret); declassify(doubled) - 42 = 42.
    let r = build_and_run("c33_go_no_go.sentinel");
    assert_eq!(r.stdout, "42\n");
    assert_eq!(r.exit, 0);
}

// ---- C3.4 / ADR 0020 D9: handler typing layer ----
//
// The handler surface parses, resolves, type-checks, and effect-
// checks (Io is discharged), but codegen lands at C3.5/C3.6. We
// can't yet run the c34 fixture end-to-end; instead the driver
// test below asserts that `snc build` surfaces a clean
// `handlers_not_yet_supported` codegen diagnostic instead of a
// panic. End-to-end pass-test arrives at C3.7.

fn build_ui_fixture(fixture: &str) -> std::process::Output {
    let src = workspace_root().join("tests/ui").join(fixture);
    let out_dir = build_dir();
    std::fs::create_dir_all(&out_dir).expect("create build dir");
    let exe = out_dir.join(PathBuf::from(fixture).with_extension(""));
    Command::new(snc_binary())
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("snc invocation failed")
}

// (Previously this asserted handle_body_not_direct_perform for the
// `handle do_work() with { ... }` fixture. At C3.5(b) that shape
// compiles + runs; the fixture has been promoted to tests/pass/
// as `c35b_handle_fn_call_body.sentinel`. The driver test below
// asserts the new runtime behavior instead.)

#[test]
fn c34_perform_undefined_effect_rejects_at_resolve() {
    let out = build_ui_fixture("c34_perform_undefined_effect.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("undefined_handler_effect"),
        "expected undefined_handler_effect; got: {stderr}"
    );
}

#[test]
fn c34_handle_undefined_op_rejects_at_resolve() {
    let out = build_ui_fixture("c34_handle_undefined_op.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("undefined_handler_op"),
        "expected undefined_handler_op; got: {stderr}"
    );
}

#[test]
fn c34_handle_duplicate_arm_rejects_at_resolve() {
    let out = build_ui_fixture("c34_handle_duplicate_arm.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("duplicate_handler_arm"),
        "expected duplicate_handler_arm; got: {stderr}"
    );
}

#[test]
fn c34_kont_used_as_value_rejects_at_types() {
    let out = build_ui_fixture("c34_kont_used_as_value.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("kont_used_as_value"),
        "expected kont_used_as_value; got: {stderr}"
    );
}

// ---- C3.5(a) / ADR 0020 D7: handler runtime end-to-end ----
//
// The minimum-viable handler runtime is in. Programs whose
// `handle` body is a direct `perform Op(args)` now compile, link
// against the runtime symbols, and produce the resumed value.
// General-case handle bodies (fn-call-that-performs, nested
// perform, perform inside binop) still need frame reification
// at every evaluation site — that's C3.5(b) / C3.6.

#[test]
fn pass_c35_handle_inline_perform() {
    // body = perform Io.read() (0-arg op); arm body = k(42).
    // No stdout; exit code = 42.
    let r = build_and_run("c35_handle_inline_perform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35_handle_log_returns_msg() {
    // body = perform Io.log(7) (1-arg op, msg bound to 7).
    // arm body = k(msg + 35) = 42. Exit code = 42.
    let r = build_and_run("c35_handle_log_returns_msg.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ---- C3.5(b) / ADR 0020 D7: effecting fn ABI ----
//
// Effecting fns (signature.effect_row non-empty) now return Kont*
// at the LLVM ABI level. handle bodies can be a direct Perform OR
// a Call to an effecting fn. A runtime switch on the kont's op_id
// dispatches the matching arm.

#[test]
fn pass_c35b_handle_fn_call_body() {
    // do_work() returns a Kont* via the effecting ABI; main's
    // handle catches it. Exit code = 42 (the arm body's k(42)).
    let r = build_and_run("c35b_handle_fn_call_body.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35b_handle_multi_arm() {
    // Multi-arm dispatch: main's handle covers both Io.read and
    // Io.write. do_work raises Io.write(7); the Io.write arm is
    // selected at runtime; k(msg + 35) = 42.
    let r = build_and_run("c35b_handle_multi_arm.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35b_handle_pure_return() {
    // Pure-return path: do_work is declared effecting but body
    // is the pure expression `41 + 1`. sentinel_kont_pure wraps
    // the value; handle's PURE_RETURN_OP_ID switch case unwraps.
    let r = build_and_run("c35b_handle_pure_return.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// (Previously this asserted effecting_fn_body_not_direct for the
// let-bound-perform fixture. At C3.5(c) that shape compiles + runs
// via per-let resumer fns; the fixture has been promoted to
// tests/pass/c35c_let_bound_perform.sentinel. The UI fixture itself
// is removed since the shape it asserts is no longer rejected.)

// ---- C3.5(c) / ADR 0020 D7: per-let frame reification ----
//
// An effecting fn whose body is a single let with effecting RHS +
// pure tail uses codegen-emitted per-let resumer fns. The resumer
// receives (resumed value, captured-state pointer) and computes
// the let-body. sentinel_kont_push at the let-site adds the
// resumer onto the kont's frame chain; sentinel_kont_resume
// replays the chain on resume.

#[test]
fn pass_c35c_let_bound_perform() {
    // do_work body: `let v: i64 = perform Io.read(); v + 1`.
    // Resumer with no captures. main resumes with 41 → v = 41 →
    // v + 1 = 42. Exit 42.
    let r = build_and_run("c35c_let_bound_perform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35c_let_bound_perform_with_capture() {
    // do_work(offset) body captures `offset` into the runtime
    // frame's captured-state struct. main calls do_work(35);
    // resumes with 7; resumer computes 7 + 35 = 42.
    let r = build_and_run("c35c_let_bound_perform_with_capture.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ---- C3.5(d) / ADR 0020 D7: unified embedded-perform shape ----
//
// Effecting fns whose tail has a single embedded perform (binop,
// pure fn-call arg, struct-lit field, index, etc.) now compile
// via the same per-site reification machinery as C3.5(c). The
// detect / substitute walker handles any pure surrounding
// context; the resumer's substituted tail re-evaluates with the
// resumed value bound to the placeholder VarId.

#[test]
fn pass_c35d_binop_with_perform() {
    // tail = `perform Io.read() + 1`; resume(41) → 41 + 1 = 42.
    let r = build_and_run("c35d_binop_with_perform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35d_perform_with_capture_and_binop() {
    // tail = `perform Io.read() * 2 + extra`; do_work(2),
    // resume(20) → 20 * 2 + 2 = 42.
    let r = build_and_run("c35d_perform_with_capture_and_binop.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35d_perform_in_call_arg() {
    // tail = `double(perform Io.read())`; resume(21) →
    // double(21) = 42.
    let r = build_and_run("c35d_perform_in_call_arg.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ============================================================================
// C3.5(e) / ADR 0020 D7: chained effecting lets via resumers-can-perform.
// Two or more `let v: i64 = perform Op` statements in sequence + a pure
// tail. Resumer-i's body itself emits a perform (the next let's RHS),
// returning a bubble kont that sentinel_kont_resume detects + propagates;
// the handle's dispatch loop re-dispatches the bubble per ADR 0020 D3
// deep-handler semantics. compile_effecting_fn_with_chained_lets emits N
// resumer fns iteratively (one per let); the last resumer wraps the pure
// tail via sentinel_kont_pure.
// ============================================================================

#[test]
fn pass_c35e_chained_perform() {
    // do_two: `let a = perform Io.read(); let b = perform Io.read();
    // a + b`. Both resumes return 21 → a + b = 42.
    let r = build_and_run("c35e_chained_perform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35e_chained_perform_with_capture() {
    // do_work(base): two chained performs + tail `a + b + base`.
    // base = 20, each resume returns 11 → 11 + 11 + 20 = 42.
    let r = build_and_run("c35e_chained_perform_with_capture.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c35e_chained_dependent_perform() {
    // do_dependent: second perform's arg references first let.
    // a = 10 (twice 5), b = 20 (twice a), a + b + 12 = 42.
    // Exercises that resumer-0 correctly captures `a` for use in
    // resumer-1's perform arg.
    let r = build_and_run("c35e_chained_dependent_perform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ============================================================================
// C3.6(a) / ADR 0020 D4: return arm with non-identity transform.
// lower_handle binds the return arm's value VarId to the unwrapped i64 +
// lowers the body in pure_block; lower_resume_kont mirrors this on the
// k(v) pure-unwrap path per Phase B's `k := \v. handle (kont.resume v)
// with H` deep-handler re-wrap semantics. HandleContext now carries the
// optional return arm (cloned at lower_handle entry) so k(v)'s pure path
// can apply it.
// ============================================================================

#[test]
fn pass_c36a_return_arm_transform() {
    // do_pure() body is the pure expression `21`; effecting ABI
    // wraps via sentinel_kont_pure. Handle's PURE_RETURN switch
    // dispatches the return arm `return v => v * 2` → 21 * 2 = 42.
    let r = build_and_run("c36a_return_arm_transform.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c36a_return_arm_after_resume() {
    // handle perform Io.read() with arm k(21) + return v => v * 2.
    // The arm body's k(21) drains pure (no frames); pure-unwrap
    // path applies the return arm per deep-handler re-wrap →
    // 21 * 2 = 42.
    let r = build_and_run("c36a_return_arm_after_resume.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ============================================================================
// C3.6(b) / ADR 0020 D3: nested handles. When an inner handle's body emits
// an op the inner doesn't catch, the inner's switch default propagates the
// un-caught kont to the merge so the outer handle's dispatch loop catches
// it. Detected via CodegenCtx.handle_depth > 1 — nested handles emit
// Kont*-typed merge values; top-level handles emit i64 as before.
// ============================================================================

#[test]
fn pass_c36b_nested_handle_basic() {
    // do_both performs Io.read + Net.fetch in chained-let
    // shape. Inner catches Io (resume 7), bubbles Net to outer
    // which catches Net (resume 35). 7 + 35 = 42.
    let r = build_and_run("c36b_nested_handle_basic.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c36b_nested_handle_inner_full() {
    // do_io performs only Io. Inner catches it fully (k(15));
    // inner's pure-path wraps via pure_kont; outer's PURE_RETURN
    // switch case dispatches + consume_pure → 15. + 27 = 42.
    let r = build_and_run("c36b_nested_handle_inner_full.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ============================================================================
// C3.7 / ADR 0020 D12: Phase C3 handler-runtime phase-go programs. Final
// fixtures for ADR 0020's PROPOSED → ACCEPTED flip + Phase C3 close-out.
// ============================================================================

#[test]
fn pass_c37_go_no_go() {
    // The D12 phase-go: do_work(42) performs Io.log; handler
    // arm resumes with msg + 1 = 43; do_work computes x +
    // logged = 42 + 43 = 85; print(85) emits "85\n" + returns 0.
    let r = build_and_run("c37_go_no_go.sentinel");
    assert_eq!(r.stdout, "85\n");
    assert_eq!(r.exit, 0);
}

#[test]
fn pass_c37_handle_return() {
    // `handle 42 with { return v => v * 2 }`. The pure body
    // is wrapped via sentinel_kont_pure at the handle site;
    // PURE_RETURN switch case unwraps + applies the return
    // arm. 42 * 2 = 84.
    let r = build_and_run("c37_handle_return.sentinel");
    assert_eq!(r.exit, 84);
    assert_eq!(r.stdout, "");
}

#[test]
fn c37_perform_outside_handle_rejects_at_effect_check() {
    // ADR 0019 D13: `main` must be effect-free. A bare
    // `perform Io.log(...)` in main with no wrapping handle
    // surfaces `sentinel::effect::unhandled_effect`.
    let out = build_ui_fixture("c37_perform_outside_handle.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unhandled_effect"),
        "expected unhandled_effect; got: {stderr}"
    );
}

// ============================================================================
// C4.1 (2/N) / ADR 0022: class declarations + method calls + Name::init(args)
// end-to-end. AST + parser landed at C4.1 (1/N); resolve / types / codegen
// wiring + the postfix method-call + ClassInit-call forms land here.
// ============================================================================

#[test]
fn pass_c41_class_basic() {
    // Smallest end-to-end class: single field + init + one
    // shared-self method. `Point::init(7).get()` ⇒ exit 7.
    let r = build_and_run("c41_class_basic.sentinel");
    assert_eq!(r.exit, 7);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c41_go_no_go() {
    // ADR 0022 D11 phase-go: Point with manhattan + translate.
    // p starts at (10, 20). translate(3, 9) updates to (13, 29)
    // and returns self.manhattan() = 42.
    let r = build_and_run("c41_go_no_go.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn c41_init_field_unassigned_rejects_at_type_check() {
    // ADR 0022 D4: every declared field must be definite-assigned
    // inside `init`. A class with two fields but only one
    // `self.field = ...` surfaces the
    // `sentinel::types::init_field_maybe_unassigned` diagnostic.
    let out = build_ui_fixture("c41_init_field_unassigned.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("init_field_maybe_unassigned"),
        "expected init_field_maybe_unassigned; got: {stderr}"
    );
}

// ============================================================================
// C4.2 (2/N) per ADR 0023: trait + impl wired through resolve /
// types / codegen. Receiver-typed (Path 1) + qualified-named
// (Path 2) dispatch end-to-end. Path 3 bounded-generic dispatch
// is deferred (see ADR 0023 D5 amendment).
// ============================================================================

#[test]
fn pass_c42_trait_basic() {
    // Smallest trait + default impl + receiver-typed dispatch:
    // `Tally::init().tick(42)` ⇒ exit 42.
    let r = build_and_run("c42_trait_basic.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c42_go_no_go() {
    // ADR 0023 D12 phase-go: FileSink with default + named
    // `Doubling` Writer impls. main calls both via `s.write(10)`
    // (receiver-typed) and `Doubling::write(&mut s, 16)`
    // (qualified-named) to return 42.
    let r = build_and_run("c42_go_no_go.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn c42_impl_missing_method_rejects_at_type_check() {
    // ADR 0023 D8 completeness check: every trait method must be
    // supplied. Default method bodies are deferred per ADR 0023
    // D10.
    let out = build_ui_fixture("c42_impl_missing_method.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("impl_missing_method"),
        "expected impl_missing_method; got: {stderr}"
    );
}

#[test]
fn c42_impl_method_sig_mismatch_rejects_at_type_check() {
    // ADR 0023 D8 per-method signature match. The impl method's
    // param type differs from the trait's; type-check surfaces
    // the dedicated diagnostic.
    let out = build_ui_fixture("c42_impl_method_sig_mismatch.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("impl_method_signature_mismatch"),
        "expected impl_method_signature_mismatch; got: {stderr}"
    );
}

#[test]
fn c42_duplicate_default_impl_rejects_at_resolve() {
    // ADR 0023 D8 coherence: at most one default impl per
    // (trait, type) per scope.
    let out = build_ui_fixture("c42_duplicate_default_impl.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("duplicate_default_impl"),
        "expected duplicate_default_impl; got: {stderr}"
    );
}

#[test]
fn c42_duplicate_impl_name_rejects_at_resolve() {
    // ADR 0023 D8 coherence: named impls must have unique names
    // within their scope.
    let out = build_ui_fixture("c42_duplicate_impl_name.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("duplicate_impl_name"),
        "expected duplicate_impl_name; got: {stderr}"
    );
}

// ============================================================================
// C4.3 per ADR 0021 D6: delegation. `delegate field: T to Trait;`
// inside a class body synthesizes a default impl of Trait for the
// class that auto-forwards every trait method to
// `self.field.method(args)`. Resolved at name-resolution time; the
// synthesized impl flows through types + codegen unchanged.
// ============================================================================

#[test]
fn pass_c43_go_no_go() {
    // ADR 0021 D6 phase-go: Logger delegates Writer to FileSink.
    // `l.write(42)` routes via the synthesized default impl →
    // `self.writer.write(42)` → FileSink's Writer impl writes 42
    // and returns count = 42.
    let r = build_and_run("c43_go_no_go.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn c43_delegate_collides_with_impl_rejects_at_resolve() {
    // Coherence: both a delegate AND an explicit
    // `impl as Trait for Class` synthesize default impls of the
    // same (Trait, Class). DuplicateDefaultImpl fires.
    let out = build_ui_fixture("c43_delegate_collides_with_impl.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("duplicate_default_impl"),
        "expected duplicate_default_impl; got: {stderr}"
    );
}

#[test]
fn c43_delegate_undefined_trait_rejects_at_resolve() {
    // `delegate field: T to UnknownTrait;` — the trait name
    // resolves through the impl-trait lookup path; the same
    // UndefinedTraitForImpl variant fires.
    let out = build_ui_fixture("c43_delegate_undefined_trait.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("undefined_trait_for_impl"),
        "expected undefined_trait_for_impl; got: {stderr}"
    );
}

// ----- C4.4 / ADR 0024: structured concurrency -----

#[test]
fn pass_c44_go_no_go() {
    // ADR 0024 D12 phase-go: main opens `scope concurrent`, spawns
    // `double(21)` on a worker thread, awaits the result (42). The
    // scope discharges the Async effect so main stays effect-free.
    let r = build_and_run("c44_go_no_go.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn c44_spawn_non_fn_call_rejects() {
    // ADR 0024 D2: spawn target must be a direct call.
    let out = build_ui_fixture("c44_spawn_non_fn_call.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("spawn_must_be_call"),
        "expected spawn_must_be_call; got: {stderr}"
    );
}

#[test]
fn c44_await_on_non_task_rejects() {
    // ADR 0024 D3: `.await` receiver must be a Task<T>.
    let out = build_ui_fixture("c44_await_on_non_task.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("await_on_non_task"),
        "expected await_on_non_task; got: {stderr}"
    );
}

#[test]
fn c44_spawn_result_must_be_i64_rejects() {
    // ADR 0024 D7: Task<T> restricted to Task<i64> at C4.4 minimum.
    let out = build_ui_fixture("c44_spawn_result_must_be_i64.sentinel");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("spawn_result_must_be_i64"),
        "expected spawn_result_must_be_i64; got: {stderr}"
    );
}
