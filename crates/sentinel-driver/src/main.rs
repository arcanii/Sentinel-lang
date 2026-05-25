//! snc: the Sentinel compiler driver.
//!
//! C0.1 ships the `parse` subcommand: read a source file, lex it,
//! parse it as a single expression, and pretty-print the AST in
//! s-expression form. Pipeline stages compose via direct function
//! calls per ADR 0009 D1a.

use std::process::ExitCode;

use miette::{NamedSource, Report};
use sentinel_syntax::parse;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, cmd, path] if cmd == "parse" => run_parse(path),
        [_] => {
            print_usage();
            ExitCode::from(2)
        }
        [_, cmd] if matches!(cmd.as_str(), "-h" | "--help" | "help") => {
            print_usage();
            ExitCode::SUCCESS
        }
        _ => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    eprintln!("snc — Sentinel compiler (C0.1)");
    eprintln!();
    eprintln!("usage:");
    eprintln!("    snc parse <file>     lex, parse, and pretty-print the AST");
    eprintln!("    snc help             show this message");
}

fn run_parse(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("snc: cannot read `{path}`: {e}");
            return ExitCode::from(1);
        }
    };
    match parse(&src) {
        Ok(expr) => {
            println!("{}", expr.kind);
            ExitCode::SUCCESS
        }
        Err(err) => {
            let report = Report::new(err).with_source_code(NamedSource::new(path, src));
            eprintln!("{report:?}");
            ExitCode::from(1)
        }
    }
}
