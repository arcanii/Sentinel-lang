//! sentinel-effects-proto (Sentinel-Mini)
//!
//! A research-grade tree-walking interpreter built to validate Sentinel's
//! effect-system design before committing to the production compiler in
//! Phase C. See HANDOVER.md §5 for the strategic framing.
//!
//! # Status
//!
//! - **B0**: lex + parse + eval, no types or effects.
//! - **B1.1**: span infrastructure; lexer emits `(Token, Span)` pairs.
//! - **B1.2**: AST nodes wrap [`ExprKind`] as [`Spanned<ExprKind>`](Spanned);
//!   parser threads spans through every construction site.
//! - **B1.3** (this commit): `let rec` keyword, [`ExprKind::LetRec`] variant,
//!   and `OnceLock`-based knot-tying in the evaluator. The recursive RHS
//!   is required to be a lambda, enforced at parse time.
//! - B1.4 - B1.8 remaining: HM inference, diagnostic rendering.

pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod span;

pub use ast::{expr, BinOp, Expr, ExprKind};
pub use eval::{eval, Env, EvalError, Value};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};
pub use span::{Span, Spanned};

/// Convenience: lex + parse + eval a source string in a fresh environment.
pub fn run(source: &str) -> Result<Value, MiniError> {
    let tokens = lex(source).map_err(MiniError::Lex)?;
    let expr = parse(&tokens).map_err(MiniError::Parse)?;
    eval(&expr, &Env::empty()).map_err(MiniError::Eval)
}

/// Top-level error type spanning every pipeline stage.
#[derive(Debug, thiserror::Error)]
pub enum MiniError {
    #[error("lex error: {0}")]
    Lex(#[from] LexError),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("eval error: {0}")]
    Eval(#[from] EvalError),
}
