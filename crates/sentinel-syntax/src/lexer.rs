//! Lexer for Sentinel C0.
//!
//! Token set: keywords (`let`, `fn`, `if`, `else`), identifiers, decimal
//! integer literals, arithmetic operators (`+ - * /`), `=`, parens,
//! braces, comma, semicolon, `->`. Whitespace and `//` line comments are
//! skipped. ADR 0009 D4 picks hand-written recursive descent for the
//! parser; the lexer uses `logos` because the regex-DFA payoff is
//! purely positive at this scale.
//!
//! Pure function per ADR 0009 D1a: `lex` returns the token stream and
//! a Vec of diagnostics; no shared mutable state.

use logos::Logos;
use sentinel_ast::Spanned;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip r"//[^\n]*")]
pub enum TokenKind {
    #[token("let")]
    Let,
    #[token("fn")]
    Fn,
    #[token("if")]
    If,
    #[token("else")]
    Else,

    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("=")]
    Eq,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(";")]
    Semi,
    #[token(":")]
    Colon,
    #[token("->")]
    Arrow,

    #[regex(r"[0-9]+")]
    IntLit,

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,
}

#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum LexError {
    #[error("invalid character `{ch}` in source")]
    #[diagnostic(
        code(sentinel::lex::invalid_char),
        help("remove the character or replace it with a valid token")
    )]
    InvalidChar {
        ch: char,
        #[label("not a valid token")]
        span: miette::SourceSpan,
    },
}

pub fn lex(src: &str) -> (Vec<Spanned<TokenKind>>, Vec<LexError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut lexer = TokenKind::lexer(src);
    while let Some(result) = lexer.next() {
        let span = lexer.span();
        match result {
            Ok(kind) => tokens.push(Spanned { kind, span }),
            Err(()) => {
                let ch = src[span.clone()].chars().next().unwrap_or('\0');
                errors.push(LexError::InvalidChar {
                    ch,
                    span: (span.start, span.len()).into(),
                });
            }
        }
    }
    (tokens, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, errs) = lex(src);
        assert!(errs.is_empty(), "expected no lex errors, got {errs:?}");
        toks.iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lex_let_arithmetic() {
        assert_eq!(
            kinds("let x = 1 + 2;"),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::IntLit,
                TokenKind::Plus,
                TokenKind::IntLit,
                TokenKind::Semi,
            ]
        );
    }

    #[test]
    fn lex_full_arithmetic_set() {
        assert_eq!(
            kinds("+ - * /"),
            vec![TokenKind::Plus, TokenKind::Minus, TokenKind::Star, TokenKind::Slash]
        );
    }

    #[test]
    fn lex_all_punctuation() {
        assert_eq!(
            kinds("( ) { } , ; : ->"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Comma,
                TokenKind::Semi,
                TokenKind::Colon,
                TokenKind::Arrow,
            ]
        );
    }

    #[test]
    fn lex_colon_in_annotation_position() {
        // Annotation grammar (C1.2 per ADR 0012 D1): `Ident ':' type`.
        assert_eq!(
            kinds("x: i64"),
            vec![TokenKind::Ident, TokenKind::Colon, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_all_keywords() {
        assert_eq!(
            kinds("let fn if else"),
            vec![TokenKind::Let, TokenKind::Fn, TokenKind::If, TokenKind::Else]
        );
    }

    #[test]
    fn lex_keyword_prefix_is_ident() {
        // `letter`, `fnord`, etc. must NOT be parsed as keyword + ident.
        assert_eq!(
            kinds("letter fnord ifeq elsewhere"),
            vec![TokenKind::Ident, TokenKind::Ident, TokenKind::Ident, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_skips_line_comment() {
        assert_eq!(kinds("// hello\nlet x"), vec![TokenKind::Let, TokenKind::Ident]);
    }

    #[test]
    fn lex_skips_mixed_whitespace() {
        assert_eq!(kinds("let\tx\r\n=\n1"), vec![
            TokenKind::Let,
            TokenKind::Ident,
            TokenKind::Eq,
            TokenKind::IntLit,
        ]);
    }

    #[test]
    fn lex_spans_track_positions() {
        let (toks, _) = lex("let x");
        assert_eq!(toks[0].span, 0..3); // "let"
        assert_eq!(toks[1].span, 4..5); // "x"
    }

    #[test]
    fn lex_reports_invalid_char() {
        let (toks, errs) = lex("let x = @");
        // Valid tokens still come through.
        assert_eq!(
            toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![TokenKind::Let, TokenKind::Ident, TokenKind::Eq]
        );
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            LexError::InvalidChar { ch, .. } => assert_eq!(*ch, '@'),
        }
    }

    #[test]
    fn lex_empty_source() {
        let (toks, errs) = lex("");
        assert!(toks.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn lex_only_whitespace_and_comments() {
        let (toks, errs) = lex("  // comment\n  \t  ");
        assert!(toks.is_empty());
        assert!(errs.is_empty());
    }
}
