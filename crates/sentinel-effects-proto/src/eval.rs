//! Tree-walking evaluator for Sentinel-Mini.
//!
//! As of B1.5b the public [`crate::run`] pipeline type-checks before
//! evaluation, so many shapes that used to surface as
//! [`EvalError::Type`] or [`EvalError::Unbound`] now surface as
//! [`crate::TypeError`] earlier. The eval-level variants remain for
//! defense in depth and for callers that bypass the pipeline.

use crate::ast::{BinOp, Expr, ExprKind, HandlerArm, ReturnArm};
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
    /// B3.2b: a captured continuation, bundled with the deep-handler
    /// re-wrap context (the same arms / ret_arm / env that produced
    /// it). Applied via the normal call form; see [`apply`].
    Resumption {
        kont: Continuation,
        arms: Arc<Vec<HandlerArm>>,
        ret_arm: Option<Arc<ReturnArm>>,
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
    /// B3.2a: defence-in-depth for the runtime top level. If a
    /// `Step::Op` escapes [`eval`] all the way up to [`crate::run`],
    /// the inference pass either did not run or has a bug — the row
    /// of a closed top-level program is `Empty`, so no operation
    /// should be reachable here. This variant is the runtime
    /// counterpart of B2.2b's `EffectNotYetSupported`.
    #[error("unhandled operation at top level: {label}")]
    UnhandledOpAtTopLevel { label: String },
    /// B3.2a: one-shot enforcement. Second call to
    /// [`Continuation::resume`] surfaces this. Keeps multi-shot
    /// extensibility open without committing to it.
    #[error("continuation already resumed (one-shot)")]
    ContinuationAlreadyResumed,
}

#[derive(Debug, Clone, Default)]
pub struct Env(Option<Arc<EnvCell>>);

#[derive(Debug)]
pub(crate) struct EnvCell {
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

#[derive(Debug)]
pub enum Step {
    Value(Value),
    Op {
        label: String,
        arg: Value,
        kont: Continuation,
    },
}

/// B3.2b: derives Clone because [`Value::Resumption`] holds a
/// Continuation by value and `Value` itself is Clone. The Cell<bool>
/// resumed-flag copies its bool on clone, so cloning a *resumed*
/// Continuation produces a clone that also refuses to resume. The
/// one-shot guarantee therefore holds per-Continuation-instance, not
/// per-logical-resumption: nothing in eval clones a Resumption today,
/// but Value being Clone means user-level multi-shot via aliasing is
/// not statically prevented. Revisit if Sentinel proper wants a
/// stricter guarantee (move-only Resumption value variant, or a
/// shared resumed-flag via Rc<Cell<bool>>).
#[derive(Debug, Clone)]
pub struct Continuation {
    frames: Vec<Frame>,
    resumed: std::cell::Cell<bool>,
}

impl Continuation {
    pub fn empty() -> Self {
        Self {
            frames: Vec::new(),
            resumed: std::cell::Cell::new(false),
        }
    }

    pub(crate) fn push(&mut self, f: Frame) {
        self.frames.push(f);
    }

    /// B3.2b: true iff no frames remain. Useful for tests that
    /// want to assert a freshly-reified Op carries an empty kont
    /// without exposing Frame internals.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn resume(self, v: Value) -> Result<Step, EvalError> {
        if self.resumed.get() {
            return Err(EvalError::ContinuationAlreadyResumed);
        }
        self.resumed.set(true);

        let mut frames = self.frames;
        let mut v = v;

        while let Some(frame) = frames.pop() {
            match step_frame(frame, v)? {
                Step::Value(next) => {
                    v = next;
                }
                Step::Op { label, arg, mut kont } => {
                    let mut new_frames = frames;
                    new_frames.extend(kont.frames.drain(..));
                    kont.frames = new_frames;
                    return Ok(Step::Op { label, arg, kont });
                }
            }
        }
        Ok(Step::Value(v))
    }
}

/// Runtime frames that record "what eval was about to do next"
/// at each resume-point in the evaluator. Pushed onto a
/// [`Continuation`] whenever a sub-evaluation produces a
/// `Step::Op`; popped by [`Continuation::resume`] when a handler
/// arm resumes the captured continuation. See ADR 0007 D5.
///
/// Clone is derived because [`Continuation`] is Clone, which in
/// turn is required so [`Value::Resumption`] can live in a Clone
/// `Value`.
#[derive(Debug, Clone)]
pub(crate) enum Frame {
    LetBody {
        name: String,
        body: Arc<Expr>,
        env: Env,
    },
    LetRecBody {
        cell: Arc<EnvCell>,
        body: Arc<Expr>,
        rec_env: Env,
    },
    IfBranch {
        then_branch: Arc<Expr>,
        else_branch: Arc<Expr>,
        env: Env,
    },
    AppArg {
        arg: Arc<Expr>,
        env: Env,
    },
    AppCall {
        func: Value,
    },
    BinOpRight {
        op: BinOp,
        rhs: Arc<Expr>,
        env: Env,
    },
    BinOpApply {
        op: BinOp,
        lhs: Value,
    },
    PerformReify {
        label: String,
    },
    /// B3.2b: when a Step::Op walks past a `handle` that has no
    /// matching arm, we record that handle as a frame on the Op's
    /// kont so that on resume, the value (or further Op) flowing back
    /// is re-dispatched through the same handler. This is what makes
    /// non-matching handlers transparent to inner operations while
    /// still re-wrapping resumed computations — deep handler semantics.
    HandleFwd {
        arms: Arc<Vec<HandlerArm>>,
        ret_arm: Option<Arc<ReturnArm>>,
        env: Env,
    },
}


fn step_frame(frame: Frame, v: Value) -> Result<Step, EvalError> {
    match frame {
        Frame::LetBody { name, body, env } => {
            eval(&body, &env.extend(name, v))
        }
        Frame::LetRecBody { cell, body, rec_env } => {
            cell.value
                .set(v)
                .map_err(|_| EvalError::LetRecUninitialised)?;
            eval(&body, &rec_env)
        }
        Frame::IfBranch { then_branch, else_branch, env } => match v {
            Value::Bool(true) => eval(&then_branch, &env),
            Value::Bool(false) => eval(&else_branch, &env),
            other => Err(EvalError::Type {
                expected: "bool",
                got: type_name(&other),
            }),
        },
        Frame::AppArg { arg, env } => {
            let func = v;
            match eval(&arg, &env)? {
                Step::Value(a) => apply(func, a),
                Step::Op { label, arg: op_arg, mut kont } => {
                    kont.frames.insert(0, Frame::AppCall { func });
                    Ok(Step::Op { label, arg: op_arg, kont })
                }
            }
        }
        Frame::AppCall { func } => apply(func, v),
        Frame::BinOpRight { op, rhs, env } => {
            let lhs = v;
            match eval(&rhs, &env)? {
                Step::Value(r) => eval_binop(op, lhs, r).map(Step::Value),
                Step::Op { label, arg, mut kont } => {
                    kont.frames.insert(0, Frame::BinOpApply { op, lhs });
                    Ok(Step::Op { label, arg, kont })
                }
            }
        }
        Frame::BinOpApply { op, lhs } => {
            eval_binop(op, lhs, v).map(Step::Value)
        }
        Frame::PerformReify { label } => Ok(Step::Op {
            label,
            arg: v,
            kont: Continuation::empty(),
        }),
        Frame::HandleFwd { arms, ret_arm, env } => {
            handle_step(Step::Value(v), &arms, &ret_arm, &env)
        }
    }
}

fn handle_step(
    step: Step,
    arms: &Arc<Vec<HandlerArm>>,
    ret_arm: &Option<Arc<ReturnArm>>,
    env: &Env,
) -> Result<Step, EvalError> {
    match step {
        Step::Value(v) => match ret_arm {
            Some(ra) => eval(&ra.body, &env.extend(ra.var.clone(), v)),
            None => Ok(Step::Value(v)),
        },
        Step::Op { label, arg, kont } => {
            for arm in arms.iter() {
                if arm.label == label {
                    let resumption = Value::Resumption {
                        kont,
                        arms: Arc::clone(arms),
                        ret_arm: ret_arm.clone(),
                        env: env.clone(),
                    };
                    let arm_env = env
                        .extend(arm.arg.clone(), arg)
                        .extend(arm.kont.clone(), resumption);
                    return eval(&arm.body, &arm_env);
                }
            }
            let mut kont = kont;
            kont.push(Frame::HandleFwd {
                arms: Arc::clone(arms),
                ret_arm: ret_arm.clone(),
                env: env.clone(),
            });
            Ok(Step::Op { label, arg, kont })
        }
    }
}

fn apply(func: Value, a: Value) -> Result<Step, EvalError> {
    match func {
        Value::Closure { param, body, env: captured } => {
            eval(&body, &captured.extend(param, a))
        }
        Value::Resumption { kont, arms, ret_arm, env } => {
            let step = kont.resume(a)?;
            handle_step(step, &arms, &ret_arm, &env)
        }
        _ => Err(EvalError::NotAFunction),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int",
        Value::Bool(_) => "bool",
        Value::Closure { .. } => "function",
        Value::Resumption { .. } => "function",
    }
}

pub fn eval(expr: &Expr, env: &Env) -> Result<Step, EvalError> {
    match &expr.node {
        ExprKind::Int(n) => Ok(Step::Value(Value::Int(*n))),
        ExprKind::Bool(b) => Ok(Step::Value(Value::Bool(*b))),
        ExprKind::Var(name) => env.lookup(name).map(Step::Value),
        ExprKind::Let { name, value, body } => {
            match eval(value, env)? {
                Step::Value(v) => eval(body, &env.extend(name.clone(), v)),
                Step::Op { label, arg, mut kont } => {
                    kont.push(Frame::LetBody {
                        name: name.clone(),
                        body: Arc::new((**body).clone()),
                        env: env.clone(),
                    });
                    Ok(Step::Op { label, arg, kont })
                }
            }
        }
        ExprKind::LetRec { name, value, body } => {
            let (rec_env, cell) = env.extend_unset(name.clone());
            match eval(value, &rec_env)? {
                Step::Value(v) => {
                    cell.value
                        .set(v)
                        .map_err(|_| EvalError::LetRecUninitialised)?;
                    eval(body, &rec_env)
                }
                Step::Op { label, arg, mut kont } => {
                    kont.push(Frame::LetRecBody {
                        cell,
                        body: Arc::new((**body).clone()),
                        rec_env,
                    });
                    Ok(Step::Op { label, arg, kont })
                }
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            match eval(cond, env)? {
                Step::Value(Value::Bool(true)) => eval(then_branch, env),
                Step::Value(Value::Bool(false)) => eval(else_branch, env),
                Step::Value(other) => Err(EvalError::Type {
                    expected: "bool",
                    got: type_name(&other),
                }),
                Step::Op { label, arg, mut kont } => {
                    kont.push(Frame::IfBranch {
                        then_branch: Arc::new((**then_branch).clone()),
                        else_branch: Arc::new((**else_branch).clone()),
                        env: env.clone(),
                    });
                    Ok(Step::Op { label, arg, kont })
                }
            }
        }
        ExprKind::Lambda { param, body } => Ok(Step::Value(Value::Closure {
            param: param.clone(),
            body: Arc::new((**body).clone()),
            env: env.clone(),
        })),
        ExprKind::App { callee, arg } => {
            let f = match eval(callee, env)? {
                Step::Value(v) => v,
                Step::Op { label, arg: op_arg, mut kont } => {
                    kont.push(Frame::AppArg {
                        arg: Arc::new((**arg).clone()),
                        env: env.clone(),
                    });
                    return Ok(Step::Op { label, arg: op_arg, kont });
                }
            };
            let a = match eval(arg, env)? {
                Step::Value(v) => v,
                Step::Op { label, arg: op_arg, mut kont } => {
                    kont.push(Frame::AppCall { func: f });
                    return Ok(Step::Op { label, arg: op_arg, kont });
                }
            };
            apply(f, a)
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let l = match eval(lhs, env)? {
                Step::Value(v) => v,
                Step::Op { label, arg, mut kont } => {
                    kont.push(Frame::BinOpRight {
                        op: *op,
                        rhs: Arc::new((**rhs).clone()),
                        env: env.clone(),
                    });
                    return Ok(Step::Op { label, arg, kont });
                }
            };
            let r = match eval(rhs, env)? {
                Step::Value(v) => v,
                Step::Op { label, arg, mut kont } => {
                    kont.push(Frame::BinOpApply { op: *op, lhs: l });
                    return Ok(Step::Op { label, arg, kont });
                }
            };
            eval_binop(*op, l, r).map(Step::Value)
        }
        // B3.2b: reify into Step::Op. The arg is evaluated first;
        // if it itself performs an effect, Frame::PerformReify
        // records the pending reification so resume can complete
        // it once the arg's effect is handled.
        ExprKind::Perform { label, arg, .. } => {
            match eval(arg, env)? {
                Step::Value(v) => Ok(Step::Op {
                    label: label.clone(),
                    arg: v,
                    kont: Continuation::empty(),
                }),
                Step::Op { label: inner_label, arg: inner_arg, mut kont } => {
                    kont.push(Frame::PerformReify { label: label.clone() });
                    Ok(Step::Op { label: inner_label, arg: inner_arg, kont })
                }
            }
        }
        // B3.2b: full handler dispatch per ADR 0007 D5. Eager-Arc
        // the arms and ret_arm once so subsequent Resumption /
        // HandleFwd constructions share them cheaply, then run the
        // body and route the resulting Step through handle_step.
        // handle_step is also the re-entry point for deep re-wrap
        // when an arm body resumes the captured continuation.
        ExprKind::Handle { body, arms, ret_arm } => {
            let arms_arc: Arc<Vec<HandlerArm>> = Arc::new(arms.clone());
            let ret_arm_arc: Option<Arc<ReturnArm>> =
                ret_arm.as_ref().map(|r| Arc::new(r.clone()));
            let step = eval(body, env)?;
            handle_step(step, &arms_arc, &ret_arm_arc, env)
        }
        // ADR 0008 D5: `declassify(e)` is a type-level audit point;
        // at the value layer it is the identity. The Step (Value or
        // Op) propagates unchanged -- declassification does not change
        // any value representation (`Value` is qualifier-blind by B0
        // design), and no resume-point work is needed because there
        // is nothing to do "after" a declassify other than yield the
        // inner value. B4.1a wired the D5 typing rule, so the full
        // pipeline now reaches this arm whenever a well-typed
        // declassify executes.
        ExprKind::Declassify { inner, .. } => eval(inner, env),
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
        match eval(&expr, &Env::empty())? {
            Step::Value(v) => Ok(v),
            Step::Op { label, .. } => Err(MiniError::Eval(
                EvalError::UnhandledOpAtTopLevel { label },
            )),
        }
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
    fn b32b_eval_perform_directly_reifies_step_op() {
        // Bypass infer; call eval on a Perform node directly.
        // B3.2b: Perform now reifies into Step::Op with an empty
        // continuation rather than producing the B2.2b placeholder
        // error. The label and arg flow through unchanged.
        let toks = crate::lexer::lex("do Print(1)").unwrap();
        let expr = crate::parser::parse(&toks).unwrap();
        let step = eval(&expr, &Env::empty()).expect("eval should reify Perform");
        match step {
            Step::Op { label, arg, kont } => {
                assert_eq!(label, "Print");
                assert_eq!(arg, Value::Int(1));
                assert!(kont.is_empty(), "freshly-reified Op carries empty kont");
            }
            Step::Value(v) => panic!("expected Step::Op, got Step::Value({v:?})"),
        }
    }

    #[test]
    fn b32b_eval_handle_no_arms_no_ret_is_identity_on_value() {
        // B3.2b: a handle with empty arms and no ret_arm is the
        // identity handler on values — the body's Step::Value flows
        // through handle_step's None-ret_arm branch unchanged.
        // Pre-B3.2b this returned HandlersNotYetSupported.
        use crate::ast::{Expr, ExprKind};
        use crate::span::Span;
        let body = Box::new(Expr { node: ExprKind::Int(0), span: Span::new(0, 1) });
        let e = Expr {
            node: ExprKind::Handle { body, arms: vec![], ret_arm: None },
            span: Span::new(0, 1),
        };
        let step = eval(&e, &Env::empty()).expect("identity handle should succeed");
        match step {
            Step::Value(Value::Int(0)) => {}
            other => panic!("expected Step::Value(Int(0)), got {other:?}"),
        }
    }
}
