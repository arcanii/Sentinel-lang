//! End-to-end tests for sentinel-effects-proto B0.
//!
//! These exercise the full lex+parse+eval pipeline via the top-level
//! [`run`] convenience function, ensuring the public surface composes.

use sentinel_effects_proto::{run, Value};

#[test]
fn pipeline_arithmetic() {
    assert_eq!(run("1 + 2 * 3 - 4").unwrap(), Value::Int(3));
}

#[test]
fn pipeline_nested_let() {
    let src = "let x = 1 in let y = x + 1 in let z = y + 1 in x + y + z";
    assert_eq!(run(src).unwrap(), Value::Int(1 + 2 + 3));
}

#[test]
fn pipeline_higher_order_function() {
    // (fn(f) => f(10))(fn(x) => x * x)  =  100
    assert_eq!(
        run("(fn(f) => f(10))(fn(x) => x * x)").unwrap(),
        Value::Int(100),
    );
}

#[test]
fn pipeline_recursion_via_y_combinator_would_need_letrec() {
    // B0 has no `letrec`; this just documents that recursion is a B1+
    // problem. We test that the missing-feature failure mode is sensible:
    // `f` is unbound inside its own definition.
    let src = "let f = fn(n) => if n == 0 then 0 else f(n - 1) in f(3)";
    let err = run(src).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unbound variable: f"), "got: {msg}");
}

#[test]
fn pipeline_line_comments_are_ignored() {
    let src = "// header comment\n1 + 2 // tail comment\n";
    assert_eq!(run(src).unwrap(), Value::Int(3));
}
