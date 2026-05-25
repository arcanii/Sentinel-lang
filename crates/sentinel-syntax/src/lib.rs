//! sentinel-syntax
//!
//! Lexer, parser, and concrete syntax tree for Sentinel source.
//!
//! C0.0 ships the lexer (see [`lex`]). Parser and AST lowering arrive
//! in C0.1 per ADR 0009 D6.

mod lexer;
pub use lexer::{lex, LexError, Span, Spanned, TokenKind};

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
