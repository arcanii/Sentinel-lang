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

// ---- ADR 0046: partial-move-through-field soundness ----

#[test]
fn pass_c25_partial_move_field() {
    // ADR 0046: `consume_arr(p.items)` partial-moves only the `items`
    // field; `main`'s drop of `p` SKIPS it (the consumer freed it), so
    // no double-free. Reading the un-moved `tag` field is fine.
    // items[0]+items[1]=30, +tag(7) = 37. (Pre-0046: accepted-but-UB.)
    assert_eq!(run_exit("c25_partial_move_field.sentinel"), 37);
}

#[test]
fn pass_c25_field_read_no_move() {
    // ADR 0046 regression: a NON-consuming field read (`p.a[0]`) does
    // NOT record a partial move — both `a` and `b` are still dropped at
    // scope exit (the C2.3 `p.a + p.b` shape is preserved). 1+3 = 4.
    assert_eq!(run_exit("c25_field_read_no_move.sentinel"), 4);
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

// (The C3.4 handler-surface rejections — undefined effect/op,
// duplicate arm, kont-used-as-value — are now UI snapshot tests in
// crates/sentinel-driver/tests/ui.rs per ADR 0025 D11.)

// (Previously this asserted handle_body_not_direct_perform for the
// `handle do_work() with { ... }` fixture. At C3.5(b) that shape
// compiles + runs; the fixture has been promoted to tests/pass/
// as `c35b_handle_fn_call_body.sentinel`. The driver test below
// asserts the new runtime behavior instead.)

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
fn pass_c66_channel() {
    // ADR 0066 M1.2: a Channel<i64> — channel_new / send / channel_close / recv
    // (the ?i64 some/null) consumed via unwrap_or. In every differential corpus.
    let r = build_and_run("c66_channel.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c66_task_bool() {
    // ADR 0066 M1.1: a generic `Task<bool>` spawn over a `bool` arg — the
    // wrapper loads `i1`, encodes the result (`zext i1`), `.await` decodes
    // (`trunc to i1`). flip(false) → true → 42. In every differential corpus.
    let r = build_and_run("c66_task_bool.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c66_channel_worker() {
    // ADR 0066 M1.2b: a `Channel<i64>` parameter (the resolve_type_expr arm) +
    // a cross-thread `spawn produce(ch)` with a Channel ARG; `main` drains as the
    // consumer until the worker closes the channel. 10 + 32 = 42. Differential.
    let r = build_and_run("c66_channel_worker.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c68_nested_array() {
    // ADR 0068: nested arrays `[[u8]]` — a `[[u8]]` literal + param + outer `len`
    // + inner indexing `rows[i]` (a `[u8]`) + its `len`. 3+2+1 + 36 = 42.
    let r = build_and_run("c68_nested_array.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c66_nullable_u8() {
    // ADR 0066 M1.2b: `?T` generalized over scalars — `?u8` (null, an implicit
    // u8→?u8 widen, is_some, unwrap_or, a ?u8 param). 30 + 12 = 42.
    let r = build_and_run("c66_nullable_u8.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c66_channel_pipeline() {
    // ADR 0066 M1.2b/M1.3: a TWO-channel-arg spawn `relay(src, dst)` — the
    // corpus's first 2-arg spawn, pinning the multi-arg packed-args lowering
    // (GEP offsets 0 + 8, collect-then-store). relay forwards src→dst. 20+22=42.
    let r = build_and_run("c66_channel_pipeline.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

// ----- C4.5 / ADR 0021 D13: full-surface phase-go close-out -----

#[test]
fn pass_c4_go_no_go() {
    // ADR 0021 D13 phase-go: class + methods + init + trait + impl
    // + delegation + scope/spawn/await composed in one program.
    // `spawn buffered_write(42)` runs the class/delegation surface
    // on a worker thread; the delegate forwards lb.write(42) to the
    // inner Buffer's Writer impl → 42; + lb.count() (0) = 42.
    let r = build_and_run("c4_go_no_go.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c4_named_impl() {
    // ADR 0021 D13 secondary smoke: two named impls of (Writer,
    // Counter) co-exist, dispatched via qualified calls.
    // Plus::write(9) → 9; Times::write(11) → 9 + 33 = 42.
    let r = build_and_run("c4_named_impl.sentinel");
    assert_eq!(r.exit, 42);
    assert_eq!(r.stdout, "");
}

#[test]
fn pass_c52_secret_ct() {
    // ADR 0026 D9 c52_secret_ct: a branch-free constant-time masked
    // select over secrets (`c*a + (1-c)*b`) compiles, runs, and PASSES
    // the D5 verification (no secret reaches a branch / index / divisor).
    // cond=1 picks x=42; declassify yields the exit code.
    assert_eq!(run_exit("c52_secret_ct.sentinel"), 42);
}

#[test]
fn pass_c52_secret_through_constructs_ok() {
    // Secret-flow conformance (review F3 / P2.2), accept side: a secret flows
    // through a field, a call, and a `match`, combined with arithmetic only
    // (no branch / index / divisor), then declassified. The conservative
    // funnels preserve taint (so the c52_secret_via_* leaks are caught) yet a
    // genuinely constant-time use still compiles. 40 + 40 = 80.
    assert_eq!(run_exit("c52_secret_through_constructs_ok.sentinel"), 80);
}

#[test]
fn pass_c53_bitwise() {
    // ADR 0027 D10 c53_bitwise: `5 & 6 ^ 3 | 8` == 15, pinning the
    // `&` > `^` > `|` precedence (a misparse would give 5).
    assert_eq!(run_exit("c53_bitwise.sentinel"), 15);
}

#[test]
fn pass_c53_ct_eq() {
    // ADR 0027 D10 c53_ct_eq: constant-time equality over secrets via
    // XOR-accumulate + OR-reduce + declassify — compiles, runs, and
    // PASSES the D5 verification. Comparing (42,7) to itself → 0; 42-0=42.
    assert_eq!(run_exit("c53_ct_eq.sentinel"), 42);
}

#[test]
fn pass_c54_cast() {
    // ADR 0049 c54_cast: the integer cast `x as T` (trunc / sign-extend + a
    // secret-i32 cast). Also exercised byte-for-byte by the selfhost
    // differentials (corpus auto-scan), proving the self-hosted `scg` mirrors
    // the cast identically to `snc`. Exit 42.
    assert_eq!(run_exit("c54_cast.sentinel"), 42);
}

#[test]
fn pass_c53_shift() {
    // ADR 0048 c53_shift: shift operators `<<` (shl) / `>>` (logical lshr) over
    // matching-width i64. Also exercised byte-for-byte by the selfhost
    // differentials (corpus auto-scan), proving the self-hosted `scg` mirrors
    // shifts identically to the `snc llvm` oracle. (1<<4)+(255>>2)-(8<<1)-21 = 42.
    assert_eq!(run_exit("c53_shift.sentinel"), 42);
}

#[test]
fn pass_c53_secret_array_memcmp() {
    // ADR 0047 c53_secret_array_memcmp: a constant-time memcmp over
    // `[secret u8]` (arrays of secret ELEMENTS). A public-index loop over the
    // full length XOR/OR-reduces the secret bytes; only the final accumulator
    // is declassified — PASSES the constant-time check. Equal buffers → 0,
    // one tampered byte → nonzero; exit 42. Also exercised byte-for-byte by the
    // selfhost codegen differential (corpus auto-scan), proving scg matches the
    // `snc llvm` oracle on the new construct with no selfhost change.
    assert_eq!(run_exit("c53_secret_array_memcmp.sentinel"), 42);
}

#[test]
fn pass_c54_scope_arena() {
    // ADR 0028 D9 c54_scope_arena: the scope→arena codegen. A
    // non-escaping primitive array's heap buffer is bump-allocated in
    // the broker arena (verified: main emits sentinel_arena_enter/_alloc
    // and ZERO sentinel_free/_alloc) and bulk-freed by one
    // sentinel_arena_exit at scope exit. Body-scope arena (`xs`) +
    // nested-block arena (`ys`); 6 + 30 + 6 = 42, behaviour identical
    // to the libc path.
    assert_eq!(run_exit("c54_scope_arena.sentinel"), 42);
}

#[test]
fn pass_c5_go_no_go() {
    // ADR 0030 D8 (2/N): the 1.0 close-bar program — a
    // TLS-1.3-handshake-shaped program whose crypto is real constant-time
    // computation over `secret` scalars (a Montgomery-ladder step + cswap,
    // an HKDF-expand-shaped mix via the cipher-suite trait, the c53_ct_eq
    // `Finished` verify). It builds ONLY if it passes the D5 constant-time
    // check (no secret reaches a branch / index / divisor) — the decisive
    // 1.0 validation — and runs to exit 42 (successful handshake →
    // Finished accumulator 0 → 42 - 0). Exercises class + trait dispatch +
    // I/O-as-effects + constant-time `secret` end-to-end.
    assert_eq!(run_exit("c5_go_no_go.sentinel"), 42);
}

#[test]
fn pass_c5d1_enum() {
    // ADR 0032 D11 (4/N) phase-go: sum types (`enum`) + pattern matching
    // (`match`) end to end. A non-generic `Shape` enum with a unit variant
    // + tuple-payload variants (arity 1 + 2), constructed (bare unit +
    // positional payloads) and `match`ed (every arm, with payload
    // bindings). Exercises the `{ i32 tag, ptr payload }` layout, the
    // `switch` lowering, payload-binding loads, and scope-exit drop of the
    // heap-boxed payloads. area(Unit)=0 + area(Circle(2))=12 +
    // area(Rect(5,6))=30 = exit 42.
    assert_eq!(run_exit("c5d1_enum.sentinel"), 42);
}

#[test]
fn pass_c5d2_strings() {
    // ADR 0033 D9 (4/N) phase-go: strings + a `u8` byte type end to end.
    // Char-literal `u8` comparison (`is_digit`), explicit `u8`↔`i64`
    // conversion (`digit_val`), string literals as `[u8]` (heap-copied,
    // owned, droppable), byte indexing (`s[i] -> u8`), and the `str_eq`
    // byte-array matcher. Parses the 2-digit source "42" into the
    // integer 42, gated on every byte op being correct. Every `[u8]` is
    // a bound variable → leak-free (verified separately via
    // `leaks --atExit`). digit_val('4')*10 + digit_val('2') = 42.
    assert_eq!(run_exit("c5d2_strings.sentinel"), 42);
}

#[test]
fn pass_c5d2_u8_unsigned() {
    // ADR 0033 D6: `u8` is unsigned — pins the two unsigned codegen
    // paths the ASCII phase-go doesn't reach. A byte ≥ 0x80 compares as
    // large (200 > 100, not signed -56 > 100) and `/` is `udiv`
    // (200 / 100 == 2, not signed 0). Exit 42 iff both are correct.
    assert_eq!(run_exit("c5d2_u8_unsigned.sentinel"), 42);
}

#[test]
fn pass_c5d3_collections() {
    // ADR 0034 D9/D10 phase-go: the growable `Vec<T>` MVP end to end.
    // (1/N) vec_new / push / len + realloc growth + drop + a Vec moved
    // out of a helper (escape); (2/N) `v[i]` (on Vec<i64> and Vec<u8>),
    // `pop`, the `Vec<u8>` -> `[u8]` bridge (vec_to_array + str_eq), and
    // `String` = `Vec<u8>`. Every Vec/array buffer is freed at scope exit
    // — leak-free under `leaks --atExit` (verified separately).
    // n0 + last + remaining + matched + first_is_l = 10+40+3+1+1 = 55.
    assert_eq!(run_exit("c5d3_collections.sentinel"), 55);
}

#[test]
fn pass_c5d4_file_io() {
    // ADR 0035 D9/D10 phase-go: the file-I/O MVP end to end. `write_file`
    // a payload to a temp path, `read_file` it back, verify the bytes
    // survived (str_eq + a `back[i]` spot-check + len), and `print_bytes`
    // the read-back content to stdout. File I/O is runtime builtins
    // (ADR 0035 D2), panic-on-failure (D5). Leak-free under `leaks
    // --atExit` (verified separately). Exit = len("hello") = 5; stdout =
    // "hello" (print_bytes writes exactly the bytes, no newline). The
    // temp path is removed before + after so the test is hermetic.
    let tmp = std::path::Path::new("/tmp/sentinel_c5d4_io.txt");
    let _ = std::fs::remove_file(tmp);
    let r = build_and_run("c5d4_file_io.sentinel");
    assert_eq!(r.exit, 5);
    assert_eq!(r.stdout, "hello"); // print_bytes(back), no trailing newline
    // The fixture must have created the file (write_file ran).
    assert!(tmp.exists(), "c5d4 fixture should have written the temp file");
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn pass_c5d5_loops() {
    // ADR 0036 D9/D10 (1/N) phase-go: the `while` loop end to end. A
    // counter loop (sum 1..=10), a Vec built by pushing in a loop +
    // len/index, and a body that allocates a [u8] each iteration (100x).
    // `while` is a statement lowering to a backward CFG branch; the body
    // drops per-iteration (ADR 0036 D5) — leak-free under `leaks --atExit`
    // (verified separately) — and body allocas hoist to the entry block so
    // the stack does not grow per pass. Exit = 55 + 5 + 4 + 3 = 67.
    assert_eq!(run_exit("c5d5_loops.sentinel"), 67);
}

#[test]
fn pass_c5d5_break_continue() {
    // ADR 0036 D9/D10 (2/N) phase-go: `break` + `continue` end to end.
    // `break` exits the innermost loop (sum 1..=5 then break -> 15);
    // `continue` skips to the next iteration (sum even j in 1..=10 -> 30);
    // and — the load-bearing property — a `break`/`continue` that fires
    // while a per-iteration `[u8]` is live drops it BEFORE branching, so
    // the body's heap is freed each pass (hits 30 + cont_hits 40). Codegen
    // drains every scope frame down to the loop body before the branch;
    // leak-free under `leaks --atExit` (verified separately). Exit =
    // 15 + 30 + 30 + 40 = 115.
    assert_eq!(run_exit("c5d5_break_continue.sentinel"), 115);
}

#[test]
fn pass_selfhost_ast_drop() {
    // ADR 0039 D4: the self-host parser's recursive-AST drop gate. A
    // recursive-enum `Node` (i64 + `[u8]` + recursive payloads) built, walked
    // by a CONSUMING recursive `match` (the parser's dump shape), and dropped
    // — leak-free under `leaks --atExit` (verified separately), confirming
    // ADR 0032 A1's box-free recursive-enum drop holds at AST scale (so the
    // parser needs no D.1b payload-ownership fix first). dump of
    // Pair(Name "ab", Pair(Lit 1, Name "cd")) = "(Nab(LNcd))" -> exit 11.
    let r = build_and_run("selfhost_ast_drop.sentinel");
    assert_eq!(r.exit, 11);
    assert_eq!(r.stdout, "(Nab(LNcd))");
}

#[test]
fn pass_c65_return() {
    // ADR 0065 c65_return: explicit early `return`. Exercises a guard return
    // (`checked_div`), a heap binding live across an early return (`first_or_default`
    // — the load-bearing drop-on-both-paths case), a `;`-terminated statement-position
    // return (`clamp_low`), and an early `return 42` on success. Also exercised
    // byte-for-byte by the selfhost differentials (corpus auto-scan), proving the
    // self-hosted `scg` mirrors `return`'s control-flow lowering identically to the
    // `snc llvm` oracle. Exit 42 iff every sub-result is correct.
    assert_eq!(run_exit("c65_return.sentinel"), 42);
}

#[test]
fn pass_c65_return_match() {
    // ADR 0065 c65_return_match: `return` inside a `match` arm — a distinct codegen path
    // from `return`-inside-an-`if`. All arms are `i64` (the `Neg` arm early-returns -1),
    // so it enters the selfhost differential too (snc llvm == scg byte-for-byte over the
    // match + return lowering). Exit 42.
    assert_eq!(run_exit("c65_return_match.sentinel"), 42);
}

#[test]
fn pass_c60_vec_struct_move() {
    // ADR 0034 D8: a Move-typed struct element (a struct owning a `[u8]`) pushed
    // into a `Vec` through a BY-VALUE function parameter is consumed/moved, so it
    // is not also freed at the caller's scope exit. Regression for a use-after-free
    // (the element was double-freed; the later read returned garbage). The `Vec`
    // is the sole owner. (1+10+20) + (2+30+40) - 61 = 42.
    assert_eq!(run_exit("c60_vec_struct_move.sentinel"), 42);
}
