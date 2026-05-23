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
//! - **B1.4**: types scaffold (Ty, Scheme, Subst, unify, ...).
//! - **B1.5a**: HM inference driver (infer, TypeEnv, infer_top).
//! - **B1.5b** (this commit): pipeline wiring. [`run`] now type-checks
//!   between parse and eval; type errors abort before evaluation.
//!   `let rec` is still typed at a monovar; B1.6 will refine.
//! - B1.6 - B1.8 remaining: letrec generalisation, diagnostic
//!   rendering with carets.

pub mod ast;
pub mod eval;
pub mod infer;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod types;

pub use ast::{expr, BinOp, Expr, ExprKind};
pub use eval::{eval, Env, EvalError, Value};
pub use infer::{
    generalize, infer, infer_top, instantiate, unify, Subst, TyVarSupply, TypeEnv, TypeError,
};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};
pub use span::{Span, Spanned};
pub use types::{Scheme, Ty, TyVar};

/// Convenience: lex + parse + **type-check** + eval a source string in
/// a fresh environment.
///
/// As of B1.5b, type errors abort the pipeline before evaluation. The
/// returned [`Value`] is therefore guaranteed to be of the type
/// reported by [`infer_top`] on the same source.
pub fn run(source: &str) -> Result<Value, MiniError> {
    let tokens = lex(source).map_err(MiniError::Lex)?;
    let expr = parse(&tokens).map_err(MiniError::Parse)?;
    infer_top(&expr).map_err(MiniError::Type)?;
    eval(&expr, &Env::empty()).map_err(MiniError::Eval)
}

/// Top-level error type spanning every pipeline stage.
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
