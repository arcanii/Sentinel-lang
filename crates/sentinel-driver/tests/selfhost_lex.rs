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

// ---------------------------------------------------------------------------
// The EXTENDED program differential: REAL programs, not just the curated
// single-file fixture corpus.
//
// `collect_fixtures` above sweeps only `tests/pass` + `tests/ui` — single-file
// fixtures written to exercise ONE construct each. Until this test, only
// `selfhost_codegen.rs` swept `examples/` + `sentinel_library/` + `tools/`,
// which left every UPSTREAM stage differential structurally blind to divergence
// in real programs. That is not hypothetical: when this test was written NOTHING
// in the fixture corpus used `export "C"`, a float literal, or any of the four
// reserved-name intrinsics (`sqrt` / `ptr_of` / `ptr_of_mut` / `is_null`) — so
// three separate unmirrored front-end surfaces had never once been compared, and
// one of them, the float literal `2.0`, was SILENTLY misparsed by the
// self-hosted parser as a field access. The same blind spot previously hid the
// ADR 0067 `module`/`part` lex gap.
//
// ALL THREE are now closed, each by the intended lifecycle — the sweep found the
// hole, the fix landed, and a FIXTURE now pins it so the corpus cannot lose sight
// of it again: `export "C"` by `tests/pass/c59_export_call.sentinel`, the ADR 0058
// float literal (with `sqrt`, which A1 makes part of the same feature) by
// `tests/pass/c58_float_math.sentinel`, and ADR 0057's `ptr_of` / `ptr_of_mut` /
// `is_null` by `tests/pass/c57_ptr_of.sentinel`. That is the point of this test
// arriving at an empty or near-empty registry: the list was never the deliverable
// on its own — converting an invisible gap into an auditable one was, and an
// auditable gap is one somebody closes.
//
// TWO FORMS of each program are compared, because the stage oracles
// (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`) do NO
// module discovery — only `snc llvm` merges. A program with a `use` edge is
// rejected outright ("`use` imports are not yet wired"), so the direct form
// alone would leave the semantic stages seeing 11 of 119 programs:
//   (a) DIRECT — the program as written;
//   (b) MERGED — `snc merge`'s single-file collapse of its module graph, which
//       is exactly the shape the self-hosted `scg` consumes at the bootstrap
//       fixed point. This is what puts REAL multi-module programs
//       (`delegation`, `rect_demo`, `process_ids`, `sort_search`, …) through
//       the stage at all. It is not redundant at `lex`/`ast` either, where the
//       direct form already covers all 119: the merge qualifies every top-level
//       item as `module$path$item`, and `$` is an identifier-CONTINUATION
//       character only so that text lexes (ADR 0045 8g) — no file under
//       `demos/` + `examples/` + `sentinel_library/` + `tools/` and no fixture in
//       `tests/pass` + `tests/ui` contains a `$`, so these 21 comparisons are
//       the only place either lexer meets one outside the fixed point itself.
//
// Every divergence must be either FIXED or listed below with the ADR that
// defers it (`DEFERRED_PROGRAMS`) or its diagnosis (`KNOWN_SCG_BUGS`). The list
// IS the deliverable — it converts an invisible gap into an auditable one, and
// a listed program that starts matching FAILS the test, so the list cannot rot.
//
// ONE LIMITATION, stated because it is not obvious and is inherited from
// `selfhost_codegen.rs`: registration is keyed by PROGRAM PATH and is
// cause-BLIND. A listed program's byte-difference is excused wholesale, so a
// SECOND, unrelated divergence introduced into an already-listed file would NOT
// fail this test — though the identical construct added to any unlisted file
// does. What still fires for a listed program: a crash, the entry going stale
// by matching, and the entry never being reached. So keep the lists short,
// prefer fixing a listed program over letting it accumulate causes, and when a
// program does have several, NAME them all — and when a slice closes only ONE of
// them, EDIT the entry down rather than deleting it. (`quadratic.sentinel` was
// the worked example of a two-cause entry until the ADR 0058 mirror closed both
// of its causes at once; `fn_value_generic.sentinel` is the live one, narrowed
// by that same slice from float+ADR-0070 down to ADR 0070 alone.)

/// Programs whose divergence is a KNOWN, deliberately-deferred feature gap,
/// each with the ADR that defers it. A listed program is still COMPARED — the
/// self-hosted stage is RUN, so a crash still fails the test — but its
/// byte-difference is not a failure. Deleting an entry is how a mirror slice
/// records that it closed the gap; an entry the sweep never REACHES fails the
/// test too, because an unreached entry is as stale as a matching one.
///
/// THE LINE BETWEEN THE TWO LISTS, stated because `deferred_reason` chains
/// them: the classification changes NOTHING about whether the test passes, so
/// it is pure documentation — which is exactly why a reader has to be able to
/// apply it to a NEW divergence. A divergence is DEFERRED when reproducing the
/// oracle needs a shape `scg` does not have (a token, an AST node, a type
/// handle) that a fix would then have to thread through every downstream stage:
/// closing it is a MIRROR SLICE, and the ADR cited says the mirror is deferred.
/// It is a BUG when `scg` already has every shape involved and the divergence
/// is a HOLE in dispatch it has already ported: closing it is a FIX that
/// changes nothing downstream.
///
/// EMPTY. Eight programs used to be listed here for the ADR 0058 float-literal
/// gap — `selfhost/lexer.sentinel` had no `FloatLit`, so `2.0` lexed as
/// `IntLit Dot IntLit`. They are DELETED, not re-labelled: the mirror landed.
/// At THIS stage it was the cheap half — `snc lex` prints the raw source span,
/// so the lexer needs only the oracle's token regex and no float FORMATTING
/// (that is the parser's problem, where the oracle prints the decoded value).
const DEFERRED_PROGRAMS: &[(&str, &str)] = &[];

/// Programs whose divergence is a REAL BUG in the self-hosted stage, not a
/// deferred feature — kept separate on purpose. Conflating "we chose not to
/// port this yet" with "this is wrong" is precisely the invisible-gap problem
/// this test exists to end, so an entry here must carry its DIAGNOSIS and is a
/// debt marker to be deleted by a fix, never by a re-label.
///
/// A BUG entry may still cite an ADR that DEFERS a mirror; that is not a
/// contradiction. ADR 0057 / 0059 defer the FEATURE mirror (typing, codegen,
/// `--lib`, the multi-module symbol policy), never the dump arm — the same
/// reason `selfhost/lexer.sentinel` already carries the `export` / `module` /
/// `part` keywords whose features are unported.
const KNOWN_SCG_BUGS: &[(&str, &str)] = &[];

/// The deferral reason for `key` (a repo-relative, forward-slashed path, with a
/// ` (merged)` suffix for the merged form) — `None` when the program is not
/// registered and must therefore match the oracle byte-for-byte.
fn deferred_reason(key: &str) -> Option<&'static str> {
    DEFERRED_PROGRAMS
        .iter()
        .chain(KNOWN_SCG_BUGS.iter())
        .find(|(p, _)| *p == key)
        .map(|(_, why)| *why)
}

/// Recursively collect `.sentinel` files under `dir`.
fn collect_under(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_under(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
            out.push(path);
        }
    }
}

/// The real-program corpus: every `.sentinel` under `demos/`, `examples/`,
/// `sentinel_library/` and `tools/`. Programs the oracle rejects (a library
/// module with no `main`, an unwired `use`, a not-yet-ported construct) are
/// simply skipped, exactly as in the fixture differential above.
///
/// `demos/` was MISSING from this list until a review caught it, and the omission
/// is worth remembering because it is the same species of hole this whole test
/// exists to close — a directory of real programs nothing compared. It is small
/// (three Win32 FFI demos) but not redundant: `demos/win32/messagebox.sentinel`
/// is a SELF-CONTAINED single-file caller of `ptr_of`, so unlike the five
/// `sentinel_library/std/**` modules that also use the intrinsic — which have no
/// `main`, and so are rejected by every semantic oracle — it runs the whole
/// pipeline. Enumerating the roots by hand is what made the omission possible;
/// if a fifth root ever appears, it will need adding here too, in eight files.
fn collect_programs() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for sub in ["demos", "examples", "sentinel_library", "tools"] {
        collect_under(&root.join(sub), &mut out);
    }
    out.sort();
    out
}

/// Copy `src`'s CONTENTS into `dst` recursively (so `sentinel_library/std`
/// lands at `<dst>/std`). Mirrors `selfhost_codegen.rs`'s staging, which is
/// what makes `use std::…` / `use Sentinel::…` resolve for `snc merge`: module
/// discovery roots at the entry file's parent directory.
fn copy_tree_contents(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree_contents(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// Compare `bin` (a compiled self-hosted stage, which reads `./input.sentinel`)
/// against `snc <oracle_cmd>` over every real program, in both the direct and
/// the merged form.
///
/// The two coverage guards exist so that a staging regression which silently
/// stops emitting fails loudly instead of passing vacuously, and they are
/// deliberately of different KINDS:
///   * `expect_direct` is EXACT. The direct form does no module discovery at
///     all, so the set of programs the oracle emits for is platform-invariant;
///     an exact count therefore catches a comparison disappearing even when it
///     was not registered (a floor with slack would absorb it). Growing the
///     corpus bumps this number — the same deliberate act `examples.rs` already
///     requires of a new example.
///   * `min_merged` is a FLOOR. `snc merge` selects target-conditional modules
///     through `host_target_os()` (ADR 0062), so in principle another host can
///     merge a different set. In practice it is exact: every ADR-0062
///     conditional module in the tree (`std/sys/random_unix` /
///     `random_windows`) fails merge-to-source identically on both, so the
///     floor is set to today's actual count.
fn real_program_differential(
    oracle_cmd: &str,
    bin: &Path,
    work: &Path,
    expect_direct: usize,
    min_merged: usize,
) {
    let root = workspace_root();
    // Stage the first-party libraries next to the entry so `use std::…` /
    // `use Sentinel::…` resolves when `snc merge` collapses the module graph.
    copy_tree_contents(&root.join("sentinel_library"), work);
    let input = work.join("input.sentinel");

    let programs = collect_programs();
    assert!(
        programs.len() > 50,
        "expected a substantial program corpus, got {}",
        programs.len()
    );

    let mut direct_emitted = 0usize;
    let mut merged_emitted = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut crashed: Vec<String> = Vec::new();
    let mut silent: Vec<String> = Vec::new();
    let mut reached: Vec<String> = Vec::new();
    for program in &programs {
        let rel = program
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(program).expect("read program");
        for merged_form in [false, true] {
            std::fs::write(&input, &bytes).expect("stage input");
            let key = if merged_form {
                format!("{rel} (merged)")
            } else {
                rel.clone()
            };
            if merged_form {
                let merged = Command::new(env!("CARGO_BIN_EXE_snc"))
                    .arg("merge")
                    .arg(&input)
                    .output()
                    .expect("run snc merge");
                // `snc merge`'s merge-to-source is a Bar-A subset printer, so a
                // program outside it simply has no merged form.
                if !merged.status.success() {
                    continue;
                }
                std::fs::write(&input, &merged.stdout).expect("stage the merged input");
            }
            let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
                .arg(oracle_cmd)
                .arg(&input)
                .output()
                .expect("run the oracle");
            if !oracle.status.success() {
                continue; // not in the emitted subset
            }
            if merged_form {
                merged_emitted += 1;
            } else {
                direct_emitted += 1;
            }
            reached.push(key.clone());
            let sentinel = Command::new(bin)
                .current_dir(work)
                .output()
                .expect("run the self-hosted stage");
            // Checked INDEPENDENTLY of byte-equality, and — unlike anything in
            // `selfhost_codegen` — NOT excused by registration: a registered
            // entry buys different BYTES, never a stage that aborts and never a
            // stage that emits NOTHING (the empty-output guard below).
            // `selfhost_codegen`'s llvm-validity check is the ancestor of both,
            // but note it EXEMPTS registered programs, which is the hole an
            // adversarial review demonstrated here: a plausible half-fix that
            // made a registered construct emit nothing would exit 0, differ
            // from the oracle, and be waved through. A text dump has no
            // `llvm-as` to validate its shape; non-emptiness is the part of
            // that check which does transfer.
            if !sentinel.status.success() {
                crashed.push(format!(
                    "  {key}: exit {:?} — {}",
                    sentinel.status.code(),
                    String::from_utf8_lossy(&sentinel.stderr)
                        .lines()
                        .next()
                        .unwrap_or("<no stderr>")
                ));
            }
            let registered = deferred_reason(&key).is_some();
            if oracle.stdout == sentinel.stdout {
                if registered {
                    stale.push(format!(
                        "  {key} now MATCHES the oracle — delete it from \
                         DEFERRED_PROGRAMS / KNOWN_SCG_BUGS"
                    ));
                }
                continue;
            }
            if registered {
                // A registration buys DIFFERENT bytes, never NO bytes. Every
                // registered key emits a non-empty dump today, and a change
                // that made one emit nothing would otherwise be invisible:
                // exit 0 (no crash), bytes differ (no staleness), key present
                // (no unreached), registered (no mismatch).
                if sentinel.stdout.is_empty() {
                    silent.push(format!(
                        "  {key}: the self-hosted stage emitted NOTHING (the oracle emitted {} bytes)",
                        oracle.stdout.len()
                    ));
                }
                continue; // a registered gap: an ADR-deferred feature or a tracked bug
            }
            mismatches.push(format!(
                "  {key} (oracle {} bytes vs sentinel {} bytes)",
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    // An entry the sweep never REACHED is as stale as one that now matches, and
    // it fails in a way `stale` cannot see: when the oracle stops emitting for a
    // program the loop `continue`s BEFORE the comparison, so the entry is
    // neither exercised nor flagged and quietly becomes dead weight. Neither
    // count guard substitutes for it: `expect_direct` is exact but reports only
    // that a NUMBER moved (and a compensating corpus addition hides even that),
    // `min_merged` is a `>=` floor so a lost merged comparison can be absorbed
    // by a host that merges one more, and neither notices a registry key whose
    // PATH is simply wrong. This is the guard that names the dead entry.
    let unreached: Vec<String> = DEFERRED_PROGRAMS
        .iter()
        .chain(KNOWN_SCG_BUGS.iter())
        .filter(|(p, _)| !reached.iter().any(|r| r == p))
        .map(|(p, _)| format!("  {p} was never compared (the oracle no longer emits for it, or the path is wrong)"))
        .collect();
    assert!(
        unreached.is_empty(),
        "{} registered program(s) were never reached by the sweep — an unreached \
         entry is as stale as a matching one:\n{}",
        unreached.len(),
        unreached.join("\n")
    );
    assert_eq!(
        direct_emitted, expect_direct,
        "the DIRECT-form comparison count changed ({direct_emitted} vs the expected \
         {expect_direct}) — the direct form does no module discovery, so this is \
         platform-invariant: either a program stopped being emitted (a regression, \
         even for an unregistered one) or the corpus grew (bump the number)"
    );
    assert!(
        merged_emitted >= min_merged,
        "the MERGED-form comparison count fell to {merged_emitted}, below the \
         floor of {min_merged} — `snc merge` stopped collapsing programs it used to"
    );
    assert!(
        crashed.is_empty(),
        "the self-hosted stage exited nonzero on {} real program(s) — a registered \
         byte-divergence excuses different bytes, never a crash:\n{}",
        crashed.len(),
        crashed.join("\n")
    );
    assert!(
        silent.is_empty(),
        "the self-hosted stage emitted EMPTY output for {} REGISTERED program(s) — a \
         registered byte-divergence excuses different bytes, never no bytes:\n{}",
        silent.len(),
        silent.join("\n")
    );
    assert!(
        stale.is_empty(),
        "DEFERRED_PROGRAMS / KNOWN_SCG_BUGS is stale:\n{}",
        stale.join("\n")
    );
    assert!(
        mismatches.is_empty(),
        "the self-hosted stage diverged from `snc {oracle_cmd}` on {}/{} emitted \
         real program(s) NOT registered as deferred:\n{}",
        mismatches.len(),
        direct_emitted + merged_emitted,
        mismatches.join("\n")
    );
}

#[test]
fn sentinel_lexer_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_lex_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lexer = build_sentinel_lexer(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // register D54: 121 -> 122 with `examples/export/mut_buffer_lib.sentinel`, the
    // header-only export demonstrator. The count is a TRIPWIRE, not bookkeeping —
    // it is what catches a program silently dropping out of the sweep — so bumping
    // it is only correct when you know why it moved. Here: one file added.
    real_program_differential("lex", &lexer, &work, 122, 23);
    let _ = std::fs::remove_dir_all(&tmp);
}
