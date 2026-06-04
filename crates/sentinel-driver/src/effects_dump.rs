//! Phase D self-host port (5/N) / ADR 0042 D2: the effect-check differential
//! oracle — a dump of `EffectCheckedProgram.effective_rows` that the
//! Sentinel-written effect-check stage (`selfhost/effects.sentinel`) will
//! reproduce byte-for-byte.
//!
//! One line per USER fn, in FnId order: `(fn #<id> <name> <effect-name>…)` — the
//! fn's effective effect row (the union of annotated/inferred effects, after
//! handler/scope discharge), each effect rendered by its source name in EffectId
//! order (the `BTreeSet` order — deterministic, no interner-ID obligation). A fn
//! with an empty row dumps `(fn #N name)`. The built-in `Async` effect renders
//! `Async`. Runtime builtins (#0–#13) are effect-free + omitted (not in
//! `program.fns`). A dev/validation surface, NOT `abi-v1` — pinned by a golden.
//!
//! `run_effects` only calls this on a clean program (no effect errors); a fixture
//! the oracle rejects (any parse/resolve/type/effect error) exits nonzero, so the
//! corpus differential skips it — the happy-path discipline (ADR 0042 D5/D7).

use sentinel_effect_check::EffectCheckedProgram;
use sentinel_types::TypedProgram;

/// Canonical effect-row dump of `program` given its computed `effective_rows`.
pub fn dump(program: &TypedProgram, checked: &EffectCheckedProgram) -> String {
    let mut out = String::new();
    // FnId order over the USER fns (builtins are effect-free + not dumped).
    let mut fns: Vec<&sentinel_types::TypedFnDef> = program.fns.iter().collect();
    fns.sort_by_key(|f| f.id.0);
    for f in fns {
        out.push_str("(fn #");
        out.push_str(&f.id.0.to_string());
        out.push(' ');
        out.push_str(&f.name);
        if let Some(row) = checked.effective_rows.get(&f.id) {
            // BTreeSet → EffectId order. Render each by its declared name.
            for eid in row {
                out.push(' ');
                let ename = program
                    .effect_decls
                    .get(eid.0 as usize)
                    .map(|d| d.name.as_str())
                    .unwrap_or("<effect>");
                out.push_str(ename);
            }
        }
        out.push(')');
        out.push('\n');
    }
    out
}
