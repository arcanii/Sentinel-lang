//! sentinel-resolve
//!
//! Name resolution for Sentinel. Takes a parsed [`Program`] and
//! produces a [`ResolvedProgram`] where every name reference is a
//! stable identifier ([`VarId`] / [`FnId`]) — no more string lookups
//! downstream.
//!
//! Per ADR 0011 D4, this crate lifts the name-resolution logic that
//! `sentinel-codegen::CodegenCtx` has been carrying since C0.3.
//! Codegen at C1.1+ consumes a `ResolvedProgram` instead of a raw
//! `Program`; it never needs to do its own var/fn lookups, and the
//! `UndefinedVariable` / `UndefinedFunction` / `RedeclaredVariable`
//! / `RedefinedFunction` / `MissingMain` / `ArityMismatch`
//! diagnostics fire here.
//!
//! The representation is a parallel tree: [`ResolvedExpr`],
//! [`ResolvedStmt`], [`ResolvedBlock`], [`ResolvedFnDef`],
//! [`ResolvedParam`], [`ResolvedProgram`] mirror their AST
//! counterparts, with name-reference fields swapped for IDs.
//! Binding sites retain their source name (for diagnostics + LLVM
//! IR debug names); reference sites carry only IDs.
//!
//! `resolve(program) -> Result<ResolvedProgram, ResolveError>` is
//! the pure-function entry point. `resolve_query(db, file)` is the
//! `#[salsa::tracked]` wrapper for the driver's incremental
//! pipeline (per ADR 0011 D1, downstream stages of C1.0b's lex/
//! parse retrofit).

use std::collections::{HashMap, HashSet};

use salsa::Accumulator;
use sentinel_ast::{
    BinOp, Block, ClassDecl, CmpOp, EffectDecl, EnumDecl, Expr, ExprKind, FnDef, HandlerArm,
    ImplDecl, LogicOp, MatchArm, Param, Pattern, Program, ReturnArm, SelfKind, Span, Spanned, Stmt,
    StmtKind, StructDecl, TraitDecl, TypeExpr, TypeExprKind, TypeParam as AstTypeParam, UnaryOp,
    Visibility,
};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};

// =============================================================================
// Stable identifiers
// =============================================================================

/// Identifier for a value binding (parameter or `let`). Unique
/// per-program; assigned by [`resolve`] in source order. Ord
/// derived (C2.4) so BorrowCheck's DropPlan can use BTreeMap
/// keyed by VarId for salsa-cacheable storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarId(pub u32);

/// Identifier for a function. Unique per-program; assigned by
/// [`resolve`] in source order, with `print` taking `FnId(0)`
/// because the runtime symbol is pre-registered before user fns.
/// Ord derived (C2.4) — same rationale as [`VarId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FnId(pub u32);

/// Sentinel's runtime `print` is always `FnId(0)`. Pre-registered
/// by [`resolve`]; user fns get higher IDs.
pub const PRINT_FN_ID: FnId = FnId(0);

/// `unwrap_or(x: ?T, default: T) -> T` per ADR 0014 D9. Generic
/// over T; the type checker special-cases the typing rule. Codegen
/// inlines per call site.
pub const UNWRAP_OR_FN_ID: FnId = FnId(1);

/// `is_some(x: ?T) -> bool` per ADR 0014 D9. Generic over T; same
/// machinery as `unwrap_or`.
pub const IS_SOME_FN_ID: FnId = FnId(2);

/// `len(a: [T]) -> i64` per ADR 0015 D4. Generic over T; the type
/// checker special-cases it the same way `unwrap_or` / `is_some`
/// are. Codegen inlines it as `extract_value(struct, 0)` of the
/// `{ i64 len, T* data }` array representation.
pub const LEN_FN_ID: FnId = FnId(3);

/// D.2 / ADR 0033 D5: `str_eq(a: [u8], b: [u8]) -> bool` — byte-wise
/// array equality (the lexer's keyword/identifier matcher). A
/// non-generic runtime builtin; codegen lowers it to a runtime
/// `sentinel_str_eq` at D.2 (4/N) (rejects until then).
pub const STR_EQ_FN_ID: FnId = FnId(4);

/// D.2 / ADR 0033 D5: `u8_to_i64(b: u8) -> i64` — explicit
/// zero-extend (mixed-width arithmetic is rejected, so the lexer
/// converts a digit byte to an integer explicitly). Codegen at (4/N).
pub const U8_TO_I64_FN_ID: FnId = FnId(5);

/// D.2 / ADR 0033 D5: `i64_to_u8(n: i64) -> u8` — explicit truncate
/// (index/byte construction). Codegen at (4/N).
pub const I64_TO_U8_FN_ID: FnId = FnId(6);

/// D.3 / ADR 0034 D5: `vec_new<T>() -> Vec<T>` — an empty growable
/// vector (`{ len: 0, cap: 0, data: null }`; no allocation). Generic
/// over the element T, which is inferred from the binding's annotation
/// (the same bidirectional pushdown as `null`'s `?T` / an empty array
/// literal). Codegen builds the `{ 0, 0, null }` struct at D.3 (1/N).
pub const VEC_NEW_FN_ID: FnId = FnId(7);

/// D.3 / ADR 0034 D5: `push<T>(v: &mut Vec<T>, x: T) -> i64` — append
/// `x` to `v`, growing the heap buffer (`realloc` to `max(1, cap*2)`)
/// when `len == cap`. The first heap-mutation primitive; takes `v` by
/// `&mut` so it participates in the shared-XOR-mutable borrow rule.
/// Returns `i64` 0 (a statement-shaped builtin like `print`). Codegen
/// at D.3 (1/N).
pub const PUSH_FN_ID: FnId = FnId(8);

/// D.3 (2/N) / ADR 0034 D5: `pop<T>(v: &mut Vec<T>) -> T` — remove and
/// return the last element, decrementing `len` (the buffer is not
/// shrunk). Panics on an empty `Vec` (like an out-of-bounds index).
/// Takes `v` by `&mut`; flows the uniform generic-call path like `push`.
pub const POP_FN_ID: FnId = FnId(9);

/// D.3 (2/N) / ADR 0034 D5: `vec_to_array<T>(v: Vec<T>) -> [T]` — the
/// `Vec<T>` -> `[T]` bridge: copy the live `len` elements into a fresh
/// owned `[T]` (so a built `Vec<u8>` string can be `str_eq`'d against a
/// keyword `[u8]`). Non-consuming (borrows `v`, like `len` / `str_eq`).
pub const VEC_TO_ARRAY_FN_ID: FnId = FnId(10);

/// D.4 / ADR 0035 D4: `read_file(path: [u8]) -> [u8]` — read a whole
/// file into a fresh owned byte array. A runtime builtin (NOT an
/// algebraic effect; ADR 0035 D2), like `str_eq`. Panics on failure
/// (ADR 0035 D5). Codegen lowers it to `sentinel_read_file`.
pub const READ_FILE_FN_ID: FnId = FnId(11);

/// D.4 / ADR 0035 D4: `write_file(path: [u8], data: [u8]) -> i64` —
/// create/truncate the file at `path` and write `data`'s bytes. Returns
/// 0 (statement-shaped, like `print`); panics on failure. Lowers to
/// `sentinel_write_file`; its args are borrowed (the ADR 0033 A3 rule).
pub const WRITE_FILE_FN_ID: FnId = FnId(12);

/// D.4 (2/N) / ADR 0035 D4: `print_bytes(data: [u8]) -> i64` — write
/// `data`'s bytes to stdout (the byte/string companion to `print`).
/// Returns 0; no added newline. Lowers to `sentinel_print_bytes`; its
/// arg is borrowed (the ADR 0033 A3 rule).
pub const PRINT_BYTES_FN_ID: FnId = FnId(13);

/// Identifier for a struct declaration. Added at C1.4 per ADR 0013
/// D4 / D5; unique per-program, assigned in source order starting
/// at 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

/// Identifier for a top-level class declaration per ADR 0022 D1
/// (C4.1). Unique per-program; assigned in source order starting
/// at 0. ClassId indexes into [`ResolvedProgram::classes`] /
/// `TypedProgram::class_decls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(pub u32);

/// Identifier for a top-level trait declaration per ADR 0023 D1
/// (C4.2). Unique per-program; assigned in source-encounter order
/// starting at 0. TraitId indexes into [`ResolvedProgram::traits`]
/// / `TypedProgram::trait_decls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitId(pub u32);

/// Identifier for a top-level impl declaration per ADR 0023 D3/D4
/// (C4.2). Unique per-program; assigned in source-encounter order
/// starting at 0. ImplId indexes into [`ResolvedProgram::impls`] /
/// `TypedProgram::impl_decls`. Default and named impls share the
/// ID space — the impl's `name: Option<String>` distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImplId(pub u32);

/// C4.2 / ADR 0023 D3: the receiver type of an impl block. At C4.2
/// minimum impls target concrete classes or structs; impl-for-
/// generic and `impl<T>` are deferred per ADR 0023 D10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImplTarget {
    Class(ClassId),
    Struct(StructId),
}

/// Position-indexed identifier for a generic type parameter
/// inside the surrounding fn / struct, added at C1.7 per ADR
/// 0016 D6c. Two distinct fns can each have `TypeParamId(0)`
/// referring to their own first type parameter — the ID is
/// scoped to its parent and only meaningful in that context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeParamId(pub u32);

/// Phase D.1 / ADR 0032 D3: identifier for a top-level sum-type
/// (`enum`) declaration. Assigned in source-encounter order at
/// resolve time; stable across cargo runs for a given program.
/// EnumIds index into `ResolvedProgram::enums` and
/// `TypedProgram::enums`. Mirrors [`StructId`] / [`ClassId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

/// A function's signature as needed by call-site resolution and
/// arity-checking. Stored on the resolved program for codegen to
/// consult without re-walking source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnSignature {
    pub id: FnId,
    pub name: String,
    /// Span of the function's name at its definition site (or
    /// `None` for the synthetic `print` runtime entry).
    pub name_span: Option<Span>,
    /// Number of formal parameters.
    pub arity: usize,
    /// Number of generic type parameters per ADR 0016 D1. `0` for
    /// non-generic fns; positive for generic fns (including the
    /// runtime builtins `unwrap_or`, `is_some`, `len` which all
    /// carry `1` type parameter).
    pub type_params_count: usize,
    /// `true` iff this is the user's `main`. Codegen uses this to
    /// emit the C-ABI i32 return.
    pub is_main: bool,
    /// `true` iff this is the runtime `print` symbol. Codegen uses
    /// this to map the call to `sentinel_print`.
    pub is_runtime: bool,
}

/// A generic type parameter at resolve time per ADR 0016 D9.
/// Carries the source-level name + span for diagnostics; the
/// position of this entry in its parent's `type_params` vector is
/// the [`TypeParamId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTypeParam {
    pub id: TypeParamId,
    pub name: String,
    pub name_span: Span,
}

// =============================================================================
// Resolved AST — parallel tree
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedProgram {
    pub fns: Vec<ResolvedFnDef>,
    /// Lookup table — index by `FnId.0 as usize` for O(1) signature
    /// access from codegen / future passes.
    pub fn_signatures: Vec<FnSignature>,
    /// Struct declarations in source order, added at C1.4. Each
    /// carries its own [`StructId`] matching its index in this vec.
    /// The struct table is built before fn signatures so fn
    /// parameters can reference struct names per ADR 0013 D4.
    pub structs: Vec<ResolvedStructDecl>,
    /// C3 / ADR 0019 D4 (C3.2): top-level effect declarations.
    /// Each carries its own [`EffectId`] matching its index here.
    /// The effect table is built before fn signatures so fn
    /// signatures' effect-row annotations can reference effect
    /// names.
    pub effects: Vec<ResolvedEffectDecl>,
    /// C4.1 / ADR 0022 D1: top-level class declarations. Each
    /// carries its own [`ClassId`] matching its index here. The
    /// class table is built alongside the struct table so type
    /// annotations like `let p: Point = ...` can be resolved
    /// against either.
    pub classes: Vec<ResolvedClassDecl>,
    /// C4.2 / ADR 0023 D1: top-level trait declarations. Each
    /// carries its own [`TraitId`] matching its index here. Trait
    /// names share a namespace with classes and structs.
    pub traits: Vec<ResolvedTraitDecl>,
    /// C4.2 / ADR 0023 D3/D4: top-level impl declarations (default
    /// and named). Each carries its own [`ImplId`] matching its
    /// index here. The per-(trait, type, name?) uniqueness rules
    /// are enforced at resolve time.
    pub impls: Vec<ResolvedImplDecl>,
    /// Phase D.1 / ADR 0032: top-level sum-type (`enum`)
    /// declarations. Each carries its own [`EnumId`] matching its
    /// index here. Enum names share the type namespace with structs,
    /// classes, and traits. Variant payload types stay as
    /// [`TypeExpr`] (string-keyed) — sentinel-types resolves them.
    pub enums: Vec<ResolvedEnumDecl>,
    pub span: Span,
}

/// Phase D.1 / ADR 0032: an `enum` declaration after name
/// resolution. The [`EnumId`] matches its index in
/// [`ResolvedProgram::enums`]. Variant payload-type annotations stay
/// as [`TypeExpr`] (string-keyed) — sentinel-types resolves them at
/// the typing pass, looking enum names up against the table built
/// here. Non-generic at the D.1 MVP (ADR 0032 D9).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedEnumDecl {
    pub id: EnumId,
    pub name: String,
    pub name_span: Span,
    pub variants: Vec<ResolvedVariantDecl>,
    pub span: Span,
}

/// Phase D.1 / ADR 0032: one variant of a [`ResolvedEnumDecl`]. The
/// variant's index in the parent's `variants` vec is its
/// discriminant (source order). `payloads` is empty for a unit
/// variant, else the positional payload-field type annotations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedVariantDecl {
    pub name: String,
    pub name_span: Span,
    pub payloads: Vec<TypeExpr>,
    pub span: Span,
}

/// C3 / ADR 0019 D4 (C3.2): a top-level effect declaration after
/// name resolution. Carries its [`EffectId`] (index into
/// `ResolvedProgram::effects`) and the resolved operations.
/// Operations don't get their own per-op IDs at C3.2 — the
/// handler runtime that uses them lands at C3.4 / ADR 0020.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedEffectDecl {
    pub id: EffectId,
    pub name: String,
    pub name_span: Span,
    pub ops: Vec<ResolvedOpDecl>,
    pub span: Span,
}

/// C3 / ADR 0019 D4 (C3.2): a single operation declaration inside
/// an effect. Param types stay as [`TypeExpr`] (string-keyed) —
/// sentinel-types resolves them at C3.2. Operations have no
/// runtime semantics at C3.2; they're declared for the future
/// handler runtime (ADR 0020).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedOpDecl {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<ResolvedParam>,
    /// Optional return type. `None` defaults to `i64` at type-
    /// check per ADR 0019 D4.
    pub return_type: Option<TypeExpr>,
    pub span: Span,
}

/// Identifier for a top-level effect declaration per ADR 0019 D4
/// (C3.2). Assigned in source-encounter order at resolve time;
/// stable across cargo runs for a given program. EffectIds index
/// into `ResolvedProgram::effects` and `TypedProgram::effect_decls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectId(pub u32);

/// A struct declaration after name resolution. The decl's
/// [`StructId`] matches its index in [`ResolvedProgram::structs`].
/// Field-type annotations are carried as [`TypeExpr`] (string-keyed)
/// — sentinel-types resolves those at C1.4.5.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStructDecl {
    pub id: StructId,
    pub name: String,
    pub name_span: Span,
    /// Generic type parameters per ADR 0016 D2 / D9. Empty for
    /// non-generic structs. The position in this vec is the
    /// [`TypeParamId`] within this struct's scope.
    pub type_params: Vec<ResolvedTypeParam>,
    pub fields: Vec<ResolvedStructField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStructField {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub span: Span,
}

/// A class declaration after name resolution per ADR 0022 D1
/// (C4.1). The [`ClassId`] matches its index in
/// [`ResolvedProgram::classes`]. Field-type annotations and the
/// init/method param/return types stay as [`TypeExpr`] (string-
/// keyed) — sentinel-types resolves them at the typing pass,
/// looking up names against the class table built here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedClassDecl {
    pub id: ClassId,
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<ResolvedClassField>,
    pub init: Option<ResolvedInitDef>,
    pub methods: Vec<ResolvedMethodDef>,
    pub span: Span,
}

/// A single class field after name resolution per ADR 0022 D2.
/// Visibility is parsed and propagated but never enforced at C4.1
/// (per D2 the enforcement substrate is Phase C5's module system).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedClassField {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub span: Span,
}

/// The resolved `init(params) { body }` constructor of a class per
/// ADR 0022 D4. The synthetic `self_var_id` is bound inside the
/// body (so `self.field = expr` inside init resolves through the
/// usual Var-lookup path).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedInitDef {
    pub visibility: Visibility,
    /// VarId of the synthetic `self` binding inside the init body.
    /// Allocated by resolve so type-check + codegen don't need to
    /// re-thread the receiver.
    pub self_var_id: VarId,
    pub params: Vec<ResolvedParam>,
    pub body: ResolvedBlock,
    pub span: Span,
}

/// A resolved method declaration per ADR 0022 D3. Methods take
/// `self: &Self` or `self: &mut Self` as the implicit first
/// parameter (captured here as `self_kind`); the explicit
/// `params` exclude `self`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedMethodDef {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    /// VarId of the synthetic `self` binding inside the method
    /// body, allocated by resolve.
    pub self_var_id: VarId,
    pub params: Vec<ResolvedParam>,
    pub return_type: TypeExpr,
    /// Effect-row annotation (each name → [`EffectId`]).
    /// Empty for methods without an annotation.
    pub effect_row: Vec<EffectId>,
    pub body: ResolvedBlock,
    pub span: Span,
}

impl ResolvedProgram {
    /// The user `main` function. Always present after a successful
    /// `resolve` (otherwise [`ResolveError::MissingMain`] would have
    /// fired).
    pub fn main(&self) -> &ResolvedFnDef {
        self.fns
            .iter()
            .find(|f| f.signature(self).is_main)
            .expect("MissingMain would have fired in resolve")
    }

    /// Signature lookup by ID. Panics on out-of-range — IDs only
    /// come from [`resolve`], so this should never fail.
    pub fn signature(&self, id: FnId) -> &FnSignature {
        &self.fn_signatures[id.0 as usize]
    }
}

/// C4.2 / ADR 0023 D1: a trait declaration after name resolution.
/// The [`TraitId`] matches its index in [`ResolvedProgram::traits`].
/// Method signatures stay as [`TypeExpr`] (string-keyed) — sentinel-
/// types resolves them at Pass 3c.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTraitDecl {
    pub id: TraitId,
    pub name: String,
    pub name_span: Span,
    pub methods: Vec<ResolvedTraitMethodSig>,
    pub span: Span,
}

/// C4.2 / ADR 0023 D2: a single method signature inside a trait
/// declaration. Body-less — the signature alone is the contract.
/// Default-method bodies are deferred per ADR 0023 D10. Params
/// don't carry VarIds (trait method sigs have no body so no
/// binding to reference) — Vec is used for the shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTraitMethodSig {
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub params: Vec<ResolvedTraitParam>,
    pub return_type: TypeExpr,
    /// Effect-row annotation (each name → [`EffectId`]).
    pub effect_row: Vec<EffectId>,
    pub span: Span,
}

/// C4.2 / ADR 0023 D2: a single param inside a trait method
/// signature. Mirrors [`ResolvedParam`] but without a VarId — trait
/// method sigs have no body so the params aren't bound anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTraitParam {
    pub mutable: bool,
    pub name: String,
    pub span: Span,
    pub ty: TypeExpr,
}

/// C4.2 / ADR 0023 D3/D4: an impl block declaration after name
/// resolution. The [`ImplId`] matches its index in
/// [`ResolvedProgram::impls`]. Both default (`name: None`) and
/// named (`name: Some(_)`) impls live in this vec; resolve has
/// validated the (trait, type, name) uniqueness rules.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedImplDecl {
    pub id: ImplId,
    /// `None` for default impls; `Some(name)` for named impls.
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub trait_id: TraitId,
    pub trait_name: String,
    pub trait_name_span: Span,
    pub target: ImplTarget,
    pub type_name: String,
    pub type_name_span: Span,
    pub methods: Vec<ResolvedImplMethodDef>,
    pub span: Span,
}

/// C4.2 / ADR 0023 D3: an impl method after name resolution.
/// Mirrors [`ResolvedMethodDef`] structurally: synthetic `self`
/// VarId bound inside the body, effect-row resolved to EffectIds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedImplMethodDef {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    /// VarId of the synthetic `self` binding inside the method
    /// body, allocated by resolve.
    pub self_var_id: VarId,
    pub params: Vec<ResolvedParam>,
    pub return_type: TypeExpr,
    pub effect_row: Vec<EffectId>,
    pub body: ResolvedBlock,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedFnDef {
    pub id: FnId,
    pub name: String,
    pub name_span: Span,
    /// Generic type parameters per ADR 0016 D1 / D9. Empty for
    /// non-generic fns. The position in this vec is the
    /// [`TypeParamId`] within this fn's scope.
    pub type_params: Vec<ResolvedTypeParam>,
    pub params: Vec<ResolvedParam>,
    /// C1.2 (per ADR 0012 D1): mandatory return-type annotation.
    /// Carried through from the AST for the type checker to consume
    /// at C1.2's check() pass.
    pub return_type: TypeExpr,
    /// C3 / ADR 0019 D1 (C3.2): resolved effect-row annotation
    /// (postfix `! { Op1, Op2 }` on the fn signature). Each entry
    /// is the [`EffectId`] of the named effect. Empty for fns with
    /// no annotation. The effect_check_query pass at C3.2 validates
    /// the annotation against the inferred row.
    pub effect_row: Vec<EffectId>,
    pub body: ResolvedBlock,
    pub span: Span,
}

impl ResolvedFnDef {
    pub fn signature<'a>(&self, program: &'a ResolvedProgram) -> &'a FnSignature {
        program.signature(self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedParam {
    pub id: VarId,
    /// C2 / ADR 0017 D2: `true` iff the source declared `mut x: T`.
    /// Binding-local mutability — does NOT imply the caller passes
    /// `&mut T`.
    pub mutable: bool,
    /// Source-level name retained for diagnostics and LLVM IR
    /// debug names.
    pub name: String,
    pub span: Span,
    /// C1.2 (per ADR 0012 D1): mandatory parameter-type annotation.
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedBlock {
    pub stmts: Vec<ResolvedStmt>,
    pub tail: ResolvedExpr,
    pub span: Span,
}

pub type ResolvedStmt = Spanned<ResolvedStmtKind>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedStmtKind {
    Let {
        id: VarId,
        /// C2 / ADR 0017 D2: `true` iff the source wrote `let mut x`.
        mutable: bool,
        /// Source-level name retained for diagnostics / IR debug.
        name: String,
        name_span: Span,
        /// C1.2 (per ADR 0012 D2): optional `let x: T = ...`
        /// annotation. None means "infer from RHS".
        ty_annot: Option<TypeExpr>,
        value: ResolvedExpr,
    },
    /// Assignment statement per ADR 0017 D2: `lhs = expr;`. LHS is
    /// resolved as an expression here (the type checker enforces it
    /// is actually an lvalue + that the target binding is mutable
    /// or the deref is through `&mut T`).
    Assign {
        target: ResolvedExpr,
        value: ResolvedExpr,
    },
    /// Phase D.5 / ADR 0036 D3: `while <cond> { <body> }`. The body is
    /// its own lexical scope (per-iteration bindings).
    While {
        cond: ResolvedExpr,
        body: Box<ResolvedBlock>,
    },
    /// Phase D.5 (2/N) / ADR 0036 D9: `break;` — exit the innermost
    /// enclosing `while` loop. Payload-free, so resolution is the
    /// identity; loop membership is validated at type-check (D7).
    Break,
    /// Phase D.5 (2/N) / ADR 0036 D9: `continue;` — next iteration of
    /// the innermost enclosing `while` loop. Payload-free.
    Continue,
    Expr(ResolvedExpr),
}

pub type ResolvedExpr = Spanned<ResolvedExprKind>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedExprKind {
    IntLit(i64),
    /// Bool literal (`true` / `false`) per ADR 0012 D5. Added at C1.3.
    BoolLit(bool),
    /// Null literal per ADR 0014 D2. Added at C1.5. The type is
    /// resolved bidirectionally at the type-check stage.
    NullLit,
    /// D.2 / ADR 0033 D2: a char/byte literal — the decoded byte.
    /// Mirrors AST's [`sentinel_ast::ExprKind::CharLit`]; types to
    /// `Type::U8` at D.2 (3/N).
    CharLit(u8),
    /// D.2 / ADR 0033 D2: a string literal — the decoded bytes.
    /// Mirrors AST's [`sentinel_ast::ExprKind::StringLit`]; types to
    /// `[u8]` (`Type::Array(ArrayElem::U8)`) at D.2 (3/N).
    StringLit(Vec<u8>),
    /// Variable reference, resolved to a binding's [`VarId`].
    Var(VarId),
    Unary(UnaryOp, Box<ResolvedExpr>),
    Binary(BinOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    /// Comparison per ADR 0012 D6. Mirrors AST's [`ExprKind::Cmp`].
    Cmp(CmpOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    /// Logical `&&` / `||` per ADR 0012 D7. Mirrors AST's
    /// [`ExprKind::Logic`]. Short-circuit semantics belong to codegen.
    Logic(LogicOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    Block(Box<ResolvedBlock>),
    If {
        cond: Box<ResolvedExpr>,
        then_branch: Box<ResolvedBlock>,
        else_branch: Box<ResolvedBlock>,
    },
    Call {
        id: FnId,
        /// Span of the callee name (for diagnostics like arity
        /// mismatches, even though arity is checked here at resolve
        /// time).
        callee_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// Struct literal per ADR 0013 D3. The struct's [`StructId`] is
    /// resolved here; field names stay as strings (the type checker
    /// validates them at C1.4.5).
    StructLit {
        id: StructId,
        name: String,
        name_span: Span,
        fields: Vec<ResolvedFieldInit>,
    },
    /// Field access per ADR 0013 D2. The field stays as a string
    /// (the type checker validates it against the struct
    /// declaration at C1.4.5; we don't know the target's type yet).
    FieldAccess {
        target: Box<ResolvedExpr>,
        field: String,
        field_span: Span,
    },
    /// Array literal `[e1, e2, …]` per ADR 0015 D2. Pass-through;
    /// type checker validates element-type consistency.
    ArrayLit(Vec<ResolvedExpr>),
    /// Postfix indexing `target[index]` per ADR 0015 D3.
    /// Pass-through; type checker validates target is `[T]` and
    /// index is `i64`.
    Index {
        target: Box<ResolvedExpr>,
        index: Box<ResolvedExpr>,
    },
    /// C3 / ADR 0019 D6 (C3.1): `declassify(e)` strips one layer
    /// of `secret` from the inner expression. Type-check validates
    /// the inner is `secret T` and produces T as the result.
    /// Idempotent on non-secret types per ADR 0008 D5.
    Declassify(Box<ResolvedExpr>),
    /// C3.4 / ADR 0020 D4 + D5: `handle expr with { arms }`. The
    /// effects handled by `arms` are subtracted from the body's
    /// inferred row at the type-check / effect-check pass per D6.
    Handle {
        body: Box<ResolvedExpr>,
        arms: Vec<ResolvedHandlerArm>,
        return_arm: Option<Box<ResolvedReturnArm>>,
    },
    /// C3.4 / ADR 0020 D4 + D5: `perform EffectName.OpName(args)`.
    /// `effect_id` + `op_index` resolve the (Effect, Op) pair
    /// against `ResolvedProgram::effects`; the op_index is the
    /// position of the op inside its parent effect's `ops` list.
    Perform {
        effect_id: EffectId,
        op_index: usize,
        effect_name: String,
        effect_span: Span,
        op_name: String,
        op_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// C3.4 / ADR 0020 D5: a continuation-resume call `k(arg)`
    /// inside a handler arm body. `kont` is the VarId of the
    /// arm's continuation binding (last entry in the arm's
    /// `param_var_ids`). The args' types are checked against the
    /// op's return type + the call's result type is the outer
    /// `handle` expression's type at type-check time.
    ///
    /// Resolve produces this variant whenever a `Call`'s callee
    /// name resolves to an in-scope VarId — type-check verifies
    /// the binding actually has a continuation type (and surfaces
    /// `TypeError::KontCallOnNonKont` if not).
    ResumeKont {
        kont: VarId,
        callee_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// C4.1 / ADR 0022 D3 + D7: postfix method call
    /// `target.method(args)`. The method name stays as a string
    /// here — type-check looks it up against the receiver's class
    /// type and inserts the auto-ref of `target` per ADR 0021 D2.
    MethodCall {
        target: Box<ResolvedExpr>,
        method: String,
        method_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// C4.1 / ADR 0022 D5: `Name::init(args)` class instantiation.
    /// The class's [`ClassId`] is resolved here; arg-typechecking
    /// against the init's params happens in type-check.
    ClassInit {
        id: ClassId,
        name: String,
        name_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// C4.2 / ADR 0023 D5 Path 2: `ImplName::method(args)` qualified
    /// call resolved to a specific named impl. The first arg is the
    /// receiver — no auto-ref; the user passes `&obj` / `&mut obj`
    /// explicitly. `impl_id` indexes into [`ResolvedProgram::impls`];
    /// `method_index` indexes into the impl's `methods` vec.
    /// Type-check validates the receiver type matches the impl's
    /// `for Type` clause + arg arity/types against the method sig.
    QualifiedCall {
        impl_id: ImplId,
        method_index: usize,
        impl_name: String,
        impl_name_span: Span,
        method: String,
        method_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// C4.4 / ADR 0024 D1: `scope concurrent { ... }`. Pass-through
    /// at resolve time; types + runtime + codegen at C4.4 (2/N).
    Scope {
        mode: sentinel_ast::ScopeMode,
        body: Box<ResolvedBlock>,
    },
    /// C4.4 / ADR 0024 D2: `spawn fn_name(args)`. Pass-through at
    /// resolve time; the inner expression must be a function call
    /// (validated at C4.4 (2/N) types layer).
    Spawn {
        call_expr: Box<ResolvedExpr>,
    },
    /// C4.4 / ADR 0024 D3: `task.await`. Pass-through.
    Await {
        task_expr: Box<ResolvedExpr>,
    },
    /// Phase D.1 / ADR 0032 (3/N): sum-type variant construction
    /// `Enum::Variant(args)` (payload) or `Enum::Variant()` (unit).
    /// Disambiguated *here* in resolve from `ImplName::method` /
    /// `Class::init` (the AST parses all three as
    /// [`ExprKind::QualifiedCall`] / [`ExprKind::ClassInit`]): when
    /// the leading name resolves to an enum in the enum table, it is
    /// variant construction. The variant *name* stays as a string —
    /// the type checker resolves it to a variant index, validates the
    /// payload arity + types, and gives the node `Type::Enum(enum_id)`
    /// (emitting `UnknownVariant` / `VariantPayloadArityMismatch` on
    /// failure).
    EnumConstruct {
        enum_id: EnumId,
        enum_name: String,
        variant_name: String,
        variant_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// Phase D.1 / ADR 0032 (3/N): `match scrutinee { arm, … }`. Each
    /// arm's pattern bindings are assigned [`VarId`]s here and scoped
    /// into that arm's body only (snapshot/restore of `vars`, like
    /// handler-arm params). The type checker resolves each pattern's
    /// variant against the scrutinee's enum, types the bindings from
    /// the variant payloads, checks arm-body unification, and enforces
    /// exhaustiveness.
    Match {
        scrutinee: Box<ResolvedExpr>,
        arms: Vec<ResolvedMatchArm>,
    },
}

/// Phase D.1 / ADR 0032 (3/N): one resolved `match` arm —
/// `pattern => body`. The body has been resolved with the pattern's
/// bindings in scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedMatchArm {
    pub pattern: ResolvedPattern,
    pub body: ResolvedExpr,
    pub span: Span,
}

/// Phase D.1 / ADR 0032 (3/N): a resolved `match`-arm pattern. The
/// enum + variant names stay as strings — the type checker resolves
/// them against the scrutinee's enum (so a pattern naming the wrong
/// enum / a non-enum scrutinee surfaces as a `TypeError`). The MVP
/// supports a qualified variant pattern and the `_` wildcard
/// (ADR 0032 D10).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedPattern {
    /// `Enum::Variant` (unit) / `Enum::Variant(b1, b2)` (binds the
    /// payload positionally).
    Variant {
        enum_name: String,
        enum_name_span: Span,
        variant_name: String,
        variant_span: Span,
        bindings: Vec<ResolvedPatternBinding>,
        span: Span,
    },
    /// The `_` wildcard (catch-all; covers all remaining variants).
    Wildcard(Span),
}

/// Phase D.1 / ADR 0032 (3/N): one positional binding inside a
/// variant pattern. Every payload slot gets a [`VarId`] (so codegen
/// can load the slot positionally), but only a named (non-`_`)
/// binding is inserted into the arm-body scope. The type checker
/// binds `var_id`'s env type from the matched variant's payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedPatternBinding {
    pub var_id: VarId,
    /// Source name; `"_"` for a wildcard binding (not in scope).
    pub name: String,
    pub span: Span,
}

/// C3.4 / ADR 0020 D5: a resolved handler arm. Each arm's effect+op
/// pair has been resolved to (`effect_id`, `op_index`); each
/// param name has been assigned its own [`VarId`]. The kont is
/// `param_var_ids.last()` — by D5 the last parameter is always
/// the continuation binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedHandlerArm {
    pub effect_id: EffectId,
    pub op_index: usize,
    pub effect_name: String,
    pub effect_span: Span,
    pub op_name: String,
    pub op_span: Span,
    /// VarIds for the op's params and (last) the continuation
    /// binding. Length = op's arity + 1.
    pub param_var_ids: Vec<VarId>,
    /// Source-level param names (parallel to `param_var_ids`)
    /// retained for diagnostics and IR debug.
    pub param_names: Vec<Spanned<String>>,
    pub body: ResolvedExpr,
    pub span: Span,
}

/// C3.4 / ADR 0020 D4: the optional `return v => body` arm.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedReturnArm {
    pub value_var_id: VarId,
    pub value_name: Spanned<String>,
    pub body: ResolvedExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedFieldInit {
    pub name: String,
    pub name_span: Span,
    pub value: ResolvedExpr,
    pub span: Span,
}

// =============================================================================
// Errors
// =============================================================================

#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum ResolveError {
    #[error("undefined variable `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_variable),
        help("declare it with `let {name} = ...;` before this reference")
    )]
    UndefinedVariable {
        name: String,
        #[label("not declared in this scope")]
        span: miette::SourceSpan,
    },

    #[error("variable `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redeclared_variable),
        help("C0 scoping is flat per function; pick a different name or remove the earlier binding")
    )]
    RedeclaredVariable {
        name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },

    #[error("undefined function `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_function),
        help("define it with `fn {name}(…) {{ … }}` or use `print` for the runtime builtin")
    )]
    UndefinedFunction {
        name: String,
        #[label("no such function")]
        span: miette::SourceSpan,
    },

    #[error("`{name}` takes {expected} argument(s), got {got}")]
    #[diagnostic(code(sentinel::resolve::arity_mismatch))]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },

    #[error("function `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_function),
        help("each fn name must be unique within a program; `print` is reserved by the runtime")
    )]
    RedefinedFunction {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): enum names share the type
    /// namespace with structs, classes, and traits.
    #[error("`{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_enum),
        help("enum names share a namespace with structs, classes, and traits — pick a unique name")
    )]
    RedefinedEnum {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): two variants of one enum share a
    /// name.
    #[error("enum `{enum_name}` has a duplicate variant `{variant_name}`")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_variant),
        help("variant names must be unique within their enum")
    )]
    DuplicateVariant {
        enum_name: String,
        variant_name: String,
        #[label("duplicate variant")]
        span: miette::SourceSpan,
    },

    /// Phase D.1 / ADR 0032 (3/N): a `match`-arm pattern binds the
    /// same name twice (e.g. `Pair::Both(x, x)`). MVP keeps bindings
    /// linear within a pattern, matching the let/handler-param rule.
    #[error("pattern binds `{name}` more than once")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_pattern_binding),
        help("each payload binding in a pattern needs a distinct name (use `_` to ignore a slot)")
    )]
    DuplicatePatternBinding {
        name: String,
        #[label("`{name}` already bound in this pattern")]
        span: miette::SourceSpan,
    },

    #[error("program has no `main` function")]
    #[diagnostic(
        code(sentinel::resolve::missing_main),
        help("add `fn main() {{ … }}` — it is the program entry point")
    )]
    MissingMain,

    /// C1.4 / ADR 0013 D1: struct names must be unique within a
    /// program.
    #[error("struct `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_struct),
        help("each struct name must be unique within a program")
    )]
    RedefinedStruct {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D3: struct literal references an unknown
    /// struct name. (Type-position uses of unknown names surface as
    /// `TypeError::UnknownType` at the type-check stage, since the
    /// type-resolution lookup happens there. Expression-position
    /// uses are caught here at resolve time.)
    #[error("undefined struct `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_struct),
        help("declare it with `struct {name} {{ … }}` at the top level before this reference")
    )]
    UndefinedStruct {
        name: String,
        #[label("no such struct")]
        span: miette::SourceSpan,
    },

    /// C1.7 / ADR 0016 D9: a fn or struct lists the same type
    /// parameter name twice (e.g., `fn f<T, T>(...)`).
    #[error("duplicate type parameter `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_type_param),
        help("each type parameter in a `<...>` list must be unique within its fn or struct per ADR 0016 D9")
    )]
    DuplicateTypeParam {
        name: String,
        #[label("redeclared here")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D4 (C3.2): two `effect` declarations share
    /// the same name. Effect names share a namespace per the
    /// current single-namespace model (revisit when modules
    /// land).
    #[error("effect `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_effect),
        help("each effect name must be unique within a program")
    )]
    RedefinedEffect {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D1 (C3.2): a fn's postfix effect-row
    /// annotation references an effect name that isn't declared
    /// anywhere in the program.
    #[error("undefined effect `{name}` in fn `{fn_name}`'s annotation")]
    #[diagnostic(
        code(sentinel::resolve::undefined_effect),
        help("declare it at the top level with `effect {name} {{ ... }}` before this reference")
    )]
    UndefinedEffect {
        name: String,
        fn_name: String,
        #[label("no such effect")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D4 (C3.2): two ops inside the same `effect`
    /// declaration share the same name.
    #[error("operation `{op_name}` is declared twice in `effect {effect_name}`")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_effect_op),
        help("each op name must be unique within its parent effect")
    )]
    DuplicateEffectOp {
        effect_name: String,
        op_name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C3 / ADR 0019 D6 (C3.0): `declassify(e)` parses at C3.0 but
    /// is not yet resolvable. The expression lands when secret
    /// typing arrives at C3.1.
    #[error("`declassify(...)` is not yet supported (lands at C3.1)")]
    #[diagnostic(
        code(sentinel::resolve::declassify_not_yet),
        help("the `secret T` qualifier + `declassify` form ship together at C3.1 per ADR 0019 D5/D6")
    )]
    DeclassifyNotYet {
        #[label("declassify expression here")]
        span: miette::SourceSpan,
    },

    /// Phase D.6 (1/N) / ADR 0037: a top-level `use` import parses, but
    /// the resolve-layer module graph + multi-file discovery that gives it
    /// meaning land in the D.6 (1/N) resolve increment. Until then a
    /// non-empty `use` set is rejected here, so single-file programs (no
    /// `use`) are unaffected and multi-file ones fail honestly.
    #[error("`use` imports are not yet wired (modules land in Phase D.6)")]
    #[diagnostic(
        code(sentinel::resolve::use_decl_not_yet),
        help("multi-file modules (file-as-module + `use`) are landing in Phase D.6 per ADR 0037; the front-end parses `use` ahead of the resolve module graph")
    )]
    UseDeclNotYet {
        #[label("`use` import here")]
        span: miette::SourceSpan,
    },

    /// Phase D.6 (1/N) / ADR 0037 D3: a `use path::Item;` names a module
    /// (`path`) that is not in the module graph reached from the entry.
    /// (Discovery normally catches a missing module FILE first; this is
    /// the resolve-layer guard.)
    #[error("module `{module}` not found")]
    #[diagnostic(
        code(sentinel::resolve::module_not_found),
        help("file-as-module: `use a::b::Item;` expects a module `a::b` (file `a/b.sentinel`) relative to the source root")
    )]
    ModuleNotFound {
        module: String,
        #[label("no such module")]
        span: miette::SourceSpan,
    },

    /// Phase D.6 (1/N) / ADR 0037 D3: a `use path::Item;` imports an item
    /// that exists in the target module but is not `pub` — module-private
    /// items are not visible across modules.
    #[error("`{item}` is private to module `{module}`")]
    #[diagnostic(
        code(sentinel::resolve::private_item),
        help("only `pub` items are importable across modules; add `pub` to the item's declaration in its module")
    )]
    PrivateItem {
        item: String,
        module: String,
        #[label("private item imported here")]
        span: miette::SourceSpan,
    },

    /// Phase D.6 (1/N) / ADR 0037 D3: a `use path::Item;` names an item
    /// (`Item`) that does not exist in the target module.
    #[error("module `{module}` has no item `{item}`")]
    #[diagnostic(
        code(sentinel::resolve::unknown_import),
        help("the last `::` segment of a `use` is the imported item; check the name + that the module declares it")
    )]
    UnknownImport {
        item: String,
        module: String,
        #[label("no such item in that module")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: a `handle` arm or `perform` expression
    /// references an effect name that isn't declared anywhere in
    /// the program.
    #[error("undefined effect `{name}` in handler arm / perform")]
    #[diagnostic(
        code(sentinel::resolve::undefined_handler_effect),
        help("declare it at the top level with `effect {name} {{ ... }}` before this reference")
    )]
    UndefinedHandlerEffect {
        name: String,
        #[label("no such effect")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: a `handle` arm or `perform` references
    /// an op name that isn't declared inside the named effect.
    #[error("operation `{op_name}` is not declared in `effect {effect_name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_handler_op),
        help("check the effect declaration for the available op names")
    )]
    UndefinedHandlerOp {
        effect_name: String,
        op_name: String,
        #[label("no such op on `{effect_name}`")]
        span: miette::SourceSpan,
    },

    /// C3.4 / ADR 0020 D5: two arms in the same `handle` cover the
    /// same (effect, op) pair.
    #[error(
        "duplicate handler arm for `{effect_name}.{op_name}` in this `handle`"
    )]
    #[diagnostic(
        code(sentinel::resolve::duplicate_handler_arm),
        help("each (effect, op) pair must appear at most once per `handle`")
    )]
    DuplicateHandlerArm {
        effect_name: String,
        op_name: String,
        #[label("duplicate arm")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D1: two `class` declarations share the
    /// same name. Class names share a namespace with structs at
    /// C4.1 — `struct Point` and `class Point` collide.
    #[error("class `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_class),
        help("each class name must be unique within a program; class and struct names share a namespace at C4.1")
    )]
    RedefinedClass {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D5: `Name::init(args)` references an
    /// unknown class. Surfaces at resolve when `Name` is not in
    /// the class table.
    #[error("undefined class `{name}` in `{name}::init(...)`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_class),
        help("declare it with `class {name} {{ ... }}` at the top level before this reference")
    )]
    UndefinedClass {
        name: String,
        #[label("no such class")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D2: two `let` field declarations inside
    /// the same class share a name.
    #[error("class `{class_name}` declares field `{field_name}` twice")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_class_field),
        help("each field name must be unique within a class")
    )]
    DuplicateClassField {
        class_name: String,
        field_name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D3: two methods inside the same class
    /// share a name (no method overloading at C4.1 — D3).
    #[error("class `{class_name}` declares method `{method_name}` twice")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_class_method),
        help("methods share a namespace per class at C4.1; no method overloading")
    )]
    DuplicateClassMethod {
        class_name: String,
        method_name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },

    /// C4.1 / ADR 0022 D8: `self` (the value) appears outside a
    /// class method or init body. Lexer reserves `self` from C4.0
    /// onward; resolve enforces the in-class-context constraint.
    #[error("`self` is only valid inside a class method or `init` body")]
    #[diagnostic(
        code(sentinel::resolve::self_outside_class_context),
        help("the `self` receiver appears only as the implicit first parameter of a class method or `init` per ADR 0022 D8")
    )]
    SelfOutsideClassContext {
        #[label("`self` used here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D1: two `trait` declarations share the same
    /// name. Trait names share a namespace with classes and structs.
    #[error("trait `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_trait),
        help("each trait name must be unique within a program; trait, class, and struct names share a namespace at C4.2")
    )]
    RedefinedTrait {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D2: two method signatures inside the same
    /// trait share a name. No method overloading at C4.2.
    #[error("trait `{trait_name}` declares method `{method_name}` twice")]
    #[diagnostic(
        code(sentinel::resolve::duplicate_trait_method),
        help("methods share a namespace per trait at C4.2; no method overloading")
    )]
    DuplicateTraitMethod {
        trait_name: String,
        method_name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D3: `impl as Trait for Type` references an
    /// unknown trait name. Surfaces at resolve when `Trait` is not
    /// in the trait table.
    #[error("undefined trait `{name}` in impl block")]
    #[diagnostic(
        code(sentinel::resolve::undefined_trait_for_impl),
        help("declare it with `trait {name} {{ ... }}` at the top level before this reference")
    )]
    UndefinedTraitForImpl {
        name: String,
        #[label("no such trait")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D3: `impl as Trait for Type` references an
    /// unknown receiver type. Currently classes + structs are valid
    /// targets; impl-for-generic is deferred per ADR 0023 D10.
    #[error("undefined type `{name}` in impl block")]
    #[diagnostic(
        code(sentinel::resolve::undefined_type_for_impl),
        help("the impl's `for Type` clause must name an existing class or struct; generic impl targets are deferred per ADR 0023 D10")
    )]
    UndefinedTypeForImpl {
        name: String,
        #[label("no such type")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D8: two default impls of the same trait for
    /// the same type in the same scope. Scope-local coherence per
    /// ADR 0021 D7 forbids duplicates.
    #[error(
        "duplicate default impl of `{trait_name}` for `{type_name}` in this scope"
    )]
    #[diagnostic(
        code(sentinel::resolve::duplicate_default_impl),
        help("each (trait, type) pair may have at most one default impl per scope; use a named impl (`impl Name as Trait for Type`) to provide an alternative")
    )]
    DuplicateDefaultImpl {
        trait_name: String,
        type_name: String,
        #[label("duplicate default impl here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D8: two named impls of the same trait for
    /// the same type share a name within a scope.
    #[error(
        "duplicate impl name `{impl_name}` for `{trait_name}` on `{type_name}`"
    )]
    #[diagnostic(
        code(sentinel::resolve::duplicate_impl_name),
        help("each named impl of a (trait, type) pair must have a unique name within its scope")
    )]
    DuplicateImplName {
        impl_name: String,
        trait_name: String,
        type_name: String,
        #[label("duplicate impl name here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D6: `ImplName::method(args)` references an
    /// unknown impl name. Surfaces at resolve when `ImplName` is
    /// not in the impl-name table.
    #[error("undefined impl `{name}` in `{name}::{method}(...)`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_impl),
        help("declare it with `impl {name} as Trait for Type {{ ... }}` at the top level before this reference")
    )]
    UndefinedImpl {
        name: String,
        method: String,
        #[label("no such impl")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D6: `ImplName::method(...)` references a
    /// method name that isn't declared on the impl's trait.
    #[error(
        "trait `{trait_name}` (impl `{impl_name}`) has no method `{method_name}`"
    )]
    #[diagnostic(
        code(sentinel::resolve::undefined_trait_method),
        help("check the trait declaration for the available method names")
    )]
    UndefinedTraitMethod {
        impl_name: String,
        trait_name: String,
        method_name: String,
        #[label("no such method on `{trait_name}`")]
        span: miette::SourceSpan,
    },
}

// =============================================================================
// resolve() — pure-function entry point
// =============================================================================

/// Phase D.6 (1/N) / ADR 0037 D3: one discovered module — its module
/// path relative to the source root (e.g. `["util", "math"]`) plus its
/// parsed AST. The driver builds these from `discover_module_graph`.
pub struct ModuleUnit<'a> {
    pub path: Vec<String>,
    pub program: &'a Program,
}

/// Phase D.6 (1/N) / ADR 0037 D3: validate every module's `use` imports
/// against the module graph. Each `use a::b::Item;` must name a module
/// `a::b` present in `modules` that declares a **`pub`** item `Item` —
/// the cross-module visibility gate (file-as-module: un-`pub` items are
/// module-private). Errors fast: `ModuleNotFound` / `UnknownImport` /
/// `PrivateItem`. (The qualification + reference rewrite that merges the
/// graph into one `Program` — the lower-risk Path A — builds on this;
/// true per-unit resolve is deferred to a later sub-phase per ADR 0037.)
pub fn resolve_imports(modules: &[ModuleUnit]) -> Result<(), ResolveError> {
    for unit in modules {
        for u in &unit.program.uses {
            // Parser guarantees ≥ 2 segments: the module is all but the
            // last segment; the last is the imported item (open point 4).
            let item = u.path.last().expect("use path has ≥ 1 segment");
            let module_path = &u.path[..u.path.len() - 1];
            let target = match modules.iter().find(|m| m.path.as_slice() == module_path) {
                Some(t) => t,
                None => {
                    return Err(ResolveError::ModuleNotFound {
                        module: module_path.join("::"),
                        span: to_source_span(&u.span),
                    });
                }
            };
            match pub_item_visibility(target.program, item) {
                None => {
                    return Err(ResolveError::UnknownImport {
                        item: item.clone(),
                        module: module_path.join("::"),
                        span: to_source_span(&u.span),
                    });
                }
                Some(false) => {
                    return Err(ResolveError::PrivateItem {
                        item: item.clone(),
                        module: module_path.join("::"),
                        span: to_source_span(&u.span),
                    });
                }
                Some(true) => {} // `pub` — visible across modules.
            }
        }
    }
    Ok(())
}

/// Whether `program` declares a top-level item named `name`, and if so
/// whether it is `pub`. `None` = no such item. Scans the importable item
/// kinds (fn / struct / enum / trait / effect — ADR 0037 D2).
fn pub_item_visibility(program: &Program, name: &str) -> Option<bool> {
    let is_pub = |v: sentinel_ast::Visibility| v == sentinel_ast::Visibility::Public;
    if let Some(f) = program.fns.iter().find(|f| f.name == name) {
        return Some(is_pub(f.visibility));
    }
    if let Some(s) = program.structs.iter().find(|s| s.name == name) {
        return Some(is_pub(s.visibility));
    }
    if let Some(e) = program.enums.iter().find(|e| e.name == name) {
        return Some(is_pub(e.visibility));
    }
    if let Some(t) = program.traits.iter().find(|t| t.name == name) {
        return Some(is_pub(t.visibility));
    }
    if let Some(ef) = program.effects.iter().find(|ef| ef.name == name) {
        return Some(is_pub(ef.visibility));
    }
    None
}

/// Phase D.6 (1/N) / ADR 0037 (Path A): merge a discovered module graph
/// into ONE [`Program`] the existing single-file pipeline can compile.
/// **Every** top-level item — fn, struct, enum, trait, effect, class, and
/// named impl — has its name qualified by module path (`util::add` → the
/// symbol `util$add`, using `$`, not a valid Sentinel identifier char, so
/// collision-free), so same-named items across modules never clash; and
/// **every reference** is rewritten per module scope: call callees, struct
/// literals, enum construction / patterns, `impl … as Trait for Type`
/// heads, `perform`/`handle` effect names, fn/method effect rows, delegate
/// trait names, and all type annotations (params / returns / fields /
/// variant payloads / `let` types). Own items + `use`d-pub imports are
/// qualified; builtins / primitives / type parameters / locals are left
/// untouched. The entry module (`modules[0]`)'s `main` keeps the symbol
/// `main`. Imports are validated first via [`resolve_imports`]
/// (visibility / existence).
///
/// What this enables: cross-module FUNCTIONS, DATA TYPES (structs / enums),
/// TRAITS, EFFECTS, and CLASSES — a `pub` item of any of those kinds can be
/// imported + used across a file boundary, and same-named items coexist.
/// (Cross-module GENERICS are (2/N); per-unit separate compilation is the
/// follow-up back end.) Spans point into per-module source (multi-module
/// error spans are approximate until per-unit compilation lands).
pub fn merge_modules(modules: &[ModuleUnit]) -> Result<Program, ResolveError> {
    resolve_imports(modules)?; // visibility / existence gate — errors fast.

    let mut fns = Vec::new();
    let mut structs = Vec::new();
    let mut effects = Vec::new();
    let mut classes = Vec::new();
    let mut traits = Vec::new();
    let mut impls = Vec::new();
    let mut enums = Vec::new();
    let mut span_end = 0usize;

    for (idx, unit) in modules.iter().enumerate() {
        let is_entry = idx == 0;
        let prefix = unit.path.join("$");

        // Build this module's name → qualified-symbol map. EVERY top-level
        // item is qualified by module path — fns, structs, enums, traits,
        // effects, classes, AND named impls — so same-named items across
        // modules never clash; only the entry's `main` keeps its symbol.
        // `use`d imports map to their defining module's qualified symbol
        // (resolve_imports already gated existence + visibility, and every
        // importable kind is qualified). Builtins / primitives / type params
        // / locals are absent from the map → left untouched at the rewrite.
        let mut rename: HashMap<String, String> = HashMap::new();
        for f in &unit.program.fns {
            let qualified = if is_entry && f.name == "main" {
                "main".to_string()
            } else {
                format!("{prefix}${}", f.name)
            };
            rename.insert(f.name.clone(), qualified);
        }
        for s in &unit.program.structs {
            rename.insert(s.name.clone(), format!("{prefix}${}", s.name));
        }
        for e in &unit.program.enums {
            rename.insert(e.name.clone(), format!("{prefix}${}", e.name));
        }
        for t in &unit.program.traits {
            rename.insert(t.name.clone(), format!("{prefix}${}", t.name));
        }
        for ef in &unit.program.effects {
            rename.insert(ef.name.clone(), format!("{prefix}${}", ef.name));
        }
        for c in &unit.program.classes {
            rename.insert(c.name.clone(), format!("{prefix}${}", c.name));
        }
        for i in &unit.program.impls {
            if let Some(n) = &i.name {
                rename.insert(n.clone(), format!("{prefix}${n}"));
            }
        }
        for u in &unit.program.uses {
            let item = u.path.last().expect("validated: ≥ 1 segment");
            let module_path = &u.path[..u.path.len() - 1];
            rename.insert(item.clone(), format!("{}${}", module_path.join("$"), item));
        }

        // Clone + qualify each declaration's name, then reference-rewrite its
        // body + signature with this module's map.
        for f in &unit.program.fns {
            let mut nf = f.clone();
            nf.name = rename.get(&f.name).cloned().unwrap_or_else(|| f.name.clone());
            rewrite_fn_def(&mut nf, &rename);
            span_end = span_end.max(nf.span.end);
            fns.push(nf);
        }
        for s in &unit.program.structs {
            let mut ns = s.clone();
            ns.name = rename.get(&s.name).cloned().unwrap_or_else(|| s.name.clone());
            rewrite_struct_decl(&mut ns, &rename);
            span_end = span_end.max(ns.span.end);
            structs.push(ns);
        }
        for e in &unit.program.enums {
            let mut ne = e.clone();
            ne.name = rename.get(&e.name).cloned().unwrap_or_else(|| e.name.clone());
            rewrite_enum_decl(&mut ne, &rename);
            span_end = span_end.max(ne.span.end);
            enums.push(ne);
        }
        for t in &unit.program.traits {
            let mut nt = t.clone();
            nt.name = rename.get(&t.name).cloned().unwrap_or_else(|| t.name.clone());
            rewrite_trait_decl(&mut nt, &rename);
            span_end = span_end.max(nt.span.end);
            traits.push(nt);
        }
        for ef in &unit.program.effects {
            let mut nef = ef.clone();
            nef.name = rename.get(&ef.name).cloned().unwrap_or_else(|| ef.name.clone());
            rewrite_effect_decl(&mut nef, &rename);
            span_end = span_end.max(nef.span.end);
            effects.push(nef);
        }
        for c in &unit.program.classes {
            let mut nc = c.clone();
            nc.name = rename.get(&c.name).cloned().unwrap_or_else(|| c.name.clone());
            rewrite_class_decl(&mut nc, &rename);
            span_end = span_end.max(nc.span.end);
            classes.push(nc);
        }
        for i in &unit.program.impls {
            let mut ni = i.clone();
            if let Some(n) = &i.name {
                ni.name = Some(rename.get(n).cloned().unwrap_or_else(|| n.clone()));
            }
            rewrite_impl_decl(&mut ni, &rename);
            span_end = span_end.max(ni.span.end);
            impls.push(ni);
        }
    }

    Ok(Program {
        uses: Vec::new(),
        fns,
        structs,
        effects,
        classes,
        traits,
        impls,
        enums,
        span: 0..span_end,
    })
}

/// A per-module rewrite context for [`merge_modules`] (Phase D.6 (1/N) /
/// ADR 0037 Path A): the module's name → module-qualified-symbol map plus
/// the type-parameter names currently in scope. A generic parameter `T`
/// shadows any same-named top-level item, so it is NEVER qualified — only
/// `tparams` differs between a decl and the surrounding module.
struct Renamer<'a> {
    rename: &'a HashMap<String, String>,
    tparams: HashSet<String>,
}

impl<'a> Renamer<'a> {
    /// A renamer with no type parameters in scope — the common case
    /// (enums / traits / effects / classes / impls are non-generic at
    /// this slice).
    fn new(rename: &'a HashMap<String, String>) -> Self {
        Renamer { rename, tparams: HashSet::new() }
    }

    /// A renamer whose `tparams` are the names of `params` — the generic
    /// parameters of the enclosing fn / struct, which must not be
    /// qualified even if a top-level item shares the name.
    fn with_type_params(rename: &'a HashMap<String, String>, params: &[AstTypeParam]) -> Self {
        Renamer { rename, tparams: params.iter().map(|p| p.name.clone()).collect() }
    }

    /// Rewrite `name` in place to its module-qualified symbol, unless it
    /// is an in-scope type parameter (never qualified) or absent from the
    /// map (a builtin / primitive / unqualified-kind item / local — left
    /// as written).
    fn apply(&self, name: &mut String) {
        if self.tparams.contains(name.as_str()) {
            return;
        }
        if let Some(qualified) = self.rename.get(name.as_str()) {
            *name = qualified.clone();
        }
    }
}

// --- per-declaration walkers ------------------------------------------------

/// Rewrite a function's signature (param + return types) and body. The
/// fn's own `type_params` shadow same-named top-level items throughout.
fn rewrite_fn_def(f: &mut FnDef, rename: &HashMap<String, String>) {
    let r = Renamer::with_type_params(rename, &f.type_params);
    rewrite_params(&mut f.params, &r);
    rewrite_type_expr(&mut f.return_type, &r);
    rewrite_effect_row(&mut f.effect_row, &r);
    rewrite_block(&mut f.body, &r);
}

/// Rewrite the effect names in a postfix effect-row annotation
/// (`! { Net, Io }`) — each entry names a (now module-qualified) effect.
fn rewrite_effect_row(row: &mut [Spanned<String>], r: &Renamer) {
    for entry in row {
        r.apply(&mut entry.kind);
    }
}

/// Rewrite a struct's field types. The struct's own `type_params` are in
/// scope (a generic field `value: T` must not be qualified).
fn rewrite_struct_decl(s: &mut StructDecl, rename: &HashMap<String, String>) {
    let r = Renamer::with_type_params(rename, &s.type_params);
    for field in &mut s.fields {
        rewrite_type_expr(&mut field.ty, &r);
    }
}

/// Rewrite an enum's variant payload types. Enums are non-generic at the
/// D.1 MVP, so no type parameters are in scope.
fn rewrite_enum_decl(e: &mut EnumDecl, rename: &HashMap<String, String>) {
    let r = Renamer::new(rename);
    for variant in &mut e.variants {
        for payload in &mut variant.payloads {
            rewrite_type_expr(payload, &r);
        }
    }
}

/// Rewrite a trait's method signatures (param + return types + effect
/// rows). The trait's own name is qualified by the merge loop.
fn rewrite_trait_decl(t: &mut TraitDecl, rename: &HashMap<String, String>) {
    let r = Renamer::new(rename);
    for m in &mut t.methods {
        rewrite_params(&mut m.params, &r);
        rewrite_type_expr(&mut m.return_type, &r);
        rewrite_effect_row(&mut m.effect_row, &r);
    }
}

/// Rewrite an effect's operation signatures (param + optional return
/// types). The effect's own name is qualified by the merge loop; op names
/// stay (they are scoped within the effect, like enum variants).
fn rewrite_effect_decl(ef: &mut EffectDecl, rename: &HashMap<String, String>) {
    let r = Renamer::new(rename);
    for op in &mut ef.ops {
        rewrite_params(&mut op.params, &r);
        if let Some(rt) = &mut op.return_type {
            rewrite_type_expr(rt, &r);
        }
    }
}

/// Rewrite a class's field types, init, methods (sig + effect row + body),
/// and delegate field types + forwarded trait names. The class's own name
/// is qualified by the merge loop. Classes are non-generic at the MVP, so
/// no type parameters are in scope.
fn rewrite_class_decl(c: &mut ClassDecl, rename: &HashMap<String, String>) {
    let r = Renamer::new(rename);
    for field in &mut c.fields {
        rewrite_type_expr(&mut field.ty, &r);
    }
    if let Some(init) = &mut c.init {
        rewrite_params(&mut init.params, &r);
        rewrite_block(&mut init.body, &r);
    }
    for m in &mut c.methods {
        rewrite_params(&mut m.params, &r);
        rewrite_type_expr(&mut m.return_type, &r);
        rewrite_effect_row(&mut m.effect_row, &r);
        rewrite_block(&mut m.body, &r);
    }
    for d in &mut c.delegates {
        rewrite_type_expr(&mut d.ty, &r);
        r.apply(&mut d.trait_name);
    }
}

/// Rewrite an impl block: the impl'd `type_name` AND the `trait_name` (so
/// an `impl … as <trait> for <type>` finds both qualified decls) plus each
/// method's sig + effect row + body. The impl's own (optional) name is
/// qualified by the merge loop.
fn rewrite_impl_decl(i: &mut ImplDecl, rename: &HashMap<String, String>) {
    let r = Renamer::new(rename);
    r.apply(&mut i.type_name);
    r.apply(&mut i.trait_name);
    for m in &mut i.methods {
        rewrite_params(&mut m.params, &r);
        rewrite_type_expr(&mut m.return_type, &r);
        rewrite_effect_row(&mut m.effect_row, &r);
        rewrite_block(&mut m.body, &r);
    }
}

fn rewrite_params(params: &mut [Param], r: &Renamer) {
    for p in params {
        rewrite_type_expr(&mut p.ty, r);
    }
}

/// Rewrite the named types inside a type expression — `Ident`s and the
/// `Generic` head — recursing through the `?T` / `[T]` / `&T` / `secret T`
/// wrappers and generic arguments. Exhaustive (no wildcard) so a new
/// `TypeExprKind` can't silently escape qualification.
fn rewrite_type_expr(ty: &mut TypeExpr, r: &Renamer) {
    match &mut ty.kind {
        TypeExprKind::Ident(name) => r.apply(name),
        TypeExprKind::Nullable(inner)
        | TypeExprKind::Array(inner)
        | TypeExprKind::Secret(inner) => rewrite_type_expr(inner, r),
        TypeExprKind::Ref { inner, .. } => rewrite_type_expr(inner, r),
        TypeExprKind::Generic { name, args, .. } => {
            r.apply(name);
            for a in args {
                rewrite_type_expr(a, r);
            }
        }
    }
}

// --- body walkers (call callees, type-naming exprs, `let` annotations) ------

/// Rewrite references in a block per the module's [`Renamer`]
/// (Phase D.6 (1/N) / ADR 0037 Path A). See [`merge_modules`].
fn rewrite_block(block: &mut Block, r: &Renamer) {
    for stmt in &mut block.stmts {
        rewrite_stmt(stmt, r);
    }
    rewrite_expr(&mut block.tail, r);
}

fn rewrite_stmt(stmt: &mut Stmt, r: &Renamer) {
    match &mut stmt.kind {
        StmtKind::Let { ty_annot, value, .. } => {
            if let Some(t) = ty_annot {
                rewrite_type_expr(t, r);
            }
            rewrite_expr(value, r);
        }
        StmtKind::Assign { target, value } => {
            rewrite_expr(target, r);
            rewrite_expr(value, r);
        }
        StmtKind::While { cond, body } => {
            rewrite_expr(cond, r);
            rewrite_block(body, r);
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Expr(e) => rewrite_expr(e, r),
    }
}

/// Exhaustive (no wildcard) so the compiler guarantees every
/// sub-expression is recursed into — a missed variant would leave a
/// nested cross-module reference un-rewritten. Rewrites the names that can
/// denote a qualified item: `Call` callees (fns), `StructLit` names
/// (structs), `QualifiedCall` / `ClassInit` heads (enum construction /
/// class init — a named-impl head isn't in the map, so it's left alone),
/// and `match` variant patterns (enum names). Every other variant just
/// recurses.
fn rewrite_expr(expr: &mut Expr, r: &Renamer) {
    match &mut expr.kind {
        ExprKind::IntLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::NullLit
        | ExprKind::CharLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::Var(_) => {}
        ExprKind::Unary(_, e) | ExprKind::Declassify(e) => rewrite_expr(e, r),
        ExprKind::Binary(_, lhs, rhs) | ExprKind::Cmp(_, lhs, rhs) | ExprKind::Logic(_, lhs, rhs) => {
            rewrite_expr(lhs, r);
            rewrite_expr(rhs, r);
        }
        ExprKind::Block(b) => rewrite_block(b, r),
        ExprKind::If { cond, then_branch, else_branch } => {
            rewrite_expr(cond, r);
            rewrite_block(then_branch, r);
            rewrite_block(else_branch, r);
        }
        ExprKind::Call { callee, args, .. } => {
            r.apply(callee);
            for a in args {
                rewrite_expr(a, r);
            }
        }
        ExprKind::StructLit { name, fields, .. } => {
            r.apply(name);
            for f in fields {
                rewrite_expr(&mut f.value, r);
            }
        }
        ExprKind::FieldAccess { target, .. } => rewrite_expr(target, r),
        ExprKind::ArrayLit(elems) => {
            for e in elems {
                rewrite_expr(e, r);
            }
        }
        ExprKind::Index { target, index } => {
            rewrite_expr(target, r);
            rewrite_expr(index, r);
        }
        ExprKind::Handle { body, arms, return_arm } => {
            rewrite_expr(body, r);
            for arm in arms {
                // `Effect.op(..) => body`: the handled effect's name is
                // qualified (the op name is scoped within it, like a variant).
                r.apply(&mut arm.effect.kind);
                rewrite_expr(&mut arm.body, r);
            }
            if let Some(ra) = return_arm {
                rewrite_expr(&mut ra.body, r);
            }
        }
        ExprKind::Perform { effect, args, .. } => {
            // `perform Effect.op(args)`: qualify the performed effect's name.
            r.apply(&mut effect.kind);
            for a in args {
                rewrite_expr(a, r);
            }
        }
        ExprKind::MethodCall { target, args, .. } => {
            rewrite_expr(target, r);
            for a in args {
                rewrite_expr(a, r);
            }
        }
        // `ClassInit` heads a class init or an `Enum::init` variant;
        // `QualifiedCall` heads enum variant construction when the name is
        // an enum (→ qualified) and a named-impl call otherwise (the impl
        // name isn't in the map → left alone). `apply` rewrites only names
        // actually in the map, so each case lands correctly.
        ExprKind::ClassInit { class_name, args, .. } => {
            r.apply(class_name);
            for a in args {
                rewrite_expr(a, r);
            }
        }
        ExprKind::QualifiedCall { impl_name, args, .. } => {
            r.apply(impl_name);
            for a in args {
                rewrite_expr(a, r);
            }
        }
        ExprKind::Scope { body, .. } => rewrite_block(body, r),
        ExprKind::Spawn { call_expr } => rewrite_expr(call_expr, r),
        ExprKind::Await { task_expr } => rewrite_expr(task_expr, r),
        ExprKind::Match { scrutinee, arms } => {
            rewrite_expr(scrutinee, r);
            for arm in arms {
                rewrite_pattern(&mut arm.pattern, r);
                rewrite_expr(&mut arm.body, r);
            }
        }
    }
}

/// Rewrite the enum name in a `match` variant pattern (so a pattern over a
/// cross-module enum names its qualified type). The wildcard binds no
/// type. Exhaustive over [`Pattern`].
fn rewrite_pattern(pat: &mut Pattern, r: &Renamer) {
    match pat {
        Pattern::Variant { enum_name, .. } => r.apply(enum_name),
        Pattern::Wildcard(_) => {}
    }
}

/// Resolve a [`Program`] to a [`ResolvedProgram`]. Fails fast on
/// the first error encountered, matching the C0 codegen pass's
/// existing diagnostic shape. Multi-error accumulation is a future
/// concern.
///
/// Order: structs are registered first (so fn signatures can
/// reference struct types in their TypeExprs), then fns, then fn
/// bodies.
pub fn resolve(program: &Program) -> Result<ResolvedProgram, ResolveError> {
    // Phase D.6 (1/N) / ADR 0037: the front-end parses top-level `use`
    // imports, but the module graph + multi-file discovery that resolve
    // them land in the D.6 (1/N) resolve increment. Until then, reject a
    // non-empty `use` set so single-file programs (no `use`) are
    // unaffected and multi-file ones fail honestly rather than silently
    // dropping the import.
    if let Some(first_use) = program.uses.first() {
        return Err(ResolveError::UseDeclNotYet {
            span: to_source_span(&first_use.span),
        });
    }

    let mut next_fn_id: u32 = 0;
    let mut next_var_id: u32 = 0;

    // C3 / ADR 0019 D4 (C3.2): build the effect table first so fn
    // signatures' effect-row annotations can reference effect
    // names. EffectIds are assigned in source-encounter order;
    // effect names share a namespace (RedefinedEffect on collision).
    let mut effect_table: HashMap<String, EffectId> = HashMap::new();
    let mut resolved_effects: Vec<ResolvedEffectDecl> =
        Vec::with_capacity(program.effects.len());
    for (idx, ed) in program.effects.iter().enumerate() {
        if effect_table.contains_key(&ed.name) {
            return Err(ResolveError::RedefinedEffect {
                name: ed.name.clone(),
                span: to_source_span(&ed.name_span),
            });
        }
        let id = EffectId(idx as u32);
        effect_table.insert(ed.name.clone(), id);
        // Resolve each op's params + return-type (carried as
        // TypeExpr until sentinel-types resolves them). Ops within
        // an effect must have unique names.
        let mut op_names: HashSet<String> = HashSet::new();
        let mut ops = Vec::with_capacity(ed.ops.len());
        for op in &ed.ops {
            if !op_names.insert(op.name.clone()) {
                return Err(ResolveError::DuplicateEffectOp {
                    effect_name: ed.name.clone(),
                    op_name: op.name.clone(),
                    span: to_source_span(&op.name_span),
                });
            }
            // Resolve op params — VarIds aren't strictly needed
            // since ops don't have bodies at C3.2, but the params
            // mirror the AST shape for future use (handler runtime
            // at ADR 0020 binds them). Reuse the same next_var_id
            // counter so IDs stay globally unique.
            let mut params = Vec::with_capacity(op.params.len());
            for p in &op.params {
                let vid = VarId(next_var_id);
                next_var_id += 1;
                params.push(ResolvedParam {
                    id: vid,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.clone(),
                });
            }
            ops.push(ResolvedOpDecl {
                name: op.name.clone(),
                name_span: op.name_span.clone(),
                params,
                return_type: op.return_type.clone(),
                span: op.span.clone(),
            });
        }
        resolved_effects.push(ResolvedEffectDecl {
            id,
            name: ed.name.clone(),
            name_span: ed.name_span.clone(),
            ops,
            span: ed.span.clone(),
        });
    }

    // C4.4 / ADR 0024 D5: auto-register the built-in `Async` effect
    // so fns that touch `spawn` / `.await` can declare `! { Async }`.
    // It declares no ops at C4.4 minimum — the marker alone drives
    // the effect-row discipline (effect-check makes spawn/await
    // contribute Async; `scope concurrent` discharges it). Registered
    // *after* user effects so user-declared EffectIds stay stable
    // (deviation from ADR 0024 D5's "EffectId(0)" suggestion — the
    // id is an internal detail; what matters is that `Async`
    // resolves). A user effect literally named `Async` collides
    // with the reserved built-in.
    if let Some(ed) = program.effects.iter().find(|e| e.name == "Async") {
        return Err(ResolveError::RedefinedEffect {
            name: "Async".to_string(),
            span: to_source_span(&ed.name_span),
        });
    }
    let async_id = EffectId(resolved_effects.len() as u32);
    effect_table.insert("Async".to_string(), async_id);
    resolved_effects.push(ResolvedEffectDecl {
        id: async_id,
        name: "Async".to_string(),
        name_span: 0..0,
        ops: Vec::new(),
        span: 0..0,
    });

    // Pass 0: collect struct declarations. Indexed by source order
    // (each struct's StructId matches its index in resolved_structs).
    // Names must be unique within a program; struct names and fn
    // names share a namespace at C1.4 (a struct named `print` is
    // illegal, same as a fn named `print`). Future ADRs may relax.
    let mut struct_table: HashMap<String, StructId> = HashMap::new();
    let mut resolved_structs: Vec<ResolvedStructDecl> =
        Vec::with_capacity(program.structs.len());
    for (idx, sd) in program.structs.iter().enumerate() {
        if struct_table.contains_key(&sd.name) {
            return Err(ResolveError::RedefinedStruct {
                name: sd.name.clone(),
                span: to_source_span(&sd.name_span),
            });
        }
        let id = StructId(idx as u32);
        struct_table.insert(sd.name.clone(), id);
        // C1.7 / ADR 0016 D9: assign TypeParamIds + reject
        // duplicate type-param names.
        let type_params = resolve_type_params(&sd.type_params)?;
        let fields = sd
            .fields
            .iter()
            .map(|f| ResolvedStructField {
                name: f.name.clone(),
                name_span: f.name_span.clone(),
                ty: f.ty.clone(),
                span: f.span.clone(),
            })
            .collect();
        resolved_structs.push(ResolvedStructDecl {
            id,
            name: sd.name.clone(),
            name_span: sd.name_span.clone(),
            type_params,
            fields,
            span: sd.span.clone(),
        });
    }

    // C4.1 / ADR 0022 D1: collect class declarations. Class names
    // share a namespace with structs — a class named `Point`
    // collides with `struct Point`. ClassIds index into
    // resolved_classes; field + method bodies are resolved in
    // Pass 3 (after fn_table is populated, so methods can call
    // free fns).
    let mut class_table: HashMap<String, ClassId> = HashMap::new();
    for (idx, cd) in program.classes.iter().enumerate() {
        if struct_table.contains_key(&cd.name) || class_table.contains_key(&cd.name) {
            return Err(ResolveError::RedefinedClass {
                name: cd.name.clone(),
                span: to_source_span(&cd.name_span),
            });
        }
        class_table.insert(cd.name.clone(), ClassId(idx as u32));
    }

    // C4.2 / ADR 0023 D1 (Pass 0d): collect trait declarations.
    // Trait names share a namespace with structs + classes. The
    // method sigs themselves are resolved in Pass 4 below (we need
    // effect_table available, which is built above; but method
    // sig resolution is small enough to inline here once the
    // namespace check is done).
    let mut trait_table: HashMap<String, TraitId> = HashMap::new();
    let mut resolved_traits: Vec<ResolvedTraitDecl> =
        Vec::with_capacity(program.traits.len());
    for (idx, td) in program.traits.iter().enumerate() {
        if struct_table.contains_key(&td.name)
            || class_table.contains_key(&td.name)
            || trait_table.contains_key(&td.name)
        {
            return Err(ResolveError::RedefinedTrait {
                name: td.name.clone(),
                span: to_source_span(&td.name_span),
            });
        }
        let id = TraitId(idx as u32);
        trait_table.insert(td.name.clone(), id);
        // Resolve method sigs inline. Each method's effect_row
        // maps name → EffectId via effect_table (built above).
        let mut method_names: HashSet<String> = HashSet::new();
        let mut methods: Vec<ResolvedTraitMethodSig> =
            Vec::with_capacity(td.methods.len());
        for m in &td.methods {
            if !method_names.insert(m.name.clone()) {
                return Err(ResolveError::DuplicateTraitMethod {
                    trait_name: td.name.clone(),
                    method_name: m.name.clone(),
                    span: to_source_span(&m.name_span),
                });
            }
            let mut effect_row = Vec::with_capacity(m.effect_row.len());
            for entry in &m.effect_row {
                let eid = effect_table.get(&entry.kind).ok_or_else(|| {
                    ResolveError::UndefinedEffect {
                        name: entry.kind.clone(),
                        fn_name: format!("{}::{}", td.name, m.name),
                        span: to_source_span(&entry.span),
                    }
                })?;
                effect_row.push(*eid);
            }
            let params: Vec<ResolvedTraitParam> = m
                .params
                .iter()
                .map(|p| ResolvedTraitParam {
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.clone(),
                })
                .collect();
            methods.push(ResolvedTraitMethodSig {
                name: m.name.clone(),
                name_span: m.name_span.clone(),
                self_kind: m.self_kind,
                params,
                return_type: m.return_type.clone(),
                effect_row,
                span: m.span.clone(),
            });
        }
        resolved_traits.push(ResolvedTraitDecl {
            id,
            name: td.name.clone(),
            name_span: td.name_span.clone(),
            methods,
            span: td.span.clone(),
        });
    }

    // Phase D.1 / ADR 0032 (3/N) (Pass 0d.5): collect enum
    // declarations. Enum names share the type namespace with structs,
    // classes, and traits (an `enum Point` collides with any of
    // them). EnumIds index into resolved_enums. Variant names must be
    // unique within their enum; payload types stay as TypeExpr (the
    // type checker resolves them). The table is built before the impl
    // pass so `impl_ctx` can carry it into expression resolution for
    // the `Enum::Variant(args)` construction disambiguation.
    let mut enum_table: HashMap<String, EnumId> = HashMap::new();
    let mut resolved_enums: Vec<ResolvedEnumDecl> =
        Vec::with_capacity(program.enums.len());
    for (idx, ed) in program.enums.iter().enumerate() {
        if struct_table.contains_key(&ed.name)
            || class_table.contains_key(&ed.name)
            || trait_table.contains_key(&ed.name)
            || enum_table.contains_key(&ed.name)
        {
            return Err(ResolveError::RedefinedEnum {
                name: ed.name.clone(),
                span: to_source_span(&ed.name_span),
            });
        }
        let id = EnumId(idx as u32);
        enum_table.insert(ed.name.clone(), id);
        let mut variant_names: HashSet<String> = HashSet::new();
        let mut variants = Vec::with_capacity(ed.variants.len());
        for v in &ed.variants {
            if !variant_names.insert(v.name.clone()) {
                return Err(ResolveError::DuplicateVariant {
                    enum_name: ed.name.clone(),
                    variant_name: v.name.clone(),
                    span: to_source_span(&v.name_span),
                });
            }
            variants.push(ResolvedVariantDecl {
                name: v.name.clone(),
                name_span: v.name_span.clone(),
                payloads: v.payloads.clone(),
                span: v.span.clone(),
            });
        }
        resolved_enums.push(ResolvedEnumDecl {
            id,
            name: ed.name.clone(),
            name_span: ed.name_span.clone(),
            variants,
            span: ed.span.clone(),
        });
    }

    // C4.2 / ADR 0023 D3/D4/D8 (Pass 0e): collect impl declarations
    // + enforce scope-local coherence. Default impls keyed by
    // (trait_id, target); named impls keyed by name. Bodies are
    // resolved in Pass 4 below.
    let mut impl_named_table: HashMap<String, ImplId> = HashMap::new();
    let mut impl_default_table: HashMap<(TraitId, ImplTarget), ImplId> =
        HashMap::new();
    let mut impl_meta: Vec<(TraitId, ImplTarget)> =
        Vec::with_capacity(program.impls.len());
    for (idx, imp) in program.impls.iter().enumerate() {
        let impl_id = ImplId(idx as u32);
        let trait_id = *trait_table.get(&imp.trait_name).ok_or_else(|| {
            ResolveError::UndefinedTraitForImpl {
                name: imp.trait_name.clone(),
                span: to_source_span(&imp.trait_name_span),
            }
        })?;
        let target = if let Some(cid) = class_table.get(&imp.type_name) {
            ImplTarget::Class(*cid)
        } else if let Some(sid) = struct_table.get(&imp.type_name) {
            ImplTarget::Struct(*sid)
        } else {
            return Err(ResolveError::UndefinedTypeForImpl {
                name: imp.type_name.clone(),
                span: to_source_span(&imp.type_name_span),
            });
        };
        impl_meta.push((trait_id, target));
        match &imp.name {
            None => {
                if impl_default_table.contains_key(&(trait_id, target)) {
                    return Err(ResolveError::DuplicateDefaultImpl {
                        trait_name: imp.trait_name.clone(),
                        type_name: imp.type_name.clone(),
                        span: to_source_span(&imp.span),
                    });
                }
                impl_default_table.insert((trait_id, target), impl_id);
            }
            Some(name) => {
                if impl_named_table.contains_key(name) {
                    return Err(ResolveError::DuplicateImplName {
                        impl_name: name.clone(),
                        trait_name: imp.trait_name.clone(),
                        type_name: imp.type_name.clone(),
                        span: to_source_span(
                            imp.name_span.as_ref().unwrap_or(&imp.span),
                        ),
                    });
                }
                impl_named_table.insert(name.clone(), impl_id);
            }
        }
    }

    // C4.3 / ADR 0021 D6 (Pass 0e extension): register delegate-
    // synthesized impls. Each `delegate field: T to Trait;` inside
    // a class body synthesizes a default impl of Trait for the
    // enclosing class. The coherence check fires uniformly with
    // user-written default impls; the body synthesis happens after
    // user-impl bodies are resolved (Pass 4.5 below). Each
    // delegate gets an ImplId starting right after the user-impl
    // count.
    let mut delegate_meta: Vec<(ImplId, ClassId, usize)> = Vec::new();
    let mut next_impl_id_u32 = program.impls.len() as u32;
    for cd in program.classes.iter() {
        let class_id = *class_table
            .get(&cd.name)
            .expect("registered in Pass 0c");
        for (d_idx, delegate) in cd.delegates.iter().enumerate() {
            let trait_id = *trait_table.get(&delegate.trait_name).ok_or_else(|| {
                ResolveError::UndefinedTraitForImpl {
                    name: delegate.trait_name.clone(),
                    span: to_source_span(&delegate.trait_name_span),
                }
            })?;
            let target = ImplTarget::Class(class_id);
            let impl_id = ImplId(next_impl_id_u32);
            next_impl_id_u32 += 1;
            if impl_default_table.contains_key(&(trait_id, target)) {
                return Err(ResolveError::DuplicateDefaultImpl {
                    trait_name: delegate.trait_name.clone(),
                    type_name: cd.name.clone(),
                    span: to_source_span(&delegate.span),
                });
            }
            impl_default_table.insert((trait_id, target), impl_id);
            impl_meta.push((trait_id, target));
            delegate_meta.push((impl_id, class_id, d_idx));
        }
    }

    // Side table: ImplId → its trait's method names in order.
    // Used by `resolve_expr` to find the method_index for
    // `ImplName::method(...)` qualified calls without re-walking
    // the trait decl. Built from resolved_traits (above) +
    // impl_meta (above).
    let impl_to_trait_methods: HashMap<ImplId, (TraitId, Vec<String>)> =
        impl_meta
            .iter()
            .enumerate()
            .map(|(i, (tid, _))| {
                let names: Vec<String> = resolved_traits[tid.0 as usize]
                    .methods
                    .iter()
                    .map(|m| m.name.clone())
                    .collect();
                (ImplId(i as u32), (*tid, names))
            })
            .collect();

    let impl_ctx = ImplCtx {
        named: &impl_named_table,
        to_trait_methods: &impl_to_trait_methods,
        enum_table: &enum_table,
    };

    // Pre-register the runtime builtins. The runtime (and codegen
    // for C1.5's generic builtins) supplies them; user code can't
    // redefine them (that path errors as RedefinedFunction below).
    let print_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "print".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let unwrap_or_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "unwrap_or".to_string(),
        name_span: None,
        arity: 2,
        // C1.7 / ADR 0016 D8a: `unwrap_or<T>(x: ?T, default: T) -> T`.
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let is_some_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "is_some".to_string(),
        name_span: None,
        arity: 1,
        // C1.7 / ADR 0016 D8a: `is_some<T>(x: ?T) -> bool`.
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let len_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "len".to_string(),
        name_span: None,
        arity: 1,
        // C1.7 / ADR 0016 D8a: `len<T>(a: [T]) -> i64`.
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    // D.2 / ADR 0033 D5: the byte-string builtins. Non-generic
    // (concrete `[u8]`/`u8`/`i64` signatures, typed in sentinel-types);
    // codegen lowers them at D.2 (4/N).
    let str_eq_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "str_eq".to_string(),
        name_span: None,
        arity: 2,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let u8_to_i64_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "u8_to_i64".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let i64_to_u8_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "i64_to_u8".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    // D.3 / ADR 0034 D5: the growable-collection builtins. Generic over
    // the element T (`type_params_count: 1`), typed in sentinel-types
    // via the uniform generic-call inference path (vec_new's T pinned
    // from the binding annotation; push's from the `&mut Vec<T>` arg).
    let vec_new_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "vec_new".to_string(),
        name_span: None,
        arity: 0,
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let push_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "push".to_string(),
        name_span: None,
        arity: 2,
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    // D.3 (2/N) / ADR 0034 D5: pop + the Vec->array bridge. Both generic
    // over the element T; typed via the uniform generic-call path.
    let pop_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "pop".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let vec_to_array_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "vec_to_array".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    // D.4 / ADR 0035 D4: file-I/O builtins. Non-generic (concrete `[u8]`
    // signatures, typed in sentinel-types), like `str_eq`.
    let read_file_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "read_file".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let write_file_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "write_file".to_string(),
        name_span: None,
        arity: 2,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let print_bytes_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "print_bytes".to_string(),
        name_span: None,
        arity: 1,
        type_params_count: 0,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;

    let mut fn_table: HashMap<String, FnId> = HashMap::new();
    let mut signatures: Vec<FnSignature> = vec![
        print_sig, unwrap_or_sig, is_some_sig, len_sig, str_eq_sig, u8_to_i64_sig,
        i64_to_u8_sig, vec_new_sig, push_sig, pop_sig, vec_to_array_sig, read_file_sig,
        write_file_sig, print_bytes_sig,
    ];
    fn_table.insert("print".to_string(), PRINT_FN_ID);
    fn_table.insert("unwrap_or".to_string(), UNWRAP_OR_FN_ID);
    fn_table.insert("is_some".to_string(), IS_SOME_FN_ID);
    fn_table.insert("len".to_string(), LEN_FN_ID);
    fn_table.insert("str_eq".to_string(), STR_EQ_FN_ID);
    fn_table.insert("u8_to_i64".to_string(), U8_TO_I64_FN_ID);
    fn_table.insert("i64_to_u8".to_string(), I64_TO_U8_FN_ID);
    fn_table.insert("vec_new".to_string(), VEC_NEW_FN_ID);
    fn_table.insert("push".to_string(), PUSH_FN_ID);
    fn_table.insert("pop".to_string(), POP_FN_ID);
    fn_table.insert("vec_to_array".to_string(), VEC_TO_ARRAY_FN_ID);
    fn_table.insert("read_file".to_string(), READ_FILE_FN_ID);
    fn_table.insert("write_file".to_string(), WRITE_FILE_FN_ID);
    fn_table.insert("print_bytes".to_string(), PRINT_BYTES_FN_ID);

    // Pass 1: collect every fn into the table.
    for fn_def in &program.fns {
        if fn_table.contains_key(&fn_def.name) {
            return Err(ResolveError::RedefinedFunction {
                name: fn_def.name.clone(),
                span: to_source_span(&fn_def.name_span),
            });
        }
        let id = FnId(next_fn_id);
        next_fn_id += 1;
        let is_main = fn_def.name == "main";
        signatures.push(FnSignature {
            id,
            name: fn_def.name.clone(),
            name_span: Some(fn_def.name_span.clone()),
            arity: fn_def.params.len(),
            type_params_count: fn_def.type_params.len(),
            is_main,
            is_runtime: false,
        });
        fn_table.insert(fn_def.name.clone(), id);
    }

    if !fn_table.contains_key("main") {
        return Err(ResolveError::MissingMain);
    }

    // Pass 2: resolve each fn body.
    let mut resolved_fns = Vec::with_capacity(program.fns.len());
    for fn_def in &program.fns {
        resolved_fns.push(resolve_fn(
            fn_def,
            &fn_table,
            &signatures,
            &struct_table,
            &class_table,
            &effect_table,
            &resolved_effects,
            impl_ctx,
            &mut next_var_id,
        )?);
    }

    // Pass 3: resolve each class body (init + methods). Mirrors
    // Pass 2 but threads a synthetic `self` binding into the
    // body's scope per ADR 0022 D8. Per-class field name
    // uniqueness is enforced here.
    let mut resolved_classes = Vec::with_capacity(program.classes.len());
    for cd in program.classes.iter() {
        let class_id = *class_table
            .get(&cd.name)
            .expect("registered in Pass 0c");
        resolved_classes.push(resolve_class_decl(
            cd,
            class_id,
            &fn_table,
            &signatures,
            &struct_table,
            &class_table,
            &effect_table,
            &resolved_effects,
            impl_ctx,
            &mut next_var_id,
        )?);
    }

    // C4.2 / ADR 0023 D3 (Pass 4): resolve each impl block's
    // method bodies. Mirrors Pass 3's per-method handling: bind a
    // synthetic `self` VarId, bind params, resolve the body block.
    let mut resolved_impls: Vec<ResolvedImplDecl> =
        Vec::with_capacity(program.impls.len());
    for (idx, imp) in program.impls.iter().enumerate() {
        let impl_id = ImplId(idx as u32);
        let (trait_id, target) = impl_meta[idx];
        let mut method_names: HashSet<String> = HashSet::new();
        let mut methods: Vec<ResolvedImplMethodDef> =
            Vec::with_capacity(imp.methods.len());
        for m in &imp.methods {
            if !method_names.insert(m.name.clone()) {
                // Two methods with the same name inside one impl
                // block: reuse DuplicateClassMethod for parity. A
                // dedicated DuplicateImplMethod variant could
                // replace this in a future polish iteration.
                return Err(ResolveError::DuplicateClassMethod {
                    class_name: format!(
                        "{}{} as {} for {}",
                        imp.name.as_ref().map(|n| format!("{n} ")).unwrap_or_default(),
                        "",
                        imp.trait_name,
                        imp.type_name,
                    ),
                    method_name: m.name.clone(),
                    span: to_source_span(&m.name_span),
                });
            }
            let mut vars: HashMap<String, VarId> = HashMap::new();
            let self_var_id = VarId(next_var_id);
            next_var_id += 1;
            vars.insert("self".to_string(), self_var_id);

            let mut m_params = Vec::with_capacity(m.params.len());
            for p in &m.params {
                if vars.contains_key(&p.name) {
                    return Err(ResolveError::RedeclaredVariable {
                        name: p.name.clone(),
                        span: to_source_span(&p.span),
                    });
                }
                let vid = VarId(next_var_id);
                next_var_id += 1;
                vars.insert(p.name.clone(), vid);
                m_params.push(ResolvedParam {
                    id: vid,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.clone(),
                });
            }
            let mut method_effect_row = Vec::with_capacity(m.effect_row.len());
            for entry in &m.effect_row {
                let eid = effect_table.get(&entry.kind).ok_or_else(|| {
                    ResolveError::UndefinedEffect {
                        name: entry.kind.clone(),
                        fn_name: format!(
                            "impl {} for {} :: {}",
                            imp.trait_name, imp.type_name, m.name
                        ),
                        span: to_source_span(&entry.span),
                    }
                })?;
                method_effect_row.push(*eid);
            }
            let body = resolve_block(
                &m.body,
                &fn_table,
                &signatures,
                &struct_table,
                &class_table,
                &effect_table,
                &resolved_effects,
                impl_ctx,
                &mut vars,
                &mut next_var_id,
            )?;
            methods.push(ResolvedImplMethodDef {
                visibility: m.visibility,
                name: m.name.clone(),
                name_span: m.name_span.clone(),
                self_kind: m.self_kind,
                self_var_id,
                params: m_params,
                return_type: m.return_type.clone(),
                effect_row: method_effect_row,
                body,
                span: m.span.clone(),
            });
        }
        resolved_impls.push(ResolvedImplDecl {
            id: impl_id,
            name: imp.name.clone(),
            name_span: imp.name_span.clone(),
            trait_id,
            trait_name: imp.trait_name.clone(),
            trait_name_span: imp.trait_name_span.clone(),
            target,
            type_name: imp.type_name.clone(),
            type_name_span: imp.type_name_span.clone(),
            methods,
            span: imp.span.clone(),
        });
    }

    // C4.3 / ADR 0021 D6 (Pass 4.5): synthesize delegate impl
    // bodies. For each registered delegate, build a
    // ResolvedImplDecl whose methods auto-forward to
    // `self.field.method(args)` — one per trait method. The
    // delegate-impl IDs were allocated in Pass 0e (extension);
    // here we materialise the actual methods + bodies.
    for (impl_id, class_id, d_idx) in &delegate_meta {
        let cd = program
            .classes
            .iter()
            .find(|c| {
                class_table.get(&c.name).copied() == Some(*class_id)
            })
            .expect("class registered in Pass 0c");
        let delegate = &cd.delegates[*d_idx];
        let trait_id = *trait_table
            .get(&delegate.trait_name)
            .expect("validated in Pass 0e");
        let trait_decl = &resolved_traits[trait_id.0 as usize];
        let mut methods: Vec<ResolvedImplMethodDef> =
            Vec::with_capacity(trait_decl.methods.len());
        for tm in &trait_decl.methods {
            let self_var_id = VarId(next_var_id);
            next_var_id += 1;
            let mut params: Vec<ResolvedParam> =
                Vec::with_capacity(tm.params.len());
            for p in &tm.params {
                let pid = VarId(next_var_id);
                next_var_id += 1;
                params.push(ResolvedParam {
                    id: pid,
                    mutable: p.mutable,
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.clone(),
                });
            }
            // Synthesize body: `self.field.method(p1, p2, ...)`.
            let self_ref = Spanned {
                kind: ResolvedExprKind::Var(self_var_id),
                span: delegate.span.clone(),
            };
            let field_access = Spanned {
                kind: ResolvedExprKind::FieldAccess {
                    target: Box::new(self_ref),
                    field: delegate.field_name.clone(),
                    field_span: delegate.field_name_span.clone(),
                },
                span: delegate.span.clone(),
            };
            let args: Vec<ResolvedExpr> = params
                .iter()
                .map(|p| Spanned {
                    kind: ResolvedExprKind::Var(p.id),
                    span: delegate.span.clone(),
                })
                .collect();
            let method_call = Spanned {
                kind: ResolvedExprKind::MethodCall {
                    target: Box::new(field_access),
                    method: tm.name.clone(),
                    method_span: tm.name_span.clone(),
                    args,
                },
                span: delegate.span.clone(),
            };
            let body = ResolvedBlock {
                stmts: Vec::new(),
                tail: method_call,
                span: delegate.span.clone(),
            };
            methods.push(ResolvedImplMethodDef {
                visibility: delegate.visibility,
                name: tm.name.clone(),
                name_span: tm.name_span.clone(),
                self_kind: tm.self_kind,
                self_var_id,
                params,
                return_type: tm.return_type.clone(),
                effect_row: tm.effect_row.clone(),
                body,
                span: delegate.span.clone(),
            });
        }
        resolved_impls.push(ResolvedImplDecl {
            id: *impl_id,
            name: None,
            name_span: None,
            trait_id,
            trait_name: delegate.trait_name.clone(),
            trait_name_span: delegate.trait_name_span.clone(),
            target: ImplTarget::Class(*class_id),
            type_name: cd.name.clone(),
            type_name_span: cd.name_span.clone(),
            methods,
            span: delegate.span.clone(),
        });
    }

    Ok(ResolvedProgram {
        fns: resolved_fns,
        fn_signatures: signatures,
        structs: resolved_structs,
        effects: resolved_effects,
        classes: resolved_classes,
        traits: resolved_traits,
        impls: resolved_impls,
        enums: resolved_enums,
        span: program.span.clone(),
    })
}

/// C4.2 / ADR 0023 D6: lookup tables for impl-name → impl + impl
/// → (trait_id, method-names-in-order). Threaded through
/// expression resolution so `ImplName::method(args)` qualified
/// calls can be routed to the right impl + method index. Borrowed
/// from the top-level `resolve()` state to avoid clone overhead.
#[derive(Clone, Copy)]
struct ImplCtx<'a> {
    named: &'a HashMap<String, ImplId>,
    to_trait_methods: &'a HashMap<ImplId, (TraitId, Vec<String>)>,
    /// Phase D.1 / ADR 0032 (3/N): enum name → [`EnumId`] table,
    /// threaded alongside the impl tables so `Enum::Variant(args)`
    /// construction disambiguates from `ImplName::method(args)` /
    /// `Class::init(args)` during expression resolution. (Despite the
    /// `ImplCtx` name, this bundle is the general name-resolution
    /// context carried into `resolve_expr`.)
    enum_table: &'a HashMap<String, EnumId>,
}

#[allow(clippy::too_many_arguments)]
fn resolve_fn(
    fn_def: &FnDef,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    next_var_id: &mut u32,
) -> Result<ResolvedFnDef, ResolveError> {
    let id = *fn_table
        .get(&fn_def.name)
        .expect("registered in pass 1");

    // C1.7 / ADR 0016 D9: assign TypeParamIds + reject duplicates.
    let type_params = resolve_type_params(&fn_def.type_params)?;

    // C3 / ADR 0019 D1 (C3.2): resolve the postfix effect-row
    // annotation (each name → EffectId). Empty for fns without
    // an annotation. UndefinedEffect on a typo'd name.
    let mut effect_row = Vec::with_capacity(fn_def.effect_row.len());
    for entry in &fn_def.effect_row {
        let eid = effect_table.get(&entry.kind).ok_or_else(|| {
            ResolveError::UndefinedEffect {
                name: entry.kind.clone(),
                fn_name: fn_def.name.clone(),
                span: to_source_span(&entry.span),
            }
        })?;
        effect_row.push(*eid);
    }

    let mut vars: HashMap<String, VarId> = HashMap::new();
    let mut params = Vec::with_capacity(fn_def.params.len());
    for param in &fn_def.params {
        if vars.contains_key(&param.name) {
            return Err(ResolveError::RedeclaredVariable {
                name: param.name.clone(),
                span: to_source_span(&param.span),
            });
        }
        let var_id = VarId(*next_var_id);
        *next_var_id += 1;
        vars.insert(param.name.clone(), var_id);
        params.push(ResolvedParam {
            id: var_id,
            mutable: param.mutable,
            name: param.name.clone(),
            span: param.span.clone(),
            ty: param.ty.clone(),
        });
    }

    let body = resolve_block(
        &fn_def.body,
        fn_table,
        signatures,
        struct_table,
        class_table,
        effect_table,
        effects,
        impls,
        &mut vars,
        next_var_id,
    )?;

    Ok(ResolvedFnDef {
        id,
        name: fn_def.name.clone(),
        name_span: fn_def.name_span.clone(),
        type_params,
        params,
        return_type: fn_def.return_type.clone(),
        effect_row,
        body,
        span: fn_def.span.clone(),
    })
}

/// C4.1 / ADR 0022 D1: resolve a class declaration end-to-end —
/// the field list (rejecting duplicate field names), the optional
/// init body (binding the synthetic `self` VarId), and each
/// method body (binding `self` per its self_kind).
#[allow(clippy::too_many_arguments)]
fn resolve_class_decl(
    cd: &ClassDecl,
    class_id: ClassId,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    next_var_id: &mut u32,
) -> Result<ResolvedClassDecl, ResolveError> {
    // Field list: name uniqueness + type-expr carry-through.
    // C4.3 / ADR 0021 D6: delegations contribute synthesized
    // fields here too — `delegate field: T to Trait;` produces a
    // regular `field: T` slot alongside the auto-forwarder impl.
    // Collisions between explicit fields + delegate fields fire
    // the standard DuplicateClassField diagnostic.
    let mut field_names: HashSet<String> = HashSet::new();
    let mut fields: Vec<ResolvedClassField> =
        Vec::with_capacity(cd.fields.len() + cd.delegates.len());
    for f in &cd.fields {
        if !field_names.insert(f.name.clone()) {
            return Err(ResolveError::DuplicateClassField {
                class_name: cd.name.clone(),
                field_name: f.name.clone(),
                span: to_source_span(&f.name_span),
            });
        }
        fields.push(ResolvedClassField {
            visibility: f.visibility,
            name: f.name.clone(),
            name_span: f.name_span.clone(),
            ty: f.ty.clone(),
            span: f.span.clone(),
        });
    }
    for delegate in &cd.delegates {
        if !field_names.insert(delegate.field_name.clone()) {
            return Err(ResolveError::DuplicateClassField {
                class_name: cd.name.clone(),
                field_name: delegate.field_name.clone(),
                span: to_source_span(&delegate.field_name_span),
            });
        }
        fields.push(ResolvedClassField {
            visibility: delegate.visibility,
            name: delegate.field_name.clone(),
            name_span: delegate.field_name_span.clone(),
            ty: delegate.ty.clone(),
            span: delegate.span.clone(),
        });
    }

    // Init body: bind `self` + params, resolve the trailing block.
    let init = if let Some(init_def) = &cd.init {
        let mut vars: HashMap<String, VarId> = HashMap::new();
        let self_var_id = VarId(*next_var_id);
        *next_var_id += 1;
        vars.insert("self".to_string(), self_var_id);

        let mut init_params = Vec::with_capacity(init_def.params.len());
        for p in &init_def.params {
            if vars.contains_key(&p.name) {
                return Err(ResolveError::RedeclaredVariable {
                    name: p.name.clone(),
                    span: to_source_span(&p.span),
                });
            }
            let vid = VarId(*next_var_id);
            *next_var_id += 1;
            vars.insert(p.name.clone(), vid);
            init_params.push(ResolvedParam {
                id: vid,
                mutable: p.mutable,
                name: p.name.clone(),
                span: p.span.clone(),
                ty: p.ty.clone(),
            });
        }
        let body = resolve_block(
            &init_def.body,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            &mut vars,
            next_var_id,
        )?;
        Some(ResolvedInitDef {
            visibility: init_def.visibility,
            self_var_id,
            params: init_params,
            body,
            span: init_def.span.clone(),
        })
    } else {
        None
    };

    // Methods: name uniqueness within the class + per-method body
    // resolution with synthetic `self`.
    let mut method_names: HashSet<String> = HashSet::new();
    let mut methods: Vec<ResolvedMethodDef> = Vec::with_capacity(cd.methods.len());
    for m in &cd.methods {
        if !method_names.insert(m.name.clone()) {
            return Err(ResolveError::DuplicateClassMethod {
                class_name: cd.name.clone(),
                method_name: m.name.clone(),
                span: to_source_span(&m.name_span),
            });
        }
        let mut vars: HashMap<String, VarId> = HashMap::new();
        let self_var_id = VarId(*next_var_id);
        *next_var_id += 1;
        vars.insert("self".to_string(), self_var_id);

        let mut m_params = Vec::with_capacity(m.params.len());
        for p in &m.params {
            if vars.contains_key(&p.name) {
                return Err(ResolveError::RedeclaredVariable {
                    name: p.name.clone(),
                    span: to_source_span(&p.span),
                });
            }
            let vid = VarId(*next_var_id);
            *next_var_id += 1;
            vars.insert(p.name.clone(), vid);
            m_params.push(ResolvedParam {
                id: vid,
                mutable: p.mutable,
                name: p.name.clone(),
                span: p.span.clone(),
                ty: p.ty.clone(),
            });
        }

        // Resolve the effect-row annotation, mirroring resolve_fn.
        let mut method_effect_row = Vec::with_capacity(m.effect_row.len());
        for entry in &m.effect_row {
            let eid = effect_table.get(&entry.kind).ok_or_else(|| {
                ResolveError::UndefinedEffect {
                    name: entry.kind.clone(),
                    fn_name: format!("{}::{}", cd.name, m.name),
                    span: to_source_span(&entry.span),
                }
            })?;
            method_effect_row.push(*eid);
        }

        let body = resolve_block(
            &m.body,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            &mut vars,
            next_var_id,
        )?;

        methods.push(ResolvedMethodDef {
            visibility: m.visibility,
            name: m.name.clone(),
            name_span: m.name_span.clone(),
            self_kind: m.self_kind,
            self_var_id,
            params: m_params,
            return_type: m.return_type.clone(),
            effect_row: method_effect_row,
            body,
            span: m.span.clone(),
        });
    }

    Ok(ResolvedClassDecl {
        id: class_id,
        name: cd.name.clone(),
        name_span: cd.name_span.clone(),
        fields,
        init,
        methods,
        span: cd.span.clone(),
    })
}

/// Walk an AST type-parameter list and assign [`TypeParamId`]s in
/// source order per ADR 0016 D6c / D9. Rejects duplicate names with
/// [`ResolveError::DuplicateTypeParam`].
fn resolve_type_params(
    src: &[AstTypeParam],
) -> Result<Vec<ResolvedTypeParam>, ResolveError> {
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut out: Vec<ResolvedTypeParam> = Vec::with_capacity(src.len());
    for (idx, tp) in src.iter().enumerate() {
        if seen.insert(tp.name.clone(), ()).is_some() {
            return Err(ResolveError::DuplicateTypeParam {
                name: tp.name.clone(),
                span: to_source_span(&tp.name_span),
            });
        }
        out.push(ResolvedTypeParam {
            id: TypeParamId(idx as u32),
            name: tp.name.clone(),
            name_span: tp.name_span.clone(),
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn resolve_block(
    block: &Block,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedBlock, ResolveError> {
    let mut stmts = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        stmts.push(resolve_stmt(
            stmt,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?);
    }
    let tail = resolve_expr(
        &block.tail,
        fn_table,
        signatures,
        struct_table,
        class_table,
        effect_table,
        effects,
        impls,
        vars,
        next_var_id,
    )?;
    Ok(ResolvedBlock {
        stmts,
        tail,
        span: block.span.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_stmt(
    stmt: &Stmt,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedStmt, ResolveError> {
    let kind = match &stmt.kind {
        StmtKind::Let { mutable, name, name_span, ty_annot, value } => {
            // Resolve the RHS BEFORE binding the name — `let x = x` with
            // `x` undefined in the outer scope is an error, not a self-
            // reference. (Matches C0's existing behaviour: lower_expr on
            // the RHS happens before vars.insert(name).)
            let value = resolve_expr(
                value,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            if vars.contains_key(name) {
                return Err(ResolveError::RedeclaredVariable {
                    name: name.clone(),
                    span: to_source_span(name_span),
                });
            }
            let id = VarId(*next_var_id);
            *next_var_id += 1;
            vars.insert(name.clone(), id);
            ResolvedStmtKind::Let {
                id,
                mutable: *mutable,
                name: name.clone(),
                name_span: name_span.clone(),
                ty_annot: ty_annot.clone(),
                value,
            }
        }
        StmtKind::Assign { target, value } => {
            let target = resolve_expr(
                target,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let value = resolve_expr(
                value,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedStmtKind::Assign { target, value }
        }
        StmtKind::While { cond, body } => {
            // Phase D.5 / ADR 0036 D3: resolve the condition in the outer
            // scope, then the body in its own scope (snapshot/restore
            // `vars` so per-iteration bindings don't leak past the loop —
            // the match-arm scoping pattern).
            let cond = resolve_expr(
                cond,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let saved_vars = vars.clone();
            let body = resolve_block(
                body,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            *vars = saved_vars;
            ResolvedStmtKind::While { cond, body: Box::new(body) }
        }
        // Phase D.5 (2/N) / ADR 0036 D9: payload-free loop control —
        // nothing to resolve. The type checker rejects them outside a
        // loop (D7); the borrow checker treats them as no-ops.
        StmtKind::Break => ResolvedStmtKind::Break,
        StmtKind::Continue => ResolvedStmtKind::Continue,
        StmtKind::Expr(e) => ResolvedStmtKind::Expr(resolve_expr(
            e,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?),
    };
    Ok(Spanned { kind, span: stmt.span.clone() })
}

#[allow(clippy::too_many_arguments)]
fn resolve_expr(
    expr: &Expr,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExpr, ResolveError> {
    let kind = match &expr.kind {
        ExprKind::IntLit(n) => ResolvedExprKind::IntLit(*n),
        ExprKind::BoolLit(b) => ResolvedExprKind::BoolLit(*b),
        ExprKind::NullLit => ResolvedExprKind::NullLit,
        // D.2 / ADR 0033 (3/N): char/string literals now flow through —
        // the bytes carry verbatim from the AST; the type checker assigns
        // char → `u8`, string → `[u8]`.
        ExprKind::CharLit(b) => ResolvedExprKind::CharLit(*b),
        ExprKind::StringLit(bytes) => ResolvedExprKind::StringLit(bytes.clone()),
        ExprKind::Var(name) => {
            let id = match vars.get(name) {
                Some(id) => *id,
                None if name == "self" => {
                    // C4.1 / ADR 0022 D8: `self` is only valid
                    // inside class methods / init bodies. Per the
                    // resolve pass's class context, `self` is
                    // pre-bound there; outside, it surfaces as a
                    // dedicated diagnostic instead of the generic
                    // UndefinedVariable.
                    return Err(ResolveError::SelfOutsideClassContext {
                        span: to_source_span(&expr.span),
                    });
                }
                None => {
                    return Err(ResolveError::UndefinedVariable {
                        name: name.clone(),
                        span: to_source_span(&expr.span),
                    });
                }
            };
            ResolvedExprKind::Var(id)
        }
        ExprKind::Unary(op, inner) => ResolvedExprKind::Unary(
            *op,
            Box::new(resolve_expr(
                inner,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?),
        ),
        ExprKind::Binary(op, lhs, rhs) => {
            let l = resolve_expr(
                lhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let r = resolve_expr(
                rhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Binary(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Cmp(op, lhs, rhs) => {
            let l = resolve_expr(
                lhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let r = resolve_expr(
                rhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Cmp(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Logic(op, lhs, rhs) => {
            let l = resolve_expr(
                lhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let r = resolve_expr(
                rhs,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Logic(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Block(b) => ResolvedExprKind::Block(Box::new(resolve_block(
            b,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?)),
        ExprKind::If { cond, then_branch, else_branch } => {
            let cond = resolve_expr(
                cond,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let then_b = resolve_block(
                then_branch,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let else_b = resolve_block(
                else_branch,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_b),
                else_branch: Box::new(else_b),
            }
        }
        ExprKind::Call { callee, callee_span, args } => {
            // C3.4 / ADR 0020 D5: a Call whose callee name resolves
            // to an in-scope VarId is a continuation-resume call
            // (`k(arg)` inside a handler arm). Vars win over fns —
            // a let-bound `k` shadows any top-level `fn k`, matching
            // the standard inner-scope-wins rule. If no var match,
            // fall through to the existing fn_table lookup.
            if let Some(kid) = vars.get(callee).copied() {
                let mut resolved_args = Vec::with_capacity(args.len());
                for arg in args {
                    resolved_args.push(resolve_expr(
                        arg,
                        fn_table,
                        signatures,
                        struct_table,
                        class_table,
                        effect_table,
                        effects,
                        impls,
                        vars,
                        next_var_id,
                    )?);
                }
                ResolvedExprKind::ResumeKont {
                    kont: kid,
                    callee_span: callee_span.clone(),
                    args: resolved_args,
                }
            } else {
                let id =
                    *fn_table
                        .get(callee)
                        .ok_or_else(|| ResolveError::UndefinedFunction {
                            name: callee.clone(),
                            span: to_source_span(callee_span),
                        })?;
                let signature = &signatures[id.0 as usize];
                if args.len() != signature.arity {
                    return Err(ResolveError::ArityMismatch {
                        name: callee.clone(),
                        expected: signature.arity,
                        got: args.len(),
                        span: to_source_span(callee_span),
                    });
                }
                let mut resolved_args = Vec::with_capacity(args.len());
                for arg in args {
                    resolved_args.push(resolve_expr(
                        arg,
                        fn_table,
                        signatures,
                        struct_table,
                        class_table,
                        effect_table,
                        effects,
                        impls,
                        vars,
                        next_var_id,
                    )?);
                }
                ResolvedExprKind::Call {
                    id,
                    callee_span: callee_span.clone(),
                    args: resolved_args,
                }
            }
        }
        ExprKind::StructLit { name, name_span, fields } => {
            let id =
                *struct_table
                    .get(name)
                    .ok_or_else(|| ResolveError::UndefinedStruct {
                        name: name.clone(),
                        span: to_source_span(name_span),
                    })?;
            let mut resolved_fields = Vec::with_capacity(fields.len());
            for fi in fields {
                let value = resolve_expr(
                    &fi.value,
                    fn_table,
                    signatures,
                    struct_table,
                    class_table,
                    effect_table,
                    effects,
                    impls,
                    vars,
                    next_var_id,
                )?;
                resolved_fields.push(ResolvedFieldInit {
                    name: fi.name.clone(),
                    name_span: fi.name_span.clone(),
                    value,
                    span: fi.span.clone(),
                });
            }
            ResolvedExprKind::StructLit {
                id,
                name: name.clone(),
                name_span: name_span.clone(),
                fields: resolved_fields,
            }
        }
        ExprKind::FieldAccess { target, field, field_span } => {
            let target = resolve_expr(
                target,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::FieldAccess {
                target: Box::new(target),
                field: field.clone(),
                field_span: field_span.clone(),
            }
        }
        ExprKind::ArrayLit(elems) => {
            let mut resolved_elems = Vec::with_capacity(elems.len());
            for e in elems {
                resolved_elems.push(resolve_expr(
                    e,
                    fn_table,
                    signatures,
                    struct_table,
                    class_table,
                    effect_table,
                    effects,
                    impls,
                    vars,
                    next_var_id,
                )?);
            }
            ResolvedExprKind::ArrayLit(resolved_elems)
        }
        ExprKind::Index { target, index } => {
            let target = resolve_expr(
                target,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let index = resolve_expr(
                index,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Index {
                target: Box::new(target),
                index: Box::new(index),
            }
        }
        ExprKind::Declassify(inner) => {
            // C3 / ADR 0019 D6 (C3.1): mirror the AST variant into
            // the resolved tree; type-check validates the inner is
            // `secret T` and strips the wrapper.
            let inner = resolve_expr(
                inner,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Declassify(Box::new(inner))
        }
        ExprKind::Handle { body, arms, return_arm } => resolve_handle_expr(
            body,
            arms,
            return_arm.as_deref(),
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?,
        ExprKind::Perform { effect, op, args } => resolve_perform_expr(
            effect,
            op,
            args,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?,
        ExprKind::MethodCall { target, method, method_span, args } => {
            // C4.1 / ADR 0022 D3 + D7: resolve target + args; the
            // method name stays a string. The typing layer looks
            // it up against the receiver's class.
            let target_r = resolve_expr(
                target,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            let mut resolved_args = Vec::with_capacity(args.len());
            for a in args {
                resolved_args.push(resolve_expr(
                    a,
                    fn_table,
                    signatures,
                    struct_table,
                    class_table,
                    effect_table,
                    effects,
                    impls,
                    vars,
                    next_var_id,
                )?);
            }
            ResolvedExprKind::MethodCall {
                target: Box::new(target_r),
                method: method.clone(),
                method_span: method_span.clone(),
                args: resolved_args,
            }
        }
        ExprKind::ClassInit { class_name, class_name_span, args } => {
            // Phase D.1 / ADR 0032 (3/N): an enum with a variant
            // literally named `init` — `Enum::init(args)` parses as
            // ClassInit, but is variant construction. Disambiguate
            // before the class lookup (enums share the name table).
            if let Some(&enum_id) = impls.enum_table.get(class_name) {
                let mut resolved_args = Vec::with_capacity(args.len());
                for a in args {
                    resolved_args.push(resolve_expr(
                        a, fn_table, signatures, struct_table, class_table,
                        effect_table, effects, impls, vars, next_var_id,
                    )?);
                }
                return Ok(Spanned {
                    kind: ResolvedExprKind::EnumConstruct {
                        enum_id,
                        enum_name: class_name.clone(),
                        variant_name: "init".to_string(),
                        variant_span: class_name_span.clone(),
                        args: resolved_args,
                    },
                    span: expr.span.clone(),
                });
            }
            // C4.1 / ADR 0022 D5: `Name::init(args)` — look up
            // the class. Arity / param type checks are the typing
            // layer's responsibility.
            let id =
                *class_table
                    .get(class_name)
                    .ok_or_else(|| ResolveError::UndefinedClass {
                        name: class_name.clone(),
                        span: to_source_span(class_name_span),
                    })?;
            let mut resolved_args = Vec::with_capacity(args.len());
            for a in args {
                resolved_args.push(resolve_expr(
                    a,
                    fn_table,
                    signatures,
                    struct_table,
                    class_table,
                    effect_table,
                    effects,
                    impls,
                    vars,
                    next_var_id,
                )?);
            }
            ResolvedExprKind::ClassInit {
                id,
                name: class_name.clone(),
                name_span: class_name_span.clone(),
                args: resolved_args,
            }
        }
        ExprKind::QualifiedCall {
            impl_name,
            impl_name_span,
            method,
            method_span,
            args,
        } => {
            // Phase D.1 / ADR 0032 (3/N): if the leading name is an
            // enum, `Enum::Variant(args)` is variant construction, not
            // an impl-method qualified call. The type checker
            // validates the variant name + payload arity/types.
            if let Some(&enum_id) = impls.enum_table.get(impl_name) {
                let mut resolved_args = Vec::with_capacity(args.len());
                for a in args {
                    resolved_args.push(resolve_expr(
                        a, fn_table, signatures, struct_table, class_table,
                        effect_table, effects, impls, vars, next_var_id,
                    )?);
                }
                return Ok(Spanned {
                    kind: ResolvedExprKind::EnumConstruct {
                        enum_id,
                        enum_name: impl_name.clone(),
                        variant_name: method.clone(),
                        variant_span: method_span.clone(),
                        args: resolved_args,
                    },
                    span: expr.span.clone(),
                });
            }
            // C4.2 (2/N) / ADR 0023 D6 Path 2: route through the
            // impl table. Find the named impl, then find the
            // method's index on its trait's method list.
            let impl_id = *impls.named.get(impl_name).ok_or_else(|| {
                ResolveError::UndefinedImpl {
                    name: impl_name.clone(),
                    method: method.clone(),
                    span: to_source_span(impl_name_span),
                }
            })?;
            let (trait_id, method_names) = impls
                .to_trait_methods
                .get(&impl_id)
                .expect("ImplId populated alongside trait method list");
            let method_index = method_names
                .iter()
                .position(|n| n == method)
                .ok_or_else(|| {
                    let trait_name = format!("trait#{}", trait_id.0);
                    ResolveError::UndefinedTraitMethod {
                        impl_name: impl_name.clone(),
                        trait_name,
                        method_name: method.clone(),
                        span: to_source_span(method_span),
                    }
                })?;
            let mut resolved_args = Vec::with_capacity(args.len());
            for a in args {
                resolved_args.push(resolve_expr(
                    a,
                    fn_table,
                    signatures,
                    struct_table,
                    class_table,
                    effect_table,
                    effects,
                    impls,
                    vars,
                    next_var_id,
                )?);
            }
            ResolvedExprKind::QualifiedCall {
                impl_id,
                method_index,
                impl_name: impl_name.clone(),
                impl_name_span: impl_name_span.clone(),
                method: method.clone(),
                method_span: method_span.clone(),
                args: resolved_args,
            }
        }
        ExprKind::Scope { mode, body } => {
            // C4.4 (2/N) / ADR 0024 D1: pass-through. Scope is a
            // plain block at resolve time (flat var scoping like
            // ExprKind::Block); the typing + runtime contract lands
            // in types + codegen.
            let resolved_body = resolve_block(
                body,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Scope {
                mode: *mode,
                body: Box::new(resolved_body),
            }
        }
        ExprKind::Spawn { call_expr } => {
            // C4.4 (2/N) / ADR 0024 D2: pass-through. The
            // call-shape restriction is validated at the types
            // layer (SpawnMustBeCall).
            let resolved_call = resolve_expr(
                call_expr,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Spawn {
                call_expr: Box::new(resolved_call),
            }
        }
        ExprKind::Await { task_expr } => {
            // C4.4 (2/N) / ADR 0024 D3: pass-through. The
            // Task<T>-receiver check lands at the types layer
            // (AwaitOnNonTask).
            let resolved_task = resolve_expr(
                task_expr,
                fn_table,
                signatures,
                struct_table,
                class_table,
                effect_table,
                effects,
                impls,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::Await {
                task_expr: Box::new(resolved_task),
            }
        }
        ExprKind::Match { scrutinee, arms } => resolve_match_expr(
            scrutinee,
            arms,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?,
    };
    Ok(Spanned { kind, span: expr.span.clone() })
}

/// Phase D.1 / ADR 0032 (3/N): resolve `match scrutinee { arms }`.
/// The scrutinee resolves in the enclosing scope. Each arm's pattern
/// bindings get fresh [`VarId`]s scoped into that arm's body only
/// (snapshot/restore of `vars`, mirroring handler-arm params in
/// [`resolve_handle_expr`]); `_`-named binding slots still get a
/// VarId (for positional codegen) but are not inserted into scope.
/// Enum + variant *names* stay as strings — the type checker resolves
/// them against the scrutinee's enum and enforces exhaustiveness.
#[allow(clippy::too_many_arguments)]
fn resolve_match_expr(
    scrutinee: &Expr,
    arms: &[MatchArm],
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExprKind, ResolveError> {
    let scrutinee_r = resolve_expr(
        scrutinee, fn_table, signatures, struct_table, class_table,
        effect_table, effects, impls, vars, next_var_id,
    )?;

    let mut resolved_arms = Vec::with_capacity(arms.len());
    for arm in arms {
        match &arm.pattern {
            Pattern::Variant {
                enum_name,
                enum_name_span,
                variant,
                variant_span,
                bindings,
                span: pat_span,
            } => {
                // Bind the pattern's payload slots in a child scope so
                // they don't leak past this arm's body. Each slot gets
                // a VarId; `_` is given a (throwaway) VarId but is not
                // inserted into `vars` (so it cannot be referenced).
                let saved_vars = vars.clone();
                let mut seen: HashSet<String> = HashSet::new();
                let mut resolved_bindings = Vec::with_capacity(bindings.len());
                for b in bindings {
                    let vid = VarId(*next_var_id);
                    *next_var_id += 1;
                    if b.kind != "_" {
                        if !seen.insert(b.kind.clone()) {
                            return Err(ResolveError::DuplicatePatternBinding {
                                name: b.kind.clone(),
                                span: to_source_span(&b.span),
                            });
                        }
                        vars.insert(b.kind.clone(), vid);
                    }
                    resolved_bindings.push(ResolvedPatternBinding {
                        var_id: vid,
                        name: b.kind.clone(),
                        span: b.span.clone(),
                    });
                }
                let body = resolve_expr(
                    &arm.body, fn_table, signatures, struct_table, class_table,
                    effect_table, effects, impls, vars, next_var_id,
                )?;
                *vars = saved_vars;
                resolved_arms.push(ResolvedMatchArm {
                    pattern: ResolvedPattern::Variant {
                        enum_name: enum_name.clone(),
                        enum_name_span: enum_name_span.clone(),
                        variant_name: variant.clone(),
                        variant_span: variant_span.clone(),
                        bindings: resolved_bindings,
                        span: pat_span.clone(),
                    },
                    body,
                    span: arm.span.clone(),
                });
            }
            Pattern::Wildcard(wspan) => {
                // No bindings — the body resolves in the enclosing
                // scope unchanged.
                let body = resolve_expr(
                    &arm.body, fn_table, signatures, struct_table, class_table,
                    effect_table, effects, impls, vars, next_var_id,
                )?;
                resolved_arms.push(ResolvedMatchArm {
                    pattern: ResolvedPattern::Wildcard(wspan.clone()),
                    body,
                    span: arm.span.clone(),
                });
            }
        }
    }

    Ok(ResolvedExprKind::Match {
        scrutinee: Box::new(scrutinee_r),
        arms: resolved_arms,
    })
}

/// C3.4 / ADR 0020 D5: resolve `handle body with { arms }`.
/// Looks up each arm's effect+op pair against the effect_table,
/// rejects duplicate (effect, op) pairs across arms, and assigns
/// VarIds to every arm param (including the trailing kont).
#[allow(clippy::too_many_arguments)]
fn resolve_handle_expr(
    body: &Expr,
    arms: &[HandlerArm],
    return_arm: Option<&ReturnArm>,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExprKind, ResolveError> {
    let body = resolve_expr(
        body,
        fn_table,
        signatures,
        struct_table,
        class_table,
        effect_table,
        effects,
        impls,
        vars,
        next_var_id,
    )?;

    let mut seen: HashSet<(EffectId, usize)> = HashSet::new();
    let mut resolved_arms = Vec::with_capacity(arms.len());
    for arm in arms {
        // Look up effect by name.
        let effect_id = *effect_table.get(&arm.effect.kind).ok_or_else(|| {
            ResolveError::UndefinedHandlerEffect {
                name: arm.effect.kind.clone(),
                span: to_source_span(&arm.effect.span),
            }
        })?;
        let effect_decl = &effects[effect_id.0 as usize];
        let op_index = effect_decl
            .ops
            .iter()
            .position(|op| op.name == arm.op.kind)
            .ok_or_else(|| ResolveError::UndefinedHandlerOp {
                effect_name: arm.effect.kind.clone(),
                op_name: arm.op.kind.clone(),
                span: to_source_span(&arm.op.span),
            })?;
        if !seen.insert((effect_id, op_index)) {
            return Err(ResolveError::DuplicateHandlerArm {
                effect_name: arm.effect.kind.clone(),
                op_name: arm.op.kind.clone(),
                span: to_source_span(&arm.span),
            });
        }

        // Bind arm params + kont in a child scope. We snapshot the
        // outer `vars` so the handler-arm bindings don't leak past
        // the arm body. The snapshot+restore pattern is also used
        // by the if/else moved-set merge in the borrow checker —
        // bindings are scope-local.
        let saved_vars = vars.clone();
        let mut param_var_ids = Vec::with_capacity(arm.param_names.len());
        for pn in &arm.param_names {
            if vars.contains_key(&pn.kind) {
                // C3.4: handler-arm param names share the standard
                // RedeclaredVariable rule. Shadowing isn't allowed
                // at the same scope; an outer-scope binding is
                // simply replaced for the arm body.
                // (Conservative: allow shadowing instead by
                // re-inserting; here we mirror RedeclaredVariable
                // only when the SAME arm declares the same param
                // twice.)
                if saved_vars.get(&pn.kind) != vars.get(&pn.kind) {
                    return Err(ResolveError::RedeclaredVariable {
                        name: pn.kind.clone(),
                        span: to_source_span(&pn.span),
                    });
                }
            }
            let vid = VarId(*next_var_id);
            *next_var_id += 1;
            vars.insert(pn.kind.clone(), vid);
            param_var_ids.push(vid);
        }
        let body = resolve_expr(
            &arm.body,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?;
        // Restore the outer scope.
        *vars = saved_vars;

        resolved_arms.push(ResolvedHandlerArm {
            effect_id,
            op_index,
            effect_name: arm.effect.kind.clone(),
            effect_span: arm.effect.span.clone(),
            op_name: arm.op.kind.clone(),
            op_span: arm.op.span.clone(),
            param_var_ids,
            param_names: arm.param_names.clone(),
            body,
            span: arm.span.clone(),
        });
    }

    let resolved_return_arm = if let Some(ra) = return_arm {
        let saved_vars = vars.clone();
        let vid = VarId(*next_var_id);
        *next_var_id += 1;
        vars.insert(ra.value_name.kind.clone(), vid);
        let body = resolve_expr(
            &ra.body,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?;
        *vars = saved_vars;
        Some(Box::new(ResolvedReturnArm {
            value_var_id: vid,
            value_name: ra.value_name.clone(),
            body,
            span: ra.span.clone(),
        }))
    } else {
        None
    };

    Ok(ResolvedExprKind::Handle {
        body: Box::new(body),
        arms: resolved_arms,
        return_arm: resolved_return_arm,
    })
}

/// C3.4 / ADR 0020 D5: resolve `perform EffectName.OpName(args)`.
#[allow(clippy::too_many_arguments)]
fn resolve_perform_expr(
    effect: &Spanned<String>,
    op: &Spanned<String>,
    args: &[Expr],
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    class_table: &HashMap<String, ClassId>,
    effect_table: &HashMap<String, EffectId>,
    effects: &[ResolvedEffectDecl],
    impls: ImplCtx<'_>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExprKind, ResolveError> {
    let effect_id = *effect_table.get(&effect.kind).ok_or_else(|| {
        ResolveError::UndefinedHandlerEffect {
            name: effect.kind.clone(),
            span: to_source_span(&effect.span),
        }
    })?;
    let effect_decl = &effects[effect_id.0 as usize];
    let op_index = effect_decl
        .ops
        .iter()
        .position(|odecl| odecl.name == op.kind)
        .ok_or_else(|| ResolveError::UndefinedHandlerOp {
            effect_name: effect.kind.clone(),
            op_name: op.kind.clone(),
            span: to_source_span(&op.span),
        })?;
    let mut resolved_args = Vec::with_capacity(args.len());
    for arg in args {
        resolved_args.push(resolve_expr(
            arg,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
            impls,
            vars,
            next_var_id,
        )?);
    }
    Ok(ResolvedExprKind::Perform {
        effect_id,
        op_index,
        effect_name: effect.kind.clone(),
        effect_span: effect.span.clone(),
        op_name: op.kind.clone(),
        op_span: op.span.clone(),
        args: resolved_args,
    })
}

/// Visible to crates that consume `ResolvedProgram` and need a
/// struct-name → [`StructId`] lookup (e.g., sentinel-types resolving
/// type-position identifiers per ADR 0013 D4). Built on demand from
/// `program.structs`.
pub fn struct_name_table(program: &ResolvedProgram) -> HashMap<String, StructId> {
    program
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.id))
        .collect()
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

// =============================================================================
// Salsa-tracked query
// =============================================================================

/// Salsa-tracked resolve query. Chains on
/// [`sentinel_syntax::parse_query`]: pulls the parsed Program out
/// of the parse query's cached result and resolves it. Returns
/// `None` if either parsing or resolving fails. Diagnostics flow
/// through the salsa accumulator (parse diagnostics from
/// parse_query, resolve diagnostics from here); the driver
/// collects everything via
/// `resolve_query::accumulated::<Diagnostic>(db, file)` and gets
/// the transitively-accumulated set.
#[salsa::tracked(return_ref)]
pub fn resolve_query(db: &dyn SentinelDb, file: SourceFile) -> Option<ResolvedProgram> {
    let program = sentinel_syntax::parse_query(db, file).as_ref()?;
    match resolve(program) {
        Ok(resolved) => Some(resolved),
        Err(err) => {
            resolve_error_to_diagnostic(&err).accumulate(db);
            None
        }
    }
}

fn resolve_error_to_diagnostic(err: &ResolveError) -> Diagnostic {
    let (code, message, span): (&'static str, String, std::ops::Range<usize>) = match err {
        ResolveError::UndefinedVariable { name, span } => (
            "sentinel::resolve::undefined_variable",
            format!("undefined variable `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedeclaredVariable { name, span } => (
            "sentinel::resolve::redeclared_variable",
            format!("variable `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedFunction { name, span } => (
            "sentinel::resolve::undefined_function",
            format!("undefined function `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::ArityMismatch { name, expected, got, span } => (
            "sentinel::resolve::arity_mismatch",
            format!("`{name}` takes {expected} argument(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedFunction { name, span } => (
            "sentinel::resolve::redefined_function",
            format!("function `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedEnum { name, span } => (
            "sentinel::resolve::redefined_enum",
            format!("`{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateVariant { enum_name, variant_name, span } => (
            "sentinel::resolve::duplicate_variant",
            format!("enum `{enum_name}` has a duplicate variant `{variant_name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicatePatternBinding { name, span } => (
            "sentinel::resolve::duplicate_pattern_binding",
            format!("pattern binds `{name}` more than once"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::MissingMain => (
            "sentinel::resolve::missing_main",
            "program has no `main` function".to_string(),
            0..0,
        ),
        ResolveError::RedefinedStruct { name, span } => (
            "sentinel::resolve::redefined_struct",
            format!("struct `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedStruct { name, span } => (
            "sentinel::resolve::undefined_struct",
            format!("undefined struct `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateTypeParam { name, span } => (
            "sentinel::resolve::duplicate_type_param",
            format!("duplicate type parameter `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedEffect { name, span } => (
            "sentinel::resolve::redefined_effect",
            format!("effect `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedEffect { name, fn_name, span } => (
            "sentinel::resolve::undefined_effect",
            format!("undefined effect `{name}` in fn `{fn_name}`'s annotation"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateEffectOp { effect_name, op_name, span } => (
            "sentinel::resolve::duplicate_effect_op",
            format!(
                "operation `{op_name}` is declared twice in `effect {effect_name}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DeclassifyNotYet { span } => (
            "sentinel::resolve::declassify_not_yet",
            "`declassify(...)` is not yet supported (lands at C3.1)".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UseDeclNotYet { span } => (
            "sentinel::resolve::use_decl_not_yet",
            "`use` imports are not yet wired (modules land in Phase D.6)".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::ModuleNotFound { module, span } => (
            "sentinel::resolve::module_not_found",
            format!("module `{module}` not found"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::PrivateItem { item, module, span } => (
            "sentinel::resolve::private_item",
            format!("`{item}` is private to module `{module}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UnknownImport { item, module, span } => (
            "sentinel::resolve::unknown_import",
            format!("module `{module}` has no item `{item}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedHandlerEffect { name, span } => (
            "sentinel::resolve::undefined_handler_effect",
            format!("undefined effect `{name}` in handler arm / perform"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedHandlerOp { effect_name, op_name, span } => (
            "sentinel::resolve::undefined_handler_op",
            format!("operation `{op_name}` is not declared in `effect {effect_name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateHandlerArm { effect_name, op_name, span } => (
            "sentinel::resolve::duplicate_handler_arm",
            format!(
                "duplicate handler arm for `{effect_name}.{op_name}` in this `handle`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedClass { name, span } => (
            "sentinel::resolve::redefined_class",
            format!("class `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedClass { name, span } => (
            "sentinel::resolve::undefined_class",
            format!("undefined class `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateClassField { class_name, field_name, span } => (
            "sentinel::resolve::duplicate_class_field",
            format!("class `{class_name}` declares field `{field_name}` twice"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateClassMethod { class_name, method_name, span } => (
            "sentinel::resolve::duplicate_class_method",
            format!("class `{class_name}` declares method `{method_name}` twice"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::SelfOutsideClassContext { span } => (
            "sentinel::resolve::self_outside_class_context",
            "`self` is only valid inside a class method or `init` body".to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedTrait { name, span } => (
            "sentinel::resolve::redefined_trait",
            format!("trait `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateTraitMethod {
            trait_name,
            method_name,
            span,
        } => (
            "sentinel::resolve::duplicate_trait_method",
            format!("trait `{trait_name}` declares method `{method_name}` twice"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedTraitForImpl { name, span } => (
            "sentinel::resolve::undefined_trait_for_impl",
            format!("undefined trait `{name}` in impl block"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedTypeForImpl { name, span } => (
            "sentinel::resolve::undefined_type_for_impl",
            format!("undefined type `{name}` in impl block"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateDefaultImpl {
            trait_name,
            type_name,
            span,
        } => (
            "sentinel::resolve::duplicate_default_impl",
            format!(
                "duplicate default impl of `{trait_name}` for `{type_name}` in this scope"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::DuplicateImplName {
            impl_name,
            trait_name,
            type_name,
            span,
        } => (
            "sentinel::resolve::duplicate_impl_name",
            format!(
                "duplicate impl name `{impl_name}` for `{trait_name}` on `{type_name}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedImpl { name, method, span } => (
            "sentinel::resolve::undefined_impl",
            format!("undefined impl `{name}` in `{name}::{method}(...)`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedTraitMethod {
            impl_name,
            trait_name,
            method_name,
            span,
        } => (
            "sentinel::resolve::undefined_trait_method",
            format!(
                "trait `{trait_name}` (impl `{impl_name}`) has no method `{method_name}`"
            ),
            span.offset()..(span.offset() + span.len()),
        ),
    };
    Diagnostic {
        stage: "resolve",
        severity: Severity::Error,
        code,
        message,
        span,
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-resolve"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_syntax::parse;

    fn resolve_ok(src: &str) -> ResolvedProgram {
        let prog = parse(src).expect("parse");
        resolve(&prog).expect("resolve")
    }

    fn resolve_err(src: &str) -> ResolveError {
        let prog = parse(src).expect("parse");
        resolve(&prog).expect_err("expected resolve error")
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-resolve");
    }

    #[test]
    fn use_import_is_gated_until_d6_module_graph() {
        // Phase D.6 (1/N) / ADR 0037: `use` parses but resolve gates it
        // until the module-graph increment — a non-empty `use` set is a
        // focused `UseDeclNotYet`. Single-file programs (no `use`) are
        // unaffected.
        let err = resolve_err("use util::math::add; fn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::UseDeclNotYet { .. }), "got {err:?}");
        // No `use` → resolves fine (regression guard).
        let _ = resolve_ok("fn main() -> i64 { 0 }");
    }

    // ----- D.6 (1/N) / ADR 0037 D3: cross-module import visibility -----

    #[test]
    fn resolve_imports_accepts_pub_item() {
        let util = parse("pub fn add(a: i64, b: i64) -> i64 { a + b }").expect("parse");
        let main = parse("use util::add; fn main() -> i64 { 0 }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["util".to_string()], program: &util },
            ModuleUnit { path: vec!["main".to_string()], program: &main },
        ];
        resolve_imports(&modules).expect("a `pub` import resolves");
    }

    #[test]
    fn resolve_imports_rejects_private_item() {
        // `add` is not `pub` → module-private → PrivateItem.
        let util = parse("fn add(a: i64, b: i64) -> i64 { a + b }").expect("parse");
        let main = parse("use util::add; fn main() -> i64 { 0 }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["util".to_string()], program: &util },
            ModuleUnit { path: vec!["main".to_string()], program: &main },
        ];
        let err = resolve_imports(&modules).expect_err("private import rejected");
        assert!(
            matches!(&err, ResolveError::PrivateItem { item, module, .. }
                if item == "add" && module == "util"),
            "got {err:?}"
        );
    }

    #[test]
    fn resolve_imports_rejects_unknown_item() {
        let util = parse("pub fn other() -> i64 { 0 }").expect("parse");
        let main = parse("use util::add; fn main() -> i64 { 0 }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["util".to_string()], program: &util },
            ModuleUnit { path: vec!["main".to_string()], program: &main },
        ];
        let err = resolve_imports(&modules).expect_err("unknown import rejected");
        assert!(
            matches!(&err, ResolveError::UnknownImport { item, .. } if item == "add"),
            "got {err:?}"
        );
    }

    #[test]
    fn resolve_imports_rejects_missing_module() {
        // No `util` module in the graph → ModuleNotFound.
        let main = parse("use util::add; fn main() -> i64 { 0 }").expect("parse");
        let modules = vec![ModuleUnit { path: vec!["main".to_string()], program: &main }];
        let err = resolve_imports(&modules).expect_err("missing module rejected");
        assert!(
            matches!(&err, ResolveError::ModuleNotFound { module, .. } if module == "util"),
            "got {err:?}"
        );
    }

    #[test]
    fn merge_modules_qualifies_fns_and_rewrites_cross_module_call() {
        // Path A: merge the graph into one Program — util's `add` becomes
        // `util$add`; the entry's `main` keeps its symbol; main's call to
        // the imported `add` is rewritten to the qualified `util$add`.
        let main = parse("use util::add; fn main() -> i64 { add(2, 3) }").expect("parse");
        let util = parse("pub fn add(a: i64, b: i64) -> i64 { a + b }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["main".to_string()], program: &main }, // entry first
            ModuleUnit { path: vec!["util".to_string()], program: &util },
        ];
        let merged = merge_modules(&modules).expect("merge");

        let names: Vec<&str> = merged.fns.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"util$add"), "util's fn is qualified; names: {names:?}");
        assert!(names.contains(&"main"), "the entry's main keeps its symbol; names: {names:?}");

        let main_fn = merged.fns.iter().find(|f| f.name == "main").expect("main fn");
        match &main_fn.body.tail.kind {
            ExprKind::Call { callee, .. } => {
                assert_eq!(callee, "util$add", "the cross-module call is rewritten");
            }
            other => panic!("expected a Call tail, got {other:?}"),
        }
        assert!(merged.uses.is_empty(), "uses are resolved into the merge");
    }

    #[test]
    fn merge_modules_qualifies_struct_and_rewrites_type_refs() {
        // A `pub struct` crosses the boundary: `geo`'s `Point` becomes
        // `geo$Point`, and the entry's references — the `let` annotation
        // AND the struct-literal head — are both rewritten to it.
        let main =
            parse("use geo::Point; fn main() -> i64 { let p: Point = Point { x: 1, y: 2 }; p.x }")
                .expect("parse");
        let geo = parse("pub struct Point { x: i64, y: i64 }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["main".to_string()], program: &main }, // entry first
            ModuleUnit { path: vec!["geo".to_string()], program: &geo },
        ];
        let merged = merge_modules(&modules).expect("merge");

        let struct_names: Vec<&str> = merged.structs.iter().map(|s| s.name.as_str()).collect();
        assert!(struct_names.contains(&"geo$Point"), "struct is qualified; {struct_names:?}");

        let main_fn = merged.fns.iter().find(|f| f.name == "main").expect("main fn");
        match &main_fn.body.stmts[0].kind {
            StmtKind::Let { ty_annot: Some(ty), value, .. } => {
                assert!(
                    matches!(&ty.kind, TypeExprKind::Ident(n) if n == "geo$Point"),
                    "the `let` annotation is rewritten; got {:?}",
                    ty.kind
                );
                match &value.kind {
                    ExprKind::StructLit { name, .. } => {
                        assert_eq!(name, "geo$Point", "the struct-literal head is rewritten");
                    }
                    other => panic!("expected a StructLit value, got {other:?}"),
                }
            }
            other => panic!("expected a typed Let, got {other:?}"),
        }
    }

    #[test]
    fn merge_modules_qualifies_enum_and_rewrites_construction_and_pattern() {
        // A `pub enum` crosses the boundary: `shape`'s `Shape` becomes
        // `shape$Shape`; the entry's construction (a `QualifiedCall` head)
        // AND a `match` variant pattern's enum name are both rewritten.
        let main = parse(
            "use shape::Shape; fn main() -> i64 { let s: Shape = Shape::Circle(3); \
             match s { Shape::Circle(r) => r, _ => 0 } }",
        )
        .expect("parse");
        let shape = parse("pub enum Shape { Circle(i64), Square(i64) }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["main".to_string()], program: &main }, // entry first
            ModuleUnit { path: vec!["shape".to_string()], program: &shape },
        ];
        let merged = merge_modules(&modules).expect("merge");

        let enum_names: Vec<&str> = merged.enums.iter().map(|e| e.name.as_str()).collect();
        assert!(enum_names.contains(&"shape$Shape"), "enum is qualified; {enum_names:?}");

        let main_fn = merged.fns.iter().find(|f| f.name == "main").expect("main fn");
        match &main_fn.body.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::QualifiedCall { impl_name, .. } => {
                    assert_eq!(impl_name, "shape$Shape", "enum-construction head rewritten");
                }
                other => panic!("expected a QualifiedCall value, got {other:?}"),
            },
            other => panic!("expected a Let, got {other:?}"),
        }
        match &main_fn.body.tail.kind {
            ExprKind::Match { arms, .. } => match &arms[0].pattern {
                Pattern::Variant { enum_name, .. } => {
                    assert_eq!(enum_name, "shape$Shape", "match-pattern enum name rewritten");
                }
                other => panic!("expected a Variant pattern, got {other:?}"),
            },
            other => panic!("expected a Match tail, got {other:?}"),
        }
    }

    #[test]
    fn merge_modules_qualifies_trait_and_rewrites_impl_ref() {
        // A `pub trait` crosses the boundary: `t`'s `Show` becomes `t$Show`;
        // the entry's `impl as Show for Widget` head is rewritten to the
        // imported qualified trait (and the impl'd class to `main$Widget`).
        let main = parse(
            "use t::Show; class Widget { let v: i64; pub init() { self.v = 0; 0 } } \
             impl as Show for Widget { fn show(self: &Self) -> i64 { self.v } } \
             fn main() -> i64 { 0 }",
        )
        .expect("parse");
        let t = parse("pub trait Show { fn show(self: &Self) -> i64; }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["main".to_string()], program: &main }, // entry first
            ModuleUnit { path: vec!["t".to_string()], program: &t },
        ];
        let merged = merge_modules(&modules).expect("merge");

        let trait_names: Vec<&str> = merged.traits.iter().map(|t| t.name.as_str()).collect();
        assert!(trait_names.contains(&"t$Show"), "trait is qualified; {trait_names:?}");

        let imp = merged
            .impls
            .iter()
            .find(|i| i.type_name == "main$Widget")
            .expect("the impl for the qualified class");
        assert_eq!(imp.trait_name, "t$Show", "impl trait_name → the imported qualified trait");
    }

    #[test]
    fn merge_modules_qualifies_effect_and_rewrites_perform_and_row() {
        // A `pub effect` crosses the boundary: `e`'s `Io` becomes `e$Io`; the
        // entry's `! { Io }` effect row AND the `perform Io.read()` head are
        // both rewritten to the imported qualified effect.
        let main = parse(
            "use e::Io; fn run() -> i64 ! { Io } { perform Io.read() } fn main() -> i64 { 0 }",
        )
        .expect("parse");
        let e = parse("pub effect Io { read() -> i64; }").expect("parse");
        let modules = vec![
            ModuleUnit { path: vec!["main".to_string()], program: &main }, // entry first
            ModuleUnit { path: vec!["e".to_string()], program: &e },
        ];
        let merged = merge_modules(&modules).expect("merge");

        let effect_names: Vec<&str> = merged.effects.iter().map(|x| x.name.as_str()).collect();
        assert!(effect_names.contains(&"e$Io"), "effect is qualified; {effect_names:?}");

        let run = merged.fns.iter().find(|f| f.name == "main$run").expect("run fn");
        assert_eq!(run.effect_row[0].kind, "e$Io", "the effect row entry is rewritten");
        match &run.body.tail.kind {
            ExprKind::Perform { effect, .. } => {
                assert_eq!(effect.kind, "e$Io", "the perform effect name is rewritten");
            }
            other => panic!("expected a Perform tail, got {other:?}"),
        }
    }

    // ----- positive paths -----

    #[test]
    fn resolves_minimal_main() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.main().name, "main");
        assert!(p.main().signature(&p).is_main);
        // FnId(0) = print, FnId(1) = unwrap_or, FnId(2) = is_some,
        // FnId(3) = len, FnId(4..=6) = str_eq/u8_to_i64/i64_to_u8 (D.2),
        // FnId(7..=10) = vec_new/push/pop/vec_to_array (D.3),
        // FnId(11..=13) = read_file/write_file/print_bytes (D.4),
        // FnId(14) = main (the first user fn).
        assert_eq!(p.main().id, FnId(14));
        assert_eq!(p.fn_signatures[0].name, "print");
        assert!(p.fn_signatures[0].is_runtime);
    }

    #[test]
    fn resolves_let_and_var() {
        let p = resolve_ok("fn main() -> i64 { let x = 5; x }");
        let body = &p.main().body;
        assert_eq!(body.stmts.len(), 1);
        // Let binds VarId(0) (first var in the program).
        match &body.stmts[0].kind {
            ResolvedStmtKind::Let { id, name, .. } => {
                assert_eq!(*id, VarId(0));
                assert_eq!(name, "x");
            }
            other => panic!("expected Let, got {other:?}"),
        }
        match &body.tail.kind {
            ResolvedExprKind::Var(id) => assert_eq!(*id, VarId(0)),
            other => panic!("expected Var, got {other:?}"),
        }
    }

    #[test]
    fn resolves_param_and_use() {
        let p = resolve_ok("fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { double(7) }");
        // double has 1 param → VarId(0)
        let double = &p.fns[0];
        assert_eq!(double.name, "double");
        assert_eq!(double.params.len(), 1);
        assert_eq!(double.params[0].id, VarId(0));
        // Body references VarId(0)
        match &double.body.tail.kind {
            ResolvedExprKind::Binary(_, lhs, _) => match &lhs.kind {
                ResolvedExprKind::Var(id) => assert_eq!(*id, VarId(0)),
                other => panic!("expected Var lhs, got {other:?}"),
            },
            other => panic!("expected Binary, got {other:?}"),
        }
        // FnId(0..=13) = the 14 runtime builtins (print, unwrap_or,
        // is_some, len, str_eq, u8_to_i64, i64_to_u8, vec_new, push, pop,
        // vec_to_array, read_file, write_file, print_bytes); FnId(14) =
        // double (first user fn), FnId(15) = main.
        let main = p.main();
        match &main.body.tail.kind {
            ResolvedExprKind::Call { id, .. } => assert_eq!(*id, FnId(14)),
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn print_resolves_to_print_fn_id() {
        let p = resolve_ok("fn main() -> i64 { print(42) }");
        match &p.main().body.tail.kind {
            ResolvedExprKind::Call { id, .. } => assert_eq!(*id, PRINT_FN_ID),
            other => panic!("expected Call to print, got {other:?}"),
        }
    }

    #[test]
    fn fn_ids_are_unique_per_program() {
        let p = resolve_ok("fn a() -> i64 { 1 }\nfn b() -> i64 { 2 }\nfn c() -> i64 { 3 }\nfn main() -> i64 { 0 }");
        let mut ids: Vec<FnId> = p.fns.iter().map(|f| f.id).collect();
        ids.sort_by_key(|f| f.0);
        ids.dedup();
        assert_eq!(ids.len(), 4); // all distinct
    }

    #[test]
    fn var_ids_increment_across_functions() {
        // Two fns each binding one param — should get distinct VarIds.
        let p = resolve_ok("fn f(x: i64) -> i64 { x }\nfn g(y: i64) -> i64 { y }\nfn main() -> i64 { f(1) + g(2) }");
        let f = &p.fns[0];
        let g = &p.fns[1];
        assert_ne!(f.params[0].id, g.params[0].id);
    }

    // ----- error paths -----

    #[test]
    fn undefined_variable_errors() {
        let err = resolve_err("fn main() -> i64 { y }");
        assert!(matches!(err, ResolveError::UndefinedVariable { ref name, .. } if name == "y"));
    }

    #[test]
    fn redeclared_variable_errors() {
        let err = resolve_err("fn main() -> i64 { let x = 1; let x = 2; x }");
        assert!(matches!(err, ResolveError::RedeclaredVariable { ref name, .. } if name == "x"));
    }

    #[test]
    fn redeclared_param_errors() {
        let err = resolve_err("fn dup(x: i64, x: i64) -> i64 { x }\nfn main() -> i64 { dup(1, 2) }");
        assert!(matches!(err, ResolveError::RedeclaredVariable { ref name, .. } if name == "x"));
    }

    #[test]
    fn undefined_function_errors() {
        let err = resolve_err("fn main() -> i64 { frobnicate(5) }");
        assert!(matches!(err, ResolveError::UndefinedFunction { ref name, .. } if name == "frobnicate"));
    }

    #[test]
    fn arity_mismatch_errors() {
        let err = resolve_err("fn main() -> i64 { print(1, 2) }");
        assert!(matches!(
            err,
            ResolveError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "print"
        ));
    }

    #[test]
    fn arity_mismatch_for_user_fn() {
        let err = resolve_err("fn one(x: i64) -> i64 { x }\nfn main() -> i64 { one(1, 2) }");
        assert!(matches!(
            err,
            ResolveError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "one"
        ));
    }

    #[test]
    fn redefined_function_errors() {
        let err = resolve_err("fn dup() -> i64 { 1 }\nfn dup() -> i64 { 2 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "dup"));
    }

    #[test]
    fn user_redefining_print_errors() {
        let err = resolve_err("fn print(x: i64) -> i64 { x }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "print"));
    }

    #[test]
    fn missing_main_errors() {
        let err = resolve_err("fn foo() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::MissingMain));
    }

    #[test]
    fn let_x_equals_x_errors_when_outer_x_undefined() {
        // The RHS resolves BEFORE the binding takes effect, so this
        // is "x undefined" not "self-reference."
        let err = resolve_err("fn main() -> i64 { let x = x; x }");
        assert!(matches!(err, ResolveError::UndefinedVariable { ref name, .. } if name == "x"));
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
    fn resolve_query_returns_some_for_valid_source() {
        let db = TestDb::default();
        let file =
            SourceFile::new(&db, "test.sentinel".to_string(), "fn main() -> i64 { 42 }".to_string());
        let result = resolve_query(&db, file);
        assert!(result.is_some());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn resolve_query_emits_diagnostic_on_error() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { undeclared }".to_string(),
        );
        let result = resolve_query(&db, file);
        assert!(result.is_none());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "resolve");
        assert_eq!(diags[0].code, "sentinel::resolve::undefined_variable");
    }

    #[test]
    fn resolve_query_propagates_parse_diagnostic_with_parse_stage() {
        // Lex/parse failure → resolve_query short-circuits to None;
        // parse_query's accumulated diagnostic flows transitively.
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() { (1 + 2 }".to_string(),
        );
        let result = resolve_query(&db, file);
        assert!(result.is_none());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "parse");
    }

    // ----- C1.4: struct decls + struct literal + field access -----

    #[test]
    fn resolves_struct_decl() {
        let p = resolve_ok(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs.len(), 1);
        assert_eq!(p.structs[0].name, "Point");
        assert_eq!(p.structs[0].id, StructId(0));
        assert_eq!(p.structs[0].fields.len(), 2);
    }

    #[test]
    fn resolves_multiple_struct_decls_in_order() {
        let p = resolve_ok(
            "struct A { x: i64 }\nstruct B { y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].id, StructId(0));
        assert_eq!(p.structs[0].name, "A");
        assert_eq!(p.structs[1].id, StructId(1));
        assert_eq!(p.structs[1].name, "B");
    }

    #[test]
    fn resolves_struct_literal_to_struct_id() {
        let p = resolve_ok(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { let p = Point { x: 3, y: 4 }; 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::StructLit { id, name, fields, .. } => {
                    assert_eq!(*id, StructId(0));
                    assert_eq!(name, "Point");
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn resolves_field_access() {
        let p = resolve_ok(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { x: 3 }; p.x }",
        );
        let tail = &p.main().body.tail;
        match &tail.kind {
            ResolvedExprKind::FieldAccess { field, target, .. } => {
                assert_eq!(field, "x");
                match &target.kind {
                    ResolvedExprKind::Var(_) => {}
                    other => panic!("expected Var target, got {other:?}"),
                }
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        }
    }

    #[test]
    fn redefined_struct_errors() {
        let err = resolve_err(
            "struct Foo { x: i64 }\nstruct Foo { y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedStruct { ref name, .. } if name == "Foo"));
    }

    #[test]
    fn undefined_struct_in_literal_errors() {
        let err = resolve_err(
            "fn main() -> i64 { let p = Bogus { x: 1 }; 0 }",
        );
        assert!(matches!(err, ResolveError::UndefinedStruct { ref name, .. } if name == "Bogus"));
    }

    #[test]
    fn struct_name_table_helper_works() {
        let p = resolve_ok(
            "struct A { x: i64 }\nstruct B { y: i64 }\nfn main() -> i64 { 0 }",
        );
        let t = struct_name_table(&p);
        assert_eq!(t["A"], StructId(0));
        assert_eq!(t["B"], StructId(1));
        assert_eq!(t.len(), 2);
    }

    // ----- C4.1 (2/N): classes -----

    #[test]
    fn resolves_class_decl_assigns_id_zero() {
        let p = resolve_ok(
            "class Point { let x: i64; let y: i64; init(x: i64, y: i64) { self.x = x; self.y = y; 0 } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.classes.len(), 1);
        let c = &p.classes[0];
        assert_eq!(c.id, ClassId(0));
        assert_eq!(c.name, "Point");
        assert_eq!(c.fields.len(), 2);
        assert!(c.init.is_some());
        assert!(c.methods.is_empty());
    }

    #[test]
    fn resolves_class_init_call() {
        let p = resolve_ok(
            "class P { let x: i64; init(x: i64) { self.x = x; 0 } }\nfn main() -> i64 { let p = P::init(5); 0 }",
        );
        let main = p.main();
        if let ResolvedStmtKind::Let { value, .. } = &main.body.stmts[0].kind {
            assert!(matches!(
                &value.kind,
                ResolvedExprKind::ClassInit { id, name, args, .. }
                    if *id == ClassId(0) && name == "P" && args.len() == 1
            ));
        } else {
            panic!("expected let");
        }
    }

    #[test]
    fn resolves_method_call_postfix() {
        let p = resolve_ok(
            "class P { let x: i64; init(v: i64) { self.x = v; 0 } pub fn get(self: &Self) -> i64 { self.x } }\n\
             fn main() -> i64 { let p = P::init(7); p.get() }",
        );
        let tail = &p.main().body.tail;
        match &tail.kind {
            ResolvedExprKind::MethodCall { method, args, .. } => {
                assert_eq!(method, "get");
                assert!(args.is_empty());
            }
            other => panic!("expected MethodCall, got {other:?}"),
        }
    }

    #[test]
    fn redefined_class_errors() {
        let err = resolve_err(
            "class C { let x: i64; init(v: i64) { self.x = v; 0 } } class C { let y: i64; init(v: i64) { self.y = v; 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedClass { ref name, .. } if name == "C"));
    }

    #[test]
    fn class_struct_name_collision_errors() {
        let err = resolve_err(
            "struct Pt { x: i64 } class Pt { let y: i64; init(v: i64) { self.y = v; 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedClass { ref name, .. } if name == "Pt"));
    }

    #[test]
    fn undefined_class_in_init_call_errors() {
        let err = resolve_err("fn main() -> i64 { Bogus::init(1); 0 }");
        assert!(matches!(err, ResolveError::UndefinedClass { ref name, .. } if name == "Bogus"));
    }

    #[test]
    fn duplicate_class_field_errors() {
        let err = resolve_err(
            "class P { let x: i64; let x: i64; init(v: i64) { self.x = v; 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateClassField { ref class_name, ref field_name, .. }
                if class_name == "P" && field_name == "x"
        ));
    }

    #[test]
    fn duplicate_class_method_errors() {
        let err = resolve_err(
            "class P { let x: i64; init(v: i64) { self.x = v; 0 } pub fn get(self: &Self) -> i64 { self.x } pub fn get(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateClassMethod { ref class_name, ref method_name, .. }
                if class_name == "P" && method_name == "get"
        ));
    }

    #[test]
    fn self_outside_class_context_errors() {
        let err = resolve_err("fn main() -> i64 { self }");
        assert!(matches!(err, ResolveError::SelfOutsideClassContext { .. }));
    }

    #[test]
    fn self_inside_method_resolves() {
        // self.x inside a method body should resolve cleanly.
        let p = resolve_ok(
            "class P { let x: i64; init(v: i64) { self.x = v; 0 } pub fn get(self: &Self) -> i64 { self.x } }\nfn main() -> i64 { 0 }",
        );
        let c = &p.classes[0];
        let m = &c.methods[0];
        if let ResolvedExprKind::FieldAccess { target, .. } = &m.body.tail.kind {
            assert!(matches!(target.kind, ResolvedExprKind::Var(_)));
        } else {
            panic!("expected FieldAccess");
        }
    }

    // ----- C4.2 (2/N): trait + impl + qualified-call positive +
    //                    rejection paths -----

    #[test]
    fn trait_decl_assigned_id() {
        let p = resolve_ok(
            "trait Writer { fn write(self: &mut Self, d: i64) -> i64; }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.traits.len(), 1);
        assert_eq!(p.traits[0].id, TraitId(0));
        assert_eq!(p.traits[0].methods.len(), 1);
        assert_eq!(p.traits[0].methods[0].name, "write");
    }

    #[test]
    fn redefined_trait_errors() {
        let err = resolve_err(
            "trait A {}\ntrait A {}\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedTrait { ref name, .. } if name == "A"));
    }

    #[test]
    fn trait_collides_with_class_namespace() {
        let err = resolve_err(
            "class Foo { let x: i64; init() { self.x = 0; 0 } }\ntrait Foo {}\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedTrait { ref name, .. } if name == "Foo"));
    }

    #[test]
    fn duplicate_trait_method_errors() {
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; fn write(self: &mut Self, d: i64) -> i64; }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateTraitMethod { ref trait_name, ref method_name, .. }
                if trait_name == "W" && method_name == "write"
        ));
    }

    #[test]
    fn default_impl_assigned_id() {
        let p = resolve_ok(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.impls.len(), 1);
        assert_eq!(p.impls[0].id, ImplId(0));
        assert!(p.impls[0].name.is_none());
        assert_eq!(p.impls[0].trait_id, TraitId(0));
        assert!(matches!(p.impls[0].target, ImplTarget::Class(_)));
        assert_eq!(p.impls[0].methods.len(), 1);
        assert_eq!(p.impls[0].methods[0].name, "write");
    }

    #[test]
    fn named_impl_assigned_id() {
        let p = resolve_ok(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl Doubling as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.impls[0].name.as_deref(), Some("Doubling"));
    }

    #[test]
    fn undefined_trait_for_impl_errors() {
        let err = resolve_err(
            "class F { let c: i64; init() { self.c = 0; 0 } }\nimpl as Missing for F { fn x(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::UndefinedTraitForImpl { ref name, .. } if name == "Missing"
        ));
    }

    #[test]
    fn undefined_type_for_impl_errors() {
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nimpl as W for Nope { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::UndefinedTypeForImpl { ref name, .. } if name == "Nope"
        ));
    }

    #[test]
    fn duplicate_default_impl_errors() {
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { d * 2 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateDefaultImpl { ref trait_name, ref type_name, .. }
                if trait_name == "W" && type_name == "F"
        ));
    }

    #[test]
    fn duplicate_impl_name_errors() {
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl Doubling as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nimpl Doubling as W for F { fn write(self: &mut Self, d: i64) -> i64 { d * 2 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateImplName { ref impl_name, .. } if impl_name == "Doubling"
        ));
    }

    #[test]
    fn qualified_call_routes_through_impl() {
        let p = resolve_ok(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl Doubling as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { let mut s: F = F::init(); Doubling::write(&mut s, 16) }",
        );
        // The main body's tail should be a QualifiedCall referencing
        // the Doubling impl (ImplId(0)) and method index 0 (write).
        match &p.main().body.tail.kind {
            ResolvedExprKind::QualifiedCall { impl_id, method_index, .. } => {
                assert_eq!(*impl_id, ImplId(0));
                assert_eq!(*method_index, 0);
            }
            other => panic!("expected QualifiedCall, got {other:?}"),
        }
    }

    #[test]
    fn undefined_impl_in_qualified_call_errors() {
        let err = resolve_err(
            "fn main() -> i64 { Nope::write(0) }",
        );
        assert!(matches!(
            err,
            ResolveError::UndefinedImpl { ref name, ref method, .. }
                if name == "Nope" && method == "write"
        ));
    }

    #[test]
    fn undefined_trait_method_in_qualified_call_errors() {
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl Doubling as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { let mut s: F = F::init(); Doubling::missing(&mut s, 0) }",
        );
        assert!(matches!(
            err,
            ResolveError::UndefinedTraitMethod { ref impl_name, ref method_name, .. }
                if impl_name == "Doubling" && method_name == "missing"
        ));
    }

    // ----- C4.3: delegate synthesis -----

    #[test]
    fn delegate_synthesizes_impl_and_field() {
        // `delegate writer: F to W` inside class L synthesizes:
        //   - a field `writer: F` on L
        //   - a default impl of W for L with the auto-forwarder
        let p = resolve_ok(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { self.c = self.c + d; self.c } }\nclass L { delegate writer: F to W; init(w: F) { self.writer = w; 0 } }\nfn main() -> i64 { 0 }",
        );
        // Class L's resolved fields include the synthesized
        // delegate field.
        let l = p.classes.iter().find(|c| c.name == "L").expect("class L");
        assert_eq!(l.fields.len(), 1);
        assert_eq!(l.fields[0].name, "writer");
        // resolved_impls has 2 entries: user `impl as W for F` +
        // synthesized `impl as W for L` (from the delegate).
        assert_eq!(p.impls.len(), 2);
        let synth = p
            .impls
            .iter()
            .find(|i| i.type_name == "L")
            .expect("synthesized impl for L");
        assert!(synth.name.is_none()); // default impl
        assert_eq!(synth.methods.len(), 1);
        assert_eq!(synth.methods[0].name, "write");
    }

    #[test]
    fn delegate_collides_with_user_impl_errors() {
        // Both an explicit `impl as W for L` AND a `delegate ... to W`
        // would produce two default impls of (W, L). DuplicateDefaultImpl.
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nclass L { delegate writer: F to W; init(w: F) { self.writer = w; 0 } }\nimpl as W for L { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateDefaultImpl { ref trait_name, ref type_name, .. }
                if trait_name == "W" && type_name == "L"
        ));
    }

    #[test]
    fn delegate_to_undefined_trait_errors() {
        let err = resolve_err(
            "class L { delegate writer: i64 to Missing; init() { 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::UndefinedTraitForImpl { ref name, .. } if name == "Missing"
        ));
    }

    // ----- C4.4 (2/N): scope/spawn/await resolve pass-through -----

    #[test]
    fn scope_concurrent_resolves() {
        let prog = resolve_ok("fn main() -> i64 { scope concurrent { 42 } }");
        let main = &prog.fns[prog.fns.len() - 1];
        assert!(matches!(
            main.body.tail.kind,
            ResolvedExprKind::Scope { .. }
        ));
    }

    #[test]
    fn spawn_resolves() {
        let prog = resolve_ok(
            "fn double(x: i64) -> i64 ! { Async } { x * 2 }\nfn main() -> i64 { spawn double(21); 0 }",
        );
        // The spawn is the first stmt of main's body.
        let main = &prog.fns[prog.fns.len() - 1];
        let first = &main.body.stmts[0];
        assert!(matches!(
            first.kind,
            ResolvedStmtKind::Expr(ref e) if matches!(e.kind, ResolvedExprKind::Spawn { .. })
        ));
    }

    #[test]
    fn await_resolves() {
        // `t.await` resolves as a pass-through Await over a Var.
        let prog = resolve_ok("fn main() -> i64 { let t = 0; t.await }");
        let main = &prog.fns[prog.fns.len() - 1];
        assert!(matches!(
            main.body.tail.kind,
            ResolvedExprKind::Await { .. }
        ));
    }

    #[test]
    fn delegate_field_collides_with_let_field_errors() {
        // Class declares `let writer: i64;` AND `delegate writer: F to W;`.
        let err = resolve_err(
            "trait W { fn write(self: &mut Self, d: i64) -> i64; }\nclass F { let c: i64; init() { self.c = 0; 0 } }\nimpl as W for F { fn write(self: &mut Self, d: i64) -> i64 { d } }\nclass L { let writer: i64; delegate writer: F to W; init() { self.writer = 0; 0 } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::DuplicateClassField { ref field_name, .. } if field_name == "writer"
        ));
    }

    // ----- C1.5: null literal + builtin registration -----

    #[test]
    fn null_literal_resolves() {
        let p = resolve_ok("fn main() -> i64 { let _x = null; 0 }");
        // The Let RHS is a NullLit.
        match &p.main().body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::NullLit => {}
                other => panic!("expected NullLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn unwrap_or_builtin_pre_registered() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        // FnId(0) is print, FnId(1) is unwrap_or, FnId(2) is is_some.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[1].name, "unwrap_or");
        assert_eq!(p.fn_signatures[1].arity, 2);
        assert!(p.fn_signatures[1].is_runtime);
        assert_eq!(p.fn_signatures[2].name, "is_some");
        assert_eq!(p.fn_signatures[2].arity, 1);
        assert!(p.fn_signatures[2].is_runtime);
    }

    #[test]
    fn user_redefining_unwrap_or_errors() {
        let err = resolve_err(
            "fn unwrap_or(x: i64, y: i64) -> i64 { x }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "unwrap_or"));
    }

    #[test]
    fn user_redefining_is_some_errors() {
        let err = resolve_err(
            "fn is_some(x: i64) -> bool { true }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "is_some"));
    }

    // ----- C1.6: array literal + indexing + len builtin -----

    #[test]
    fn array_literal_resolves() {
        let p = resolve_ok("fn main() -> i64 { let xs = [1, 2, 3]; 0 }");
        match &p.main().body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::ArrayLit(elems) => assert_eq!(elems.len(), 3),
                other => panic!("expected ArrayLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn array_index_resolves() {
        let p = resolve_ok("fn main() -> i64 { let xs = [42]; xs[0] }");
        match &p.main().body.tail.kind {
            ResolvedExprKind::Index { .. } => {}
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn len_builtin_pre_registered() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        // FnId(3) is len at C1.6.
        assert_eq!(p.fn_signatures[3].name, "len");
        assert_eq!(p.fn_signatures[3].arity, 1);
        assert!(p.fn_signatures[3].is_runtime);
    }

    #[test]
    fn user_redefining_len_errors() {
        let err = resolve_err(
            "fn len(x: i64) -> i64 { x }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "len"));
    }

    #[test]
    fn resolve_query_caches_across_reruns() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { 1 + 2 }".to_string(),
        );
        let r1 = resolve_query(&db, file).clone();
        let r2 = resolve_query(&db, file).clone();
        assert_eq!(r1, r2);
        assert!(r1.is_some());
    }

    // ----- C1.7 / ADR 0016: generics resolve -----

    #[test]
    fn c17_generic_fn_carries_type_params() {
        let p = resolve_ok("fn id<T>(x: T) -> T { x }\nfn main() -> i64 { 0 }");
        let id_fn = p.fns.iter().find(|f| f.name == "id").expect("id fn");
        assert_eq!(id_fn.type_params.len(), 1);
        assert_eq!(id_fn.type_params[0].name, "T");
        assert_eq!(id_fn.type_params[0].id, TypeParamId(0));
        // Signature also tracks the count.
        let sig = p.signature(id_fn.id);
        assert_eq!(sig.type_params_count, 1);
    }

    #[test]
    fn c17_generic_struct_carries_type_params() {
        let p = resolve_ok(
            "struct Box<T> { value: T }\nfn main() -> i64 { 0 }",
        );
        let s = &p.structs[0];
        assert_eq!(s.type_params.len(), 1);
        assert_eq!(s.type_params[0].name, "T");
        assert_eq!(s.type_params[0].id, TypeParamId(0));
    }

    #[test]
    fn c17_multi_type_param_fn() {
        let p = resolve_ok(
            "fn pair<A, B>(a: A, b: B) -> A { a }\nfn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "pair").expect("pair");
        assert_eq!(f.type_params.len(), 2);
        assert_eq!(f.type_params[0].name, "A");
        assert_eq!(f.type_params[0].id, TypeParamId(0));
        assert_eq!(f.type_params[1].name, "B");
        assert_eq!(f.type_params[1].id, TypeParamId(1));
    }

    #[test]
    fn c17_duplicate_type_param_in_fn_errors() {
        let err =
            resolve_err("fn f<T, T>(x: T) -> T { x }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, ResolveError::DuplicateTypeParam { ref name, .. } if name == "T"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_duplicate_type_param_in_struct_errors() {
        let err = resolve_err(
            "struct Foo<T, T> { x: T }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, ResolveError::DuplicateTypeParam { ref name, .. } if name == "T"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_builtins_carry_type_params_count() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        // print is non-generic; the three builtins post-print are.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[0].type_params_count, 0);
        assert_eq!(p.fn_signatures[1].name, "unwrap_or");
        assert_eq!(p.fn_signatures[1].type_params_count, 1);
        assert_eq!(p.fn_signatures[2].name, "is_some");
        assert_eq!(p.fn_signatures[2].type_params_count, 1);
        assert_eq!(p.fn_signatures[3].name, "len");
        assert_eq!(p.fn_signatures[3].type_params_count, 1);
    }

    #[test]
    fn c17_non_generic_fn_has_empty_type_params() {
        let p = resolve_ok("fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { 0 }");
        let f = p.fns.iter().find(|f| f.name == "add").expect("add");
        assert!(f.type_params.is_empty());
        assert_eq!(p.signature(f.id).type_params_count, 0);
    }

    // ---- C3.2(a) / ADR 0019 D4 + D1: effect surface resolves ----

    #[test]
    fn c32_effect_decl_resolves() {
        // Effect declarations now mirror into ResolvedProgram.effects.
        let p = resolve_ok(
            "effect Io { log(msg: i64) -> i64; }\nfn main() -> i64 { 0 }",
        );
        // C4.4 / ADR 0024 D5: the built-in `Async` effect is
        // auto-registered after user effects, so the table holds
        // `Io` (id 0) + `Async` (id 1).
        assert_eq!(p.effects.len(), 2);
        assert_eq!(p.effects[0].name, "Io");
        assert_eq!(p.effects[0].id, EffectId(0));
        assert_eq!(p.effects[0].ops.len(), 1);
        assert_eq!(p.effects[0].ops[0].name, "log");
        assert_eq!(p.effects[1].name, "Async");
        assert_eq!(p.effects[1].id, EffectId(1));
        assert!(p.effects[1].ops.is_empty());
    }

    #[test]
    fn c32_redefined_effect_errors() {
        let err = resolve_err(
            "effect Io { } effect Io { } fn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, ResolveError::RedefinedEffect { ref name, .. } if name == "Io"),
            "got {err:?}"
        );
    }

    #[test]
    fn c32_duplicate_effect_op_errors() {
        let err = resolve_err(
            "effect Io { log(x: i64); log(x: i64); }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, ResolveError::DuplicateEffectOp { ref effect_name, ref op_name, .. }
                if effect_name == "Io" && op_name == "log"),
            "got {err:?}"
        );
    }

    #[test]
    fn c32_fn_with_declared_effect_annotation_resolves() {
        let p = resolve_ok(
            "effect Io { } fn run() -> i64 ! { Io } { 0 } fn main() -> i64 { 0 }",
        );
        let run = p.fns.iter().find(|f| f.name == "run").expect("run");
        assert_eq!(run.effect_row, vec![EffectId(0)]);
    }

    #[test]
    fn c32_undefined_effect_in_fn_annotation_errors() {
        let err = resolve_err("fn run() -> i64 ! { Io } { 0 }\nfn main() -> i64 { 0 }");
        assert!(
            matches!(err, ResolveError::UndefinedEffect { ref name, ref fn_name, .. }
                if name == "Io" && fn_name == "run"),
            "got {err:?}"
        );
    }

    #[test]
    fn c30_declassify_on_undefined_var_still_routes_to_undefined_var() {
        // declassify resolves at C3.1, but its inner is still
        // resolved — an undefined Var inside surfaces normally.
        let err = resolve_err("fn main() -> i64 { declassify(x) }");
        assert!(
            matches!(err, ResolveError::UndefinedVariable { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn c31_declassify_on_defined_var_resolves() {
        // At C3.1 declassify is no longer rejected at resolve.
        // Type-checking is what validates the inner is `secret T`.
        // The resolve step alone succeeds.
        let p = resolve_ok(
            "fn main() -> i64 { let x: i64 = 1; declassify(x) }",
        );
        assert!(!p.fns.is_empty());
    }

    // ----- C3.4 / ADR 0020: handle + perform resolve -----

    #[test]
    fn c34_perform_resolves() {
        let p = resolve_ok(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { handle perform Io.read() with { Io.read(k) => 0 } }",
        );
        let main = p.main();
        // The body should resolve cleanly.
        assert!(matches!(main.body.tail.kind, ResolvedExprKind::Handle { .. }));
    }

    #[test]
    fn c34_perform_undefined_effect_errors() {
        let err = resolve_err(
            "fn main() -> i64 { perform Io.read() }",
        );
        assert!(
            matches!(err, ResolveError::UndefinedHandlerEffect { ref name, .. } if name == "Io"),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_perform_undefined_op_errors() {
        let err = resolve_err(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { perform Io.write() }",
        );
        assert!(
            matches!(err, ResolveError::UndefinedHandlerOp { ref op_name, .. } if op_name == "write"),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_handle_undefined_effect_errors() {
        let err = resolve_err(
            "fn main() -> i64 { handle 0 with { Io.read(k) => 1 } }",
        );
        assert!(
            matches!(err, ResolveError::UndefinedHandlerEffect { ref name, .. } if name == "Io"),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_handle_undefined_op_errors() {
        let err = resolve_err(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { handle 0 with { Io.write(k) => 1 } }",
        );
        assert!(
            matches!(err, ResolveError::UndefinedHandlerOp { ref op_name, .. } if op_name == "write"),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_handle_duplicate_arm_errors() {
        let err = resolve_err(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { handle 0 with { Io.read(k) => 1, Io.read(k) => 2 } }",
        );
        assert!(
            matches!(err, ResolveError::DuplicateHandlerArm { ref op_name, .. } if op_name == "read"),
            "got {err:?}"
        );
    }

    #[test]
    fn c34_kont_call_resolves_as_resume_kont() {
        // `k(0)` inside a handler arm body resolves as a
        // ResumeKont, NOT as a (failed) fn lookup.
        let p = resolve_ok(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { handle 0 with { Io.read(k) => k(0) } }",
        );
        // Drill into the handle's first arm and confirm its
        // body is a ResumeKont reference to the arm's kont VarId.
        let main = p.main();
        let kind = &main.body.tail.kind;
        let arms = match kind {
            ResolvedExprKind::Handle { arms, .. } => arms,
            other => panic!("expected Handle, got {other:?}"),
        };
        let arm = &arms[0];
        let kont_id = *arm.param_var_ids.last().expect("kont VarId present");
        match &arm.body.kind {
            ResolvedExprKind::ResumeKont { kont, args, .. } => {
                assert_eq!(*kont, kont_id);
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected ResumeKont, got {other:?}"),
        }
    }

    #[test]
    fn c34_handle_with_return_arm_resolves() {
        let p = resolve_ok(
            "effect Io { read() -> i64; }\
             fn main() -> i64 { handle 0 with { Io.read(k) => 1, return v => v } }",
        );
        let main = p.main();
        match &main.body.tail.kind {
            ResolvedExprKind::Handle { return_arm: Some(_), .. } => {}
            other => panic!("expected Handle with return arm, got {other:?}"),
        }
    }

    // ----- Phase D.1 (3/N) / ADR 0032: enum + match resolve -----

    #[test]
    fn enum_decl_resolves_with_variants() {
        let p = resolve_ok("enum Color { Red, Green, Rgb(i64, i64, i64) }\nfn main() -> i64 { 0 }");
        assert_eq!(p.enums.len(), 1);
        let e = &p.enums[0];
        assert_eq!(e.name, "Color");
        assert_eq!(e.id, EnumId(0));
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].name, "Red");
        assert!(e.variants[0].payloads.is_empty());
        assert_eq!(e.variants[2].name, "Rgb");
        assert_eq!(e.variants[2].payloads.len(), 3);
    }

    #[test]
    fn enum_name_collides_with_struct() {
        let err = resolve_err("struct Color { x: i64 }\nenum Color { Red }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::RedefinedEnum { .. }));
    }

    #[test]
    fn duplicate_variant_rejected() {
        let err = resolve_err("enum Color { Red, Red }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::DuplicateVariant { .. }));
    }

    #[test]
    fn payload_variant_construction_resolves_to_enum_construct() {
        // `Color::Rgb(1,2,3)` parses as a QualifiedCall but resolves
        // to EnumConstruct because `Color` is an enum.
        let p = resolve_ok(
            "enum Color { Red, Rgb(i64, i64, i64) }\n\
             fn main() -> i64 { let c = Color::Rgb(1, 2, 3); 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::EnumConstruct { enum_id, variant_name, args, .. } => {
                    assert_eq!(*enum_id, EnumId(0));
                    assert_eq!(variant_name, "Rgb");
                    assert_eq!(args.len(), 3);
                }
                other => panic!("expected EnumConstruct, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn unit_variant_construction_via_empty_parens_resolves() {
        let p = resolve_ok(
            "enum Color { Red, Green }\n\
             fn main() -> i64 { let c = Color::Red(); 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::EnumConstruct { variant_name, args, .. } => {
                    assert_eq!(variant_name, "Red");
                    assert!(args.is_empty());
                }
                other => panic!("expected EnumConstruct, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn match_resolves_with_arms_and_bindings() {
        let p = resolve_ok(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\n\
             fn area(s: Shape) -> i64 {\n\
                match s {\n\
                    Shape::Unit => 0,\n\
                    Shape::Circle(r) => r,\n\
                    Shape::Rect(w, h) => w,\n\
                }\n\
             }\n\
             fn main() -> i64 { 0 }",
        );
        let area = p.fns.iter().find(|f| f.name == "area").expect("area fn");
        match &area.body.tail.kind {
            ResolvedExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 3);
                // Each variant pattern carries its bindings.
                match &arms[1].pattern {
                    ResolvedPattern::Variant { variant_name, bindings, .. } => {
                        assert_eq!(variant_name, "Circle");
                        assert_eq!(bindings.len(), 1);
                        assert_eq!(bindings[0].name, "r");
                    }
                    other => panic!("expected Variant pattern, got {other:?}"),
                }
            }
            other => panic!("expected Match, got {other:?}"),
        }
    }

    #[test]
    fn match_binding_is_scoped_to_its_arm_only() {
        // `r` from the Circle arm must not leak into a later
        // reference outside the match — it resolves as undefined.
        let err = resolve_err(
            "enum Shape { Circle(i64), Square(i64) }\n\
             fn f(s: Shape) -> i64 {\n\
                let x = match s { Shape::Circle(r) => r, Shape::Square(q) => q };\n\
                r\n\
             }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::UndefinedVariable { .. }));
    }

    #[test]
    fn duplicate_pattern_binding_rejected() {
        let err = resolve_err(
            "enum Pair { Both(i64, i64) }\n\
             fn f(p: Pair) -> i64 { match p { Pair::Both(x, x) => x } }\n\
             fn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::DuplicatePatternBinding { .. }));
    }

    #[test]
    fn wildcard_binding_not_in_scope() {
        // `_` slots get a VarId but aren't referenceable.
        let p = resolve_ok(
            "enum Shape { Circle(i64) }\n\
             fn f(s: Shape) -> i64 { match s { Shape::Circle(_) => 0 } }\n\
             fn main() -> i64 { 0 }",
        );
        let f = p.fns.iter().find(|f| f.name == "f").expect("f fn");
        match &f.body.tail.kind {
            ResolvedExprKind::Match { arms, .. } => match &arms[0].pattern {
                ResolvedPattern::Variant { bindings, .. } => {
                    assert_eq!(bindings.len(), 1);
                    assert_eq!(bindings[0].name, "_");
                }
                other => panic!("expected Variant, got {other:?}"),
            },
            other => panic!("expected Match, got {other:?}"),
        }
    }

    // ----- D.2 (3/N) / ADR 0033 D2: char/string literals now resolve -----

    #[test]
    fn char_lit_resolves() {
        // D.2 (3/N): the decoded byte carries through resolve verbatim
        // (no longer rejected — the type layer assigns `u8`).
        let p = resolve_ok("fn main() -> i64 { let c = 'a'; 0 }");
        match &p.main().body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => {
                assert!(matches!(value.kind, ResolvedExprKind::CharLit(97)), "got {:?}", value.kind);
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn string_lit_resolves() {
        let p = resolve_ok("fn main() -> i64 { let s = \"hi\"; 0 }");
        match &p.main().body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::StringLit(bytes) => assert_eq!(bytes, &vec![104, 105]),
                other => panic!("expected StringLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }
}
