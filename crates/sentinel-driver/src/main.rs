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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use miette::{LabeledSpan, MietteDiagnostic, NamedSource, Report, Severity as MietteSeverity, SourceSpan};
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
    eprintln!("    snc lex <file>                   lex and dump the token stream (self-host oracle)");
    eprintln!("    snc ast <file>                   parse and dump the canonical AST (self-host oracle)");
    eprintln!("    snc parse <file>                 lex, parse, and pretty-print the program");
    eprintln!("    snc build <file> [-o <output>]   compile and link to an executable");
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
        TokenKind::Ident | TokenKind::IntLit | TokenKind::StringLit | TokenKind::CharLit
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
        vec![DiscoveredModule { path: vec![entry_stem], program: entry_prog }];
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
            out.push(DiscoveredModule { path: module, program });
        }
        scan += 1;
    }
    Ok(out)
}

fn run_build(path: &str, output: Option<&str>) -> ExitCode {
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
                Ok(merged) => run_build_merged(merged, path, output),
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

    match link(&object_path, &exe_path) {
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
fn run_build_merged(merged: Program, path: &str, output: Option<&str>) -> ExitCode {
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
