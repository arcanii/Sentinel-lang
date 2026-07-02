//! sentinel-types
//!
//! Type checking for Sentinel. Takes a [`ResolvedProgram`] (the
//! output of `sentinel-resolve::resolve`) and produces a
//! [`TypedProgram`] where every expression carries its inferred or
//! checked [`Type`]. Per ADR 0011 D5 + ADR 0012 D1-D4, C1.2's
//! type-check pass validates that every annotation in the source
//! matches the universe of types C1.2 supports (just `i64`); the
//! universe widens to `i32` and `bool` at C1.3 per ADR 0012 D5-D8.
//!
//! Same parallel-tree shape as sentinel-resolve per the precedent
//! established at C1.1.1 — [`TypedExpr`], [`TypedStmt`],
//! [`TypedBlock`], [`TypedFnDef`], [`TypedParam`], [`TypedProgram`]
//! mirror their resolved counterparts but each expression node
//! carries a `ty: Type` field. Codegen at C1.2.4 onward consumes
//! the typed program and the LLVM lowering becomes type-aware (a
//! prerequisite for the bool/i32 work at C1.3).
//!
//! `check(program) -> Result<TypedProgram, TypeError>` is the
//! pure-function entry point. `check_query(db, file)` is the
//! `#[salsa::tracked]` wrapper that chains on
//! `sentinel_resolve::resolve_query`.

use std::collections::HashMap;

use salsa::Accumulator;
use sentinel_ast::{BinOp, CmpOp, LogicOp, Span, Spanned, TypeExpr, TypeExprKind, UnaryOp};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_resolve::{
    ClassId, EffectId, EnumId, FnId, ImplId, ImplTarget, ResolvedBlock, ResolvedExpr,
    ResolvedExprKind, ResolvedFnDef, ResolvedPattern, ResolvedProgram, ResolvedStmt,
    ResolvedStmtKind, StructId, TraitId, TypeParamId, VarId, APPLY_FN_ID, CHANNEL_CLOSE_FN_ID,
    CHANNEL_NEW_FN_ID, LEN_FN_ID, PROCESS_RECV_FN_ID, PROCESS_SEND_FN_ID, RECV_FN_ID, SEND_FN_ID,
    MUTEX_NEW_FN_ID, SHARED_GET_FN_ID, SHARED_NEW_FN_ID,
};
use sentinel_ast::SelfKind;

// =============================================================================
// Type universe
// =============================================================================

/// Sentinel's type universe. C1.2 shipped only `I64`; C1.3 (per ADR
/// 0012 D5-D8) widened to `I32` and `Bool`; C1.4 (per ADR 0013 D4)
/// added user-defined struct types tagged by [`StructId`]; C1.5
/// (per ADR 0014 D4) adds `Nullable(NullableInner)` for `?T`.
/// Nominal equality — two structs with identical field shapes are
/// distinct types per ADR 0013 D5.
///
/// Implementation note (revises ADR 0014 D4): the Nullable variant
/// holds a [`NullableInner`] subset rather than `Box<Type>`. This
/// keeps `Type` `Copy` (no cascading `.clone()` refactor across
/// the codebase) and makes the no-nested-nullables rule (ADR 0014
/// D6) structural rather than convention-based — `?(?T)` is
/// literally unrepresentable in the AST. The cost is duplicating
/// the variant list in [`NullableInner`] — every new Type at C1.6+
/// adds one line there; small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    I32,
    /// Phase D.2 / ADR 0033 D4: `u8` — an 8-bit **unsigned** integer
    /// scalar (the byte). A primitive `Type` variant with no interner
    /// table (like `I64`/`I32`/`Bool`); lowers to LLVM `i8`. It is a
    /// full integer for the op-generic `Binary`/`Cmp`/bitwise + secret
    /// pipelines (unsignedness affects only codegen — `udiv`/unsigned
    /// compares — not types); mixed-width arithmetic with `i32`/`i64`
    /// stays a `Mismatch` (explicit `u8_to_i64`/`i64_to_u8` convert).
    /// A string literal is a `[u8]` (`Array(ArrayElem::U8)`) — ADR D3.
    U8,
    /// ADR 0055: `u128` — an unsigned 128-bit integer scalar, lowering to
    /// LLVM `i128`. A primitive integer (joins `is_int`) for 64-bit-limb
    /// bignum / field arithmetic (radix-2^51): a 64x64 limb product is 128
    /// bits, which `i64` cannot hold. Unsigned like `u8` (logical `>>`,
    /// unsigned compares, zero-extend widening); mixed-width arithmetic with
    /// `i64`/`i32`/`u8` stays a `Mismatch` (use an explicit `x as u128` cast).
    U128,
    /// ADR 0058: `f64` — an IEEE-754 binary64 (double-precision) float,
    /// lowering to LLVM `double`. A **PUBLIC-ONLY** scalar primitive — it
    /// joins a NEW `is_float` predicate (NOT `is_int`), because float
    /// arithmetic is a different LLVM op family (`fadd`/`fsub`/`fmul`/`fdiv`/
    /// `fneg`/ordered `fcmp`), not the width-agnostic integer ops. There is
    /// NO `secret f64`: float ops are not constant-time on real hardware
    /// (subnormal slow paths, data-dependent `fdiv`/`sqrt`, NaN microcode
    /// branches), so a `secret f64` would be a FALSE constant-time guarantee.
    /// `secret f64` is rejected at type resolution and a `secret` value
    /// cannot be cast to `f64` (the fence). Mixed int/float arithmetic is a
    /// `Mismatch` (no implicit promotion — use an explicit `x as f64` /
    /// `x as i64` cast). Copy, like the other scalars.
    F64,
    /// ADR 0057 Phase 1b: `ptr` — an opaque raw C pointer (a machine-word
    /// address), lowering to an LLVM opaque pointer. It is NOT dereferenceable
    /// or indexable in Sentinel and joins no numeric predicate (so arithmetic /
    /// indexing it is a `Mismatch` — opacity falls out of its absence from the
    /// numeric / array sets). It is produced ONLY by an FFI return or by
    /// `ptr_of(&[u8])` / `ptr_of_mut(&mut [u8])` (the buffer's `data` field) and
    /// consumed ONLY by passing it to an `extern` call — modelling a C handle
    /// (`void*` / `char*`) as an opaque token. PUBLIC and FFI-safe (it may cross
    /// the FFI fence); Copy; owns nothing (no drop).
    Ptr,
    Bool,
    Struct(StructId),
    /// `?T` per ADR 0014 D1. Payload is the inner base type.
    Nullable(NullableInner),
    /// `[T]` per ADR 0015 D1. Payload is the element type.
    Array(ArrayElem),
    /// Phase D.3 / ADR 0034 D3: a growable, owned, mutable vector
    /// `Vec<T>`. Payload is the element type ([`VecElem`], the same
    /// flat subset as [`ArrayElem`]). A `Vec<T>` is `[T]` *plus a
    /// capacity field and mutation* — it lowers to the abi-v1
    /// `{ i64 len, i64 cap, ptr data }` (the `[T]` layout with a
    /// capacity inserted) and reuses the array element-typing / move /
    /// drop machinery, adding only growth (`realloc`) + `push`. It is
    /// **Move** (owns its heap buffer), like `Array`. Non-generic at
    /// the D.3 MVP (`Vec` inside a generic fn body — `VecElem::
    /// TypeParam` — is deferred, alongside the array deferral; ADR
    /// 0034 D8). `String` = `Vec<u8>` is deferred to D.3 (2/N) with
    /// the `[u8]`↔`Vec<u8>` bridge (ADR 0034 D5).
    Vec(VecElem),
    /// Abstract type parameter in the body of a generic fn or
    /// generic struct per ADR 0016 D6. The [`TypeParamId`] (re-
    /// exported from `sentinel_resolve`) is scoped to the
    /// surrounding fn / struct — two distinct generic fns can each
    /// have `TypeParam(0)` referring to their own first type
    /// parameter. Outside its scope, a TypeParam is meaningless;
    /// codegen monomorphization substitutes concrete types in per-
    /// instance via the `type_args` carried on each
    /// [`TypedExprKind::Call`].
    TypeParam(TypeParamId),
    /// Concrete generic-struct instance per ADR 0016 D6a. The
    /// [`GenericInstanceId`] indexes into
    /// `TypedProgram.generic_instances`, where the underlying
    /// `{ struct_id, args: Vec<Type> }` lives. Keeping the
    /// indirection through an ID lets `Type` stay `Copy + Hash`
    /// (cf. the C1.5 / C1.6 amendments that preserved this
    /// invariant for `?T` / `[T]`).
    GenericInstance(GenericInstanceId),
    /// `&T` (shared) or `&mut T` (exclusive) reference type per ADR
    /// 0017 D1 + D11. The [`RefId`] indexes into
    /// `TypedProgram.refs`, where `(mutable, inner: Type)` lives.
    /// Following the C1.7.4b interned-instance precedent — keeps
    /// `Type: Copy` (the load-bearing invariant carried through
    /// every ADR since C1.5). At C2.0.2 no borrow checking yet
    /// (per ADR 0017's sub-phase split — that lands at C2.1 / C2.2).
    Ref(RefId),
    /// `secret T` per ADR 0019 D5 (C3.1). The [`SecretId`] indexes
    /// into `TypedProgram.secrets`, where `SecretData { inner: Type }`
    /// lives. Sixth interner-table ADR running to preserve
    /// `Type: Copy + Hash`. Phase B's `Ty::Secret(Box<Ty>)` becomes
    /// `Type::Secret(SecretId)` here for the same reason
    /// `GenericInstance` and `Ref` are interned: the structural
    /// recursion would force `Box` indirection somewhere that
    /// breaks `Copy`.
    Secret(SecretId),
    /// C3.4 / ADR 0020 D5: continuation-binding type. Only appears
    /// as the env type of a handler arm's last parameter (the kont
    /// `k`). The [`KontId`] indexes into `TypedProgram.konts`,
    /// where `KontData { arg_ty, ret_ty }` lives. Seventh interner-
    /// table ADR running to preserve `Type: Copy + Hash`.
    /// Operationally: `ResumeKont { kont, args }` is the only
    /// valid use of a kont-typed binding. Any other reference
    /// (Var, let-bind, arithmetic, ...) is rejected at type-check
    /// with [`TypeError::KontUsedAsValue`].
    Kont(KontId),
    /// C4.1 / ADR 0022 D1: user-defined class type tagged by
    /// [`ClassId`]. Nominal equality, same as `Struct`. The
    /// underlying field + method signatures live in
    /// `TypedProgram.class_decls`. Stays `Copy` (a `ClassId` is
    /// just a `u32`).
    Class(ClassId),
    /// C4.2 / ADR 0023 D7: the abstract implementing type `Self`
    /// inside a trait method signature. Becomes concrete (either a
    /// `Type::Class(_)` or `Type::Struct(_)`) at impl-signature
    /// type-check via substitution against the impl's `for Type`
    /// clause. Ninth interner-table-style variant; preserves
    /// `Copy + Hash` (a `TraitId` is just a `u32`).
    TraitSelf(TraitId),
    /// C4.4 / ADR 0024 D4: `Task<T>` — the handle to a spawned
    /// concurrent task. The [`TaskId`] indexes into
    /// `TypedProgram.tasks`, where `TaskData { result_ty }` lives.
    /// Tenth interner-table-style variant; preserves `Copy + Hash`
    /// (a `TaskId` is just a `u32`). At C4.4 minimum `result_ty` is
    /// restricted to `I64` per ADR 0024 D7, so in practice the
    /// `tasks` table holds at most one entry — but the interner
    /// indirection keeps the generalisation path open + matches the
    /// `Kont` / `Secret` / `Ref` precedent.
    Task(TaskId),
    /// Phase D.1 / ADR 0032 D3: a user-defined sum type (`enum`)
    /// tagged by [`EnumId`]. Nominal equality, same as `Struct` /
    /// `Class` (two enums with identical variants are distinct
    /// types). The underlying variant + payload data lives in
    /// `TypedProgram.enums`. Eleventh interner-table-style variant;
    /// preserves `Copy + Hash` (an `EnumId` is just a `u32`).
    /// Non-generic at the D.1 MVP (generic enums are D.1b per
    /// ADR 0032 D9).
    Enum(EnumId),
    /// ADR 0066 M1.2: `Channel<T>` — a typed message-passing channel
    /// handle (mpsc, ownership-transfer `send`/`recv`). The [`ChanId`]
    /// indexes into `TypedProgram.channels`, where `ChannelData { elem_ty }`
    /// lives. Twelfth interner-table-style variant; preserves `Copy + Hash`
    /// (a `ChanId` is just a `u32`). The handle is **`Copy`** (like `Task`)
    /// — it is shared producer↔consumer by copying; it is the *values* that
    /// move on `send`, fitting the lexical borrow checker (ADR 0066 D2). At
    /// M1.2 minimum `elem_ty` is a word-sized scalar (the payload moves as
    /// an `i64`-encoded value, reusing the M1.1 spawn encode/decode).
    Channel(ChanId),
    /// ADR 0066 M2.1: `Process` — a handle to a spawned child process
    /// (`std::process::Command`). A **plain** handle (no element type, so NOT
    /// interner-generic, unlike `Task`/`Channel`) — a unit variant that lowers to
    /// an opaque `ptr` (`*SentinelProcess`). `Copy` (pointer-like, runtime-owned;
    /// `process_wait` consumes the child). Produced by `process_spawn`, consumed
    /// by `process_wait`.
    Process,
    /// ADR 0066 M2.4a / ADR 0069: `SealedChannel<secret i64>` — the AEAD-encrypted
    /// secret-cross-process endpoint (the cryptographic-`declassify` escape from the
    /// D8 fence). Like [`Type::Process`] it is a **plain** handle — a unit variant
    /// fixed at `secret i64` at the M2.4a minimum (no element type, so NOT
    /// interner-generic; M2.4c promotes it to `SealedChannel(SealId)` carrying a
    /// generic `secret T`). It lowers to the same opaque `ptr` as `Process` (bridge
    /// (iii): a `SealedChannel` *wraps* the child's pipe — the AEAD key is threaded as
    /// an ordinary `[secret i32]` value, never stored behind the handle). `Copy`
    /// (pointer-like, runtime-owned). Produced by the `sealed_channel(Process)` bridge
    /// builtin; the underlying `Process` is recovered by `sealed_process(SealedChannel)`
    /// for the stdlib `seal`/`open` framing. The distinct type is what makes the fence a
    /// **static** property (ADR 0069 D1/D9): a `secret` may cross the pipe only after
    /// `seal` (a cryptographic declassify), never raw.
    SealedChannel,
    /// ADR 0070 (generalized, M-cont): `Fn<T,R>` — a non-capturing first-class
    /// function value (a bare code pointer, no environment). The [`FnValueSigId`]
    /// indexes `TypedProgram.fn_value_sigs`, where `FnValueSigData { param_ty,
    /// ret_ty }` lives — both restricted to word-scalars
    /// (`is_spawn_word_scalar`), mirroring `Channel<T>`'s M1.2b-cont
    /// generalization. `Copy` (a bare LLVM function pointer — pointer-like, owns
    /// nothing, like `Process`/`Ptr`; the signature id doesn't change the
    /// runtime representation, which is always just a `ptr`). Produced by
    /// referencing a non-generic, non-builtin, effect-free top-level fn by bare
    /// name in value position (`ResolvedExprKind::FnRef`); consumed by the
    /// `apply(f, x)` builtin (an indirect call — NOT ordinary `f(x)` syntax, see
    /// ADR 0070 D3), context-typed from `f`'s own `FnValueSigId` (the
    /// `Channel<T>` `recv`-style pattern).
    Fn(FnValueSigId),
    /// ADR 0071 M1.4a: `Shared<T>` — a runtime-refcounted shared-ownership
    /// handle (an `Arc<T>`-shaped cell behind the C-ABI). The [`SharedId`]
    /// indexes into `TypedProgram.shared`, where `SharedData { elem_ty }`
    /// lives — mirroring `Channel<T>`'s interner-table shape (preserves
    /// `Copy + Hash`; a `SharedId` is just a `u32`). The handle is **`Copy`**
    /// for the borrow checker (frictionless N-way co-ownership, no
    /// move-tracking, like `Channel`), and — starting at slice 3 — the first
    /// such handle that ALSO emits a scope-exit drop (`sentinel_shared_release`,
    /// rc--); at THIS slice (2b) it still leaks (`needs_drop == false`, like
    /// `Channel`), the refcount accounting being deferred to slice 3. At the
    /// M1.4a minimum `elem_ty` is a word-sized scalar (encoded into the cell's
    /// i64 slot, reusing the `Channel<T>` send/recv encode/decode). Produced by
    /// `shared_new(v)`; the value is read back by `shared_get(s)`.
    Shared(SharedId),
    /// ADR 0071 M1.4b: `Mutex<T>` — a runtime-refcounted, lock-protected shared
    /// cell (`Mutex<T> = Shared<SentinelMutex<T>>`, D4). The [`MutexId`] indexes
    /// into `TypedProgram.mutexes`, where `MutexData { elem_ty }` lives —
    /// mirroring [`Type::Shared`]'s interner-table shape (preserves `Copy + Hash`;
    /// a `MutexId` is just a `u32`). Like `Shared`, the handle is **`Copy`** for
    /// the borrow checker (frictionless N-way co-ownership) and — starting at
    /// slice 3 — emits a scope-exit drop (`sentinel_mutex_release`, rc--); at THIS
    /// slice (2b) it still leaks (`needs_drop == false`). At the M1.4b minimum
    /// `elem_ty` is a word-sized scalar (encoded into the cell's i64 slot).
    /// Produced by `mutex_new(v)`; `lock(m)` yields a `?Guard<T>` (slice 2b-ii).
    Mutex(MutexId),
}

/// Identifier for an interned reference type. C2 / ADR 0017 D11.
/// Assigned in source-encounter order during type-check; stable
/// across cargo runs for a given program — same scheme as
/// [`GenericInstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefId(pub u32);

/// Identifier for an interned secret type. C3 / ADR 0019 D5
/// (C3.1). Assigned in source-encounter order during type-check;
/// same scheme as [`RefId`] / [`GenericInstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SecretId(pub u32);

/// Identifier for an interned continuation type per ADR 0020 D5
/// (C3.4). Assigned in source-encounter order during type-check
/// of `handle ... with { ... }` expressions; same scheme as
/// [`SecretId`] / [`RefId`] / [`GenericInstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KontId(pub u32);

/// The underlying data of a [`Type::Kont`] per ADR 0020 D5.
/// `arg_ty` = the op's return type (the value the handler arm
/// supplies via `k(v)`). `ret_ty` = the outer `handle` expression's
/// type (what the resume call's result evaluates to). Owned by
/// [`TypedProgram::konts`]; not `Copy` (carries two `Type` payloads).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KontData {
    pub arg_ty: Type,
    pub ret_ty: Type,
}

/// ADR 0070 (generalized): identifier for a `Fn<T,R>` value signature —
/// **not** an interner-table index (unlike `RefId`/`SecretId`/`KontId`).
/// `(param_ty, ret_ty)` are both restricted to word-scalars
/// (`is_spawn_word_scalar` ∩ has-`NullableInner`, the same 6-element set
/// `Channel<T>`'s M1.2b-cont generalization uses), so the id is computed
/// ARITHMETICALLY as `param_index * 6 + ret_index` (see
/// [`fn_value_sig_id_for`] / [`fn_value_sig_param_ret`]) — avoiding
/// threading an interner table through `resolve_type_expr`'s full
/// recursion (the way `channel_chanid_for`/`channel_elem_for` avoid
/// threading `channels` through `check_call`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnValueSigId(pub u32);

/// Identifier for an interned task type per ADR 0024 D4 (C4.4).
/// Assigned in source-encounter order during type-check of `spawn`
/// expressions; same scheme as [`KontId`] / [`SecretId`] /
/// [`RefId`] / [`GenericInstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u32);

/// The underlying data of a [`Type::Task`] per ADR 0024 D4.
/// `result_ty` = the type the spawned fn produces (the value
/// `task.await` evaluates to). Owned by [`TypedProgram::tasks`].
/// At C4.4 minimum `result_ty` is always [`Type::I64`] per ADR
/// 0024 D7 (broader result types deferred); the field is kept
/// general so the generalisation is a runtime-only change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskData {
    pub result_ty: Type,
}

/// Identifier for an interned channel type per ADR 0066 M1.2.
/// Assigned in source-encounter order during type-check of a
/// `channel_new` call; same scheme as [`TaskId`] / [`KontId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChanId(pub u32);

/// The underlying data of a [`Type::Channel`] per ADR 0066 M1.2.
/// `elem_ty` = the message type the channel carries (`send` moves a
/// value of this type in; `recv` yields `?elem_ty`). Owned by
/// [`TypedProgram::channels`]. At M1.2 minimum `elem_ty` is a word-sized
/// scalar (see [`is_spawn_word_scalar`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelData {
    pub elem_ty: Type,
}

/// ADR 0071 M1.4a: identifier for an interned `Shared<T>` type. Assigned like
/// [`ChanId`] — the 6 word-scalar `Shared<T>` types are pre-interned at FIXED
/// ids 0..=5 during builtin signature setup (see [`shared_id_for`]), so a
/// `Shared<T>` annotation / `shared_new(v)` result maps to a stable id WITHOUT
/// threading the `shared` interner through the checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SharedId(pub u32);

/// The underlying data of a [`Type::Shared`] per ADR 0071 M1.4a. `elem_ty` =
/// the value type the cell holds (`shared_new` wraps a value of this type;
/// `shared_get` yields it). Owned by [`TypedProgram::shared`]. At the M1.4a
/// minimum `elem_ty` is a word-sized scalar (see [`is_spawn_word_scalar`]),
/// encoded into the cell's i64 slot. Mirrors [`ChannelData`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SharedData {
    pub elem_ty: Type,
}

/// ADR 0071 M1.4b: identifier for an interned `Mutex<T>` type. Assigned like
/// [`SharedId`] — the 6 word-scalar `Mutex<T>` types are pre-interned at FIXED
/// ids 0..=5 during builtin signature setup (see [`mutex_id_for`]), so a
/// `Mutex<T>` annotation / `mutex_new(v)` result maps to a stable id WITHOUT
/// threading the `mutexes` interner through the checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexId(pub u32);

/// The underlying data of a [`Type::Mutex`] per ADR 0071 M1.4b. `elem_ty` = the
/// value type the lock-protected cell holds (`mutex_new` wraps a value of this
/// type; `lock`'s guard reads/writes it). Owned by [`TypedProgram::mutexes`]. At
/// the M1.4b minimum `elem_ty` is a word-sized scalar, encoded into the cell's
/// i64 slot. Mirrors [`SharedData`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutexData {
    pub elem_ty: Type,
}

/// ADR 0068: identifier for an interned NESTED-array element. A
/// `Type::Array(ArrayElem::Array(id))` is a `[[T]]` whose inner array's
/// element is `arrays[id]` (an [`ArrayElem`]). This is how the depth-1 array
/// rule (ADR 0015 D6) is lifted while keeping `Type: Copy` — the inner element
/// lives in [`TypedProgram::arrays`] and the variant carries only a `u32`,
/// exactly like [`ChanId`] / [`SecretId`] / [`GenericInstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrayId(pub u32);

/// Phase D.1 / ADR 0032 D3: the underlying data of a [`Type::Enum`].
/// Owned by [`TypedProgram::enums`] (the [`EnumId`] indexes that
/// vec); nominal, like [`ClassData`]. Variant payload types are
/// already resolved to concrete [`Type`]s here (the type checker
/// resolves the resolve-stage `TypeExpr`s against the program's
/// name tables). Non-generic at the D.1 MVP (ADR 0032 D9).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumData {
    pub id: EnumId,
    pub name: String,
    pub name_span: Span,
    pub variants: Vec<VariantData>,
}

impl EnumData {
    /// Find a variant by name, returning `(index, data)`. The index
    /// is the variant's discriminant (source order). `None` if no
    /// such variant — the caller surfaces [`TypeError::UnknownVariant`].
    pub fn variant(&self, name: &str) -> Option<(usize, &VariantData)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, v)| v.name == name)
    }
}

/// Phase D.1 / ADR 0032 D3: one variant of an [`EnumData`]. `payloads`
/// is empty for a unit variant, else the positional payload-field
/// types (already resolved). The variant's index in
/// [`EnumData::variants`] is its discriminant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VariantData {
    pub name: String,
    pub name_span: Span,
    pub payloads: Vec<Type>,
}

/// The underlying data of a [`Type::Secret`]. Owned by
/// [`TypedProgram::secrets`]. Not `Copy` (carries a `Type` payload).
/// `inner` is never itself a `Type::Secret(_)` — the depth-1 rule
/// from ADR 0019 D5 (mirroring C1.5's nested-nullable rejection)
/// is enforced at parse time by [`ParseError::DoubleSecret`] AND
/// at intern time by [`intern_secret`] (defensive: if any future
/// substitution would re-wrap, we collapse to the inner instead).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretData {
    pub inner: Type,
}

/// The underlying data of a [`Type::Ref`]. Owned by
/// [`TypedProgram::refs`]; not `Copy` (carries a `Type` payload).
/// Borrow checking does NOT happen at C2.0.2 — this struct just
/// records the reference's shape (mutability + pointed-to type).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefData {
    /// `true` for `&mut T`, `false` for `&T`.
    pub mutable: bool,
    /// The referenced type. May contain [`Type::TypeParam`] inside
    /// a generic-fn body (e.g., `fn f<T>(x: &T)`); concrete
    /// substitution happens at monomorphization per the standard
    /// interner pattern.
    pub inner: Type,
}

/// Identifier for an interned generic-struct instance like
/// `Pair<i64, bool>`. C1.7 / ADR 0016 D6a. Assigned in source-
/// encounter order during type-check; stable across cargo runs
/// for a given program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenericInstanceId(pub u32);

/// The concrete type arguments backing a [`GenericInstanceId`].
/// Owned by [`TypedProgram::generic_instances`]; never `Copy`
/// (the `Vec<Type>` payload precludes that — but we don't need
/// it Copy since callers always reach the data through the
/// program-level table).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericInstanceData {
    pub struct_id: StructId,
    /// Concrete (or abstract — TypeParam-containing for generic-
    /// fn signature instances) type arguments, one per type-param
    /// of the underlying generic struct.
    pub args: Vec<Type>,
}

/// The base types that can appear inside a `?T` per ADR 0014 D6 +
/// ADR 0015 D6. Structurally a subset of [`Type`] minus the
/// `Nullable` and `Array` constructors — enforces no-nested-
/// nullables AND no-nullable-of-array at the type level.
///
/// **C1.6 implementation amendment of ADR 0015 D6**: the ADR
/// proposed extending NullableInner with `Array(ArrayElem)` to
/// represent `?[T]`. Rust's mutual recursion (NullableInner has
/// Array(ArrayElem), ArrayElem has Nullable(NullableInner)) forces
/// `Box` indirection somewhere, which breaks `Type`'s `Copy`.
/// The simpler choice for C1.6: bound the depth to 1 — `?T` and
/// `[T]` can each contain primitives or structs only, never each
/// other. `?[T]` and `[?T]` become "not yet representable"; a
/// future ADR adds them when generics or a more sophisticated type
/// representation is in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NullableInner {
    I64,
    I32,
    Bool,
    /// ADR 0066 M1.2b: `?T` made fully general over scalars — `?u8` / `?u128`
    /// / `?f64` / `?ptr` join `?i64` / `?i32` / `?bool` (previously these scalars
    /// had no `NullableInner` variant, so `recv<Channel<u8>> -> ?u8` was not
    /// representable). All are inline `{ i1 valid, T value }` like the other
    /// scalars (no heap indirection — that is only the `Struct`/`GenericInstance`
    /// case). This unblocks generic channel ELEMENTS (the word-scalar element set
    /// is exactly word-scalar ∩ has-nullable-inner = {i64,i32,u8,bool,f64,ptr}).
    U8,
    U128,
    F64,
    Ptr,
    Struct(StructId),
    /// `?T` where T is a generic type parameter (only meaningful
    /// inside a generic fn body). C1.7 / ADR 0016 D6b.
    TypeParam(TypeParamId),
    /// `?Foo<args>` — nullable of a generic-struct instance.
    /// C1.7 / ADR 0016 D6b — partially closes the ADR 0015 D6
    /// deferral (nullable-of-generic now representable; `?[T]`
    /// stays deferred since that needs a different mutual
    /// recursion).
    GenericInstance(GenericInstanceId),
    /// `?&T` or `?&mut T` — nullable of a reference. C2 / ADR 0017
    /// D1 + D11. The matching `&?T` (ref of nullable) goes through
    /// [`Type::Ref`] with `inner: Type::Nullable(_)`.
    Ref(RefId),
}

impl NullableInner {
    /// Promote to the corresponding [`Type`].
    pub fn to_type(self) -> Type {
        match self {
            NullableInner::I64 => Type::I64,
            NullableInner::I32 => Type::I32,
            NullableInner::Bool => Type::Bool,
            // ADR 0066 M1.2b: the scalar `?T` generalization.
            NullableInner::U8 => Type::U8,
            NullableInner::U128 => Type::U128,
            NullableInner::F64 => Type::F64,
            NullableInner::Ptr => Type::Ptr,
            NullableInner::Struct(id) => Type::Struct(id),
            NullableInner::TypeParam(id) => Type::TypeParam(id),
            NullableInner::GenericInstance(id) => Type::GenericInstance(id),
            NullableInner::Ref(id) => Type::Ref(id),
        }
    }

    /// Apply a TypeParam substitution. Falls back to `self` if the
    /// substituted form would nest (Nullable or Array — neither is
    /// supported as an inner per the C1.6 depth-1 amendment of ADR
    /// 0015 D6). C1.7 / ADR 0016. C2 / ADR 0017: also threads through
    /// the refs interner so substituted ref types pick up new RefIds.
    pub fn substitute(
        self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> NullableInner {
        self.to_type()
            .substitute(subst, instances, refs)
            .to_nullable_inner()
            .unwrap_or(self)
    }
}

/// The base types that can appear inside an `[T]` per ADR 0015 D6.
/// Same shape and same C1.6 amendment as [`NullableInner`]: just
/// primitives and structs, no Nullable, no Array. `[?T]` is
/// deferred along with `?[T]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArrayElem {
    I64,
    I32,
    Bool,
    /// Phase D.2 / ADR 0033 D3: `[u8]` — the byte-array element of a
    /// string. The one genuinely new `ArrayElem` for D.2 (a string
    /// literal types to `Type::Array(ArrayElem::U8)`).
    U8,
    Struct(StructId),
    /// `[T]` where T is a generic type parameter (only meaningful
    /// inside a generic fn body). C1.7 / ADR 0016 D6b.
    TypeParam(TypeParamId),
    /// `[Foo<args>]` — array of a generic-struct instance. C1.7
    /// / ADR 0016 D6b — partially closes the ADR 0015 D6
    /// deferral.
    GenericInstance(GenericInstanceId),
    /// ADR 0047: `[secret T]` — an array whose ELEMENTS are individually
    /// secret (e.g. `[secret u8]`, a secret byte buffer). The [`SecretId`]
    /// names the element's `secret T` via `TypedProgram.secrets`, exactly as
    /// [`Type::Secret`] does for scalars — so `ArrayElem` stays `Copy` (a
    /// `Box` would break it, which is why `[secret T]` is feasible where
    /// `[?T]`/`[[T]]` were deferred). Indexing yields `Type::Secret(id)` (see
    /// [`ArrayElem::to_type`]), so the constant-time taint check fires on the
    /// element automatically, while the array's base pointer + length stay
    /// public. Admitted only for a secret SCALAR element — the demote guard
    /// ([`Type::to_array_elem_secret`]) rejects `[secret [T]]` / `[secret ?T]`.
    Secret(SecretId),
    /// ADR 0068: a NESTED array element — `[[T]]`'s element is itself an array.
    /// The [`ArrayId`] indexes [`TypedProgram::arrays`], where the *inner*
    /// array's [`ArrayElem`] lives, so `[[u8]]` is
    /// `Array(Array(id))` with `arrays[id] = U8`. Lifts the ADR 0015 D6 depth-1
    /// rule while keeping `Type: Copy` (a `u32`, not a `Box`). Its element type
    /// is recovered via [`TypedProgram::array_elem_type`] (the bare
    /// [`ArrayElem::to_type`] can't — it has no interner handle).
    Array(ArrayId),
}

impl ArrayElem {
    /// Promote to the corresponding [`Type`].
    pub fn to_type(self) -> Type {
        match self {
            ArrayElem::I64 => Type::I64,
            ArrayElem::I32 => Type::I32,
            ArrayElem::Bool => Type::Bool,
            ArrayElem::U8 => Type::U8,
            ArrayElem::Struct(id) => Type::Struct(id),
            ArrayElem::TypeParam(id) => Type::TypeParam(id),
            ArrayElem::GenericInstance(id) => Type::GenericInstance(id),
            // ADR 0047: a secret array element promotes back to the scalar
            // `secret T` it names — so `s[i]` on a `[secret T]` types as
            // `secret T`, seeding the constant-time check with no other change.
            ArrayElem::Secret(id) => Type::Secret(id),
            // ADR 0068: a nested-array element needs the `arrays` interner to
            // recover its inner element, which this bare (table-less) method
            // doesn't have. Promotion of a nested element goes through
            // [`TypedProgram::array_elem_type`]; this arm is unreachable for the
            // flat-element callers (codegen / type-check route nested elements
            // through the program-aware helper). See ADR 0068 D1.
            ArrayElem::Array(_) => {
                unreachable!("ArrayElem::Array: use TypedProgram::array_elem_type (ADR 0068)")
            }
        }
    }

    /// Apply a TypeParam substitution to this ArrayElem. Returns
    /// the substituted ArrayElem when the result still fits the
    /// flat subset; otherwise (substitution would introduce
    /// Nullable or Array nesting, deferred per ADR 0015 D6) returns
    /// `self` unchanged. C1.7 / ADR 0016. C2 / ADR 0017: refs in
    /// arrays are rejected at resolve-type-expr time (per D1's
    /// `RefInArray` rule), so substitution should never produce a
    /// `Type::Ref` here.
    pub fn substitute(
        self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> ArrayElem {
        // ADR 0068: a nested-array element carries no TypeParam (its inner
        // element is interned), so substitution is the identity. (Substitution
        // INTO a nested array — `[T]` with `T := [u8]` — is the deferred
        // generic-nested case, D6; the `to_type()` call below would panic on
        // `Array`, so short-circuit here.)
        if let ArrayElem::Array(_) = self {
            return self;
        }
        self.to_type()
            .substitute(subst, instances, refs)
            // ADR 0053: `to_array_elem_subst` so a TypeParam bound to `secret SCALAR`
            // round-trips to `ArrayElem::Secret` (the `vec_to_array`-over-secret path).
            .to_array_elem_subst()
            .unwrap_or(self)
    }
}

/// The base types that can appear inside a `Vec<T>` per ADR 0034 D3.
/// The **same flat subset** as [`ArrayElem`] (primitives + structs, no
/// Nullable, no Array, no Vec) — a `Vec<T>` mirrors `[T]`'s element
/// model so it reuses the array element-typing machinery. `Vec<Vec<T>>`
/// and `Vec<[T]>` nesting are deferred alongside the `[[T]]` deferral
/// (ADR 0034 D8 / the ADR 0015 D6 depth-1 limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VecElem {
    I64,
    I32,
    Bool,
    /// `Vec<u8>` — the growable byte buffer (the D.3 (2/N) `String`).
    U8,
    Struct(StructId),
    /// `Vec<T>` where T is a generic type parameter (only meaningful
    /// inside a generic fn body). Deferred at the D.3 MVP per ADR 0034
    /// D8, but the variant exists so the element subset matches
    /// `ArrayElem` exactly (the cascade groups Vec with Array).
    TypeParam(TypeParamId),
    /// `Vec<Foo<args>>` — vector of a generic-struct instance.
    GenericInstance(GenericInstanceId),
    /// ADR 0052: `Vec<secret T>` — a growable vector whose ELEMENTS are
    /// individually secret (e.g. `Vec<secret u8>`, a variable-length secret byte
    /// buffer). The exact sibling of [`ArrayElem::Secret`]: the [`SecretId`] names
    /// the element's `secret T` via `TypedProgram.secrets`, keeping `VecElem`
    /// `Copy`. Indexing yields `Type::Secret(id)` (see [`VecElem::to_type`]), so
    /// the constant-time taint check fires on the element automatically, while the
    /// `Vec`'s base pointer + length + capacity stay public. Admitted only for a
    /// secret SCALAR element — the demote guard ([`Type::to_vec_elem_secret`])
    /// rejects `Vec<secret [T]>` / `Vec<secret ?T>`.
    Secret(SecretId),
}

impl VecElem {
    /// Promote to the corresponding [`Type`]. Mirrors
    /// [`ArrayElem::to_type`].
    pub fn to_type(self) -> Type {
        match self {
            VecElem::I64 => Type::I64,
            VecElem::I32 => Type::I32,
            VecElem::Bool => Type::Bool,
            VecElem::U8 => Type::U8,
            VecElem::Struct(id) => Type::Struct(id),
            VecElem::TypeParam(id) => Type::TypeParam(id),
            VecElem::GenericInstance(id) => Type::GenericInstance(id),
            // ADR 0052: a secret Vec element promotes back to the scalar
            // `secret T` it names — so `v[j]` on a `Vec<secret T>` types as
            // `secret T`, seeding the constant-time check (mirrors
            // [`ArrayElem::to_type`]'s Secret arm).
            VecElem::Secret(id) => Type::Secret(id),
        }
    }

    /// Apply a TypeParam substitution to this VecElem, mirroring
    /// [`ArrayElem::substitute`]: substitute through the promoted
    /// `Type`, then demote back to the flat subset (falling back to
    /// `self` if substitution would introduce a deferred nesting).
    pub fn substitute(
        self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> VecElem {
        self.to_type()
            .substitute(subst, instances, refs)
            // ADR 0052: `to_vec_elem_subst` (not the bare `to_vec_elem`) so a
            // TypeParam bound to `secret SCALAR` round-trips to `VecElem::Secret`.
            .to_vec_elem_subst()
            .unwrap_or(self)
    }
}

/// ADR 0057: `true` iff `ty` is a Phase-1 FFI-safe type — a PUBLIC `i64` or
/// `f64`. A `secret` type is NOT FFI-safe (the constant-time fence: secret
/// data cannot cross an unverified `extern` call), nor are structs / arrays /
/// other widths (deferred to a later FFI phase). Gates `extern` param/return
/// types.
fn is_ffi_safe(ty: Type) -> bool {
    // ADR 0057: public `i64` / `f64` (value ABI) + the opaque `ptr` (Phase 1b).
    matches!(ty, Type::I64 | Type::F64 | Type::Ptr)
}

/// ADR 0059 Phase 1b: `true` iff `ty` is `&[u8]` / `&mut [u8]` — a reference to
/// a byte array. Such a param is presented to C as the idiomatic
/// `(const uint8_t* data, int64_t len)` pair (the export wrapper rebuilds the
/// Sentinel `[u8]` fat pointer internally). The buffer ABI for byte-slice
/// EXPORT params; a non-byte-slice ref or an owned `[u8]` return is not yet
/// FFI-safe (later phases).
fn is_byte_slice_ref(ty: Type, refs: &[RefData]) -> bool {
    if let Type::Ref(id) = ty {
        if let Some(rd) = refs.get(id.0 as usize) {
            return matches!(rd.inner, Type::Array(ArrayElem::U8));
        }
    }
    false
}

/// ADR 0059 Phase 1b (A7): `true` iff `ty` is an owned PUBLIC `[u8]` — the
/// FFI-safe RETURN type for a byte-buffer export. C receives it via the
/// out-param pair `(uint8_t** out_data, int64_t* out_len)` and frees it with
/// `sentinel_free_bytes`. A `[secret u8]` (`ArrayElem::Secret`) or a
/// `secret [u8]` (`Type::Secret`) is NOT matched — the secret-fence stands, so
/// a secret value can cross the boundary only via an explicit `declassify`.
fn is_owned_byte_array(ty: Type) -> bool {
    matches!(ty, Type::Array(ArrayElem::U8))
}

impl Type {
    /// `true` if this is an integer type (`I32`, `I64`, or `U8`).
    /// Used to gate arithmetic-operator typing rules — comparisons
    /// accept any integer type but logicals require [`Type::Bool`].
    /// Phase D.2 / ADR 0033 D4: `U8` joins the integers so the
    /// op-generic `Binary` pipeline types `u8` arithmetic + bitwise
    /// with no other change (mixed-width is still caught by the
    /// `l.ty != r.ty` operand check).
    pub fn is_int(self) -> bool {
        matches!(self, Type::I32 | Type::I64 | Type::U8 | Type::U128)
    }

    /// ADR 0058: `true` if this is a floating-point type (`F64`). Kept
    /// SEPARATE from [`is_int`] on purpose — float arithmetic lowers to a
    /// different LLVM op family (`fadd`/`fcmp`/…), and `f64` is excluded from
    /// the `secret` / constant-time domain, so the two predicates must not be
    /// conflated. Gates float `+ - * /`, comparisons, unary `-`, and the
    /// int↔float `as` casts.
    pub fn is_float(self) -> bool {
        matches!(self, Type::F64)
    }

    /// `true` if this is a struct type — either a non-generic
    /// nominal `Struct(_)` or a generic-instance `GenericInstance(_)`
    /// (which is also a struct at the runtime level). Used by the
    /// field-access rule to gate "the target must be a struct".
    pub fn is_struct(self) -> bool {
        matches!(self, Type::Struct(_) | Type::GenericInstance(_))
    }

    /// `true` if this is a nullable type (`?T`).
    pub fn is_nullable(self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// `true` if this is an array type (`[T]`).
    pub fn is_array(self) -> bool {
        matches!(self, Type::Array(_))
    }

    /// `true` if this is a growable-vector type (`Vec<T>`). Phase D.3
    /// / ADR 0034 D3.
    pub fn is_vec(self) -> bool {
        matches!(self, Type::Vec(_))
    }

    /// `true` if this is a generic type parameter (`T`, `U`, …).
    /// C1.7 / ADR 0016. Only meaningful inside a generic fn body
    /// or struct decl.
    pub fn is_type_param(self) -> bool {
        matches!(self, Type::TypeParam(_))
    }

    /// `true` if this is a concrete generic-struct instance
    /// (e.g., `Pair<i64, bool>`). C1.7 / ADR 0016 D6a.
    pub fn is_generic_instance(self) -> bool {
        matches!(self, Type::GenericInstance(_))
    }

    /// `true` if this is a reference type (`&T` or `&mut T`).
    /// C2 / ADR 0017 D11.
    pub fn is_ref(self) -> bool {
        matches!(self, Type::Ref(_))
    }

    /// Try to demote this Type to a [`NullableInner`] for use as
    /// the payload of a `Nullable`. Returns `None` for `Nullable`
    /// (would be nested per ADR 0014 D6) AND for `Array` (the
    /// `?[T]` combination is deferred per C1.6's depth-1 amendment
    /// of ADR 0015 D6).
    pub fn to_nullable_inner(self) -> Option<NullableInner> {
        match self {
            Type::I64 => Some(NullableInner::I64),
            Type::I32 => Some(NullableInner::I32),
            Type::Bool => Some(NullableInner::Bool),
            Type::Struct(id) => Some(NullableInner::Struct(id)),
            Type::TypeParam(id) => Some(NullableInner::TypeParam(id)),
            Type::GenericInstance(id) => Some(NullableInner::GenericInstance(id)),
            Type::Ref(id) => Some(NullableInner::Ref(id)),
            // ADR 0066 M1.2b: `?T` is now general over scalars — `?u8` / `?u128`
            // / `?f64` / `?ptr` are representable (they were previously in the
            // `None` group below). All inline `{ i1, T }`.
            Type::U8 => Some(NullableInner::U8),
            Type::U128 => Some(NullableInner::U128),
            Type::F64 => Some(NullableInner::F64),
            Type::Ptr => Some(NullableInner::Ptr),
            // C3 / ADR 0019 D5: `?(secret T)` is not yet
            // representable — NullableInner has no Secret variant
            // at C3.1 (depth-1 composition limit). Caller surfaces
            // the appropriate diagnostic.
            // C4.1: `?Class` deferred (NullableInner gains no Class
            // variant at C4.1; `?Class` shows up naturally only when
            // classes become storable in arrays / nullable
            // wrappers).
            Type::Array(_)
            | Type::Nullable(_)
            | Type::Secret(_)
            | Type::Kont(_)
            | Type::Task(_)
            // ADR 0066 M1.2: no `?Channel` / `[Channel]` (a channel handle
            // is not a nullable payload nor an array element at M1.2).
            | Type::Channel(_)
            | Type::Process
            // ADR 0066 M2.4a: no `?SealedChannel` (a handle, like `Process`).
            | Type::SealedChannel
            | Type::Class(_)
            | Type::TraitSelf(_)
            // Phase D.1 / ADR 0032: `?Enum` is out of scope at the MVP
            // (NullableInner gains no Enum variant) — `?Enum` shows up
            // only when enums become storable in nullable wrappers.
            | Type::Enum(_)
            // Phase D.3 / ADR 0034 D8: `?Vec<T>` is out of scope
            // (NullableInner gains no Vec variant).
            | Type::Vec(_)
            // ADR 0070: no `?Fn` (a handle, like `Process`/`SealedChannel`).
            | Type::Fn(_)
            // ADR 0071 M1.4a: no `?Shared` (a handle).
            | Type::Shared(_)
            // ADR 0071 M1.4b: no `?Mutex` (a handle).
            | Type::Mutex(_) => None,
        }
    }

    /// Try to demote this Type to an [`ArrayElem`] for use as the
    /// payload of an `Array`. Returns `None` for `Array` (would be
    /// nested per ADR 0015 D6) AND for `Nullable` (the `[?T]`
    /// combination is deferred per C1.6's depth-1 amendment).
    /// `Type::Ref` is also rejected — refs in arrays need named
    /// regions per ADR 0017 D7 / D12 (`RefInArray`), so this
    /// returns `None` for refs and the caller surfaces the right
    /// diagnostic.
    pub fn to_array_elem(self) -> Option<ArrayElem> {
        match self {
            Type::I64 => Some(ArrayElem::I64),
            Type::I32 => Some(ArrayElem::I32),
            Type::Bool => Some(ArrayElem::Bool),
            // Phase D.2 / ADR 0033 D3: `[u8]` IS the string type, so
            // `u8` must demote to an array element.
            Type::U8 => Some(ArrayElem::U8),
            Type::Struct(id) => Some(ArrayElem::Struct(id)),
            Type::TypeParam(id) => Some(ArrayElem::TypeParam(id)),
            Type::GenericInstance(id) => Some(ArrayElem::GenericInstance(id)),
            // C3 / ADR 0019 D5: `[secret T]` is not yet
            // representable at C3.1 — ArrayElem has no Secret
            // variant. `secret [T]` (array-of-secret-elements at
            // the outer-secret level) IS representable via
            // `Type::Secret(secret_id_for_[T])` and works through
            // the regular Secret arm.
            // ADR 0055: `[u128]` is out of scope (ArrayElem gains no U128
            // variant) — the radix-2^51 field uses scalar `u128`.
            Type::U128
            // ADR 0058: `[f64]` is out of scope (ArrayElem gains no F64
            // variant) — scalar `f64` only this increment.
            | Type::F64
            // ADR 0057: `[ptr]` is out of scope — `ptr` is an FFI-only opaque.
            | Type::Ptr
            | Type::Array(_)
            | Type::Nullable(_)
            | Type::Ref(_)
            | Type::Secret(_)
            | Type::Kont(_)
            | Type::Task(_)
            // ADR 0066 M1.2: no `?Channel` / `[Channel]` (a channel handle
            // is not a nullable payload nor an array element at M1.2).
            | Type::Channel(_)
            | Type::Process
            // ADR 0066 M2.4a: no `[SealedChannel]` (a handle, like `Process`).
            | Type::SealedChannel
            | Type::Class(_)
            | Type::TraitSelf(_)
            // Phase D.1 / ADR 0032: `[Enum]` is out of scope at the MVP
            // (ArrayElem gains no Enum variant).
            | Type::Enum(_)
            // Phase D.3 / ADR 0034 D8: `[Vec<T>]` (array-of-vec) is out
            // of scope — VecElem/ArrayElem are the flat subset, no
            // collection nesting at the MVP.
            | Type::Vec(_)
            // ADR 0070: no `[Fn]` (a handle, like `Process`/`SealedChannel`).
            | Type::Fn(_)
            // ADR 0071 M1.4a: no `[Shared]` (a handle).
            | Type::Shared(_)
            // ADR 0071 M1.4b: no `[Mutex]` (a handle).
            | Type::Mutex(_) => None,
        }
    }

    /// ADR 0047: demote this Type to an [`ArrayElem`] for use as an array
    /// element, additionally admitting a `secret` SCALAR element (`[secret u8]`)
    /// — the form the bare [`Type::to_array_elem`] rejects. A `Type::Secret(id)`
    /// is admitted **only if** the secret wraps a flat scalar (i.e. its
    /// `secrets[id].inner` itself demotes), so `[secret [T]]` / `[secret ?T]`
    /// stay rejected and the depth-1 no-nested-collection rule holds. Used at
    /// the array-element resolution sites (the `[T]` annotation arm + the two
    /// array-literal demotes); the bare `to_array_elem` stays conservative for
    /// callers without the `secrets` interner (e.g. generic substitution, where
    /// secrets do not compose today).
    pub fn to_array_elem_secret(self, secrets: &[SecretData]) -> Option<ArrayElem> {
        if let Type::Secret(id) = self {
            // `[secret SCALAR]` only: the secret's own inner must demote.
            let inner = secrets[id.0 as usize].inner;
            return inner.to_array_elem().map(|_| ArrayElem::Secret(id));
        }
        self.to_array_elem()
    }

    /// ADR 0053: demote for the generic SUBSTITUTION round-trip — the array twin of
    /// ADR 0052's [`Type::to_vec_elem_subst`]. Like [`Type::to_array_elem`] but also
    /// admits a `secret SCALAR` element produced by substituting a TypeParam, so
    /// `vec_to_array<T>(Vec<T>) -> [T]` instantiated with `T := secret u8` yields
    /// `[secret u8]` (the `Vec<secret u8> → [secret u8]` bridge; else it falls back
    /// to `[T]` and the annotation match fails). The [`SecretId`] is taken straight
    /// from the `Type::Secret`, so no `secrets` table is needed (the resolution-site
    /// [`Type::to_array_elem_secret`] validates scalar-ness). A `[secret NON-scalar]`
    /// is unreachable on this path: every source spelling is rejected at resolution,
    /// so a `secret` reaching here wraps a scalar (see ADR 0053).
    fn to_array_elem_subst(self) -> Option<ArrayElem> {
        if let Type::Secret(id) = self {
            return Some(ArrayElem::Secret(id));
        }
        self.to_array_elem()
    }

    /// Try to demote this Type to a [`VecElem`] for use as the element
    /// of a `Vec`. Mirrors [`Type::to_array_elem`]: returns `None` for
    /// every type outside the flat subset (Array, Vec, Nullable, Ref,
    /// and the rest), so `Vec<[T]>` / `Vec<Vec<T>>` / `Vec<?T>` /
    /// `Vec<&T>` are rejected at resolve-type-expr time (ADR 0034 D8).
    pub fn to_vec_elem(self) -> Option<VecElem> {
        match self {
            Type::I64 => Some(VecElem::I64),
            Type::I32 => Some(VecElem::I32),
            Type::Bool => Some(VecElem::Bool),
            // Phase D.3 / ADR 0034 D5: `Vec<u8>` is the growable byte
            // buffer (the D.3 (2/N) `String`).
            Type::U8 => Some(VecElem::U8),
            Type::Struct(id) => Some(VecElem::Struct(id)),
            Type::TypeParam(id) => Some(VecElem::TypeParam(id)),
            Type::GenericInstance(id) => Some(VecElem::GenericInstance(id)),
            // ADR 0055: `Vec<u128>` is out of scope (VecElem gains no U128
            // variant) — the radix-2^51 field uses scalar `u128`.
            Type::U128
            // ADR 0058: `Vec<f64>` is out of scope (VecElem gains no F64
            // variant) — scalar `f64` only this increment.
            | Type::F64
            // ADR 0057: `Vec<ptr>` is out of scope — `ptr` is an FFI-only opaque.
            | Type::Ptr
            | Type::Array(_)
            | Type::Vec(_)
            | Type::Nullable(_)
            | Type::Ref(_)
            | Type::Secret(_)
            | Type::Kont(_)
            | Type::Task(_)
            // ADR 0066 M1.2: no `?Channel` / `[Channel]` (a channel handle
            // is not a nullable payload nor an array element at M1.2).
            | Type::Channel(_)
            | Type::Process
            // ADR 0066 M2.4a: no `Vec<SealedChannel>` (a handle, like `Process`).
            | Type::SealedChannel
            | Type::Class(_)
            | Type::TraitSelf(_)
            | Type::Enum(_)
            // ADR 0070: no `Vec<Fn>` (a handle, like `Process`/`SealedChannel`).
            | Type::Fn(_)
            // ADR 0071 M1.4a: no `Vec<Shared>` (a handle).
            | Type::Shared(_)
            // ADR 0071 M1.4b: no `Vec<Mutex>` (a handle).
            | Type::Mutex(_) => None,
        }
    }

    /// ADR 0052: demote this Type to a [`VecElem`] for use as a `Vec` element,
    /// additionally admitting a `secret` SCALAR element (`Vec<secret u8>`) — the
    /// form the bare [`Type::to_vec_elem`] rejects. Exactly mirrors
    /// [`Type::to_array_elem_secret`]: a `Type::Secret(id)` is admitted **only if**
    /// the secret wraps a flat scalar (its `secrets[id].inner` itself demotes), so
    /// `Vec<secret [T]>` / `Vec<secret ?T>` stay rejected and the depth-1
    /// no-nested-collection rule holds. Used at the `Vec<T>` annotation-resolution
    /// site; the bare `to_vec_elem` stays conservative for callers without the
    /// `secrets` interner.
    pub fn to_vec_elem_secret(self, secrets: &[SecretData]) -> Option<VecElem> {
        if let Type::Secret(id) = self {
            // `Vec<secret SCALAR>` only: the secret's own inner must demote.
            let inner = secrets[id.0 as usize].inner;
            return inner.to_vec_elem().map(|_| VecElem::Secret(id));
        }
        self.to_vec_elem()
    }

    /// ADR 0052: demote for the generic SUBSTITUTION round-trip. Like
    /// [`Type::to_vec_elem`] but also admits a `secret SCALAR` element produced by
    /// substituting a TypeParam, so `Vec<T>[T := secret u8] = Vec<secret u8>` (the
    /// `Vec` builtins `vec_new`/`push` are generic over the element, so the element
    /// TypeParam may bind to `secret u8`). The [`SecretId`] is taken straight from
    /// the `Type::Secret`, so — unlike the resolution-site
    /// [`Type::to_vec_elem_secret`] — no `secrets` table is needed (and
    /// `Type::substitute` need not thread one, sidestepping the ripple ADR 0047 A2
    /// avoided). A `Vec<secret NON-scalar>` is unreachable on this path: every
    /// source spelling is rejected at resolution by `to_vec_elem_secret`, so a
    /// `secret` reaching here wraps a scalar (see ADR 0052 "Scope / Deferred").
    fn to_vec_elem_subst(self) -> Option<VecElem> {
        if let Type::Secret(id) = self {
            return Some(VecElem::Secret(id));
        }
        self.to_vec_elem()
    }

    /// Substitute every [`Type::TypeParam`] inside `self` against
    /// the given substitution map. Used at generic-fn call sites
    /// per ADR 0016 D7c to compute the concrete parameter / return
    /// type of a monomorphic instantiation. Handles substitution
    /// through `Nullable`, `Array`, `GenericInstance`, and `Ref`
    /// payloads.
    ///
    /// The mutable `instances` and `refs` tables are extended
    /// whenever substitution would produce a new interned pair not
    /// already present — this is what enables transitive
    /// monomorphisation of nested generics like `Pair<Box<T>, T>`
    /// (and `&T` inside generic fns) without losing `Type: Copy`.
    ///
    /// Concrete (non-TypeParam, non-GenericInstance-with-
    /// TypeParam-args) types pass through unchanged.
    pub fn substitute(
        self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> Type {
        match self {
            // Classes (like structs) don't carry TypeParam payloads
            // at C4.1 (generic classes deferred per ADR 0022 D1);
            // substitution is the identity.
            Type::I64
            | Type::I32
            // Phase D.2 / ADR 0033 D4: `u8` is a scalar primitive with
            // no TypeParam payload — substitution is the identity.
            | Type::U8
            // ADR 0055: `u128` is likewise a scalar — identity.
            | Type::U128
            // ADR 0058: `f64` is likewise a scalar — identity.
            | Type::F64
            // ADR 0057: `ptr` is a scalar opaque — substitution is the identity.
            | Type::Ptr
            | Type::Bool
            | Type::Struct(_)
            | Type::Class(_)
            | Type::TraitSelf(_)
            // Phase D.1 / ADR 0032: enums are non-generic at the MVP —
            // no TypeParam payload, so substitution is the identity.
            | Type::Enum(_) => self,
            Type::TypeParam(id) => {
                let idx = id.0 as usize;
                debug_assert!(
                    idx < subst.len(),
                    "TypeParam({idx}) out of substitution range ({len})",
                    len = subst.len()
                );
                subst[idx]
            }
            Type::Nullable(ni) => {
                let inner = ni.to_type();
                let new_inner = inner.substitute(subst, instances, refs);
                match new_inner.to_nullable_inner() {
                    Some(new_ni) => Type::Nullable(new_ni),
                    // Substitution can't legally produce a nested
                    // nullable at C1.7 (no `?(?T)` shape exists in
                    // the surface), but if a future representation
                    // allows it, fall back to the unsubstituted
                    // value rather than panicking.
                    None => Type::Nullable(ni),
                }
            }
            Type::Array(ae) => {
                // ADR 0068: a nested-array element is interned (concrete) — it
                // carries no TypeParam, so substitution is the identity. (Generic
                // nested arrays `[[T]]` are deferred, D6; `ae.to_type()` would
                // panic on `Array`, so short-circuit.)
                if let ArrayElem::Array(_) = ae {
                    return Type::Array(ae);
                }
                let inner = ae.to_type();
                let new_inner = inner.substitute(subst, instances, refs);
                // ADR 0053: secret-aware demote so `vec_to_array<T>() -> [T]` over
                // `T := secret u8` yields `[secret u8]` (mirrors the Vec arm).
                match new_inner.to_array_elem_subst() {
                    Some(new_ae) => Type::Array(new_ae),
                    None => Type::Array(ae),
                }
            }
            // Phase D.3 / ADR 0034: substitute through a Vec's element,
            // mirroring the Array arm exactly.
            Type::Vec(ve) => {
                let inner = ve.to_type();
                let new_inner = inner.substitute(subst, instances, refs);
                // ADR 0052: `to_vec_elem_subst` round-trips a substituted
                // `secret SCALAR` element to `VecElem::Secret`, so
                // `vec_new<T>() -> Vec<T>` instantiated with `T = secret u8`
                // yields `Vec<secret u8>` (else it falls back to `Vec<T>` and the
                // `let`-annotation match fails).
                match new_inner.to_vec_elem_subst() {
                    Some(new_ve) => Type::Vec(new_ve),
                    None => Type::Vec(ve),
                }
            }
            Type::GenericInstance(id) => {
                let idx = id.0 as usize;
                let (struct_id, old_args) = {
                    let data = &instances[idx];
                    (data.struct_id, data.args.clone())
                };
                let new_args: Vec<Type> = old_args
                    .into_iter()
                    .map(|a| a.substitute(subst, instances, refs))
                    .collect();
                let new_id = intern_generic_instance(instances, struct_id, new_args);
                Type::GenericInstance(new_id)
            }
            Type::Ref(id) => {
                // C2 / ADR 0017 D11: substitute through the ref's
                // inner type and re-intern. Mirrors the
                // GenericInstance branch's read-clone-substitute-
                // intern shape.
                let idx = id.0 as usize;
                let (mutable, old_inner) = {
                    let data = &refs[idx];
                    (data.mutable, data.inner)
                };
                let new_inner = old_inner.substitute(subst, instances, refs);
                let new_id = intern_ref(refs, mutable, new_inner);
                Type::Ref(new_id)
            }
            Type::Secret(_) => {
                // C3 / ADR 0019 D5: secrets don't carry TypeParams
                // at the C3.1 minimum (secret of a TypeParam is
                // structurally weird; future ADRs may relax). Pass
                // through unchanged. The mutable `secrets` table is
                // threaded by [`substitute_secret`] in places that
                // need to re-intern after substituting the inner.
                self
            }
            Type::Kont(_) => {
                // C3.4 / ADR 0020 D5: konts only appear inside
                // handler-arm bodies; generic substitution can't
                // reach them at C3.4 minimum (handlers + generics
                // don't compose yet). Pass through unchanged — a
                // future ADR may revisit if effect-polymorphism
                // lands per ADR 0020 D10.
                self
            }
            Type::Task(_) => {
                // C4.4 / ADR 0024 D4: `Task<T>` is `Task<i64>` at
                // C4.4 minimum — no TypeParam payload to substitute.
                // Pass through unchanged (generic tasks deferred per
                // ADR 0024 D10).
                self
            }
            // ADR 0066 M1.2: `Channel<i64>` carries no TypeParam at the
            // minimum — pass through unchanged.
            Type::Channel(_) => self,
            // ADR 0071 M1.4a: `Shared<T>`'s word-scalar element carries no
            // TypeParam — pass through unchanged.
            Type::Shared(_) => self,
            // ADR 0071 M1.4b: `Mutex<T>`'s word-scalar element carries no
            // TypeParam — pass through unchanged.
            Type::Mutex(_) => self,
            // ADR 0066 M2.1: `Process` carries no TypeParam — pass through.
            Type::Process => self,
            // ADR 0066 M2.4a: `SealedChannel` carries no TypeParam — pass through.
            Type::SealedChannel => self,
            // ADR 0070 (generalized): Fn<T,R>'s T/R are always concrete
            // word-scalars, never a TypeParam — pass through unchanged.
            Type::Fn(_) => self,
        }
    }

    /// C3 / ADR 0019 D5 (C3.1): does this type carry the `secret`
    /// qualifier at the outermost layer?
    pub fn is_secret(self) -> bool {
        matches!(self, Type::Secret(_))
    }

    /// ADR 0051: `true` if `self` is a public→secret widen TARGET — a `secret T`
    /// scalar or a `[secret T]` array. Used by the bidirectional expected-type
    /// pushdown (call args, returns) so a public value / public `[u8]` flowing
    /// into a secret position is widened by [`coerce_to_expected`].
    pub fn is_secret_widen_target(self) -> bool {
        matches!(self, Type::Secret(_) | Type::Array(ArrayElem::Secret(_)))
    }

    /// C3 / ADR 0019 D5 (C3.1): strip one layer of `secret` if
    /// present. Returns `(inner, was_secret)`. Used by operator
    /// typing to compute the underlying-type compatibility check
    /// while preserving the secret qualifier in the result.
    pub fn strip_secret(self, secrets: &[SecretData]) -> (Type, bool) {
        match self {
            Type::Secret(id) => (secrets[id.0 as usize].inner, true),
            other => (other, false),
        }
    }
}

/// Intern a `(struct_id, args)` pair into `instances`, returning
/// its [`GenericInstanceId`]. Linear search — at C1.7 scale (few
/// instances per program) this is fine. Profile-driven HashMap
/// interning is a trivial future optimisation per ADR 0016
/// "Consequences/Negative". Per ADR 0016 D6a.
pub fn intern_generic_instance(
    instances: &mut Vec<GenericInstanceData>,
    struct_id: StructId,
    args: Vec<Type>,
) -> GenericInstanceId {
    for (idx, existing) in instances.iter().enumerate() {
        if existing.struct_id == struct_id && existing.args == args {
            return GenericInstanceId(idx as u32);
        }
    }
    let id = GenericInstanceId(instances.len() as u32);
    instances.push(GenericInstanceData { struct_id, args });
    id
}

/// Intern a secret-of-inner pair into `secrets`, returning its
/// [`SecretId`]. Linear search; same scale + future-optimisation
/// story as [`intern_generic_instance`]. C3 / ADR 0019 D5 (C3.1).
///
/// Idempotent on already-secret types: `intern_secret(s, secret T)`
/// returns the existing SecretId for `secret T` (collapses; the
/// depth-1 rule from D5 — `secret secret T` is rejected at parse
/// time, but substitution could try to produce one; we defend in
/// depth here by no-op'ing).
pub fn intern_secret(secrets: &mut Vec<SecretData>, inner: Type) -> SecretId {
    // Defensive: if the inner is already secret, return the
    // existing id rather than nesting. Parse-time DoubleSecret
    // rejection should make this unreachable for source-level
    // inputs.
    if let Type::Secret(existing) = inner {
        return existing;
    }
    for (idx, existing) in secrets.iter().enumerate() {
        if existing.inner == inner {
            return SecretId(idx as u32);
        }
    }
    let id = SecretId(secrets.len() as u32);
    secrets.push(SecretData { inner });
    id
}

/// Intern a `(mutable, inner)` pair into `refs`, returning its
/// [`RefId`]. Linear search; same scale + future-optimisation story
/// as [`intern_generic_instance`]. C2 / ADR 0017 D11.
pub fn intern_ref(refs: &mut Vec<RefData>, mutable: bool, inner: Type) -> RefId {
    for (idx, existing) in refs.iter().enumerate() {
        if existing.mutable == mutable && existing.inner == inner {
            return RefId(idx as u32);
        }
    }
    let id = RefId(refs.len() as u32);
    refs.push(RefData { mutable, inner });
    id
}

/// Intern a `(arg_ty, ret_ty)` pair into `konts`, returning its
/// [`KontId`] per ADR 0020 D5 (C3.4). Linear search; one kont per
/// handler arm in practice, so the scale is even smaller than
/// the other interner tables.
pub fn intern_kont(konts: &mut Vec<KontData>, arg_ty: Type, ret_ty: Type) -> KontId {
    for (idx, existing) in konts.iter().enumerate() {
        if existing.arg_ty == arg_ty && existing.ret_ty == ret_ty {
            return KontId(idx as u32);
        }
    }
    let id = KontId(konts.len() as u32);
    konts.push(KontData { arg_ty, ret_ty });
    id
}

/// ADR 0070 (generalized): word-scalar index for one axis of a `Fn<T,R>`
/// signature — the same 6-element enumeration + order as
/// [`channel_chanid_for`] (kept independent/duplicated rather than reused,
/// so the two features don't couple on an incidental shared numbering).
fn fn_value_word_scalar_index(ty: Type) -> Option<u32> {
    match ty {
        Type::I64 => Some(0),
        Type::I32 => Some(1),
        Type::U8 => Some(2),
        Type::Bool => Some(3),
        Type::F64 => Some(4),
        Type::Ptr => Some(5),
        _ => None,
    }
}

/// The inverse of [`fn_value_word_scalar_index`].
fn fn_value_word_scalar_at(idx: u32) -> Type {
    match idx {
        1 => Type::I32,
        2 => Type::U8,
        3 => Type::Bool,
        4 => Type::F64,
        5 => Type::Ptr,
        _ => Type::I64,
    }
}

/// ADR 0070 (generalized): compute the [`FnValueSigId`] for a `Fn<param_ty,
/// ret_ty>` value type — `None` unless BOTH are word-scalars. Pure
/// arithmetic (`param_index * 6 + ret_index`); no interner table, so no
/// threading through `resolve_type_expr` or the check pipeline.
pub fn fn_value_sig_id_for(param_ty: Type, ret_ty: Type) -> Option<FnValueSigId> {
    let p = fn_value_word_scalar_index(param_ty)?;
    let r = fn_value_word_scalar_index(ret_ty)?;
    Some(FnValueSigId(p * 6 + r))
}

/// The inverse of [`fn_value_sig_id_for`]: recover `(param_ty, ret_ty)`
/// from a `Type::Fn(id)`. Total (defensive `(i64, i64)` default for an
/// out-of-range id, mirroring [`channel_elem_for`]).
pub fn fn_value_sig_param_ret(id: FnValueSigId) -> (Type, Type) {
    (fn_value_word_scalar_at(id.0 / 6), fn_value_word_scalar_at(id.0 % 6))
}

/// ADR 0066 M1.1: is `ty` a **word-sized scalar** that the per-spawn
/// wrapper can pack into an 8-byte arg slot and encode into the Task's
/// `i64` result slot? These are the single-LLVM-value types ≤ 8 bytes:
/// the scalars `i64`/`i32`/`u8`/`bool`/`f64`/`ptr` and the pointer-lowered
/// `Task` handle. Codegen encodes the result to/from `i64` (zext/trunc for
/// narrow ints, bitcast for `f64`, ptrtoint/inttoptr for pointers), so the
/// runtime `SentinelTask.result` slot stays `i64` and existing `Task<i64>`
/// IR is byte-identical (the i64 case is a no-op).
///
/// `Class` is intentionally NOT word-scalar: a class instance is a named
/// aggregate VALUE (`%Class.N`), not a pointer, so it doesn't fit the
/// 8-byte slot. Deferred with a clean `SpawnTypeUnsupported` diagnostic:
/// `u128` (16 bytes) and the aggregates `struct`/`class`/`enum`/`Vec`/
/// `[T]`/`?T`, which need a wider slot plus a boxed result (a D6 follow-on);
/// and `secret`, where a secret crossing a thread boundary lands with
/// channels (ADR 0066 D8 / M1.2).
pub fn is_spawn_word_scalar(ty: Type) -> bool {
    matches!(
        ty,
        Type::I64
            | Type::I32
            | Type::U8
            | Type::Bool
            | Type::F64
            | Type::Ptr
            | Type::Task(_)
            // ADR 0066 M1.2/D4: a `Channel<T>` handle is a pointer — passing a
            // channel endpoint into a spawned producer/consumer is the whole
            // point of the worker pattern.
            | Type::Channel(_)
            | Type::Process
            // ADR 0066 M2.4a: a `SealedChannel` handle is a pointer (like `Process`).
            | Type::SealedChannel
            // ADR 0071 M1.4a: a `Shared<T>` handle is a pointer — capturing a shared
            // cell into a spawned worker is a core use (the refcount `clone` at the
            // spawn site + the task's own scope-exit `release` keep it balanced).
            | Type::Shared(_)
    )
}

/// ADR 0071 M1.4a slice 3: is `expr` a bare `Var` (recursing through a trivial Block
/// wrapper, matching how a tail is structured) of `Shared<T>` type? Such a value in
/// return position (a fn tail or an explicit `return`) TRANSFERS a refcount unit to
/// the caller — but the transfer exemption in the drop drain is only implemented in
/// inkwell (via `tail_returned_var`); the byte-identical `snc llvm` oracle + scg
/// mirror is a deferred follow-on (slice 3b), so returning a named `Shared` is
/// guarded (rejected) to keep the emitted IR sound. An RVALUE return (a
/// `shared_new(...)` result / any call) is not a named binding → allowed.
fn is_named_shared_return(expr: &TypedExpr) -> bool {
    match &expr.kind {
        TypedExprKind::Var(_) => matches!(expr.ty, Type::Shared(_)),
        TypedExprKind::Block(b) => is_named_shared_return(&b.tail),
        _ => false,
    }
}

/// ADR 0066 M2.3b: the element types a `process_send`/`process_recv` framed
/// channel over a pipe can carry — the word-sized scalars that also have a `?T`
/// form (so `process_recv -> ?T` is representable): `i64`/`i32`/`u8`/`bool`/
/// `f64`/`ptr`. Each is encoded into the 8-byte LE i64 frame (the M1.1 spawn
/// encode: zext a narrow int / bitcast an `f64` / ptrtoint a `ptr`), so the
/// runtime stays i64-based — no runtime/ABI change. A `secret` element is NOT a
/// word scalar, so it is rejected here: the cross-process secret fence (D8).
/// `u128` (16 bytes) doesn't fit the i64 frame and is excluded (not a word
/// scalar); the `Task`/`Channel`/`Process` handles have no `?T` form and would be
/// nonsensical across a process boundary anyway.
pub fn is_process_channel_elem(ty: Type) -> bool {
    is_spawn_word_scalar(ty) && ty.to_nullable_inner().is_some()
}

/// ADR 0066 M1.2b-cont: the word-scalar in-process channel element types are
/// pre-interned at FIXED [`ChanId`]s 0..=5 during channel-builtin signature setup,
/// so a `Channel<T>` annotation maps to a stable `ChanId` WITHOUT threading the
/// `channels` interner through the checker (the snag the generic-channel design
/// avoids). Returns the fixed `ChanId` index for a word-scalar `elem`, or `None`
/// for a non-channel element. The set matches [`is_process_channel_elem`].
pub fn channel_chanid_for(elem: Type) -> Option<u32> {
    match elem {
        Type::I64 => Some(0),
        Type::I32 => Some(1),
        Type::U8 => Some(2),
        Type::Bool => Some(3),
        Type::F64 => Some(4),
        Type::Ptr => Some(5),
        _ => None,
    }
}

/// The inverse of [`channel_chanid_for`]: the element type of a pre-interned channel
/// `ChanId`. Lets `send`/`recv`/`channel_close` read a channel arg's element from its
/// `Type::Channel(id)` alone (no `channels`-table access in the checker). `i64` for
/// an unknown id (only the M1.2 singleton existed before; defensive).
pub fn channel_elem_for(id: ChanId) -> Type {
    match id.0 {
        1 => Type::I32,
        2 => Type::U8,
        3 => Type::Bool,
        4 => Type::F64,
        5 => Type::Ptr,
        _ => Type::I64,
    }
}

/// Intern a `result_ty` into `tasks`, returning its [`TaskId`] per
/// ADR 0024 D4 (C4.4), generalised by ADR 0066 M1.1 to any word-sized
/// scalar `result_ty` (see [`is_spawn_word_scalar`]). Linear search,
/// dedup by structural equality; mirrors [`intern_kont`]'s shape.
pub fn intern_task(tasks: &mut Vec<TaskData>, result_ty: Type) -> TaskId {
    for (idx, existing) in tasks.iter().enumerate() {
        if existing.result_ty == result_ty {
            return TaskId(idx as u32);
        }
    }
    let id = TaskId(tasks.len() as u32);
    tasks.push(TaskData { result_ty });
    id
}

/// Intern an `elem_ty` into `channels`, returning its [`ChanId`] per
/// ADR 0066 M1.2. Linear search, dedup by structural equality; mirrors
/// [`intern_task`]'s shape.
pub fn intern_channel(channels: &mut Vec<ChannelData>, elem_ty: Type) -> ChanId {
    for (idx, existing) in channels.iter().enumerate() {
        if existing.elem_ty == elem_ty {
            return ChanId(idx as u32);
        }
    }
    let id = ChanId(channels.len() as u32);
    channels.push(ChannelData { elem_ty });
    id
}

/// ADR 0071 M1.4a: the `Shared<T>` word-scalar element types are pre-interned at
/// FIXED [`SharedId`]s 0..=5 during builtin signature setup (mirroring
/// [`channel_chanid_for`]), so a `Shared<T>` annotation / `shared_new(v)` result
/// maps to a stable `SharedId` WITHOUT threading the `shared` interner through the
/// checker. Returns the fixed index for a word-scalar `elem`, else `None`.
pub fn shared_id_for(elem: Type) -> Option<u32> {
    match elem {
        Type::I64 => Some(0),
        Type::I32 => Some(1),
        Type::U8 => Some(2),
        Type::Bool => Some(3),
        Type::F64 => Some(4),
        Type::Ptr => Some(5),
        _ => None,
    }
}

/// The inverse of [`shared_id_for`]: the element type of a pre-interned
/// `Shared<T>` `SharedId`. Lets `shared_get` read a shared arg's element from its
/// `Type::Shared(id)` alone (no `shared`-table access in the checker). `i64` for
/// an unknown id (defensive). Mirrors [`channel_elem_for`].
pub fn shared_elem_for(id: SharedId) -> Type {
    match id.0 {
        1 => Type::I32,
        2 => Type::U8,
        3 => Type::Bool,
        4 => Type::F64,
        5 => Type::Ptr,
        _ => Type::I64,
    }
}

/// Intern an `elem_ty` into `shared`, returning its [`SharedId`] per ADR 0071
/// M1.4a. Linear search, dedup by structural equality; mirrors [`intern_channel`].
pub fn intern_shared(shared: &mut Vec<SharedData>, elem_ty: Type) -> SharedId {
    for (idx, existing) in shared.iter().enumerate() {
        if existing.elem_ty == elem_ty {
            return SharedId(idx as u32);
        }
    }
    let id = SharedId(shared.len() as u32);
    shared.push(SharedData { elem_ty });
    id
}

/// ADR 0071 M1.4b: the `Mutex<T>` word-scalar element types are pre-interned at
/// FIXED [`MutexId`]s 0..=5 during builtin signature setup (mirroring
/// [`shared_id_for`]), so a `Mutex<T>` annotation / `mutex_new(v)` result maps to
/// a stable `MutexId` WITHOUT threading the `mutexes` interner. Returns the fixed
/// index for a word-scalar `elem`, else `None`.
pub fn mutex_id_for(elem: Type) -> Option<u32> {
    match elem {
        Type::I64 => Some(0),
        Type::I32 => Some(1),
        Type::U8 => Some(2),
        Type::Bool => Some(3),
        Type::F64 => Some(4),
        Type::Ptr => Some(5),
        _ => None,
    }
}

/// The inverse of [`mutex_id_for`]: the element type of a pre-interned `Mutex<T>`
/// `MutexId`. Lets `lock` read a mutex arg's element from its `Type::Mutex(id)`
/// alone (no `mutexes`-table access in the checker). `i64` for an unknown id
/// (defensive). Mirrors [`shared_elem_for`].
pub fn mutex_elem_for(id: MutexId) -> Type {
    match id.0 {
        1 => Type::I32,
        2 => Type::U8,
        3 => Type::Bool,
        4 => Type::F64,
        5 => Type::Ptr,
        _ => Type::I64,
    }
}

/// Intern an `elem_ty` into `mutexes`, returning its [`MutexId`] per ADR 0071
/// M1.4b. Linear search, dedup by structural equality; mirrors [`intern_shared`].
pub fn intern_mutex(mutexes: &mut Vec<MutexData>, elem_ty: Type) -> MutexId {
    for (idx, existing) in mutexes.iter().enumerate() {
        if existing.elem_ty == elem_ty {
            return MutexId(idx as u32);
        }
    }
    let id = MutexId(mutexes.len() as u32);
    mutexes.push(MutexData { elem_ty });
    id
}

/// ADR 0068: intern a nested array's inner [`ArrayElem`] into `arrays`,
/// returning its [`ArrayId`]. Linear search, dedup by structural equality;
/// mirrors [`intern_channel`]'s shape. `Type::Array(ArrayElem::Array(id))` is
/// then the `[[T]]` whose inner element is `arrays[id]`.
pub fn intern_array_elem(arrays: &mut Vec<ArrayElem>, elem: ArrayElem) -> ArrayId {
    for (idx, existing) in arrays.iter().enumerate() {
        if *existing == elem {
            return ArrayId(idx as u32);
        }
    }
    let id = ArrayId(arrays.len() as u32);
    arrays.push(elem);
    id
}

/// ADR 0068: demote an array-element [`Type`] to an [`ArrayElem`], interning a
/// NESTED array (`[[T]]`'s element is itself a `Type::Array`) into `arrays`. The
/// non-nested path is the flat / `secret SCALAR` demote ([`Type::to_array_elem_secret`]).
/// `None` for a genuinely-unrepresentable element (`[?T]`, `[&T]`, `[secret [T]]`).
/// Shared by `resolve_type_expr`'s `Array` arm and the array-literal type-check.
fn array_elem_of(
    elem_ty: Type,
    secrets: &[SecretData],
    arrays: &mut Vec<ArrayElem>,
) -> Option<ArrayElem> {
    if let Type::Array(inner_ae) = elem_ty {
        return Some(ArrayElem::Array(intern_array_elem(arrays, inner_ae)));
    }
    elem_ty.to_array_elem_secret(secrets)
}

/// ADR 0068: promote an [`ArrayElem`] to its [`Type`] given the `arrays` interner
/// slice — the free-function form of [`TypedProgram::array_elem_type`], for the
/// type-check sites that hold the in-progress `arrays` vec rather than a finished
/// [`TypedProgram`]. For a flat element this is `ae.to_type()`; for `Array(id)` it
/// is the nested `[inner]` = `Type::Array(arrays[id])`.
fn array_elem_type_in(ae: ArrayElem, arrays: &[ArrayElem]) -> Type {
    match ae {
        ArrayElem::Array(id) => Type::Array(arrays[id.0 as usize]),
        flat => flat.to_type(),
    }
}

/// Format a [`Type`] for display, looking up the struct name when
/// the type is `Struct(StructId)`. Pass `None` when no program is
/// available (e.g. error rendering in tests) — struct types render
/// as `<struct#N>` in that case.
pub fn type_display(ty: Type, program: Option<&TypedProgram>) -> String {
    match ty {
        Type::I64 => "i64".to_string(),
        Type::I32 => "i32".to_string(),
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U128 => "u128".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Ptr => "ptr".to_string(),
        Type::Struct(id) => match program.and_then(|p| p.structs.get(id.0 as usize)) {
            Some(s) => s.name.clone(),
            None => format!("<struct#{}>", id.0),
        },
        Type::Nullable(inner) => format!("?{}", type_display(inner.to_type(), program)),
        // ADR 0068: render `[[T]]` by resolving the nested element via the program's
        // `arrays` interner when available (the full dump); without a program, fall
        // back to an id-placeholder for a nested element (flat elements still render).
        Type::Array(elem) => {
            let inner = match (elem, program) {
                (ArrayElem::Array(id), None) => return format!("[<arr#{}>]", id.0),
                (ae, Some(p)) => p.array_elem_type(ae),
                (ae, None) => ae.to_type(),
            };
            format!("[{}]", type_display(inner, program))
        }
        Type::Vec(elem) => format!("Vec<{}>", type_display(elem.to_type(), program)),
        Type::TypeParam(id) => format!("<T#{}>", id.0),
        Type::GenericInstance(id) => {
            if let Some(p) = program {
                if let Some(inst) = p.generic_instances.get(id.0 as usize) {
                    let name = p
                        .structs
                        .get(inst.struct_id.0 as usize)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| format!("<struct#{}>", inst.struct_id.0));
                    let args = inst
                        .args
                        .iter()
                        .map(|a| type_display(*a, program))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("{name}<{args}>");
                }
            }
            format!("<gi#{}>", id.0)
        }
        Type::Ref(id) => {
            if let Some(p) = program {
                if let Some(data) = p.refs.get(id.0 as usize) {
                    let inner = type_display(data.inner, program);
                    return if data.mutable {
                        format!("&mut {inner}")
                    } else {
                        format!("&{inner}")
                    };
                }
            }
            format!("<ref#{}>", id.0)
        }
        Type::Secret(id) => {
            if let Some(p) = program {
                if let Some(data) = p.secrets.get(id.0 as usize) {
                    return format!("secret {}", type_display(data.inner, program));
                }
            }
            format!("<secret#{}>", id.0)
        }
        Type::Kont(id) => {
            if let Some(p) = program {
                if let Some(data) = p.konts.get(id.0 as usize) {
                    let arg = type_display(data.arg_ty, program);
                    let ret = type_display(data.ret_ty, program);
                    return format!("kont({arg}) -> {ret}");
                }
            }
            format!("<kont#{}>", id.0)
        }
        Type::Task(id) => {
            if let Some(p) = program {
                if let Some(data) = p.tasks.get(id.0 as usize) {
                    return format!("Task<{}>", type_display(data.result_ty, program));
                }
            }
            format!("<task#{}>", id.0)
        }
        // ADR 0066 M1.2: render `Channel<elem>` (mirrors `Task<…>`).
        Type::Channel(id) => {
            if let Some(p) = program {
                if let Some(data) = p.channels.get(id.0 as usize) {
                    return format!("Channel<{}>", type_display(data.elem_ty, program));
                }
            }
            format!("<channel#{}>", id.0)
        }
        // ADR 0071 M1.4a: render `Shared<elem>` (mirrors `Channel<…>`).
        Type::Shared(id) => {
            if let Some(p) = program {
                if let Some(data) = p.shared.get(id.0 as usize) {
                    return format!("Shared<{}>", type_display(data.elem_ty, program));
                }
            }
            format!("<shared#{}>", id.0)
        }
        // ADR 0071 M1.4b: render `Mutex<elem>` (mirrors `Shared<…>`).
        Type::Mutex(id) => {
            if let Some(p) = program {
                if let Some(data) = p.mutexes.get(id.0 as usize) {
                    return format!("Mutex<{}>", type_display(data.elem_ty, program));
                }
            }
            format!("<mutex#{}>", id.0)
        }
        // ADR 0066 M2.1: a plain `Process` handle (no element type).
        Type::Process => "Process".to_string(),
        // ADR 0066 M2.4a: the `secret i64`-minimum sealed endpoint (unit variant).
        Type::SealedChannel => "SealedChannel<secret i64>".to_string(),
        // ADR 0070 (generalized): render the concrete word-scalar signature.
        Type::Fn(id) => {
            let (param_ty, ret_ty) = fn_value_sig_param_ret(id);
            format!(
                "Fn<{},{}>",
                type_display(param_ty, program),
                type_display(ret_ty, program)
            )
        }
        Type::Class(id) => match program.and_then(|p| p.class_decls.get(id.0 as usize)) {
            Some(c) => c.name.clone(),
            None => format!("<class#{}>", id.0),
        },
        Type::TraitSelf(id) => match program.and_then(|p| p.trait_decls.get(id.0 as usize)) {
            Some(t) => format!("Self ({})", t.name),
            None => format!("<Self-trait#{}>", id.0),
        },
        Type::Enum(id) => match program.and_then(|p| p.enums.get(id.0 as usize)) {
            Some(e) => e.name.clone(),
            None => format!("<enum#{}>", id.0),
        },
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::I64 => write!(f, "i64"),
            Type::I32 => write!(f, "i32"),
            Type::Bool => write!(f, "bool"),
            Type::U8 => write!(f, "u8"),
            Type::U128 => write!(f, "u128"),
            Type::F64 => write!(f, "f64"),
            Type::Ptr => write!(f, "ptr"),
            Type::Struct(id) => write!(f, "<struct#{}>", id.0),
            Type::Nullable(inner) => write!(f, "?{}", inner.to_type()),
            // ADR 0068: a nested-array element can't be resolved without the
            // `arrays` interner; the table-less `Display` renders an id-placeholder
            // like the other interned types (`<ref#N>` etc.). `type_display(Some(p))`
            // gives the full `[[T]]` render (used by the dumps + the differential).
            Type::Array(ArrayElem::Array(id)) => write!(f, "[<arr#{}>]", id.0),
            Type::Array(elem) => write!(f, "[{}]", elem.to_type()),
            Type::Vec(elem) => write!(f, "Vec<{}>", elem.to_type()),
            Type::TypeParam(id) => write!(f, "<T#{}>", id.0),
            Type::GenericInstance(id) => write!(f, "<gi#{}>", id.0),
            Type::Ref(id) => write!(f, "<ref#{}>", id.0),
            Type::Secret(id) => write!(f, "<secret#{}>", id.0),
            Type::Kont(id) => write!(f, "<kont#{}>", id.0),
            Type::Task(id) => write!(f, "<task#{}>", id.0),
            Type::Channel(id) => write!(f, "<channel#{}>", id.0),
            Type::Shared(id) => write!(f, "<shared#{}>", id.0),
            Type::Mutex(id) => write!(f, "<mutex#{}>", id.0),
            Type::Process => write!(f, "Process"),
            Type::SealedChannel => write!(f, "SealedChannel"),
            Type::Fn(id) => {
                let (param_ty, ret_ty) = fn_value_sig_param_ret(*id);
                write!(f, "Fn<{param_ty},{ret_ty}>")
            }
            Type::Class(id) => write!(f, "<class#{}>", id.0),
            Type::TraitSelf(id) => write!(f, "<Self-trait#{}>", id.0),
            Type::Enum(id) => write!(f, "<enum#{}>", id.0),
        }
    }
}

/// Per-fn / per-struct type-parameter scope per ADR 0016 D6c / D9.
/// Maps a source-level type-parameter name (e.g., `"T"`) to its
/// [`TypeParamId`] within the surrounding fn or struct. An empty
/// scope means "no type params in scope" — typical for non-generic
/// items, where the standard struct + primitive lookup applies.
type TypeParamScope = HashMap<String, TypeParamId>;

/// Resolve a surface-level [`TypeExpr`] to a concrete [`Type`].
/// C1.2 recognised only `"i64"`; C1.3 (per ADR 0012 D3 + D5) added
/// `"i32"` and `"bool"`; C1.4 (per ADR 0013 D4) extends to look up
/// user-defined struct names against the struct table; C1.7 (per
/// ADR 0016 D6 + D9) extends to look up type-parameter names against
/// the surrounding fn / struct's type-param scope. Anything not
/// matching surfaces as [`TypeError::UnknownType`].
///
/// At C1.7.4a generic struct instances (`TypeExprKind::Generic`)
/// are explicitly rejected — that closes at C1.7.4b. Type-param
/// args inside Nullable / Array are supported (e.g., `?T` and `[T]`
/// inside a generic fn body).
#[allow(clippy::too_many_arguments)]
fn resolve_type_expr(
    te: &TypeExpr,
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    enum_table: &HashMap<String, EnumId>,
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
) -> Result<Type, TypeError> {
    let empty: TypeParamScope = HashMap::new();
    resolve_type_expr_with_scope(
        te,
        struct_table,
        class_table,
        enum_table,
        &empty,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_type_expr_with_scope(
    te: &TypeExpr,
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    enum_table: &HashMap<String, EnumId>,
    type_param_scope: &TypeParamScope,
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
) -> Result<Type, TypeError> {
    match &te.kind {
        TypeExprKind::Ident(name) => {
            // ADR 0016 D9 lookup precedence: type-param scope first,
            // then struct table, then class table per ADR 0022 D1,
            // then primitives.
            if let Some(&tp_id) = type_param_scope.get(name) {
                return Ok(Type::TypeParam(tp_id));
            }
            match name.as_str() {
                "i64" => Ok(Type::I64),
                "i32" => Ok(Type::I32),
                "bool" => Ok(Type::Bool),
                // Phase D.2 / ADR 0033 D4: `u8` is a primitive type name
                // (no keyword token; lexes as an Ident), recognised here
                // like `i64`/`i32`/`bool`. `[u8]` flows through the array
                // type-expr path, which demotes via `to_array_elem`.
                "u8" => Ok(Type::U8),
                // ADR 0055: `u128` — an unsigned 128-bit integer scalar
                // (the 64-bit-limb / 128-bit-product numeric gap).
                "u128" => Ok(Type::U128),
                // ADR 0058: `f64` — an IEEE-754 double-precision float
                // (PUBLIC-ONLY; `secret f64` is rejected in the Secret arm).
                "f64" => Ok(Type::F64),
                // ADR 0057 Phase 1b: `ptr` — an opaque raw C pointer for the
                // FFI (produced by `ptr_of`/an extern return; consumed by an
                // extern call). Not dereferenceable / indexable in Sentinel.
                "ptr" => Ok(Type::Ptr),
                // Phase D.3 (2/N) / ADR 0034 D5 (Amendment A1): `String`
                // is a thin alias for `Vec<u8>` — a growable byte buffer,
                // not a separate nominal type. Recognised now that the
                // `Vec<u8>` -> `[u8]` bridge (`vec_to_array`) makes a
                // built `String` usable against keyword `[u8]`s. (A string
                // *literal* is still a `[u8]`; converting one to a
                // `String` is a `vec_new` + `push` build until a
                // `[u8]` -> `Vec<u8>` direction lands.)
                "String" => Ok(Type::Vec(VecElem::U8)),
                other => {
                    if let Some(&id) = struct_table.get(other) {
                        // ADR 0016 D3: a bare struct name used in
                        // type position requires zero type-params.
                        // If the struct is generic, the user must
                        // supply args via `Foo<...>`.
                        let expected = struct_type_param_counts.get(&id).copied().unwrap_or(0);
                        if expected != 0 {
                            return Err(TypeError::MissingTypeArgs {
                                type_name: other.to_string(),
                                expected_count: expected,
                                span: to_source_span(&te.span),
                            });
                        }
                        Ok(Type::Struct(id))
                    } else if let Some(&cid) = class_table.get(other) {
                        Ok(Type::Class(cid))
                    } else if let Some(&eid) = enum_table.get(other) {
                        // Phase D.1 / ADR 0032: enum names are usable
                        // in type position (e.g. `fn area(s: Shape)`).
                        // Lookup precedence is struct → class → enum →
                        // primitive (names are unique across these
                        // namespaces, so order only affects diagnostics).
                        Ok(Type::Enum(eid))
                    } else {
                        Err(TypeError::UnknownType {
                            name: other.to_string(),
                            span: to_source_span(&te.span),
                        })
                    }
                }
            }
        }
        TypeExprKind::Nullable(inner) => {
            // Recursively resolve the inner; reject if it's also
            // nullable (the parser already rejects `??T` per ADR
            // 0014 D6, so this should be unreachable for source-
            // level inputs).
            let inner_ty = resolve_type_expr_with_scope(
                inner,
                struct_table,
                class_table,
                enum_table,
                type_param_scope,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
            )?;
            match inner_ty.to_nullable_inner() {
                Some(ni) => Ok(Type::Nullable(ni)),
                None => Err(TypeError::UnknownType {
                    name: "?(nullable)".to_string(),
                    span: to_source_span(&te.span),
                }),
            }
        }
        TypeExprKind::Array(inner) => {
            // Recursively resolve the inner; reject `[&T]` per ADR 0017 D1
            // (refs in arrays need named regions for soundness, deferred).
            // ADR 0068: `[[T]]` is now admitted — the depth-1 rule (ADR 0015 D6)
            // is lifted via the `arrays` interner.
            let inner_ty = resolve_type_expr_with_scope(
                inner,
                struct_table,
                class_table,
                enum_table,
                type_param_scope,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
            )?;
            if inner_ty.is_ref() {
                return Err(TypeError::RefInArray {
                    span: to_source_span(&te.span),
                });
            }
            // ADR 0068: a nested array `[[T]]` — the element is itself an array.
            // Demote the inner array's element to an `ArrayElem`, intern it, and
            // wrap as `ArrayElem::Array(id)`. (`[secret [T]]` stays rejected: a
            // `Type::Secret` inner doesn't match `Type::Array` here, and its
            // `to_array_elem_secret` demote rejects a non-scalar secret leaf.)
            if let Type::Array(inner_ae) = inner_ty {
                let id = intern_array_elem(arrays, inner_ae);
                return Ok(Type::Array(ArrayElem::Array(id)));
            }
            // ADR 0047: admit `[secret SCALAR]` (e.g. `[secret u8]`) in addition
            // to the flat subset. The guarded demote keeps `[secret [T]]` / `[?T]`
            // rejected as NestedArray (those inners have no `ArrayElem`).
            match inner_ty.to_array_elem_secret(secrets) {
                Some(ae) => Ok(Type::Array(ae)),
                None => Err(TypeError::NestedArray {
                    span: to_source_span(&te.span),
                }),
            }
        }
        TypeExprKind::Ref { mutable, inner } => {
            // C2 / ADR 0017 D1 + D11. Recursively resolve the inner;
            // reject `&&T` (NestedRef per the depth-1 amendment of
            // D11). `?&T` / `&?T` / `&[T]` are all valid.
            let inner_ty = resolve_type_expr_with_scope(
                inner,
                struct_table,
                class_table,
                enum_table,
                type_param_scope,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
            )?;
            if inner_ty.is_ref() {
                return Err(TypeError::NestedRef {
                    span: to_source_span(&te.span),
                });
            }
            let id = intern_ref(refs, *mutable, inner_ty);
            Ok(Type::Ref(id))
        }
        TypeExprKind::Generic { name, args, .. } => {
            // ADR 0016 D3: `Foo<T1, T2, ...>` in type position.
            // Lookup precedence: type-param scope first (in case
            // someone wrote `T<...>` — rejected as
            // `TypeArgsOnNonGeneric`), then struct table.
            if type_param_scope.contains_key(name) {
                return Err(TypeError::TypeArgsOnNonGeneric {
                    type_name: name.clone(),
                    span: to_source_span(&te.span),
                });
            }
            match name.as_str() {
                // Phase D.2 / ADR 0033 D4: `u8<...>` is rejected like any
                // other non-generic primitive.
                "i64" | "i32" | "bool" | "u8" | "u128" | "f64" => Err(TypeError::TypeArgsOnNonGeneric {
                    type_name: name.clone(),
                    span: to_source_span(&te.span),
                }),
                // Phase D.3 / ADR 0034 D2: `Vec<T>` is a builtin generic
                // type (not a user `class Vec<T>`). Exactly one type
                // argument, whose resolved type must be in the flat
                // VecElem subset (primitives + structs). This is the only
                // generic name resolved here that isn't a user struct.
                // (`String` = `Vec<u8>` is deferred to D.3 (2/N) with the
                // `[u8]`<->`Vec<u8>` bridge — ADR 0034 D5.)
                "Vec" => {
                    if args.len() != 1 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "Vec".to_string(),
                            expected: 1,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let elem_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    // ADR 0052: admit `Vec<secret SCALAR>` (e.g. `Vec<secret u8>`)
                    // in addition to the flat subset; the guarded demote keeps
                    // `Vec<secret [T]>` rejected (the depth-1 rule).
                    match elem_ty.to_vec_elem_secret(secrets) {
                        Some(ve) => Ok(Type::Vec(ve)),
                        // Vec<[T]> / Vec<Vec<T>> / Vec<?T> / Vec<&T> / Vec<secret [T]>:
                        // outside the flat (scalar) subset, deferred (D8 / ADR 0052).
                        None => Err(TypeError::VecElementNotSupported {
                            span: to_source_span(&te.span),
                        }),
                    }
                }
                // ADR 0066 M1.2b: `Channel<T>` in type position — so a fn can
                // take a channel endpoint as a parameter (the cross-thread
                // producer/consumer shape, D4). At the M1.2b minimum the element
                // type is `i64`, resolving to the singleton `Channel<i64>`
                // interned at `ChanId(0)` during channel-builtin signature setup
                // (no per-annotation interning — "no threading"). Generic
                // word-scalar/aggregate elements are a follow-on.
                "Channel" => {
                    if args.len() != 1 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "Channel".to_string(),
                            expected: 1,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let elem_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    // ADR 0066 M1.2b-cont: any word-scalar element {i64,i32,u8,bool,
                    // f64,ptr} resolves to its pre-interned ChanId (i64 → the M1.2
                    // singleton ChanId(0), byte-identical). A non-word-scalar (e.g.
                    // `secret`, `u128`, an aggregate) is still rejected.
                    match channel_chanid_for(elem_ty) {
                        Some(cid) => Ok(Type::Channel(ChanId(cid))),
                        None => Err(TypeError::ChannelElementNotSupported {
                            span: to_source_span(&te.span),
                        }),
                    }
                }
                // ADR 0071 M1.4a: `Shared<T>` in type position — so a fn can take a
                // shared handle as a parameter (mirrors the `Channel<T>` arm). Any
                // word-scalar element resolves to its pre-interned SharedId (i64 →
                // SharedId(0)); a non-word-scalar is rejected (SharedElementNotSupported).
                "Shared" => {
                    if args.len() != 1 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "Shared".to_string(),
                            expected: 1,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let elem_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    match shared_id_for(elem_ty) {
                        Some(sid) => Ok(Type::Shared(SharedId(sid))),
                        None => Err(TypeError::SharedElementNotSupported {
                            span: to_source_span(&te.span),
                        }),
                    }
                }
                // ADR 0071 M1.4b: `Mutex<T>` in type position — so a fn can take a
                // mutex handle as a parameter (mirrors the `Shared<T>` arm). Any
                // word-scalar element resolves to its pre-interned MutexId (i64 →
                // MutexId(0)); a non-word-scalar is rejected (MutexElementNotSupported).
                "Mutex" => {
                    if args.len() != 1 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "Mutex".to_string(),
                            expected: 1,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let elem_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    match mutex_id_for(elem_ty) {
                        Some(mid) => Ok(Type::Mutex(MutexId(mid))),
                        None => Err(TypeError::MutexElementNotSupported {
                            span: to_source_span(&te.span),
                        }),
                    }
                }
                // ADR 0066 M2.4a / ADR 0069: `SealedChannel<secret i64>` in type
                // position — so the stdlib `seal`/`open` framing fns can take a
                // sealed endpoint as a parameter. The element MUST be `secret i64`
                // at the M2.4a minimum (D1: a public element is a type error —
                // sealing a public value is pointless; generic `secret T` is M2.4c).
                // A unit `Type::SealedChannel` (no interner) — fixed at `secret i64`.
                "SealedChannel" => {
                    if args.len() != 1 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "SealedChannel".to_string(),
                            expected: 1,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let elem_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    let is_secret_i64 = match elem_ty {
                        Type::Secret(id) => secrets[id.0 as usize].inner == Type::I64,
                        _ => false,
                    };
                    if !is_secret_i64 {
                        return Err(TypeError::SealedChannelElementNotSupported {
                            span: to_source_span(&te.span),
                        });
                    }
                    Ok(Type::SealedChannel)
                }
                // ADR 0070 D1/D2 (generalized, M-cont): `Fn<T, R>` in type position —
                // any WORD-SCALAR param/return (mirrors Channel<T>'s M1.2b-cont
                // generalization). `fn_value_sig_id_for` is pure arithmetic (no
                // interner table), so no threading here beyond the ordinary
                // recursive resolve of each type arg.
                "Fn" => {
                    if args.len() != 2 {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: "Fn".to_string(),
                            expected: 2,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    let param_ty = resolve_type_expr_with_scope(
                        &args[0],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    let ret_ty = resolve_type_expr_with_scope(
                        &args[1],
                        struct_table,
                        class_table,
                        enum_table,
                        type_param_scope,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?;
                    match fn_value_sig_id_for(param_ty, ret_ty) {
                        Some(id) => Ok(Type::Fn(id)),
                        None => Err(TypeError::FnTypeArgsNotSupported {
                            span: to_source_span(&te.span),
                        }),
                    }
                }
                other => {
                    let struct_id = match struct_table.get(other) {
                        Some(&id) => id,
                        None => {
                            return Err(TypeError::UnknownType {
                                name: other.to_string(),
                                span: to_source_span(&te.span),
                            });
                        }
                    };
                    let expected = struct_type_param_counts
                        .get(&struct_id)
                        .copied()
                        .unwrap_or(0);
                    if expected == 0 {
                        return Err(TypeError::TypeArgsOnNonGeneric {
                            type_name: other.to_string(),
                            span: to_source_span(&te.span),
                        });
                    }
                    if args.len() != expected {
                        return Err(TypeError::TypeArgCountMismatch {
                            type_name: other.to_string(),
                            expected,
                            found: args.len(),
                            span: to_source_span(&te.span),
                        });
                    }
                    // Recursively resolve each arg (possibly itself
                    // a generic instance), then intern.
                    let mut resolved_args = Vec::with_capacity(args.len());
                    for arg in args {
                        resolved_args.push(resolve_type_expr_with_scope(
                            arg,
                            struct_table,
                            class_table,
                            enum_table,
                            type_param_scope,
                            instances,
                            refs,
                            secrets,
                            arrays,
                            struct_type_param_counts,
                        )?);
                    }
                    let id = intern_generic_instance(instances, struct_id, resolved_args);
                    Ok(Type::GenericInstance(id))
                }
            }
        }
        TypeExprKind::Secret(inner) => {
            // C3 / ADR 0019 D5 (C3.1): `secret T` interns into the
            // program-level `secrets` table. Nested `secret secret T`
            // is rejected at parse time per the DoubleSecret rule;
            // `intern_secret` also defends in depth (idempotent if
            // a future substitution produces an already-secret type).
            let inner_ty = resolve_type_expr_with_scope(
                inner,
                struct_table,
                class_table,
                enum_table,
                type_param_scope,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
            )?;
            // ADR 0058: `secret f64` is rejected — float ops are not
            // constant-time, so a secret float would be a false guarantee.
            // Floats are a disjoint PUBLIC domain (the fence that keeps the
            // constant-time proof sound; contrast `secret u128`, which is
            // valid because integer ops CAN be made constant-time).
            if matches!(inner_ty, Type::F64) {
                return Err(TypeError::SecretFloat {
                    span: to_source_span(&inner.span),
                });
            }
            let id = intern_secret(secrets, inner_ty);
            Ok(Type::Secret(id))
        }
    }
}

// =============================================================================
// Typed AST — parallel tree mirroring ResolvedProgram
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedProgram {
    pub fns: Vec<TypedFnDef>,
    pub fn_signatures: Vec<TypedFnSignature>,
    /// ADR 0057: the `FnId`s of `extern "C"` declarations (body-less C-ABI
    /// functions). Their [`TypedFnSignature`]s are in `fn_signatures` like
    /// any fn; this set tells codegen to declare each as an `External` symbol
    /// under its BARE name (the C symbol) and to skip emitting a body.
    /// Empty for programs with no FFI.
    pub externs: Vec<FnId>,
    /// ADR 0059: the `FnId`s of `export "C"` functions (normal fns WITH a
    /// body, additionally exported under their bare un-mangled C symbol). Tells
    /// codegen to pin the symbol bare + `External`, and the driver which
    /// signatures to emit into the generated C header. Empty for non-library
    /// programs. Each one's signature was validated FFI-safe + secret-fenced.
    pub exports: Vec<FnId>,
    /// Struct declarations with resolved field types (parallel-tree
    /// mirror of [`sentinel_resolve::ResolvedProgram::structs`]).
    /// Each struct's [`StructId`] matches its index here.
    pub structs: Vec<TypedStructDecl>,
    /// C1.7 / ADR 0016 D6a: interned generic-struct instances. Each
    /// `Type::GenericInstance(id)` indexes this vector to recover
    /// `(struct_id, args)`. Populated by the type checker as
    /// `Foo<i64, bool>` shows up in source; potentially extended at
    /// codegen monomorphisation time when substituting through
    /// nested generic instances (per ADR 0016 D6a's "linear
    /// search; HashMap if profile demands").
    pub generic_instances: Vec<GenericInstanceData>,
    /// C2 / ADR 0017 D11: interned reference types. Each
    /// `Type::Ref(id)` indexes this vector to recover
    /// `(mutable, inner)`. Same scale + scheme as
    /// [`generic_instances`].
    pub refs: Vec<RefData>,
    /// C3 / ADR 0019 D5 (C3.1): interned secret types. Each
    /// `Type::Secret(id)` indexes this vector to recover
    /// `(inner: Type)`. Same scale + scheme as [`refs`] /
    /// [`generic_instances`].
    pub secrets: Vec<SecretData>,
    /// C3 / ADR 0019 D4 (C3.2): top-level effect declarations with
    /// type-checked op signatures. Each carries its
    /// [`EffectId`] matching its index. The fn-signature
    /// effect-row machinery (C3.2(b)) refers to effects by
    /// EffectId. At C3.2 minimum ops are declared-but-not-
    /// invocable (handler runtime is ADR 0020).
    pub effect_decls: Vec<TypedEffectDecl>,
    /// C3.4 / ADR 0020 D5: interned continuation types. Each
    /// `Type::Kont(id)` indexes this vector to recover
    /// `(arg_ty, ret_ty)`. Same scale + scheme as [`refs`] /
    /// [`secrets`]. Populated during type-check of `handle ...
    /// with { ... }` expressions.
    pub konts: Vec<KontData>,
    /// C4.4 / ADR 0024 D4: interned task types. Each
    /// `Type::Task(id)` indexes this vector to recover
    /// `(result_ty)`. Populated during type-check of `spawn`
    /// expressions; at C4.4 minimum holds at most one entry
    /// (`Task<i64>`) per the D7 result-type restriction.
    pub tasks: Vec<TaskData>,
    /// ADR 0066 M1.2: interned channel types. Each `Type::Channel(id)`
    /// indexes this vector to recover `(elem_ty)`. Populated during
    /// type-check of `channel_new` calls.
    pub channels: Vec<ChannelData>,
    /// ADR 0071 M1.4a: interned `Shared<T>` types. Each `Type::Shared(id)`
    /// indexes this vector to recover `(elem_ty)`. Pre-populated with the 6
    /// word-scalar elements at fixed ids 0..=5 during builtin signature setup
    /// (see [`shared_id_for`]). Mirrors [`channels`].
    pub shared: Vec<SharedData>,
    /// ADR 0071 M1.4b: interned `Mutex<T>` types. Each `Type::Mutex(id)` indexes
    /// this vector to recover `(elem_ty)`. Pre-populated with the 6 word-scalar
    /// elements at fixed ids 0..=5 during builtin signature setup (see
    /// [`mutex_id_for`]). Mirrors [`shared`].
    pub mutexes: Vec<MutexData>,
    /// ADR 0068: interned NESTED-array elements. Each
    /// `Type::Array(ArrayElem::Array(id))` (`[[T]]`) indexes this vector to
    /// recover the *inner* array's [`ArrayElem`]. Populated when a `[[T]]` type
    /// annotation / literal is resolved. Same scheme as [`channels`] / [`refs`].
    pub arrays: Vec<ArrayElem>,
    /// C4.1 / ADR 0022 D1: class declarations with resolved field
    /// types + init signature + method signatures. Each ClassId
    /// matches its index here.
    pub class_decls: Vec<ClassData>,
    /// C4.2 / ADR 0023 D1: trait declarations with type-checked
    /// method signatures. Each TraitId matches its index here.
    /// Method sigs reference `Type::TraitSelf(self_trait_id)` for
    /// `self`-typed positions; impl-sig type-check substitutes.
    pub trait_decls: Vec<TraitData>,
    /// C4.2 / ADR 0023 D3/D4: impl declarations with type-checked
    /// method signatures + bodies. Each ImplId matches its index
    /// here. The (trait_id, target, name) coherence is already
    /// validated at resolve; types verifies completeness +
    /// per-method signature match against the trait.
    pub impl_decls: Vec<ImplData>,
    /// Phase D.1 / ADR 0032 D3: enum declarations with resolved
    /// variant payload types. Each [`EnumId`] matches its index here.
    /// Built from `ResolvedProgram::enums`.
    pub enums: Vec<EnumData>,
    pub span: Span,
}

impl TypedProgram {
    pub fn main(&self) -> &TypedFnDef {
        self.fns
            .iter()
            .find(|f| self.signature(f.id).is_main)
            .expect("MissingMain would have fired in resolve already")
    }

    pub fn signature(&self, id: FnId) -> &TypedFnSignature {
        &self.fn_signatures[id.0 as usize]
    }

    pub fn struct_decl(&self, id: StructId) -> &TypedStructDecl {
        &self.structs[id.0 as usize]
    }

    /// Look up the underlying `(struct_id, args)` for a generic-
    /// instance type. Panics on out-of-range — IDs only come from
    /// [`check`] or [`intern_generic_instance`].
    pub fn generic_instance(&self, id: GenericInstanceId) -> &GenericInstanceData {
        &self.generic_instances[id.0 as usize]
    }

    /// Look up the underlying `(mutable, inner)` for a reference
    /// type. Panics on out-of-range — IDs only come from [`check`]
    /// or [`intern_ref`]. C2 / ADR 0017 D11.
    pub fn ref_data(&self, id: RefId) -> &RefData {
        &self.refs[id.0 as usize]
    }

    /// Look up the underlying `(inner)` for a secret type. Panics
    /// on out-of-range — IDs only come from [`check`] or
    /// [`intern_secret`]. C3 / ADR 0019 D5 (C3.1).
    pub fn secret_data(&self, id: SecretId) -> &SecretData {
        &self.secrets[id.0 as usize]
    }

    /// Look up the `(arg_ty, ret_ty)` for a continuation type per
    /// ADR 0020 D5 (C3.4). Panics on out-of-range — IDs only come
    /// from [`check`] / [`intern_kont`].
    pub fn kont_data(&self, id: KontId) -> &KontData {
        &self.konts[id.0 as usize]
    }

    /// Look up the `(result_ty)` for a task type per ADR 0024 D4
    /// (C4.4). Panics on out-of-range — IDs only come from [`check`]
    /// / [`intern_task`].
    pub fn task_data(&self, id: TaskId) -> &TaskData {
        &self.tasks[id.0 as usize]
    }

    /// Look up the `(elem_ty)` for a channel type per ADR 0066 M1.2.
    /// Panics on out-of-range — IDs only come from [`intern_channel`].
    pub fn channel_data(&self, id: ChanId) -> &ChannelData {
        &self.channels[id.0 as usize]
    }

    /// Look up the `(elem_ty)` for a `Shared<T>` type per ADR 0071 M1.4a.
    /// Panics on out-of-range — IDs only come from [`intern_shared`].
    pub fn shared_data(&self, id: SharedId) -> &SharedData {
        &self.shared[id.0 as usize]
    }

    /// Look up the `(elem_ty)` for a `Mutex<T>` type per ADR 0071 M1.4b.
    /// Panics on out-of-range — IDs only come from [`intern_mutex`].
    pub fn mutex_data(&self, id: MutexId) -> &MutexData {
        &self.mutexes[id.0 as usize]
    }

    /// ADR 0068: the inner [`ArrayElem`] of a nested-array element. Panics on
    /// out-of-range — IDs only come from [`intern_array_elem`].
    pub fn array_elem(&self, id: ArrayId) -> ArrayElem {
        self.arrays[id.0 as usize]
    }

    /// ADR 0068: promote an [`ArrayElem`] to its [`Type`], resolving a nested
    /// `Array(id)` via the [`arrays`](Self::arrays) interner (which the bare,
    /// table-less [`ArrayElem::to_type`] cannot). For a flat element this is just
    /// `ae.to_type()`; for `Array(id)` it is `[inner]` = `Type::Array(arrays[id])`.
    /// This is the canonical "element type of an array" accessor for codegen /
    /// type-check sites that may see a nested element.
    pub fn array_elem_type(&self, ae: ArrayElem) -> Type {
        array_elem_type_in(ae, &self.arrays)
    }

    /// Look up the [`ClassData`] for a class type per ADR 0022 D1
    /// (C4.1). Panics on out-of-range — IDs only come from
    /// [`check`].
    pub fn class_decl(&self, id: ClassId) -> &ClassData {
        &self.class_decls[id.0 as usize]
    }

    /// Look up the [`TraitData`] for a trait per ADR 0023 D1
    /// (C4.2). Panics on out-of-range.
    pub fn trait_decl(&self, id: TraitId) -> &TraitData {
        &self.trait_decls[id.0 as usize]
    }

    /// Look up the [`ImplData`] for an impl per ADR 0023 D3/D4
    /// (C4.2). Panics on out-of-range.
    pub fn impl_decl(&self, id: ImplId) -> &ImplData {
        &self.impl_decls[id.0 as usize]
    }

    /// Look up the [`EnumData`] for an enum type per ADR 0032 D3
    /// (D.1). Panics on out-of-range — IDs only come from [`check`].
    pub fn enum_data(&self, id: EnumId) -> &EnumData {
        &self.enums[id.0 as usize]
    }
}

// =============================================================================
// Eager monomorphic substitution helpers per ADR 0016 D7c.
// =============================================================================
//
// These functions deep-clone a typed AST node, applying a TypeParam
// substitution to every [`Type`] embedded in it. Codegen uses this
// at C1.7.5 to materialise a monomorphic instance of a generic fn
// from its abstract definition + a concrete type-arg vector.
//
// The substitution semantics mirror [`Type::substitute`]: each
// `Type::TypeParam(idx)` is replaced with `subst[idx]`. Concrete
// types pass through unchanged. `Call` nodes also have their
// `type_args: Vec<Type>` substituted, which is what enables
// transitive monomorphic emission (`f<T>` calling `g<T>` produces
// the correct `g_<concrete>` instance for each `f_<concrete>`).

impl TypedFnDef {
    /// Deep-clone this fn def, substituting every [`Type::TypeParam`]
    /// against `subst`. The resulting fn is a monomorphic instance
    /// of the original generic fn — its `type_params` list is empty
    /// and every embedded Type is concrete (or a `GenericInstance`
    /// of a generic struct's monomorphic image, interned in the
    /// shared `instances` table).
    pub fn substitute(
        &self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> TypedFnDef {
        TypedFnDef {
            id: self.id,
            name: self.name.clone(),
            name_span: self.name_span.clone(),
            // Monomorphic instance: no type-params remain.
            type_params: Vec::new(),
            params: self
                .params
                .iter()
                .map(|p| TypedParam {
                    id: p.id,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.substitute(subst, instances, refs),
                })
                .collect(),
            return_type: self.return_type.substitute(subst, instances, refs),
            body: self.body.substitute(subst, instances, refs),
            span: self.span.clone(),
        }
    }
}

impl TypedBlock {
    pub fn substitute(
        &self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> TypedBlock {
        TypedBlock {
            stmts: self
                .stmts
                .iter()
                .map(|s| s.substitute(subst, instances, refs))
                .collect(),
            tail: self.tail.substitute(subst, instances, refs),
            span: self.span.clone(),
            ty: self.ty.substitute(subst, instances, refs),
        }
    }
}

impl TypedStmt {
    pub fn substitute(
        &self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> TypedStmt {
        let kind = match &self.kind {
            TypedStmtKind::Let { id, mutable, name, name_span, ty, value } => {
                TypedStmtKind::Let {
                    id: *id,
                    mutable: *mutable,
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.substitute(subst, instances, refs),
                    value: value.substitute(subst, instances, refs),
                }
            }
            TypedStmtKind::Assign { target, value } => TypedStmtKind::Assign {
                target: target.substitute(subst, instances, refs),
                value: value.substitute(subst, instances, refs),
            },
            TypedStmtKind::While { cond, body } => TypedStmtKind::While {
                cond: cond.substitute(subst, instances, refs),
                body: Box::new(body.substitute(subst, instances, refs)),
            },
            // D.5 (2/N): payload-free loop control carries no TypeParam.
            TypedStmtKind::Break => TypedStmtKind::Break,
            TypedStmtKind::Continue => TypedStmtKind::Continue,
            TypedStmtKind::Expr(e) => TypedStmtKind::Expr(e.substitute(subst, instances, refs)),
        };
        TypedStmt { kind, span: self.span.clone() }
    }
}

impl TypedExpr {
    pub fn substitute(
        &self,
        subst: &[Type],
        instances: &mut Vec<GenericInstanceData>,
        refs: &mut Vec<RefData>,
    ) -> TypedExpr {
        let kind = match &self.kind {
            TypedExprKind::IntLit(n) => TypedExprKind::IntLit(*n),
            TypedExprKind::BoolLit(b) => TypedExprKind::BoolLit(*b),
            TypedExprKind::NullLit => TypedExprKind::NullLit,
            // D.2 / ADR 0033: literal bytes carry no TypeParam — identity.
            TypedExprKind::FloatLit(bits) => TypedExprKind::FloatLit(*bits),
            TypedExprKind::CharLit(b) => TypedExprKind::CharLit(*b),
            TypedExprKind::StringLit(bytes) => TypedExprKind::StringLit(bytes.clone()),
            TypedExprKind::WidenToNullable(inner) => TypedExprKind::WidenToNullable(
                Box::new(inner.substitute(subst, instances, refs)),
            ),
            TypedExprKind::WidenToSecret(inner) => TypedExprKind::WidenToSecret(
                Box::new(inner.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Declassify(inner) => TypedExprKind::Declassify(
                Box::new(inner.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Return(inner) => {
                TypedExprKind::Return(Box::new(inner.substitute(subst, instances, refs)))
            }
            TypedExprKind::Cast(inner) => {
                TypedExprKind::Cast(Box::new(inner.substitute(subst, instances, refs)))
            }
            TypedExprKind::Var(id) => TypedExprKind::Var(*id),
            TypedExprKind::FnRef(id) => TypedExprKind::FnRef(*id),
            TypedExprKind::Unary(op, inner) => TypedExprKind::Unary(
                *op,
                Box::new(inner.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Binary(op, l, r) => TypedExprKind::Binary(
                *op,
                Box::new(l.substitute(subst, instances, refs)),
                Box::new(r.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Cmp(op, l, r) => TypedExprKind::Cmp(
                *op,
                Box::new(l.substitute(subst, instances, refs)),
                Box::new(r.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Logic(op, l, r) => TypedExprKind::Logic(
                *op,
                Box::new(l.substitute(subst, instances, refs)),
                Box::new(r.substitute(subst, instances, refs)),
            ),
            TypedExprKind::Block(b) => {
                TypedExprKind::Block(Box::new(b.substitute(subst, instances, refs)))
            }
            TypedExprKind::If { cond, then_branch, else_branch } => TypedExprKind::If {
                cond: Box::new(cond.substitute(subst, instances, refs)),
                then_branch: Box::new(then_branch.substitute(subst, instances, refs)),
                else_branch: Box::new(else_branch.substitute(subst, instances, refs)),
            },
            TypedExprKind::Call { id, callee_span, args, type_args } => {
                TypedExprKind::Call {
                    id: *id,
                    callee_span: callee_span.clone(),
                    args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
                    // Substitute the call's type_args too — this is
                    // what enables transitive monomorphic emission.
                    type_args: type_args
                        .iter()
                        .map(|t| t.substitute(subst, instances, refs))
                        .collect(),
                }
            }
            TypedExprKind::StructLit { id, name, name_span, fields } => {
                TypedExprKind::StructLit {
                    id: *id,
                    name: name.clone(),
                    name_span: name_span.clone(),
                    fields: fields.iter().map(|f| f.substitute(subst, instances, refs)).collect(),
                }
            }
            TypedExprKind::FieldAccess { target, field, field_span, field_index } => {
                TypedExprKind::FieldAccess {
                    target: Box::new(target.substitute(subst, instances, refs)),
                    field: field.clone(),
                    field_span: field_span.clone(),
                    field_index: *field_index,
                }
            }
            TypedExprKind::ArrayLit { elem_ty, elements } => TypedExprKind::ArrayLit {
                elem_ty: elem_ty.substitute(subst, instances, refs),
                elements: elements.iter().map(|e| e.substitute(subst, instances, refs)).collect(),
            },
            TypedExprKind::Index { target, index, elem_ty } => TypedExprKind::Index {
                target: Box::new(target.substitute(subst, instances, refs)),
                index: Box::new(index.substitute(subst, instances, refs)),
                elem_ty: elem_ty.substitute(subst, instances, refs),
            },
            // C3.4 / ADR 0020 D5: handle/perform/resume don't compose
            // with generics at C3.4 minimum (no effect polymorphism
            // per D10). Clone the bodies straight through —
            // substitute() through TypedExpr children would only
            // touch concrete-type fields and the arm-level kont_id
            // already references concrete types in the konts table.
            TypedExprKind::Handle { body, arms, return_arm, handled } => {
                TypedExprKind::Handle {
                    body: Box::new(body.substitute(subst, instances, refs)),
                    arms: arms
                        .iter()
                        .map(|a| TypedHandlerArm {
                            effect_id: a.effect_id,
                            op_index: a.op_index,
                            effect_name: a.effect_name.clone(),
                            op_name: a.op_name.clone(),
                            op_span: a.op_span.clone(),
                            param_var_ids: a.param_var_ids.clone(),
                            param_names: a.param_names.clone(),
                            kont_id: a.kont_id,
                            body: a.body.substitute(subst, instances, refs),
                            span: a.span.clone(),
                        })
                        .collect(),
                    return_arm: return_arm.as_ref().map(|ra| {
                        Box::new(TypedReturnArm {
                            value_var_id: ra.value_var_id,
                            value_name: ra.value_name.clone(),
                            body: ra.body.substitute(subst, instances, refs),
                            span: ra.span.clone(),
                        })
                    }),
                    handled: handled.clone(),
                }
            }
            TypedExprKind::Perform {
                effect_id,
                op_index,
                effect_name,
                op_name,
                op_span,
                args,
            } => TypedExprKind::Perform {
                effect_id: *effect_id,
                op_index: *op_index,
                effect_name: effect_name.clone(),
                op_name: op_name.clone(),
                op_span: op_span.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            TypedExprKind::ResumeKont { kont, callee_span, args, kont_id } => {
                TypedExprKind::ResumeKont {
                    kont: *kont,
                    callee_span: callee_span.clone(),
                    args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
                    kont_id: *kont_id,
                }
            }
            // C4.1 / ADR 0022: classes aren't generic at C4.1 so
            // substitution is just a deep-clone with the same
            // ClassId. Once generic classes land, the class's
            // ClassData expands its generic-instance interner and
            // the substitution looks up the right monomorphic
            // instance — mirroring the C1.7 generic-struct pattern.
            TypedExprKind::MethodCall {
                target,
                class_id,
                method_index,
                method,
                method_span,
                args,
            } => TypedExprKind::MethodCall {
                target: Box::new(target.substitute(subst, instances, refs)),
                class_id: *class_id,
                method_index: *method_index,
                method: method.clone(),
                method_span: method_span.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            TypedExprKind::ClassInit { id, name, name_span, args } => TypedExprKind::ClassInit {
                id: *id,
                name: name.clone(),
                name_span: name_span.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            // C4.2 / ADR 0023: impls aren't generic at C4.2 so
            // substitution is just a deep-clone with same IDs.
            TypedExprKind::ImplMethodCall {
                target,
                impl_id,
                method_index,
                method,
                method_span,
                args,
            } => TypedExprKind::ImplMethodCall {
                target: Box::new(target.substitute(subst, instances, refs)),
                impl_id: *impl_id,
                method_index: *method_index,
                method: method.clone(),
                method_span: method_span.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            TypedExprKind::QualifiedCall {
                impl_id,
                method_index,
                impl_name,
                method,
                method_span,
                args,
            } => TypedExprKind::QualifiedCall {
                impl_id: *impl_id,
                method_index: *method_index,
                impl_name: impl_name.clone(),
                method: method.clone(),
                method_span: method_span.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            // C4.4 / ADR 0024: `Task<T>` is `Task<i64>` at C4.4
            // minimum (no TypeParam payload), so the task_id survives
            // substitution unchanged — only the subexpressions need
            // the deep-clone.
            TypedExprKind::Scope { mode, body } => TypedExprKind::Scope {
                mode: *mode,
                body: Box::new(body.substitute(subst, instances, refs)),
            },
            TypedExprKind::Spawn { call, task_id } => TypedExprKind::Spawn {
                call: Box::new(call.substitute(subst, instances, refs)),
                task_id: *task_id,
            },
            TypedExprKind::Await { task_expr, task_id } => TypedExprKind::Await {
                task_expr: Box::new(task_expr.substitute(subst, instances, refs)),
                task_id: *task_id,
            },
            // Phase D.1 / ADR 0032: enums are non-generic at the MVP
            // (no TypeParam payloads), so the enum_id / variant_index
            // survive substitution unchanged — only the subexpressions
            // (args / scrutinee / arm bodies / binding types) need the
            // deep-clone. Mirrors the non-generic Class/Task arms.
            TypedExprKind::EnumConstruct {
                enum_id,
                variant_index,
                enum_name,
                variant_name,
                args,
            } => TypedExprKind::EnumConstruct {
                enum_id: *enum_id,
                variant_index: *variant_index,
                enum_name: enum_name.clone(),
                variant_name: variant_name.clone(),
                args: args.iter().map(|a| a.substitute(subst, instances, refs)).collect(),
            },
            TypedExprKind::Match { scrutinee, enum_id, arms } => TypedExprKind::Match {
                scrutinee: Box::new(scrutinee.substitute(subst, instances, refs)),
                enum_id: *enum_id,
                arms: arms
                    .iter()
                    .map(|a| TypedMatchArm {
                        pattern: match &a.pattern {
                            TypedPattern::Variant { variant_index, variant_name, bindings, span } => {
                                TypedPattern::Variant {
                                    variant_index: *variant_index,
                                    variant_name: variant_name.clone(),
                                    bindings: bindings
                                        .iter()
                                        .map(|b| TypedPatternBinding {
                                            var_id: b.var_id,
                                            name: b.name.clone(),
                                            ty: b.ty.substitute(subst, instances, refs),
                                            span: b.span.clone(),
                                        })
                                        .collect(),
                                    span: span.clone(),
                                }
                            }
                            TypedPattern::Wildcard(s) => TypedPattern::Wildcard(s.clone()),
                        },
                        body: a.body.substitute(subst, instances, refs),
                        span: a.span.clone(),
                    })
                    .collect(),
            },
        };
        TypedExpr {
            kind,
            span: self.span.clone(),
            ty: self.ty.substitute(subst, instances, refs),
        }
    }
}

/// A struct declaration after type-checking: each field's
/// [`TypeExpr`] has been resolved to a concrete [`Type`] (or to
/// [`Type::TypeParam`] inside a generic struct body per ADR 0016).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedStructDecl {
    pub id: StructId,
    pub name: String,
    pub name_span: Span,
    /// Generic type parameters per ADR 0016 D2 / D9. Empty for
    /// non-generic structs.
    pub type_params: Vec<TypedTypeParam>,
    pub fields: Vec<TypedStructField>,
    pub span: Span,
}

/// A generic type parameter at type-check time. Mirrors the
/// resolve-stage [`ResolvedTypeParam`] one-to-one — the type
/// checker doesn't introduce new TypeParamIds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedTypeParam {
    pub id: TypeParamId,
    pub name: String,
    pub name_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedStructField {
    pub name: String,
    pub name_span: Span,
    pub ty: Type,
    pub span: Span,
}

/// C4.1 / ADR 0022 D1: a class declaration after type-check, with
/// every field-type annotation resolved + the init signature and
/// per-method signatures populated. ClassId matches its index in
/// `TypedProgram::class_decls`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClassData {
    pub id: ClassId,
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<TypedClassField>,
    pub init: Option<TypedInitDef>,
    pub methods: Vec<TypedMethodDef>,
    pub span: Span,
}

impl ClassData {
    /// Find a method by name. Returns the index into `methods`
    /// (used by codegen to pick the right mangled fn name) or
    /// `None` if no such method.
    pub fn method_index(&self, name: &str) -> Option<usize> {
        self.methods.iter().position(|m| m.name == name)
    }

    /// Find a field by name. Returns (index, type) or None.
    pub fn field(&self, name: &str) -> Option<(usize, &TypedClassField)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedClassField {
    pub visibility: sentinel_ast::Visibility,
    pub name: String,
    pub name_span: Span,
    pub ty: Type,
    pub span: Span,
}

/// The type-checked `init(params)` constructor of a class per ADR
/// 0022 D4. The body has been type-checked with a synthetic `self`
/// binding; codegen emits an `out_ptr` ABI per D9.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedInitDef {
    pub visibility: sentinel_ast::Visibility,
    /// VarId of the synthetic `self` binding inside the init body.
    /// Its type inside the body is `Type::Ref(&mut Class)` — at C4.1
    /// we don't track partial-init separately; the dataflow check
    /// runs before codegen.
    pub self_var_id: VarId,
    pub params: Vec<TypedParam>,
    pub body: TypedBlock,
    pub span: Span,
}

/// The type-checked method signature + body per ADR 0022 D3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedMethodDef {
    pub visibility: sentinel_ast::Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    /// VarId of the synthetic `self` binding inside the method.
    /// Its type is `Type::Ref(&Class)` (shared) or
    /// `Type::Ref(&mut Class)` (exclusive) per `self_kind`.
    pub self_var_id: VarId,
    pub params: Vec<TypedParam>,
    pub return_type: Type,
    /// Effect-row annotation as a sorted-dedup set of EffectIds.
    pub effect_row: Vec<EffectId>,
    pub body: TypedBlock,
    pub span: Span,
}

/// C4.2 / ADR 0023 D1: a trait declaration after type-checking. The
/// [`TraitId`] matches its index in [`TypedProgram::trait_decls`].
/// Each method's params + return type have been resolved against the
/// type-param-free top-level scope; positions referencing the
/// implementing type use `Type::TraitSelf(self.id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitData {
    pub id: TraitId,
    pub name: String,
    pub name_span: Span,
    pub methods: Vec<TypedTraitMethodSig>,
    pub span: Span,
}

impl TraitData {
    /// Find a method by name. Returns the index into `methods`.
    pub fn method_index(&self, name: &str) -> Option<usize> {
        self.methods.iter().position(|m| m.name == name)
    }
}

/// C4.2 / ADR 0023 D2: a single type-checked method signature
/// inside a trait. Mirrors [`TypedMethodDef`] minus the body.
/// Param types and return type may reference `Type::TraitSelf(_)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedTraitMethodSig {
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub params: Vec<TypedParam>,
    pub return_type: Type,
    pub effect_row: Vec<EffectId>,
    pub span: Span,
}

/// C4.2 / ADR 0023 D3/D4: an impl block after type-checking.
/// Per-method signatures have been verified against the trait
/// (completeness + signature equality with `Type::TraitSelf(_)`
/// substituted to the impl's `for Type` clause).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImplData {
    pub id: ImplId,
    /// `None` for the default impl; `Some(name)` for a named impl.
    pub name: Option<String>,
    pub trait_id: TraitId,
    pub target: ImplTarget,
    pub trait_name: String,
    pub type_name: String,
    pub methods: Vec<TypedImplMethodDef>,
    pub span: Span,
}

impl ImplData {
    /// Find a method by name. Returns the index into `methods`.
    pub fn method_index(&self, name: &str) -> Option<usize> {
        self.methods.iter().position(|m| m.name == name)
    }
}

/// C4.2 / ADR 0023 D3: a type-checked impl method, structurally
/// identical to [`TypedMethodDef`]. Stored separately so codegen
/// can iterate impl methods distinctly from class methods.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedImplMethodDef {
    pub visibility: sentinel_ast::Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub self_var_id: VarId,
    pub params: Vec<TypedParam>,
    pub return_type: Type,
    pub effect_row: Vec<EffectId>,
    pub body: TypedBlock,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedFnSignature {
    pub id: FnId,
    pub name: String,
    pub name_span: Option<Span>,
    /// Generic type parameters per ADR 0016 D1. Empty for non-
    /// generic fns. When non-empty, `param_types` and `return_type`
    /// may contain [`Type::TypeParam`] references that get
    /// substituted at each call site.
    pub type_params: Vec<TypedTypeParam>,
    pub param_types: Vec<Type>,
    pub return_type: Type,
    /// C3 / ADR 0019 D1 (C3.2): the fn's effect-row annotation as
    /// a sorted-dedup set of [`EffectId`]s. Empty for fns with no
    /// annotation. The `effect_check_query` pass at C3.2(b)
    /// validates this against the body's inferred row.
    pub effect_row: Vec<EffectId>,
    pub is_main: bool,
    pub is_runtime: bool,
    /// Phase D.6 / ADR 0037 D5.1: `Some(module_path)` iff this fn is an
    /// EXTERN imported from another module — propagated from the resolved
    /// signature so codegen declares it external as
    /// `mangle_qualified(origin, name)` (D7). `None` for local fns +
    /// builtins; inert in single-file / Path-A builds.
    pub extern_origin: Option<Vec<String>>,
}

/// C3 / ADR 0019 D4 (C3.2): a top-level effect declaration after
/// type-checking. Each op's param-type annotations have been
/// resolved to concrete [`Type`]s. EffectId matches the index in
/// `TypedProgram::effect_decls`. Ops have no runtime semantics
/// at C3.2 — they're declared for the future handler runtime
/// (ADR 0020).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedEffectDecl {
    pub id: EffectId,
    pub name: String,
    pub name_span: Span,
    pub ops: Vec<TypedOpDecl>,
    pub span: Span,
}

/// C3 / ADR 0019 D4 (C3.2): a single operation declaration inside
/// a [`TypedEffectDecl`]. Param types are concrete. Return type
/// defaults to `i64` if the source didn't supply one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedOpDecl {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<TypedParam>,
    pub return_type: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedFnDef {
    pub id: FnId,
    pub name: String,
    pub name_span: Span,
    /// Generic type parameters per ADR 0016 D1 / D9. Empty for
    /// non-generic fns; matches `signature.type_params`.
    pub type_params: Vec<TypedTypeParam>,
    pub params: Vec<TypedParam>,
    pub return_type: Type,
    pub body: TypedBlock,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedParam {
    pub id: VarId,
    /// C2 / ADR 0017 D2: `true` iff the source declared `mut x: T`.
    /// Binding-local — passes through from
    /// [`sentinel_resolve::ResolvedParam`].
    pub mutable: bool,
    pub name: String,
    pub span: Span,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedBlock {
    pub stmts: Vec<TypedStmt>,
    pub tail: TypedExpr,
    pub span: Span,
    /// Always equals `tail.ty`. Carried explicitly so codegen and
    /// future LSP queries can read the block's type without
    /// recursing through the tail.
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedStmt {
    pub kind: TypedStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypedStmtKind {
    Let {
        id: VarId,
        /// C2 / ADR 0017 D2: mutability of the binding. Type checker
        /// uses this to validate `&mut x` and `x = v;` against the
        /// binding's declaration.
        mutable: bool,
        name: String,
        name_span: Span,
        /// The variable's resolved type (from the annotation if
        /// present, otherwise inferred from the RHS).
        ty: Type,
        value: TypedExpr,
    },
    /// Assignment statement per ADR 0017 D2: `lhs = expr;`. LHS must
    /// be a *mutable* lvalue (validated here at type-check time);
    /// the LHS expression's type must match the RHS's. Mutable
    /// indexing (`a[i] = v;`) is out of scope at C2 per ADR 0017 D12.
    Assign {
        target: TypedExpr,
        value: TypedExpr,
    },
    /// Phase D.5 / ADR 0036 D3: `while <cond> { <body> }`. `cond` is
    /// `bool`-typed (validated here); the body block type-checks with
    /// its value discarded each iteration. The body's bindings drop
    /// per-iteration at codegen (ADR 0036 D5).
    While {
        cond: TypedExpr,
        body: Box<TypedBlock>,
    },
    /// Phase D.5 (2/N) / ADR 0036 D9: `break;` — exit the innermost
    /// enclosing `while` loop. The type checker has already verified
    /// (via the env's loop-nesting depth) that this sits inside a loop;
    /// codegen branches to that loop's `loop_after`.
    Break,
    /// Phase D.5 (2/N) / ADR 0036 D9: `continue;` — next iteration of
    /// the innermost enclosing `while` loop; codegen branches to its
    /// `loop_cond`.
    Continue,
    Expr(TypedExpr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    pub span: Span,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypedExprKind {
    IntLit(i64),
    /// ADR 0058: a float literal — the IEEE-754 bits. Always carries
    /// [`Type::F64`]. Codegen lowers it to a `double` constant via
    /// `f64::from_bits`.
    FloatLit(u64),
    /// Bool literal. Always carries [`Type::Bool`].
    BoolLit(bool),
    /// Null literal per ADR 0014 D2. Always carries
    /// [`Type::Nullable`]; the inner type comes from bidirectional
    /// checking against the expected context.
    NullLit,
    /// D.2 / ADR 0033 D2: a char/byte literal — the decoded byte.
    /// Always carries [`Type::U8`]. Codegen lowers it to an `i8`
    /// constant at D.2 (4/N) (it rejects until then).
    CharLit(u8),
    /// D.2 / ADR 0033 D2: a string literal — the decoded bytes.
    /// Always carries `[u8]` (`Type::Array(ArrayElem::U8)`). Codegen
    /// lowers it to a private global `[N x i8]` + heap copy so it
    /// drops/moves like any array (D6) at D.2 (4/N).
    StringLit(Vec<u8>),
    /// Implicit `T → ?T` widening per ADR 0014 D3. Wraps a `T`-typed
    /// expression so that the outer node carries `?T`. Codegen
    /// lowers this as constructing the `{ i1 true, T payload }`
    /// struct value.
    WidenToNullable(Box<TypedExpr>),
    /// C3 / ADR 0019 D5 (C3.1): implicit `T → secret T` widening.
    /// Used at let-annotation and arg-passing boundaries where the
    /// expected type is `secret T` but the synthesized expression
    /// is `T`. Codegen lowers as identity — secret wrapping is
    /// purely at the type level at C3.1 (constant-time codegen is
    /// deferred per ADR 0019 D12).
    WidenToSecret(Box<TypedExpr>),
    /// C3 / ADR 0019 D6 (C3.1): `declassify(e)` strips one layer
    /// of `secret` from the inner expression. The outer node's
    /// `ty` is the unwrapped inner type; idempotent on non-secret
    /// inputs (the inner just flows through, no `Type::Secret`
    /// wrap to strip).
    Declassify(Box<TypedExpr>),
    /// ADR 0065: `return expr` — an early return (divergent expression). The
    /// inner expression's type is checked against the enclosing function's
    /// declared return type; the outer `Return` node's `ty` is the **inner's
    /// type carried verbatim** (a concrete placeholder), but the node is
    /// *divergent* — at every type-join site (if/match branches, block tail)
    /// the checker treats a `Return` (or a branch that always returns) as
    /// yielding no value, so it unifies with whatever the position expects
    /// (see `expr_diverges`). Codegen lowers it as control flow (drops +
    /// branch to the function exit), so the `ty` is never read as a value.
    Return(Box<TypedExpr>),
    /// ADR 0049: integer cast `expr as T`. The resolved target type lives on
    /// the outer node's `ty` (possibly `secret`, preserving the operand's
    /// secrecy) — like `Declassify`, no separate target field. Codegen reads
    /// the source width from the inner's `ty` and the target from `ty`.
    Cast(Box<TypedExpr>),
    Var(VarId),
    /// ADR 0070: a non-capturing function value — a bare top-level fn name
    /// used in value position. The outer node's `ty` is always
    /// [`Type::Fn`]. Codegen lowers this to the LLVM function's address
    /// (`@name` as a pointer constant) — no wrapper, no captured state.
    FnRef(FnId),
    Unary(UnaryOp, Box<TypedExpr>),
    Binary(BinOp, Box<TypedExpr>, Box<TypedExpr>),
    /// Comparison. Both operands are the same numeric type; the
    /// expression's `ty` field is always [`Type::Bool`].
    Cmp(CmpOp, Box<TypedExpr>, Box<TypedExpr>),
    /// Logical `&&` / `||`. Both operands are `bool`; result is
    /// `bool`. Short-circuit semantics live in codegen.
    Logic(LogicOp, Box<TypedExpr>, Box<TypedExpr>),
    Block(Box<TypedBlock>),
    If {
        cond: Box<TypedExpr>,
        then_branch: Box<TypedBlock>,
        else_branch: Box<TypedBlock>,
    },
    Call {
        id: FnId,
        callee_span: Span,
        args: Vec<TypedExpr>,
        /// Concrete type arguments for a generic-fn call per ADR
        /// 0016 D7c. Empty `Vec` for calls to non-generic fns.
        /// Codegen consults this at monomorphic-instance emission
        /// to substitute the callee's body. Index `i` corresponds
        /// to [`TypeParamId(i)`] in the callee's type-param list.
        type_args: Vec<Type>,
    },
    /// Struct literal per ADR 0013 D3. The fields are reordered to
    /// match the declaration order so codegen can lower by index
    /// without consulting field names.
    StructLit {
        id: StructId,
        name: String,
        name_span: Span,
        /// Field values in **declaration order** (not source order).
        /// The check() pass rearranges source-order field inits to
        /// match the struct decl so codegen can iterate by index.
        fields: Vec<TypedExpr>,
    },
    /// Field access per ADR 0013 D2. `field_index` is the field's
    /// position in the declaration, for codegen's GEP offset.
    FieldAccess {
        target: Box<TypedExpr>,
        field: String,
        field_span: Span,
        field_index: usize,
    },
    /// Array literal `[e1, e2, …]` per ADR 0015 D2. All elements
    /// have been checked against the expected element type; the
    /// outer node's ty is `[T]` for the inferred T.
    ArrayLit {
        elem_ty: ArrayElem,
        elements: Vec<TypedExpr>,
    },
    /// Postfix indexing `target[index]` per ADR 0015 D3. The
    /// target's type is `[T]`; the index's type is `i64`; the
    /// outer node's ty is `T` (the element type promoted from
    /// ArrayElem).
    Index {
        target: Box<TypedExpr>,
        index: Box<TypedExpr>,
        elem_ty: ArrayElem,
    },
    /// C3.4 / ADR 0020 D5: `handle body with { arms }`. The body's
    /// inferred row is reduced by the union of effects covered by
    /// `arms` (the `handled` field). At C3.4 codegen rejects this
    /// variant — only type-check + effect-check consume it.
    Handle {
        body: Box<TypedExpr>,
        arms: Vec<TypedHandlerArm>,
        return_arm: Option<Box<TypedReturnArm>>,
        /// The (EffectId, op_index) pairs covered by `arms`,
        /// in arm order. Effect-check consults this to compute
        /// the discharge: body row - { effect of each entry } =
        /// outer row.
        handled: Vec<(EffectId, usize)>,
    },
    /// C3.4 / ADR 0020 D5: `perform Effect.Op(args)`. Contributes
    /// `effect_id` to the enclosing fn's inferred row. The outer
    /// expression's type is the op's declared return type.
    Perform {
        effect_id: EffectId,
        op_index: usize,
        effect_name: String,
        op_name: String,
        op_span: Span,
        args: Vec<TypedExpr>,
    },
    /// C3.4 / ADR 0020 D5: continuation resume `k(arg)` inside a
    /// handler arm body. `kont` is the VarId of the arm's
    /// continuation binding; its type in env is [`Type::Kont`].
    /// The expression's `ty` is the kont's `ret_ty` (= the outer
    /// `handle`'s type).
    ResumeKont {
        kont: VarId,
        callee_span: Span,
        args: Vec<TypedExpr>,
        kont_id: KontId,
    },
    /// C4.1 / ADR 0022 D3 + D7: method call `target.method(args)`.
    /// `class_id` is the receiver's static class; `method_index`
    /// is the index into the class's `methods` vec. Type-check has
    /// already verified the method exists, the arg types match,
    /// and the auto-ref of the receiver matches the method's
    /// `self_kind`. The receiver in `target` is the original lvalue
    /// expression (not pre-ref'd) — codegen emits the
    /// `&target` / `&mut target` per the method's self_kind via
    /// `lower_lvalue_ptr`.
    MethodCall {
        target: Box<TypedExpr>,
        class_id: ClassId,
        method_index: usize,
        method: String,
        method_span: Span,
        args: Vec<TypedExpr>,
    },
    /// C4.1 / ADR 0022 D5: `Name::init(args)` class instantiation.
    /// The outer node's `ty` is `Type::Class(class_id)`.
    ClassInit {
        id: ClassId,
        name: String,
        name_span: Span,
        args: Vec<TypedExpr>,
    },
    /// C4.2 / ADR 0023 D5 Path 1: receiver-typed dispatch — class
    /// `s.write(10)` routed to a default-impl method when the
    /// class itself has no such method. `impl_id` + `method_index`
    /// pick out the impl method; `target` is the receiver lvalue
    /// (codegen GEPs into its storage like a class MethodCall).
    /// `args` excludes the receiver.
    ImplMethodCall {
        target: Box<TypedExpr>,
        impl_id: ImplId,
        method_index: usize,
        method: String,
        method_span: Span,
        args: Vec<TypedExpr>,
    },
    /// C4.2 / ADR 0023 D5 Path 2: qualified-named dispatch —
    /// `Doubling::write(&mut s, 16)`. `args[0]` is the receiver as
    /// a ref-typed expression (not GEP'd); `args[1..]` are the
    /// remaining declared params.
    QualifiedCall {
        impl_id: ImplId,
        method_index: usize,
        impl_name: String,
        method: String,
        method_span: Span,
        args: Vec<TypedExpr>,
    },
    /// C4.4 / ADR 0024 D1: `scope concurrent { ... }`. The block's
    /// value is its tail; codegen wraps the body in
    /// scope_enter / scope_exit per ADR 0024 D8.
    Scope {
        mode: sentinel_ast::ScopeMode,
        body: Box<TypedBlock>,
    },
    /// C4.4 / ADR 0024 D2: `spawn fn_name(args)`. `call` is the
    /// validated inner [`TypedExprKind::Call`]; `task_id` interns
    /// `Task<result_ty>` (result_ty restricted to `I64` at C4.4
    /// minimum per D7). The outer node's `ty` is `Type::Task(task_id)`.
    Spawn {
        call: Box<TypedExpr>,
        task_id: TaskId,
    },
    /// C4.4 / ADR 0024 D3: `task.await`. `task_expr` is a
    /// `Type::Task`-typed receiver; the outer node's `ty` is the
    /// interned [`TaskData::result_ty`].
    Await {
        task_expr: Box<TypedExpr>,
        task_id: TaskId,
    },
    /// Phase D.1 / ADR 0032 (3/N): sum-type variant construction
    /// `Enum::Variant(args)`. The variant has been resolved to
    /// `variant_index` (its discriminant) against the enum's variant
    /// list, and `args` checked against the variant's payload types.
    /// The outer node's `ty` is `Type::Enum(enum_id)`. Codegen at
    /// D.1 (4/N) lowers this to the `{ i32 tag, ptr payload }`
    /// layout; at (3/N) codegen rejects it.
    EnumConstruct {
        enum_id: EnumId,
        variant_index: usize,
        enum_name: String,
        variant_name: String,
        args: Vec<TypedExpr>,
    },
    /// Phase D.1 / ADR 0032 (3/N): `match scrutinee { arms }`. The
    /// scrutinee is `Type::Enum(enum_id)`; the arms cover every
    /// variant (or include a wildcard) — exhaustiveness is a
    /// type-check guarantee (ADR 0032 D2/D5). Each arm's body has
    /// been checked against the outer expression's type (`ty`). At
    /// D.1 (4/N) codegen lowers this to an LLVM `switch` on the tag;
    /// at (3/N) codegen rejects it.
    Match {
        scrutinee: Box<TypedExpr>,
        enum_id: EnumId,
        arms: Vec<TypedMatchArm>,
    },
}

/// Phase D.1 / ADR 0032 (3/N): a type-checked `match` arm. The
/// pattern has been resolved to a variant index (or wildcard) and
/// its bindings typed from the matched variant's payloads; the body
/// has been checked against the outer `match` expression's type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedMatchArm {
    pub pattern: TypedPattern,
    pub body: TypedExpr,
    pub span: Span,
}

/// Phase D.1 / ADR 0032 (3/N): a type-checked `match`-arm pattern.
/// The variant is resolved to its index; each binding carries the
/// [`VarId`] it was scoped to plus the payload [`Type`] it binds
/// (so codegen can load + type the payload slot positionally).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypedPattern {
    Variant {
        variant_index: usize,
        variant_name: String,
        /// One entry per payload slot, in order: the binding VarId +
        /// its payload type. A `_` binding keeps its VarId (unused).
        bindings: Vec<TypedPatternBinding>,
        span: Span,
    },
    Wildcard(Span),
}

/// Phase D.1 / ADR 0032 (3/N): one positional binding of a variant
/// pattern, typed from the matched variant's payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedPatternBinding {
    pub var_id: VarId,
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// C3.4 / ADR 0020 D5: a type-checked handler arm. The arm's
/// effect+op pair has been resolved to (`effect_id`, `op_index`);
/// each param VarId is bound in env with the op's declared param
/// types (and the kont with [`Type::Kont`]). The arm body has been
/// checked against the outer `handle` expression's type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedHandlerArm {
    pub effect_id: EffectId,
    pub op_index: usize,
    pub effect_name: String,
    pub op_name: String,
    pub op_span: Span,
    pub param_var_ids: Vec<VarId>,
    pub param_names: Vec<Spanned<String>>,
    /// The KontId for the arm's continuation binding (last entry
    /// in `param_var_ids`). Used by codegen at C3.5/C3.6 to look
    /// up the kont's (arg_ty, ret_ty).
    pub kont_id: KontId,
    pub body: TypedExpr,
    pub span: Span,
}

/// C3.4 / ADR 0020 D4: the optional `return v => body` arm after
/// type-checking. `value_var_id` is bound in env with the handled
/// expression's type; `body`'s type equals the outer `handle`'s
/// type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedReturnArm {
    pub value_var_id: VarId,
    pub value_name: Spanned<String>,
    pub body: TypedExpr,
    pub span: Span,
}

// =============================================================================
// Errors
// =============================================================================

#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum TypeError {
    #[error("unknown type `{name}`")]
    #[diagnostic(
        code(sentinel::types::unknown_type),
        help("C1.2 only recognises `i64`; `i32` and `bool` arrive at C1.3 per ADR 0012 D5-D8")
    )]
    UnknownType {
        name: String,
        #[label("not a known type")]
        span: miette::SourceSpan,
    },

    /// ADR 0037 (2/N): an imported effecting `pub fn` names an effect the
    /// importing unit didn't `use`, so it isn't in scope to be handled.
    /// Span-free: the extern has no body in this unit; the fix is a `use`.
    #[error(
        "imported fn `{fn_name}` uses effect `{effect_name}`, which is not in scope \
         — add a `use` for it"
    )]
    #[diagnostic(code(sentinel::types::unknown_imported_effect))]
    UnknownImportedEffect { fn_name: String, effect_name: String },

    #[error("type mismatch: expected {expected}, found {got}")]
    #[diagnostic(code(sentinel::types::mismatch))]
    Mismatch {
        expected: Type,
        got: Type,
        #[label("expected {expected}, found {got}")]
        span: miette::SourceSpan,
    },

    #[error("`{name}` returns {expected} but its body produces {got}")]
    #[diagnostic(code(sentinel::types::return_type_mismatch))]
    ReturnTypeMismatch {
        name: String,
        expected: Type,
        got: Type,
        #[label("body produces {got}")]
        span: miette::SourceSpan,
    },

    #[error("argument {arg_index} of `{callee}` expects {expected}, got {got}")]
    #[diagnostic(code(sentinel::types::call_arg_mismatch))]
    CallArgMismatch {
        callee: String,
        arg_index: usize,
        expected: Type,
        got: Type,
        #[label("expected {expected}, got {got}")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D2: postfix `.field` requires the target to
    /// be a struct.
    #[error("field access on non-struct type `{got}`")]
    #[diagnostic(code(sentinel::types::field_access_on_non_struct))]
    FieldAccessOnNonStruct {
        got: Type,
        #[label("expected a struct, got {got}")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D2 + D3: the struct named `struct_name` has
    /// no field named `field`. Same diagnostic for both
    /// `expr.unknown` and `Foo { unknown: 1 }`.
    #[error("struct `{struct_name}` has no field `{field}`")]
    #[diagnostic(code(sentinel::types::unknown_field))]
    UnknownField {
        struct_name: String,
        field: String,
        #[label("no such field on {struct_name}")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D3: struct literal omits a field that the
    /// declaration requires (no defaults at C1.4).
    #[error("struct literal `{struct_name}` is missing field `{field}`")]
    #[diagnostic(code(sentinel::types::missing_field))]
    MissingField {
        struct_name: String,
        field: String,
        #[label("missing field {field}")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D7: a struct contains itself (directly or
    /// transitively) with no indirection. C1.5 / ADR 0014 D10
    /// relaxes the check so cycles via nullable edges are accepted;
    /// only direct-edge cycles surface this error now.
    #[error("recursive struct `{name}` has no representable size")]
    #[diagnostic(
        code(sentinel::types::recursive_struct),
        help("recursive structs need indirection — make at least one edge nullable via `?T`")
    )]
    RecursiveStruct {
        name: String,
        /// Names of the structs in the cycle, in order.
        cycle: Vec<String>,
        #[label("recursive struct cycle")]
        span: miette::SourceSpan,
    },

    /// C1.5 / ADR 0014 D2: a bare `null` literal without enough
    /// context to infer `?T` for some concrete T.
    #[error("ambiguous `null` — cannot infer the nullable's inner type")]
    #[diagnostic(
        code(sentinel::types::ambiguous_null),
        help("add a type annotation, e.g. `let x: ?i64 = null;`")
    )]
    AmbiguousNull {
        #[label("inner type unknown here")]
        span: miette::SourceSpan,
    },

    /// C1.6 / ADR 0015 D5: an empty array literal `[]` without
    /// enough context to infer `[T]` for some concrete T.
    #[error("ambiguous empty array — cannot infer the element type")]
    #[diagnostic(
        code(sentinel::types::ambiguous_empty_array),
        help("add a type annotation, e.g. `let xs: [i64] = [];`")
    )]
    AmbiguousEmptyArray {
        #[label("element type unknown here")]
        span: miette::SourceSpan,
    },

    /// C1.6 / ADR 0015 D3: indexing a non-array value.
    #[error("indexing on non-array type `{got}`")]
    #[diagnostic(code(sentinel::types::index_on_non_array))]
    IndexOnNonArray {
        got: Type,
        #[label("expected an array, got {got}")]
        span: miette::SourceSpan,
    },

    /// C1.6 / ADR 0015 D3: array index must be `i64`.
    #[error("array index must be `i64`, got `{got}`")]
    #[diagnostic(code(sentinel::types::index_not_int))]
    IndexNotInt {
        got: Type,
        #[label("expected i64, got {got}")]
        span: miette::SourceSpan,
    },

    /// C1.6 / ADR 0015 D6: nested arrays `[[T]]` are rejected.
    #[error("nested array types `[[T]]` are not allowed at C1.6")]
    #[diagnostic(
        code(sentinel::types::nested_array),
        help("multi-dimensional arrays are deferred to a future ADR")
    )]
    NestedArray {
        #[label("nested array here")]
        span: miette::SourceSpan,
    },

    /// Phase D.3 / ADR 0034 D8: a `Vec<T>` element must be in the flat
    /// subset (primitives or a struct). `Vec<Vec<T>>`, `Vec<[T]>`,
    /// `Vec<?T>`, and `Vec<&T>` are deferred — the same depth-1 limit
    /// as `[T]`'s `ArrayElem`.
    #[error("`Vec<T>` element type is not supported at the D.3 MVP")]
    #[diagnostic(
        code(sentinel::types::vec_element_not_supported),
        help("a Vec element must be a primitive (i64/i32/bool/u8) or a struct; collection / nullable / reference element types are deferred to a future ADR")
    )]
    VecElementNotSupported {
        #[label("unsupported Vec element type here")]
        span: miette::SourceSpan,
    },

    /// ADR 0066 M1.2b: a `Channel<T>` type annotation's element type must
    /// be `i64` at the M1.2b minimum (the singleton `Channel<i64>` the
    /// channel builtins are typed against). Generic word-scalar/aggregate
    /// element types are a follow-on (they need generic channel builtins,
    /// gated on `recv -> ?T` requiring a nullable-inner for `T`).
    #[error("`Channel<T>` element type is not supported yet")]
    #[diagnostic(
        code(sentinel::types::channel_element_not_supported),
        help("at the M1.2b minimum a channel carries `i64` (write `Channel<i64>`); other element types are a follow-on")
    )]
    ChannelElementNotSupported {
        #[label("unsupported Channel element type here")]
        span: miette::SourceSpan,
    },

    /// ADR 0071 M1.4a: a `Shared<T>` element (from a `Shared<T>` annotation or
    /// the `shared_new(v)` argument) must be a word-scalar {i64,i32,u8,bool,
    /// f64,ptr} at the M1.4a minimum (it is encoded into the cell's i64 slot).
    /// A non-word-scalar (an aggregate, `secret`, `u128`) is rejected — generic
    /// `Shared<secret T>` is M1.4c. Mirrors [`ChannelElementNotSupported`].
    #[error("`Shared<T>` element type is not supported yet")]
    #[diagnostic(
        code(sentinel::types::shared_element_not_supported),
        help("at the M1.4a minimum a `Shared<T>` holds a word-scalar (i64/i32/u8/bool/f64/ptr); aggregates and `secret` are a follow-on (M1.4c)")
    )]
    SharedElementNotSupported {
        #[label("unsupported Shared element type here")]
        span: miette::SourceSpan,
    },

    /// ADR 0071 M1.4b: a `Mutex<T>` element (from a `Mutex<T>` annotation or the
    /// `mutex_new(v)` argument) must be a word-scalar {i64,i32,u8,bool,f64,ptr} at
    /// the M1.4b minimum (it is encoded into the cell's i64 slot). A non-word-scalar
    /// (an aggregate, `secret`, `u128`) is rejected — generic `Mutex<secret T>` is
    /// M1.4c. Mirrors [`SharedElementNotSupported`].
    #[error("`Mutex<T>` element type is not supported yet")]
    #[diagnostic(
        code(sentinel::types::mutex_element_not_supported),
        help("at the M1.4b minimum a `Mutex<T>` holds a word-scalar (i64/i32/u8/bool/f64/ptr); aggregates and `secret` are a follow-on (M1.4c)")
    )]
    MutexElementNotSupported {
        #[label("unsupported Mutex element type here")]
        span: miette::SourceSpan,
    },

    /// ADR 0071 M1.4a slice 3: returning a NAMED `Shared<T>` binding (a bare `Var`
    /// in tail or `return` position) is not yet supported — the refcount TRANSFER
    /// exemption for such a return is deferred (M1.4a slice 3b: the byte-identical
    /// self-host mirror needs a reliable direct-Var-tail signal the append-only
    /// dialect lacks today). Return `shared_new(...)` (or any call/expression)
    /// DIRECTLY instead — an rvalue return transfers its refcount unit with no
    /// exemption needed; or bind the handle in the caller. Without this guard a
    /// returned named `Shared` would be both transferred AND dropped (a
    /// double-release / use-after-free), since a `Shared` is `Copy` and so is never
    /// move-recorded like the returned Move-typed bindings the drop drain skips.
    #[error("returning a named `Shared<T>` binding is not yet supported")]
    #[diagnostic(
        code(sentinel::types::shared_return_not_supported),
        help("return `shared_new(...)` (or the producing expression) directly — an rvalue return transfers the refcount unit; or bind the handle in the caller. Returning a named `Shared` local/param is a deferred follow-on (ADR 0071 M1.4a slice 3b).")
    )]
    SharedReturnNotSupported {
        #[label("a named `Shared` binding returned here")]
        span: miette::SourceSpan,
    },

    /// ADR 0066 M2.4a / ADR 0069 D1: a `SealedChannel<…>` type annotation's
    /// element must be `secret i64` at the M2.4a minimum. A *non-secret* element
    /// (`SealedChannel<i64>`) is a type error — sealing a public value is
    /// pointless (use the raw `process_send` path). Generic `secret T` + a wider
    /// payload are M2.4c.
    #[error("`SealedChannel<T>` element type must be `secret i64`")]
    #[diagnostic(
        code(sentinel::types::sealed_channel_element_not_supported),
        help("a SealedChannel carries an encrypted SECRET — write `SealedChannel<secret i64>` (a public element is pointless; use the raw `process_send` path); generic `secret T` is a follow-on")
    )]
    SealedChannelElementNotSupported {
        #[label("unsupported SealedChannel element type here")]
        span: miette::SourceSpan,
    },

    /// ADR 0070 D1 (generalized): `Fn<T, R>` requires both T and R to be
    /// word-scalars (i64/i32/u8/bool/f64/ptr) — any other type (aggregates,
    /// `secret`, generics) is a type error.
    #[error("`Fn<T, R>` requires word-scalar T and R")]
    #[diagnostic(
        code(sentinel::types::fn_type_args_not_supported),
        help("both T and R must be word-scalars (i64/i32/u8/bool/f64/ptr) — other shapes (aggregates, secret, generics) are a follow-on")
    )]
    FnTypeArgsNotSupported {
        #[label("unsupported Fn<..> shape here")]
        span: miette::SourceSpan,
    },

    /// ADR 0070 D4 (generalized): a bare fn name used as a value
    /// (`let op = name;`) must name a non-generic, non-builtin, effect-free
    /// top-level fn with exactly one word-scalar param and a word-scalar
    /// return to be eligible as a `Fn<T,R>` value.
    #[error("`{name}` cannot be used as a function value")]
    #[diagnostic(
        code(sentinel::types::fn_value_signature_not_supported),
        help("a function value must be a non-generic, non-builtin, effect-free top-level fn taking exactly one word-scalar param and returning a word-scalar")
    )]
    FnValueSignatureNotSupported {
        name: String,
        #[label("not eligible as a Fn<T,R> value")]
        span: miette::SourceSpan,
    },

    /// ADR 0070: `apply(f, x)`'s first argument must be a `Fn<T,R>` value —
    /// surfaced separately from `CallArgMismatch` because there is no single
    /// "expected" type to show (any word-scalar `Fn<T,R>` is acceptable).
    #[error("`apply`'s first argument must be a function value (`Fn<T,R>`), got `{got}`")]
    #[diagnostic(
        code(sentinel::types::apply_target_not_fn),
        help("pass a bare top-level fn name (e.g. `apply(square, x)`), not a call result or other value")
    )]
    ApplyTargetNotFn {
        got: Type,
        #[label("not a Fn<T,R> value")]
        span: miette::SourceSpan,
    },

    /// ADR 0070 (D3-revisit): a bound local variable was called (`f(x)`)
    /// but is neither a continuation (`Type::Kont`, inside a handler arm)
    /// nor a function value (`Type::Fn`) — e.g. `let x = 5; x(3);`.
    /// Replaces the old `Mismatch{expected: Type::Kont(KontId(u32::MAX))}`
    /// sentinel-value hack, which misleadingly implied Kont was the only
    /// valid type now that `Fn<T,R>` is also callable this way.
    #[error("cannot call `{got}` — expected a continuation or a function value (`Fn<T,R>`)")]
    #[diagnostic(
        code(sentinel::types::callee_not_callable),
        help("only a handler arm's continuation parameter or a `Fn<T,R>`-typed value can be called directly")
    )]
    CalleeNotCallable {
        got: Type,
        #[label("not callable")]
        span: miette::SourceSpan,
    },

    /// ADR 0070 (D3-revisit): calling a `Fn<T,R>` value directly (`f(x)`)
    /// with the wrong number of arguments — `Fn<T,R>` is always exactly
    /// one parameter, mirroring `KontArityMismatch`'s shape for resumes.
    #[error("function value expects {expected} argument(s), got {got}")]
    #[diagnostic(
        code(sentinel::types::fn_value_arity_mismatch),
        help("a `Fn<T,R>` value always takes exactly one argument")
    )]
    FnValueArityMismatch {
        expected: usize,
        got: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },

    /// ADR 0070 (D3-revisit): calling a `Fn<T,R>` value directly (`f(x)`)
    /// with an argument of the wrong type — the direct-call-syntax twin of
    /// `apply`'s own `CallArgMismatch`. Kept as a separate diagnostic
    /// rather than reusing `CallArgMismatch` because that variant's
    /// message names the callee (e.g. "argument 1 of `apply` expects...")
    /// and there is no `apply` token in the source to name here.
    #[error("function value expects an argument of type `{expected}`, got `{got}`")]
    #[diagnostic(code(sentinel::types::fn_value_arg_mismatch))]
    FnValueArgMismatch {
        expected: Type,
        got: Type,
        #[label("wrong argument type")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D11: `fn main` cannot be generic (the C ABI
    /// is monomorphic; main is the program entry point).
    #[error("`fn main` cannot have type parameters")]
    #[diagnostic(
        code(sentinel::types::generic_main),
        help("`main` is the C-ABI entry point per ADR 0016 D11; remove the generic parameter list")
    )]
    GenericMain {
        #[label("type parameters on main are forbidden")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D4: a generic-fn call's type argument can't
    /// be inferred from the supplied arguments (e.g., null literals
    /// in every position that mentions T, with no other binding
    /// site). The user adds an annotation to resolve.
    #[error("ambiguous type argument for `{type_param}` in call to `{callee}`")]
    #[diagnostic(
        code(sentinel::types::ambiguous_type_arg),
        help("add a type annotation, e.g. `let x: ?i64 = {callee}(...)`, so the type checker can pin {type_param}")
    )]
    AmbiguousTypeArg {
        callee: String,
        type_param: String,
        #[label("can't infer the type here")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D4: a generic-fn call binds the same
    /// type-parameter to two different concrete types across
    /// different arg positions (e.g., `pair<T>(a: T, b: T)` called
    /// with `pair(1, true)` — T = i64 vs T = bool).
    #[error("conflicting inference for `{type_param}` in call to `{callee}`: bound to `{first}` then to `{second}`")]
    #[diagnostic(code(sentinel::types::type_arg_inference_conflict))]
    TypeArgInferenceConflict {
        callee: String,
        type_param: String,
        first: Type,
        second: Type,
        #[label("inferred {second} here, conflicts with {first}")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D3: a `Name<T1, T2>` use in type position
    /// supplies the wrong number of type arguments.
    #[error("`{type_name}` takes {expected} type argument(s), got {found}")]
    #[diagnostic(code(sentinel::types::type_arg_count_mismatch))]
    TypeArgCountMismatch {
        type_name: String,
        expected: usize,
        found: usize,
        #[label("wrong number of type arguments")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D3: `<...>` was supplied for a non-generic
    /// type name (a primitive, a non-generic struct, or a
    /// type-parameter).
    #[error("`{type_name}` is not a generic type")]
    #[diagnostic(
        code(sentinel::types::type_args_on_non_generic),
        help("only generic structs accept type arguments; `{type_name}` doesn't take any")
    )]
    TypeArgsOnNonGeneric {
        type_name: String,
        #[label("type arguments on a non-generic type")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D3: a generic struct was referenced without
    /// its type arguments (e.g., bare `Box` instead of `Box<i64>`).
    #[error("`{type_name}` is generic; supply {expected_count} type argument(s)")]
    #[diagnostic(
        code(sentinel::types::missing_type_args),
        help("write `{type_name}<...>` with the type arguments")
    )]
    MissingTypeArgs {
        type_name: String,
        expected_count: usize,
        #[label("missing type arguments")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016: a generic struct literal needs an expected
    /// type to pin its type arguments. Bare `Pair { ... }` with no
    /// `let x: Pair<...> = ...` context surfaces this.
    #[error("ambiguous generic struct literal `{struct_name}` — supply type arguments via context")]
    #[diagnostic(
        code(sentinel::types::ambiguous_generic_struct_lit),
        help("annotate the binding, e.g. `let p: {struct_name}<...> = {struct_name} {{ ... }};`")
    )]
    AmbiguousGenericStructLit {
        struct_name: String,
        #[label("can't infer type arguments")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D1 + D11: nested references like `&&T` are
    /// not allowed (depth-1 amendment). The parser allows `& &T`
    /// to land here; `&&` lexes as logical-and at the token level.
    #[error("nested references `&&T` are not allowed")]
    #[diagnostic(
        code(sentinel::types::nested_ref),
        help("references are depth-1 at C2 per ADR 0017 D11; remove one of the `&`s")
    )]
    NestedRef {
        #[label("nested reference here")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D1: refs in array elements are rejected. The
    /// array could outlive the refs (a "first-class refs" problem),
    /// which needs named regions per D7 / D12.
    #[error("references in array elements are not allowed at C2")]
    #[diagnostic(
        code(sentinel::types::ref_in_array),
        help("first-class refs (storable in arrays) need named regions per ADR 0017 D7; this is deferred to a later ADR")
    )]
    RefInArray {
        #[label("reference in array element type")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D7 / D12: refs in struct field types are
    /// rejected for the same reason as `RefInArray` — first-class
    /// refs need named regions.
    #[error("references in struct fields are not allowed at C2")]
    #[diagnostic(
        code(sentinel::types::ref_in_struct_field),
        help("first-class refs (storable in struct fields) need named regions per ADR 0017 D7; this is deferred to a later ADR")
    )]
    RefInStructField {
        #[label("reference in struct field type")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D3 + D5: `&expr` / `&mut expr` requires the
    /// operand to be an lvalue (a place — Var, deref, field-access,
    /// or index). R-value borrowing like `&5` or `&(a + b)` is
    /// rejected; later ADRs may add temporary-promotion.
    #[error("cannot borrow a non-lvalue expression")]
    #[diagnostic(
        code(sentinel::types::borrow_of_rvalue),
        help("only variables, dereferences, field accesses, and indexes can be borrowed; bind the expression to a `let` first")
    )]
    BorrowOfRvalue {
        #[label("not an lvalue")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D2 + D5: assignment LHS must be an lvalue.
    #[error("cannot assign to a non-lvalue expression")]
    #[diagnostic(
        code(sentinel::types::assign_to_rvalue),
        help("assignment requires an lvalue (variable, deref, or field-access)")
    )]
    AssignToRvalue {
        #[label("not an lvalue")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D2: assignment to an immutable binding requires
    /// `let mut`.
    #[error("cannot assign to immutable binding `{name}`")]
    #[diagnostic(
        code(sentinel::types::assign_to_immutable),
        help("change the declaration to `let mut {name}` to allow re-assignment")
    )]
    AssignToImmutable {
        name: String,
        #[label("binding `{name}` is immutable")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D3: exclusive borrow of an immutable binding
    /// requires `let mut` (or `mut` on a param).
    #[error("cannot take `&mut` of immutable binding `{name}`")]
    #[diagnostic(
        code(sentinel::types::borrow_mut_of_immutable),
        help("change the declaration to `let mut {name}` to allow exclusive borrowing")
    )]
    BorrowMutOfImmutable {
        name: String,
        #[label("binding `{name}` is immutable")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D4: dereference applied to a non-ref operand.
    #[error("dereference of non-reference type `{got}`")]
    #[diagnostic(
        code(sentinel::types::deref_of_non_ref),
        help("`*expr` requires `expr` to have a `&T` or `&mut T` type")
    )]
    DerefOfNonRef {
        got: Type,
        #[label("expected a reference, got {got}")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D2: deref-assignment (`*r = v;`) through a
    /// shared `&T` is rejected — only `&mut T` allows writes.
    #[error("cannot assign through a shared reference `&T`")]
    #[diagnostic(
        code(sentinel::types::assign_through_shared_ref),
        help("the reference must be `&mut T` to be written to; declare or pass it as `&mut`")
    )]
    AssignThroughSharedRef {
        #[label("shared reference is read-only")]
        span: miette::SourceSpan,
    },

    /// C2 / ADR 0017 D12: mutable indexing (`a[i] = v;`) was out of
    /// scope at C2. ADR 0050 lifts the *assignment* case
    /// (`check_mutable_lvalue`); this variant now fires only for the
    /// `&mut a[i]` element-borrow case (`check_mutable_borrow_target`),
    /// which stays out of scope.
    #[error("mutable indexing `&mut a[i]` is not supported")]
    #[diagnostic(
        code(sentinel::types::index_assign_not_supported),
        help("taking `&mut` of an array element is not supported; bind the element via a new `let` or assign it with `a[i] = v;` (ADR 0050)")
    )]
    IndexAssignNotSupported {
        #[label("indexing on LHS of borrow")]
        span: miette::SourceSpan,
    },

    /// ADR 0050: index-assignment `a[i] = v;` ships for Copy elements
    /// (scalars and `secret` scalars — a plain element store). A **Move**
    /// element (a struct, or a generic `TypeParam` / `GenericInstance`)
    /// would need drop-on-overwrite semantics, deferred to a later ADR;
    /// reject it for now with a clear message.
    #[error("cannot index-assign an element of non-Copy type `{elem_ty}`")]
    #[diagnostic(
        code(sentinel::types::index_assign_non_copy),
        help("`a[i] = v;` currently supports scalar / `secret` scalar elements; for struct / generic elements rebuild the array via a new `let` (ADR 0050)")
    )]
    IndexAssignNonCopyElem {
        elem_ty: Type,
        #[label("non-Copy element type")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D5 (C3.0 deferral): `secret T` parses at
    /// C3.0 but the `Type::Secret(SecretId)` interner + the five
    /// static constant-time rejections ship together at C3.1.
    /// The types layer rejects any `secret T` annotation until
    /// then.
    #[error("`secret T` is not yet supported (lands at C3.1)")]
    #[diagnostic(
        code(sentinel::types::secret_not_yet),
        help("the `Type::Secret(SecretId)` qualifier + the five static constant-time checks ship at C3.1 per ADR 0019 D5/D7")
    )]
    SecretNotYet {
        #[label("`secret T` here")]
        span: miette::SourceSpan,
    },

    /// ADR 0058: `secret f64` is rejected. Floating-point operations are
    /// not constant-time on real hardware (subnormal slow paths,
    /// data-dependent `fdiv`/`sqrt` latency, NaN microcode branches), so a
    /// `secret f64` would advertise a constant-time guarantee the hardware
    /// cannot keep. Floats are a disjoint PUBLIC domain; the `secret` /
    /// constant-time type system is for integer crypto. Surfaced both at a
    /// `secret f64` annotation and at an `x as f64` cast of a `secret` value
    /// (the fence — no secret float value can ever exist).
    #[error("`secret f64` is not allowed — float operations are not constant-time")]
    #[diagnostic(
        code(sentinel::types::secret_float),
        help("floats are a public-only domain (ADR 0058); keep secret data in integer types, or `declassify` before converting to `f64`")
    )]
    SecretFloat {
        #[label("`secret f64` here")]
        span: miette::SourceSpan,
    },

    /// ADR 0058: bitwise (`& | ^`) and shift (`<< >>`) operators are not
    /// supported on `f64`. Floats support only `+ - * /` and the ordered
    /// comparisons; bit manipulation is meaningless on an IEEE-754 value.
    #[error("bitwise / shift operators are not supported on `f64`")]
    #[diagnostic(
        code(sentinel::types::float_bitwise),
        help("`f64` supports `+ - * /`, comparisons, unary `-`, and `sqrt`; reinterpret via an `as` cast to an integer type for bit manipulation")
    )]
    FloatBitwise {
        #[label("bitwise / shift on `f64` here")]
        span: miette::SourceSpan,
    },

    /// ADR 0057: an `extern "C"` parameter / return type is not in the
    /// FFI-safe set. Phase 1 allows only PUBLIC `i64` and `f64` (the native
    /// word + C `double`). A `secret` type is rejected here — the FFI fence:
    /// an `extern` call jumps into code the compiler cannot verify, so secret
    /// data may not cross it (declassify first). Structs / arrays / `ptr` /
    /// other widths are deferred to a later FFI phase.
    #[error("`extern` fn types must be public FFI-safe scalars (`i64` or `f64`)")]
    #[diagnostic(
        code(sentinel::types::extern_ffi_type),
        help("Phase 1 FFI allows only public `i64` / `f64`; a `secret` value cannot cross the FFI boundary (declassify first), and structs / pointers / other widths are a later phase (ADR 0057)")
    )]
    ExternFfiType {
        #[label("not an FFI-safe type")]
        span: miette::SourceSpan,
    },

    /// ADR 0057 Phase 1b: `ptr_of` / `ptr_of_mut` were applied to something
    /// other than a borrow of a PUBLIC byte array. `ptr_of` needs `&[u8]` (or
    /// `&mut [u8]`); `ptr_of_mut` needs `&mut [u8]` specifically. A
    /// `&[secret u8]` is rejected — the FFI fence keeps a secret buffer's
    /// pointer from crossing to unverified C.
    #[error("`ptr_of` / `ptr_of_mut` need a borrow of a public `[u8]` (`&[u8]` / `&mut [u8]`)")]
    #[diagnostic(
        code(sentinel::types::ptr_of_arg),
        help("write `ptr_of(&buf)` over a `[u8]` (or `ptr_of_mut(&mut buf)` for a writable buffer); a `[secret u8]` cannot cross the FFI fence (ADR 0057)")
    )]
    PtrOfArg {
        #[label("not a public byte-array borrow")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D7 (C3.1) — constant-time rejection: an `if` or
    /// (since D.5) a `while` whose condition has type `secret bool`
    /// would leak the secret via timing. Reject. `kw` is the offending
    /// construct (`"if"` or `"while"`). Workaround: `declassify(cond)`
    /// first if the user accepts the leak.
    #[error("`{kw}` on a `secret bool` condition would leak via timing")]
    #[diagnostic(
        code(sentinel::types::secret_branch),
        help("branching on a secret value leaks the secret via timing; declassify the condition first if the leak is acceptable")
    )]
    SecretBranch {
        kw: &'static str,
        #[label("secret-typed condition here")]
        span: miette::SourceSpan,
    },

    /// Phase D.5 (2/N) / ADR 0036 D9: `break` / `continue` appeared
    /// outside any enclosing `while` loop. There is no `loop_after` /
    /// `loop_cond` to branch to, so it is rejected here (the env's
    /// loop-nesting depth is zero at this point). `kw` is the offending
    /// keyword (`"break"` or `"continue"`).
    #[error("`{kw}` used outside of a loop")]
    #[diagnostic(
        code(sentinel::types::loop_control_outside_loop),
        help("`break` and `continue` may only appear inside a `while` loop body")
    )]
    LoopControlOutsideLoop {
        kw: &'static str,
        #[label("`{kw}` is not inside a `while` loop")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D7 (C3.1) — constant-time rejection:
    /// variable-time `/` and `%` on a secret divisor leak the
    /// divisor's bit pattern via timing on most CPUs. Reject.
    #[error("variable-time division by a `secret` value leaks via timing")]
    #[diagnostic(
        code(sentinel::types::secret_divisor),
        help("division and modulo by a secret value have data-dependent latency; declassify the divisor first if the leak is acceptable")
    )]
    SecretDivisor {
        #[label("secret divisor here")]
        span: miette::SourceSpan,
    },

    /// ADR 0048 — constant-time rejection: shifting by a `secret`
    /// amount (`x << secret_n` / `x >> secret_n`) is a variable-time
    /// operation that leaks the amount via timing, like a secret
    /// divisor. The shifted VALUE may be secret (constant-time when the
    /// amount is public); only a secret AMOUNT is rejected.
    #[error("variable-time shift by a `secret` amount leaks via timing")]
    #[diagnostic(
        code(sentinel::types::secret_shift_amount),
        help("a shift by a secret amount has data-dependent latency; declassify the amount first if the leak is acceptable")
    )]
    SecretShiftAmount {
        #[label("secret shift amount here")]
        span: miette::SourceSpan,
    },

    /// ADR 0049 — a cast `x as T` requires both the operand and the
    /// target `T` to be integer types (`i64` / `i32` / `u8`). Anything
    /// else (a non-integer operand, or a non-integer / `secret` / array
    /// target) is rejected.
    #[error("`as` cast requires integer operand and target (i64 / i32 / u8)")]
    #[diagnostic(
        code(sentinel::types::non_integer_cast),
        help("only integer casts are supported; widen secrecy with a `let`, not a cast")
    )]
    NonIntegerCast {
        #[label("invalid cast here")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D7 (C3.1) — constant-time rejection:
    /// dereferencing a `secret &T` (where the *pointer* is
    /// secret, distinct from `& secret T` where the pointee is
    /// secret) leaks through the cache side channel. Reject.
    #[error("dereferencing a secret reference leaks via the memory side channel")]
    #[diagnostic(
        code(sentinel::types::secret_in_ref_deref),
        help("`*r` where `r: secret &T` is rejected at C3.1; the access pattern depends on the secret pointer. Use `& secret T` instead to make only the pointee secret.")
    )]
    SecretInRefDeref {
        #[label("deref of `secret &T` here")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: a continuation binding `k` was used as
    /// a value (e.g., `let f = k;` or `k + 1`) instead of in a
    /// resume call. Konts are only valid as the callee of a
    /// resume — passing them around is rejected so handlers
    /// can't be smuggled past their handle.
    #[error("continuation binding can only be called, not used as a value")]
    #[diagnostic(
        code(sentinel::types::kont_used_as_value),
        help("inside a handler arm, the continuation `k` may only appear as `k(arg)` — assigning it, passing it, or operating on it is rejected at C3.4")
    )]
    KontUsedAsValue {
        #[label("continuation used as a value here")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D6: a handler arm's body type doesn't
    /// match the outer `handle` expression's type. Every arm —
    /// op arms and the optional return arm — must produce the
    /// same type, the `handle`'s value.
    #[error(
        "handler arm `{effect_name}.{op_name}` body returns {got} but the `handle` expression's type is {expected}"
    )]
    #[diagnostic(
        code(sentinel::types::handler_arm_type_mismatch),
        help("all arms of a `handle` (op arms + optional return arm) must produce the same type as the handle expression")
    )]
    HandlerArmTypeMismatch {
        effect_name: String,
        op_name: String,
        expected: Type,
        got: Type,
        #[label("arm body has the wrong type")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: a handler arm's param list doesn't
    /// match the op's declared arity (plus the trailing
    /// continuation). For `op(p1: T1, p2: T2) -> R`, an arm
    /// must declare exactly three params: two for the op + one
    /// for `k`.
    #[error(
        "handler arm `{effect_name}.{op_name}` has {got} parameter(s) but expected {expected} (including the trailing continuation `k`)"
    )]
    #[diagnostic(
        code(sentinel::types::operation_arity_mismatch),
        help("the op's params come first; the last param is always the continuation `k`")
    )]
    OperationArityMismatch {
        effect_name: String,
        op_name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of arm parameters")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: a continuation-resume call `k(arg)`
    /// passed the wrong number of args. At C3.4 minimum konts
    /// are always unary (single-value resume per the op's
    /// return type).
    #[error(
        "continuation resume call expected {expected} argument(s), got {got}"
    )]
    #[diagnostic(
        code(sentinel::types::kont_arity_mismatch),
        help("a resume call passes one value per element of the op's return type — typically a single i64")
    )]
    KontArityMismatch {
        expected: usize,
        got: usize,
        #[label("wrong number of resume arguments")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D4: init body did not assign `self.field`
    /// on every path before returning.
    #[error("class `{class_name}` field `{field_name}` is not assigned by the end of `init`")]
    #[diagnostic(
        code(sentinel::types::init_field_maybe_unassigned),
        help("every declared field must be definite-assigned inside `init` per ADR 0022 D4")
    )]
    InitFieldMaybeUnassigned {
        class_name: String,
        field_name: String,
        #[label("field declared here")]
        field_span: miette::SourceSpan,
        #[label("init body ends here without assigning `{field_name}`")]
        init_span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D5: a struct-literal expression targets a
    /// class — `Name { field: value, ... }` for a class is
    /// rejected to enforce D4's no-half-constructed invariant.
    #[error("class `{name}` cannot be constructed with struct-literal syntax")]
    #[diagnostic(
        code(sentinel::types::class_construction_must_use_init),
        help("call `{name}::init(args)` instead — classes use the init constructor per ADR 0022 D5")
    )]
    ClassConstructionMustUseInit {
        name: String,
        #[label("use `{name}::init(...)` here")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D3 + D7: a method call references a name
    /// not declared on the receiver's class.
    #[error("class `{class_name}` has no method `{method_name}`")]
    #[diagnostic(
        code(sentinel::types::method_not_found),
        help("check the class declaration for available method names")
    )]
    MethodNotFound {
        class_name: String,
        method_name: String,
        #[label("no such method")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D7: postfix `.method(args)` on a receiver
    /// whose static type isn't a class.
    #[error("method call requires a class receiver (got `{got_ty}`)")]
    #[diagnostic(
        code(sentinel::types::method_call_on_non_class),
        help("only class instances + class references support postfix method-call syntax at C4.1")
    )]
    MethodCallOnNonClass {
        got_ty: String,
        #[label("not a class receiver")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D5: `Name::init(args)` was given the wrong
    /// number of arguments.
    #[error("class `{class_name}::init` expected {expected} argument(s), got {got}")]
    #[diagnostic(code(sentinel::types::class_init_arity_mismatch))]
    ClassInitArityMismatch {
        class_name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of init arguments")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D3: a method call passes the wrong number
    /// of arguments (excluding the implicit `self`).
    #[error("method `{class_name}.{method_name}` expected {expected} argument(s), got {got}")]
    #[diagnostic(code(sentinel::types::method_arity_mismatch))]
    MethodArityMismatch {
        class_name: String,
        method_name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of method arguments")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D8: an impl block omits a method declared on
    /// its trait. Every trait method must be supplied (no default
    /// bodies at C4.2 per D10).
    #[error("impl of `{trait_name}` for `{type_name}` is missing method `{method_name}`")]
    #[diagnostic(
        code(sentinel::types::impl_missing_method),
        help("supply a `fn {method_name}(...)` matching the trait signature; default method bodies are deferred per ADR 0023 D10")
    )]
    ImplMissingMethod {
        trait_name: String,
        type_name: String,
        method_name: String,
        #[label("impl block here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D8: an impl method's signature doesn't match
    /// the trait's after substituting `Self` → impl target.
    #[error(
        "impl method `{method_name}` for `{trait_name}` on `{type_name}` has a signature that doesn't match the trait"
    )]
    #[diagnostic(
        code(sentinel::types::impl_method_signature_mismatch),
        help("the impl method's params + return type (with `Self` substituted to `{type_name}`) must match the trait's signature exactly")
    )]
    ImplMethodSignatureMismatch {
        trait_name: String,
        type_name: String,
        method_name: String,
        #[label("signature mismatch here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D6: a `obj.method(...)` call where the
    /// receiver type has more than one default impl providing a
    /// method by that name (across distinct traits).
    #[error("ambiguous method call `{method_name}` on `{recv_ty}`")]
    #[diagnostic(
        code(sentinel::types::ambiguous_method_call),
        help("multiple traits provide a default impl with method `{method_name}` on `{recv_ty}` — use the qualified form `ImplName::{method_name}(receiver, ...)` to disambiguate")
    )]
    AmbiguousMethodCall {
        method_name: String,
        recv_ty: String,
        #[label("ambiguous method call here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D6: a `ImplName::method(receiver, ...)` call
    /// where the receiver's type doesn't match the impl's `for
    /// Type` clause (or a `&Type` / `&mut Type` ref to it).
    #[error(
        "impl `{impl_name}` (for `{type_name}`) doesn't apply to receiver type `{got_ty}`"
    )]
    #[diagnostic(
        code(sentinel::types::impl_method_receiver_mismatch),
        help("the first argument's type (after auto-deref) must match the impl's `for Type` clause")
    )]
    ImplMethodReceiverMismatch {
        impl_name: String,
        type_name: String,
        got_ty: String,
        #[label("receiver here")]
        span: miette::SourceSpan,
    },

    /// C4.4 / ADR 0024 D2: `spawn expr` where `expr` is not a
    /// direct function call. At C4.4 minimum the spawn target must
    /// be a `fn_name(args)` call (arbitrary spawnable expressions
    /// are deferred per ADR 0024 D10).
    #[error("`spawn` requires a function-call target")]
    #[diagnostic(
        code(sentinel::types::spawn_must_be_call),
        help("at C4.4 minimum `spawn` takes a direct call like `spawn double(21)`; blocks / method calls / closures are deferred per ADR 0024 D10")
    )]
    SpawnMustBeCall {
        #[label("not a function call")]
        span: miette::SourceSpan,
    },

    /// ADR 0066 M1.1: a `spawn` argument or result type that codegen
    /// can't yet pack/return. M1.1 lifts the ADR 0024 D7 `Task<i64>`-only
    /// restriction to any **word-sized scalar** (`i64`/`i32`/`u8`/`bool`/
    /// `f64`/`ptr`/`Task`/class) — the value the per-spawn wrapper packs
    /// into an 8-byte slot and encodes into the Task's `i64` result slot
    /// (see [`is_spawn_word_scalar`]). Aggregates (`u128`/struct/enum/
    /// `Vec`/`[T]`/`?T`) and `secret` are deferred: a struct needs a
    /// wider slot + a boxed result, and `secret` crossing a thread
    /// boundary lands with channels (ADR 0066 D8 / M1.2).
    #[error("`spawn` {role} type `{got}` is not supported yet")]
    #[diagnostic(
        code(sentinel::types::spawn_type_unsupported),
        help("ADR 0066 M1.1 supports word-sized scalar `spawn` arg/result types (i64/i32/u8/bool/f64/ptr/Task/class); aggregates (u128/struct/enum/Vec/[T]/?T) and `secret` are deferred")
    )]
    SpawnTypeUnsupported {
        got: Type,
        role: &'static str,
        #[label("unsupported `spawn` type")]
        span: miette::SourceSpan,
    },

    /// C4.4 / ADR 0024 D3: `.await` applied to a non-`Task<T>`
    /// receiver.
    #[error("`.await` requires a `Task<T>` receiver (got `{got}`)")]
    #[diagnostic(
        code(sentinel::types::await_on_non_task),
        help("`.await` may only be applied to the result of a `spawn` (a `Task<T>` value)")
    )]
    AwaitOnNonTask {
        got: Type,
        #[label("not a Task")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): `Enum::Variant` names a variant
    /// that the enum doesn't declare (construction or pattern).
    #[error("enum `{enum_name}` has no variant `{variant_name}`")]
    #[diagnostic(code(sentinel::types::unknown_variant))]
    UnknownVariant {
        enum_name: String,
        variant_name: String,
        #[label("no such variant on {enum_name}")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): a variant construction or pattern
    /// supplies the wrong number of payload values / bindings.
    #[error("variant `{enum_name}::{variant_name}` takes {expected} payload(s), got {got}")]
    #[diagnostic(code(sentinel::types::variant_payload_arity_mismatch))]
    VariantPayloadArityMismatch {
        enum_name: String,
        variant_name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of payloads")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): the scrutinee of a `match` is not
    /// an `enum` type.
    #[error("`match` requires an enum scrutinee (got `{got}`)")]
    #[diagnostic(
        code(sentinel::types::match_scrutinee_not_enum),
        help("`match` at the D.1 MVP works over `enum` values; the scrutinee must be an enum")
    )]
    MatchScrutineeNotEnum {
        got: Type,
        #[label("not an enum")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N) D2: a `match` doesn't cover every
    /// variant of its scrutinee's enum, and has no `_` arm.
    #[error("non-exhaustive `match` on `{enum_name}`: missing {}", missing.join(", "))]
    #[diagnostic(
        code(sentinel::types::non_exhaustive_match),
        help("add an arm for each missing variant, or a `_` wildcard arm")
    )]
    NonExhaustiveMatch {
        enum_name: String,
        missing: Vec<String>,
        #[label("not all variants are covered")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): two `match` arms produce different
    /// types (`match` is an expression — all arms share a type).
    #[error("`match` arms have incompatible types: expected {expected}, found {got}")]
    #[diagnostic(code(sentinel::types::match_arm_type_mismatch))]
    MatchArmTypeMismatch {
        expected: Type,
        got: Type,
        #[label("expected {expected}, found {got}")]
        span: miette::SourceSpan,
    },
}

// =============================================================================
// check() — pure-function entry point
// =============================================================================

/// Type-check a [`ResolvedProgram`] and produce a [`TypedProgram`].
/// Fails fast on the first error, matching the C0/C1.1 fail-fast
/// pattern of lex / parse / resolve.
///
/// Phase D.6 / ADR 0037 D5.1: an imported `pub fn`'s **typed** signature
/// (the types half of the per-unit extern model). The driver supplies one
/// per extern, derived from the defining module's checked signature;
/// [`check_module`] builds the extern's `TypedFnSignature` from it. The
/// resolved signature (name + arity + `extern_origin`) comes via the
/// `ResolvedProgram`; this carries the param/return `Type`s resolve does
/// not have. (1/N is non-generic + primitive-signature; cross-module type
/// args are a (2/N) concern.)
#[derive(Debug, Clone, PartialEq)]
pub struct TypedImportedFn {
    /// The imported fn's name (matches the resolved extern signature).
    pub name: String,
    /// Parameter type *expressions*, in order — RE-RESOLVED in the
    /// importing unit's type space, so a cross-module type (e.g. `Point`)
    /// maps to the importer's local `StructId`/`EnumId` (the defining
    /// unit's id is meaningless here) and scalars resolve as usual. The
    /// importer must have imported any type a signature references.
    pub param_type_exprs: Vec<TypeExpr>,
    /// Return type expression (re-resolved in the importer).
    pub return_type_expr: TypeExpr,
    /// ADR 0037 (2/N): the imported fn's declared effect-row NAMES (empty
    /// for a pure fn). RE-RESOLVED to the importer's `EffectId`s in
    /// [`check_module`] — like the param/return TypeExprs — against the
    /// effect decls the driver inlined from the unit's `use`s, so a
    /// cross-UNIT effecting extern type-checks + lowers under the Kont ABI
    /// with the importer's local id. The build-wide op-id base map then
    /// keeps the runtime op id consistent with the performing unit.
    pub effect_row_names: Vec<String>,
}

/// Type-check a single-file [`ResolvedProgram`] — the `imports == []` case
/// of [`check_module`]. Single-file / Path-A builds use this and are
/// byte-identical to before the per-unit model.
pub fn check(program: &ResolvedProgram) -> Result<TypedProgram, TypeError> {
    check_module(program, &[])
}

/// Type-check a [`ResolvedProgram`] against its imported externs'
/// **typed** signatures (`imports`) — the per-unit type-check of ADR 0037
/// D5.1. Each imported extern (a resolved signature with `extern_origin`
/// set, no body) gets a `TypedFnSignature` built from its matching
/// `imports` entry, so cross-module calls type-check against the imported
/// signature; a single-file program passes `imports == []`.
///
/// Order:
///   0. Build struct table (name → StructId) from resolved structs.
///   1. Resolve every struct's field types into `TypedStructDecl`s
///      — UnknownType fires here for stale references.
///   2. Detect recursive structs and emit RecursiveStruct on cycle.
///   3. Resolve fn signatures' param + return types (struct names
///      now resolve cleanly against the struct table), plus the imported
///      externs' typed signatures.
///   4. Type-check each fn body (externs have no body).
pub fn check_module(
    program: &ResolvedProgram,
    imports: &[TypedImportedFn],
) -> Result<TypedProgram, TypeError> {
    // Pass 0: struct name table + type-param counts + class name
    // table. Both tables must exist before Pass 1 so field /
    // signature type-expressions can reference either.
    let struct_table: HashMap<String, StructId> =
        sentinel_resolve::struct_name_table(program);
    let class_table: HashMap<String, ClassId> = program
        .classes
        .iter()
        .map(|c| (c.name.clone(), c.id))
        .collect();
    // Phase D.1 / ADR 0032 (3/N): enum name table, so variant payload
    // types + fn-signature / let annotations can reference enum names.
    let enum_table: HashMap<String, EnumId> = program
        .enums
        .iter()
        .map(|e| (e.name.clone(), e.id))
        .collect();
    let struct_type_param_counts: HashMap<StructId, usize> = program
        .structs
        .iter()
        .map(|sd| (sd.id, sd.type_params.len()))
        .collect();
    let mut generic_instances: Vec<GenericInstanceData> = Vec::new();
    let mut refs: Vec<RefData> = Vec::new();
    let mut secrets: Vec<SecretData> = Vec::new();
    // ADR 0066 M1.2: the channel-type interner. M1.2 minimum interns a single
    // `Channel<i64>` while building the channel-builtin signatures below.
    let mut channels: Vec<ChannelData> = Vec::new();
    // ADR 0071 M1.4a: the `Shared<T>`-type interner. Pre-populated with the 6
    // word-scalar elements at fixed SharedIds 0..=5 while building the shared
    // builtin signatures below (mirrors `channels`).
    let mut shared: Vec<SharedData> = Vec::new();
    // ADR 0071 M1.4b: the `Mutex<T>`-type interner. Pre-populated with the 6
    // word-scalar elements at fixed MutexIds 0..=5 while building the mutex
    // builtin signatures below (mirrors `shared`).
    let mut mutexes: Vec<MutexData> = Vec::new();
    // ADR 0068: the nested-array element interner — populated when a `[[T]]` type
    // annotation / literal is resolved (threaded through resolve_type_expr +
    // the check pipeline, like `secrets`/`refs`).
    let mut arrays: Vec<ArrayElem> = Vec::new();

    // Pass 0.5 / ADR 0032 (3/N): resolve enum variant payload types.
    // Each payload `TypeExpr` is resolved against the name tables (so
    // a payload may be a primitive, struct, class, or another enum —
    // including this enum itself: directly-recursive enums are sound
    // because the layout is heap-boxed per ADR 0032 D4, so unlike
    // structs they need no nullable indirection). The EnumId is the
    // index in `typed_enums`.
    let mut typed_enums: Vec<EnumData> = Vec::with_capacity(program.enums.len());
    for ed in &program.enums {
        let mut variants = Vec::with_capacity(ed.variants.len());
        for v in &ed.variants {
            let mut payloads = Vec::with_capacity(v.payloads.len());
            for p in &v.payloads {
                payloads.push(resolve_type_expr(
                    p,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?);
            }
            variants.push(VariantData {
                name: v.name.clone(),
                name_span: v.name_span.clone(),
                payloads,
            });
        }
        typed_enums.push(EnumData {
            id: ed.id,
            name: ed.name.clone(),
            name_span: ed.name_span.clone(),
            variants,
        });
    }

    // Pass 1: resolve struct field types. C1.7.4b / ADR 0016 D2 /
    // D6: generic structs are now supported; their fields can
    // reference the struct's type-params (which appear as
    // `Type::TypeParam(_)` in the typed AST). C2 / ADR 0017 D7:
    // refs in field types are rejected (`RefInStructField`).
    let mut typed_structs: Vec<TypedStructDecl> =
        Vec::with_capacity(program.structs.len());
    for sd in &program.structs {
        let typed_type_params: Vec<TypedTypeParam> = sd
            .type_params
            .iter()
            .map(|tp| TypedTypeParam {
                id: tp.id,
                name: tp.name.clone(),
                name_span: tp.name_span.clone(),
            })
            .collect();
        // Build the per-struct type-param scope so field types can
        // mention `T`, `U`, etc.
        let mut tp_scope: TypeParamScope = HashMap::new();
        for tp in &sd.type_params {
            tp_scope.insert(tp.name.clone(), tp.id);
        }
        let mut fields = Vec::with_capacity(sd.fields.len());
        for f in &sd.fields {
            let ty = resolve_type_expr_with_scope(
                &f.ty,
                &struct_table,
                &class_table,
                &enum_table,
                &tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            // C2 / ADR 0017 D7 / D12: refs can't live in struct
            // fields at C2 — that's the first-class-refs case,
            // deferred until named regions land.
            if ty.is_ref() {
                return Err(TypeError::RefInStructField {
                    span: to_source_span(&f.ty.span),
                });
            }
            fields.push(TypedStructField {
                name: f.name.clone(),
                name_span: f.name_span.clone(),
                ty,
                span: f.span.clone(),
            });
        }
        typed_structs.push(TypedStructDecl {
            id: sd.id,
            name: sd.name.clone(),
            name_span: sd.name_span.clone(),
            type_params: typed_type_params,
            fields,
            span: sd.span.clone(),
        });
    }

    // Pass 2: cycle detection.
    detect_struct_cycle(&typed_structs)?;

    // C3 / ADR 0019 D4 (C3.2): type-check effect declarations.
    // Each op's param-type annotations + return-type annotation
    // resolve against the struct table. Effects can't be generic
    // at C3.2 (no `<T>` on `effect E`) so the type-param scope
    // is empty. EffectIds were assigned at resolve time; we
    // preserve them here as the ResolvedProgram::effects index.
    let mut typed_effect_decls: Vec<TypedEffectDecl> =
        Vec::with_capacity(program.effects.len());
    for ed in &program.effects {
        let empty_tp_scope: TypeParamScope = HashMap::new();
        let mut typed_ops = Vec::with_capacity(ed.ops.len());
        for op in &ed.ops {
            let mut typed_params = Vec::with_capacity(op.params.len());
            for p in &op.params {
                let ty = resolve_type_expr_with_scope(
                    &p.ty,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?;
                typed_params.push(TypedParam {
                    id: p.id,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty,
                });
            }
            // Return type defaults to `i64` if the source omitted
            // `-> RetT` per ADR 0019 D4. Future: revisit when
            // `unit` lands.
            let return_type = match &op.return_type {
                Some(rt) => resolve_type_expr_with_scope(
                    rt,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?,
                None => Type::I64,
            };
            typed_ops.push(TypedOpDecl {
                name: op.name.clone(),
                name_span: op.name_span.clone(),
                params: typed_params,
                return_type,
                span: op.span.clone(),
            });
        }
        typed_effect_decls.push(TypedEffectDecl {
            id: ed.id,
            name: ed.name.clone(),
            name_span: ed.name_span.clone(),
            ops: typed_ops,
            span: ed.span.clone(),
        });
    }

    // Pass 3: fn signatures.
    let mut typed_signatures: Vec<TypedFnSignature> =
        Vec::with_capacity(program.fn_signatures.len());

    // Builtins (FnId 0..3): print + the three C1.7-generic builtins
    // (unwrap_or, is_some, len). Their signatures are now expressed
    // with real `Type::TypeParam` references per ADR 0016 D8a so the
    // standard generic-call inference path handles them uniformly
    // with user-defined generic fns. Codegen retains its special-
    // case lowering per D8b (force-unwrap / discriminator-extract /
    // length-extract have no Sentinel-source bodies at C1.7).
    let print_sig = &program.fn_signatures[0];
    typed_signatures.push(TypedFnSignature {
        id: print_sig.id,
        name: print_sig.name.clone(),
        name_span: print_sig.name_span.clone(),
        type_params: vec![],
        param_types: vec![Type::I64],
        return_type: Type::I64,
        // C3.2(a): runtime builtins are treated as effect-free at
        // C3.2 minimum so existing programs keep type-checking.
        // A future ADR may promote `print` to carry `Io`.
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let unwrap_or_sig = &program.fn_signatures[1];
    typed_signatures.push(TypedFnSignature {
        id: unwrap_or_sig.id,
        name: unwrap_or_sig.name.clone(),
        name_span: unwrap_or_sig.name_span.clone(),
        // `unwrap_or<T>(x: ?T, default: T) -> T` per ADR 0014 D9
        // / ADR 0016 D8a.
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![
            Type::Nullable(NullableInner::TypeParam(TypeParamId(0))),
            Type::TypeParam(TypeParamId(0)),
        ],
        return_type: Type::TypeParam(TypeParamId(0)),
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let is_some_sig = &program.fn_signatures[2];
    typed_signatures.push(TypedFnSignature {
        id: is_some_sig.id,
        name: is_some_sig.name.clone(),
        name_span: is_some_sig.name_span.clone(),
        // `is_some<T>(x: ?T) -> bool` per ADR 0014 D9 / ADR 0016 D8a.
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![Type::Nullable(NullableInner::TypeParam(TypeParamId(0)))],
        return_type: Type::Bool,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let len_sig = &program.fn_signatures[3];
    typed_signatures.push(TypedFnSignature {
        id: len_sig.id,
        name: len_sig.name.clone(),
        name_span: len_sig.name_span.clone(),
        // `len<T>(a: [T]) -> i64` per ADR 0015 D4 / ADR 0016 D8a.
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![Type::Array(ArrayElem::TypeParam(TypeParamId(0)))],
        return_type: Type::I64,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // D.2 / ADR 0033 D5: the byte-string builtins — non-generic
    // concrete signatures (no type_params). Calls type-check at D.2
    // (3/N); codegen lowers them at (4/N).
    let str_eq_sig = &program.fn_signatures[4];
    typed_signatures.push(TypedFnSignature {
        id: str_eq_sig.id,
        name: str_eq_sig.name.clone(),
        name_span: str_eq_sig.name_span.clone(),
        // `str_eq(a: [u8], b: [u8]) -> bool`.
        type_params: vec![],
        param_types: vec![Type::Array(ArrayElem::U8), Type::Array(ArrayElem::U8)],
        return_type: Type::Bool,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let u8_to_i64_sig = &program.fn_signatures[5];
    typed_signatures.push(TypedFnSignature {
        id: u8_to_i64_sig.id,
        name: u8_to_i64_sig.name.clone(),
        name_span: u8_to_i64_sig.name_span.clone(),
        // `u8_to_i64(b: u8) -> i64` (zero-extend).
        type_params: vec![],
        param_types: vec![Type::U8],
        return_type: Type::I64,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let i64_to_u8_sig = &program.fn_signatures[6];
    typed_signatures.push(TypedFnSignature {
        id: i64_to_u8_sig.id,
        name: i64_to_u8_sig.name.clone(),
        name_span: i64_to_u8_sig.name_span.clone(),
        // `i64_to_u8(n: i64) -> u8` (truncate).
        type_params: vec![],
        param_types: vec![Type::I64],
        return_type: Type::U8,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // D.3 / ADR 0034 D5: the growable-collection builtins. Generic over
    // the element T; typed through the uniform generic-call inference
    // path (no special-casing in `check_call`, unlike `len`):
    //   - `vec_new<T>() -> Vec<T>` — T is inferred by seeding the subst
    //     from the expected return type (the binding annotation), the
    //     same bidirectional pushdown that types `null` / empty arrays.
    //   - `push<T>(v: &mut Vec<T>, x: T) -> i64` — T is bound by
    //     unifying the `&mut Vec<T>` param against the `&mut Vec<i64>`
    //     argument (the new `(Vec, Vec)` arm in `unify_one` recurses to
    //     the element). The `&mut Vec<T>` is an interned mutable Ref.
    let vec_new_sig = &program.fn_signatures[7];
    typed_signatures.push(TypedFnSignature {
        id: vec_new_sig.id,
        name: vec_new_sig.name.clone(),
        name_span: vec_new_sig.name_span.clone(),
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![],
        return_type: Type::Vec(VecElem::TypeParam(TypeParamId(0))),
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let push_sig = &program.fn_signatures[8];
    let push_mut_vec_ref =
        Type::Ref(intern_ref(&mut refs, true, Type::Vec(VecElem::TypeParam(TypeParamId(0)))));
    typed_signatures.push(TypedFnSignature {
        id: push_sig.id,
        name: push_sig.name.clone(),
        name_span: push_sig.name_span.clone(),
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![push_mut_vec_ref, Type::TypeParam(TypeParamId(0))],
        return_type: Type::I64,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // D.3 (2/N) / ADR 0034 D5: `pop<T>(&mut Vec<T>) -> T` and
    // `vec_to_array<T>(Vec<T>) -> [T]`. Both flow the uniform generic
    // path; `pop`'s `&mut Vec<T>` is an interned mutable Ref like push's.
    let pop_sig = &program.fn_signatures[9];
    let pop_mut_vec_ref =
        Type::Ref(intern_ref(&mut refs, true, Type::Vec(VecElem::TypeParam(TypeParamId(0)))));
    typed_signatures.push(TypedFnSignature {
        id: pop_sig.id,
        name: pop_sig.name.clone(),
        name_span: pop_sig.name_span.clone(),
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![pop_mut_vec_ref],
        return_type: Type::TypeParam(TypeParamId(0)),
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let vec_to_array_sig = &program.fn_signatures[10];
    typed_signatures.push(TypedFnSignature {
        id: vec_to_array_sig.id,
        name: vec_to_array_sig.name.clone(),
        name_span: vec_to_array_sig.name_span.clone(),
        type_params: vec![builtin_type_param("T", 0)],
        param_types: vec![Type::Vec(VecElem::TypeParam(TypeParamId(0)))],
        return_type: Type::Array(ArrayElem::TypeParam(TypeParamId(0))),
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // D.4 / ADR 0035 D4/D7: the file-I/O builtins — non-generic concrete
    // `[u8]` signatures (no type_params), like `str_eq`. Paths + content
    // are byte arrays. `read_file([u8]) -> [u8]`, `write_file([u8], [u8])
    // -> i64`. Codegen lowers them to `sentinel_read_file` /
    // `sentinel_write_file` (D.4 (1/N)); they panic on I/O failure.
    let read_file_sig = &program.fn_signatures[11];
    typed_signatures.push(TypedFnSignature {
        id: read_file_sig.id,
        name: read_file_sig.name.clone(),
        name_span: read_file_sig.name_span.clone(),
        type_params: vec![],
        param_types: vec![Type::Array(ArrayElem::U8)],
        return_type: Type::Array(ArrayElem::U8),
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    let write_file_sig = &program.fn_signatures[12];
    typed_signatures.push(TypedFnSignature {
        id: write_file_sig.id,
        name: write_file_sig.name.clone(),
        name_span: write_file_sig.name_span.clone(),
        type_params: vec![],
        param_types: vec![Type::Array(ArrayElem::U8), Type::Array(ArrayElem::U8)],
        return_type: Type::I64,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // D.4 (2/N) / ADR 0035 D4: `print_bytes([u8]) -> i64` — write a byte
    // array to stdout (the byte/string companion to `print`).
    let print_bytes_sig = &program.fn_signatures[13];
    typed_signatures.push(TypedFnSignature {
        id: print_bytes_sig.id,
        name: print_bytes_sig.name.clone(),
        name_span: print_bytes_sig.name_span.clone(),
        type_params: vec![],
        param_types: vec![Type::Array(ArrayElem::U8)],
        return_type: Type::I64,
        effect_row: vec![],
        is_main: false,
        is_runtime: true,
        extern_origin: None,
    });
    // ADR 0056: the TCP sockets builtins (FnId 14..=20). Concrete signatures —
    // handles / ports / counts are `i64`, byte buffers are `[u8]`. Codegen lowers
    // them to the `sentinel_tcp_*` runtime symbols.
    {
        let socket_sigs: &[(usize, &[Type], Type)] = &[
            (14, &[Type::I64], Type::I64),                                      // tcp_listen
            (15, &[Type::I64], Type::I64),                                      // tcp_local_port
            (16, &[Type::I64], Type::I64),                                      // tcp_accept
            (17, &[Type::Array(ArrayElem::U8), Type::I64], Type::I64),          // tcp_connect
            (18, &[Type::I64, Type::I64], Type::Array(ArrayElem::U8)),          // tcp_read
            (19, &[Type::I64, Type::Array(ArrayElem::U8)], Type::I64),          // tcp_write
            (20, &[Type::I64], Type::I64),                                      // tcp_close
        ];
        for (idx, params, ret) in socket_sigs {
            let sig = &program.fn_signatures[*idx];
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                type_params: vec![],
                param_types: params.to_vec(),
                return_type: *ret,
                effect_row: vec![],
                is_main: false,
                is_runtime: true,
                extern_origin: None,
            });
        }
    }
    // ADR 0066 M1.2: the channel builtins (FnId 21..=24). At M1.2 minimum the
    // element type is `i64`, so `Channel<i64>` is interned ONCE here and used
    // directly in concrete signatures (no generic-builtin path). `send` /
    // `channel_close` return an `i64` status (like the socket builtins); `recv`
    // returns `?i64` (`null` = closed+drained, ADR 0066 D4). Codegen lowers
    // each to its `sentinel_channel_*` runtime symbol.
    {
        // ADR 0066 M1.2b-cont: pre-intern the word-scalar channel element types at
        // FIXED ChanIds 0..=5 (i64, i32, u8, bool, f64, ptr — matching
        // `channel_chanid_for`), so a generic `Channel<T>` annotation resolves to a
        // stable ChanId without threading the `channels` interner. The concrete sigs
        // below stay `Channel<i64>` (the default); `check_call` special-cases the 4
        // builtins for the generic element. The extra ChanIds are snc-internal (the
        // i64-only differential corpus never references them — byte-identical).
        let chan_i64 = Type::Channel(intern_channel(&mut channels, Type::I64));
        intern_channel(&mut channels, Type::I32);
        intern_channel(&mut channels, Type::U8);
        intern_channel(&mut channels, Type::Bool);
        intern_channel(&mut channels, Type::F64);
        intern_channel(&mut channels, Type::Ptr);
        let opt_i64 = Type::Nullable(NullableInner::I64);
        let channel_sigs: &[(usize, &[Type], Type)] = &[
            (21, &[], chan_i64),                       // channel_new() -> Channel<i64>
            (22, &[chan_i64, Type::I64], Type::I64),   // send(Channel<i64>, i64) -> i64
            (23, &[chan_i64], opt_i64),                // recv(Channel<i64>) -> ?i64
            (24, &[chan_i64], Type::I64),              // channel_close(Channel<i64>) -> i64
        ];
        for (idx, params, ret) in channel_sigs {
            let sig = &program.fn_signatures[*idx];
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                type_params: vec![],
                param_types: params.to_vec(),
                return_type: *ret,
                effect_row: vec![],
                is_main: false,
                is_runtime: true,
                extern_origin: None,
            });
        }
    }
    // ADR 0066 M2.1 / M2.2: the subprocess builtins (FnId 25..=28). `process_spawn`
    // takes a `[u8]` path + a `[[u8]]` args list (the nested-array type, ADR 0068)
    // and returns a `Process` handle; `process_wait` takes the handle → the i64 exit
    // code; `process_write(p, [u8]) -> i64` byte-writes to the child's stdin;
    // `process_read(p) -> [u8]` reads the child's stdout (M2.2). The `secret` fence
    // is implicit: every payload param is public `[u8]` / `[[u8]]`, so a `secret`
    // value can't reach them (a type mismatch). Codegen lowers each to its
    // `sentinel_process_*` runtime symbol.
    {
        let bytes_ty = Type::Array(ArrayElem::U8);
        let argv_ty = Type::Array(ArrayElem::Array(intern_array_elem(&mut arrays, ArrayElem::U8)));
        // ADR 0066 M2.1 / D7: `process_spawn` carries the built-in `Subprocess`
        // capability effect (auto-registered in resolve, after `Async`); it
        // propagates to callers + bubbles to `main`. `process_wait` / `_write` /
        // `_read` are effect-free (they operate on an already-acquired handle —
        // spawning is the capability-acquiring op).
        let subprocess_eid: EffectId = EffectId(
            typed_effect_decls
                .iter()
                .position(|d| d.name == "Subprocess")
                .expect("Subprocess auto-registered in resolve") as u32,
        );
        // ADR 0066 M2.3: `process_send(p, v: i64) -> i64` (frame an i64 to the
        // child's stdin) + `process_recv(p) -> ?i64` (read one i64 frame; `null` =
        // closed/EOF — the cross-process twin of `recv`). The element is the public
        // `i64`, so the cross-process secret fence is structural (a `secret i64`
        // arg to `process_send` is a type mismatch); all effect-free.
        let opt_i64 = Type::Nullable(NullableInner::I64);
        // ADR 0066 M2.4a / ADR 0069: the `SealedChannel` bridge builtins —
        // `sealed_channel(p: Process) -> SealedChannel` re-types the pipe as the
        // encrypted endpoint (the fence-as-type, D1/D9); `sealed_process(sc) ->
        // Process` recovers it for the stdlib seal/open framing. Both effect-free
        // (no I/O; they re-type an already-acquired handle) and identity in codegen.
        let process_sigs: &[(usize, &[Type], Type, &[EffectId])] = &[
            (25, &[bytes_ty, argv_ty], Type::Process, &[subprocess_eid]), // process_spawn
            (26, &[Type::Process], Type::I64, &[]),                       // process_wait
            (27, &[Type::Process, bytes_ty], Type::I64, &[]),             // process_write
            (28, &[Type::Process], bytes_ty, &[]),                        // process_read
            (29, &[Type::Process, Type::I64], Type::I64, &[]),            // process_send
            (30, &[Type::Process], opt_i64, &[]),                        // process_recv
            (31, &[Type::Process], Type::SealedChannel, &[]),            // sealed_channel
            (32, &[Type::SealedChannel], Type::Process, &[]),            // sealed_process
            // ADR 0066 M2.4b: the child-side self-stdin/stdout framed builtins —
            // `stdin_recv() -> ?i64` (read one i64 frame from own stdin) +
            // `stdout_send(v: i64) -> i64` (frame an i64 to own stdout). The element
            // is the public `i64` (the structural cross-process fence). Effect-free
            // (they operate on the already-acquired own stdio).
            (33, &[], opt_i64, &[]),                                     // stdin_recv
            (34, &[Type::I64], Type::I64, &[]),                          // stdout_send
            // ADR 0066 M2.4 follow-on: own command-line argument reflection.
            // `arg_count() -> i64`; `arg(i: i64) -> [u8]`. Effect-free.
            (35, &[], Type::I64, &[]),                                   // arg_count
            (36, &[Type::I64], bytes_ty, &[]),                           // arg
        ];
        for (idx, params, ret, eff) in process_sigs {
            let sig = &program.fn_signatures[*idx];
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                type_params: vec![],
                param_types: params.to_vec(),
                return_type: *ret,
                effect_row: eff.to_vec(),
                is_main: false,
                is_runtime: true,
                extern_origin: None,
            });
        }
    }
    // ADR 0070 (generalized): `apply(f: Fn<T,R>, x: T) -> R` (FnId 37) — the
    // indirect-call builtin for non-capturing function values, context-typed
    // from `f`'s own `Fn<T,R>` signature (special-cased in `check_call`,
    // the `process_recv`/`recv` pattern). The registered param_types below
    // are a PLACEHOLDER (arity=2 only — `check_call` intercepts `id ==
    // APPLY_FN_ID` before any generic param-type comparison, exactly like
    // `channel_new`/`send`/`recv`'s registered `Channel<i64>` default).
    {
        let apply_sig = &program.fn_signatures[37];
        typed_signatures.push(TypedFnSignature {
            id: apply_sig.id,
            name: apply_sig.name.clone(),
            name_span: apply_sig.name_span.clone(),
            type_params: vec![],
            param_types: vec![Type::Fn(FnValueSigId(0)), Type::I64],
            return_type: Type::I64,
            effect_row: vec![],
            is_main: false,
            is_runtime: true,
            extern_origin: None,
        });
    }
    // ADR 0071 M1.4a: the `Shared<T>` refcounted-handle builtins — `shared_new(v:
    // T) -> Shared<T>` (FnId 38) and `shared_get(s: Shared<T>) -> T` (FnId 39).
    // Pre-intern the 6 word-scalar element types at FIXED SharedIds 0..=5 (i64,
    // i32, u8, bool, f64, ptr — matching `shared_id_for`), so a `Shared<T>`
    // annotation / `shared_new(v)` result resolves to a stable SharedId without
    // threading the `shared` interner. The concrete sigs below stay `Shared<i64>`
    // (the default); `check_call` special-cases both builtins for the generic
    // element (the `send`/`recv` pattern — `shared_new`'s element from its arg,
    // `shared_get`'s from the handle's SharedId). The extra SharedIds are
    // snc-internal (the i64-only differential corpus never references them —
    // byte-identical). `shared_new` returns the handle; codegen calls
    // `sentinel_shared_new`; the handle is Copy + (at this slice) leaked
    // (`needs_drop == false`), the refcount accounting being slice 3.
    {
        let shared_i64 = Type::Shared(intern_shared(&mut shared, Type::I64));
        intern_shared(&mut shared, Type::I32);
        intern_shared(&mut shared, Type::U8);
        intern_shared(&mut shared, Type::Bool);
        intern_shared(&mut shared, Type::F64);
        intern_shared(&mut shared, Type::Ptr);
        let shared_sigs: &[(usize, &[Type], Type)] = &[
            (38, &[Type::I64], shared_i64),   // shared_new(i64) -> Shared<i64>
            (39, &[shared_i64], Type::I64),   // shared_get(Shared<i64>) -> i64
        ];
        for (idx, params, ret) in shared_sigs {
            let sig = &program.fn_signatures[*idx];
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                type_params: vec![],
                param_types: params.to_vec(),
                return_type: *ret,
                effect_row: vec![],
                is_main: false,
                is_runtime: true,
                extern_origin: None,
            });
        }
    }
    // ADR 0071 M1.4b: the `Mutex<T>` builtins — `mutex_new(v: T) -> Mutex<T>`
    // (FnId 40) and `lock(m: Mutex<T>) -> ?Guard` (FnId 41). Pre-intern the 6
    // word-scalar element types at FIXED MutexIds 0..=5 (matching `mutex_id_for`),
    // so a `Mutex<T>` annotation / `mutex_new(v)` result resolves to a stable
    // MutexId without threading the `mutexes` interner (mirrors `shared`). At THIS
    // slice (2b-i) `mutex_new` gets its real `Mutex<i64>` return + a `check_call`
    // special-case; `lock`'s return stays a PLACEHOLDER (`i64`) until slice 2b-ii
    // adds `Type::Guard` + `?Guard` — no fixture calls `lock` yet, so it is inert.
    {
        let mutex_i64 = Type::Mutex(intern_mutex(&mut mutexes, Type::I64));
        intern_mutex(&mut mutexes, Type::I32);
        intern_mutex(&mut mutexes, Type::U8);
        intern_mutex(&mut mutexes, Type::Bool);
        intern_mutex(&mut mutexes, Type::F64);
        intern_mutex(&mut mutexes, Type::Ptr);
        let mutex_sigs: &[(usize, &[Type], Type)] = &[
            (40, &[Type::I64], mutex_i64), // mutex_new(i64) -> Mutex<i64>
            (41, &[mutex_i64], Type::I64), // lock (placeholder — real ?Guard is 2b-ii)
        ];
        for (idx, params, ret) in mutex_sigs {
            let sig = &program.fn_signatures[*idx];
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                type_params: vec![],
                param_types: params.to_vec(),
                return_type: *ret,
                effect_row: vec![],
                is_main: false,
                is_runtime: true,
                extern_origin: None,
            });
        }
    }

    for fn_def in &program.fns {
        let resolved_sig = &program.fn_signatures[fn_def.id.0 as usize];
        // C1.7 / ADR 0016 D11: reject `fn main<T>(...)` early.
        if resolved_sig.is_main && !fn_def.type_params.is_empty() {
            return Err(TypeError::GenericMain {
                span: to_source_span(&fn_def.name_span),
            });
        }
        // Build the per-fn type-param scope so subsequent
        // resolve_type_expr_with_scope calls see them.
        let mut tp_scope: TypeParamScope = HashMap::new();
        for tp in &fn_def.type_params {
            tp_scope.insert(tp.name.clone(), tp.id);
        }
        let typed_type_params: Vec<TypedTypeParam> = fn_def
            .type_params
            .iter()
            .map(|tp| TypedTypeParam {
                id: tp.id,
                name: tp.name.clone(),
                name_span: tp.name_span.clone(),
            })
            .collect();
        let mut param_types = Vec::with_capacity(fn_def.params.len());
        for param in &fn_def.params {
            param_types.push(resolve_type_expr_with_scope(
                &param.ty,
                &struct_table,
                &class_table,
                &enum_table,
                &tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?);
        }
        let return_type = resolve_type_expr_with_scope(
            &fn_def.return_type,
            &struct_table,
            &class_table,
            &enum_table,
            &tp_scope,
            &mut generic_instances,
            &mut refs,
            &mut secrets,
            &mut arrays,
            &struct_type_param_counts,
        )?;
        // C3 / ADR 0019 D1 (C3.2): sorted + dedup the effect-row
        // entries so set-equality checks against the inferred row
        // at C3.2(b)'s effect_check_query are O(n). Resolve has
        // already validated that each effect name is declared.
        let mut effect_row: Vec<EffectId> = fn_def.effect_row.clone();
        effect_row.sort_by_key(|e| e.0);
        effect_row.dedup();
        typed_signatures.push(TypedFnSignature {
            id: fn_def.id,
            name: resolved_sig.name.clone(),
            name_span: resolved_sig.name_span.clone(),
            type_params: typed_type_params,
            param_types,
            return_type,
            effect_row,
            is_main: resolved_sig.is_main,
            is_runtime: resolved_sig.is_runtime,
            extern_origin: resolved_sig.extern_origin.clone(),
        });
    }

    // Phase D.6 / ADR 0037 D5.1: build a TypedFnSignature for each imported
    // EXTERN (a resolved signature with `extern_origin` set, no body). The
    // extern's param/return type EXPRESSIONS (from the matching
    // `typed_imports` entry) are RE-RESOLVED here in THIS unit's type space —
    // `struct_table`/`enum_table` include the imported types (inlined by the
    // driver), so a cross-module `Point` maps to the importer's local
    // `StructId`, not the defining unit's. `imports == []` (single-file /
    // Path-A) → no externs → byte-identical. The `sort_by_key` below places
    // each by its FnId regardless of push order.
    if !imports.is_empty() {
        let imports_by_name: HashMap<&str, &TypedImportedFn> =
            imports.iter().map(|i| (i.name.as_str(), i)).collect();
        // ADR 0037 (2/N): effect NAME → THIS unit's local EffectId (the
        // resolved effect decls — own + the driver-inlined imports — carry
        // their id). Lets an effecting extern's row re-resolve like its
        // param/return TypeExprs do (a cross-UNIT effect's local id differs
        // per unit; the build-wide op-id base map reconciles the runtime id).
        let effect_id_by_name: HashMap<&str, EffectId> =
            program.effects.iter().map(|ed| (ed.name.as_str(), ed.id)).collect();
        // Externs are non-generic in this slice → empty type-param scope.
        let extern_scope: TypeParamScope = HashMap::new();
        for sig in &program.fn_signatures {
            if sig.extern_origin.is_none() {
                continue;
            }
            let imp = imports_by_name.get(sig.name.as_str()).expect(
                "ADR 0037 D5.1: every extern fn_signature has a matching typed import",
            );
            let mut param_types = Vec::with_capacity(imp.param_type_exprs.len());
            for te in &imp.param_type_exprs {
                param_types.push(resolve_type_expr_with_scope(
                    te,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &extern_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?);
            }
            let return_type = resolve_type_expr_with_scope(
                &imp.return_type_expr,
                &struct_table,
                &class_table,
                &enum_table,
                &extern_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            // ADR 0037 (2/N): re-resolve the extern's effect-row NAMES to
            // THIS unit's EffectIds (the effect analogue of the param/return
            // TypeExpr re-resolution above). A name not in scope means the
            // importer didn't `use` the effect → UnknownImportedEffect.
            let mut effect_row: Vec<EffectId> = Vec::with_capacity(imp.effect_row_names.len());
            for name in &imp.effect_row_names {
                match effect_id_by_name.get(name.as_str()) {
                    Some(eid) => effect_row.push(*eid),
                    None => {
                        return Err(TypeError::UnknownImportedEffect {
                            fn_name: imp.name.clone(),
                            effect_name: name.clone(),
                        });
                    }
                }
            }
            effect_row.sort_by_key(|e| e.0);
            effect_row.dedup();
            typed_signatures.push(TypedFnSignature {
                id: sig.id,
                name: sig.name.clone(),
                name_span: None,
                type_params: vec![],
                param_types,
                return_type,
                effect_row,
                is_main: false,
                is_runtime: false,
                extern_origin: sig.extern_origin.clone(),
            });
        }
    }

    // ADR 0057: build a TypedFnSignature for each `extern "C"` declaration.
    // The param/return type-exprs are resolved in THIS unit's type space and
    // restricted to the FFI-safe public set (`i64` / `f64`) — a `secret`,
    // struct, array, or other-width type is rejected (the secret-fence + the
    // Phase-1 ABI). The FnId is recorded in `typed_externs` so codegen
    // declares it `External` under its bare C symbol. No effect row (an FFI
    // leaf), no generics. `program.externs == []` → byte-identical.
    let extern_scope: TypeParamScope = HashMap::new();
    let mut typed_externs: Vec<FnId> = Vec::with_capacity(program.externs.len());
    for ext in &program.externs {
        let mut param_types = Vec::with_capacity(ext.param_types.len());
        for te in &ext.param_types {
            let ty = resolve_type_expr_with_scope(
                te,
                &struct_table,
                &class_table,
                &enum_table,
                &extern_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            if !is_ffi_safe(ty) {
                return Err(TypeError::ExternFfiType {
                    span: to_source_span(&te.span),
                });
            }
            param_types.push(ty);
        }
        let return_type = resolve_type_expr_with_scope(
            &ext.return_type,
            &struct_table,
            &class_table,
            &enum_table,
            &extern_scope,
            &mut generic_instances,
            &mut refs,
            &mut secrets,
            &mut arrays,
            &struct_type_param_counts,
        )?;
        if !is_ffi_safe(return_type) {
            return Err(TypeError::ExternFfiType {
                span: to_source_span(&ext.return_type.span),
            });
        }
        typed_externs.push(ext.id);
        typed_signatures.push(TypedFnSignature {
            id: ext.id,
            name: ext.name.clone(),
            name_span: Some(ext.name_span.clone()),
            type_params: vec![],
            param_types,
            return_type,
            effect_row: vec![],
            is_main: false,
            is_runtime: false,
            extern_origin: None,
        });
    }

    // ADR 0059: validate each `export "C"` fn's signature. An export is a
    // normal fn (its body was type-checked above); additionally its params +
    // return must be FFI-safe public scalars (`i64`/`f64`) and NON-secret — the
    // same fence as an `extern` import, since the C caller is outside the
    // verified constant-time region (a secret crosses the boundary only via an
    // explicit `declassify` inside the export). The FnIds flow to
    // `TypedProgram.exports` for codegen + the header generator.
    for id in &program.exports {
        let sig = typed_signatures
            .iter()
            .find(|s| s.id == *id)
            .expect("ADR 0059: every export FnId has a TypedFnSignature");
        for (idx, pty) in sig.param_types.iter().enumerate() {
            // ADR 0059 Phase 1b: a param is the value ABI (`i64`/`f64`) OR a
            // `&[u8]` byte slice (presented to C as `(ptr, len)`).
            if !is_ffi_safe(*pty) && !is_byte_slice_ref(*pty, &refs) {
                let span = program
                    .fns
                    .iter()
                    .find(|f| f.id == *id)
                    .map(|f| to_source_span(&f.params[idx].span))
                    .unwrap_or_else(|| to_source_span(&program.span));
                return Err(TypeError::ExternFfiType { span });
            }
        }
        // ADR 0059 Phase 1b (A7): the return is the value ABI (`i64`/`f64`) OR
        // an owned PUBLIC `[u8]` (presented to C via the `(uint8_t** out_data,
        // int64_t* out_len)` out-params + `sentinel_free_bytes`). A `[secret u8]`
        // return is still fenced (not `is_owned_byte_array`).
        if !is_ffi_safe(sig.return_type) && !is_owned_byte_array(sig.return_type) {
            let span = program
                .fns
                .iter()
                .find(|f| f.id == *id)
                .map(|f| to_source_span(&f.return_type.span))
                .unwrap_or_else(|| to_source_span(&program.span));
            return Err(TypeError::ExternFfiType { span });
        }
    }

    // Sort typed_signatures by id so signatures[i] corresponds to
    // FnId(i) — matches ResolvedProgram's invariant.
    typed_signatures.sort_by_key(|s| s.id.0);

    // C4.1 / ADR 0022 D1 (Pass 3.5): class signatures. Build the
    // typed_class_decls vec with each class's resolved field
    // types + init param types + method signatures. Bodies are
    // populated in Pass 5 — at this point each init/method has a
    // stub body so fn-body type-checking in Pass 4 can look up
    // method signatures via `Type::Class(ClassId)` receivers.
    let mut typed_class_decls: Vec<ClassData> =
        Vec::with_capacity(program.classes.len());
    for cd in &program.classes {
        let empty_tp_scope: TypeParamScope = HashMap::new();
        // Field types (no generics on classes at C4.1).
        let mut fields = Vec::with_capacity(cd.fields.len());
        for f in &cd.fields {
            let ty = resolve_type_expr_with_scope(
                &f.ty,
                &struct_table,
                &class_table,
                &enum_table,
                &empty_tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            fields.push(TypedClassField {
                visibility: f.visibility,
                name: f.name.clone(),
                name_span: f.name_span.clone(),
                ty,
                span: f.span.clone(),
            });
        }
        // Init params (no body yet).
        let init = if let Some(init_def) = &cd.init {
            let mut params = Vec::with_capacity(init_def.params.len());
            for p in &init_def.params {
                let ty = resolve_type_expr_with_scope(
                    &p.ty,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?;
                params.push(TypedParam {
                    id: p.id,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty,
                });
            }
            Some(TypedInitDef {
                visibility: init_def.visibility,
                self_var_id: init_def.self_var_id,
                params,
                body: stub_block(init_def.body.span.clone()),
                span: init_def.span.clone(),
            })
        } else {
            None
        };
        // Method signatures (no bodies yet).
        let mut methods = Vec::with_capacity(cd.methods.len());
        for m in &cd.methods {
            let mut params = Vec::with_capacity(m.params.len());
            for p in &m.params {
                let ty = resolve_type_expr_with_scope(
                    &p.ty,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?;
                params.push(TypedParam {
                    id: p.id,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty,
                });
            }
            let return_type = resolve_type_expr_with_scope(
                &m.return_type,
                &struct_table,
                &class_table,
                &enum_table,
                &empty_tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            let mut effect_row: Vec<EffectId> = m.effect_row.clone();
            effect_row.sort_by_key(|e| e.0);
            effect_row.dedup();
            methods.push(TypedMethodDef {
                visibility: m.visibility,
                name: m.name.clone(),
                name_span: m.name_span.clone(),
                self_kind: m.self_kind,
                self_var_id: m.self_var_id,
                params,
                return_type,
                effect_row,
                body: stub_block(m.body.span.clone()),
                span: m.span.clone(),
            });
        }
        typed_class_decls.push(ClassData {
            id: cd.id,
            name: cd.name.clone(),
            name_span: cd.name_span.clone(),
            fields,
            init,
            methods,
            span: cd.span.clone(),
        });
    }

    // C4.2 / ADR 0023 D8 Pass 3c: type-check trait method
    // signatures. Each method's params + return type resolve
    // against the standard (struct + class) lookup with `self`
    // captured positionally per ADR 0022 A2. `Self` in general
    // type position is deferred at C4.2 minimum (it carries
    // through C4.1's amendment); only positional `self: &Self` /
    // `self: &mut Self` via `self_kind` works.
    let mut typed_trait_decls: Vec<TraitData> = Vec::with_capacity(program.traits.len());
    for td in &program.traits {
        let empty_tp_scope: TypeParamScope = HashMap::new();
        let mut typed_methods: Vec<TypedTraitMethodSig> =
            Vec::with_capacity(td.methods.len());
        for m in &td.methods {
            let mut params: Vec<TypedParam> = Vec::with_capacity(m.params.len());
            for p in &m.params {
                let ty = resolve_type_expr_with_scope(
                    &p.ty,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?;
                // Trait method sigs have no VarIds for their
                // params (no body). Use VarId(u32::MAX) as a
                // sentinel — the typed AST exposes the param
                // shape but doesn't bind anything env-wise.
                params.push(TypedParam {
                    id: VarId(u32::MAX),
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty,
                });
            }
            let return_type = resolve_type_expr_with_scope(
                &m.return_type,
                &struct_table,
                &class_table,
                &enum_table,
                &empty_tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            let mut effect_row: Vec<EffectId> = m.effect_row.clone();
            effect_row.sort_by_key(|e| e.0);
            effect_row.dedup();
            typed_methods.push(TypedTraitMethodSig {
                name: m.name.clone(),
                name_span: m.name_span.clone(),
                self_kind: m.self_kind,
                params,
                return_type,
                effect_row,
                span: m.span.clone(),
            });
        }
        typed_trait_decls.push(TraitData {
            id: td.id,
            name: td.name.clone(),
            name_span: td.name_span.clone(),
            methods: typed_methods,
            span: td.span.clone(),
        });
    }

    // C4.2 / ADR 0023 D8 Pass 3d: type-check impl signatures +
    // verify completeness + signature match against the trait.
    // Bodies use stub blocks for now; Pass 6 below overwrites
    // them. The signature-match check is conservative: param
    // count, per-param type equality, return type, effect-row,
    // self_kind. `Self` in trait sigs collapses to the impl
    // target's concrete type — at C4.2 minimum the sigs don't
    // reference TraitSelf in params/returns (only positional via
    // `self_kind`), so the check is direct equality.
    let mut typed_impl_decls: Vec<ImplData> = Vec::with_capacity(program.impls.len());
    for imp in &program.impls {
        let empty_tp_scope: TypeParamScope = HashMap::new();
        let trait_data = &typed_trait_decls[imp.trait_id.0 as usize];
        let mut sigs_by_name: HashMap<String, TypedImplMethodDef> = HashMap::new();
        for m in &imp.methods {
            let mut params: Vec<TypedParam> = Vec::with_capacity(m.params.len());
            for p in &m.params {
                let ty = resolve_type_expr_with_scope(
                    &p.ty,
                    &struct_table,
                    &class_table,
                    &enum_table,
                    &empty_tp_scope,
                    &mut generic_instances,
                    &mut refs,
                    &mut secrets,
                    &mut arrays,
                    &struct_type_param_counts,
                )?;
                params.push(TypedParam {
                    id: p.id,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty,
                });
            }
            let return_type = resolve_type_expr_with_scope(
                &m.return_type,
                &struct_table,
                &class_table,
                &enum_table,
                &empty_tp_scope,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
            )?;
            let mut effect_row: Vec<EffectId> = m.effect_row.clone();
            effect_row.sort_by_key(|e| e.0);
            effect_row.dedup();
            sigs_by_name.insert(
                m.name.clone(),
                TypedImplMethodDef {
                    visibility: m.visibility,
                    name: m.name.clone(),
                    name_span: m.name_span.clone(),
                    self_kind: m.self_kind,
                    self_var_id: m.self_var_id,
                    params,
                    return_type,
                    effect_row,
                    body: stub_block(m.body.span.clone()),
                    span: m.span.clone(),
                },
            );
        }
        // Completeness: every trait method must be supplied.
        for tm in &trait_data.methods {
            if !sigs_by_name.contains_key(&tm.name) {
                return Err(TypeError::ImplMissingMethod {
                    trait_name: imp.trait_name.clone(),
                    type_name: imp.type_name.clone(),
                    method_name: tm.name.clone(),
                    span: to_source_span(&imp.span),
                });
            }
        }
        // Per-method signature match (D8).
        let mut ordered_methods: Vec<TypedImplMethodDef> =
            Vec::with_capacity(trait_data.methods.len());
        for tm in &trait_data.methods {
            let im = sigs_by_name.remove(&tm.name).expect("checked above");
            if im.self_kind != tm.self_kind
                || im.params.len() != tm.params.len()
                || im.return_type != tm.return_type
                || im.effect_row != tm.effect_row
                || im
                    .params
                    .iter()
                    .zip(tm.params.iter())
                    .any(|(a, b)| a.ty != b.ty)
            {
                return Err(TypeError::ImplMethodSignatureMismatch {
                    trait_name: imp.trait_name.clone(),
                    type_name: imp.type_name.clone(),
                    method_name: tm.name.clone(),
                    span: to_source_span(&im.name_span),
                });
            }
            ordered_methods.push(im);
        }
        typed_impl_decls.push(ImplData {
            id: imp.id,
            name: imp.name.clone(),
            trait_id: imp.trait_id,
            target: imp.target,
            trait_name: imp.trait_name.clone(),
            type_name: imp.type_name.clone(),
            methods: ordered_methods,
            span: imp.span.clone(),
        });
    }

    // Pass 4: type-check each fn body. Any `Pair<i64, bool>` /
    // similar new instances that show up here (e.g., from let
    // annotations or generic call sites) are interned into
    // `generic_instances`. Same for `&T` / `&mut T` refs into
    // `refs` per ADR 0017 D11.
    // C3.4 / ADR 0020 D5: per-handle-arm continuation interner.
    // Starts empty; populated whenever check_expr enters a
    // `handle ... with { ... }` and interns a kont per arm.
    let mut konts: Vec<KontData> = Vec::new();
    let mut tasks: Vec<TaskData> = Vec::new();

    let mut typed_fns = Vec::with_capacity(program.fns.len());
    for fn_def in &program.fns {
        typed_fns.push(check_fn(
            fn_def,
            &typed_signatures,
            &typed_structs,
            &typed_class_decls,
            &typed_enums,
            &mut generic_instances,
            &mut refs,
            &mut secrets,
            &mut arrays,
            &struct_type_param_counts,
            &typed_effect_decls,
            &typed_trait_decls,
            &typed_impl_decls,
            &mut konts,
            &mut tasks,
        )?);
    }

    // Pass 5: type-check each class body (init + methods),
    // overwriting the stub bodies populated in Pass 3.5.
    for cd in &program.classes {
        let idx = cd.id.0 as usize;
        // Init body.
        let init = if let Some(init_def) = &cd.init {
            let mut env: VarTypeEnv = VarTypeEnv::new();
            // Self binding: bound directly as `Type::Class(cd.id)`
            // with mutable=true. The "&mut Self" connotation is
            // tracked by the mutable bit in env; field access /
            // assignment work through the existing struct
            // machinery (no extra deref). Codegen treats self's
            // slot as the actual class storage pointer (init's
            // `out_ptr`).
            env.insert(init_def.self_var_id, (Type::Class(cd.id), true));
            env.record_name(init_def.self_var_id, "self");
            // ADR 0065: an `init` body produces the class value; a `return`
            // inside it returns the constructed object.
            env.set_return_type(Type::Class(cd.id));
            // Bind init params.
            for tp in &typed_class_decls[idx]
                .init
                .as_ref()
                .expect("init present")
                .params
            {
                env.insert(tp.id, (tp.ty, tp.mutable));
                env.record_name(tp.id, tp.name.clone());
            }
            let body = check_block(
                &init_def.body,
                None,
                &mut env,
                &typed_signatures,
                &typed_structs,
                &typed_class_decls,
                &typed_enums,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
                &typed_effect_decls,
                &typed_trait_decls,
                &typed_impl_decls,
                &mut konts,
                &mut tasks,
            )?;
            // C4.1 / ADR 0022 D4: definite-assignment (minimal at
            // this iteration). Walk the body's stmts and the tail
            // skip-recursively; collect the set of fields ever
            // assigned via `self.field = expr`. Reject any field
            // not in that set with InitFieldMaybeUnassigned.
            // Branch-aware (if/else snapshot+merge) dataflow is a
            // follow-on iteration; the C4.1 (2/N) minimum catches
            // the obvious "field never written" case.
            let assigned = collect_init_assigned_fields(
                &body,
                init_def.self_var_id,
                &typed_class_decls[idx],
            );
            for f in &typed_class_decls[idx].fields {
                if !assigned.contains(&f.name) {
                    return Err(TypeError::InitFieldMaybeUnassigned {
                        class_name: typed_class_decls[idx].name.clone(),
                        field_name: f.name.clone(),
                        field_span: to_source_span(&f.name_span),
                        init_span: to_source_span(&init_def.body.span),
                    });
                }
            }
            Some(TypedInitDef {
                visibility: init_def.visibility,
                self_var_id: init_def.self_var_id,
                params: typed_class_decls[idx]
                    .init
                    .as_ref()
                    .expect("init present")
                    .params
                    .clone(),
                body,
                span: init_def.span.clone(),
            })
        } else {
            None
        };
        // Method bodies.
        let mut methods = Vec::with_capacity(cd.methods.len());
        for (m_idx, m) in cd.methods.iter().enumerate() {
            let sig = &typed_class_decls[idx].methods[m_idx];
            let mut env: VarTypeEnv = VarTypeEnv::new();
            // Self binding: bound directly as `Type::Class(cd.id)`
            // with the mutable bit tracking `&Self` (false) vs
            // `&mut Self` (true). Codegen treats self's slot as
            // the class storage pointer (the method's first arg).
            let mutable = matches!(m.self_kind, SelfKind::Exclusive);
            env.insert(m.self_var_id, (Type::Class(cd.id), mutable));
            env.record_name(m.self_var_id, "self");
            for tp in &sig.params {
                env.insert(tp.id, (tp.ty, tp.mutable));
                env.record_name(tp.id, tp.name.clone());
            }
            let return_type = sig.return_type;
            // ADR 0065: record the return type for early `return` in the body.
            env.set_return_type(return_type);
            let body_expected =
                if return_type.is_nullable()
                    || return_type.is_generic_instance()
                    // D.3 / ADR 0034 D5: push a `Vec<T>` return type down
                    // into the body tail so `fn f() -> Vec<i64> { vec_new() }`
                    // infers vec_new's element (the same seeding `?T`/null
                    // and generic struct literals use).
                    || return_type.is_vec()
                    // ADR 0051: push a `secret T` / `[secret u8]` return type
                    // down so a public-typed body tail is widened to secret.
                    || return_type.is_secret_widen_target()
                {
                    Some(return_type)
                } else {
                    None
                };
            let body = check_block(
                &m.body,
                body_expected,
                &mut env,
                &typed_signatures,
                &typed_structs,
                &typed_class_decls,
                &typed_enums,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
                &typed_effect_decls,
                &typed_trait_decls,
                &typed_impl_decls,
                &mut konts,
                &mut tasks,
            )?;
            if !block_diverges(&body) && body.ty != return_type {
                return Err(TypeError::ReturnTypeMismatch {
                    name: format!("{}::{}", cd.name, m.name),
                    expected: return_type,
                    got: body.ty,
                    span: to_source_span(&m.body.tail.span),
                });
            }
            methods.push(TypedMethodDef {
                visibility: m.visibility,
                name: m.name.clone(),
                name_span: m.name_span.clone(),
                self_kind: m.self_kind,
                self_var_id: m.self_var_id,
                params: sig.params.clone(),
                return_type,
                effect_row: sig.effect_row.clone(),
                body,
                span: m.span.clone(),
            });
        }
        typed_class_decls[idx].init = init;
        typed_class_decls[idx].methods = methods;
    }

    // C4.2 / ADR 0023 D8 Pass 6: type-check each impl method body.
    // Mirrors Pass 5's class-method handling — bind synthetic self
    // + params, check the body against the declared return type.
    for imp in &program.impls {
        let idx = imp.id.0 as usize;
        let target_ty = match imp.target {
            ImplTarget::Class(cid) => Type::Class(cid),
            ImplTarget::Struct(sid) => Type::Struct(sid),
        };
        let mut new_methods = Vec::with_capacity(imp.methods.len());
        for (m_idx, m) in imp.methods.iter().enumerate() {
            let sig = &typed_impl_decls[idx].methods[m_idx];
            let mut env: VarTypeEnv = VarTypeEnv::new();
            let mutable = matches!(m.self_kind, SelfKind::Exclusive);
            // Self binding: bound directly to the impl target's
            // type, with mutable bit reflecting `self_kind`.
            env.insert(m.self_var_id, (target_ty, mutable));
            env.record_name(m.self_var_id, "self");
            for tp in &sig.params {
                env.insert(tp.id, (tp.ty, tp.mutable));
                env.record_name(tp.id, tp.name.clone());
            }
            let return_type = sig.return_type;
            // ADR 0065: record the return type for early `return` in the body.
            env.set_return_type(return_type);
            let body_expected =
                if return_type.is_nullable()
                    || return_type.is_generic_instance()
                    // D.3 / ADR 0034 D5: push a `Vec<T>` return type down
                    // into the body tail so `fn f() -> Vec<i64> { vec_new() }`
                    // infers vec_new's element (the same seeding `?T`/null
                    // and generic struct literals use).
                    || return_type.is_vec()
                    // ADR 0051: push a `secret T` / `[secret u8]` return type
                    // down so a public-typed body tail is widened to secret.
                    || return_type.is_secret_widen_target()
                {
                    Some(return_type)
                } else {
                    None
                };
            let body = check_block(
                &m.body,
                body_expected,
                &mut env,
                &typed_signatures,
                &typed_structs,
                &typed_class_decls,
                &typed_enums,
                &mut generic_instances,
                &mut refs,
                &mut secrets,
                &mut arrays,
                &struct_type_param_counts,
                &typed_effect_decls,
                &typed_trait_decls,
                &typed_impl_decls,
                &mut konts,
                &mut tasks,
            )?;
            if !block_diverges(&body) && body.ty != return_type {
                return Err(TypeError::ReturnTypeMismatch {
                    name: format!(
                        "impl {} for {} :: {}",
                        imp.trait_name, imp.type_name, m.name
                    ),
                    expected: return_type,
                    got: body.ty,
                    span: to_source_span(&m.body.tail.span),
                });
            }
            new_methods.push(TypedImplMethodDef {
                visibility: sig.visibility,
                name: sig.name.clone(),
                name_span: sig.name_span.clone(),
                self_kind: sig.self_kind,
                self_var_id: sig.self_var_id,
                params: sig.params.clone(),
                return_type,
                effect_row: sig.effect_row.clone(),
                body,
                span: sig.span.clone(),
            });
        }
        typed_impl_decls[idx].methods = new_methods;
    }

    Ok(TypedProgram {
        fns: typed_fns,
        fn_signatures: typed_signatures,
        externs: typed_externs,
        exports: program.exports.clone(),
        structs: typed_structs,
        generic_instances,
        refs,
        secrets,
        arrays,
        effect_decls: typed_effect_decls,
        konts,
        tasks,
        // ADR 0066 M1.2: populated when the channel builtins are typed
        // (their `Channel<i64>` return/param interns here). Empty until then.
        channels,
        // ADR 0071 M1.4a: pre-populated with the 6 word-scalar `Shared<T>`
        // elements at fixed SharedIds 0..=5 (matching `shared_id_for`).
        shared,
        // ADR 0071 M1.4b: pre-populated with the 6 word-scalar `Mutex<T>`
        // elements at fixed MutexIds 0..=5 (matching `mutex_id_for`).
        mutexes,
        class_decls: typed_class_decls,
        trait_decls: typed_trait_decls,
        impl_decls: typed_impl_decls,
        enums: typed_enums,
        span: program.span.clone(),
    })
}

/// Produce a trivial `{ 0 }` block used as a stub for class init /
/// method bodies during Pass 3.5 (signature population). Pass 5
/// overwrites these with real type-checked bodies.
fn stub_block(span: Span) -> TypedBlock {
    TypedBlock {
        stmts: vec![],
        tail: TypedExpr {
            kind: TypedExprKind::IntLit(0),
            span: span.clone(),
            ty: Type::I64,
        },
        span,
        ty: Type::I64,
    }
}

/// Walk a type-checked init body and return the set of field
/// names assigned via `self.field = expr` somewhere in the body
/// (any stmt or the tail). At C4.1 minimum this is a flat
/// collection — if any if/else branch leaves a field unassigned,
/// we accept it. Branch-aware merge is a follow-on iteration.
fn collect_init_assigned_fields(
    body: &TypedBlock,
    self_var_id: VarId,
    _class_data: &ClassData,
) -> std::collections::HashSet<String> {
    let mut acc: std::collections::HashSet<String> = std::collections::HashSet::new();
    for stmt in &body.stmts {
        collect_init_assigned_in_stmt(stmt, self_var_id, &mut acc);
    }
    collect_init_assigned_in_expr(&body.tail, self_var_id, &mut acc);
    acc
}

fn collect_init_assigned_in_stmt(
    stmt: &TypedStmt,
    self_var_id: VarId,
    acc: &mut std::collections::HashSet<String>,
) {
    match &stmt.kind {
        TypedStmtKind::Assign { target, value } => {
            collect_init_assigned_in_assign_target(target, self_var_id, acc);
            collect_init_assigned_in_expr(value, self_var_id, acc);
        }
        TypedStmtKind::Let { value, .. } => {
            collect_init_assigned_in_expr(value, self_var_id, acc);
        }
        TypedStmtKind::While { cond, body } => {
            // Phase D.5 / ADR 0036: a `while` body may assign `self.f`
            // (in a class init); recurse into the cond + body like the
            // If/Block forms.
            collect_init_assigned_in_expr(cond, self_var_id, acc);
            for s in &body.stmts {
                collect_init_assigned_in_stmt(s, self_var_id, acc);
            }
            collect_init_assigned_in_expr(&body.tail, self_var_id, acc);
        }
        // D.5 (2/N): payload-free loop control assigns no `self.field`.
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
        TypedStmtKind::Expr(e) => {
            collect_init_assigned_in_expr(e, self_var_id, acc);
        }
    }
}

fn collect_init_assigned_in_assign_target(
    target: &TypedExpr,
    self_var_id: VarId,
    acc: &mut std::collections::HashSet<String>,
) {
    if let TypedExprKind::FieldAccess { target: inner, field, .. } = &target.kind {
        if let TypedExprKind::Var(v) = &inner.kind {
            if *v == self_var_id {
                acc.insert(field.clone());
            }
        }
    }
}

fn collect_init_assigned_in_expr(
    expr: &TypedExpr,
    self_var_id: VarId,
    acc: &mut std::collections::HashSet<String>,
) {
    match &expr.kind {
        TypedExprKind::Block(b) => {
            for stmt in &b.stmts {
                collect_init_assigned_in_stmt(stmt, self_var_id, acc);
            }
            collect_init_assigned_in_expr(&b.tail, self_var_id, acc);
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            collect_init_assigned_in_expr(cond, self_var_id, acc);
            for stmt in &then_branch.stmts {
                collect_init_assigned_in_stmt(stmt, self_var_id, acc);
            }
            collect_init_assigned_in_expr(&then_branch.tail, self_var_id, acc);
            for stmt in &else_branch.stmts {
                collect_init_assigned_in_stmt(stmt, self_var_id, acc);
            }
            collect_init_assigned_in_expr(&else_branch.tail, self_var_id, acc);
        }
        _ => {}
    }
}

/// Walk the struct-field graph looking for cycles consisting of
/// only direct (non-nullable) edges. C1.6 / ADR 0015 D11 implements
/// the ADR 0014 D10 relaxation: cycles that pass through at least
/// one `Nullable(Struct)` edge are accepted because the nullable
/// payload is now heap-allocated (per the C1.6 codegen `?Struct =
/// { i1, T* }` representation), which provides the indirection
/// that bounds the recursive struct's size at runtime.
///
/// The detector walks only `Type::Struct(_)` edges as cycle-
/// contributing. `Type::Nullable(NullableInner::Struct(_))` edges
/// are skipped — the heap indirection breaks the cycle. Arrays
/// (`Type::Array(_)`) are also heap-backed so they similarly break
/// cycles, but at C1.6 array elements can't themselves be arrays
/// (per ADR 0015 D6), so array-via-cycle is a corner case captured
/// implicitly.
fn detect_struct_cycle(structs: &[TypedStructDecl]) -> Result<(), TypeError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let n = structs.len();
    let mut color = vec![Color::White; n];
    // path: the current DFS stack, indices into structs.
    let mut path: Vec<usize> = Vec::new();

    fn visit(
        i: usize,
        structs: &[TypedStructDecl],
        color: &mut [Color],
        path: &mut Vec<usize>,
    ) -> Result<(), TypeError> {
        color[i] = Color::Gray;
        path.push(i);
        for field in &structs[i].fields {
            // C1.6 / ADR 0015 D11: only direct `Struct(_)` edges
            // contribute to cycles. Nullable struct edges
            // (`?Struct`) and array edges break cycles via heap
            // indirection.
            if let Type::Struct(child_id) = field.ty {
                let j = child_id.0 as usize;
                match color[j] {
                    Color::Gray => {
                        // Cycle found — collect names from the path
                        // starting at the first occurrence of j.
                        let start = path.iter().position(|&p| p == j).unwrap_or(0);
                        let mut cycle: Vec<String> =
                            path[start..].iter().map(|&p| structs[p].name.clone()).collect();
                        cycle.push(structs[j].name.clone()); // close the loop
                        return Err(TypeError::RecursiveStruct {
                            name: structs[j].name.clone(),
                            cycle,
                            span: to_source_span(&structs[j].span),
                        });
                    }
                    Color::White => visit(j, structs, color, path)?,
                    Color::Black => {}
                }
            }
            // Nullable struct edges (`Type::Nullable(
            // NullableInner::Struct(_))`) intentionally don't
            // contribute. The codegen `{ i1, T* }` heap representation
            // makes the cycle finite-sized.
        }
        path.pop();
        color[i] = Color::Black;
        Ok(())
    }

    for i in 0..n {
        if color[i] == Color::White {
            visit(i, structs, &mut color, &mut path)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_fn(
    fn_def: &ResolvedFnDef,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<TypedFnDef, TypeError> {
    // Pull our own signature.
    let signature = &signatures[fn_def.id.0 as usize];

    // Build typed params from our signature (already type-resolved
    // when the signature table was built).
    let mut params = Vec::with_capacity(fn_def.params.len());
    for (param, ty) in fn_def.params.iter().zip(signature.param_types.iter()) {
        params.push(TypedParam {
            id: param.id,
            mutable: param.mutable,
            name: param.name.clone(),
            span: param.span.clone(),
            ty: *ty,
        });
    }

    // Build a VarId -> (Type, mutable) map for this fn's scope.
    // C2 / ADR 0017 D2: mutability is per-binding; the type checker
    // consults this to validate `&mut x` / `x = v;` operations.
    let mut env: VarTypeEnv = VarTypeEnv::new();
    for tp in &params {
        env.insert(tp.id, (tp.ty, tp.mutable));
        env.record_name(tp.id, tp.name.clone());
    }

    let return_type = signature.return_type;
    // ADR 0065: record the return type so an early `return expr` in the body
    // checks its inner against it.
    env.set_return_type(return_type);
    // ADR 0014 D5: push the declared return type down into the body
    // so NullLit / T→?T widening at the tail can resolve against it.
    // We only push for the type-shapes that actually NEED context:
    // nullables (per ADR 0014 D5), generic-struct instances (per
    // ADR 0016 D6 — the literal `Pair { ... }` inside `make_pair<A, B>`
    // can't synthesise its own type args), and `Vec<T>` (per ADR 0034
    // D5 — `vec_new()` in tail position can't synthesise its element).
    // Other returns synthesise freely so the more specific
    // ReturnTypeMismatch fires on primitive shape errors.
    let body_expected =
        if return_type.is_nullable()
            || return_type.is_generic_instance()
            || return_type.is_vec()
            // ADR 0051: a `secret T` / `[secret u8]` return widens its tail.
            || return_type.is_secret_widen_target()
        {
            Some(return_type)
        } else {
            None
        };
    let body = check_block(
        &fn_def.body,
        body_expected,
        &mut env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;

    // ADR 0065: if every path through the body early-`return`s (so it has no
    // reachable tail value), each `return` already checked its own inner
    // against `return_type` — skip the tail-vs-return-type match (the tail is
    // dead). Otherwise the tail must produce the declared return type.
    if !block_diverges(&body) && body.ty != return_type {
        return Err(TypeError::ReturnTypeMismatch {
            name: fn_def.name.clone(),
            expected: return_type,
            got: body.ty,
            span: to_source_span(&fn_def.body.tail.span),
        });
    }
    // ADR 0071 M1.4a slice 3: a fn whose reachable TAIL is a named `Shared` binding
    // transfers that binding's refcount unit to the caller — guarded until slice 3b
    // mirrors the transfer exemption into the oracle + scg (return the producing
    // expression directly instead). Early `return <named Shared>` is guarded in
    // `check_expr`'s `Return` arm (covering method bodies too).
    if !block_diverges(&body) && is_named_shared_return(&body.tail) {
        return Err(TypeError::SharedReturnNotSupported {
            span: to_source_span(&fn_def.body.tail.span),
        });
    }

    let typed_type_params: Vec<TypedTypeParam> = fn_def
        .type_params
        .iter()
        .map(|tp| TypedTypeParam {
            id: tp.id,
            name: tp.name.clone(),
            name_span: tp.name_span.clone(),
        })
        .collect();

    Ok(TypedFnDef {
        id: fn_def.id,
        name: fn_def.name.clone(),
        name_span: fn_def.name_span.clone(),
        type_params: typed_type_params,
        params,
        return_type,
        body,
        span: fn_def.span.clone(),
    })
}

/// Per-fn type environment: VarId → (Type, mutable), plus the
/// binding's source name for diagnostics. Inside a generic-fn body
/// the value type may be `Type::TypeParam(_)`; concrete substitution
/// happens at the caller. The `mutable` bit (C2 / ADR 0017 D2) records
/// whether the binding was declared with `let mut` / `mut param` — the
/// type checker reads it for `&mut x` and `x = v;` validation.
///
/// The struct `Deref`s to the inner `VarId → (Type, mutable)` map, so
/// every `env.insert` / `env.get` / `env.remove` call works unchanged;
/// the parallel `names` map is populated at binding sites and read by
/// the `BorrowMutOfImmutable` / `AssignToImmutable` diagnostics so they
/// can name the offending binding (a `VarId`'s name is stable, so
/// `names` is write-once and untouched by the scoped save/restore of
/// the type map).
#[derive(Default)]
struct VarTypeEnv {
    types: std::collections::HashMap<VarId, (Type, bool)>,
    names: std::collections::HashMap<VarId, String>,
    /// Phase D.5 (2/N) / ADR 0036 D9: how many `while` bodies enclose the
    /// statement currently being checked. Incremented around a `while`
    /// body (`enter_loop`/`exit_loop`), so `break`/`continue` are legal
    /// iff `> 0` (`in_loop`). With no labels at D.5, the target is always
    /// the innermost loop, so a nonzero depth is the whole validity rule.
    /// A fresh env is built per fn, so this resets at every fn boundary
    /// (you cannot `break` out of a fn into an enclosing loop). Not part
    /// of the scope save/restore — only the `types` map is snapshot.
    loop_depth: u32,
    /// ADR 0065: the enclosing function's declared return type, set when the
    /// per-fn env is built (a fresh env per fn, like `loop_depth`). A
    /// `return expr` checks its inner against this. `None` only in the
    /// degenerate case of checking an expression with no enclosing fn (never
    /// happens for real bodies); the `Return` arm then skips the match.
    current_return_type: Option<Type>,
}

impl VarTypeEnv {
    fn new() -> Self {
        Self::default()
    }

    /// Record a binding's source name for diagnostics. Called at every
    /// binding site (params, `let`s, `self`, `match` bindings) so an
    /// `&mut`-of-immutable / assign-to-immutable error can name it.
    fn record_name(&mut self, id: VarId, name: impl Into<String>) {
        self.names.insert(id, name.into());
    }

    /// The binding's source name, if recorded.
    fn name_of(&self, id: VarId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Phase D.5 (2/N) / ADR 0036 D9: enter a `while` body — bump the
    /// loop-nesting depth so `break`/`continue` inside it are legal.
    fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    /// Leave a `while` body — restore the enclosing loop-nesting depth.
    fn exit_loop(&mut self) {
        self.loop_depth -= 1;
    }

    /// Whether the statement being checked is inside at least one
    /// enclosing `while` loop (so `break`/`continue` are legal here).
    fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    /// ADR 0065: record the enclosing function's declared return type, so a
    /// `return expr` deep in the body can check its inner against it.
    fn set_return_type(&mut self, ty: Type) {
        self.current_return_type = Some(ty);
    }
}

impl std::ops::Deref for VarTypeEnv {
    type Target = std::collections::HashMap<VarId, (Type, bool)>;
    fn deref(&self) -> &Self::Target {
        &self.types
    }
}

impl std::ops::DerefMut for VarTypeEnv {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.types
    }
}

// `build_struct_type_param_counts` lived here briefly as a
// helper but was inlined into the few sites that needed it; the
// `struct_type_param_counts` parameter is threaded through `check`
// itself.


/// Synthesize a [`TypedTypeParam`] for one of the C1.7 generic
/// builtins (`unwrap_or`, `is_some`, `len`). The name is the
/// source-level identifier (`"T"`) for diagnostics; the span is
/// synthetic (`0..0`) since builtins have no source-level decl.
fn builtin_type_param(name: &str, idx: u32) -> TypedTypeParam {
    TypedTypeParam {
        id: TypeParamId(idx),
        name: name.to_string(),
        name_span: 0..0,
    }
}

#[allow(clippy::too_many_arguments)]
fn check_block(
    block: &ResolvedBlock,
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<TypedBlock, TypeError> {
    let mut stmts = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        stmts.push(check_stmt(
            stmt,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?);
    }
    // Only the tail receives the expected-type pushdown (the block's
    // value is the tail's value).
    let tail = check_expr(
        &block.tail,
        expected,
        env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;
    let ty = tail.ty;
    Ok(TypedBlock {
        stmts,
        tail,
        span: block.span.clone(),
        ty,
    })
}

/// ADR 0065: whether evaluating `e` always diverges (returns from the enclosing
/// function), so it yields no value to its position. Consulted at type-join
/// sites (if/match branches, the fn body tail) so a `return` branch unifies
/// with whatever the other branch produces, and a fully-returning body is not
/// re-checked against the declared return type. Structural + conservative: only
/// `Return`, and a `Block`/`If`/`Match` ALL of whose paths diverge, count.
/// Exotic embeddings (`f(return x)`) are treated as non-divergent — sound, just
/// over-rejecting, matching the 1.0 lexical-checker philosophy.
fn expr_diverges(e: &TypedExpr) -> bool {
    match &e.kind {
        TypedExprKind::Return(_) => true,
        TypedExprKind::Block(b) => block_diverges(b),
        TypedExprKind::If { then_branch, else_branch, .. } => {
            block_diverges(then_branch) && block_diverges(else_branch)
        }
        TypedExprKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body))
        }
        _ => false,
    }
}

/// ADR 0065: whether a block always diverges — either some statement before the
/// tail unconditionally diverges (making the tail dead), or the tail diverges.
fn block_diverges(b: &TypedBlock) -> bool {
    if expr_diverges(&b.tail) {
        return true;
    }
    b.stmts.iter().any(stmt_diverges)
}

/// ADR 0065: whether a statement unconditionally diverges. A `while` may not
/// run, and `break`/`continue` stay inside the function, so only a diverging
/// value-expression counts.
fn stmt_diverges(s: &TypedStmt) -> bool {
    match &s.kind {
        TypedStmtKind::Expr(e) => expr_diverges(e),
        TypedStmtKind::Let { value, .. } | TypedStmtKind::Assign { value, .. } => {
            expr_diverges(value)
        }
        TypedStmtKind::While { .. } | TypedStmtKind::Break | TypedStmtKind::Continue => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn check_stmt(
    stmt: &ResolvedStmt,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<TypedStmt, TypeError> {
    let kind = match &stmt.kind {
        ResolvedStmtKind::Let { id, mutable, name, name_span, ty_annot, value } => {
            // ADR 0014 D5: if the let has a type annotation, push it
            // down into the RHS as the expected type. This is what
            // makes `let x: ?i64 = null;` typecheck.
            let expected = match ty_annot {
                Some(annot) => {
                    let struct_table = struct_name_table_local(structs);
                    let class_table = class_name_table_local(class_decls);
                    let enum_table = enum_name_table_local(enums);
                    Some(resolve_type_expr(
                        annot,
                        &struct_table,
                        &class_table,
                        &enum_table,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                    )?)
                }
                None => None,
            };
            let value_typed = check_expr(
                value,
                expected,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            let ty = match (ty_annot, expected) {
                (Some(_), Some(annotated)) => {
                    // check_expr already validated; result must match.
                    if annotated != value_typed.ty {
                        return Err(TypeError::Mismatch {
                            expected: annotated,
                            got: value_typed.ty,
                            span: to_source_span(&value.span),
                        });
                    }
                    annotated
                }
                _ => value_typed.ty,
            };
            env.insert(*id, (ty, *mutable));
            env.record_name(*id, name.clone());
            TypedStmtKind::Let {
                id: *id,
                mutable: *mutable,
                name: name.clone(),
                name_span: name_span.clone(),
                ty,
                value: value_typed,
            }
        }
        ResolvedStmtKind::Assign { target, value } => {
            // C2 / ADR 0017 D2 + D5: type-check the LHS first (as
            // a normal expression) so we can read its type; then
            // type-check the RHS with the LHS type as the expected
            // (so widening / null work inside `x = null;` for
            // `x: ?T`). Then validate the LHS is an lvalue and is
            // mutable-assignable.
            let target_typed = check_expr(
                target,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            let value_typed = check_expr(
                value,
                Some(target_typed.ty),
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            if value_typed.ty != target_typed.ty {
                return Err(TypeError::Mismatch {
                    expected: target_typed.ty,
                    got: value_typed.ty,
                    span: to_source_span(&value.span),
                });
            }
            check_assign_lvalue(&target_typed, env, refs, arrays)?;
            TypedStmtKind::Assign {
                target: target_typed,
                value: value_typed,
            }
        }
        ResolvedStmtKind::While { cond, body } => {
            // Phase D.5 / ADR 0036 D7: the condition must be `bool`
            // (mirrors `if`). A `secret bool` condition leaks via timing
            // (ADR 0019 D7 SecretBranch) — reject before the generic
            // Mismatch. The body block type-checks with no expected type
            // (its value is discarded each iteration, D3).
            let cond_t = check_expr(
                cond,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            if let Type::Secret(sid) = cond_t.ty {
                if secrets[sid.0 as usize].inner == Type::Bool {
                    return Err(TypeError::SecretBranch {
                        kw: "while",
                        span: to_source_span(&cond.span),
                    });
                }
            }
            if cond_t.ty != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: cond_t.ty,
                    span: to_source_span(&cond.span),
                });
            }
            // Phase D.5 (2/N) / ADR 0036 D9: bump the loop-nesting depth
            // around the body so `break`/`continue` inside it (including
            // inside nested `if`/`match` blocks, which thread the same
            // `env`) are accepted; restore it afterwards (even on error,
            // so sibling code sees the right depth).
            env.enter_loop();
            let body_result = check_block(
                body,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            );
            env.exit_loop();
            let body_t = body_result?;
            TypedStmtKind::While {
                cond: cond_t,
                body: Box::new(body_t),
            }
        }
        // Phase D.5 (2/N) / ADR 0036 D9: `break`/`continue` are legal only
        // inside a `while` body (else there is no `loop_after`/`loop_cond`
        // to branch to at codegen). The env's loop-nesting depth — bumped
        // around every `while` body above — is the whole rule (no labels,
        // so the target is always the innermost loop).
        ResolvedStmtKind::Break => {
            if !env.in_loop() {
                return Err(TypeError::LoopControlOutsideLoop {
                    kw: "break",
                    span: to_source_span(&stmt.span),
                });
            }
            TypedStmtKind::Break
        }
        ResolvedStmtKind::Continue => {
            if !env.in_loop() {
                return Err(TypeError::LoopControlOutsideLoop {
                    kw: "continue",
                    span: to_source_span(&stmt.span),
                });
            }
            TypedStmtKind::Continue
        }
        ResolvedStmtKind::Expr(e) => TypedStmtKind::Expr(check_expr(
            e,
            None,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?),
    };
    Ok(TypedStmt { kind, span: stmt.span.clone() })
}

/// `true` iff `expr` is an lvalue per ADR 0017 D5 — a Var, a
/// dereference, a field access on an lvalue, or an index on an
/// lvalue. The borrow-take operator `&` / `&mut` and assignment
/// statements both require their operand to be an lvalue.
fn is_lvalue(expr: &TypedExpr) -> bool {
    matches!(
        &expr.kind,
        TypedExprKind::Var(_)
            | TypedExprKind::Unary(UnaryOp::Deref, _)
            | TypedExprKind::FieldAccess { .. }
            | TypedExprKind::Index { .. }
    )
}

/// Validate the LHS of an assignment per ADR 0017 D2 + D5.
/// Checks: (a) the target is an lvalue; (b) it's mutable —
/// either the binding was declared `let mut` / `mut param`, or
/// the deref is through `&mut T`, or transitively through a
/// chain of those.
fn check_assign_lvalue(
    target: &TypedExpr,
    env: &VarTypeEnv,
    refs: &[RefData],
    arrays: &[ArrayElem],
) -> Result<(), TypeError> {
    if !is_lvalue(target) {
        return Err(TypeError::AssignToRvalue {
            span: to_source_span(&target.span),
        });
    }
    check_mutable_lvalue(target, env, refs, arrays)
}

/// Validate that the operand of `&mut expr` is a mutable lvalue.
/// Symmetric with [`check_mutable_lvalue`] (the assignment-LHS
/// path); diagnostics use `BorrowMutOfImmutable` instead of
/// `AssignToImmutable` for the Var case.
fn check_mutable_borrow_target(
    target: &TypedExpr,
    env: &VarTypeEnv,
    refs: &[RefData],
) -> Result<(), TypeError> {
    match &target.kind {
        TypedExprKind::Var(id) => {
            let (_ty, mutable) = env
                .get(id)
                .copied()
                .expect("VarId in scope per resolve invariants");
            if !mutable {
                return Err(TypeError::BorrowMutOfImmutable {
                    name: env.name_of(*id).unwrap_or("<binding>").to_string(),
                    span: to_source_span(&target.span),
                });
            }
            Ok(())
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => match inner.ty {
            Type::Ref(id) => {
                let data = &refs[id.0 as usize];
                if !data.mutable {
                    return Err(TypeError::AssignThroughSharedRef {
                        span: to_source_span(&target.span),
                    });
                }
                Ok(())
            }
            _ => Err(TypeError::DerefOfNonRef {
                got: inner.ty,
                span: to_source_span(&inner.span),
            }),
        },
        TypedExprKind::FieldAccess { target: inner_target, .. } => {
            check_mutable_borrow_target(inner_target, env, refs)
        }
        // ADR 0054: `&mut a[i]`. An array/Vec element is a mutable place exactly
        // like a struct field `&mut s.f` — recurse so the base collection must be a
        // mutable lvalue. The index is already constrained to a public `i64` by
        // `IndexNotInt` when the inner `a[i]` was type-checked, so a secret index is
        // rejected for borrows exactly as for reads/writes (the constant-time story,
        // no new sink). Granularity is whole-array (binding-precise), the same
        // conservative pre-Polonius choice the borrow checker makes for `&mut s.f`.
        TypedExprKind::Index { target: inner_target, .. } => {
            check_mutable_borrow_target(inner_target, env, refs)
        }
        _ => Err(TypeError::BorrowOfRvalue {
            span: to_source_span(&target.span),
        }),
    }
}

/// Inner check: recurse through lvalue shape and gate on
/// mutability. Surfaces `AssignToImmutable` for var stores,
/// `AssignThroughSharedRef` for deref-stores via `&T`, and
/// `IndexAssignNotSupported` for `a[i] = v;` which ADR 0017 D12
/// punts.
fn check_mutable_lvalue(
    target: &TypedExpr,
    env: &VarTypeEnv,
    refs: &[RefData],
    arrays: &[ArrayElem],
) -> Result<(), TypeError> {
    match &target.kind {
        TypedExprKind::Var(id) => {
            let (_ty, mutable) = env
                .get(id)
                .copied()
                .expect("VarId in scope per resolve invariants");
            if !mutable {
                // The binding's source name is recorded in `env.names`
                // at its binding site (param / `let` / `self` / `match`
                // binding); fall back only for synthetic bindings with
                // no source name.
                return Err(TypeError::AssignToImmutable {
                    name: env.name_of(*id).unwrap_or("<binding>").to_string(),
                    span: to_source_span(&target.span),
                });
            }
            Ok(())
        }
        TypedExprKind::Unary(UnaryOp::Deref, inner) => {
            // `*r = v;` requires r: &mut T.
            match inner.ty {
                Type::Ref(id) => {
                    let data = &refs[id.0 as usize];
                    if !data.mutable {
                        return Err(TypeError::AssignThroughSharedRef {
                            span: to_source_span(&target.span),
                        });
                    }
                    Ok(())
                }
                _ => Err(TypeError::DerefOfNonRef {
                    got: inner.ty,
                    span: to_source_span(&inner.span),
                }),
            }
        }
        TypedExprKind::FieldAccess { target: inner_target, .. } => {
            check_mutable_lvalue(inner_target, env, refs, arrays)
        }
        // ADR 0050: `a[i] = v;`. The base collection must be a mutable
        // lvalue (recurse, exactly like field-assign), and the element
        // must be Copy — a scalar or `secret` scalar (a plain store). A
        // Move element (struct / generic) would need drop-on-overwrite,
        // deferred. The index is already constrained to a public `i64`
        // by `IndexNotInt` when the target was type-checked, so a secret
        // index is rejected for writes exactly as for reads.
        TypedExprKind::Index { target: inner_target, elem_ty, .. } => {
            check_mutable_lvalue(inner_target, env, refs, arrays)?;
            match elem_ty {
                ArrayElem::Struct(_)
                | ArrayElem::TypeParam(_)
                | ArrayElem::GenericInstance(_) => Err(TypeError::IndexAssignNonCopyElem {
                    elem_ty: elem_ty.to_type(),
                    span: to_source_span(&target.span),
                }),
                ArrayElem::I64
                | ArrayElem::I32
                | ArrayElem::Bool
                | ArrayElem::U8
                | ArrayElem::Secret(_) => Ok(()),
                // ADR 0068: a nested-array element (`a[i] = v` on a `[[T]]`) is
                // non-Copy (the inner `[T]` owns a buffer), so index-assignment
                // is rejected once nested arrays are constructible (drop-on-
                // overwrite deferred). Unreachable until the resolution-wiring
                // slice produces `ArrayElem::Array` (the precise element type
                // for the diagnostic needs the `arrays` interner, threaded then).
                // ADR 0068: a nested-array element (`a[i] = v` on a `[[T]]`) is
                // non-Copy (the inner `[T]` owns a heap buffer), like a Struct
                // element — index-assignment is rejected (drop-on-overwrite
                // deferred). The inner element type comes from the `arrays`
                // interner for the diagnostic.
                ArrayElem::Array(id) => Err(TypeError::IndexAssignNonCopyElem {
                    elem_ty: Type::Array(arrays[id.0 as usize]),
                    span: to_source_span(&target.span),
                }),
            }
        }
        _ => Err(TypeError::AssignToRvalue {
            span: to_source_span(&target.span),
        }),
    }
}

/// Build a local struct-name table from the typed struct list.
/// Used by [`check_stmt`] when resolving let-binding annotations
/// (which arrive as TypeExprs at C1.4).
fn struct_name_table_local(structs: &[TypedStructDecl]) -> HashMap<String, StructId> {
    structs.iter().map(|s| (s.name.clone(), s.id)).collect()
}

/// C4.1 / ADR 0022 D1: build a class name → ClassId table from the
/// in-progress typed class_decls. Used by `check_stmt`'s let-annotation
/// path to resolve class names in type position.
fn class_name_table_local(class_decls: &[ClassData]) -> HashMap<String, ClassId> {
    class_decls.iter().map(|c| (c.name.clone(), c.id)).collect()
}

/// Phase D.1 / ADR 0032 (3/N): build an enum name → EnumId table from
/// the typed enum decls. Used by `check_stmt`'s let-annotation path to
/// resolve enum names in type position.
fn enum_name_table_local(enums: &[EnumData]) -> HashMap<String, EnumId> {
    enums.iter().map(|e| (e.name.clone(), e.id)).collect()
}

/// Apply ADR 0014 D3 widening / D2 null-literal context to a typed
/// expression against an expected type. If `expected` is None, the
/// expression's synthesized type passes through unchanged. If it's
/// `Some(?T)` and the synth type is `T`, wrap with WidenToNullable.
/// If it's `Some(secret T)` and the synth type is `T`, wrap with
/// WidenToSecret per ADR 0019 D5 (C3.1). Mismatches surface as
/// `TypeError::Mismatch`.
fn coerce_to_expected(
    synth: TypedExpr,
    expected: Option<Type>,
    span_for_mismatch: &Span,
    secrets: &[SecretData],
) -> Result<TypedExpr, TypeError> {
    let Some(exp) = expected else {
        return Ok(synth);
    };
    if synth.ty == exp {
        return Ok(synth);
    }
    // Implicit T → ?T widening per ADR 0014 D3.
    if let Type::Nullable(inner) = exp {
        if synth.ty == inner.to_type() {
            let span = synth.span.clone();
            return Ok(TypedExpr {
                kind: TypedExprKind::WidenToNullable(Box::new(synth)),
                span,
                ty: exp,
            });
        }
    }
    // C3 / ADR 0019 D5 (C3.1): implicit T → secret T widening.
    // Lets `let pw: secret i64 = 42;` and `f(42)` where `f(x:
    // secret i64)` type-check. The wrap is purely type-level
    // (codegen lowers as identity).
    if let Type::Secret(id) = exp {
        if synth.ty == secrets[id.0 as usize].inner {
            let span = synth.span.clone();
            return Ok(TypedExpr {
                kind: TypedExprKind::WidenToSecret(Box::new(synth)),
                span,
                ty: exp,
            });
        }
    }
    // ADR 0051: implicit `[u8] -> [secret u8]` array widening — a public byte
    // buffer flowing into a secret-byte context (a `let` annotation, a `secret
    // [u8]` parameter, or a `secret [u8]` return). `[u8]` and `[secret u8]` share
    // the `{ i64 len, ptr data }` layout, so this is a pure type re-tag; the
    // `WidenToSecret` node lowers as identity (ADR 0047 restricts secret arrays
    // to scalar elements, so the element is always a scalar).
    if let Type::Array(ArrayElem::Secret(id)) = exp {
        let elem_scalar = secrets[id.0 as usize].inner;
        if let Some(public_elem) = elem_scalar.to_array_elem() {
            if synth.ty == Type::Array(public_elem) {
                let span = synth.span.clone();
                return Ok(TypedExpr {
                    kind: TypedExprKind::WidenToSecret(Box::new(synth)),
                    span,
                    ty: exp,
                });
            }
        }
    }
    Err(TypeError::Mismatch {
        expected: exp,
        got: synth.ty,
        span: to_source_span(span_for_mismatch),
    })
}

/// Wrap a public expression in `WidenToSecret`, yielding `secret <inner>`
/// (ADR 0051). The wrap is purely type-level — codegen lowers it as identity,
/// exactly like the `let pw: secret i64 = 42;` widen. Used by the binary / cmp
/// operand widen so `secret_x + 5` needs no pre-bound `secret` constant.
/// `inner` must be `expr`'s (public) type.
fn widen_operand_to_secret(
    expr: TypedExpr,
    inner: Type,
    secrets: &mut Vec<SecretData>,
) -> TypedExpr {
    let span = expr.span.clone();
    let id = intern_secret(secrets, inner);
    TypedExpr {
        kind: TypedExprKind::WidenToSecret(Box::new(expr)),
        span,
        ty: Type::Secret(id),
    }
}

/// Try to substitute every [`Type::TypeParam`] in `ty` using the
/// partial substitution map. Returns `Some(concrete)` iff every
/// TypeParam encountered has been bound; `None` otherwise. Used by
/// [`check_call`] to determine whether a generic param's type is
/// fully concretized enough to push down to its arg as bidirectional
/// expected-type.
fn try_substitute(
    ty: Type,
    subst: &[Option<Type>],
    instances: &[GenericInstanceData],
    refs: &[RefData],
) -> Option<Type> {
    match ty {
        Type::I64
        | Type::I32
        | Type::U8
        | Type::U128
        // ADR 0058: `f64` is a scalar — no TypeParam payload.
        | Type::F64
        // ADR 0057: `ptr` is a scalar opaque — no TypeParam payload.
        | Type::Ptr
        | Type::Bool
        | Type::Struct(_)
        | Type::Class(_)
        | Type::TraitSelf(_)
        // Phase D.1 / ADR 0032: enums carry no TypeParam at the MVP.
        | Type::Enum(_) => Some(ty),
        Type::TypeParam(id) => subst.get(id.0 as usize).copied().flatten(),
        Type::Nullable(ni) => {
            let inner = try_substitute(ni.to_type(), subst, instances, refs)?;
            inner.to_nullable_inner().map(Type::Nullable)
        }
        Type::Array(ae) => {
            // ADR 0068: a nested-array element is interned/concrete — no TypeParam
            // to substitute (generic-nested deferred, D6). Pass through unchanged.
            if let ArrayElem::Array(_) = ae {
                return Some(Type::Array(ae));
            }
            let inner = try_substitute(ae.to_type(), subst, instances, refs)?;
            // ADR 0053: secret-aware demote (the substitution round-trip).
            inner.to_array_elem_subst().map(Type::Array)
        }
        // Phase D.3 / ADR 0034: concrete iff the element is fully bound.
        Type::Vec(ve) => {
            let inner = try_substitute(ve.to_type(), subst, instances, refs)?;
            // ADR 0052: secret-aware demote (the substitution round-trip).
            inner.to_vec_elem_subst().map(Type::Vec)
        }
        Type::GenericInstance(id) => {
            // Concrete only if every arg is concrete (under
            // current subst). Doesn't re-intern — that's the
            // mutable-substitute path.
            let inst = &instances[id.0 as usize];
            for arg in &inst.args {
                try_substitute(*arg, subst, instances, refs)?;
            }
            Some(ty)
        }
        Type::Ref(id) => {
            // Concrete only if the inner type is fully bound.
            let data = &refs[id.0 as usize];
            try_substitute(data.inner, subst, instances, refs)?;
            Some(ty)
        }
        // C3 / ADR 0019 D5: `Type::Secret(_)` at C3.1 wraps a
        // concrete (non-TypeParam) inner — secrets don't compose
        // with generics in the minimum-viable surface. Just pass
        // through.
        Type::Secret(_) => Some(ty),
        // C3.4 / ADR 0020 D5: konts never compose with generics at
        // C3.4 minimum (no effect polymorphism per D10).
        Type::Kont(_) => Some(ty),
        // C4.4 / ADR 0024 D4: `Task<i64>` carries no TypeParam.
        Type::Task(_) => Some(ty),
        // ADR 0066 M1.2: `Channel<i64>` carries no TypeParam.
        Type::Channel(_) => Some(ty),
        // ADR 0071 M1.4a: `Shared<T>`'s word-scalar element carries no TypeParam.
        Type::Shared(_) => Some(ty),
        // ADR 0071 M1.4b: `Mutex<T>`'s word-scalar element carries no TypeParam.
        Type::Mutex(_) => Some(ty),
        // ADR 0066 M2.1: `Process` carries no TypeParam.
        Type::Process => Some(ty),
        // ADR 0066 M2.4a: `SealedChannel` carries no TypeParam.
        Type::SealedChannel => Some(ty),
        // ADR 0070 (generalized): Fn<T,R>'s T/R are always concrete word-scalars
        // (never a TypeParam) — no substitution needed.
        Type::Fn(_) => Some(ty),
    }
}

/// `true` iff `ty` mentions at least one [`Type::TypeParam`],
/// either directly or inside the args of a GenericInstance / the
/// inner of a Ref.
fn contains_type_param(
    ty: Type,
    instances: &[GenericInstanceData],
    refs: &[RefData],
) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::Nullable(ni) => contains_type_param(ni.to_type(), instances, refs),
        // ADR 0068: a nested-array element is interned/concrete (generic-nested
        // `[[T]]` deferred, D6), so it carries no TypeParam — and `ae.to_type()`
        // would panic on `Array`.
        Type::Array(ArrayElem::Array(_)) => false,
        Type::Array(ae) => contains_type_param(ae.to_type(), instances, refs),
        // Phase D.3 / ADR 0034: a Vec mentions a TypeParam iff its
        // element does (mirrors the Array arm).
        Type::Vec(ve) => contains_type_param(ve.to_type(), instances, refs),
        Type::I64
        | Type::I32
        | Type::U8
        | Type::U128
        // ADR 0058: `f64` is a scalar — never a TypeParam.
        | Type::F64
        // ADR 0057: `ptr` is a scalar opaque — never a TypeParam.
        | Type::Ptr
        | Type::Bool
        | Type::Struct(_)
        | Type::Class(_)
        | Type::TraitSelf(_)
        // Phase D.1 / ADR 0032: enums carry no TypeParam at the MVP.
        | Type::Enum(_) => false,
        Type::GenericInstance(id) => instances[id.0 as usize]
            .args
            .iter()
            .any(|a| contains_type_param(*a, instances, refs)),
        Type::Ref(id) => contains_type_param(refs[id.0 as usize].inner, instances, refs),
        // C3 / ADR 0019 D5: secret-of-non-TypeParam at C3.1 — no
        // recursion needed. Will revisit if `secret T<U>` style
        // ever lands.
        Type::Secret(_) => false,
        // C3.4 / ADR 0020 D5: konts don't carry TypeParams at C3.4
        // minimum.
        Type::Kont(_) => false,
        // C4.4 / ADR 0024 D4: `Task<i64>` carries no TypeParam.
        Type::Task(_) => false,
        // ADR 0066 M1.2: `Channel<i64>` carries no TypeParam.
        Type::Channel(_) => false,
        // ADR 0071 M1.4a: `Shared<T>`'s word-scalar element is never a TypeParam.
        Type::Shared(_) => false,
        // ADR 0071 M1.4b: `Mutex<T>`'s word-scalar element is never a TypeParam.
        Type::Mutex(_) => false,
        Type::Process => false,
        // ADR 0066 M2.4a: `SealedChannel` carries no TypeParam.
        Type::SealedChannel => false,
        // ADR 0070 (generalized): Fn<T,R>'s T/R are always concrete word-scalars.
        Type::Fn(_) => false,
    }
}

/// Internal result type for [`unify_one`]. The outer call site
/// translates these into [`TypeError::CallArgMismatch`] or
/// [`TypeError::TypeArgInferenceConflict`] with the proper arg
/// index / callee name context.
enum UnifyFailure {
    /// The structural shape of `param` doesn't match `arg` — the
    /// classic CallArgMismatch (the inner pair is `(expected, got)`).
    Mismatch(Type, Type),
    /// The same TypeParam was bound twice to different concrete
    /// types — surfaces as `TypeError::TypeArgInferenceConflict`.
    TypeParamConflict {
        idx: u32,
        first: Type,
        second: Type,
    },
}

/// Walk `param` and `arg` in parallel, binding any [`Type::TypeParam`]
/// in `param` to the corresponding piece of `arg`. Recurses through
/// [`Type::Nullable`] and [`Type::Array`] payloads. Per ADR 0016 D7c.
fn unify_one(
    param: Type,
    arg: Type,
    subst: &mut [Option<Type>],
    instances: &[GenericInstanceData],
    refs: &[RefData],
) -> Result<(), UnifyFailure> {
    match (param, arg) {
        (Type::TypeParam(id), other) => {
            let idx = id.0 as usize;
            match subst[idx] {
                None => {
                    subst[idx] = Some(other);
                    Ok(())
                }
                Some(existing) if existing == other => Ok(()),
                Some(existing) => Err(UnifyFailure::TypeParamConflict {
                    idx: id.0,
                    first: existing,
                    second: other,
                }),
            }
        }
        (Type::Nullable(p), Type::Nullable(a)) => {
            unify_one(p.to_type(), a.to_type(), subst, instances, refs)
        }
        (Type::Array(p), Type::Array(a)) => {
            // ADR 0068: a nested-array element is interned + structurally dedup'd,
            // so two nested arrays unify iff they are the SAME `ArrayId` (and
            // `ArrayElem::to_type()` can't resolve `Array(_)` without the interner).
            // Generic-nested binding (`[[T]]` vs `[[u8]]`) is deferred (D6). For
            // flat elements, recurse so a TypeParam binds (`[T]` vs `[u8]`).
            if matches!(p, ArrayElem::Array(_)) || matches!(a, ArrayElem::Array(_)) {
                if p == a {
                    Ok(())
                } else {
                    Err(UnifyFailure::Mismatch(param, arg))
                }
            } else {
                unify_one(p.to_type(), a.to_type(), subst, instances, refs)
            }
        }
        // Phase D.3 / ADR 0034: unify a Vec's element so e.g.
        // `vec_new<T>() -> Vec<T>` binds T from the expected `Vec<i64>`,
        // and `push<T>(&mut Vec<T>, T)` binds T from the `&mut Vec<i64>`
        // arg. WITHOUT this explicit arm the `(p, a) if p == a` wildcard
        // below would reject `Vec<T>` vs `Vec<i64>` as a Mismatch and the
        // TypeParam would never bind — the inference path the Vec
        // builtins depend on.
        (Type::Vec(p), Type::Vec(a)) => {
            unify_one(p.to_type(), a.to_type(), subst, instances, refs)
        }
        (Type::GenericInstance(p_id), Type::GenericInstance(a_id)) => {
            // Same instance id → trivially equal.
            if p_id == a_id {
                return Ok(());
            }
            let p_inst = &instances[p_id.0 as usize];
            let a_inst = &instances[a_id.0 as usize];
            // Both must be instances of the same generic struct;
            // otherwise it's a structural mismatch.
            if p_inst.struct_id != a_inst.struct_id || p_inst.args.len() != a_inst.args.len() {
                return Err(UnifyFailure::Mismatch(param, arg));
            }
            for (pa, aa) in p_inst.args.iter().zip(a_inst.args.iter()) {
                unify_one(*pa, *aa, subst, instances, refs)?;
            }
            Ok(())
        }
        (Type::Ref(p_id), Type::Ref(a_id)) => {
            // C2 / ADR 0017 D11: refs unify when their mutability
            // matches and their inner types unify. Recurse so
            // `fn f<T>(x: &T)` called with `&i64` binds T = i64.
            if p_id == a_id {
                return Ok(());
            }
            let p_data = &refs[p_id.0 as usize];
            let a_data = &refs[a_id.0 as usize];
            if p_data.mutable != a_data.mutable {
                return Err(UnifyFailure::Mismatch(param, arg));
            }
            unify_one(p_data.inner, a_data.inner, subst, instances, refs)
        }
        (p, a) if p == a => Ok(()),
        (p, a) => Err(UnifyFailure::Mismatch(p, a)),
    }
}

/// Type-check a fn call per ADR 0016 D4 / D7c / D8a. Handles both
/// non-generic calls (signature.type_params is empty) and generic
/// calls (TypeParams in param / return types). For generic calls,
/// uses an iterative bidirectional inference pass:
///
/// 1. Initialize an empty substitution `subst[TypeParamId → Option<Type>]`.
/// 2. Loop: for each not-yet-typed arg, compute its effective
///    expected type: substitute the param under the current `subst`
///    if every TypeParam in the param is already bound, else use
///    `None`. Skip null literals if their expected is still None
///    (they'll be retried after some other arg has bound the
///    relevant TypeParam).
/// 3. After typing an arg, unify its synthesized type against the
///    param type, refining `subst`.
/// 4. Repeat until all args are typed or progress halts.
/// 5. Final: any unbound TypeParam → [`TypeError::AmbiguousTypeArg`];
///    any untyped arg → AmbiguousNull (existing C1.5 path).
/// 6. Substitute the return type using the final `subst`.
#[allow(clippy::too_many_arguments)]
fn check_call(
    id: FnId,
    callee_span: &Span,
    args: &[ResolvedExpr],
    expected_return: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
    call_span: &Span,
) -> Result<(TypedExprKind, Type), TypeError> {
    let signature = &signatures[id.0 as usize];
    let arity = signature.param_types.len();
    debug_assert_eq!(
        args.len(),
        arity,
        "resolve guarantees arity match for FnId({})",
        id.0
    );

    // D.3 / ADR 0034 D5: `len` is the one builtin overloaded over both
    // `[T]` and `Vec<T>` (both "a collection with a length"). The uniform
    // generic-call path below unifies a single `[T]` param shape, so a
    // `Vec<T>` argument would be rejected as a Mismatch — resolve `len`
    // here instead. Behaviour for an `[T]` argument is preserved exactly,
    // including the `Mismatch` error on a non-collection arg (the
    // `len_on_non_array_errors` test pins `Mismatch { got: i64 }`).
    if id == LEN_FN_ID {
        let typed = check_expr(
            &args[0],
            None,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        let elem = match typed.ty {
            // ADR 0068: resolve a nested-array element via the `arrays` interner.
            Type::Array(ae) => array_elem_type_in(ae, arrays),
            Type::Vec(ve) => ve.to_type(),
            other => {
                return Err(TypeError::Mismatch {
                    expected: Type::Array(ArrayElem::TypeParam(TypeParamId(0))),
                    got: other,
                    span: to_source_span(&args[0].span),
                });
            }
        };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![typed],
                type_args: vec![elem],
            },
            Type::I64,
        ));
    }

    // ADR 0066 M2.3b: the framed-channel-over-pipe builtins are generic over a
    // word-scalar element T (the `Channel<T>`-style minimum, but element-from-
    // context rather than a generic sig — `Process` carries no element type).
    // `process_send(p, v: T)` takes T from the value arg; `process_recv(p) -> ?T`
    // takes T from the expected return type. Codegen encodes/decodes T to/from the
    // 8-byte i64 frame (M1.1). The `i64` case is byte-identical to M2.3. A `secret`
    // / aggregate element is rejected — the cross-process secret fence (D8).
    if id == PROCESS_SEND_FN_ID {
        let p_typed = check_expr(
            &args[0], Some(Type::Process), env, signatures, structs, class_decls, enums,
            instances, refs, secrets, arrays, struct_type_param_counts, effect_decls,
            trait_decls, impl_decls, konts, tasks,
        )?;
        if p_typed.ty != Type::Process {
            return Err(TypeError::CallArgMismatch {
                callee: "process_send".to_string(),
                arg_index: 0,
                expected: Type::Process,
                got: p_typed.ty,
                span: to_source_span(&args[0].span),
            });
        }
        let v_typed = check_expr(
            &args[1], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        if !is_process_channel_elem(v_typed.ty) {
            // A `secret`/aggregate element can't cross the pipe (D8): report it
            // against the public-scalar ABI exactly as M2.3's concrete-i64 sig did
            // (the `c66_process_channel_secret_fence` message is unchanged).
            return Err(TypeError::CallArgMismatch {
                callee: "process_send".to_string(),
                arg_index: 1,
                expected: Type::I64,
                got: v_typed.ty,
                span: to_source_span(&args[1].span),
            });
        }
        // No `type_args`: codegen encodes by LLVM value kind, and an empty
        // `type_args` keeps the `i64` case's dump byte-identical to M2.3 (the
        // non-i64 element is carried by the value arg's own `:T` annotation).
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![p_typed, v_typed],
                type_args: vec![],
            },
            Type::I64,
        ));
    }
    if id == PROCESS_RECV_FN_ID {
        let p_typed = check_expr(
            &args[0], Some(Type::Process), env, signatures, structs, class_decls, enums,
            instances, refs, secrets, arrays, struct_type_param_counts, effect_decls,
            trait_decls, impl_decls, konts, tasks,
        )?;
        if p_typed.ty != Type::Process {
            return Err(TypeError::CallArgMismatch {
                callee: "process_recv".to_string(),
                arg_index: 0,
                expected: Type::Process,
                got: p_typed.ty,
                span: to_source_span(&args[0].span),
            });
        }
        // T from the expected `?T` (default `?i64`). A non-channel-elem expected
        // (e.g. `?u128`) falls back to `i64`; the outer context surfaces a Mismatch.
        let elem = match expected_return {
            Some(Type::Nullable(inner)) if is_process_channel_elem(inner.to_type()) => {
                inner.to_type()
            }
            _ => Type::I64,
        };
        let ret = Type::Nullable(
            elem.to_nullable_inner().expect("a process-channel elem has a NullableInner"),
        );
        // Carry the element in `type_args` ONLY for a non-`i64` element (codegen's
        // decode reads it); the `i64` case emits no `type_args` so its dump stays
        // byte-identical to M2.3 (the differential corpus is i64-only — the generic
        // elements are snc-side demonstrators in `examples/`, scg mirror deferred).
        let type_args = if elem == Type::I64 { vec![] } else { vec![elem] };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![p_typed],
                type_args,
            },
            ret,
        ));
    }
    // ADR 0070 (generalized): `apply(f: Fn<T,R>, x: T) -> R` — context-typed
    // from `f`'s own `Fn<T,R>` signature (the `process_recv` pattern: read
    // the element/shape off the arg's interned/computed id, no type_args
    // needed since codegen derives both LLVM types from the typed AST
    // directly — see `lower_apply`).
    if id == APPLY_FN_ID {
        let f_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums,
            instances, refs, secrets, arrays, struct_type_param_counts, effect_decls,
            trait_decls, impl_decls, konts, tasks,
        )?;
        let sig_id = match f_typed.ty {
            Type::Fn(sig_id) => sig_id,
            other => {
                return Err(TypeError::ApplyTargetNotFn {
                    got: other,
                    span: to_source_span(&args[0].span),
                });
            }
        };
        let (param_ty, ret_ty) = fn_value_sig_param_ret(sig_id);
        // `None`, not `Some(param_ty)`: passing an expected type here would
        // route through `coerce_to_expected`, which emits a generic
        // `Mismatch` on disagreement BEFORE this function ever sees the
        // result — silently pre-empting the more specific
        // `CallArgMismatch` below (the same reasoning `check_handle_expr`
        // already documents for its own arm-body check). `Fn<T,R>`'s
        // param is always a plain word-scalar, never a `coerce_to_expected`
        // widening target (`?T`/`secret T`/`[secret u8]`), so this loses no
        // legitimate coercion.
        let x_typed = check_expr(
            &args[1], None, env, signatures, structs, class_decls, enums,
            instances, refs, secrets, arrays, struct_type_param_counts, effect_decls,
            trait_decls, impl_decls, konts, tasks,
        )?;
        if x_typed.ty != param_ty {
            return Err(TypeError::CallArgMismatch {
                callee: "apply".to_string(),
                arg_index: 1,
                expected: param_ty,
                got: x_typed.ty,
                span: to_source_span(&args[1].span),
            });
        }
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![f_typed, x_typed],
                type_args: vec![],
            },
            ret_ty,
        ));
    }

    // ADR 0066 M1.2b-cont: the in-process channel builtins generalized over a
    // word-scalar element T (the cross-thread twins of `process_send`/`recv` — same
    // encode/decode, but in-memory mpsc, not a pipe). `Channel<T>` carries its element
    // (unlike `Process`), pre-interned at a fixed `ChanId` (`channel_chanid_for`); the
    // element is read back from the channel arg's `ChanId` (`channel_elem_for`) with no
    // `channels`-table threading. The `i64` case is byte-identical to M1.2 (no encode/
    // decode, no `type_args`); generic elements are snc-side demonstrators in
    // `examples/` (scg mirror deferred, like M2.3b).
    if id == CHANNEL_NEW_FN_ID {
        // `channel_new() -> Channel<T>`: T from the expected type (default `Channel<i64>`,
        // the M1.2 ChanId(0)). The element is element-agnostic in codegen (a runtime ptr).
        let chan_ty = match expected_return {
            Some(Type::Channel(cid)) => Type::Channel(cid),
            _ => Type::Channel(ChanId(0)),
        };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![],
                type_args: vec![],
            },
            chan_ty,
        ));
    }
    if id == SEND_FN_ID {
        let ch_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        let elem = match ch_typed.ty {
            Type::Channel(cid) => channel_elem_for(cid),
            _ => {
                return Err(TypeError::CallArgMismatch {
                    callee: "send".to_string(),
                    arg_index: 0,
                    expected: Type::Channel(ChanId(0)),
                    got: ch_typed.ty,
                    span: to_source_span(&args[0].span),
                })
            }
        };
        let v_typed = check_expr(
            &args[1], Some(elem), env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        if v_typed.ty != elem {
            return Err(TypeError::CallArgMismatch {
                callee: "send".to_string(),
                arg_index: 1,
                expected: elem,
                got: v_typed.ty,
                span: to_source_span(&args[1].span),
            });
        }
        // Codegen encodes the element to the i64 slot by LLVM value kind (no `type_args`);
        // the `i64` case emits no encode, byte-identical to M1.2.
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![ch_typed, v_typed],
                type_args: vec![],
            },
            Type::I64,
        ));
    }
    if id == RECV_FN_ID {
        let ch_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        let elem = match ch_typed.ty {
            Type::Channel(cid) => channel_elem_for(cid),
            _ => {
                return Err(TypeError::CallArgMismatch {
                    callee: "recv".to_string(),
                    arg_index: 0,
                    expected: Type::Channel(ChanId(0)),
                    got: ch_typed.ty,
                    span: to_source_span(&args[0].span),
                })
            }
        };
        let ret = Type::Nullable(
            elem.to_nullable_inner().expect("a channel elem has a NullableInner"),
        );
        let type_args = if elem == Type::I64 { vec![] } else { vec![elem] };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![ch_typed],
                type_args,
            },
            ret,
        ));
    }
    if id == CHANNEL_CLOSE_FN_ID {
        let ch_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        if !matches!(ch_typed.ty, Type::Channel(_)) {
            return Err(TypeError::CallArgMismatch {
                callee: "channel_close".to_string(),
                arg_index: 0,
                expected: Type::Channel(ChanId(0)),
                got: ch_typed.ty,
                span: to_source_span(&args[0].span),
            });
        }
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![ch_typed],
                type_args: vec![],
            },
            Type::I64,
        ));
    }
    // ADR 0071 M1.4a: the `Shared<T>` builtins (the `send`/`recv` element pattern).
    if id == SHARED_NEW_FN_ID {
        // `shared_new(v: T) -> Shared<T>`: `T` is the word-scalar element read from
        // the VALUE arg (like `send`). Codegen encodes it into the cell's i64 slot
        // by LLVM value kind (no `type_args`); the `i64` case emits no encode.
        let v_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        let sid = match shared_id_for(v_typed.ty) {
            Some(sid) => sid,
            None => {
                return Err(TypeError::SharedElementNotSupported {
                    span: to_source_span(&args[0].span),
                })
            }
        };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![v_typed],
                type_args: vec![],
            },
            Type::Shared(SharedId(sid)),
        ));
    }
    if id == SHARED_GET_FN_ID {
        // `shared_get(s: Shared<T>) -> T`: the element from the handle's `SharedId`
        // (like `recv`), returned DIRECTLY (a `Shared` is always valid — no `?T`).
        // Codegen decodes the i64 slot into `T`; `type_args=[elem]` when non-i64
        // (the `i64` case emits no decode, byte-identical).
        let s_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        let elem = match s_typed.ty {
            Type::Shared(sid) => shared_elem_for(sid),
            _ => {
                return Err(TypeError::CallArgMismatch {
                    callee: "shared_get".to_string(),
                    arg_index: 0,
                    expected: Type::Shared(SharedId(0)),
                    got: s_typed.ty,
                    span: to_source_span(&args[0].span),
                })
            }
        };
        let type_args = if elem == Type::I64 { vec![] } else { vec![elem] };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![s_typed],
                type_args,
            },
            elem,
        ));
    }
    // ADR 0071 M1.4b: the `Mutex<T>` constructor (mirrors `shared_new`).
    if id == MUTEX_NEW_FN_ID {
        // `mutex_new(v: T) -> Mutex<T>`: `T` is the word-scalar element read from
        // the VALUE arg. Codegen encodes it into the cell's i64 slot by LLVM value
        // kind (no `type_args`); the `i64` case emits no encode. `lock(m)` (2b-ii)
        // reads/writes it through a guard.
        let v_typed = check_expr(
            &args[0], None, env, signatures, structs, class_decls, enums, instances, refs,
            secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls,
            konts, tasks,
        )?;
        let mid = match mutex_id_for(v_typed.ty) {
            Some(mid) => mid,
            None => {
                return Err(TypeError::MutexElementNotSupported {
                    span: to_source_span(&args[0].span),
                })
            }
        };
        return Ok((
            TypedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: vec![v_typed],
                type_args: vec![],
            },
            Type::Mutex(MutexId(mid)),
        ));
    }

    let n_type_params = signature.type_params.len();
    let mut subst: Vec<Option<Type>> = vec![None; n_type_params];
    let mut typed_args: Vec<Option<TypedExpr>> = (0..arity).map(|_| None).collect();

    // If the caller has an expected return type AND the signature's
    // return type is exactly a TypeParam, seed the substitution from
    // it. This is the bidirectional pushdown for generic returns —
    // e.g., `let x: ?i64 = id(null)` pre-binds T = ?i64 so `null`
    // can be typed against the expected param ?T.
    if let (Some(exp), true) = (expected_return, n_type_params > 0) {
        // Seed subst by unifying the signature's return type
        // against the expected. Best-effort — failures here are
        // silent because a real downstream mismatch will surface
        // as ReturnTypeMismatch / Mismatch later.
        let _ = unify_one(signature.return_type, exp, &mut subst, instances, refs);
    }

    // Iterative inference. Each iteration types as many args as
    // possible given the current `subst`. Halts when no further
    // progress can be made.
    loop {
        let mut progressed = false;
        for i in 0..arity {
            if typed_args[i].is_some() {
                continue;
            }
            let param = signature.param_types[i];
            // What expected type can we push down to arg i?
            // Preserve the C1.5 / C1.6 behavior: push the param
            // down only when the *concrete* form is nullable
            // (enables T→?T widening per ADR 0014 D3). Non-nullable
            // concrete params synthesize without pushdown so the
            // more specific `CallArgMismatch` (rather than the
            // generic `Mismatch`) surfaces on shape errors.
            let concrete_param = if contains_type_param(param, instances, refs) {
                try_substitute(param, &subst, instances, refs)
            } else {
                Some(param)
            };
            let arg_expected: Option<Type> = match concrete_param {
                // ADR 0051: push a `secret T` / `[secret u8]` param type down so
                // a public argument is widened by `coerce_to_expected` (like the
                // existing `?T` pushdown). `f(42)` for `f(x: secret i64)` works.
                Some(c) if c.is_nullable() || c.is_secret_widen_target() => Some(c),
                _ => None,
            };
            // Null literals require a concrete expected — skip
            // them until enough subst is built up to push one down.
            if matches!(args[i].kind, ResolvedExprKind::NullLit)
                && arg_expected.is_none()
            {
                continue;
            }
            let typed = check_expr(
                &args[i],
                arg_expected,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            // Validate the arg's type against the param (possibly
            // refining subst with any new TypeParam bindings).
            match unify_one(param, typed.ty, &mut subst, instances, refs) {
                Ok(()) => {}
                Err(UnifyFailure::Mismatch(expected, got)) => {
                    // If the failing param has unbound TypeParams,
                    // surface as `Mismatch` (matches the pre-C1.7
                    // generic-builtin error shape — e.g.,
                    // `unwrap_or(5, 0)` says "expected ?T, got i64").
                    // If the param fully substitutes to a concrete
                    // shape, the more specific `CallArgMismatch`
                    // pinpoints the arg position by callee + index.
                    return match try_substitute(expected, &subst, instances, refs) {
                        Some(concrete) => Err(TypeError::CallArgMismatch {
                            callee: signature.name.clone(),
                            arg_index: i,
                            expected: concrete,
                            got,
                            span: to_source_span(&args[i].span),
                        }),
                        None => Err(TypeError::Mismatch {
                            expected,
                            got,
                            span: to_source_span(&args[i].span),
                        }),
                    };
                }
                Err(UnifyFailure::TypeParamConflict { idx, first, second }) => {
                    let tp_name = signature.type_params[idx as usize].name.clone();
                    return Err(TypeError::TypeArgInferenceConflict {
                        callee: signature.name.clone(),
                        type_param: tp_name,
                        first,
                        second,
                        span: to_source_span(&args[i].span),
                    });
                }
            }
            typed_args[i] = Some(typed);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }

    // Any still-untyped arg means we hit a null literal whose
    // surrounding TypeParam never got bound (only inference path
    // was via the null itself, which is circular).
    for (i, opt) in typed_args.iter().enumerate() {
        if opt.is_none() {
            return Err(TypeError::AmbiguousNull {
                span: to_source_span(&args[i].span),
            });
        }
    }

    // Validate every TypeParam got bound. A param like `fn f<T>(x: i64)
    // -> ?T { null }` could hit this — T appears only in the return.
    // Use the call's overall span for the diagnostic since there's no
    // single arg to blame.
    let mut concrete_type_args: Vec<Type> = Vec::with_capacity(n_type_params);
    for (idx, opt) in subst.iter().enumerate() {
        match opt {
            Some(t) => concrete_type_args.push(*t),
            None => {
                let tp_name = signature.type_params[idx].name.clone();
                return Err(TypeError::AmbiguousTypeArg {
                    callee: signature.name.clone(),
                    type_param: tp_name,
                    span: to_source_span(call_span),
                });
            }
        }
    }

    // Compute the substituted return type. This may extend
    // `instances` (and `refs`) if the return type contains a
    // GenericInstance with TypeParam args (or a ref-of-TypeParam).
    let ret_ty = signature
        .return_type
        .substitute(&concrete_type_args, instances, refs);

    let typed_args: Vec<TypedExpr> =
        typed_args.into_iter().map(|o| o.expect("filled above")).collect();
    Ok((
        TypedExprKind::Call {
            id,
            callee_span: callee_span.clone(),
            args: typed_args,
            type_args: concrete_type_args,
        },
        ret_ty,
    ))
}

#[allow(clippy::too_many_arguments)]
fn check_expr(
    expr: &ResolvedExpr,
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<TypedExpr, TypeError> {
    // C1.5 / ADR 0014 D2: NullLit has no synthesis type — it MUST
    // see an expected `?T` context to type-check.
    if matches!(expr.kind, ResolvedExprKind::NullLit) {
        return match expected {
            Some(Type::Nullable(_)) => Ok(TypedExpr {
                kind: TypedExprKind::NullLit,
                span: expr.span.clone(),
                ty: expected.expect("matched Some"),
            }),
            _ => Err(TypeError::AmbiguousNull {
                span: to_source_span(&expr.span),
            }),
        };
    }

    let (kind, ty) = match &expr.kind {
        ResolvedExprKind::IntLit(n) => (TypedExprKind::IntLit(*n), Type::I64),
        ResolvedExprKind::BoolLit(b) => (TypedExprKind::BoolLit(*b), Type::Bool),
        ResolvedExprKind::NullLit => unreachable!("handled above"),
        // D.2 / ADR 0033 D2 + D3: a char literal IS a `u8` byte; a
        // string literal IS a `[u8]` (the bytes carry verbatim — the
        // array machinery handles len/index/drop/move). No new typing
        // surface beyond the literal's own type.
        // ADR 0058: a float literal always types to `f64` (public — there
        // is no secret float). The bits carry through verbatim.
        ResolvedExprKind::FloatLit(bits) => (TypedExprKind::FloatLit(*bits), Type::F64),
        ResolvedExprKind::CharLit(b) => (TypedExprKind::CharLit(*b), Type::U8),
        ResolvedExprKind::StringLit(bytes) => (
            TypedExprKind::StringLit(bytes.clone()),
            Type::Array(ArrayElem::U8),
        ),
        ResolvedExprKind::Var(id) => {
            let (ty, _mutable) = *env
                .get(id)
                .expect("resolve guarantees VarId is bound in the current scope");
            // C3.4 / ADR 0020 D5: a continuation binding (kont) is
            // only valid in resume-call position. Surfacing it as
            // a value would let the user smuggle it past the
            // handler — reject here. ResumeKont's own check_expr
            // arm constructs a TypedExprKind without going through
            // Var lookup, so this rejection doesn't bite the legal
            // path.
            if matches!(ty, Type::Kont(_)) {
                return Err(TypeError::KontUsedAsValue {
                    span: to_source_span(&expr.span),
                });
            }
            (TypedExprKind::Var(*id), ty)
        }
        // ADR 0070 D4 (generalized): a bare top-level fn name used as a value.
        // Eligible only if the referenced fn is non-generic, not a
        // builtin/runtime symbol, not `main`, not a cross-module extern
        // import, has exactly ONE word-scalar param + a word-scalar return
        // (any pair — `fn_value_sig_id_for`), and an EMPTY effect row (so
        // the `apply` builtin can stay effect-free itself — see ADR 0070
        // D4 for why effecting fn values are deferred).
        ResolvedExprKind::FnRef(fid) => {
            let sig = &signatures[fid.0 as usize];
            let sig_id = (sig.type_params.is_empty()
                && !sig.is_main
                && !sig.is_runtime
                && sig.extern_origin.is_none()
                && sig.param_types.len() == 1
                && sig.effect_row.is_empty())
            .then(|| fn_value_sig_id_for(sig.param_types[0], sig.return_type))
            .flatten();
            let sig_id = match sig_id {
                Some(id) => id,
                None => {
                    return Err(TypeError::FnValueSignatureNotSupported {
                        name: sig.name.clone(),
                        span: to_source_span(&expr.span),
                    });
                }
            };
            (TypedExprKind::FnRef(*fid), Type::Fn(sig_id))
        }
        ResolvedExprKind::Unary(op, inner) => {
            // C2 / ADR 0017 D3 + D4: borrow / deref need
            // structural checks beyond Neg / Not's "operand has
            // the right base type". Dispatch up front.
            match op {
                UnaryOp::Ref | UnaryOp::RefMut => {
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    // Operand must be an lvalue.
                    if !is_lvalue(&inner_t) {
                        return Err(TypeError::BorrowOfRvalue {
                            span: to_source_span(&inner.span),
                        });
                    }
                    let mutable_borrow = matches!(op, UnaryOp::RefMut);
                    if mutable_borrow {
                        // `&mut` requires a mutable lvalue.
                        check_mutable_borrow_target(&inner_t, env, refs)?;
                    }
                    let inner_ty = inner_t.ty;
                    // Reject `&&T` at the type level — though the
                    // lexer + parser typically rule this out via
                    // longest-match (`&&` lexes as logical-and).
                    if inner_ty.is_ref() {
                        return Err(TypeError::NestedRef {
                            span: to_source_span(&expr.span),
                        });
                    }
                    let id = intern_ref(refs, mutable_borrow, inner_ty);
                    (
                        TypedExprKind::Unary(*op, Box::new(inner_t)),
                        Type::Ref(id),
                    )
                }
                UnaryOp::Deref => {
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    let inner_ty = match inner_t.ty {
                        Type::Ref(id) => refs[id.0 as usize].inner,
                        // C3 / ADR 0019 D7 (C3.1) — SecretInRefDeref:
                        // dereferencing a `secret &T` (secret
                        // pointer) leaks via the memory side
                        // channel. Reject. Note: `& secret T`
                        // (pointer to secret) is allowed and goes
                        // through the Ref(id) arm above with
                        // inner = Secret(_).
                        Type::Secret(sid) => {
                            if matches!(secrets[sid.0 as usize].inner, Type::Ref(_)) {
                                return Err(TypeError::SecretInRefDeref {
                                    span: to_source_span(&inner.span),
                                });
                            }
                            return Err(TypeError::DerefOfNonRef {
                                got: inner_t.ty,
                                span: to_source_span(&inner.span),
                            });
                        }
                        other => {
                            return Err(TypeError::DerefOfNonRef {
                                got: other,
                                span: to_source_span(&inner.span),
                            });
                        }
                    };
                    (TypedExprKind::Unary(*op, Box::new(inner_t)), inner_ty)
                }
                UnaryOp::Neg | UnaryOp::Not => {
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    // C3 / ADR 0019 D5 (C3.1b): unary preserves
                    // the secret qualifier — `-secret_x` is
                    // `secret int`, `!secret_bool` is `secret bool`.
                    let (inner_unwrapped, inner_secret) =
                        inner_t.ty.strip_secret(secrets);
                    let ty = match op {
                        UnaryOp::Neg => {
                            // ADR 0058: unary `-` on `f64` lowers to `fneg`.
                            // `f64` is public-only, so the secret branch never
                            // fires for it (it can't be secret).
                            if !inner_unwrapped.is_int() && !inner_unwrapped.is_float() {
                                return Err(TypeError::Mismatch {
                                    expected: Type::I64,
                                    got: inner_t.ty,
                                    span: to_source_span(&inner.span),
                                });
                            }
                            if inner_secret {
                                Type::Secret(intern_secret(secrets, inner_unwrapped))
                            } else {
                                inner_unwrapped
                            }
                        }
                        UnaryOp::Not => {
                            if inner_unwrapped != Type::Bool {
                                return Err(TypeError::Mismatch {
                                    expected: Type::Bool,
                                    got: inner_t.ty,
                                    span: to_source_span(&inner.span),
                                });
                            }
                            if inner_secret {
                                Type::Secret(intern_secret(secrets, Type::Bool))
                            } else {
                                Type::Bool
                            }
                        }
                        _ => unreachable!(),
                    };
                    (TypedExprKind::Unary(*op, Box::new(inner_t)), ty)
                }
                UnaryOp::Sqrt => {
                    // ADR 0058: `sqrt(x)` — the operand must be `f64`; the
                    // result is `f64` (lowers to `llvm.sqrt.f64`). `f64` is
                    // public-only, so there is no secret variant to handle.
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    if !matches!(inner_t.ty, Type::F64) {
                        return Err(TypeError::Mismatch {
                            expected: Type::F64,
                            got: inner_t.ty,
                            span: to_source_span(&inner.span),
                        });
                    }
                    (TypedExprKind::Unary(*op, Box::new(inner_t)), Type::F64)
                }
                UnaryOp::PtrOf | UnaryOp::PtrOfMut => {
                    // ADR 0057 Phase 1b: `ptr_of(&[u8])` / `ptr_of_mut(&mut [u8])`
                    // take a borrow of a PUBLIC byte array and produce the opaque
                    // `ptr` (the buffer's `data` field) for an `extern` call.
                    // `ptr_of_mut` requires a `&mut` borrow (so C may write
                    // through it). A `&[secret u8]` is rejected — the FFI fence:
                    // a secret buffer's pointer may not cross to unverified C.
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    let (rd_inner, rd_mutable) = match inner_t.ty {
                        Type::Ref(id) => (refs[id.0 as usize].inner, refs[id.0 as usize].mutable),
                        _ => {
                            return Err(TypeError::PtrOfArg {
                                span: to_source_span(&inner.span),
                            });
                        }
                    };
                    // The referent must be a PUBLIC `[u8]` (a `[secret u8]` has an
                    // `ArrayElem::Secret` element, so this rejects it — the fence).
                    if !matches!(rd_inner, Type::Array(ArrayElem::U8)) {
                        return Err(TypeError::PtrOfArg {
                            span: to_source_span(&inner.span),
                        });
                    }
                    // `ptr_of_mut` needs an exclusive (`&mut`) borrow.
                    if matches!(op, UnaryOp::PtrOfMut) && !rd_mutable {
                        return Err(TypeError::PtrOfArg {
                            span: to_source_span(&inner.span),
                        });
                    }
                    (TypedExprKind::Unary(*op, Box::new(inner_t)), Type::Ptr)
                }
                UnaryOp::IsNull => {
                    // ADR 0057 Phase 1b: `is_null(p)` — the operand must be a
                    // `ptr` (an FFI handle); the result is a public `bool`. Lets a
                    // wrapper null-check an FFI return before reading through it.
                    let inner_t = check_expr(
                        inner,
                        None,
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    if !matches!(inner_t.ty, Type::Ptr) {
                        return Err(TypeError::Mismatch {
                            expected: Type::Ptr,
                            got: inner_t.ty,
                            span: to_source_span(&inner.span),
                        });
                    }
                    (TypedExprKind::Unary(*op, Box::new(inner_t)), Type::Bool)
                }
            }
        }
        ResolvedExprKind::Binary(op, lhs, rhs) => {
            let l = check_expr(lhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let r = check_expr(rhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            // C3 / ADR 0019 D5 + D7 (C3.1b): operator-secret-
            // preserving. Strip one layer of `secret` from both
            // sides, run the usual int-type check on the inners,
            // and re-wrap the result if both were secret. Mixing
            // public + secret operands surfaces as Mismatch (the
            // "SecretFlow" semantics from Phase B ADR 0008 D4).
            // `l.ty != r.ty` below handles the SecretFlow-via-
            // Mismatch case (mixed); we only need the l-side
            // wrappers plus the r_secret flag for SecretDivisor.
            let (l_inner, l_secret) = l.ty.strip_secret(secrets);
            let (r_inner, r_secret) = r.ty.strip_secret(secrets);
            // C1.3: arithmetic requires the LEFT operand be an int
            // type (I32 / I64 / U8); result is that int type. Bool /
            // struct arithmetic is rejected. ADR 0058: `f64` is also a
            // valid arithmetic operand (handled by the float branch below).
            if !l_inner.is_int() && !l_inner.is_float() {
                return Err(TypeError::Mismatch {
                    expected: Type::I64,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            let ty;
            let (l, r) = if l_inner.is_float() {
                // ADR 0058: float arithmetic — `+ - * /` only, lowering to
                // `fadd`/`fsub`/`fmul`/`fdiv`. Bitwise (`& | ^`) and shifts
                // (`<< >>`) are rejected (FloatBitwise). `f64` is public-only
                // (no `secret f64`), so none of the secret machinery (widen,
                // SecretDivisor, SecretShiftAmount) applies. Both operands
                // must be the same type — a mixed `f64`/int operand surfaces
                // as `Mismatch` (no implicit int↔float promotion; use `as`).
                if !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div) {
                    return Err(TypeError::FloatBitwise {
                        span: to_source_span(&expr.span),
                    });
                }
                if l.ty != r.ty {
                    return Err(TypeError::Mismatch {
                        expected: l.ty,
                        got: r.ty,
                        span: to_source_span(&rhs.span),
                    });
                }
                ty = l_inner;
                (l, r)
            } else if matches!(op, BinOp::Shl | BinOp::Shr) {
                // ADR 0048: shifts are ASYMMETRIC (value `<<` amount) and do
                // NOT obey the matching-secrecy rule the other ops enforce via
                // `l.ty != r.ty`. The amount may be any integer width/secrecy;
                // the result takes the VALUE (left) operand's secrecy alone, so
                // `secret i32 << 16` is accepted and types to `secret i32`. The
                // amount is NEVER widened to secret (ADR 0051) — that would trip
                // SecretShiftAmount.
                if !r_inner.is_int() {
                    return Err(TypeError::Mismatch {
                        expected: Type::I64,
                        got: r.ty,
                        span: to_source_span(&rhs.span),
                    });
                }
                // A SECRET amount is a variable-time shift — a timing leak,
                // exactly like a secret divisor. Reject (the MIR `secret_leak`
                // pass is the backstop). A secret VALUE shifted by a public
                // amount is constant-time and accepted.
                if r_secret {
                    return Err(TypeError::SecretShiftAmount {
                        span: to_source_span(&rhs.span),
                    });
                }
                ty = if l_secret {
                    Type::Secret(intern_secret(secrets, l_inner))
                } else {
                    l_inner
                };
                (l, r)
            } else {
                // The symmetric ops (`+ - *` + bitwise; `/` handled below):
                // both operands the same int type. ADR 0051: if the inner int
                // types match but exactly one operand is secret, WIDEN the
                // public operand to secret (a monotone, codegen-no-op re-tag) so
                // `secret_x + 5` needs no pre-bound secret constant. `/` is
                // EXCLUDED from the widen — its divisor must stay public
                // (SecretDivisor below); different int WIDTHS still mismatch.
                let result_secret = l_secret || r_secret;
                let (l, r) = if !matches!(op, BinOp::Div)
                    && l_inner == r_inner
                    && l_secret != r_secret
                {
                    if l_secret {
                        (l, widen_operand_to_secret(r, r_inner, secrets))
                    } else {
                        (widen_operand_to_secret(l, l_inner, secrets), r)
                    }
                } else {
                    (l, r)
                };
                // Mixing different int WIDTHS (or `secret / public`) surfaces as
                // Mismatch (the "SecretFlow" semantics from Phase B ADR 0008 D4).
                if l.ty != r.ty {
                    return Err(TypeError::Mismatch {
                        expected: l.ty,
                        got: r.ty,
                        span: to_source_span(&rhs.span),
                    });
                }
                // C3 / ADR 0019 D7 (C3.1b) — SecretDivisor: variable-time `/`
                // on a secret divisor leaks the divisor's bit pattern via
                // timing. Reject. `secret a / secret b` hits this; `a / b`
                // (both public) is fine.
                if matches!(op, BinOp::Div) && r_secret {
                    return Err(TypeError::SecretDivisor {
                        span: to_source_span(&rhs.span),
                    });
                }
                // Result is secret iff EITHER operand was secret (the public one
                // was widened above when they differed).
                ty = if result_secret {
                    Type::Secret(intern_secret(secrets, l_inner))
                } else {
                    l_inner
                };
                (l, r)
            };
            (
                TypedExprKind::Binary(*op, Box::new(l), Box::new(r)),
                ty,
            )
        }
        ResolvedExprKind::Cmp(op, lhs, rhs) => {
            // ADR 0014 D7: equality against `null` requires special
            // handling. If one side is NullLit and the other types
            // to `?T`, the result is bool. The Cmp must be Eq/Ne for
            // null comparisons; <, <=, >, >= on nullables are rejected.
            let lhs_is_null = matches!(lhs.kind, ResolvedExprKind::NullLit);
            let rhs_is_null = matches!(rhs.kind, ResolvedExprKind::NullLit);
            if lhs_is_null || rhs_is_null {
                if !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                    return Err(TypeError::Mismatch {
                        expected: Type::Bool,
                        got: Type::Bool, // placeholder; real issue is op shape
                        span: to_source_span(&expr.span),
                    });
                }
                // The non-null side determines the expected ?T type.
                // First, synthesize the non-null side.
                let (non_null_side, non_null_expr, null_span) = if lhs_is_null {
                    let r = check_expr(rhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
                    (r.ty, r, lhs.span.clone())
                } else {
                    let l = check_expr(lhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
                    (l.ty, l, rhs.span.clone())
                };
                // Non-null side must be Nullable for null-comparison.
                if !non_null_side.is_nullable() {
                    return Err(TypeError::Mismatch {
                        expected: Type::Nullable(NullableInner::I64), // hint
                        got: non_null_side,
                        span: to_source_span(if lhs_is_null { &rhs.span } else { &lhs.span }),
                    });
                }
                let null_typed = TypedExpr {
                    kind: TypedExprKind::NullLit,
                    span: null_span,
                    ty: non_null_side,
                };
                let (l, r) = if lhs_is_null {
                    (null_typed, non_null_expr)
                } else {
                    (non_null_expr, null_typed)
                };
                return Ok(TypedExpr {
                    kind: TypedExprKind::Cmp(*op, Box::new(l), Box::new(r)),
                    span: expr.span.clone(),
                    ty: Type::Bool,
                });
            }
            let l = check_expr(lhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let r = check_expr(rhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            // C3 / ADR 0019 D5 (C3.1b): operator-secret-preserving
            // for comparisons. `secret T == secret T -> secret bool`
            // per Phase B ADR 0008 D4. Strip wrappers to check inner
            // compatibility; the result is `secret bool` iff either side
            // is secret.
            let (l_inner, l_secret) = l.ty.strip_secret(secrets);
            let (r_inner, r_secret) = r.ty.strip_secret(secrets);
            // C1.3: comparisons require both operands the same type.
            // C1.4 + C1.5 keep this as int + bool only (ADR 0013 D6
            // defers struct equality; nullable-vs-nullable equality
            // also deferred). Reject struct + nullable operands when
            // neither side is null.
            if l_inner.is_struct() || l_inner.is_nullable() {
                return Err(TypeError::Mismatch {
                    expected: Type::I64,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            // ADR 0051: widen a public operand to secret when the other is
            // secret and the inner types match, so `secret_x == 5` type-checks
            // to `secret bool` without a pre-bound secret constant. (The result
            // is secret, so it still cannot reach an `if` — SecretBranch.)
            let result_secret = l_secret || r_secret;
            let (l, r) = if l_inner == r_inner && l_secret != r_secret {
                if l_secret {
                    (l, widen_operand_to_secret(r, r_inner, secrets))
                } else {
                    (widen_operand_to_secret(l, l_inner, secrets), r)
                }
            } else {
                (l, r)
            };
            if l.ty != r.ty {
                return Err(TypeError::Mismatch {
                    expected: l.ty,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            let ty = if result_secret {
                Type::Secret(intern_secret(secrets, Type::Bool))
            } else {
                Type::Bool
            };
            (
                TypedExprKind::Cmp(*op, Box::new(l), Box::new(r)),
                ty,
            )
        }
        ResolvedExprKind::Logic(op, lhs, rhs) => {
            let l = check_expr(lhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let r = check_expr(rhs, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            // C3 / ADR 0019 D5 (C3.1b): operator-secret-preserving
            // for logicals. Both operands must be the same bool-
            // shape (`bool` or `secret bool`); result preserves
            // the secret qualifier.
            let (l_inner, l_secret) = l.ty.strip_secret(secrets);
            let (r_inner, r_secret) = r.ty.strip_secret(secrets);
            if l_inner != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            if r_inner != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            // Mixed secret-then-public surfaces as Mismatch (the
            // SecretFlow path from Phase B ADR 0008 D4 — public
            // and secret can't merge without explicit declassify
            // / widening).
            if l_secret != r_secret {
                return Err(TypeError::Mismatch {
                    expected: l.ty,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            let ty = if l_secret {
                Type::Secret(intern_secret(secrets, Type::Bool))
            } else {
                Type::Bool
            };
            (
                TypedExprKind::Logic(*op, Box::new(l), Box::new(r)),
                ty,
            )
        }
        ResolvedExprKind::Block(b) => {
            let typed_block = check_block(b, expected, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let ty = typed_block.ty;
            (TypedExprKind::Block(Box::new(typed_block)), ty)
        }
        ResolvedExprKind::If { cond, then_branch, else_branch } => {
            let cond_t = check_expr(cond, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            // C3 / ADR 0019 D7 (C3.1) — SecretBranch: an
            // `if` condition with type `secret bool` would
            // leak via timing. Reject before the generic
            // Mismatch path.
            if let Type::Secret(sid) = cond_t.ty {
                if secrets[sid.0 as usize].inner == Type::Bool {
                    return Err(TypeError::SecretBranch {
                        kw: "if",
                        span: to_source_span(&cond.span),
                    });
                }
            }
            // C1.3 step 5: if-condition must be bool.
            if cond_t.ty != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: cond_t.ty,
                    span: to_source_span(&cond.span),
                });
            }
            // ADR 0014 D5: push the expected type down into both
            // branches so `null` in either branch can resolve.
            let then_t = check_block(then_branch, expected, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let else_t = check_block(else_branch, expected, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            // ADR 0065: a branch that always diverges (an early `return`)
            // yields no value, so it does not constrain the join — the `if`'s
            // type is the OTHER branch's. If both diverge, the `if` diverges
            // too (its `ty` is a placeholder, treated as bottom by parents).
            let then_div = block_diverges(&then_t);
            let else_div = block_diverges(&else_t);
            let ty = if then_div && else_div {
                then_t.ty
            } else if then_div {
                else_t.ty
            } else if else_div {
                then_t.ty
            } else {
                if then_t.ty != else_t.ty {
                    return Err(TypeError::Mismatch {
                        expected: then_t.ty,
                        got: else_t.ty,
                        span: to_source_span(&else_branch.span),
                    });
                }
                then_t.ty
            };
            (
                TypedExprKind::If {
                    cond: Box::new(cond_t),
                    then_branch: Box::new(then_t),
                    else_branch: Box::new(else_t),
                },
                ty,
            )
        }
        ResolvedExprKind::Call { id, callee_span, args } => check_call(
            *id,
            callee_span,
            args,
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
            &expr.span,
        )?,
        ResolvedExprKind::StructLit { id, name, name_span, fields } => {
            // The struct decl provides the expected field set + types.
            let decl = &structs[id.0 as usize];

            // C1.7.4b / ADR 0016 D6: if the decl is generic, the
            // literal needs type arguments to pin field types. We
            // recover them from the expected type (a `let p: Pair
            // <i64, i64> = Pair { ... }` context) or, failing that,
            // surface `AmbiguousGenericStructLit`.
            let (instance_id, type_args): (
                Option<GenericInstanceId>,
                Vec<Type>,
            ) = if decl.type_params.is_empty() {
                (None, Vec::new())
            } else {
                match expected {
                    Some(Type::GenericInstance(gi_id)) => {
                        // The expected instance's struct_id must
                        // match this literal's struct id; if not,
                        // standard `Mismatch` later catches it.
                        let inst = &instances[gi_id.0 as usize];
                        if inst.struct_id != *id {
                            return Err(TypeError::AmbiguousGenericStructLit {
                                struct_name: decl.name.clone(),
                                span: to_source_span(name_span),
                            });
                        }
                        (Some(gi_id), inst.args.clone())
                    }
                    _ => {
                        return Err(TypeError::AmbiguousGenericStructLit {
                            struct_name: decl.name.clone(),
                            span: to_source_span(name_span),
                        });
                    }
                }
            };

            // Type-check each provided field's value.
            let mut provided: Vec<(usize, TypedExpr)> = Vec::with_capacity(fields.len());
            for fi in fields {
                // Find the field's index in the declaration.
                let decl_idx = decl
                    .fields
                    .iter()
                    .position(|df| df.name == fi.name)
                    .ok_or_else(|| TypeError::UnknownField {
                        struct_name: decl.name.clone(),
                        field: fi.name.clone(),
                        span: to_source_span(&fi.name_span),
                    })?;
                // Field type, substituted by the instance's type-
                // args when the struct is generic.
                let raw_field_ty = decl.fields[decl_idx].ty;
                let expected_field_ty = if type_args.is_empty() {
                    raw_field_ty
                } else {
                    raw_field_ty.substitute(&type_args, instances, refs)
                };
                // ADR 0014 D5: push the field's expected type down so
                // `null` / widening work inside struct literals.
                let value_t = check_expr(
                    &fi.value,
                    Some(expected_field_ty),
                    env,
                    signatures,
                    structs,
                    class_decls,
                    enums,
                    instances,
                    refs,
                    secrets,
                    arrays,
                    struct_type_param_counts,
                    effect_decls,
                    trait_decls,
                    impl_decls,
                    konts,
                    tasks,
                )?;
                if value_t.ty != expected_field_ty {
                    return Err(TypeError::Mismatch {
                        expected: expected_field_ty,
                        got: value_t.ty,
                        span: to_source_span(&fi.value.span),
                    });
                }
                provided.push((decl_idx, value_t));
            }

            if provided.len() < decl.fields.len() {
                for df in &decl.fields {
                    if !provided
                        .iter()
                        .any(|(idx, _)| decl.fields[*idx].name == df.name)
                    {
                        return Err(TypeError::MissingField {
                            struct_name: decl.name.clone(),
                            field: df.name.clone(),
                            span: to_source_span(name_span),
                        });
                    }
                }
            }

            provided.sort_by_key(|(idx, _)| *idx);
            let mut by_index: Vec<Option<TypedExpr>> = vec![None; decl.fields.len()];
            for (idx, val) in provided {
                by_index[idx] = Some(val);
            }
            let ordered: Vec<TypedExpr> = by_index
                .into_iter()
                .map(|opt| opt.expect("every field was provided"))
                .collect();

            let result_ty = match instance_id {
                Some(gi_id) => Type::GenericInstance(gi_id),
                None => Type::Struct(*id),
            };
            (
                TypedExprKind::StructLit {
                    id: *id,
                    name: name.clone(),
                    name_span: name_span.clone(),
                    fields: ordered,
                },
                result_ty,
            )
        }
        ResolvedExprKind::ArrayLit(elems) => {
            // ADR 0015 D2 + D7: each element checked against the
            // expected element type (from `[T]` context) if present.
            // If no expected, infer T from the first element.
            let expected_elem: Option<Type> = match expected {
                // ADR 0068: resolve a nested expected element via the `arrays` interner.
                Some(Type::Array(elem)) => Some(array_elem_type_in(elem, arrays)),
                _ => None,
            };
            if elems.is_empty() {
                // ADR 0015 D5: empty array needs context.
                let elem_ty = match expected_elem {
                    Some(t) => t,
                    None => {
                        return Err(TypeError::AmbiguousEmptyArray {
                            span: to_source_span(&expr.span),
                        });
                    }
                };
                let ae = array_elem_of(elem_ty, secrets, arrays).ok_or_else(|| {
                    TypeError::NestedArray {
                        span: to_source_span(&expr.span),
                    }
                })?;
                (
                    TypedExprKind::ArrayLit {
                        elem_ty: ae,
                        elements: Vec::new(),
                    },
                    Type::Array(ae),
                )
            } else {
                // Type-check elements. First element synthesizes T
                // if no expected; subsequent elements get T as
                // expected (so widening / null work inside `[T]`).
                let first = check_expr(&elems[0], expected_elem, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
                let elem_ty = first.ty;
                let mut typed = Vec::with_capacity(elems.len());
                typed.push(first);
                for e in &elems[1..] {
                    let t = check_expr(
                        e,
                        Some(elem_ty),
                        env,
                        signatures,
                        structs,
                        class_decls,
                        enums,
                        instances,
                        refs,
                        secrets,
                        arrays,
                        struct_type_param_counts,
                        effect_decls,
                        trait_decls,
                        impl_decls,
                        konts,
                        tasks,
                    )?;
                    if t.ty != elem_ty {
                        return Err(TypeError::Mismatch {
                            expected: elem_ty,
                            got: t.ty,
                            span: to_source_span(&e.span),
                        });
                    }
                    typed.push(t);
                }
                let ae = array_elem_of(elem_ty, secrets, arrays).ok_or_else(|| {
                    TypeError::NestedArray {
                        span: to_source_span(&expr.span),
                    }
                })?;
                (
                    TypedExprKind::ArrayLit {
                        elem_ty: ae,
                        elements: typed,
                    },
                    Type::Array(ae),
                )
            }
        }
        ResolvedExprKind::Index { target, index } => {
            let target_t = check_expr(target, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let elem_ty = match target_t.ty {
                Type::Array(ae) => ae,
                // Phase D.3 (2/N) / ADR 0034 D5: `v[i]` on a `Vec<T>`
                // reuses the C1.6 bounds-checked Index. `VecElem` is the
                // same flat subset as `ArrayElem`, so the element demotes
                // to an `ArrayElem` for the typed node (codegen reads the
                // data pointer from the Vec's field 2 vs the array's
                // field 1, keyed on the target type). ADR 0052: the demote
                // is secret-aware so `v[i]` on a `Vec<secret u8>` yields
                // `ArrayElem::Secret` (→ `secret u8`, seeding the CT check);
                // `VecElem::Secret` only ever wraps a scalar, so the demote
                // is guaranteed `Some`.
                Type::Vec(ve) => ve
                    .to_type()
                    .to_array_elem_secret(secrets)
                    .expect("VecElem variants are a (secret-aware) subset of ArrayElem"),
                other => {
                    return Err(TypeError::IndexOnNonArray {
                        got: other,
                        span: to_source_span(&target.span),
                    });
                }
            };
            // Synthesize the index without pushdown so a non-int
            // index surfaces as the more-specific `IndexNotInt`
            // rather than a generic `Mismatch`.
            let index_t = check_expr(index, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            if index_t.ty != Type::I64 {
                return Err(TypeError::IndexNotInt {
                    got: index_t.ty,
                    span: to_source_span(&index.span),
                });
            }
            (
                TypedExprKind::Index {
                    target: Box::new(target_t),
                    index: Box::new(index_t),
                    elem_ty,
                },
                // ADR 0068: a nested-array index yields the inner array `[T]`.
                array_elem_type_in(elem_ty, arrays),
            )
        }
        ResolvedExprKind::FieldAccess { target, field, field_span } => {
            let target_t = check_expr(
                target,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            // C1.7.4b / ADR 0016 D6: field access on a generic
            // instance substitutes the field type by the instance's
            // type-args. C4.1 / ADR 0022 D6: field access also
            // works on `Type::Class` — class fields share the
            // same lookup machinery as struct fields. Resolves a
            // (field_index, raw_field_ty) tuple per target type.
            let (field_index, raw_field_ty, type_args): (usize, Type, Vec<Type>) =
                match target_t.ty {
                    Type::Struct(id) => {
                        let decl = &structs[id.0 as usize];
                        let (i, ty) = decl
                            .fields
                            .iter()
                            .enumerate()
                            .find_map(|(i, df)| {
                                if df.name == *field {
                                    Some((i, df.ty))
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| TypeError::UnknownField {
                                struct_name: decl.name.clone(),
                                field: field.clone(),
                                span: to_source_span(field_span),
                            })?;
                        (i, ty, Vec::new())
                    }
                    Type::GenericInstance(gi_id) => {
                        let inst = &instances[gi_id.0 as usize];
                        let decl = &structs[inst.struct_id.0 as usize];
                        let (i, ty) = decl
                            .fields
                            .iter()
                            .enumerate()
                            .find_map(|(i, df)| {
                                if df.name == *field {
                                    Some((i, df.ty))
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| TypeError::UnknownField {
                                struct_name: decl.name.clone(),
                                field: field.clone(),
                                span: to_source_span(field_span),
                            })?;
                        (i, ty, inst.args.clone())
                    }
                    Type::Class(cid) => {
                        let decl = &class_decls[cid.0 as usize];
                        let (i, cf) = decl.field(field).ok_or_else(|| TypeError::UnknownField {
                            struct_name: decl.name.clone(),
                            field: field.clone(),
                            span: to_source_span(field_span),
                        })?;
                        (i, cf.ty, Vec::new())
                    }
                    other => {
                        return Err(TypeError::FieldAccessOnNonStruct {
                            got: other,
                            span: to_source_span(&target.span),
                        });
                    }
                };
            let field_ty = if type_args.is_empty() {
                raw_field_ty
            } else {
                raw_field_ty.substitute(&type_args, instances, refs)
            };
            (
                TypedExprKind::FieldAccess {
                    target: Box::new(target_t),
                    field: field.clone(),
                    field_span: field_span.clone(),
                    field_index,
                },
                field_ty,
            )
        }
        ResolvedExprKind::Declassify(inner) => {
            // C3 / ADR 0019 D6 (C3.1): `declassify(e)` strips one
            // layer of `secret T` from the inner. Idempotent on
            // non-secret inputs per Phase B ADR 0008 D5.
            let inner_t = check_expr(
                inner,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            let (stripped, _was_secret) = inner_t.ty.strip_secret(secrets);
            (
                TypedExprKind::Declassify(Box::new(inner_t)),
                stripped,
            )
        }
        ResolvedExprKind::Return(inner) => {
            // ADR 0065: `return expr` — an early return. Check the inner
            // against the enclosing function's declared return type, with the
            // SAME expected-type pushdown the body tail uses (nullable /
            // generic-instance / Vec / secret-widen returns need the context;
            // primitives synthesise and compare). The `Return` node is
            // divergent — its `ty` is a placeholder treated as bottom at join
            // sites (see `expr_diverges`).
            let ret_ty = env.current_return_type;
            let inner_expected = match ret_ty {
                Some(rt)
                    if rt.is_nullable()
                        || rt.is_generic_instance()
                        || rt.is_vec()
                        || rt.is_secret_widen_target() =>
                {
                    Some(rt)
                }
                _ => None,
            };
            let inner_t = check_expr(
                inner,
                inner_expected,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            if let Some(rt) = ret_ty {
                if inner_t.ty != rt {
                    return Err(TypeError::Mismatch {
                        expected: rt,
                        got: inner_t.ty,
                        span: to_source_span(&inner.span),
                    });
                }
            }
            // ADR 0071 M1.4a slice 3: an early `return <named Shared>` transfers a
            // refcount unit — guarded until the slice-3b transfer exemption is
            // mirrored into the oracle + scg (return `shared_new(...)` directly).
            if is_named_shared_return(&inner_t) {
                return Err(TypeError::SharedReturnNotSupported {
                    span: to_source_span(&inner.span),
                });
            }
            let ty = inner_t.ty;
            (TypedExprKind::Return(Box::new(inner_t)), ty)
        }
        ResolvedExprKind::Cast(inner, te) => {
            // ADR 0049: `expr as T` — an integer width conversion. The operand
            // must be an integer; the target must be a plain integer type
            // (i64 / i32 / u8). The result takes the operand's secrecy (a
            // width conversion is data-independent / constant-time, so casting
            // a secret is allowed and stays secret — NOT a constant-time sink).
            let inner_t = check_expr(
                inner,
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            let (stripped, was_secret) = inner_t.ty.strip_secret(secrets);
            // ADR 0049 + ADR 0058: the operand must be an integer OR a float
            // (`i64 as f64` / `f64 as i64` move between the domains via
            // `sitofp`/`fptosi`, the ONLY bridge — there is no implicit
            // conversion).
            if !stripped.is_int() && !stripped.is_float() {
                return Err(TypeError::NonIntegerCast {
                    span: to_source_span(&inner.span),
                });
            }
            // The target is restricted to a plain integer or float type ident.
            let target = match &te.kind {
                TypeExprKind::Ident(name) => match name.as_str() {
                    "i64" => Type::I64,
                    "i32" => Type::I32,
                    "u8" => Type::U8,
                    "u128" => Type::U128,
                    // ADR 0058: `x as f64` (`sitofp`/`uitofp`); the result is
                    // PUBLIC — see the secret rejection below.
                    "f64" => Type::F64,
                    _ => {
                        return Err(TypeError::NonIntegerCast {
                            span: to_source_span(&te.span),
                        })
                    }
                },
                _ => {
                    return Err(TypeError::NonIntegerCast {
                        span: to_source_span(&te.span),
                    })
                }
            };
            // ADR 0058 (the fence): casting a `secret` value to `f64` would
            // create a `secret f64`, which does not exist. Reject — the user
            // must `declassify` first if they intend a public float. (A
            // secret→secret-int cast stays secret as before; only the f64
            // target is fenced.)
            if matches!(target, Type::F64) && was_secret {
                return Err(TypeError::SecretFloat {
                    span: to_source_span(&expr.span),
                });
            }
            let result_ty = if was_secret {
                Type::Secret(intern_secret(secrets, target))
            } else {
                target
            };
            (TypedExprKind::Cast(Box::new(inner_t)), result_ty)
        }
        ResolvedExprKind::Handle { body, arms, return_arm } => check_handle_expr(
            body,
            arms,
            return_arm.as_deref(),
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
            &expr.span,
        )?,
        ResolvedExprKind::Perform {
            effect_id,
            op_index,
            effect_name,
            effect_span: _,
            op_name,
            op_span,
            args,
        } => check_perform_expr(
            *effect_id,
            *op_index,
            effect_name,
            op_name,
            op_span,
            args,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?,
        ResolvedExprKind::ResumeKont { kont, callee_span, args } => check_resume_kont_expr(
            *kont,
            callee_span,
            args,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?,
        ResolvedExprKind::MethodCall { target, method, method_span, args } => {
            check_method_call_expr(
                target,
                method,
                method_span,
                args,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?
        }
        ResolvedExprKind::ClassInit { id, name, name_span, args } => {
            check_class_init_expr(
                *id,
                name,
                name_span,
                args,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?
        }
        ResolvedExprKind::QualifiedCall {
            impl_id,
            method_index,
            impl_name,
            method,
            method_span,
            args,
            ..
        } => check_qualified_call_expr(
            *impl_id,
            *method_index,
            impl_name,
            method,
            method_span,
            args,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?,
        // C4.4 / ADR 0024 D1: `scope concurrent { ... }` types as
        // its body's tail (like a plain block). The concurrency
        // contract (auto-await on exit) is a codegen concern.
        ResolvedExprKind::Scope { mode, body } => {
            let typed_block = check_block(body, expected, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let ty = typed_block.ty;
            (
                TypedExprKind::Scope { mode: *mode, body: Box::new(typed_block) },
                ty,
            )
        }
        // C4.4 / ADR 0024 D2 + ADR 0066 M1.1: `spawn fn(args)` — the
        // target must be a direct call (D2). ADR 0066 M1.1 lifts the
        // ADR 0024 D7 `Task<i64>`-only restriction: the result type may
        // be any word-sized scalar, and the result is `Task<result_ty>`.
        // Each spawned-fn argument must also be a word-sized scalar (the
        // per-spawn wrapper packs each into an 8-byte slot); aggregates +
        // `secret` are deferred (`is_spawn_word_scalar`).
        ResolvedExprKind::Spawn { call_expr } => {
            if !matches!(call_expr.kind, ResolvedExprKind::Call { .. }) {
                return Err(TypeError::SpawnMustBeCall {
                    span: to_source_span(&call_expr.span),
                });
            }
            let typed_call = check_expr(call_expr, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            if !is_spawn_word_scalar(typed_call.ty) {
                return Err(TypeError::SpawnTypeUnsupported {
                    got: typed_call.ty,
                    role: "result",
                    span: to_source_span(&call_expr.span),
                });
            }
            if let TypedExprKind::Call { args, .. } = &typed_call.kind {
                for arg in args {
                    if !is_spawn_word_scalar(arg.ty) {
                        return Err(TypeError::SpawnTypeUnsupported {
                            got: arg.ty,
                            role: "argument",
                            span: to_source_span(&call_expr.span),
                        });
                    }
                }
            }
            let task_id = intern_task(tasks, typed_call.ty);
            (
                TypedExprKind::Spawn { call: Box::new(typed_call), task_id },
                Type::Task(task_id),
            )
        }
        // C4.4 / ADR 0024 D3: `task.await` — receiver must be a
        // `Task<T>`; the result type is the interned result_ty.
        ResolvedExprKind::Await { task_expr } => {
            let typed_task = check_expr(task_expr, None, env, signatures, structs, class_decls, enums, instances, refs, secrets, arrays, struct_type_param_counts, effect_decls, trait_decls, impl_decls, konts, tasks)?;
            let task_id = match typed_task.ty {
                Type::Task(tid) => tid,
                other => {
                    return Err(TypeError::AwaitOnNonTask {
                        got: other,
                        span: to_source_span(&task_expr.span),
                    });
                }
            };
            let result_ty = tasks[task_id.0 as usize].result_ty;
            (
                TypedExprKind::Await { task_expr: Box::new(typed_task), task_id },
                result_ty,
            )
        }
        // Phase D.1 / ADR 0032 (3/N): sum-type variant construction.
        ResolvedExprKind::EnumConstruct {
            enum_id,
            enum_name,
            variant_name,
            variant_span,
            args,
        } => check_enum_construct_expr(
            *enum_id,
            enum_name,
            variant_name,
            variant_span,
            args,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?,
        // Phase D.1 / ADR 0032 (3/N): `match` with exhaustiveness.
        ResolvedExprKind::Match { scrutinee, arms } => check_match_expr(
            scrutinee,
            arms,
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
            &expr.span,
        )?,
    };
    let synth = TypedExpr { kind, span: expr.span.clone(), ty };
    // ADR 0065: a DIVERGENT node (an early `return`, or a fully-diverging
    // if/match/block) never produces a value, so it must NOT be coerced to the
    // expected type — `return e` is valid in ANY expected context (an arm of a
    // `match` / branch of an `if` / a `let` whose type differs from `e`'s). The
    // node carries its own (divergent-placeholder) type; the enclosing join
    // (the if-join / match-arm join) recognises the divergence and takes the
    // OTHER paths' type. Without this skip, `coerce_to_expected` would reject a
    // sound program — e.g. `let r: bool = match c { A => return 0, B => b }`,
    // where the `A` arm `return`s the function's `i64`.
    if expr_diverges(&synth) {
        return Ok(synth);
    }
    // ADR 0014 D3: apply T→?T widening if the expected type is ?T and
    // the synthesized type is T.
    coerce_to_expected(synth, expected, &expr.span, secrets)
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

/// C3.4 / ADR 0020 D5 + D6: type-check `handle body with { arms,
/// return_arm }`. Each arm's body type must equal the outer
/// handle expression's type. The outer type is determined by
/// the return arm if present (binds `value_name` to the body's
/// type and uses the return arm body's type); else it's the
/// body's own type (default `return v => v`).
#[allow(clippy::too_many_arguments)]
fn check_handle_expr(
    body: &ResolvedExpr,
    arms: &[sentinel_resolve::ResolvedHandlerArm],
    return_arm: Option<&sentinel_resolve::ResolvedReturnArm>,
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
    _handle_span: &Span,
) -> Result<(TypedExprKind, Type), TypeError> {
    // Check the handle body without an expected type — the body's
    // type seeds the outer type. (We could push `expected` down, but
    // doing so would require coordinating with the arms; simpler to
    // synthesize bottom-up and let the outer `coerce_to_expected`
    // pass handle any T→?T widening.)
    let body_typed = check_expr(
        body,
        None,
        env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;
    let body_ty = body_typed.ty;

    // Compute the outer type. With a return arm, the outer type is
    // the return arm body's type (the arm rebinds `value_name` to
    // `body_ty`). Without, the outer is body_ty (the identity
    // `return v => v` default per ADR 0020 D4).
    let (outer_ty, typed_return_arm) = if let Some(ra) = return_arm {
        let saved = env.get(&ra.value_var_id).copied();
        env.insert(ra.value_var_id, (body_ty, false));
        let ra_body_typed = check_expr(
            &ra.body,
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        let ra_ty = ra_body_typed.ty;
        match saved {
            Some(prev) => {
                env.insert(ra.value_var_id, prev);
            }
            None => {
                env.remove(&ra.value_var_id);
            }
        }
        (
            ra_ty,
            Some(Box::new(TypedReturnArm {
                value_var_id: ra.value_var_id,
                value_name: ra.value_name.clone(),
                body: ra_body_typed,
                span: ra.span.clone(),
            })),
        )
    } else {
        (body_ty, None)
    };

    // Walk each arm: bind params + kont in env, check arm body, then
    // restore env to its prior state. Collect arms + handled-effect
    // set.
    let mut typed_arms: Vec<TypedHandlerArm> = Vec::with_capacity(arms.len());
    let mut handled: Vec<(EffectId, usize)> = Vec::with_capacity(arms.len());
    for arm in arms {
        let effect_decl = &effect_decls[arm.effect_id.0 as usize];
        let op = &effect_decl.ops[arm.op_index];
        // Arm's param_var_ids = op params + kont. Expected length:
        // op.params.len() + 1.
        let expected_len = op.params.len() + 1;
        if arm.param_var_ids.len() != expected_len {
            return Err(TypeError::OperationArityMismatch {
                effect_name: arm.effect_name.clone(),
                op_name: arm.op_name.clone(),
                expected: expected_len,
                got: arm.param_var_ids.len(),
                span: to_source_span(&arm.span),
            });
        }
        // Save env entries we're about to overwrite so we can
        // restore after the arm body.
        let saved: Vec<(VarId, Option<(Type, bool)>)> = arm
            .param_var_ids
            .iter()
            .map(|vid| (*vid, env.get(vid).copied()))
            .collect();

        // Bind op params with their declared types.
        for (i, p) in op.params.iter().enumerate() {
            env.insert(arm.param_var_ids[i], (p.ty, p.mutable));
        }
        // Bind the kont with Type::Kont(KontId) where KontData =
        // (op.return_type, outer_ty).
        let kont_id = intern_kont(konts, op.return_type, outer_ty);
        let kont_vid = *arm
            .param_var_ids
            .last()
            .expect("arity check guarantees at least one VarId");
        env.insert(kont_vid, (Type::Kont(kont_id), false));

        // Synthesize the arm body without pushing the outer type
        // down — this lets the arm-mismatch surface as the more
        // specific `HandlerArmTypeMismatch` rather than the
        // generic `Mismatch` that `coerce_to_expected` would emit.
        let arm_body_typed = check_expr(
            &arm.body,
            None,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;

        if arm_body_typed.ty != outer_ty {
            return Err(TypeError::HandlerArmTypeMismatch {
                effect_name: arm.effect_name.clone(),
                op_name: arm.op_name.clone(),
                expected: outer_ty,
                got: arm_body_typed.ty,
                span: to_source_span(&arm.body.span),
            });
        }

        // Restore env entries.
        for (vid, prev) in saved {
            match prev {
                Some(p) => {
                    env.insert(vid, p);
                }
                None => {
                    env.remove(&vid);
                }
            }
        }

        typed_arms.push(TypedHandlerArm {
            effect_id: arm.effect_id,
            op_index: arm.op_index,
            effect_name: arm.effect_name.clone(),
            op_name: arm.op_name.clone(),
            op_span: arm.op_span.clone(),
            param_var_ids: arm.param_var_ids.clone(),
            param_names: arm.param_names.clone(),
            kont_id,
            body: arm_body_typed,
            span: arm.span.clone(),
        });
        handled.push((arm.effect_id, arm.op_index));
    }

    Ok((
        TypedExprKind::Handle {
            body: Box::new(body_typed),
            arms: typed_arms,
            return_arm: typed_return_arm,
            handled,
        },
        outer_ty,
    ))
}

/// C3.4 / ADR 0020 D5: type-check `perform Effect.Op(args)`. Each
/// arg is checked against the op's declared param type; the
/// expression's type is the op's declared return type.
#[allow(clippy::too_many_arguments)]
fn check_perform_expr(
    effect_id: EffectId,
    op_index: usize,
    effect_name: &str,
    op_name: &str,
    op_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    let op = &effect_decls[effect_id.0 as usize].ops[op_index];
    if args.len() != op.params.len() {
        return Err(TypeError::OperationArityMismatch {
            effect_name: effect_name.to_string(),
            op_name: op_name.to_string(),
            expected: op.params.len(),
            got: args.len(),
            span: to_source_span(op_span),
        });
    }
    let mut typed_args = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let expected = Some(op.params[i].ty);
        let typed = check_expr(
            arg,
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        if typed.ty != op.params[i].ty {
            return Err(TypeError::Mismatch {
                expected: op.params[i].ty,
                got: typed.ty,
                span: to_source_span(&arg.span),
            });
        }
        typed_args.push(typed);
    }
    Ok((
        TypedExprKind::Perform {
            effect_id,
            op_index,
            effect_name: effect_name.to_string(),
            op_name: op_name.to_string(),
            op_span: op_span.clone(),
            args: typed_args,
        },
        op.return_type,
    ))
}

/// C3.4 / ADR 0020 D5, generalized by ADR 0070 (D3-revisit): type-check
/// `f(arg)` where `f` is a bound local variable. Two legitimate cases:
/// `f: Type::Kont(KontId)` (resuming a continuation inside a handler arm,
/// set up by [`check_handle_expr`]) or `f: Type::Fn(FnValueSigId)` (calling
/// a non-capturing function value — ADR 0070 — the direct-call twin of the
/// `apply(f, x)` builtin, producing the identical `TypedExprKind::Call`
/// shape so the two spellings are indistinguishable after type-check). Any
/// other type is `TypeError::CalleeNotCallable`.
#[allow(clippy::too_many_arguments)]
fn check_resume_kont_expr(
    kont: VarId,
    callee_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    let (kont_ty, _) = env
        .get(&kont)
        .copied()
        .expect("resolve guarantees the kont VarId is bound");
    match kont_ty {
        Type::Kont(kont_id) => {
            let kont_data = &konts[kont_id.0 as usize];
            let arg_ty = kont_data.arg_ty;
            let ret_ty = kont_data.ret_ty;
            // C3.4 minimum: konts always take exactly one arg (the value
            // being resumed with). Multi-arg konts are a future ADR if
            // ops grow tuple returns.
            let expected_args: usize = 1;
            if args.len() != expected_args {
                return Err(TypeError::KontArityMismatch {
                    expected: expected_args,
                    got: args.len(),
                    span: to_source_span(callee_span),
                });
            }
            let typed_arg = check_expr(
                &args[0],
                Some(arg_ty),
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            if typed_arg.ty != arg_ty {
                return Err(TypeError::Mismatch {
                    expected: arg_ty,
                    got: typed_arg.ty,
                    span: to_source_span(&args[0].span),
                });
            }
            Ok((
                TypedExprKind::ResumeKont {
                    kont,
                    callee_span: callee_span.clone(),
                    args: vec![typed_arg],
                    kont_id,
                },
                ret_ty,
            ))
        }
        Type::Fn(sig_id) => {
            // ADR 0070 (D3-revisit): `f(x)` on a `Fn<T,R>`-typed local var —
            // the direct-call twin of `apply(f, x)` (check_call's
            // APPLY_FN_ID branch, mirrored exactly below). `Fn<T,R>` is
            // always exactly one parameter (mirrors the kont arm's own
            // `expected_args`).
            let (param_ty, ret_ty) = fn_value_sig_param_ret(sig_id);
            let expected_args: usize = 1;
            if args.len() != expected_args {
                return Err(TypeError::FnValueArityMismatch {
                    expected: expected_args,
                    got: args.len(),
                    span: to_source_span(callee_span),
                });
            }
            // `None`, not `Some(param_ty)` — see the identical note on
            // `apply`'s own arg check in check_call: this avoids
            // `coerce_to_expected` pre-empting `FnValueArgMismatch` with a
            // generic `Mismatch`.
            let x_typed = check_expr(
                &args[0],
                None,
                env,
                signatures,
                structs,
                class_decls,
                enums,
                instances,
                refs,
                secrets,
                arrays,
                struct_type_param_counts,
                effect_decls,
                trait_decls,
                impl_decls,
                konts,
                tasks,
            )?;
            if x_typed.ty != param_ty {
                return Err(TypeError::FnValueArgMismatch {
                    expected: param_ty,
                    got: x_typed.ty,
                    span: to_source_span(&args[0].span),
                });
            }
            // Hand-build the `Var` node from what `env.get` already gave
            // us — do NOT re-dispatch through check_expr's general `Var`
            // path, which has its own `Type::Kont`-smuggling guard
            // (`TypeError::KontUsedAsValue`) that `ResumeKont` has always
            // deliberately bypassed by hand-constructing its typed node.
            let f_typed = TypedExpr {
                kind: TypedExprKind::Var(kont),
                span: callee_span.clone(),
                ty: kont_ty,
            };
            Ok((
                TypedExprKind::Call {
                    id: APPLY_FN_ID,
                    callee_span: callee_span.clone(),
                    args: vec![f_typed, x_typed],
                    type_args: vec![],
                },
                ret_ty,
            ))
        }
        other => Err(TypeError::CalleeNotCallable {
            got: other,
            span: to_source_span(callee_span),
        }),
    }
}

/// C4.1 / ADR 0022 D3 + D7: type-check a postfix `target.method(args)`
/// call. The receiver's type must reduce to `Type::Class(_)` (either
/// directly or via auto-deref of `Type::Ref(&Class)` /
/// `Type::Ref(&mut Class)`). The method's `self_kind` determines
/// whether the auto-ref is shared or exclusive; methods that take
/// `&mut Self` reject if the receiver's static type can't be made
/// mutable.
#[allow(clippy::too_many_arguments)]
fn check_method_call_expr(
    target: &ResolvedExpr,
    method: &str,
    method_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    let target_typed = check_expr(
        target,
        None,
        env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;
    // Auto-deref `&T` / `&mut T` to find the underlying nominal
    // type for method lookup (D6 step 1 + step 2).
    let recv_concrete = match target_typed.ty {
        Type::Class(_) | Type::Struct(_) => target_typed.ty,
        Type::Ref(rid) => refs[rid.0 as usize].inner,
        _ => {
            return Err(TypeError::MethodCallOnNonClass {
                got_ty: type_display(target_typed.ty, None),
                span: to_source_span(method_span),
            });
        }
    };
    let impl_target = match recv_concrete {
        Type::Class(cid) => ImplTarget::Class(cid),
        Type::Struct(sid) => ImplTarget::Struct(sid),
        _ => {
            return Err(TypeError::MethodCallOnNonClass {
                got_ty: type_display(target_typed.ty, None),
                span: to_source_span(method_span),
            });
        }
    };

    // D6 step 1: class-method lookup (when receiver is a class).
    if let Type::Class(class_id) = recv_concrete {
        let class = &class_decls[class_id.0 as usize];
        if let Some(method_index) = class.method_index(method) {
            let m = &class.methods[method_index];
            if args.len() != m.params.len() {
                return Err(TypeError::MethodArityMismatch {
                    class_name: class.name.clone(),
                    method_name: method.to_string(),
                    expected: m.params.len(),
                    got: args.len(),
                    span: to_source_span(method_span),
                });
            }
            let mut typed_args = Vec::with_capacity(args.len());
            for (arg, param) in args.iter().zip(m.params.iter()) {
                let arg_typed = check_expr(
                    arg,
                    Some(param.ty),
                    env,
                    signatures,
                    structs,
                    class_decls,
                    enums,
                    instances,
                    refs,
                    secrets,
                    arrays,
                    struct_type_param_counts,
                    effect_decls,
                    trait_decls,
                    impl_decls,
                    konts,
                    tasks,
                )?;
                if arg_typed.ty != param.ty {
                    return Err(TypeError::Mismatch {
                        expected: param.ty,
                        got: arg_typed.ty,
                        span: to_source_span(&arg.span),
                    });
                }
                typed_args.push(arg_typed);
            }
            let return_type = m.return_type;
            return Ok((
                TypedExprKind::MethodCall {
                    target: Box::new(target_typed),
                    class_id,
                    method_index,
                    method: method.to_string(),
                    method_span: method_span.clone(),
                    args: typed_args,
                },
                return_type,
            ));
        }
    }

    // D6 step 2: default-impl lookup. Find every default impl
    // whose `for Type` matches the receiver's nominal type AND
    // whose trait declares a method named `method`. Exactly one
    // match → ImplMethodCall. >1 → AmbiguousMethodCall. 0 →
    // MethodNotFound.
    let matches: Vec<(ImplId, usize)> = impl_decls
        .iter()
        .filter(|imp| imp.name.is_none() && imp.target == impl_target)
        .filter_map(|imp| {
            imp.method_index(method).map(|idx| (imp.id, idx))
        })
        .collect();
    let recv_ty_disp = type_display(recv_concrete, None);
    match matches.len() {
        0 => Err(TypeError::MethodNotFound {
            class_name: recv_ty_disp,
            method_name: method.to_string(),
            span: to_source_span(method_span),
        }),
        n if n > 1 => Err(TypeError::AmbiguousMethodCall {
            method_name: method.to_string(),
            recv_ty: recv_ty_disp,
            span: to_source_span(method_span),
        }),
        _ => {
            let (impl_id, method_index) = matches[0];
            let imp = &impl_decls[impl_id.0 as usize];
            let m = &imp.methods[method_index];
            if args.len() != m.params.len() {
                return Err(TypeError::MethodArityMismatch {
                    class_name: imp.type_name.clone(),
                    method_name: method.to_string(),
                    expected: m.params.len(),
                    got: args.len(),
                    span: to_source_span(method_span),
                });
            }
            let mut typed_args = Vec::with_capacity(args.len());
            for (arg, param) in args.iter().zip(m.params.iter()) {
                let arg_typed = check_expr(
                    arg,
                    Some(param.ty),
                    env,
                    signatures,
                    structs,
                    class_decls,
                    enums,
                    instances,
                    refs,
                    secrets,
                    arrays,
                    struct_type_param_counts,
                    effect_decls,
                    trait_decls,
                    impl_decls,
                    konts,
                    tasks,
                )?;
                if arg_typed.ty != param.ty {
                    return Err(TypeError::Mismatch {
                        expected: param.ty,
                        got: arg_typed.ty,
                        span: to_source_span(&arg.span),
                    });
                }
                typed_args.push(arg_typed);
            }
            let return_type = m.return_type;
            Ok((
                TypedExprKind::ImplMethodCall {
                    target: Box::new(target_typed),
                    impl_id,
                    method_index,
                    method: method.to_string(),
                    method_span: method_span.clone(),
                    args: typed_args,
                },
                return_type,
            ))
        }
    }
}

/// C4.1 / ADR 0022 D5: type-check `Name::init(args)`. The class's
/// signature has been populated in Pass 3.5; check_class_init
/// validates the arg count + types against the init's param list.
#[allow(clippy::too_many_arguments)]
fn check_class_init_expr(
    class_id: ClassId,
    name: &str,
    name_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    let class = &class_decls[class_id.0 as usize];
    let init_params: &[TypedParam] = class
        .init
        .as_ref()
        .map(|i| i.params.as_slice())
        .unwrap_or(&[]);
    if args.len() != init_params.len() {
        return Err(TypeError::ClassInitArityMismatch {
            class_name: class.name.clone(),
            expected: init_params.len(),
            got: args.len(),
            span: to_source_span(name_span),
        });
    }
    let mut typed_args = Vec::with_capacity(args.len());
    for (arg, param) in args.iter().zip(init_params.iter()) {
        let arg_typed = check_expr(
            arg,
            Some(param.ty),
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        if arg_typed.ty != param.ty {
            return Err(TypeError::Mismatch {
                expected: param.ty,
                got: arg_typed.ty,
                span: to_source_span(&arg.span),
            });
        }
        typed_args.push(arg_typed);
    }
    Ok((
        TypedExprKind::ClassInit {
            id: class_id,
            name: name.to_string(),
            name_span: name_span.clone(),
            args: typed_args,
        },
        Type::Class(class_id),
    ))
}

/// Phase D.1 / ADR 0032 (3/N): type-check `Enum::Variant(args)`
/// construction. The enum was resolved at resolve time; here we
/// resolve the variant *name* to its index, check the payload arity,
/// and check each arg against the variant's payload type (pushing the
/// payload type down so `T → secret T` / `T → ?T` widening applies,
/// like a class-init arg). The result type is `Type::Enum(enum_id)`.
#[allow(clippy::too_many_arguments)]
fn check_enum_construct_expr(
    enum_id: EnumId,
    enum_name: &str,
    variant_name: &str,
    variant_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    // Resolve the variant to its index + payload types. Clone the
    // payloads (cheap — `Type: Copy`) so we don't hold a borrow of
    // `enums` across the `check_expr` calls below (which also need it).
    let (variant_index, payloads): (usize, Vec<Type>) = {
        let ed = &enums[enum_id.0 as usize];
        match ed.variant(variant_name) {
            Some((idx, v)) => (idx, v.payloads.clone()),
            None => {
                return Err(TypeError::UnknownVariant {
                    enum_name: ed.name.clone(),
                    variant_name: variant_name.to_string(),
                    span: to_source_span(variant_span),
                });
            }
        }
    };
    if args.len() != payloads.len() {
        return Err(TypeError::VariantPayloadArityMismatch {
            enum_name: enum_name.to_string(),
            variant_name: variant_name.to_string(),
            expected: payloads.len(),
            got: args.len(),
            span: to_source_span(variant_span),
        });
    }
    let mut typed_args = Vec::with_capacity(args.len());
    for (arg, payload_ty) in args.iter().zip(payloads.iter()) {
        let arg_typed = check_expr(
            arg,
            Some(*payload_ty),
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        if arg_typed.ty != *payload_ty {
            return Err(TypeError::Mismatch {
                expected: *payload_ty,
                got: arg_typed.ty,
                span: to_source_span(&arg.span),
            });
        }
        typed_args.push(arg_typed);
    }
    Ok((
        TypedExprKind::EnumConstruct {
            enum_id,
            variant_index,
            enum_name: enum_name.to_string(),
            variant_name: variant_name.to_string(),
            args: typed_args,
        },
        Type::Enum(enum_id),
    ))
}

/// Phase D.1 / ADR 0032 (3/N): type-check a `match` expression. The
/// scrutinee must be an enum; each arm pattern is resolved against
/// that enum (variant index + payload-typed bindings scoped into the
/// arm body), all arm bodies unify to one result type, and the arm
/// set must be exhaustive (cover every variant, or include `_`).
#[allow(clippy::too_many_arguments)]
fn check_match_expr(
    scrutinee: &ResolvedExpr,
    arms: &[sentinel_resolve::ResolvedMatchArm],
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
    match_span: &Span,
) -> Result<(TypedExprKind, Type), TypeError> {
    // 1. The scrutinee must be an enum (no `secret enum` / `?enum` /
    //    `&enum` at the MVP — a bare `Type::Enum`).
    let scrutinee_typed = check_expr(
        scrutinee,
        None,
        env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;
    let enum_id = match scrutinee_typed.ty {
        Type::Enum(eid) => eid,
        other => {
            return Err(TypeError::MatchScrutineeNotEnum {
                got: other,
                span: to_source_span(&scrutinee.span),
            });
        }
    };
    // Snapshot the enum's name + per-variant (name, payloads) so we
    // don't hold a borrow of `enums` across the arm `check_expr`s.
    let (enum_name, variants): (String, Vec<(String, Vec<Type>)>) = {
        let ed = &enums[enum_id.0 as usize];
        (
            ed.name.clone(),
            ed.variants.iter().map(|v| (v.name.clone(), v.payloads.clone())).collect(),
        )
    };

    let mut covered = vec![false; variants.len()];
    let mut has_wildcard = false;
    let mut result_ty: Option<Type> = None;
    // ADR 0065: the type of the first arm that DIVERGES (its body always
    // `return`s). Used only when EVERY arm diverges (the whole `match`
    // diverges) — a placeholder treated as bottom by parents, like a fully
    // diverging `if`.
    let mut fallback_ty: Option<Type> = None;
    let mut typed_arms = Vec::with_capacity(arms.len());

    for arm in arms {
        // Resolve the pattern to a typed pattern (variant index +
        // payload-typed bindings), recording coverage.
        let typed_pattern = match &arm.pattern {
            ResolvedPattern::Variant {
                enum_name: pat_enum,
                variant_name,
                variant_span,
                bindings,
                span: pat_span,
                ..
            } => {
                // The pattern must name the scrutinee's enum + a real
                // variant of it.
                let variant_index = if *pat_enum == enum_name {
                    variants.iter().position(|(n, _)| n == variant_name)
                } else {
                    None
                };
                let variant_index = variant_index.ok_or_else(|| TypeError::UnknownVariant {
                    enum_name: enum_name.clone(),
                    variant_name: variant_name.clone(),
                    span: to_source_span(variant_span),
                })?;
                let payloads = &variants[variant_index].1;
                if bindings.len() != payloads.len() {
                    return Err(TypeError::VariantPayloadArityMismatch {
                        enum_name: enum_name.clone(),
                        variant_name: variant_name.clone(),
                        expected: payloads.len(),
                        got: bindings.len(),
                        span: to_source_span(pat_span),
                    });
                }
                covered[variant_index] = true;
                let typed_bindings: Vec<TypedPatternBinding> = bindings
                    .iter()
                    .zip(payloads.iter())
                    .map(|(b, ty)| TypedPatternBinding {
                        var_id: b.var_id,
                        name: b.name.clone(),
                        ty: *ty,
                        span: b.span.clone(),
                    })
                    .collect();
                TypedPattern::Variant {
                    variant_index,
                    variant_name: variant_name.clone(),
                    bindings: typed_bindings,
                    span: pat_span.clone(),
                }
            }
            ResolvedPattern::Wildcard(wspan) => {
                has_wildcard = true;
                TypedPattern::Wildcard(wspan.clone())
            }
        };

        // Bind the pattern's payload bindings into env for the arm
        // body, snapshotting + restoring like handler-arm params.
        let saved: Vec<(VarId, Option<(Type, bool)>)> = match &typed_pattern {
            TypedPattern::Variant { bindings, .. } => {
                bindings.iter().map(|b| (b.var_id, env.get(&b.var_id).copied())).collect()
            }
            TypedPattern::Wildcard(_) => Vec::new(),
        };
        if let TypedPattern::Variant { bindings, .. } = &typed_pattern {
            for b in bindings {
                env.insert(b.var_id, (b.ty, false));
                env.record_name(b.var_id, b.name.clone());
            }
        }

        // Check the arm body with the expected type pushed down (so
        // `null` / secret widening resolves per arm, like `if`).
        let body_typed = check_expr(
            &arm.body,
            expected,
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;

        for (vid, prev) in saved {
            match prev {
                Some(p) => {
                    env.insert(vid, p);
                }
                None => {
                    env.remove(&vid);
                }
            }
        }

        // All arms must produce the same type. When `expected` is
        // Some, each arm already coerced to it (a mismatch surfaces as
        // the generic `Mismatch` inside the arm); the explicit check
        // below catches divergence in the inferred (`expected == None`)
        // case, like `if`'s then/else equality.
        //
        // ADR 0065: an arm that always diverges (an early `return`) yields no
        // value, so it does not constrain the join — the `match`'s type is the
        // OTHER arms', exactly like an `if` branch that diverges. A diverging
        // arm is recorded only as a `fallback_ty` (used if EVERY arm diverges).
        if expr_diverges(&body_typed) {
            if fallback_ty.is_none() {
                fallback_ty = Some(body_typed.ty);
            }
        } else {
            match result_ty {
                None => result_ty = Some(body_typed.ty),
                Some(rt) => {
                    if body_typed.ty != rt {
                        return Err(TypeError::MatchArmTypeMismatch {
                            expected: rt,
                            got: body_typed.ty,
                            span: to_source_span(&arm.body.span),
                        });
                    }
                }
            }
        }

        typed_arms.push(TypedMatchArm {
            pattern: typed_pattern,
            body: body_typed,
            span: arm.span.clone(),
        });
    }

    // Exhaustiveness (ADR 0032 D2): every variant covered, or a `_`.
    if !has_wildcard {
        let missing: Vec<String> = covered
            .iter()
            .enumerate()
            .filter(|(_, c)| !**c)
            .map(|(i, _)| variants[i].0.clone())
            .collect();
        if !missing.is_empty() {
            return Err(TypeError::NonExhaustiveMatch {
                enum_name,
                missing,
                span: to_source_span(match_span),
            });
        }
    }

    // `result_ty` is None only for a zero-arm match — which, for a
    // non-empty enum, already failed exhaustiveness above — or when EVERY
    // arm diverges (ADR 0065), in which case `fallback_ty` carries a
    // placeholder (the whole `match` diverges). The remaining (empty-enum)
    // case is unconstructable; default to i64 (the node is dead, and codegen
    // rejects `match` at (3/N) anyway).
    let result_ty = result_ty.or(fallback_ty).unwrap_or(Type::I64);

    Ok((
        TypedExprKind::Match {
            scrutinee: Box::new(scrutinee_typed),
            enum_id,
            arms: typed_arms,
        },
        result_ty,
    ))
}

/// C4.2 / ADR 0023 D5 Path 2 + D6: type-check
/// `ImplName::method(receiver, args)`. The impl + method_index were
/// resolved at resolve time. Verify the receiver (args[0])'s type
/// is the impl target's type or a ref-to-it, then arity + per-arg
/// type-check against the impl method's params.
#[allow(clippy::too_many_arguments)]
fn check_qualified_call_expr(
    impl_id: ImplId,
    method_index: usize,
    impl_name: &str,
    method: &str,
    method_span: &Span,
    args: &[ResolvedExpr],
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
    class_decls: &[ClassData],
    enums: &[EnumData],
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    secrets: &mut Vec<SecretData>,
    arrays: &mut Vec<ArrayElem>,
    struct_type_param_counts: &HashMap<StructId, usize>,
    effect_decls: &[TypedEffectDecl],
    trait_decls: &[TraitData],
    impl_decls: &[ImplData],
    konts: &mut Vec<KontData>,
    tasks: &mut Vec<TaskData>,
) -> Result<(TypedExprKind, Type), TypeError> {
    let imp = &impl_decls[impl_id.0 as usize];
    let m = &imp.methods[method_index];
    // Expected total arg count = 1 (receiver) + method.params.len().
    let expected_total = 1 + m.params.len();
    if args.len() != expected_total {
        return Err(TypeError::MethodArityMismatch {
            class_name: imp.type_name.clone(),
            method_name: method.to_string(),
            expected: expected_total,
            got: args.len(),
            span: to_source_span(method_span),
        });
    }
    // First arg = receiver. Its type after auto-deref must match
    // the impl's target type, and its ref mutability must match
    // self_kind.
    let recv = check_expr(
        &args[0],
        None,
        env,
        signatures,
        structs,
        class_decls,
        enums,
        instances,
        refs,
        secrets,
        arrays,
        struct_type_param_counts,
        effect_decls,
        trait_decls,
        impl_decls,
        konts,
        tasks,
    )?;
    let recv_concrete = match recv.ty {
        Type::Ref(rid) => refs[rid.0 as usize].inner,
        other => other,
    };
    let target_concrete = match imp.target {
        ImplTarget::Class(cid) => Type::Class(cid),
        ImplTarget::Struct(sid) => Type::Struct(sid),
    };
    if recv_concrete != target_concrete {
        return Err(TypeError::ImplMethodReceiverMismatch {
            impl_name: impl_name.to_string(),
            type_name: imp.type_name.clone(),
            got_ty: type_display(recv.ty, None),
            span: to_source_span(&args[0].span),
        });
    }
    // Type-check the remaining args against the method's params.
    let mut typed_args = Vec::with_capacity(args.len());
    typed_args.push(recv);
    for (arg, param) in args[1..].iter().zip(m.params.iter()) {
        let arg_typed = check_expr(
            arg,
            Some(param.ty),
            env,
            signatures,
            structs,
            class_decls,
            enums,
            instances,
            refs,
            secrets,
            arrays,
            struct_type_param_counts,
            effect_decls,
            trait_decls,
            impl_decls,
            konts,
            tasks,
        )?;
        if arg_typed.ty != param.ty {
            return Err(TypeError::Mismatch {
                expected: param.ty,
                got: arg_typed.ty,
                span: to_source_span(&arg.span),
            });
        }
        typed_args.push(arg_typed);
    }
    let return_type = m.return_type;
    Ok((
        TypedExprKind::QualifiedCall {
            impl_id,
            method_index,
            impl_name: impl_name.to_string(),
            method: method.to_string(),
            method_span: method_span.clone(),
            args: typed_args,
        },
        return_type,
    ))
}

// =============================================================================
// Salsa-tracked query
// =============================================================================

/// Salsa-tracked type-check query. Chains on
/// [`sentinel_resolve::resolve_query`]: pulls the resolved program
/// out of the resolve query's cached result and type-checks it.
/// Returns `None` if any prior pipeline stage failed or if
/// type-checking itself fails. Diagnostics flow through the salsa
/// accumulator (lex/parse/resolve diagnostics propagate
/// transitively from upstream queries; type errors are accumulated
/// here with stage="types").
#[salsa::tracked(return_ref)]
pub fn check_query(db: &dyn SentinelDb, file: SourceFile) -> Option<TypedProgram> {
    let resolved = sentinel_resolve::resolve_query(db, file).as_ref()?;
    match check(resolved) {
        Ok(typed) => Some(typed),
        Err(err) => {
            type_error_to_diagnostic(&err).accumulate(db);
            None
        }
    }
}

fn type_error_to_diagnostic(err: &TypeError) -> Diagnostic {
    let (code, message, span): (&'static str, String, std::ops::Range<usize>) = match err {
        TypeError::UnknownType { name, span } => (
            "sentinel::types::unknown_type",
            format!("unknown type `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::Mismatch { expected, got, span } => (
            "sentinel::types::mismatch",
            format!("type mismatch: expected {expected}, found {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ReturnTypeMismatch { name, expected, got, span } => (
            "sentinel::types::return_type_mismatch",
            format!("`{name}` returns {expected} but its body produces {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::CallArgMismatch { callee, arg_index, expected, got, span } => (
            "sentinel::types::call_arg_mismatch",
            format!("argument {arg_index} of `{callee}` expects {expected}, got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FieldAccessOnNonStruct { got, span } => (
            "sentinel::types::field_access_on_non_struct",
            format!("field access on non-struct type `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::UnknownField { struct_name, field, span } => (
            "sentinel::types::unknown_field",
            format!("struct `{struct_name}` has no field `{field}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MissingField { struct_name, field, span } => (
            "sentinel::types::missing_field",
            format!("struct literal `{struct_name}` is missing field `{field}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::RecursiveStruct { name, span, .. } => (
            "sentinel::types::recursive_struct",
            format!("recursive struct `{name}` has no representable size"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AmbiguousNull { span } => (
            "sentinel::types::ambiguous_null",
            "ambiguous `null` — cannot infer the nullable's inner type".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AmbiguousEmptyArray { span } => (
            "sentinel::types::ambiguous_empty_array",
            "ambiguous empty array — cannot infer the element type".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::IndexOnNonArray { got, span } => (
            "sentinel::types::index_on_non_array",
            format!("indexing on non-array type `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::IndexNotInt { got, span } => (
            "sentinel::types::index_not_int",
            format!("array index must be `i64`, got `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::NestedArray { span } => (
            "sentinel::types::nested_array",
            "nested array types `[[T]]` are not allowed at C1.6".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::VecElementNotSupported { span } => (
            "sentinel::types::vec_element_not_supported",
            "`Vec<T>` element type is not supported at the D.3 MVP".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SealedChannelElementNotSupported { span } => (
            "sentinel::types::sealed_channel_element_not_supported",
            "`SealedChannel<T>` element type must be `secret i64`".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ChannelElementNotSupported { span } => (
            "sentinel::types::channel_element_not_supported",
            "`Channel<T>` element type is not supported yet".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SharedElementNotSupported { span } => (
            "sentinel::types::shared_element_not_supported",
            "`Shared<T>` element type is not supported yet".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MutexElementNotSupported { span } => (
            "sentinel::types::mutex_element_not_supported",
            "`Mutex<T>` element type is not supported yet".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SharedReturnNotSupported { span } => (
            "sentinel::types::shared_return_not_supported",
            "returning a named `Shared<T>` binding is not yet supported".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FnTypeArgsNotSupported { span } => (
            "sentinel::types::fn_type_args_not_supported",
            "`Fn<T, R>` requires word-scalar T and R".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FnValueSignatureNotSupported { name, span } => (
            "sentinel::types::fn_value_signature_not_supported",
            format!("`{name}` cannot be used as a function value"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ApplyTargetNotFn { got, span } => (
            "sentinel::types::apply_target_not_fn",
            format!("`apply`'s first argument must be a function value (`Fn<T,R>`), got `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::CalleeNotCallable { got, span } => (
            "sentinel::types::callee_not_callable",
            format!("cannot call `{got}` — expected a continuation or a function value (`Fn<T,R>`)"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FnValueArityMismatch { expected, got, span } => (
            "sentinel::types::fn_value_arity_mismatch",
            format!("function value expects {expected} argument(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FnValueArgMismatch { expected, got, span } => (
            "sentinel::types::fn_value_arg_mismatch",
            format!("function value expects an argument of type `{expected}`, got `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::GenericMain { span } => (
            "sentinel::types::generic_main",
            "`fn main` cannot have type parameters".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AmbiguousTypeArg { callee, type_param, span } => (
            "sentinel::types::ambiguous_type_arg",
            format!("ambiguous type argument for `{type_param}` in call to `{callee}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::TypeArgInferenceConflict {
            callee,
            type_param,
            first,
            second,
            span,
        } => (
            "sentinel::types::type_arg_inference_conflict",
            format!(
                "conflicting inference for `{type_param}` in call to `{callee}`: bound to `{first}` then to `{second}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::TypeArgCountMismatch { type_name, expected, found, span } => (
            "sentinel::types::type_arg_count_mismatch",
            format!("`{type_name}` takes {expected} type argument(s), got {found}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::TypeArgsOnNonGeneric { type_name, span } => (
            "sentinel::types::type_args_on_non_generic",
            format!("`{type_name}` is not a generic type"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MissingTypeArgs { type_name, expected_count, span } => (
            "sentinel::types::missing_type_args",
            format!("`{type_name}` is generic; supply {expected_count} type argument(s)"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AmbiguousGenericStructLit { struct_name, span } => (
            "sentinel::types::ambiguous_generic_struct_lit",
            format!("ambiguous generic struct literal `{struct_name}` — supply type arguments via context"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::NestedRef { span } => (
            "sentinel::types::nested_ref",
            "nested references `&&T` are not allowed".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::RefInArray { span } => (
            "sentinel::types::ref_in_array",
            "references in array elements are not allowed at C2".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::RefInStructField { span } => (
            "sentinel::types::ref_in_struct_field",
            "references in struct fields are not allowed at C2".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::BorrowOfRvalue { span } => (
            "sentinel::types::borrow_of_rvalue",
            "cannot borrow a non-lvalue expression".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AssignToRvalue { span } => (
            "sentinel::types::assign_to_rvalue",
            "cannot assign to a non-lvalue expression".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AssignToImmutable { name, span } => (
            "sentinel::types::assign_to_immutable",
            format!("cannot assign to immutable binding `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::BorrowMutOfImmutable { name, span } => (
            "sentinel::types::borrow_mut_of_immutable",
            format!("cannot take `&mut` of immutable binding `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::DerefOfNonRef { got, span } => (
            "sentinel::types::deref_of_non_ref",
            format!("dereference of non-reference type `{got}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AssignThroughSharedRef { span } => (
            "sentinel::types::assign_through_shared_ref",
            "cannot assign through a shared reference `&T`".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::IndexAssignNotSupported { span } => (
            "sentinel::types::index_assign_not_supported",
            "mutable indexing `&mut a[i]` is not supported".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::IndexAssignNonCopyElem { elem_ty, span } => (
            "sentinel::types::index_assign_non_copy",
            format!("cannot index-assign an element of non-Copy type `{elem_ty}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretNotYet { span } => (
            "sentinel::types::secret_not_yet",
            "`secret T` is not yet supported (lands at C3.1)".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretFloat { span } => (
            "sentinel::types::secret_float",
            "`secret f64` is not allowed — float operations are not constant-time".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::FloatBitwise { span } => (
            "sentinel::types::float_bitwise",
            "bitwise / shift operators are not supported on `f64`".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ExternFfiType { span } => (
            "sentinel::types::extern_ffi_type",
            "`extern` fn types must be public FFI-safe scalars (`i64` or `f64`)".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::PtrOfArg { span } => (
            "sentinel::types::ptr_of_arg",
            "`ptr_of` / `ptr_of_mut` need a borrow of a public `[u8]`".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretBranch { kw, span } => (
            "sentinel::types::secret_branch",
            format!("`{kw}` on a `secret bool` condition would leak via timing"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::LoopControlOutsideLoop { kw, span } => (
            "sentinel::types::loop_control_outside_loop",
            format!("`{kw}` used outside of a loop"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretDivisor { span } => (
            "sentinel::types::secret_divisor",
            "variable-time division by a `secret` value leaks via timing".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretShiftAmount { span } => (
            "sentinel::types::secret_shift_amount",
            "variable-time shift by a `secret` amount leaks via timing".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::NonIntegerCast { span } => (
            "sentinel::types::non_integer_cast",
            "`as` cast requires integer operand and target (i64 / i32 / u8)".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SecretInRefDeref { span } => (
            "sentinel::types::secret_in_ref_deref",
            "dereferencing a secret reference leaks via the memory side channel"
                .to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::KontUsedAsValue { span } => (
            "sentinel::types::kont_used_as_value",
            "continuation binding can only be called, not used as a value".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::HandlerArmTypeMismatch { effect_name, op_name, expected, got, span } => (
            "sentinel::types::handler_arm_type_mismatch",
            format!(
                "handler arm `{effect_name}.{op_name}` body returns {got} but the `handle` expression's type is {expected}"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::OperationArityMismatch { effect_name, op_name, expected, got, span } => (
            "sentinel::types::operation_arity_mismatch",
            format!(
                "handler arm `{effect_name}.{op_name}` has {got} parameter(s) but expected {expected}"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::KontArityMismatch { expected, got, span } => (
            "sentinel::types::kont_arity_mismatch",
            format!("continuation resume call expected {expected} argument(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::InitFieldMaybeUnassigned {
            class_name,
            field_name,
            init_span,
            ..
        } => (
            "sentinel::types::init_field_maybe_unassigned",
            format!(
                "class `{class_name}` field `{field_name}` is not assigned by the end of `init`"
            ),
            init_span.offset()..(init_span.offset() + init_span.len()),
        ),
        TypeError::ClassConstructionMustUseInit { name, span } => (
            "sentinel::types::class_construction_must_use_init",
            format!("class `{name}` cannot be constructed with struct-literal syntax"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MethodNotFound { class_name, method_name, span } => (
            "sentinel::types::method_not_found",
            format!("class `{class_name}` has no method `{method_name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MethodCallOnNonClass { got_ty, span } => (
            "sentinel::types::method_call_on_non_class",
            format!("method call requires a class receiver (got `{got_ty}`)"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ClassInitArityMismatch { class_name, expected, got, span } => (
            "sentinel::types::class_init_arity_mismatch",
            format!("class `{class_name}::init` expected {expected} argument(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MethodArityMismatch { class_name, method_name, expected, got, span } => (
            "sentinel::types::method_arity_mismatch",
            format!(
                "method `{class_name}.{method_name}` expected {expected} argument(s), got {got}"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ImplMissingMethod {
            trait_name,
            type_name,
            method_name,
            span,
        } => (
            "sentinel::types::impl_missing_method",
            format!(
                "impl of `{trait_name}` for `{type_name}` is missing method `{method_name}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ImplMethodSignatureMismatch {
            trait_name,
            type_name,
            method_name,
            span,
        } => (
            "sentinel::types::impl_method_signature_mismatch",
            format!(
                "impl method `{method_name}` for `{trait_name}` on `{type_name}` has a signature that doesn't match the trait"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AmbiguousMethodCall { method_name, recv_ty, span } => (
            "sentinel::types::ambiguous_method_call",
            format!("ambiguous method call `{method_name}` on `{recv_ty}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::ImplMethodReceiverMismatch {
            impl_name,
            type_name,
            got_ty,
            span,
        } => (
            "sentinel::types::impl_method_receiver_mismatch",
            format!(
                "impl `{impl_name}` (for `{type_name}`) doesn't apply to receiver type `{got_ty}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SpawnMustBeCall { span } => (
            "sentinel::types::spawn_must_be_call",
            "`spawn` requires a function-call target".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::SpawnTypeUnsupported { got, role, span } => (
            "sentinel::types::spawn_type_unsupported",
            format!("`spawn` {role} type `{got}` is not supported yet"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::AwaitOnNonTask { got, span } => (
            "sentinel::types::await_on_non_task",
            format!("`.await` requires a `Task<T>` receiver (got `{got}`)"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::UnknownVariant { enum_name, variant_name, span } => (
            "sentinel::types::unknown_variant",
            format!("enum `{enum_name}` has no variant `{variant_name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::VariantPayloadArityMismatch {
            enum_name,
            variant_name,
            expected,
            got,
            span,
        } => (
            "sentinel::types::variant_payload_arity_mismatch",
            format!("variant `{enum_name}::{variant_name}` takes {expected} payload(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MatchScrutineeNotEnum { got, span } => (
            "sentinel::types::match_scrutinee_not_enum",
            format!("`match` requires an enum scrutinee (got `{got}`)"),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::NonExhaustiveMatch { enum_name, missing, span } => (
            "sentinel::types::non_exhaustive_match",
            format!("non-exhaustive `match` on `{enum_name}`: missing {}", missing.join(", ")),
            span.offset()..(span.offset() + span.len()),
        ),
        TypeError::MatchArmTypeMismatch { expected, got, span } => (
            "sentinel::types::match_arm_type_mismatch",
            format!("`match` arms have incompatible types: expected {expected}, found {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        // ADR 0037 (2/N): span-free (the extern has no body in this unit) —
        // the range collapses to 0..0; the message names the fix (a `use`).
        TypeError::UnknownImportedEffect { fn_name, effect_name } => (
            "sentinel::types::unknown_imported_effect",
            format!(
                "imported fn `{fn_name}` uses effect `{effect_name}`, which is not in scope \
                 — add a `use` for it"
            ),
            0..0,
        ),
    };
    Diagnostic {
        stage: "types",
        severity: Severity::Error,
        code,
        message,
        span,
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-types"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_resolve::resolve;
    use sentinel_syntax::parse;

    fn check_ok(src: &str) -> TypedProgram {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        check(&resolved).expect("check")
    }

    fn check_err(src: &str) -> TypeError {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        check(&resolved).expect_err("expected type error")
    }

    /// A bare `Ident` type expression (e.g. `i64`) for building
    /// `TypedImportedFn`s in tests.
    fn ident_te(name: &str) -> TypeExpr {
        Spanned { kind: TypeExprKind::Ident(name.to_string()), span: 0..0 }
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-types");
    }

    // ----- D.6 / ADR 0037 D5.1: check_module + imported externs -----

    #[test]
    fn check_module_builds_extern_typed_signature() {
        use sentinel_resolve::{resolve_module, ImportedFn};
        let main =
            parse("use util::math::add; fn main() -> i64 { add(1, 2) }").expect("parse");
        let resolved = resolve_module(
            &main,
            &[ImportedFn {
                name: "add".to_string(),
                arity: 2,
                type_params_count: 0,
                origin: vec!["util".to_string(), "math".to_string()],
                span: 0..0,
            }],
        )
        .expect("resolve_module");
        let typed_imports = vec![TypedImportedFn {
            name: "add".to_string(),
            param_type_exprs: vec![ident_te("i64"), ident_te("i64")],
            return_type_expr: ident_te("i64"),
            effect_row_names: vec![],
        }];
        let tp = check_module(&resolved, &typed_imports).expect("check_module");

        // The extern's typed signature carries the imported param/return
        // types + its origin; only `main` has a body.
        let add = tp.fn_signatures.iter().find(|s| s.name == "add").expect("add sig");
        assert_eq!(add.param_types, vec![Type::I64, Type::I64]);
        assert_eq!(add.return_type, Type::I64);
        assert!(!add.is_runtime);
        assert_eq!(
            add.extern_origin,
            Some(vec!["util".to_string(), "math".to_string()])
        );
        assert_eq!(tp.fns.len(), 1);
    }

    #[test]
    fn check_module_typechecks_call_against_extern_signature() {
        use sentinel_resolve::{resolve_module, ImportedFn};
        // `add` is imported as `(i64, i64) -> i64`; calling it with a bool
        // first arg must be rejected — proving the extern's typed signature
        // is consulted for cross-module call checking.
        let main =
            parse("use util::math::add; fn main() -> i64 { add(true, 2) }").expect("parse");
        let resolved = resolve_module(
            &main,
            &[ImportedFn {
                name: "add".to_string(),
                arity: 2,
                type_params_count: 0,
                origin: vec!["util".to_string(), "math".to_string()],
                span: 0..0,
            }],
        )
        .expect("resolve_module");
        let typed_imports = vec![TypedImportedFn {
            name: "add".to_string(),
            param_type_exprs: vec![ident_te("i64"), ident_te("i64")],
            return_type_expr: ident_te("i64"),
            effect_row_names: vec![],
        }];
        assert!(check_module(&resolved, &typed_imports).is_err());
    }

    // ----- positive paths -----

    #[test]
    fn checks_minimal_main() {
        let p = check_ok("fn main() -> i64 { 0 }");
        assert_eq!(p.fns.len(), 1);
        let main = p.main();
        assert_eq!(main.return_type, Type::I64);
        assert_eq!(main.body.ty, Type::I64);
        // Signature table: FnId(0)=print, (1)=unwrap_or, (2)=is_some,
        // (3)=len, (4)=str_eq, (5)=u8_to_i64, (6)=i64_to_u8 (D.2 / ADR
        // 0033 D5), (7)=vec_new, (8)=push, (9)=pop, (10)=vec_to_array
        // (D.3 / ADR 0034 D5), (11)=read_file, (12)=write_file,
        // (13)=print_bytes (D.4 / ADR 0035 D4), (14..=20)=tcp_* (ADR 0056);
        // channels/subprocess/sealed/stdio/arg/apply occupy (21..=37), the ADR
        // 0071 M1.4a shared builtins (38..=39), the M1.4b mutex builtins (40..=41),
        // so (42)=main (first user fn). The generic
        // builtins occupy FnId(1..=3) per ADR 0014 D9 + ADR 0015 D4; the
        // byte-string builtins FnId(4..=6) per ADR 0033 D5; the collection
        // builtins FnId(7..=10) per ADR 0034 D5; the file-I/O builtins
        // FnId(11..=13) per ADR 0035.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[0].param_types, vec![Type::I64]);
        assert_eq!(p.fn_signatures[1].name, "unwrap_or");
        assert_eq!(p.fn_signatures[2].name, "is_some");
        assert_eq!(p.fn_signatures[3].name, "len");
        assert_eq!(p.fn_signatures[4].name, "str_eq");
        assert_eq!(p.fn_signatures[5].name, "u8_to_i64");
        assert_eq!(p.fn_signatures[6].name, "i64_to_u8");
        assert_eq!(p.fn_signatures[7].name, "vec_new");
        assert_eq!(p.fn_signatures[8].name, "push");
        assert_eq!(p.fn_signatures[9].name, "pop");
        assert_eq!(p.fn_signatures[10].name, "vec_to_array");
        assert_eq!(p.fn_signatures[11].name, "read_file");
        assert_eq!(p.fn_signatures[12].name, "write_file");
        assert_eq!(p.fn_signatures[13].name, "print_bytes");
        // ADR 0056: the TCP socket builtins occupy FnId(14..=20).
        assert_eq!(p.fn_signatures[14].name, "tcp_listen");
        assert_eq!(p.fn_signatures[15].name, "tcp_local_port");
        assert_eq!(p.fn_signatures[16].name, "tcp_accept");
        assert_eq!(p.fn_signatures[17].name, "tcp_connect");
        assert_eq!(p.fn_signatures[18].name, "tcp_read");
        assert_eq!(p.fn_signatures[19].name, "tcp_write");
        assert_eq!(p.fn_signatures[20].name, "tcp_close");
        // ADR 0066 M1.2: the channel builtins occupy FnId(21..=24).
        assert_eq!(p.fn_signatures[21].name, "channel_new");
        assert_eq!(p.fn_signatures[22].name, "send");
        assert_eq!(p.fn_signatures[23].name, "recv");
        assert_eq!(p.fn_signatures[24].name, "channel_close");
        // ADR 0066 M2.1/M2.2/M2.3: the subprocess builtins occupy FnId(25..=30).
        assert_eq!(p.fn_signatures[25].name, "process_spawn");
        assert_eq!(p.fn_signatures[26].name, "process_wait");
        assert_eq!(p.fn_signatures[27].name, "process_write");
        assert_eq!(p.fn_signatures[28].name, "process_read");
        assert_eq!(p.fn_signatures[29].name, "process_send");
        assert_eq!(p.fn_signatures[30].name, "process_recv");
        // ADR 0066 M2.4a: the SealedChannel bridge builtins occupy FnId(31..=32).
        assert_eq!(p.fn_signatures[31].name, "sealed_channel");
        assert_eq!(p.fn_signatures[32].name, "sealed_process");
        // ADR 0066 M2.4b: the self-stdin/stdout framed builtins occupy FnId(33..=34).
        assert_eq!(p.fn_signatures[33].name, "stdin_recv");
        assert_eq!(p.fn_signatures[34].name, "stdout_send");
        // ADR 0066 M2.4 follow-on: arg reflection occupies FnId(35..=36).
        assert_eq!(p.fn_signatures[35].name, "arg_count");
        assert_eq!(p.fn_signatures[36].name, "arg");
        // ADR 0070: the `apply` indirect-call builtin occupies FnId(37).
        assert_eq!(p.fn_signatures[37].name, "apply");
        // ADR 0071 M1.4a: the Shared<T> builtins occupy FnId(38..=39).
        assert_eq!(p.fn_signatures[38].name, "shared_new");
        assert_eq!(p.fn_signatures[39].name, "shared_get");
        // ADR 0071 M1.4b: the Mutex<T> builtins occupy FnId(40..=41).
        assert_eq!(p.fn_signatures[40].name, "mutex_new");
        assert_eq!(p.fn_signatures[41].name, "lock");
        assert_eq!(p.fn_signatures[42].name, "main");
        assert!(p.signature(main.id).is_main);
    }

    #[test]
    fn adr0070_direct_fn_value_call_matches_apply_call() {
        // ADR 0070 (D3-revisit): `op(5)` and `apply(op, 5)` must type-check
        // to the identical `Call{id: APPLY_FN_ID, ...}` shape — this is the
        // drift guard in place of a shared helper between check_call's
        // `apply` branch and check_resume_kont_expr's new `Type::Fn` arm.
        let via_apply = check_ok(
            "fn square(x: i64) -> i64 { x * x } \
             fn main() -> i64 { let op = square; apply(op, 5) }",
        );
        let via_direct = check_ok(
            "fn square(x: i64) -> i64 { x * x } \
             fn main() -> i64 { let op = square; op(5) }",
        );
        let apply_tail = &via_apply.main().body.tail;
        let direct_tail = &via_direct.main().body.tail;
        match (&apply_tail.kind, &direct_tail.kind) {
            (
                TypedExprKind::Call { id: id_a, args: args_a, type_args: ta_a, .. },
                TypedExprKind::Call { id: id_b, args: args_b, type_args: ta_b, .. },
            ) => {
                assert_eq!(*id_a, APPLY_FN_ID);
                assert_eq!(id_a, id_b);
                assert_eq!(ta_a, ta_b);
                assert_eq!(args_a.len(), 2);
                assert_eq!(args_b.len(), 2);
                assert_eq!(args_a[0].ty, args_b[0].ty, "the Fn-value arg's type must match");
                assert_eq!(args_a[1].ty, args_b[1].ty, "the value arg's type must match");
            }
            other => panic!("expected both spellings to produce Call{{APPLY_FN_ID}}, got {other:?}"),
        }
        assert_eq!(apply_tail.ty, direct_tail.ty);
        assert_eq!(apply_tail.ty, Type::I64);
    }

    #[test]
    fn adr0070_direct_call_on_non_callable_var_is_rejected() {
        let err = check_err("fn main() -> i64 { let x = 5; x(3) }");
        match err {
            TypeError::CalleeNotCallable { got, .. } => assert_eq!(got, Type::I64),
            other => panic!("expected CalleeNotCallable, got {other:?}"),
        }
    }

    #[test]
    fn adr0070_direct_call_wrong_arity_is_rejected() {
        let err = check_err(
            "fn square(x: i64) -> i64 { x * x } \
             fn main() -> i64 { let op = square; op(1, 2) }",
        );
        match err {
            TypeError::FnValueArityMismatch { expected, got, .. } => {
                assert_eq!(expected, 1);
                assert_eq!(got, 2);
            }
            other => panic!("expected FnValueArityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn adr0070_direct_call_wrong_arg_type_is_rejected() {
        let err = check_err(
            "fn square(x: i64) -> i64 { x * x } \
             fn main() -> i64 { let op = square; op(true) }",
        );
        match err {
            TypeError::FnValueArgMismatch { expected, got, .. } => {
                assert_eq!(expected, Type::I64);
                assert_eq!(got, Type::Bool);
            }
            other => panic!("expected FnValueArgMismatch, got {other:?}"),
        }
    }

    #[test]
    fn checks_let_with_matching_annotation() {
        let p = check_ok("fn main() -> i64 { let x: i64 = 5; x }");
        let body = &p.main().body;
        match &body.stmts[0].kind {
            TypedStmtKind::Let { ty, value, .. } => {
                assert_eq!(*ty, Type::I64);
                assert_eq!(value.ty, Type::I64);
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn checks_let_without_annotation_infers() {
        let p = check_ok("fn main() -> i64 { let x = 5; x }");
        let body = &p.main().body;
        match &body.stmts[0].kind {
            TypedStmtKind::Let { ty, .. } => assert_eq!(*ty, Type::I64),
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn checks_param_and_use() {
        let p = check_ok(
            "fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { double(7) }",
        );
        let double = &p.fns[0];
        assert_eq!(double.params[0].ty, Type::I64);
        assert_eq!(double.body.ty, Type::I64);
        let main = p.main();
        assert_eq!(main.body.ty, Type::I64);
    }

    #[test]
    fn checks_if_else_branches_match() {
        // C1.3: if-condition must be Bool. `if 1` rewrites to `if true`.
        let p = check_ok("fn main() -> i64 { if true { 10 } else { 20 } }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn if_condition_rejects_non_bool() {
        // ADR 0010 D9 retired at C1.3 step 5: `if x` with x: i64 is
        // a type error.
        let err = check_err("fn main() -> i64 { let x = 1; if x { 1 } else { 2 } }");
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Bool, got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn checks_call_to_print() {
        let p = check_ok("fn main() -> i64 { print(42) }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn checks_go_no_go_program() {
        // The C1.3 phase-go program type-checks end-to-end. The pick
        // function takes a bool condition per the ADR 0012 D10 / step 5
        // rewrite.
        let src = "\
fn double(x: i64) -> i64 { x * 2 }
fn is_positive(x: i64) -> bool { x > 0 }
fn pick(cond: bool, a: i64, b: i64) -> i64 { if cond { a } else { b } }
fn main() -> i64 {
    let x: i64 = 5;
    let y = pick(is_positive(x), double(x), 0);
    print(y)
}
";
        let p = check_ok(src);
        assert_eq!(p.fns.len(), 4);
        assert_eq!(p.main().body.ty, Type::I64);
    }

    // ----- error paths -----

    #[test]
    fn unknown_type_in_param_errors() {
        let err = check_err("fn id(x: foo) -> i64 { x }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "foo"),
            "got {err:?}"
        );
    }

    #[test]
    fn unknown_type_in_return_errors() {
        let err = check_err("fn main() -> foo { 0 }");
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "foo"),
            "got {err:?}"
        );
    }

    #[test]
    fn unknown_type_in_let_annotation_errors() {
        let err = check_err("fn main() -> i64 { let x: foo = 5; x }");
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "foo"),
            "got {err:?}"
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
    fn check_query_returns_some_for_valid_source() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { 42 }".to_string(),
        );
        let result = check_query(&db, file);
        assert!(result.is_some());
        let diags = check_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn check_query_emits_diagnostic_on_unknown_type() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> bogus { 0 }".to_string(),
        );
        let result = check_query(&db, file);
        assert!(result.is_none());
        let diags = check_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "types");
        assert_eq!(diags[0].code, "sentinel::types::unknown_type");
    }

    #[test]
    fn check_query_propagates_resolve_diagnostic_with_resolve_stage() {
        // Resolve-stage error → check_query short-circuits to None;
        // resolve_query's accumulated diagnostic flows transitively.
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { undeclared }".to_string(),
        );
        let result = check_query(&db, file);
        assert!(result.is_none());
        let diags = check_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "resolve");
    }

    #[test]
    fn check_query_caches_across_reruns() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { 1 + 2 }".to_string(),
        );
        let r1 = check_query(&db, file).clone();
        let r2 = check_query(&db, file).clone();
        assert_eq!(r1, r2);
        assert!(r1.is_some());
    }

    // ----- C1.3: bool literals + comparisons + logicals + unary ! -----

    #[test]
    fn bool_lit_true_types_to_bool() {
        let p = check_ok("fn pred() -> bool { true }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].return_type, Type::Bool);
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn bool_lit_false_types_to_bool() {
        let p = check_ok("fn pred() -> bool { false }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn comparison_produces_bool() {
        let p = check_ok("fn gt(x: i64, y: i64) -> bool { x > y }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn all_six_comparisons_produce_bool() {
        for op in ["==", "!=", "<", "<=", ">", ">="] {
            let src = format!(
                "fn cmp(x: i64, y: i64) -> bool {{ x {op} y }}\nfn main() -> i64 {{ 0 }}"
            );
            let p = check_ok(&src);
            assert_eq!(p.fns[0].body.ty, Type::Bool, "op = {op}");
        }
    }

    #[test]
    fn logic_and_requires_both_bool() {
        let p = check_ok("fn pred(b: bool) -> bool { b && true }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn logic_or_requires_both_bool() {
        let p = check_ok("fn pred(b: bool) -> bool { b || false }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn unary_not_requires_bool() {
        let p = check_ok("fn neg(b: bool) -> bool { !b }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    #[test]
    fn comparison_chain_in_logical_typechecks() {
        let p = check_ok(
            "fn between(x: i64, lo: i64, hi: i64) -> bool { x > lo && x < hi }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].body.ty, Type::Bool);
    }

    // ----- C1.3 error paths -----

    #[test]
    fn logical_and_rejects_int_operand() {
        let err = check_err("fn bad(x: i64) -> bool { x && true }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Bool, got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn logical_or_rejects_int_operand() {
        let err = check_err("fn bad() -> bool { true || 1 }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Bool, got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn unary_not_rejects_int() {
        let err = check_err("fn bad(x: i64) -> bool { !x }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Bool, got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn arithmetic_rejects_bool_operand() {
        // `+` on bool — should error.
        let err = check_err("fn bad(b: bool) -> bool { b + b }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::Mismatch { got: Type::Bool, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn comparison_rejects_mismatched_operands() {
        let err =
            check_err("fn bad(x: i64, b: bool) -> bool { x == b }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn return_type_mismatch_bool_for_i64() {
        let err = check_err("fn wrong() -> i64 { true }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(
                err,
                TypeError::ReturnTypeMismatch {
                    expected: Type::I64,
                    got: Type::Bool,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn call_arg_mismatch_int_for_bool_param() {
        let err = check_err(
            "fn takes_bool(b: bool) -> i64 { 0 }\nfn main() -> i64 { takes_bool(1) }",
        );
        assert!(
            matches!(
                err,
                TypeError::CallArgMismatch { expected: Type::Bool, got: Type::I64, .. }
            ),
            "got {err:?}"
        );
    }

    // ----- C1.3: i32 + bool universe sanity -----

    // ----- C2.0.2 / ADR 0017: refs + mutability + deref + assignment -----

    #[test]
    fn ref_param_type_resolves() {
        let p = check_ok("fn f(x: &i64) -> i64 { *x }\nfn main() -> i64 { 0 }");
        let param_ty = p.fns[0].params[0].ty;
        assert!(param_ty.is_ref());
        if let Type::Ref(id) = param_ty {
            let data = p.ref_data(id);
            assert!(!data.mutable);
            assert_eq!(data.inner, Type::I64);
        }
    }

    #[test]
    fn ref_mut_param_type_resolves() {
        let p = check_ok("fn f(x: &mut i64) -> i64 { *x }\nfn main() -> i64 { 0 }");
        let param_ty = p.fns[0].params[0].ty;
        if let Type::Ref(id) = param_ty {
            let data = p.ref_data(id);
            assert!(data.mutable);
            assert_eq!(data.inner, Type::I64);
        } else {
            panic!("expected Ref param");
        }
    }

    #[test]
    fn ref_interner_dedupes() {
        // Two distinct `&i64` mentions should reuse the same RefId.
        let p = check_ok(
            "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn main() -> i64 { 0 }",
        );
        let t0 = p.fns[0].params[0].ty;
        let t1 = p.fns[0].params[1].ty;
        assert_eq!(t0, t1);
    }

    #[test]
    fn ref_borrow_take_produces_ref_type() {
        let p = check_ok(
            "fn f() -> i64 { let x: i64 = 5; let r: &i64 = &x; *r }\nfn main() -> i64 { 0 }",
        );
        // The let-r binding's RHS is `&x`; its type is `&i64`.
        let body = &p.fns[0].body;
        match &body.stmts[1].kind {
            TypedStmtKind::Let { ty, .. } => assert!(ty.is_ref()),
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn deref_produces_inner_type() {
        let p = check_ok("fn read(x: &i64) -> i64 { *x }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].body.ty, Type::I64);
    }

    #[test]
    fn ref_in_array_rejected() {
        let err = check_err("fn f(xs: [&i64]) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::RefInArray { .. }), "got {err:?}");
    }

    #[test]
    fn ref_in_struct_field_rejected() {
        let err = check_err(
            "struct Bad { r: &i64 }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::RefInStructField { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_ref_rejected() {
        // `& &T` produces `&&T` at parse time (whitespace mandatory
        // since `&&` lexes as logical-and). The type checker rejects.
        let err = check_err("fn f(x: & &i64) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::NestedRef { .. }), "got {err:?}");
    }

    #[test]
    fn borrow_of_rvalue_rejected() {
        let err = check_err(
            "fn f() -> i64 { let r: &i64 = &5; *r }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::BorrowOfRvalue { .. }), "got {err:?}");
    }

    #[test]
    fn borrow_mut_of_immutable_rejected() {
        let err = check_err(
            "fn f() -> i64 { let counter: i64 = 5; let r: &mut i64 = &mut counter; *r }\nfn main() -> i64 { 0 }",
        );
        // The diagnostic must name the actual binding — regression guard:
        // this used to hardcode `x` regardless of the real name (a
        // distinctive name here would have passed the old placeholder).
        match err {
            TypeError::BorrowMutOfImmutable { name, .. } => assert_eq!(name, "counter"),
            other => panic!("expected BorrowMutOfImmutable, got {other:?}"),
        }
    }

    #[test]
    fn index_assign_scalar_and_secret_ok() {
        // ADR 0050: `a[i] = v;` type-checks for a mutable array of a Copy
        // element — a scalar and a `secret` scalar (the secret value is
        // stored at a PUBLIC index, so the access pattern is public).
        check_ok(
            "fn main() -> i64 { let mut a: [i64] = [1, 2, 3]; a[0] = 9; a[0] }",
        );
        check_ok(
            "fn main() -> i64 { let z: secret i64 = 0; let mut s: [secret i64] = [z, z]; let v: secret i64 = 7; s[1] = v; declassify(s[1]) }",
        );
    }

    #[test]
    fn index_assign_to_immutable_rejected() {
        // The base collection must be a mutable lvalue — the `Index` arm
        // recurses to the base Var, surfacing `AssignToImmutable` (ADR 0050).
        let err = check_err(
            "fn main() -> i64 { let a: [i64] = [1, 2, 3]; a[0] = 9; a[0] }",
        );
        match err {
            TypeError::AssignToImmutable { name, .. } => assert_eq!(name, "a"),
            other => panic!("expected AssignToImmutable, got {other:?}"),
        }
    }

    #[test]
    fn index_assign_secret_index_rejected() {
        // A SECRET index on the LHS is a timing leak — rejected by the same
        // `IndexNotInt` rule the read path uses (an index must be a public
        // `i64`), so write and read are symmetric (ADR 0050 constant-time).
        let err = check_err(
            "fn main() -> i64 { let mut a: [i64] = [1, 2, 3]; let s: secret i64 = 1; a[s] = 9; a[0] }",
        );
        assert!(matches!(err, TypeError::IndexNotInt { .. }), "got {err:?}");
    }

    #[test]
    fn index_assign_non_copy_element_rejected() {
        // A Move element (struct) would need drop-on-overwrite, deferred —
        // rejected with `IndexAssignNonCopyElem` (ADR 0050 MVP scope).
        let err = check_err(
            "struct P { x: i64 }\nfn main() -> i64 { let mut a: [P] = [P { x: 1 }]; a[0] = P { x: 2 }; 0 }",
        );
        assert!(
            matches!(err, TypeError::IndexAssignNonCopyElem { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn borrow_mut_of_immutable_param_names_the_binding() {
        // An immutable fn parameter: `&mut p` names `p` (params record
        // their source name in the type env alongside the type/mut bit).
        let err = check_err(
            "fn f(p: i64) -> i64 { let r: &mut i64 = &mut p; *r }\nfn main() -> i64 { 0 }",
        );
        match err {
            TypeError::BorrowMutOfImmutable { name, .. } => assert_eq!(name, "p"),
            other => panic!("expected BorrowMutOfImmutable, got {other:?}"),
        }
    }

    #[test]
    fn borrow_mut_of_mutable_ok() {
        let p = check_ok(
            "fn f() -> i64 { let mut x: i64 = 5; let r: &mut i64 = &mut x; *r }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].body.ty, Type::I64);
    }

    #[test]
    fn assign_to_immutable_rejected() {
        let err = check_err(
            "fn f() -> i64 { let total: i64 = 5; total = 7; total }\nfn main() -> i64 { 0 }",
        );
        // Same regression guard as the borrow case: the assign-to-
        // immutable diagnostic names the real binding, not a placeholder.
        match err {
            TypeError::AssignToImmutable { name, .. } => assert_eq!(name, "total"),
            other => panic!("expected AssignToImmutable, got {other:?}"),
        }
    }

    #[test]
    fn assign_to_mutable_ok() {
        let p = check_ok(
            "fn f() -> i64 { let mut x: i64 = 5; x = 7; x }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].body.ty, Type::I64);
    }

    #[test]
    fn deref_assign_through_mut_ref_ok() {
        let p = check_ok(
            "fn set(x: &mut i64, v: i64) -> i64 { *x = v; *x }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].body.ty, Type::I64);
    }

    #[test]
    fn deref_assign_through_shared_ref_rejected() {
        let err = check_err(
            "fn set(x: &i64, v: i64) -> i64 { *x = v; *x }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::AssignThroughSharedRef { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn deref_of_non_ref_rejected() {
        let err = check_err("fn f(x: i64) -> i64 { *x }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::DerefOfNonRef { got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn mut_param_allows_assign() {
        let p = check_ok(
            "fn f(mut x: i64) -> i64 { x = x + 1; x }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.fns[0].body.ty, Type::I64);
    }

    #[test]
    fn immutable_param_assign_rejected() {
        let err = check_err(
            "fn f(x: i64) -> i64 { x = x + 1; x }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::AssignToImmutable { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn ref_of_array_type_ok() {
        // `&[T]` — ref to array. Per ADR 0017 D1.
        let p = check_ok(
            "fn first(xs: &[i64]) -> i64 { (*xs)[0] }\nfn main() -> i64 { 0 }",
        );
        let param_ty = p.fns[0].params[0].ty;
        assert!(param_ty.is_ref());
    }

    #[test]
    fn ref_of_nullable_type_ok() {
        // `&?T` — ref to nullable. Per ADR 0017 D1.
        let p = check_ok("fn f(x: &?i64) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        let param_ty = p.fns[0].params[0].ty;
        assert!(param_ty.is_ref());
    }

    #[test]
    fn assign_type_mismatch_rejected() {
        let err = check_err(
            "fn f() -> i64 { let mut x: i64 = 5; x = true; x }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn i32_type_resolves() {
        let p = check_ok("fn echo(x: i32) -> i32 { x }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty, Type::I32);
        assert_eq!(p.fns[0].return_type, Type::I32);
    }

    #[test]
    fn bool_type_resolves() {
        let p = check_ok("fn echo(x: bool) -> bool { x }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty, Type::Bool);
        assert_eq!(p.fns[0].return_type, Type::Bool);
    }

    // ----- C1.4: structs + field access + struct literal -----

    #[test]
    fn struct_decl_typechecks() {
        let p = check_ok(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs.len(), 1);
        assert_eq!(p.structs[0].name, "Point");
        assert_eq!(p.structs[0].fields[0].ty, Type::I64);
        assert_eq!(p.structs[0].fields[1].ty, Type::I64);
    }

    #[test]
    fn struct_with_mixed_field_types() {
        let p = check_ok(
            "struct Mixed { i: i64, b: bool, j: i32 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[0].ty, Type::I64);
        assert_eq!(p.structs[0].fields[1].ty, Type::Bool);
        assert_eq!(p.structs[0].fields[2].ty, Type::I32);
    }

    #[test]
    fn struct_literal_typechecks() {
        let p = check_ok(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let p = P { x: 3, y: 4 }; 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { ty, value, .. } => {
                assert!(matches!(ty, Type::Struct(_)));
                assert_eq!(value.ty, *ty);
                match &value.kind {
                    TypedExprKind::StructLit { fields, .. } => {
                        assert_eq!(fields.len(), 2);
                        assert_eq!(fields[0].ty, Type::I64);
                        assert_eq!(fields[1].ty, Type::I64);
                    }
                    other => panic!("expected StructLit, got {other:?}"),
                }
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn field_access_typechecks() {
        let p = check_ok(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { x: 7 }; p.x }",
        );
        match &p.main().body.tail.kind {
            TypedExprKind::FieldAccess { field, field_index, .. } => {
                assert_eq!(field, "x");
                assert_eq!(*field_index, 0);
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        }
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn struct_in_fn_signature_typechecks() {
        let p = check_ok(
            "struct P { x: i64, y: i64 }\nfn sum(p: P) -> i64 { p.x + p.y }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(p.fns[0].params[0].ty, Type::Struct(_)));
    }

    #[test]
    fn struct_field_order_reordered_to_decl_order() {
        // Source order: `P { y: 4, x: 3 }`. Decl order: `x`, then `y`.
        // After check(), the fields vec is reordered to decl order.
        let p = check_ok(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let p = P { y: 4, x: 3 }; 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { value, .. } => match &value.kind {
                TypedExprKind::StructLit { fields, .. } => {
                    // After reorder: fields[0] is x=3, fields[1] is y=4
                    match &fields[0].kind {
                        TypedExprKind::IntLit(n) => assert_eq!(*n, 3),
                        other => panic!("expected IntLit 3 at index 0, got {other:?}"),
                    }
                    match &fields[1].kind {
                        TypedExprKind::IntLit(n) => assert_eq!(*n, 4),
                        other => panic!("expected IntLit 4 at index 1, got {other:?}"),
                    }
                }
                other => panic!("expected StructLit, got {other:?}"),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn struct_let_annotation_typechecks() {
        let p = check_ok(
            "struct P { x: i64 }\nfn main() -> i64 { let p: P = P { x: 1 }; 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { ty, .. } => assert!(matches!(ty, Type::Struct(_))),
            _ => unreachable!(),
        }
    }

    // ----- C1.4 error paths -----

    #[test]
    fn struct_literal_unknown_field_errors() {
        let err = check_err(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { y: 1 }; 0 }",
        );
        assert!(
            matches!(err, TypeError::UnknownField { ref field, ref struct_name, .. } if field == "y" && struct_name == "P"),
            "got {err:?}"
        );
    }

    #[test]
    fn struct_literal_missing_field_errors() {
        let err = check_err(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let p = P { x: 1 }; 0 }",
        );
        assert!(
            matches!(err, TypeError::MissingField { ref field, ref struct_name, .. } if field == "y" && struct_name == "P"),
            "got {err:?}"
        );
    }

    #[test]
    fn struct_literal_field_type_mismatch_errors() {
        let err = check_err(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { x: true }; 0 }",
        );
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::I64, got: Type::Bool, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn field_access_unknown_field_errors() {
        let err = check_err(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { x: 1 }; p.y }",
        );
        assert!(
            matches!(err, TypeError::UnknownField { ref field, ref struct_name, .. } if field == "y" && struct_name == "P"),
            "got {err:?}"
        );
    }

    #[test]
    fn field_access_on_non_struct_errors() {
        let err =
            check_err("fn main() -> i64 { let x = 5; x.y }");
        assert!(
            matches!(err, TypeError::FieldAccessOnNonStruct { got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn struct_cmp_eq_rejected_for_now() {
        // ADR 0013 D6: struct == struct is deferred.
        let err = check_err(
            "struct P { x: i64 }\nfn eq(a: P, b: P) -> bool { a == b }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn struct_arithmetic_rejected() {
        // struct + struct is rejected (arithmetic requires int).
        let err = check_err(
            "struct P { x: i64 }\nfn add(a: P, b: P) -> P { a + b }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn recursive_struct_direct_errors() {
        let err = check_err(
            "struct Node { next: Node }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::RecursiveStruct { ref name, .. } if name == "Node"),
            "got {err:?}"
        );
    }

    #[test]
    fn recursive_struct_mutual_errors() {
        let err = check_err(
            "struct A { b: B }\nstruct B { a: A }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::RecursiveStruct { .. }), "got {err:?}");
    }

    #[test]
    fn non_recursive_struct_chain_ok() {
        // A contains B contains nothing. No cycle.
        let _ = check_ok(
            "struct B { x: i64 }\nstruct A { b: B }\nfn main() -> i64 { 0 }",
        );
    }

    #[test]
    fn empty_struct_typechecks() {
        let p = check_ok(
            "struct Empty { }\nfn main() -> i64 { 0 }",
        );
        assert!(p.structs[0].fields.is_empty());
    }

    // ----- C1.5: nullable types + null literal + builtins -----

    #[test]
    fn nullable_type_resolves() {
        let p = check_ok("fn f(x: ?i64) -> i64 { unwrap_or(x, 0) }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty, Type::Nullable(NullableInner::I64));
    }

    #[test]
    fn null_with_annotation_typechecks() {
        let _ = check_ok("fn main() -> i64 { let x: ?i64 = null; 0 }");
    }

    #[test]
    fn null_without_annotation_errors() {
        let err = check_err("fn main() -> i64 { let x = null; 0 }");
        assert!(matches!(err, TypeError::AmbiguousNull { .. }), "got {err:?}");
    }

    #[test]
    fn implicit_widen_i64_to_nullable_i64() {
        // `let x: ?i64 = 42;` — `42` (I64) widens to ?i64.
        let _ = check_ok("fn main() -> i64 { let x: ?i64 = 42; 0 }");
    }

    #[test]
    fn implicit_widen_in_call_arg() {
        let _ = check_ok(
            "fn takes_opt(x: ?i64) -> i64 { 0 }\nfn main() -> i64 { takes_opt(7) }",
        );
    }

    #[test]
    fn unwrap_or_typechecks() {
        let p = check_ok(
            "fn main() -> i64 { let x: ?i64 = 42; unwrap_or(x, 0) }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn is_some_typechecks() {
        let p = check_ok(
            "fn main() -> i64 { let x: ?i64 = null; if is_some(x) { 1 } else { 0 } }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn unwrap_or_with_non_nullable_errors() {
        let err = check_err(
            "fn main() -> i64 { unwrap_or(5, 0) }",
        );
        // The first arg is I64, not ?T — special-cased Mismatch.
        assert!(matches!(err, TypeError::Mismatch { got: Type::I64, .. }), "got {err:?}");
    }

    #[test]
    fn cmp_eq_against_null_typechecks() {
        let p = check_ok(
            "fn main() -> i64 { let x: ?i64 = null; if x == null { 1 } else { 0 } }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn cmp_ne_against_null_typechecks() {
        let p = check_ok(
            "fn main() -> i64 { let x: ?i64 = null; if x != null { 1 } else { 0 } }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn cmp_lt_against_null_errors() {
        // `<` on nullable is rejected per ADR 0014 D7.
        let err = check_err(
            "fn main() -> i64 { let x: ?i64 = null; if x < null { 1 } else { 0 } }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn cmp_null_against_non_nullable_errors() {
        let err = check_err(
            "fn main() -> i64 { let x = 5; if x == null { 1 } else { 0 } }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn nullable_in_fn_return_typechecks() {
        let _ = check_ok(
            "fn maybe(c: bool) -> ?i64 { if c { 42 } else { null } }\nfn main() -> i64 { 0 }",
        );
    }

    #[test]
    fn nullable_struct_field_typechecks() {
        // A struct with a nullable field (non-recursive). This works
        // even though recursive nullable structs are rejected per
        // the C1.5 codegen limitation noted in detect_struct_cycle.
        let p = check_ok(
            "struct Pair { first: ?i64, second: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(
            p.structs[0].fields[0].ty,
            Type::Nullable(NullableInner::I64)
        );
        assert_eq!(p.structs[0].fields[1].ty, Type::I64);
    }

    #[test]
    fn recursive_nullable_struct_now_accepted() {
        // C1.6 / ADR 0015 D11: the ADR 0014 D10 deferral is now
        // implemented. Recursive structs via nullable edges are
        // accepted because `?Struct` uses heap indirection in
        // codegen — the cycle is broken at runtime by the pointer.
        let p = check_ok(
            "struct Node { value: i64, next: ?Node }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].name, "Node");
        // First field is i64; second is ?Node (Nullable struct).
        assert_eq!(p.structs[0].fields[0].ty, Type::I64);
        match p.structs[0].fields[1].ty {
            Type::Nullable(NullableInner::Struct(id)) => {
                assert_eq!(id, p.structs[0].id);
            }
            other => panic!("expected ?Node, got {other:?}"),
        }
    }

    #[test]
    fn direct_recursive_struct_still_rejected() {
        // `struct Bad { x: Bad }` — direct cycle, no nullable
        // indirection, still rejected.
        let err = check_err(
            "struct Bad { x: Bad }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::RecursiveStruct { .. }), "got {err:?}");
    }

    #[test]
    fn nullable_display_renders_with_question_prefix() {
        let p = check_ok("fn f(x: ?i64) -> i64 { unwrap_or(x, 0) }\nfn main() -> i64 { 0 }");
        let param_ty = p.fns[0].params[0].ty;
        assert_eq!(type_display(param_ty, None), "?i64");
        assert_eq!(format!("{param_ty}"), "?i64");
    }

    #[test]
    fn nullable_inner_to_type_roundtrip() {
        assert_eq!(NullableInner::I64.to_type(), Type::I64);
        assert_eq!(NullableInner::Bool.to_type(), Type::Bool);
        assert_eq!(
            Type::I64.to_nullable_inner(),
            Some(NullableInner::I64)
        );
        assert_eq!(
            Type::Nullable(NullableInner::I64).to_nullable_inner(),
            None
        );
    }

    // ----- C1.6: arrays + indexing + len + recursive struct unlock -----

    #[test]
    fn array_type_resolves() {
        let p = check_ok("fn f(xs: [i64]) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        assert_eq!(p.fns[0].params[0].ty, Type::Array(ArrayElem::I64));
    }

    #[test]
    fn array_literal_typechecks() {
        let p = check_ok("fn main() -> i64 { let xs = [1, 2, 3]; 0 }");
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { ty, value, .. } => {
                assert_eq!(*ty, Type::Array(ArrayElem::I64));
                assert_eq!(value.ty, Type::Array(ArrayElem::I64));
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn array_index_typechecks() {
        let p = check_ok("fn main() -> i64 { let xs = [42]; xs[0] }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn len_builtin_typechecks() {
        let p = check_ok("fn main() -> i64 { let xs = [1, 2, 3]; len(xs) }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn empty_array_with_annotation_typechecks() {
        let _ = check_ok("fn main() -> i64 { let xs: [i64] = []; 0 }");
    }

    #[test]
    fn empty_array_without_annotation_errors() {
        let err = check_err("fn main() -> i64 { let xs = []; 0 }");
        assert!(matches!(err, TypeError::AmbiguousEmptyArray { .. }), "got {err:?}");
    }

    #[test]
    fn array_index_on_non_array_errors() {
        let err = check_err("fn main() -> i64 { let x = 5; x[0] }");
        assert!(
            matches!(err, TypeError::IndexOnNonArray { got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn array_index_non_int_errors() {
        let err = check_err("fn main() -> i64 { let xs = [1]; xs[true] }");
        assert!(
            matches!(err, TypeError::IndexNotInt { got: Type::Bool, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_array_typechecks() {
        // ADR 0068: `[[T]]` is representable now — the depth-1 array rule (ADR 0015
        // D6) is lifted via the `arrays` interner. `[[i64]]` resolves to
        // `Array(Array(id))` with the inner element `i64` interned into `arrays`.
        let p = check_ok("fn f(x: [[i64]]) -> i64 { len(x) }\nfn main() -> i64 { 0 }");
        // The `[[i64]]` param interns its inner `[i64]` element (`ArrayElem::I64`).
        // (`arrays` also holds `ArrayElem::U8` from the `process_spawn` builtin's
        // `[[u8]]` arg type, interned at sig-setup — ADR 0066 M2.1.)
        assert!(p.arrays.contains(&ArrayElem::I64), "the inner `[i64]` element is interned");
    }

    #[test]
    fn array_mixed_element_types_errors() {
        let err = check_err("fn main() -> i64 { let xs = [1, true]; 0 }");
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn len_on_non_array_errors() {
        let err = check_err("fn main() -> i64 { len(5) }");
        assert!(matches!(err, TypeError::Mismatch { got: Type::I64, .. }), "got {err:?}");
    }

    #[test]
    fn array_in_struct_field_typechecks() {
        let p = check_ok(
            "struct Bag { items: [i64] }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].fields[0].ty, Type::Array(ArrayElem::I64));
    }

    #[test]
    fn linked_list_struct_typechecks() {
        // C1.6 / ADR 0015 D11: the ADR 0014 D10 unlock — recursive
        // structs via `?T` work now.
        let p = check_ok(
            "struct Node { value: i64, next: ?Node }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].name, "Node");
    }

    #[test]
    fn c16_phasego_sum_typechecks() {
        let src = "\
fn sum_from(a: [i64], i: i64) -> i64 {
    if i == len(a) { 0 } else { a[i] + sum_from(a, i + 1) }
}
fn main() -> i64 {
    let arr: [i64] = [1, 2, 3, 4, 5];
    sum_from(arr, 0)
}
";
        let p = check_ok(src);
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn c15_phasego_value_program_typechecks() {
        let src = "\
fn find_or(x: ?i64, default: i64) -> i64 {
    unwrap_or(x, default)
}
fn main() -> i64 {
    let some: ?i64 = 42;
    let none: ?i64 = null;
    print(find_or(some, 0) + find_or(none, 100))
}
";
        let p = check_ok(src);
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn c14_phasego_program_typechecks() {
        let src = "\
struct Point { x: i64, y: i64 }
fn manhattan(p: Point) -> i64 { p.x + p.y }
fn main() -> i64 {
    let p = Point { x: 3, y: 4 };
    print(manhattan(p))
}
";
        let p = check_ok(src);
        assert_eq!(p.structs.len(), 1);
        assert_eq!(p.fns.len(), 2);
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn c13_phasego_program_typechecks() {
        let src = "\
fn double(x: i64) -> i64 { x * 2 }
fn is_positive(x: i64) -> bool { x > 0 }
fn pick(cond: bool, a: i64, b: i64) -> i64 { if cond { a } else { b } }
fn main() -> i64 {
    let x: i64 = 5;
    let y = pick(is_positive(x), double(x), 0);
    print(y)
}
";
        let p = check_ok(src);
        assert_eq!(p.fns.len(), 4);
        assert_eq!(p.main().body.ty, Type::I64);
    }

    // ----- C1.7.4a / ADR 0016: generic fns typing -----

    #[test]
    fn c17_generic_fn_signature_uses_type_param() {
        let p = check_ok("fn id<T>(x: T) -> T { x }\nfn main() -> i64 { 0 }");
        let id_fn = p.fns.iter().find(|f| f.name == "id").expect("id");
        assert_eq!(id_fn.type_params.len(), 1);
        assert_eq!(id_fn.type_params[0].name, "T");
        // The signature's param + return types must use TypeParam(0).
        let sig = p.signature(id_fn.id);
        assert_eq!(sig.param_types, vec![Type::TypeParam(TypeParamId(0))]);
        assert_eq!(sig.return_type, Type::TypeParam(TypeParamId(0)));
        assert_eq!(sig.type_params.len(), 1);
    }

    #[test]
    fn c17_builtins_have_generic_signatures() {
        let p = check_ok("fn main() -> i64 { 0 }");
        // unwrap_or<T>(x: ?T, default: T) -> T
        let unwrap = &p.fn_signatures[1];
        assert_eq!(unwrap.name, "unwrap_or");
        assert_eq!(unwrap.type_params.len(), 1);
        assert_eq!(
            unwrap.param_types,
            vec![
                Type::Nullable(NullableInner::TypeParam(TypeParamId(0))),
                Type::TypeParam(TypeParamId(0)),
            ]
        );
        assert_eq!(unwrap.return_type, Type::TypeParam(TypeParamId(0)));
        // is_some<T>(x: ?T) -> bool
        let is_some = &p.fn_signatures[2];
        assert_eq!(is_some.name, "is_some");
        assert_eq!(is_some.type_params.len(), 1);
        assert_eq!(
            is_some.param_types,
            vec![Type::Nullable(NullableInner::TypeParam(TypeParamId(0)))],
        );
        assert_eq!(is_some.return_type, Type::Bool);
        // len<T>(a: [T]) -> i64
        let len = &p.fn_signatures[3];
        assert_eq!(len.name, "len");
        assert_eq!(len.type_params.len(), 1);
        assert_eq!(
            len.param_types,
            vec![Type::Array(ArrayElem::TypeParam(TypeParamId(0)))]
        );
        assert_eq!(len.return_type, Type::I64);
    }

    #[test]
    fn c17_generic_fn_with_nullable_param_typechecks() {
        // A generic fn whose param is `?T` — exercises NullableInner::TypeParam.
        let p = check_ok(
            "fn first_or<T>(x: ?T, default: T) -> T { unwrap_or(x, default) }\nfn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "first_or").expect("first_or");
        assert_eq!(f.type_params.len(), 1);
        let sig = p.signature(f.id);
        assert_eq!(
            sig.param_types,
            vec![
                Type::Nullable(NullableInner::TypeParam(TypeParamId(0))),
                Type::TypeParam(TypeParamId(0)),
            ]
        );
    }

    #[test]
    fn c17_generic_fn_with_array_param_typechecks() {
        let p = check_ok(
            "fn count<T>(a: [T]) -> i64 { len(a) }\nfn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "count").expect("count");
        let sig = p.signature(f.id);
        assert_eq!(
            sig.param_types,
            vec![Type::Array(ArrayElem::TypeParam(TypeParamId(0)))]
        );
        assert_eq!(sig.return_type, Type::I64);
    }

    #[test]
    fn c17_call_to_unwrap_or_infers_t_from_first_arg() {
        // unwrap_or(maybe_i64, 0) — infer T = i64 from arg[0]'s ?i64.
        let p = check_ok(
            "fn main() -> i64 { let x: ?i64 = null; unwrap_or(x, 0) }",
        );
        // Find the unwrap_or call in main; verify type_args.
        match &p.main().body.tail.kind {
            TypedExprKind::Call { id, type_args, .. } => {
                assert_eq!(*id, FnId(1)); // unwrap_or
                assert_eq!(type_args, &vec![Type::I64]);
            }
            other => panic!("expected Call in tail, got {other:?}"),
        }
    }

    #[test]
    fn c17_call_to_len_infers_t() {
        let p = check_ok(
            "fn main() -> i64 { let xs: [bool] = [true, false]; len(xs) }",
        );
        match &p.main().body.tail.kind {
            TypedExprKind::Call { id, type_args, .. } => {
                assert_eq!(*id, FnId(3)); // len
                assert_eq!(type_args, &vec![Type::Bool]);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn c17_call_to_is_some_infers_t() {
        let p = check_ok(
            "fn main() -> i64 { let x: ?bool = null; if is_some(x) { 1 } else { 0 } }",
        );
        // The body is an if; pull the cond.
        match &p.main().body.tail.kind {
            TypedExprKind::If { cond, .. } => match &cond.kind {
                TypedExprKind::Call { id, type_args, .. } => {
                    assert_eq!(*id, FnId(2)); // is_some
                    assert_eq!(type_args, &vec![Type::Bool]);
                }
                other => panic!("expected Call inside If cond, got {other:?}"),
            },
            other => panic!("expected If in tail, got {other:?}"),
        }
    }

    #[test]
    fn c17_generic_main_rejected() {
        let err = check_err("fn main<T>() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::GenericMain { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_generic_struct_decl_typechecks() {
        // C1.7.4b: generic struct decls are now supported. Field
        // types reference TypeParam(_) inside the struct's scope.
        let p = check_ok("struct Box<T> { value: T }\nfn main() -> i64 { 0 }");
        let s = &p.structs[0];
        assert_eq!(s.name, "Box");
        assert_eq!(s.type_params.len(), 1);
        // value: T → TypeParam(0)
        assert_eq!(s.fields[0].ty, Type::TypeParam(TypeParamId(0)));
    }

    #[test]
    fn c17_generic_instance_in_signature_typechecks() {
        // `Box<i64>` in type position now resolves to a
        // Type::GenericInstance, interned in program.generic_instances.
        let p = check_ok(
            "struct Box<T> { value: T }\n\
             fn unbox(b: Box<i64>) -> i64 { b.value }\n\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "unbox").expect("unbox");
        let sig = p.signature(f.id);
        // param[0]: Box<i64> → GenericInstance referencing
        // (BoxStructId, [I64]).
        match sig.param_types[0] {
            Type::GenericInstance(gi_id) => {
                let inst = p.generic_instance(gi_id);
                assert_eq!(inst.struct_id, StructId(0));
                assert_eq!(inst.args, vec![Type::I64]);
            }
            other => panic!("expected GenericInstance, got {other:?}"),
        }
    }

    #[test]
    fn c17_generic_struct_arity_mismatch() {
        // Box takes 1 type-arg; giving 2 surfaces TypeArgCountMismatch.
        let err = check_err(
            "struct Box<T> { value: T }\n\
             fn unbox(b: Box<i64, bool>) -> i64 { 0 }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::TypeArgCountMismatch { ref type_name, expected: 1, found: 2, .. } if type_name == "Box"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_missing_type_args_on_generic_struct() {
        // Bare `Box` without args surfaces MissingTypeArgs.
        let err = check_err(
            "struct Box<T> { value: T }\n\
             fn unbox(b: Box) -> i64 { 0 }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::MissingTypeArgs { ref type_name, expected_count: 1, .. } if type_name == "Box"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_type_args_on_non_generic() {
        // `i64<bool>` is meaningless.
        let err = check_err("fn f(b: i64<bool>) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, TypeError::TypeArgsOnNonGeneric { ref type_name, .. } if type_name == "i64"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_unwrap_or_mismatch_first_arg_keeps_legacy_shape() {
        // `unwrap_or(5, 0)` — first arg should be ?T, got i64.
        // Pre-C1.7 surfaced this as Mismatch with a hint; the new
        // generic-call path keeps Mismatch because the failing param
        // mentions an unbound TypeParam.
        let err = check_err("fn main() -> i64 { unwrap_or(5, 0) }");
        assert!(
            matches!(err, TypeError::Mismatch { got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_generic_call_arg_conflict_detected() {
        // unwrap_or<T>(?T, T): arg[0] is ?bool so T = bool; arg[1] is
        // i64 — conflicts. The standard Mismatch / CallArgMismatch
        // path triggers via the second-arg bidirectional pushdown
        // (T = bool, expected ?bool inner -> bool, got i64).
        let err = check_err(
            "fn main() -> i64 { let x: ?bool = null; unwrap_or(x, 0) }",
        );
        // Either CallArgMismatch (concrete pushdown caught it) or
        // TypeArgInferenceConflict (unify path) is acceptable here —
        // both indicate the right diagnosis to the user. The legacy
        // C1.5 special-case fired CallArgMismatch.
        assert!(
            matches!(
                err,
                TypeError::CallArgMismatch { .. } | TypeError::TypeArgInferenceConflict { .. }
            ),
            "got {err:?}"
        );
    }

    // ---- C3.1 / ADR 0019 D5: `secret T` interns into Type::Secret ----

    #[test]
    fn c31_secret_in_param_type_interns() {
        // A fn with `secret i64` in its signature type-checks; the
        // body still has to return the declared type (i64 here).
        let p = check_ok("fn f(x: secret i64) -> i64 { 0 } fn main() -> i64 { 0 }");
        // One secret entry should be in the program's interner.
        assert_eq!(p.secrets.len(), 1);
        assert_eq!(p.secrets[0].inner, Type::I64);
    }

    #[test]
    fn c31_secret_in_return_type_interns() {
        // `secret bool` return type type-checks when the body
        // tail-evaluates to `secret bool` (here: declassify a
        // dummy expression — defer once declassify lands. For
        // C3.1 minimum we accept that the body can't easily
        // produce a secret literal without widening; this test
        // just exercises the signature-side interner).
        let p = check_ok(
            "fn f(x: secret bool) -> bool { false } fn main() -> i64 { 0 }",
        );
        assert_eq!(p.secrets.len(), 1);
        assert_eq!(p.secrets[0].inner, Type::Bool);
    }

    #[test]
    fn c31_secret_dedupes_via_interner() {
        // Two `secret i64` annotations share one SecretId.
        let p = check_ok(
            "fn f(x: secret i64, y: secret i64) -> i64 { 0 } fn main() -> i64 { 0 }",
        );
        assert_eq!(p.secrets.len(), 1);
    }

    #[test]
    fn c31_secret_inside_ref_interns() {
        // `& secret T` — the secret is the inner type; the ref is
        // public. Allowed per ADR 0019 D5.
        let p = check_ok("fn f(x: & secret i64) -> i64 { 0 } fn main() -> i64 { 0 }");
        // The ref + the secret it wraps both land in the interner
        // tables.
        assert_eq!(p.secrets.len(), 1);
        assert!(p.refs.iter().any(|r| matches!(r.inner, Type::Secret(_))));
    }

    // ---- C3.1 / ADR 0019 D5 + D6: implicit widening + declassify ----

    #[test]
    fn c31_implicit_widen_at_let_annotation() {
        // `let pw: secret i64 = 42;` — RHS is i64, annotation is
        // secret i64. coerce_to_expected inserts WidenToSecret.
        let p = check_ok(
            "fn main() -> i64 { let pw: secret i64 = 42; 0 }",
        );
        assert_eq!(p.secrets.len(), 1);
    }

    #[test]
    fn c31_declassify_strips_secret() {
        // `declassify(s)` where s: secret i64 produces i64.
        let p = check_ok(
            "fn unwrap(s: secret i64) -> i64 { declassify(s) }\
             fn main() -> i64 { 0 }",
        );
        let unwrap = p.fns.iter().find(|f| f.name == "unwrap").expect("unwrap");
        assert_eq!(unwrap.body.ty, Type::I64);
    }

    #[test]
    fn c31_declassify_idempotent_on_non_secret() {
        // Phase B ADR 0008 D5: declassify on a non-secret type
        // is a no-op (the inner.ty just flows through).
        let p = check_ok(
            "fn main() -> i64 { let x: i64 = 5; declassify(x) }",
        );
        let main = p.main();
        assert_eq!(main.body.ty, Type::I64);
    }

    // ---- C3.1 / ADR 0019 D7: constant-time rejections ----

    #[test]
    fn c31_if_on_secret_bool_rejects_secret_branch() {
        let err = check_err(
            "fn main() -> i64 { let b: secret bool = true; if b { 1 } else { 0 } }",
        );
        assert!(
            matches!(err, TypeError::SecretBranch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c31_if_on_public_bool_after_declassify_accepts() {
        // The natural fix for SecretBranch: declassify before
        // branching.
        let p = check_ok(
            "fn main() -> i64 { let b: secret bool = true; if declassify(b) { 1 } else { 0 } }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn c31_deref_secret_ref_rejects_secret_in_ref_deref() {
        // `secret &T` (secret pointer) — deref'ing it leaks the
        // pointer via the memory side channel.
        let err = check_err(
            "fn main() -> i64 { let x: i64 = 5; let r: secret &i64 = &x; *r }",
        );
        assert!(
            matches!(err, TypeError::SecretInRefDeref { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c31_deref_ref_to_secret_accepts() {
        // `& secret T` (pointer to secret) — deref produces
        // `secret T`. Allowed; only the pointee is secret.
        let p = check_ok(
            "fn main() -> i64 { let x: secret i64 = 5; let r: & secret i64 = &x; declassify(*r) }",
        );
        assert_eq!(p.main().body.ty, Type::I64);
    }

    // ---- C3.1b / ADR 0019 D5+D7: operator-secret-preserving ----

    #[test]
    fn c31_secret_arithmetic_preserves_secret() {
        // `secret i64 + secret i64 -> secret i64`. The result of
        // `a + b` in the body has type `secret i64`; declassify
        // strips it.
        let p = check_ok(
            "fn add_secrets(a: secret i64, b: secret i64) -> i64 { declassify(a + b) }\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "add_secrets").expect("add_secrets");
        // The body tail is declassify((a + b)); the declassify
        // strips. So the tail type is i64.
        assert_eq!(f.body.ty, Type::I64);
    }

    #[test]
    fn c31_mixed_public_secret_arithmetic_widens() {
        // ADR 0051: `secret i64 + i64` now WIDENS the public operand to secret
        // (result secret), so it type-checks (it was a Mismatch pre-0051).
        check_ok(
            "fn f(a: secret i64, b: i64) -> i64 { declassify(a + b) }\
             fn main() -> i64 { 0 }",
        );
        // The constant-time-sink exclusions are NOT widened: a secret divisor
        // path (`secret / public` stays a Mismatch — the divisor must be public)
        // and a secret SHIFT AMOUNT (SecretShiftAmount) still reject.
        let div = check_err(
            "fn f(a: secret i64, b: i64) -> i64 { declassify(a / b) }\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(div, TypeError::Mismatch { .. }), "got {div:?}");
        let sh = check_err(
            "fn f(a: secret i64, n: secret i64) -> i64 { declassify(a << n) }\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(sh, TypeError::SecretShiftAmount { .. }), "got {sh:?}");
    }

    #[test]
    fn adr0051_widen_forms_typecheck_and_stay_constant_time() {
        // Operand widen (cmp) -> secret bool; the result still cannot branch.
        check_ok(
            "fn f(a: secret i64) -> bool { declassify(a == 5) }\nfn main() -> i64 { 0 }",
        );
        let branch = check_err(
            "fn f(a: secret i64) -> i64 { if a == 5 { 1 } else { 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(branch, TypeError::SecretBranch { .. }), "got {branch:?}");
        // Call-arg widen: a public argument flows into a secret parameter.
        check_ok(
            "fn g(x: secret i64) -> i64 { declassify(x) }\nfn main() -> i64 { g(7) }",
        );
        // Array widen: a public `[u8]` flows into a `[secret u8]` annotation.
        check_ok(
            "fn main() -> i64 { let p: [u8] = [i64_to_u8(1)]; let s: [secret u8] = p; u8_to_i64(declassify(s[0])) }",
        );
        // Return widen: a public-typed tail widens to the secret return type.
        check_ok(
            "fn h() -> secret i64 { 9 }\nfn main() -> i64 { declassify(h()) }",
        );
    }

    #[test]
    fn c31_secret_cmp_yields_secret_bool() {
        // `secret i64 == secret i64 -> secret bool`. The body
        // declassifies the comparison to a public bool, then
        // converts to i64.
        let p = check_ok(
            "fn eq_secrets(a: secret i64, b: secret i64) -> i64 { if declassify(a == b) { 1 } else { 0 } }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn c31_secret_logic_preserves_secret() {
        // `secret bool && secret bool -> secret bool`. Declassify
        // to compare against literal.
        let p = check_ok(
            "fn f(a: secret bool, b: secret bool) -> i64 { if declassify(a && b) { 1 } else { 0 } }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn c31_unary_neg_preserves_secret() {
        // `-secret_i64 -> secret i64`.
        let p = check_ok(
            "fn neg_secret(x: secret i64) -> i64 { declassify(-x) }\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "neg_secret").expect("neg_secret");
        assert_eq!(f.body.ty, Type::I64);
    }

    #[test]
    fn c31_unary_not_preserves_secret() {
        // `!secret_bool -> secret bool`.
        let p = check_ok(
            "fn not_secret(x: secret bool) -> i64 { if declassify(!x) { 1 } else { 0 } }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn c31_secret_divisor_rejects() {
        // `secret_a / secret_b` — rejected with SecretDivisor.
        // Variable-time division on a secret divisor leaks the
        // divisor's bit pattern.
        let err = check_err(
            "fn f(a: secret i64, b: secret i64) -> i64 { declassify(a / b) }\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::SecretDivisor { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c31_public_div_unaffected() {
        // Confirms SecretDivisor only fires when the divisor is
        // secret — public divisor is fine.
        let p = check_ok("fn main() -> i64 { 10 / 2 }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn adr0048_secret_value_public_amount_shift_ok() {
        // ADR 0048: a `secret` value shifted by a PUBLIC amount is
        // constant-time and accepted; the result stays secret (so
        // `declassify` — which requires a secret operand — type-checks).
        // This is the deliberate exception to the matching-secrecy rule.
        let p = check_ok(
            "fn f(a: secret i64, n: i64) -> i64 { declassify(a << n) }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn adr0048_secret_shift_amount_rejects() {
        // ADR 0048: shifting by a `secret` amount is variable-time and
        // rejected with SecretShiftAmount (the analogue of SecretDivisor).
        let err = check_err(
            "fn f(a: secret i64, n: secret i64) -> i64 { declassify(a << n) }\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::SecretShiftAmount { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn adr0048_shift_right_is_logical_typing_ok() {
        // `>>` types the same as `<<` (the lshr-vs-ashr choice is codegen's);
        // a public shift just produces the left operand's type.
        let p = check_ok("fn main() -> i64 { 256 >> 2 }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn adr0049_cast_constructs_i32_and_roundtrips() {
        // ADR 0049: `i64 as i32` (truncate) then `i32 as i64` (sign-extend).
        let p = check_ok("fn main() -> i64 { (5 as i32) as i64 }");
        assert_eq!(p.main().body.ty, Type::I64);
    }

    #[test]
    fn adr0049_cast_preserves_secrecy() {
        // A cast of a `secret` value stays secret (data-independent, constant-
        // time — NOT a sink); so `declassify` (which requires a secret operand)
        // type-checks on the cast result.
        let p = check_ok(
            "fn f(x: secret i64) -> i64 { declassify((x as i32) as i64) }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn adr0049_non_integer_cast_rejects() {
        // The operand and target must be integers; casting a `bool` is rejected.
        let err = check_err(
            "fn f(b: bool) -> i64 { (b as i64) }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::NonIntegerCast { .. }),
            "got {err:?}"
        );
    }

    // ---- C5.3 / ADR 0027 D5: bitwise operators (secret-preserving) ----

    #[test]
    fn c53_bitwise_xor_secret_secret_ok() {
        // `secret ^ secret -> secret`, declassified back to i64. Bitwise
        // ops are constant-time, so a secret operand is fine (no leak).
        let p = check_ok(
            "fn f(a: secret i64, b: secret i64) -> i64 { declassify(a ^ b) }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn c53_bitwise_secret_public_widens() {
        // ADR 0051: `secret ^ public` now widens the public operand to secret
        // (result secret), so it type-checks — the same as `secret + public`
        // (it was a Mismatch pre-0051).
        check_ok(
            "fn f(a: secret i64, b: i64) -> i64 { declassify(a ^ b) }\
             fn main() -> i64 { 0 }",
        );
    }

    #[test]
    fn c53_bitwise_on_bool_rejected() {
        // Bitwise ops are integer-only; `&`/`|`/`^` on `bool` is a
        // Mismatch — `&&` / `||` remain the boolean operators.
        let err = check_err("fn main() -> i64 { let r = true & true; 0 }");
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    // ---- C3.2(a) / ADR 0019 D4: effect_decls type-check ----

    #[test]
    fn c32_effect_decl_type_checks_with_no_ops() {
        let p = check_ok("effect Io { } fn main() -> i64 { 0 }");
        // C4.4 / ADR 0024 D5 + ADR 0066 M2.1: the built-in `Async` then
        // `Subprocess` effects are auto-registered after user effects (Io at 0,
        // Async at 1, Subprocess at 2).
        assert_eq!(p.effect_decls.len(), 3);
        assert!(p.effect_decls[0].ops.is_empty());
        assert_eq!(p.effect_decls[0].name, "Io");
        assert_eq!(p.effect_decls[1].name, "Async");
        assert!(p.effect_decls[1].ops.is_empty());
        assert_eq!(p.effect_decls[2].name, "Subprocess");
    }

    #[test]
    fn c32_effect_decl_type_checks_op_signatures() {
        // Op param + return types resolve against the struct table
        // / primitives.
        let p = check_ok(
            "effect Net { send(payload: i64) -> i64; recv() -> bool; }\
             fn main() -> i64 { 0 }",
        );
        let net = &p.effect_decls[0];
        assert_eq!(net.ops.len(), 2);
        assert_eq!(net.ops[0].name, "send");
        assert_eq!(net.ops[0].params[0].ty, Type::I64);
        assert_eq!(net.ops[0].return_type, Type::I64);
        assert_eq!(net.ops[1].name, "recv");
        assert!(net.ops[1].params.is_empty());
        assert_eq!(net.ops[1].return_type, Type::Bool);
    }

    #[test]
    fn c32_op_return_type_defaults_to_i64() {
        // `recv()` (no -> RetT) gets `i64` default per ADR 0019 D4.
        let p = check_ok(
            "effect Io { recv(); } fn main() -> i64 { 0 }",
        );
        assert_eq!(p.effect_decls[0].ops[0].return_type, Type::I64);
    }

    #[test]
    fn c32_op_unknown_type_errors() {
        let err = check_err(
            "effect Io { log(x: Nonexistent); } fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "Nonexistent"),
            "got {err:?}"
        );
    }

    #[test]
    fn c32_fn_with_effect_annotation_carries_effect_row_on_signature() {
        let p = check_ok(
            "effect Io { } fn run() -> i64 ! { Io } { 0 } fn main() -> i64 { 0 }",
        );
        let run_sig = p.fn_signatures.iter().find(|s| s.name == "run").expect("run");
        assert_eq!(run_sig.effect_row, vec![EffectId(0)]);
    }

    #[test]
    fn c32_fn_without_effect_annotation_has_empty_row() {
        let p = check_ok("fn main() -> i64 { 0 }");
        let main_sig = p.fn_signatures.iter().find(|s| s.name == "main").expect("main");
        assert!(main_sig.effect_row.is_empty());
    }

    // ----- C3.4 / ADR 0020 D5+D6: handle / perform / resume typing -----

    #[test]
    fn c34_handle_with_perform_typechecks() {
        // Body performs Io.read (i64); arm calls k(42) which is
        // i64; handle's outer type is i64.
        let p = check_ok(
            "effect Io { read() -> i64; }\
             fn main() -> i64 {\
                 handle perform Io.read() with { Io.read(k) => k(42) }\
             }",
        );
        // Konts table should have one entry.
        assert_eq!(p.konts.len(), 1);
        assert_eq!(p.konts[0].arg_ty, Type::I64);
        assert_eq!(p.konts[0].ret_ty, Type::I64);
    }

    #[test]
    fn c34_handle_with_return_arm_typechecks() {
        // `handle 42 with { return v => v * 2 }` — pure handle
        // with only the return arm. Outer type is `i64` (the
        // return arm's body, `v * 2`).
        let _p = check_ok(
            "fn main() -> i64 {\
                 handle 42 with { return v => v * 2 }\
             }",
        );
    }

    #[test]
    fn c34_handler_arm_type_mismatch_rejects() {
        // Arm body type-mismatches the handle's outer type. Body
        // is i64; arm produces bool (true). Surfaces
        // HandlerArmTypeMismatch.
        let err = check_err(
            "effect Io { read() -> i64; }\
             fn main() -> i64 {\
                 handle 0 with { Io.read(k) => true }\
             }",
        );
        assert!(
            matches!(err, TypeError::HandlerArmTypeMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_kont_used_as_value_rejects() {
        // Using k as a value (binding it via let) surfaces
        // KontUsedAsValue.
        let err = check_err(
            "effect Io { read() -> i64; }\
             fn main() -> i64 {\
                 handle 0 with {\
                     Io.read(k) => { let f: i64 = k; 42 }\
                 }\
             }",
        );
        assert!(
            matches!(err, TypeError::KontUsedAsValue { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_perform_arg_type_mismatch_rejects() {
        // Op Io.log takes an i64; passing bool surfaces a Mismatch.
        let err = check_err(
            "effect Io { log(msg: i64) -> i64; }\
             fn main() -> i64 {\
                 handle perform Io.log(true) with { Io.log(m, k) => k(0) }\
             }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    // ----- Phase D.1 (3/N) / ADR 0032: enum + match type-check -----

    #[test]
    fn enum_decl_typechecks_with_resolved_payloads() {
        let p = check_ok(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.enums.len(), 1);
        let e = &p.enums[0];
        assert_eq!(e.name, "Shape");
        assert_eq!(e.variants.len(), 3);
        assert!(e.variants[0].payloads.is_empty());
        assert_eq!(e.variants[1].payloads, vec![Type::I64]);
        assert_eq!(e.variants[2].payloads, vec![Type::I64, Type::I64]);
    }

    #[test]
    fn enum_name_usable_as_param_type() {
        // `fn area(s: Shape)` — enum name resolves in type position.
        let p = check_ok(
            "enum Shape { Unit, Circle(i64) }\n\
             fn area(s: Shape) -> i64 { 0 }\n\
             fn main() -> i64 { 0 }",
        );
        let area = p.fns.iter().find(|f| f.name == "area").expect("area");
        assert!(matches!(area.params[0].ty, Type::Enum(_)));
    }

    #[test]
    fn payload_construction_types_to_enum() {
        let p = check_ok(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\n\
             fn main() -> i64 { let s = Shape::Rect(3, 4); 0 }",
        );
        // The let-bound value has type Type::Enum.
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { ty, value, .. } => {
                assert!(matches!(ty, Type::Enum(_)));
                match &value.kind {
                    TypedExprKind::EnumConstruct { variant_index, args, .. } => {
                        assert_eq!(*variant_index, 2); // Rect is the 3rd variant
                        assert_eq!(args.len(), 2);
                    }
                    other => panic!("expected EnumConstruct, got {other:?}"),
                }
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn unit_construction_types_to_enum() {
        let p = check_ok(
            "enum Shape { Unit, Circle(i64) }\n\
             fn main() -> i64 { let s = Shape::Unit(); 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            TypedStmtKind::Let { value, .. } => match &value.kind {
                TypedExprKind::EnumConstruct { variant_index, args, .. } => {
                    assert_eq!(*variant_index, 0);
                    assert!(args.is_empty());
                }
                other => panic!("expected EnumConstruct, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn unknown_variant_rejected() {
        let err = check_err(
            "enum Shape { Unit, Circle(i64) }\n\
             fn main() -> i64 { let s = Shape::Square(1); 0 }",
        );
        assert!(matches!(err, TypeError::UnknownVariant { .. }), "got {err:?}");
    }

    #[test]
    fn variant_payload_arity_mismatch_rejected() {
        let err = check_err(
            "enum Shape { Circle(i64) }\n\
             fn main() -> i64 { let s = Shape::Circle(1, 2); 0 }",
        );
        assert!(
            matches!(err, TypeError::VariantPayloadArityMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn variant_payload_type_mismatch_rejected() {
        let err = check_err(
            "enum Shape { Circle(i64) }\n\
             fn main() -> i64 { let s = Shape::Circle(true); 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn exhaustive_match_typechecks_and_binds_payloads() {
        let p = check_ok(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\n\
             fn area(s: Shape) -> i64 {\n\
                match s {\n\
                    Shape::Unit => 0,\n\
                    Shape::Circle(r) => r * r * 3,\n\
                    Shape::Rect(w, h) => w * h,\n\
                }\n\
             }\n\
             fn main() -> i64 { area(Shape::Circle(2)) }",
        );
        let area = p.fns.iter().find(|f| f.name == "area").expect("area");
        match &area.body.tail.kind {
            TypedExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 3);
                // The Circle arm binds `r: i64`.
                match &arms[1].pattern {
                    TypedPattern::Variant { variant_index, bindings, .. } => {
                        assert_eq!(*variant_index, 1);
                        assert_eq!(bindings.len(), 1);
                        assert_eq!(bindings[0].ty, Type::I64);
                    }
                    other => panic!("expected Variant, got {other:?}"),
                }
            }
            other => panic!("expected Match, got {other:?}"),
        }
        // The match expression types to i64 (all arms produce i64).
        match &area.body.tail.kind {
            TypedExprKind::Match { .. } => {}
            _ => unreachable!(),
        }
        assert_eq!(area.body.tail.ty, Type::I64);
    }

    #[test]
    fn wildcard_match_is_exhaustive() {
        let p = check_ok(
            "enum Color { Red, Green, Blue }\n\
             fn f(c: Color) -> i64 { match c { Color::Red => 1, _ => 0 } }\n\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "f").expect("f");
        assert!(matches!(f.body.tail.kind, TypedExprKind::Match { .. }));
    }

    #[test]
    fn non_exhaustive_match_rejected() {
        let err = check_err(
            "enum Color { Red, Green, Blue }\n\
             fn f(c: Color) -> i64 { match c { Color::Red => 1, Color::Green => 2 } }\n\
             fn main() -> i64 { 0 }",
        );
        match err {
            TypeError::NonExhaustiveMatch { ref missing, .. } => {
                assert_eq!(missing, &vec!["Blue".to_string()]);
            }
            other => panic!("expected NonExhaustiveMatch, got {other:?}"),
        }
    }

    #[test]
    fn match_scrutinee_not_enum_rejected() {
        let err = check_err(
            "fn f(x: i64) -> i64 { match x { _ => 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::MatchScrutineeNotEnum { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn match_arm_type_mismatch_rejected() {
        // Arms must share a result type: bool vs i64.
        let err = check_err(
            "enum Color { Red, Green }\n\
             fn f(c: Color) -> i64 { let x = match c { Color::Red => true, Color::Green => 0 }; 0 }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::MatchArmTypeMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn match_pattern_arity_mismatch_rejected() {
        let err = check_err(
            "enum Shape { Rect(i64, i64) }\n\
             fn f(s: Shape) -> i64 { match s { Shape::Rect(w) => w } }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::VariantPayloadArityMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn match_pattern_binding_typed_and_usable() {
        // A bound payload is usable at its payload type in the arm body.
        let p = check_ok(
            "enum Box { Val(i64) }\n\
             fn unwrap(b: Box) -> i64 { match b { Box::Val(n) => n + 1 } }\n\
             fn main() -> i64 { unwrap(Box::Val(41)) }",
        );
        assert_eq!(p.main().body.tail.ty, Type::I64);
    }

    #[test]
    fn recursive_enum_typechecks() {
        // The headline D.1 enabler: a directly-recursive enum (an AST
        // node references itself) type-checks — heap-boxed payloads
        // (ADR 0032 D4) make this sound, so unlike a recursive struct
        // it needs no nullable indirection.
        let p = check_ok(
            "enum Tree { Leaf(i64), Node(Tree, Tree) }\n\
             fn d(x: Tree) -> i64 { match x { Tree::Leaf(v) => v, Tree::Node(l, r) => 1 } }\n\
             fn main() -> i64 { 0 }",
        );
        let e = &p.enums[0];
        assert_eq!(e.name, "Tree");
        // The Node variant's payloads are both the enum itself.
        assert_eq!(e.variants[1].payloads, vec![Type::Enum(e.id), Type::Enum(e.id)]);
    }

    #[test]
    fn pattern_naming_wrong_enum_rejected() {
        // A pattern that names a different enum than the scrutinee's
        // surfaces as UnknownVariant against the scrutinee's enum.
        let err = check_err(
            "enum Shape { Circle(i64) }\n\
             enum Color { Red }\n\
             fn f(s: Shape) -> i64 { match s { Color::Red => 0, Shape::Circle(r) => r } }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::UnknownVariant { .. }), "got {err:?}");
    }

    #[test]
    fn match_payload_widens_into_secret_arm_via_let() {
        // Arm bodies coerce to the expected (let-annotated) type.
        let p = check_ok(
            "enum Color { Red, Green }\n\
             fn f(c: Color) -> i64 {\n\
                let x: i64 = match c { Color::Red => 1, Color::Green => 2 };\n\
                x\n\
             }\n\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "f").expect("f");
        match &f.body.stmts[0].kind {
            TypedStmtKind::Let { ty, .. } => assert_eq!(*ty, Type::I64),
            other => panic!("expected Let, got {other:?}"),
        }
    }

    // ----- D.2 / ADR 0033: u8 + char/string literals + byte builtins -----

    /// The type of the tail expression of fn `name`'s body.
    fn fn_body_ty(p: &TypedProgram, name: &str) -> Type {
        p.fns.iter().find(|f| f.name == name).expect("fn").body.ty
    }

    #[test]
    fn char_lit_types_to_u8() {
        let p = check_ok("fn f() -> u8 { 'A' }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::U8);
    }

    #[test]
    fn string_lit_types_to_byte_array() {
        // ADR 0033 D3: a string IS a `[u8]`.
        let p = check_ok("fn f() -> [u8] { \"let\" }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::Array(ArrayElem::U8));
    }

    #[test]
    fn u8_arithmetic_types_to_u8() {
        // `'9' - '0'` — the lexer's digit-value idiom — is `u8`.
        let p = check_ok("fn f() -> u8 { '9' - '0' }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::U8);
    }

    #[test]
    fn u8_comparison_types_to_bool() {
        // `'a' <= c && c <= 'z'` shape: a u8 comparison yields bool.
        let p = check_ok("fn f() -> bool { 'a' < 'z' }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::Bool);
    }

    #[test]
    fn u8_bitwise_types_to_u8() {
        // ADR 0033 D4: bitwise reuses the op-generic `Binary` pipeline.
        let p = check_ok("fn f() -> u8 { 'A' & '_' }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::U8);
    }

    #[test]
    fn mixed_width_u8_plus_i64_rejected() {
        // ADR 0033 D4: no implicit width mixing — `u8 + i64` is a Mismatch
        // (the `l.ty != r.ty` operand check; `'a'` is u8, `5` is i64).
        let err = check_err("fn f() -> u8 { 'a' + 5 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn u8_has_no_implicit_widening_to_i64() {
        // A bare `u8` is not an `i64` — the explicit `u8_to_i64` is required.
        let err = check_err("fn f(c: u8) -> i64 { c }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::ReturnTypeMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn index_into_byte_array_yields_u8() {
        // `s[i]` over a `[u8]` is a `u8` (reuses the C1.6 Index rule).
        let p = check_ok("fn first(s: [u8]) -> u8 { s[0] }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "first"), Type::U8);
    }

    #[test]
    fn str_eq_builtin_typechecks() {
        let p = check_ok(
            "fn cmp(a: [u8], b: [u8]) -> bool { str_eq(a, b) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "cmp"), Type::Bool);
    }

    #[test]
    fn conversion_builtins_typecheck() {
        let p = check_ok(
            "fn widen(c: u8) -> i64 { u8_to_i64(c) }\n\
             fn narrow(n: i64) -> u8 { i64_to_u8(n) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "widen"), Type::I64);
        assert_eq!(fn_body_ty(&p, "narrow"), Type::U8);
    }

    #[test]
    fn str_eq_rejects_non_byte_array_arg() {
        // `str_eq` is concrete `[u8]` — an `[i64]` arg is a CallArgMismatch.
        let err = check_err(
            "fn bad(a: [i64], b: [u8]) -> bool { str_eq(a, b) }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::CallArgMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn secret_u8_is_representable() {
        // ADR 0033 D4: `secret u8` inherits the secret-preserving rules.
        let p = check_ok(
            "fn f(c: secret u8) -> secret u8 { c }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(fn_body_ty(&p, "f"), Type::Secret(_)));
    }

    // ----- D.3 / ADR 0034: growable collections (`Vec<T>`) -----

    #[test]
    fn vec_new_infers_element_from_return_type() {
        // ADR 0034 D5: `vec_new<T>() -> Vec<T>` — T pinned from the
        // expected return type (the same bidirectional seeding as `null`).
        let p = check_ok("fn f() -> Vec<i64> { vec_new() }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::Vec(VecElem::I64));
    }

    #[test]
    fn vec_new_infers_u8_element() {
        // ADR 0034 D5: `Vec<u8>` (the future `String`) — element `u8`.
        let p = check_ok("fn f() -> Vec<u8> { vec_new() }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::Vec(VecElem::U8));
    }

    #[test]
    fn vec_new_infers_element_from_let_annotation() {
        // The let-annotation pushdown also pins the element (as for `[]`).
        let p = check_ok("fn main() -> i64 { let v: Vec<i64> = vec_new(); 0 }");
        match &p.main().body.stmts[0].kind {
            TypedStmtKind::Let { ty, .. } => assert_eq!(*ty, Type::Vec(VecElem::I64)),
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn vec_new_without_annotation_is_ambiguous() {
        // No annotation, no args → T cannot be inferred (like `null` / `[]`).
        let err = check_err("fn main() -> i64 { let v = vec_new(); 0 }");
        assert!(matches!(err, TypeError::AmbiguousTypeArg { .. }), "got {err:?}");
    }

    #[test]
    fn push_typechecks_and_returns_i64() {
        // ADR 0034 D5: `push<T>(&mut Vec<T>, T) -> i64` — T bound from
        // the `&mut Vec<i64>` arg; the call is statement-shaped (i64).
        let p = check_ok(
            "fn f() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 5) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn push_element_type_mismatch_rejected() {
        // Pushing a `bool` into a `Vec<i64>`: T is bound to i64 by the
        // `&mut Vec<i64>` arg, then to bool by the element → conflict.
        let err = check_err(
            "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, true); 0 }",
        );
        assert!(
            matches!(err, TypeError::TypeArgInferenceConflict { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn len_accepts_vec() {
        // ADR 0034 D5: `len` is overloaded over `[T]` and `Vec<T>`.
        let p = check_ok(
            "fn f() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 1); len(v) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn push_on_immutable_vec_rejected() {
        // ADR 0034 D6: `push` takes `&mut v`, so `v` must be `let mut`.
        let err = check_err(
            "fn main() -> i64 { let v: Vec<i64> = vec_new(); push(&mut v, 1); 0 }",
        );
        assert!(
            matches!(err, TypeError::BorrowMutOfImmutable { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn vec_of_struct_element_typechecks() {
        // ADR 0034 D3: a struct is in the flat VecElem subset.
        let p = check_ok(
            "struct P { x: i64 }\nfn f() -> Vec<P> { vec_new() }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(fn_body_ty(&p, "f"), Type::Vec(VecElem::Struct(_))));
    }

    #[test]
    fn vec_of_array_element_rejected() {
        // ADR 0034 D8: `Vec<[i64]>` is outside the flat subset (deferred).
        let err = check_err("fn main() -> i64 { let v: Vec<[i64]> = vec_new(); 0 }");
        assert!(
            matches!(err, TypeError::VecElementNotSupported { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_vec_element_rejected() {
        // ADR 0034 D8: `Vec<Vec<i64>>` nesting is deferred.
        let err = check_err("fn main() -> i64 { let v: Vec<Vec<i64>> = vec_new(); 0 }");
        assert!(
            matches!(err, TypeError::VecElementNotSupported { .. }),
            "got {err:?}"
        );
    }

    // ----- ADR 0052: `Vec<secret T>` (growable buffers of secret elements) -----

    #[test]
    fn vec_secret_u8_element_resolves() {
        // ADR 0052: `Vec<secret u8>` is representable — `VecElem::Secret`.
        let p = check_ok(
            "fn f(v: Vec<secret u8>) -> Vec<secret u8> { v }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(fn_body_ty(&p, "f"), Type::Vec(VecElem::Secret(_))));
    }

    #[test]
    fn vec_new_infers_secret_element_from_annotation() {
        // ADR 0052: the generic substitution round-trip — `vec_new<T>() -> Vec<T>`
        // with `T := secret u8` must yield `Vec<secret u8>` (via `to_vec_elem_subst`),
        // so the `let`-annotation match succeeds (else it falls back to `Vec<T>`).
        let p = check_ok("fn main() -> i64 { let v: Vec<secret u8> = vec_new(); 0 }");
        match &p.main().body.stmts[0].kind {
            TypedStmtKind::Let { ty, .. } => {
                assert!(matches!(ty, Type::Vec(VecElem::Secret(_))), "got {ty:?}")
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn vec_secret_index_yields_secret() {
        // ADR 0052: indexing a `Vec<secret u8>` with a public index yields
        // `secret u8` (seeding the constant-time taint), mirroring `[secret u8]`.
        let p = check_ok(
            "fn f(v: Vec<secret u8>, i: i64) -> secret u8 { v[i] }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(fn_body_ty(&p, "f"), Type::Secret(_)));
    }

    #[test]
    fn vec_secret_index_assign_typechecks() {
        // ADR 0052 + ADR 0050: a `secret u8` is a Copy element, writable in place
        // through a public index (`buf[i] = v`).
        let p = check_ok(
            "fn f(x: secret u8) -> i64 { let mut v: Vec<secret u8> = vec_new(); \
             push(&mut v, x); v[0] = x; 0 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn vec_secret_index_must_be_public() {
        // ADR 0052: a SECRET index into a `Vec<secret u8>` is rejected (the
        // constant-time story), exactly as for a `[secret u8]` array read.
        let err = check_err(
            "fn f(v: Vec<secret u8>, i: secret i64) -> secret u8 { v[i] }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::IndexNotInt { .. }), "got {err:?}");
    }

    #[test]
    fn vec_of_secret_array_element_rejected() {
        // ADR 0052: the demote guard keeps `Vec<secret [u8]>` rejected (the
        // depth-1 no-nested-collection rule — a secret wrapping a non-scalar).
        let err = check_err("fn main() -> i64 { let v: Vec<secret [u8]> = vec_new(); 0 }");
        assert!(
            matches!(err, TypeError::VecElementNotSupported { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn vec_to_array_over_secret_yields_secret_array() {
        // ADR 0053: `vec_to_array<T>(Vec<T>) -> [T]` over `T := secret u8` must yield
        // `[secret u8]` (via the secret-aware array substitution `to_array_elem_subst`),
        // not fall back to `[T]`. The `Vec<secret u8> -> [secret u8]` bridge.
        let p = check_ok(
            "fn f(v: Vec<secret u8>) -> [secret u8] { vec_to_array(v) }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(fn_body_ty(&p, "f"), Type::Array(ArrayElem::Secret(_))));
    }

    #[test]
    fn vec_wrong_type_arg_count_rejected() {
        // `Vec<i64, bool>` — `Vec` takes exactly one type argument.
        let err = check_err("fn main() -> i64 { let v: Vec<i64, bool> = vec_new(); 0 }");
        assert!(
            matches!(err, TypeError::TypeArgCountMismatch { .. }),
            "got {err:?}"
        );
    }

    // ----- D.3 (2/N): v[i] + pop + the Vec->[u8] bridge + String -----

    #[test]
    fn vec_index_yields_element() {
        // ADR 0034 D5: `v[i]` on a `Vec<i64>` reuses the Index rule and
        // yields the element type.
        let p = check_ok(
            "fn f() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 7); v[0] }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn vec_u8_index_yields_u8() {
        let p = check_ok(
            "fn f() -> u8 { let mut v: Vec<u8> = vec_new(); push(&mut v, 'a'); v[0] }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::U8);
    }

    #[test]
    fn index_on_non_collection_still_rejected() {
        // The Vec arm must not loosen the array/Vec-only Index rule.
        let err = check_err("fn main() -> i64 { let x: i64 = 5; x[0] }");
        assert!(matches!(err, TypeError::IndexOnNonArray { .. }), "got {err:?}");
    }

    #[test]
    fn pop_typechecks_and_returns_element() {
        // ADR 0034 D5: `pop<T>(&mut Vec<T>) -> T`.
        let p = check_ok(
            "fn f() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 1); pop(&mut v) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn pop_on_immutable_vec_rejected() {
        // `pop` takes `&mut v`, so `v` must be `let mut`.
        let err = check_err(
            "fn main() -> i64 { let v: Vec<i64> = vec_new(); pop(&mut v); 0 }",
        );
        assert!(
            matches!(err, TypeError::BorrowMutOfImmutable { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn vec_to_array_yields_array_of_element() {
        // ADR 0034 D5: the `Vec<T>` -> `[T]` bridge.
        let p = check_ok(
            "fn f() -> [u8] { let mut v: Vec<u8> = vec_new(); push(&mut v, 'a'); vec_to_array(v) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::Array(ArrayElem::U8));
    }

    #[test]
    fn vec_to_array_then_str_eq_typechecks() {
        // The bridge lets a built `Vec<u8>` be `str_eq`'d against a `[u8]`.
        let p = check_ok(
            "fn f(kw: [u8]) -> bool { let mut v: Vec<u8> = vec_new(); push(&mut v, 'l'); str_eq(vec_to_array(v), kw) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::Bool);
    }

    #[test]
    fn string_alias_resolves_to_vec_u8() {
        // ADR 0034 D5 (Amendment A1): `String` is `Vec<u8>`.
        let p = check_ok("fn f() -> String { vec_new() }\nfn main() -> i64 { 0 }");
        assert_eq!(fn_body_ty(&p, "f"), Type::Vec(VecElem::U8));
    }

    #[test]
    fn string_param_accepts_vec_ops() {
        // A `String` parameter is a `Vec<u8>` — `len` (overloaded) applies.
        let p = check_ok(
            "fn f(s: String) -> i64 { len(s) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    // ----- D.4 / ADR 0035: file I/O (read_file / write_file) -----

    #[test]
    fn read_file_typechecks() {
        // ADR 0035 D4: `read_file([u8]) -> [u8]`.
        let p = check_ok(
            "fn f(path: [u8]) -> [u8] { read_file(path) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::Array(ArrayElem::U8));
    }

    #[test]
    fn write_file_typechecks() {
        // ADR 0035 D4: `write_file([u8], [u8]) -> i64` (statement-shaped).
        let p = check_ok(
            "fn f(path: [u8], data: [u8]) -> i64 { write_file(path, data) }\n\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn read_file_rejects_non_byte_array_arg() {
        // The path is concrete `[u8]` — an i64 arg is a CallArgMismatch
        // (same shape as `str_eq`'s arg check).
        let err = check_err("fn main() -> i64 { let x: [u8] = read_file(5); 0 }");
        assert!(matches!(err, TypeError::CallArgMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn write_file_rejects_wrong_arg_type() {
        // `write_file([i64], [u8])` — the path must be `[u8]`.
        let err = check_err(
            "fn f(p: [i64], d: [u8]) -> i64 { write_file(p, d) }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::CallArgMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn print_bytes_typechecks() {
        // D.4 (2/N) / ADR 0035 D4: `print_bytes([u8]) -> i64`.
        let p = check_ok(
            "fn f(s: [u8]) -> i64 { print_bytes(s) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(fn_body_ty(&p, "f"), Type::I64);
    }

    #[test]
    fn print_bytes_rejects_non_byte_array() {
        // The arg is concrete `[u8]` — an i64 is a CallArgMismatch.
        let err = check_err("fn main() -> i64 { print_bytes(5) }");
        assert!(matches!(err, TypeError::CallArgMismatch { .. }), "got {err:?}");
    }

    // ----- D.5 / ADR 0036: loops (`while`) -----

    #[test]
    fn while_loop_typechecks() {
        // ADR 0036 D7: a `while` with a bool condition + a mutated
        // loop-carried counter type-checks (the body's value is discarded).
        let _ = check_ok(
            "fn main() -> i64 { let mut i: i64 = 0; while i < 5 { i = i + 1; } i }",
        );
    }

    #[test]
    fn while_statement_only_body_typechecks() {
        // ADR 0036 D3: a statement-only `while` body (no tail) is valid
        // (the parser synthesises a discarded unit tail).
        let _ = check_ok("fn main() -> i64 { let mut i: i64 = 0; while i < 1 { i = i + 1; } 0 }");
    }

    #[test]
    fn while_cond_must_be_bool() {
        // ADR 0036 D7: the condition must be `bool` (mirrors `if`).
        let err = check_err("fn main() -> i64 { while 1 { } 0 }");
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Bool, got: Type::I64, .. }),
            "got {err:?}"
        );
    }

    // ----- D.5 (2/N) / ADR 0036 D9: break / continue -----

    #[test]
    fn break_and_continue_inside_loop_typecheck() {
        // ADR 0036 D9: `break;` / `continue;` are legal inside a `while`
        // body (env loop-depth > 0). (`while true` so the body is the
        // only exit; the `if` uses the tail idiom — `if` requires `else`
        // + a tail, ADR 0010/0013.)
        let _ = check_ok(
            "fn main() -> i64 { \
                let mut i: i64 = 0; \
                while true { \
                    if i >= 5 { break; 0 } else { 0 }; \
                    if i == 2 { continue; 0 } else { 0 }; \
                    i = i + 1; \
                } \
                i \
            }",
        );
    }

    #[test]
    fn break_inside_nested_if_in_loop_typechecks() {
        // The loop-nesting depth threads through nested `if` blocks (they
        // share the same `env`), so a `break` inside an `if` inside the
        // `while` body is accepted.
        let _ = check_ok(
            "fn main() -> i64 { \
                let mut i: i64 = 0; \
                while i < 10 { \
                    if i == 5 { break; 0 } else { 0 }; \
                    i = i + 1; \
                } \
                i \
            }",
        );
    }

    #[test]
    fn break_outside_loop_rejected() {
        // ADR 0036 D9: `break` at fn-body level (no enclosing loop) →
        // LoopControlOutsideLoop, naming the keyword.
        let err = check_err("fn main() -> i64 { break; 0 }");
        assert!(
            matches!(err, TypeError::LoopControlOutsideLoop { kw: "break", .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn continue_outside_loop_rejected() {
        let err = check_err("fn main() -> i64 { continue; 0 }");
        assert!(
            matches!(err, TypeError::LoopControlOutsideLoop { kw: "continue", .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn break_after_loop_rejected() {
        // The loop-nesting depth is restored at the end of the `while`
        // body (`exit_loop`), so a `break` AFTER the loop is out of a loop.
        let err = check_err(
            "fn main() -> i64 { let mut i: i64 = 0; while i < 1 { i = i + 1; } break; 0 }",
        );
        assert!(
            matches!(err, TypeError::LoopControlOutsideLoop { kw: "break", .. }),
            "got {err:?}"
        );
    }

    // ===== ADR 0058: the `f64` float type =====
    // (Float literals land in a later stage; these build `f64` via `as`
    // casts, which is the only way to obtain a float value at this stage.)

    #[test]
    fn f64_arithmetic_and_compare_typecheck() {
        // `+ - * /`, unary `-`, and ordered comparison all type to the
        // expected types (`f64` arithmetic, `bool` comparison).
        let p = check_ok(
            "fn f(a: f64, b: f64) -> i64 { let s: f64 = a + b; let d: f64 = a - b; \
             let m: f64 = a * b; let q: f64 = a / b; let n: f64 = -a; \
             if s > d { (m as i64) + (q as i64) + (n as i64) } else { 0 } }\
             fn main() -> i64 { f(7 as f64, 3 as f64) }",
        );
        let f = p.fns.iter().find(|f| f.name == "f").expect("f");
        assert_eq!(f.return_type, Type::I64);
    }

    #[test]
    fn f64_int_casts_typecheck() {
        // `i64 as f64` (sitofp) and `f64 as i64` (fptosi) round-trip.
        let p = check_ok(
            "fn rt(x: i64) -> i64 { let f: f64 = x as f64; f as i64 }\
             fn main() -> i64 { rt(42) }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn secret_f64_annotation_rejected() {
        // ADR 0058 (the fence): `secret f64` is a type error — float ops
        // are not constant-time, so a secret float is a false guarantee.
        let err = check_err(
            "fn f(x: secret f64) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::SecretFloat { .. }), "got {err:?}");
    }

    #[test]
    fn ptr_of_typechecks() {
        // ADR 0057 Phase 1b: `ptr_of(&[u8])` / `ptr_of_mut(&mut [u8])` produce
        // `ptr`, which an `extern` may take. A whole FFI buffer call type-checks.
        let p = check_ok(
            "extern \"C\" { fn getentropy(b: ptr, n: i64) -> i64; fn strlen(s: ptr) -> i64; }\
             fn f() -> i64 { let b: [u8] = \"hi\"; let mut m: [u8] = \"ab\"; \
             getentropy(ptr_of_mut(&mut m), 2) + strlen(ptr_of(&b)) }\
             fn main() -> i64 { f() }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn ptr_of_non_byte_ref_rejected() {
        // `ptr_of` of something that is not a `&[u8]` (here a bare `[u8]`, not a
        // reference) is rejected.
        let err = check_err(
            "extern \"C\" { fn strlen(s: ptr) -> i64; }\
             fn f() -> i64 { let b: [u8] = \"hi\"; strlen(ptr_of(b)) }\
             fn main() -> i64 { f() }",
        );
        assert!(matches!(err, TypeError::PtrOfArg { .. }), "got {err:?}");
    }

    #[test]
    fn ptr_of_mut_requires_mutable_borrow() {
        // `ptr_of_mut` needs a `&mut` borrow (C may write through it); a shared
        // `&buf` is rejected.
        let err = check_err(
            "extern \"C\" { fn getentropy(b: ptr, n: i64) -> i64; }\
             fn f() -> i64 { let b: [u8] = \"hi\"; getentropy(ptr_of_mut(&b), 2) }\
             fn main() -> i64 { f() }",
        );
        assert!(matches!(err, TypeError::PtrOfArg { .. }), "got {err:?}");
    }

    #[test]
    fn ptr_of_secret_buffer_rejected() {
        // The FFI fence: a `&[secret u8]` cannot cross via `ptr_of` — a secret
        // buffer's pointer may not reach unverified C.
        let err = check_err(
            "extern \"C\" { fn strlen(s: ptr) -> i64; }\
             fn f() -> i64 { let b: [secret u8] = \"hi\"; strlen(ptr_of(&b)) }\
             fn main() -> i64 { f() }",
        );
        assert!(matches!(err, TypeError::PtrOfArg { .. }), "got {err:?}");
    }

    #[test]
    fn is_null_typechecks() {
        // ADR 0057 Phase 1b (A7): `is_null(p)` over a `ptr` (an FFI return) is a
        // public `bool` — usable as a (public) `if` condition.
        let p = check_ok(
            "extern \"C\" { fn getenv(name: ptr) -> ptr; }\
             fn f() -> i64 { let c: [u8] = \"X\"; let v: ptr = getenv(ptr_of(&c)); \
             if is_null(v) { 0 } else { 1 } }\
             fn main() -> i64 { f() }",
        );
        assert_eq!(p.main().return_type, Type::I64);
    }

    #[test]
    fn is_null_non_ptr_rejected() {
        // `is_null` requires a `ptr` operand; anything else is a `Mismatch`.
        let err = check_err(
            "fn main() -> i64 { if is_null(5) { 1 } else { 0 } }",
        );
        assert!(
            matches!(err, TypeError::Mismatch { expected: Type::Ptr, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn secret_cast_to_f64_rejected() {
        // Casting a `secret` value to `f64` would create a `secret f64` —
        // rejected (the fence; `declassify` first if a public float is meant).
        let err = check_err(
            "fn f(x: secret i64) -> i64 { let y: f64 = x as f64; y as i64 }\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::SecretFloat { .. }), "got {err:?}");
    }

    #[test]
    fn f64_bitwise_rejected() {
        // Bitwise / shift operators are meaningless on a float.
        let err = check_err(
            "fn f(a: f64, b: f64) -> f64 { a & b }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::FloatBitwise { .. }), "got {err:?}");
    }

    #[test]
    fn f64_mixed_int_float_rejected() {
        // No implicit int↔float promotion — `f64 + i64` is a Mismatch.
        let err = check_err(
            "fn f(a: f64, b: i64) -> f64 { a + b }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn export_ffi_typechecks() {
        // ADR 0059: an `export "C"` fn over the value ABI type-checks and is
        // recorded in `TypedProgram.exports`.
        let p = check_ok(
            "export \"C\" fn ct_choose(c: i64, a: i64, b: i64) -> i64 { \
             let m: secret i64 = 0 - c; let nm: secret i64 = m ^ (0 - 1); \
             let sa: secret i64 = a; let sb: secret i64 = b; \
             declassify((sa & m) | (sb & nm)) }\
             fn main() -> i64 { 0 }",
        );
        assert_eq!(p.exports.len(), 1);
    }

    #[test]
    fn export_byte_slice_param_typechecks() {
        // ADR 0059 Phase 1b: a `&[u8]` export param is FFI-safe (presented to C
        // as a (ptr, len) pair); other refs are not.
        let p = check_ok(
            "export \"C\" fn len_of(a: &[u8]) -> i64 { len(*a) }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.exports.len(), 1);
        // A `&i64` (non-byte-slice ref) is still rejected.
        let err = check_err(
            "export \"C\" fn f(a: &i64) -> i64 { *a }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn export_secret_param_rejected() {
        // The export fence: an export may not take a `secret` param (the C
        // caller is outside the verified region).
        let err = check_err(
            "export \"C\" fn f(x: secret i64) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn export_owned_byte_array_return_typechecks() {
        // ADR 0059 Phase 1b (A7): an owned PUBLIC `[u8]` return is FFI-safe
        // (presented to C via the (uint8_t** out_data, int64_t* out_len) pair +
        // sentinel_free_bytes).
        let p = check_ok(
            "export \"C\" fn f() -> [u8] { \"hi\" }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.exports.len(), 1);
    }

    #[test]
    fn export_secret_byte_array_return_rejected() {
        // The export fence still stands: a `[secret u8]` return cannot cross the
        // boundary (a secret leaves the verified region only via `declassify`).
        let err = check_err(
            "export \"C\" fn f(a: &[u8]) -> [secret u8] { \
             let mut v: Vec<secret u8> = vec_new(); push(&mut v, (*a)[0]); vec_to_array(v) }\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn export_non_ffi_return_rejected() {
        // A non-FFI return (a struct) is still rejected — only value-ABI scalars
        // and the owned `[u8]` byte buffer may cross.
        let err = check_err(
            "struct P { x: i64 }\nexport \"C\" fn f() -> P { P { x: 1 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn extern_ffi_typechecks() {
        // ADR 0057: an `extern "C"` fn over the FFI-safe set type-checks, and
        // a call to it resolves + returns its type.
        let p = check_ok(
            "extern \"C\" { fn getpid() -> i64; fn pow(b: f64, e: f64) -> f64; }\
             fn main() -> i64 { let q: f64 = pow(2.0, 3.0); getpid() + (q as i64) }",
        );
        // Both externs are recorded.
        assert_eq!(p.externs.len(), 2);
    }

    #[test]
    fn extern_secret_param_rejected() {
        // The FFI fence: a `secret` value cannot cross an `extern` boundary.
        let err = check_err(
            "extern \"C\" { fn f(x: secret i64) -> i64; } fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn extern_non_ffi_type_rejected() {
        // A non-FFI-safe type (`u8`, a struct, an array) is rejected in Phase 1.
        let err = check_err(
            "extern \"C\" { fn f(x: u8) -> i64; } fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::ExternFfiType { .. }), "got {err:?}");
    }

    #[test]
    fn sqrt_typechecks_f64() {
        // ADR 0058: `sqrt(f64) -> f64`.
        let p = check_ok(
            "fn root(x: f64) -> i64 { let r: f64 = sqrt(x); r as i64 }\
             fn main() -> i64 { root(16.0) }",
        );
        let f = p.fns.iter().find(|f| f.name == "root").expect("root");
        assert_eq!(f.return_type, Type::I64);
    }

    #[test]
    fn sqrt_on_non_f64_rejected() {
        // `sqrt` requires an `f64` operand — an integer is a Mismatch.
        let err = check_err(
            "fn f(x: i64) -> i64 { let r: f64 = sqrt(x); r as i64 }\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, TypeError::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn f64_type_args_rejected() {
        // `f64<...>` is rejected like any other non-generic primitive.
        let err = check_err(
            "fn f(x: f64<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::TypeArgsOnNonGeneric { .. }),
            "got {err:?}"
        );
    }
}
