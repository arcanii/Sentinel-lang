#![allow(clippy::result_large_err)]
//! ^ B2.0: widening `Ty::Fun` with `Row` pushed `TypeError::Mismatch`
//! over the 128-byte threshold that triggers `clippy::result_large_err`.
//! Per STATE.md sec B.5 design decision 5 (effects-proto is a research
//! prototype; we are not optimising error shape), we allow this lint
//! crate-wide rather than boxing every `Result<_, TypeError>`. Phase C
//! production code will revisit.

//! sentinel-effects-proto (Sentinel-Mini)
//!
//! A research-grade tree-walking interpreter built to validate Sentinel's
//! effect-system design before committing to the production compiler in
//! Phase C. See HANDOVER.md §5 for the strategic framing.
//!
//! # Status
//!
//! - **B0-B1**: complete. Lex + parse + HM type inference (with
//!   let-polymorphism and let-rec generalization) + eval, all
//!   span-tracked, with hand-rolled caret diagnostics via
//!   [`MiniError::render`].
//! - **B2** (next): effect rows and effect declarations. Per
//!   ADR 0002, B1's function arrows are bare `Fun(Ty, Ty)`; B2
//!   extends this with a row component.
//! - **B3-B4** (planned): effect handlers, `secret` qualifier.
//!
//! See `docs/STATE.md` Section B for the authoritative status.

pub mod ast;
pub mod diag;
pub mod eval;
pub mod infer;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod types;

pub use ast::{expr, BinOp, EffectDecl, Expr, ExprKind, Program, TyExpr};
pub use eval::{eval, Env, EvalError, Value};
pub use infer::{
    generalize, infer, infer_program, infer_top, instantiate, unify, EffectEnv, Subst,
    TyVarSupply, TypeEnv, TypeError,
};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, parse_program, ParseError};
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
    // B2.2b: pipeline now goes through `parse_program` and
    // `infer_program`. Effect declarations are parsed but inert
    // until B2.3.
    let program = parse_program(&tokens).map_err(MiniError::Parse)?;
    infer_program(&program).map_err(MiniError::Type)?;
    eval(&program.body, &Env::empty()).map_err(MiniError::Eval)
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

impl MiniError {
    /// Render this error with a caret-underlined source excerpt.
    ///
    /// `Display` produces a single terse line; this method produces the
    /// multi-line rustc-style diagnostic. Callers that have the source
    /// string handy should prefer this for human-facing output.
    pub fn render(&self, source: &str) -> String {
        use crate::diag::render;
        match self {
            MiniError::Lex(e) => match e {
                LexError::Unrecognised { span, .. } => {
                    render(source, Some(*span), "lex error", &e.to_string())
                }
            },
            MiniError::Parse(e) => {
                let span = match e {
                    ParseError::UnexpectedEof { .. } => None,
                    ParseError::Unexpected { span, .. } => Some(*span),
                    ParseError::Trailing { span, .. } => Some(*span),
                    ParseError::LetRecNotLambda { span } => Some(*span),
                    ParseError::EffectLabelNotUpper { span, .. } => Some(*span),
                    ParseError::ExpectedTypeExpr { span, .. } => Some(*span),
                    ParseError::HandlerArmLabelNotUpper { span, .. } => Some(*span),
                    ParseError::EmptyHandler { span } => Some(*span),
                };
                render(source, span, "parse error", &e.to_string())
            }
            MiniError::Type(e) => {
                let span = match e {
                    TypeError::Mismatch { span, .. } => Some(*span),
                    TypeError::OccursCheck { span, .. } => Some(*span),
                    TypeError::Unbound { span, .. } => Some(*span),
                    TypeError::RowMismatch { span, .. } => Some(*span),
                    TypeError::RowOccursCheck { span, .. } => Some(*span),
                    TypeError::UnknownEffect { span, .. } => Some(*span),
                    TypeError::UnhandledEffects { span, .. } => Some(*span),
                    TypeError::HandlerLabelNotInRow { span, .. } => Some(*span),
                    TypeError::HandlersNotYetSupported { span } => Some(*span),
                };
                render(source, span, "type error", &e.to_string())
            }
            MiniError::Eval(e) => {
                // Eval errors carry no spans in B1; they print terse.
                render(source, None, "eval error", &e.to_string())
            }
        }
    }
}
