//! sentinel-borrow-check
//!
//! Lexical borrow checker for Sentinel per ADR 0017 D6 + D9. C2.1
//! shipped the **shared-only** borrow subset (`&T` only); C2.2
//! added `&mut T` and the **shared-XOR-mutable** rule with
//! place-tracking + transient/rooted borrow lifetimes; C2.3 adds
//! **move semantics** + **use-after-move detection** with
//! branch-aware merging at if/else. The remaining sub-phases per
//! ADR 0017's D9 sub-phase split:
//!
//!   - ✅ C2.1 — shared-only lexical borrow checker (`&T` only).
//!   - ✅ C2.2 — `&mut T` + the shared-XOR-mutable rule.
//!   - ✅ C2.3 — move semantics + use-after-move (this
//!     sub-phase).
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
//! ## What C2.1 + C2.2 + C2.3 check
//!
//! 1. **Use-after-scope** (C2.1) — `let r = { let inner = 5;
//!    &inner }; *r` rejected at `*r` because `inner`'s scope has
//!    ended.
//! 2. **Ref escapes via return** (C2.1) — `fn f() -> &i64 { let
//!    x = 5; &x }` rejected because `x` is fn-local and dies at
//!    return. Per ADR 0017 D7 "second-class refs everywhere", the
//!    only sound returnable refs come from incoming `&T` params.
//! 3. **Shared-XOR-mutable** (C2.2) — at any program point, a
//!    place is either (a) free of borrows, (b) has N ≥ 1 shared
//!    `&T` borrows active, or (c) has exactly one `&mut T` borrow
//!    active. Mixing a `&T` and `&mut T` of the same place is
//!    rejected.
//! 4. **Write while borrowed** (C2.2) — direct assignment `x =
//!    v;` while any `&x` or `&mut x` is active is rejected. The
//!    owner can't write while the binding is borrowed.
//! 5. **Read while exclusively borrowed** (C2.2) — reading a
//!    binding (e.g., `print(x)`, or passing `x` by value) while
//!    `&mut x` is active is rejected.
//! 6. **Use-after-move** (C2.3) — reading a Move-classified
//!    binding after it has been consumed (passed by value to a
//!    fn, re-assigned into another binding, used in a non-lvalue
//!    expression). The first such read marks the binding `Moved`;
//!    any subsequent read surfaces `UseAfterMove`. Per-binding
//!    move-state lives in `FnCtx.moved`; if-then-else branches
//!    each get an isolated walk with conservative merge ("moved
//!    in either branch → moved after").
//!
//! ## Type classification at C2.3
//!
//! `is_copy_type(ty) -> bool` partitions types:
//!   - **Copy**: `i64`, `i32`, `bool`, `&T`, `&mut T`, `?T` where
//!     T is Copy.
//!   - **Move**: structs, arrays `[T]`, nullables `?T` where T is
//!     Move, generic-struct instances, `TypeParam` (conservative —
//!     concrete substitution may be Copy but borrow-check is per-
//!     definition and uses the abstract form).
//!
//! Reads in non-lvalue, non-postfix-receiver context CONSUME the
//! binding when its type is Move. Postfix receivers
//! (`p.field` / `xs[i]` — the target side) read the binding
//! WITHOUT consuming; this matches the existing codegen
//! (`build_extract_value` copies the field). Lvalue contexts
//! (`& x`, `&mut x`, LHS of assignment) also don't consume.
//!
//! ## What this checker does NOT catch at C2.5 close
//!
//! See `docs/borrow-check-limitations.md` for reproducers + closure
//! plans. Summary:
//!
//! - **Over-rejection: borrow lives past last use** — the canonical
//!   NLL case. Workaround: wrap the borrow + use in an inner block.
//!   Closes via the ADR 0018 Polonius migration.
//! - **Over-rejection: field-disjoint borrows** — `&p.x` blocks all
//!   mutation of `p` under C2.2's binding-precise place tracking.
//!   Polonius-style field-precise borrows are a post-Polonius ADR.
//! - **Soundness gap: partial move + drop ⇒ double-free.**
//!   Postfix `.field` on a Move-typed binding is non-consuming
//!   (per the `p.x + p.y` common shape). With drop shipping at
//!   C2.4, passing `p.items` by value to a fn that drops it
//!   causes a double-free at main's drop. C2.3's docstring noted
//!   "benign at C2.3"; no longer benign at C2.5. Closure: a
//!   follow-on sub-phase (C2.6 or ADR 0019) adds per-
//!   (VarId, FieldPath) move state. Until it lands, programs that
//!   pass Move-typed struct fields by value to a drop-eligible
//!   consumer are unsound. Highest-priority post-C2.5 work on
//!   this side.
//!
//! ## Borrow-source representation
//!
//! Each ref-typed binding gets a [`BorrowSource`]:
//!
//!   - [`BorrowSource::Local(VarId)`] — the ref points to a
//!     binding declared in this fn (`let x = ...;` or by-value
//!     param). Source dies when the declaring scope exits; cannot
//!     escape via return.
//!   - [`BorrowSource::Incoming(VarId)`] — the ref came in via a
//!     `&T` param. The VarId is the param itself (used as the
//!     place-key for C2.2's XOR tracking). Source lives in the
//!     caller's scope; always alive within this fn body; can
//!     escape via return.
//!   - [`BorrowSource::LocalAnonymous`] — fallback for fn-call
//!     results where no ref arg contributes a source. Treated
//!     like Local for lifetime purposes; gets no XOR tracking.
//!
//! C2.2 adds per-place active-borrow tracking: each `&x` /
//! `&mut x` site records a [`BorrowInstance`] in `FnCtx.places`
//! keyed by the source's VarId. Borrows have a
//! [`BorrowLifetime`] tag:
//!
//!   - [`BorrowLifetime::Transient`] — added during an expression
//!     evaluation but not yet rooted in a binding. Cleared at the
//!     end of the containing statement.
//!   - [`BorrowLifetime::UntilScope(depth)`] — promoted to live
//!     until the scope at `depth` pops. Set when the borrow gets
//!     bound to a `let r: &T = ...` (or ref-typed `let mut`).
//!
//! The check is **per-fn** and uses no inter-procedural reasoning
//! beyond "a fn-call returning a ref has source = most-restrictive
//! of its ref-arg sources". Each generic-fn instance is checked
//! once at the abstract definition; monomorphic copies (C1.7.5)
//! don't need re-checking because TypeParam substitution preserves
//! the ref structure that borrow-check analysed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use salsa::Accumulator;
use sentinel_ast::{Span, UnaryOp};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_resolve::{FnId, VarId};
use sentinel_types::{
    NullableInner, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedProgram,
    TypedStmt, TypedStmtKind,
};

// =============================================================================
// Errors
// =============================================================================

/// Borrow-check error variants. C2.1 shipped the shared-only
/// pair (OutlivesSource / ReturnsLocalRef); C2.2 adds the
/// shared-XOR-mutable family + the write-while-borrowed +
/// read-while-mutably-borrowed checks. C2.3 + C2.4 will add
/// `UseAfterMove` and drop-related variants.
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

    /// A `&mut T` borrow was attempted while a `&T` (shared)
    /// borrow of the same place is still active. The shared-XOR-
    /// mutable rule per ADR 0017 D6 — shared borrows ARE allowed
    /// to coexist with each other, but adding `&mut` requires
    /// exclusive access.
    #[error("cannot take `&mut {place_name}` while it is already borrowed shared")]
    #[diagnostic(
        code(sentinel::borrow::mutable_borrow_of_shared),
        help("shared (`&T`) borrows must die before `&mut T` can be taken; introduce a new scope to bound the shared borrows")
    )]
    MutableBorrowOfShared {
        place_name: String,
        #[label("existing shared borrow")]
        prior_borrow_span: miette::SourceSpan,
        #[label("attempted mutable borrow here")]
        attempt_span: miette::SourceSpan,
    },

    /// A `&T` (shared) borrow was attempted while a `&mut T`
    /// borrow of the same place is still active. Same rule —
    /// the exclusive borrow excludes ALL other borrows.
    #[error("cannot take `&{place_name}` while it is already borrowed mutably")]
    #[diagnostic(
        code(sentinel::borrow::shared_borrow_of_mutable),
        help("the `&mut T` borrow must die before any other borrow can be taken; introduce a new scope to bound the mutable borrow")
    )]
    SharedBorrowOfMutable {
        place_name: String,
        #[label("existing mutable borrow")]
        prior_borrow_span: miette::SourceSpan,
        #[label("attempted shared borrow here")]
        attempt_span: miette::SourceSpan,
    },

    /// A `&mut T` borrow was attempted while another `&mut T`
    /// of the same place is still active. The shared-XOR-mutable
    /// rule per ADR 0017 D6's strict interpretation — at most one
    /// `&mut T` to a place at any time.
    #[error("cannot take `&mut {place_name}` while it is already borrowed mutably")]
    #[diagnostic(
        code(sentinel::borrow::borrow_conflict),
        help("only one `&mut T` to a place can exist at a time; introduce a new scope to bound the prior mutable borrow")
    )]
    BorrowConflict {
        place_name: String,
        #[label("existing mutable borrow")]
        prior_borrow_span: miette::SourceSpan,
        #[label("conflicting mutable borrow here")]
        attempt_span: miette::SourceSpan,
    },

    /// A direct write to a binding (`x = v;`) while one or more
    /// borrows of that binding are still active. The owner can't
    /// mutate the place while it's borrowed — would invalidate
    /// the borrowers' view.
    #[error("cannot assign to `{place_name}` while it is borrowed")]
    #[diagnostic(
        code(sentinel::borrow::write_while_borrowed),
        help("the binding's borrow must die before the owner can write; introduce a new scope to bound the borrow")
    )]
    WriteWhileBorrowed {
        place_name: String,
        #[label("existing borrow")]
        prior_borrow_span: miette::SourceSpan,
        #[label("attempted write here")]
        attempt_span: miette::SourceSpan,
    },

    /// A read of a binding while a `&mut T` of it is active.
    /// Reading from the owner while exclusively borrowed
    /// violates the exclusivity invariant — the `&mut T` holder
    /// has the only legal read/write path during its lifetime.
    #[error("cannot read `{place_name}` while it is borrowed mutably")]
    #[diagnostic(
        code(sentinel::borrow::read_while_mut_borrowed),
        help("the `&mut T` borrow must die before the owner can read; introduce a new scope to bound the mutable borrow")
    )]
    ReadWhileMutBorrowed {
        place_name: String,
        #[label("existing mutable borrow")]
        prior_borrow_span: miette::SourceSpan,
        #[label("attempted read here")]
        attempt_span: miette::SourceSpan,
    },

    /// A Move-classified binding (struct, array, generic-instance,
    /// nullable-of-non-Copy, or TypeParam) is read after it has
    /// already been consumed — passed by value to a fn, re-bound
    /// into another `let`, or used in a non-lvalue expression
    /// elsewhere. Per ADR 0017 D9.
    #[error("use of moved binding `{binding_name}`")]
    #[diagnostic(
        code(sentinel::borrow::use_after_move),
        help("`{binding_name}` was consumed by an earlier use; either bind via reference (`&{binding_name}`) at the call site, or duplicate the value (e.g., reconstruct the struct)")
    )]
    UseAfterMove {
        binding_name: String,
        #[label("binding declared here")]
        decl_span: miette::SourceSpan,
        #[label("moved here")]
        move_span: miette::SourceSpan,
        #[label("used here after move")]
        use_span: miette::SourceSpan,
    },

    /// Phase D.5 / ADR 0036 D8: a Move-classified binding declared
    /// OUTSIDE a `while` loop is moved INSIDE its condition or body.
    /// The borrow checker walks the body once, but the loop runs
    /// repeatedly — so the move is a use-after-move on the *next*
    /// iteration. Rejected conservatively; a binding declared *inside*
    /// the body is fresh each iteration and may be moved freely.
    #[error("cannot move out of `{binding_name}` inside a `while` loop")]
    #[diagnostic(
        code(sentinel::borrow::moved_in_loop_body),
        help("`{binding_name}` is declared outside the loop, so moving it leaves it consumed on the next iteration; move a binding declared inside the body, or borrow (`&{binding_name}`) instead")
    )]
    MovedInLoopBody {
        binding_name: String,
        #[label("`{binding_name}` declared here, outside the loop")]
        decl_span: miette::SourceSpan,
        #[label("moved here inside the loop")]
        move_span: miette::SourceSpan,
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
    /// Tied to caller's scope via an incoming `&T` param. The
    /// VarId is the *param* itself (used as the place-key for
    /// C2.2's XOR tracking — conflicts on derived refs route
    /// through this param's place). Always alive in this fn; can
    /// escape via return.
    Incoming(VarId),
    /// Fallback for fn-call returns with no attributable arg
    /// source. Treated like Local for lifetime purposes; no place-
    /// key, so no XOR tracking applies.
    LocalAnonymous,
}

impl BorrowSource {
    /// The VarId that serves as the place-key for C2.2 borrow
    /// tracking, if any. `LocalAnonymous` returns None.
    fn place_key(self) -> Option<VarId> {
        match self {
            BorrowSource::Local(id) | BorrowSource::Incoming(id) => Some(id),
            BorrowSource::LocalAnonymous => None,
        }
    }
}

/// When a borrow expires. Set at borrow creation and possibly
/// promoted at the containing statement's rooting moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorrowLifetime {
    /// Lives until the end of the containing statement. Default
    /// for borrows in expression position; cleared by
    /// [`FnCtx::clear_transients`] at every statement boundary.
    Transient,
    /// Lives until the scope at the given depth pops. Set when
    /// a borrow is rooted in a ref-typed `let r = &x;` (or
    /// equivalent assignment).
    UntilScope(usize),
}

/// A single active borrow record. Carries enough info to surface
/// a clean diagnostic when a later borrow / write / read attempts
/// to conflict with it. The borrow's kind (shared vs. mut) is
/// implicit in where it's stored — [`PlaceState::shared`] for
/// shared, [`PlaceState::mut_borrow`] for exclusive.
#[derive(Debug, Clone)]
struct BorrowInstance {
    /// Source span of the `&` / `&mut` expression that created
    /// this borrow — used as the "prior borrow" label.
    span: Span,
    lifetime: BorrowLifetime,
}

/// Per-place active-borrow state. Per ADR 0017 D6's shared-XOR-
/// mutable rule: at most one mut borrow, OR any number of shared
/// borrows.
#[derive(Debug, Default, Clone)]
struct PlaceState {
    shared: Vec<BorrowInstance>,
    mut_borrow: Option<BorrowInstance>,
}

impl PlaceState {
    fn has_mut(&self) -> Option<&BorrowInstance> {
        self.mut_borrow.as_ref()
    }

    fn first_shared(&self) -> Option<&BorrowInstance> {
        self.shared.first()
    }
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
    /// C2.2: per-place active-borrow tracking keyed by source
    /// VarId. Borrows are added at `&x` / `&mut x` sites and
    /// expire either at statement-end (transient) or at scope
    /// pop (rooted). See [`BorrowLifetime`].
    places: HashMap<VarId, PlaceState>,
    /// C2.3: per-binding move state. Absent = `Live`. Present
    /// (with the span of the consuming use) = `Moved`. A read in
    /// a consuming context when the binding is already Moved
    /// surfaces [`BorrowError::UseAfterMove`]; otherwise a read
    /// in a consuming context of a Move-classified binding
    /// transitions it to Moved. Reset by if/else snapshot/restore
    /// for branch-aware merging.
    moved: HashMap<VarId, Span>,
    /// C2.4: union of ALL bindings that were ever moved during
    /// this fn's analysis (even across branches that got
    /// snapshot/restored from [`moved`]). Codegen uses this set
    /// via [`DropPlan`] to skip dropping moved-from bindings.
    /// Never reset by snapshot/restore — always growing within
    /// a single fn analysis.
    moved_sources_union: HashSet<VarId>,
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
            places: HashMap::new(),
            moved: HashMap::new(),
            moved_sources_union: HashSet::new(),
        }
    }

    fn current_depth(&self) -> usize {
        self.scopes.len().saturating_sub(1)
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let popped_depth = self.scopes.len().saturating_sub(1);
        if let Some(popped) = self.scopes.pop() {
            for id in popped {
                self.var_in_scope.remove(&id);
            }
        }
        // C2.2: remove borrows that expire when this scope ends.
        // Borrows with `UntilScope(popped_depth)` lifetime die here.
        for state in self.places.values_mut() {
            state.shared.retain(|b| {
                !matches!(b.lifetime, BorrowLifetime::UntilScope(d) if d == popped_depth)
            });
            if let Some(b) = &state.mut_borrow {
                if matches!(b.lifetime, BorrowLifetime::UntilScope(d) if d == popped_depth)
                {
                    state.mut_borrow = None;
                }
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
            BorrowSource::Incoming(_) => true,
            // Anonymous: pessimistically "alive within the fn body"
            // — the check that matters for this variant is the fn-
            // return check (it can't escape via return).
            BorrowSource::LocalAnonymous => true,
        }
    }

    /// C2.2: promote every transient borrow to the given scope
    /// depth. Called at ref-typed `let r = ...;` (or equivalent
    /// assign) — borrows from the RHS get rooted in r's scope.
    /// Over-conservative when the RHS has multiple borrows of
    /// which only some flow to r (e.g., `let r = { foo(&y); &x }`
    /// would promote both); the cost is rejecting some valid
    /// programs, never accepting unsound ones.
    fn promote_transients(&mut self, depth: usize) {
        for state in self.places.values_mut() {
            for b in &mut state.shared {
                if matches!(b.lifetime, BorrowLifetime::Transient) {
                    b.lifetime = BorrowLifetime::UntilScope(depth);
                }
            }
            if let Some(b) = &mut state.mut_borrow {
                if matches!(b.lifetime, BorrowLifetime::Transient) {
                    b.lifetime = BorrowLifetime::UntilScope(depth);
                }
            }
        }
    }

    /// C2.2: remove all transient borrows. Called at every
    /// statement boundary; rooted borrows (those promoted to a
    /// scope by [`promote_transients`]) survive.
    fn clear_transients(&mut self) {
        for state in self.places.values_mut() {
            state
                .shared
                .retain(|b| !matches!(b.lifetime, BorrowLifetime::Transient));
            if let Some(b) = &state.mut_borrow {
                if matches!(b.lifetime, BorrowLifetime::Transient) {
                    state.mut_borrow = None;
                }
            }
        }
    }
}

// =============================================================================
// Drop plan — codegen artifact (C2.4 / ADR 0017 D8)
// =============================================================================

/// Per-fn metadata produced by [`borrow_check`] for codegen to
/// consume when emitting drop calls at scope-exit per ADR 0017 D8.
/// At C2.4 the plan carries just the **moved-source set** — the
/// VarIds that act as the source of a move somewhere in their
/// fn's body. Codegen uses this to skip dropping moved-from
/// bindings (the destination owns the value).
///
/// Future C2.5+ may extend this to track per-scope drop sites
/// explicitly (e.g., for non-block scope boundaries like `if`
/// arms that take ownership) — for C2.4 the per-fn set is
/// sufficient because every let-binding lives in some block and
/// codegen emits drops at every block-exit.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct DropPlan {
    /// VarIds that are sources of moves somewhere in their fn's
    /// body. Conservative — a VarId moved in *either* branch of
    /// an `if/else` is included. Stored as `BTreeMap`/`BTreeSet`
    /// rather than `HashMap`/`HashSet` so the plan implements
    /// `Hash + Eq` for salsa-tracked caching.
    pub moved_sources: BTreeMap<FnId, BTreeSet<VarId>>,
}

impl DropPlan {
    /// Look up the moved-source set for a fn. Returns an empty
    /// set if the fn has no recorded moves.
    pub fn moved_sources_for(&self, fn_id: FnId) -> &BTreeSet<VarId> {
        static EMPTY: std::sync::OnceLock<BTreeSet<VarId>> = std::sync::OnceLock::new();
        self.moved_sources
            .get(&fn_id)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }
}

// =============================================================================
// Entry point + per-fn walk
// =============================================================================

/// Borrow-check a [`TypedProgram`]. Returns a [`DropPlan`] (drop
/// info for codegen) and a vector of errors. Empty errors = the
/// program borrow-checks; non-empty = at least one fn failed and
/// codegen must NOT proceed (the DropPlan is still computed
/// best-effort for partial diagnostics, but isn't sound to use).
///
/// Per ADR 0017 D6, the analysis is *lexical*: each borrow's
/// lifetime extends from creation to the end of its enclosing
/// block. At C2.4 the check is **per-fn** with branch-aware move-
/// state merging at if/else (per D9).
pub fn borrow_check(program: &TypedProgram) -> (DropPlan, Vec<BorrowError>) {
    let mut errors = Vec::new();
    let mut drop_plan = DropPlan::default();
    for fn_def in &program.fns {
        borrow_check_fn(fn_def, program, &mut errors, &mut drop_plan);
    }
    (drop_plan, errors)
}

fn borrow_check_fn(
    fn_def: &TypedFnDef,
    program: &TypedProgram,
    errors: &mut Vec<BorrowError>,
    drop_plan: &mut DropPlan,
) {
    let mut ctx = FnCtx::new();
    // Register params at "depth 0" — they're alive for the whole
    // fn body. By-value params die at return (Local source for
    // any `&x` taken on them); incoming ref params have Incoming
    // source (the caller owns the underlying place).
    for param in &fn_def.params {
        ctx.declare(param.id, param.name.clone(), param.span.clone());
        if param.ty.is_ref() {
            ctx.ref_source
                .insert(param.id, BorrowSource::Incoming(param.id));
        }
    }

    // Walk the body. Inner Block expressions push/pop their own
    // scopes via [`walk_expr`]'s [`TypedExprKind::Block`] arm.
    walk_block_contents(&fn_def.body, &mut ctx, errors, program);

    // C2.4: record the per-fn move-sources union into the
    // DropPlan before the return-source check (which may move
    // the tail's source, e.g. `fn f() -> Pair { p }`).
    let mut moved_btree = BTreeSet::new();
    moved_btree.extend(ctx.moved_sources_union.iter().copied());
    drop_plan.moved_sources.insert(fn_def.id, moved_btree);

    // ADR 0017 D7's "second-class refs everywhere" check: if the
    // fn returns a ref, the tail's source must be Incoming. We
    // compute source_of_expr on the (still-walked) tail; var_info
    // persists across scope pops so we can name the offending
    // source binding in the diagnostic.
    if fn_def.return_type.is_ref() {
        let tail_source = source_of_expr(&fn_def.body.tail, &ctx, program);
        match tail_source {
            Some(BorrowSource::Incoming(_)) | None => {}
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
                // C2.2: promote any transient borrows created by
                // the RHS to live until the *current* scope pops
                // — they're now rooted in `id`, which lives at
                // the current scope's depth.
                ctx.promote_transients(ctx.current_depth());
            }
            ctx.declare(*id, name.clone(), name_span.clone());
            ctx.clear_transients();
        }
        TypedStmtKind::Assign { target, value } => {
            // C2.2: walk the value first so its borrows are
            // visible; then check the target for write-conflicts
            // before recording the assignment.
            walk_expr(value, ctx, errors, program);
            walk_assign_target(target, ctx, errors, program);
            // If the assignment target is a ref-typed Var, update
            // its recorded source — re-assignment shifts which
            // place the ref points to. Same transient promotion
            // as the ref-typed Let path.
            if target.ty.is_ref() {
                if let TypedExprKind::Var(id) = &target.kind {
                    if let Some(source) = source_of_expr(value, ctx, program) {
                        ctx.ref_source.insert(*id, source);
                    }
                }
                ctx.promote_transients(ctx.current_depth());
            }
            ctx.clear_transients();
        }
        TypedStmtKind::While { cond, body } => {
            // Phase D.5 / ADR 0036 D8: the loop-carried move rule. The
            // condition + body run repeatedly, but the borrow checker
            // walks them once. Snapshot the in-scope (outer) bindings +
            // the already-moved set; after walking, any OUTER binding
            // newly moved in the cond/body is a use-after-move on the
            // next iteration — reject it (`MovedInLoopBody`). A binding
            // declared INSIDE the body is fresh each iteration (fine).
            let outer_vars: std::collections::HashSet<VarId> =
                ctx.var_in_scope.keys().copied().collect();
            let moved_before: std::collections::HashSet<VarId> =
                ctx.moved.keys().copied().collect();

            // The condition is evaluated each iteration; its transient
            // borrows die before the body runs.
            walk_expr(cond, ctx, errors, program);
            ctx.clear_transients();

            // The body is its own scope (per-iteration bindings).
            ctx.push_scope();
            walk_block_contents(body, ctx, errors, program);
            ctx.pop_scope();

            // Flag outer bindings newly moved in the cond/body
            // (deterministic order for stable diagnostics).
            let mut carried: Vec<(VarId, Span)> = ctx
                .moved
                .iter()
                .filter(|(id, _)| !moved_before.contains(id) && outer_vars.contains(id))
                .map(|(id, span)| (*id, span.clone()))
                .collect();
            carried.sort_by_key(|(id, _)| id.0);
            for (id, move_span) in carried {
                let decl_span = ctx
                    .var_info
                    .get(&id)
                    .map(|vi| vi.span.clone())
                    .unwrap_or_else(|| move_span.clone());
                errors.push(BorrowError::MovedInLoopBody {
                    binding_name: place_name(ctx, id),
                    decl_span: to_source_span(&decl_span),
                    move_span: to_source_span(&move_span),
                });
            }
            ctx.clear_transients();
        }
        TypedStmtKind::Expr(e) => {
            walk_expr(e, ctx, errors, program);
            ctx.clear_transients();
        }
    }
}

/// Walk the LHS of an assignment statement. The target is an
/// lvalue — we DON'T trigger a read-check at the Var leaf (we're
/// writing, not reading) but we DO trigger a write-conflict check
/// against any active borrows of the place.
fn walk_assign_target(
    target: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    match &target.kind {
        TypedExprKind::Var(id) => {
            // Direct write to a binding. C2.2: error if any
            // borrow of this place is active.
            check_write_conflict(*id, &target.span, ctx, errors);
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => {
            // `*r = v;` — the write goes through r (a `&mut T`
            // by type-check invariant). Walk r as a normal
            // expression so its read-check fires (use-after-scope
            // for r itself); the write to r's pointee is sound
            // because XOR already ensured r is the only active
            // borrow of its source.
            walk_expr(inner, ctx, errors, program);
        }
        TypedExprKind::FieldAccess { target: inner_target, .. } => {
            // `p.field = v;` — recurse. The eventual Var leaf
            // triggers the write-conflict on p.
            walk_assign_target(inner_target, ctx, errors, program);
        }
        // `a[i] = v;` is rejected at type-check (per ADR 0017 D12
        // / TypeError::IndexAssignNotSupported); we won't reach
        // it here. Fall through to a normal walk for defensiveness.
        _ => walk_expr(target, ctx, errors, program),
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
        | TypedExprKind::NullLit
        // D.2 / ADR 0033: char/string literals have no sub-expressions
        // (the bytes are literal — no variable is read/moved). A string's
        // owned `[u8]` is dropped via its binding's type, like any array.
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_) => {}

        // Ref-typed Var reads trigger the C2.1 use-after-scope
        // check. Non-ref Var reads trigger the C2.2 read-while-
        // mutably-borrowed check AND the C2.3 use-after-move
        // check + consume.
        TypedExprKind::Var(id) => {
            if expr.ty.is_ref() {
                if let Some(source) = ctx.ref_source.get(id).copied() {
                    if !ctx.is_alive(source) {
                        emit_outlives(ctx, errors, source, &expr.span);
                    }
                }
            } else {
                // C2.2: reading a non-ref binding while a `&mut T`
                // of it is active violates exclusivity.
                check_read_conflict(*id, &expr.span, ctx, errors);
                // C2.3: check use-after-move + consume if the
                // type is Move-classified. This is a CONSUMING
                // context (Var in expr position, not postfix
                // receiver / not lvalue).
                check_and_record_move(*id, expr.ty, &expr.span, ctx, errors, program);
            }
        }

        TypedExprKind::WidenToNullable(inner) => {
            walk_expr(inner, ctx, errors, program);
        }

        // C3 / ADR 0019 D5+D6 (C3.1): the new typed wrappers
        // (`secret(T)` widen + declassify strip) don't change move/
        // borrow semantics — they're purely type-level. Walk the
        // inner and let normal moves/borrows flow through.
        TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Declassify(inner) => {
            walk_expr(inner, ctx, errors, program);
        }

        TypedExprKind::Unary(UnaryOp::Ref, inner) => {
            // C2.2: walk the inner lvalue (no read-check on its
            // leaves) then attempt to add a shared borrow.
            walk_expr_lvalue(inner, ctx, errors, program);
            if let Some(source) = source_of_lvalue(inner, ctx, program) {
                check_and_add_shared_borrow(source, &expr.span, ctx, errors);
            }
        }
        TypedExprKind::Unary(UnaryOp::RefMut, inner) => {
            // C2.2: same as Ref but for exclusive borrows.
            walk_expr_lvalue(inner, ctx, errors, program);
            if let Some(source) = source_of_lvalue(inner, ctx, program) {
                check_and_add_mut_borrow(source, &expr.span, ctx, errors);
            }
        }
        TypedExprKind::Unary(_, inner) => {
            // Deref / Neg / Not: walk the inner. Deref through a
            // ref-typed Var triggers the C2.1 OutlivesSource
            // check on r; the inner value's `*r` doesn't create
            // a new borrow.
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
            // C2.3: snapshot the move-state, walk each branch in
            // isolation, then merge — "moved in either branch →
            // moved after". This is what makes `if c { fst(p) }
            // else { snd(p) }` accept: each branch independently
            // moves p, but the merge sees p as Moved after, which
            // is fine when no further uses follow.
            let snapshot_moved = ctx.moved.clone();
            ctx.push_scope();
            walk_block_contents(then_branch, ctx, errors, program);
            ctx.pop_scope();
            let then_moved = std::mem::replace(&mut ctx.moved, snapshot_moved);
            ctx.push_scope();
            walk_block_contents(else_branch, ctx, errors, program);
            ctx.pop_scope();
            // Merge: any binding moved in the then-branch but
            // not in the else-branch is conservatively Moved
            // after (we can't statically know which branch ran).
            for (id, span) in then_moved {
                ctx.moved.entry(id).or_insert(span);
            }
        }

        TypedExprKind::Call { id, args, .. } => {
            // C2.3: runtime builtins (`print`, `unwrap_or`,
            // `is_some`, `len`) semantically take their args by
            // reference — the inline LLVM lowering reads but
            // doesn't consume. Treating their args as
            // non-consuming keeps c15_maybe_compose
            // (`is_some(x) ... unwrap_or(x, ...)`) and
            // c16_go_no_go (`len(a)` in cond, then `a[i]` /
            // `sum_from(a, ...)` in branches) valid under move
            // semantics. User-defined fns walk normally —
            // pass-by-value moves their args. Future ADRs may
            // add real `&T` builtin signatures + trait-bounded
            // generics to retire this special case.
            let signature = program.signature(*id);
            if signature.is_runtime {
                for arg in args {
                    match &arg.kind {
                        // D.3 / ADR 0034 D6: a `&mut`/`&` reference
                        // argument to a runtime builtin (e.g. the
                        // `&mut v` of `push(&mut v, x)`) IS a borrow and
                        // must register through the normal Ref/RefMut
                        // path — so a `push` participates in the
                        // shared-XOR-mutable rule (mutable borrow of v).
                        // This extends the ADR 0033 A3 "builtin args are
                        // borrowed, not consumed" rule: an explicit
                        // reference arg is a borrow (mutable for `&mut`),
                        // not a non-consuming by-value read. No existing
                        // builtin takes a reference arg, so this only
                        // affects `push`.
                        TypedExprKind::Unary(UnaryOp::Ref, _)
                        | TypedExprKind::Unary(UnaryOp::RefMut, _) => {
                            walk_expr(arg, ctx, errors, program);
                        }
                        // Every other runtime-builtin arg (e.g. `len(a)`,
                        // `str_eq(a, b)`, and `push`'s by-value element
                        // `x`) stays a non-consuming lvalue read. NB: a
                        // Move-typed `push` element (Vec<Struct>) is
                        // actually consumed; that's deferred with
                        // droppable-element Vecs (ADR 0034 D8), and the
                        // MVP's primitive elements are Copy.
                        _ => walk_expr_lvalue(arg, ctx, errors, program),
                    }
                }
            } else {
                for arg in args {
                    walk_expr(arg, ctx, errors, program);
                }
            }
        }

        TypedExprKind::StructLit { fields, .. } => {
            for fv in fields {
                walk_expr(fv, ctx, errors, program);
            }
        }

        TypedExprKind::FieldAccess { target, .. } => {
            // C2.3: postfix receiver — non-consuming. The target
            // is read for projection but not moved. Use
            // walk_expr_lvalue which checks liveness without
            // consuming.
            walk_expr_lvalue(target, ctx, errors, program);
        }

        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_expr(el, ctx, errors, program);
            }
        }

        TypedExprKind::Index { target, index, .. } => {
            // C2.3: same as FieldAccess — postfix receiver is
            // non-consuming. The index is a regular expression
            // (consuming read).
            walk_expr_lvalue(target, ctx, errors, program);
            walk_expr(index, ctx, errors, program);
        }

        // C3.4 / ADR 0020 D5: handle/perform/resume don't reach
        // codegen at C3.4 minimum — borrow-checking them is a
        // best-effort recursive walk so any borrow violations
        // inside the bodies surface, but the kont binding itself
        // is treated as Copy-ish (no move tracking for it). The
        // proper handler-aware borrow check lands alongside
        // codegen at C3.5/C3.6.
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            walk_expr(body, ctx, errors, program);
            for arm in arms {
                walk_expr(&arm.body, ctx, errors, program);
            }
            if let Some(ra) = return_arm {
                walk_expr(&ra.body, ctx, errors, program);
            }
        }
        TypedExprKind::Perform { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // C4.1 / ADR 0022 D3 + D7: method call. The receiver is
        // walked as an lvalue — the auto-ref produces `&target` /
        // `&mut target` at codegen, which is a non-consuming read
        // (refs are Copy). Same treatment as FieldAccess + Index.
        // Without this, repeated method calls on a Move-typed
        // receiver (e.g., two `s.write(...)`-style calls on a
        // class instance) would surface use-after-move spuriously.
        TypedExprKind::MethodCall { target, args, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // C4.1 / ADR 0022 D5: `Name::init(args)` is purely a
        // value-producing expression — walk its args.
        TypedExprKind::ClassInit { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // C4.2 / ADR 0023 D5 Path 1: receiver-typed dispatch. The
        // receiver is non-consuming (auto-ref produces a borrow),
        // mirroring the class MethodCall arm above.
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // C4.2 / ADR 0023 D5 Path 2: args includes the receiver.
        TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // C4.4 / ADR 0024: `scope concurrent { ... }` is a nested
        // block — push/pop a scope and walk its contents.
        TypedExprKind::Scope { body, .. } => {
            ctx.push_scope();
            walk_block_contents(body, ctx, errors, program);
            ctx.pop_scope();
        }
        // `spawn fn(args)` walks the inner call; its args are moved
        // into the task (spawned fns take owned args per D10), so
        // the normal Call walk (consuming for user fns) is correct.
        TypedExprKind::Spawn { call, .. } => {
            walk_expr(call, ctx, errors, program);
        }
        // `task.await` reads the Task receiver. Task is Copy
        // (is_copy_type), so this is a non-consuming liveness read.
        TypedExprKind::Await { task_expr, .. } => {
            walk_expr(task_expr, ctx, errors, program);
        }
        // Phase D.1 / ADR 0032 (3/N): variant construction moves its
        // payload args into the new enum value — a consuming walk,
        // like a struct literal / user-fn call.
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
            }
        }
        // Phase D.1 / ADR 0032 (3/N): `match` reads the scrutinee
        // (its tag + payload projections — a non-consuming read at
        // the MVP, like a postfix receiver) and runs exactly one arm.
        // Mirror the `if` branch-merge: walk each arm in isolation
        // from a shared move snapshot, then union their moved sets
        // (moved in any arm → conservatively moved after). Pattern
        // bindings are fresh arm-scoped VarIds; the move maps treat
        // an unseen VarId as live, so they need no pre-registration.
        TypedExprKind::Match { scrutinee, arms, .. } => {
            walk_expr_lvalue(scrutinee, ctx, errors, program);
            let snapshot_moved = ctx.moved.clone();
            let mut union_moved = snapshot_moved.clone();
            for arm in arms {
                ctx.moved = snapshot_moved.clone();
                ctx.push_scope();
                walk_expr(&arm.body, ctx, errors, program);
                ctx.pop_scope();
                for (id, span) in std::mem::take(&mut ctx.moved) {
                    union_moved.entry(id).or_insert(span);
                }
            }
            ctx.moved = union_moved;
        }
    }
}

/// Walk an lvalue expression — `& x` / `&mut x` / assignment
/// targets. Differs from [`walk_expr`] in that Var leaves don't
/// trigger the read-while-mutably-borrowed check (we're taking
/// the address, not reading the value).
fn walk_expr_lvalue(
    expr: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    match &expr.kind {
        TypedExprKind::Var(id) => {
            // C2.3: lvalue Var still needs the use-after-move
            // liveness check (so `let _ = consume(p); p.field`
            // is rejected at the `p.field` site). No CONSUME
            // here — postfix projection is non-destructive.
            check_use_alive(*id, &expr.span, ctx, errors);
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => {
            // `& *r` — walk r as a normal expression so its
            // OutlivesSource check fires.
            walk_expr(inner, ctx, errors, program);
        }
        TypedExprKind::FieldAccess { target, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
        }
        TypedExprKind::Index { target, index, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
            walk_expr(index, ctx, errors, program);
        }
        // Other shapes shouldn't appear here (type-check would
        // have rejected). Fall through to the normal walk for
        // defensiveness.
        _ => walk_expr(expr, ctx, errors, program),
    }
}

/// C2.2: attempt to add a shared borrow at the source's place-
/// key. If a `&mut T` is already active, emit
/// `SharedBorrowOfMutable`. Otherwise record the borrow as
/// transient (the containing statement may promote it later).
fn check_and_add_shared_borrow(
    source: BorrowSource,
    span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    let Some(place) = source.place_key() else {
        return; // LocalAnonymous — no place-key, no tracking
    };
    let state = ctx.places.entry(place).or_default();
    if let Some(mut_borrow) = state.has_mut() {
        let prior_span = mut_borrow.span.clone();
        let name = place_name(ctx, place);
        errors.push(BorrowError::SharedBorrowOfMutable {
            place_name: name,
            prior_borrow_span: to_source_span(&prior_span),
            attempt_span: to_source_span(span),
        });
        return;
    }
    let state = ctx.places.entry(place).or_default();
    state.shared.push(BorrowInstance {
        span: span.clone(),
        lifetime: BorrowLifetime::Transient,
    });
}

/// C2.2: attempt to add a `&mut T` borrow at the source's place-
/// key. Either fires `BorrowConflict` (existing `&mut`) or
/// `MutableBorrowOfShared` (existing `&`s), or records as
/// transient on success.
fn check_and_add_mut_borrow(
    source: BorrowSource,
    span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    let Some(place) = source.place_key() else {
        return;
    };
    let state = ctx.places.entry(place).or_default();
    if let Some(mut_borrow) = state.has_mut() {
        let prior_span = mut_borrow.span.clone();
        let name = place_name(ctx, place);
        errors.push(BorrowError::BorrowConflict {
            place_name: name,
            prior_borrow_span: to_source_span(&prior_span),
            attempt_span: to_source_span(span),
        });
        return;
    }
    if let Some(shared) = state.first_shared() {
        let prior_span = shared.span.clone();
        let name = place_name(ctx, place);
        errors.push(BorrowError::MutableBorrowOfShared {
            place_name: name,
            prior_borrow_span: to_source_span(&prior_span),
            attempt_span: to_source_span(span),
        });
        return;
    }
    let state = ctx.places.entry(place).or_default();
    state.mut_borrow = Some(BorrowInstance {
        span: span.clone(),
        lifetime: BorrowLifetime::Transient,
    });
}

/// C2.2: writing through `x = v;` is rejected if ANY borrow of
/// the place is currently active. The owner can't mutate a
/// borrowed binding.
fn check_write_conflict(
    place: VarId,
    span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    let state = ctx.places.entry(place).or_default();
    let prior = state
        .mut_borrow
        .as_ref()
        .map(|b| b.span.clone())
        .or_else(|| state.shared.first().map(|b| b.span.clone()));
    if let Some(prior_span) = prior {
        let name = place_name(ctx, place);
        errors.push(BorrowError::WriteWhileBorrowed {
            place_name: name,
            prior_borrow_span: to_source_span(&prior_span),
            attempt_span: to_source_span(span),
        });
    }
}

/// C2.2: reading a non-ref binding while a `&mut T` of it is
/// active is rejected (exclusivity violation).
fn check_read_conflict(
    place: VarId,
    span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    let state = ctx.places.entry(place).or_default();
    if let Some(mut_borrow) = state.has_mut() {
        let prior_span = mut_borrow.span.clone();
        let name = place_name(ctx, place);
        errors.push(BorrowError::ReadWhileMutBorrowed {
            place_name: name,
            prior_borrow_span: to_source_span(&prior_span),
            attempt_span: to_source_span(span),
        });
    }
}

/// Look up a place's source-level name for diagnostics. Falls
/// back to a `var<N>` placeholder if the VarId isn't in
/// `var_info` (shouldn't happen for any VarId reachable here).
fn place_name(ctx: &FnCtx, id: VarId) -> String {
    ctx.var_info
        .get(&id)
        .map(|info| info.name.clone())
        .unwrap_or_else(|| format!("var{}", id.0))
}

/// C2.3: classify a [`Type`] as Copy (free to duplicate at
/// reads) or Move (consumed on read). Per ADR 0017 D9:
///
///   - **Copy**: primitives (i64, i32, bool), references (&T,
///     &mut T), and nullables `?T` where the inner is itself
///     Copy.
///   - **Move**: structs, arrays `[T]`, generic-struct instances,
///     nullables of non-Copy types, and `TypeParam` (conservative
///     — concrete substitution may be Copy but borrow-check is
///     per-definition).
///
/// The C2.3 implementation deliberately punts on partial-move
/// tracking (field-disjoint moves) per the module-doc rationale.
fn is_copy_type(ty: Type, program: &TypedProgram) -> bool {
    match ty {
        // Phase D.2 / ADR 0033 D4: `u8` is a 1-byte Copy scalar (a
        // `[u8]` string is `Type::Array(_)` below → Move, owning a heap
        // buffer like any array).
        Type::I64 | Type::I32 | Type::U8 | Type::Bool | Type::Ref(_) => true,
        Type::Nullable(inner) => is_copy_nullable_inner(inner),
        Type::Struct(_)
        | Type::Array(_)
        // Phase D.3 / ADR 0034 D3: a `Vec<T>` owns its heap buffer, so
        // it is Move (consumed on read), exactly like an array `[T]`.
        | Type::Vec(_)
        | Type::GenericInstance(_)
        | Type::TypeParam(_)
        | Type::Class(_)
        // Phase D.1 / ADR 0032 D6: an enum owns its heap-boxed
        // payload, so it is Move (consumed on read), like a struct.
        | Type::Enum(_)
        // C4.2 / ADR 0023 D7: `Self` inside a trait method sig is
        // abstract — borrow-check conservatively treats it as Move
        // until impl-sig substitution resolves it (which happens
        // before impl-method bodies are borrow-checked, so this arm
        // is effectively unreachable in practice).
        | Type::TraitSelf(_) => false,
        // C3 / ADR 0019 D5: Copy-ness of `secret T` follows the
        // inner type — `secret i64` is Copy; `secret Bag` is Move.
        // The secret qualifier is orthogonal to ownership;
        // borrow-check unwraps it before deciding.
        Type::Secret(id) => is_copy_type(program.secret_data(id).inner, program),
        // C3.4 / ADR 0020 D5: konts are pointer-like (they reference
        // heap-allocated frames at C3.5+) and never carry move
        // semantics — they live and die inside their arm's scope.
        // Treating them as Copy avoids spurious move tracking; the
        // kont VarId is never reassigned or referenced as a value
        // (KontUsedAsValue rejects that at type-check).
        Type::Kont(_) => true,
        // C4.4 / ADR 0024: a Task handle is pointer-like + reclaimed
        // by the runtime (await / scope_exit), not by codegen drop.
        // Treat it as Copy to avoid spurious move tracking — double-
        // await is idempotent-safe at the runtime (the `owned` flag).
        Type::Task(_) => true,
    }
}

fn is_copy_nullable_inner(inner: NullableInner) -> bool {
    match inner {
        NullableInner::I64 | NullableInner::I32 | NullableInner::Bool => true,
        NullableInner::Ref(_) => true,
        NullableInner::Struct(_)
        | NullableInner::GenericInstance(_)
        | NullableInner::TypeParam(_) => false,
    }
}

/// C2.3: at a Var(id) read in CONSUMING context, fire the move
/// check. If the binding is already Moved, surface UseAfterMove
/// pointing at the prior move site + this use. If the binding is
/// Live and Move-classified, transition to Moved. C2.4: also
/// record into [`FnCtx::moved_sources_union`] for the DropPlan.
fn check_and_record_move(
    id: VarId,
    ty: Type,
    use_span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    if let Some(move_span) = ctx.moved.get(&id).cloned() {
        emit_use_after_move(ctx, errors, id, &move_span, use_span);
        return;
    }
    if !is_copy_type(ty, program) {
        ctx.moved.insert(id, use_span.clone());
        ctx.moved_sources_union.insert(id);
    }
}

/// C2.3: at a Var(id) read in NON-CONSUMING context (postfix
/// receiver, lvalue), check only that the binding hasn't already
/// been moved. Doesn't transition to Moved.
fn check_use_alive(
    id: VarId,
    use_span: &Span,
    ctx: &FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    if let Some(move_span) = ctx.moved.get(&id).cloned() {
        emit_use_after_move(ctx, errors, id, &move_span, use_span);
    }
}

fn emit_use_after_move(
    ctx: &FnCtx,
    errors: &mut Vec<BorrowError>,
    id: VarId,
    move_span: &Span,
    use_span: &Span,
) {
    let info = ctx.var_info.get(&id).cloned().unwrap_or(VarInfo {
        name: format!("var{}", id.0),
        span: 0..0,
    });
    errors.push(BorrowError::UseAfterMove {
        binding_name: info.name,
        decl_span: to_source_span(&info.span),
        move_span: to_source_span(move_span),
        use_span: to_source_span(use_span),
    });
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
        (BorrowSource::Incoming(_), BorrowSource::Incoming(_)) => a,
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
        BorrowSource::Incoming(_) => return, // Incoming is always alive; no error
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
/// Returns `Some(DropPlan)` if the program borrow-checks (codegen
/// consumes the plan to emit drop calls at scope-exit per ADR
/// 0017 D8); `None` if any errors fired. Diagnostics from upstream
/// queries (parse / resolve / types) flow through transitively
/// when callers ask for `borrow_check_query::accumulated::<Diagnostic>`.
///
/// C3 / ADR 0019 D11 (C3.2): chains on `effect_check_query` (which
/// itself chains on `check_query`), so EffectError diagnostics
/// surface in the borrow_check_query's transitive accumulated set.
/// If effect-check fails, borrow-check short-circuits.
#[salsa::tracked(return_ref)]
pub fn borrow_check_query(db: &dyn SentinelDb, file: SourceFile) -> Option<DropPlan> {
    // C3.2: ensure effect-check has passed (and accumulated any
    // diagnostics) before we do borrow-check. The effect_check
    // pass itself depends on check_query, so this transitively
    // covers parse + resolve + check + effect_check upstream of
    // borrow-check.
    sentinel_effect_check::effect_check_query(db, file).as_ref()?;
    let typed = sentinel_types::check_query(db, file).as_ref()?;
    let (drop_plan, errors) = borrow_check(typed);
    if errors.is_empty() {
        Some(drop_plan)
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
        BorrowError::MutableBorrowOfShared { place_name, attempt_span, .. } => (
            "sentinel::borrow::mutable_borrow_of_shared",
            format!(
                "cannot take `&mut {place_name}` while it is already borrowed shared"
            ),
            attempt_span.offset()..(attempt_span.offset() + attempt_span.len()),
        ),
        BorrowError::SharedBorrowOfMutable { place_name, attempt_span, .. } => (
            "sentinel::borrow::shared_borrow_of_mutable",
            format!(
                "cannot take `&{place_name}` while it is already borrowed mutably"
            ),
            attempt_span.offset()..(attempt_span.offset() + attempt_span.len()),
        ),
        BorrowError::BorrowConflict { place_name, attempt_span, .. } => (
            "sentinel::borrow::borrow_conflict",
            format!(
                "cannot take `&mut {place_name}` while it is already borrowed mutably"
            ),
            attempt_span.offset()..(attempt_span.offset() + attempt_span.len()),
        ),
        BorrowError::WriteWhileBorrowed { place_name, attempt_span, .. } => (
            "sentinel::borrow::write_while_borrowed",
            format!("cannot assign to `{place_name}` while it is borrowed"),
            attempt_span.offset()..(attempt_span.offset() + attempt_span.len()),
        ),
        BorrowError::ReadWhileMutBorrowed { place_name, attempt_span, .. } => (
            "sentinel::borrow::read_while_mut_borrowed",
            format!("cannot read `{place_name}` while it is borrowed mutably"),
            attempt_span.offset()..(attempt_span.offset() + attempt_span.len()),
        ),
        BorrowError::UseAfterMove { binding_name, use_span, .. } => (
            "sentinel::borrow::use_after_move",
            format!("use of moved binding `{binding_name}`"),
            use_span.offset()..(use_span.offset() + use_span.len()),
        ),
        BorrowError::MovedInLoopBody { binding_name, move_span, .. } => (
            "sentinel::borrow::moved_in_loop_body",
            format!("cannot move out of `{binding_name}` inside a `while` loop"),
            move_span.offset()..(move_span.offset() + move_span.len()),
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
        let (_plan, errors) = borrow_check(&typed);
        assert!(errors.is_empty(), "expected no borrow errors, got {errors:?}");
    }

    fn borrow_check_err(src: &str) -> Vec<BorrowError> {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let (_plan, errors) = borrow_check(&typed);
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

    // ----- C2.2 positive: shared-XOR-mutable accepts -----

    #[test]
    fn multiple_shared_borrows_ok() {
        // Multiple `&x` borrows of the same place coexist fine.
        borrow_check_ok(
            "fn main() -> i64 { let x: i64 = 5; let r1: &i64 = &x; let r2: &i64 = &x; *r1 + *r2 }",
        );
    }

    #[test]
    fn single_mut_borrow_with_use_ok() {
        // One `&mut x` lifetime + deref-write through it. No
        // other access to x while the mut borrow is alive.
        borrow_check_ok(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &mut i64 = &mut x; *r = 9; *r }",
        );
    }

    #[test]
    fn mut_in_inner_scope_then_shared_outside_ok() {
        // The `&mut x` borrow dies with the inner block; the
        // subsequent direct read of x is in a fresh borrow-state.
        borrow_check_ok(
            "fn main() -> i64 { let mut x: i64 = 5; { let r: &mut i64 = &mut x; *r = 7; *r }; x }",
        );
    }

    #[test]
    fn shared_then_mut_in_separate_blocks_ok() {
        // Shared borrows die with their block; subsequent &mut
        // is fine.
        borrow_check_ok(
            "fn main() -> i64 { let mut x: i64 = 5; { let r: &i64 = &x; *r }; { let r2: &mut i64 = &mut x; *r2 = 9; *r2 } }",
        );
    }

    #[test]
    fn transient_borrows_die_at_stmt_end_ok() {
        // `add(&x, &x)` adds two TRANSIENT shared borrows that
        // expire at end of statement. The next `&mut x` is then
        // free to take exclusive access.
        borrow_check_ok(
            "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn bump(r: &mut i64) -> i64 { *r = *r + 1; *r }\nfn main() -> i64 { let mut x: i64 = 5; let sum: i64 = add(&x, &x); let b: i64 = bump(&mut x); sum + b }",
        );
    }

    #[test]
    fn c22_go_no_go_borrow_checks() {
        // c22_go_no_go phase-go shape: multi-shared block then
        // mut block.
        borrow_check_ok(
            "fn main() -> i64 { let mut x: i64 = 10; let a: i64 = { let r: &i64 = &x; let s: &i64 = &x; *r + *s }; let b: i64 = { let r2: &mut i64 = &mut x; *r2 = *r2 + 5; *r2 }; a + b }",
        );
    }

    // ----- C2.2 negative: XOR rule rejects -----

    #[test]
    fn double_mut_rejected() {
        // Two `&mut x` simultaneously — BorrowConflict.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r1: &mut i64 = &mut x; let r2: &mut i64 = &mut x; *r1 + *r2 }",
        );
        assert!(
            matches!(&errs[0], BorrowError::BorrowConflict { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn shared_then_mut_rejected() {
        // `&x` first, then `&mut x` — MutableBorrowOfShared.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &i64 = &x; let r2: &mut i64 = &mut x; *r + *r2 }",
        );
        assert!(
            matches!(&errs[0], BorrowError::MutableBorrowOfShared { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn mut_then_shared_rejected() {
        // `&mut x` first, then `&x` — SharedBorrowOfMutable.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &mut i64 = &mut x; let s: &i64 = &x; *r + *s }",
        );
        assert!(
            matches!(&errs[0], BorrowError::SharedBorrowOfMutable { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn write_while_shared_borrowed_rejected() {
        // `let r = &x; x = 9;` — direct write while shared
        // borrow active.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &i64 = &x; x = 9; *r }",
        );
        assert!(
            matches!(&errs[0], BorrowError::WriteWhileBorrowed { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn write_while_mut_borrowed_rejected() {
        // `let r = &mut x; x = 9;` — direct write while &mut
        // active. The exclusive holder hasn't released; owner
        // writes conflict.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &mut i64 = &mut x; x = 9; *r }",
        );
        assert!(
            matches!(&errs[0], BorrowError::WriteWhileBorrowed { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn read_while_mut_borrowed_rejected() {
        // `let r = &mut x; let _ = x;` — read of x while &mut
        // x active violates exclusivity. (Using the value in
        // an arithmetic expression triggers the Var(x) read.)
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut x: i64 = 5; let r: &mut i64 = &mut x; let y: i64 = x + 1; *r + y }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReadWhileMutBorrowed { place_name, .. } if place_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn read_while_shared_borrowed_ok() {
        // Shared borrows allow further reads of the owner —
        // sharing means readers, multiple are fine.
        borrow_check_ok(
            "fn main() -> i64 { let x: i64 = 5; let r: &i64 = &x; let y: i64 = x + 1; *r + y }",
        );
    }

    #[test]
    fn shared_in_block_then_mut_outside_ok() {
        // Use a scoped block to bound a shared borrow, then take
        // `&mut` after. The classic "introduce a new scope to
        // satisfy the borrow checker" workaround.
        borrow_check_ok(
            "fn main() -> i64 { let mut x: i64 = 5; { let r: &i64 = &x; let y: i64 = *r; y }; let r2: &mut i64 = &mut x; *r2 = 9; *r2 }",
        );
    }

    // ----- C2.3 positive: move semantics + use-after-move accepts -----

    #[test]
    fn primitives_are_copy_no_move_check() {
        // i64 is Copy — multiple reads are fine.
        borrow_check_ok(
            "fn main() -> i64 { let x: i64 = 5; let y: i64 = x; let z: i64 = x; x + y + z }",
        );
    }

    #[test]
    fn struct_moved_once_ok() {
        // Single move of a struct is fine.
        borrow_check_ok(
            "struct P { x: i64 } fn consume(p: P) -> i64 { p.x } fn main() -> i64 { let p: P = P { x: 7 }; consume(p) }",
        );
    }

    #[test]
    fn field_access_does_not_move_struct() {
        // `p.x + p.y` reads two fields without consuming p.
        borrow_check_ok(
            "struct Point { x: i64, y: i64 } fn main() -> i64 { let p: Point = Point { x: 3, y: 4 }; p.x + p.y }",
        );
    }

    #[test]
    fn array_index_does_not_move() {
        // `xs[0] + xs[1]` reads two elements without moving xs.
        borrow_check_ok(
            "fn main() -> i64 { let xs: [i64] = [10, 20]; xs[0] + xs[1] }",
        );
    }

    #[test]
    fn builtin_call_is_non_consuming() {
        // `len(xs) + xs[0]` — len is runtime, treated as
        // borrowing xs; xs[0] is postfix-receiver, also
        // non-consuming.
        borrow_check_ok(
            "fn main() -> i64 { let xs: [i64] = [10, 20, 30]; len(xs) + xs[0] }",
        );
    }

    #[test]
    fn branch_moves_in_both_arms_ok() {
        // `if c { fst(p) } else { snd(p) }` — each branch
        // moves p independently. Branch-aware merge declares p
        // Moved after, but no further use → OK.
        borrow_check_ok(
            "struct Pair { first: i64, second: i64 } fn fst(p: Pair) -> i64 { p.first } fn snd(p: Pair) -> i64 { p.second } fn pick(c: bool, p: Pair) -> i64 { if c { fst(p) } else { snd(p) } } fn main() -> i64 { let p: Pair = Pair { first: 1, second: 2 }; pick(true, p) }",
        );
    }

    #[test]
    fn two_distinct_bindings_each_moved_once_ok() {
        // c17_go_no_go shape: two Pair instances, each moved
        // into its own pick call.
        borrow_check_ok(
            "struct Pair { first: i64, second: i64 } fn consume(p: Pair) -> i64 { p.first + p.second } fn main() -> i64 { let p1: Pair = Pair { first: 7, second: 35 }; let p2: Pair = Pair { first: 7, second: 35 }; consume(p1) + consume(p2) }",
        );
    }

    #[test]
    fn nullable_of_primitive_is_copy() {
        // ?i64 is Nullable<I64> — i64 is Copy, so ?i64 is too.
        // c15_maybe_compose shape: is_some(x) then unwrap_or(x, ...).
        borrow_check_ok(
            "fn main() -> i64 { let x: ?i64 = 42; let y: ?i64 = x; unwrap_or(x, 0) + unwrap_or(y, 1) }",
        );
    }

    // ----- C2.3 negative: use-after-move rejects -----

    #[test]
    fn double_pass_by_value_rejected() {
        // `consume(p) + consume(p)` — p moved on first call;
        // second call uses moved p.
        let errs = borrow_check_err(
            "struct P { x: i64 } fn consume(p: P) -> i64 { p.x } fn main() -> i64 { let p: P = P { x: 5 }; consume(p) + consume(p) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::UseAfterMove { binding_name, .. } if binding_name == "p"),
            "got {errs:?}"
        );
    }

    #[test]
    fn rebind_then_use_original_rejected() {
        // `let q = p; print(p.x);` — q is bound to p (which
        // moves p), then p.x is accessed.
        let errs = borrow_check_err(
            "struct P { x: i64 } fn main() -> i64 { let p: P = P { x: 5 }; let q: P = p; p.x + q.x }",
        );
        assert!(
            matches!(&errs[0], BorrowError::UseAfterMove { binding_name, .. } if binding_name == "p"),
            "got {errs:?}"
        );
    }

    #[test]
    fn array_double_consume_rejected() {
        // `consume_arr(xs) + consume_arr(xs)` — arrays are
        // Move-classified ([T] regardless of T).
        let errs = borrow_check_err(
            "fn consume_arr(xs: [i64]) -> i64 { len(xs) } fn main() -> i64 { let arr: [i64] = [1, 2, 3]; consume_arr(arr) + consume_arr(arr) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::UseAfterMove { binding_name, .. } if binding_name == "arr"),
            "got {errs:?}"
        );
    }

    #[test]
    fn use_after_move_via_let_rhs_rejected() {
        // `let q = p; q.x + p.x` — q binds p (move), p.x reads
        // moved p.
        let errs = borrow_check_err(
            "struct P { x: i64 } fn main() -> i64 { let p: P = P { x: 5 }; let q: P = p; q.x + p.x }",
        );
        assert!(
            matches!(&errs[0], BorrowError::UseAfterMove { binding_name, .. } if binding_name == "p"),
            "got {errs:?}"
        );
    }

    // ----- D.5 / ADR 0036 D8: the loop-carried move rule -----

    #[test]
    fn while_loop_carried_move_rejected() {
        // Moving an OUTER binding (`p`) inside a `while` body is a
        // use-after-move on the next iteration — rejected.
        let errs = borrow_check_err(
            "struct P { x: i64 } fn consume(p: P) -> i64 { p.x } \
             fn main() -> i64 { let p: P = P { x: 5 }; let mut i: i64 = 0; \
             while i < 3 { consume(p); i = i + 1; } 0 }",
        );
        assert!(
            matches!(&errs[0], BorrowError::MovedInLoopBody { binding_name, .. } if binding_name == "p"),
            "got {errs:?}"
        );
    }

    #[test]
    fn while_inner_binding_move_ok() {
        // A binding declared INSIDE the body is fresh each iteration, so
        // moving it is fine (no loop-carried move).
        borrow_check_ok(
            "struct P { x: i64 } fn consume(p: P) -> i64 { p.x } \
             fn main() -> i64 { let mut i: i64 = 0; \
             while i < 3 { let q: P = P { x: 1 }; consume(q); i = i + 1; } 0 }",
        );
    }

    #[test]
    fn while_loop_carried_mutation_ok() {
        // Mutating an outer `let mut` via `Assign` (the termination
        // pattern) is not a move — accepted.
        borrow_check_ok(
            "fn main() -> i64 { let mut total: i64 = 0; let mut i: i64 = 0; \
             while i < 5 { total = total + i; i = i + 1; } total }",
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
        assert!(result.is_some());
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
        assert!(result.is_none());
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
        assert!(result.is_none());
        let diags = borrow_check_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "types");
    }

    // ----- C2.4: DropPlan computation -----

    #[test]
    fn drop_plan_records_moved_sources() {
        // Single move: `consume(p)` marks p as moved. DropPlan
        // should include p in moved_sources.
        let prog = parse(
            "struct P { x: i64 } fn consume(p: P) -> i64 { p.x } fn main() -> i64 { let p: P = P { x: 5 }; consume(p) }",
        )
        .expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let (plan, errors) = borrow_check(&typed);
        assert!(errors.is_empty(), "got {errors:?}");
        // main's fn_id: 14 runtime builtins + consume + main = FnId(15)
        // (looked up by name, so robust to the builtin count).
        let main = typed.fns.iter().find(|f| f.name == "main").unwrap();
        let moved = plan.moved_sources_for(main.id);
        // p's VarId is 0 (first binding in main).
        assert_eq!(moved.len(), 1, "expected 1 moved source, got {moved:?}");
    }

    #[test]
    fn drop_plan_empty_for_no_moves() {
        // Pure i64 program — no Move-typed bindings, no moves.
        let prog = parse("fn main() -> i64 { let x: i64 = 5; x + 1 }").expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let (plan, errors) = borrow_check(&typed);
        assert!(errors.is_empty());
        let main = typed.fns.iter().find(|f| f.name == "main").unwrap();
        let moved = plan.moved_sources_for(main.id);
        assert!(moved.is_empty(), "expected no moved sources, got {moved:?}");
    }
}
