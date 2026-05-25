//! End-to-end pipeline integration tests for Sentinel-Mini.
//!
//! These use only the public `run()` API and serve as the canonical
//! examples of what the language can do at each milestone.

use sentinel_effects_proto::{run, Value};

#[test]
fn pipeline_arithmetic() {
    assert_eq!(run("1 + 2 * 3 - 4").unwrap(), Value::Int(3));
}

#[test]
fn pipeline_nested_let() {
    let src = "let x = 1 in let y = 2 in x + y";
    assert_eq!(run(src).unwrap(), Value::Int(3));
}

#[test]
fn pipeline_higher_order_function() {
    let src = "let twice = fn(f) => fn(x) => f(f(x)) in twice(fn(n) => n + 1)(10)";
    assert_eq!(run(src).unwrap(), Value::Int(12));
}

#[test]
fn pipeline_line_comments_are_ignored() {
    let src = "// preamble\n1 + 2 // tail";
    assert_eq!(run(src).unwrap(), Value::Int(3));
}

/// B1.3 flipped this test from "documents the gap" to a positive
/// assertion. Before B1.3, recursion via the Y combinator failed at
/// evaluation time because non-recursive `let` could not bind a name
/// in its own RHS, and `let rec` did not yet exist.
#[test]
fn pipeline_letrec_factorial() {
    let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact(6)";
    assert_eq!(run(src).unwrap(), Value::Int(720));
}

#[test]
fn pipeline_type_error_renders_with_caret() {
    use sentinel_effects_proto::run;
    let source = "1 + true";
    let err = run(source).expect_err("Int + Bool should fail type-check");
    let rendered = err.render(source);
    // Header carries the severity tag.
    assert!(rendered.starts_with("type error:"), "header: {rendered}");
    // Source line excerpt is present.
    assert!(rendered.contains("1 | 1 + true"), "excerpt: {rendered}");
    // Some caret is present.
    assert!(rendered.contains("^"), "caret: {rendered}");
}

// ---- B2.2b: end-to-end effect surface ----

#[test]
fn pipeline_effect_decl_with_pure_body_evaluates() {
    // The declaration is accepted by the parser, ignored by
    // inference (B2.3 wires the real env), and the pure body runs.
    let src = "effect Print : Int -> Bool ; 1 + 2";
    assert_eq!(run(src).unwrap(), Value::Int(3));
}

#[test]
fn pipeline_perform_with_declared_effect_typechecks_and_evaluates() {
    // B2.3b2: with `Print` declared, `do Print(1)` type-checks. The
    // evaluator still rejects perform at runtime (no handlers until
    // B3), so we assert the eval-side error rather than a value.
    use sentinel_effects_proto::MiniError;
    let source = "effect Print : Int -> Bool ; do Print(1)";
    let err = run(source).expect_err("evaluator has no handlers yet");
    assert!(
        matches!(err, MiniError::Eval(_)),
        "expected an Eval error (not Type), got {err:?}"
    );
}

#[test]
fn pipeline_perform_undeclared_label_is_unknown_effect() {
    use sentinel_effects_proto::{MiniError, TypeError};
    let source = "do Print(1)";
    let err = run(source).expect_err("undeclared Print should fail type-check");
    match &err {
        MiniError::Type(TypeError::UnknownEffect { label, .. }) => {
            assert_eq!(label, "Print");
        }
        other => panic!("expected UnknownEffect, got {other:?}"),
    }
    let rendered = err.render(source);
    assert!(rendered.starts_with("type error:"), "header: {rendered}");
    assert!(rendered.contains("^"), "caret: {rendered}");
}

#[test]
fn pipeline_handle_resumes_with_arg_through_run() {
    // B3.2b: handlers run end-to-end through `run`. The arm
    // `Get(x, k) => k(x)` resumes with the operation's argument;
    // with an empty kont and no return arm, handle_step's identity
    // branch yields the value unchanged. Pre-B3.2b this surfaced
    // MiniError::Eval(HandlersNotYetSupported) — see commit history
    // around ADR 0007 D5.
    use sentinel_effects_proto::Value;
    let source = "effect Get : Int -> Int ; handle do Get(1) with { Get(x, k) => k(x) }";
    let v = run(source).expect("handle should produce a value");
    assert_eq!(v, Value::Int(1));
}
