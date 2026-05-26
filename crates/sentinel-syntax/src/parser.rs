//! Hand-written recursive descent parser for Sentinel C0.1-C0.4.
//!
//! C0.4 grammar (subset of ADR 0010):
//!
//! ```text
//! program     = stmt* tail_expr
//! stmt        = let_stmt | expr_stmt
//! let_stmt    = 'let' Ident '=' expr ';'
//! expr_stmt   = expr ';'
//! tail_expr   = expr                                  (no trailing ';')
//!
//! expr        = if_expr | add_expr                    (if only at expr top)
//! if_expr     = 'if' expr block else_branch
//! else_branch = 'else' (if_expr | block)
//! block       = '{' stmt* tail_expr '}'
//! add_expr    = mul_expr (('+' | '-') mul_expr)*       left-assoc
//! mul_expr    = unary    (('*' | '/') unary)*          left-assoc
//! unary       = '-' unary | atom
//! atom        = IntLit
//!             | Ident                                 (Var, unless followed by `(`)
//!             | Ident '(' arg_list ')'                (Call)
//!             | '(' expr ')'
//!             | block
//! arg_list    = (expr (',' expr)*)? ','?
//! ```
//!
//! Per ADR 0009 D1a, the public entry points are pure functions:
//! [`parse`] returns a [`Program`] (statements + trailing
//! expression), [`parse_expr`] returns a single [`Expr`] (used by
//! library callers and existing C0.1 tests that pre-date the
//! statement layer). Lex errors are surfaced as a transparent
//! `ParseError::Lex` variant so the front-end has a single error
//! type. Parse errors are fail-fast — error recovery is deferred
//! until parser ergonomics demand it.
//!
//! `fn` definitions arrive in C0.5 per ADR 0009 D6.

use sentinel_ast::{
    BinOp, Block, Expr, ExprKind, FnDef, Param, Program, Span, Spanned, Stmt, StmtKind, TypeExpr,
    TypeExprKind, UnaryOp,
};

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

/// Parse a Sentinel source string into a [`Program`] — zero or
/// more statements followed by a trailing expression. This is the
/// C0.3+ top-level entry point.
///
/// Per ADR 0009 D1a the function is pure: input is the source
/// string, output is a `Result`. Lex errors block parsing and are
/// surfaced as `ParseError::Lex`.
pub fn parse(src: &str) -> Result<Program, ParseError> {
    let mut p = lex_into_parser(src)?;
    p.parse_program()
}

/// Parse a Sentinel source string as a single expression. The whole
/// input must be one expression with no surrounding statements or
/// trailing `;` — used by library callers that want to parse an
/// expression in isolation (notably the parser's own unit tests
/// and any future REPL / language-server completion machinery).
pub fn parse_expr(src: &str) -> Result<Expr, ParseError> {
    let mut p = lex_into_parser(src)?;
    p.parse_top()
}

/// Parse a Sentinel source string as a single brace-wrapped block.
/// The input must be `{ stmt* tail_expr }` with no surrounding
/// content. Used by tests and REPL/completion machinery that need
/// to parse a block in isolation; the rest of the parser exercises
/// block parsing through `parse_fn_def` -> `parse_block`.
pub fn parse_block_str(src: &str) -> Result<Block, ParseError> {
    let mut p = lex_into_parser(src)?;
    p.parse_block_str()
}

fn lex_into_parser<'a>(src: &'a str) -> Result<ParserOwned<'a>, ParseError> {
    let (tokens, lex_errs) = lex(src);
    if let Some(err) = lex_errs.into_iter().next() {
        return Err(ParseError::from(err));
    }
    Ok(ParserOwned { src, tokens })
}

/// Helper type that owns the lexed tokens so the [`Parser`] borrow
/// is locally scoped — keeps the two entry points symmetric.
struct ParserOwned<'a> {
    src: &'a str,
    tokens: Vec<Spanned<TokenKind>>,
}

impl<'a> ParserOwned<'a> {
    fn parse_top(&mut self) -> Result<Expr, ParseError> {
        let mut p = Parser::new(self.src, &self.tokens);
        p.parse_top()
    }
    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut p = Parser::new(self.src, &self.tokens);
        p.parse_program()
    }
    fn parse_block_str(&mut self) -> Result<Block, ParseError> {
        let mut p = Parser::new(self.src, &self.tokens);
        let block = p.parse_block()?;
        if let Some(t) = p.peek() {
            return Err(ParseError::UnexpectedToken {
                got: format!("{:?}", t.kind),
                expected: "end of input",
                span: to_source_span(&t.span),
            });
        }
        Ok(block)
    }
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

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let start = self.peek().map_or(0, |t| t.span.start);
        let mut fns = Vec::new();
        while self.peek().is_some() {
            let f = self.parse_fn_def()?;
            fns.push(f);
        }
        if fns.is_empty() {
            return Err(ParseError::UnexpectedEof {
                expected: "`fn` (programs are one or more function definitions)",
                span: to_source_span(&self.eof_span()),
            });
        }
        let end = fns.last().expect("non-empty").span.end;
        Ok(Program { fns, span: start..end })
    }

    fn parse_fn_def(&mut self) -> Result<FnDef, ParseError> {
        let fn_start = match self.peek_kind() {
            Some(TokenKind::Fn) => self.advance().expect("peeked").span.start,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`fn`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`fn`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // Function name.
        let (name, name_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let name = self.src[span.clone()].to_string();
                self.advance();
                (name, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "function name after `fn`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "function name after `fn`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // `(`
        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after function name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after function name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Parameter list.
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break; // trailing comma allowed
                }
                params.push(self.parse_param()?);
            }
        }

        // `)`
        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // `-> return_type` is mandatory at C1.2 per ADR 0012 D1.
        let return_type = self.parse_return_type()?;

        // Body.
        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(FnDef { name, name_span, params, return_type, body, span: fn_start..end })
    }

    /// Consume `-> type`. Required after every fn's parameter list
    /// per ADR 0012 D1. Returns the parsed [`TypeExpr`].
    fn parse_return_type(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Arrow) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`->` introducing return type",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`->` introducing return type",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        self.parse_type()
    }

    /// Parse a [`TypeExpr`]. At C1.2 this is just an `Ident`. Later
    /// sub-phases extend the grammar per ADR 0012 D3 + D10's revisit
    /// notes (struct names at C1.4, `?T` at C1.5, generics at C1.7).
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let name = self.src[span.clone()].to_string();
                self.advance();
                Ok(Spanned { kind: TypeExprKind::Ident(name), span })
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "type",
                    span: to_source_span(&span),
                })
            }
            None => Err(ParseError::UnexpectedEof {
                expected: "type",
                span: to_source_span(&self.eof_span()),
            }),
        }
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let (name, name_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let name = self.src[span.clone()].to_string();
                self.advance();
                (name, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "parameter name",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "parameter name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // `:` between name and type per ADR 0012 D1.
        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` followed by parameter type",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` followed by parameter type",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let ty = self.parse_type()?;
        let span_end = ty.span.end;
        Ok(Param { name, span: name_span.start..span_end, ty })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let let_start = self.advance().expect("checked above").span.start;

        let name_token = self.peek().ok_or_else(|| ParseError::UnexpectedEof {
            expected: "identifier after `let`",
            span: to_source_span(&self.eof_span()),
        })?;
        if name_token.kind != TokenKind::Ident {
            let kind = name_token.kind;
            let span = name_token.span.clone();
            return Err(ParseError::UnexpectedToken {
                got: format!("{kind:?}"),
                expected: "identifier after `let`",
                span: to_source_span(&span),
            });
        }
        let name_span = name_token.span.clone();
        let name = self.src[name_span.clone()].to_string();
        self.advance();

        // Optional `: type` annotation per ADR 0012 D2.
        let ty_annot = if self.peek_kind() == Some(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        match self.peek_kind() {
            Some(TokenKind::Eq) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`=` in let-binding",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`=` in let-binding",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let value = self.parse_expr()?;

        let semi_end = match self.peek_kind() {
            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`;` after let-binding",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`;` after let-binding",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(Spanned {
            kind: StmtKind::Let { name, name_span, ty_annot, value },
            span: let_start..semi_end,
        })
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
        if self.peek_kind() == Some(TokenKind::If) {
            return self.parse_if();
        }
        self.parse_add()
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let if_start = self.advance().expect("checked `if`").span.start;
        let cond = self.parse_expr()?;
        let then_branch = self.parse_block()?;

        match self.peek_kind() {
            Some(TokenKind::Else) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`else` after if-then block",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`else` after if-then block",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // else_branch is either a chained `if` or a brace-wrapped block.
        let else_branch = if self.peek_kind() == Some(TokenKind::If) {
            let inner_if = self.parse_if()?;
            let span = inner_if.span.clone();
            Block { stmts: vec![], tail: inner_if, span }
        } else {
            self.parse_block()?
        };

        let end = else_branch.span.end;
        Ok(Spanned {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            },
            span: if_start..end,
        })
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let lbrace_start = match self.peek_kind() {
            Some(TokenKind::LBrace) => self.advance().expect("peeked").span.start,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open block",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open block",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        let mut stmts = Vec::new();
        loop {
            match self.peek_kind() {
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "expression or `}` in block",
                        span: to_source_span(&self.eof_span()),
                    });
                }
                Some(TokenKind::RBrace) => {
                    // ADR 0010 D6: no empty blocks — a trailing expression is required.
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: "RBrace".to_string(),
                        expected: "expression before `}` (blocks must end with an expression)",
                        span: to_source_span(&t.span),
                    });
                }
                Some(TokenKind::Let) => {
                    let stmt = self.parse_let_stmt()?;
                    stmts.push(stmt);
                }
                Some(_) => {
                    let expr = self.parse_expr()?;
                    if self.peek_kind() == Some(TokenKind::Semi) {
                        let semi_end = self.advance().expect("peeked").span.end;
                        let span = expr.span.start..semi_end;
                        stmts.push(Spanned { kind: StmtKind::Expr(expr), span });
                        continue;
                    }
                    // Tail expression — expect `}`.
                    let rbrace_end = match self.peek_kind() {
                        Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
                        Some(other) => {
                            let t = self.peek().expect("peeked");
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{other:?}"),
                                expected: "`}` after trailing block expression",
                                span: to_source_span(&t.span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`}` after trailing block expression",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    return Ok(Block {
                        stmts,
                        tail: expr,
                        span: lbrace_start..rbrace_end,
                    });
                }
            }
        }
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
            Some(TokenKind::Ident) => {
                let name_span = self.advance().expect("peeked").span.clone();
                let name = self.src[name_span.clone()].to_string();
                // Lookahead for a call: Ident '(' ...
                if self.peek_kind() == Some(TokenKind::LParen) {
                    self.advance(); // consume `(`
                    let mut args = Vec::new();
                    if self.peek_kind() != Some(TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.peek_kind() == Some(TokenKind::Comma) {
                            self.advance();
                            if self.peek_kind() == Some(TokenKind::RParen) {
                                break; // trailing comma allowed
                            }
                            args.push(self.parse_expr()?);
                        }
                    }
                    let rparen_end = match self.peek_kind() {
                        Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                        Some(other) => {
                            let t = self.peek().expect("peeked");
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{other:?}"),
                                expected: "`,` or `)` in argument list",
                                span: to_source_span(&t.span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`,` or `)` in argument list",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    Ok(Spanned {
                        kind: ExprKind::Call { callee: name, callee_span: name_span.clone(), args },
                        span: name_span.start..rparen_end,
                    })
                } else {
                    Ok(Spanned { kind: ExprKind::Var(name), span: name_span })
                }
            }
            Some(TokenKind::LBrace) => {
                let block = self.parse_block()?;
                let span = block.span.clone();
                Ok(Spanned { kind: ExprKind::Block(Box::new(block)), span })
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
        parse_expr(src).unwrap_or_else(|e| panic!("expected parse to succeed: {e:?}"))
    }

    fn pretty(src: &str) -> String {
        parse_ok(src).kind.to_string()
    }

    /// Parse `src` as a block by wrapping it in `{ … }` and calling
    /// [`parse_block_str`]. Lets the existing C0.3-0.4 stmt/tail tests
    /// keep their literal input strings unchanged after the C0.5
    /// hard break — they're now testing the block parser, which has
    /// the same internal logic as the old top-level parse_program.
    fn parse_block_ok(src: &str) -> Block {
        let wrapped = format!("{{ {src} }}");
        parse_block_str(&wrapped)
            .unwrap_or_else(|e| panic!("expected parse to succeed: {e:?}"))
    }

    fn pretty_block(src: &str) -> String {
        parse_block_ok(src).to_string()
    }

    /// Parse `src` as a block by wrapping it in `{ … }` and return
    /// the error. Used by tests that previously called `parse(src)`
    /// to test stmt-level error cases — those tests now apply to
    /// block bodies inside fn defs.
    fn parse_block_err(src: &str) -> ParseError {
        let wrapped = format!("{{ {src} }}");
        parse_block_str(&wrapped).expect_err("expected parse error")
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
        let err = parse_expr("(1 + 2").unwrap_err();
        assert!(matches!(err, ParseError::UnmatchedParen { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_unexpected_close_paren() {
        let err = parse_expr(")").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_eof_after_operator() {
        let err = parse_expr("1 +").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_eof_after_unary() {
        let err = parse_expr("-").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_lex_passthrough() {
        let err = parse_expr("1 + @").unwrap_err();
        assert!(matches!(err, ParseError::Lex(_)), "got {err:?}");
    }

    #[test]
    fn parse_error_int_lit_overflow() {
        let err = parse_expr("99999999999999999999").unwrap_err();
        assert!(matches!(err, ParseError::IntLitOverflow { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_trailing_garbage() {
        // The parser accepts an expression and then expects EOF;
        // `1 2` parses `1` as an expression but `2` is leftover.
        let err = parse_expr("1 2").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { expected, .. } if expected == "end of input"));
    }

    // C0.3: identifier atom and program-level statements.

    #[test]
    fn parse_var_atom() {
        // `parse_expr` recognises a bare identifier as a Var reference.
        assert_eq!(pretty("foo"), "foo");
    }

    #[test]
    fn parse_var_in_arithmetic() {
        assert_eq!(pretty("x + y * 2"), "(+ x (* y 2))");
    }

    #[test]
    fn parse_program_empty_stmts() {
        // Pure expression program still parses; the Program wraps the
        // expression with an empty stmts list.
        let p = parse_block_ok("42");
        assert!(p.stmts.is_empty());
        assert_eq!(p.tail.kind.to_string(), "42");
    }

    #[test]
    fn parse_program_single_let() {
        let p = parse_block_ok("let x = 5; x");
        assert_eq!(p.stmts.len(), 1);
        match &p.stmts[0].kind {
            StmtKind::Let { name, value, .. } => {
                assert_eq!(name, "x");
                assert_eq!(value.kind.to_string(), "5");
            }
            other => panic!("expected Let, got {other:?}"),
        }
        assert_eq!(p.tail.kind.to_string(), "x");
    }

    #[test]
    fn parse_program_multiple_lets() {
        assert_eq!(
            pretty_block("let x = 1; let y = 2; x + y"),
            "(block (let x 1) (let y 2) (+ x y))"
        );
    }

    #[test]
    fn parse_program_expr_statement() {
        // `1 + 2;` is an expression-statement; `3` is the trailing expr.
        let p = parse_block_ok("1 + 2; 3");
        assert_eq!(p.stmts.len(), 1);
        match &p.stmts[0].kind {
            StmtKind::Expr(e) => assert_eq!(e.kind.to_string(), "(+ 1 2)"),
            other => panic!("expected Expr stmt, got {other:?}"),
        }
        assert_eq!(p.tail.kind.to_string(), "3");
    }

    #[test]
    fn parse_program_uses_prior_let() {
        assert_eq!(
            pretty_block("let a = 2; let b = a * 3; b + 1"),
            "(block (let a 2) (let b (* a 3)) (+ b 1))"
        );
    }

    #[test]
    fn parse_program_let_span_covers_let_through_semi() {
        // parse_block_ok wraps the input in `{ ` + src + ` }`, so all
        // byte offsets in the result shift by 2 (the leading `{ `).
        let p = parse_block_ok("let x = 5; x");
        let let_stmt = &p.stmts[0];
        assert_eq!(let_stmt.span, 2..12); // `let x = 5;` shifted by `{ `
        match &let_stmt.kind {
            StmtKind::Let { name_span, .. } => assert_eq!(*name_span, 6..7), // `x` shifted
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_program_error_empty_input() {
        // Top-level: programs must contain at least one fn def.
        let err = parse("").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if expected.contains("`fn`")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_program_error_only_let_no_tail() {
        // The wrapped input `{ let x = 1; }` has a stmt but no tail
        // expression — the block parser flags this.
        let err = parse_block_err("let x = 1;");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("blocks must end with an expression")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_program_error_let_missing_semi() {
        let err = parse_block_err("let x = 1");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`;` after let-binding"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_program_error_let_missing_eq() {
        let err = parse_block_err("let x 1;");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`=` in let-binding"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_program_error_let_missing_ident() {
        let err = parse_block_err("let = 1;");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "identifier after `let`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_program_error_let_bare() {
        let err = parse_block_err("let");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "identifier after `let`"),
            "got {err:?}"
        );
    }

    // C0.4: blocks, if/else, calls.

    #[test]
    fn parse_block_expr() {
        assert_eq!(pretty("{ 42 }"), "(block 42)");
    }

    #[test]
    fn parse_block_with_stmt() {
        assert_eq!(pretty("{ let x = 1; x + 2 }"), "(block (let x 1) (+ x 2))");
    }

    #[test]
    fn parse_block_in_arithmetic() {
        // `{ 5 } + 3` is valid because block is an atom.
        assert_eq!(pretty("{ 5 } + 3"), "(+ (block 5) 3)");
    }

    #[test]
    fn parse_if_simple() {
        assert_eq!(pretty("if 1 { 2 } else { 3 }"), "(if 1 (block 2) (block 3))");
    }

    #[test]
    fn parse_if_else_if_chain() {
        assert_eq!(
            pretty("if 1 { 2 } else if 3 { 4 } else { 5 }"),
            "(if 1 (block 2) (block (if 3 (block 4) (block 5))))"
        );
    }

    #[test]
    fn parse_if_with_var_condition() {
        assert_eq!(pretty("if x { 1 } else { 2 }"), "(if x (block 1) (block 2))");
    }

    #[test]
    fn parse_if_nested_in_parens_in_arithmetic() {
        // `if` is only at expr top; embedding in arithmetic needs parens.
        assert_eq!(
            pretty("(if 1 { 2 } else { 3 }) + 4"),
            "(+ (if 1 (block 2) (block 3)) 4)"
        );
    }

    #[test]
    fn parse_call_zero_args() {
        assert_eq!(pretty("foo()"), "(foo)");
    }

    #[test]
    fn parse_call_one_arg() {
        assert_eq!(pretty("print(42)"), "(print 42)");
    }

    #[test]
    fn parse_call_multi_args() {
        assert_eq!(pretty("f(1, 2, 3)"), "(f 1 2 3)");
    }

    #[test]
    fn parse_call_trailing_comma() {
        assert_eq!(pretty("f(1, 2,)"), "(f 1 2)");
    }

    #[test]
    fn parse_call_in_arithmetic() {
        assert_eq!(pretty("1 + f(2) * 3"), "(+ 1 (* (f 2) 3))");
    }

    #[test]
    fn parse_call_with_complex_arg() {
        assert_eq!(pretty("print(x + 1)"), "(print (+ x 1))");
    }

    #[test]
    fn parse_var_followed_by_non_paren_is_var() {
        // Make sure Ident-followed-by-something-other-than-`(` is Var.
        assert_eq!(pretty("foo + 1"), "(+ foo 1)");
    }

    #[test]
    fn parse_program_with_print_statement() {
        // Pass tests will use this shape: `print(x);` as expr-stmt
        // inside a fn body block.
        assert_eq!(
            pretty_block("let x = 5; print(x); x"),
            "(block (let x 5) (print x); x)"
        );
    }

    #[test]
    fn parse_error_if_missing_else() {
        let err = parse_expr("if 1 { 2 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`else` after if-then block"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_if_missing_then_block() {
        let err = parse_expr("if 1 else { 2 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`{` to open block"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_empty_block() {
        let err = parse_expr("{ }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("blocks must end with an expression")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_call_unclosed_args() {
        let err = parse_expr("f(1, 2").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`,` or `)` in argument list"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_block_unclosed_after_tail() {
        let err = parse_expr("{ 42").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`}` after trailing block expression"),
            "got {err:?}"
        );
    }

    // C0.5: fn-definition parsing at top level.

    fn parse_ok_program(src: &str) -> Program {
        parse(src).unwrap_or_else(|e| panic!("expected parse to succeed: {e:?}"))
    }

    #[test]
    fn parse_one_fn_main() {
        let p = parse_ok_program("fn main() -> i64 { 42 }");
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.fns[0].name, "main");
        assert!(p.fns[0].params.is_empty());
        assert_eq!(p.fns[0].return_type.kind.to_string(), "i64");
        assert_eq!(p.fns[0].body.tail.kind.to_string(), "42");
    }

    #[test]
    fn parse_fn_with_one_param() {
        let p = parse_ok_program("fn double(x: i64) -> i64 { x * 2 }");
        assert_eq!(p.fns[0].name, "double");
        assert_eq!(p.fns[0].params.len(), 1);
        assert_eq!(p.fns[0].params[0].name, "x");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "i64");
    }

    #[test]
    fn parse_fn_with_multi_params() {
        let p = parse_ok_program("fn add(a: i64, b: i64, c: i64) -> i64 { a + b + c }");
        let names: Vec<&str> = p.fns[0].params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        for param in &p.fns[0].params {
            assert_eq!(param.ty.kind.to_string(), "i64");
        }
    }

    #[test]
    fn parse_fn_with_trailing_comma_in_params() {
        let p = parse_ok_program("fn add(a: i64, b: i64,) -> i64 { a + b }");
        assert_eq!(p.fns[0].params.len(), 2);
    }

    #[test]
    fn parse_multi_fn_program() {
        let p = parse_ok_program(
            "fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { double(7) }",
        );
        assert_eq!(p.fns.len(), 2);
        assert_eq!(p.fns[0].name, "double");
        assert_eq!(p.fns[1].name, "main");
    }

    #[test]
    fn parse_fn_display_round_trip() {
        let p = parse_ok_program("fn main() -> i64 { 1 + 2 }");
        assert_eq!(p.to_string(), "(fn main () -> i64 (block (+ 1 2)))");
    }

    // C1.2 annotation grammar tests.

    #[test]
    fn parse_fn_with_annotated_params_and_return() {
        let p = parse_ok_program("fn id(x: i64) -> i64 { x }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "i64");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "i64");
    }

    #[test]
    fn parse_let_with_annotation() {
        let block = parse_block_str("{ let x: i64 = 5; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { ty_annot, .. } => {
                let ty = ty_annot.as_ref().expect("annotation present");
                assert_eq!(ty.kind.to_string(), "i64");
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_let_without_annotation_has_none() {
        let block = parse_block_str("{ let x = 5; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { ty_annot: None, .. } => {}
            other => panic!("expected Let with no annotation, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_fn_missing_return_type_arrow() {
        // Per ADR 0012 D1 the return type is mandatory; missing `->`
        // surfaces as a parse error.
        let err = parse("fn main() { 1 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`->` introducing return type"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_param_missing_colon() {
        let err = parse("fn id(x) -> i64 { x }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`:` followed by parameter type"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_param_colon_without_type() {
        let err = parse("fn id(x:) -> i64 { x }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "type"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_top_level_not_fn() {
        // Top level expects `fn`, not `let`.
        let err = parse("let x = 1;").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`fn`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_top_level_bare_expression() {
        // Bare expressions at top level no longer parse — they're fn-body content now.
        let err = parse("42").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`fn`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_fn_missing_name() {
        let err = parse("fn () { 1 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "function name after `fn`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_fn_missing_open_paren() {
        let err = parse("fn main { 1 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`(` after function name"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_fn_missing_close_paren() {
        // `fn main(x: i64 { 1 }` — annotated param but unclosed `)`.
        let err = parse("fn main(x: i64 { 1 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`,` or `)` in parameter list"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_fn_missing_body() {
        // Annotated `fn name() -> i64` with no body — `{` is expected next.
        let err = parse("fn main() -> i64").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`{` to open block"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_fn_bad_param_name() {
        let err = parse("fn main(1) { 1 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "parameter name"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_fn_span_covers_keyword_through_close_brace() {
        let src = "fn main() -> i64 { 42 }";
        let p = parse_ok_program(src);
        assert_eq!(p.fns[0].span, 0..src.len());
        assert_eq!(p.fns[0].name_span, 3..7); // `main`
    }
}
