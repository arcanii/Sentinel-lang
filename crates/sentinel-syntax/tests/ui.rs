//! UI tests for sentinel-syntax.
//!
//! Each `.sentinel` file under workspace-root `tests/ui/` is run through
//! the lexer; diagnostics are formatted via miette's graphical handler
//! (themed for snapshot stability) and compared against an insta
//! snapshot. ADR 0009 D5.

use std::path::PathBuf;

use miette::{GraphicalReportHandler, GraphicalTheme, NamedSource, Report};
use sentinel_syntax::{lex, LexError};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .parent()
        .expect("crates/ has a parent")
        .to_path_buf()
}

fn read_fixture(name: &str) -> String {
    let path = workspace_root().join("tests/ui").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn format_lex_errors(src: &str, source_name: &str, errors: &[LexError]) -> String {
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::none()).with_width(80);
    let mut out = String::new();
    for err in errors {
        let report = Report::new(err.clone())
            .with_source_code(NamedSource::new(source_name, src.to_string()));
        handler
            .render_report(&mut out, report.as_ref())
            .expect("render diagnostic");
    }
    out
}

#[test]
fn ui_lex_invalid_char() {
    let name = "lex_invalid_char.sentinel";
    let src = read_fixture(name);
    let (_, errs) = lex(&src);
    assert_eq!(errs.len(), 1, "expected exactly one lex error");
    let formatted = format_lex_errors(&src, name, &errs);
    insta::assert_snapshot!(formatted);
}
