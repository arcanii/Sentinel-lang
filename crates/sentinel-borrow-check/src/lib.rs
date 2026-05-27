//! sentinel-borrow-check
//!
//! Lexical borrow checker for Sentinel per ADR 0017 D6. C2.1 ships
//! the **shared-only** subset — `&T` only, no `&mut`, no move
//! semantics, no drop. The remaining sub-phases per ADR 0017's
//! D9 sub-phase split:
//!
//!   - C2.2 — `&mut T` + the shared-XOR-mutable rule.
//!   - C2.3 — move semantics + use-after-move.
//!   - C2.4 — RAII / drop + `sentinel_free` runtime symbol.
//!   - C2.5 — Polonius migration plan + ADR 0017 → ACCEPTED.
//!
//! The pipeline at C2.1 becomes:
//!
//! ```text
//! parse_query → resolve_query → check_query → borrow_check_query → codegen
//! ```
//!
//! Per ADR 0017 D6's "lexical formulation" — a borrow's lifetime
//! is from creation to the end of the enclosing scope. The check
//! is bounded ~few-hundred LOC; rejected programs all have a
//! polite local workaround (introduce a wider scope; bind by-value
//! instead of borrowing). Polonius / NLL precision is the C2.5
//! migration target per D6's "lexical first" call.
//!
//! ## What C2.1 checks
//!
//! 1. **Use-after-scope** — `let r = { let inner = 5; &inner };
//!    *r` rejected at `*r` because `inner`'s scope has ended.
//! 2. **Ref escapes via return** — `fn f() -> &i64 { let x = 5;
//!    &x }` rejected because `x` is fn-local and dies at return.
//!    Per ADR 0017 D7 "second-class refs everywhere", the only
//!    sound returnable refs come from incoming `&T` params.
//!
//! ## What C2.1 does NOT check
//!
//! - Multiple `&T` borrows of the same place — these are fine
//!   under shared-only rules. (XOR with `&mut` ships at C2.2.)
//! - Use-after-move — bindings are still implicitly `Copy` at
//!   C2.1. Move semantics ship at C2.3.
//! - Drop / RAII — heap allocations from C1.6+ still leak.
//!   Closes at C2.4 per ADR 0017 D8.
//!
//! ## Borrow-source representation
//!
//! Each ref-typed binding gets a [`BorrowSource`]:
//!
//!   - [`BorrowSource::Local`] — the ref points to a binding
//!     declared in this fn (`let x = ...;` or by-value param).
//!     Source dies when the declaring scope exits; cannot escape
//!     via return.
//!   - [`BorrowSource::Incoming`] — the ref came in via a `&T`
//!     param. Source lives in the caller's scope; always alive
//!     within this fn body; can escape via return.
//!   - [`BorrowSource::LocalAnonymous`] — fallback for fn-call
//!     results where no ref arg contributes a source. At C2.1
//!     this only fires if a fn returns a ref without any ref
//!     args, which would itself fail borrow-check — so this
//!     variant is mostly defensive.
//!
//! The check is **per-fn** and uses no inter-procedural reasoning
//! beyond "a fn-call returning a ref has source = most-restrictive
//! of its ref-arg sources". Each generic-fn instance is checked
//! once at the abstract definition; monomorphic copies (C1.7.5)
//! don't need re-checking because TypeParam substitution preserves
//! the ref structure that borrow-check analysed.

use std::collections::HashMap;

use salsa::Accumulator;
use sentinel_ast::{Span, UnaryOp};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_resolve::VarId;
use sentinel_types::{
    TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedProgram, TypedStmt, TypedStmtKind,
};

// =============================================================================
// Errors
// =============================================================================

/// C2.1 borrow-check error variants. The shared-only subset
/// surfaces exactly two categories — block-scoped use-after-scope
/// and fn-return ref-to-local. C2.2 + C2.3 + C2.4 will add more
/// variants as `&mut` / move / drop semantics arrive.
#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum BorrowError {
    /// A reference is read at a point where its ultimate source
    /// binding has gone out of scope. Covers the canonical
    /// "borrow lives past last use" lexical-borrow-check failure:
    ///
    /// ```text
    /// let r: &i64 = {
    ///     let inner: i64 = 5;
    ///     &inner
    /// };
    /// *r  // ERROR: borrow of `inner` outlives its source
    /// ```
    #[error("borrow of `{source_name}` outlives its source")]
    #[diagnostic(
        code(sentinel::borrow::outlives_source),
        help("`{source_name}` is declared in a narrower scope than the reference; widen the source's scope or take the reference at a wider scope")
    )]
    OutlivesSource {
        source_name: String,
        #[label("source binding here")]
        source_span: miette::SourceSpan,
        #[label("borrow used here")]
        use_span: miette::SourceSpan,
    },

    /// A function returns a reference whose source is a fn-local
    /// binding (a `let` or a by-value param) — both die at fn
    /// return, leaving the returned ref dangling. Per ADR 0017 D7
    /// "second-class refs everywhere", the only sound returnable
    /// refs come from incoming `&T` params (or transitively from
    /// fn calls that received them).
    #[error("function `{fn_name}` returns a reference to local `{source_name}`")]
    #[diagnostic(
        code(sentinel::borrow::returns_local_ref),
        help("local bindings (`let` or by-value params) die at function return; return a copy by value, or thread an existing `&T` from the caller through")
    )]
    ReturnsLocalRef {
        fn_name: String,
        source_name: String,
        #[label("source binding here")]
        source_span: miette::SourceSpan,
        #[label("returned here")]
        return_span: miette::SourceSpan,
    },
}

// =============================================================================
// Internal analysis state
// =============================================================================

/// What a ref-typed binding ultimately points to, for liveness
/// purposes. See module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorrowSource {
    /// Tied to a specific binding in this fn (let or by-value
    /// param). The VarId lets us look up the source's name + span
    /// when emitting diagnostics. Source dies when its declaring
    /// scope exits; cannot escape via return.
    Local(VarId),
    /// Tied to caller's scope via an incoming `&T` param. Always
    /// alive in this fn; can escape via return.
    Incoming,
    /// Fallback for fn-call returns with no attributable arg
    /// source. Treated like Local for lifetime purposes.
    LocalAnonymous,
}

/// Per-fn analysis context. Reset for each fn body.
struct FnCtx {
    /// For every binding declared in this fn (param or let), its
    /// source name + span for diagnostics. Survives scope pops —
    /// we may need the name later for the fn-return check.
    var_info: HashMap<VarId, VarInfo>,
    /// For every ref-typed binding, what does it point to? Updated
    /// at declaration time + on `*` deref-assignment and var
    /// re-assignment of ref-typed bindings.
    ref_source: HashMap<VarId, BorrowSource>,
    /// Stack of scopes; each scope is the list of VarIds
    /// declared in it. Popping a scope removes those VarIds from
    /// [`var_in_scope`].
    scopes: Vec<Vec<VarId>>,
    /// Live bindings — VarIds whose declaring scope hasn't been
    /// popped. Queried by [`FnCtx::is_alive`] for the use-after-
    /// scope check.
    var_in_scope: HashMap<VarId, ()>,
}

#[derive(Debug, Clone)]
struct VarInfo {
    name: String,
    span: Span,
}

impl FnCtx {
    fn new() -> Self {
        Self {
            var_info: HashMap::new(),
            ref_source: HashMap::new(),
            scopes: Vec::new(),
            var_in_scope: HashMap::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(popped) = self.scopes.pop() {
            for id in popped {
                self.var_in_scope.remove(&id);
            }
        }
    }

    /// Register a binding (param or let) in the current scope.
    fn declare(&mut self, id: VarId, name: String, span: Span) {
        if let Some(top) = self.scopes.last_mut() {
            top.push(id);
        }
        self.var_in_scope.insert(id, ());
        self.var_info.insert(id, VarInfo { name, span });
    }

    /// `true` if the binding referenced by `source` is still in
    /// scope at this point. Per ADR 0017 D6's lexical formulation.
    fn is_alive(&self, source: BorrowSource) -> bool {
        match source {
            BorrowSource::Local(id) => self.var_in_scope.contains_key(&id),
            BorrowSource::Incoming => true,
            // Anonymous: pessimistically "alive within the fn body"
            // — the check that matters for this variant is the fn-
            // return check (it can't escape via return).
            BorrowSource::LocalAnonymous => true,
        }
    }
}

// =============================================================================
// Entry point + per-fn walk
// =============================================================================

/// Borrow-check a [`TypedProgram`]. Returns an empty Vec if every
/// fn passes; otherwise the accumulated errors (one or more per
/// failing fn). Per ADR 0017 D6, the analysis is *lexical*: each
/// borrow's lifetime extends from creation to the end of its
/// enclosing block.
///
/// At C2.1 the check is **per-fn** and shared-only. Inter-procedural
/// reasoning is limited to "a call returning a ref inherits the
/// most-restrictive source among its ref args" — sufficient for
/// the no-`&mut` subset.
pub fn borrow_check(program: &TypedProgram) -> Vec<BorrowError> {
    let mut errors = Vec::new();
    for fn_def in &program.fns {
        borrow_check_fn(fn_def, program, &mut errors);
    }
    errors
}

fn borrow_check_fn(
    fn_def: &TypedFnDef,
    program: &TypedProgram,
    errors: &mut Vec<BorrowError>,
) {
    let mut ctx = FnCtx::new();
    // Register params at "depth 0" — they're alive for the whole
    // fn body. By-value params die at return (Local source for
    // any `&x` taken on them); incoming ref params have Incoming
    // source (the caller owns the underlying place).
    for param in &fn_def.params {
        ctx.declare(param.id, param.name.clone(), param.span.clone());
        if param.ty.is_ref() {
            ctx.ref_source.insert(param.id, BorrowSource::Incoming);
        }
    }

    // Walk the body. Inner Block expressions push/pop their own
    // scopes via [`walk_expr`]'s [`TypedExprKind::Block`] arm.
    walk_block_contents(&fn_def.body, &mut ctx, errors, program);

    // ADR 0017 D7's "second-class refs everywhere" check: if the
    // fn returns a ref, the tail's source must be Incoming. We
    // compute source_of_expr on the (still-walked) tail; var_info
    // persists across scope pops so we can name the offending
    // source binding in the diagnostic.
    if fn_def.return_type.is_ref() {
        let tail_source = source_of_expr(&fn_def.body.tail, &ctx, program);
        match tail_source {
            Some(BorrowSource::Incoming) | None => {}
            Some(BorrowSource::Local(src_id)) => {
                let info = ctx
                    .var_info
                    .get(&src_id)
                    .cloned()
                    .unwrap_or(VarInfo { name: "<unknown>".into(), span: 0..0 });
                errors.push(BorrowError::ReturnsLocalRef {
                    fn_name: fn_def.name.clone(),
                    source_name: info.name,
                    source_span: to_source_span(&info.span),
                    return_span: to_source_span(&fn_def.body.tail.span),
                });
            }
            Some(BorrowSource::LocalAnonymous) => {
                errors.push(BorrowError::ReturnsLocalRef {
                    fn_name: fn_def.name.clone(),
                    source_name: "<anonymous>".to_string(),
                    source_span: to_source_span(&fn_def.body.tail.span),
                    return_span: to_source_span(&fn_def.body.tail.span),
                });
            }
        }
    }
}

/// Walk a block's statements + tail in the **current** scope
/// (the caller manages push/pop). Used for the fn body where
/// params already occupy the outer scope; inner `Block`
/// expressions go through [`walk_expr`] which pushes/pops.
fn walk_block_contents(
    block: &TypedBlock,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    for stmt in &block.stmts {
        walk_stmt(stmt, ctx, errors, program);
    }
    walk_expr(&block.tail, ctx, errors, program);
}

fn walk_stmt(
    stmt: &TypedStmt,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    match &stmt.kind {
        TypedStmtKind::Let { id, name, name_span, ty, value, .. } => {
            walk_expr(value, ctx, errors, program);
            // Record the source if this binding holds a ref. Done
            // BEFORE declaring `id` so `let r = r;` (self-ref RHS)
            // wouldn't see itself — though such a program would
            // already fail resolve.
            if ty.is_ref() {
                if let Some(source) = source_of_expr(value, ctx, program) {
                    ctx.ref_source.insert(*id, source);
                }
            }
            ctx.declare(*id, name.clone(), name_span.clone());
        }
        TypedStmtKind::Assign { target, value } => {
            walk_expr(target, ctx, errors, program);
            walk_expr(value, ctx, errors, program);
            // If the assignment target is a ref-typed Var, update
            // its recorded source — re-assignment shifts which
            // place the ref points to.
            if target.ty.is_ref() {
                if let TypedExprKind::Var(id) = &target.kind {
                    if let Some(source) = source_of_expr(value, ctx, program) {
                        ctx.ref_source.insert(*id, source);
                    }
                }
            }
        }
        TypedStmtKind::Expr(e) => walk_expr(e, ctx, errors, program),
    }
}

fn walk_expr(
    expr: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    match &expr.kind {
        // Leaves — no children.
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit => {}

        // Ref-typed Var reads trigger the use-after-scope check.
        // Non-ref Vars don't — their values were copied at bind
        // time per Sentinel's implicit-Copy semantics through C2.2.
        TypedExprKind::Var(id) => {
            if expr.ty.is_ref() {
                if let Some(source) = ctx.ref_source.get(id).copied() {
                    if !ctx.is_alive(source) {
                        emit_outlives(ctx, errors, source, &expr.span);
                    }
                }
            }
        }

        TypedExprKind::WidenToNullable(inner) => {
            walk_expr(inner, ctx, errors, program);
        }

        TypedExprKind::Unary(_, inner) => {
            // Borrow-take / deref / neg / not: walk the inner.
            // The borrow-take itself doesn't *use* the source,
            // so no liveness check at this point — the check
            // fires later when the resulting ref is *read*.
            walk_expr(inner, ctx, errors, program);
        }

        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_expr(l, ctx, errors, program);
            walk_expr(r, ctx, errors, program);
        }

        TypedExprKind::Block(b) => {
            ctx.push_scope();
            walk_block_contents(b, ctx, errors, program);
            ctx.pop_scope();
        }

        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_expr(cond, ctx, errors, program);
            ctx.push_scope();
            walk_block_contents(then_branch, ctx, errors, program);
            ctx.pop_scope();
            ctx.push_scope();
            walk_block_contents(else_branch, ctx, errors, program);
            ctx.pop_scope();
        }

        TypedExprKind::Call { args, .. } => {
            for arg in args {
                walk_expr(arg, ctx, errors, program);
            }
        }

        TypedExprKind::StructLit { fields, .. } => {
            for fv in fields {
                walk_expr(fv, ctx, errors, program);
            }
        }

        TypedExprKind::FieldAccess { target, .. } => {
            walk_expr(target, ctx, errors, program);
        }

        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_expr(el, ctx, errors, program);
            }
        }

        TypedExprKind::Index { target, index, .. } => {
            walk_expr(target, ctx, errors, program);
            walk_expr(index, ctx, errors, program);
        }
    }
}

/// Compute the [`BorrowSource`] of a ref-typed expression. Used
/// at let-RHS and tail-expr sites. Returns `None` for non-ref
/// expressions (which should never happen if the caller already
/// gated on `ty.is_ref()`).
fn source_of_expr(
    expr: &TypedExpr,
    ctx: &FnCtx,
    program: &TypedProgram,
) -> Option<BorrowSource> {
    match &expr.kind {
        TypedExprKind::Unary(UnaryOp::Ref, inner)
        | TypedExprKind::Unary(UnaryOp::RefMut, inner) => {
            source_of_lvalue(inner, ctx, program)
        }
        TypedExprKind::Var(id) => ctx.ref_source.get(id).copied(),
        TypedExprKind::Block(b) => source_of_expr(&b.tail, ctx, program),
        TypedExprKind::If { then_branch, else_branch, .. } => {
            // Both branches contribute; merge to the most
            // restrictive (Local wins over Incoming).
            let t = source_of_expr(&then_branch.tail, ctx, program);
            let e = source_of_expr(&else_branch.tail, ctx, program);
            merge_sources(t, e)
        }
        TypedExprKind::Call { args, .. } => {
            // Conservative inter-procedural rule: the result ref
            // inherits the most-restrictive source among the
            // call's ref args. If no ref args contribute, return
            // LocalAnonymous — the call must have constructed a
            // ref out of thin air, which can only borrow-check
            // if it's actually a Local of some inaccessible
            // scope. Either way, not escapable via return.
            let mut acc: Option<BorrowSource> = None;
            for arg in args {
                if arg.ty.is_ref() {
                    let arg_source = source_of_expr(arg, ctx, program);
                    acc = merge_sources(acc, arg_source);
                }
            }
            acc.or(Some(BorrowSource::LocalAnonymous))
        }
        _ => None,
    }
}

/// Compute the [`BorrowSource`] of an lvalue (the operand of `&`
/// / `&mut`). The lvalue's source is the binding the lvalue
/// ultimately denotes a place inside.
fn source_of_lvalue(
    expr: &TypedExpr,
    ctx: &FnCtx,
    program: &TypedProgram,
) -> Option<BorrowSource> {
    match &expr.kind {
        TypedExprKind::Var(id) => {
            // If this Var holds a ref, taking its address
            // (`&r`) would give `&&T` which is rejected at type-
            // check. So we only reach here for non-ref Vars —
            // their source is just themselves (Local).
            if ctx.ref_source.contains_key(id) {
                // Defensive: `&` of a ref-typed Var; surface as
                // the underlying source so diagnostics chain.
                ctx.ref_source.get(id).copied()
            } else {
                Some(BorrowSource::Local(*id))
            }
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => {
            // `& *r` — the source is r's source (the underlying
            // place r points to).
            source_of_expr(inner, ctx, program)
        }
        TypedExprKind::FieldAccess { target, .. } => {
            source_of_lvalue(target, ctx, program)
        }
        TypedExprKind::Index { target, .. } => {
            source_of_lvalue(target, ctx, program)
        }
        _ => None,
    }
}

/// Merge two optional borrow sources — Local wins over Incoming
/// over None.
fn merge_sources(
    a: Option<BorrowSource>,
    b: Option<BorrowSource>,
) -> Option<BorrowSource> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(x), Some(y)) => Some(merge(x, y)),
    }
}

fn merge(a: BorrowSource, b: BorrowSource) -> BorrowSource {
    match (a, b) {
        (BorrowSource::Local(_), _) => a,
        (_, BorrowSource::Local(_)) => b,
        (BorrowSource::LocalAnonymous, _) => a,
        (_, BorrowSource::LocalAnonymous) => b,
        (BorrowSource::Incoming, BorrowSource::Incoming) => BorrowSource::Incoming,
    }
}

fn emit_outlives(
    ctx: &FnCtx,
    errors: &mut Vec<BorrowError>,
    source: BorrowSource,
    use_span: &Span,
) {
    let (source_name, source_span) = match source {
        BorrowSource::Local(id) => {
            let info = ctx
                .var_info
                .get(&id)
                .cloned()
                .unwrap_or(VarInfo { name: "<unknown>".into(), span: 0..0 });
            (info.name, info.span)
        }
        BorrowSource::LocalAnonymous => ("<anonymous>".to_string(), use_span.clone()),
        BorrowSource::Incoming => return, // Incoming is always alive; no error
    };
    errors.push(BorrowError::OutlivesSource {
        source_name,
        source_span: to_source_span(&source_span),
        use_span: to_source_span(use_span),
    });
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

// =============================================================================
// Salsa-tracked query
// =============================================================================

/// Salsa-tracked borrow-check query. Chains on
/// [`sentinel_types::check_query`]; runs the borrow analysis on
/// the typed program and accumulates [`BorrowError`] diagnostics.
/// Returns `Some(())` if the program borrow-checks; `None`
/// otherwise. Diagnostics from upstream queries (parse / resolve
/// / types) flow through transitively when callers ask for
/// `borrow_check_query::accumulated::<Diagnostic>`.
#[salsa::tracked(return_ref)]
pub fn borrow_check_query(db: &dyn SentinelDb, file: SourceFile) -> Option<()> {
    let typed = sentinel_types::check_query(db, file).as_ref()?;
    let errors = borrow_check(typed);
    if errors.is_empty() {
        Some(())
    } else {
        for err in &errors {
            borrow_error_to_diagnostic(err).accumulate(db);
        }
        None
    }
}

fn borrow_error_to_diagnostic(err: &BorrowError) -> Diagnostic {
    let (code, message, span): (&'static str, String, std::ops::Range<usize>) = match err {
        BorrowError::OutlivesSource { source_name, use_span, .. } => (
            "sentinel::borrow::outlives_source",
            format!("borrow of `{source_name}` outlives its source"),
            use_span.offset()..(use_span.offset() + use_span.len()),
        ),
        BorrowError::ReturnsLocalRef { fn_name, source_name, return_span, .. } => (
            "sentinel::borrow::returns_local_ref",
            format!("function `{fn_name}` returns a reference to local `{source_name}`"),
            return_span.offset()..(return_span.offset() + return_span.len()),
        ),
    };
    Diagnostic {
        stage: "borrow",
        severity: Severity::Error,
        code,
        message,
        span,
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-borrow-check"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_resolve::resolve;
    use sentinel_syntax::parse;
    use sentinel_types::check;

    fn borrow_check_ok(src: &str) {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let errors = borrow_check(&typed);
        assert!(errors.is_empty(), "expected no borrow errors, got {errors:?}");
    }

    fn borrow_check_err(src: &str) -> Vec<BorrowError> {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let errors = borrow_check(&typed);
        assert!(!errors.is_empty(), "expected borrow errors, got none");
        errors
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-borrow-check");
    }

    // ----- Positive paths: programs that should borrow-check -----

    #[test]
    fn no_refs_no_errors() {
        borrow_check_ok("fn main() -> i64 { let x: i64 = 5; x + 1 }");
    }

    #[test]
    fn shared_ref_to_local_within_scope_ok() {
        // `&x` consumed within x's scope — fine.
        borrow_check_ok(
            "fn main() -> i64 { let x: i64 = 5; let r: &i64 = &x; *r }",
        );
    }

    #[test]
    fn add_with_two_shared_refs_ok() {
        // Multiple `&T` borrows are allowed under shared-only rules.
        borrow_check_ok(
            "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn main() -> i64 { let a: i64 = 10; let b: i64 = 32; add(&a, &b) }",
        );
    }

    #[test]
    fn passing_incoming_ref_through_ok() {
        // `fn pass(r: &i64) -> &i64 { r }` — returning an incoming
        // ref is sound; its source is the caller's scope.
        borrow_check_ok(
            "fn pass(r: &i64) -> &i64 { r }\nfn main() -> i64 { let x: i64 = 7; *pass(&x) }",
        );
    }

    #[test]
    fn deref_then_borrow_through_incoming_ok() {
        // `& *r` is the canonical reborrow shape. Its source is
        // r's source (Incoming) — escapable via return.
        borrow_check_ok(
            "fn pass(r: &i64) -> &i64 { &*r }\nfn main() -> i64 { let x: i64 = 7; *pass(&x) }",
        );
    }

    #[test]
    fn c20_go_no_go_borrow_checks() {
        // The C2.0.2 phase-go program: shared+exclusive borrows,
        // let mut, deref-assign, print. Must still borrow-check
        // at C2.1.
        borrow_check_ok(
            "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn increment(x: &mut i64) -> i64 { let new_val: i64 = *x + 1; *x = new_val; *x }\nfn main() -> i64 { let mut a: i64 = 10; let b: i64 = 32; let sum: i64 = add(&a, &b); let inc: i64 = increment(&mut a); print(sum + inc) }",
        );
    }

    #[test]
    fn ref_in_inner_block_used_within_block_ok() {
        // `{ let inner = 5; let r = &inner; *r }` — r used while
        // inner is still alive. Fine.
        borrow_check_ok(
            "fn main() -> i64 { { let inner: i64 = 5; let r: &i64 = &inner; *r } }",
        );
    }

    // ----- Negative paths: programs that should fail -----

    #[test]
    fn returns_local_ref_rejected() {
        // `fn f() -> &i64 { let x = 5; &x }` — x dies at fn end.
        let errs = borrow_check_err(
            "fn f() -> &i64 { let x: i64 = 5; &x }\nfn main() -> i64 { *f() }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { fn_name, .. } if fn_name == "f"),
            "got {errs:?}"
        );
    }

    #[test]
    fn returns_ref_to_by_value_param_rejected() {
        // `fn f(x: i64) -> &i64 { &x }` — by-value param dies at
        // return too.
        let errs = borrow_check_err(
            "fn f(x: i64) -> &i64 { &x }\nfn main() -> i64 { let y: i64 = 5; *f(y) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { fn_name, .. } if fn_name == "f"),
            "got {errs:?}"
        );
    }

    #[test]
    fn use_after_inner_scope_rejected() {
        // `let r = { let inner = 5; &inner }; *r` — inner is dead
        // by the time r is dereferenced.
        let errs = borrow_check_err(
            "fn main() -> i64 { let r: &i64 = { let inner: i64 = 5; &inner }; *r }",
        );
        assert!(
            matches!(&errs[0], BorrowError::OutlivesSource { source_name, .. } if source_name == "inner"),
            "got {errs:?}"
        );
    }

    #[test]
    fn return_local_via_call_chain_rejected() {
        // `fn make() -> &i64 { let x = 5; &x }` — even though x
        // is local, this surfaces as ReturnsLocalRef.
        let errs = borrow_check_err(
            "fn make() -> &i64 { let x: i64 = 5; &x }\nfn main() -> i64 { *make() }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { .. }),
            "got {errs:?}"
        );
    }

    // ----- Salsa query smoke -----

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {
        fn salsa_event(&self, _event: &dyn Fn() -> salsa::Event) {}
    }

    #[salsa::db]
    impl SentinelDb for TestDb {}

    #[test]
    fn borrow_check_query_succeeds_for_valid_source() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { 42 }".to_string(),
        );
        let result = borrow_check_query(&db, file);
        assert_eq!(result.as_ref(), Some(&()));
        let diags = borrow_check_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn borrow_check_query_emits_diagnostic_on_error() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn f() -> &i64 { let x: i64 = 5; &x }\nfn main() -> i64 { *f() }".to_string(),
        );
        let result = borrow_check_query(&db, file);
        assert_eq!(result.as_ref(), None);
        let diags = borrow_check_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "borrow");
        assert_eq!(diags[0].code, "sentinel::borrow::returns_local_ref");
    }

    #[test]
    fn borrow_check_query_propagates_type_diagnostic() {
        // Type-check failure → borrow_check_query short-circuits;
        // the upstream check_query diagnostic flows through.
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> bogus { 0 }".to_string(),
        );
        let result = borrow_check_query(&db, file);
        assert_eq!(result.as_ref(), None);
        let diags = borrow_check_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "types");
    }
}
