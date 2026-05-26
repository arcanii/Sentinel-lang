//! Lexer for Sentinel C0 / C1.
//!
//! Token set (C0): keywords (`let`, `fn`, `if`, `else`), identifiers,
//! decimal integer literals, arithmetic operators (`+ - * /`), `=`,
//! parens, braces, comma, semicolon, `->`.
//!
//! C1.2 added `:` (per ADR 0012 D9).
//!
//! C1.3 adds (per ADR 0012 D9): `true` / `false` keywords, six
//! comparison operators (`== != < <= > >=`), and three logical
//! operators (`&& || !`). logos's longest-match guarantee handles
//! the precedence-aware lexing — `!=` lexes as a single token before
//! `!`, `<=` before `<`, `>=` before `>`, listed below in
//! longer-first order accordingly.
//!
//! C1.4 adds (per ADR 0013 D8): `struct` keyword and `.` token for
//! postfix field access. No longest-match concerns at C1.4 — `.`
//! has no `..` / `.=` / `...` neighbours until ranges arrive in C2+.
//!
//! Whitespace and `//` line comments are skipped. ADR 0009 D4 picks
//! hand-written recursive descent for the parser; the lexer uses
//! `logos` because the regex-DFA payoff is purely positive at this
//! scale.
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
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("struct")]
    Struct,

    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    // `==` must lex before `=`; logos's longest-match makes this
    // automatic but ordering here mirrors the intent.
    #[token("==")]
    EqEq,
    #[token("=")]
    Eq,
    #[token("!=")]
    BangEq,
    #[token("!")]
    Bang,
    #[token("<=")]
    LtEq,
    #[token("<")]
    Lt,
    #[token(">=")]
    GtEq,
    #[token(">")]
    Gt,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
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
    #[token(".")]
    Dot,
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
            kinds("( ) { } , ; : . ->"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Comma,
                TokenKind::Semi,
                TokenKind::Colon,
                TokenKind::Dot,
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
            kinds("let fn if else true false struct"),
            vec![
                TokenKind::Let,
                TokenKind::Fn,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Struct,
            ]
        );
    }

    #[test]
    fn lex_struct_keyword_distinct_from_ident_prefix() {
        // `structure`, `structured` must NOT lex as Struct + Ident.
        assert_eq!(
            kinds("structure structured"),
            vec![TokenKind::Ident, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_dot_token() {
        assert_eq!(kinds("."), vec![TokenKind::Dot]);
    }

    #[test]
    fn lex_field_access_shape() {
        // The shape the parser will see for `p.x`.
        assert_eq!(
            kinds("p.x"),
            vec![TokenKind::Ident, TokenKind::Dot, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_struct_literal_shape() {
        // `Foo { x: 1, y: 2 }`
        assert_eq!(
            kinds("Foo { x: 1, y: 2 }"),
            vec![
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::IntLit,
                TokenKind::Comma,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::IntLit,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_bool_literals_distinct_from_ident_prefixes() {
        // `truely`, `falsehood` must NOT be parsed as keyword + ident.
        assert_eq!(
            kinds("truely falsehood"),
            vec![TokenKind::Ident, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_all_comparison_ops() {
        assert_eq!(
            kinds("== != < <= > >="),
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::Lt,
                TokenKind::LtEq,
                TokenKind::Gt,
                TokenKind::GtEq,
            ]
        );
    }

    #[test]
    fn lex_all_logical_ops() {
        assert_eq!(
            kinds("&& || !"),
            vec![TokenKind::AmpAmp, TokenKind::PipePipe, TokenKind::Bang]
        );
    }

    #[test]
    fn lex_longest_match_eq_vs_eqeq() {
        // `==` is one token; `= =` is two.
        assert_eq!(kinds("=="), vec![TokenKind::EqEq]);
        assert_eq!(kinds("= ="), vec![TokenKind::Eq, TokenKind::Eq]);
    }

    #[test]
    fn lex_longest_match_bang_vs_bangeq() {
        // `!=` is one token; `! =` is two.
        assert_eq!(kinds("!="), vec![TokenKind::BangEq]);
        assert_eq!(kinds("! ="), vec![TokenKind::Bang, TokenKind::Eq]);
    }

    #[test]
    fn lex_longest_match_lt_vs_lteq() {
        assert_eq!(kinds("<="), vec![TokenKind::LtEq]);
        assert_eq!(kinds("< ="), vec![TokenKind::Lt, TokenKind::Eq]);
    }

    #[test]
    fn lex_longest_match_gt_vs_gteq() {
        assert_eq!(kinds(">="), vec![TokenKind::GtEq]);
        assert_eq!(kinds("> ="), vec![TokenKind::Gt, TokenKind::Eq]);
    }

    #[test]
    fn lex_logical_ops_packed_against_atoms() {
        // No whitespace between operands and operators — common case.
        assert_eq!(
            kinds("a&&b||!c"),
            vec![
                TokenKind::Ident,
                TokenKind::AmpAmp,
                TokenKind::Ident,
                TokenKind::PipePipe,
                TokenKind::Bang,
                TokenKind::Ident,
            ]
        );
    }

    #[test]
    fn lex_comparison_in_expression_context() {
        // The kind of source the C1.3 parser will see.
        assert_eq!(
            kinds("if x != 0"),
            vec![
                TokenKind::If,
                TokenKind::Ident,
                TokenKind::BangEq,
                TokenKind::IntLit,
            ]
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
