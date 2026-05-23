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
