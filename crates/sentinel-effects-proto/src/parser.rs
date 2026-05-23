//! Hand-written recursive-descent parser for Sentinel-Mini.

use crate::ast::{expr, BinOp, Expr, ExprKind};
use crate::lexer::Token;
use crate::span::Span;
use thiserror::Error;

/// Errors produced by [`parse`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ParseError {
    #[error("unexpected end of input; expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("unexpected token {found:?}; expected {expected}")]
    Unexpected { found: Token, expected: &'static str, span: Span },
    #[error("trailing tokens after expression: {found:?}")]
    Trailing { found: Token, span: Span },
    #[error("'let rec' requires a lambda on the right-hand side; got something else")]
    LetRecNotLambda { span: Span },
}

/// Parse a flat token stream into a single [`Expr`].
pub fn parse(tokens: &[(Token, Span)]) -> Result<Expr, ParseError> {
    let mut p = Parser { toks: tokens, pos: 0 };
    let e = p.parse_expr()?;
    if let Some((t, span)) = p.peek().cloned() {
        return Err(ParseError::Trailing { found: t, span });
    }
    Ok(e)
}

struct Parser<'a> {
    toks: &'a [(Token, Span)],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&(Token, Span)> {
        self.toks.get(self.pos)
    }

    fn peek_tok(&self) -> Option<&Token> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn peek_tok_at(&self, offset: usize) -> Option<&Token> {
        self.toks.get(self.pos + offset).map(|(t, _)| t)
    }

    fn peek_span(&self) -> Option<Span> {
        self.toks.get(self.pos).map(|(_, s)| *s)
    }

    fn last_span(&self) -> Span {
        let idx = self.pos.saturating_sub(1);
        self.toks
            .get(idx)
            .map(|(_, s)| *s)
            .unwrap_or_else(|| Span::point(0))
    }

    fn bump(&mut self) -> Option<(Token, Span)> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(
        &mut self,
        expected: &'static str,
        pred: impl Fn(&Token) -> bool,
    ) -> Result<(Token, Span), ParseError> {
        match self.peek().cloned() {
            None => Err(ParseError::UnexpectedEof { expected }),
            Some((t, _)) if pred(&t) => Ok(self.bump().unwrap()),
            Some((t, span)) => Err(ParseError::Unexpected { found: t, expected, span }),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek_tok() {
            Some(Token::Let) => {
                // Disambiguate `let` vs `let rec` by one-token lookahead.
                if matches!(self.peek_tok_at(1), Some(Token::Rec)) {
                    self.parse_let_rec()
                } else {
                    self.parse_let()
                }
            }
            Some(Token::If) => self.parse_if(),
            Some(Token::Fn) => self.parse_lambda(),
            _ => self.parse_compare(),
        }
    }

    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span().expect("parse_let called with no 'let' token");
        self.bump(); // 'let'
        let name = match self.bump() {
            Some((Token::Ident(s), _)) => s,
            Some((t, span)) => {
                return Err(ParseError::Unexpected { found: t, expected: "identifier", span })
            }
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("'='", |t| matches!(t, Token::Eq))?;
        let value = Box::new(self.parse_expr()?);
        self.expect("'in'", |t| matches!(t, Token::In))?;
        let body = Box::new(self.parse_expr()?);
        let span = start.merge(body.span);
        Ok(expr(ExprKind::Let { name, value, body }, span))
    }

    fn parse_let_rec(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span().expect("parse_let_rec called with no 'let' token");
        self.bump(); // 'let'
        self.bump(); // 'rec'
        let name = match self.bump() {
            Some((Token::Ident(s), _)) => s,
            Some((t, span)) => {
                return Err(ParseError::Unexpected { found: t, expected: "identifier", span })
            }
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("'='", |t| matches!(t, Token::Eq))?;
        // Require a lambda on the RHS at parse time. We accept any expression
        // and then check; this gives a precise span when it fails.
        let value = self.parse_expr()?;
        if !matches!(value.node, ExprKind::Lambda { .. }) {
            return Err(ParseError::LetRecNotLambda { span: value.span });
        }
        let value = Box::new(value);
        self.expect("'in'", |t| matches!(t, Token::In))?;
        let body = Box::new(self.parse_expr()?);
        let span = start.merge(body.span);
        Ok(expr(ExprKind::LetRec { name, value, body }, span))
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span().expect("parse_if called with no 'if' token");
        self.bump(); // 'if'
        let cond = Box::new(self.parse_expr()?);
        self.expect("'then'", |t| matches!(t, Token::Then))?;
        let then_branch = Box::new(self.parse_expr()?);
        self.expect("'else'", |t| matches!(t, Token::Else))?;
        let else_branch = Box::new(self.parse_expr()?);
        let span = start.merge(else_branch.span);
        Ok(expr(ExprKind::If { cond, then_branch, else_branch }, span))
    }

    fn parse_lambda(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span().expect("parse_lambda called with no 'fn' token");
        self.bump(); // 'fn'
        self.expect("'('", |t| matches!(t, Token::LParen))?;
        let param = match self.bump() {
            Some((Token::Ident(s), _)) => s,
            Some((t, span)) => {
                return Err(ParseError::Unexpected { found: t, expected: "identifier", span })
            }
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("')'", |t| matches!(t, Token::RParen))?;
        self.expect("'=>'", |t| matches!(t, Token::FatArrow))?;
        let body = Box::new(self.parse_expr()?);
        let span = start.merge(body.span);
        Ok(expr(ExprKind::Lambda { param, body }, span))
    }

    fn parse_compare(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.peek_tok() {
            Some(Token::EqEq) => BinOp::Eq,
            Some(Token::Lt) => BinOp::Lt,
            Some(Token::Gt) => BinOp::Gt,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_add()?;
        let span = lhs.span.merge(rhs.span);
        Ok(expr(ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span))
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek_tok() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            let span = lhs.span.merge(rhs.span);
            lhs = expr(ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span);
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_app()?;
        loop {
            let op = match self.peek_tok() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_app()?;
            let span = lhs.span.merge(rhs.span);
            lhs = expr(ExprKind::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span);
        }
        Ok(lhs)
    }

    fn parse_app(&mut self) -> Result<Expr, ParseError> {
        let mut callee = self.parse_atom()?;
        while matches!(self.peek_tok(), Some(Token::LParen)) {
            self.bump();
            let arg = self.parse_expr()?;
            self.expect("')'", |t| matches!(t, Token::RParen))?;
            let close_span = self.last_span();
            let span = callee.span.merge(close_span);
            callee = expr(
                ExprKind::App { callee: Box::new(callee), arg: Box::new(arg) },
                span,
            );
        }
        Ok(callee)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Some((Token::Int(n), span)) => Ok(expr(ExprKind::Int(n), span)),
            Some((Token::Bool(b), span)) => Ok(expr(ExprKind::Bool(b), span)),
            Some((Token::Ident(s), span)) => Ok(expr(ExprKind::Var(s), span)),
            Some((Token::LParen, open_span)) => {
                let inner = self.parse_expr()?;
                self.expect("')'", |t| matches!(t, Token::RParen))?;
                let close_span = self.last_span();
                let span = open_span.merge(close_span);
                Ok(expr(inner.node, span))
            }
            Some((t, span)) => Err(ParseError::Unexpected { found: t, expected: "atom", span }),
            None => Err(ParseError::UnexpectedEof { expected: "atom" }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn p(src: &str) -> Expr {
        parse(&lex(src).unwrap()).unwrap()
    }

    #[test]
    fn parses_integer_literal() {
        let e = p("42");
        assert_eq!(e.node, ExprKind::Int(42));
        assert_eq!(e.span, Span::new(0, 2));
    }

    #[test]
    fn arithmetic_precedence() {
        let e = p("1 + 2 * 3");
        match e.node {
            ExprKind::BinOp { op: BinOp::Add, lhs, rhs } => {
                assert_eq!(lhs.node, ExprKind::Int(1));
                match rhs.node {
                    ExprKind::BinOp { op: BinOp::Mul, .. } => {}
                    other => panic!("rhs not mul: {other:?}"),
                }
            }
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn parses_let_in() {
        let e = p("let x = 1 in x + 2");
        match e.node {
            ExprKind::Let { name, .. } => assert_eq!(name, "x"),
            other => panic!("not a let: {other:?}"),
        }
    }

    #[test]
    fn parses_lambda_and_application() {
        let e = p("fn(x) => x + 1");
        assert!(matches!(e.node, ExprKind::Lambda { .. }));
        let e = p("(fn(x) => x + 1)(41)");
        assert!(matches!(e.node, ExprKind::App { .. }));
    }

    #[test]
    fn parse_error_on_missing_then() {
        let err = parse(&lex("if true 1 else 2").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::Unexpected { expected: "'then'", .. }));
    }

    #[test]
    fn spans_cover_full_source() {
        let src = "let x = 1 in x + 2";
        let e = p(src);
        assert_eq!(e.span, Span::new(0, src.len()));
    }

    #[test]
    fn binop_span_merges_children() {
        let e = p("1 + 2");
        assert_eq!(e.span, Span::new(0, 5));
        match e.node {
            ExprKind::BinOp { lhs, rhs, .. } => {
                assert_eq!(lhs.span, Span::new(0, 1));
                assert_eq!(rhs.span, Span::new(4, 5));
            }
            other => panic!("expected binop, got {other:?}"),
        }
    }

    #[test]
    fn application_span_covers_through_closing_paren() {
        let e = p("f(x)");
        assert_eq!(e.span, Span::new(0, 4));
    }

    #[test]
    fn parses_let_rec_with_lambda_rhs() {
        let src = "let rec f = fn(n) => n in f(1)";
        let e = p(src);
        match e.node {
            ExprKind::LetRec { name, value, body } => {
                assert_eq!(name, "f");
                assert!(matches!(value.node, ExprKind::Lambda { .. }));
                assert!(matches!(body.node, ExprKind::App { .. }));
            }
            other => panic!("expected let rec, got {other:?}"),
        }
    }

    #[test]
    fn let_rec_rejects_non_lambda_rhs() {
        // `let rec x = 1 in x` is a parse error.
        let err = parse(&lex("let rec x = 1 in x").unwrap()).unwrap_err();
        match err {
            ParseError::LetRecNotLambda { span } => {
                // The span points at the offending RHS (`1`), which starts at byte 12.
                assert_eq!(span, Span::new(12, 13));
            }
            other => panic!("expected LetRecNotLambda, got {other:?}"),
        }
    }

    #[test]
    fn plain_let_still_works_alongside_let_rec() {
        // `let` without `rec` must still parse as ExprKind::Let.
        let e = p("let x = 1 in x");
        assert!(matches!(e.node, ExprKind::Let { .. }));
    }
}
