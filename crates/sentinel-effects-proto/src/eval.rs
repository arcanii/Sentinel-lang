//! Tree-walking evaluator for Sentinel-Mini.
//!
//! As of B1.5b the public [`crate::run`] pipeline type-checks before
//! evaluation, so many shapes that used to surface as
//! [`EvalError::Type`] or [`EvalError::Unbound`] now surface as
//! [`crate::TypeError`] earlier. The eval-level variants remain for
//! defense in depth and for callers that bypass the pipeline.

use crate::ast::{BinOp, Expr, ExprKind};
use std::sync::{Arc, OnceLock};
use thiserror::Error;

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
    #[error("internal: letrec cell read before initialisation")]
    LetRecUninitialised,
    /// B2.2b: defence-in-depth. Inference catches `do Label(arg)`
    /// in the standard pipeline, but a caller that bypasses
    /// [`crate::infer_program`] and goes straight to [`eval`] will
    /// surface this instead of a panic.
    #[error("effect {0:?} cannot be performed yet (handlers arrive in B3)")]
    EffectNotYetSupported(String),
    /// B3.0: `handle e with { ... }` is parseable but has no runtime
    /// yet. B3.2 lands the operation-reification model (ADR 0007 D5);
    /// this variant is removed at that point alongside
    /// `EffectNotYetSupported`.
    #[error("handlers are not yet supported at runtime (B3.2 lands eval)")]
    HandlersNotYetSupported,
}

#[derive(Debug, Clone, Default)]
pub struct Env(Option<Arc<EnvCell>>);

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

    pub fn extend(&self, name: String, value: Value) -> Self {
        let cell = EnvCell {
            name,
            value: OnceLock::new(),
            rest: self.clone(),
        };
        cell.value.set(value).expect("freshly-constructed OnceLock");
        Env(Some(Arc::new(cell)))
    }

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
            let (rec_env, cell) = env.extend_unset(name.clone());
            let v = eval(value, &rec_env)?;
            cell.value
                .set(v)
                .map_err(|_| EvalError::LetRecUninitialised)?;
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
        // B2.2b: unreachable through the pipeline (inference catches
        // it first), but if eval is called directly we surface a
        // dedicated error instead of panicking.
        ExprKind::Perform { label, .. } => {
            Err(EvalError::EffectNotYetSupported(label.clone()))
        }
        // B3.0: handler surface parses but runtime arrives in B3.2.
        ExprKind::Handle { .. } => {
            Err(EvalError::HandlersNotYetSupported)
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
    //! These tests go through the full pipeline via a local `run`
    //! helper that mirrors the library top-level `run`: lex, parse,
    //! type-check, eval. Programs that used to surface eval-time
    //! type errors now surface them as `MiniError::Type` instead.

    use super::*;
    use crate::{lex, parse, MiniError, TypeError};

    fn run(src: &str) -> Result<Value, MiniError> {
        let toks = lex(src)?;
        let expr = parse(&toks)?;
        crate::infer::infer_top(&expr).map_err(MiniError::Type)?;
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
    fn unbound_variable_now_surfaces_as_type_error() {
        // Pre-B1.5b this was EvalError::Unbound. B1.5b catches it earlier.
        let err = run("oops").unwrap_err();
        assert!(matches!(
            err,
            MiniError::Type(TypeError::Unbound { .. })
        ));
    }

    #[test]
    fn eval_div_by_zero_is_runtime() {
        // Division by zero is value-level; the type system does not
        // and cannot catch it.
        let err = run("10 / 0").unwrap_err();
        assert!(matches!(err, MiniError::Eval(EvalError::DivByZero)));
    }

    #[test]
    fn if_non_bool_cond_now_surfaces_at_type_check() {
        // Pre-B1.5b this was EvalError::Type. B1.5b catches it earlier.
        let err = run("if 1 then 2 else 3").unwrap_err();
        assert!(matches!(
            err,
            MiniError::Type(TypeError::Mismatch { .. })
        ));
    }

    #[test]
    fn eval_let_rec_factorial() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact(5)";
        assert_eq!(run(src).unwrap(), Value::Int(120));
    }

    #[test]
    fn eval_let_rec_countdown_to_zero() {
        let src = "let rec down = fn(n) => if n == 0 then 0 else down(n - 1) in down(7)";
        assert_eq!(run(src).unwrap(), Value::Int(0));
    }

    #[test]
    fn eval_let_rec_can_be_passed_around() {
        let src = "
            let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in
            let apply = fn(f) => f(4) in
            apply(fact)
        ";
        assert_eq!(run(src).unwrap(), Value::Int(24));
    }

    // ---- B2.2b: direct-eval defence in depth ----

    #[test]
    fn b22b_eval_perform_directly_returns_effect_error() {
        // Bypass infer; call eval on a Perform node directly.
        let toks = crate::lexer::lex("do Print(1)").unwrap();
        let expr = crate::parser::parse(&toks).unwrap();
        let err = eval(&expr, &Env::empty()).expect_err("eval should reject Perform");
        match err {
            EvalError::EffectNotYetSupported(label) => assert_eq!(label, "Print"),
            other => panic!("expected EffectNotYetSupported, got {other:?}"),
        }
    }

    #[test]
    fn b30_eval_handle_directly_returns_handlers_not_yet_supported() {
        use crate::ast::{Expr, ExprKind};
        use crate::span::Span;
        let body = Box::new(Expr { node: ExprKind::Int(0), span: Span::new(0, 1) });
        let e = Expr {
            node: ExprKind::Handle { body, arms: vec![], ret_arm: None },
            span: Span::new(0, 1),
        };
        let err = eval(&e, &Env::empty()).unwrap_err();
        assert!(
            matches!(err, EvalError::HandlersNotYetSupported),
            "expected HandlersNotYetSupported, got {err:?}"
        );
    }
}
