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

mod ast_dump;
mod borrow_dump;
mod effects_dump;
mod llvm_dump;
mod mir_dump;
mod resolve_dump;
mod source_dump;
mod types_dump;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use miette::{LabeledSpan, MietteDiagnostic, NamedSource, Report, Severity as MietteSeverity, SourceSpan};
use sentinel_ast::ExternFnDecl;
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_borrow_check::borrow_check_query;
use sentinel_syntax::{Program, TokenKind};
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
        [_, cmd, path] if cmd == "lex" => run_lex(path),
        [_, cmd, path] if cmd == "ast" => run_ast(path),
        [_, cmd, path] if cmd == "resolve" => run_resolve(path),
        [_, cmd, path] if cmd == "types" => run_types(path),
        [_, cmd, path] if cmd == "effects" => run_effects(path),
        [_, cmd, path] if cmd == "borrow" => run_borrow(path),
        [_, cmd, path] if cmd == "mir" => run_mir(path),
        [_, cmd, path] if cmd == "ctverify" => run_ctverify(path),
        [_, cmd, path] if cmd == "llvm" => run_llvm(path),
        [_, cmd, path] if cmd == "merge" => run_merge(path),
        [_, cmd, path] if cmd == "parse" => run_parse(path),
        // `snc build <file> [-o <out>] [--link <lib>]...` — the plain executable
        // build. Excludes the `--separate` / `--lib` / `--shared` sub-modes
        // (their own arms below); `--link` threads extra native libraries into
        // the link (ADR 0057 pillar 4 / ADR 0060), e.g. `--link user32`.
        [_, cmd, rest @ ..]
            if cmd == "build"
                && !rest
                    .iter()
                    .any(|a| a == "--separate" || a == "--lib" || a == "--shared") =>
        {
            run_build_cli(rest)
        }
        // Phase D.6 / ADR 0037 (a): TRUE per-unit separate compilation
        // (opt-in until it reaches Path-A parity).
        [_, cmd, path, sep] if cmd == "build" && sep == "--separate" => {
            run_build_separate(path, None)
        }
        [_, cmd, path, sep, o, output]
            if cmd == "build" && sep == "--separate" && o == "-o" =>
        {
            run_build_separate(path, Some(output))
        }
        // ADR 0059: `snc build --lib <file> [-o <out.a>] [--emit-header <h.h>]`
        // — compile to a C-ABI static library (no `main`) so other languages
        // link + call its `export "C"` functions. `--shared` (ADR 0059 A9) emits
        // a SHARED library (`.dylib`) instead, for `dlopen` / `ctypes`. Flags
        // scan order-free.
        [_, cmd, rest @ ..]
            if cmd == "build" && rest.iter().any(|a| a == "--lib" || a == "--shared") =>
        {
            run_build_lib_cli(rest)
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
    eprintln!("    snc lex <file>                   lex and dump the token stream (self-host oracle)");
    eprintln!("    snc ast <file>                   parse and dump the canonical AST (self-host oracle)");
    eprintln!("    snc parse <file>                 lex, parse, and pretty-print the program");
    eprintln!("    snc build <file> [-o <output>] [--link <lib>]...");
    eprintln!("                                     compile + link to an executable (--link adds a");
    eprintln!("                                     native library to the link, e.g. user32)");
    eprintln!("    snc build --lib <file> [-o <lib.a>] [--emit-header <h.h>]");
    eprintln!("                                     compile to a C-ABI static library (ADR 0059)");
    eprintln!("    snc build --shared <file> [-o <lib.dylib>] [--emit-header <h.h>]");
    eprintln!("                                     compile to a C-ABI shared library (dlopen/ctypes)");
    eprintln!("    snc help                         show this message");
    eprintln!();
    eprintln!("programs are one or more `fn` definitions; `main` is the entry point.");
}

/// Phase D self-host port (1/N) / ADR 0038 D3+D4: the lexer differential
/// oracle. Lex `path` and print the **canonical token dump** the
/// Sentinel-written lexer (`selfhost/lexer.sentinel`) must reproduce
/// byte-for-byte. One line per token:
///
/// ```text
/// <KIND> <start> <end> [<lexeme>]
/// ```
///
/// where `<KIND>` is the `TokenKind` variant *name* (so the two lexers
/// need not agree on enum discriminant order), `<start>`/`<end>` are byte
/// offsets, and `<lexeme>` (the raw source slice) is present only for the
/// value-bearing variants (`Ident` / `IntLit` / `StringLit` / `CharLit`).
/// A trailing `EOF` line terminates the dump. A clean lex exits 0; any
/// `LexError` is printed to stderr and exits 1 (the dump still covers the
/// tokens that lexed). The format is a dev/validation surface, NOT
/// `abi-v1` — but it is pinned by a golden test so it can't drift under
/// the Sentinel lexer.
fn run_lex(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (tokens, errors) = sentinel_syntax::lex(&src);
    let mut out = String::new();
    for t in &tokens {
        let kind = format!("{:?}", t.kind);
        let (start, end) = (t.span.start, t.span.end);
        if is_value_bearing(t.kind) {
            // Safe slice: token spans fall on byte boundaries of valid
            // UTF-8 source, and the literal regexes forbid raw newlines,
            // so the lexeme never breaks the line-oriented format.
            out.push_str(&format!("{kind} {start} {end} {}\n", &src[t.span.clone()]));
        } else {
            out.push_str(&format!("{kind} {start} {end}\n"));
        }
    }
    out.push_str("EOF\n");
    print!("{out}");
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// Whether a token carries source text beyond its kind + span — i.e. two
/// tokens of this kind at the same span length can still differ. Only
/// these emit a `<lexeme>` field in the dump (ADR 0038 D4).
fn is_value_bearing(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::IntLit
            // ADR 0058: a float literal carries its text (`3.14` vs `2.5`),
            // so it is value-bearing like `IntLit`.
            | TokenKind::FloatLit
            | TokenKind::StringLit
            | TokenKind::CharLit
    )
}

/// Phase D self-host port (2/N) / ADR 0039 D2: the parser differential
/// oracle. Parse `path` and print the canonical S-expression AST dump
/// (`ast_dump`) the Sentinel-written parser must reproduce byte-for-byte.
/// Distinct from `snc parse` (the human pretty-print). A dev surface, not
/// `abi-v1`; pinned by a golden test.
fn run_ast(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    match program_opt {
        Some(program) => {
            print!("{}", ast_dump::dump(program));
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

/// `snc resolve <file>` — Phase D self-host port (3/N) / ADR 0040 D2: parse +
/// name-resolve, then emit the canonical resolved-AST dump (`resolve_dump`) the
/// Sentinel-written resolve stage (`selfhost/resolve.sentinel`) reproduces
/// byte-for-byte. The `snc ast` form extended with the resolved IDs (VarId /
/// FnId / StructId / …) + the parser's `qcall` / `class-init` disambiguated. A
/// dev surface, not `abi-v1`; pinned by a golden test. A parse OR resolve error
/// exits non-zero with no dump, so the differential test skips it (happy-path
/// resolution first, ADR 0040 D7).
fn run_resolve(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    match sentinel_resolve::resolve(program) {
        Ok(resolved) => {
            print!("{}", resolve_dump::dump(&resolved));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("snc: {e}");
            ExitCode::from(1)
        }
    }
}

/// Phase D self-host port (4/N) / ADR 0041 D2: `snc types <file>` — the
/// types differential oracle. Runs parse → resolve → `check` and prints
/// the canonical typed-program dump (`types_dump::dump`) the Sentinel
/// types stage reproduces. Mirrors `run_resolve` + one `check` call.
fn run_types(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    match sentinel_types::check(&resolved) {
        Ok(typed) => {
            print!("{}", types_dump::dump(&typed));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("snc: {e}");
            ExitCode::from(1)
        }
    }
}

/// Phase D self-host port (5/N) / ADR 0042 D2: `snc effects <file>` — the
/// effect-check differential oracle. Runs parse → resolve → `check` →
/// `effect_check` and prints the canonical effective-row dump
/// (`effects_dump::dump`) the Sentinel effect-check stage reproduces. Exits
/// nonzero on ANY error — parse/resolve/type (as `run_types`) OR an effect
/// error (annotation-mismatch / unhandled-effect) — so the corpus differential
/// skips rejected fixtures (the happy-path discipline, ADR 0042 D5/D7).
fn run_effects(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(typed) => typed,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let (checked, errors) = sentinel_effect_check::effect_check(&typed);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    print!("{}", effects_dump::dump(&typed, &checked));
    ExitCode::SUCCESS
}

/// Phase D self-host port (6/N) / ADR 0043 D2: `snc borrow <file>` — the
/// borrow-check differential oracle. Runs parse → resolve → `check` →
/// `borrow_check` and prints the canonical moved-sources dump
/// (`borrow_dump::dump`) the Sentinel borrow-check stage reproduces. Exits
/// nonzero on ANY error — parse/resolve/type (as `run_types`) OR a borrow error
/// (use-after-move, borrow conflict, returns-local-ref, …) — so the corpus
/// differential skips rejected fixtures (the happy-path discipline, ADR 0043 D5/D7).
fn run_borrow(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(typed) => typed,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let (plan, errors) = sentinel_borrow_check::borrow_check(&typed);
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    print!("{}", borrow_dump::dump(&typed, &plan));
    ExitCode::SUCCESS
}

/// Phase D self-host port (7/N) / ADR 0044 D2: `snc mir <file>` — the MIR
/// differential oracle. Runs parse → resolve → `check` → `lower_to_mir` and
/// prints the canonical lowered-form dump (`mir_dump::dump`) the Sentinel MIR
/// stage reproduces. Lowering is TOTAL (it never rejects), so this exits nonzero
/// only on an upstream parse/resolve/type error (as `run_types`) — the corpus
/// differential skips those. The const-time VERIFIER is NOT run here (the dump is
/// the lowered form regardless of any leak; the verifier is validated separately,
/// ADR 0044 D6).
fn run_mir(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(typed) => typed,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let mir = sentinel_mir::lower_to_mir(&typed);
    print!("{}", mir_dump::dump(&mir, &typed));
    ExitCode::SUCCESS
}

/// Phase D self-host port (7/N) / ADR 0044 D6: `snc ctverify <file>` — the
/// constant-time verifier oracle. Runs parse → resolve → `check` → `lower_to_mir`
/// → `verify_constant_time` and prints its leak set, one `(leak <SinkKind>)` per
/// line in iteration order (fn → block → inst → terminator). An empty result means
/// the program is constant-time at the MIR level. Exits nonzero only on an upstream
/// parse/resolve/type error (as `run_mir`); the verifier itself never rejects here
/// (it reports — the `snc build` gate is what rejects a leaking program).
fn run_ctverify(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(typed) => typed,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let mir = sentinel_mir::lower_to_mir(&typed);
    let leaks = sentinel_mir::verify_constant_time(&mir);
    let mut out = String::new();
    for leak in &leaks {
        out.push_str("(leak ");
        out.push_str(match leak.sink {
            sentinel_mir::SinkKind::Branch => "Branch",
            sentinel_mir::SinkKind::MemoryIndex => "MemoryIndex",
            sentinel_mir::SinkKind::MemoryAddress => "MemoryAddress",
            sentinel_mir::SinkKind::Division => "Division",
            sentinel_mir::SinkKind::ShiftAmount => "ShiftAmount",
        });
        out.push_str(")\n");
    }
    print!("{out}");
    ExitCode::SUCCESS
}

/// Phase D self-host port (8/N) / ADR 0045 D2+D3: `snc llvm <file>` — the
/// codegen differential oracle. Runs parse → resolve → `check` and emits the
/// canonical textual LLVM IR (`.ll`) that `selfhost/codegen.sentinel` will
/// reproduce byte-for-byte (and that `clang`/`llc` lowers to a runnable
/// object — the behavioural half). Exits nonzero on an upstream
/// parse/resolve/type error OR on a construct not yet ported (the corpus
/// differential skips those, as it skips upstream rejects — the happy-path
/// discipline; the supported subset grows per sub-slice 8a..8l).
fn run_llvm(path: &str) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // 8f-2: a multi-module entry (one that `use`s other files) lowers via the MERGED
    // program, mirroring `run_build`'s D.6 discovery + `merge_modules`. The single-file
    // `snc llvm` (below) is unchanged; this is what lets the oracle emit the full
    // self-hosting compiler's `.ll` (the multi-module stages: types/codegen/…).
    match discover_module_graph(Path::new(path)) {
        Ok(modules) if !modules.is_empty() => {
            let units: Vec<sentinel_resolve::ModuleUnit> = modules
                .iter()
                .map(|m| sentinel_resolve::ModuleUnit { path: m.path.clone(), program: &m.program })
                .collect();
            return match sentinel_resolve::merge_modules(&units) {
                Ok(merged) => run_llvm_merged(merged),
                Err(e) => {
                    eprintln!("snc: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Ok(_) => {} // single-file: fall through to the existing pipeline.
        Err(msg) => {
            eprintln!("snc: {msg}");
            return ExitCode::from(1);
        }
    }
    let db = SentinelDatabase::default();
    let file = SourceFile::new(&db, path.to_string(), src.clone());
    let program_opt = sentinel_syntax::parse_query(&db, file);
    let diags = sentinel_syntax::parse_query::accumulated::<Diagnostic>(&db, file);
    render_diagnostics(&diags, path, &src);
    let program = match program_opt {
        Some(p) => p,
        None => return ExitCode::from(1),
    };
    let resolved = match sentinel_resolve::resolve(program) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(typed) => typed,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    // 8d-drops: codegen consults the borrow-check DropPlan (moved-sources) to skip
    // freeing bindings whose ownership was moved out. The emitting subset is
    // borrow-clean (all "pass" fixtures), so any borrow errors are ignored here —
    // a real reject would have failed the full pipeline upstream.
    let (drop_plan, _borrow_errors) = sentinel_borrow_check::borrow_check(&typed);
    match llvm_dump::dump(&typed, &drop_plan) {
        Ok(ll) => {
            print!("{ll}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("snc: llvm: {why}");
            ExitCode::from(1)
        }
    }
}

/// 8f-2: emit the canonical `.ll` for a MERGED multi-module program (one `Program` from
/// `merge_modules`), mirroring `run_build_merged` but with `llvm_dump::dump` in place of
/// `compile_to_object` + link. Runs the same passes the single-file `run_llvm` does —
/// resolve → check → borrow-check → dump — over the merged program (it bypasses the
/// Salsa query layer, like `run_build_merged`, since the merged program is synthesized).
fn run_llvm_merged(merged: Program) -> ExitCode {
    let resolved = match sentinel_resolve::resolve(&merged) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let (drop_plan, _borrow_errors) = sentinel_borrow_check::borrow_check(&typed);
    match llvm_dump::dump(&typed, &drop_plan) {
        Ok(ll) => {
            print!("{ll}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("snc: llvm: {why}");
            ExitCode::from(1)
        }
    }
}

/// Phase D self-host port (8g) / ADR 0045 D8(ii): **merge-to-source** — the
/// lighter bootstrap-fixed-point path. Discover the module graph from
/// `path`, `merge_modules` it into one `Program` (the existing D.6
/// machinery), and print that merged program back as a single re-parseable
/// `.sentinel` file (`source_dump`). The single-file Sentinel codegen
/// (`scg`) then reads this one file — so `snc llvm` and `scg` lower the
/// *same* merged source and must emit byte-identical `.ll`. A single-file
/// program (no `use`) is parsed and re-printed directly (its own merge).
fn run_merge(path: &str) -> ExitCode {
    let merged: Program = match discover_module_graph(Path::new(path)) {
        Ok(modules) if !modules.is_empty() => {
            let units: Vec<sentinel_resolve::ModuleUnit> = modules
                .iter()
                .map(|m| sentinel_resolve::ModuleUnit { path: m.path.clone(), program: &m.program })
                .collect();
            match sentinel_resolve::merge_modules(&units) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("snc: {e}");
                    return ExitCode::from(1);
                }
            }
        }
        Ok(_) => {
            // Single-file: parse + re-print directly (no module graph).
            let src = match read_source(path) {
                Ok(s) => s,
                Err(code) => return code,
            };
            match sentinel_syntax::parse(&src) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("snc: parse error: {e:?}");
                    return ExitCode::from(1);
                }
            }
        }
        Err(msg) => {
            eprintln!("snc: {msg}");
            return ExitCode::from(1);
        }
    };
    match source_dump::dump(&merged) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("snc: merge: {why}");
            ExitCode::from(1)
        }
    }
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

/// Phase D.6 (1/N) / ADR 0037 D3: one discovered module — its module
/// path relative to the source root + its parsed AST.
struct DiscoveredModule {
    path: Vec<String>,
    program: Program,
    /// The module's raw source bytes — retained for the (3/N) incremental
    /// cache fingerprint (a unit's `.o` is a function of its source + its
    /// imports' sources + the graph effect set).
    source: String,
}

/// Phase D.6 (1/N) / ADR 0037 D3: discover the module graph reachable
/// from `entry` by following `use` edges. File-as-module: a `use
/// a::b::Item;` references module `a::b`, whose file is
/// `<root>/a/b.sentinel` — the **source root** is the entry file's
/// directory (ADR 0037 open point 3), and the **last** path segment is
/// the imported item, not part of the module path (point 4). Import
/// cycles are fine (a `visited` set; point 1). Returns the full graph
/// (entry module FIRST, then the reached modules) with each module's
/// parsed `Program` — or an **empty** vec for a single-file program (no
/// `use`), or a human-readable error when a `use`d module's file is
/// missing (ModuleNotFound).
///
/// Parsing the entry is lenient: a parse error there yields "no modules"
/// so the main Salsa pipeline renders the proper diagnostic rather than
/// this discovery pass duplicating it.
fn discover_module_graph(entry: &Path) -> Result<Vec<DiscoveredModule>, String> {
    let root = entry.parent().unwrap_or_else(|| Path::new("."));
    let entry_stem = entry
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let entry_src = std::fs::read_to_string(entry)
        .map_err(|e| format!("cannot read `{}`: {e}", entry.display()))?;
    let entry_prog = match sentinel_syntax::parse(&entry_src) {
        Ok(p) => p,
        // Defer to the main pipeline for a proper parse diagnostic.
        Err(_) => return Ok(Vec::new()),
    };
    if entry_prog.uses.is_empty() {
        return Ok(Vec::new());
    }

    let mut visited: BTreeSet<Vec<String>> = BTreeSet::new();
    visited.insert(vec![entry_stem.clone()]);
    // The entry module is first; reached modules are appended (BFS over
    // `out`, scanning each module's `use` edges as we go).
    let mut out: Vec<DiscoveredModule> =
        vec![DiscoveredModule { path: vec![entry_stem], program: entry_prog, source: entry_src }];
    let mut scan = 0;
    while scan < out.len() {
        let modules: Vec<Vec<String>> = out[scan]
            .program
            .uses
            .iter()
            .filter(|u| u.path.len() >= 2)
            .map(|u| u.path[..u.path.len() - 1].to_vec())
            .collect();
        for module in modules {
            if !visited.insert(module.clone()) {
                continue; // already seen — a cycle is fine.
            }
            let mut file = root.to_path_buf();
            for seg in &module {
                file.push(seg);
            }
            file.set_extension("sentinel");
            if !file.is_file() {
                return Err(format!(
                    "module `{}` not found (expected file `{}`)",
                    module.join("::"),
                    file.display()
                ));
            }
            let src = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read module `{}`: {e}", file.display()))?;
            let program = sentinel_syntax::parse(&src).map_err(|e| {
                format!("parse error in module `{}`: {e:?}", module.join("::"))
            })?;
            out.push(DiscoveredModule { path: module, program, source: src });
        }
        scan += 1;
    }
    Ok(out)
}

/// Parse `snc build <file> [-o <out>] [--link <lib>]...` — flags are order-free
/// and `--link` is repeatable (ADR 0057 pillar 4 / ADR 0060: thread extra
/// libraries into the link, e.g. `--link user32` for the Win32 GUI). Routes to
/// [`run_build`].
fn run_build_cli(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut output: Option<&str> = None;
    let mut link_libs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                match args.get(i) {
                    Some(o) => output = Some(o),
                    None => {
                        eprintln!("snc: `-o` requires an output path");
                        return ExitCode::from(2);
                    }
                }
            }
            "--link" => {
                i += 1;
                match args.get(i) {
                    Some(l) => link_libs.push(l.clone()),
                    None => {
                        eprintln!("snc: `--link` requires a library name");
                        return ExitCode::from(2);
                    }
                }
            }
            other => {
                if path.is_some() {
                    eprintln!("snc: unexpected argument `{other}`");
                    return ExitCode::from(2);
                }
                path = Some(other);
            }
        }
        i += 1;
    }
    match path {
        Some(p) => run_build(p, output, &link_libs),
        None => {
            eprintln!("snc: `build` requires a source file");
            ExitCode::from(2)
        }
    }
}

fn run_build(path: &str, output: Option<&str>, link_libs: &[String]) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };

    // Phase D.6 (1/N) / ADR 0037: discover the module graph by following
    // `use` edges. A single-file program (no `use`) compiles exactly as
    // before. Multi-module compilation — per-unit resolve + separate
    // codegen + link — lands in the next D.6 (1/N) increment; until then a
    // discovered multi-module graph is reported + gated honestly (and a
    // missing `use`d file is surfaced here as ModuleNotFound).
    match discover_module_graph(Path::new(path)) {
        Ok(modules) if !modules.is_empty() => {
            // Multi-file (Path A): merge the graph into one `Program` —
            // qualify each module's fns + rewrite cross-module references,
            // enforcing `use`/`pub` visibility — then compile it. (Per-unit
            // object emission + module-qualified mangling + multi-object
            // link is the follow-up; this first slice emits one object.)
            let units: Vec<sentinel_resolve::ModuleUnit> = modules
                .iter()
                .map(|m| sentinel_resolve::ModuleUnit { path: m.path.clone(), program: &m.program })
                .collect();
            return match sentinel_resolve::merge_modules(&units) {
                Ok(merged) => run_build_merged(merged, path, output, link_libs),
                Err(e) => {
                    eprintln!("snc: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Ok(_) => {} // single-file: fall through to the existing pipeline.
        Err(msg) => {
            eprintln!("snc: {msg}");
            return ExitCode::from(1);
        }
    }
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

    // C5.2 / ADR 0026 D5: the constant-time verification. Lower the typed
    // program to analysis MIR and reject any `secret` value that reaches a
    // conditional branch, a memory index/address, or a division divisor —
    // the machine-checkable form of ADR 0008's guarantee, gating codegen.
    // (Codegen still consumes the typed program via the HIR seam per the
    // D3 escape hatch; MIR is analysis-only, so this sits between
    // type-check and codegen.) `lower_to_mir` borrows `typed`, returning
    // an owned MirProgram, so `typed` stays usable for the HIR below.
    let mir = sentinel_mir::lower_to_mir(typed);
    let leaks = sentinel_mir::verify_constant_time(&mir);
    if !leaks.is_empty() {
        for leak in leaks {
            let report = Report::new(leak).with_source_code(NamedSource::new(path, src.clone()));
            eprintln!("{report:?}");
        }
        return ExitCode::from(1);
    }

    let exe_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(""),
    };
    let object_path = exe_path.with_extension("o");

    // C5.1a / ADR 0026 D1+D3: the pipeline middle. Lower the
    // type-checked program + its DropPlan (C2.4 / ADR 0017 D8: scope-
    // exit drops for un-moved heap-backed bindings) into the HIR, then
    // hand the HIR to codegen. At this increment `lower_to_hir` is the
    // identity bundle; desugaring fills in across later C5.1a steps.
    let hir = sentinel_hir::lower_to_hir(typed, drop_plan);
    if let Err(err) = sentinel_codegen::compile_to_object(&hir, &object_path) {
        let report = Report::new(err).with_source_code(NamedSource::new(path, src));
        eprintln!("{report:?}");
        return ExitCode::from(1);
    }

    // ADR 0057 A9: union the CLI `--link` libs with any declared via
    // `extern "C" link("…") { … }` in this program's extern blocks.
    let extern_libs = match sentinel_syntax::parse_query(&db, file).as_ref() {
        Some(p) => collect_link_libs(link_libs, &p.externs),
        None => link_libs.to_vec(),
    };
    match link(&object_path, &exe_path, &extern_libs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("snc: link failed: {msg}");
            ExitCode::from(1)
        }
    }
}

/// Phase D.6 (1/N) / ADR 0037 (Path A): compile a MERGED multi-module
/// program (one `Program` from `merge_modules`) through the pipeline via
/// direct calls — the merged program is synthesized, not a single
/// `SourceFile`, so it bypasses the Salsa query layer. First slice emits
/// ONE object (per-unit objects + module-qualified mangling + multi-object
/// link are the follow-up). Errors are reported by message; span-accurate
/// multi-source diagnostics + effect-check parity are follow-ups (the
/// merged program's spans point into per-module sources).
fn run_build_merged(merged: Program, path: &str, output: Option<&str>, link_libs: &[String]) -> ExitCode {
    // ADR 0057 A9: union the CLI `--link` libs with any declared via
    // `extern "C" link("…") { … }` across the merged modules' extern blocks.
    let extern_libs = collect_link_libs(link_libs, &merged.externs);
    let resolved = match sentinel_resolve::resolve(&merged) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    let typed = match sentinel_types::check(&resolved) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    // C3 / ADR 0019 D13: effect-check parity for the merged path. The
    // single-file pipeline runs this via `borrow_check_query` chaining on
    // `effect_check_query` (salsa); the merged path calls the pure passes
    // directly, so it must invoke `effect_check` itself — else a multi-file
    // `main` with an unhandled effect would slip through to codegen. Matches
    // the salsa order: effect-check sits between type-check and borrow-check.
    let (_effect_checked, effect_errors) = sentinel_effect_check::effect_check(&typed);
    if !effect_errors.is_empty() {
        for e in &effect_errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    let (drop_plan, borrow_errors) = sentinel_borrow_check::borrow_check(&typed);
    if !borrow_errors.is_empty() {
        for e in &borrow_errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    // C5.2 / ADR 0026: the constant-time verification gates codegen.
    let mir = sentinel_mir::lower_to_mir(&typed);
    let leaks = sentinel_mir::verify_constant_time(&mir);
    if !leaks.is_empty() {
        for leak in &leaks {
            eprintln!("snc: {leak}");
        }
        return ExitCode::from(1);
    }

    let exe_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(""),
    };
    let object_path = exe_path.with_extension("o");
    let hir = sentinel_hir::lower_to_hir(&typed, &drop_plan);
    if let Err(err) = sentinel_codegen::compile_to_object(&hir, &object_path) {
        eprintln!("snc: codegen failed: {err}");
        return ExitCode::from(1);
    }
    match link(&object_path, &exe_path, &extern_libs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("snc: link failed: {msg}");
            ExitCode::from(1)
        }
    }
}

/// ADR 0059: the library output kind — a static archive (`--lib`) or a shared
/// object (`--shared`). The whole front end + object emission is identical; only
/// the final link step (archive vs `-dynamiclib`) and the default extension
/// differ.
#[derive(Clone, Copy, PartialEq)]
enum LibMode {
    Static,
    Shared,
}

/// ADR 0059: scan `snc build --lib` / `--shared` flags (order-free): the input
/// path, an optional `-o <out>`, and an optional `--emit-header <h.h>`.
fn run_build_lib_cli(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut output: Option<&str> = None;
    let mut header: Option<&str> = None;
    let mut mode = LibMode::Static;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lib" => i += 1,
            "--shared" => {
                mode = LibMode::Shared;
                i += 1;
            }
            "-o" => {
                output = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--emit-header" => {
                header = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            p => {
                if path.is_none() {
                    path = Some(p);
                }
                i += 1;
            }
        }
    }
    match path {
        Some(p) => run_build_lib(p, output, header, mode),
        None => {
            eprintln!("snc: `build --lib` / `--shared` needs an input file");
            ExitCode::from(2)
        }
    }
}

/// ADR 0059: compile a Sentinel source file to a **C-ABI library** — no `main`,
/// the `export "C"` functions exposed under their bare un-mangled symbols,
/// bundled with the runtime into a `.a` (`LibMode::Static`) or a `.dylib`
/// (`LibMode::Shared`, A9 — for `dlopen` / `ctypes`) that C / Rust / Python / …
/// can link / load and call. Optionally writes a C header. The emitted object is
/// already PIC (`RelocMode::PIC`), so the same object serves both modes.
fn run_build_lib(
    path: &str,
    output: Option<&str>,
    header: Option<&str>,
    mode: LibMode,
) -> ExitCode {
    let src = match read_source(path) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let entry_program = match sentinel_syntax::parse(&src) {
        Ok(p) => p,
        Err(e) => {
            let report = Report::new(e).with_source_code(NamedSource::new(path, src.clone()));
            eprintln!("{report:?}");
            return ExitCode::from(1);
        }
    };
    // ADR 0059 A8: a library may span modules. Discover the `use` graph and
    // `merge_modules` it into one `Program` (the executable build's Path-A
    // discovery + merge, ADR 0037), then resolve WITHOUT `main`. A single-file
    // library (no `use`) keeps the entry program unchanged. `merge_modules`
    // keeps `export "C"` names bare (A3) and clears `uses`, so the merged
    // program resolves + codegen's the export wrappers exactly as single-file.
    let (program, is_merged) = match discover_module_graph(Path::new(path)) {
        Ok(modules) if !modules.is_empty() => {
            let units: Vec<sentinel_resolve::ModuleUnit> = modules
                .iter()
                .map(|m| sentinel_resolve::ModuleUnit { path: m.path.clone(), program: &m.program })
                .collect();
            match sentinel_resolve::merge_modules(&units) {
                Ok(merged) => (merged, true),
                Err(e) => {
                    eprintln!("snc: {e}");
                    return ExitCode::from(1);
                }
            }
        }
        Ok(_) => (entry_program, false), // single-file
        Err(msg) => {
            eprintln!("snc: {msg}");
            return ExitCode::from(1);
        }
    };
    // Resolve WITHOUT the `main` requirement — a library has no entry point.
    let resolved = match sentinel_resolve::resolve_module(&program, &[]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    if resolved.exports.is_empty() {
        eprintln!("snc: `build --lib` produced no `export \"C\"` functions (nothing to export)");
        return ExitCode::from(1);
    }
    let typed = match sentinel_types::check(&resolved) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("snc: {e}");
            return ExitCode::from(1);
        }
    };
    // The same passes the executable build runs (a library is still verified):
    // effect-check, borrow-check, and the constant-time gate.
    let (_ec, effect_errors) = sentinel_effect_check::effect_check(&typed);
    if !effect_errors.is_empty() {
        for e in &effect_errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    let (drop_plan, borrow_errors) = sentinel_borrow_check::borrow_check(&typed);
    if !borrow_errors.is_empty() {
        for e in &borrow_errors {
            eprintln!("snc: {e}");
        }
        return ExitCode::from(1);
    }
    let mir = sentinel_mir::lower_to_mir(&typed);
    let leaks = sentinel_mir::verify_constant_time(&mir);
    if !leaks.is_empty() {
        for leak in &leaks {
            eprintln!("snc: {leak}");
        }
        return ExitCode::from(1);
    }

    let default_ext = match mode {
        LibMode::Static => "a",
        LibMode::Shared => "dylib",
    };
    let lib_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(default_ext),
    };
    let object_path = lib_path.with_extension("o");
    let hir = sentinel_hir::lower_to_hir(&typed, &drop_plan);
    if let Err(err) = sentinel_codegen::compile_to_object(&hir, &object_path) {
        // A merged program's spans point into per-module sources, so attach the
        // single entry source only for a single-file build (else report by
        // message, like the executable merge path).
        if is_merged {
            eprintln!("snc: codegen failed: {err}");
        } else {
            let report = Report::new(err).with_source_code(NamedSource::new(path, src));
            eprintln!("{report:?}");
        }
        return ExitCode::from(1);
    }
    let link_result = match mode {
        LibMode::Static => archive_lib(&object_path, &lib_path),
        LibMode::Shared => link_shared(&object_path, &lib_path),
    };
    if let Err(msg) = link_result {
        eprintln!("snc: library link failed: {msg}");
        return ExitCode::from(1);
    }
    if let Some(h) = header {
        if let Err(msg) = emit_c_header(&typed, Path::new(h)) {
            eprintln!("snc: header generation failed: {msg}");
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// ADR 0059: bundle the emitted object together with the runtime staticlib into
/// ONE self-contained `.a` (so a consumer links a single archive). macOS uses
/// `libtool -static`; the Sentinel toolchain currently targets macOS (Linux's
/// `ar`-MRI path is a follow-up).
fn archive_lib(object: &Path, lib: &Path) -> Result<(), String> {
    let runtime = find_runtime()?;
    if cfg!(target_os = "windows") {
        // MSVC: `lib.exe` merges objects + static libs into one archive.
        let mut cmd = Command::new("lib.exe");
        cmd.arg("/NOLOGO")
            .arg(format!("/OUT:{}", lib.display()))
            .arg(object)
            .arg(&runtime);
        run_linker(cmd, "lib.exe")
    } else if cfg!(target_os = "macos") {
        let mut cmd = Command::new("libtool");
        cmd.arg("-static").arg("-o").arg(lib).arg(object).arg(&runtime);
        run_linker(cmd, "libtool")
    } else {
        // Linux: `ar` cannot merge an archive positionally, so drive `ar -M`
        // with an MRI script that pulls the runtime archive's members + the
        // emitted object into one static archive.
        archive_lib_ar_mri(object, &runtime, lib)
    }
}

/// Linux `--lib`: merge the runtime archive + the emitted object into a single
/// static archive via an `ar -M` MRI script (ADR 0060 Phase 2). The `cc`/exe
/// path is already portable; only archiving differs from macOS's `libtool`.
fn archive_lib_ar_mri(object: &Path, runtime: &Path, lib: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("ar")
        .arg("-M")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to invoke `ar`: {e}"))?;
    let script = format!(
        "create {}\naddlib {}\naddmod {}\nsave\nend\n",
        lib.display(),
        runtime.display(),
        object.display(),
    );
    child
        .stdin
        .take()
        .ok_or_else(|| "ar: failed to open stdin".to_string())?
        .write_all(script.as_bytes())
        .map_err(|e| format!("ar: failed to write MRI script: {e}"))?;
    let status = child.wait().map_err(|e| format!("ar: wait failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`ar` exited with {status}"))
    }
}

/// ADR 0059 A9: link the emitted (PIC) object + the runtime staticlib into a
/// SHARED library (`.dylib`) other languages `dlopen` / `ctypes`-load and call.
/// macOS uses `cc -dynamiclib`; the dylib's `install_name` is set to its own
/// absolute path so a consumer that links against it (or `dlopen`s it by path)
/// resolves it at runtime. The runtime's symbols are pulled in from the
/// staticlib so the `.dylib` is self-contained. (Linux's `cc -shared -fPIC` +
/// `soname` path is a follow-up, like the static `ar`-MRI path.)
fn link_shared(object: &Path, lib: &Path) -> Result<(), String> {
    let runtime = find_runtime()?;
    if cfg!(target_os = "windows") {
        // A Windows DLL requires exporting the `export "C"` symbols (a `.def`
        // file or dllexport) — deferred to an ADR 0060 Phase 2 follow-up.
        return Err("snc build --shared is not yet supported on Windows \
                    (ADR 0060 follow-up: DLL symbol export); use --lib for a \
                    static library"
            .to_string());
    }
    if cfg!(target_os = "macos") {
        // Prefer an absolute install_name so the dylib is locatable; fall back
        // to the given path if canonicalization fails (the file may not exist
        // yet).
        let install_name = std::fs::canonicalize(lib.parent().unwrap_or(Path::new(".")))
            .ok()
            .and_then(|dir| lib.file_name().map(|f| dir.join(f)))
            .unwrap_or_else(|| lib.to_path_buf());
        let mut cmd = Command::new("cc");
        cmd.arg("-dynamiclib")
            .arg("-install_name")
            .arg(&install_name)
            .arg("-o")
            .arg(lib)
            .arg(object)
            .arg(&runtime);
        run_linker(cmd, "cc -dynamiclib")
    } else {
        // Linux: a PIC shared object with a soname (ADR 0060 Phase 2).
        let soname = lib
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "libsentinel.so".to_string());
        let mut cmd = Command::new("cc");
        cmd.arg("-shared")
            .arg(format!("-Wl,-soname,{soname}"))
            .arg("-o")
            .arg(lib)
            .arg(object)
            .arg(&runtime);
        run_linker(cmd, "cc -shared")
    }
}

/// ADR 0059: render a Sentinel `Type` as its C type for the generated header.
/// Phase 1a is the value ABI: `i64` → `int64_t`, `f64` → `double`.
fn c_type_name(ty: sentinel_types::Type) -> Option<&'static str> {
    match ty {
        sentinel_types::Type::I64 => Some("int64_t"),
        sentinel_types::Type::F64 => Some("double"),
        // ADR 0057 Phase 1b: the opaque `ptr` is a C `void*`.
        sentinel_types::Type::Ptr => Some("void*"),
        _ => None,
    }
}

/// ADR 0059 Phase 1b: `true` iff `ty` is `&[u8]` / `&mut [u8]` — presented to
/// C as a `(const uint8_t* data, int64_t len)` pair in the generated header.
fn is_byte_slice_ref_header(ty: sentinel_types::Type, typed: &sentinel_types::TypedProgram) -> bool {
    if let sentinel_types::Type::Ref(id) = ty {
        if let Some(rd) = typed.refs.get(id.0 as usize) {
            return matches!(
                rd.inner,
                sentinel_types::Type::Array(sentinel_types::ArrayElem::U8)
            );
        }
    }
    false
}

/// ADR 0059: write a C header from the `export "C"` signatures — `#include
/// <stdint.h>`, include guards, and one prototype per export (the value ABI).
fn emit_c_header(typed: &sentinel_types::TypedProgram, header: &Path) -> Result<(), String> {
    let mut out = String::new();
    out.push_str("/* Generated by snc (ADR 0059). C-ABI exports of a Sentinel library. */\n");
    out.push_str("#ifndef SENTINEL_EXPORTS_H\n#define SENTINEL_EXPORTS_H\n\n");
    out.push_str("#include <stdint.h>\n\n");
    out.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");
    let mut any_bytes_return = false;
    for id in &typed.exports {
        let sig = typed
            .fn_signatures
            .iter()
            .find(|s| s.id == *id)
            .ok_or_else(|| "export FnId has no signature".to_string())?;
        // ADR 0059 Phase 1b (A7): an owned `[u8]` return is handed to C via two
        // trailing out-params `(uint8_t** out_data, int64_t* out_len)`, and the
        // function returns `void`; a value return (`i64`/`f64`) is rendered
        // directly. Collect the input-param pieces, then append the out-params.
        let ret_is_bytes = matches!(
            sig.return_type,
            sentinel_types::Type::Array(sentinel_types::ArrayElem::U8)
        );
        let ret = if ret_is_bytes {
            "void"
        } else {
            c_type_name(sig.return_type)
                .ok_or_else(|| format!("export `{}` has a non-value-ABI return type", sig.name))?
        };
        let mut pieces: Vec<&str> = Vec::new();
        for pty in sig.param_types.iter() {
            // ADR 0059 Phase 1b: a `&[u8]` param expands to the idiomatic C
            // `(const uint8_t* data, int64_t len)` pair.
            if is_byte_slice_ref_header(*pty, typed) {
                pieces.push("const uint8_t*");
                pieces.push("int64_t");
            } else {
                pieces.push(
                    c_type_name(*pty)
                        .ok_or_else(|| format!("export `{}` has a non-FFI parameter", sig.name))?,
                );
            }
        }
        if ret_is_bytes {
            any_bytes_return = true;
            pieces.push("uint8_t**"); // out_data: the heap buffer (C frees it)
            pieces.push("int64_t*"); // out_len:  the byte count
        }
        let params = if pieces.is_empty() {
            "void".to_string()
        } else {
            pieces.join(", ")
        };
        out.push_str(&format!("{ret} {}({params});\n", sig.name));
    }
    // ADR 0059 Phase 1b (A7): a C caller releases an owned `[u8]` return with
    // this runtime export (supplied by the bundled runtime staticlib).
    if any_bytes_return {
        out.push_str("\nvoid sentinel_free_bytes(uint8_t* data);\n");
    }
    out.push_str("\n#ifdef __cplusplus\n}\n#endif\n\n#endif /* SENTINEL_EXPORTS_H */\n");
    std::fs::write(header, out).map_err(|e| format!("write {}: {e}", header.display()))
}

/// Phase D.6 / ADR 0037 (a): an exported `pub fn`'s signature, extracted
/// from a module's AST for the per-unit exports table. Carries the
/// resolve-level (arity / type-param count) and the types-level (param /
/// return `Type`s) info. FIRST SLICE: SCALAR signatures only (i64 / i32 /
/// bool / u8) — cross-module struct / array / generic / effect signatures
/// are a later D.6 slice.
///
/// `Hash` feeds the (3/N) incremental cache: an importer's fingerprint hashes
/// the imported ITEMS it uses. Because this is SIGNATURE-only (no body), an
/// importer of a non-generic `pub fn` is NOT recompiled when only that fn's
/// body changes — it extern-calls the symbol, so the relink picks up the new
/// body. (An inlined item — struct / enum / generic body — carries its full
/// decl, so a change there does recompile importers.)
#[derive(Hash)]
struct ExportedFn {
    arity: usize,
    type_params_count: usize,
    /// Param/return type EXPRESSIONS (not resolved `Type`s) — the importer
    /// re-resolves them in its own type space, so a cross-module type in a
    /// signature (`sum(Point) -> i64`) maps to the importer's local id.
    param_type_exprs: Vec<sentinel_ast::TypeExpr>,
    return_type_expr: sentinel_ast::TypeExpr,
    /// ADR 0037 (2/N): the fn's declared effect-row NAMES (empty for a pure
    /// fn). Carried so a cross-UNIT effecting extern (a library `perform`s,
    /// the entry `handle`s) re-resolves its row to the importer's EffectIds
    /// in check_module + lowers under the Kont ABI; the build-wide op-id
    /// base map keeps the runtime op id consistent across the two units.
    effect_row_names: Vec<String>,
}

/// An exported item in the per-unit exports table. A non-generic `pub fn`
/// is an EXTERN the importer declares + links to; a `pub struct`/`pub enum`
/// is a layout the importer RE-MATERIALIZES (no link symbol, ADR 0037 D4);
/// a GENERIC `pub fn`'s BODY crosses the boundary (ADR 0037 D6) — the
/// importer INLINES it + monomorphizes LOCALLY, its instances qualified by
/// the importer's module path (each importer self-contains them; no link
/// symbol, no `linkonce_odr` dedup yet — a 2/N optimization).
///
/// `Hash` (over the contained AST decl, all of which derive it) feeds the
/// (3/N) incremental cache at item granularity — see [`unit_fingerprint`].
#[derive(Hash)]
enum ExportedItem {
    Fn(ExportedFn),
    Struct(sentinel_ast::StructDecl),
    Enum(sentinel_ast::EnumDecl),
    GenericFn(Box<sentinel_ast::FnDef>),
    /// A `pub trait` — a decl the importer RE-MATERIALIZES (no link symbol);
    /// the importer impls it for its OWN class + dispatches, emitting the
    /// impl methods under its own module-qualified symbols (ADR 0037 D6).
    Trait(Box<sentinel_ast::TraitDecl>),
    /// A `pub effect` decl — re-materialized in the importer. FIRST CUT: the
    /// `perform` + `handle` both live in the IMPORTER, so its `EffectId`
    /// (→ the runtime `op_id = (eid<<16)|op`) is consistent within that unit.
    /// Cross-UNIT perform/handle (a library performs, the entry handles)
    /// needs EffectId portability across units — a later (2/N) piece.
    Effect(Box<sentinel_ast::EffectDecl>),
}

/// Extract a module's `pub` items for the exports table: `pub fn`s (non-
/// generic, pure — params/returns may reference imported types, re-resolved
/// in the importer) as Fn exports, and `pub struct`s / `pub enum`s as the
/// corresponding type exports (the importer inlines the decl — types are
/// layout, ADR 0037 D4). Errors on an item outside the slice.
fn extract_exports(program: &Program) -> Result<Vec<(String, ExportedItem)>, String> {
    let mut out = Vec::new();
    for f in &program.fns {
        if f.visibility != sentinel_ast::Visibility::Public {
            continue;
        }
        // A GENERIC `pub fn` exports its BODY (ADR 0037 D6) — the importer
        // inlines it + monomorphizes locally. A non-generic one is an extern
        // carrying its param/return TYPE EXPRESSIONS (re-resolved in the
        // importer, so a cross-module type in a sig maps to the local id; an
        // un-resolvable type surfaces there as a normal UnknownType).
        if !f.type_params.is_empty() {
            out.push((f.name.clone(), ExportedItem::GenericFn(Box::new(f.clone()))));
            continue;
        }
        // ADR 0037 (2/N): a cross-UNIT effecting `pub fn` is allowed now —
        // it carries its effect-row NAMES (re-resolved in the importer); the
        // op-id base map keeps its `perform`'s runtime op id consistent with
        // the importer's `handle`. (The effect itself must be `pub` + `use`d
        // by the importer, like a type in a signature.)
        out.push((
            f.name.clone(),
            ExportedItem::Fn(ExportedFn {
                arity: f.params.len(),
                type_params_count: f.type_params.len(),
                param_type_exprs: f.params.iter().map(|p| p.ty.clone()).collect(),
                return_type_expr: f.return_type.clone(),
                effect_row_names: f.effect_row.iter().map(|e| e.kind.clone()).collect(),
            }),
        ));
    }
    // `pub struct`s — the importer re-materializes the decl (ADR 0037 D4:
    // types are layout, no link symbol). Non-generic for this slice; the
    // importer re-resolves the field types in its own type space, so a field
    // referencing an un-imported type surfaces as a normal UnknownType there.
    for s in &program.structs {
        if s.visibility != sentinel_ast::Visibility::Public {
            continue;
        }
        if !s.type_params.is_empty() {
            return Err(format!(
                "`pub struct {}`: cross-module generics are ADR 0037 (2/N), not yet in --separate",
                s.name
            ));
        }
        out.push((s.name.clone(), ExportedItem::Struct(s.clone())));
    }
    // `pub enum`s — same layout-only re-materialization (enums are
    // non-generic; variant payloads re-resolve in the importer).
    for e in &program.enums {
        if e.visibility != sentinel_ast::Visibility::Public {
            continue;
        }
        out.push((e.name.clone(), ExportedItem::Enum(e.clone())));
    }
    // `pub trait`s — a decl re-materialized in the importer, which impls it
    // for its own class (the impl + class are the importer's own; ADR D6).
    for t in &program.traits {
        if t.visibility != sentinel_ast::Visibility::Public {
            continue;
        }
        out.push((t.name.clone(), ExportedItem::Trait(Box::new(t.clone()))));
    }
    // `pub effect`s — a decl re-materialized in the importer (first cut: the
    // perform + handle live in the importer, so the EffectId is unit-local).
    for ef in &program.effects {
        if ef.visibility != sentinel_ast::Visibility::Public {
            continue;
        }
        out.push((ef.name.clone(), ExportedItem::Effect(Box::new(ef.clone()))));
    }
    Ok(out)
}

/// ADR 0060 Phase 2: native system libraries a Rust `staticlib` pulls in that a
/// foreign (non-cargo) link must resolve explicitly. On Unix `cc` links the
/// platform's C/runtime deps automatically, so this is empty there; on Windows
/// the MSVC linker needs them named — this is the set `rustc --print
/// native-static-libs` reports for `sentinel-runtime`.
#[allow(dead_code)]
const WINDOWS_NATIVE_LIBS: &[&str] = &[
    "legacy_stdio_definitions.lib",
    "kernel32.lib",
    "ntdll.lib",
    "userenv.lib",
    "ws2_32.lib",
    "dbghelp.lib",
];

/// Run a linker/archiver `cmd`, mapping spawn + nonzero-exit failures to a
/// `String`. `tool` names the program for diagnostics; a spawn failure on
/// Windows points the user at the Developer-prompt requirement.
fn run_linker(mut cmd: Command, tool: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| {
        if cfg!(target_os = "windows") {
            format!(
                "failed to invoke `{tool}`: {e} — on Windows, run snc from a Developer \
                 Command Prompt so the MSVC linker + libraries are on PATH"
            )
        } else {
            format!("failed to invoke `{tool}`: {e}")
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{tool}` exited with {status}"))
    }
}

/// Link object(s) + the runtime into an executable. ADR 0060 Phase 2: `cc` on
/// Unix (macOS + Linux); the MSVC `link.exe` on Windows, where the runtime's
/// native deps ([`WINDOWS_NATIVE_LIBS`]) are named explicitly and the MSVC
/// environment (the `LIB` search paths) must be present — run snc from a
/// Developer Command Prompt.
/// ADR 0057 A9: union the CLI `--link` libraries with any declared in the
/// program's extern blocks (`extern "C" link("…") { … }`), deduped and
/// order-preserving (CLI first, then source-declared). Lets a binding module be
/// self-linking — a consumer needs no `--link` flag.
fn collect_link_libs(cli: &[String], externs: &[ExternFnDecl]) -> Vec<String> {
    let mut libs: Vec<String> = cli.to_vec();
    for e in externs {
        for l in &e.link_libs {
            if !libs.iter().any(|x| x == l) {
                libs.push(l.clone());
            }
        }
    }
    libs
}

fn link_exe(objects: &[&Path], exe: &Path, link_libs: &[String]) -> Result<(), String> {
    let runtime = find_runtime()?;
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("link.exe");
        cmd.arg("/NOLOGO").arg("/SUBSYSTEM:CONSOLE");
        cmd.arg(format!("/OUT:{}", exe.display()));
        for obj in objects {
            cmd.arg(obj);
        }
        cmd.arg(&runtime);
        for lib in WINDOWS_NATIVE_LIBS {
            cmd.arg(lib);
        }
        // ADR 0057 pillar 4: user-requested extra libraries (`--link user32`).
        // MSVC links the import library `<name>.lib`.
        for lib in link_libs {
            cmd.arg(if lib.ends_with(".lib") {
                lib.clone()
            } else {
                format!("{lib}.lib")
            });
        }
        cmd.arg("/DEFAULTLIB:msvcrt");
        run_linker(cmd, "link.exe")
    } else {
        let mut cmd = Command::new("cc");
        for obj in objects {
            cmd.arg(obj);
        }
        cmd.arg(&runtime);
        // ADR 0057 pillar 4: user-requested extra libraries (`--link m` → `-lm`).
        for lib in link_libs {
            cmd.arg(format!("-l{lib}"));
        }
        cmd.arg("-o").arg(exe);
        run_linker(cmd, "cc")
    }
}

/// Link several object files + the runtime into one executable. The caller
/// pre-sorts `objects` for a deterministic link.
fn link_objects(objects: &[PathBuf], exe: &Path, link_libs: &[String]) -> Result<(), String> {
    let refs: Vec<&Path> = objects.iter().map(PathBuf::as_path).collect();
    link_exe(&refs, exe, link_libs)
}

/// Phase D.6 / ADR 0037 (a): TRUE per-unit separate compilation —
/// `snc build --separate <entry>`. Discovers the module graph, then
/// ADR 0037 (3/N): a content fingerprint for one separately-compiled unit —
/// a hash over everything that affects its `.o`, so an UNCHANGED unit can reuse
/// its cached object on the next build. Sound and ITEM-GRANULAR: the unit's own
/// source; the content of EACH `pub` ITEM it imports (the matching
/// [`ExportedItem`] from the exports table, NOT the whole defining module's
/// source); the graph-wide sorted effect names (the op-id base map a unit's
/// `perform`/`handle` op ids ride on); the unit's module path (its symbol
/// prefix); and the compiler version (invalidate on upgrade). `DefaultHasher`
/// has fixed keys → process-stable (unlike a `HashMap`'s random seed).
///
/// Item granularity makes the cache PRECISE: an `ExportedItem::Fn` is
/// SIGNATURE-only, so editing a non-generic imported fn's BODY doesn't
/// recompile its importers (they extern-call it; the relink picks up the new
/// body). Inlined items (struct / enum / generic body / trait / effect) carry
/// their full decl, so a change there does recompile importers.
fn unit_fingerprint(
    m: &DiscoveredModule,
    exports: &HashMap<(Vec<String>, String), ExportedItem>,
    effect_names: &[String],
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut h);
    m.path.hash(&mut h);
    m.source.hash(&mut h);
    // Each imported item, keyed `(origin_module, item)`, sorted for a stable
    // order regardless of `use`-declaration order. Hash the item's content so a
    // change to a USED item recompiles this unit but a change to an unused
    // sibling in the same module does not.
    let mut keys: Vec<(Vec<String>, String)> = m
        .program
        .uses
        .iter()
        .filter(|u| u.path.len() >= 2)
        .map(|u| {
            let item = u.path.last().expect("len >= 2").clone();
            let origin = u.path[..u.path.len() - 1].to_vec();
            (origin, item)
        })
        .collect();
    keys.sort();
    keys.dedup();
    for key in &keys {
        key.hash(&mut h);
        if let Some(item) = exports.get(key) {
            item.hash(&mut h);
        }
    }
    effect_names.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// compiles EACH module to its OWN object independently (`resolve_module` +
/// `check_module` against the imported `pub fn` signatures, then the
/// effect / borrow / CT gates + codegen with the module path threaded for
/// D7 mangling), and links the per-unit objects + the runtime. Cross-module
/// references resolve at LINK time via module-qualified `abi-v1` symbols —
/// the back end the merge (`run_build_merged`) stands in for, opt-in until
/// it reaches parity. A single-file program (no `use`) has no
/// separate-compilation work and routes to the normal build.
///
/// (3/N) INCREMENTAL: each unit's `.o` is fingerprinted ([`unit_fingerprint`])
/// into an `<obj>.fp` sidecar; on a rebuild a unit whose fingerprint is
/// unchanged reuses its cached object (the per-unit pipeline + codegen are
/// skipped, printing `fresh <module>`) — the per-unit `.o` is reproducible
/// (see `repro.rs`), so reusing it is sound.
fn run_build_separate(path: &str, output: Option<&str>) -> ExitCode {
    let modules = match discover_module_graph(Path::new(path)) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("snc: {msg}");
            return ExitCode::from(1);
        }
    };
    if modules.is_empty() {
        // Single-file: nothing to compile separately. (`--separate` does not
        // thread `--link` yet; ADR 0060 follow-up.)
        return run_build(path, output, &[]);
    }

    // Visibility / existence gate (ModuleNotFound / UnknownImport /
    // PrivateItem) — reuse the Path-A validator before any compilation.
    let units: Vec<sentinel_resolve::ModuleUnit> = modules
        .iter()
        .map(|m| sentinel_resolve::ModuleUnit { path: m.path.clone(), program: &m.program })
        .collect();
    if let Err(e) = sentinel_resolve::resolve_imports(&units) {
        eprintln!("snc: {e}");
        return ExitCode::from(1);
    }

    // Pre-pass: the exports table — each module's `pub fn` signatures keyed
    // by (module_path, fn_name). Signatures only (not bodies) — cheap.
    let mut exports: HashMap<(Vec<String>, String), ExportedItem> = HashMap::new();
    for m in &modules {
        let items = match extract_exports(&m.program) {
            Ok(items) => items,
            Err(e) => {
                eprintln!("snc: {e}");
                return ExitCode::from(1);
            }
        };
        for (name, item) in items {
            exports.insert((m.path.clone(), name), item);
        }
    }

    let exe_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => PathBuf::from(path).with_extension(""),
    };
    let obj_dir = exe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // ADR 0037 (2/N): the build-wide op-id base map. Every effect NAME
    // declared anywhere in the graph gets a graph-stable index (sorted for
    // determinism). Codegen uses it as the `(base << 16) | op` basis so a
    // `perform` in the defining unit and a `handle` in an importing unit
    // agree on the runtime op id even though each unit numbers its own
    // `EffectId`s locally (the index into its `effect_decls[]`). Built from
    // the modules' OWN effect decls (pre-inlining), so each effect counts
    // once at its definition site. (MVP: keyed by NAME — same-named
    // cross-module effects would collide; an origin-qualified key is the
    // robust upgrade, flagged in ADR 0037's SETTLED DESIGN POINTS.)
    let mut effect_names: Vec<String> = Vec::new();
    for m in &modules {
        for ed in &m.program.effects {
            if !effect_names.contains(&ed.name) {
                effect_names.push(ed.name.clone());
            }
        }
    }
    effect_names.sort();
    let op_id_base: HashMap<String, u32> = effect_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i as u32))
        .collect();

    // Compile each module to its own object.
    let mut objects: Vec<PathBuf> = Vec::new();
    for (idx, m) in modules.iter().enumerate() {
        let is_entry = idx == 0;
        let obj_path = obj_dir.join(format!("{}.o", m.path.join("_")));

        // (3/N) INCREMENTAL: skip a unit whose object is already on disk with a
        // matching fingerprint. The whole per-unit pipeline (resolve → check →
        // effect/borrow/CT → codegen) is bypassed; the cached `.o` is sound to
        // reuse because per-unit codegen is reproducible (`repro.rs`). A unit
        // with a build error never wrote a `.o`/`.fp`, so it is never "fresh".
        let fingerprint = unit_fingerprint(m, &exports, &effect_names);
        let fp_path = obj_dir.join(format!("{}.o.fp", m.path.join("_")));
        if obj_path.is_file()
            && std::fs::read_to_string(&fp_path).ok().as_deref() == Some(fingerprint.as_str())
        {
            eprintln!("snc: fresh `{}`", m.path.join("::"));
            objects.push(obj_path);
            continue;
        }

        // Resolve this module's `use` imports against the exports table: a
        // `pub fn` becomes an extern descriptor (resolve + types levels); a
        // `pub struct` is re-materialized by INLINING its decl into this
        // unit's program (types are layout — no link symbol, ADR 0037 D4).
        let mut import_fns: Vec<sentinel_resolve::ImportedFn> = Vec::new();
        let mut typed_imports: Vec<sentinel_types::TypedImportedFn> = Vec::new();
        let mut imported_structs: Vec<sentinel_ast::StructDecl> = Vec::new();
        // ADR 0037 (2/N) point 8: (imported struct name → its origin module
        // path), so codegen can ORIGIN-qualify the struct's tag in a
        // linkonce_odr mono key and dedup `id<geo::Point>` soundly.
        let mut imported_struct_origins: Vec<(String, Vec<String>)> = Vec::new();
        let mut imported_enums: Vec<sentinel_ast::EnumDecl> = Vec::new();
        // ADR 0037 (2/N) point 8: (imported enum name → origin), mirror of the
        // struct map, so codegen origin-qualifies an enum tag in a mono key.
        let mut imported_enum_origins: Vec<(String, Vec<String>)> = Vec::new();
        let mut imported_generic_fns: Vec<sentinel_ast::FnDef> = Vec::new();
        // ADR 0037 (2/N) `linkonce_odr`: (imported generic fn name → its origin
        // module path), so codegen can emit collision-safe instances under the
        // ORIGIN symbol + dedup them across importers.
        let mut imported_generic_origins: Vec<(String, Vec<String>)> = Vec::new();
        let mut imported_traits: Vec<sentinel_ast::TraitDecl> = Vec::new();
        let mut imported_effects: Vec<sentinel_ast::EffectDecl> = Vec::new();
        for u in &m.program.uses {
            let item = u.path.last().expect("validated: >= 1 segment").clone();
            let origin = u.path[..u.path.len() - 1].to_vec();
            match exports.get(&(origin.clone(), item.clone())) {
                Some(ExportedItem::Fn(ex)) => {
                    import_fns.push(sentinel_resolve::ImportedFn {
                        name: item.clone(),
                        arity: ex.arity,
                        type_params_count: ex.type_params_count,
                        origin: origin.clone(),
                        span: u.span.clone(),
                    });
                    typed_imports.push(sentinel_types::TypedImportedFn {
                        name: item,
                        param_type_exprs: ex.param_type_exprs.clone(),
                        return_type_expr: ex.return_type_expr.clone(),
                        effect_row_names: ex.effect_row_names.clone(),
                    });
                }
                Some(ExportedItem::Struct(decl)) => {
                    imported_struct_origins.push((decl.name.clone(), origin.clone()));
                    imported_structs.push(decl.clone());
                }
                Some(ExportedItem::Enum(decl)) => {
                    imported_enum_origins.push((decl.name.clone(), origin.clone()));
                    imported_enums.push(decl.clone());
                }
                Some(ExportedItem::GenericFn(fndef)) => {
                    imported_generic_origins.push((fndef.name.clone(), origin.clone()));
                    imported_generic_fns.push(fndef.as_ref().clone())
                }
                Some(ExportedItem::Trait(decl)) => imported_traits.push(decl.as_ref().clone()),
                Some(ExportedItem::Effect(decl)) => imported_effects.push(decl.as_ref().clone()),
                None => {
                    eprintln!(
                        "snc: `{}` from `{}` is not an exported `pub fn` / `pub struct` / \
                         `pub enum` / `pub trait` / `pub effect` (this D.6 slice)",
                        item,
                        origin.join("::")
                    );
                    return ExitCode::from(1);
                }
            }
        }

        // Build this unit's program: own items + the inlined imported type
        // decls + imported GENERIC fn bodies, with `use`s cleared (the driver
        // has resolved them — non-generic fns via `import_fns` externs; types
        // + generic fns inlined here). resolve_module re-materializes the
        // imported structs/enums in this unit's StructId/EnumId space and
        // treats the imported generic fns as local generics (monomorphized
        // locally, ADR 0037 D6), transparent to the types + codegen layers.
        let mut prog = m.program.clone();
        prog.uses.clear();
        prog.structs.extend(imported_structs);
        prog.enums.extend(imported_enums);
        prog.fns.extend(imported_generic_fns);
        prog.traits.extend(imported_traits);
        prog.effects.extend(imported_effects);

        // resolve_module → check_module → effect → borrow → CT → hir → object.
        let resolved = match sentinel_resolve::resolve_module(&prog, &import_fns) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("snc: {e}");
                return ExitCode::from(1);
            }
        };
        // ADR 0037 (2/N) `linkonce_odr`: map each imported generic's resolved
        // FnId → its origin module path (resolve assigned the id when the body
        // was inlined; names are unique — resolve rejects collisions). Codegen
        // emits collision-safe instances of these under the origin symbol so
        // importers share one definition.
        let generic_origins: HashMap<sentinel_resolve::FnId, Vec<String>> = imported_generic_origins
            .iter()
            .filter_map(|(name, origin)| {
                resolved
                    .fn_signatures
                    .iter()
                    .find(|s| &s.name == name)
                    .map(|s| (s.id, origin.clone()))
            })
            .collect();
        // ADR 0037 (2/N) point 8: imported struct name → resolved StructId →
        // origin, so codegen origin-qualifies its tag in a linkonce_odr mono
        // key (names are unique — resolve rejects collisions).
        let struct_origins: HashMap<sentinel_resolve::StructId, Vec<String>> = imported_struct_origins
            .iter()
            .filter_map(|(name, origin)| {
                resolved
                    .structs
                    .iter()
                    .find(|s| &s.name == name)
                    .map(|s| (s.id, origin.clone()))
            })
            .collect();
        // …and the same for imported enums (name → resolved EnumId → origin).
        let enum_origins: HashMap<sentinel_resolve::EnumId, Vec<String>> = imported_enum_origins
            .iter()
            .filter_map(|(name, origin)| {
                resolved
                    .enums
                    .iter()
                    .find(|e| &e.name == name)
                    .map(|e| (e.id, origin.clone()))
            })
            .collect();
        let named_origins = sentinel_codegen::NamedTypeOrigins {
            structs: struct_origins,
            enums: enum_origins,
        };
        let has_main = resolved.fn_signatures.iter().any(|s| s.is_main);
        if is_entry && !has_main {
            eprintln!("snc: the entry module `{}` has no `main`", m.path.join("::"));
            return ExitCode::from(1);
        }
        if !is_entry && has_main {
            eprintln!(
                "snc: only the entry module may define `main` (`{}` does)",
                m.path.join("::")
            );
            return ExitCode::from(1);
        }
        let typed = match sentinel_types::check_module(&resolved, &typed_imports) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("snc: {e}");
                return ExitCode::from(1);
            }
        };
        let (_ec, effect_errors) = sentinel_effect_check::effect_check(&typed);
        if !effect_errors.is_empty() {
            for e in &effect_errors {
                eprintln!("snc: {e}");
            }
            return ExitCode::from(1);
        }
        let (drop_plan, borrow_errors) = sentinel_borrow_check::borrow_check(&typed);
        if !borrow_errors.is_empty() {
            for e in &borrow_errors {
                eprintln!("snc: {e}");
            }
            return ExitCode::from(1);
        }
        let mir = sentinel_mir::lower_to_mir(&typed);
        let leaks = sentinel_mir::verify_constant_time(&mir);
        if !leaks.is_empty() {
            for leak in &leaks {
                eprintln!("snc: {leak}");
            }
            return ExitCode::from(1);
        }
        let hir = sentinel_hir::lower_to_hir(&typed, &drop_plan);
        // The build-wide op-id base map (computed above) is passed to EVERY
        // unit so a cross-UNIT `perform`/`handle` pair encodes the same op id.
        if let Err(err) = sentinel_codegen::compile_to_object_for_module(
            &hir,
            &m.path,
            &op_id_base,
            &generic_origins,
            &named_origins,
            &obj_path,
        ) {
            eprintln!("snc: codegen failed for module `{}`: {err}", m.path.join("::"));
            return ExitCode::from(1);
        }
        // (3/N) cache: stamp the unit's fingerprint beside its fresh object so
        // the next build can reuse it if nothing it depends on changed. A write
        // failure only forfeits caching (the object is correct), so ignore it.
        let _ = std::fs::write(&fp_path, &fingerprint);
        objects.push(obj_path);
    }

    // Deterministic link order (path-sorted) + the runtime.
    objects.sort();
    match link_objects(&objects, &exe_path, &[]) {
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

fn link(object: &Path, exe: &Path, link_libs: &[String]) -> Result<(), String> {
    link_exe(&[object], exe, link_libs)
}

/// Locate the runtime staticlib adjacent to the snc binary. Cargo puts the snc
/// bin and the runtime staticlib in the same target directory
/// (`target/<profile>/`), so a single lookup off `current_exe().parent()`
/// covers both `cargo run --bin snc` and `CARGO_BIN_EXE_snc`-driven integration
/// tests. ADR 0060 Phase 2: the staticlib name is host-dependent —
/// `libsentinel_runtime.a` (Unix) vs `sentinel_runtime.lib` (MSVC).
fn find_runtime() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe(): {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent directory".to_string())?;
    let name = if cfg!(target_os = "windows") {
        "sentinel_runtime.lib"
    } else {
        "libsentinel_runtime.a"
    };
    let runtime = dir.join(name);
    if !runtime.exists() {
        return Err(format!(
            "{name} not found at {} — \
             run `cargo build -p sentinel-runtime` to produce it",
            runtime.display()
        ));
    }
    Ok(runtime)
}
