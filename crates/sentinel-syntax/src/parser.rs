//! Hand-written recursive descent parser for Sentinel C0.1.
//!
//! Implements the strict expression-only subset of ADR 0010 D8:
//!
//! ```text
//! expr     = add_expr
//! add_expr = mul_expr (('+' | '-') mul_expr)*       left-assoc
//! mul_expr = unary    (('*' | '/') unary)*          left-assoc
//! unary    = '-' unary | atom
//! atom     = IntLit | '(' expr ')'
//! ```
//!
//! Identifiers, calls, `let`, `if`, and `fn` arrive in C0.3-0.5
//! per ADR 0009 D6. Per ADR 0009 D1a, `parse` is a pure function:
//! input is the source string, output is `Result<Expr, ParseError>`.
//! Lex errors are surfaced as a transparent `ParseError::Lex` variant
//! so a single error type carries every diagnostic the front-end can
//! produce.

use sentinel_ast::{BinOp, Expr, ExprKind, Span, Spanned, UnaryOp};

use crate::{lex, LexError, TokenKind};

#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum ParseError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Lex(#[from] LexError),

    #[error("unexpected {got}, expected {expected}")]
    #[diagnostic(code(sentinel::parse::unexpected_token))]
    UnexpectedToken {
        got: String,
        expected: &'static str,
        #[label("expected {expected}")]
        span: miette::SourceSpan,
    },

    #[error("unexpected end of input, expected {expected}")]
    #[diagnostic(code(sentinel::parse::unexpected_eof))]
    UnexpectedEof {
        expected: &'static str,
        #[label("expected {expected} here")]
        span: miette::SourceSpan,
    },

    #[error("unmatched opening parenthesis")]
    #[diagnostic(
        code(sentinel::parse::unmatched_paren),
        help("add a matching `)` to close the parenthesized expression")
    )]
    UnmatchedParen {
        #[label("this `(` is never closed")]
        open_span: miette::SourceSpan,
    },

    #[error("integer literal `{text}` does not fit in i64")]
    #[diagnostic(code(sentinel::parse::int_lit_overflow))]
    IntLitOverflow {
        text: String,
        #[label("does not fit in i64")]
        span: miette::SourceSpan,
    },
}

/// Parse a Sentinel C0.1 source string into a single expression.
///
/// Lex errors block parsing and are surfaced as `ParseError::Lex`.
/// Per ADR 0009 D1a the function takes only its input and returns
/// its output; no shared mutable state.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let (tokens, lex_errs) = lex(src);
    if let Some(err) = lex_errs.into_iter().next() {
        return Err(ParseError::from(err));
    }
    let mut p = Parser::new(src, &tokens);
    p.parse_top()
}

pub struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Spanned<TokenKind>],
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, tokens: &'a [Spanned<TokenKind>]) -> Self {
        Self { src, tokens, pos: 0 }
    }

    pub fn parse_top(&mut self) -> Result<Expr, ParseError> {
        let e = self.parse_expr()?;
        if let Some(t) = self.peek() {
            return Err(ParseError::UnexpectedToken {
                got: format!("{:?}", t.kind),
                expected: "end of input",
                span: to_source_span(&t.span),
            });
        }
        Ok(e)
    }

    fn peek(&self) -> Option<&Spanned<TokenKind>> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    fn advance(&mut self) -> Option<&Spanned<TokenKind>> {
        let t = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(t)
    }

    fn eof_span(&self) -> Span {
        self.src.len()..self.src.len()
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_add()
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Plus) => BinOp::Add,
                Some(TokenKind::Minus) => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_mul()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Star) => BinOp::Mul,
                Some(TokenKind::Slash) => BinOp::Div,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.peek_kind() == Some(TokenKind::Minus) {
            let start = self.peek().expect("checked above").span.start;
            self.advance();
            let inner = self.parse_unary()?;
            let span = start..inner.span.end;
            return Ok(Spanned {
                kind: ExprKind::Unary(UnaryOp::Neg, Box::new(inner)),
                span,
            });
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::IntLit) => {
                let span = self.advance().expect("peeked").span.clone();
                let text = &self.src[span.clone()];
                let n: i64 = text.parse().map_err(|_| ParseError::IntLitOverflow {
                    text: text.to_string(),
                    span: to_source_span(&span),
                })?;
                Ok(Spanned { kind: ExprKind::IntLit(n), span })
            }
            Some(TokenKind::LParen) => {
                let open_span = self.advance().expect("peeked").span.clone();
                let inner = self.parse_expr()?;
                match self.peek_kind() {
                    Some(TokenKind::RParen) => {
                        let close = self.advance().expect("peeked");
                        let span = open_span.start..close.span.end;
                        Ok(Spanned { kind: inner.kind, span })
                    }
                    _ => Err(ParseError::UnmatchedParen {
                        open_span: to_source_span(&open_span),
                    }),
                }
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "expression",
                    span: to_source_span(&t.span),
                })
            }
            None => Err(ParseError::UnexpectedEof {
                expected: "expression",
                span: to_source_span(&self.eof_span()),
            }),
        }
    }
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Expr {
        parse(src).unwrap_or_else(|e| panic!("expected parse to succeed: {e:?}"))
    }

    fn pretty(src: &str) -> String {
        parse_ok(src).kind.to_string()
    }

    #[test]
    fn parse_int_lit() {
        assert_eq!(pretty("42"), "42");
    }

    #[test]
    fn parse_int_lit_zero() {
        assert_eq!(pretty("0"), "0");
    }

    #[test]
    fn parse_simple_add() {
        assert_eq!(pretty("1 + 2"), "(+ 1 2)");
    }

    #[test]
    fn parse_precedence_mul_higher_than_add() {
        assert_eq!(pretty("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(pretty("1 * 2 + 3"), "(+ (* 1 2) 3)");
    }

    #[test]
    fn parse_left_assoc_add() {
        assert_eq!(pretty("1 - 2 - 3"), "(- (- 1 2) 3)");
    }

    #[test]
    fn parse_left_assoc_mul() {
        assert_eq!(pretty("8 / 4 / 2"), "(/ (/ 8 4) 2)");
    }

    #[test]
    fn parse_parens_override_precedence() {
        assert_eq!(pretty("(1 + 2) * 3"), "(* (+ 1 2) 3)");
    }

    #[test]
    fn parse_unary_minus() {
        assert_eq!(pretty("-5"), "(- 5)");
    }

    #[test]
    fn parse_unary_double() {
        assert_eq!(pretty("--5"), "(- (- 5))");
    }

    #[test]
    fn parse_unary_binds_tighter_than_mul() {
        assert_eq!(pretty("-2 * 3"), "(* (- 2) 3)");
    }

    #[test]
    fn parse_unary_after_minus() {
        // `1 - -2` is `(- 1 (- 2))`. The first `-` is binary because
        // there is a left operand; the second `-` is unary.
        assert_eq!(pretty("1 - -2"), "(- 1 (- 2))");
    }

    #[test]
    fn parse_all_four_operators() {
        assert_eq!(pretty("1 + 2 - 3 * 4 / 5"), "(- (+ 1 2) (/ (* 3 4) 5))");
    }

    #[test]
    fn parse_nested_parens() {
        assert_eq!(pretty("(((1 + 2)))"), "(+ 1 2)");
    }

    #[test]
    fn parse_span_covers_full_expression() {
        let e = parse_ok("1 + 2");
        assert_eq!(e.span, 0..5);
    }

    #[test]
    fn parse_span_for_parenthesized_expr_includes_parens() {
        let e = parse_ok("(1 + 2)");
        assert_eq!(e.span, 0..7);
    }

    #[test]
    fn parse_span_for_unary_includes_operator() {
        let e = parse_ok("-5");
        assert_eq!(e.span, 0..2);
    }

    #[test]
    fn parse_error_unmatched_open_paren() {
        let err = parse("(1 + 2").unwrap_err();
        assert!(matches!(err, ParseError::UnmatchedParen { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_unexpected_close_paren() {
        let err = parse(")").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_eof_after_operator() {
        let err = parse("1 +").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_eof_after_unary() {
        let err = parse("-").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_lex_passthrough() {
        let err = parse("1 + @").unwrap_err();
        assert!(matches!(err, ParseError::Lex(_)), "got {err:?}");
    }

    #[test]
    fn parse_error_int_lit_overflow() {
        let err = parse("99999999999999999999").unwrap_err();
        assert!(matches!(err, ParseError::IntLitOverflow { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_trailing_garbage() {
        // The parser accepts an expression and then expects EOF;
        // `1 2` parses `1` as an expression but `2` is leftover.
        let err = parse("1 2").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { expected, .. } if expected == "end of input"));
    }
}
