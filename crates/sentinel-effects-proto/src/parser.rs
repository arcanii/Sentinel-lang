//! Hand-written recursive-descent parser for Sentinel-Mini (B0).
//!
//! Precedence climbing handles the binary operators. Single-pass; the
//! first error aborts parsing. Spans are not tracked yet (B0 keeps
//! diagnostics minimal); they will be added in B1 alongside the type
//! checker so error highlighting can be meaningful.

use crate::ast::{BinOp, Expr};
use crate::lexer::Token;
use thiserror::Error;

/// Errors produced by [`parse`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ParseError {
    #[error("unexpected end of input; expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("unexpected token {found:?}; expected {expected}")]
    Unexpected { found: Token, expected: &'static str },
    #[error("trailing tokens after expression: {found:?}")]
    Trailing { found: Token },
}

/// Parse a flat token stream into a single [`Expr`].
pub fn parse(tokens: &[Token]) -> Result<Expr, ParseError> {
    let mut p = Parser { toks: tokens, pos: 0 };
    let e = p.parse_expr()?;
    if let Some(t) = p.peek().cloned() {
        return Err(ParseError::Trailing { found: t });
    }
    Ok(e)
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, expected: &'static str, pred: impl Fn(&Token) -> bool) -> Result<Token, ParseError> {
        match self.peek() {
            None => Err(ParseError::UnexpectedEof { expected }),
            Some(t) if pred(t) => Ok(self.bump().unwrap()),
            Some(t) => Err(ParseError::Unexpected { found: t.clone(), expected }),
        }
    }

    // --- expression grammar ------------------------------------------------
    //
    //   expr      := let | if | lambda | binop
    //   let       := "let" IDENT "=" expr "in" expr
    //   if        := "if" expr "then" expr "else" expr
    //   lambda    := "fn" "(" IDENT ")" "=>" expr
    //   binop     := compare
    //   compare   := add ( ("==" | "<" | ">") add )?
    //   add       := mul ( ("+" | "-") mul )*
    //   mul       := app ( ("*" | "/") app )*
    //   app       := atom ( "(" expr ")" )*
    //   atom      := INT | BOOL | IDENT | "(" expr ")"

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Some(Token::Let) => self.parse_let(),
            Some(Token::If) => self.parse_if(),
            Some(Token::Fn) => self.parse_lambda(),
            _ => self.parse_compare(),
        }
    }

    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'let'
        let name = match self.bump() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(ParseError::Unexpected { found: t, expected: "identifier" }),
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("'='", |t| matches!(t, Token::Eq))?;
        let value = Box::new(self.parse_expr()?);
        self.expect("'in'", |t| matches!(t, Token::In))?;
        let body = Box::new(self.parse_expr()?);
        Ok(Expr::Let { name, value, body })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'if'
        let cond = Box::new(self.parse_expr()?);
        self.expect("'then'", |t| matches!(t, Token::Then))?;
        let then_branch = Box::new(self.parse_expr()?);
        self.expect("'else'", |t| matches!(t, Token::Else))?;
        let else_branch = Box::new(self.parse_expr()?);
        Ok(Expr::If { cond, then_branch, else_branch })
    }

    fn parse_lambda(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'fn'
        self.expect("'('", |t| matches!(t, Token::LParen))?;
        let param = match self.bump() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(ParseError::Unexpected { found: t, expected: "identifier" }),
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("')'", |t| matches!(t, Token::RParen))?;
        self.expect("'=>'", |t| matches!(t, Token::FatArrow))?;
        let body = Box::new(self.parse_expr()?);
        Ok(Expr::Lambda { param, body })
    }

    fn parse_compare(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.peek() {
            Some(Token::EqEq) => BinOp::Eq,
            Some(Token::Lt) => BinOp::Lt,
            Some(Token::Gt) => BinOp::Gt,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_add()?;
        Ok(Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) })
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_app()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_app()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_app(&mut self) -> Result<Expr, ParseError> {
        let mut callee = self.parse_atom()?;
        while matches!(self.peek(), Some(Token::LParen)) {
            self.bump(); // '('
            let arg = self.parse_expr()?;
            self.expect("')'", |t| matches!(t, Token::RParen))?;
            callee = Expr::App { callee: Box::new(callee), arg: Box::new(arg) };
        }
        Ok(callee)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Some(Token::Int(n)) => Ok(Expr::Int(n)),
            Some(Token::Bool(b)) => Ok(Expr::Bool(b)),
            Some(Token::Ident(s)) => Ok(Expr::Var(s)),
            Some(Token::LParen) => {
                let e = self.parse_expr()?;
                self.expect("')'", |t| matches!(t, Token::RParen))?;
                Ok(e)
            }
            Some(t) => Err(ParseError::Unexpected { found: t, expected: "atom" }),
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
        assert_eq!(p("42"), Expr::Int(42));
    }

    #[test]
    fn arithmetic_precedence() {
        // 1 + 2 * 3  parses as  1 + (2 * 3)
        let e = p("1 + 2 * 3");
        match e {
            Expr::BinOp { op: BinOp::Add, lhs, rhs } => {
                assert_eq!(*lhs, Expr::Int(1));
                match *rhs {
                    Expr::BinOp { op: BinOp::Mul, .. } => {}
                    other => panic!("rhs not mul: {other:?}"),
                }
            }
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn parses_let_in() {
        let e = p("let x = 1 in x + 2");
        match e {
            Expr::Let { name, .. } => assert_eq!(name, "x"),
            other => panic!("not a let: {other:?}"),
        }
    }

    #[test]
    fn parses_lambda_and_application() {
        let e = p("fn(x) => x + 1");
        assert!(matches!(e, Expr::Lambda { .. }));
        let e = p("(fn(x) => x + 1)(41)");
        assert!(matches!(e, Expr::App { .. }));
    }

    #[test]
    fn parse_error_on_missing_then() {
        let err = parse(&lex("if true 1 else 2").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::Unexpected { expected: "'then'", .. }));
    }
}
