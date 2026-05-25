//! Lexer for Sentinel-Mini.
//!
//! Uses [`logos`] for token recognition. Returns a flat `Vec<(Token, Span)>`
//! so downstream stages can attach source positions to AST nodes and
//! diagnostics. The hand-written parser indexes into this vector directly.

use crate::span::Span;
use logos::Logos;
use thiserror::Error;

/// All tokens recognised by the lexer.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\r\f]+")]
#[logos(skip r"//[^\n]*")]
pub enum Token {
    // Literals
    #[regex(r"-?[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    Int(i64),

    #[token("true", |_| true)]
    #[token("false", |_| false)]
    Bool(bool),

    // Keywords
    #[token("let")]
    Let,
    #[token("rec")]
    Rec,
    #[token("in")]
    In,
    #[token("if")]
    If,
    #[token("then")]
    Then,
    #[token("else")]
    Else,
    #[token("fn")]
    Fn,

    // Identifiers (after keywords so keywords match first)
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_owned())]
    Ident(String),

    // Punctuation
    #[token("=")]
    Eq,
    #[token("==")]
    EqEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    // B3.0: braces for `handle e with { ... }`.
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token("=>")]
    FatArrow,
    // B2.2: effect-surface punctuation and keywords.
    #[token(":")]
    Colon,
    #[token(";")]
    Semicolon,
    #[token("->")]
    Arrow,
    #[token("effect")]
    Effect,
    #[token("do")]
    Do,
    // B3.0: handler-surface keywords.
    #[token("handle")]
    Handle,
    #[token("with")]
    With,
    #[token("return")]
    Return,
    // B4.0 (ADR 0008 D6): secret qualifier and declassify keywords.
    // Both are globally reserved; `secret` and `declassify` cannot be
    // used as identifiers anywhere in the source.
    #[token("secret")]
    Secret,
    #[token("declassify")]
    Declassify,
}

/// Errors produced by [`lex`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LexError {
    #[error("unrecognised token at bytes {}..{}: {snippet:?}", span.start, span.end)]
    Unrecognised { span: Span, snippet: String },
}

/// Tokenise `source` into a flat vector of `(Token, Span)` pairs.
pub fn lex(source: &str) -> Result<Vec<(Token, Span)>, LexError> {
    let mut out: Vec<(Token, Span)> = Vec::new();
    let mut lx = Token::lexer(source);
    while let Some(res) = lx.next() {
        match res {
            Ok(tok) => {
                let r = lx.span();
                out.push((tok, Span::new(r.start, r.end)));
            }
            Err(()) => {
                let r = lx.span();
                let snippet: String = source
                    .get(r.start..r.end)
                    .unwrap_or("")
                    .chars()
                    .take(16)
                    .collect();
                return Err(LexError::Unrecognised {
                    span: Span::new(r.start, r.end),
                    snippet,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens_only(v: Vec<(Token, Span)>) -> Vec<Token> {
        v.into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn lex_integers_and_arith() {
        let toks = tokens_only(lex("1 + 2 * 3").unwrap());
        assert_eq!(
            toks,
            vec![
                Token::Int(1),
                Token::Plus,
                Token::Int(2),
                Token::Star,
                Token::Int(3),
            ]
        );
    }

    #[test]
    fn lex_keywords_vs_idents() {
        let toks = tokens_only(lex("let lettuce = true in lettuce").unwrap());
        assert_eq!(
            toks,
            vec![
                Token::Let,
                Token::Ident("lettuce".into()),
                Token::Eq,
                Token::Bool(true),
                Token::In,
                Token::Ident("lettuce".into()),
            ]
        );
    }

    #[test]
    fn lex_lambda_syntax() {
        let toks = tokens_only(lex("fn(x) => x + 1").unwrap());
        assert_eq!(
            toks,
            vec![
                Token::Fn,
                Token::LParen,
                Token::Ident("x".into()),
                Token::RParen,
                Token::FatArrow,
                Token::Ident("x".into()),
                Token::Plus,
                Token::Int(1),
            ]
        );
    }

    #[test]
    fn lex_skips_line_comments() {
        let toks = tokens_only(lex("// hello\n1 + 2 // trailing").unwrap());
        assert_eq!(toks, vec![Token::Int(1), Token::Plus, Token::Int(2)]);
    }

    #[test]
    fn lex_unrecognised_char() {
        let err = lex("1 + @").unwrap_err();
        match err {
            LexError::Unrecognised { span, snippet } => {
                assert_eq!(span, Span::new(4, 5));
                assert_eq!(snippet, "@");
            }
        }
    }

    #[test]
    fn lex_records_spans() {
        let v = lex("1 + 2").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], (Token::Int(1), Span::new(0, 1)));
        assert_eq!(v[1], (Token::Plus, Span::new(2, 3)));
        assert_eq!(v[2], (Token::Int(2), Span::new(4, 5)));
    }

    #[test]
    fn lex_let_rec_keyword() {
        let toks = tokens_only(lex("let rec f = fn(n) => n").unwrap());
        assert_eq!(toks[0], Token::Let);
        assert_eq!(toks[1], Token::Rec);
        assert_eq!(toks[2], Token::Ident("f".into()));
    }

    #[test]
    fn lex_rec_is_reserved_not_ident() {
        // After B1.3, `rec` cannot be used as an identifier anywhere.
        // This is the trade-off accepted in the B1 scope discussion.
        let toks = tokens_only(lex("rec").unwrap());
        assert_eq!(toks, vec![Token::Rec]);
    }

    // ---- B2.2a: effect-surface tokens ----

    #[test]
    fn b22a_lex_effect_keyword() {
        let toks = tokens_only(lex("effect").unwrap());
        assert_eq!(toks, vec![Token::Effect]);
    }

    #[test]
    fn b22a_lex_do_keyword() {
        let toks = tokens_only(lex("do").unwrap());
        assert_eq!(toks, vec![Token::Do]);
    }

    #[test]
    fn b22a_lex_colon_and_semicolon() {
        let toks = tokens_only(lex(": ;").unwrap());
        assert_eq!(toks, vec![Token::Colon, Token::Semicolon]);
    }

    #[test]
    fn b22a_lex_arrow_distinct_from_minus() {
        let toks = tokens_only(lex("a -> b - c").unwrap());
        assert_eq!(toks, vec![
            Token::Ident("a".into()),
            Token::Arrow,
            Token::Ident("b".into()),
            Token::Minus,
            Token::Ident("c".into()),
        ]);
    }

    #[test]
    fn b22a_lex_effect_decl_full_token_stream() {
        let toks = tokens_only(lex("effect Print : Int -> Bool ;").unwrap());
        assert_eq!(toks, vec![
            Token::Effect,
            Token::Ident("Print".into()),
            Token::Colon,
            Token::Ident("Int".into()),
            Token::Arrow,
            Token::Ident("Bool".into()),
            Token::Semicolon,
        ]);
    }

    #[test]
    fn b30_lex_handle_with_return_keywords() {
        let toks = lex("handle e with { return v => v }").unwrap();
        let kinds: Vec<&Token> = toks.iter().map(|(t, _)| t).collect();
        assert!(matches!(kinds[0], Token::Handle));
        assert!(matches!(kinds[1], Token::Ident(s) if s == "e"));
        assert!(matches!(kinds[2], Token::With));
        assert!(matches!(kinds[4], Token::Return));
        assert!(matches!(kinds[5], Token::Ident(s) if s == "v"));
        assert!(matches!(kinds[6], Token::FatArrow));
    }

    // ---- B4.0b: secret + declassify keyword tokens (ADR 0008 D6) ----

    #[test]
    fn b40b_lex_secret_keyword() {
        let toks = tokens_only(lex("secret").unwrap());
        assert_eq!(toks, vec![Token::Secret]);
    }

    #[test]
    fn b40b_lex_declassify_keyword() {
        let toks = tokens_only(lex("declassify").unwrap());
        assert_eq!(toks, vec![Token::Declassify]);
    }

    #[test]
    fn b40b_lex_secret_is_reserved_not_ident() {
        // After B4.0b, `secret` cannot be used as an identifier.
        // Same global-reservation policy as `rec` (B1.3) / `handle`,
        // `with`, `return` (B3.0).
        let toks = tokens_only(lex("let secret = 1 in secret").unwrap());
        assert_eq!(toks, vec![
            Token::Let,
            Token::Secret,
            Token::Eq,
            Token::Int(1),
            Token::In,
            Token::Secret,
        ]);
    }

    #[test]
    fn b40b_lex_declassify_is_reserved_not_ident() {
        // Used in an identifier position; lexer still emits the keyword.
        let toks = tokens_only(lex("let declassify = 1 in declassify").unwrap());
        assert_eq!(toks, vec![
            Token::Let,
            Token::Declassify,
            Token::Eq,
            Token::Int(1),
            Token::In,
            Token::Declassify,
        ]);
    }

    #[test]
    fn b40b_lex_secret_type_annotation_stream() {
        // `secret Bytes` (Bytes is just an Ident at the lexer level;
        // type-atom recognition happens in the parser).
        let toks = tokens_only(lex("secret Bytes").unwrap());
        assert_eq!(toks, vec![
            Token::Secret,
            Token::Ident("Bytes".into()),
        ]);
    }

    #[test]
    fn b40b_lex_declassify_call_stream() {
        let toks = tokens_only(lex("declassify(x)").unwrap());
        assert_eq!(toks, vec![
            Token::Declassify,
            Token::LParen,
            Token::Ident("x".into()),
            Token::RParen,
        ]);
    }

    #[test]
    fn b22a_lex_do_invocation() {
        let toks = tokens_only(lex("do Print(1)").unwrap());
        assert_eq!(toks, vec![
            Token::Do,
            Token::Ident("Print".into()),
            Token::LParen,
            Token::Int(1),
            Token::RParen,
        ]);
    }
}
