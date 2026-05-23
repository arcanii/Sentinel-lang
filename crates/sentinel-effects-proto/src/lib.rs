//! sentinel-effects-proto (Sentinel-Mini)
//!
//! A research-grade tree-walking interpreter built to validate Sentinel's
//! effect-system design before committing to the production compiler in
//! Phase C. See HANDOVER.md §5 for the strategic framing.
//!
//! # Status
//!
//! - **B0**: lex + parse + eval.
//! - **B1.1**: span infrastructure.
//! - **B1.2**: AST nodes carry spans.
//! - **B1.3**: `let rec` with `OnceLock` knot-tying.
//! - **B1.4** (this commit): types scaffold - [`Ty`], [`Scheme`],
//!   [`Subst`], [`unify`], [`instantiate`], [`generalize`]. Not yet
//!   integrated into the pipeline; `run()` still skips type checking.
//! - B1.5 - B1.8 remaining: wire inference into `run`, letrec typing,
//!   diagnostic rendering.

pub mod ast;
pub mod eval;
pub mod infer;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod types;

pub use ast::{expr, BinOp, Expr, ExprKind};
pub use eval::{eval, Env, EvalError, Value};
pub use infer::{generalize, instantiate, unify, Subst, TyVarSupply, TypeError};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};
pub use span::{Span, Spanned};
pub use types::{Scheme, Ty, TyVar};

/// Convenience: lex + parse + eval a source string in a fresh environment.
///
/// Note: as of B1.4 type inference is implemented as a library
/// (see [`infer`]) but is **not yet** invoked here. Wiring lands in B1.5.
pub fn run(source: &str) -> Result<Value, MiniError> {
    let tokens = lex(source).map_err(MiniError::Lex)?;
    let expr = parse(&tokens).map_err(MiniError::Parse)?;
    eval(&expr, &Env::empty()).map_err(MiniError::Eval)
}

/// Top-level error type spanning every pipeline stage.
///
/// `Type` is wired up now so B1.5's pipeline change is purely additive.
#[derive(Debug, thiserror::Error)]
pub enum MiniError {
    #[error("lex error: {0}")]
    Lex(#[from] LexError),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("type error: {0}")]
    Type(#[from] TypeError),
    #[error("eval error: {0}")]
    Eval(#[from] EvalError),
}
