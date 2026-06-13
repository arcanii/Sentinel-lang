//! Phase D self-host port (6/N) / ADR 0043 D2: the borrow-check differential
//! oracle — a dump of the `DropPlan.moved_sources` that the Sentinel-written
//! borrow-check stage (`selfhost/borrow.sentinel`) will reproduce byte-for-byte.
//!
//! One line per USER fn, in FnId order: `(fn #<id> <name> #<vid>… #<vid>.<field>…)` —
//! the fn's MOVED-SOURCE set (the VarIds used in a consuming/move position whose type
//! is non-Copy, which codegen must NOT drop), VarIds in ascending (BTreeSet) order,
//! followed by the ADR 0046 PARTIAL-move set (`#<vid>.<field>` — a Move-typed field
//! consumed by value, codegen elides it from the binding's recursive drop), in
//! ascending `(VarId, field)` order. A fn with no moves dumps `(fn #N name)`. The
//! partial-move suffix is empty for every fixture that consumes no Move-typed field
//! (the whole existing corpus), so those lines are byte-unchanged. A dev/validation
//! surface, NOT `abi-v1` — pinned by a golden.
//!
//! `run_borrow` only calls this on a clean program (no borrow errors); a fixture the
//! oracle rejects (any parse/resolve/type/borrow error) exits nonzero, so the corpus
//! differential skips it — the happy-path discipline (ADR 0043 D5/D7).

use sentinel_borrow_check::DropPlan;
use sentinel_types::TypedProgram;

/// Canonical moved-sources dump of `program` given its computed `DropPlan`.
pub fn dump(program: &TypedProgram, plan: &DropPlan) -> String {
    let mut out = String::new();
    // FnId order over the USER fns (the DropPlan is keyed by FnId).
    let mut fns: Vec<&sentinel_types::TypedFnDef> = program.fns.iter().collect();
    fns.sort_by_key(|f| f.id.0);
    for f in fns {
        out.push_str("(fn #");
        out.push_str(&f.id.0.to_string());
        out.push(' ');
        out.push_str(&f.name);
        // `moved_sources_for` returns the (empty if absent) BTreeSet — ascending VarId.
        for vid in plan.moved_sources_for(f.id) {
            out.push_str(" #");
            out.push_str(&vid.0.to_string());
        }
        // ADR 0046: the PARTIAL-move set — `#<vid>.<field>` for each Move-typed field
        // consumed by value, in ascending `(VarId, field)` order (the BTreeSet's order).
        // Empty (no extra bytes) unless the fn consumes a Move-typed field.
        for &(vid, field) in plan.moved_fields_for(f.id) {
            out.push_str(" #");
            out.push_str(&vid.0.to_string());
            out.push('.');
            out.push_str(&field.to_string());
        }
        out.push(')');
        out.push('\n');
    }
    out
}
