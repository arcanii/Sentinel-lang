//! Phase D self-host port (8g) path (a) — the Sentinel un-parser (`selfhost/merge.sentinel`).
//!
//! Path (a) self-hosts the D.6 module merge so `scg` discovers + merges + emits with no
//! Rust pre-pass. Its foundation is a Sentinel un-parser: a parsed program re-emitted as
//! re-parseable source (the Sentinel port of `crates/sentinel-driver/src/source_dump.rs`).
//!
//! (a-1)+(a-1b) — the SINGLE-MODULE un-parser (fns + structs + enums + the full
//! expr/stmt/type grammar). This test is the round-trip gate: the un-parser re-emits a
//! single-module program, and `snc llvm` on the un-parsed source is BYTE-IDENTICAL to
//! `snc llvm` on the original (parens are AST-transparent → the re-parsed AST, hence the
//! `.ll`, is identical). The per-module rename + BFS discovery + the `scg`-side merge land
//! in (a-2)…(a-4).

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `merge.sentinel` into `tmp` (so `use parser::…` resolves)
/// and compile the un-parser binary.
fn build_sentinel_merge(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    let entry = tmp.join("merge.sentinel");
    std::fs::copy(root.join("selfhost/merge.sentinel"), &entry).expect("stage merge.sentinel");
    let bin = tmp.join("merge");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/merge.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// (a-1)+(a-1b) The un-parser re-emits the real single-module selfhost stages
/// (`lexer.sentinel`, `parser.sentinel` — no `use`, so they pass through unmerged) as
/// re-parseable source whose `snc llvm` `.ll` is byte-identical to the originals'.
#[test]
fn sentinel_merge_unparser_round_trips_single_module_stages() {
    let tmp = std::env::temp_dir().join(format!("snc_merge_unparse_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let merge = build_sentinel_merge(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");
    let root = workspace_root();

    for stage in ["lexer", "parser"] {
        let src = root.join("selfhost").join(format!("{stage}.sentinel"));
        let bytes = std::fs::read(&src).expect("read selfhost stage");
        std::fs::write(&input, &bytes).expect("stage input");

        let orig = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&src)
            .output()
            .expect("run snc llvm (original)");
        assert!(
            orig.status.success(),
            "snc llvm rejected selfhost/{stage}.sentinel:\n{}",
            String::from_utf8_lossy(&orig.stderr)
        );

        // The un-parser reads `./input.sentinel` and prints the re-emitted source.
        let unparsed = Command::new(&merge)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel un-parser");
        assert!(
            unparsed.status.success(),
            "the un-parser failed on selfhost/{stage}.sentinel:\n{}",
            String::from_utf8_lossy(&unparsed.stderr)
        );
        let up_path = work.join("unparsed.sentinel");
        std::fs::write(&up_path, &unparsed.stdout).expect("write un-parsed source");

        let rt = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&up_path)
            .output()
            .expect("run snc llvm (round-trip)");
        assert!(
            rt.status.success(),
            "snc llvm rejected the un-parsed selfhost/{stage}.sentinel:\n{}",
            String::from_utf8_lossy(&rt.stderr)
        );

        assert_eq!(
            orig.stdout,
            rt.stdout,
            "selfhost/{stage}.sentinel: the un-parser round-trip diverged (orig {} vs rt {} bytes)",
            orig.stdout.len(),
            rt.stdout.len()
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// (a-2)+(a-3) The Sentinel merge — discovery (BFS over `use` edges) + the per-module
/// rename map (own names qualified by the module's `$`-prefix; `use a::b::Item` →
/// `a$b$Item`) + the fused rewrite — collapses the real multi-module compiler to one
/// `$`-qualified source. Validate: `snc llvm` on the Sentinel-merged source is
/// BYTE-IDENTICAL to `snc llvm` on the entry (which goes through the Rust `merge_modules`)
/// — i.e. the Sentinel merge == the Rust merge, semantically. The entry is staged as
/// `input.sentinel` so its stem ("input") is the same for both sides.
#[test]
fn sentinel_merge_matches_oracle_on_multi_module_stages() {
    let tmp = std::env::temp_dir().join(format!("snc_merge_multi_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let merge = build_sentinel_merge(&tmp);
    let root = workspace_root();

    // (entry stage, dependency stages reachable via `use`). `types` uses `parser`;
    // `codegen` uses `merge` + `types` (which both use `parser`).
    let cases: &[(&str, &[&str])] =
        &[("types", &["parser"]), ("codegen", &["merge", "types", "parser"])];

    for (entry, deps) in cases {
        let work = tmp.join(entry);
        std::fs::create_dir_all(&work).expect("create work dir");
        // The entry is staged as `input.sentinel` (the name `merge` reads); deps keep
        // their own names so the `use` edges resolve.
        std::fs::copy(
            root.join("selfhost").join(format!("{entry}.sentinel")),
            work.join("input.sentinel"),
        )
        .expect("stage entry as input.sentinel");
        for dep in *deps {
            std::fs::copy(
                root.join("selfhost").join(format!("{dep}.sentinel")),
                work.join(format!("{dep}.sentinel")),
            )
            .expect("stage dependency");
        }

        // The Rust oracle: `snc llvm` on the entry (discovers + `merge_modules` + dumps).
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(work.join("input.sentinel"))
            .output()
            .expect("run snc llvm (oracle)");
        assert!(
            oracle.status.success(),
            "snc llvm rejected the {entry} entry:\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );

        // The Sentinel merge: reads `./input.sentinel`, discovers + merges, prints source.
        let merged = Command::new(&merge)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel merge");
        assert!(
            merged.status.success(),
            "the Sentinel merge failed on {entry}:\n{}",
            String::from_utf8_lossy(&merged.stderr)
        );
        let merged_path = work.join("merged.sentinel");
        std::fs::write(&merged_path, &merged.stdout).expect("write merged source");

        // `snc llvm` on the Sentinel-merged single source.
        let sg = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&merged_path)
            .output()
            .expect("run snc llvm (sentinel-merged)");
        assert!(
            sg.status.success(),
            "snc llvm rejected the Sentinel-merged {entry}:\n{}",
            String::from_utf8_lossy(&sg.stderr)
        );

        assert_eq!(
            oracle.stdout,
            sg.stdout,
            "{entry}: the Sentinel merge diverged from the Rust merge (oracle {} vs sentinel {} bytes)",
            oracle.stdout.len(),
            sg.stdout.len()
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
