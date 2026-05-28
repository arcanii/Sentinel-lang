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
//! C1.5 adds (per ADR 0014 D8): `null` keyword (the nullable
//! literal) and `?` token (postfix type-position marker for `?T`).
//! `?` is reserved for type-position only at C1.5 — the
//! expression-position uses (`?.`, `??`, `?` propagation, postfix
//! `x!` unwrap) are deferred per D11 to avoid token-role conflicts.
//!
//! C1.6 adds (per ADR 0015 D8): `[` and `]` brackets for array type
//! `[T]`, array literal `[e1, e2, ...]`, and postfix indexing
//! `a[i]`. No new keywords — the `len` builtin per D4 is registered
//! by the resolve pass alongside `print` / `unwrap_or` / `is_some`.
//!
//! C4.0 adds (per ADR 0021 D11): twelve new keywords for the
//! Phase C4 surface — `class`, `trait`, `impl`, `init`,
//! `delegate`, `scope`, `spawn`, `await`, `Self`, `self`,
//! `as`, `for`. Reserved at the lexer layer; the parser
//! activates them per sub-phase per ADR 0021 D14 (C4.1 for
//! class/init/Self/self; C4.2 for trait/impl/as/for; C4.3
//! for delegate; C4.4 for scope/spawn/await).
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
    #[token("null")]
    Null,
    /// C2 / ADR 0017 D2: `let mut x` and `mut` param prefix.
    #[token("mut")]
    Mut,
    /// C3 / ADR 0019 D4 + D10: top-level effect declarations.
    #[token("effect")]
    Effect,
    /// C3 / ADR 0019 D5 + D10: `secret T` type prefix.
    #[token("secret")]
    Secret,
    /// C3 / ADR 0019 D6 + D10: `declassify(e)` special form.
    #[token("declassify")]
    Declassify,
    /// C3 / ADR 0019 D8 + D10: `handle e with { ... }` — reserved
    /// at C3.0; rejected at type-check until D8 closure
    /// (handler runtime in C3.last / ADR 0020).
    #[token("handle")]
    Handle,
    /// C3 / ADR 0019 D8 + D10: pairs with `handle`.
    #[token("with")]
    With,
    /// C3 / ADR 0019 D8 + D10: `perform E.op(args)` — reserved
    /// at C3.0; rejected at type-check until D8 closure.
    #[token("perform")]
    Perform,
    /// C3 / ADR 0020 D4 (C3.4): the `return v => body` arm inside
    /// `handle ... with { ... }`. Reserved as a contextual keyword
    /// only inside handler-arm position — outside handlers the
    /// identifier `return` is currently unused by Sentinel (no
    /// early-return statement at C3).
    #[token("return")]
    Return,

    /// C4 / ADR 0021 D1 + D11: `class Name { ... }` declares a
    /// struct + methods + impl + delegation group.
    #[token("class")]
    Class,
    /// C4 / ADR 0021 D4 + D11: `trait Name { ... }` declares a
    /// trait with method signatures.
    #[token("trait")]
    Trait,
    /// C4 / ADR 0021 D5 + D11: `impl [Name as] Trait for Type
    /// { ... }` declares a default or named implementation.
    #[token("impl")]
    Impl,
    /// C4 / ADR 0021 D3 + D11: `init(args) { ... }` constructor
    /// with definite-assignment check.
    #[token("init")]
    Init,
    /// C4 / ADR 0021 D6 + D11: `delegate inner: T to Trait;`
    /// inside a class body — auto-forwarder codegen at C4.3.
    #[token("delegate")]
    Delegate,
    /// C4 / ADR 0021 D8 + D11: `scope concurrent { ... }` task-
    /// cancellation boundary.
    #[token("scope")]
    Scope,
    /// C4 / ADR 0021 D8 + D11: `spawn expr` creates a Task.
    #[token("spawn")]
    Spawn,
    /// C4 / ADR 0021 D8 + D11: `task.await` blocks until the
    /// task produces a value.
    #[token("await")]
    Await,
    /// C4 / ADR 0021 D2 + D11: `Self` refers to the implementing
    /// type inside method / trait bodies. Distinct from `self`
    /// (the receiver binding) by case.
    #[token("Self")]
    SelfTy,
    /// C4 / ADR 0021 D2 + D11: `self` is the receiver binding
    /// inside method bodies (`fn foo(self: &Self, ...)`).
    #[token("self")]
    SelfVal,
    /// C4 / ADR 0021 D5 + D11: the `as` connector inside
    /// `impl Name as Trait for Type` named-implementation
    /// syntax. Reserved keyword (no other use at C4 minimum).
    #[token("as")]
    As,
    /// C4 / ADR 0021 D5 + D11: `for` connector inside `impl
    /// [Name as] Trait for Type { ... }`. Reserved at C4.0;
    /// the parser activates it at C4.2. (Future for-loop usage
    /// reclaims this same keyword token.)
    #[token("for")]
    For,

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
    // `&&` must lex before `&` per logos longest-match.
    #[token("&&")]
    AmpAmp,
    /// C2 / ADR 0017 D1 + D3 + D10: `&T` / `&mut T` ref types,
    /// `&expr` / `&mut expr` borrow-take, address-of prefix unary.
    #[token("&")]
    Amp,
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
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token(";")]
    Semi,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
    #[token("?")]
    Question,
    #[token("->")]
    Arrow,
    /// C3 / ADR 0020 D4 (C3.4): the `=>` separator inside handler
    /// arms (`Effect.op(...) => body` and `return v => body`).
    /// Distinct from `->` (return-type arrow) and `==`
    /// (equality); longest-match keeps them unambiguous.
    #[token("=>")]
    FatArrow,

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
            kinds("( ) { } [ ] , ; : . ? ->"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Comma,
                TokenKind::Semi,
                TokenKind::Colon,
                TokenKind::Dot,
                TokenKind::Question,
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
            kinds("let fn if else true false struct null"),
            vec![
                TokenKind::Let,
                TokenKind::Fn,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Struct,
                TokenKind::Null,
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

    // ----- C1.5: null keyword + `?` token -----

    #[test]
    fn lex_null_keyword() {
        assert_eq!(kinds("null"), vec![TokenKind::Null]);
    }

    #[test]
    fn lex_null_keyword_distinct_from_ident_prefix() {
        // `nullify`, `null_value` must NOT lex as Null + ident-fragment.
        assert_eq!(
            kinds("nullify null_value nullable"),
            vec![TokenKind::Ident, TokenKind::Ident, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_question_token() {
        assert_eq!(kinds("?"), vec![TokenKind::Question]);
    }

    #[test]
    fn lex_nullable_type_shape() {
        // The shape the parser will see for `?i64`.
        assert_eq!(
            kinds("?i64"),
            vec![TokenKind::Question, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_nullable_annotation_in_let() {
        // `let x: ?i64 = null`
        assert_eq!(
            kinds("let x: ?i64 = null"),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Question,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::Null,
            ]
        );
    }

    #[test]
    fn lex_question_token_with_whitespace() {
        // Whitespace allowed between `?` and base type per ADR 0014 D1.
        assert_eq!(
            kinds("? i64"),
            vec![TokenKind::Question, TokenKind::Ident]
        );
    }

    // ----- C1.6: `[` and `]` brackets -----

    #[test]
    fn lex_bracket_tokens() {
        assert_eq!(
            kinds("[ ]"),
            vec![TokenKind::LBracket, TokenKind::RBracket]
        );
    }

    #[test]
    fn lex_array_type_shape() {
        // The shape the parser will see for `[i64]`.
        assert_eq!(
            kinds("[i64]"),
            vec![TokenKind::LBracket, TokenKind::Ident, TokenKind::RBracket]
        );
    }

    #[test]
    fn lex_nullable_array_type_shape() {
        // `?[i64]` — nullable of array per ADR 0015 D6.
        assert_eq!(
            kinds("?[i64]"),
            vec![
                TokenKind::Question,
                TokenKind::LBracket,
                TokenKind::Ident,
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn lex_array_literal_shape() {
        // `[1, 2, 3]`
        assert_eq!(
            kinds("[1, 2, 3]"),
            vec![
                TokenKind::LBracket,
                TokenKind::IntLit,
                TokenKind::Comma,
                TokenKind::IntLit,
                TokenKind::Comma,
                TokenKind::IntLit,
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn lex_array_index_shape() {
        // `a[i]`
        assert_eq!(
            kinds("a[i]"),
            vec![
                TokenKind::Ident,
                TokenKind::LBracket,
                TokenKind::Ident,
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn lex_empty_array_shape() {
        // `[]` — empty array literal per ADR 0015 D5.
        assert_eq!(
            kinds("[]"),
            vec![TokenKind::LBracket, TokenKind::RBracket]
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

    // ----- C2.0 / ADR 0017: `&` token + `mut` keyword -----

    #[test]
    fn lex_amp_token() {
        assert_eq!(kinds("&"), vec![TokenKind::Amp]);
    }

    #[test]
    fn lex_amp_then_ident() {
        // `&x` lexes as two tokens — parser disambiguates as
        // borrow-take prefix unary.
        assert_eq!(kinds("&x"), vec![TokenKind::Amp, TokenKind::Ident]);
    }

    #[test]
    fn lex_amp_amp_still_lexes_as_logical_and() {
        // Longest-match: `&&` stays a single AmpAmp token, not two `&`s.
        assert_eq!(kinds("a && b"), vec![
            TokenKind::Ident,
            TokenKind::AmpAmp,
            TokenKind::Ident,
        ]);
    }

    #[test]
    fn lex_amp_amp_amp() {
        // Triple `&` — longest-match takes `&&` first, then `&`.
        assert_eq!(kinds("&&&"), vec![TokenKind::AmpAmp, TokenKind::Amp]);
    }

    #[test]
    fn lex_mut_keyword() {
        assert_eq!(kinds("mut"), vec![TokenKind::Mut]);
    }

    #[test]
    fn lex_mut_in_let_binding() {
        // `let mut x = 5;` — the canonical D2 binding form.
        assert_eq!(
            kinds("let mut x = 5;"),
            vec![
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::IntLit,
                TokenKind::Semi,
            ]
        );
    }

    #[test]
    fn lex_mut_as_ident_prefix_still_lexes_as_keyword() {
        // `mutable` should lex as Mut + Ident? No — logos regex match
        // `[A-Za-z_][A-Za-z0-9_]*` is greedy, so `mutable` lexes as a
        // single Ident. Reserves `mut` as a keyword only when standalone.
        assert_eq!(kinds("mutable"), vec![TokenKind::Ident]);
    }

    #[test]
    fn lex_amp_mut_ref_type() {
        // `&mut T` — the canonical D1 ref-type form.
        assert_eq!(
            kinds("&mut i64"),
            vec![TokenKind::Amp, TokenKind::Mut, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_amp_x_then_amp_mut_y() {
        // `&x; &mut y;` — both forms in sequence.
        assert_eq!(
            kinds("&x; &mut y;"),
            vec![
                TokenKind::Amp,
                TokenKind::Ident,
                TokenKind::Semi,
                TokenKind::Amp,
                TokenKind::Mut,
                TokenKind::Ident,
                TokenKind::Semi,
            ]
        );
    }

    #[test]
    fn lex_star_dereference() {
        // `*x` — the `*` token from C0 multiplication is reused per
        // ADR 0017 D10 / D4. The parser disambiguates dereference
        // vs multiplication positionally.
        assert_eq!(kinds("*x"), vec![TokenKind::Star, TokenKind::Ident]);
    }

    // ---- C3 / ADR 0019 D10: effect-system lexer additions ----

    #[test]
    fn lex_effect_keyword() {
        assert_eq!(kinds("effect"), vec![TokenKind::Effect]);
    }

    #[test]
    fn lex_secret_keyword() {
        assert_eq!(kinds("secret"), vec![TokenKind::Secret]);
    }

    #[test]
    fn lex_declassify_keyword() {
        assert_eq!(kinds("declassify"), vec![TokenKind::Declassify]);
    }

    #[test]
    fn lex_handle_keyword() {
        assert_eq!(kinds("handle"), vec![TokenKind::Handle]);
    }

    #[test]
    fn lex_with_keyword() {
        assert_eq!(kinds("with"), vec![TokenKind::With]);
    }

    #[test]
    fn lex_perform_keyword() {
        assert_eq!(kinds("perform"), vec![TokenKind::Perform]);
    }

    #[test]
    fn lex_c3_keywords_ident_prefix_regression() {
        // logos's `[A-Za-z_][A-Za-z0-9_]*` regex is greedy, so an
        // identifier that *starts* with a keyword lexes as a single
        // Ident — not as keyword + suffix. Matches the C1.4 struct /
        // C2 mut precedent.
        assert_eq!(kinds("effective"), vec![TokenKind::Ident]);
        assert_eq!(kinds("secrets"), vec![TokenKind::Ident]);
        assert_eq!(kinds("declassifier"), vec![TokenKind::Ident]);
        assert_eq!(kinds("handler"), vec![TokenKind::Ident]);
        assert_eq!(kinds("within"), vec![TokenKind::Ident]);
        assert_eq!(kinds("performance"), vec![TokenKind::Ident]);
    }

    #[test]
    fn lex_effect_decl_skeleton() {
        // `effect Io { read() -> i64; }` — the D4 surface.
        assert_eq!(
            kinds("effect Io { read() -> i64; }"),
            vec![
                TokenKind::Effect,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Arrow,
                TokenKind::Ident,
                TokenKind::Semi,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_postfix_effect_annotation() {
        // `fn f() -> i64 ! { Io }` — the D1 surface. `!` reuses the
        // existing C1.3 logical-not token; parser disambiguates
        // positionally.
        assert_eq!(
            kinds("fn f() -> i64 ! { Io }"),
            vec![
                TokenKind::Fn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Arrow,
                TokenKind::Ident,
                TokenKind::Bang,
                TokenKind::LBrace,
                TokenKind::Ident,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_secret_type_prefix() {
        // `secret i64` — the D5 surface.
        assert_eq!(
            kinds("secret i64"),
            vec![TokenKind::Secret, TokenKind::Ident]
        );
    }

    #[test]
    fn lex_declassify_call() {
        // `declassify(x)` — the D6 surface. Mandatory parens.
        assert_eq!(
            kinds("declassify(x)"),
            vec![
                TokenKind::Declassify,
                TokenKind::LParen,
                TokenKind::Ident,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lex_handle_with_skeleton() {
        // `handle e with { ... }` — the D8 placeholder surface.
        assert_eq!(
            kinds("handle e with { }"),
            vec![
                TokenKind::Handle,
                TokenKind::Ident,
                TokenKind::With,
                TokenKind::LBrace,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_perform_call_skeleton() {
        // `perform Io.read()` — the D8 placeholder surface.
        assert_eq!(
            kinds("perform Io.read()"),
            vec![
                TokenKind::Perform,
                TokenKind::Ident,
                TokenKind::Dot,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
            ]
        );
    }

    // ----- C3.4 / ADR 0020 D4: handler-arm syntax tokens -----

    #[test]
    fn lex_fat_arrow() {
        assert_eq!(kinds("=>"), vec![TokenKind::FatArrow]);
    }

    #[test]
    fn lex_return_keyword() {
        assert_eq!(kinds("return"), vec![TokenKind::Return]);
    }

    #[test]
    fn lex_return_keyword_ident_prefix_regression() {
        // `returning` / `returns` / `return_value` lex as Ident,
        // not as keyword + suffix.
        assert_eq!(kinds("returning"), vec![TokenKind::Ident]);
        assert_eq!(kinds("returns"), vec![TokenKind::Ident]);
        assert_eq!(kinds("return_value"), vec![TokenKind::Ident]);
    }

    #[test]
    fn lex_handler_arm_skeleton() {
        // `Io.read(k) => k(42)` — the D4 surface.
        assert_eq!(
            kinds("Io.read(k) => k(42)"),
            vec![
                TokenKind::Ident,
                TokenKind::Dot,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::Ident,
                TokenKind::RParen,
                TokenKind::FatArrow,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::IntLit,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lex_return_arm_skeleton() {
        // `return v => v` — the optional return-arm surface.
        assert_eq!(
            kinds("return v => v"),
            vec![
                TokenKind::Return,
                TokenKind::Ident,
                TokenKind::FatArrow,
                TokenKind::Ident,
            ]
        );
    }

    #[test]
    fn lex_fat_arrow_vs_eq_eq_vs_eq() {
        // `==` is longest among {=, ==, =>}; `=>` is its own token.
        // The trio must lex unambiguously side-by-side.
        assert_eq!(
            kinds("= == =>"),
            vec![TokenKind::Eq, TokenKind::EqEq, TokenKind::FatArrow]
        );
    }

    // ========================================================================
    // C4.0 / ADR 0021 D11: lexer keywords for Phase C4 — classes, traits,
    // delegation, structured concurrency. Reserved at the lexer layer; the
    // parser activates them per sub-phase per ADR 0021 D14.
    // ========================================================================

    #[test]
    fn lex_class_keyword() {
        assert_eq!(kinds("class"), vec![TokenKind::Class]);
    }

    #[test]
    fn lex_trait_keyword() {
        assert_eq!(kinds("trait"), vec![TokenKind::Trait]);
    }

    #[test]
    fn lex_impl_keyword() {
        assert_eq!(kinds("impl"), vec![TokenKind::Impl]);
    }

    #[test]
    fn lex_init_keyword() {
        assert_eq!(kinds("init"), vec![TokenKind::Init]);
    }

    #[test]
    fn lex_delegate_keyword() {
        assert_eq!(kinds("delegate"), vec![TokenKind::Delegate]);
    }

    #[test]
    fn lex_scope_keyword() {
        assert_eq!(kinds("scope"), vec![TokenKind::Scope]);
    }

    #[test]
    fn lex_spawn_keyword() {
        assert_eq!(kinds("spawn"), vec![TokenKind::Spawn]);
    }

    #[test]
    fn lex_await_keyword() {
        assert_eq!(kinds("await"), vec![TokenKind::Await]);
    }

    #[test]
    fn lex_self_ty_keyword() {
        // `Self` (capital S) refers to the implementing type.
        assert_eq!(kinds("Self"), vec![TokenKind::SelfTy]);
    }

    #[test]
    fn lex_self_val_keyword() {
        // `self` (lower s) is the receiver binding.
        assert_eq!(kinds("self"), vec![TokenKind::SelfVal]);
    }

    #[test]
    fn lex_as_keyword() {
        assert_eq!(kinds("as"), vec![TokenKind::As]);
    }

    #[test]
    fn lex_for_keyword() {
        assert_eq!(kinds("for"), vec![TokenKind::For]);
    }

    #[test]
    fn lex_c4_keywords_ident_prefix_regression() {
        // Identifier that *starts* with a C4 keyword lexes as a
        // single Ident — not as keyword + suffix. Matches the
        // C1.4 struct / C2 mut / C3 effect/secret/perform
        // precedent.
        assert_eq!(kinds("classroom"), vec![TokenKind::Ident]);
        assert_eq!(kinds("traitor"), vec![TokenKind::Ident]);
        assert_eq!(kinds("implementation"), vec![TokenKind::Ident]);
        assert_eq!(kinds("initial"), vec![TokenKind::Ident]);
        assert_eq!(kinds("delegated"), vec![TokenKind::Ident]);
        assert_eq!(kinds("scoped"), vec![TokenKind::Ident]);
        assert_eq!(kinds("spawned"), vec![TokenKind::Ident]);
        assert_eq!(kinds("awaiting"), vec![TokenKind::Ident]);
        assert_eq!(kinds("Selfish"), vec![TokenKind::Ident]);
        assert_eq!(kinds("selfish"), vec![TokenKind::Ident]);
        assert_eq!(kinds("ask"), vec![TokenKind::Ident]);
        assert_eq!(kinds("foreach"), vec![TokenKind::Ident]);
    }

    #[test]
    fn lex_class_decl_skeleton() {
        // `class Point { let x: i64; init(x: i64) { } }` — the
        // C4.1 placeholder surface.
        assert_eq!(
            kinds("class Point { let x: i64; init(x: i64) { } }"),
            vec![
                TokenKind::Class,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Ident,
                TokenKind::Semi,
                TokenKind::Init,
                TokenKind::LParen,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Ident,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_trait_decl_skeleton() {
        // `trait Writer { fn write(self: &mut Self) -> i64; }`
        assert_eq!(
            kinds("trait Writer { fn write(self: &mut Self) -> i64; }"),
            vec![
                TokenKind::Trait,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Fn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::SelfVal,
                TokenKind::Colon,
                TokenKind::Amp,
                TokenKind::Mut,
                TokenKind::SelfTy,
                TokenKind::RParen,
                TokenKind::Arrow,
                TokenKind::Ident,
                TokenKind::Semi,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_named_impl_skeleton() {
        // `impl Logged as Writer for File { }` — C4.2 surface.
        assert_eq!(
            kinds("impl Logged as Writer for File { }"),
            vec![
                TokenKind::Impl,
                TokenKind::Ident,
                TokenKind::As,
                TokenKind::Ident,
                TokenKind::For,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_delegate_stmt_skeleton() {
        // `delegate inner: FileWriter to Writer;` — C4.3 surface.
        // Note: `to` is currently lexed as Ident; the parser
        // recognises it positionally rather than as a keyword
        // (per the smallest-surface principle).
        assert_eq!(
            kinds("delegate inner: FileWriter to Writer;"),
            vec![
                TokenKind::Delegate,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Semi,
            ]
        );
    }

    #[test]
    fn lex_scope_spawn_await_skeleton() {
        // `scope concurrent { let t = spawn f(); t.await }` — C4.4.
        // `concurrent` is currently an Ident (parser-positional).
        assert_eq!(
            kinds("scope concurrent { let t = spawn f(); t.await }"),
            vec![
                TokenKind::Scope,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::Spawn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Semi,
                TokenKind::Ident,
                TokenKind::Dot,
                TokenKind::Await,
                TokenKind::RBrace,
            ]
        );
    }
}
