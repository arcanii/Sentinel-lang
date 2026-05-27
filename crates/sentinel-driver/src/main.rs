//! snc: the Sentinel compiler driver.
//!
//! C0.1: `snc parse <file>` lexes, parses, pretty-prints the AST.
//! C0.2: `snc build <file> [-o <output>]` additionally lowers to
//! LLVM IR, emits an object file, and links it into an executable
//! via the system `cc`. The compiled program's exit code is the
//! evaluated program's tail expression truncated to i32.
//! C0.3: parse and build now operate on full [`sentinel_syntax::Program`]s
//! (`stmt* tail_expr`) rather than single expressions.
//! C0.4: `print(x)`, `if`/`else`, and block expressions land; the
//! linker invocation now pulls in `libsentinel_runtime.a` from the
//! same directory as the snc binary so `sentinel_print` resolves.
//! C1.0b: front-end stages run through Salsa-tracked queries per
//! ADR 0011 D1. The driver instantiates a [`SentinelDatabase`],
//! sets a [`SourceFile`] input, calls `parse_query`, and collects
//! diagnostics via the accumulator. Codegen remains a direct
//! function call; its salsa retrofit is deferred to C1.2+ per
//! ADR 0011 D1's C1.0c amendment.
//! C1.1.2: pipeline now chains parse_query → resolve_query →
//! compile_to_object. Name resolution lives in sentinel-resolve
//! per ADR 0011 D4; codegen consumes a `ResolvedProgram` (no more
//! string-keyed lookups in codegen).
//! C1.2.4: type-check pass joins the pipeline:
//!   parse_query → resolve_query → check_query → codegen.
//! Codegen now consumes a `TypedProgram` per ADR 0011 D5 + ADR
//! 0012 D1-D4; diagnostics from every front-end stage transitively
//! accumulate on check_query.
//! C2.1: borrow-check pass joins the pipeline per ADR 0017 D6:
//!   parse_query → resolve_query → check_query → borrow_check_query
//!   → codegen.
//! Codegen is gated on borrow_check_query succeeding; diagnostics
//! from every front-end stage including borrow check transitively
//! accumulate on borrow_check_query.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use miette::{LabeledSpan, MietteDiagnostic, NamedSource, Report, Severity as MietteSeverity, SourceSpan};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_borrow_check::borrow_check_query;
use sentinel_types::check_query;

/// The concrete Salsa database for the snc driver. Per ADR 0011 D1
/// the cross-crate database trait [`SentinelDb`] lives in
/// `sentinel-base`; the concrete struct lives here because the
/// driver is the assembly point where the pipeline is instantiated.
#[salsa::db]
#[derive(Default, Clone)]
struct SentinelDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for SentinelDatabase {
    fn salsa_event(&self, _event: &dyn Fn() -> salsa::Event) {}
}

#[salsa::db]
impl SentinelDb for SentinelDatabase {}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, cmd, path] if cmd == "parse" => run_parse(path),
        [_, cmd, path] if cmd == "build" => run_build(path, None),
        [_, cmd, path, flag, output] if cmd == "build" && flag == "-o" => {
            run_build(path, Some(output))
        }
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
    eprintln!("snc — Sentinel compiler (C1.0b)");
    eprintln!();
    eprintln!("usage:");
    eprintln!("    snc parse <file>                 lex, parse, and pretty-print the program");
    eprintln!("    snc build <file> [-o <output>]   compile and link to an executable");
    eprintln!("    snc help                         show this message");
    eprintln!();
    eprintln!("programs are one or more `fn` definitions; `main` is the entry point.");
}

fn run_parse(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    // `parse <file>` only needs the parse stage — pretty-print the
    // AST and stop. resolve_query exists for `build`.
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    match program_opt {
        Some(program) => {
            println!("{program}");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

fn run_build(path: &str, output: Option<&str>) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    // Pipeline: parse_query → resolve_query → check_query →
    // borrow_check_query → codegen (per ADR 0017 D6). The
    // borrow-check query chains on check_query, so
    // accumulated::<Diagnostic> on it picks up parse, resolve,
    // type-check, AND borrow-check diagnostics in one collection.
    // The TypedProgram itself comes from check_query (no clone
    // through borrow_check_query); both queries are salsa-cached.
    let drop_plan_opt = borrow_check_query(&db, file);
    let diags = borrow_check_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let drop_plan = match drop_plan_opt {
        Some(plan) => plan,
        None => return ExitCode::from(1),
    };
    let typed = match check_query(&db, file) {
        Some(t) => t,
        // Should be unreachable: if check_query failed, drop_plan
        // would already be None. Kept defensive.
        None => return ExitCode::from(1),
    };

    let exe_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(""),
    };
    let object_path = exe_path.with_extension("o");

    // C2.4 / ADR 0017 D8: pass DropPlan to codegen so it emits
    // drop calls at scope-exit for un-moved heap-backed bindings.
    if let Err(err) = sentinel_codegen::compile_to_object(typed, drop_plan, &object_path) {
        let report = Report::new(err).with_source_code(NamedSource::new(path, src));
        eprintln!("{report:?}");
        return ExitCode::from(1);
    }

    match link(&object_path, &exe_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("snc: link failed: {msg}");
            ExitCode::from(1)
        }
    }
}

/// Render each accumulated [`Diagnostic`] through miette's fancy
/// reporter. The conversion drops the per-variant help text and
/// label text that the lex/parse error enums carried as
/// `#[diagnostic(help(...))]` / `#[label(...)]` attributes; the
/// stage/code/message/span quartet is what survives. Refining this
/// (e.g., a code → help-text table, or carrying labels through the
/// accumulator) is a follow-up — for C1.0b the goal is to prove
/// the salsa retrofit works end-to-end.
fn render_diagnostics(diags: &[Diagnostic], path: &str, src: &str) {
    for d in diags {
        let severity = match d.severity {
            Severity::Error => MietteSeverity::Error,
            Severity::Warning => MietteSeverity::Warning,
        };
        let source_span: SourceSpan = (d.span.start, d.span.end - d.span.start).into();
        let mietted = MietteDiagnostic::new(d.message.clone())
            .with_code(d.code)
            .with_severity(severity)
            .with_label(LabeledSpan::at(source_span, ""));
        let report = Report::new(mietted)
            .with_source_code(NamedSource::new(path, src.to_string()));
        eprintln!("{report:?}");
    }
}

fn read_source(path: &str) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|e| {
        eprintln!("snc: cannot read `{path}`: {e}");
        ExitCode::from(1)
    })
}

fn link(object: &Path, exe: &Path) -> Result<(), String> {
    let runtime = find_runtime()?;
    let status = Command::new("cc")
        .arg(object)
        .arg(&runtime)
        .arg("-o")
        .arg(exe)
        .status()
        .map_err(|e| format!("failed to invoke cc: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cc exited with {status}"))
    }
}

/// Locate `libsentinel_runtime.a` adjacent to the snc binary. Cargo
/// puts the snc bin and the runtime staticlib in the same target
/// directory (`target/<profile>/`), so a single lookup off
/// `current_exe().parent()` covers both `cargo run --bin snc` and
/// `CARGO_BIN_EXE_snc`-driven integration tests.
fn find_runtime() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe(): {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent directory".to_string())?;
    let runtime = dir.join("libsentinel_runtime.a");
    if !runtime.exists() {
        return Err(format!(
            "libsentinel_runtime.a not found at {} — \
             run `cargo build -p sentinel-runtime` to produce it",
            runtime.display()
        ));
    }
    Ok(runtime)
}
