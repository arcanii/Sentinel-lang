//! snc: the Sentinel compiler driver.
//!
//! C0.1: `snc parse <file>` lexes, parses, pretty-prints the AST.
//! C0.2: `snc build <file> [-o <output>]` additionally lowers to
//! LLVM IR, emits an object file, and links it into an executable
//! via the system `cc`. The compiled program's exit code is the
//! evaluated program's tail expression truncated to i32.
//! C0.3: parse and build now operate on full [`Program`]s
//! (`stmt* tail_expr`) rather than single expressions.
//! C0.4: `print(x)`, `if`/`else`, and block expressions land; the
//! linker invocation now pulls in `libsentinel_runtime.a` from the
//! same directory as the snc binary so `sentinel_print` resolves.
//!
//! Pipeline stages compose via direct function calls per ADR 0009
//! D1a. Linker invocation lives here, not in sentinel-codegen,
//! because it is platform glue rather than a compiler concern.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use miette::{NamedSource, Report};
use sentinel_syntax::parse;

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
    eprintln!("snc — Sentinel compiler (C0.5)");
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
    match parse(&src) {
        Ok(program) => {
            println!("{program}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            let report = Report::new(err).with_source_code(NamedSource::new(path, src));
            eprintln!("{report:?}");
            ExitCode::from(1)
        }
    }
}

fn run_build(path: &str, output: Option<&str>) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let program = match parse(&src) {
        Ok(p) => p,
        Err(err) => {
            let report = Report::new(err).with_source_code(NamedSource::new(path, src));
            eprintln!("{report:?}");
            return ExitCode::from(1);
        }
    };

    let exe_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(""),
    };
    let object_path = exe_path.with_extension("o");

    if let Err(err) = sentinel_codegen::compile_to_object(&program, &object_path) {
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
