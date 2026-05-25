//! sentinel-syntax
//!
//! Lexer, parser, and concrete syntax tree for Sentinel source.
//!
//! C0.0 ships the lexer (see [`lex`]); C0.1 adds the hand-written
//! recursive descent parser (see [`parse`]) per ADR 0009 D4 and
//! ADR 0010.
//!
//! `Span` and `Spanned<T>` are re-exported from `sentinel-ast` for
//! the convenience of crates that consume both lexer output and AST
//! nodes.

mod lexer;
mod parser;

pub use lexer::{lex, LexError, TokenKind};
pub use parser::{parse, parse_expr, ParseError, Parser};
pub use sentinel_ast::{Program, Span, Spanned, Stmt, StmtKind};

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-syntax"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-syntax");
    }
}
