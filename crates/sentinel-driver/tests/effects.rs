//! Phase D self-host port (5/N) / ADR 0042 D2: golden tests for the
//! `snc effects` effect-row-dump oracle — the per-fn effective effect row
//! the Sentinel-written effect-check stage (`selfhost/effects.sentinel`)
//! must reproduce byte-for-byte. One line per user fn in FnId order:
//! `(fn #<id> <name> <effect-name>…)`, effects in EffectId order; an empty
//! row dumps `(fn #N name)`. Builtins (#0–#39) are omitted (the dump covers
//! user fns only; e.g. `process_spawn` carries `Subprocess` but isn't listed).
//! `snc effects` exits nonzero on any error (incl. effect errors), so the
//! corpus differential skips rejected fixtures (the happy-path discipline).

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_effects_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn effects_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("effects")
        .arg(&path)
        .output()
        .expect("run snc effects");
    assert!(
        out.status.success(),
        "snc effects failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn effects_dump_effect_free() {
    // No effects anywhere → every fn dumps an empty row `(fn #N name)`.
    assert_eq!(
        effects_dump(
            "free",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n"
        ),
        "(fn #40 add)\n(fn #41 main)\n"
    );
}

#[test]
fn effects_dump_annotated_and_handled() {
    // `do_work` is annotated `! { Io }` (its row = the annotation); `main`
    // handles the call, discharging Io → empty row.
    assert_eq!(
        effects_dump(
            "handled",
            "effect Io { read() -> i64; }\n\
             fn do_work() -> i64 ! { Io } { perform Io.read() }\n\
             fn main() -> i64 { handle do_work() with { Io.read(k) => k(1) } }\n"
        ),
        "(fn #40 do_work Io)\n(fn #41 main)\n"
    );
}

#[test]
fn effects_dump_inferred_through_call_graph() {
    // `b` is UNANNOTATED but its inferred row = `{ Io }` (it calls `a`, which
    // is `! { Io }`) — the fixed-point at work; `main` handles + discharges.
    assert_eq!(
        effects_dump(
            "inferred",
            "effect Io { read() -> i64; }\n\
             fn a() -> i64 ! { Io } { perform Io.read() }\n\
             fn b() -> i64 { a() }\n\
             fn main() -> i64 { handle b() with { Io.read(k) => k(1) } }\n"
        ),
        "(fn #40 a Io)\n(fn #41 b Io)\n(fn #42 main)\n"
    );
}

#[test]
fn effects_dump_concurrency_scope_discharges_async() {
    // `dbl` is `! { Async }`; `main`'s `scope concurrent { … }` discharges the
    // Async contributed by `spawn` / `.await` → empty row.
    assert_eq!(
        effects_dump(
            "async",
            "fn dbl(x: i64) -> i64 ! { Async } { x * 2 }\n\
             fn main() -> i64 { let r: i64 = scope concurrent { let t = spawn dbl(21); t.await }; r }\n"
        ),
        "(fn #40 dbl Async)\n(fn #41 main)\n"
    );
}
