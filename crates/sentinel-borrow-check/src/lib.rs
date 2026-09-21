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
use sentinel_resolve::{ClassId, FnId, ImplId, PUSH_FN_ID, VarId};
use sentinel_types::{
    NullableInner, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedParam,
    TypedProgram, TypedStmt, TypedStmtKind,
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

    /// ADR 0075 D3 (register D87): the loop-carried move rule of ADR 0036 D8, for a
    /// HANDLER ARM. An arm is not walked once and run once: the `handle`'s dispatch
    /// loop re-enters it for every operation the handled computation performs, and a
    /// `k(v)` whose resume bubbles branches straight back into it. So an arm body is a
    /// loop body the borrow checker had no rule for, and a Move-classified binding
    /// declared OUTSIDE the arm that the arm moves is consumed on the next dispatch —
    /// the same use-after-move `MovedInLoopBody` rejects, one construct over. Covers a
    /// whole binding and a field of one (ADR 0046's partial moves), by the ROOT of the
    /// moved place. A binding declared INSIDE the arm is fresh on each entry and may be
    /// moved freely. This is also what lets the bubble drain the arm's scopes at all
    /// (ADR 0075 D1): the drain is per-entry, so it is only sound while nothing an arm
    /// binding owns outlives the entry that bound it.
    #[error("cannot move out of `{binding_name}` inside a handler arm")]
    #[diagnostic(
        code(sentinel::borrow::moved_in_handler_arm),
        help("`{binding_name}` is declared outside the arm, and the handle's dispatch loop re-enters the arm for every operation performed, so moving it leaves it consumed on the next dispatch; move a binding declared inside the arm, or borrow (`&{binding_name}`) instead")
    )]
    MovedInHandlerArm {
        binding_name: String,
        #[label("`{binding_name}` declared here, outside the arm")]
        decl_span: miette::SourceSpan,
        #[label("moved here inside the arm")]
        move_span: miette::SourceSpan,
    },

    /// Register D61: a Move-typed value is moved OUT of `self`. A method's `self` is
    /// ALWAYS a borrow — `SelfKind` has exactly two variants, `&Self` and `&mut Self`,
    /// and an `init`'s `self` is the caller's object under construction — so the value
    /// still belongs to the object, and taking it by value leaves two owners of one
    /// allocation. For a STRUCT receiver the struct's owner frees it as well: a
    /// struct-target `take(self: &Self) -> [i64] { self.v }` called in a loop died with
    /// 0xC0000374 before D61 (measured). For a CLASS receiver the object is left holding
    /// a pointer to memory its new owner frees. Covers `self` itself and any field or
    /// index path rooted at it, at any depth.
    #[error("cannot move `{place}` out: `self` is only borrowed")]
    #[diagnostic(
        code(sentinel::borrow::move_out_of_self),
        help("`self` is a borrow (`&Self` or `&mut Self`), so `{place}` still belongs to the object; taking it by value would leave two owners of one allocation. Read it in place instead")
    )]
    MoveOutOfSelf {
        place: String,
        #[label("moved out of `self` here")]
        move_span: miette::SourceSpan,
    },

    /// A binding is moved while a borrow of it is still active
    /// (lexically). The move may transfer ownership to a callee that frees it,
    /// leaving the reference dangling (RESULTS R14). The lexical rule: a place
    /// cannot be moved out of while it is borrowed.
    #[error("cannot move out of `{binding_name}` while it is borrowed")]
    #[diagnostic(
        code(sentinel::borrow::move_while_borrowed),
        help("the reference into `{binding_name}` must die before it is moved; end the borrow's scope first (wrap it in an inner block)")
    )]
    MoveWhileBorrowed {
        binding_name: String,
        #[label("borrowed here")]
        borrow_span: miette::SourceSpan,
        #[label("moved here while still borrowed")]
        move_span: miette::SourceSpan,
    },

    /// A reference-carrying value is stored into a place other
    /// than a binding (a field, an element, or through a deref). Refs are
    /// second-class (ADR 0017 D6/D7): they may live only in bindings, where the
    /// checker tracks them. Unreachable while the type layer's rules hold; this
    /// is the fail-closed backstop.
    #[error("a reference cannot be stored into `{place}`")]
    #[diagnostic(
        code(sentinel::borrow::ref_stored_into_place),
        help("references are second-class (ADR 0017 D7): bind the reference to a `let` instead of storing it in a field, element, or through a dereference")
    )]
    RefStoredIntoPlace {
        place: String,
        #[label("reference stored here")]
        span: miette::SourceSpan,
    },

    /// A `let` binding is INITIALIZED with a reference to storage that is
    /// already dead — a block-local whose scope just closed, or a temporary:
    ///
    /// ```text
    /// let r: &i64 = { let inner: i64 = 5; &inner };  // ERROR: `inner` is gone
    /// ```
    ///
    /// Distinct from [`BorrowError::OutlivesSource`], which fires where such a
    /// reference is READ. This one fires at the binding, which is both earlier
    /// and where the fix belongs.
    #[error("`{binding_name}` would be bound to a reference into `{source_name}`, which is already gone")]
    #[diagnostic(
        code(sentinel::borrow::ref_outlives_binding),
        help("`{source_name}` dies before `{binding_name}` does; give the value its own `let` in the same scope as `{binding_name}` and borrow that, or bind it by copy instead of by reference")
    )]
    RefOutlivesBinding {
        binding_name: String,
        source_name: String,
        #[label("source binding here")]
        source_span: miette::SourceSpan,
        #[label("bound here")]
        init_span: miette::SourceSpan,
    },

    /// A reference-typed binding is RE-POINTED by assignment at storage that
    /// does not live as long as the binding — either already dead, or a local
    /// declared in a deeper scope (which dies first):
    ///
    /// ```text
    /// let mut q: &i64 = &outer;
    /// { let inner: i64 = 5; q = &inner; }  // ERROR: `inner` dies at the brace
    /// ```
    ///
    /// Checking this is what makes the strong update sound: the target can
    /// never come to point at something narrower than itself.
    #[error("`{binding_name}` would be re-pointed at `{source_name}`, which does not live as long as it does")]
    #[diagnostic(
        code(sentinel::borrow::ref_outlives_assignment),
        help("`{source_name}` dies before `{binding_name}` does; assign a reference whose source outlives `{binding_name}`, or narrow `{binding_name}` to an inner scope")
    )]
    RefOutlivesAssignment {
        binding_name: String,
        source_name: String,
        #[label("source here")]
        source_span: miette::SourceSpan,
        #[label("assigned here")]
        assign_span: miette::SourceSpan,
    },

    /// A reference-carrying value is passed as a call argument or dereferenced
    /// AS AN OPERAND, and the storage it points at is already dead. The operand
    /// is bound to nothing, so no other check sees it:
    ///
    /// ```text
    /// read(&{ let v: [i64] = [5]; &v[0] })  // ERROR: `v` dies at the brace
    /// ```
    #[error("this operand carries a reference into `{source_name}`, which is already gone")]
    #[diagnostic(
        code(sentinel::borrow::ref_operand_dead),
        help("`{source_name}` dies before the operand is used; give the value its own `let` in a scope that outlives the call and pass a reference to that, or pass it by copy")
    )]
    RefOperandDead {
        source_name: String,
        #[label("source here")]
        source_span: miette::SourceSpan,
        #[label("dead reference used here")]
        use_span: miette::SourceSpan,
    },
}

// =============================================================================
// Internal analysis state
// =============================================================================

/// What a ref-typed binding ultimately points to, for liveness
/// purposes. See module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Points into storage that is already dead once the
    /// current statement ends — a temporary (the non-place base of a borrow or
    /// receiver), or anything the source function cannot attribute. The MOST
    /// restrictive source: never alive at a later read, never returnable. This
    /// is what makes `source_of_expr` total: an unmodelled ref-carrying kind
    /// fails CLOSED here instead of returning `None` (= accepted).
    Temporary,
}

impl BorrowSource {
    /// The VarId that serves as the place-key for C2.2 borrow
    /// tracking, if any. `LocalAnonymous` returns None.
    fn place_key(self) -> Option<VarId> {
        match self {
            BorrowSource::Local(id) | BorrowSource::Incoming(id) => Some(id),
            BorrowSource::LocalAnonymous | BorrowSource::Temporary => None,
        }
    }
}

/// Does a value of this type hold a reference? Refs are
/// second-class (ADR 0017 D6/D7): the type layer keeps them out of struct/class
/// fields, enum payloads, array/Vec/container elements and generic-instance
/// FIELDS, so the only ref-carrying types are a ref, a nullable ref, and
/// `secret` over either. EXHAUSTIVE ON PURPOSE — a new variant must be
/// classified deliberately (a wrong `false` re-opens the gate-skip routes).
fn carries_ref(ty: Type, program: &TypedProgram) -> bool {
    match ty {
        Type::Ref(_) => true,
        Type::Nullable(inner) => match inner {
            NullableInner::Ref(_) => true,
            NullableInner::I64
            | NullableInner::I32
            | NullableInner::Bool
            | NullableInner::U8
            | NullableInner::U128
            | NullableInner::F64
            | NullableInner::Ptr
            | NullableInner::Struct(_)
            | NullableInner::TypeParam(_)
            | NullableInner::GenericInstance(_)
            | NullableInner::Guard(_)
            | NullableInner::Channel(_) => false,
        },
        Type::Secret(id) => carries_ref(program.secret_data(id).inner, program),
        // Aggregates: ref-free by the type layer's second-class rules (struct /
        // class fields, enum payloads, generic-instance FIELDS; a phantom ref type
        // ARG on a generic struct is allowed and carries nothing).
        Type::Struct(_)
        | Type::Class(_)
        | Type::Enum(_)
        | Type::GenericInstance(_)
        | Type::Array(_)
        | Type::Vec(_)
        // A generic body's `T` is abstract; the body can only hand back what it
        // was given (it can form `&T`, never a `T` pointing at its own frame).
        | Type::TypeParam(_)
        | Type::TraitSelf(_)
        // Handles + scalars: element / result types are word scalars.
        | Type::I64
        | Type::I32
        | Type::U8
        | Type::U128
        | Type::F64
        | Type::Ptr
        | Type::Bool
        | Type::Kont(_)
        | Type::Task(_)
        | Type::Channel(_)
        | Type::Process
        | Type::SealedChannel
        | Type::Fn(_)
        | Type::Shared(_)
        | Type::Mutex(_)
        | Type::Guard(_) => false,
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
    /// Creation order, so the borrows taken while evaluating one
    /// sub-expression can be ended when it completes (`end_transients_since`).
    seq: u64,
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
    /// `Some(None)` = a declared ref-carrying binding that points
    /// at nothing fn-local (initialized from `null`); a MISSING entry for a
    /// ref-carrying binding (an unseeded pattern binding / handler param) fails
    /// closed as `Temporary` at every read and flow.
    ref_source: HashMap<VarId, Option<BorrowSource>>,
    /// Declaration scope depth of every binding, for the
    /// binding-widening check and the depth-aware source merge.
    var_depth: HashMap<VarId, usize>,
    /// The enclosing fn's name + whether its return type carries
    /// a ref, so a `return` operand is checked AT the return site.
    fn_name: String,
    returns_ref: bool,
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
    /// ADR 0046: per-(VarId, field-index) PARTIAL move state. A
    /// Move-typed field `p.field` consumed by value (passed to a
    /// dropping fn) marks `(p, field)` Moved *without* moving the
    /// whole `p` — so `p`'s other fields stay usable + droppable.
    /// Any later access of a moved field surfaces `UseAfterMove`;
    /// snapshot/restored at if/else like [`moved`].
    moved_fields: HashMap<(VarId, u32), Span>,
    /// ADR 0046: the DropPlan union of partial moves — every
    /// `(VarId, field)` ever moved (across branches). Codegen
    /// skips these fields in the binding's recursive drop. Never
    /// reset by snapshot/restore.
    moved_fields_union: HashSet<(VarId, u32)>,
    /// Register D61: the method's (or init's) `self` binding, which is always a
    /// borrow, so nothing Move-typed may be moved out of it. `None` in a free fn.
    self_var: Option<VarId>,
    /// Next `BorrowInstance::seq`.
    next_seq: u64,
    /// Sources already reported dead in this fn. One dead binding is reachable
    /// from many positions at once — the `let` that binds it, every later read of
    /// that binding, and every operand the reference flows into — and each of
    /// those is the SAME mistake. The first report wins (it is the earliest in
    /// program order, so the most actionable) and the rest are dropped.
    ///
    /// Only the duplicates go: at least one report always survives, so a program
    /// with a dead reference is still rejected. `ReturnsLocalRef` is deliberately
    /// outside this set — escaping the function is a distinct hazard with a
    /// distinct fix, and it names the function, so it is worth saying separately.
    reported_dead: HashSet<BorrowSource>,
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
            var_depth: HashMap::new(),
            fn_name: String::new(),
            returns_ref: false,
            scopes: Vec::new(),
            var_in_scope: HashMap::new(),
            places: HashMap::new(),
            moved: HashMap::new(),
            moved_sources_union: HashSet::new(),
            moved_fields: HashMap::new(),
            moved_fields_union: HashSet::new(),
            next_seq: 0,
            reported_dead: HashSet::new(),
            self_var: None,
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
        self.var_depth.insert(id, self.current_depth());
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
            // A temporary is dead by the next statement.
            BorrowSource::Temporary => false,
        }
    }

    /// Is this the FIRST time `source` is reported dead in this fn? Records it
    /// either way. See [`FnCtx::reported_dead`].
    fn first_report_of(&mut self, source: BorrowSource) -> bool {
        self.reported_dead.insert(source)
    }

    /// The recorded source of a ref-carrying binding, failing
    /// CLOSED (`Temporary`) when the binding was never seeded.
    fn var_source(&self, id: VarId) -> Option<BorrowSource> {
        match self.ref_source.get(&id) {
            Some(s) => *s,
            None => Some(BorrowSource::Temporary),
        }
    }

    /// How restrictive a source is, for the merge. Dead (a
    /// temporary, or a Local whose scope already ended) beats every live one; a
    /// live Local declared DEEPER dies first; then anonymous; then incoming.
    fn restrictiveness(&self, s: BorrowSource) -> (u8, usize) {
        match s {
            BorrowSource::Local(id) if !self.var_in_scope.contains_key(&id) => (5, 1),
            BorrowSource::Temporary => (5, 0),
            BorrowSource::Local(id) => (4, self.var_depth.get(&id).copied().unwrap_or(0)),
            BorrowSource::LocalAnonymous => (2, 0),
            BorrowSource::Incoming(_) => (1, 0),
        }
    }

    /// Root every borrow that is transient, or rooted in a scope
    /// DEEPER than `depth`, at `depth` — a ref-carrying value stored into a binding
    /// declared at `depth` keeps its places borrowed for that binding's life.
    fn root_borrows_at(&mut self, depth: usize) {
        for state in self.places.values_mut() {
            for b in state.shared.iter_mut().chain(state.mut_borrow.iter_mut()) {
                match b.lifetime {
                    BorrowLifetime::Transient => b.lifetime = BorrowLifetime::UntilScope(depth),
                    BorrowLifetime::UntilScope(d) if d > depth => {
                        b.lifetime = BorrowLifetime::UntilScope(depth)
                    }
                    BorrowLifetime::UntilScope(_) => {}
                }
            }
        }
    }

    /// Pop a scope whose VALUE carries a ref: the borrows rooted
    /// in it may back that value, so they survive the pop as TRANSIENT (the
    /// enclosing statement then roots them in its binding, or clears them).
    fn pop_scope_yield(&mut self, yields_ref: bool) {
        if yields_ref {
            let d = self.current_depth();
            for state in self.places.values_mut() {
                for b in state.shared.iter_mut().chain(state.mut_borrow.iter_mut()) {
                    if b.lifetime == BorrowLifetime::UntilScope(d) {
                        b.lifetime = BorrowLifetime::Transient;
                    }
                }
            }
        }
        self.pop_scope();
    }

    /// End the TRANSIENT borrows created since `mark`. Called when
    /// a sub-expression whose value carries no ref has been evaluated (a call
    /// returning a non-ref, an `if` condition): refs are second-class, so nothing
    /// that expression was lent can outlive it. Keeps `if pred(&x) { x } else ..`
    /// (selfhost `fixpoint`) clear of the move-while-borrowed rule.
    fn end_transients_since(&mut self, mark: u64) {
        for state in self.places.values_mut() {
            state
                .shared
                .retain(|b| !(b.lifetime == BorrowLifetime::Transient && b.seq >= mark));
            if state
                .mut_borrow
                .as_ref()
                .is_some_and(|b| b.lifetime == BorrowLifetime::Transient && b.seq >= mark)
            {
                state.mut_borrow = None;
            }
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
    /// ADR 0046: per-fn PARTIAL moves — `(VarId, field-index)`
    /// pairs for Move-typed fields consumed by value. Codegen
    /// skips these fields in the binding's recursive drop (the
    /// consumer owns + frees them). A binding in `moved_sources`
    /// is skipped wholesale; one with entries here is dropped but
    /// with the named fields elided.
    pub moved_fields: BTreeMap<FnId, BTreeSet<(VarId, u32)>>,
    /// Register D61: the same two sets for METHOD bodies. Methods are not in
    /// `program.fns` and have no `FnId`, so they are keyed by [`MethodKey`]. Before
    /// D61 no method was borrow-checked at all, so these did not exist and every method
    /// looked up an EMPTY moved-set: a heap local the method moved into a call was freed
    /// again at scope exit in every back end, and one it returned was freed before the
    /// `ret` — in the text oracle and scg always, in inkwell unless it was the bare tail
    /// variable (which inkwell skips by name).
    pub method_moved_sources: BTreeMap<MethodKey, BTreeSet<VarId>>,
    pub method_moved_fields: BTreeMap<MethodKey, BTreeSet<(VarId, u32)>>,
}

/// Register D61: identifies a method body in the [`DropPlan`] — a class `init`, the
/// `n`th method of a class, or the `n`th method of an impl, in declaration order
/// (the index is the method's position in `ClassData::methods` /
/// `ImplData::methods`, the same order every back end walks them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MethodKey {
    ClassInit(ClassId),
    ClassMethod(ClassId, u32),
    ImplMethod(ImplId, u32),
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

    /// ADR 0046: look up the partial-move set for a fn (empty if
    /// none). Codegen consults it to skip moved fields in a
    /// partially-moved binding's drop.
    pub fn moved_fields_for(&self, fn_id: FnId) -> &BTreeSet<(VarId, u32)> {
        static EMPTY: std::sync::OnceLock<BTreeSet<(VarId, u32)>> = std::sync::OnceLock::new();
        self.moved_fields
            .get(&fn_id)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }

    /// Register D61: the moved-source set of a METHOD body (empty if none).
    pub fn method_moved_sources_for(&self, key: MethodKey) -> &BTreeSet<VarId> {
        static EMPTY: std::sync::OnceLock<BTreeSet<VarId>> = std::sync::OnceLock::new();
        self.method_moved_sources
            .get(&key)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }

    /// Register D61: the partial-move set of a METHOD body (empty if none).
    pub fn method_moved_fields_for(&self, key: MethodKey) -> &BTreeSet<(VarId, u32)> {
        static EMPTY: std::sync::OnceLock<BTreeSet<(VarId, u32)>> = std::sync::OnceLock::new();
        self.method_moved_fields
            .get(&key)
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
///
/// Register D61: METHOD bodies are checked too — every class `init`, every class
/// method and every impl method. They live in `class_decls` / `impl_decls`, not in
/// `program.fns`, and until D61 this loop was the only entry point, so no method was
/// ever borrow-checked: a double move or a move out of `self` was accepted in a method
/// where the identical free-fn body is rejected, and no method's moves reached codegen,
/// which then freed memory the method had already given away.
pub fn borrow_check(program: &TypedProgram) -> (DropPlan, Vec<BorrowError>) {
    let mut errors = Vec::new();
    let mut drop_plan = DropPlan::default();
    for fn_def in &program.fns {
        borrow_check_fn(fn_def, program, &mut errors, &mut drop_plan);
    }
    for cd in &program.class_decls {
        if let Some(init) = &cd.init {
            let body = MethodBody {
                name: "init",
                self_var: init.self_var_id,
                self_span: &init.span,
                params: &init.params,
                return_type: None,
                body: &init.body,
            };
            borrow_check_method(MethodKey::ClassInit(cd.id), body, program, &mut errors, &mut drop_plan);
        }
        for (i, m) in cd.methods.iter().enumerate() {
            let body = MethodBody {
                name: &m.name,
                self_var: m.self_var_id,
                self_span: &m.name_span,
                params: &m.params,
                return_type: Some(m.return_type),
                body: &m.body,
            };
            let key = MethodKey::ClassMethod(cd.id, i as u32);
            borrow_check_method(key, body, program, &mut errors, &mut drop_plan);
        }
    }
    for imp in &program.impl_decls {
        for (i, m) in imp.methods.iter().enumerate() {
            let body = MethodBody {
                name: &m.name,
                self_var: m.self_var_id,
                self_span: &m.name_span,
                params: &m.params,
                return_type: Some(m.return_type),
                body: &m.body,
            };
            let key = MethodKey::ImplMethod(imp.id, i as u32);
            borrow_check_method(key, body, program, &mut errors, &mut drop_plan);
        }
    }
    (drop_plan, errors)
}

/// Register D61: the parts of a method (or init) body the walk needs.
struct MethodBody<'p> {
    name: &'p str,
    self_var: VarId,
    self_span: &'p Span,
    params: &'p [TypedParam],
    /// `None` for an `init`, which returns nothing.
    return_type: Option<Type>,
    body: &'p TypedBlock,
}

fn borrow_check_fn(
    fn_def: &TypedFnDef,
    program: &TypedProgram,
    errors: &mut Vec<BorrowError>,
    drop_plan: &mut DropPlan,
) {
    let (moved, moved_fields) = check_body(
        &fn_def.name,
        None,
        &fn_def.params,
        Some(fn_def.return_type),
        &fn_def.body,
        program,
        errors,
    );
    drop_plan.moved_sources.insert(fn_def.id, moved);
    drop_plan.moved_fields.insert(fn_def.id, moved_fields);
}

/// Register D61: borrow-check one method body and record its move sets under `key`.
fn borrow_check_method(
    key: MethodKey,
    m: MethodBody<'_>,
    program: &TypedProgram,
    errors: &mut Vec<BorrowError>,
    drop_plan: &mut DropPlan,
) {
    let (moved, moved_fields) = check_body(
        m.name,
        Some((m.self_var, m.self_span)),
        m.params,
        m.return_type,
        m.body,
        program,
        errors,
    );
    drop_plan.method_moved_sources.insert(key, moved);
    drop_plan.method_moved_fields.insert(key, moved_fields);
}

/// Walk one body — a free fn's, or (register D61) a method's with its `self` — and
/// return its moved-source and partial-move sets for the [`DropPlan`].
#[allow(clippy::type_complexity)]
fn check_body(
    fn_name: &str,
    self_var: Option<(VarId, &Span)>,
    params: &[TypedParam],
    return_type: Option<Type>,
    body: &TypedBlock,
    program: &TypedProgram,
    errors: &mut Vec<BorrowError>,
) -> (BTreeSet<VarId>, BTreeSet<(VarId, u32)>) {
    let mut ctx = FnCtx::new();
    // The fn body is scope 0 and every nested scope is >= 1.
    // Without this push the body and the FIRST nested scope both sat at depth 0,
    // so popping any inner block / if-branch / match arm / loop body erased every
    // borrow the body had rooted (write-while-borrowed and `&mut`-vs-`&` then went
    // unchecked for the rest of the fn: a `push` could realloc under a live `&v[0]`).
    ctx.push_scope();
    ctx.fn_name = fn_name.to_string();
    ctx.returns_ref = return_type.is_some_and(|t| carries_ref(t, program));
    // Register D61: `self` is an INCOMING borrow — the caller owns the object — so it
    // is alive for the whole body, may be returned through, and nothing Move-typed may
    // be moved out of it (`ctx.self_var`).
    if let Some((id, span)) = self_var {
        ctx.declare(id, "self".to_string(), span.clone());
        ctx.ref_source.insert(id, Some(BorrowSource::Incoming(id)));
        ctx.self_var = Some(id);
    }
    // Register params at "depth 0" — they're alive for the whole
    // fn body. By-value params die at return (Local source for
    // any `&x` taken on them); incoming ref params have Incoming
    // source (the caller owns the underlying place).
    for param in params {
        ctx.declare(param.id, param.name.clone(), param.span.clone());
        if carries_ref(param.ty, program) {
            ctx.ref_source
                .insert(param.id, Some(BorrowSource::Incoming(param.id)));
        }
    }

    // Walk the body. Inner Block expressions push/pop their own
    // scopes via [`walk_expr`]'s [`TypedExprKind::Block`] arm.
    walk_block_contents(body, &mut ctx, errors, program);

    // C2.4: capture the move-sources union for the DropPlan before
    // the return-source check (which may move the tail's source,
    // e.g. `fn f() -> Pair { p }`).
    let mut moved_btree = BTreeSet::new();
    moved_btree.extend(ctx.moved_sources_union.iter().copied());
    // ADR 0046: the partial-move set (Move-typed fields consumed by value), so
    // codegen elides them from the binding's recursive drop.
    let mut moved_fields_btree = BTreeSet::new();
    moved_fields_btree.extend(ctx.moved_fields_union.iter().copied());

    // ADR 0017 D7's "second-class refs everywhere" check: if the
    // fn returns a ref, the tail's source must be Incoming. We
    // compute source_of_expr on the (still-walked) tail; var_info
    // persists across scope pops so we can name the offending
    // source binding in the diagnostic.
    // Gated on "the return type CARRIES a ref" (`?&T`, `secret &T`),
    // not `is_ref()`; every `return e` operand is checked at its own site in walk_expr.
    if ctx.returns_ref {
        let tail_source = source_of_expr(&body.tail, &ctx, program);
        check_returned_source(tail_source, &body.tail.span, &ctx, errors);
    }
    (moved_btree, moved_fields_btree)
}

/// ADR 0017 D7 "second-class refs": a returned ref-carrying value may point only
/// into the caller's storage (`Incoming`), or nowhere fn-local (`None`: a `null`,
/// or a divergent `return` checked at its own site). Shared by the
/// tail check and every `return` operand; `Temporary` is refused like a local.
fn check_returned_source(
    source: Option<BorrowSource>,
    return_span: &Span,
    ctx: &FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    let (source_name, source_span) = match source {
        None | Some(BorrowSource::Incoming(_)) => return,
        Some(BorrowSource::Local(src_id)) => {
            let info = ctx
                .var_info
                .get(&src_id)
                .cloned()
                .unwrap_or(VarInfo { name: "<unknown>".into(), span: 0..0 });
            (info.name, info.span)
        }
        Some(BorrowSource::LocalAnonymous) => ("<anonymous>".to_string(), return_span.clone()),
        Some(BorrowSource::Temporary) => ("<temporary>".to_string(), return_span.clone()),
    };
    errors.push(BorrowError::ReturnsLocalRef {
        fn_name: ctx.fn_name.clone(),
        source_name,
        source_span: to_source_span(&source_span),
        return_span: to_source_span(return_span),
    });
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
            if carries_ref(*ty, program) {
                // Total source (never a silent None for a ref-
                // carrying RHS); binding widening — a binding may not start out
                // pointing at a referent that is ALREADY dead (a block-local that
                // just went out of scope, a temporary).
                let source = source_of_expr(value, ctx, program);
                if let Some(s) = source {
                    if !ctx.is_alive(s) {
                        emit_outlives_binding(ctx, errors, name, s, &value.span);
                    }
                }
                ctx.ref_source.insert(*id, source);
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
            if carries_ref(target.ty, program) {
                if let TypedExprKind::Var(id) = &target.kind {
                    // Binding widening. The assigned referent must
                    // outlive the TARGET binding (declared at `td`): reject a dead
                    // one or a live Local declared deeper than the target. With
                    // that, the strong update below is sound — the target can never
                    // come to point at something narrower than itself, so a
                    // conditional overwrite cannot hide a dead source.
                    let td = ctx.var_depth.get(id).copied().unwrap_or(0);
                    let source = source_of_expr(value, ctx, program);
                    if let Some(s) = source {
                        let too_narrow = match s {
                            BorrowSource::Local(src) => {
                                ctx.var_depth.get(&src).copied().unwrap_or(0) > td
                            }
                            _ => false,
                        };
                        if !ctx.is_alive(s) || too_narrow {
                            emit_outlives_assignment(ctx, errors, *id, s, &value.span);
                        }
                    }
                    ctx.ref_source.insert(*id, source);
                    // Root at the TARGET's depth, not the current one: `{ q = &v[0]; }`
                    // roots the borrow where `q` lives, so it is not dropped at the inner
                    // brace while `q` still holds it.
                    ctx.root_borrows_at(td);
                } else {
                    // Fail-closed backstop — a ref stored into a
                    // field / element / through a deref is untrackable (D6/D7).
                    errors.push(BorrowError::RefStoredIntoPlace {
                        place: render_projection(target, ctx),
                        span: to_source_span(&target.span),
                    });
                }
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
        // Phase D.5 (2/N) / ADR 0036 D9: `break` / `continue` reference no
        // place and move nothing, so there is no borrow to check and
        // nothing to add to the moved set — the `MovedInLoopBody` snapshot
        // around the enclosing `while` (above) is unaffected. (Their
        // codegen DOES drop the body scope before branching, but that is a
        // drop, not a move, so it does not concern the borrow checker.)
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
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
            // ADR 0071 M1.4b slice 3c: `*g = v;` writes THROUGH the (Move-typed)
            // `?Guard` without consuming it — same non-consuming read as the
            // rvalue `*g` (see walk_expr's guard-deref arm; consuming would skip
            // the unlock drop and poison later `*g` uses with UseAfterMove).
            if matches!(inner.ty, Type::Nullable(NullableInner::Guard(_))) {
                if let TypedExprKind::Var(id) = &inner.kind {
                    check_read_conflict(*id, &inner.span, ctx, errors);
                    check_use_alive(*id, &inner.span, ctx, errors);
                } else {
                    walk_expr(inner, ctx, errors, program);
                }
                return;
            }
            // `*r = v;` — the write goes through r (a `&mut T`
            // by type-check invariant). Walk r as a normal
            // expression so its read-check fires (use-after-scope
            // for r itself); the write to r's pointee is sound
            // because XOR already ensured r is the only active
            // borrow of its source.
            //
            // That read-check only reaches a `Var`, so a COMPUTED operand
            // (`*{ let v = [5]; &mut v[0] } = 9`) needs the operand check too —
            // it is bound to nothing, and the write lands on storage that is
            // already gone.
            walk_expr(inner, ctx, errors, program);
            check_operand_alive(inner, ctx, errors, program);
        }
        TypedExprKind::FieldAccess { target: inner_target, .. } => {
            // `p.field = v;` — recurse. The eventual Var leaf
            // triggers the write-conflict on p.
            walk_assign_target(inner_target, ctx, errors, program);
        }
        TypedExprKind::Index { target: inner_target, index, .. } => {
            // `a[i] = v;` (ADR 0050) — the element store mutates the
            // base collection, so recurse on the base exactly like
            // field-assign (its Var leaf triggers the write-conflict
            // on `a`, so a write while `a` is borrowed is rejected).
            // The index is a read: walk it so its own read-check fires.
            walk_assign_target(inner_target, ctx, errors, program);
            walk_expr(index, ctx, errors, program);
        }
        _ => walk_expr(target, ctx, errors, program),
    }
}

fn walk_expr(
    expr: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    // A call-like expression whose value carries no ref ends the
    // transient borrows its operands took (see `end_transients_since`).
    let mark = ctx.next_seq;
    walk_expr_inner(expr, ctx, errors, program);
    let call_like = matches!(
        expr.kind,
        TypedExprKind::Call { .. }
            | TypedExprKind::QualifiedCall { .. }
            | TypedExprKind::MethodCall { .. }
            | TypedExprKind::ImplMethodCall { .. }
            | TypedExprKind::ClassInit { .. }
            | TypedExprKind::EnumConstruct { .. }
            | TypedExprKind::StructLit { .. }
            | TypedExprKind::Perform { .. }
            | TypedExprKind::ResumeKont { .. }
    );
    if call_like && !carries_ref(expr.ty, program) {
        ctx.end_transients_since(mark);
    }
}

fn walk_expr_inner(
    expr: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    match &expr.kind {
        // Leaves — no children.
        TypedExprKind::IntLit(_)
        // ADR 0058: a float literal is a leaf (a Copy `f64` constant).
        | TypedExprKind::FloatLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        // D.2 / ADR 0033: char/string literals have no sub-expressions
        // (the bytes are literal — no variable is read/moved). A string's
        // owned `[u8]` is dropped via its binding's type, like any array.
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        // ADR 0070: a bare fn name used as a value references a top-level fn,
        // not a local binding — no `VarId` to track (`FnId` is a disjoint
        // namespace), so it's a leaf like any other literal.
        | TypedExprKind::FnRef(_) => {}

        // Ref-typed Var reads trigger the C2.1 use-after-scope
        // check. Non-ref Var reads trigger the C2.2 read-while-
        // mutably-borrowed check AND the C2.3 use-after-move
        // check + consume.
        TypedExprKind::Var(id) => {
            if carries_ref(expr.ty, program) {
                check_ref_read(*id, &expr.span, ctx, errors);
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
        | TypedExprKind::Declassify(inner)
        // ADR 0049: an integer cast is a pure type-level width conversion of a
        // Copy scalar — walk the inner; no move/borrow change.
        | TypedExprKind::Cast(inner) => {
            walk_expr(inner, ctx, errors, program);
        }
        // ADR 0065: `return expr` moves/borrows its inner like a returned tail
        // value (the lexical 1.0 checker is path-insensitive, so code after an
        // early return is still walked — fix a false rejection by scoping).
        // AND it is a fn exit, so the ADR 0017 D7 second-class
        // check runs HERE, at the site, with the operand's scopes still live —
        // the tail check alone never saw a `return` in statement position, in a
        // handler arm, or inside a `scope` body.
        TypedExprKind::Return(inner) => {
            walk_expr(inner, ctx, errors, program);
            if ctx.returns_ref {
                let source = source_of_expr(inner, ctx, program);
                check_returned_source(source, &expr.span, ctx, errors);
            }
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
        // ADR 0071 M1.4b slice 3c: `*g` reads THROUGH the (Move-typed) `?Guard`
        // without consuming it — the guard must stay live for its scope-exit unlock,
        // and the motivating RMW shape derefs it repeatedly (`let v = *g; *g = v+6`).
        // A consuming walk would mark `g` Moved: the unlock drop would be skipped
        // (a leak + a free-while-locked abort) and the second `*g` would be a bogus
        // UseAfterMove. Non-consuming ALSO mirrors scg's unary walk, whose deref
        // operand is unconditionally non-consuming — for `Ref` operands (Copy) the
        // difference was moot, for the Move guard it is load-bearing (the
        // moved-sources dump must stay byte-identical). The use-after-move CHECK
        // still fires (a genuinely moved `g` is still rejected).
        TypedExprKind::Unary(UnaryOp::Deref, inner)
            if matches!(inner.ty, Type::Nullable(NullableInner::Guard(_))) =>
        {
            if let TypedExprKind::Var(id) = &inner.kind {
                check_read_conflict(*id, &inner.span, ctx, errors);
                check_use_alive(*id, &inner.span, ctx, errors);
            } else {
                walk_expr(inner, ctx, errors, program);
            }
        }
        TypedExprKind::Unary(op, inner) => {
            // Deref / Neg / Not: walk the inner. Deref through a
            // ref-typed Var triggers the C2.1 OutlivesSource
            // check on r; the inner value's `*r` doesn't create
            // a new borrow.
            walk_expr(inner, ctx, errors, program);
            // `*pass({ let v = [5]; &v[0] })` — the deref operand carries a
            // reference to storage that is already dead, and is bound to nothing.
            if matches!(op, UnaryOp::Deref) {
                check_operand_alive(inner, ctx, errors, program);
            }
        }

        TypedExprKind::Binary(_, l, r) | TypedExprKind::Logic(_, l, r) => {
            walk_expr(l, ctx, errors, program);
            walk_expr(r, ctx, errors, program);
        }
        TypedExprKind::Cmp(_, l, r) => {
            // Register D61: a comparison READS its operands; nothing changes owner. For an
            // operand rooted at `self` that matters, because the consuming walk would
            // report `self.next == null` as a move out of `self`. Every other operand
            // keeps the consuming walk it always had.
            for side in [l, r] {
                if ctx.self_var.is_some() && projection_root(side) == ctx.self_var {
                    walk_expr_lvalue(side, ctx, errors, program);
                } else {
                    walk_expr(side, ctx, errors, program);
                }
            }
        }

        TypedExprKind::Block(b) => {
            ctx.push_scope();
            walk_block_contents(b, ctx, errors, program);
            ctx.pop_scope_yield(carries_ref(expr.ty, program));
        }

        TypedExprKind::If { cond, then_branch, else_branch } => {
            // A `bool` condition carries no ref — its borrows end.
            let cond_mark = ctx.next_seq;
            walk_expr(cond, ctx, errors, program);
            ctx.end_transients_since(cond_mark);
            // C2.3: snapshot the move-state, walk each branch in
            // isolation, then merge — "moved in either branch →
            // moved after". This is what makes `if c { fst(p) }
            // else { snd(p) }` accept: each branch independently
            // moves p, but the merge sees p as Moved after, which
            // is fine when no further uses follow.
            let snapshot_moved = ctx.moved.clone();
            let snapshot_moved_fields = ctx.moved_fields.clone();
            let yields_ref = carries_ref(expr.ty, program);
            ctx.push_scope();
            walk_block_contents(then_branch, ctx, errors, program);
            ctx.pop_scope_yield(yields_ref);
            let then_moved = std::mem::replace(&mut ctx.moved, snapshot_moved);
            let then_moved_fields =
                std::mem::replace(&mut ctx.moved_fields, snapshot_moved_fields);
            ctx.push_scope();
            walk_block_contents(else_branch, ctx, errors, program);
            ctx.pop_scope_yield(yields_ref);
            // Merge: any binding (or field — ADR 0046) moved in the then-branch but
            // not in the else-branch is conservatively Moved after (we can't statically
            // know which branch ran).
            for (id, span) in then_moved {
                ctx.moved.entry(id).or_insert(span);
            }
            for (key, span) in then_moved_fields {
                ctx.moved_fields.entry(key).or_insert(span);
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
                let is_push = *id == PUSH_FN_ID;
                for (argi, arg) in args.iter().enumerate() {
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
                        // ADR 0034 D8: `push`'s by-value element (the 2nd
                        // arg) IS consumed — it is moved into the Vec, which
                        // then owns and frees it. Walk it CONSUMING, exactly
                        // like a by-value user-fn arg, so a Move-typed element
                        // (e.g. a `Vec<Struct>` whose struct holds a `[u8]`)
                        // is marked Moved and NOT also freed at the caller's
                        // scope exit. Without this, the element's heap buffer
                        // is double-freed: a use-after-free that silently
                        // corrupts the stored data. A Copy element (the common
                        // `Vec<i64>`/`Vec<u8>` case) is a no-op here, so this
                        // is sound and non-regressing. Every OTHER runtime
                        // builtin (`len(a)`, `str_eq(a, b)`, …) takes its
                        // Move-typed args by reference, so they stay a
                        // non-consuming lvalue read.
                        _ if is_push && argi == 1 => {
                            walk_expr(arg, ctx, errors, program);
                        }
                        _ => walk_expr_lvalue(arg, ctx, errors, program),
                    }
                    // Whichever of the three ways it was walked, a ref-carrying
                    // argument bound to no binding is checked by nothing else.
                    check_operand_alive(arg, ctx, errors, program);
                }
            } else {
                for arg in args {
                    walk_expr(arg, ctx, errors, program);
                    check_operand_alive(arg, ctx, errors, program);
                }
            }
        }

        TypedExprKind::StructLit { fields, .. } => {
            for fv in fields {
                walk_expr(fv, ctx, errors, program);
            }
        }

        TypedExprKind::FieldAccess { target, field_index, .. } => {
            // C2.3 + ADR 0046: in the CONSUMING walk, a Move-typed field passed by
            // value is a PARTIAL move of the base binding — mark `(base, field)` Moved
            // (the consumer owns + frees it) WITHOUT moving the whole base, so the
            // base's other fields stay usable + droppable. A Copy field (`p.tag`: i64)
            // is the C2.3 non-consuming receiver read. A nested projection (target not
            // a direct Var) falls back to the conservative non-consuming walk (deep
            // paths deferred — ADR 0046 D5).
            //
            // Register D61: a Move-typed projection ROOTED AT `self` is refused at any
            // depth — `self` is always a borrow, so the object keeps the value and would
            // free it too. Checked before the depth split so that it applies at every
            // depth.
            if is_copy_type(expr.ty, program) {
                walk_expr_lvalue(target, ctx, errors, program);
            } else if ctx.self_var.is_some() && projection_root(target) == ctx.self_var {
                errors.push(BorrowError::MoveOutOfSelf {
                    place: render_projection(expr, ctx),
                    move_span: to_source_span(&expr.span),
                });
            } else if let TypedExprKind::Var(base) = &target.kind {
                check_and_record_field_move(
                    *base,
                    *field_index as u32,
                    &expr.span,
                    ctx,
                    errors,
                );
            } else {
                walk_expr_lvalue(target, ctx, errors, program);
            }
        }

        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_expr(el, ctx, errors, program);
            }
        }

        TypedExprKind::Index { target, index, .. } => {
            // Register D61: a Move-typed ELEMENT taken out of an array rooted at `self`
            // (`self.vv[0]` with `vv: [[i64]]`) is a move out of `self` just as a field
            // is — the array keeps the element — so refuse it.
            if !is_copy_type(expr.ty, program)
                && ctx.self_var.is_some()
                && projection_root(target) == ctx.self_var
            {
                errors.push(BorrowError::MoveOutOfSelf {
                    place: render_projection(expr, ctx),
                    move_span: to_source_span(&expr.span),
                });
            }
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
            let yields_ref = carries_ref(expr.ty, program);
            // Handler-arm params and the return-arm binding are
            // DECLARED in their arm's scope (they were never declared, so a ref in
            // one had no source and every read of it went unchecked). A ref-typed
            // op param is left UNSEEDED → fails closed (its referent is in the
            // performing frame, which is gone once `k` has run).
            for arm in arms {
                // ADR 0075 D3 (register D87): an arm body is a LOOP BODY. The dispatch
                // loop re-enters it for every operation the handled computation
                // performs, and a bubbling `k(v)` branches back into it — but the
                // borrow checker walks it once. Snapshot the in-scope bindings and the
                // already-moved sets exactly as `While` above does, and afterwards flag
                // any OUTER binding the arm newly moved: it is consumed on the next
                // dispatch. Partial moves (ADR 0046) are flagged by the ROOT of the
                // moved place, because the root is what the outer scope still owns.
                let outer_vars: std::collections::HashSet<VarId> =
                    ctx.var_in_scope.keys().copied().collect();
                let moved_before: std::collections::HashSet<VarId> =
                    ctx.moved.keys().copied().collect();
                let fields_before: std::collections::HashSet<(VarId, u32)> =
                    ctx.moved_fields.keys().copied().collect();
                ctx.push_scope();
                for (vid, nm) in arm.param_var_ids.iter().zip(arm.param_names.iter()) {
                    ctx.declare(*vid, nm.kind.clone(), nm.span.clone());
                }
                walk_expr(&arm.body, ctx, errors, program);
                ctx.pop_scope_yield(yields_ref);
                let mut carried: Vec<(VarId, Span)> = ctx
                    .moved
                    .iter()
                    .filter(|(id, _)| !moved_before.contains(id) && outer_vars.contains(id))
                    .map(|(id, span)| (*id, span.clone()))
                    .collect();
                for ((root, fi), span) in ctx.moved_fields.iter() {
                    if !fields_before.contains(&(*root, *fi))
                        && outer_vars.contains(root)
                        && !carried.iter().any(|(id, _)| id == root)
                    {
                        carried.push((*root, span.clone()));
                    }
                }
                carried.sort_by_key(|(id, _)| id.0);
                for (id, move_span) in carried {
                    let decl_span = ctx
                        .var_info
                        .get(&id)
                        .map(|vi| vi.span.clone())
                        .unwrap_or_else(|| move_span.clone());
                    errors.push(BorrowError::MovedInHandlerArm {
                        binding_name: place_name(ctx, id),
                        decl_span: to_source_span(&decl_span),
                        move_span: to_source_span(&move_span),
                    });
                }
            }
            if let Some(ra) = return_arm {
                ctx.push_scope();
                // The handled body's value flows into the return-arm binding.
                if carries_ref(body.ty, program) {
                    let s = source_of_expr(body, ctx, program);
                    ctx.ref_source.insert(ra.value_var_id, s);
                }
                ctx.declare(ra.value_var_id, ra.value_name.kind.clone(), ra.value_name.span.clone());
                walk_expr(&ra.body, ctx, errors, program);
                ctx.pop_scope_yield(yields_ref);
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
        TypedExprKind::MethodCall { target, args, class_id, method_index, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
            for a in args {
                walk_expr(a, ctx, errors, program);
                check_operand_alive(a, ctx, errors, program);
            }
            // The auto-ref IS a borrow (ADR 0022 D3) — register it,
            // AFTER the args (two-phase, so `k.set(k.get())` stays legal). Transient:
            // it ends with the call unless the result carries a ref, in which case
            // the enclosing `let` roots it (`let r = k.p(); k.grow()` → conflict).
            let kind = program.class_decl(*class_id).methods[*method_index].self_kind;
            add_receiver_borrow(target, kind, &expr.span, ctx, errors, program);
        }
        // C4.1 / ADR 0022 D5: `Name::init(args)` is purely a
        // value-producing expression — walk its args.
        TypedExprKind::ClassInit { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
                check_operand_alive(a, ctx, errors, program);
            }
        }
        // C4.2 / ADR 0023 D5 Path 1: receiver-typed dispatch. The
        // receiver is non-consuming (auto-ref produces a borrow),
        // mirroring the class MethodCall arm above.
        TypedExprKind::ImplMethodCall { target, args, impl_id, method_index, .. } => {
            walk_expr_lvalue(target, ctx, errors, program);
            for a in args {
                walk_expr(a, ctx, errors, program);
                check_operand_alive(a, ctx, errors, program);
            }
            // As MethodCall — the receiver's auto-ref is a borrow.
            let kind = program.impl_decl(*impl_id).methods[*method_index].self_kind;
            add_receiver_borrow(target, kind, &expr.span, ctx, errors, program);
        }
        // C4.2 / ADR 0023 D5 Path 2: args includes the receiver.
        TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_expr(a, ctx, errors, program);
                check_operand_alive(a, ctx, errors, program);
            }
        }
        // C4.4 / ADR 0024: `scope concurrent { ... }` is a nested
        // block — push/pop a scope and walk its contents.
        TypedExprKind::Scope { body, .. } => {
            ctx.push_scope();
            walk_block_contents(body, ctx, errors, program);
            ctx.pop_scope_yield(carries_ref(expr.ty, program));
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
            let snapshot_moved_fields = ctx.moved_fields.clone();
            let mut union_moved = snapshot_moved.clone();
            let mut union_moved_fields = snapshot_moved_fields.clone();
            let yields_ref = carries_ref(expr.ty, program);
            for arm in arms {
                ctx.moved = snapshot_moved.clone();
                ctx.moved_fields = snapshot_moved_fields.clone();
                ctx.push_scope();
                // Pattern bindings are declared in the arm scope, so
                // `&k` of a payload binding is `Local(k)` (named, depth-tracked) and
                // dies at the arm's end. A ref-typed binding is unreachable (enum
                // payloads cannot carry refs) and, unseeded, would fail closed.
                if let sentinel_types::TypedPattern::Variant { bindings, .. } = &arm.pattern {
                    for b in bindings {
                        ctx.declare(b.var_id, b.name.clone(), b.span.clone());
                    }
                }
                walk_expr(&arm.body, ctx, errors, program);
                ctx.pop_scope_yield(yields_ref);
                for (id, span) in std::mem::take(&mut ctx.moved) {
                    union_moved.entry(id).or_insert(span);
                }
                for (key, span) in std::mem::take(&mut ctx.moved_fields) {
                    union_moved_fields.entry(key).or_insert(span);
                }
            }
            ctx.moved = union_moved;
            ctx.moved_fields = union_moved_fields;
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
            // A ref-carrying lvalue root is a READ of the ref too
            // — a method receiver `r.m()` (auto-deref, ADR 0022 D7) or a builtin's
            // by-reference arg — so it gets the same liveness check as `*r`.
            if carries_ref(expr.ty, program) {
                check_ref_read(*id, &expr.span, ctx, errors);
            }
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => {
            // ADR 0071 M1.4b slice 3c: `& *g` reads through the (Move-typed)
            // `?Guard` without consuming it — same non-consuming read as `*g`
            // (see walk_expr's guard-deref arm).
            if matches!(inner.ty, Type::Nullable(NullableInner::Guard(_))) {
                if let TypedExprKind::Var(id) = &inner.kind {
                    check_read_conflict(*id, &inner.span, ctx, errors);
                    check_use_alive(*id, &inner.span, ctx, errors);
                } else {
                    walk_expr(inner, ctx, errors, program);
                }
                return;
            }
            // `& *r` — walk r as a normal expression so its
            // OutlivesSource check fires.
            walk_expr(inner, ctx, errors, program);
        }
        TypedExprKind::FieldAccess { target, field_index, .. } => {
            // ADR 0046: reading a moved field path — even non-consuming, e.g.
            // `p.items[0]` after `p.items` was consumed — is use-after-move (the
            // field's heap is gone). The whole-base move is checked by the recursive
            // lvalue walk below; here we add the field-specific check.
            if let TypedExprKind::Var(base) = &target.kind {
                check_field_use_alive(*base, *field_index as u32, &expr.span, ctx, errors);
            }
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

/// Records a method receiver's implicit `&target` / `&mut target` borrow (per
/// the method's `self_kind`) against the receiver's place.
fn add_receiver_borrow(
    target: &TypedExpr,
    kind: sentinel_ast::SelfKind,
    span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    if let Some(source) = source_of_lvalue(target, ctx, program) {
        match kind {
            sentinel_ast::SelfKind::Shared => check_and_add_shared_borrow(source, span, ctx, errors),
            sentinel_ast::SelfKind::Exclusive => check_and_add_mut_borrow(source, span, ctx, errors),
        }
    }
}

/// A read of ref-carrying binding `id` — its referent must still
/// be alive. An unseeded binding fails closed (`var_source` → `Temporary`).
fn check_ref_read(id: VarId, span: &Span, ctx: &mut FnCtx, errors: &mut Vec<BorrowError>) {
    if let Some(source) = ctx.var_source(id) {
        if !ctx.is_alive(source) {
            emit_outlives(ctx, errors, source, span);
        }
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
    // `seq` first: `places.entry` and `next_seq` both borrow `ctx`.
    let seq = ctx.next_seq;
    ctx.next_seq += 1;
    let state = ctx.places.entry(place).or_default();
    state.shared.push(BorrowInstance {
        span: span.clone(),
        lifetime: BorrowLifetime::Transient,
        seq,
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
    // `seq` first: `places.entry` and `next_seq` both borrow `ctx`.
    let seq = ctx.next_seq;
    ctx.next_seq += 1;
    let state = ctx.places.entry(place).or_default();
    state.mut_borrow = Some(BorrowInstance {
        span: span.clone(),
        lifetime: BorrowLifetime::Transient,
        seq,
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
        // ADR 0058: `f64` is a Copy scalar (a `double`, no heap payload).
        // ADR 0057: `ptr` is a Copy scalar (an opaque address, owns nothing).
        Type::I64
        | Type::I32
        | Type::U8
        | Type::U128
        | Type::F64
        | Type::Ptr
        | Type::Bool
        | Type::Ref(_) => true,
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
        // ADR 0066 M1.2: a Channel handle is pointer-like + Copy (shared
        // producer↔consumer by copying; it is the values that move on
        // `send`, not the handle). Runtime-reclaimed, not codegen-drop.
        Type::Channel(_) => true,
        // ADR 0066 M2.1: a Process handle is pointer-like + Copy (runtime-owned;
        // `process_wait` consumes the child). Not codegen-drop.
        Type::Process => true,
        // ADR 0066 M2.4a: a SealedChannel wraps a Process pipe — pointer-like + Copy.
        Type::SealedChannel => true,
        // ADR 0070 (generalized): a Fn value is a bare LLVM function pointer —
        // pointer-like + Copy, owns nothing (no captured environment; the
        // signature id doesn't change the runtime representation).
        Type::Fn(_) => true,
        // ADR 0071 M1.4a: a `Shared<T>` handle is Copy for the borrow checker
        // (frictionless N-way co-ownership, no move-tracking — like `Channel`).
        // Unlike the other handles it will ALSO emit a scope-exit drop starting
        // at slice 3 (the refcount `--`); at this slice (2b) it still leaks
        // (`needs_drop == false`), so Copy is all the checker needs today.
        Type::Shared(_) => true,
        // ADR 0071 M1.4b: a `Mutex<T>` handle is Copy for the borrow checker
        // (like `Shared`). At slice 2b it still leaks (`needs_drop == false`);
        // slice 3 adds the scope-exit drop (`sentinel_mutex_release`, rc--).
        Type::Mutex(_) => true,
        // ADR 0071 M1.4b slice 3b: a `Guard<T>` (and its `?Guard`, below) is MOVE,
        // NOT Copy — unlike the `Shared`/`Mutex`/`Channel` handles. Its scope-exit
        // drop is `sentinel_mutex_unlock`, which (unlike the refcounted `release`)
        // has no clone accounting: duplicating a guard (`let g2 = g`) would
        // double-unlock a cell locked once. Move-tracking makes `let g2 = g`
        // CONSUME `g`, so exactly one owner drops → exactly one unlock. The D3
        // no-escape rule (below) additionally forbids a guard leaving its lock
        // scope (return / store), which would let it outlive the cell (UAF).
        Type::Guard(_) => false,
    }
}

fn is_copy_nullable_inner(inner: NullableInner) -> bool {
    match inner {
        // ADR 0066 M1.2b: the scalar `?T` set — all Copy (no heap payload).
        NullableInner::I64
        | NullableInner::I32
        | NullableInner::Bool
        | NullableInner::U8
        | NullableInner::U128
        | NullableInner::F64
        | NullableInner::Ptr => true,
        NullableInner::Ref(_) => true,
        // ADR 0071 M1.4b slice 3b: `?Guard<T>` is MOVE (like the bare `Guard`) —
        // its scope-exit drop conditionally unlocks, with no clone accounting, so
        // it must not be duplicated (a copy would double-unlock). Move-tracking
        // makes binding it into a new owner CONSUME the source.
        NullableInner::Guard(_) => false,
        // ADR 0066 M1.2c (D6a): `?Channel<T>` is COPY, tracking the bare
        // `Channel` (`Type::Channel(_) => true` above). The contrast with
        // `Guard` right above is the whole reason both arms exist: a guard is
        // Move because its drop unlocks with no clone accounting, whereas a
        // channel handle is `Copy` + deliberately LEAKED (`needs_drop ==
        // false`), so duplicating one accounts for nothing and frees nothing.
        NullableInner::Channel(_) => true,
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
    // Register D61: `self` is always a borrow; moving the whole object out of it (a
    // `self` tail in a method returning the class, `let o: P = self;`) gives the
    // object a second owner.
    if ctx.self_var == Some(id) && !is_copy_type(ty, program) {
        errors.push(BorrowError::MoveOutOfSelf {
            place: "self".to_string(),
            move_span: to_source_span(use_span),
        });
        return;
    }
    if let Some(move_span) = ctx.moved.get(&id).cloned() {
        emit_use_after_move(ctx, errors, id, &move_span, use_span);
        return;
    }
    // ADR 0046: a partially-moved binding cannot be moved as a whole (it would
    // double-free the already-moved field). Reject on the first partial move's span.
    if let Some(move_span) = ctx
        .moved_fields
        .iter()
        .find(|((b, _), _)| *b == id)
        .map(|(_, s)| s.clone())
    {
        emit_use_after_move(ctx, errors, id, &move_span, use_span);
        return;
    }
    if !is_copy_type(ty, program) {
        // A place cannot be moved out of while borrowed — the new
        // owner may free it under a live reference (RESULTS R14). Keyed on the
        // per-place borrow records, so it is exact through merges (`if c {&a[0]}
        // else {&b[0]}` borrows BOTH places) where a single recorded source is not.
        if let Some(state) = ctx.places.get(&id) {
            if let Some(b) = state.mut_borrow.as_ref().or(state.shared.first()) {
                let borrow_span = b.span.clone();
                errors.push(BorrowError::MoveWhileBorrowed {
                    binding_name: place_name(ctx, id),
                    borrow_span: to_source_span(&borrow_span),
                    move_span: to_source_span(use_span),
                });
            }
        }
        // Record the move even when it was just reported. The [`DropPlan`]
        // describes what the program DOES — codegen elides the drop of a
        // moved-from binding — and it must not depend on which diagnostics fired:
        // the emitted IR is compared byte-for-byte against the self-hosted
        // compiler, whose checker has no `MoveWhileBorrowed`. Returning early here
        // left the move unrecorded, so the oracle emitted a drop that `scg` did
        // not, and the codegen differential diverged on the two `c23_*` fixtures.
        ctx.moved.insert(id, use_span.clone());
        ctx.moved_sources_union.insert(id);
    }
}

/// Register D61: the binding a field or index path is rooted at (`self` for
/// `self.a[i].b`), or `None` when the chain starts anywhere other than a variable.
/// It follows INDEX steps as well as field steps: the first cut stopped at an index,
/// so `self.items[0].data` escaped the `self` rule entirely.
fn projection_root(e: &TypedExpr) -> Option<VarId> {
    match &e.kind {
        TypedExprKind::Var(id) => Some(*id),
        TypedExprKind::FieldAccess { target, .. } | TypedExprKind::Index { target, .. } => {
            projection_root(target)
        }
        _ => None,
    }
}

/// Register D61: `self.a.b` spelled for a diagnostic.
fn render_projection(e: &TypedExpr, ctx: &FnCtx) -> String {
    match &e.kind {
        TypedExprKind::Var(id) => ctx
            .var_info
            .get(id)
            .map(|v| v.name.clone())
            .unwrap_or_else(|| "<unknown>".to_string()),
        TypedExprKind::FieldAccess { target, field, .. } => {
            format!("{}.{}", render_projection(target, ctx), field)
        }
        TypedExprKind::Index { target, .. } => format!("{}[..]", render_projection(target, ctx)),
        _ => "<expression>".to_string(),
    }
}

/// ADR 0046: at a Move-typed `base.field` consumed by value, record the PARTIAL move
/// `(base, field_index)`. If the whole base — or this field — is already Moved, surface
/// `UseAfterMove`. Only called for non-Copy fields (the caller checks `is_copy_type`).
fn check_and_record_field_move(
    base: VarId,
    field_index: u32,
    use_span: &Span,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    if let Some(move_span) = ctx.moved.get(&base).cloned() {
        emit_use_after_move(ctx, errors, base, &move_span, use_span);
        return;
    }
    if let Some(move_span) = ctx.moved_fields.get(&(base, field_index)).cloned() {
        emit_use_after_move(ctx, errors, base, &move_span, use_span);
        return;
    }
    ctx.moved_fields.insert((base, field_index), use_span.clone());
    ctx.moved_fields_union.insert((base, field_index));
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

/// ADR 0046: at a NON-consuming `base.field` read, reject if the FIELD has been moved.
/// The whole-base move is checked separately by the recursive lvalue walk (so this only
/// fires when the base is live but the named field is gone — no duplicate diagnostic).
fn check_field_use_alive(
    base: VarId,
    field_index: u32,
    use_span: &Span,
    ctx: &FnCtx,
    errors: &mut Vec<BorrowError>,
) {
    if ctx.moved.contains_key(&base) {
        return;
    }
    if let Some(move_span) = ctx.moved_fields.get(&(base, field_index)).cloned() {
        emit_use_after_move(ctx, errors, base, &move_span, use_span);
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

/// A ref-carrying value CONSUMED AS AN OPERAND — a call or method argument, or
/// a deref operand — that is bound to no binding, and so is checked by nothing
/// else. `f({ let v = [5]; &v[0] })` hands `f` a reference to a block local that
/// is already dead at the call; the `let`-RHS and tail checks never see it
/// because it is never a let-RHS and never a tail.
///
/// A plain `Var` operand is skipped: it is checked where it is read. A BORROW
/// operand is NOT skipped — `&place` resolves to the place it names, so a live
/// one passes anyway, while `& *<computed operand>` is rooted at no binding and
/// was reached by nothing else. Exit positions (a tail, a `return`) are skipped,
/// so the clearer `ReturnsLocalRef` wins there rather than both firing.
///
/// Call this AFTER walking the operand, so any scope it opened has been popped
/// and its locals read as dead.
fn check_operand_alive(
    e: &TypedExpr,
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    program: &TypedProgram,
) {
    if !carries_ref(e.ty, program) {
        return;
    }
    if matches!(e.kind, TypedExprKind::Var(_)) {
        return;
    }
    if let Some(src) = source_of_expr(e, ctx, program) {
        if !ctx.is_alive(src) {
            emit_operand_dead(ctx, errors, src, &e.span);
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
    // Total over the ref-carrying kinds: `None` means only "points at nothing
    // fn-local" (a `null`, a divergent `return` — checked at its own site — or a
    // value that carries no ref). Every ref-carrying kind has an arm, and an
    // unmodelled one fails CLOSED (`Temporary`), never open.
    match &expr.kind {
        TypedExprKind::Unary(UnaryOp::Ref, inner)
        | TypedExprKind::Unary(UnaryOp::RefMut, inner) => {
            source_of_lvalue(inner, ctx, program)
        }
        TypedExprKind::Var(id) => ctx.var_source(*id),
        TypedExprKind::NullLit | TypedExprKind::Return(_) => None,
        // Type-level wrappers are transparent (as walk_expr already treats them).
        TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Declassify(inner)
        | TypedExprKind::Cast(inner) => source_of_expr(inner, ctx, program),
        TypedExprKind::Block(b) => source_of_expr(&b.tail, ctx, program),
        TypedExprKind::Scope { body, .. } => source_of_expr(&body.tail, ctx, program),
        TypedExprKind::If { then_branch, else_branch, .. } => {
            // Both branches contribute; merge to the most
            // restrictive (Local wins over Incoming).
            let t = source_of_expr(&then_branch.tail, ctx, program);
            let e = source_of_expr(&else_branch.tail, ctx, program);
            merge_sources(t, e, ctx)
        }
        TypedExprKind::Match { arms, .. } => {
            let mut acc = None;
            for arm in arms {
                acc = merge_sources(acc, source_of_expr(&arm.body, ctx, program), ctx);
            }
            acc
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            // The value is the return arm's (which sees the body's value through
            // its seeded binding), else the body's; any op arm may yield instead.
            let mut acc = match return_arm {
                Some(ra) => source_of_expr(&ra.body, ctx, program),
                None => source_of_expr(body, ctx, program),
            };
            for arm in arms {
                acc = merge_sources(acc, source_of_expr(&arm.body, ctx, program), ctx);
            }
            acc
        }
        TypedExprKind::Call { args, .. } | TypedExprKind::QualifiedCall { args, .. } => {
            // Conservative inter-procedural rule: the result ref
            // inherits the most-restrictive source among the
            // call's ref args. If no ref args contribute, return
            // LocalAnonymous — the call must have constructed a
            // ref out of thin air, which can only borrow-check
            // if it's actually a Local of some inaccessible
            // scope. Either way, not escapable via return.
            // Sound now that EVERY body is checked at every
            // exit (a callee can hand back only what came in by reference).
            // `QualifiedCall` passes its receiver as `args[0]` (`&s`).
            let mut acc: Option<BorrowSource> = None;
            for arg in args {
                if carries_ref(arg.ty, program) {
                    let arg_source = source_of_expr(arg, ctx, program);
                    acc = merge_sources(acc, arg_source, ctx);
                }
            }
            acc.or(Some(BorrowSource::LocalAnonymous))
        }
        TypedExprKind::MethodCall { target, args, .. }
        | TypedExprKind::ImplMethodCall { target, args, .. } => {
            // The auto-ref'd receiver `&target` is an implicit ref
            // arg (ADR 0022 D7) — `k.p()` on a local `k` may point into `k`.
            let mut acc = source_of_lvalue(target, ctx, program);
            for arg in args {
                if carries_ref(arg.ty, program) {
                    acc = merge_sources(acc, source_of_expr(arg, ctx, program), ctx);
                }
            }
            acc.or(Some(BorrowSource::LocalAnonymous))
        }
        // `perform` yields whatever the handler resumed with — alive for the rest
        // of the handled computation, i.e. this whole fn body, but not beyond it.
        TypedExprKind::Perform { .. } => Some(BorrowSource::LocalAnonymous),
        // Everything else (ResumeKont, the aggregates, projections, handles and
        // scalar ops) either carries no ref, or carries one the rules above cannot
        // attribute — fail closed.
        _ => {
            if carries_ref(expr.ty, program) {
                Some(BorrowSource::Temporary)
            } else {
                None
            }
        }
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
            // ...except as an auto-deref method receiver
            // (`c.p()` with `c: &K`), where the place is `*c` — the ref's own
            // source. A SEEDED binding (a ref param, D61's `self` — whose Var node
            // is typed as the object, not `&Self`) resolves to its entry; an
            // UNSEEDED ref-carrying one fails closed.
            if ctx.ref_source.contains_key(id) || carries_ref(expr.ty, program) {
                ctx.var_source(*id)
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
        // Any other base is not a place — a temporary (`&mk().x`,
        // `K::init(5).p()`), dead by the next statement. Was `None` (= accepted).
        _ => Some(BorrowSource::Temporary),
    }
}

/// Merge two optional borrow sources to the MOST RESTRICTIVE; `None` (points at
/// nothing fn-local) is the identity. Depth-aware — a dead source
/// beats a live one and a deeper Local beats a shallower one, where the old rule
/// kept the FIRST Local (`if c { &outer } else { let inner..; &inner }` resolved to
/// the live `outer` and the dead `inner` path went unchecked).
fn merge_sources(
    a: Option<BorrowSource>,
    b: Option<BorrowSource>,
    ctx: &FnCtx,
) -> Option<BorrowSource> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(x), Some(y)) => Some(if ctx.restrictiveness(y) > ctx.restrictiveness(x) { y } else { x }),
    }
}

/// Render a [`BorrowSource`] for a diagnostic: the name to print and the span
/// to point at. `None` for `Incoming`, which is always alive and so never
/// reaches a diagnostic — every caller gates on `is_alive` first, and this is
/// the backstop if one ever forgets.
fn describe_source(
    ctx: &FnCtx,
    source: BorrowSource,
    use_span: &Span,
) -> Option<(String, Span)> {
    match source {
        BorrowSource::Local(id) => {
            let info = ctx
                .var_info
                .get(&id)
                .cloned()
                .unwrap_or(VarInfo { name: "<unknown>".into(), span: 0..0 });
            Some((info.name, info.span))
        }
        BorrowSource::LocalAnonymous => Some(("<anonymous>".to_string(), use_span.clone())),
        BorrowSource::Temporary => Some(("<temporary>".to_string(), use_span.clone())),
        BorrowSource::Incoming(_) => None,
    }
}

/// C2.1: a dead reference is READ. The four positions a dead source can be
/// caught at each get their own code (read / bind / assign / operand) so the
/// `help` line can name the fix that actually applies there.
fn emit_outlives(
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    source: BorrowSource,
    use_span: &Span,
) {
    let Some((source_name, source_span)) = describe_source(ctx, source, use_span) else {
        return;
    };
    if !ctx.first_report_of(source) {
        return;
    }
    errors.push(BorrowError::OutlivesSource {
        source_name,
        source_span: to_source_span(&source_span),
        use_span: to_source_span(use_span),
    });
}

/// A `let` is initialized with a reference to already-dead storage.
fn emit_outlives_binding(
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    binding_name: &str,
    source: BorrowSource,
    init_span: &Span,
) {
    let Some((source_name, source_span)) = describe_source(ctx, source, init_span) else {
        return;
    };
    if !ctx.first_report_of(source) {
        return;
    }
    errors.push(BorrowError::RefOutlivesBinding {
        binding_name: binding_name.to_string(),
        source_name,
        source_span: to_source_span(&source_span),
        init_span: to_source_span(init_span),
    });
}

/// A ref binding is re-pointed at storage that dies before it does.
fn emit_outlives_assignment(
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    target: VarId,
    source: BorrowSource,
    assign_span: &Span,
) {
    let Some((source_name, source_span)) = describe_source(ctx, source, assign_span) else {
        return;
    };
    if !ctx.first_report_of(source) {
        return;
    }
    let binding_name = ctx
        .var_info
        .get(&target)
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "<unknown>".to_string());
    errors.push(BorrowError::RefOutlivesAssignment {
        binding_name,
        source_name,
        source_span: to_source_span(&source_span),
        assign_span: to_source_span(assign_span),
    });
}

/// A call argument or deref operand carries a reference into dead storage.
fn emit_operand_dead(
    ctx: &mut FnCtx,
    errors: &mut Vec<BorrowError>,
    source: BorrowSource,
    use_span: &Span,
) {
    let Some((source_name, source_span)) = describe_source(ctx, source, use_span) else {
        return;
    };
    if !ctx.first_report_of(source) {
        return;
    }
    errors.push(BorrowError::RefOperandDead {
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
        BorrowError::RefOutlivesBinding { binding_name, source_name, init_span, .. } => (
            "sentinel::borrow::ref_outlives_binding",
            format!(
                "`{binding_name}` would be bound to a reference into `{source_name}`, which is already gone"
            ),
            init_span.offset()..(init_span.offset() + init_span.len()),
        ),
        BorrowError::RefOutlivesAssignment { binding_name, source_name, assign_span, .. } => (
            "sentinel::borrow::ref_outlives_assignment",
            format!(
                "`{binding_name}` would be re-pointed at `{source_name}`, which does not live as long as it does"
            ),
            assign_span.offset()..(assign_span.offset() + assign_span.len()),
        ),
        BorrowError::RefOperandDead { source_name, use_span, .. } => (
            "sentinel::borrow::ref_operand_dead",
            format!("this operand carries a reference into `{source_name}`, which is already gone"),
            use_span.offset()..(use_span.offset() + use_span.len()),
        ),
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
        BorrowError::MovedInHandlerArm { binding_name, move_span, .. } => (
            "sentinel::borrow::moved_in_handler_arm",
            format!("cannot move out of `{binding_name}` inside a handler arm"),
            move_span.offset()..(move_span.offset() + move_span.len()),
        ),
        BorrowError::MoveOutOfSelf { place, move_span } => (
            "sentinel::borrow::move_out_of_self",
            format!("cannot move `{place}` out: `self` is only borrowed"),
            move_span.offset()..(move_span.offset() + move_span.len()),
        ),
        BorrowError::MoveWhileBorrowed { binding_name, move_span, .. } => (
            "sentinel::borrow::move_while_borrowed",
            format!("cannot move out of `{binding_name}` while it is borrowed"),
            move_span.offset()..(move_span.offset() + move_span.len()),
        ),
        BorrowError::RefStoredIntoPlace { place, span } => (
            "sentinel::borrow::ref_stored_into_place",
            format!("a reference cannot be stored into `{place}`"),
            span.offset()..(span.offset() + span.len()),
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
        // by the time r is dereferenced. Caught at the BINDING now (the earlier,
        // more actionable site), and the `*r` read is suppressed as the same
        // mistake reported again — so this is one error, not two.
        let errs = borrow_check_err(
            "fn main() -> i64 { let r: &i64 = { let inner: i64 = 5; &inner }; *r }",
        );
        assert_eq!(errs.len(), 1, "one mistake, one error: {errs:?}");
        assert!(
            matches!(
                &errs[0],
                BorrowError::RefOutlivesBinding { binding_name, source_name, .. }
                    if binding_name == "r" && source_name == "inner"
            ),
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

    // ----- ADR 0046: partial-move-through-field soundness -----

    #[test]
    fn partial_move_of_field_then_read_other_field_ok() {
        // ADR 0046: consuming a Move-typed field (`p.items`: [i64]) by value moves
        // ONLY that field; reading a different, un-moved field (`p.tag`: i64) is fine.
        // (The reproducer from borrow-check-limitations.md — formerly accepted-but-UB,
        // now accepted-and-correct: the consumer owns + frees `items`, drop skips it.)
        borrow_check_ok(
            "struct Pair { items: [i64], tag: i64 } \
             fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] } \
             fn main() -> i64 { let p: Pair = Pair { items: [10, 20], tag: 7 }; \
             let used: i64 = consume_arr(p.items); used + p.tag }",
        );
    }

    #[test]
    fn use_after_partial_field_move_rejected() {
        // ADR 0046: reading a field after it was consumed by value is use-after-move
        // (the field's heap is gone), even via a non-consuming postfix read.
        let errs = borrow_check_err(
            "struct Pair { items: [i64], tag: i64 } \
             fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] } \
             fn main() -> i64 { let p: Pair = Pair { items: [10, 20], tag: 7 }; \
             let used: i64 = consume_arr(p.items); used + p.items[0] }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::UseAfterMove { .. })),
            "got {errs:?}"
        );
    }

    // ----- Register D61: METHOD bodies are borrow-checked -----
    //
    // Until D61 `borrow_check` walked `program.fns` only, so none of these was checked.

    fn plan_of(src: &str) -> DropPlan {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        let (plan, errors) = borrow_check(&typed);
        assert!(errors.is_empty(), "expected no borrow errors, got {errors:?}");
        plan
    }

    const CLASS_P: &str = "class P { let x: i64; pub init(x: i64) { self.x = x; 0 } ";

    #[test]
    fn method_double_move_rejected() {
        // The identical free-fn body was always rejected; in a method it was accepted.
        let errs = borrow_check_err(&format!(
            "{CLASS_P} pub fn g(self: &Self) -> i64 {{ let a: [i64] = [1, 2]; \
             let b: [i64] = a; let c: [i64] = a; len(b) + len(c) }} }} \
             fn main() -> i64 {{ let p: P = P::init(1); p.g() }}"
        ));
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::UseAfterMove { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn impl_method_double_move_rejected() {
        // Impl methods (here on a STRUCT target) are walked too, not just class methods.
        let errs = borrow_check_err(
            "trait T { fn g(self: &Self) -> i64; } struct S { x: i64 } \
             impl as T for S { fn g(self: &Self) -> i64 { let a: [i64] = [1]; \
             let b: [i64] = a; let c: [i64] = a; len(b) + len(c) } } \
             fn main() -> i64 { let s: S = S { x: 1 }; s.g() }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::UseAfterMove { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn move_field_out_of_self_rejected() {
        let errs = borrow_check_err(
            "class Q { let v: [i64]; pub init(n: i64) { self.v = [n, n]; 0 } \
             pub fn take(self: &Self) -> [i64] { self.v } } \
             fn main() -> i64 { let q: Q = Q::init(3); let t: [i64] = q.take(); len(t) }",
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, BorrowError::MoveOutOfSelf { place, .. } if place == "self.v")),
            "got {errs:?}"
        );
    }

    #[test]
    fn move_whole_self_rejected() {
        let errs = borrow_check_err(
            "class P { let v: [i64]; pub init(n: i64) { self.v = [n, n]; 0 } \
             pub fn dup(self: &Self) -> P { self } } \
             fn main() -> i64 { let p: P = P::init(3); let q: P = p.dup(); len(q.v) }",
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, BorrowError::MoveOutOfSelf { place, .. } if place == "self")),
            "got {errs:?}"
        );
    }

    #[test]
    fn move_out_of_self_through_exclusive_self_rejected() {
        // `&mut Self` is still a borrow — exclusivity does not confer ownership.
        let errs = borrow_check_err(
            "class Q { let v: [i64]; pub init(n: i64) { self.v = [n, n]; 0 } \
             pub fn take(self: &mut Self) -> [i64] { self.v } } \
             fn main() -> i64 { let mut q: Q = Q::init(3); let t: [i64] = q.take(); len(t) }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::MoveOutOfSelf { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn reading_self_fields_in_place_ok() {
        // The negative control: `len(self.v)` and `self.v[0]` READ the field, they do not
        // take it, so the new rule must leave them alone.
        borrow_check_ok(
            "class Q { let v: [i64]; pub init(n: i64) { self.v = [n, n]; 0 } \
             pub fn size(self: &Self) -> i64 { len(self.v) + self.v[0] } } \
             fn main() -> i64 { let q: Q = Q::init(3); q.size() }",
        );
    }

    #[test]
    fn method_returned_local_is_in_its_moved_set() {
        // The use-after-free: the returned local must be in the METHOD's moved-set, or
        // codegen frees it before the `ret`.
        let plan = plan_of(&format!(
            "{CLASS_P} pub fn mk(self: &Self) -> [i64] {{ let a: [i64] = [40, 2]; a }} }} \
             fn main() -> i64 {{ let p: P = P::init(1); let s: [i64] = p.mk(); s[0] }}"
        ));
        assert!(
            !plan.method_moved_sources_for(MethodKey::ClassMethod(ClassId(0), 0)).is_empty(),
            "the returned local is missing from the method's moved-set: {plan:?}"
        );
        // The init moved nothing.
        assert!(plan.method_moved_sources_for(MethodKey::ClassInit(ClassId(0))).is_empty());
    }

    #[test]
    fn init_param_stored_into_field_is_in_its_moved_set() {
        // The dangling field: a param stored into `self.v` must count as moved, or the
        // init's param-frame drop frees what the object now owns.
        let plan = plan_of(
            "class Holder { let v: [i64]; pub init(v: [i64]) { self.v = v; 0 } } \
             fn main() -> i64 { let h: Holder = Holder::init([40, 2]); len(h.v) }",
        );
        assert!(
            !plan.method_moved_sources_for(MethodKey::ClassInit(ClassId(0))).is_empty(),
            "the stored param is missing from the init's moved-set: {plan:?}"
        );
    }

    #[test]
    fn move_indexed_element_out_of_self_rejected() {
        // The first cut followed field steps only; an INDEX rooted at `self` escaped.
        let errs = borrow_check_err(
            "class Q { let vv: [[i64]]; pub init(n: i64) { self.vv = [[n, n], [n]]; 0 } \
             pub fn first(self: &Self) -> [i64] { self.vv[0] } } \
             fn main() -> i64 { let q: Q = Q::init(3); let t: [i64] = q.first(); len(t) }",
        );
        assert!(
            errs.iter().any(
                |e| matches!(e, BorrowError::MoveOutOfSelf { place, .. } if place == "self.vv[..]")
            ),
            "got {errs:?}"
        );
    }

    #[test]
    fn move_field_through_index_out_of_self_rejected() {
        let errs = borrow_check_err(
            "struct Item { data: [i64] } \
             class Q { let items: [Item]; pub init(n: i64) { self.items = [Item { data: [n, n] }]; 0 } \
             pub fn grab(self: &Self) -> [i64] { self.items[0].data } } \
             fn main() -> i64 { let q: Q = Q::init(3); let t: [i64] = q.grab(); len(t) }",
        );
        assert!(
            errs.iter().any(|e| matches!(
                e,
                BorrowError::MoveOutOfSelf { place, .. } if place == "self.items[..].data"
            )),
            "got {errs:?}"
        );
    }

    #[test]
    fn comparing_self_field_with_null_ok() {
        // The false positive the first cut introduced: a comparison READS `self.o`.
        borrow_check_ok(
            "struct P { v: [i64] } \
             class Q { let o: ?P; pub init() { self.o = null; 0 } \
             pub fn empty(self: &Self) -> bool { self.o == null } } \
             fn main() -> i64 { let q: Q = Q::init(); if q.empty() { 42 } else { 0 } }",
        );
    }

    #[test]
    fn struct_target_move_out_of_self_rejected() {
        // The receiver kind where the stated double free is real: a struct's owner frees
        // the field too. Before D61 this ran and died with 0xC0000374 in a loop.
        let errs = borrow_check_err(
            "trait Take { fn take(self: &Self) -> [i64]; } struct S { v: [i64] } \
             impl as Take for S { fn take(self: &Self) -> [i64] { self.v } } \
             fn main() -> i64 { let s: S = S { v: [40, 2] }; let t: [i64] = s.take(); len(t) }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::MoveOutOfSelf { place, .. } if place == "self.v")),
            "got {errs:?}"
        );
    }

    #[test]
    fn method_returning_a_ref_into_self_ok() {
        // Pins the INCOMING seeding of `self`. Without it, `&self.x` looks like a borrow of a
        // local and is refused as ReturnsLocalRef; the only other test that would notice
        // does so through a diagnostic's wording (`<unknown>.v` instead of `self.v`).
        borrow_check_ok(
            "class P { let x: i64; pub init(x: i64) { self.x = x; 0 }              pub fn at(self: &Self) -> &i64 { &self.x } }              fn main() -> i64 { let p: P = P::init(42); *p.at() }",
        );
    }

    #[test]
    fn conflicting_borrows_of_self_in_a_method_rejected() {
        // Method bodies get the borrow-conflict rules too, not only the move rules.
        let errs = borrow_check_err(
            "class Q { let v: [i64]; pub init(n: i64) { self.v = [n]; 0 }              pub fn clash(self: &mut Self) -> i64 { let a: &mut [i64] = &mut self.v;              let b: &[i64] = &self.v; 0 } }              fn main() -> i64 { let mut q: Q = Q::init(3); q.clash() }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::SharedBorrowOfMutable { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn whole_move_after_partial_field_move_rejected() {
        // ADR 0046: a partially-moved binding cannot be moved as a whole (it would
        // double-free the already-moved field).
        let errs = borrow_check_err(
            "struct Pair { items: [i64], tag: i64 } \
             fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] } \
             fn consume_pair(q: Pair) -> i64 { q.tag } \
             fn main() -> i64 { let p: Pair = Pair { items: [10, 20], tag: 7 }; \
             let a: i64 = consume_arr(p.items); a + consume_pair(p) }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::UseAfterMove { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn double_partial_move_of_same_field_rejected() {
        // ADR 0046: consuming the same field twice is use-after-move.
        let errs = borrow_check_err(
            "struct Pair { items: [i64], tag: i64 } \
             fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] } \
             fn main() -> i64 { let p: Pair = Pair { items: [10, 20], tag: 7 }; \
             consume_arr(p.items) + consume_arr(p.items) }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::UseAfterMove { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn partial_move_of_two_distinct_fields_ok() {
        // ADR 0046: moving two DIFFERENT Move-typed fields is fine — each field is
        // independently owned by its consumer; the binding's drop skips both.
        borrow_check_ok(
            "struct Two { a: [i64], b: [i64] } \
             fn take(xs: [i64]) -> i64 { xs[0] } \
             fn main() -> i64 { let p: Two = Two { a: [1, 2], b: [3, 4] }; \
             take(p.a) + take(p.b) }",
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
        // main's fn_id: 21 runtime builtins (+ ADR 0056 sockets) + consume + main = FnId(22)
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

    // ----- ADR 0017 D7 (ref-escape): a reference may not outlive the storage it
    // points at. One test per POSITION a dead source can be caught at, plus the
    // precision controls that keep each position from over-rejecting. -----

    #[test]
    fn return_local_stmt_rejected() {
        // `return &cell` in STATEMENT position, not as the tail.
        let errs = borrow_check_err(
            "fn f(fallback: &i64, c: bool) -> &i64 { let cell: i64 = 5; if c { return &cell } else { 0 }; fallback }\nfn main() -> i64 { let z: i64 = 1; *f(&z, true) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "cell"),
            "got {errs:?}"
        );
    }

    #[test]
    fn return_refmut_local_rejected() {
        // `&mut` escapes exactly as `&` does.
        let errs = borrow_check_err(
            "fn f() -> &mut i64 { let mut x: i64 = 5; &mut x }\nfn main() -> i64 { *f() }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "x"),
            "got {errs:?}"
        );
    }

    #[test]
    fn return_incoming_via_return_ok() {
        // The control for the two above: a caller-owned `&T` escapes fine, from
        // both the statement and the tail position.
        borrow_check_ok(
            "fn f(x: &i64, c: bool) -> &i64 { if c { return x } else { 0 }; x }\nfn main() -> i64 { let y: i64 = 5; *f(&y, true) }",
        );
    }

    #[test]
    fn if_return_local_branch_rejected() {
        // Only ONE branch returns the local; the other is fine. A merge that kept
        // the live source would let this through.
        let errs = borrow_check_err(
            "fn f(x: &i64, c: bool) -> &i64 { let local: i64 = 5; if c { &local } else { x } }\nfn main() -> i64 { let y: i64 = 5; *f(&y, true) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "local"),
            "got {errs:?}"
        );
    }

    #[test]
    fn match_tail_local_ref_rejected() {
        let errs = borrow_check_err(
            "enum One { Only }\nfn g(o: One) -> &i64 { let v: i64 = 5; match o { One::Only => &v } }\nfn main() -> i64 { *g(One::Only) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn scope_tail_local_ref_rejected() {
        let errs = borrow_check_err(
            "fn g() -> &i64 { let v: i64 = 5; scope concurrent { &v } }\nfn main() -> i64 { *g() }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn return_in_handler_arm_rejected() {
        // A handler arm is a fn exit like any other. Pinned HERE rather than as a
        // corpus fixture: `return` inside a handler arm makes the Rust and
        // self-hosted MIR lowerers disagree (constructed on a reference-free
        // program, so it is not about references — registered), and every fixture
        // in `tests/` is swept by that differential.
        let errs = borrow_check_err(
            "fn escape(x: &i64) -> &i64 { let doomed: i64 = 5; let h: i64 = handle 1 with { return v => if v == 1 { return &doomed } else { 0 } }; x }\nfn main() -> i64 { let z: i64 = 1; *escape(&z) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "doomed"),
            "got {errs:?}"
        );
    }

    #[test]
    fn return_null_nullable_ref_ok() {
        // `return null` of a `?&T` returns no reference at all, so it is accepted.
        // Also not a corpus fixture: `snc llvm` emits `ptr 0` for the null half of
        // the `{i1, ptr}` pair and LLVM 18 will not assemble it (registered).
        borrow_check_ok(
            "fn maybe(x: &i64, some: bool) -> ?&i64 { if some { return x } else { 0 }; return null }\nfn main() -> i64 { let a: i64 = 5; let z: i64 = 0; *unwrap_or(maybe(&a, true), &z) }",
        );
    }

    #[test]
    fn ref_return_nullable_rejected() {
        // `?&T` carries a reference just as `&T` does — the gate must reach
        // through `Nullable`.
        let errs = borrow_check_err(
            "fn g() -> ?&i64 { let s: i64 = 5; &s }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "s"),
            "got {errs:?}"
        );
    }

    #[test]
    fn ref_return_secret_rejected() {
        // ...and through `Secret`. ADR 0019 D5 keeps `secret &T` a legal TYPE;
        // what is refused here is the escape, not the type.
        let errs = borrow_check_err(
            "fn g() -> secret &i64 { let v: i64 = 9; &v }\nfn main() -> i64 { let r: secret &i64 = g(); let p: &i64 = declassify(r); *p }",
        );
        assert!(
            matches!(&errs[0], BorrowError::ReturnsLocalRef { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn laundered_read_via_match_rhs_rejected() {
        // The source is laundered through a `match` before being bound. Caught at
        // the BINDING, which is where the fix belongs.
        let errs = borrow_check_err(
            "enum One { Only }\nfn main() -> i64 { let s: One = One::Only; let r: &i64 = { let v: [i64] = [5, 5, 5, 5]; match s { One::Only => &v[0] } }; *r }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOutlivesBinding { binding_name, .. } if binding_name == "r"),
            "got {errs:?}"
        );
    }

    #[test]
    fn assign_narrower_ref_rejected() {
        // The ASSIGNMENT position: `q` outlives the inner block, so re-pointing it
        // at a binding declared deeper leaves it dangling at the closing brace.
        // Checking this is what makes the strong update sound.
        let errs = borrow_check_err(
            "fn main() -> i64 { let outer: i64 = 1; let mut q: &i64 = &outer; { let inner: i64 = 5; q = &inner; 0 }; *q }",
        );
        assert!(
            matches!(
                &errs[0],
                BorrowError::RefOutlivesAssignment { binding_name, source_name, .. }
                    if binding_name == "q" && source_name == "inner"
            ),
            "got {errs:?}"
        );
    }

    #[test]
    fn borrow_of_temporary_rejected() {
        // `&mk().x` borrows into a value no binding owns, so no scope keeps it
        // alive. `source_of_expr` fails closed to `Temporary`, which is never
        // alive. A conservative over-rejection, documented in
        // `docs/borrow-check-limitations.md`.
        let errs = borrow_check_err(
            "struct P { x: i64 }\nfn mk() -> P { P { x: 5 } }\nfn main() -> i64 { let r: &i64 = &mk().x; *r }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOutlivesBinding { source_name, .. } if source_name == "<temporary>"),
            "got {errs:?}"
        );
    }

    #[test]
    fn borrow_field_of_bound_value_ok() {
        // The workaround the limitations doc prescribes for the case above, and
        // the help text's own suggestion: give the value a `let` and borrow that.
        // Pinned so the documented fix cannot quietly stop working.
        borrow_check_ok(
            "struct P { x: i64 }\nfn mk() -> P { P { x: 5 } }\nfn main() -> i64 { let p: P = mk(); let r: &i64 = &p.x; *r }",
        );
    }

    #[test]
    fn class_init_arg_computed_ref_rejected() {
        // `Name::init(args)` is its own walk arm; it walked its arguments without
        // checking them, so the operand position was open there.
        let errs = borrow_check_err(
            "class K { let n: i64; pub init(r: &i64) { self.n = *r; 0 } pub fn get(self: &Self) -> i64 { self.n } }\nfn main() -> i64 { let k: K = K::init({ let v: [i64] = [5, 5, 5, 5]; &v[0] }); k.get() }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOperandDead { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn class_init_arg_live_ref_ok() {
        // The control: a live computed reference into an init stays accepted.
        borrow_check_ok(
            "class K { let n: i64; pub init(r: &i64) { self.n = *r; 0 } pub fn get(self: &Self) -> i64 { self.n } }\nfn main() -> i64 { let x: i64 = 5; let k: K = K::init({ &x }); k.get() }",
        );
    }

    #[test]
    fn deref_assign_target_computed_ref_rejected() {
        // The WRITE twin of the operand check: `*<computed operand> = v`. The
        // deref target's own read-check only reaches a `Var`.
        let errs = borrow_check_err(
            "fn main() -> i64 { let keep: i64 = 1; *{ let mut v: [i64] = [5, 5]; &mut v[0] } = 9; keep }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOperandDead { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn reborrow_of_computed_ref_rejected() {
        // `& *<computed operand>` re-wraps the reference, which used to skip the
        // operand check outright. A borrow is only checked where it is taken when
        // the thing borrowed is rooted at a binding; this one is rooted at nothing.
        let errs = borrow_check_err(
            "fn read(r: &i64) -> i64 { *r }\nfn main() -> i64 { read(& *{ let v: [i64] = [5, 5, 5, 5]; &v[0] }) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOperandDead { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn borrow_operand_of_live_place_ok() {
        // The control for narrowing that skip list: an ordinary `&place` operand
        // resolves to the place it names, so a live one still passes.
        borrow_check_ok(
            "struct P { x: i64 }\nfn read(r: &i64) -> i64 { *r }\nfn main() -> i64 { let x: i64 = 5; let p: P = P { x: 7 }; let a: [i64] = [1, 2]; read(&x) + read(&p.x) + read(&a[0]) }",
        );
    }

    #[test]
    fn block_yielding_ref_keeps_its_borrows_overrejects() {
        // A documented OVER-rejection (docs/borrow-check-limitations.md). The only
        // borrow of `v` is inside a block that closes, and the value the block
        // yields points at `x` — but the checker has no provenance to tell which
        // of a block's borrows the yielded reference depends on, so when the block
        // yields one it keeps them all (`{ let s = &v[0]; s }` genuinely needs `v`
        // to stay borrowed). Pinned so making this precise is a deliberate change.
        let errs = borrow_check_err(
            "fn main() -> i64 { let mut v: [i64] = [1, 2, 3]; let x: i64 = 9; let r: &i64 = { let s: &i64 = &v[0]; let t: i64 = *s; &x }; v[0] = 7; *r + v[0] }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::WriteWhileBorrowed { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn block_yielding_ref_workaround_ok() {
        // The workaround the limitations doc prescribes: split the borrow out of
        // the block whose value is a reference.
        borrow_check_ok(
            "fn main() -> i64 { let mut v: [i64] = [1, 2, 3]; let x: i64 = 9; let t: i64 = { let s: &i64 = &v[0]; *s }; let r: &i64 = &x; v[0] = 7; *r + v[0] + t }",
        );
    }

    #[test]
    fn ref_returning_method_receiver_moved_overrejects() {
        // The other documented over-rejection: `p()` returns a reference, so the
        // receiver's auto-ref (ADR 0022 D3) is rooted for the statement and `k`
        // cannot also be moved by it — even though what is consumed is the
        // dereferenced `i64`.
        let errs = borrow_check_err(
            "class K { let n: i64; pub init(n: i64) { self.n = n; 0 } pub fn p(self: &Self) -> &i64 { &self.n } pub fn get(self: &Self) -> i64 { self.n } }\nfn sink(a: i64, k: K) -> i64 { a + k.get() }\nfn main() -> i64 { let k: K = K::init(7); sink(*k.p(), k) }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::MoveWhileBorrowed { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn ref_returning_method_receiver_workaround_ok() {
        // Bind the read before the move.
        borrow_check_ok(
            "class K { let n: i64; pub init(n: i64) { self.n = n; 0 } pub fn p(self: &Self) -> &i64 { &self.n } pub fn get(self: &Self) -> i64 { self.n } }\nfn sink(a: i64, k: K) -> i64 { a + k.get() }\nfn main() -> i64 { let k: K = K::init(7); let a: i64 = *k.p(); sink(a, k) }",
        );
    }

    #[test]
    fn call_arg_computed_ref_rejected() {
        // The OPERAND position. The block's local is dead by the time the callee
        // runs, and the operand is bound to nothing — so the let-RHS, assignment
        // and return checks all look right past it. This is the position that
        // needs its own check.
        let errs = borrow_check_err(
            "fn read(r: &i64) -> i64 { *r }\nfn main() -> i64 { read({ let v: [i64] = [5, 5, 5, 5]; &v[0] }) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOperandDead { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn deref_computed_ref_rejected() {
        // The same position reached by dereferencing the result instead of passing
        // it on. The value that comes out is an i64, so nothing downstream carries
        // a reference and no other check has a reason to look.
        let errs = borrow_check_err(
            "fn id(r: &i64) -> &i64 { r }\nfn main() -> i64 { *id({ let v: [i64] = [5, 5, 5, 5]; &v[0] }) }",
        );
        assert!(
            matches!(&errs[0], BorrowError::RefOperandDead { source_name, .. } if source_name == "v"),
            "got {errs:?}"
        );
    }

    #[test]
    fn call_arg_live_ref_ok() {
        // The control for the two above: a COMPUTED ref operand whose source is
        // still in scope is fine. Without the liveness gate this check would
        // reject every computed operand.
        borrow_check_ok(
            "fn read(r: &i64) -> i64 { *r }\nfn main() -> i64 { let x: i64 = 5; read({ &x }) + read(&x) }",
        );
    }

    #[test]
    fn move_while_borrowed_rejected() {
        // Moving a value out from under a live reference into it: the reference is
        // left pointing at storage the new owner may free.
        let errs = borrow_check_err(
            "fn sink(v: [i64]) -> i64 { v[0] - v[1] }\nfn main() -> i64 { let data: [i64] = [5, 8, 11]; let p: &i64 = &data[0]; let dropped: i64 = sink(data); *p }",
        );
        assert!(
            errs.iter().any(|e| matches!(e, BorrowError::MoveWhileBorrowed { .. })),
            "got {errs:?}"
        );
    }

    #[test]
    fn pick_if_two_incoming_ok() {
        // Two CALLER-OWNED sources merged by an if: both escape fine, so the merge
        // must not manufacture a conflict.
        borrow_check_ok(
            "fn pick(a: &i64, b: &i64, c: bool) -> &i64 { if c { a } else { b } }\nfn main() -> i64 { let x: i64 = 1; let y: i64 = 2; *pick(&x, &y, true) }",
        );
    }

    #[test]
    fn two_same_scope_locals_merged_ok() {
        // The merge resolves to the most restrictive source so a dead branch cannot
        // hide behind a live one; two sources at the SAME depth are not a conflict.
        borrow_check_ok(
            "fn main() -> i64 { let a: i64 = 9; let b: i64 = 7; let c: bool = true; let r: &i64 = if c { &a } else { &b }; *r }",
        );
    }
}
