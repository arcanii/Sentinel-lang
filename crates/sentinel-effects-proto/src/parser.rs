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
    #[error("effect label {label:?} must start with an uppercase letter")]
    EffectLabelNotUpper { label: String, span: Span },
    #[error("expected a type expression; got {found:?}")]
    ExpectedTypeExpr { found: Option<Token>, span: Span },
    /// B3.0: handler arm label must start with an uppercase letter
    /// (mirrors the `do Label` rule from B2.2a).
    #[error("handler arm label {label:?} must start with an uppercase letter")]
    HandlerArmLabelNotUpper { label: String, span: Span },
    /// B3.0: a `handle ... with { ... }` form had no arms between the
    /// braces. At minimum a single arm or a `return` arm is required.
    #[error("handler must contain at least one arm")]
    EmptyHandler { span: Span },
    /// B4.0c (ADR 0008 D1/D6): literal `secret secret T` (or
    /// `secret (secret T)`) rejected at parse time. The smart
    /// constructor [`crate::types::Ty::secret`] separately collapses
    /// doubly-wrapped types should they arrive through substitution;
    /// this parser rejection is the early complaint for source.
    #[error("`secret` may not be applied to a type that is already secret")]
    DoubleSecret { span: Span },
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

/// Parse a full Sentinel-Mini program: zero or more `effect` declarations
/// followed by a single body expression. Added in B2.2a.
pub fn parse_program(tokens: &[(Token, Span)]) -> Result<crate::ast::Program, ParseError> {
    let mut p = Parser { toks: tokens, pos: 0 };
    let mut effects = Vec::new();
    while matches!(p.peek_tok(), Some(Token::Effect)) {
        effects.push(p.parse_effect_decl()?);
    }
    let body = p.parse_expr()?;
    if let Some((t, span)) = p.peek().cloned() {
        return Err(ParseError::Trailing { found: t, span });
    }
    Ok(crate::ast::Program { effects, body })
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
            Some(Token::Handle) => self.parse_handle(),
            _ => self.parse_compare(),
        }
    }

    fn parse_handle(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span().expect("parse_handle called with no 'handle' token");
        self.bump(); // 'handle'
        let body = Box::new(self.parse_expr()?);
        self.expect("'with'", |t| matches!(t, Token::With))?;
        self.expect("'{'", |t| matches!(t, Token::LBrace))?;
        let mut arms: Vec<crate::ast::HandlerArm> = Vec::new();
        let mut ret_arm: Option<crate::ast::ReturnArm> = None;
        // Empty `{}` is rejected after the loop. We accept a trailing
        // comma after the last arm (D1).
        loop {
            if matches!(self.peek_tok(), Some(Token::RBrace)) {
                break;
            }
            if matches!(self.peek_tok(), Some(Token::Return)) {
                let ret_start = self.peek_span().expect("return token has span");
                self.bump(); // 'return'
                let var = match self.bump() {
                    Some((Token::Ident(s), _)) => s,
                    Some((t, span)) => return Err(ParseError::Unexpected {
                        found: t, expected: "identifier", span,
                    }),
                    None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
                };
                self.expect("'=>'", |t| matches!(t, Token::FatArrow))?;
                let body = Box::new(self.parse_expr()?);
                let span = ret_start.merge(body.span);
                ret_arm = Some(crate::ast::ReturnArm { var, body, span });
            } else {
                let (label, label_span) = match self.bump() {
                    Some((Token::Ident(s), sp)) => (s, sp),
                    Some((t, span)) => return Err(ParseError::Unexpected {
                        found: t, expected: "handler arm label", span,
                    }),
                    None => return Err(ParseError::UnexpectedEof { expected: "handler arm label" }),
                };
                if !label.chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
                    return Err(ParseError::HandlerArmLabelNotUpper { label, span: label_span });
                }
                self.expect("'('", |t| matches!(t, Token::LParen))?;
                let arg = match self.bump() {
                    Some((Token::Ident(s), _)) => s,
                    Some((t, span)) => return Err(ParseError::Unexpected {
                        found: t, expected: "identifier", span,
                    }),
                    None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
                };
                self.expect("','", |t| matches!(t, Token::Comma))?;
                let kont = match self.bump() {
                    Some((Token::Ident(s), _)) => s,
                    Some((t, span)) => return Err(ParseError::Unexpected {
                        found: t, expected: "identifier", span,
                    }),
                    None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
                };
                self.expect("')'", |t| matches!(t, Token::RParen))?;
                self.expect("'=>'", |t| matches!(t, Token::FatArrow))?;
                let body = Box::new(self.parse_expr()?);
                let span = label_span.merge(body.span);
                arms.push(crate::ast::HandlerArm {
                    label, label_span, arg, kont, body, span,
                });
            }
            if matches!(self.peek_tok(), Some(Token::Comma)) {
                self.bump();
                continue;
            }
            break;
        }
        self.expect("'}'", |t| matches!(t, Token::RBrace))?;
        let close_span = self.last_span();
        if arms.is_empty() && ret_arm.is_none() {
            return Err(ParseError::EmptyHandler { span: start.merge(close_span) });
        }
        let span = start.merge(close_span);
        Ok(expr(ExprKind::Handle { body, arms, ret_arm }, span))
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
            Some((Token::Do, start_span)) => self.parse_perform(start_span),
            // ADR 0008 D5/D6: `declassify(e)` at atom precedence,
            // mandatory parens (parallel to `do Label(arg)`).
            Some((Token::Declassify, start_span)) => self.parse_declassify(start_span),
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

    // ---- B2.2a: effect-surface parsing helpers ----

    fn parse_perform(&mut self, start_span: Span) -> Result<Expr, ParseError> {
        // The `do` token has already been consumed; `start_span` is its span.
        let (label, label_span) = match self.bump() {
            Some((Token::Ident(s), sp)) => (s, sp),
            Some((t, span)) => {
                return Err(ParseError::Unexpected { found: t, expected: "effect label", span })
            }
            None => return Err(ParseError::UnexpectedEof { expected: "effect label" }),
        };
        if !label.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(ParseError::EffectLabelNotUpper { label, span: label_span });
        }
        self.expect("'('", |t| matches!(t, Token::LParen))?;
        let arg = self.parse_expr()?;
        self.expect("')'", |t| matches!(t, Token::RParen))?;
        let close = self.last_span();
        let span = start_span.merge(close);
        Ok(expr(ExprKind::Perform { label, label_span, arg: Box::new(arg) }, span))
    }

    /// B4.0c (ADR 0008 D5/D6): `declassify(e)` as a parser-special
    /// atom-precedence form. Mandatory parens around the argument,
    /// paralleling `do Label(arg)`. The parens around `declassify`'s
    /// argument are part of the surface (D6) so every declassification
    /// site is syntactically grep-able in source -- this preserves
    /// the audit-point property called out in D5.
    fn parse_declassify(&mut self, start_span: Span) -> Result<Expr, ParseError> {
        // The `declassify` token has already been consumed.
        self.expect("'('", |t| matches!(t, Token::LParen))?;
        let inner = self.parse_expr()?;
        self.expect("')'", |t| matches!(t, Token::RParen))?;
        let close = self.last_span();
        let span = start_span.merge(close);
        Ok(expr(
            ExprKind::Declassify { inner: Box::new(inner), span },
            span,
        ))
    }

    fn parse_effect_decl(&mut self) -> Result<crate::ast::EffectDecl, ParseError> {
        // Caller has NOT yet consumed `effect`.
        let start = self.peek_span().ok_or(ParseError::UnexpectedEof { expected: "'effect'" })?;
        self.expect("'effect'", |t| matches!(t, Token::Effect))?;
        let (label, label_span) = match self.bump() {
            Some((Token::Ident(s), sp)) => (s, sp),
            Some((t, span)) => {
                return Err(ParseError::Unexpected { found: t, expected: "effect label", span })
            }
            None => return Err(ParseError::UnexpectedEof { expected: "effect label" }),
        };
        if !label.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(ParseError::EffectLabelNotUpper { label, span: label_span });
        }
        self.expect("':'", |t| matches!(t, Token::Colon))?;
        // B2.2a: parse a single type expression for the signature, then
        // require it to be an arrow at the top level. Writing
        // `effect L : ArgTy -> RetTy ;` as `arg + '->' + ret` is
        // ambiguous when ArgTy itself contains `->` (right-associative
        // arrows would eat the separator), so we parse one TyExpr and
        // destructure.
        let sig = self.parse_ty_expr()?;
        let sig_span = sig.span();
        let (arg, ret) = match sig {
            crate::ast::TyExpr::Arrow(a, b, _) => (*a, *b),
            other => return Err(ParseError::ExpectedTypeExpr {
                found: None,
                span: other.span(),
            }),
        };
        self.expect("';'", |t| matches!(t, Token::Semicolon))?;
        let end = self.last_span();
        let span = start.merge(end);
        let _ = sig_span;
        Ok(crate::ast::EffectDecl { label, label_span, arg, ret, span })
    }

    fn parse_ty_expr(&mut self) -> Result<crate::ast::TyExpr, ParseError> {
        let lhs = self.parse_ty_atom()?;
        if matches!(self.peek_tok(), Some(Token::Arrow)) {
            self.bump();
            let rhs = self.parse_ty_expr()?; // right-associative
            let span = lhs.span().merge(rhs.span());
            Ok(crate::ast::TyExpr::Arrow(Box::new(lhs), Box::new(rhs), span))
        } else {
            Ok(lhs)
        }
    }

    fn parse_ty_atom(&mut self) -> Result<crate::ast::TyExpr, ParseError> {
        match self.bump() {
            Some((Token::Ident(s), span)) if s == "Int" => Ok(crate::ast::TyExpr::Int(span)),
            Some((Token::Ident(s), span)) if s == "Bool" => Ok(crate::ast::TyExpr::Bool(span)),
            // ADR 0008 D6: prefix `secret T` binds tighter than `->`,
            // so `secret int -> bool` parses as `(secret int) -> bool`.
            // Recursing on parse_ty_atom (not parse_ty_expr) gives that
            // precedence; users wanting the arrow inside the secret
            // write `secret (int -> bool)`. DoubleSecret rejects
            // literal `secret secret T` and `secret (secret T)` -- the
            // smart-constructor still collapses double-wraps that
            // arrive through unification, this is the human-source
            // early complaint.
            Some((Token::Secret, start)) => {
                let inner = self.parse_ty_atom()?;
                if matches!(inner, crate::ast::TyExpr::Secret(_, _)) {
                    let span = start.merge(inner.span());
                    return Err(ParseError::DoubleSecret { span });
                }
                let span = start.merge(inner.span());
                Ok(crate::ast::TyExpr::Secret(Box::new(inner), span))
            }
            Some((Token::LParen, open)) => {
                let inner = self.parse_ty_expr()?;
                self.expect("')'", |t| matches!(t, Token::RParen))?;
                let close = self.last_span();
                let span = open.merge(close);
                match inner {
                    crate::ast::TyExpr::Int(_) => Ok(crate::ast::TyExpr::Int(span)),
                    crate::ast::TyExpr::Bool(_) => Ok(crate::ast::TyExpr::Bool(span)),
                    crate::ast::TyExpr::Arrow(a, b, _) => Ok(crate::ast::TyExpr::Arrow(a, b, span)),
                    // ADR 0008 D6: `( secret T )` preserves the
                    // Secret wrapper; the outer span absorbs the
                    // parens just like the other arms.
                    crate::ast::TyExpr::Secret(inner, _) => {
                        Ok(crate::ast::TyExpr::Secret(inner, span))
                    }
                }
            }
            Some((t, span)) => Err(ParseError::ExpectedTypeExpr { found: Some(t), span }),
            None => Err(ParseError::ExpectedTypeExpr { found: None, span: self.last_span() }),
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
    fn b30_handle_parses_single_arm_no_return() {
        let toks = crate::lexer::lex("handle e with { Get(x, k) => k }").unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Handle { arms, ret_arm, .. } => {
                assert_eq!(arms.len(), 1);
                assert_eq!(arms[0].label, "Get");
                assert_eq!(arms[0].arg, "x");
                assert_eq!(arms[0].kont, "k");
                assert!(ret_arm.is_none());
            }
            other => panic!("expected Handle, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_parses_multiple_arms() {
        let src = "handle e with { Get(x, k) => k, Put(s, k) => k }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Handle { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].label, "Get");
                assert_eq!(arms[1].label, "Put");
            }
            other => panic!("expected Handle, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_with_return_arm() {
        let src = "handle e with { Get(x, k) => k, return v => v }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Handle { arms, ret_arm, .. } => {
                assert_eq!(arms.len(), 1);
                let ret = ret_arm.expect("return arm");
                assert_eq!(ret.var, "v");
            }
            other => panic!("expected Handle, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_return_arm_only() {
        let src = "handle e with { return v => v }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Handle { arms, ret_arm, .. } => {
                assert!(arms.is_empty());
                assert!(ret_arm.is_some());
            }
            other => panic!("expected Handle, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_trailing_comma_allowed() {
        let src = "handle e with { Get(x, k) => k, }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        assert!(matches!(e.node, ExprKind::Handle { .. }));
    }

    #[test]
    fn b30_handle_empty_braces_errors() {
        let toks = crate::lexer::lex("handle e with { }").unwrap();
        let err = parse(&toks).unwrap_err();
        assert!(matches!(err, ParseError::EmptyHandler { .. }),
                "expected EmptyHandler, got {err:?}");
    }

    #[test]
    fn b30_handle_arm_label_must_be_uppercase() {
        let toks = crate::lexer::lex("handle e with { get(x, k) => k }").unwrap();
        let err = parse(&toks).unwrap_err();
        match err {
            ParseError::HandlerArmLabelNotUpper { label, .. } => {
                assert_eq!(label, "get");
            }
            other => panic!("expected HandlerArmLabelNotUpper, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_nested_in_let() {
        let src = "let r = handle e with { Get(x, k) => k } in r";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Let { value, .. } => {
                assert!(matches!(value.node, ExprKind::Handle { .. }));
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_as_lambda_body() {
        let src = "fn(e) => handle e with { Get(x, k) => k }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        match e.node {
            ExprKind::Lambda { body, .. } => {
                assert!(matches!(body.node, ExprKind::Handle { .. }));
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn b30_handle_span_covers_handle_through_close_brace() {
        let src = "handle e with { Get(x, k) => k }";
        let toks = crate::lexer::lex(src).unwrap();
        let e = parse(&toks).unwrap();
        assert_eq!(e.span.start, 0);
        assert_eq!(e.span.end as usize, src.len());
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

    // ---- B2.2a: effect-surface parser tests ----

    fn pp(src: &str) -> crate::ast::Program {
        parse_program(&lex(src).unwrap()).unwrap()
    }

    #[test]
    fn b22a_program_with_no_effects_parses_body() {
        let prog = pp("1 + 2");
        assert!(prog.effects.is_empty());
        assert!(matches!(prog.body.node, ExprKind::BinOp { .. }));
    }

    #[test]
    fn b22a_single_effect_decl_then_body() {
        let prog = pp("effect Print : Int -> Bool ; 1");
        assert_eq!(prog.effects.len(), 1);
        assert_eq!(prog.effects[0].label, "Print");
        assert_eq!(prog.body.node, ExprKind::Int(1));
    }

    #[test]
    fn b22a_two_effect_decls_in_order() {
        let prog = pp("effect Print : Int -> Bool ; effect Ask : Bool -> Int ; 0");
        assert_eq!(prog.effects.len(), 2);
        assert_eq!(prog.effects[0].label, "Print");
        assert_eq!(prog.effects[1].label, "Ask");
    }

    #[test]
    fn b22a_ty_expr_arrow_is_right_associative() {
        // With Fix A, `effect F : Int -> Bool -> Int ;` parses as a single
        // TyExpr `Int -> Bool -> Int` (right-associative: `Int -> (Bool -> Int)`),
        // then splits into arg=Int and ret=(Bool -> Int).
        let prog = pp("effect F : Int -> Bool -> Int ; 0");
        assert!(matches!(prog.effects[0].arg, crate::ast::TyExpr::Int(_)));
        match &prog.effects[0].ret {
            crate::ast::TyExpr::Arrow(a, b, _) => {
                assert!(matches!(**a, crate::ast::TyExpr::Bool(_)));
                assert!(matches!(**b, crate::ast::TyExpr::Int(_)));
            }
            other => panic!("expected ret to be an arrow, got {other:?}"),
        }
    }

    #[test]
    fn b22a_do_invocation_parses_as_perform() {
        let e = p("do Print(1)");
        match e.node {
            ExprKind::Perform { label, arg, .. } => {
                assert_eq!(label, "Print");
                assert_eq!(arg.node, ExprKind::Int(1));
            }
            other => panic!("not a Perform: {other:?}"),
        }
    }

    #[test]
    fn b22a_do_label_must_be_uppercase() {
        let err = parse(&lex("do print(1)").unwrap()).unwrap_err();
        match err {
            ParseError::EffectLabelNotUpper { label, .. } => assert_eq!(label, "print"),
            other => panic!("expected EffectLabelNotUpper, got {other:?}"),
        }
    }

    #[test]
    fn b22a_effect_decl_label_must_be_uppercase() {
        let err = parse_program(&lex("effect print : Int -> Bool ; 0").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::EffectLabelNotUpper { .. }));
    }

    #[test]
    fn b22a_effect_decl_missing_semicolon_errors() {
        // With Fix A the signature `Int -> Bool` is parsed as one TyExpr,
        // then a `;` is required. `0` shows up where `;` was expected.
        let err = parse_program(&lex("effect Print : Int -> Bool 0").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::Unexpected { expected: "';'", .. }),
                "got {err:?}");
    }

    #[test]
    fn b22a_effect_decl_signature_must_be_arrow() {
        // `effect Foo : Int ; 0` -- no arrow -- must error.
        let err = parse_program(&lex("effect Foo : Int ; 0").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::ExpectedTypeExpr { .. }),
                "got {err:?}");
    }

    #[test]
    fn b22a_effect_decl_with_paren_ty_expr() {
        // Parens force the LHS of the top-level arrow to itself be an arrow.
        let prog = pp("effect F : (Int -> Bool) -> Int ; 0");
        match &prog.effects[0].arg {
            crate::ast::TyExpr::Arrow(..) => {}
            other => panic!("expected arg to be an arrow, got {other:?}"),
        }
        assert!(matches!(prog.effects[0].ret, crate::ast::TyExpr::Int(_)));
    }

    #[test]
    fn b22a_do_inside_arithmetic_context() {
        // `do Print(1) + 2` should parse: Perform is an atom, BinOp wraps it.
        let e = p("do Print(1) + 2");
        match e.node {
            ExprKind::BinOp { lhs, rhs, .. } => {
                assert!(matches!(lhs.node, ExprKind::Perform { .. }));
                assert_eq!(rhs.node, ExprKind::Int(2));
            }
            other => panic!("expected BinOp wrapping Perform, got {other:?}"),
        }
    }

    // ---- B4.0c: secret + declassify parser surface (ADR 0008 D5/D6) ----

    #[test]
    fn b40c_secret_prefix_on_int_in_effect_decl() {
        let prog = pp("effect ReadKey : Int -> secret Int ; 0");
        match &prog.effects[0].ret {
            crate::ast::TyExpr::Secret(inner, _) => {
                assert!(matches!(**inner, crate::ast::TyExpr::Int(_)));
            }
            other => panic!("expected ret to be Secret, got {other:?}"),
        }
    }

    #[test]
    fn b40c_secret_binds_tighter_than_arrow() {
        // `secret Int -> Bool` parses as `(secret Int) -> Bool`,
        // not `secret (Int -> Bool)`. The arrow is at outer precedence.
        let prog = pp("effect F : secret Int -> Bool ; 0");
        match &prog.effects[0].arg {
            crate::ast::TyExpr::Secret(inner, _) => {
                assert!(matches!(**inner, crate::ast::TyExpr::Int(_)));
            }
            other => panic!("expected arg to be Secret(Int), got {other:?}"),
        }
        assert!(matches!(prog.effects[0].ret, crate::ast::TyExpr::Bool(_)));
    }

    #[test]
    fn b40c_secret_arrow_inside_parens() {
        // `secret (Int -> Bool)` keeps the arrow under the Secret.
        let prog = pp("effect F : Int -> secret (Int -> Bool) ; 0");
        match &prog.effects[0].ret {
            crate::ast::TyExpr::Secret(inner, _) => match &**inner {
                crate::ast::TyExpr::Arrow(_, _, _) => {}
                other => panic!("expected Secret(Arrow), inner was {other:?}"),
            },
            other => panic!("expected ret to be Secret, got {other:?}"),
        }
    }

    #[test]
    fn b40c_double_secret_literal_rejected() {
        let err = parse_program(&lex("effect F : Int -> secret secret Int ; 0").unwrap())
            .unwrap_err();
        assert!(matches!(err, ParseError::DoubleSecret { .. }), "got {err:?}");
    }

    #[test]
    fn b40c_double_secret_through_parens_rejected() {
        // `secret (secret Int)` is also flagged at parse time.
        let err = parse_program(&lex("effect F : Int -> secret (secret Int) ; 0").unwrap())
            .unwrap_err();
        assert!(matches!(err, ParseError::DoubleSecret { .. }), "got {err:?}");
    }

    #[test]
    fn b40c_declassify_call_parses() {
        let e = p("declassify(1)");
        match e.node {
            ExprKind::Declassify { inner, .. } => {
                assert_eq!(inner.node, ExprKind::Int(1));
            }
            other => panic!("expected Declassify, got {other:?}"),
        }
    }

    #[test]
    fn b40c_declassify_requires_parens() {
        // `declassify 1` (no parens) is rejected -- mirrors `do` which
        // also requires parens around its argument.
        let err = parse(&lex("declassify 1").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::Unexpected { expected: "'('", .. }),
                "got {err:?}");
    }

    #[test]
    fn b40c_declassify_inside_arithmetic_context() {
        // declassify is atom-precedence, so it composes with BinOp.
        let e = p("declassify(x) + 1");
        match e.node {
            ExprKind::BinOp { lhs, rhs, .. } => {
                assert!(matches!(lhs.node, ExprKind::Declassify { .. }));
                assert_eq!(rhs.node, ExprKind::Int(1));
            }
            other => panic!("expected BinOp wrapping Declassify, got {other:?}"),
        }
    }

    #[test]
    fn b40c_declassify_arg_can_be_full_expr() {
        // The argument inside the parens is a full expression, not an atom.
        let e = p("declassify(1 + 2)");
        match e.node {
            ExprKind::Declassify { inner, .. } => {
                assert!(matches!(inner.node, ExprKind::BinOp { .. }));
            }
            other => panic!("expected Declassify, got {other:?}"),
        }
    }
}
