//! sentinel-effect-check
//!
//! Effect-row checker per ADR 0019 D1 + D2 + D11 + D13 (Phase
//! C3.2). The C3.0 surface accepts effect declarations + postfix
//! `! { Op1, Op2 }` annotations on fn signatures; resolve mirrors
//! them at C3.2(a); types interns them into
//! `TypedProgram.effect_decls` + `TypedFnSignature.effect_row`.
//! This crate runs the actual check:
//!
//!   1. For each fn, compute the **inferred effect row** = the
//!      union of effects from every fn it calls. Use a fixed-
//!      point pass — unannotated fns' rows only grow, so the
//!      iteration converges in O(fn-count) steps.
//!   2. For fns WITH an annotation: validate that the inferred
//!      row ⊆ the annotation. If not, surface
//!      `EffectError::EffectAnnotationMismatch` (the fn declares
//!      it does effects `{A, B}` but actually performs `{A, B,
//!      C}` — the annotation is lying).
//!   3. **`fn main` invariant** per ADR 0019 D13: main's row
//!      must be empty. Any effect bubbling to main rejects with
//!      `EffectError::UnhandledEffect`. At C3.2 minimum (no
//!      handlers yet per D8), this is the only way effects get
//!      "discharged" — main can't have any.
//!
//! Runs as a new salsa-tracked query slotted between
//! `check_query` and `borrow_check_query`:
//!
//! ```text
//! parse_query → resolve_query → check_query → effect_check_query → borrow_check_query → codegen
//! ```
//!
//! At C3.2 there's no `perform` (deferred to ADR 0020 alongside
//! handler runtime), so the ONLY way for a fn to contribute
//! effects to its inferred row is by calling another annotated
//! fn. Runtime builtins (print / unwrap_or / is_some / len) are
//! declared effect-free per the C3.2(a) sig-construction —
//! future ADRs may promote them.
//!
//! ## Annotation as constraint, not declaration (D2)
//!
//! Per ADR 0019 D2, annotations are checked against inference,
//! not the other way around. A fn declaring `! { Io }` whose
//! body calls only effect-free fns is allowed: the annotation
//! widens the row visible to callers (some future use of this
//! fn body might actually perform Io). What's NOT allowed: a
//! fn declaring `! { Io }` whose body calls an effect-bearing
//! fn with `! { Net }` — the body has effect Net that the
//! annotation doesn't list, so the annotation lies.

use std::collections::{BTreeSet, HashMap};

use salsa::Accumulator;
use sentinel_ast::Span;
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_resolve::{EffectId, FnId};
use sentinel_types::{
    TypedBlock, TypedExpr, TypedExprKind, TypedProgram, TypedStmt, TypedStmtKind,
};

/// Per-fn check error variants. Mirrors the ADR 0019 D2 +
/// D13 constraint shape.
#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum EffectError {
    /// A fn's annotated effect row doesn't cover what its body
    /// inferred. `! { Io }` declared but body calls a fn with
    /// `! { Net }` → the annotation is missing `Net`.
    #[error(
        "fn `{fn_name}` declared effect row `{declared}` but body uses `{got_extra}` too"
    )]
    #[diagnostic(
        code(sentinel::effect::annotation_mismatch),
        help(
            "either widen the annotation to include the inferred effects, or wrap the offending call site in a handler (handlers land at ADR 0020)"
        )
    )]
    EffectAnnotationMismatch {
        fn_name: String,
        /// The effects in the body's inferred row not covered
        /// by the annotation. Rendered as a comma-separated list.
        got_extra: String,
        /// The annotation itself, rendered the same way.
        declared: String,
        #[label("annotated `! {{ {declared} }}` on `{fn_name}`")]
        span: miette::SourceSpan,
    },

    /// `fn main` per ADR 0019 D13 must have an empty effect row;
    /// any effect bubbling to main is unhandled at C3.2 (since
    /// handlers are deferred per D8). Surfaces the FIRST effect
    /// observed; future improvements can list all of them.
    #[error("unhandled effect `{effect_name}` in `fn main`")]
    #[diagnostic(
        code(sentinel::effect::unhandled_effect),
        help(
            "main must be effect-free at C3.2 (handlers land at ADR 0020); refactor to push the effect call inside a handler"
        )
    )]
    UnhandledEffect {
        effect_name: String,
        #[label("effect `{effect_name}` bubbles to here")]
        span: miette::SourceSpan,
    },
}

/// Effect-checked program. At C3.2 the only payload is the
/// per-fn effective row (annotated, or inferred for unannotated
/// fns) — useful for diagnostic display + future codegen rounds.
///
/// Stored as `BTreeMap` / `BTreeSet` rather than the more usual
/// `HashMap` / `HashSet` so the type implements `Hash + Eq` for
/// salsa-tracked caching (same pattern as `DropPlan` in
/// sentinel-borrow-check).
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct EffectCheckedProgram {
    /// Effective row per FnId — the union of (annotated or
    /// inferred) effects this fn contributes when called. For
    /// annotated fns: equals the annotation. For unannotated:
    /// equals the inferred row (= union of called fns' effective
    /// rows). Empty `BTreeSet` for fns with no effects.
    pub effective_rows: std::collections::BTreeMap<FnId, BTreeSet<EffectId>>,
}

/// Run the effect check. Returns the per-fn effective row map
/// plus any errors. Errors don't stop the analysis: the row map
/// is always populated so downstream passes can consult it even
/// when a mismatch is reported.
pub fn effect_check(program: &TypedProgram) -> (EffectCheckedProgram, Vec<EffectError>) {
    let mut errors: Vec<EffectError> = Vec::new();

    // C4.4 / ADR 0024 D5: the built-in `Async` effect's id (resolve
    // auto-registers it). `spawn` / `.await` contribute Async to a
    // fn's inferred row; a `scope concurrent { ... }` block
    // discharges it (like a handler discharges its handled effects).
    // `None` only if a program somehow lacks the auto-registered
    // Async — defensive; in practice always `Some`.
    let async_id: Option<EffectId> = program
        .effect_decls
        .iter()
        .position(|d| d.name == "Async")
        .map(|i| EffectId(i as u32));

    // ADR 0066 M2.1 / D7: the built-in `Subprocess` capability effect bubbles to
    // `main` (a program that spawns processes is honestly subprocess-using), so —
    // unlike every other effect — it is EXEMPT from the Pass-3 main-effect-free
    // check. `process_spawn`'s signature row carries it; it propagates normally.
    let subprocess_id: Option<EffectId> = program
        .effect_decls
        .iter()
        .position(|d| d.name == "Subprocess")
        .map(|i| EffectId(i as u32));

    // Initialize: annotated fns get their annotation as the
    // initial row; unannotated fns start with empty rows that
    // grow during the fixed-point iteration. Runtime builtins
    // also get their declared row (effect-free at C3.2 per the
    // type-checker setup).
    let mut effective: HashMap<FnId, BTreeSet<EffectId>> = HashMap::new();
    for sig in &program.fn_signatures {
        let row: BTreeSet<EffectId> = sig.effect_row.iter().copied().collect();
        effective.insert(sig.id, row);
    }

    // Fixed-point: for each unannotated fn, set row = union of
    // called fns' rows. Annotated fns stay frozen at their
    // annotation. Iterations are bounded by the call-graph depth
    // for unannotated cycles (all empty); for annotated callees,
    // each unannotated caller picks up the annotation in one
    // pass. Practical bound is fn-count + 1 sweeps.
    let max_sweeps = program.fns.len() + 4;
    for _ in 0..max_sweeps {
        let mut changed = false;
        for fn_def in &program.fns {
            let sig = &program.fn_signatures[fn_def.id.0 as usize];
            if !sig.effect_row.is_empty() {
                // Annotated: trusted; row stays at the annotation.
                continue;
            }
            let inferred = collect_inferred_row(&fn_def.body, &effective, async_id);
            if effective.get(&fn_def.id) != Some(&inferred) {
                effective.insert(fn_def.id, inferred);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Pass 2: for each annotated fn, check the inferred row
    // (computed independently of the annotation) is ⊆ the
    // annotation. The annotation is a CONSTRAINT, not a
    // declaration — it says "the body must not use effects
    // outside this set."
    for fn_def in &program.fns {
        let sig = &program.fn_signatures[fn_def.id.0 as usize];
        if sig.effect_row.is_empty() {
            continue;
        }
        let inferred = collect_inferred_row(&fn_def.body, &effective, async_id);
        let annotation: BTreeSet<EffectId> = sig.effect_row.iter().copied().collect();
        let extra: BTreeSet<EffectId> = inferred.difference(&annotation).copied().collect();
        if !extra.is_empty() {
            errors.push(EffectError::EffectAnnotationMismatch {
                fn_name: sig.name.clone(),
                got_extra: render_effect_set(&extra, program),
                declared: render_effect_set(&annotation, program),
                span: to_source_span(&fn_def.span),
            });
        }
    }

    // Pass 3: main must be effect-free per ADR 0019 D13. Surface
    // the first effect that bubbles to it. A separately-compiled LIBRARY
    // module (ADR 0037) has no `main` — its effect-freedom is checked in
    // whichever unit IS the entry — so skip Pass 3 when main is absent
    // (the guarded find is identical to `program.main()` when present).
    if let Some(main) = program.fns.iter().find(|f| program.signature(f.id).is_main) {
        if let Some(main_row) = effective.get(&main.id) {
            // ADR 0066 M2.1 / D7: `Subprocess` is allowed at `main` (it bubbles);
            // every other effect must be handled before `main`.
            if let Some(&first) = main_row.iter().find(|&&e| Some(e) != subprocess_id) {
                let effect_name = effect_name(first, program);
                errors.push(EffectError::UnhandledEffect {
                    effect_name,
                    span: to_source_span(&main.name_span),
                });
            }
        }
    }

    // Convert the working HashMap to a BTreeMap so the returned
    // type can derive Eq + Hash (salsa-tracked return_ref needs
    // both for cache invalidation).
    let effective_rows = effective.into_iter().collect();
    (EffectCheckedProgram { effective_rows }, errors)
}

/// Walk a typed block + every nested expression collecting each
/// Call's callee FnId, and union the called fn's current
/// effective row into the result.
fn collect_inferred_row(
    block: &TypedBlock,
    effective: &HashMap<FnId, BTreeSet<EffectId>>,
    async_id: Option<EffectId>,
) -> BTreeSet<EffectId> {
    let mut acc = BTreeSet::new();
    walk_block(block, effective, async_id, &mut acc);
    acc
}

fn walk_block(
    block: &TypedBlock,
    effective: &HashMap<FnId, BTreeSet<EffectId>>,
    async_id: Option<EffectId>,
    acc: &mut BTreeSet<EffectId>,
) {
    for stmt in &block.stmts {
        walk_stmt(stmt, effective, async_id, acc);
    }
    walk_expr(&block.tail, effective, async_id, acc);
}

fn walk_stmt(
    stmt: &TypedStmt,
    effective: &HashMap<FnId, BTreeSet<EffectId>>,
    async_id: Option<EffectId>,
    acc: &mut BTreeSet<EffectId>,
) {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => walk_expr(value, effective, async_id, acc),
        TypedStmtKind::Assign { target, value } => {
            walk_expr(target, effective, async_id, acc);
            walk_expr(value, effective, async_id, acc);
        }
        TypedStmtKind::While { cond, body } => {
            // Phase D.5 / ADR 0036: the loop's effects are its
            // condition's + its body's (collected once — the effect row
            // is a set, so iteration count is irrelevant).
            walk_expr(cond, effective, async_id, acc);
            walk_block(body, effective, async_id, acc);
        }
        // Phase D.5 (2/N) / ADR 0036 D9: `break` / `continue` perform no
        // effect and contain no sub-expression — nothing to collect.
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
        TypedStmtKind::Expr(e) => walk_expr(e, effective, async_id, acc),
    }
}

fn walk_expr(
    expr: &TypedExpr,
    effective: &HashMap<FnId, BTreeSet<EffectId>>,
    async_id: Option<EffectId>,
    acc: &mut BTreeSet<EffectId>,
) {
    match &expr.kind {
        TypedExprKind::IntLit(_)
        // ADR 0058: a float literal is pure (no effects), like any literal.
        | TypedExprKind::FloatLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        // D.2 / ADR 0033: literals are pure (no effects).
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_)
        // ADR 0070: referencing a fn by name performs no effect itself (the
        // FnRef eligibility check already requires an empty effect row on
        // the referenced fn, so there is nothing to union in here either).
        | TypedExprKind::FnRef(_) => {}

        TypedExprKind::Call { id, args, .. } => {
            // The substantive case: union the callee's effective
            // row into the caller's accumulator.
            if let Some(callee_row) = effective.get(id) {
                acc.extend(callee_row.iter().copied());
            }
            for arg in args {
                walk_expr(arg, effective, async_id, acc);
            }
        }

        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        // ADR 0065: a `return expr` contributes whatever effects its inner
        // performs (the early return itself raises no effect).
        | TypedExprKind::Return(inner)
        | TypedExprKind::Declassify(inner) => walk_expr(inner, effective, async_id, acc),

        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_expr(l, effective, async_id, acc);
            walk_expr(r, effective, async_id, acc);
        }

        TypedExprKind::Block(b) => walk_block(b, effective, async_id, acc),

        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_expr(cond, effective, async_id, acc);
            walk_block(then_branch, effective, async_id, acc);
            walk_block(else_branch, effective, async_id, acc);
        }

        TypedExprKind::StructLit { fields, .. } => {
            // Fields are TypedExpr values directly (declaration
            // order); no inner Value wrapper.
            for f in fields {
                walk_expr(f, effective, async_id, acc);
            }
        }

        TypedExprKind::FieldAccess { target, .. } => {
            walk_expr(target, effective, async_id, acc);
        }

        TypedExprKind::ArrayLit { elements, .. } => {
            for e in elements {
                walk_expr(e, effective, async_id, acc);
            }
        }

        TypedExprKind::Index { target, index, .. } => {
            walk_expr(target, effective, async_id, acc);
            walk_expr(index, effective, async_id, acc);
        }

        // C3.4 / ADR 0020 D6: handle removes the handled (Effect,
        // op) effects from the body's contribution. We walk the
        // body into a *temporary* set, subtract the handled
        // effects, then merge into the outer accumulator. Arms +
        // return arm bodies contribute their own effects normally.
        TypedExprKind::Handle { body, arms, return_arm, handled } => {
            let mut body_row = BTreeSet::new();
            walk_expr(body, effective, async_id, &mut body_row);
            let handled_effects: BTreeSet<EffectId> =
                handled.iter().map(|(eid, _)| *eid).collect();
            for eid in body_row.difference(&handled_effects) {
                acc.insert(*eid);
            }
            for arm in arms {
                walk_expr(&arm.body, effective, async_id, acc);
            }
            if let Some(ra) = return_arm {
                walk_expr(&ra.body, effective, async_id, acc);
            }
        }

        // C3.4 / ADR 0020 D6: perform contributes the op's
        // effect to the row, plus walks args. Inside a matching
        // `handle` the effect is discharged at the outer node;
        // outside one it bubbles to the enclosing fn (and to
        // main, where D13 rejects).
        TypedExprKind::Perform { effect_id, args, .. } => {
            acc.insert(*effect_id);
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }

        // C3.4 / ADR 0020 D5: ResumeKont is the call `k(arg)` —
        // contributes whatever effects the arg expression carries
        // but no new effects of its own. Resuming a continuation
        // doesn't perform additional ops.
        TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }
        // C4.1 / ADR 0022 D3: method calls walk the receiver +
        // args. Method-effect propagation through the receiver's
        // class method-effect table is a follow-on (matches the
        // free-fn pattern via the class's method signature).
        TypedExprKind::MethodCall { target, args, .. } => {
            walk_expr(target, effective, async_id, acc);
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }
        // C4.1 / ADR 0022 D5: `Name::init(args)` walks its args.
        // The init's effect-row is empty at C4.1 (init has no
        // effect annotation surface — D4); future ADR can extend.
        TypedExprKind::ClassInit { args, .. } => {
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }
        // C4.2 / ADR 0023 D5 Path 1: impl-method call (receiver-
        // typed dispatch). Walk receiver + args; impl-method effect
        // propagation is a follow-on like classes (D6).
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            walk_expr(target, effective, async_id, acc);
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }
        // C4.2 / ADR 0023 D5 Path 2: qualified-named call. args[0]
        // is the receiver; args[1..] are the method args. Walk all.
        TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }

        // C4.4 / ADR 0024 D5: `scope concurrent { ... }` discharges
        // the Async effect (the structured-concurrency analogue of a
        // handler discharging its handled effects). Other effects in
        // the body flow through per ADR 0024 D10 ("no special
        // handling of mixed effects").
        TypedExprKind::Scope { body, .. } => {
            let mut body_row = BTreeSet::new();
            walk_block(body, effective, async_id, &mut body_row);
            if let Some(aid) = async_id {
                body_row.remove(&aid);
            }
            acc.extend(body_row);
        }

        // C4.4 / ADR 0024 D5: `spawn fn(args)` contributes Async to
        // the enclosing row (so a fn that spawns must declare
        // `! { Async }` unless inside a discharging scope) + walks
        // the call so the spawned fn's own effects propagate too.
        TypedExprKind::Spawn { call, .. } => {
            if let Some(aid) = async_id {
                acc.insert(aid);
            }
            walk_expr(call, effective, async_id, acc);
        }

        // C4.4 / ADR 0024 D5: `.await` likewise contributes Async +
        // walks the awaited Task expression.
        TypedExprKind::Await { task_expr, .. } => {
            if let Some(aid) = async_id {
                acc.insert(aid);
            }
            walk_expr(task_expr, effective, async_id, acc);
        }

        // Phase D.1 / ADR 0032 (3/N): enum construction + `match`
        // introduce no effect of their own; they propagate whatever
        // their subexpressions carry. Construction walks its payload
        // args; `match` walks the scrutinee + each arm body.
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                walk_expr(a, effective, async_id, acc);
            }
        }
        TypedExprKind::Match { scrutinee, arms, .. } => {
            walk_expr(scrutinee, effective, async_id, acc);
            for arm in arms {
                walk_expr(&arm.body, effective, async_id, acc);
            }
        }
    }
}

/// Render a set of EffectIds as a comma-separated list of
/// declared names. Used in diagnostic strings.
fn render_effect_set(set: &BTreeSet<EffectId>, program: &TypedProgram) -> String {
    set.iter()
        .map(|eid| effect_name(*eid, program))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Look up the source-declared name for an EffectId. Falls back
/// to `<effect#N>` if out of range (defensive).
fn effect_name(eid: EffectId, program: &TypedProgram) -> String {
    program
        .effect_decls
        .get(eid.0 as usize)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| format!("<effect#{}>", eid.0))
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

// =============================================================================
// Salsa-tracked entry point.
// =============================================================================

/// C3.2 / ADR 0019 D11: salsa-tracked effect-check query. Slots
/// between `check_query` and `borrow_check_query` in the
/// pipeline. Consumes the TypedProgram + emits per-error
/// diagnostics through the accumulator; returns the
/// EffectCheckedProgram on success (or None if check or earlier
/// passes errored out).
#[salsa::tracked(return_ref)]
pub fn effect_check_query(
    db: &dyn SentinelDb,
    file: SourceFile,
) -> Option<EffectCheckedProgram> {
    let program = sentinel_types::check_query(db, file).as_ref()?;
    let (checked, errors) = effect_check(program);
    for err in &errors {
        effect_error_to_diagnostic(err).accumulate(db);
    }
    if errors.is_empty() {
        Some(checked)
    } else {
        None
    }
}

fn effect_error_to_diagnostic(err: &EffectError) -> Diagnostic {
    let (code, message, span): (&'static str, String, std::ops::Range<usize>) = match err {
        EffectError::EffectAnnotationMismatch {
            fn_name,
            got_extra,
            declared,
            span,
        } => (
            "sentinel::effect::annotation_mismatch",
            format!(
                "fn `{fn_name}` declared effect row `{declared}` but body uses `{got_extra}` too"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        EffectError::UnhandledEffect { effect_name, span } => (
            "sentinel::effect::unhandled_effect",
            format!("unhandled effect `{effect_name}` in `fn main`"),
            span.offset()..(span.offset() + span.len()),
        ),
    };
    Diagnostic {
        stage: "effect",
        severity: Severity::Error,
        code,
        message,
        span,
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-effect-check"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_resolve::resolve;
    use sentinel_syntax::parse;
    use sentinel_types::check;

    fn check_program(src: &str) -> Result<(EffectCheckedProgram, Vec<EffectError>), String> {
        let prog = parse(src).map_err(|e| format!("parse: {e:?}"))?;
        let resolved = resolve(&prog).map_err(|e| format!("resolve: {e:?}"))?;
        let typed = check(&resolved).map_err(|e| format!("check: {e:?}"))?;
        Ok(effect_check(&typed))
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-effect-check");
    }

    #[test]
    fn pure_program_has_no_errors() {
        let (_, errors) = check_program("fn main() -> i64 { 0 }").unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn unannotated_fn_calling_unannotated_is_pure() {
        let (checked, errors) = check_program(
            "fn helper() -> i64 { 7 }\nfn main() -> i64 { helper() }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
        // main's row should be empty. FnId(0..=13) = the 14 runtime
        // builtins (print/unwrap_or/is_some/len + D.2's str_eq/
        // u8_to_i64/i64_to_u8 + D.3's vec_new/push/pop/vec_to_array +
        // D.4's read_file/write_file/print_bytes); FnId(14) = helper,
        // FnId(15) = main.
        let main_id = FnId(22);
        assert!(checked.effective_rows.get(&main_id).unwrap().is_empty());
    }

    #[test]
    fn annotated_fn_with_empty_body_passes() {
        // `! { Io }` annotation with empty inferred row is fine —
        // annotations are constraints, not requirements.
        let (_, errors) = check_program(
            "effect Io { } fn run() -> i64 ! { Io } { 0 }\nfn main() -> i64 { 0 }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn calling_annotated_fn_from_main_rejects_unhandled() {
        // main calls a fn that declares Io. main's row inflates
        // to {Io}; ADR 0019 D13 rejects.
        let (_, errors) = check_program(
            "effect Io { } fn run() -> i64 ! { Io } { 0 }\nfn main() -> i64 { run() }",
        )
        .unwrap();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, EffectError::UnhandledEffect { effect_name, .. } if effect_name == "Io")),
            "got {errors:?}"
        );
    }

    #[test]
    fn annotation_mismatch_when_body_uses_unlisted_effect() {
        // outer declares `! { Io }` but its body calls a fn
        // with `! { Net }` — the annotation is missing Net.
        let (_, errors) = check_program(
            "effect Io { } effect Net { }\
             fn net_op() -> i64 ! { Net } { 0 }\
             fn outer() -> i64 ! { Io } { net_op() }\
             fn main() -> i64 { 0 }",
        )
        .unwrap();
        assert!(
            errors.iter().any(|e| matches!(e, EffectError::EffectAnnotationMismatch { fn_name, .. } if fn_name == "outer")),
            "got {errors:?}"
        );
    }

    #[test]
    fn annotated_fn_chain_propagates_via_annotation() {
        // outer (annotated `! { Io }`) calls inner (annotated
        // `! { Io }`) — match, no error.
        let (_, errors) = check_program(
            "effect Io { }\
             fn inner() -> i64 ! { Io } { 0 }\
             fn outer() -> i64 ! { Io } { inner() }\
             fn main() -> i64 { 0 }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn unannotated_intermediate_carries_effect_to_main() {
        // bottom (annotated `! { Io }`) → middle (unannotated)
        // → main. middle's inferred row = {Io}; main's inferred
        // row = {Io}; main rejects with UnhandledEffect.
        let (_, errors) = check_program(
            "effect Io { }\
             fn bottom() -> i64 ! { Io } { 0 }\
             fn middle() -> i64 { bottom() }\
             fn main() -> i64 { middle() }",
        )
        .unwrap();
        assert!(
            errors.iter().any(|e| matches!(e, EffectError::UnhandledEffect { .. })),
            "got {errors:?}"
        );
    }

    // ----- C3.4 / ADR 0020 D6: handle discharges effects -----

    #[test]
    fn handle_discharges_io_so_main_is_pure() {
        // do_work() ! { Io } performs Io.read. main wraps it in
        // `handle ... with { Io.read(k) => k(42) }`. After the
        // handle expression, the {Io} row is discharged — main's
        // effective row is empty, ADR 0019 D13 is satisfied.
        let (_, errors) = check_program(
            "effect Io { read() -> i64; }\
             fn do_work() -> i64 ! { Io } { perform Io.read() }\
             fn main() -> i64 {\
                 handle do_work() with { Io.read(k) => k(42) }\
             }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn handle_partial_discharge_leaves_unhandled_effect() {
        // do_two() ! { Io, Net }. main wraps it in a handler that
        // only discharges Io. Net still bubbles to main → reject.
        let (_, errors) = check_program(
            "effect Io { read() -> i64; }\
             effect Net { fetch() -> i64; }\
             fn do_two() -> i64 ! { Io, Net } {\
                 let a: i64 = perform Io.read();\
                 let b: i64 = perform Net.fetch();\
                 a + b\
             }\
             fn main() -> i64 {\
                 handle do_two() with { Io.read(k) => k(1) }\
             }",
        )
        .unwrap();
        assert!(
            errors.iter().any(|e| matches!(e, EffectError::UnhandledEffect { effect_name, .. } if effect_name == "Net")),
            "got {errors:?}"
        );
    }

    #[test]
    fn perform_inside_handle_does_not_bubble() {
        // Inline perform inside a handle — same as above with
        // body=perform-call. Effect discharges cleanly.
        let (_, errors) = check_program(
            "effect Io { read() -> i64; }\
             fn main() -> i64 {\
                 handle perform Io.read() with { Io.read(k) => k(7) }\
             }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    // ----- C4.4 / ADR 0024 D5: scope discharges Async -----

    #[test]
    fn scope_concurrent_discharges_async_so_main_is_pure() {
        // spawn + await contribute Async to the body's row; the
        // `scope concurrent { ... }` discharges it, so main's
        // effective row is empty and ADR 0019 D13 is satisfied.
        let (_, errors) = check_program(
            "fn double(x: i64) -> i64 ! { Async } { x * 2 }\n\
             fn main() -> i64 { scope concurrent { let t = spawn double(21); t.await } }",
        )
        .unwrap();
        assert!(errors.is_empty(), "got {errors:?}");
    }

    #[test]
    fn spawn_outside_scope_bubbles_async_to_main() {
        // No discharging `scope` — Async bubbles to main and ADR
        // 0019 D13 rejects. This is what makes the Async marker
        // load-bearing per ADR 0024 D5.
        let (_, errors) = check_program(
            "fn double(x: i64) -> i64 ! { Async } { x * 2 }\n\
             fn main() -> i64 { let t = spawn double(21); t.await }",
        )
        .unwrap();
        assert!(
            errors.iter().any(|e| matches!(e, EffectError::UnhandledEffect { effect_name, .. } if effect_name == "Async")),
            "got {errors:?}"
        );
    }
}
