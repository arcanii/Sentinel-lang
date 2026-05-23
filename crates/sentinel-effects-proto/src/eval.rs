//! Tree-walking evaluator for Sentinel-Mini (B0).
//!
//! Closures capture by sharing a persistent environment (a cons-list of
//! `(name, value)` cells behind an `Arc`). This is the textbook
//! Crafting-Interpreters-style approach; we'll revisit it if/when we
//! integrate the broker as a value heap in a later B milestone.

use crate::ast::{BinOp, Expr};
use std::sync::Arc;
use thiserror::Error;

/// Runtime values.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    /// A closure: parameter, body, and the environment captured at
    /// definition time.
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
            // Closures compare by identity-of-source, which we don't
            // track at B0; treat them as never equal.
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
}

/// A persistent environment. Cheaply cloneable.
#[derive(Debug, Clone, Default)]
pub struct Env(Option<Arc<EnvCell>>);

#[derive(Debug)]
struct EnvCell {
    name: String,
    value: Value,
    rest: Env,
}

impl Env {
    /// The empty environment.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Extend with a new binding. Returns a new environment; the original
    /// is unchanged (shared structurally).
    pub fn extend(&self, name: String, value: Value) -> Self {
        Env(Some(Arc::new(EnvCell { name, value, rest: self.clone() })))
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        let mut cur = self.0.as_deref();
        while let Some(cell) = cur {
            if cell.name == name {
                return Some(cell.value.clone());
            }
            cur = cell.rest.0.as_deref();
        }
        None
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
    match expr {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Var(name) => env
            .lookup(name)
            .ok_or_else(|| EvalError::Unbound(name.clone())),
        Expr::Let { name, value, body } => {
            let v = eval(value, env)?;
            eval(body, &env.extend(name.clone(), v))
        }
        Expr::If { cond, then_branch, else_branch } => {
            let c = eval(cond, env)?;
            match c {
                Value::Bool(true) => eval(then_branch, env),
                Value::Bool(false) => eval(else_branch, env),
                other => Err(EvalError::Type { expected: "bool", got: type_name(&other) }),
            }
        }
        Expr::Lambda { param, body } => Ok(Value::Closure {
            param: param.clone(),
            body: Arc::new((**body).clone()),
            env: env.clone(),
        }),
        Expr::App { callee, arg } => {
            let f = eval(callee, env)?;
            let a = eval(arg, env)?;
            match f {
                Value::Closure { param, body, env: captured } => {
                    eval(&body, &captured.extend(param, a))
                }
                _ => Err(EvalError::NotAFunction),
            }
        }
        Expr::BinOp { op, lhs, rhs } => {
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
}
