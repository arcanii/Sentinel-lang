//! Lexer for Sentinel-Mini (B0).
//!
//! Uses [`logos`] for token recognition. Returns a flat `Vec<Token>` so
//! the hand-written parser can index into it freely (recovery and span
//! tracking are deliberately minimal at B0; both will get attention in
//! later phases).

use logos::Logos;
use thiserror::Error;

/// All tokens recognised by the B0 lexer.
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
    #[token(",")]
    Comma,
    #[token("=>")]
    FatArrow,
}

/// Errors produced by [`lex`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LexError {
    #[error("unrecognised token at byte offset {offset}: {snippet:?}")]
    Unrecognised { offset: usize, snippet: String },
}

/// Tokenise `source` into a flat vector of tokens.
///
/// Whitespace and `// line comments` are skipped. The first unrecognised
/// character produces [`LexError::Unrecognised`] and aborts lexing.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    let mut out = Vec::new();
    let mut lx = Token::lexer(source);
    while let Some(res) = lx.next() {
        match res {
            Ok(tok) => out.push(tok),
            Err(()) => {
                let span = lx.span();
                let snippet: String = source
                    .get(span.start..span.end)
                    .unwrap_or("")
                    .chars()
                    .take(16)
                    .collect();
                return Err(LexError::Unrecognised { offset: span.start, snippet });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_integers_and_arith() {
        let toks = lex("1 + 2 * 3").unwrap();
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
        let toks = lex("let lettuce = true in lettuce").unwrap();
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
        let toks = lex("fn(x) => x + 1").unwrap();
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
        let toks = lex("// hello\n1 + 2 // trailing").unwrap();
        assert_eq!(toks, vec![Token::Int(1), Token::Plus, Token::Int(2)]);
    }

    #[test]
    fn lex_unrecognised_char() {
        let err = lex("1 + @").unwrap_err();
        match err {
            LexError::Unrecognised { offset, .. } => assert_eq!(offset, 4),
        }
    }
}
