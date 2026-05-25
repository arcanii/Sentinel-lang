//! UI tests for sentinel-syntax.
//!
//! Each `.sentinel` file under workspace-root `tests/ui/` is run through
//! the lexer; diagnostics are formatted via miette's graphical handler
//! (themed for snapshot stability) and compared against an insta
//! snapshot. ADR 0009 D5.

use std::path::PathBuf;

use miette::{GraphicalReportHandler, GraphicalTheme, NamedSource, Report};
use sentinel_syntax::{lex, parse, LexError, ParseError};

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

fn handler() -> GraphicalReportHandler {
    GraphicalReportHandler::new_themed(GraphicalTheme::none()).with_width(80)
}

fn format_lex_errors(src: &str, source_name: &str, errors: &[LexError]) -> String {
    let mut out = String::new();
    let h = handler();
    for err in errors {
        let report = Report::new(err.clone())
            .with_source_code(NamedSource::new(source_name, src.to_string()));
        h.render_report(&mut out, report.as_ref())
            .expect("render diagnostic");
    }
    out
}

fn format_parse_error(src: &str, source_name: &str, err: &ParseError) -> String {
    let mut out = String::new();
    let report = Report::new(err.clone())
        .with_source_code(NamedSource::new(source_name, src.to_string()));
    handler()
        .render_report(&mut out, report.as_ref())
        .expect("render diagnostic");
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

#[test]
fn ui_parse_unbalanced_paren() {
    let name = "parse_unbalanced_paren.sentinel";
    let src = read_fixture(name);
    let err = parse(&src).expect_err("expected parse error");
    let formatted = format_parse_error(&src, name, &err);
    insta::assert_snapshot!(formatted);
}
