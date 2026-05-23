//! Tree-walking evaluator for Sentinel-Mini.
//!
//! Closures capture by sharing a persistent environment - a cons-list of
//! `(name, value)` cells behind an [`Arc`].
//!
//! # `let rec` knot-tying (B1.3)
//!
//! For [`ExprKind::LetRec`] the binding's value is itself a lambda that
//! must be able to see its own name. We do this by allocating the
//! environment cell *first* with a [`OnceLock`]-backed value slot, then
//! constructing the closure with that environment already in scope, then
//! filling the cell. The closure's captured environment therefore
//! contains a cell that, by the time the closure is ever *invoked*, has
//! been initialised.
//!
//! Two consequences worth noting:
//!
//! 1. The cell's value is read through `OnceLock::get`, which returns
//!    `None` until set. Lookup falls back to "unbound" in that window.
//!    In practice the parser guarantees the RHS is a lambda, so no code
//!    can observe the uninitialised window during evaluation of the
//!    binding itself - lambdas don't evaluate their bodies eagerly.
//! 2. We use `std::sync::OnceLock` (stable, no extra dependency) rather
//!    than `once_cell::sync::OnceCell`.

use crate::ast::{BinOp, Expr, ExprKind};
use std::sync::{Arc, OnceLock};
use thiserror::Error;

/// Runtime values.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Closure {
        param: String,
        body: Arc<Expr>,
        env: Env,
    },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            _ => false,
        }
    }
}

/// Errors produced by [`eval`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum EvalError {
    #[error("unbound variable: {0}")]
    Unbound(String),
    #[error("type error: expected {expected}, got {got}")]
    Type { expected: &'static str, got: &'static str },
    #[error("division by zero")]
    DivByZero,
    #[error("cannot apply non-function value")]
    NotAFunction,
    /// A `let rec` cell was read before being initialised. Should be
    /// unreachable given the lambda-RHS restriction; surfaced as an error
    /// rather than a panic for safety.
    #[error("internal: letrec cell read before initialisation")]
    LetRecUninitialised,
}

/// A persistent environment. Cheaply cloneable.
#[derive(Debug, Clone, Default)]
pub struct Env(Option<Arc<EnvCell>>);

/// A single environment binding. `value` is wrapped in `OnceLock` so that
/// `let rec` can construct the cell, then back-patch it. Non-recursive
/// `let` initialises the cell immediately and never observes the empty
/// state.
#[derive(Debug)]
struct EnvCell {
    name: String,
    value: OnceLock<Value>,
    rest: Env,
}

impl Env {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Extend with a fully-initialised binding (non-recursive `let`).
    pub fn extend(&self, name: String, value: Value) -> Self {
        let cell = EnvCell {
            name,
            value: OnceLock::new(),
            rest: self.clone(),
        };
        // `set` only fails if already set; we just created it, so unwrap is safe.
        cell.value.set(value).expect("freshly-constructed OnceLock");
        Env(Some(Arc::new(cell)))
    }

    /// Extend with a placeholder binding to be filled later. Returns the
    /// new environment plus a handle to the cell whose value still needs
    /// to be set. Used only by `let rec`.
    fn extend_unset(&self, name: String) -> (Self, Arc<EnvCell>) {
        let cell = Arc::new(EnvCell {
            name,
            value: OnceLock::new(),
            rest: self.clone(),
        });
        (Env(Some(cell.clone())), cell)
    }

    fn lookup(&self, name: &str) -> Result<Value, EvalError> {
        let mut cur = self.0.as_deref();
        while let Some(cell) = cur {
            if cell.name == name {
                return cell
                    .value
                    .get()
                    .cloned()
                    .ok_or(EvalError::LetRecUninitialised);
            }
            cur = cell.rest.0.as_deref();
        }
        Err(EvalError::Unbound(name.to_string()))
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int",
        Value::Bool(_) => "bool",
        Value::Closure { .. } => "function",
    }
}

/// Evaluate `expr` in the given environment.
pub fn eval(expr: &Expr, env: &Env) -> Result<Value, EvalError> {
    match &expr.node {
        ExprKind::Int(n) => Ok(Value::Int(*n)),
        ExprKind::Bool(b) => Ok(Value::Bool(*b)),
        ExprKind::Var(name) => env.lookup(name),
        ExprKind::Let { name, value, body } => {
            let v = eval(value, env)?;
            eval(body, &env.extend(name.clone(), v))
        }
        ExprKind::LetRec { name, value, body } => {
            // Step 1: allocate the cell with no value yet, with `name`
            // already in scope.
            let (rec_env, cell) = env.extend_unset(name.clone());
            // Step 2: evaluate the RHS *in the recursive environment*.
            // The parser guarantees `value` is a Lambda, so this produces
            // a closure that captures `rec_env` - which contains the
            // still-empty cell.
            let v = eval(value, &rec_env)?;
            // Step 3: tie the knot. `set` returns Err if already set;
            // shouldn't happen, but if it does it's diagnostic.
            cell.value
                .set(v)
                .map_err(|_| EvalError::LetRecUninitialised)?;
            // Step 4: evaluate the body in the now-complete environment.
            eval(body, &rec_env)
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            let c = eval(cond, env)?;
            match c {
                Value::Bool(true) => eval(then_branch, env),
                Value::Bool(false) => eval(else_branch, env),
                other => Err(EvalError::Type { expected: "bool", got: type_name(&other) }),
            }
        }
        ExprKind::Lambda { param, body } => Ok(Value::Closure {
            param: param.clone(),
            body: Arc::new((**body).clone()),
            env: env.clone(),
        }),
        ExprKind::App { callee, arg } => {
            let f = eval(callee, env)?;
            let a = eval(arg, env)?;
            match f {
                Value::Closure { param, body, env: captured } => {
                    eval(&body, &captured.extend(param, a))
                }
                _ => Err(EvalError::NotAFunction),
            }
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let l = eval(lhs, env)?;
            let r = eval(rhs, env)?;
            eval_binop(*op, l, r)
        }
    }
}

fn eval_binop(op: BinOp, l: Value, r: Value) -> Result<Value, EvalError> {
    use BinOp::*;
    match (op, &l, &r) {
        (Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_add(*b))),
        (Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_sub(*b))),
        (Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_mul(*b))),
        (Div, Value::Int(_), Value::Int(0)) => Err(EvalError::DivByZero),
        (Div, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_div(*b))),
        (Eq, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a == b)),
        (Eq, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
        (Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
        (Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
        _ => Err(EvalError::Type {
            expected: "matching numeric/boolean operands",
            got: type_name(&l),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse};

    fn run(src: &str) -> Result<Value, super::super::MiniError> {
        let toks = lex(src)?;
        let expr = parse(&toks)?;
        Ok(eval(&expr, &Env::empty())?)
    }

    #[test]
    fn eval_arithmetic() {
        assert_eq!(run("1 + 2 * 3").unwrap(), Value::Int(7));
    }

    #[test]
    fn eval_let_in() {
        assert_eq!(run("let x = 10 in x + x").unwrap(), Value::Int(20));
    }

    #[test]
    fn eval_if_then_else() {
        assert_eq!(run("if 1 < 2 then 100 else 200").unwrap(), Value::Int(100));
    }

    #[test]
    fn eval_lambda_application() {
        assert_eq!(run("(fn(x) => x + 1)(41)").unwrap(), Value::Int(42));
    }

    #[test]
    fn eval_closure_captures_env() {
        let v = run("let y = 10 in (fn(x) => x + y)(5)").unwrap();
        assert_eq!(v, Value::Int(15));
    }

    #[test]
    fn eval_unbound_variable() {
        let err = run("oops").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::Unbound(_))));
    }

    #[test]
    fn eval_div_by_zero() {
        let err = run("10 / 0").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::DivByZero)));
    }

    #[test]
    fn eval_type_error_in_if() {
        let err = run("if 1 then 2 else 3").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::Type { .. })));
    }

    #[test]
    fn eval_let_rec_factorial() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact(5)";
        assert_eq!(run(src).unwrap(), Value::Int(120));
    }

    #[test]
    fn eval_let_rec_countdown_to_zero() {
        // A simpler letrec test that doesn't lean on multiplication.
        let src = "let rec down = fn(n) => if n == 0 then 0 else down(n - 1) in down(7)";
        assert_eq!(run(src).unwrap(), Value::Int(0));
    }

    #[test]
    fn eval_let_rec_can_be_passed_around() {
        // The recursive function survives being used as a first-class value.
        let src = "
            let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in
            let apply = fn(f) => f(4) in
            apply(fact)
        ";
        assert_eq!(run(src).unwrap(), Value::Int(24));
    }
}
