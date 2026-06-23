//! Hand-written recursive descent parser for Sentinel C0/C1.
//!
//! C1.3 grammar (subset of ADRs 0010 + 0012):
//!
//! ```text
//! program     = fn_def+
//! fn_def      = 'fn' Ident '(' param_list ')' '->' type block
//! param       = Ident ':' type
//! type        = Ident
//!
//! stmt        = let_stmt | expr_stmt
//! let_stmt    = 'let' Ident (':' type)? '=' expr ';'
//! expr_stmt   = expr ';'
//! tail_expr   = expr                                  (no trailing ';')
//!
//! expr        = if_expr | or_expr                     (if only at expr top)
//! if_expr     = 'if' expr block else_branch
//! else_branch = 'else' (if_expr | block)
//! block       = '{' stmt* tail_expr '}'
//!
//! Precedence ladder (C1.3, ADR 0012 D7):
//!   or_expr   = and_expr ('||' and_expr)*              left-assoc, short-circuit
//!   and_expr  = cmp_expr ('&&' cmp_expr)*              left-assoc, short-circuit
//!   cmp_expr  = add_expr (cmp_op add_expr)?            NON-associative (D6)
//!   add_expr  = mul_expr (('+' | '-') mul_expr)*       left-assoc
//!   mul_expr  = unary    (('*' | '/') unary)*          left-assoc
//!   unary     = ('-' | '!') unary | atom
//!
//!   cmp_op    = '==' | '!=' | '<' | '<=' | '>' | '>='
//!
//! atom        = IntLit
//!             | 'true' | 'false'                       (C1.3 BoolLit)
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

use sentinel_ast::{
    BinOp, Block, ClassDecl, ClassField, CmpOp, DelegateDecl, EffectDecl, EnumDecl, Expr, ExprKind,
    FieldInit, FnDef, HandlerArm, ImplDecl, ImplMethodDef, InitDef, LogicOp, MatchArm, MethodDef,
    OpDecl, Param, Pattern, Program, ReturnArm, SelfKind, Span, Spanned, Stmt, StmtKind, StructDecl,
    StructField, TraitDecl, TraitMethodSig, TypeExpr, TypeExprKind, TypeParam, UnaryOp,
    UseDecl, VariantDecl, Visibility,
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

    #[error("chained comparison is not allowed")]
    #[diagnostic(
        code(sentinel::parse::chained_comparison),
        help("comparison operators are non-associative per ADR 0012 D6; parenthesise one side, e.g. `(a < b) && (b < c)`")
    )]
    ChainedComparison {
        #[label("second comparison operator")]
        span: miette::SourceSpan,
    },

    #[error("nested nullable types are not allowed")]
    #[diagnostic(
        code(sentinel::parse::nested_nullable),
        help("`??T` is rejected at C1.5 per ADR 0014 D6 — nested nullables don't earn their keep yet")
    )]
    NestedNullable {
        #[label("second `?` here")]
        span: miette::SourceSpan,
    },

    #[error("empty generic parameter list `<>` is not allowed")]
    #[diagnostic(
        code(sentinel::parse::empty_type_params),
        help("either drop the `<>` for a non-generic definition or list at least one parameter, e.g. `<T>` per ADR 0016 D1")
    )]
    EmptyTypeParams {
        #[label("expected at least one type parameter")]
        span: miette::SourceSpan,
    },

    #[error("empty generic argument list `<>` is not allowed")]
    #[diagnostic(
        code(sentinel::parse::empty_type_args),
        help("drop the `<>` for a non-generic type or list at least one argument per ADR 0016 D3")
    )]
    EmptyTypeArgs {
        #[label("expected at least one type argument")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D5: nested `secret secret T` is rejected at
    /// parse time. The depth-1 rule mirrors C1.5's `??T` rejection
    /// per ADR 0014 D6 — qualifier composition stays bounded at
    /// the C3 minimum.
    #[error("nested `secret secret T` is not allowed")]
    #[diagnostic(
        code(sentinel::parse::double_secret),
        help("`secret secret T` is rejected at C3.0 per ADR 0019 D5 — secret-qualifier depth is bounded to 1")
    )]
    DoubleSecret {
        #[label("second `secret` here")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D1: an empty effect-row annotation `! { }`
    /// is rejected at parse time. Write no annotation at all to
    /// mean "no effects" (`fn f() -> T { ... }` already implies
    /// no effects); `! { Op }` is the single-effect case.
    #[error("empty effect-row annotation `! {{ }}` is not allowed")]
    #[diagnostic(
        code(sentinel::parse::empty_effect_annotation),
        help("either drop the `! {{ }}` for a no-effect fn or list at least one effect, e.g. `! {{ Io }}` per ADR 0019 D1")
    )]
    EmptyEffectAnnotation {
        #[label("`!` here")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D4: at most one `init` declaration is
    /// permitted per class. Multiple init overloads are deferred
    /// to a future ADR.
    #[error("class `{class_name}` declares multiple `init` constructors")]
    #[diagnostic(
        code(sentinel::parse::duplicate_class_init),
        help("a class may have at most one `init` at C4.1 per ADR 0022 D4; multiple-init overloads are deferred")
    )]
    DuplicateClassInit {
        class_name: String,
        #[label("second `init` here")]
        span: miette::SourceSpan,
    },

    /// D.2 / ADR 0033 D2: a char/byte literal `'…'` must decode to
    /// exactly one byte. `''` (empty), `'ab'` (too many), and a
    /// multi-byte source character are all rejected — char literals
    /// are single-byte at this MVP (no Unicode code points — D8).
    #[error("char literal `{text}` must contain exactly one byte")]
    #[diagnostic(
        code(sentinel::parse::char_lit_not_single_byte),
        help("a char literal is a single `u8` byte per ADR 0033 D2; use a string literal `\"…\"` for more than one byte")
    )]
    CharLitNotSingleByte {
        text: String,
        #[label("not a single byte")]
        span: miette::SourceSpan,
    },

    /// D.2 / ADR 0033 D2: an invalid escape sequence in a char or
    /// string literal. The valid escapes are `\n \t \r \0 \\ \' \"`
    /// and `\xHH` (two hex digits); an unknown escape letter or a
    /// malformed `\x` (non-hex or fewer than two digits) is rejected
    /// at parse time, where the span bytes are first decoded.
    #[error("invalid escape sequence in literal `{text}`")]
    #[diagnostic(
        code(sentinel::parse::invalid_escape),
        help("valid escapes are \\n \\t \\r \\0 \\\\ \\' \\\" and \\xHH (two hex digits) per ADR 0033 D2")
    )]
    InvalidEscape {
        text: String,
        #[label("invalid escape here")]
        span: miette::SourceSpan,
    },

    /// Review F11 / P2.5: the expression nesting exceeded
    /// [`MAX_EXPR_DEPTH`]. The recursive-descent parser would otherwise
    /// stack-overflow (a denial-of-service on adversarial input — e.g. a
    /// few hundred nested `(`); this turns the crash into a clean rejection.
    /// No legitimate program nests expressions remotely this deep (binary
    /// chains are iterative, not recursive — only true nesting counts).
    #[error("expression nested too deeply (limit {MAX_EXPR_DEPTH})")]
    #[diagnostic(
        code(sentinel::parse::recursion_limit),
        help("the expression is nested past the parser's depth limit; flatten it")
    )]
    RecursionLimit {
        #[label("nesting exceeds the depth limit here")]
        span: miette::SourceSpan,
    },
}

/// Review F11 / P2.5: the maximum expression-nesting depth the recursive-
/// descent parser will recurse to before returning [`ParseError::RecursionLimit`]
/// instead of risking a stack overflow. Generous for any real program (genuine
/// nesting — parens, nested calls, nested `if`/`match` — is rarely past ~20;
/// binary/postfix chains are iterative), and well under the overflow threshold
/// even on a small (2 MiB) thread stack.
const MAX_EXPR_DEPTH: usize = 128;

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
    /// Per ADR 0013 D3a: when false, `Name { ... }` parses as just
    /// `Name` (a Var atom) followed by separate tokens — never as a
    /// struct literal. Set to false while parsing an `if` condition;
    /// parens / let-RHS / fn-call args / etc. restore it to true.
    allow_struct_lit: bool,
    /// Review F11 / P2.5: current expression-nesting depth, bounded by
    /// [`MAX_EXPR_DEPTH`] in [`Parser::parse_expr`] so adversarial deep
    /// nesting is rejected, not a stack overflow.
    depth: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, tokens: &'a [Spanned<TokenKind>]) -> Self {
        Self { src, tokens, pos: 0, allow_struct_lit: true, depth: 0 }
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
        let mut uses = Vec::new();
        let mut fns = Vec::new();
        let mut structs = Vec::new();
        let mut effects = Vec::new();
        let mut classes = Vec::new();
        let mut traits = Vec::new();
        let mut impls = Vec::new();
        let mut enums = Vec::new();
        while self.peek().is_some() {
            // Phase D.6 (1/N) / ADR 0037 D3: an optional `pub` precedes a
            // top-level item, exporting it across modules. Allowed on the
            // importable items (`fn`/`struct`/`enum`/`trait`/`effect`, D2);
            // `pub` on `use` (re-export) / `class` / `impl` is rejected at
            // the MVP (deferred, ADR 0037 D8).
            let visibility = self.parse_optional_visibility();
            match self.peek_kind() {
                // Phase D.6 (1/N) / ADR 0037: top-level `use a::b::Item;`.
                Some(TokenKind::Use) => {
                    self.reject_top_level_pub(visibility, "use")?;
                    uses.push(self.parse_use_decl()?);
                }
                Some(TokenKind::Fn) => fns.push(self.parse_fn_def(visibility)?),
                Some(TokenKind::Struct) => structs.push(self.parse_struct_decl(visibility)?),
                Some(TokenKind::Effect) => effects.push(self.parse_effect_decl(visibility)?),
                Some(TokenKind::Class) => {
                    self.reject_top_level_pub(visibility, "class")?;
                    classes.push(self.parse_class_decl()?);
                }
                Some(TokenKind::Trait) => traits.push(self.parse_trait_decl(visibility)?),
                Some(TokenKind::Impl) => {
                    self.reject_top_level_pub(visibility, "impl")?;
                    impls.push(self.parse_impl_decl()?);
                }
                // Phase D.1 / ADR 0032: top-level `enum` declarations.
                Some(TokenKind::Enum) => enums.push(self.parse_enum_decl(visibility)?),
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`use`, `fn`, `struct`, `effect`, `class`, `trait`, `impl`, or `enum`",
                        span: to_source_span(&t.span),
                    });
                }
                // Review F11 / P2.5: NOT unreachable — `parse_optional_visibility`
                // consumes a `pub`, so a trailing `pub` with no item following hits
                // EOF here (the fuzzer found `"pub"` alone panicked). Reject cleanly.
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "an item (`fn`, `struct`, `effect`, `trait`, or `enum`) after `pub`",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }
        if fns.is_empty()
            && structs.is_empty()
            && effects.is_empty()
            && classes.is_empty()
            && traits.is_empty()
            && impls.is_empty()
            && enums.is_empty()
        {
            return Err(ParseError::UnexpectedEof {
                expected: "`fn` (programs are one or more function definitions)",
                span: to_source_span(&self.eof_span()),
            });
        }
        // End-of-program span = end of the last item (whichever is latest).
        let fn_end = fns.last().map_or(0, |f| f.span.end);
        let struct_end = structs.last().map_or(0, |s| s.span.end);
        let effect_end = effects.last().map_or(0, |e| e.span.end);
        let class_end = classes.last().map_or(0, |c| c.span.end);
        let trait_end = traits.last().map_or(0, |t| t.span.end);
        let impl_end = impls.last().map_or(0, |i| i.span.end);
        let enum_end = enums.last().map_or(0, |e| e.span.end);
        let end = fn_end
            .max(struct_end)
            .max(effect_end)
            .max(class_end)
            .max(trait_end)
            .max(impl_end)
            .max(enum_end);
        Ok(Program {
            uses,
            fns,
            structs,
            effects,
            classes,
            traits,
            impls,
            enums,
            span: start..end,
        })
    }

    /// Phase D.6 (1/N) / ADR 0037 D2: parse a top-level import
    /// `use a::b::Item;` — one or more `::`-separated identifier segments
    /// followed by `;`. The whole segment list (including the trailing
    /// item) is stored as written; resolve splits module path from item
    /// (the last segment is the item). No globs / groups / aliases at the
    /// MVP (ADR 0037 D8).
    fn parse_use_decl(&mut self) -> Result<UseDecl, ParseError> {
        let use_start = match self.peek_kind() {
            Some(TokenKind::Use) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Use"),
        };
        let mut path = Vec::new();
        // First segment.
        let first = self.peek().ok_or_else(|| ParseError::UnexpectedEof {
            expected: "identifier after `use`",
            span: to_source_span(&self.eof_span()),
        })?;
        if first.kind != TokenKind::Ident {
            let kind = first.kind;
            let span = first.span.clone();
            return Err(ParseError::UnexpectedToken {
                got: format!("{kind:?}"),
                expected: "identifier after `use`",
                span: to_source_span(&span),
            });
        }
        path.push(self.src[first.span.clone()].to_string());
        self.advance();
        // Subsequent `:: Ident` segments.
        while self.peek_kind() == Some(TokenKind::ColonColon) {
            self.advance();
            let seg = self.peek().ok_or_else(|| ParseError::UnexpectedEof {
                expected: "identifier after `::` in a `use` path",
                span: to_source_span(&self.eof_span()),
            })?;
            if seg.kind != TokenKind::Ident {
                let kind = seg.kind;
                let span = seg.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "identifier after `::` in a `use` path",
                    span: to_source_span(&span),
                });
            }
            path.push(self.src[seg.span.clone()].to_string());
            self.advance();
        }
        // A bare `use foo;` (single segment, no item to import) is
        // meaningless under file-as-module (you import an item, not a
        // module); require at least `module::item` (≥ 2 segments).
        if path.len() < 2 {
            return Err(ParseError::UnexpectedToken {
                got: "`;`".to_string(),
                expected: "`::` then an item name (`use module::Item;`)",
                span: to_source_span(&(use_start..use_start + 3)),
            });
        }
        let semi_end = match self.peek_kind() {
            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`;` after a `use` import",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`;` after a `use` import",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        Ok(UseDecl { path, span: use_start..semi_end })
    }

    /// Parse a top-level struct declaration per ADR 0013 D1:
    ///
    /// ```text
    /// struct_decl  = 'struct' Ident '{' field_list '}'
    /// field_list   = (field (',' field)*)? ','?
    /// field        = Ident ':' type
    /// ```
    fn parse_struct_decl(&mut self, visibility: Visibility) -> Result<StructDecl, ParseError> {
        let struct_start = match self.peek_kind() {
            Some(TokenKind::Struct) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Struct"),
        };

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
                    expected: "struct name after `struct`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "struct name after `struct`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // Optional `<T1, T2, ...>` generic parameter list per ADR
        // 0016 D2. Empty `<>` is rejected by the helper.
        let type_params = if self.peek_kind() == Some(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open struct body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open struct body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Field list (possibly empty, trailing comma allowed).
        let mut fields = Vec::new();
        if self.peek_kind() != Some(TokenKind::RBrace) {
            fields.push(self.parse_struct_field()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break; // trailing comma allowed
                }
                fields.push(self.parse_struct_field()?);
            }
        }

        let rbrace_end = match self.peek_kind() {
            Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `}` in struct body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `}` in struct body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(StructDecl {
            visibility,
            name,
            name_span,
            type_params,
            fields,
            span: struct_start..rbrace_end,
        })
    }

    /// Phase D.1 / ADR 0032: parse a top-level enum declaration:
    ///
    /// ```text
    /// enum_decl    = 'enum' Ident '{' variant_list '}'
    /// variant_list = (variant (',' variant)*)? ','?
    /// variant      = Ident ('(' type (',' type)* ')')?
    /// ```
    ///
    /// Non-generic at the D.1 MVP (generic enums are a fast-follow per
    /// ADR 0032 D9). Resolve + types check `enum`s at D.1 (3/N); codegen
    /// lowers construction + `match` at D.1 (4/N).
    fn parse_enum_decl(&mut self, visibility: Visibility) -> Result<EnumDecl, ParseError> {
        let enum_start = match self.peek_kind() {
            Some(TokenKind::Enum) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Enum"),
        };

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
                    expected: "enum name after `enum`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "enum name after `enum`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open enum body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open enum body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut variants = Vec::new();
        if self.peek_kind() != Some(TokenKind::RBrace) {
            variants.push(self.parse_variant()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break; // trailing comma allowed
                }
                variants.push(self.parse_variant()?);
            }
        }

        let rbrace_end = match self.peek_kind() {
            Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `}` in enum body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `}` in enum body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(EnumDecl {
            visibility,
            name,
            name_span,
            variants,
            span: enum_start..rbrace_end,
        })
    }

    /// Parse one enum variant: `Ident` (unit) or
    /// `Ident '(' type (',' type)* ')'` (positional tuple payload).
    fn parse_variant(&mut self) -> Result<VariantDecl, ParseError> {
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
                    expected: "variant name in enum body",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "variant name in enum body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        let v_start = name_span.start;
        let mut end = name_span.end;
        let mut payloads = Vec::new();
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.advance();
            if self.peek_kind() != Some(TokenKind::RParen) {
                payloads.push(self.parse_type()?);
                while self.peek_kind() == Some(TokenKind::Comma) {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break; // trailing comma allowed
                    }
                    payloads.push(self.parse_type()?);
                }
            }
            end = match self.peek_kind() {
                Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`,` or `)` in variant payload",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`,` or `)` in variant payload",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            };
        }

        Ok(VariantDecl {
            name,
            name_span,
            payloads,
            span: v_start..end,
        })
    }

    /// Parse a C4.1 class declaration per ADR 0021 D1 + ADR 0022
    /// D1:
    ///
    /// ```text
    /// class_decl    = 'class' Ident '{' class_item* '}'
    /// class_item    = field_decl | init_decl | method_decl
    /// field_decl    = ('pub')? 'let' Ident ':' type_expr ';'
    /// init_decl     = ('pub')? 'init' '(' params? ')' block
    /// method_decl   = ('pub')? 'fn' Ident '(' self_param (',' param)*
    ///                  ')' ('->' type_expr)? effect_row? block
    /// self_param    = 'self' ':' '&' 'mut'? 'Self'
    /// ```
    ///
    /// Per ADR 0022 D4 at most one `init` is permitted; a second
    /// `init` surfaces as [`ParseError::DuplicateClassInit`].
    /// Generic classes (`class Pair<A, B>`) are deferred per ADR
    /// 0022 D1 — type-params position is rejected here with the
    /// generic-classes-not-yet diagnostic via reuse of the
    /// existing [`ParseError::UnexpectedToken`].
    fn parse_class_decl(&mut self) -> Result<ClassDecl, ParseError> {
        let class_start = match self.peek_kind() {
            Some(TokenKind::Class) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Class"),
        };

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
                    expected: "class name after `class`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "class name after `class`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // Generic classes (`class Pair<A, B>`) deferred per ADR
        // 0022 D1. Surface an explicit error rather than parsing
        // partway.
        if self.peek_kind() == Some(TokenKind::Lt) {
            let t = self.peek().expect("peeked");
            return Err(ParseError::UnexpectedToken {
                got: "Lt".to_string(),
                expected: "`{` (generic classes deferred per ADR 0022 D1)",
                span: to_source_span(&t.span),
            });
        }

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open class body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open class body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut fields = Vec::new();
        let mut init: Option<InitDef> = None;
        let mut methods = Vec::new();
        let mut delegates: Vec<DelegateDecl> = Vec::new();

        while self.peek_kind() != Some(TokenKind::RBrace) {
            // Optional `pub` visibility prefix per ADR 0022 D2.
            let visibility = self.parse_optional_visibility();
            match self.peek_kind() {
                Some(TokenKind::Let) => {
                    fields.push(self.parse_class_field(visibility)?);
                }
                Some(TokenKind::Init) => {
                    let init_def = self.parse_init_decl(visibility)?;
                    if init.is_some() {
                        return Err(ParseError::DuplicateClassInit {
                            class_name: name.clone(),
                            span: to_source_span(&init_def.span),
                        });
                    }
                    init = Some(init_def);
                }
                Some(TokenKind::Fn) => {
                    methods.push(self.parse_method_decl(visibility)?);
                }
                Some(TokenKind::Delegate) => {
                    delegates.push(self.parse_delegate_decl(visibility)?);
                }
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`let`, `init`, `fn`, or `delegate` inside class body",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "class item or `}`",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }

        let rbrace_end = self.advance().expect("peeked == RBrace").span.end;

        Ok(ClassDecl {
            name,
            name_span,
            fields,
            init,
            methods,
            delegates,
            span: class_start..rbrace_end,
        })
    }

    /// Optional `pub` keyword consumer used inside class bodies.
    /// Returns [`Visibility::Public`] if `pub` is present (and
    /// advances past it); otherwise returns [`Visibility::Private`]
    /// without advancing.
    fn parse_optional_visibility(&mut self) -> Visibility {
        if self.peek_kind() == Some(TokenKind::Ident) {
            if let Some(t) = self.peek() {
                if &self.src[t.span.clone()] == "pub" {
                    self.advance();
                    return Visibility::Public;
                }
            }
        }
        Visibility::Private
    }

    /// Phase D.6 (1/N) / ADR 0037 D3: reject a `pub` that precedes a
    /// top-level item which does not take visibility at the MVP — `use`
    /// (a `pub use` re-export), `class`, or `impl` (deferred, ADR 0037 D8).
    /// `pub` on `fn`/`struct`/`enum`/`trait`/`effect` is accepted (D2). The
    /// span points at the item keyword following the consumed `pub`.
    fn reject_top_level_pub(
        &self,
        visibility: Visibility,
        item: &'static str,
    ) -> Result<(), ParseError> {
        if visibility == Visibility::Public {
            let span = self.peek().map(|t| t.span.clone()).unwrap_or_else(|| self.eof_span());
            return Err(ParseError::UnexpectedToken {
                got: format!("pub {item}"),
                expected: "`pub` only on `fn` / `struct` / `enum` / `trait` / `effect`",
                span: to_source_span(&span),
            });
        }
        Ok(())
    }

    /// Parse a single field declaration inside a class body:
    /// `'let' Ident ':' type_expr ';'`. Optional `pub` is
    /// consumed by [`parse_optional_visibility`] before this is
    /// called.
    fn parse_class_field(
        &mut self,
        visibility: Visibility,
    ) -> Result<ClassField, ParseError> {
        let let_start = match self.peek_kind() {
            Some(TokenKind::Let) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Let"),
        };

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
                    expected: "field name after `let`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "field name after `let`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` after class field name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` after class field name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let ty = self.parse_type()?;

        let semi_end = match self.peek_kind() {
            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`;` after class field declaration",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`;` after class field declaration",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(ClassField {
            visibility,
            name,
            name_span,
            ty,
            span: let_start..semi_end,
        })
    }

    /// Parse a C4.3 delegation declaration inside a class body
    /// per ADR 0021 D6:
    ///
    /// ```text
    /// delegate_decl = 'delegate' Ident ':' type_expr 'to' Ident ';'
    /// ```
    ///
    /// `to` is a positional Ident (per C4.0 lexer state — kept
    /// as a plain Ident per the smallest-surface principle).
    /// Optional `pub` prefix is consumed by
    /// [`parse_optional_visibility`] before this is called.
    /// Resolve synthesizes the auto-forwarder impl at name-
    /// resolution time; downstream passes see a regular impl
    /// block + field.
    fn parse_delegate_decl(
        &mut self,
        visibility: Visibility,
    ) -> Result<DelegateDecl, ParseError> {
        let kw_start = match self.peek_kind() {
            Some(TokenKind::Delegate) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Delegate"),
        };

        let (field_name, field_name_span) = match self.peek() {
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
                    expected: "field name after `delegate`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "field name after `delegate`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` after delegate field name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` after delegate field name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let ty = self.parse_type()?;

        // Positional `to` keyword (a plain Ident at the lexer
        // level per C4.0 D11).
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident && &self.src[t.span.clone()] == "to" => {
                self.advance();
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "`to` after delegate type",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`to` after delegate type",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let (trait_name, trait_name_span) = match self.peek() {
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
                    expected: "trait name after `to`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "trait name after `to`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        let semi_end = match self.peek_kind() {
            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`;` after delegate declaration",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`;` after delegate declaration",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(DelegateDecl {
            visibility,
            field_name,
            field_name_span,
            ty,
            trait_name,
            trait_name_span,
            span: kw_start..semi_end,
        })
    }

    /// Parse a C4.4 `scope concurrent { ... }` expression per ADR
    /// 0024 D1. `concurrent` is a positional Ident (kept as a plain
    /// Ident at the C4.0 lexer per the smallest-surface principle).
    /// Other scope modes (`sequential`, `race`) are reserved for
    /// future ADRs.
    fn parse_scope_expr(&mut self) -> Result<Expr, ParseError> {
        let kw_start = match self.peek_kind() {
            Some(TokenKind::Scope) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Scope"),
        };
        // Positional `concurrent` mode keyword (an Ident token).
        match self.peek() {
            Some(t)
                if t.kind == TokenKind::Ident && &self.src[t.span.clone()] == "concurrent" =>
            {
                self.advance();
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "`concurrent` after `scope`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`concurrent` after `scope`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        // Block body. Reuses the standard parse_block; the trailing
        // expression is the scope's value at the surface level.
        let body = self.parse_block()?;
        let body_span_end = body.span.end;
        Ok(Spanned {
            kind: ExprKind::Scope {
                mode: sentinel_ast::ScopeMode::Concurrent,
                body: Box::new(body),
            },
            span: kw_start..body_span_end,
        })
    }

    /// Parse a C4.4 `spawn expr` expression per ADR 0024 D2. The
    /// inner expression is restricted to a function-call shape at
    /// C4.4 minimum (validated at the resolve / types layer);
    /// here the parser accepts any expression — narrowing happens
    /// downstream.
    fn parse_spawn_expr(&mut self) -> Result<Expr, ParseError> {
        let kw_start = match self.peek_kind() {
            Some(TokenKind::Spawn) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Spawn"),
        };
        // Use parse_postfix so the call's argument list parses;
        // arbitrary expressions are accepted at the parser level
        // (types layer rejects non-call shapes per ADR 0024 D2).
        let call_expr = self.parse_postfix()?;
        let span = kw_start..call_expr.span.end;
        Ok(Spanned {
            kind: ExprKind::Spawn {
                call_expr: Box::new(call_expr),
            },
            span,
        })
    }

    /// Parse a C4.2 trait declaration per ADR 0021 D4 + ADR 0023
    /// D1:
    ///
    /// ```text
    /// trait_decl    = 'trait' Ident '{' method_sig* '}'
    /// method_sig    = 'fn' Ident '(' self_param (',' param)* ')'
    ///                 ('->' type_expr)? ('!' effect_row)? ';'
    /// ```
    ///
    /// Empty trait declarations (`trait T {}`) are allowed
    /// structurally — they're marker traits at C4.2.
    fn parse_trait_decl(&mut self, visibility: Visibility) -> Result<TraitDecl, ParseError> {
        let trait_start = match self.peek_kind() {
            Some(TokenKind::Trait) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Trait"),
        };

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
                    expected: "trait name after `trait`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "trait name after `trait`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open trait body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open trait body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut methods: Vec<TraitMethodSig> = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokenKind::RBrace) => break,
                Some(TokenKind::Fn) => {
                    methods.push(self.parse_trait_method_sig()?);
                }
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`fn` (trait method signature) or `}` to close trait body",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`fn` or `}` in trait body",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }

        let rbrace_end = self.advance().expect("RBrace").span.end;

        Ok(TraitDecl {
            visibility,
            name,
            name_span,
            methods,
            span: trait_start..rbrace_end,
        })
    }

    /// Parse a single trait method signature per ADR 0023 D2.
    /// Same shape as a class method declaration except terminated
    /// with `;` instead of a `block`.
    fn parse_trait_method_sig(&mut self) -> Result<TraitMethodSig, ParseError> {
        let fn_start = match self.peek_kind() {
            Some(TokenKind::Fn) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Fn"),
        };

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
                    expected: "trait method name after `fn`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "trait method name after `fn`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after trait method name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after trait method name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let self_kind = self.parse_self_param()?;

        let mut params = Vec::new();
        if self.peek_kind() == Some(TokenKind::Comma) {
            self.advance();
            if self.peek_kind() != Some(TokenKind::RParen) {
                params.push(self.parse_param()?);
                while self.peek_kind() == Some(TokenKind::Comma) {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_param()?);
                }
            }
        }

        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in trait method parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in trait method parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let return_type = self.parse_return_type()?;
        let effect_row = self.parse_optional_effect_row()?;

        let semi_end = match self.peek_kind() {
            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`;` after trait method signature (trait methods have no body at C4.2)",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`;` after trait method signature",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(TraitMethodSig {
            name,
            name_span,
            self_kind,
            params,
            return_type,
            effect_row,
            span: fn_start..semi_end,
        })
    }

    /// Parse a C4.2 impl declaration per ADR 0021 D5 + ADR 0023
    /// D3+D4:
    ///
    /// ```text
    /// impl_decl     = 'impl' Ident? 'as' trait_ident 'for' type_ident
    ///                 '{' method_def* '}'
    /// ```
    ///
    /// The optional `Ident` before `as` is the impl name; when
    /// absent, the impl is the default for `(Trait, Type)` in
    /// the current scope per ADR 0023 D3.
    fn parse_impl_decl(&mut self) -> Result<ImplDecl, ParseError> {
        let impl_start = match self.peek_kind() {
            Some(TokenKind::Impl) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Impl"),
        };

        // Optional impl name: an Ident that is NOT followed by
        // `as`-keyword-position. The parser disambiguates by
        // checking: if the current Ident is followed by `as`, the
        // Ident is the impl name; otherwise the current `as` should
        // be at the front (which means no name). Since `as` is a
        // distinct keyword (TokenKind::As) we can check directly.
        let (name, name_span) = if let Some(TokenKind::Ident) = self.peek_kind() {
            let t = self.peek().expect("peeked");
            let span = t.span.clone();
            let n = self.src[span.clone()].to_string();
            self.advance();
            (Some(n), Some(span))
        } else {
            (None, None)
        };

        // `as`
        match self.peek_kind() {
            Some(TokenKind::As) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`as` (impl Name? as Trait for Type)",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`as` after `impl`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Trait name (Ident).
        let (trait_name, trait_name_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let n = self.src[span.clone()].to_string();
                self.advance();
                (n, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "trait name after `as`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "trait name after `as`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // `for`
        match self.peek_kind() {
            Some(TokenKind::For) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`for` (impl Trait for Type)",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`for` after trait name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Type name (Ident at C4.2 minimum — generic types deferred per
        // ADR 0023 D3).
        let (type_name, type_name_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let n = self.src[span.clone()].to_string();
                self.advance();
                (n, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "type name after `for`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "type name after `for`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // `{`
        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open impl body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open impl body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut methods: Vec<ImplMethodDef> = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            // Optional `pub` visibility per ADR 0022 D2 / ADR 0023 D3.
            let visibility = self.parse_optional_visibility();
            match self.peek_kind() {
                Some(TokenKind::Fn) => {
                    methods.push(self.parse_impl_method_def(visibility)?);
                }
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`fn` (impl method) or `}` to close impl body",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`fn` or `}` in impl body",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }

        let rbrace_end = self.advance().expect("peeked == RBrace").span.end;

        Ok(ImplDecl {
            name,
            name_span,
            trait_name,
            trait_name_span,
            type_name,
            type_name_span,
            methods,
            span: impl_start..rbrace_end,
        })
    }

    /// Parse a single impl method definition per ADR 0023 D3 —
    /// identical shape to a class method (`parse_method_decl`)
    /// but in an impl context. Visibility is already consumed by
    /// the caller.
    fn parse_impl_method_def(
        &mut self,
        visibility: Visibility,
    ) -> Result<ImplMethodDef, ParseError> {
        let fn_start = match self.peek_kind() {
            Some(TokenKind::Fn) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Fn"),
        };

        let (name, name_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let n = self.src[span.clone()].to_string();
                self.advance();
                (n, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "impl method name after `fn`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "impl method name after `fn`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after impl method name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after impl method name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let self_kind = self.parse_self_param()?;

        let mut params = Vec::new();
        if self.peek_kind() == Some(TokenKind::Comma) {
            self.advance();
            if self.peek_kind() != Some(TokenKind::RParen) {
                params.push(self.parse_param()?);
                while self.peek_kind() == Some(TokenKind::Comma) {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_param()?);
                }
            }
        }

        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in impl method parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in impl method parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let return_type = self.parse_return_type()?;
        let effect_row = self.parse_optional_effect_row()?;
        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(ImplMethodDef {
            visibility,
            name,
            name_span,
            self_kind,
            params,
            return_type,
            effect_row,
            body,
            span: fn_start..end,
        })
    }

    /// Parse an `init(params) { body }` constructor inside a
    /// class body per ADR 0022 D4.
    fn parse_init_decl(&mut self, visibility: Visibility) -> Result<InitDef, ParseError> {
        let init_start = match self.peek_kind() {
            Some(TokenKind::Init) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Init"),
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
                    expected: "`(` after `init`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after `init`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Param list (no `self` — `init` has no self receiver).
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }

        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in init parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in init parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Body — no return-type annotation per ADR 0022 D4.
        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(InitDef {
            visibility,
            params,
            body,
            span: init_start..end,
        })
    }

    /// Parse a method declaration inside a class body per ADR
    /// 0022 D3: `fn name(self: &Self/&mut Self, params*) ->
    /// return_type effect_row? block`.
    fn parse_method_decl(
        &mut self,
        visibility: Visibility,
    ) -> Result<MethodDef, ParseError> {
        let fn_start = match self.peek_kind() {
            Some(TokenKind::Fn) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Fn"),
        };

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
                    expected: "method name after `fn`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "method name after `fn`",
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
                    expected: "`(` after method name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after method name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // First param MUST be `self: &Self` or `self: &mut Self`.
        let self_kind = self.parse_self_param()?;

        // Remaining params.
        let mut params = Vec::new();
        if self.peek_kind() == Some(TokenKind::Comma) {
            self.advance();
            if self.peek_kind() != Some(TokenKind::RParen) {
                params.push(self.parse_param()?);
                while self.peek_kind() == Some(TokenKind::Comma) {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_param()?);
                }
            }
        }

        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in method parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in method parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let return_type = self.parse_return_type()?;
        let effect_row = self.parse_optional_effect_row()?;
        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(MethodDef {
            visibility,
            name,
            name_span,
            self_kind,
            params,
            return_type,
            effect_row,
            body,
            span: fn_start..end,
        })
    }

    /// Parse the mandatory `self: &Self` or `self: &mut Self`
    /// receiver clause at the start of a method parameter list
    /// per ADR 0022 D3.
    fn parse_self_param(&mut self) -> Result<SelfKind, ParseError> {
        // `self`
        match self.peek_kind() {
            Some(TokenKind::SelfVal) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`self` as first method parameter",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`self` as first method parameter",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // `:`
        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` after `self`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` after `self`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // `&`
        match self.peek_kind() {
            Some(TokenKind::Amp) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`&` (self must be `&Self` or `&mut Self`)",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`&` (self must be `&Self` or `&mut Self`)",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Optional `mut`
        let kind = if self.peek_kind() == Some(TokenKind::Mut) {
            self.advance();
            SelfKind::Exclusive
        } else {
            SelfKind::Shared
        };

        // `Self`
        match self.peek_kind() {
            Some(TokenKind::SelfTy) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`Self` (the implementing type)",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`Self`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        Ok(kind)
    }

    /// Parse a single `field: expr` clause inside a struct literal
    /// per ADR 0013 D3.
    fn parse_field_init(&mut self) -> Result<FieldInit, ParseError> {
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
                    expected: "field name in struct literal",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "field name in struct literal",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` followed by field value",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` followed by field value",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let value = self.parse_expr()?;
        let span_end = value.span.end;
        Ok(FieldInit {
            name,
            name_span: name_span.clone(),
            value,
            span: name_span.start..span_end,
        })
    }

    fn parse_struct_field(&mut self) -> Result<StructField, ParseError> {
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
                    expected: "field name",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "field name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`:` followed by field type",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`:` followed by field type",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let ty = self.parse_type()?;
        let span_end = ty.span.end;
        Ok(StructField {
            name,
            name_span: name_span.clone(),
            ty,
            span: name_span.start..span_end,
        })
    }

    fn parse_fn_def(&mut self, visibility: Visibility) -> Result<FnDef, ParseError> {
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

        // Optional `<T1, T2, ...>` generic parameter list per ADR
        // 0016 D1. Empty `<>` is rejected by the helper.
        let type_params = if self.peek_kind() == Some(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
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

        // Optional postfix effect-row annotation per ADR 0019 D1
        // (C3.0): `! { Op1, Op2 }`. The `!` token doubles as
        // logical-not (C1.3) — parser disambiguates positionally:
        // at fn-signature position immediately after the return
        // type, `!` opens the effect-row annotation.
        let effect_row = self.parse_optional_effect_row()?;

        // Body.
        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(FnDef {
            visibility,
            name,
            name_span,
            type_params,
            params,
            return_type,
            effect_row,
            body,
            span: fn_start..end,
        })
    }

    /// Parse an optional postfix effect-row annotation per ADR
    /// 0019 D1: `! { Op1, Op2, ... }`. Trailing comma allowed.
    /// Empty annotation `! { }` is rejected at parse (write
    /// no annotation at all to mean "no effects"). Returns the
    /// (possibly empty) list of effect-label spans.
    fn parse_optional_effect_row(
        &mut self,
    ) -> Result<Vec<Spanned<String>>, ParseError> {
        if self.peek_kind() != Some(TokenKind::Bang) {
            return Ok(Vec::new());
        }
        let bang_start = self.advance().expect("peeked").span.start;
        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` after `!` in fn effect-row annotation",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` after `!` in fn effect-row annotation",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let mut effects: Vec<Spanned<String>> = Vec::new();
        if self.peek_kind() != Some(TokenKind::RBrace) {
            effects.push(self.parse_effect_label()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break; // trailing comma allowed
                }
                effects.push(self.parse_effect_label()?);
            }
        }
        // RBrace
        match self.peek_kind() {
            Some(TokenKind::RBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `}` in fn effect-row annotation",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `}` in fn effect-row annotation",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        if effects.is_empty() {
            return Err(ParseError::EmptyEffectAnnotation {
                span: to_source_span(&(bang_start..bang_start + 1)),
            });
        }
        Ok(effects)
    }

    /// Parse a single effect-label identifier inside an effect-row
    /// annotation or inside an `effect E { ... }` declaration's
    /// op-name position. Returns the spanned name.
    fn parse_effect_label(&mut self) -> Result<Spanned<String>, ParseError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let name = self.src[span.clone()].to_string();
                self.advance();
                Ok(Spanned { kind: name, span })
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "effect-label identifier",
                    span: to_source_span(&span),
                })
            }
            None => Err(ParseError::UnexpectedEof {
                expected: "effect-label identifier",
                span: to_source_span(&self.eof_span()),
            }),
        }
    }

    /// Parse a top-level effect declaration per ADR 0019 D4:
    ///
    /// ```text
    /// effect_decl  = 'effect' Ident '{' op_decl (';' op_decl)* ';'? '}'
    /// op_decl      = Ident '(' params ')' ('->' type)?
    /// ```
    ///
    /// Empty effect declarations (zero ops) are allowed.
    fn parse_effect_decl(&mut self, visibility: Visibility) -> Result<EffectDecl, ParseError> {
        let effect_start = match self.peek_kind() {
            Some(TokenKind::Effect) => self.advance().expect("peeked").span.start,
            _ => unreachable!("called only after peek_kind() == Effect"),
        };

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
                    expected: "effect name after `effect`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "effect name after `effect`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open effect body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open effect body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut ops = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            ops.push(self.parse_op_decl()?);
            // Each op must be `;`-terminated (allows trailing `;`).
            match self.peek_kind() {
                Some(TokenKind::Semi) => {
                    self.advance();
                }
                Some(TokenKind::RBrace) => break,
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`;` after op declaration",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`;` after op declaration",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }
        let end = self.advance().expect("RBrace peeked").span.end;

        Ok(EffectDecl {
            visibility,
            name,
            name_span,
            ops,
            span: effect_start..end,
        })
    }

    /// Parse a single op declaration inside an `effect E { ... }`
    /// body: `name(p1: T1, ...) -> RetT`. The `-> RetT` clause is
    /// optional per ADR 0019 D4 (default to no explicit return
    /// type; types layer treats absence as `i64` for now).
    fn parse_op_decl(&mut self) -> Result<OpDecl, ParseError> {
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
                    expected: "op name",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "op name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let op_start = name_span.start;

        // `(`
        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after op name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after op name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        // `)`
        let mut end = match self.peek_kind() {
            Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in op parameter list",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in op parameter list",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // Optional `-> RetT`
        let return_type = if self.peek_kind() == Some(TokenKind::Arrow) {
            self.advance();
            let ty = self.parse_type()?;
            end = ty.span.end;
            Some(ty)
        } else {
            None
        };

        Ok(OpDecl {
            name,
            name_span,
            params,
            return_type,
            span: op_start..end,
        })
    }

    /// Parse a `<T1, T2, ...>` type-parameter list per ADR 0016 D1
    /// / D2. Called when the next token is known to be `<`. Returns
    /// the parsed list, which is guaranteed non-empty (empty `<>`
    /// surfaces as [`ParseError::EmptyTypeParams`]).
    fn parse_type_params(&mut self) -> Result<Vec<TypeParam>, ParseError> {
        let lt_start = self.advance().expect("`<` peeked").span.start;
        let mut params: Vec<TypeParam> = Vec::new();
        if self.peek_kind() == Some(TokenKind::Gt) {
            // Empty `<>`.
            let gt = self.peek().expect("peeked");
            let span = lt_start..gt.span.end;
            return Err(ParseError::EmptyTypeParams { span: to_source_span(&span) });
        }
        loop {
            let (name, span) = match self.peek() {
                Some(t) if t.kind == TokenKind::Ident => {
                    let s = t.span.clone();
                    let n = self.src[s.clone()].to_string();
                    self.advance();
                    (n, s)
                }
                Some(t) => {
                    let kind = t.kind;
                    let s = t.span.clone();
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{kind:?}"),
                        expected: "type parameter name",
                        span: to_source_span(&s),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "type parameter name",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            };
            params.push(TypeParam { name, name_span: span });
            match self.peek_kind() {
                Some(TokenKind::Comma) => {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::Gt) {
                        break; // trailing comma allowed
                    }
                }
                Some(TokenKind::Gt) => break,
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`,` or `>` in type parameter list",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`,` or `>` in type parameter list",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }
        // Consume the closing `>`.
        match self.peek_kind() {
            Some(TokenKind::Gt) => {
                self.advance();
            }
            _ => unreachable!("loop only breaks on Gt"),
        }
        Ok(params)
    }

    /// Parse a `<TypeArg1, TypeArg2, ...>` type-argument list per
    /// ADR 0016 D3. Called when the next token is known to be `<`
    /// in type-position. Returns the parsed list, guaranteed non-
    /// empty (empty `<>` surfaces as [`ParseError::EmptyTypeArgs`]).
    fn parse_type_args(&mut self) -> Result<(Vec<TypeExpr>, Span), ParseError> {
        let lt_start = self.advance().expect("`<` peeked").span.start;
        let mut args: Vec<TypeExpr> = Vec::new();
        if self.peek_kind() == Some(TokenKind::Gt) {
            let gt = self.peek().expect("peeked");
            let span = lt_start..gt.span.end;
            return Err(ParseError::EmptyTypeArgs { span: to_source_span(&span) });
        }
        loop {
            let arg = self.parse_type()?;
            args.push(arg);
            match self.peek_kind() {
                Some(TokenKind::Comma) => {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::Gt) {
                        break;
                    }
                }
                Some(TokenKind::Gt) => break,
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`,` or `>` in type argument list",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`,` or `>` in type argument list",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }
        let gt_end = match self.peek_kind() {
            Some(TokenKind::Gt) => self.advance().expect("peeked").span.end,
            _ => unreachable!("loop only breaks on Gt"),
        };
        Ok((args, lt_start..gt_end))
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

    /// Parse a [`TypeExpr`]. C1.2 shipped only the `Ident` form;
    /// C1.4 widened to recognize struct names; C1.5 adds the
    /// `?T` prefix per ADR 0014 D1; C1.6 adds the `[T]` array
    /// form per ADR 0015 D1. Nested `??T` is rejected at parse
    /// time per ADR 0014 D6; nested `[[T]]` parses at the AST
    /// level and is rejected at the type-resolution stage per
    /// ADR 0015 D6.
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        // C3 / ADR 0019 D5: `secret T` type prefix. Nested
        // `secret secret T` rejected at parse time per D5; depth-1
        // qualifier matching C1.5's `??T` rule.
        if self.peek_kind() == Some(TokenKind::Secret) {
            let s_start = self.advance().expect("peeked").span.start;
            if self.peek_kind() == Some(TokenKind::Secret) {
                let t = self.peek().expect("peeked");
                return Err(ParseError::DoubleSecret {
                    span: to_source_span(&t.span),
                });
            }
            let inner = self.parse_type()?;
            let span = s_start..inner.span.end;
            return Ok(Spanned {
                kind: TypeExprKind::Secret(Box::new(inner)),
                span,
            });
        }
        // C2 / ADR 0017 D1: `&T` and `&mut T` reference types. The
        // optional `mut` keyword between `&` and the inner type
        // marks the borrow as exclusive. Nested refs (`&&T`) parse
        // syntactically but are rejected at type-resolve time with
        // [`TypeError::NestedRef`] (the depth-1 amendment of ADR
        // 0017 D11). Note: `&&` lexes as `AmpAmp` (logical-and)
        // due to logos longest-match, so `&&T` shows up here as
        // `Amp` followed by another `Amp` only if the source had
        // `& &T` with whitespace; the `AmpAmp`-flavoured `&&T`
        // can't be re-interpreted as two `&` tokens (different
        // lexeme). The type checker still rejects via the
        // recursive parse_type call seeing `&T` followed by
        // another `&T`.
        if self.peek_kind() == Some(TokenKind::Amp) {
            let amp_start = self.advance().expect("peeked").span.start;
            let mutable = if self.peek_kind() == Some(TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            let inner = self.parse_type()?;
            let span = amp_start..inner.span.end;
            return Ok(Spanned {
                kind: TypeExprKind::Ref { mutable, inner: Box::new(inner) },
                span,
            });
        }
        // Optional leading `?` for the C1.5 nullable type form.
        if self.peek_kind() == Some(TokenKind::Question) {
            let q_start = self.advance().expect("peeked").span.start;
            // Reject `??T` at the second `?`.
            if self.peek_kind() == Some(TokenKind::Question) {
                let t = self.peek().expect("peeked");
                return Err(ParseError::NestedNullable {
                    span: to_source_span(&t.span),
                });
            }
            let inner = self.parse_type()?;
            let span = q_start..inner.span.end;
            return Ok(Spanned {
                kind: TypeExprKind::Nullable(Box::new(inner)),
                span,
            });
        }
        // C1.6 / ADR 0015 D1: `[T]` array type. Parser accepts any
        // T including arrays; nested-array rejection happens at
        // type-resolve.
        if self.peek_kind() == Some(TokenKind::LBracket) {
            let lb_start = self.advance().expect("peeked").span.start;
            let inner = self.parse_type()?;
            match self.peek_kind() {
                Some(TokenKind::RBracket) => {
                    let rb_end = self.advance().expect("peeked").span.end;
                    return Ok(Spanned {
                        kind: TypeExprKind::Array(Box::new(inner)),
                        span: lb_start..rb_end,
                    });
                }
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`]` to close array type",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`]` to close array type",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            }
        }
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let name_span = t.span.clone();
                let name = self.src[name_span.clone()].to_string();
                self.advance();
                // C1.7 / ADR 0016 D3: optional `<TypeArg1, ...>`
                // after an Ident in type position promotes to
                // `TypeExprKind::Generic`. The parser is permissive
                // about arity / generic-vs-not; the type checker
                // validates.
                if self.peek_kind() == Some(TokenKind::Lt) {
                    let (args, args_span) = self.parse_type_args()?;
                    let span = name_span.start..args_span.end;
                    Ok(Spanned {
                        kind: TypeExprKind::Generic { name, name_span, args },
                        span,
                    })
                } else {
                    Ok(Spanned { kind: TypeExprKind::Ident(name), span: name_span })
                }
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
        // C2 / ADR 0017 D2: optional `mut` prefix on the parameter.
        // Binding-local — distinct from passing `&mut T`.
        let (mutable, mut_start) = if self.peek_kind() == Some(TokenKind::Mut) {
            let s = self.advance().expect("peeked").span.start;
            (true, Some(s))
        } else {
            (false, None)
        };
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
        let span_start = mut_start.unwrap_or(name_span.start);
        Ok(Param { mutable, name, span: span_start..span_end, ty })
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let let_start = self.advance().expect("checked above").span.start;

        // C2 / ADR 0017 D2: optional `mut` after `let` makes the
        // binding re-assignable + exclusive-borrowable.
        let mutable = if self.peek_kind() == Some(TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };

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
            kind: StmtKind::Let { mutable, name, name_span, ty_annot, value },
            span: let_start..semi_end,
        })
    }

    fn peek(&self) -> Option<&Spanned<TokenKind>> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    /// The token AFTER the current one (2-token lookahead). ADR 0048: the shift
    /// operators `<<` / `>>` are reconstructed from two span-adjacent `<` / `>`
    /// tokens, so the shift parser needs to inspect the next token's kind AND
    /// span without consuming.
    fn peek2(&self) -> Option<&Spanned<TokenKind>> {
        self.tokens.get(self.pos + 1)
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
        // Review F11 / P2.5: `parse_expr` is the recursion cycle's entry — every
        // nested expression (parens, call args, nested `if`/`match`, block tails)
        // funnels back through here. Bound the depth so adversarial deep nesting
        // is a clean `RecursionLimit` rejection, not a stack overflow. Always
        // decrement so a recovered parse keeps an accurate count.
        self.depth += 1;
        let result = if self.depth > MAX_EXPR_DEPTH {
            let span = self
                .peek()
                .map(|t| t.span.clone())
                .unwrap_or_else(|| self.eof_span());
            Err(ParseError::RecursionLimit { span: to_source_span(&span) })
        } else if self.peek_kind() == Some(TokenKind::If) {
            self.parse_if()
        } else if self.peek_kind() == Some(TokenKind::Match) {
            // Phase D.1 / ADR 0032: `match` is a control-flow expression,
            // dispatched here alongside `if`.
            self.parse_match_expr()
        } else {
            self.parse_or()
        };
        self.depth -= 1;
        result
    }

    /// `or_expr = and_expr ('||' and_expr)*` — left-associative,
    /// short-circuit (semantics handled by codegen).
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.peek_kind() == Some(TokenKind::PipePipe) {
            self.advance();
            let rhs = self.parse_and()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Logic(LogicOp::Or, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    /// `and_expr = cmp_expr ('&&' cmp_expr)*` — left-associative.
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_cmp()?;
        while self.peek_kind() == Some(TokenKind::AmpAmp) {
            self.advance();
            let rhs = self.parse_cmp()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Logic(LogicOp::And, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    /// `cmp_expr = add_expr (cmp_op add_expr)?` — non-associative
    /// per ADR 0012 D6. A second comparison operator is rejected at
    /// parse time as [`ParseError::ChainedComparison`].
    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_bitor()?;
        let Some(op) = cmp_op_from_token(self.peek_kind()) else {
            return Ok(lhs);
        };
        self.advance();
        let rhs = self.parse_bitor()?;
        // Reject a chained comparison: `a < b < c` — the third atom
        // would imply a second cmp op next.
        if let Some(t) = self.peek() {
            if cmp_op_from_token(Some(t.kind)).is_some() {
                return Err(ParseError::ChainedComparison {
                    span: to_source_span(&t.span),
                });
            }
        }
        let span = lhs.span.start..rhs.span.end;
        Ok(Spanned {
            kind: ExprKind::Cmp(op, Box::new(lhs), Box::new(rhs)),
            span,
        })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let if_start = self.advance().expect("checked `if`").span.start;
        // ADR 0013 D3a: forbid struct literals in the if-condition
        // position so `if x { ... }` is unambiguous.
        let saved = self.allow_struct_lit;
        self.allow_struct_lit = false;
        let cond = self.parse_expr()?;
        self.allow_struct_lit = saved;
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

    /// Phase D.1 / ADR 0032: `match scrutinee { pat => body, … }`. The
    /// scrutinee forbids struct literals (like the if-condition) so
    /// `match x { … }` is unambiguous. Arms are comma-separated; a
    /// trailing comma is allowed.
    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let match_start = self.advance().expect("checked `match`").span.start;

        let saved = self.allow_struct_lit;
        self.allow_struct_lit = false;
        let scrutinee = self.parse_expr()?;
        self.allow_struct_lit = saved;

        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` to open match body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` to open match body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut arms = Vec::new();
        if self.peek_kind() != Some(TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break; // trailing comma allowed
                }
                arms.push(self.parse_match_arm()?);
            }
        }

        let rbrace_end = match self.peek_kind() {
            Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `}` in match body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `}` in match body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(Spanned {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: match_start..rbrace_end,
        })
    }

    /// `match_arm = pattern '=>' expr`.
    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_pattern()?;
        let arm_start = match &pattern {
            Pattern::Variant { span, .. } => span.start,
            Pattern::Wildcard(span) => span.start,
        };
        match self.peek_kind() {
            Some(TokenKind::FatArrow) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`=>` after a match pattern",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`=>` after a match pattern",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let body = self.parse_expr()?;
        let end = body.span.end;
        Ok(MatchArm { pattern, body, span: arm_start..end })
    }

    /// `pattern = '_' | Ident '::' Ident ('(' binding (',' binding)* ')')?`
    /// — the `_` wildcard or a qualified variant pattern (bindings
    /// positional; a binding may itself be `_`). Per ADR 0032 D10,
    /// nested / or- / literal patterns are out of scope.
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let (head, head_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let text = self.src[span.clone()].to_string();
                self.advance();
                (text, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "a match pattern (`_` or `Enum::Variant`)",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "a match pattern (`_` or `Enum::Variant`)",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        // The `_` wildcard (lexes as an Ident).
        if head == "_" {
            return Ok(Pattern::Wildcard(head_span));
        }

        // Qualified variant pattern: `Enum::Variant` (+ optional bindings).
        match self.peek_kind() {
            Some(TokenKind::ColonColon) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`::` after the enum name in a variant pattern",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`::` after the enum name in a variant pattern",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let (variant, variant_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let text = self.src[span.clone()].to_string();
                self.advance();
                (text, span)
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "a variant name after `::`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "a variant name after `::`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        let p_start = head_span.start;
        let mut end = variant_span.end;
        let mut bindings = Vec::new();
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.advance();
            if self.peek_kind() != Some(TokenKind::RParen) {
                bindings.push(self.parse_pattern_binding()?);
                while self.peek_kind() == Some(TokenKind::Comma) {
                    self.advance();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break; // trailing comma allowed
                    }
                    bindings.push(self.parse_pattern_binding()?);
                }
            }
            end = match self.peek_kind() {
                Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                Some(other) => {
                    let t = self.peek().expect("peeked");
                    return Err(ParseError::UnexpectedToken {
                        got: format!("{other:?}"),
                        expected: "`,` or `)` in variant-pattern bindings",
                        span: to_source_span(&t.span),
                    });
                }
                None => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "`,` or `)` in variant-pattern bindings",
                        span: to_source_span(&self.eof_span()),
                    });
                }
            };
        }

        Ok(Pattern::Variant {
            enum_name: head,
            enum_name_span: head_span,
            variant,
            variant_span,
            bindings,
            span: p_start..end,
        })
    }

    /// A single positional binding inside a variant pattern — an `Ident`
    /// (which may be `_`).
    fn parse_pattern_binding(&mut self) -> Result<Spanned<String>, ParseError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let text = self.src[span.clone()].to_string();
                self.advance();
                Ok(Spanned { kind: text, span })
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "a binding name in a variant pattern",
                    span: to_source_span(&span),
                })
            }
            None => Err(ParseError::UnexpectedEof {
                expected: "a binding name in a variant pattern",
                span: to_source_span(&self.eof_span()),
            }),
        }
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        self.parse_block_inner(false)
    }

    /// Phase D.5 / ADR 0036 D3: parse a `while` body — a block that may
    /// be statement-only (no trailing tail expression). A regular block
    /// (ADR 0010 D6) requires a tail; a loop body runs for effect, so a
    /// missing tail is synthesised as the unit value `0` (discarded each
    /// iteration). A body that *does* end with an expression keeps it as
    /// the (still-discarded) tail.
    fn parse_loop_body(&mut self) -> Result<Block, ParseError> {
        self.parse_block_inner(true)
    }

    fn parse_block_inner(&mut self, allow_stmt_only: bool) -> Result<Block, ParseError> {
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
                    // Phase D.5 / ADR 0036 D3: a `while` body may be
                    // statement-only — synthesise a discarded unit tail
                    // (`0`) at the closing brace. A regular block still
                    // requires a trailing expression (ADR 0010 D6).
                    if allow_stmt_only {
                        let rbrace_end = self.advance().expect("peeked").span.end;
                        let tail = Spanned {
                            kind: ExprKind::IntLit(0),
                            span: rbrace_end..rbrace_end,
                        };
                        return Ok(Block {
                            stmts,
                            tail,
                            span: lbrace_start..rbrace_end,
                        });
                    }
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
                Some(TokenKind::While) => {
                    // Phase D.5 / ADR 0036: `while <cond> { <body> }` — a
                    // loop statement. Forbid struct literals in the
                    // condition (like `if`) so `while x { ... }` is
                    // unambiguous. No trailing `;` (the body's `}` ends
                    // the statement).
                    let while_start = self.advance().expect("checked `while`").span.start;
                    let saved = self.allow_struct_lit;
                    self.allow_struct_lit = false;
                    let cond = self.parse_expr()?;
                    self.allow_struct_lit = saved;
                    let body = self.parse_loop_body()?;
                    let span = while_start..body.span.end;
                    stmts.push(Spanned {
                        kind: StmtKind::While { cond, body: Box::new(body) },
                        span,
                    });
                }
                Some(kw @ (TokenKind::Break | TokenKind::Continue)) => {
                    // Phase D.5 (2/N) / ADR 0036 D9: `break;` / `continue;`
                    // — payload-free loop-control statements (exit /
                    // next-iteration of the innermost enclosing `while`).
                    // Each requires a `;`. Whether it is actually inside a
                    // loop is a type-check concern (D7), not a syntactic one.
                    let start = self.advance().expect("checked break/continue").span.start;
                    let semi_end = match self.peek_kind() {
                        Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
                        Some(other) => {
                            let t = self.peek().expect("peeked");
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{other:?}"),
                                expected: "`;` after `break` / `continue`",
                                span: to_source_span(&t.span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`;` after `break` / `continue`",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    let kind = if kw == TokenKind::Break {
                        StmtKind::Break
                    } else {
                        StmtKind::Continue
                    };
                    stmts.push(Spanned { kind, span: start..semi_end });
                }
                Some(_) => {
                    let expr = self.parse_expr()?;
                    // C2 / ADR 0017 D2: assignment statement
                    // `lhs = rhs;`. After parsing the LHS, check
                    // for `=` and consume the RHS + semi. The
                    // type checker validates the LHS is an lvalue
                    // and that any binding being written is
                    // mutable.
                    if self.peek_kind() == Some(TokenKind::Eq) {
                        self.advance();
                        let value = self.parse_expr()?;
                        let semi_end = match self.peek_kind() {
                            Some(TokenKind::Semi) => self.advance().expect("peeked").span.end,
                            Some(other) => {
                                let t = self.peek().expect("peeked");
                                return Err(ParseError::UnexpectedToken {
                                    got: format!("{other:?}"),
                                    expected: "`;` after assignment",
                                    span: to_source_span(&t.span),
                                });
                            }
                            None => {
                                return Err(ParseError::UnexpectedEof {
                                    expected: "`;` after assignment",
                                    span: to_source_span(&self.eof_span()),
                                });
                            }
                        };
                        let span = expr.span.start..semi_end;
                        stmts.push(Spanned {
                            kind: StmtKind::Assign { target: expr, value },
                            span,
                        });
                        continue;
                    }
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

    // C5.3 / ADR 0027 D3: bitwise precedence sits between comparison
    // (looser) and additive (tighter), with `&` binding tightest and `|`
    // loosest — the Rust ordering. Three left-associative levels:
    // `parse_bitor` → `parse_bitxor` → `parse_bitand` → `parse_add`.

    fn parse_bitor(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bitxor()?;
        while self.peek_kind() == Some(TokenKind::Pipe) {
            self.advance();
            let rhs = self.parse_bitxor()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(BinOp::BitOr, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_bitxor(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bitand()?;
        while self.peek_kind() == Some(TokenKind::Caret) {
            self.advance();
            let rhs = self.parse_bitand()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(BinOp::BitXor, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_bitand(&mut self) -> Result<Expr, ParseError> {
        // Infix `&` is bitwise-and. A *prefix* `&` (borrow) has already
        // been consumed by `parse_unary` while building each operand, so
        // an `&` seen here is unambiguously infix.
        let mut lhs = self.parse_shift()?;
        while self.peek_kind() == Some(TokenKind::Amp) {
            self.advance();
            let rhs = self.parse_shift()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(BinOp::BitAnd, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    /// ADR 0048: shift operators `<<` / `>>`, between bitwise-and and additive
    /// (tighter than `& ^ |` + comparison, looser than `+ -`), left-associative.
    /// They are NOT lexer tokens: `>` lexes as `Gt` and nested generics close
    /// one `Gt` at a time (`Vec<Box<i64>>`), so a `>>` token would mis-lex that.
    /// Instead a shift is two SPAN-ADJACENT `<`/`>` tokens (no whitespace
    /// between) — unambiguous in expression position, where explicit generic
    /// args never appear.
    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_add()?;
        loop {
            let op = match (self.peek(), self.peek2()) {
                (Some(a), Some(b)) if a.kind == TokenKind::Lt
                    && b.kind == TokenKind::Lt
                    && a.span.end == b.span.start =>
                {
                    BinOp::Shl
                }
                (Some(a), Some(b)) if a.kind == TokenKind::Gt
                    && b.kind == TokenKind::Gt
                    && a.span.end == b.span.start =>
                {
                    BinOp::Shr
                }
                _ => break,
            };
            self.advance();
            self.advance();
            let rhs = self.parse_add()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
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
        let mut lhs = self.parse_cast()?;
        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Star) => BinOp::Mul,
                Some(TokenKind::Slash) => BinOp::Div,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_cast()?;
            let span = lhs.span.start..rhs.span.end;
            lhs = Spanned {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    /// ADR 0049: integer cast `expr as T`, between multiplicative and unary
    /// (Rust precedence: `as` binds tighter than the binary operators but
    /// looser than unary — `-x as i32` is `(-x) as i32`, `a * b as i32` is
    /// `a * (b as i32)`). Left-associative (`x as i32 as u8` chains). The `as`
    /// token is shared with `impl as Trait`, but that is parsed only in item
    /// position, so there is no conflict here in expression position.
    fn parse_cast(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_unary()?;
        while self.peek_kind() == Some(TokenKind::As) {
            self.advance();
            let ty = self.parse_type()?;
            let span = expr.span.start..ty.span.end;
            expr = Spanned {
                kind: ExprKind::Cast(Box::new(expr), ty),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        // C2 / ADR 0017 D3: `&expr` / `&mut expr` borrow-take prefix.
        // The presence of `mut` after `&` switches between shared
        // and exclusive borrow. Combined with parse_type's `&T` /
        // `&mut T` handling, the `&` token is now overloaded
        // between type position and expression position; the parser
        // picks the right path positionally.
        if self.peek_kind() == Some(TokenKind::Amp) {
            let start = self.advance().expect("peeked").span.start;
            let op = if self.peek_kind() == Some(TokenKind::Mut) {
                self.advance();
                UnaryOp::RefMut
            } else {
                UnaryOp::Ref
            };
            let inner = self.parse_unary()?;
            let span = start..inner.span.end;
            return Ok(Spanned {
                kind: ExprKind::Unary(op, Box::new(inner)),
                span,
            });
        }
        // C2 / ADR 0017 D4 + D10: `*expr` dereference prefix. Reuses
        // the multiplication `*` token; the parser disambiguates by
        // position (prefix here vs. infix in parse_mul).
        if self.peek_kind() == Some(TokenKind::Star) {
            let start = self.advance().expect("peeked").span.start;
            let inner = self.parse_unary()?;
            let span = start..inner.span.end;
            return Ok(Spanned {
                kind: ExprKind::Unary(UnaryOp::Deref, Box::new(inner)),
                span,
            });
        }
        let unary_op = match self.peek_kind() {
            Some(TokenKind::Minus) => Some(UnaryOp::Neg),
            Some(TokenKind::Bang) => Some(UnaryOp::Not),
            _ => None,
        };
        if let Some(op) = unary_op {
            let start = self.peek().expect("checked above").span.start;
            self.advance();
            let inner = self.parse_unary()?;
            let span = start..inner.span.end;
            return Ok(Spanned {
                kind: ExprKind::Unary(op, Box::new(inner)),
                span,
            });
        }
        self.parse_postfix()
    }

    /// Parse an atom followed by zero or more postfix operators. At
    /// C1.4 the only postfix operator is `.field` (per ADR 0013 D2);
    /// arrays at C1.6 will add `[index]`, methods at C4 will add
    /// `.method()` (which is `.method` + call shape).
    ///
    /// Field access is left-associative as a side effect of the loop
    /// shape: `a.b.c` parses as `(a.b).c`.
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut atom = self.parse_atom()?;
        loop {
            match self.peek_kind() {
                Some(TokenKind::Dot) => {
                    self.advance();
                    // C4.4 / ADR 0024 D3: `task.await` postfix.
                    // The `await` keyword is reserved at C4.0
                    // (TokenKind::Await), distinct from a regular
                    // Ident. Check here before the field/method
                    // dispatch falls through.
                    if self.peek_kind() == Some(TokenKind::Await) {
                        let await_end = self.advance().expect("peeked").span.end;
                        let span = atom.span.start..await_end;
                        atom = Spanned {
                            kind: ExprKind::Await {
                                task_expr: Box::new(atom),
                            },
                            span,
                        };
                        continue;
                    }
                    let (field, field_span) = match self.peek() {
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
                                expected: "field name after `.`",
                                span: to_source_span(&span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "field name after `.`",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    // C4.1 / ADR 0022 D3 + D7: distinguish field
                    // access `target.field` from method call
                    // `target.method(args)`. Lookahead on `(`
                    // promotes to MethodCall; otherwise FieldAccess.
                    if self.peek_kind() == Some(TokenKind::LParen) {
                        self.advance();
                        let saved = self.allow_struct_lit;
                        self.allow_struct_lit = true;
                        let mut args = Vec::new();
                        if self.peek_kind() != Some(TokenKind::RParen) {
                            args.push(self.parse_expr()?);
                            while self.peek_kind() == Some(TokenKind::Comma) {
                                self.advance();
                                if self.peek_kind() == Some(TokenKind::RParen) {
                                    break;
                                }
                                args.push(self.parse_expr()?);
                            }
                        }
                        self.allow_struct_lit = saved;
                        let rparen_end = match self.peek_kind() {
                            Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                            Some(other) => {
                                let t = self.peek().expect("peeked");
                                return Err(ParseError::UnexpectedToken {
                                    got: format!("{other:?}"),
                                    expected: "`,` or `)` in method-call arguments",
                                    span: to_source_span(&t.span),
                                });
                            }
                            None => {
                                return Err(ParseError::UnexpectedEof {
                                    expected: "`,` or `)` in method-call arguments",
                                    span: to_source_span(&self.eof_span()),
                                });
                            }
                        };
                        let span = atom.span.start..rparen_end;
                        atom = Spanned {
                            kind: ExprKind::MethodCall {
                                target: Box::new(atom),
                                method: field,
                                method_span: field_span,
                                args,
                            },
                            span,
                        };
                        continue;
                    }
                    let span = atom.span.start..field_span.end;
                    atom = Spanned {
                        kind: ExprKind::FieldAccess {
                            target: Box::new(atom),
                            field,
                            field_span,
                        },
                        span,
                    };
                }
                Some(TokenKind::LBracket) => {
                    // C1.6 / ADR 0015 D3: postfix `[index]` indexing.
                    // Inside `[...]` struct literals are
                    // unambiguous; restore allow_struct_lit.
                    self.advance();
                    let saved = self.allow_struct_lit;
                    self.allow_struct_lit = true;
                    let index = self.parse_expr()?;
                    self.allow_struct_lit = saved;
                    let rb_end = match self.peek_kind() {
                        Some(TokenKind::RBracket) => self.advance().expect("peeked").span.end,
                        Some(other) => {
                            let t = self.peek().expect("peeked");
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{other:?}"),
                                expected: "`]` to close index expression",
                                span: to_source_span(&t.span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`]` to close index expression",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    let span = atom.span.start..rb_end;
                    atom = Spanned {
                        kind: ExprKind::Index {
                            target: Box::new(atom),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(atom)
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
            Some(TokenKind::StringLit) => {
                // D.2 / ADR 0033 D2: decode `"..."` into its bytes. Like
                // `IntLit`, the value is recovered from the span at parse
                // time (the lexer only *recognised* the literal). The two
                // surrounding `"` are ASCII (1 byte each), so strip them
                // by byte index before decoding the escapes.
                let span = self.advance().expect("peeked").span.clone();
                let text = &self.src[span.clone()];
                let inner = &text.as_bytes()[1..text.len() - 1];
                let bytes = decode_byte_literal(inner).map_err(|()| ParseError::InvalidEscape {
                    text: text.to_string(),
                    span: to_source_span(&span),
                })?;
                Ok(Spanned { kind: ExprKind::StringLit(bytes), span })
            }
            Some(TokenKind::CharLit) => {
                // D.2 / ADR 0033 D2: decode `'c'` into its single byte.
                // Same span-strip + escape-decode as a string, then
                // enforce the single-byte rule (`''` / `'ab'` / a
                // multi-byte source char are rejected — bytes only).
                let span = self.advance().expect("peeked").span.clone();
                let text = &self.src[span.clone()];
                let inner = &text.as_bytes()[1..text.len() - 1];
                let bytes = decode_byte_literal(inner).map_err(|()| ParseError::InvalidEscape {
                    text: text.to_string(),
                    span: to_source_span(&span),
                })?;
                if bytes.len() != 1 {
                    return Err(ParseError::CharLitNotSingleByte {
                        text: text.to_string(),
                        span: to_source_span(&span),
                    });
                }
                Ok(Spanned { kind: ExprKind::CharLit(bytes[0]), span })
            }
            Some(TokenKind::True) => {
                let span = self.advance().expect("peeked").span.clone();
                Ok(Spanned { kind: ExprKind::BoolLit(true), span })
            }
            Some(TokenKind::False) => {
                let span = self.advance().expect("peeked").span.clone();
                Ok(Spanned { kind: ExprKind::BoolLit(false), span })
            }
            Some(TokenKind::Null) => {
                // C1.5 / ADR 0014 D2: the `null` keyword. The type
                // checker infers `?T` from context.
                let span = self.advance().expect("peeked").span.clone();
                Ok(Spanned { kind: ExprKind::NullLit, span })
            }
            Some(TokenKind::SelfVal) => {
                // C4.1 / ADR 0022 D8: `self` inside a method body
                // surfaces as a Var node named "self". Resolve
                // checks "are we inside a method context" + binds
                // the synthetic VarId. Outside class methods, the
                // identifier `self` is reserved by the C4.0 lexer
                // but cannot be used as a binding — resolve
                // surfaces `SelfOutsideClassContext`.
                let span = self.advance().expect("peeked").span.clone();
                Ok(Spanned {
                    kind: ExprKind::Var("self".to_string()),
                    span,
                })
            }
            Some(TokenKind::Scope) => self.parse_scope_expr(),
            Some(TokenKind::Spawn) => self.parse_spawn_expr(),
            Some(TokenKind::Declassify) => {
                // C3 / ADR 0019 D6: `declassify(e)` special form
                // with mandatory parens. Type-check rejects with
                // `TypeError::DeclassifyNotYet` at C3.0; lands at
                // C3.1 alongside `Type::Secret(SecretId)`.
                let d_start = self.advance().expect("peeked").span.start;
                match self.peek_kind() {
                    Some(TokenKind::LParen) => {
                        self.advance();
                    }
                    Some(other) => {
                        let t = self.peek().expect("peeked");
                        return Err(ParseError::UnexpectedToken {
                            got: format!("{other:?}"),
                            expected: "`(` after `declassify`",
                            span: to_source_span(&t.span),
                        });
                    }
                    None => {
                        return Err(ParseError::UnexpectedEof {
                            expected: "`(` after `declassify`",
                            span: to_source_span(&self.eof_span()),
                        });
                    }
                }
                let saved = self.allow_struct_lit;
                self.allow_struct_lit = true;
                let inner = self.parse_expr()?;
                self.allow_struct_lit = saved;
                let rp_end = match self.peek_kind() {
                    Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                    Some(other) => {
                        let t = self.peek().expect("peeked");
                        return Err(ParseError::UnexpectedToken {
                            got: format!("{other:?}"),
                            expected: "`)` to close `declassify(...)`",
                            span: to_source_span(&t.span),
                        });
                    }
                    None => {
                        return Err(ParseError::UnexpectedEof {
                            expected: "`)` to close `declassify(...)`",
                            span: to_source_span(&self.eof_span()),
                        });
                    }
                };
                Ok(Spanned {
                    kind: ExprKind::Declassify(Box::new(inner)),
                    span: d_start..rp_end,
                })
            }
            Some(TokenKind::Handle) => self.parse_handle_expr(),
            Some(TokenKind::Perform) => self.parse_perform_expr(),
            Some(TokenKind::LBracket) => {
                // C1.6 / ADR 0015 D2: array literal `[e1, e2, ...]`.
                // Inside `[...]`, struct literals are unambiguous.
                let lb_start = self.advance().expect("peeked").span.start;
                let saved = self.allow_struct_lit;
                self.allow_struct_lit = true;
                let mut elems = Vec::new();
                if self.peek_kind() != Some(TokenKind::RBracket) {
                    elems.push(self.parse_expr()?);
                    while self.peek_kind() == Some(TokenKind::Comma) {
                        self.advance();
                        if self.peek_kind() == Some(TokenKind::RBracket) {
                            break; // trailing comma allowed
                        }
                        elems.push(self.parse_expr()?);
                    }
                }
                let rb_end = match self.peek_kind() {
                    Some(TokenKind::RBracket) => self.advance().expect("peeked").span.end,
                    Some(other) => {
                        let t = self.peek().expect("peeked");
                        return Err(ParseError::UnexpectedToken {
                            got: format!("{other:?}"),
                            expected: "`,` or `]` in array literal",
                            span: to_source_span(&t.span),
                        });
                    }
                    None => {
                        return Err(ParseError::UnexpectedEof {
                            expected: "`,` or `]` in array literal",
                            span: to_source_span(&self.eof_span()),
                        });
                    }
                };
                self.allow_struct_lit = saved;
                Ok(Spanned {
                    kind: ExprKind::ArrayLit(elems),
                    span: lb_start..rb_end,
                })
            }
            Some(TokenKind::Ident) => {
                let name_span = self.advance().expect("peeked").span.clone();
                let name = self.src[name_span.clone()].to_string();
                // C4.1 / ADR 0022 D5: `Name::init(args)` class
                // instantiation. C4.2 / ADR 0023 D5: `ImplName::method(args)`
                // qualified call. The `::` separator is followed by
                // either `init` (→ ClassInit) or an Ident (→
                // QualifiedCall). Disambiguated at parse time by the
                // token kind after `::`.
                if self.peek_kind() == Some(TokenKind::ColonColon) {
                    self.advance();
                    let is_init: bool;
                    let (method_name, method_span) = match self.peek() {
                        Some(t) if t.kind == TokenKind::Init => {
                            is_init = true;
                            let span = t.span.clone();
                            self.advance();
                            ("init".to_string(), span)
                        }
                        Some(t) if t.kind == TokenKind::Ident => {
                            is_init = false;
                            let span = t.span.clone();
                            let n = self.src[span.clone()].to_string();
                            self.advance();
                            (n, span)
                        }
                        Some(t) => {
                            let kind = t.kind;
                            let span = t.span.clone();
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{kind:?}"),
                                expected: "`init` or method name after `::`",
                                span: to_source_span(&span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`init` or method name after `::`",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    // `(args)` present → `Name::init(args)` class init or
                    // `Name::method(args)` qualified call / `Enum::Variant
                    // (args)` payload construction. Absent → bare
                    // `Enum::Variant` *unit* construction (ADR 0032 D2) —
                    // the `::` path with no call. The bare form is
                    // disambiguated at resolve: when `Name` is an enum it
                    // becomes an `EnumConstruct` (0-arg variant); otherwise
                    // (a bare method reference, which has no meaning) it
                    // surfaces a resolve error.
                    let has_parens = self.peek_kind() == Some(TokenKind::LParen);
                    let (args, end) = if has_parens {
                        self.advance();
                        let saved = self.allow_struct_lit;
                        self.allow_struct_lit = true;
                        let mut args = Vec::new();
                        if self.peek_kind() != Some(TokenKind::RParen) {
                            args.push(self.parse_expr()?);
                            while self.peek_kind() == Some(TokenKind::Comma) {
                                self.advance();
                                if self.peek_kind() == Some(TokenKind::RParen) {
                                    break;
                                }
                                args.push(self.parse_expr()?);
                            }
                        }
                        self.allow_struct_lit = saved;
                        let rparen_end = match self.peek_kind() {
                            Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
                            Some(other) => {
                                let t = self.peek().expect("peeked");
                                return Err(ParseError::UnexpectedToken {
                                    got: format!("{other:?}"),
                                    expected: "`,` or `)` in call arguments",
                                    span: to_source_span(&t.span),
                                });
                            }
                            None => {
                                return Err(ParseError::UnexpectedEof {
                                    expected: "`,` or `)` in call arguments",
                                    span: to_source_span(&self.eof_span()),
                                });
                            }
                        };
                        (args, rparen_end)
                    } else {
                        (Vec::new(), method_span.end)
                    };
                    // Bare `Name::init` (no parens) is NOT a class init —
                    // only the parenthesised form flows through `init`.
                    let kind = if is_init && has_parens {
                        ExprKind::ClassInit {
                            class_name: name,
                            class_name_span: name_span.clone(),
                            args,
                        }
                    } else {
                        ExprKind::QualifiedCall {
                            impl_name: name,
                            impl_name_span: name_span.clone(),
                            method: method_name,
                            method_span,
                            args,
                        }
                    };
                    return Ok(Spanned {
                        kind,
                        span: name_span.start..end,
                    });
                }
                // Lookahead: `Ident '('` = call; `Ident '{'` = struct
                // literal (when allow_struct_lit is on per ADR 0013
                // D3a); otherwise = Var.
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
                } else if self.allow_struct_lit
                    && self.peek_kind() == Some(TokenKind::LBrace)
                {
                    // Struct literal: `Name { field: expr, ... }`.
                    self.advance(); // consume `{`
                    // Inside the struct-literal body, struct literals
                    // are unambiguous again (we're definitely in expr
                    // position now).
                    let saved = self.allow_struct_lit;
                    self.allow_struct_lit = true;
                    let mut fields = Vec::new();
                    if self.peek_kind() != Some(TokenKind::RBrace) {
                        fields.push(self.parse_field_init()?);
                        while self.peek_kind() == Some(TokenKind::Comma) {
                            self.advance();
                            if self.peek_kind() == Some(TokenKind::RBrace) {
                                break; // trailing comma allowed
                            }
                            fields.push(self.parse_field_init()?);
                        }
                    }
                    let rbrace_end = match self.peek_kind() {
                        Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
                        Some(other) => {
                            let t = self.peek().expect("peeked");
                            return Err(ParseError::UnexpectedToken {
                                got: format!("{other:?}"),
                                expected: "`,` or `}` in struct literal",
                                span: to_source_span(&t.span),
                            });
                        }
                        None => {
                            return Err(ParseError::UnexpectedEof {
                                expected: "`,` or `}` in struct literal",
                                span: to_source_span(&self.eof_span()),
                            });
                        }
                    };
                    self.allow_struct_lit = saved;
                    Ok(Spanned {
                        kind: ExprKind::StructLit {
                            name,
                            name_span: name_span.clone(),
                            fields,
                        },
                        span: name_span.start..rbrace_end,
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
                // Parens always escape D3a's no-struct-lit-in-cond
                // rule. `if (Foo { x: 1 }.x == 1) { ... }` works.
                let saved = self.allow_struct_lit;
                self.allow_struct_lit = true;
                let inner = self.parse_expr()?;
                self.allow_struct_lit = saved;
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

    /// C3.4 / ADR 0020 D4 + D5: parse `handle expr with { arms }`.
    /// Called from `parse_atom` when the current token is `handle`.
    /// The handler body parses with `allow_struct_lit = true` so a
    /// `handle Point { x: 1 } with { ... }` form parses as
    /// expected (the `with` keyword unambiguously closes the body).
    fn parse_handle_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.advance().expect("peeked Handle").span.start;
        let saved = self.allow_struct_lit;
        self.allow_struct_lit = true;
        let body = self.parse_expr()?;
        self.allow_struct_lit = saved;

        match self.peek_kind() {
            Some(TokenKind::With) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`with` after handle body",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`with` after handle body",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`{` after `with`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`{` after `with`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        let mut arms: Vec<HandlerArm> = Vec::new();
        let mut return_arm: Option<Box<ReturnArm>> = None;
        // First arm (if any). Empty handler-arm lists are accepted
        // structurally; type-check decides whether they make sense.
        if self.peek_kind() != Some(TokenKind::RBrace) {
            self.parse_handler_or_return_arm(&mut arms, &mut return_arm)?;
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break; // trailing comma allowed
                }
                self.parse_handler_or_return_arm(&mut arms, &mut return_arm)?;
            }
        }
        let rb_end = match self.peek_kind() {
            Some(TokenKind::RBrace) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `}` in handler arms",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `}` in handler arms",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };

        Ok(Spanned {
            kind: ExprKind::Handle {
                body: Box::new(body),
                arms,
                return_arm,
            },
            span: start..rb_end,
        })
    }

    /// C3.4 / ADR 0020 D4: parse a single handler arm OR the
    /// optional `return v => body` arm. Operation arms are
    /// `EffectName.OpName ( param (, param)* ) => expr`; the
    /// final param is the continuation binding `k` per D5. A
    /// second `return` arm is rejected at parse via
    /// `DuplicateReturnArm`.
    fn parse_handler_or_return_arm(
        &mut self,
        arms: &mut Vec<HandlerArm>,
        return_arm: &mut Option<Box<ReturnArm>>,
    ) -> Result<(), ParseError> {
        if self.peek_kind() == Some(TokenKind::Return) {
            let ra = self.parse_return_arm()?;
            if return_arm.is_some() {
                return Err(ParseError::UnexpectedToken {
                    got: "second `return` arm".to_string(),
                    expected: "at most one `return v => body` arm per handler",
                    span: to_source_span(&ra.span),
                });
            }
            *return_arm = Some(Box::new(ra));
            return Ok(());
        }
        let arm = self.parse_handler_arm()?;
        arms.push(arm);
        Ok(())
    }

    /// C3.4 / ADR 0020 D4: `EffectName.OpName(param (, param)*) => expr`.
    /// Per D5 the last param is the continuation binding `k`.
    fn parse_handler_arm(&mut self) -> Result<HandlerArm, ParseError> {
        // Effect name (Ident).
        let effect_span = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                self.advance();
                span
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "effect name (Ident) at start of handler arm",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "effect name (Ident) at start of handler arm",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let effect_name = self.src[effect_span.clone()].to_string();
        let arm_start = effect_span.start;

        // `.`
        match self.peek_kind() {
            Some(TokenKind::Dot) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`.` after effect name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`.` after effect name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Op name (Ident).
        let op_span = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                self.advance();
                span
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "op name after `.`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "op name after `.`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let op_name = self.src[op_span.clone()].to_string();

        // `(`
        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after op name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after op name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Param names: at least one (the kont). The lexer accepts
        // any Ident here; resolve enforces uniqueness within the
        // arm body.
        let mut param_names: Vec<Spanned<String>> = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            param_names.push(self.parse_handler_arm_param()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break;
                }
                param_names.push(self.parse_handler_arm_param()?);
            }
        }
        match self.peek_kind() {
            Some(TokenKind::RParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in handler arm parameters",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in handler arm parameters",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // `=>`
        match self.peek_kind() {
            Some(TokenKind::FatArrow) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`=>` after handler arm parameters",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`=>` after handler arm parameters",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }

        // Body expression. Struct literals are unambiguous here
        // (no leading-Ident ambiguity for `{` since `{` of a
        // struct literal is preceded by an Ident-call shape, not
        // by a bare expression).
        let saved = self.allow_struct_lit;
        self.allow_struct_lit = true;
        let body = self.parse_expr()?;
        self.allow_struct_lit = saved;
        let body_end = body.span.end;

        Ok(HandlerArm {
            effect: Spanned { kind: effect_name, span: effect_span },
            op: Spanned { kind: op_name, span: op_span },
            param_names,
            body,
            span: arm_start..body_end,
        })
    }

    fn parse_handler_arm_param(&mut self) -> Result<Spanned<String>, ParseError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                let name = self.src[span.clone()].to_string();
                self.advance();
                Ok(Spanned { kind: name, span })
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "parameter name (Ident) in handler arm",
                    span: to_source_span(&span),
                })
            }
            None => Err(ParseError::UnexpectedEof {
                expected: "parameter name (Ident) in handler arm",
                span: to_source_span(&self.eof_span()),
            }),
        }
    }

    /// C3.4 / ADR 0020 D4: `return Ident => expr` — the optional
    /// arm bound to the handle body's value.
    fn parse_return_arm(&mut self) -> Result<ReturnArm, ParseError> {
        let arm_start = self.advance().expect("peeked Return").span.start;
        // value name (Ident)
        let value_span = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                self.advance();
                span
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "binding name after `return`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "binding name after `return`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let value_name = self.src[value_span.clone()].to_string();
        // `=>`
        match self.peek_kind() {
            Some(TokenKind::FatArrow) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`=>` after return binding name",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`=>` after return binding name",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let saved = self.allow_struct_lit;
        self.allow_struct_lit = true;
        let body = self.parse_expr()?;
        self.allow_struct_lit = saved;
        let body_end = body.span.end;
        Ok(ReturnArm {
            value_name: Spanned { kind: value_name, span: value_span },
            body,
            span: arm_start..body_end,
        })
    }

    /// C3.4 / ADR 0020 D4 + D5: parse `perform EffectName.OpName(args)`.
    /// Called from `parse_atom` when the current token is `perform`.
    fn parse_perform_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.advance().expect("peeked Perform").span.start;
        let effect_span = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                self.advance();
                span
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "effect name after `perform`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "effect name after `perform`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let effect_name = self.src[effect_span.clone()].to_string();
        match self.peek_kind() {
            Some(TokenKind::Dot) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`.` after effect name in `perform`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`.` after effect name in `perform`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let op_span = match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                let span = t.span.clone();
                self.advance();
                span
            }
            Some(t) => {
                let kind = t.kind;
                let span = t.span.clone();
                return Err(ParseError::UnexpectedToken {
                    got: format!("{kind:?}"),
                    expected: "op name after `.` in `perform`",
                    span: to_source_span(&span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "op name after `.` in `perform`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        let op_name = self.src[op_span.clone()].to_string();
        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                self.advance();
            }
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`(` after op name in `perform`",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`(` after op name in `perform`",
                    span: to_source_span(&self.eof_span()),
                });
            }
        }
        let mut args = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            let saved = self.allow_struct_lit;
            self.allow_struct_lit = true;
            args.push(self.parse_expr()?);
            while self.peek_kind() == Some(TokenKind::Comma) {
                self.advance();
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_expr()?);
            }
            self.allow_struct_lit = saved;
        }
        let rp_end = match self.peek_kind() {
            Some(TokenKind::RParen) => self.advance().expect("peeked").span.end,
            Some(other) => {
                let t = self.peek().expect("peeked");
                return Err(ParseError::UnexpectedToken {
                    got: format!("{other:?}"),
                    expected: "`,` or `)` in `perform` arguments",
                    span: to_source_span(&t.span),
                });
            }
            None => {
                return Err(ParseError::UnexpectedEof {
                    expected: "`,` or `)` in `perform` arguments",
                    span: to_source_span(&self.eof_span()),
                });
            }
        };
        Ok(Spanned {
            kind: ExprKind::Perform {
                effect: Spanned { kind: effect_name, span: effect_span },
                op: Spanned { kind: op_name, span: op_span },
                args,
            },
            span: start..rp_end,
        })
    }
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

/// D.2 / ADR 0033 D2: decode the interior bytes of a char/string
/// literal — the source bytes *between* the quotes — into the bytes
/// the literal denotes, processing escape sequences. Non-escape bytes
/// (including multi-byte UTF-8) pass through verbatim, so a string IS
/// its UTF-8 source bytes (ADR 0033 D3). The recognised escapes are
/// `\n \t \r \0 \\ \' \"` and `\xHH` (two hex digits → one byte).
///
/// Returns `Err(())` on an invalid escape (an unknown escape letter,
/// a non-hex or short `\x`, or a dangling `\`); the caller attaches
/// the literal's span. The lexer's regex already guarantees a `\` is
/// followed by at least one char, but the bounds are checked anyway
/// so this is panic-free for any input.
fn decode_byte_literal(inner: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] != b'\\' {
            out.push(inner[i]);
            i += 1;
            continue;
        }
        // An escape: `inner[i]` is `\`; the next byte selects the kind.
        match *inner.get(i + 1).ok_or(())? {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'0' => out.push(0),
            b'\\' => out.push(b'\\'),
            b'\'' => out.push(b'\''),
            b'"' => out.push(b'"'),
            b'x' => {
                // `\xHH` — exactly two hex digits → one byte.
                let hi = hex_digit(*inner.get(i + 2).ok_or(())?).ok_or(())?;
                let lo = hex_digit(*inner.get(i + 3).ok_or(())?).ok_or(())?;
                out.push(hi * 16 + lo);
                i += 2; // consume the two hex digits (plus the +2 below)
            }
            _ => return Err(()),
        }
        i += 2; // consume the `\` and the selector byte
    }
    Ok(out)
}

/// D.2 / ADR 0033 D2: the value of a single hex digit byte, or `None`
/// if `b` is not `[0-9a-fA-F]`.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Map a comparison-operator [`TokenKind`] to its [`CmpOp`].
/// Returns `None` for non-comparison tokens (and for `None`).
fn cmp_op_from_token(tok: Option<TokenKind>) -> Option<CmpOp> {
    match tok? {
        TokenKind::EqEq => Some(CmpOp::Eq),
        TokenKind::BangEq => Some(CmpOp::Ne),
        TokenKind::Lt => Some(CmpOp::Lt),
        TokenKind::LtEq => Some(CmpOp::Le),
        TokenKind::Gt => Some(CmpOp::Gt),
        TokenKind::GtEq => Some(CmpOp::Ge),
        _ => None,
    }
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
    fn parse_bitwise_precedence() {
        // C5.3 / ADR 0027 D3: `&` > `^` > `|`.
        assert_eq!(pretty("5 & 6 ^ 3 | 8"), "(| (^ (& 5 6) 3) 8)");
    }

    #[test]
    fn parse_bitand_binds_looser_than_arithmetic() {
        // Additive is tighter than `&`, so `1 + 2 & 3` is `(1 + 2) & 3`.
        assert_eq!(pretty("1 + 2 & 3"), "(& (+ 1 2) 3)");
    }

    #[test]
    fn parse_bitor_is_left_associative() {
        assert_eq!(pretty("1 | 2 | 3"), "(| (| 1 2) 3)");
    }

    #[test]
    fn parse_shift_basic() {
        // ADR 0048: `<<` / `>>` are reconstructed from two span-adjacent
        // `<` / `>` tokens (no dedicated lexer token), left-associative.
        assert_eq!(pretty("a << b"), "(<< a b)");
        assert_eq!(pretty("a >> b"), "(>> a b)");
        assert_eq!(pretty("1 << 2 << 3"), "(<< (<< 1 2) 3)");
    }

    #[test]
    fn parse_shift_precedence() {
        // Shift is looser than additive, tighter than the bitwise ops — so a
        // rotate `(x << n) | (x >> m)` parses without parentheses.
        assert_eq!(pretty("1 + 2 << 1"), "(<< (+ 1 2) 1)");
        assert_eq!(pretty("1 << 2 & 3"), "(& (<< 1 2) 3)");
        assert_eq!(pretty("x << 4 | x >> 60"), "(| (<< x 4) (>> x 60))");
    }

    #[test]
    fn parse_cast_basic_and_precedence() {
        // ADR 0049: `x as T` is a cast; binds tighter than the binary operators
        // (so `a * b as i32` is `a * (b as i32)`) and chains left-associatively.
        assert_eq!(pretty("x as i32"), "(cast x i32)");
        assert_eq!(pretty("a * b as i32"), "(* a (cast b i32))");
        assert_eq!(pretty("x as i32 as u8"), "(cast (cast x i32) u8)");
    }

    #[test]
    fn parse_infix_amp_is_bitand_not_borrow() {
        // After a complete left operand, `&` is infix bitwise-and; a
        // prefix `&` (borrow) only appears at the start of an operand.
        assert_eq!(pretty("a & b"), "(& a b)");
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
    fn parse_while_statement() {
        // ADR 0036 D2/D3: `while <cond> { <body> }` parses to a
        // `StmtKind::While` (a statement, not an expression).
        let p = parse_block_ok("while i < 3 { x = x + 1; } 0");
        assert_eq!(p.stmts.len(), 1);
        match &p.stmts[0].kind {
            StmtKind::While { body, .. } => {
                // The body has the one assignment statement.
                assert_eq!(body.stmts.len(), 1);
            }
            other => panic!("expected While, got {other:?}"),
        }
        assert_eq!(p.tail.kind.to_string(), "0");
    }

    #[test]
    fn parse_while_statement_only_body() {
        // ADR 0036 D3: a statement-only `while` body (no trailing tail)
        // parses — the tail is synthesised.
        let p = parse_block_ok("while c { y = 1; } 0");
        assert!(matches!(&p.stmts[0].kind, StmtKind::While { .. }));
    }

    #[test]
    fn parse_break_and_continue_statements() {
        // ADR 0036 D9: `break;` / `continue;` parse to payload-free
        // `StmtKind::Break` / `StmtKind::Continue` statements. (Loop
        // membership is checked at type-check time, not here, so they
        // parse fine even at top level.)
        let p = parse_block_ok("break; continue; 0");
        assert_eq!(p.stmts.len(), 2);
        assert!(matches!(&p.stmts[0].kind, StmtKind::Break));
        assert!(matches!(&p.stmts[1].kind, StmtKind::Continue));
        assert_eq!(p.tail.kind.to_string(), "0");
    }

    #[test]
    fn parse_break_continue_pretty() {
        assert_eq!(
            pretty_block("while c { break; continue; } 0"),
            "(block (while c (block (break) (continue) 0)) 0)"
        );
    }

    #[test]
    fn parse_break_without_semi_is_error() {
        // `break` is a statement terminated by `;` (it is not an
        // expression / tail). A bare `break` before `}` is rejected.
        let err = parse_block_err("while c { break }");
        assert!(matches!(
            err,
            ParseError::UnexpectedToken { expected, .. } if expected == "`;` after `break` / `continue`"
        ));
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
    fn parse_use_decls_before_fns() {
        // ADR 0037 D2: top-level `use a::b::Item;` imports parse into
        // `Program.uses`, ahead of the fns; the full path (incl. the
        // trailing item) is stored as written.
        let p = parse_ok_program(
            "use util::math::add; use util::Pair; fn main() -> i64 { 0 }",
        );
        assert_eq!(p.uses.len(), 2);
        assert_eq!(p.uses[0].path, vec!["util", "math", "add"]);
        assert_eq!(p.uses[1].path, vec!["util", "Pair"]);
        assert_eq!(p.uses[0].to_string(), "(use util::math::add)");
        assert_eq!(p.fns.len(), 1);
    }

    #[test]
    fn parse_use_requires_module_and_item() {
        // A bare `use foo;` is rejected — file-as-module imports an item
        // (`module::Item`), not a module (ADR 0037 D2 MVP).
        let err = parse("use foo; fn main() -> i64 { 0 }").unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnexpectedToken { expected, .. }
                if expected == "`::` then an item name (`use module::Item;`)"
        ));
    }

    #[test]
    fn parse_use_requires_semicolon() {
        let err = parse("use a::b fn main() -> i64 { 0 }").unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnexpectedToken { expected, .. } if expected == "`;` after a `use` import"
        ));
    }

    #[test]
    fn parse_top_level_pub_visibility() {
        // ADR 0037 D3: `pub` on a top-level item records Public; bare items
        // are Private. (`pub` is a contextual keyword — Ident "pub" — as
        // since C4.1; here it precedes a top-level item.)
        let p = parse_ok_program(
            "pub fn add(a: i64, b: i64) -> i64 { a + b }\n\
             struct P { x: i64 }\n\
             pub enum E { A }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].name, "add");
        assert_eq!(p.fns[0].visibility, Visibility::Public);
        assert_eq!(p.fns[1].name, "main");
        assert_eq!(p.fns[1].visibility, Visibility::Private);
        assert_eq!(p.structs[0].visibility, Visibility::Private);
        assert_eq!(p.enums[0].visibility, Visibility::Public);
    }

    #[test]
    fn parse_pub_on_use_is_rejected() {
        // `pub use` (a re-export) is deferred (ADR 0037 D8) — rejected so
        // it can't silently look exported.
        let err = parse("pub use a::b; fn main() -> i64 { 0 }").unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnexpectedToken { expected, .. }
                if expected == "`pub` only on `fn` / `struct` / `enum` / `trait` / `effect`"
        ));
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
        // Top level expects `fn` or `struct` (C1.4), not `let`.
        let err = parse("let x = 1;").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`use`, `fn`, `struct`, `effect`, `class`, `trait`, `impl`, or `enum`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_top_level_bare_expression() {
        // Bare expressions at top level no longer parse — they're fn-body content now.
        let err = parse("42").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`use`, `fn`, `struct`, `effect`, `class`, `trait`, `impl`, or `enum`"),
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

    // C1.3 bool literals, comparisons, logicals, unary `!`.

    #[test]
    fn parse_bool_literal_true() {
        assert_eq!(pretty("true"), "true");
    }

    #[test]
    fn parse_bool_literal_false() {
        assert_eq!(pretty("false"), "false");
    }

    #[test]
    fn parse_cmp_eq() {
        assert_eq!(pretty("1 == 2"), "(== 1 2)");
    }

    #[test]
    fn parse_cmp_ne() {
        assert_eq!(pretty("a != b"), "(!= a b)");
    }

    #[test]
    fn parse_cmp_lt_le_gt_ge() {
        assert_eq!(pretty("1 < 2"), "(< 1 2)");
        assert_eq!(pretty("1 <= 2"), "(<= 1 2)");
        assert_eq!(pretty("1 > 2"), "(> 1 2)");
        assert_eq!(pretty("1 >= 2"), "(>= 1 2)");
    }

    #[test]
    fn parse_logic_and() {
        assert_eq!(pretty("true && false"), "(&& true false)");
    }

    #[test]
    fn parse_logic_or() {
        assert_eq!(pretty("true || false"), "(|| true false)");
    }

    #[test]
    fn parse_unary_not() {
        assert_eq!(pretty("!true"), "(! true)");
    }

    #[test]
    fn parse_unary_not_double() {
        assert_eq!(pretty("!!true"), "(! (! true))");
    }

    #[test]
    fn parse_precedence_cmp_higher_than_logic_and() {
        // a < b && c < d  parses as (&& (< a b) (< c d))
        assert_eq!(pretty("a < b && c < d"), "(&& (< a b) (< c d))");
    }

    #[test]
    fn parse_precedence_and_higher_than_or() {
        // a || b && c parses as (|| a (&& b c))
        assert_eq!(pretty("a || b && c"), "(|| a (&& b c))");
    }

    #[test]
    fn parse_precedence_add_higher_than_cmp() {
        // 1 + 2 < 3 parses as (< (+ 1 2) 3)
        assert_eq!(pretty("1 + 2 < 3"), "(< (+ 1 2) 3)");
    }

    #[test]
    fn parse_precedence_unary_not_higher_than_logic() {
        // !a && b parses as (&& (! a) b), not (! (&& a b))
        assert_eq!(pretty("!a && b"), "(&& (! a) b)");
    }

    #[test]
    fn parse_left_assoc_and() {
        assert_eq!(pretty("a && b && c"), "(&& (&& a b) c)");
    }

    #[test]
    fn parse_left_assoc_or() {
        assert_eq!(pretty("a || b || c"), "(|| (|| a b) c)");
    }

    #[test]
    fn parse_error_chained_comparison() {
        let err = parse_expr("1 < 2 < 3").unwrap_err();
        assert!(matches!(err, ParseError::ChainedComparison { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_chained_comparison_mixed_ops() {
        // Mixed cmp ops are also rejected: `a == b != c`
        let err = parse_expr("a == b != c").unwrap_err();
        assert!(matches!(err, ParseError::ChainedComparison { .. }), "got {err:?}");
    }

    #[test]
    fn parse_chained_comparison_via_parens_ok() {
        // Parenthesising one side allows the comparison ladder via &&.
        assert_eq!(pretty("(1 < 2) && (2 < 3)"), "(&& (< 1 2) (< 2 3))");
    }

    #[test]
    fn parse_if_condition_with_comparison() {
        assert_eq!(
            pretty("if x != 0 { 1 } else { 2 }"),
            "(if (!= x 0) (block 1) (block 2))"
        );
    }

    #[test]
    fn parse_fn_returning_bool() {
        let p = parse_ok_program("fn is_pos(x: i64) -> bool { x > 0 }");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "bool");
        assert_eq!(p.fns[0].body.tail.kind.to_string(), "(> x 0)");
    }

    #[test]
    fn parse_let_with_bool_value() {
        let block = parse_block_str("{ let b = true; b }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { value, .. } => {
                assert_eq!(value.kind.to_string(), "true");
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    // ----- C1.4: struct decl, struct literal, field access -----

    // ========================================================================
    // C4.1 / ADR 0022 D1-D4: class declaration parser tests. ASTs from the
    // parser layer are then consumed by resolve/types/codegen in follow-up
    // sessions; at this point classes parse but downstream passes leave them
    // untouched.
    // ========================================================================

    #[test]
    fn parse_class_decl_empty_body() {
        let p = parse_ok_program("class Empty { }\nfn main() -> i64 { 0 }");
        assert_eq!(p.classes.len(), 1);
        assert_eq!(p.classes[0].name, "Empty");
        assert!(p.classes[0].fields.is_empty());
        assert!(p.classes[0].init.is_none());
        assert!(p.classes[0].methods.is_empty());
    }

    #[test]
    fn parse_class_decl_fields_only() {
        let p = parse_ok_program(
            "class Point { let x: i64; let y: i64; }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        assert_eq!(c.name, "Point");
        assert_eq!(c.fields.len(), 2);
        assert_eq!(c.fields[0].name, "x");
        assert_eq!(c.fields[1].name, "y");
        assert_eq!(c.fields[0].ty.kind.to_string(), "i64");
        assert_eq!(c.fields[0].visibility, Visibility::Private);
    }

    #[test]
    fn parse_class_decl_pub_field() {
        let p = parse_ok_program(
            "class Point { pub let x: i64; let y: i64; }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        assert_eq!(c.fields[0].visibility, Visibility::Public);
        assert_eq!(c.fields[1].visibility, Visibility::Private);
    }

    #[test]
    fn parse_class_decl_init_only() {
        let p = parse_ok_program(
            "class Origin { init() { 0 } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        let init = c.init.as_ref().expect("init present");
        assert_eq!(init.visibility, Visibility::Private);
        assert!(init.params.is_empty());
    }

    #[test]
    fn parse_class_decl_init_with_params() {
        // Note: init body has a trailing `0` because the existing
        // Block structure requires a trailing expression. ADR 0022
        // D4 says init has no return value — the placeholder will
        // be stripped at the typing layer (the 0 is parser-shape
        // sugar at C4.1 minimum until block.tail becomes Option).
        let p = parse_ok_program(
            "class Point { let x: i64; pub init(x: i64) { self.x = x; 0 } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        let init = c.init.as_ref().expect("init present");
        assert_eq!(init.visibility, Visibility::Public);
        assert_eq!(init.params.len(), 1);
        assert_eq!(init.params[0].name, "x");
    }

    #[test]
    fn parse_class_decl_method_shared_self() {
        let p = parse_ok_program(
            "class Point { let x: i64; pub fn get(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        assert_eq!(c.methods.len(), 1);
        let m = &c.methods[0];
        assert_eq!(m.name, "get");
        assert_eq!(m.self_kind, SelfKind::Shared);
        assert!(m.params.is_empty());
    }

    #[test]
    fn parse_class_decl_method_exclusive_self() {
        let p = parse_ok_program(
            "class Point { let x: i64; pub fn set(self: &mut Self, v: i64) -> i64 { 0 } }\nfn main() -> i64 { 0 }",
        );
        let m = &p.classes[0].methods[0];
        assert_eq!(m.self_kind, SelfKind::Exclusive);
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "v");
    }

    #[test]
    fn parse_class_decl_full_surface() {
        // C4.1 (2/N) — the ADR 0022 D11 phase-go shape: Point with
        // manhattan + translate methods, where translate calls
        // self.manhattan() via the new postfix method-call form
        // (parsed as `ExprKind::MethodCall`).
        let src = r#"
            class Point {
                let x: i64;
                let y: i64;
                pub init(x: i64, y: i64) {
                    self.x = x;
                    self.y = y;
                    0
                }
                pub fn manhattan(self: &Self) -> i64 {
                    self.x + self.y
                }
                pub fn translate(self: &mut Self, dx: i64, dy: i64) -> i64 {
                    self.x = self.x + dx;
                    self.y = self.y + dy;
                    self.manhattan()
                }
            }
            fn main() -> i64 { 0 }
        "#;
        let p = parse_ok_program(src);
        let c = &p.classes[0];
        assert_eq!(c.name, "Point");
        assert_eq!(c.fields.len(), 2);
        assert!(c.init.is_some());
        assert_eq!(c.methods.len(), 2);
        assert_eq!(c.methods[0].name, "manhattan");
        assert_eq!(c.methods[0].self_kind, SelfKind::Shared);
        assert_eq!(c.methods[1].name, "translate");
        assert_eq!(c.methods[1].self_kind, SelfKind::Exclusive);
        // translate's body tail is `self.manhattan()` — a MethodCall.
        assert!(matches!(
            c.methods[1].body.tail.kind,
            ExprKind::MethodCall { ref method, ref args, .. }
                if method == "manhattan" && args.is_empty()
        ));
    }

    #[test]
    fn parse_class_init_call() {
        // C4.1 (2/N) / ADR 0022 D5: `Name::init(args)`.
        let src = "fn main() -> i64 { Point::init(3, 4); 0 }";
        let p = parse_ok_program(src);
        let main = &p.fns[0];
        let stmt0 = &main.body.stmts[0];
        if let StmtKind::Expr(e) = &stmt0.kind {
            assert!(matches!(
                &e.kind,
                ExprKind::ClassInit { class_name, args, .. }
                    if class_name == "Point" && args.len() == 2
            ));
        } else {
            panic!("expected expr stmt");
        }
    }

    #[test]
    fn parse_method_call_postfix() {
        // C4.1 (2/N) / ADR 0022 D3 + D7: postfix `.method(args)`.
        let src = "fn main() -> i64 { let p = q.compute(1, 2); 0 }";
        let p = parse_ok_program(src);
        let main = &p.fns[0];
        if let StmtKind::Let { value, .. } = &main.body.stmts[0].kind {
            assert!(matches!(
                &value.kind,
                ExprKind::MethodCall { method, args, .. }
                    if method == "compute" && args.len() == 2
            ));
        } else {
            panic!("expected let stmt");
        }
    }

    #[test]
    fn parse_method_call_chains_with_field_access() {
        // `obj.field.method()` should parse as MethodCall over
        // a FieldAccess target.
        let src = "fn main() -> i64 { x.inner.do_thing(); 0 }";
        let p = parse_ok_program(src);
        let main = &p.fns[0];
        if let StmtKind::Expr(e) = &main.body.stmts[0].kind {
            if let ExprKind::MethodCall { target, method, .. } = &e.kind {
                assert_eq!(method, "do_thing");
                assert!(matches!(
                    &target.kind,
                    ExprKind::FieldAccess { field, .. } if field == "inner"
                ));
            } else {
                panic!("expected MethodCall");
            }
        } else {
            panic!("expected expr stmt");
        }
    }

    #[test]
    fn parse_qualified_call_with_non_init_ident() {
        // C4.2 (1/N) / ADR 0023 D5 Path 2: `ImplName::method(args)`
        // qualified call. Promoted from C4.1's "non-init rejected"
        // test now that QualifiedCall ships at C4.2 (1/N).
        let p = parse_ok_program("fn main() -> i64 { Buffered::write(0, 1) }");
        let main = &p.fns[0];
        let tail = &main.body.tail;
        assert!(matches!(
            &tail.kind,
            ExprKind::QualifiedCall { impl_name, method, args, .. }
                if impl_name == "Buffered" && method == "write" && args.len() == 2
        ));
    }

    // ----- C4.2 (1/N): trait + impl + qualified-call -----

    #[test]
    fn parse_trait_decl_empty() {
        let p = parse_ok_program("trait Marker { }\nfn main() -> i64 { 0 }");
        assert_eq!(p.traits.len(), 1);
        assert_eq!(p.traits[0].name, "Marker");
        assert!(p.traits[0].methods.is_empty());
    }

    #[test]
    fn parse_trait_decl_one_method() {
        let p = parse_ok_program(
            "trait Writer { fn write(self: &mut Self, data: i64) -> i64; }\nfn main() -> i64 { 0 }",
        );
        let t = &p.traits[0];
        assert_eq!(t.name, "Writer");
        assert_eq!(t.methods.len(), 1);
        let m = &t.methods[0];
        assert_eq!(m.name, "write");
        assert_eq!(m.self_kind, SelfKind::Exclusive);
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "data");
    }

    #[test]
    fn parse_trait_decl_two_methods() {
        let p = parse_ok_program(
            "trait Writer {\n  fn write(self: &mut Self, data: i64) -> i64;\n  fn flush(self: &mut Self) -> i64;\n}\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.traits[0].methods.len(), 2);
        assert_eq!(p.traits[0].methods[0].name, "write");
        assert_eq!(p.traits[0].methods[1].name, "flush");
    }

    #[test]
    fn parse_trait_method_sig_requires_semi() {
        // Body-with-block instead of `;` rejected.
        let err = parse(
            "trait T { fn f(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn parse_impl_decl_default_form() {
        // `impl as Trait for Type { ... }` — no name, default impl.
        let p = parse_ok_program(
            "impl as Writer for File { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.impls.len(), 1);
        let i = &p.impls[0];
        assert_eq!(i.name, None);
        assert_eq!(i.trait_name, "Writer");
        assert_eq!(i.type_name, "File");
        assert_eq!(i.methods.len(), 1);
        assert_eq!(i.methods[0].name, "write");
    }

    #[test]
    fn parse_impl_decl_named_form() {
        // `impl Name as Trait for Type { ... }`.
        let p = parse_ok_program(
            "impl Buffered as Writer for File { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        let i = &p.impls[0];
        assert_eq!(i.name.as_deref(), Some("Buffered"));
        assert_eq!(i.trait_name, "Writer");
        assert_eq!(i.type_name, "File");
    }

    #[test]
    fn parse_impl_decl_pub_method() {
        let p = parse_ok_program(
            "impl as Writer for File { pub fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.impls[0].methods[0].visibility, Visibility::Public);
    }

    #[test]
    fn parse_impl_decl_rejects_missing_as() {
        // `impl Writer for File { ... }` (no `as`) — rejected at C4.2.
        let err = parse(
            "impl Writer for File { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn parse_impl_decl_rejects_missing_for() {
        // `impl as Writer File { ... }` (no `for`) — rejected.
        let err = parse(
            "impl as Writer File { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn parse_impl_with_two_methods() {
        let p = parse_ok_program(
            "impl as Writer for File {\n  fn write(self: &mut Self, d: i64) -> i64 { d }\n  fn flush(self: &mut Self) -> i64 { 0 }\n}\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.impls[0].methods.len(), 2);
        assert_eq!(p.impls[0].methods[0].name, "write");
        assert_eq!(p.impls[0].methods[1].name, "flush");
    }

    #[test]
    fn parse_full_c42_surface() {
        // ADR 0023 D12 phase-go shape: trait + class + default impl
        // + named impl + main using both dispatch paths.
        let src = r#"
            trait Writer {
                fn write(self: &mut Self, data: i64) -> i64;
            }
            class FileSink {
                let count: i64;
                pub init() {
                    self.count = 0;
                    0
                }
            }
            impl as Writer for FileSink {
                fn write(self: &mut Self, data: i64) -> i64 {
                    self.count = self.count + data;
                    self.count
                }
            }
            impl Doubling as Writer for FileSink {
                fn write(self: &mut Self, data: i64) -> i64 {
                    self.count = self.count + data * 2;
                    self.count
                }
            }
            fn main() -> i64 {
                let mut s: FileSink = FileSink::init();
                let a: i64 = s.write(10);
                let b: i64 = Doubling::write(&mut s, 16);
                b
            }
        "#;
        let p = parse_ok_program(src);
        assert_eq!(p.traits.len(), 1);
        assert_eq!(p.classes.len(), 1);
        assert_eq!(p.impls.len(), 2);
        assert_eq!(p.impls[0].name, None);
        assert_eq!(p.impls[1].name.as_deref(), Some("Doubling"));
    }

    // ========================================================================
    // C4.3 (delegation) tests per ADR 0021 D6. `delegate field: T to Trait;`
    // inside a class body. `to` is recognised positionally (a plain Ident
    // token per C4.0 D11).
    // ========================================================================

    #[test]
    fn parse_class_decl_one_delegate() {
        let p = parse_ok_program(
            "class Logger { delegate writer: FileSink to Writer; init() { 0 } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        assert_eq!(c.delegates.len(), 1);
        let d = &c.delegates[0];
        assert_eq!(d.field_name, "writer");
        assert_eq!(d.ty.kind.to_string(), "FileSink");
        assert_eq!(d.trait_name, "Writer");
        assert_eq!(d.visibility, Visibility::Private);
    }

    #[test]
    fn parse_class_decl_pub_delegate() {
        let p = parse_ok_program(
            "class Logger { pub delegate writer: FileSink to Writer; init() { 0 } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.classes[0].delegates[0].visibility, Visibility::Public);
    }

    #[test]
    fn parse_class_decl_two_delegates() {
        let p = parse_ok_program(
            "class Pipe { delegate w: FileSink to Writer; delegate r: FileSrc to Reader; init() { 0 } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        assert_eq!(c.delegates.len(), 2);
        assert_eq!(c.delegates[0].field_name, "w");
        assert_eq!(c.delegates[0].trait_name, "Writer");
        assert_eq!(c.delegates[1].field_name, "r");
        assert_eq!(c.delegates[1].trait_name, "Reader");
    }

    #[test]
    fn parse_class_decl_delegate_missing_to_rejects() {
        let err = parse(
            "class Logger { delegate writer: FileSink Writer; init() { 0 } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("`to`")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_class_decl_delegate_missing_trait_rejects() {
        let err = parse(
            "class Logger { delegate writer: FileSink to ; init() { 0 } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("trait name after `to`")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_class_decl_delegate_missing_semi_rejects() {
        let err = parse(
            "class Logger { delegate writer: FileSink to Writer init() { 0 } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("`;` after delegate")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_class_decl_full_surface_with_delegate() {
        // Mixed class items: field + delegate + init + method.
        let p = parse_ok_program(
            r#"
            class Logger {
                let count: i64;
                delegate writer: FileSink to Writer;
                init(w: FileSink) {
                    self.count = 0;
                    0
                }
                pub fn tally(self: &Self) -> i64 { self.count }
            }
            fn main() -> i64 { 0 }
            "#,
        );
        let c = &p.classes[0];
        assert_eq!(c.fields.len(), 1);
        assert_eq!(c.delegates.len(), 1);
        assert!(c.init.is_some());
        assert_eq!(c.methods.len(), 1);
    }

    // ========================================================================
    // C4.4 (1/N): scope / spawn / await parser tests per ADR 0024 D1-D3.
    // `concurrent` is positional (a plain Ident); `await` is reserved at
    // C4.0 (TokenKind::Await). Downstream resolve surfaces NotYet
    // diagnostics until C4.4 (2/N) brings up the runtime + codegen.
    // ========================================================================

    #[test]
    fn parse_scope_concurrent_block() {
        let p = parse_ok_program(
            "fn main() -> i64 { scope concurrent { 42 } }",
        );
        match &p.fns.iter().find(|f| f.name == "main").expect("has main").body.tail.kind {
            ExprKind::Scope { mode, body } => {
                assert_eq!(*mode, sentinel_ast::ScopeMode::Concurrent);
                assert_eq!(body.stmts.len(), 0);
            }
            other => panic!("expected Scope, got {other:?}"),
        }
    }

    #[test]
    fn parse_scope_missing_concurrent_rejects() {
        let err = parse("fn main() -> i64 { scope { 0 } }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("`concurrent`")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_spawn_fn_call() {
        let p = parse_ok_program(
            "fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { spawn double(21); 0 }",
        );
        match &p.fns.iter().find(|f| f.name == "main").expect("has main").body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Spawn { call_expr } => {
                    assert!(matches!(call_expr.kind, ExprKind::Call { .. }));
                }
                other => panic!("expected Spawn, got {other:?}"),
            },
            other => panic!("expected Expr stmt, got {other:?}"),
        }
    }

    #[test]
    fn parse_await_postfix() {
        // Postfix `.await` on a let-bound variable. `spawn` is
        // prefix and binds looser than postfix `.await`, so
        // `spawn double(21).await` parses as `spawn (double(21).await)`.
        // A let binding sidesteps the precedence question.
        let p = parse_ok_program(
            "fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { let t = 0; t.await }",
        );
        match &p.fns.iter().find(|f| f.name == "main").expect("has main").body.tail.kind {
            ExprKind::Await { task_expr } => {
                assert!(matches!(task_expr.kind, ExprKind::Var(_)));
            }
            other => panic!("expected Await, got {other:?}"),
        }
    }

    #[test]
    fn parse_await_on_parenthesized_spawn() {
        // Explicit parens override the prefix-spawn / postfix-.await
        // precedence so `(spawn fn(x)).await` lands as
        // `Await { task_expr: Spawn { ... } }`.
        let p = parse_ok_program(
            "fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { (spawn double(21)).await }",
        );
        match &p.fns.iter().find(|f| f.name == "main").expect("has main").body.tail.kind {
            ExprKind::Await { task_expr } => {
                assert!(matches!(task_expr.kind, ExprKind::Spawn { .. }));
            }
            other => panic!("expected Await, got {other:?}"),
        }
    }

    #[test]
    fn parse_scope_spawn_await_combined() {
        // Phase-go shape: scope concurrent { let t = spawn fn(args); t.await }
        let p = parse_ok_program(
            r#"
            fn double(x: i64) -> i64 { x * 2 }
            fn main() -> i64 {
                scope concurrent {
                    let t = spawn double(21);
                    t.await
                }
            }
            "#,
        );
        match &p.fns.iter().find(|f| f.name == "main").expect("has main").body.tail.kind {
            ExprKind::Scope { body, .. } => {
                assert_eq!(body.stmts.len(), 1);
                assert!(matches!(body.tail.kind, ExprKind::Await { .. }));
            }
            other => panic!("expected Scope, got {other:?}"),
        }
    }

    #[test]
    fn parse_class_decl_duplicate_init_rejects() {
        let err = parse(
            "class Bad { init() { 0 } init() { 0 } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(
            matches!(&err, ParseError::DuplicateClassInit { class_name, .. } if class_name == "Bad"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_class_decl_generic_rejected() {
        // Generic classes deferred per ADR 0022 D1.
        let err = parse("class Pair<A, B> { }\nfn main() -> i64 { 0 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("generic classes deferred")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_class_decl_method_missing_self_rejects() {
        // Methods must start with `self: &Self` or `self: &mut Self`.
        let err = parse(
            "class Bad { fn no_self(x: i64) -> i64 { x } }\nfn main() -> i64 { 0 }",
        )
        .unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if expected.contains("`self`")),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_struct_decl_two_fields() {
        let p = parse_ok_program(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs.len(), 1);
        let s = &p.structs[0];
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].name, "x");
        assert_eq!(s.fields[0].ty.kind.to_string(), "i64");
        assert_eq!(s.fields[1].name, "y");
    }

    #[test]
    fn parse_struct_decl_empty() {
        let p = parse_ok_program("struct Empty { }\nfn main() -> i64 { 0 }");
        assert_eq!(p.structs[0].name, "Empty");
        assert!(p.structs[0].fields.is_empty());
    }

    #[test]
    fn parse_struct_decl_trailing_comma() {
        let p = parse_ok_program(
            "struct P { x: i64, y: i64, }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields.len(), 2);
    }

    #[test]
    fn parse_struct_lit_in_let() {
        let p = parse_ok_program(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let p = P { x: 3, y: 4 }; 0 }",
        );
        let main = &p.fns[0];
        match &main.body.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::StructLit { name, fields, .. } => {
                    assert_eq!(name, "P");
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name, "x");
                    assert_eq!(fields[1].name, "y");
                }
                other => panic!("expected StructLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_struct_lit_display_pretty() {
        // Struct literal Display: (struct-lit Name (x 3) (y 4))
        let e = parse_expr("Foo { x: 3, y: 4 }").expect("parse");
        assert_eq!(e.kind.to_string(), "(struct-lit Foo (x 3) (y 4))");
    }

    #[test]
    fn parse_field_access_simple() {
        assert_eq!(pretty("p.x"), "(. p x)");
    }

    #[test]
    fn parse_field_access_chained() {
        assert_eq!(pretty("a.b.c"), "(. (. a b) c)");
    }

    #[test]
    fn parse_field_access_after_call() {
        // f(x).y -> (. (f x) y)
        assert_eq!(pretty("f(x).y"), "(. (f x) y)");
    }

    #[test]
    fn parse_field_access_in_arithmetic() {
        assert_eq!(pretty("p.x + p.y"), "(+ (. p x) (. p y))");
    }

    #[test]
    fn parse_field_access_binds_tighter_than_unary() {
        // -p.x -> (- (. p x))
        assert_eq!(pretty("-p.x"), "(- (. p x))");
    }

    #[test]
    fn parse_field_access_binds_tighter_than_not() {
        // !p.b -> (! (. p b))
        assert_eq!(pretty("!p.b"), "(! (. p b))");
    }

    #[test]
    fn parse_struct_lit_with_trailing_comma() {
        let e = parse_expr("Foo { x: 1, y: 2, }").expect("parse");
        match e.kind {
            ExprKind::StructLit { fields, .. } => assert_eq!(fields.len(), 2),
            other => panic!("expected StructLit, got {other:?}"),
        }
    }

    #[test]
    fn parse_struct_lit_forbidden_in_if_cond() {
        // ADR 0013 D3a: bare `Foo { ... }` is forbidden in if-cond
        // position. The parser should treat `Foo` as a Var atom
        // and then expect a block for the if-then; the `{ x: 1 }`
        // becomes the if-then block but the `x: 1` inside is a
        // parse error.
        let err = parse("fn main() -> i64 { if Foo { x: 1 } { 1 } else { 2 } }").unwrap_err();
        // The exact error message depends on which token fails first,
        // but it should be a parse error of some sort.
        assert!(
            matches!(err, ParseError::UnexpectedToken { .. } | ParseError::UnexpectedEof { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_struct_lit_in_if_cond_via_parens_ok() {
        // Per D3a, parens escape: `(Foo { x: 1 }.x == 1)` is fine.
        let p = parse_ok_program(
            "struct Foo { x: i64 }\nfn main() -> i64 { if (Foo { x: 1 }.x == 1) { 7 } else { 0 } }",
        );
        // We don't deeply inspect; the success of parse is the assertion.
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.structs.len(), 1);
    }

    #[test]
    fn parse_struct_decl_with_bool_field() {
        let p = parse_ok_program(
            "struct Flag { value: bool }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[0].ty.kind.to_string(), "bool");
    }

    #[test]
    fn parse_mixed_structs_and_fns() {
        let p = parse_ok_program(
            "struct A { x: i64 }\nfn foo() -> i64 { 1 }\nstruct B { y: i64 }\nfn main() -> i64 { 0 }",
        );
        // Both structs and both fns land in the right vectors regardless of source order.
        assert_eq!(p.structs.len(), 2);
        assert_eq!(p.fns.len(), 2);
    }

    #[test]
    fn parse_error_struct_missing_name() {
        let err = parse("struct { x: i64 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "struct name after `struct`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_struct_missing_open_brace() {
        let err = parse("struct Foo x: i64 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`{` to open struct body"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_struct_field_missing_colon() {
        let err = parse("struct Foo { x i64 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`:` followed by field type"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_field_access_no_field_name() {
        let err = parse_expr("p.").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "field name after `.`"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_field_access_not_ident() {
        // `p.1` — second token after `.` is an IntLit, not an Ident.
        let err = parse_expr("p.1").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "field name after `.`"),
            "got {err:?}"
        );
    }

    // ----- C1.5: nullable types + null literal -----

    #[test]
    fn parse_null_literal() {
        assert_eq!(pretty("null"), "null");
    }

    #[test]
    fn parse_nullable_type_in_param() {
        let p = parse_ok_program("fn f(x: ?i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "?i64");
    }

    #[test]
    fn parse_nullable_type_in_return() {
        let p = parse_ok_program("fn f() -> ?i64 { null }");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "?i64");
    }

    #[test]
    fn parse_nullable_let_annotation() {
        let block = parse_block_str("{ let x: ?i64 = null; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { ty_annot, value, .. } => {
                let ty = ty_annot.as_ref().expect("annot present");
                assert_eq!(ty.kind.to_string(), "?i64");
                match &value.kind {
                    ExprKind::NullLit => {}
                    other => panic!("expected NullLit, got {other:?}"),
                }
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_nullable_struct_field() {
        let p = parse_ok_program(
            "struct Node { value: i64, next: ?Node }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[1].ty.kind.to_string(), "?Node");
    }

    #[test]
    fn parse_nullable_type_with_whitespace() {
        // ADR 0014 D1: whitespace allowed between `?` and base type.
        let p = parse_ok_program("fn f(x: ? i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "?i64");
    }

    #[test]
    fn parse_error_nested_nullable() {
        // ADR 0014 D6: `??T` is rejected at parse time.
        let err = parse("fn f(x: ??i64) -> i64 { 0 }").unwrap_err();
        assert!(matches!(&err, ParseError::NestedNullable { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_question_with_no_type() {
        // `?` not followed by a base type — parse_type recurses
        // and hits an UnexpectedToken or UnexpectedEof.
        let err = parse_expr("?").unwrap_err();
        // The parser sees `?` in an expression position (parse_expr →
        // parse_or → ... → parse_atom), which doesn't know about `?`,
        // so it errors as UnexpectedToken.
        assert!(
            matches!(&err, ParseError::UnexpectedToken { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_null_in_arithmetic_position_parses() {
        // `null + 1` parses (syntactic level); the type checker
        // rejects later.
        assert_eq!(pretty("null + 1"), "(+ null 1)");
    }

    // ----- C1.6: arrays, indexing, array literals -----

    #[test]
    fn parse_array_type_in_param() {
        let p = parse_ok_program("fn f(xs: [i64]) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "[i64]");
    }

    #[test]
    fn parse_array_type_in_return() {
        let p = parse_ok_program("fn f() -> [i64] { [] }");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "[i64]");
    }

    #[test]
    fn parse_nullable_array_type() {
        // `?[i64]` per ADR 0015 D6.
        let p = parse_ok_program("fn f(x: ?[i64]) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "?[i64]");
    }

    #[test]
    fn parse_array_of_nullable_type() {
        // `[?i64]` per ADR 0015 D6.
        let p = parse_ok_program("fn f(x: [?i64]) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "[?i64]");
    }

    #[test]
    fn parse_array_literal_three_elems() {
        assert_eq!(pretty("[1, 2, 3]"), "(array 1 2 3)");
    }

    #[test]
    fn parse_array_literal_empty() {
        assert_eq!(pretty("[]"), "(array)");
    }

    #[test]
    fn parse_array_literal_trailing_comma() {
        assert_eq!(pretty("[1, 2,]"), "(array 1 2)");
    }

    #[test]
    fn parse_array_literal_one_elem() {
        assert_eq!(pretty("[42]"), "(array 42)");
    }

    #[test]
    fn parse_array_index() {
        assert_eq!(pretty("a[0]"), "(index a 0)");
    }

    #[test]
    fn parse_array_index_with_var() {
        assert_eq!(pretty("a[i]"), "(index a i)");
    }

    // ----- D.2 / ADR 0033 D2: char + string literals (decode) -----

    #[test]
    fn parse_char_lit_basic() {
        // The decoded byte is the character's ASCII value.
        assert_eq!(pretty("'a'"), "(char 97)");
        assert_eq!(pretty("'A'"), "(char 65)");
        assert_eq!(pretty("'0'"), "(char 48)");
        assert_eq!(pretty("' '"), "(char 32)");
    }

    #[test]
    fn parse_char_lit_escapes() {
        // Every recognised non-hex escape decodes to its byte (the char
        // surface needs `\'`; `\"` is exercised in the string tests).
        assert_eq!(pretty(r"'\n'"), "(char 10)");
        assert_eq!(pretty(r"'\t'"), "(char 9)");
        assert_eq!(pretty(r"'\r'"), "(char 13)");
        assert_eq!(pretty(r"'\0'"), "(char 0)");
        assert_eq!(pretty(r"'\\'"), "(char 92)");
        assert_eq!(pretty(r"'\''"), "(char 39)");
    }

    #[test]
    fn parse_char_lit_hex_escape() {
        // `\xHH` — two hex digits → one byte; case-insensitive; the full
        // 0..=255 range is reachable (beyond what a bare ASCII char gives).
        assert_eq!(pretty(r"'\x41'"), "(char 65)");
        assert_eq!(pretty(r"'\x00'"), "(char 0)");
        assert_eq!(pretty(r"'\xff'"), "(char 255)");
        assert_eq!(pretty(r"'\xFF'"), "(char 255)");
        assert_eq!(pretty(r"'\x7e'"), "(char 126)");
    }

    #[test]
    fn parse_char_lit_digit_arithmetic_shape() {
        // The lexer's `c - '0'` idiom: char literals inside an expression.
        assert_eq!(pretty("'7' - '0'"), "(- (char 55) (char 48))");
    }

    #[test]
    fn parse_char_lit_empty_rejected() {
        // `''` decodes to zero bytes — a char must be exactly one byte.
        let err = parse_expr("''").unwrap_err();
        assert!(matches!(err, ParseError::CharLitNotSingleByte { .. }), "got {err:?}");
    }

    #[test]
    fn parse_char_lit_multi_byte_rejected() {
        // Two ASCII bytes — a char is exactly one byte.
        let err = parse_expr("'ab'").unwrap_err();
        assert!(matches!(err, ParseError::CharLitNotSingleByte { .. }), "got {err:?}");
    }

    #[test]
    fn parse_char_lit_multibyte_utf8_rejected() {
        // `'é'` is two UTF-8 bytes (0xC3 0xA9) — rejected (single-byte
        // only; no Unicode code points at this MVP — ADR 0033 D8).
        let err = parse_expr("'é'").unwrap_err();
        assert!(matches!(err, ParseError::CharLitNotSingleByte { .. }), "got {err:?}");
    }

    #[test]
    fn parse_char_lit_unknown_escape_rejected() {
        let err = parse_expr(r"'\q'").unwrap_err();
        assert!(matches!(err, ParseError::InvalidEscape { .. }), "got {err:?}");
    }

    #[test]
    fn parse_char_lit_short_hex_escape_rejected() {
        // `\x` with fewer than two hex digits.
        let err = parse_expr(r"'\x4'").unwrap_err();
        assert!(matches!(err, ParseError::InvalidEscape { .. }), "got {err:?}");
    }

    #[test]
    fn parse_char_lit_non_hex_escape_rejected() {
        // `\x` with non-hex digits.
        let err = parse_expr(r"'\xZZ'").unwrap_err();
        assert!(matches!(err, ParseError::InvalidEscape { .. }), "got {err:?}");
    }

    #[test]
    fn parse_string_lit_basic() {
        // The decoded bytes ARE the string (`"let"` → l e t = 108 101 116).
        assert_eq!(pretty(r#""let""#), "(string 108 101 116)");
    }

    #[test]
    fn parse_string_lit_empty() {
        assert_eq!(pretty(r#""""#), "(string)");
    }

    #[test]
    fn parse_string_lit_escapes() {
        // Escapes decode inside strings too, including `\"` and `\xHH`.
        assert_eq!(pretty(r#""a\nb""#), "(string 97 10 98)");
        assert_eq!(pretty(r#""\x41\x42""#), "(string 65 66)");
        assert_eq!(pretty(r#""a\"b""#), "(string 97 34 98)");
    }

    #[test]
    fn parse_string_lit_utf8_bytes() {
        // A multi-byte source char is kept as its UTF-8 bytes — a string
        // IS its UTF-8 bytes (ADR 0033 D3). `é` = 0xC3 0xA9 = 195 169.
        assert_eq!(pretty(r#""é""#), "(string 195 169)");
    }

    #[test]
    fn parse_string_lit_bad_escape_rejected() {
        // A malformed `\x` inside a string is rejected at parse time.
        let err = parse_expr(r#""ab\x""#).unwrap_err();
        assert!(matches!(err, ParseError::InvalidEscape { .. }), "got {err:?}");
    }

    #[test]
    fn parse_string_lit_in_let() {
        // The `let s = "let";` shape — a string literal as a let value.
        let block = parse_block_str(r#"{ let s = "let"; s }"#).expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { value, .. } => {
                assert_eq!(value.kind.to_string(), "(string 108 101 116)");
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_u8_in_type_position() {
        // D.2 / ADR 0033 D2 (2/N): `u8` is a plain `TypeExpr` Ident (no
        // keyword token, no type-parser change) — usable as a param
        // type, return type, and array element `[u8]`. It resolves to
        // `Type::U8` at D.2 (3/N); here it must simply parse.
        let p = parse_ok_program("fn first(s: [u8]) -> u8 { s[0] }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "[u8]");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "u8");
    }

    #[test]
    fn parse_array_index_chained() {
        // a[i].x — indexing then field access — postfix chain.
        assert_eq!(pretty("a[i].x"), "(. (index a i) x)");
    }

    #[test]
    fn parse_field_then_index() {
        // p.xs[0] — field access then index.
        assert_eq!(pretty("p.xs[0]"), "(index (. p xs) 0)");
    }

    #[test]
    fn parse_array_index_in_arithmetic() {
        assert_eq!(pretty("a[0] + a[1]"), "(+ (index a 0) (index a 1))");
    }

    #[test]
    fn parse_nested_array_literal() {
        // `[[1, 2], [3, 4]]` parses; rejection happens at the
        // type-resolve stage per ADR 0015 D6.
        let _ = parse_expr("[[1, 2], [3, 4]]").expect("parse");
    }

    #[test]
    fn parse_nested_array_type_parses() {
        // `[[i64]]` parses (the grammar accepts); type-resolve
        // will reject.
        let p = parse_ok_program("fn f(x: [[i64]]) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "[[i64]]");
    }

    #[test]
    fn parse_error_unclosed_array_type() {
        let err = parse("fn f(x: [i64) -> i64 { 0 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`]` to close array type"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_unclosed_array_literal() {
        let err = parse_expr("[1, 2").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`,` or `]` in array literal"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_unclosed_index() {
        let err = parse_expr("a[0").unwrap_err();
        assert!(
            matches!(&err, ParseError::UnexpectedEof { expected, .. } if *expected == "`]` to close index expression"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_array_literal_in_let() {
        let block = parse_block_str("{ let xs = [1, 2, 3]; xs }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::ArrayLit(elems) => assert_eq!(elems.len(), 3),
                other => panic!("expected ArrayLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_struct_with_array_field() {
        let p = parse_ok_program(
            "struct Bag { items: [i64] }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[0].ty.kind.to_string(), "[i64]");
    }

    // ----- C1.7 / ADR 0016: generic fns + generic structs + type args -----

    #[test]
    fn parse_generic_fn_single_type_param() {
        let p = parse_ok_program("fn id<T>(x: T) -> T { x }\nfn main() -> i64 { 0 }");
        let f = &p.fns[0];
        assert_eq!(f.name, "id");
        assert_eq!(f.type_params.len(), 1);
        assert_eq!(f.type_params[0].name, "T");
    }

    #[test]
    fn parse_generic_fn_multiple_type_params() {
        let p = parse_ok_program(
            "fn pair<A, B>(a: A, b: B) -> A { a }\nfn main() -> i64 { 0 }",
        );
        let f = &p.fns[0];
        assert_eq!(f.type_params.len(), 2);
        assert_eq!(f.type_params[0].name, "A");
        assert_eq!(f.type_params[1].name, "B");
    }

    #[test]
    fn parse_generic_fn_trailing_comma_in_type_params() {
        let p = parse_ok_program(
            "fn pair<A, B,>(a: A, b: B) -> A { a }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].type_params.len(), 2);
    }

    #[test]
    fn parse_generic_struct_single_type_param() {
        let p = parse_ok_program(
            "struct Box<T> { value: T }\nfn main() -> i64 { 0 }",
        );
        let s = &p.structs[0];
        assert_eq!(s.name, "Box");
        assert_eq!(s.type_params.len(), 1);
        assert_eq!(s.type_params[0].name, "T");
        assert_eq!(s.fields[0].ty.kind.to_string(), "T");
    }

    #[test]
    fn parse_generic_struct_multiple_type_params() {
        let p = parse_ok_program(
            "struct Pair<A, B> { first: A, second: B }\nfn main() -> i64 { 0 }",
        );
        let s = &p.structs[0];
        assert_eq!(s.type_params.len(), 2);
        assert_eq!(s.fields.len(), 2);
    }

    #[test]
    fn parse_generic_type_in_param_position() {
        let p = parse_ok_program(
            "fn unbox(b: Box<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "Box<i64>");
    }

    #[test]
    fn parse_generic_type_multi_arg() {
        let p = parse_ok_program(
            "fn f(p: Pair<i64, bool>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "Pair<i64, bool>");
    }

    #[test]
    fn parse_generic_type_nested() {
        let p = parse_ok_program(
            "fn f(p: Pair<Box<i64>, bool>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "Pair<Box<i64>, bool>");
    }

    #[test]
    fn parse_generic_type_with_nullable_arg() {
        let p = parse_ok_program(
            "fn f(b: Box<?i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "Box<?i64>");
    }

    #[test]
    fn parse_generic_type_with_array_arg() {
        let p = parse_ok_program(
            "fn f(b: Box<[i64]>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "Box<[i64]>");
    }

    #[test]
    fn parse_generic_in_return_type() {
        let p = parse_ok_program(
            "fn make_box(x: i64) -> Box<i64> { 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].return_type.kind.to_string(), "Box<i64>");
    }

    #[test]
    fn parse_generic_in_let_annotation() {
        let p = parse_ok_program(
            "fn main() -> i64 { let x: Box<i64> = 0; x }",
        );
        let f = &p.fns[0];
        match &f.body.stmts[0].kind {
            StmtKind::Let { ty_annot: Some(ty), .. } => {
                assert_eq!(ty.kind.to_string(), "Box<i64>");
            }
            other => panic!("expected Let with annotation, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_empty_type_params_on_fn() {
        let err = parse("fn id<>(x: i64) -> i64 { x }").unwrap_err();
        assert!(matches!(&err, ParseError::EmptyTypeParams { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_empty_type_params_on_struct() {
        let err = parse("struct Foo<> { x: i64 }\nfn main() -> i64 { 0 }").unwrap_err();
        assert!(matches!(&err, ParseError::EmptyTypeParams { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_empty_type_args() {
        let err = parse("fn f(x: Box<>) -> i64 { 0 }").unwrap_err();
        assert!(matches!(&err, ParseError::EmptyTypeArgs { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_unclosed_type_params() {
        let err = parse("fn id<T(x: T) -> T { x }").unwrap_err();
        assert!(matches!(&err, ParseError::UnexpectedToken { .. }), "got {err:?}");
    }

    #[test]
    fn parse_error_unclosed_type_args() {
        let err = parse("fn f(x: Box<i64) -> i64 { 0 }").unwrap_err();
        assert!(matches!(&err, ParseError::UnexpectedToken { .. }), "got {err:?}");
    }

    #[test]
    fn parse_generic_struct_with_nullable_field() {
        // `?T` as a field type inside a generic struct.
        let p = parse_ok_program(
            "struct Maybe<T> { inner: ?T }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[0].ty.kind.to_string(), "?T");
    }

    // ----- C2.0.2 / ADR 0017: refs, mut, deref, assignment -----

    #[test]
    fn parse_ref_type_in_param() {
        let p = parse_ok_program("fn f(x: &i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&i64");
    }

    #[test]
    fn parse_ref_mut_type_in_param() {
        let p = parse_ok_program("fn f(x: &mut i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&mut i64");
    }

    #[test]
    fn parse_ref_type_with_whitespace() {
        // `& mut T` whitespace-tolerant per ADR 0017 D1.
        let p = parse_ok_program("fn f(x: & mut i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&mut i64");
    }

    #[test]
    fn parse_ref_type_in_return() {
        let p = parse_ok_program("fn f() -> &i64 { let x: i64 = 0; &x }");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "&i64");
    }

    #[test]
    fn parse_unary_ref_expr() {
        // `&x` is a prefix unary borrow-take.
        assert_eq!(pretty("&x"), "(& x)");
    }

    #[test]
    fn parse_unary_ref_mut_expr() {
        assert_eq!(pretty("&mut x"), "(&mut x)");
    }

    #[test]
    fn parse_unary_deref_expr() {
        assert_eq!(pretty("*r"), "(* r)");
    }

    #[test]
    fn parse_deref_in_arithmetic() {
        // `*a + *b` — both `*` are prefix unary derefs, not multiplies.
        assert_eq!(pretty("*a + *b"), "(+ (* a) (* b))");
    }

    #[test]
    fn parse_mul_still_works() {
        // Make sure parse_unary's new `*` prefix doesn't break the
        // infix `*` in parse_mul.
        assert_eq!(pretty("a * b"), "(* a b)");
    }

    #[test]
    fn parse_let_mut_no_annotation() {
        let block = parse_block_str("{ let mut x = 5; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { mutable: true, name, .. } => assert_eq!(name, "x"),
            other => panic!("expected Let-mut, got {other:?}"),
        }
    }

    #[test]
    fn parse_let_mut_with_annotation() {
        let block = parse_block_str("{ let mut x: i64 = 5; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { mutable: true, ty_annot, .. } => {
                assert_eq!(ty_annot.as_ref().expect("annot").kind.to_string(), "i64");
            }
            other => panic!("expected Let-mut, got {other:?}"),
        }
    }

    #[test]
    fn parse_let_without_mut_is_immutable() {
        let block = parse_block_str("{ let x = 5; x }").expect("parse");
        match &block.stmts[0].kind {
            StmtKind::Let { mutable: false, .. } => {}
            other => panic!("expected immutable Let, got {other:?}"),
        }
    }

    #[test]
    fn parse_mut_param() {
        // `fn f(mut x: i64) -> i64 { x }` — `mut` is a binding-local
        // modifier per ADR 0017 D2.
        let p = parse_ok_program("fn f(mut x: i64) -> i64 { x }\nfn main() -> i64 { 0 }");
        assert!(p.fns[0].params[0].mutable);
        assert_eq!(p.fns[0].params[0].name, "x");
    }

    #[test]
    fn parse_assign_var_stmt() {
        let block = parse_block_str("{ let mut x = 0; x = 5; x }").expect("parse");
        // Stmt[1] is the assignment.
        match &block.stmts[1].kind {
            StmtKind::Assign { target, value } => {
                assert_eq!(target.kind.to_string(), "x");
                assert_eq!(value.kind.to_string(), "5");
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn parse_assign_deref_stmt() {
        // `*r = v;` deref-assignment.
        let p = parse_ok_program(
            "fn set(r: &mut i64, v: i64) -> i64 { *r = v; v }\nfn main() -> i64 { 0 }",
        );
        let f = &p.fns[0];
        match &f.body.stmts[0].kind {
            StmtKind::Assign { target, .. } => {
                assert_eq!(target.kind.to_string(), "(* r)");
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn parse_assign_index_stmt() {
        // `a[i] = v;` index-assignment (ADR 0050). The parser already
        // accepts an `Index` lvalue target (all lvalue / mutability
        // validation is deferred to the type checker); no parser change.
        let block =
            parse_block_str("{ let mut a = [1, 2, 3]; let i = 1; a[i] = 9; a[0] }").expect("parse");
        match &block.stmts[2].kind {
            StmtKind::Assign { target, value } => {
                assert_eq!(target.kind.to_string(), "(index a i)");
                assert_eq!(value.kind.to_string(), "9");
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_assign_missing_semi() {
        let err = parse_block_err("let mut x = 0; x = 5");
        assert!(
            matches!(&err, ParseError::UnexpectedToken { expected, .. } if *expected == "`;` after assignment"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_ref_take_of_field_access() {
        // `&p.x` — `&` is unary so the LHS is the postfix chain.
        // Precedence: `&p.x` parses as `&(p.x)` because postfix `.`
        // binds tighter than prefix `&`.
        assert_eq!(pretty("&p.x"), "(& (. p x))");
    }

    #[test]
    fn parse_deref_then_field_access() {
        // `(*r).x` — explicit paren. Without parens, `*r.x` would
        // parse as `*(r.x)` since postfix binds tighter than prefix.
        assert_eq!(pretty("*r.x"), "(* (. r x))");
        assert_eq!(pretty("(*r).x"), "(. (* r) x)");
    }

    #[test]
    fn parse_ref_then_deref_round_trip() {
        // `*&x` — first parse `&x`, then deref.
        assert_eq!(pretty("*&x"), "(* (& x))");
    }

    #[test]
    fn parse_nested_ref_type_parses() {
        // `&&T` lexes as AmpAmp (logical-and); the type checker
        // never sees a doubled-ref via this token. The user can
        // write `& &T` with whitespace to provoke a `NestedRef`
        // type error.
        let p = parse_ok_program("fn f(x: & &i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&&i64");
    }

    #[test]
    fn parse_nullable_ref_type() {
        // `?&T` — ADR 0017 D1.
        let p = parse_ok_program("fn f(x: ?&i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "?&i64");
    }

    #[test]
    fn parse_ref_of_nullable_type() {
        // `&?T` — ADR 0017 D1.
        let p = parse_ok_program("fn f(x: &?i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&?i64");
    }

    #[test]
    fn parse_ref_of_array_type() {
        // `&[T]` — passes parse; the type checker accepts it.
        let p = parse_ok_program("fn f(x: &[i64]) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&[i64]");
    }

    #[test]
    fn parse_generic_in_call_arg_position_is_not_a_thing() {
        // C1.7 has no turbofish (per ADR 0016 D4). `f(g, h)` is
        // still parsed as a call with two args; `f<g, h>(x)` is
        // NOT special at call sites — the `<` would be lexed as
        // comparison and the parser handles that under expression
        // grammar. We don't assert success here; just confirm the
        // parser doesn't get confused into believing call sites
        // accept type args.
        let _ = parse_expr("f(g, h)").expect("plain call still works");
    }

    // ---- C3 / ADR 0019 D1+D4+D5+D6: surface parser tests ----

    #[test]
    fn parse_effect_decl_empty() {
        let p = parse_ok_program("effect Io { } fn main() -> i64 { 0 }");
        assert_eq!(p.effects.len(), 1);
        assert_eq!(p.effects[0].name, "Io");
        assert!(p.effects[0].ops.is_empty());
    }

    #[test]
    fn parse_effect_decl_one_op() {
        let p = parse_ok_program(
            "effect Io { log(msg: i64) -> i64; } fn main() -> i64 { 0 }",
        );
        assert_eq!(p.effects[0].ops.len(), 1);
        let op = &p.effects[0].ops[0];
        assert_eq!(op.name, "log");
        assert_eq!(op.params.len(), 1);
        assert_eq!(op.params[0].name, "msg");
        assert_eq!(op.return_type.as_ref().unwrap().kind.to_string(), "i64");
    }

    #[test]
    fn parse_effect_decl_multi_ops() {
        let p = parse_ok_program(
            "effect Net { connect(host: i64); send(x: i64) -> i64; }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.effects[0].name, "Net");
        assert_eq!(p.effects[0].ops.len(), 2);
        assert_eq!(p.effects[0].ops[0].name, "connect");
        // First op has no return type.
        assert!(p.effects[0].ops[0].return_type.is_none());
        assert_eq!(p.effects[0].ops[1].name, "send");
        assert!(p.effects[0].ops[1].return_type.is_some());
    }

    #[test]
    fn parse_effect_decl_trailing_semi_allowed() {
        let p = parse_ok_program(
            "effect Io { log(msg: i64) -> i64; }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.effects[0].ops.len(), 1);
    }

    #[test]
    fn parse_fn_with_one_effect_annotation() {
        let p = parse_ok_program("fn f() -> i64 ! { Io } { 0 }");
        assert_eq!(p.fns[0].effect_row.len(), 1);
        assert_eq!(p.fns[0].effect_row[0].kind, "Io");
    }

    #[test]
    fn parse_fn_with_multi_effect_annotation() {
        let p = parse_ok_program("fn f() -> i64 ! { Io, Net, Panic } { 0 }");
        let labels: Vec<&str> = p.fns[0]
            .effect_row
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(labels, vec!["Io", "Net", "Panic"]);
    }

    #[test]
    fn parse_fn_no_effect_annotation_means_empty_row() {
        let p = parse_ok_program("fn f() -> i64 { 0 }");
        assert!(p.fns[0].effect_row.is_empty());
    }

    #[test]
    fn parse_fn_effect_annotation_with_trailing_comma() {
        let p = parse_ok_program("fn f() -> i64 ! { Io, } { 0 }");
        assert_eq!(p.fns[0].effect_row.len(), 1);
    }

    #[test]
    fn parse_error_empty_effect_annotation() {
        // `! { }` is rejected — write no annotation to mean "no
        // effects" per ADR 0019 D1.
        let err = parse("fn f() -> i64 ! { } { 0 }").unwrap_err();
        assert!(
            matches!(&err, ParseError::EmptyEffectAnnotation { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_secret_type_in_param() {
        let p = parse_ok_program("fn f(x: secret i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "secret i64");
    }

    #[test]
    fn parse_secret_type_in_return() {
        let p = parse_ok_program("fn f() -> secret i64 { 0 }");
        assert_eq!(p.fns[0].return_type.kind.to_string(), "secret i64");
    }

    #[test]
    fn parse_secret_type_composes_with_ref() {
        // `& secret T` and `secret &T` both parse. The type
        // checker rejects both at C3.0 (SecretNotYet); at C3.1
        // `& secret T` is allowed and `secret &T` is rejected per
        // ADR 0019 D7 (`SecretInRefDeref`).
        let p = parse_ok_program("fn f(x: & secret i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "&secret i64");
        let p = parse_ok_program("fn f(x: secret &i64) -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty.kind.to_string(), "secret &i64");
    }

    #[test]
    fn parse_error_double_secret() {
        // `secret secret T` rejected at the second `secret` per ADR
        // 0019 D5. Mirrors C1.5's `??T` NestedNullable rule.
        let err = parse("fn f(x: secret secret i64) -> i64 { 0 }").unwrap_err();
        assert!(matches!(&err, ParseError::DoubleSecret { .. }), "got {err:?}");
    }

    #[test]
    fn parse_declassify_call() {
        // `declassify(x)` is an atom; the parsed expression appears
        // inside the body's tail position.
        let p = parse_ok_program("fn f() -> i64 { declassify(x) }");
        assert_eq!(p.fns[0].body.tail.kind.to_string(), "(declassify x)");
    }

    #[test]
    fn parse_declassify_call_inner_expression() {
        // Parens around arbitrary expression — the inner is a Cmp
        // with logical-not chain.
        let p = parse_ok_program("fn f() -> i64 { declassify(a + b) }");
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(declassify (+ a b))"
        );
    }

    #[test]
    fn parse_error_declassify_without_parens() {
        // `declassify x` — mandatory parens (D6).
        let err = parse("fn f() -> i64 { declassify x }").unwrap_err();
        assert!(
            matches!(
                &err,
                ParseError::UnexpectedToken { expected, .. }
                    if *expected == "`(` after `declassify`"
            ),
            "got {err:?}"
        );
    }

    // ----- C3.4 / ADR 0020 D4 + D5: handle + perform parser -----

    #[test]
    fn parse_perform_no_args() {
        let p = parse_ok_program("fn f() -> i64 { perform Io.read() }");
        assert_eq!(p.fns[0].body.tail.kind.to_string(), "(perform Io.read)");
    }

    #[test]
    fn parse_perform_one_arg() {
        let p = parse_ok_program("fn f() -> i64 { perform Io.log(42) }");
        assert_eq!(p.fns[0].body.tail.kind.to_string(), "(perform Io.log 42)");
    }

    #[test]
    fn parse_perform_multi_args() {
        let p = parse_ok_program("fn f() -> i64 { perform Io.write(1, 2) }");
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(perform Io.write 1 2)"
        );
    }

    #[test]
    fn parse_handle_minimal() {
        // The body and one arm; no return arm.
        let p = parse_ok_program(
            "fn f() -> i64 { handle 42 with { Io.read(k) => 7 } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle 42 (arm Io.read (k) 7))"
        );
    }

    #[test]
    fn parse_handle_with_return_arm() {
        let p = parse_ok_program(
            "fn f() -> i64 { handle 42 with { Io.log(msg, k) => msg, return v => v } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle 42 (arm Io.log (msg k) msg) (return v v))"
        );
    }

    #[test]
    fn parse_handle_trailing_comma() {
        let p = parse_ok_program(
            "fn f() -> i64 { handle 0 with { Io.read(k) => 1, } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle 0 (arm Io.read (k) 1))"
        );
    }

    #[test]
    fn parse_handle_multi_param_arm() {
        let p = parse_ok_program(
            "fn f() -> i64 { handle 0 with { Io.write(a, b, k) => a } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle 0 (arm Io.write (a b k) a))"
        );
    }

    #[test]
    fn parse_handle_only_return_arm() {
        // `handle 42 with { return v => v * 2 }` — pure-compute handler.
        let p = parse_ok_program(
            "fn f() -> i64 { handle 42 with { return v => v * 2 } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle 42 (return v (* v 2)))"
        );
    }

    #[test]
    fn parse_error_handle_without_with() {
        let err = parse("fn f() -> i64 { handle 42 { } }").unwrap_err();
        assert!(
            matches!(
                &err,
                ParseError::UnexpectedToken { expected, .. }
                    if *expected == "`with` after handle body"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_perform_missing_dot() {
        let err = parse("fn f() -> i64 { perform Io read() }").unwrap_err();
        assert!(
            matches!(
                &err,
                ParseError::UnexpectedToken { expected, .. }
                    if *expected == "`.` after effect name in `perform`"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_handler_arm_without_fat_arrow() {
        let err = parse(
            "fn f() -> i64 { handle 0 with { Io.read(k) 7 } }",
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                ParseError::UnexpectedToken { expected, .. }
                    if *expected == "`=>` after handler arm parameters"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_error_duplicate_return_arm() {
        let err = parse(
            "fn f() -> i64 { handle 0 with { return a => a, return b => b } }",
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                ParseError::UnexpectedToken { expected, .. }
                    if *expected
                        == "at most one `return v => body` arm per handler"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_handle_body_is_call() {
        // The body can be any expression; here it's a function call.
        let p = parse_ok_program(
            "fn f() -> i64 { handle do_work() with { Io.read(k) => 1 } }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(handle (do_work) (arm Io.read (k) 1))"
        );
    }

    // ===== Phase D.1 / ADR 0032: enum declarations + match expressions =====

    #[test]
    fn parse_enum_decl_unit_and_payload_variants() {
        let p = parse_ok_program(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.enums.len(), 1);
        let e = &p.enums[0];
        assert_eq!(e.name, "Shape");
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].name, "Unit");
        assert!(e.variants[0].payloads.is_empty());
        assert_eq!(e.variants[1].name, "Circle");
        assert_eq!(e.variants[1].payloads.len(), 1);
        assert_eq!(e.variants[2].name, "Rect");
        assert_eq!(e.variants[2].payloads.len(), 2);
    }

    #[test]
    fn parse_enum_trailing_commas_allowed() {
        // Trailing comma after the last variant AND inside a payload list.
        let p = parse_ok_program("enum E { A, B(i64,), }\nfn main() -> i64 { 0 }");
        assert_eq!(p.enums[0].variants.len(), 2);
        assert_eq!(p.enums[0].variants[1].payloads.len(), 1);
    }

    #[test]
    fn parse_match_variants_and_wildcard() {
        let p = parse_ok_program(
            "fn area(s: Shape) -> i64 { match s { Shape::Circle(r) => r, Shape::Rect(w, h) => w, _ => 0 } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(match s (Shape::Circle r r) (Shape::Rect w h w) (_ 0))"
        );
    }

    #[test]
    fn parse_match_unit_variants() {
        let p = parse_ok_program(
            "fn f(c: Color) -> i64 { match c { Color::Red => 1, Color::Blue => 2 } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(
            p.fns[0].body.tail.kind.to_string(),
            "(match c (Color::Red 1) (Color::Blue 2))"
        );
    }

    #[test]
    fn parse_match_missing_fat_arrow_errors() {
        // `Shape::Circle(r) 0` — the `=>` is missing.
        assert!(parse(
            "fn f(s: Shape) -> i64 { match s { Shape::Circle(r) 0 } }\nfn main() -> i64 { 0 }"
        )
        .is_err());
    }
}
