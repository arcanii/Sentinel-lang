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
    BinOp, Block, ClassDecl, CmpOp, Expr, ExprKind, FnDef, HandlerArm, LogicOp, Program, ReturnArm,
    SelfKind, Span, Spanned, Stmt, StmtKind, TypeExpr, TypeParam as AstTypeParam, UnaryOp,
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

/// Position-indexed identifier for a generic type parameter
/// inside the surrounding fn / struct, added at C1.7 per ADR
/// 0016 D6c. Two distinct fns can each have `TypeParamId(0)`
/// referring to their own first type parameter — the ID is
/// scoped to its parent and only meaningful in that context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeParamId(pub u32);

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

    /// C4.2 / ADR 0023 D5 Path 2: `ImplName::method(args)` qualified
    /// call parses at C4.2 (1/N) but isn't resolvable until the
    /// impl table lands at C4.2 (2/N). Surface a clear "not yet"
    /// diagnostic in the meantime.
    #[error("`Name::method(args)` qualified call is not yet supported (lands at C4.2 (2/N))")]
    #[diagnostic(
        code(sentinel::resolve::qualified_call_not_yet),
        help("qualified calls dispatch via the impl table — wait for C4.2 (2/N) to bring up the resolve / types / codegen wiring per ADR 0023 D8")
    )]
    QualifiedCallNotYet {
        #[label("qualified call here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D1: trait declarations parse at C4.2 (1/N)
    /// but the resolve / types / codegen wiring lands at C4.2
    /// (2/N). Trait declarations that appear in the program
    /// surface this diagnostic until then.
    #[error("trait declarations are not yet supported (land at C4.2 (2/N))")]
    #[diagnostic(
        code(sentinel::resolve::trait_decl_not_yet),
        help("trait declarations parse + AST-mirror at C4.2 (1/N); resolve / types / codegen wiring per ADR 0023 D8 lands at C4.2 (2/N)")
    )]
    TraitDeclNotYet {
        name: String,
        #[label("trait `{name}` declared here")]
        span: miette::SourceSpan,
    },

    /// C4.2 / ADR 0023 D3+D4: impl block declarations parse at
    /// C4.2 (1/N) but the resolve / types / codegen wiring lands
    /// at C4.2 (2/N).
    #[error("impl declarations are not yet supported (land at C4.2 (2/N))")]
    #[diagnostic(
        code(sentinel::resolve::impl_decl_not_yet),
        help("impl declarations parse + AST-mirror at C4.2 (1/N); the per-(scope, trait, type) impl table per ADR 0023 D8 lands at C4.2 (2/N)")
    )]
    ImplDeclNotYet {
        trait_name: String,
        type_name: String,
        #[label("`impl ... as {trait_name} for {type_name}` here")]
        span: miette::SourceSpan,
    },
}

// =============================================================================
// resolve() — pure-function entry point
// =============================================================================

/// Resolve a [`Program`] to a [`ResolvedProgram`]. Fails fast on
/// the first error encountered, matching the C0 codegen pass's
/// existing diagnostic shape. Multi-error accumulation is a future
/// concern.
///
/// Order: structs are registered first (so fn signatures can
/// reference struct types in their TypeExprs), then fns, then fn
/// bodies.
pub fn resolve(program: &Program) -> Result<ResolvedProgram, ResolveError> {
    // C4.2 (1/N) / ADR 0023 D8: reject trait + impl declarations
    // until (2/N) brings up the impl table + per-trait method
    // signatures. Parser-level surface is in; resolve / types /
    // codegen wiring is the next iteration. Mirrors the C3.0
    // EffectDeclNotYet pattern.
    if let Some(td) = program.traits.first() {
        return Err(ResolveError::TraitDeclNotYet {
            name: td.name.clone(),
            span: to_source_span(&td.name_span),
        });
    }
    if let Some(id) = program.impls.first() {
        return Err(ResolveError::ImplDeclNotYet {
            trait_name: id.trait_name.clone(),
            type_name: id.type_name.clone(),
            span: to_source_span(&id.span),
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

    let mut fn_table: HashMap<String, FnId> = HashMap::new();
    let mut signatures: Vec<FnSignature> =
        vec![print_sig, unwrap_or_sig, is_some_sig, len_sig];
    fn_table.insert("print".to_string(), PRINT_FN_ID);
    fn_table.insert("unwrap_or".to_string(), UNWRAP_OR_FN_ID);
    fn_table.insert("is_some".to_string(), IS_SOME_FN_ID);
    fn_table.insert("len".to_string(), LEN_FN_ID);

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
            &mut next_var_id,
        )?);
    }

    Ok(ResolvedProgram {
        fns: resolved_fns,
        fn_signatures: signatures,
        structs: resolved_structs,
        effects: resolved_effects,
        classes: resolved_classes,
        span: program.span.clone(),
    })
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
    next_var_id: &mut u32,
) -> Result<ResolvedClassDecl, ResolveError> {
    // Field list: name uniqueness + type-expr carry-through.
    let mut field_names: HashSet<String> = HashSet::new();
    let mut fields: Vec<ResolvedClassField> = Vec::with_capacity(cd.fields.len());
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
                vars,
                next_var_id,
            )?;
            ResolvedStmtKind::Assign { target, value }
        }
        StmtKind::Expr(e) => ResolvedStmtKind::Expr(resolve_expr(
            e,
            fn_table,
            signatures,
            struct_table,
            class_table,
            effect_table,
            effects,
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
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExpr, ResolveError> {
    let kind = match &expr.kind {
        ExprKind::IntLit(n) => ResolvedExprKind::IntLit(*n),
        ExprKind::BoolLit(b) => ResolvedExprKind::BoolLit(*b),
        ExprKind::NullLit => ResolvedExprKind::NullLit,
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
        ExprKind::QualifiedCall { .. } => {
            // C4.2 (1/N) / ADR 0023 D8: parser ships the surface;
            // resolve / types / codegen wiring lands at (2/N).
            return Err(ResolveError::QualifiedCallNotYet {
                span: to_source_span(&expr.span),
            });
        }
    };
    Ok(Spanned { kind, span: expr.span.clone() })
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
        ResolveError::QualifiedCallNotYet { span } => (
            "sentinel::resolve::qualified_call_not_yet",
            "`Name::method(args)` qualified call is not yet supported (lands at C4.2 (2/N))"
                .to_string(),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::TraitDeclNotYet { name, span } => (
            "sentinel::resolve::trait_decl_not_yet",
            format!("trait `{name}` declaration is not yet supported (lands at C4.2 (2/N))"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::ImplDeclNotYet { trait_name, type_name, span } => (
            "sentinel::resolve::impl_decl_not_yet",
            format!(
                "impl `... as {trait_name} for {type_name}` is not yet supported (lands at C4.2 (2/N))"
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

    // ----- positive paths -----

    #[test]
    fn resolves_minimal_main() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.main().name, "main");
        assert!(p.main().signature(&p).is_main);
        // FnId(0) = print, FnId(1) = unwrap_or, FnId(2) = is_some,
        // FnId(3) = len, FnId(4) = main (the first user fn).
        assert_eq!(p.main().id, FnId(4));
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
        // FnId(0) = print, FnId(1) = unwrap_or, FnId(2) = is_some,
        // FnId(3) = len, FnId(4) = double (first user fn), FnId(5) = main.
        let main = p.main();
        match &main.body.tail.kind {
            ResolvedExprKind::Call { id, .. } => assert_eq!(*id, FnId(4)),
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

    // ----- C4.2 (1/N): trait + impl + qualified-call rejections -----

    #[test]
    fn trait_decl_rejected_at_resolve() {
        let err = resolve_err(
            "trait Writer { fn write(self: &mut Self, d: i64) -> i64; }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::TraitDeclNotYet { ref name, .. } if name == "Writer"));
    }

    #[test]
    fn impl_decl_rejected_at_resolve() {
        let err = resolve_err(
            "impl as Writer for File { fn write(self: &mut Self, d: i64) -> i64 { d } }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(
            err,
            ResolveError::ImplDeclNotYet { ref trait_name, ref type_name, .. }
                if trait_name == "Writer" && type_name == "File"
        ));
    }

    #[test]
    fn qualified_call_rejected_at_resolve() {
        let err = resolve_err("fn main() -> i64 { Buffered::write(0, 1) }");
        assert!(matches!(err, ResolveError::QualifiedCallNotYet { .. }));
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
        assert_eq!(p.effects.len(), 1);
        assert_eq!(p.effects[0].name, "Io");
        assert_eq!(p.effects[0].id, EffectId(0));
        assert_eq!(p.effects[0].ops.len(), 1);
        assert_eq!(p.effects[0].ops[0].name, "log");
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
}
