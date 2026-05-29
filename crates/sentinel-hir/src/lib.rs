//! sentinel-hir — the typed, qualifier-resolved high-level IR.
//!
//! Per ADR 0009 §6.1 the pipeline is `types → hir → mir → codegen`;
//! C0–C4 short-circuited `types → codegen`. ADR 0026 (Phase C5.1)
//! stands up the HIR as the stage between the type checker's
//! [`TypedProgram`] and codegen. The HIR's eventual job (ADR 0026 D1)
//! is to *desugar*: resolve method/trait/qualified dispatch to concrete
//! callees, materialise monomorphic instances, make drops / scope-exits
//! explicit (folding in the borrow checker's [`DropPlan`]), and carry
//! `secret` qualifiers through unchanged.
//!
//! ## C5.1a (this increment): the stage boundary only
//!
//! [`HirProgram`] is, for now, a thin *borrowing view* bundling the
//! type checker's [`TypedProgram`] with the borrow checker's
//! [`DropPlan`]; codegen consumes `&HirProgram` rather than the two
//! separately. No desugaring happens yet — the lowering is the identity
//! bundle, so the pipeline's behaviour is unchanged by construction
//! (the ADR 0026 D9 `c51` bar: every `tests/pass/` fixture and every
//! `tests/repro.rs` object stays byte-identical). The desugaring passes
//! land in later C5.1a increments, each *thickening* the HIR with owned,
//! desugared fields and *thinning* codegen, while holding that bar.
//!
//! ## Why a pure function, not a salsa query (yet)
//!
//! [`lower_to_hir`] is a pure function the driver calls between borrow
//! checking and codegen, matching codegen's own non-salsa status
//! (ADR 0011 C1.0c: stages downstream of the type checker that only
//! feed codegen need not be salsa-tracked, and LLVM `'ctx` lifetimes
//! don't fit salsa cleanly). The salsa-tracked `hir_query` of ADR 0026
//! D1 is deferred until the HIR is a self-contained *owned* structure
//! worth memoising for the MIR / LSP — recorded as a D1 amendment at
//! C5.1 close, not a silent deviation.

use sentinel_borrow_check::DropPlan;
use sentinel_types::TypedProgram;

/// The high-level IR handed to codegen.
///
/// At C5.1a this is a borrowing bundle of the type-checked program and
/// its drop plan (see the module docs); it grows owned, desugared
/// fields as later C5.1a increments push work across the codegen
/// boundary.
pub struct HirProgram<'a> {
    program: &'a TypedProgram,
    drop_plan: &'a DropPlan,
}

impl<'a> HirProgram<'a> {
    /// The type-checked program.
    ///
    /// Transitional accessor: codegen still lowers [`TypedProgram`]
    /// directly at C5.1a. Later increments replace these reads with
    /// desugared HIR nodes and this accessor narrows or disappears.
    pub fn program(&self) -> &'a TypedProgram {
        self.program
    }

    /// The borrow checker's drop plan (the scope-exit drop sites).
    ///
    /// Transitional accessor: a later C5.1a increment folds these into
    /// the HIR as explicit drop statements (ADR 0026 D1).
    pub fn drop_plan(&self) -> &'a DropPlan {
        self.drop_plan
    }
}

/// Lower the type-checked program + its drop plan into the HIR.
///
/// At C5.1a the lowering is the identity bundle (see the module docs);
/// the real desugar — dispatch resolution, monomorphisation, explicit
/// drops — arrives in later C5.1a increments (ADR 0026 D1).
pub fn lower_to_hir<'a>(program: &'a TypedProgram, drop_plan: &'a DropPlan) -> HirProgram<'a> {
    HirProgram { program, drop_plan }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lower_to_hir` bundles a real type-checked program with a drop
    /// plan and the accessors round-trip it. Built through the actual
    /// pipeline because `TypedProgram` is not `Default`-constructible.
    #[test]
    fn lowers_a_minimal_program() {
        let prog = sentinel_syntax::parse("fn main() -> i64 { 0 }").expect("parse");
        let resolved = sentinel_resolve::resolve(&prog).expect("resolve");
        let typed = sentinel_types::check(&resolved).expect("check");
        let drop_plan = DropPlan::default();

        let hir = lower_to_hir(&typed, &drop_plan);

        // The bundle exposes exactly the inputs it was built from.
        assert_eq!(hir.program().fns.len(), typed.fns.len());
        assert!(
            hir.program().fns.iter().any(|f| f.name == "main"),
            "the lowered program retains `main`"
        );
        let _ = hir.drop_plan();
    }
}
