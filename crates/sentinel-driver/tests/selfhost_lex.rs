//! Phase D self-host port (1/N) / ADR 0038 D10: the lexer differential
//! test. Compile `selfhost/lexer.sentinel` with the Rust `snc`, then assert
//! its token dump is byte-identical to `snc lex` for every clean-lexing
//! fixture in `tests/pass` + `tests/ui`. This is the corpus-wide proof that
//! the Sentinel-written lexer reproduces the Rust lexer (the oracle).
//!
//! Excluded: `tests/ui/lex_invalid_char.sentinel` — a deliberate lex ERROR
//! fixture (`let x = @`). Lexer error parity is a follow-on (ADR 0038
//! D6/D8); (1/N) validates happy-path token production, where the oracle
//! exits 0 with a full dump.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Compile `selfhost/lexer.sentinel` once into a temp binary.
fn build_sentinel_lexer(tmp: &Path) -> PathBuf {
    let src = workspace_root().join("selfhost/lexer.sentinel");
    let bin = tmp.join("slex");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/lexer.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Every `*.sentinel` fixture except the deliberate lex-error one.
fn collect_fixtures() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut fixtures = Vec::new();
    for sub in ["tests/pass", "tests/ui"] {
        for entry in std::fs::read_dir(root.join(sub)).expect("read fixture dir") {
            let path = entry.expect("dir entry").path();
            let is_sentinel = path.extension().and_then(|e| e.to_str()) == Some("sentinel");
            let is_lex_error =
                path.file_name().and_then(|n| n.to_str()) == Some("lex_invalid_char.sentinel");
            if is_sentinel && !is_lex_error {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    fixtures
}

#[test]
fn sentinel_lexer_matches_oracle_on_corpus() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_lex_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lexer = build_sentinel_lexer(&tmp);

    // The Sentinel lexer reads `./input.sentinel`; stage each fixture there
    // and run it with this as the cwd.
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    assert!(fixtures.len() > 50, "expected a substantial corpus, got {}", fixtures.len());

    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");

        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("lex")
            .arg(&input)
            .output()
            .expect("run snc lex");
        let sentinel = Command::new(&lexer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel lexer");

        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "the Sentinel lexer diverged from `snc lex` on {}/{} fixture(s):\n{}",
        mismatches.len(),
        fixtures.len(),
        mismatches.join("\n")
    );
}
