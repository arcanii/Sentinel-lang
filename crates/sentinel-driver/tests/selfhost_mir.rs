//! Phase D self-host port (7/N) / ADR 0044 D8: the MIR differential test.
//! Compile `selfhost/mir.sentinel` (which REUSES `selfhost/types.sentinel` via a D.6
//! `use`, mode 2 — which in turn `use`s the parser, a 3-deep module chain) with the
//! Rust `snc`, then assert its lowered-form dump is byte-identical to `snc mir`.
//!
//! (7a) covers the STRAIGHT-LINE grammar (const / var / unary / binop / cmp / `let`
//! → one block + Return); (7b) adds CONTROL FLOW (`if` / `&&` / `||` → branch + a
//! merge block whose params reconcile diverged vars, VarId-sorted); (7c) adds the
//! `Opaque` catch-all (struct/array/field/widen/char/string/null), `Load`
//! (index/`*`-deref), `declassify`, and calls (plain + builtin/generic via the
//! `mir_args` operand stack). The effect/class/enum forms + the full-corpus phase-go
//! (`sentinel_mir_matches_oracle_on_corpus`) close (7c)/(7e). Fixtures the oracle
//! rejects (parse/resolve/type errors) exit nonzero and are skipped — error parity is
//! out of scope (ADR 0044 D7), as in every prior stage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// ADR 0067: stage a multi-file module's `<module>/` parts dir alongside the
/// staged `<module>.sentinel`. A no-op if the module has no parts dir.
fn stage_module_parts(root: &Path, dst: &Path, module: &str) {
    let src = root.join("selfhost").join(module);
    if !src.is_dir() {
        return;
    }
    let pd = dst.join(module);
    std::fs::create_dir_all(&pd).expect("create parts dir");
    for ent in std::fs::read_dir(&src).expect("read parts dir") {
        let p = ent.expect("dir entry").path();
        if p.extension().and_then(|x| x.to_str()) == Some("sentinel") {
            std::fs::copy(&p, pd.join(p.file_name().unwrap())).expect("stage a part");
        }
    }
}

/// Stage `parser.sentinel` + `types.sentinel` + `mir.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry
/// `mir.sentinel`.
fn build_sentinel_mir_lowerer(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    stage_module_parts(&root, tmp, "parser");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    stage_module_parts(&root, tmp, "types");
    let entry = tmp.join("mir.sentinel");
    std::fs::copy(root.join("selfhost/mir.sentinel"), &entry).expect("stage mir.sentinel");
    let bin = tmp.join("smir");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/mir.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Straight-line seeds (7a): arithmetic + comparison + a `let` + a unary negate, each
/// lowering to a single block ending in Return. The trailing `main` is lowered too.
const SEEDS: &[&str] = &[
    "fn dbl(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { 0 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { 0 }\n",
    "fn poly(a: i64, b: i64, c: i64) -> i64 { a * b + c }\nfn main() -> i64 { 0 }\n",
    "fn lt(a: i64, b: i64) -> bool { a < b }\nfn main() -> i64 { 0 }\n",
    "fn t() -> bool { true }\nfn main() -> i64 { 0 }\n",
    "fn lets(x: i64) -> i64 { let y = x + 1; y * 2 }\nfn main() -> i64 { 0 }\n",
    "fn neg(x: i64) -> i64 { let n = -x; n }\nfn main() -> i64 { 0 }\n",
    "fn cmp_let(a: i64, b: i64) -> bool { let d = a - b; d < 0 }\nfn main() -> i64 { 0 }\n",
    // (7b) control flow: if/branch + merge param, && / || short-circuit (incl. secret
    // — a secret-dependent branch), a var diverged across arms (1 and 2 vars), nested
    // ifs, and a logic chain.
    "fn pick(c: bool) -> i64 { if c { 1 } else { 2 } }\nfn main() -> i64 { 0 }\n",
    "fn both(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }\n",
    "fn either(a: bool, b: bool) -> bool { a || b }\nfn main() -> i64 { 0 }\n",
    "fn sec(s: secret bool, t: secret bool) -> secret bool { s && t }\nfn main() -> i64 { 0 }\n",
    "fn g(c: bool) -> i64 { let mut x = 1; if c { x = 2; 0 } else { 0 }; x }\nfn main() -> i64 { 0 }\n",
    "fn nested(a: bool, b: bool) -> i64 { if a { if b { 1 } else { 2 } } else { 3 } }\nfn main() -> i64 { 0 }\n",
    "fn chain(a: bool, b: bool, cc: bool) -> bool { a && b || cc }\nfn main() -> i64 { 0 }\n",
    "fn two(c: bool, d: bool) -> i64 { let mut x = 0; let mut y = 10; if c { x = 1; 0 } else { y = 20; 0 }; x + y }\nfn main() -> i64 { 0 }\n",
    // (7c) the Opaque catch-all + Load + calls: a plain call with args, a nested call, a
    // struct-lit + field read, an array-lit + index, a `*`-deref load, a widen-to-nullable
    // Opaque, declassify, a char/string Opaque, and generic/builtin calls (vec_new/push).
    "fn id(x: i64) -> i64 { x }\nfn use_it(a: i64, b: i64) -> i64 { id(a) + id(b) }\nfn main() -> i64 { 0 }\n",
    "fn add3(a: i64, b: i64, cc: i64) -> i64 { a + b + cc }\nfn nest(x: i64) -> i64 { add3(x, add3(x, x, x), x) }\nfn main() -> i64 { 0 }\n",
    "struct P { x: i64, y: i64 }\nfn mk(a: i64, b: i64) -> P { P { x: a, y: b } }\nfn getx(p: P) -> i64 { p.x }\nfn main() -> i64 { 0 }\n",
    "fn arr() -> [i64] { [1, 2, 3] }\nfn at(a: [i64], i: i64) -> i64 { a[i] / 2 }\nfn main() -> i64 { 0 }\n",
    "fn deref(r: &i64) -> i64 { *r }\nfn main() -> i64 { 0 }\n",
    "fn widen(x: i64) -> ?i64 { x }\nfn wb(b: i64) -> ?i64 { let y: ?i64 = b + 1; y }\nfn main() -> i64 { 0 }\n",
    "fn unwrap(s: secret i64) -> i64 { declassify(s) }\nfn main() -> i64 { 0 }\n",
    "fn ch() -> u8 { 'a' }\nfn st() -> [u8] { \"hi\" }\nfn main() -> i64 { 0 }\n",
    "fn pushy(n: i64) -> [i64] { let mut v: Vec<i64> = vec_new(); push(&mut v, n); vec_to_array(v) }\nfn ln(a: [i64]) -> i64 { len(a) }\nfn main() -> i64 { 0 }\n",
    "fn id<T>(x: T) -> T { x }\nfn use_g() -> i64 { id(5) }\nfn main() -> i64 { 0 }\n",
];

#[test]
fn sentinel_mir_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_mir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lowerer = build_sentinel_mir_lowerer(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("mir")
            .arg(&input)
            .output()
            .expect("run snc mir");
        let sentinel = Command::new(&lowerer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel MIR lowerer");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  seed {seed:?}\n    oracle:   {}\n    sentinel: {}",
                String::from_utf8_lossy(&oracle.stdout).trim_end(),
                String::from_utf8_lossy(&sentinel.stdout).trim_end()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the Sentinel MIR lowerer diverged from `snc mir` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}

/// Every `.sentinel` fixture under tests/pass + tests/ui, sorted.
fn collect_fixtures() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut fixtures = Vec::new();
    for sub in ["tests/pass", "tests/ui"] {
        for entry in std::fs::read_dir(root.join(sub)).expect("read fixture dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    fixtures
}

/// (7e) the D8 PHASE-GO: the Sentinel MIR lowerer matches `snc mir` byte-for-byte
/// over the ENTIRE clean-lowering corpus (= the type-clean set — lowering is total).
/// Mirrors `sentinel_borrow_checker_matches_oracle_on_corpus`; fixtures the oracle
/// rejects (parse/resolve/type errors) exit nonzero and are skipped.
#[test]
fn sentinel_mir_matches_oracle_on_corpus() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_mir_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lowerer = build_sentinel_mir_lowerer(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    assert!(fixtures.len() > 100, "expected a substantial corpus, got {}", fixtures.len());

    let mut clean = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("mir")
            .arg(&input)
            .output()
            .expect("run snc mir");
        if !oracle.status.success() {
            continue;
        }
        clean += 1;
        let sentinel = Command::new(&lowerer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel MIR lowerer");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-lowering fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel MIR lowerer diverged from `snc mir` on {}/{} clean-lowering fixture(s):\n{}",
        mismatches.len(),
        clean,
        mismatches.join("\n")
    );
}

// ---- (7d) the const-time verifier (`ctverify.sentinel`, types::run mode 3) --------

/// Stage parser + types + `ctverify.sentinel` and compile the entry (the verifier
/// builds the MIR via the mode-2 fused lowering, then runs `verify_constant_time`).
fn build_sentinel_ctverifier(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    stage_module_parts(&root, tmp, "parser");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    stage_module_parts(&root, tmp, "types");
    let entry = tmp.join("ctverify.sentinel");
    std::fs::copy(root.join("selfhost/ctverify.sentinel"), &entry).expect("stage ctverify.sentinel");
    let bin = tmp.join("sctv");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/ctverify.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Verifier seeds: the only type-clean MIR leak is a secret `&&`/`||` short-circuit
/// (a `Branch` on a secret cond) — every other sink (a secret index / divisor /
/// pointer) is a source-level type rejection. Plus no-false-positive cases (a secret
/// flowing through arithmetic / compare / declassify is constant-time).
const CTVERIFY_SEEDS: &[&str] = &[
    "fn f(s: secret bool, t: secret bool) -> secret bool { s && t }\nfn main() -> i64 { 0 }\n",
    "fn f(s: secret bool, t: secret bool) -> secret bool { s || t }\nfn main() -> i64 { 0 }\n",
    "fn f(a: secret i64, b: secret i64) -> i64 { declassify(a + b) }\nfn main() -> i64 { 0 }\n",
    "fn f(a: secret i64, b: secret i64) -> secret bool { a == b }\nfn main() -> i64 { 0 }\n",
    "fn f(a: secret i64) -> secret i64 { a * 2 }\nfn main() -> i64 { 0 }\n",
];

#[test]
fn sentinel_ctverifier_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_ctv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let verifier = build_sentinel_ctverifier(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in CTVERIFY_SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("ctverify")
            .arg(&input)
            .output()
            .expect("run snc ctverify");
        let sentinel = Command::new(&verifier)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel verifier");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  seed {seed:?}\n    oracle:   {}\n    sentinel: {}",
                String::from_utf8_lossy(&oracle.stdout).trim_end(),
                String::from_utf8_lossy(&sentinel.stdout).trim_end()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the Sentinel verifier diverged from `snc ctverify` on {}/{} seed(s):\n{}",
        mismatches.len(),
        CTVERIFY_SEEDS.len(),
        mismatches.join("\n")
    );
}

/// (7d) the verifier phase-go: the Sentinel const-time verifier matches `snc ctverify`
/// over the ENTIRE type-clean corpus (empty for all but `c52_secret_leak`, which leaks
/// a `Branch`). A no-false-positive sweep + the one true positive.
#[test]
fn sentinel_ctverifier_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_ctv_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let verifier = build_sentinel_ctverifier(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    let mut clean = 0usize;
    let mut leaking = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("ctverify")
            .arg(&input)
            .output()
            .expect("run snc ctverify");
        if !oracle.status.success() {
            continue;
        }
        clean += 1;
        if !oracle.stdout.is_empty() {
            leaking += 1;
        }
        let sentinel = Command::new(&verifier)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel verifier");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {:?} vs sentinel {:?})",
                fixture.file_name().unwrap().to_string_lossy(),
                String::from_utf8_lossy(&oracle.stdout),
                String::from_utf8_lossy(&sentinel.stdout),
            ));
        }
    }
    assert!(clean > 100, "expected >100 clean fixtures, got {clean}");
    assert!(leaking >= 1, "expected at least one leaking fixture (c52_secret_leak), got {leaking}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel verifier diverged from `snc ctverify` on {}/{} fixture(s):\n{}",
        mismatches.len(),
        clean,
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
//
// NOTE this file hosts TWO stages (`mir` and `ctverify`), so unlike its six
// sibling differentials the registry is a PARAMETER of the helper rather than a
// file-global pair of consts. Everything else is identical.

/// `snc mir` — programs whose divergence is a KNOWN, deliberately-deferred
/// feature gap, each with the ADR that defers it. A listed program is still
/// COMPARED — the self-hosted stage is RUN, so a crash still fails the test —
/// but its byte-difference is not a failure. Deleting an entry is how a mirror
/// slice records that it closed the gap; an entry the sweep never REACHES fails
/// the test too, because an unreached entry is as stale as a matching one.
///
/// THE LINE BETWEEN THE TWO LISTS, stated because the lookup chains them: the
/// classification changes NOTHING about whether the test passes, so it is pure
/// documentation — which is exactly why a reader has to be able to apply it to
/// a NEW divergence. A divergence is DEFERRED when reproducing the oracle needs
/// a shape `scg` does not have (a token, an AST node, a type handle) that a fix
/// would then have to thread through every downstream stage: closing it is a
/// MIRROR SLICE, and the ADR cited says the mirror is deferred. It is a BUG
/// when `scg` already has every shape involved and the divergence is a HOLE in
/// dispatch it has already ported: closing it is a FIX that changes nothing
/// downstream.
const MIR_DEFERRED_PROGRAMS: &[(&str, &str)] = &[
    // ADR 0070 D3-revisit: a `Fn<T,R>`-typed local called DIRECTLY (`op(x)`).
    // scg keeps ADR 0020 D5's "vars win over fns" dispatch unconditionally, so
    // it lowers `(v2:i64 opaque v1)` — a kont resume — where the oracle lowers
    // `(v2:i64 call apply v0 v1)`. Already registered at codegen; this test
    // shows it is present at MIR, i.e. upstream of where it was first seen.
    ("examples/lang/fn_value.sentinel", "ADR 0070 D3-revisit: a direct call of a Fn-typed var lowers as a kont resume in scg"),
    ("examples/lang/fn_value.sentinel (merged)", "ADR 0070 D3-revisit: a direct call of a Fn-typed var lowers as a kont resume in scg"),
    // NARROWED, not deleted: this entry carried both the ADR 0058 float cause
    // (now closed — `f64` is scalar code 4 and a literal lowers to the oracle's
    // `(vN:f64 opaque)`) and the ADR 0070 direct call, which is still live.
    // (`task_generic.sentinel` was float-only and IS deleted.)
    ("examples/lang/fn_value_generic.sentinel", "ADR 0070 D3-revisit: `apply_bool` uses direct-call sugar, which scg lowers as a kont resume"),
    // ADR 0066 M2.3b: generic word-scalar elements for the PROCESS channel are
    // snc-only — scg lowers `process_recv` to `?i64` where the oracle has `?u8`.
    ("examples/lang/process_channel_typed.sentinel", "ADR 0066 M2.3b: generic process-channel elements are snc-only (scg lowers process_recv `?i64`)"),
];

/// `snc mir` — programs whose divergence is a REAL BUG in the self-hosted
/// stage, not a deferred feature. An entry here must carry its DIAGNOSIS and is
/// a debt marker to be deleted by a fix, never by a re-label. See the
/// DEFERRED/BUG criterion on `MIR_DEFERRED_PROGRAMS` above; a BUG entry citing
/// an ADR that defers a mirror is not a contradiction (the ADRs defer the
/// FEATURE mirror, never the dump arm).
const MIR_KNOWN_SCG_BUGS: &[(&str, &str)] = &[];

/// `snc ctverify` — EMPTY, but read WHY before treating that as parity
/// evidence, because on this one stage the byte-comparison is VACUOUS.
/// `snc ctverify` prints one `(leak <SinkKind>)` line per leak and nothing
/// else, and not one of the 22 real-program comparisons DECLARES a `secret`
/// value (five of the source files say the word, all five only in a comment) —
/// so every one of the 22 is `"" == ""`. Two things it therefore does
/// NOT show: that the self-hosted verifier DETECTS anything, and that its
/// detection matches the oracle's. (The one true positive in the repo lives in
/// the fixture corpus, `tests/ui/c52_secret_leak.sentinel`, which the corpus
/// differential above sweeps — and that test carries an explicit `leaking >= 1`
/// non-vacuity floor for exactly this reason.)
///
/// What it DOES pin, and why it earns its place: (a) no FALSE POSITIVE — if the
/// self-hosted verifier reported a leak on any of the 22, its dump would be
/// non-empty and this test would fail; (b) no CRASH — scg's whole front end
/// (lex → resolve → types → effect → borrow → ctverify) runs to completion on
/// 22 real-program comparisons, which the crash guard checks unconditionally.
///
/// Why no secret-bearing real program reaches this stage: every one of them is
/// multi-module, so it can only arrive via the MERGED form, and `snc merge`'s
/// Bar-A source printer rejects `declassify` (36 of the 119 programs) and
/// `cast` (37) — which is what every secret-bearing program in `examples/` uses.
/// Widening that printer is the change that would make this sweep non-vacuous.
const CTVERIFY_DEFERRED_PROGRAMS: &[(&str, &str)] = &[];

/// `snc ctverify` — see `MIR_KNOWN_SCG_BUGS`. Empty for the same reason, and
/// subject to the same vacuity caveat as `CTVERIFY_DEFERRED_PROGRAMS` above.
const CTVERIFY_KNOWN_SCG_BUGS: &[(&str, &str)] = &[];

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
    deferred: &[(&str, &str)],
    bugs: &[(&str, &str)],
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
            let registered = deferred
                .iter()
                .chain(bugs.iter())
                .any(|(p, _)| *p == key.as_str());
            if oracle.stdout == sentinel.stdout {
                if registered {
                    stale.push(format!(
                        "  {key} now MATCHES the oracle — delete it from the \
                         deferred / known-bug list"
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
    let unreached: Vec<String> = deferred
        .iter()
        .chain(bugs.iter())
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
    assert!(stale.is_empty(), "the deferred / known-bug list is stale:\n{}", stale.join("\n"));
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
fn sentinel_mir_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_mir_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lowerer = build_sentinel_mir_lowerer(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // The semantic stages do no module discovery, so the MERGED form is what
    // supplies the multi-module programs (`delegation`, `rect_demo`,
    // `process_ids`, `sort_search`, …) — 12 direct + 12 merged (the 12th of
    // each is `examples/lang/phantom_type_param.sentinel`, ADR 0016 A1 — it merges, so
    // the merged FLOOR moves with the direct count, per this helper's own
    // "the floor is set to today's actual count" rule).
    real_program_differential(
        "mir",
        &lowerer,
        &work,
        MIR_DEFERRED_PROGRAMS,
        MIR_KNOWN_SCG_BUGS,
        12,
        12,
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn sentinel_ctverifier_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_ctv_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let ctv = build_sentinel_ctverifier(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    real_program_differential(
        "ctverify",
        &ctv,
        &work,
        CTVERIFY_DEFERRED_PROGRAMS,
        CTVERIFY_KNOWN_SCG_BUGS,
        12,
        12,
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
