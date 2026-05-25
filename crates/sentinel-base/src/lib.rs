//! sentinel-base
//!
//! Shared Salsa database trait and inputs for the Sentinel compiler
//! pipeline. Phase C1.0 (ADR 0011 D1) introduces Salsa so subsequent
//! type-system passes can plug into a query-graph that supports
//! incremental recompilation from day one — cashing in the ADR 0009
//! D1a pure-function discipline that C0 held.
//!
//! Layout:
//!   - [`SourceFile`]: the `#[salsa::input]` for a source string
//!   - [`SentinelDb`]: the database trait every pipeline query
//!     takes as its first argument
//!   - [`Diagnostic`]: a `#[salsa::accumulator]` for warnings and
//!     errors that propagate through queries without breaking
//!     invalidation
//!
//! Per ADR 0011 D1, each pipeline crate (sentinel-syntax,
//! sentinel-codegen, future sentinel-resolve, sentinel-types)
//! defines its `#[salsa::tracked]` queries against this trait. The
//! actual database struct (`SentinelDatabase`) lives in
//! sentinel-driver where the pipeline is instantiated.

use std::sync::Arc;

/// A source file input. Salsa hands each query an immutable
/// snapshot of the source text; mutating the source means setting
/// a new revision via `SourceFile::set_text`.
#[salsa::input]
pub struct SourceFile {
    #[returns(ref)]
    pub path: String,
    #[returns(ref)]
    pub text: String,
}

/// A diagnostic accumulator. Pipeline stages call
/// `Diagnostic { … }.accumulate(db)` to attach diagnostics to the
/// current query; the driver collects them via
/// `Query::accumulated::<Diagnostic>(db, key)` at the end.
#[salsa::accumulator]
pub struct Diagnostic {
    /// Stage that produced the diagnostic: "lex", "parse",
    /// "resolve", "types", "codegen". Used to scope errors per
    /// pipeline phase when the driver displays them.
    pub stage: &'static str,

    /// Severity. Errors block further processing of the same
    /// salsa query; warnings allow it to continue.
    pub severity: Severity,

    /// Stage-specific code (e.g., `"sentinel::lex::invalid_char"`).
    pub code: &'static str,

    /// Human-readable message.
    pub message: String,

    /// Byte range in the originating SourceFile's text.
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

/// The database trait every pipeline query takes as its first
/// argument. Implementations of this trait live in the driver and
/// in test harnesses; crates between (syntax/codegen/etc.) only
/// ever see `&dyn SentinelDb`.
#[salsa::db]
pub trait SentinelDb: salsa::Database {}

/// Owned `Arc<String>` wrapper: convenience type for query results
/// that need to be cheaply clonable across Salsa's snapshotting.
/// Some salsa-tracked return types want immutable shared
/// ownership; this is the standard helper for that.
pub type ArcStr = Arc<String>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny tracked query to verify the salsa machinery is wired
    /// up. Real queries land in sentinel-syntax and downstream.
    #[salsa::tracked]
    fn double_text_len(db: &dyn SentinelDb, file: SourceFile) -> usize {
        file.text(db).len() * 2
    }

    /// Test-only DB. The real `SentinelDatabase` lives in
    /// sentinel-driver where the pipeline is instantiated.
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

    #[test]
    fn salsa_query_runs() {
        let db = TestDb::default();
        let file = SourceFile::new(&db, "test".to_string(), "hello".to_string());
        assert_eq!(double_text_len(&db, file), 10);
    }

    #[test]
    fn salsa_query_caches() {
        // Two calls with the same input should hit the cache the
        // second time. We don't have a great way to assert
        // "didn't recompute" from outside salsa, but at minimum
        // the result should be stable.
        let db = TestDb::default();
        let file = SourceFile::new(&db, "x".to_string(), "abc".to_string());
        let r1 = double_text_len(&db, file);
        let r2 = double_text_len(&db, file);
        assert_eq!(r1, r2);
        assert_eq!(r1, 6);
    }

    #[test]
    fn source_file_accessors() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "/path/to/file.sentinel".to_string(),
            "fn main() { 42 }".to_string(),
        );
        assert_eq!(file.path(&db), "/path/to/file.sentinel");
        assert_eq!(file.text(&db), "fn main() { 42 }");
    }
}
