//! Salsa-tracked queries for the lex and parse pipeline stages.
//!
//! Per ADR 0011 D1 (Phase C1.0), each pipeline stage gets a
//! `#[salsa::tracked]` wrapper so subsequent type-system work has
//! incremental recompilation from day one. The C0 pure-function
//! discipline (ADR 0009 D1a) made this retrofit mechanical: the
//! pure entry points (`lex`, `parse`) remain available as the
//! library API; the queries here are the salsa-aware layer on top.
//!
//! Design notes:
//!   - `lex_query` returns the token vector and accumulates
//!     `Diagnostic`s for any [`LexError`]s.
//!   - `parse_query` returns `Option<Program>` (None on parse
//!     failure) and accumulates `Diagnostic`s for any [`ParseError`].
//!   - Errors are NOT carried in the tracked return type. The
//!     C1.0a session paused because `Vec<LexError>` requires its
//!     inner type to be Hash, but [`miette::SourceSpan`] doesn't
//!     derive Hash. Routing errors through the accumulator side-
//!     steps the Hash bound entirely.
//!   - `parse_query` does NOT depend on `lex_query` — it calls
//!     [`parse`] directly, which re-lexes internally. This matches
//!     [`parse`]'s existing fail-fast-on-lex-error semantics
//!     exactly. A future C1.0c may share lex output across queries
//!     when codegen / types passes need both tokens and AST.

use salsa::Accumulator;
use sentinel_ast::{Program, Spanned};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};

use crate::{lex, parse, LexError, ParseError, TokenKind};

/// Salsa-tracked lex query. Returns the token vector and emits a
/// `Diagnostic` via the accumulator for each `LexError`.
#[salsa::tracked(return_ref)]
pub fn lex_query(db: &dyn SentinelDb, file: SourceFile) -> Vec<Spanned<TokenKind>> {
    let src = file.text(db);
    let (tokens, errors) = lex(&src);
    for err in &errors {
        lex_error_to_diagnostic(err).accumulate(db);
    }
    tokens
}

/// Salsa-tracked parse query. Returns `Some(Program)` on success or
/// `None` on parse failure; in either case any diagnostics
/// (including pass-through lex errors) are emitted via the
/// accumulator. Use `parse_query::accumulated::<Diagnostic>(db,
/// file)` to collect them.
#[salsa::tracked(return_ref)]
pub fn parse_query(db: &dyn SentinelDb, file: SourceFile) -> Option<Program> {
    let src = file.text(db);
    match parse(&src) {
        Ok(program) => Some(program),
        Err(err) => {
            parse_error_to_diagnostic(&err).accumulate(db);
            None
        }
    }
}

/// Convert a [`LexError`] into a [`Diagnostic`] for the accumulator.
/// Keeps the stage/code labels aligned with the miette-derived
/// `#[diagnostic(code(...))]` codes so downstream renderers can
/// route by code if needed.
fn lex_error_to_diagnostic(err: &LexError) -> Diagnostic {
    match err {
        LexError::InvalidChar { ch, span } => Diagnostic {
            stage: "lex",
            severity: Severity::Error,
            code: "sentinel::lex::invalid_char",
            message: format!("invalid character `{ch}` in source"),
            span: span.offset()..(span.offset() + span.len()),
        },
    }
}

/// Convert a [`ParseError`] into a [`Diagnostic`]. Lex errors
/// surfaced through `ParseError::Lex` get the lex stage/code so the
/// diagnostic is unambiguous about where it came from.
fn parse_error_to_diagnostic(err: &ParseError) -> Diagnostic {
    match err {
        ParseError::Lex(lex_err) => lex_error_to_diagnostic(lex_err),
        ParseError::UnexpectedToken { got, expected, span } => Diagnostic {
            stage: "parse",
            severity: Severity::Error,
            code: "sentinel::parse::unexpected_token",
            message: format!("unexpected {got}, expected {expected}"),
            span: span.offset()..(span.offset() + span.len()),
        },
        ParseError::UnexpectedEof { expected, span } => Diagnostic {
            stage: "parse",
            severity: Severity::Error,
            code: "sentinel::parse::unexpected_eof",
            message: format!("unexpected end of input, expected {expected}"),
            span: span.offset()..(span.offset() + span.len()),
        },
        ParseError::UnmatchedParen { open_span } => Diagnostic {
            stage: "parse",
            severity: Severity::Error,
            code: "sentinel::parse::unmatched_paren",
            message: "unmatched opening parenthesis".to_string(),
            span: open_span.offset()..(open_span.offset() + open_span.len()),
        },
        ParseError::IntLitOverflow { text, span } => Diagnostic {
            stage: "parse",
            severity: Severity::Error,
            code: "sentinel::parse::int_lit_overflow",
            message: format!("integer literal `{text}` does not fit in i64"),
            span: span.offset()..(span.offset() + span.len()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only database, mirroring the one in `sentinel-base`'s
    /// test module. Lives here because integration testing the
    /// queries requires a concrete `SentinelDb` implementation.
    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {
        fn salsa_event(&self, _event: &dyn Fn() -> salsa::Event) {}
    }

    #[salsa::db]
    impl SentinelDb for TestDb {}

    fn make_file(db: &TestDb, src: &str) -> SourceFile {
        SourceFile::new(db, "test.sentinel".to_string(), src.to_string())
    }

    #[test]
    fn lex_query_returns_tokens_for_valid_source() {
        let db = TestDb::default();
        let file = make_file(&db, "let x = 1;");
        let tokens = lex_query(&db, file);
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::IntLit,
                TokenKind::Semi,
            ]
        );
        // No lex errors expected.
        let diags = lex_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn lex_query_accumulates_diagnostic_on_invalid_char() {
        let db = TestDb::default();
        let file = make_file(&db, "let x = @");
        let _tokens = lex_query(&db, file);
        let diags = lex_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "lex");
        assert_eq!(diags[0].code, "sentinel::lex::invalid_char");
        assert!(diags[0].message.contains('@'), "got {:?}", diags[0].message);
    }

    #[test]
    fn parse_query_returns_program_for_valid_source() {
        let db = TestDb::default();
        let file = make_file(&db, "fn main() { 42 }");
        let result = parse_query(&db, file);
        assert!(result.is_some(), "expected Some(Program), got {result:?}");
        let p = result.as_ref().unwrap();
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.fns[0].name, "main");
        let diags = parse_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn parse_query_emits_diagnostic_on_parse_error() {
        let db = TestDb::default();
        // Unmatched paren in fn body.
        let file = make_file(&db, "fn main() { (1 + 2 }");
        let result = parse_query(&db, file);
        assert!(result.is_none());
        let diags = parse_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "parse");
        assert_eq!(diags[0].code, "sentinel::parse::unmatched_paren");
    }

    #[test]
    fn parse_query_propagates_lex_error_with_lex_stage() {
        let db = TestDb::default();
        let file = make_file(&db, "fn main() { @ }");
        let result = parse_query(&db, file);
        assert!(result.is_none());
        let diags = parse_query::accumulated::<Diagnostic>(&db, file);
        // parse_error_to_diagnostic forwards through ParseError::Lex
        // so the diagnostic carries the lex stage/code.
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "lex");
        assert_eq!(diags[0].code, "sentinel::lex::invalid_char");
    }

    #[test]
    fn parse_query_caches_across_reruns() {
        // HANDOVER §0.2 step 6: same input produces stable results
        // across query reruns. This doesn't directly assert "salsa
        // returned the cached value" but it pins the contract that
        // repeated calls don't observe drift.
        let db = TestDb::default();
        let file = make_file(&db, "fn main() { 1 + 2 }");
        let r1 = parse_query(&db, file).clone();
        let r2 = parse_query(&db, file).clone();
        assert_eq!(r1, r2);
        assert!(r1.is_some());
        // Diagnostics should also be stable (empty in this case).
        let d1 = parse_query::accumulated::<Diagnostic>(&db, file);
        let d2 = parse_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(d1.len(), d2.len());
        assert!(d1.is_empty());
    }

    #[test]
    fn lex_query_caches_across_reruns() {
        let db = TestDb::default();
        let file = make_file(&db, "fn main() { 42 }");
        let t1 = lex_query(&db, file).clone();
        let t2 = lex_query(&db, file).clone();
        assert_eq!(t1, t2);
    }
}
