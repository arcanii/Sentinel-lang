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

#[test]
fn pipeline_two_effect_handler_discharges_both() {
    // B3.2c: two distinct arms in one handler. The handle body is
    // `do A(1) + do B(2)`; lhs raises first, the BinOp arm pushes
    // Frame::BinOpRight onto the kont and re-raises; the A arm
    // resumes with x, the resume hits BinOpRight which evals the
    // rhs `do B(2)` and raises again (with Frame::BinOpApply{lhs:1}
    // pushed onto the new Op's kont); the B arm resumes with x, the
    // resume hits BinOpApply, producing Int(1 + 2). Exercises deep
    // re-wrap (apply's handle_step after resume) and the splice
    // mechanism (BinOpRight producing a nested Op).
    use sentinel_effects_proto::Value;
    let source = "\
        effect A : Int -> Int ; \
        effect B : Int -> Int ; \
        handle do A(1) + do B(2) with { \
            A(x, k) => k(x), \
            B(x, k) => k(x) \
        }";
    let v = run(source).expect("two-effect handle should produce a value");
    assert_eq!(v, Value::Int(3));
}

#[test]
fn pipeline_arm_body_computes_with_op_arg_before_resume() {
    // B3.2c: arm body does work with x before passing to k. Handle
    // body is `10 + do Ask(0)`; lhs is a Value so BinOp evaluates
    // rhs which raises, BinOp pushes Frame::BinOpApply{lhs:10} and
    // re-raises. Arm body `k(x + 5)` computes 0+5=5 and resumes,
    // hitting BinOpApply which produces 10+5=15. Exercises the
    // rhs-raises BinOp path (sibling to test 1's lhs-raises path).
    use sentinel_effects_proto::Value;
    let source = "\
        effect Ask : Int -> Int ; \
        handle 10 + do Ask(0) with { \
            Ask(x, k) => k(x + 5) \
        }";
    let v = run(source).expect("arm-body-computes handle should produce a value");
    assert_eq!(v, Value::Int(15));
}

#[test]
fn pipeline_arm_body_uses_outer_let_binding() {
    // B3.2c: arm body references a variable bound in the enclosing
    // scope, not introduced by the operation. The Resumption value
    // bundles the env captured at Handle-eval time, so the arm body
    // (which evaluates under that env extended with x and k) can see
    // outer bindings. This is the closest the current language gets
    // to a state handler without CPS gymnastics.
    use sentinel_effects_proto::Value;
    let source = "\
        effect Get : Int -> Int ; \
        let s = 42 in \
        handle do Get(0) with { \
            Get(dummy, k) => k(s) \
        }";
    let v = run(source).expect("outer-let handle should produce a value");
    assert_eq!(v, Value::Int(42));
}

#[test]
fn pipeline_return_arm_runs_on_resumed_value() {
    // B3.2c: explicit return arm transforms the final value. Handle
    // body raises immediately; the Ask arm resumes with x; the
    // resumed Value flows through handle_step's Some(ret_arm) branch
    // and is rebound as `v` inside `v + 1`. The integration test
    // from B3.2b had no ret_arm (identity branch); this covers the
    // present branch.
    use sentinel_effects_proto::Value;
    let source = "\
        effect E : Int -> Int ; \
        handle do E(7) with { \
            E(x, k) => k(x), \
            return v => v + 1 \
        }";
    let v = run(source).expect("ret-arm handle should produce a value");
    assert_eq!(v, Value::Int(8));
}

// ---- B4.0c: secret/declassify surface end-to-end (ADR 0008 D9) ----

#[test]
fn pipeline_declassify_on_non_secret_is_secret_flow() {
    // B4.1a wired the D5 typing rule: `e : Secret<T> ⊢ declassify(e) : T`.
    // On `declassify(1)`, `1 : Int` cannot unify with `Secret(α)` so
    // the catch-all SecretFlow arm fires. (Was a placeholder
    // rejection at B4.0c, renamed and rewritten here.)
    use sentinel_effects_proto::{MiniError, TypeError};
    let err = run("declassify(1)").unwrap_err();
    match err {
        MiniError::Type(TypeError::SecretFlow { .. }) => {}
        other => panic!("expected SecretFlow, got {other:?}"),
    }
}

#[test]
fn pipeline_effect_decl_with_secret_now_type_checks() {
    // B4.1b removed the effect-decl walker. `effect ReadKey : Int ->
    // secret Int ;` now type-checks; the body `0` just evaluates to
    // Int. (Was a placeholder rejection at B4.0c, rewritten as a
    // positive end-to-end check here.)
    let v = run("effect ReadKey : Int -> secret Int ; 0").expect("should type-check");
    assert_eq!(v, Value::Int(0));
}

#[test]
fn pipeline_double_secret_rejected_at_parse() {
    // `secret secret T` is rejected by the parser. The smart
    // constructor would still collapse it, but the parser-level
    // rejection is the human-source early complaint (ADR 0008 D1/D6).
    use sentinel_effects_proto::{MiniError, ParseError};
    let err = run("effect F : Int -> secret secret Int ; 0").unwrap_err();
    match err {
        MiniError::Parse(ParseError::DoubleSecret { .. }) => {}
        other => panic!("expected DoubleSecret, got {other:?}"),
    }
}

// ---- B4.1b: password-verify demo (HANDOVER §5.2 deliverable) ----
//
// The full demo per HANDOVER: a program that tries to branch on a
// secret comparison fails to compile. The chain is D4 -> D3:
//   - `do GetStored(unit) == provided` types as Secret<Bool> per D4
//   - the surrounding `if` then rejects per D3 SecretBranch
// This is the load-bearing rejection deliverable that motivated B4.

#[test]
fn pipeline_password_verify_naive_rejects_with_secret_branch() {
    // `if (do GetStored(0)) == 42 then 1 else 0` with GetStored
    // returning `secret Int`. The comparison produces Secret<Bool>,
    // the surrounding if rejects.
    use sentinel_effects_proto::{MiniError, TypeError};
    let source = "\
        effect GetStored : Int -> secret Int ; \
        if do GetStored(0) == 42 then 1 else 0";
    let err = run(source).expect_err("naive password verify should reject");
    match err {
        MiniError::Type(TypeError::SecretBranch { .. }) => {}
        other => panic!("expected SecretBranch, got {other:?}"),
    }
}

#[test]
fn pipeline_secret_in_arithmetic_is_secret_flow() {
    // Adding a public Int to a secret-returning effect's result is
    // SecretFlow -- arithmetic isn't D4-extended (only comparisons
    // are), so unify(Int, Secret<Int>) fires the catch-all SecretFlow.
    use sentinel_effects_proto::{MiniError, TypeError};
    let source = "effect ReadKey : Int -> secret Int ; do ReadKey(0) + 1";
    let err = run(source).expect_err("secret + int should reject");
    match err {
        MiniError::Type(TypeError::SecretFlow { .. }) => {}
        other => panic!("expected SecretFlow, got {other:?}"),
    }
}

// Note: a positive end-to-end test that runs `declassify(e)` at
// runtime cannot be constructed without a `classify` primitive
// (the Secret-introduction dual of declassify). ADR 0008 D5
// intentionally omits such a form -- declassification sites must be
// syntactically distinguishable in source as the only Secret-removing
// construct. The Secret-introducing path is restricted to typing of
// `do L(arg)` for effect-decls naming secret. Inside the arm body
// the captured continuation `k : Secret<T> -> ...` requires a Secret
// value to resume, and there is no source form that constructs one.
// Positive coverage for the D5 typing rule lives in
// `b41a_declassify_on_secret_unwraps_the_inner_type` (lib, synthetic
// env). HANDOVER §5.2's password-verify demo is purely a rejection
// deliverable per ADR 0008 D9.
