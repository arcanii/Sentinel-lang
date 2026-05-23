//! sentinel-effects-proto (Sentinel-Mini)
//!
//! A research-grade tree-walking interpreter built to validate Sentinel's
//! effect-system design before committing to the production compiler in
//! Phase C. See HANDOVER.md §5 ("The Effects Prototype") for the strategic
//! framing.
//!
//! # Scope (B0)
//!
//! This is the **B0** milestone: the smallest possible end-to-end
//! pipeline. The language at B0 is a pure expression calculus with:
//!
//! - integer literals (`42`, `-3`)
//! - boolean literals (`true`, `false`)
//! - identifiers
//! - `let x = e1 in e2`
//! - `if cond then e1 else e2`
//! - lambdas: `fn(x) => body`
//! - application: `f(x)`
//! - arithmetic: `+ - * /`
//! - comparison: `== < >`
//! - parenthesised grouping
//!
//! Everything is an expression; there are no statements. There are no
//! types, no effects, no `secret` qualifier, no broker integration.
//! Those land in B1 through B4 (see BACKLOG / future plan).
//!
//! # Non-goals
//!
//! Performance is irrelevant. Error recovery is minimal (first error
//! wins). The AST is plain `enum`s; no arena allocation. This is a
//! research artifact, not a production compiler.

pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use ast::Expr;
pub use eval::{eval, EvalError, Value};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};

/// Convenience: lex + parse + eval a source string in a fresh environment.
///
/// Returns the resulting [`Value`] or the first error encountered.
pub fn run(source: &str) -> Result<Value, MiniError> {
    let tokens = lex(source).map_err(MiniError::Lex)?;
    let expr = parse(&tokens).map_err(MiniError::Parse)?;
    eval(&expr, &eval::Env::empty()).map_err(MiniError::Eval)
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
