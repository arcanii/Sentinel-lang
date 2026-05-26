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
use sentinel_ast::{BinOp, CmpOp, LogicOp, Span, TypeExpr, TypeExprKind, UnaryOp};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};
use sentinel_resolve::{
    FnId, ResolvedBlock, ResolvedExpr, ResolvedExprKind, ResolvedFnDef, ResolvedProgram,
    ResolvedStmt, ResolvedStmtKind, StructId, TypeParamId, VarId,
};

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
    Bool,
    Struct(StructId),
    /// `?T` per ADR 0014 D1. Payload is the inner base type.
    Nullable(NullableInner),
    /// `[T]` per ADR 0015 D1. Payload is the element type.
    Array(ArrayElem),
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
    Struct(StructId),
    /// `?T` where T is a generic type parameter (only meaningful
    /// inside a generic fn body). C1.7 / ADR 0016 D6b.
    TypeParam(TypeParamId),
}

impl NullableInner {
    /// Promote to the corresponding [`Type`].
    pub fn to_type(self) -> Type {
        match self {
            NullableInner::I64 => Type::I64,
            NullableInner::I32 => Type::I32,
            NullableInner::Bool => Type::Bool,
            NullableInner::Struct(id) => Type::Struct(id),
            NullableInner::TypeParam(id) => Type::TypeParam(id),
        }
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
    Struct(StructId),
    /// `[T]` where T is a generic type parameter (only meaningful
    /// inside a generic fn body). C1.7 / ADR 0016 D6b.
    TypeParam(TypeParamId),
}

impl ArrayElem {
    /// Promote to the corresponding [`Type`].
    pub fn to_type(self) -> Type {
        match self {
            ArrayElem::I64 => Type::I64,
            ArrayElem::I32 => Type::I32,
            ArrayElem::Bool => Type::Bool,
            ArrayElem::Struct(id) => Type::Struct(id),
            ArrayElem::TypeParam(id) => Type::TypeParam(id),
        }
    }
}

impl Type {
    /// `true` if this is a signed-integer type (`I32` or `I64`).
    /// Used to gate arithmetic-operator typing rules — comparisons
    /// accept any integer type but logicals require [`Type::Bool`].
    pub fn is_int(self) -> bool {
        matches!(self, Type::I32 | Type::I64)
    }

    /// `true` if this is a struct type. Used by the field-access
    /// rule to gate "the target must be a struct".
    pub fn is_struct(self) -> bool {
        matches!(self, Type::Struct(_))
    }

    /// `true` if this is a nullable type (`?T`).
    pub fn is_nullable(self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// `true` if this is an array type (`[T]`).
    pub fn is_array(self) -> bool {
        matches!(self, Type::Array(_))
    }

    /// `true` if this is a generic type parameter (`T`, `U`, …).
    /// C1.7 / ADR 0016. Only meaningful inside a generic fn body
    /// or struct decl.
    pub fn is_type_param(self) -> bool {
        matches!(self, Type::TypeParam(_))
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
            Type::Array(_) | Type::Nullable(_) => None,
        }
    }

    /// Try to demote this Type to an [`ArrayElem`] for use as the
    /// payload of an `Array`. Returns `None` for `Array` (would be
    /// nested per ADR 0015 D6) AND for `Nullable` (the `[?T]`
    /// combination is deferred per C1.6's depth-1 amendment).
    pub fn to_array_elem(self) -> Option<ArrayElem> {
        match self {
            Type::I64 => Some(ArrayElem::I64),
            Type::I32 => Some(ArrayElem::I32),
            Type::Bool => Some(ArrayElem::Bool),
            Type::Struct(id) => Some(ArrayElem::Struct(id)),
            Type::TypeParam(id) => Some(ArrayElem::TypeParam(id)),
            Type::Array(_) | Type::Nullable(_) => None,
        }
    }

    /// Substitute every [`Type::TypeParam`] inside `self` against
    /// the given substitution map. Used at generic-fn call sites
    /// per ADR 0016 D7c to compute the concrete parameter / return
    /// type of a monomorphic instantiation. Handles substitution
    /// through `Nullable` and `Array` payloads.
    ///
    /// Concrete (non-TypeParam) types pass through unchanged.
    pub fn substitute(self, subst: &[Type]) -> Type {
        match self {
            Type::I64 | Type::I32 | Type::Bool | Type::Struct(_) => self,
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
                let new_inner = inner.substitute(subst);
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
                let inner = ae.to_type();
                let new_inner = inner.substitute(subst);
                match new_inner.to_array_elem() {
                    Some(new_ae) => Type::Array(new_ae),
                    None => Type::Array(ae),
                }
            }
        }
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
        Type::Struct(id) => match program.and_then(|p| p.structs.get(id.0 as usize)) {
            Some(s) => s.name.clone(),
            None => format!("<struct#{}>", id.0),
        },
        Type::Nullable(inner) => format!("?{}", type_display(inner.to_type(), program)),
        Type::Array(elem) => format!("[{}]", type_display(elem.to_type(), program)),
        Type::TypeParam(id) => format!("<T#{}>", id.0),
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::I64 => write!(f, "i64"),
            Type::I32 => write!(f, "i32"),
            Type::Bool => write!(f, "bool"),
            Type::Struct(id) => write!(f, "<struct#{}>", id.0),
            Type::Nullable(inner) => write!(f, "?{}", inner.to_type()),
            Type::Array(elem) => write!(f, "[{}]", elem.to_type()),
            Type::TypeParam(id) => write!(f, "<T#{}>", id.0),
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
fn resolve_type_expr(
    te: &TypeExpr,
    struct_table: &HashMap<String, StructId>,
) -> Result<Type, TypeError> {
    let empty: TypeParamScope = HashMap::new();
    resolve_type_expr_with_scope(te, struct_table, &empty)
}

fn resolve_type_expr_with_scope(
    te: &TypeExpr,
    struct_table: &HashMap<String, StructId>,
    type_param_scope: &TypeParamScope,
) -> Result<Type, TypeError> {
    match &te.kind {
        TypeExprKind::Ident(name) => {
            // ADR 0016 D9 lookup precedence: type-param scope first,
            // then struct table, then primitives.
            if let Some(&tp_id) = type_param_scope.get(name) {
                return Ok(Type::TypeParam(tp_id));
            }
            match name.as_str() {
                "i64" => Ok(Type::I64),
                "i32" => Ok(Type::I32),
                "bool" => Ok(Type::Bool),
                other => {
                    if let Some(&id) = struct_table.get(other) {
                        Ok(Type::Struct(id))
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
            let inner_ty =
                resolve_type_expr_with_scope(inner, struct_table, type_param_scope)?;
            match inner_ty.to_nullable_inner() {
                Some(ni) => Ok(Type::Nullable(ni)),
                None => Err(TypeError::UnknownType {
                    name: "?(nullable)".to_string(),
                    span: to_source_span(&te.span),
                }),
            }
        }
        TypeExprKind::Array(inner) => {
            // Recursively resolve the inner; reject `[[T]]` per
            // ADR 0015 D6 (multi-dim arrays are deferred).
            let inner_ty =
                resolve_type_expr_with_scope(inner, struct_table, type_param_scope)?;
            match inner_ty.to_array_elem() {
                Some(ae) => Ok(Type::Array(ae)),
                None => Err(TypeError::NestedArray {
                    span: to_source_span(&te.span),
                }),
            }
        }
        TypeExprKind::Generic { name, .. } => {
            // C1.7.4a: generic struct instances are deferred to
            // C1.7.4b — surface a "generics-not-yet" diagnostic
            // carrying the offending name so the placeholder
            // failure mode is clear. (`name<args>` arity / lookup
            // errors will land at C1.7.4b when the real resolution
            // path arrives.)
            Err(TypeError::GenericStructNotYetSupported {
                name: name.clone(),
                span: to_source_span(&te.span),
            })
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
    /// Struct declarations with resolved field types (parallel-tree
    /// mirror of [`sentinel_resolve::ResolvedProgram::structs`]).
    /// Each struct's [`StructId`] matches its index here.
    pub structs: Vec<TypedStructDecl>,
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
    pub is_main: bool,
    pub is_runtime: bool,
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
        name: String,
        name_span: Span,
        /// The variable's resolved type (from the annotation if
        /// present, otherwise inferred from the RHS).
        ty: Type,
        value: TypedExpr,
    },
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
    /// Bool literal. Always carries [`Type::Bool`].
    BoolLit(bool),
    /// Null literal per ADR 0014 D2. Always carries
    /// [`Type::Nullable`]; the inner type comes from bidirectional
    /// checking against the expected context.
    NullLit,
    /// Implicit `T → ?T` widening per ADR 0014 D3. Wraps a `T`-typed
    /// expression so that the outer node carries `?T`. Codegen
    /// lowers this as constructing the `{ i1 true, T payload }`
    /// struct value.
    WidenToNullable(Box<TypedExpr>),
    Var(VarId),
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

    /// C1.7 / ADR 0016 D6: generic struct instances (`Name<T1, T2>`
    /// in type position) are accepted by the parser but not yet
    /// resolvable in C1.7.4a. Closes at C1.7.4b.
    #[error("generic struct types like `{name}<...>` are not yet supported at C1.7.4a")]
    #[diagnostic(
        code(sentinel::types::generic_struct_not_yet_supported),
        help("generic struct instances land at C1.7.4b — generic fns work at C1.7.4a, but generic structs follow in the next commit")
    )]
    GenericStructNotYetSupported {
        name: String,
        #[label("generic struct type")]
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
/// Order:
///   0. Build struct table (name → StructId) from resolved structs.
///   1. Resolve every struct's field types into `TypedStructDecl`s
///      — UnknownType fires here for stale references.
///   2. Detect recursive structs and emit RecursiveStruct on cycle.
///   3. Resolve fn signatures' param + return types (struct names
///      now resolve cleanly against the struct table).
///   4. Type-check each fn body.
pub fn check(program: &ResolvedProgram) -> Result<TypedProgram, TypeError> {
    // Pass 0: struct name table.
    let struct_table: HashMap<String, StructId> =
        sentinel_resolve::struct_name_table(program);

    // Pass 1: resolve struct field types.
    let mut typed_structs: Vec<TypedStructDecl> =
        Vec::with_capacity(program.structs.len());
    for sd in &program.structs {
        // C1.7.4a: generic structs are NOT yet supported in this
        // sub-phase — defer to C1.7.4b. Reject early with a clear
        // diagnostic (the resolve stage already accepted them).
        if !sd.type_params.is_empty() {
            return Err(TypeError::GenericStructNotYetSupported {
                name: sd.name.clone(),
                span: to_source_span(&sd.name_span),
            });
        }
        let typed_type_params: Vec<TypedTypeParam> = Vec::new();
        let empty_scope: TypeParamScope = HashMap::new();
        let mut fields = Vec::with_capacity(sd.fields.len());
        for f in &sd.fields {
            let ty = resolve_type_expr_with_scope(&f.ty, &struct_table, &empty_scope)?;
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
        is_main: false,
        is_runtime: true,
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
        is_main: false,
        is_runtime: true,
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
        is_main: false,
        is_runtime: true,
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
        is_main: false,
        is_runtime: true,
    });

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
                &tp_scope,
            )?);
        }
        let return_type =
            resolve_type_expr_with_scope(&fn_def.return_type, &struct_table, &tp_scope)?;
        typed_signatures.push(TypedFnSignature {
            id: fn_def.id,
            name: resolved_sig.name.clone(),
            name_span: resolved_sig.name_span.clone(),
            type_params: typed_type_params,
            param_types,
            return_type,
            is_main: resolved_sig.is_main,
            is_runtime: resolved_sig.is_runtime,
        });
    }

    // Sort typed_signatures by id so signatures[i] corresponds to
    // FnId(i) — matches ResolvedProgram's invariant.
    typed_signatures.sort_by_key(|s| s.id.0);

    // Pass 4: type-check each fn body.
    let mut typed_fns = Vec::with_capacity(program.fns.len());
    for fn_def in &program.fns {
        typed_fns.push(check_fn(fn_def, &typed_signatures, &typed_structs)?);
    }

    Ok(TypedProgram {
        fns: typed_fns,
        fn_signatures: typed_signatures,
        structs: typed_structs,
        span: program.span.clone(),
    })
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

fn check_fn(
    fn_def: &ResolvedFnDef,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
) -> Result<TypedFnDef, TypeError> {
    // Pull our own signature.
    let signature = &signatures[fn_def.id.0 as usize];

    // Build typed params from our signature (already type-resolved
    // when the signature table was built).
    let mut params = Vec::with_capacity(fn_def.params.len());
    for (param, ty) in fn_def.params.iter().zip(signature.param_types.iter()) {
        params.push(TypedParam {
            id: param.id,
            name: param.name.clone(),
            span: param.span.clone(),
            ty: *ty,
        });
    }

    // Build a VarId -> Type map for this fn's scope.
    let mut env: VarTypeEnv = VarTypeEnv::new();
    for tp in &params {
        env.insert(tp.id, tp.ty);
    }

    let return_type = signature.return_type;
    // ADR 0014 D5: push the declared return type down into the body
    // so NullLit / T→?T widening at the tail can resolve against it.
    // But only push if the return type is nullable — otherwise we'd
    // pre-empt the more specific ReturnTypeMismatch diagnostic with
    // a less-specific Mismatch on the tail's span.
    let body_expected = if return_type.is_nullable() {
        Some(return_type)
    } else {
        None
    };
    let body = check_block(&fn_def.body, body_expected, &mut env, signatures, structs)?;

    if body.ty != return_type {
        return Err(TypeError::ReturnTypeMismatch {
            name: fn_def.name.clone(),
            expected: return_type,
            got: body.ty,
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

/// Per-fn type environment: VarId → Type. Inside a generic-fn
/// body the value type may be `Type::TypeParam(_)`; concrete
/// substitution happens at the caller.
type VarTypeEnv = std::collections::HashMap<VarId, Type>;

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

fn check_block(
    block: &ResolvedBlock,
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
) -> Result<TypedBlock, TypeError> {
    let mut stmts = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        stmts.push(check_stmt(stmt, env, signatures, structs)?);
    }
    // Only the tail receives the expected-type pushdown (the block's
    // value is the tail's value).
    let tail = check_expr(&block.tail, expected, env, signatures, structs)?;
    let ty = tail.ty;
    Ok(TypedBlock {
        stmts,
        tail,
        span: block.span.clone(),
        ty,
    })
}

fn check_stmt(
    stmt: &ResolvedStmt,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
) -> Result<TypedStmt, TypeError> {
    let kind = match &stmt.kind {
        ResolvedStmtKind::Let { id, name, name_span, ty_annot, value } => {
            // ADR 0014 D5: if the let has a type annotation, push it
            // down into the RHS as the expected type. This is what
            // makes `let x: ?i64 = null;` typecheck.
            let expected = match ty_annot {
                Some(annot) => {
                    let struct_table = struct_name_table_local(structs);
                    Some(resolve_type_expr(annot, &struct_table)?)
                }
                None => None,
            };
            let value_typed = check_expr(value, expected, env, signatures, structs)?;
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
            env.insert(*id, ty);
            TypedStmtKind::Let {
                id: *id,
                name: name.clone(),
                name_span: name_span.clone(),
                ty,
                value: value_typed,
            }
        }
        ResolvedStmtKind::Expr(e) => {
            TypedStmtKind::Expr(check_expr(e, None, env, signatures, structs)?)
        }
    };
    Ok(TypedStmt { kind, span: stmt.span.clone() })
}

/// Build a local struct-name table from the typed struct list.
/// Used by [`check_stmt`] when resolving let-binding annotations
/// (which arrive as TypeExprs at C1.4).
fn struct_name_table_local(structs: &[TypedStructDecl]) -> HashMap<String, StructId> {
    structs.iter().map(|s| (s.name.clone(), s.id)).collect()
}

/// Apply ADR 0014 D3 widening / D2 null-literal context to a typed
/// expression against an expected type. If `expected` is None, the
/// expression's synthesized type passes through unchanged. If it's
/// `Some(?T)` and the synth type is `T`, wrap with WidenToNullable.
/// Mismatches surface as `TypeError::Mismatch`.
fn coerce_to_expected(
    synth: TypedExpr,
    expected: Option<Type>,
    span_for_mismatch: &Span,
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
    Err(TypeError::Mismatch {
        expected: exp,
        got: synth.ty,
        span: to_source_span(span_for_mismatch),
    })
}

/// Try to substitute every [`Type::TypeParam`] in `ty` using the
/// partial substitution map. Returns `Some(concrete)` iff every
/// TypeParam encountered has been bound; `None` otherwise. Used by
/// [`check_call`] to determine whether a generic param's type is
/// fully concretized enough to push down to its arg as bidirectional
/// expected-type.
fn try_substitute(ty: Type, subst: &[Option<Type>]) -> Option<Type> {
    match ty {
        Type::I64 | Type::I32 | Type::Bool | Type::Struct(_) => Some(ty),
        Type::TypeParam(id) => subst.get(id.0 as usize).copied().flatten(),
        Type::Nullable(ni) => {
            let inner = try_substitute(ni.to_type(), subst)?;
            inner.to_nullable_inner().map(Type::Nullable)
        }
        Type::Array(ae) => {
            let inner = try_substitute(ae.to_type(), subst)?;
            inner.to_array_elem().map(Type::Array)
        }
    }
}

/// `true` iff `ty` mentions at least one [`Type::TypeParam`].
fn contains_type_param(ty: Type) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::Nullable(ni) => contains_type_param(ni.to_type()),
        Type::Array(ae) => contains_type_param(ae.to_type()),
        Type::I64 | Type::I32 | Type::Bool | Type::Struct(_) => false,
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
            unify_one(p.to_type(), a.to_type(), subst)
        }
        (Type::Array(p), Type::Array(a)) => {
            unify_one(p.to_type(), a.to_type(), subst)
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
        let _ = unify_one(signature.return_type, exp, &mut subst);
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
            let concrete_param = if contains_type_param(param) {
                try_substitute(param, &subst)
            } else {
                Some(param)
            };
            let arg_expected: Option<Type> = match concrete_param {
                Some(c) if c.is_nullable() => Some(c),
                _ => None,
            };
            // Null literals require a concrete expected — skip
            // them until enough subst is built up to push one down.
            if matches!(args[i].kind, ResolvedExprKind::NullLit)
                && arg_expected.is_none()
            {
                continue;
            }
            let typed = check_expr(&args[i], arg_expected, env, signatures, structs)?;
            // Validate the arg's type against the param (possibly
            // refining subst with any new TypeParam bindings).
            match unify_one(param, typed.ty, &mut subst) {
                Ok(()) => {}
                Err(UnifyFailure::Mismatch(expected, got)) => {
                    // If the failing param has unbound TypeParams,
                    // surface as `Mismatch` (matches the pre-C1.7
                    // generic-builtin error shape — e.g.,
                    // `unwrap_or(5, 0)` says "expected ?T, got i64").
                    // If the param fully substitutes to a concrete
                    // shape, the more specific `CallArgMismatch`
                    // pinpoints the arg position by callee + index.
                    return match try_substitute(expected, &subst) {
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

    // Compute the substituted return type.
    let ret_ty = signature.return_type.substitute(&concrete_type_args);

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

fn check_expr(
    expr: &ResolvedExpr,
    expected: Option<Type>,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
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
        ResolvedExprKind::Var(id) => {
            let ty = *env
                .get(id)
                .expect("resolve guarantees VarId is bound in the current scope");
            (TypedExprKind::Var(*id), ty)
        }
        ResolvedExprKind::Unary(op, inner) => {
            let inner_t = check_expr(inner, None, env, signatures, structs)?;
            // C1.3: `-x` requires int; `!x` requires bool.
            let ty = match op {
                UnaryOp::Neg => {
                    if !inner_t.ty.is_int() {
                        return Err(TypeError::Mismatch {
                            expected: Type::I64,
                            got: inner_t.ty,
                            span: to_source_span(&inner.span),
                        });
                    }
                    inner_t.ty
                }
                UnaryOp::Not => {
                    if inner_t.ty != Type::Bool {
                        return Err(TypeError::Mismatch {
                            expected: Type::Bool,
                            got: inner_t.ty,
                            span: to_source_span(&inner.span),
                        });
                    }
                    Type::Bool
                }
            };
            (TypedExprKind::Unary(*op, Box::new(inner_t)), ty)
        }
        ResolvedExprKind::Binary(op, lhs, rhs) => {
            let l = check_expr(lhs, None, env, signatures, structs)?;
            let r = check_expr(rhs, None, env, signatures, structs)?;
            // C1.3: arithmetic requires both operands the same int
            // type (I32 or I64); result is that int type. Bool /
            // struct arithmetic is rejected.
            if !l.ty.is_int() {
                return Err(TypeError::Mismatch {
                    expected: Type::I64,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            if l.ty != r.ty {
                return Err(TypeError::Mismatch {
                    expected: l.ty,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            let _ = op; // arithmetic dispatch is codegen's concern
            let ty = l.ty;
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
                    let r = check_expr(rhs, None, env, signatures, structs)?;
                    (r.ty, r, lhs.span.clone())
                } else {
                    let l = check_expr(lhs, None, env, signatures, structs)?;
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
            let l = check_expr(lhs, None, env, signatures, structs)?;
            let r = check_expr(rhs, None, env, signatures, structs)?;
            // C1.3: comparisons require both operands the same type.
            // C1.4 + C1.5 keep this as int + bool only (ADR 0013 D6
            // defers struct equality; nullable-vs-nullable equality
            // also deferred). Reject struct + nullable operands when
            // neither side is null.
            if l.ty.is_struct() || l.ty.is_nullable() {
                return Err(TypeError::Mismatch {
                    expected: Type::I64,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            if l.ty != r.ty {
                return Err(TypeError::Mismatch {
                    expected: l.ty,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            (
                TypedExprKind::Cmp(*op, Box::new(l), Box::new(r)),
                Type::Bool,
            )
        }
        ResolvedExprKind::Logic(op, lhs, rhs) => {
            let l = check_expr(lhs, None, env, signatures, structs)?;
            let r = check_expr(rhs, None, env, signatures, structs)?;
            if l.ty != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: l.ty,
                    span: to_source_span(&lhs.span),
                });
            }
            if r.ty != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: r.ty,
                    span: to_source_span(&rhs.span),
                });
            }
            (
                TypedExprKind::Logic(*op, Box::new(l), Box::new(r)),
                Type::Bool,
            )
        }
        ResolvedExprKind::Block(b) => {
            let typed_block = check_block(b, expected, env, signatures, structs)?;
            let ty = typed_block.ty;
            (TypedExprKind::Block(Box::new(typed_block)), ty)
        }
        ResolvedExprKind::If { cond, then_branch, else_branch } => {
            let cond_t = check_expr(cond, None, env, signatures, structs)?;
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
            let then_t = check_block(then_branch, expected, env, signatures, structs)?;
            let else_t = check_block(else_branch, expected, env, signatures, structs)?;
            if then_t.ty != else_t.ty {
                return Err(TypeError::Mismatch {
                    expected: then_t.ty,
                    got: else_t.ty,
                    span: to_source_span(&else_branch.span),
                });
            }
            let ty = then_t.ty;
            (
                TypedExprKind::If {
                    cond: Box::new(cond_t),
                    then_branch: Box::new(then_t),
                    else_branch: Box::new(else_t),
                },
                ty,
            )
        }
        ResolvedExprKind::Call { id, callee_span, args } => {
            check_call(*id, callee_span, args, expected, env, signatures, structs, &expr.span)?
        }
        ResolvedExprKind::StructLit { id, name, name_span, fields } => {
            // The struct decl provides the expected field set + types.
            let decl = &structs[id.0 as usize];

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
                let expected_field_ty = decl.fields[decl_idx].ty;
                // ADR 0014 D5: push the field's expected type down so
                // `null` / widening work inside struct literals.
                let value_t = check_expr(
                    &fi.value,
                    Some(expected_field_ty),
                    env,
                    signatures,
                    structs,
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

            // Validate every declared field was provided. (Duplicate
            // detection is implicit — a duplicate field name in the
            // literal would lead to two provided entries with the
            // same decl_idx, both pointing at the same expected type
            // and both being type-checked; codegen would emit two
            // stores to the same slot. C1.4 accepts this without a
            // diagnostic — Rust does too. A future ADR may revisit.)
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

            // Reorder to declaration order so codegen can iterate
            // by index without consulting field names.
            provided.sort_by_key(|(idx, _)| *idx);
            // After sort, the index sequence may have gaps if a
            // field was provided multiple times; for C1.4 we just
            // take the last value at each index.
            let mut by_index: Vec<Option<TypedExpr>> = vec![None; decl.fields.len()];
            for (idx, val) in provided {
                by_index[idx] = Some(val);
            }
            let ordered: Vec<TypedExpr> = by_index
                .into_iter()
                .map(|opt| opt.expect("every field was provided"))
                .collect();

            (
                TypedExprKind::StructLit {
                    id: *id,
                    name: name.clone(),
                    name_span: name_span.clone(),
                    fields: ordered,
                },
                Type::Struct(*id),
            )
        }
        ResolvedExprKind::ArrayLit(elems) => {
            // ADR 0015 D2 + D7: each element checked against the
            // expected element type (from `[T]` context) if present.
            // If no expected, infer T from the first element.
            let expected_elem: Option<Type> = match expected {
                Some(Type::Array(elem)) => Some(elem.to_type()),
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
                let ae = elem_ty.to_array_elem().ok_or_else(|| {
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
                let first = check_expr(&elems[0], expected_elem, env, signatures, structs)?;
                let elem_ty = first.ty;
                let mut typed = Vec::with_capacity(elems.len());
                typed.push(first);
                for e in &elems[1..] {
                    let t = check_expr(e, Some(elem_ty), env, signatures, structs)?;
                    if t.ty != elem_ty {
                        return Err(TypeError::Mismatch {
                            expected: elem_ty,
                            got: t.ty,
                            span: to_source_span(&e.span),
                        });
                    }
                    typed.push(t);
                }
                let ae = elem_ty.to_array_elem().ok_or_else(|| {
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
            let target_t = check_expr(target, None, env, signatures, structs)?;
            let elem_ty = match target_t.ty {
                Type::Array(ae) => ae,
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
            let index_t = check_expr(index, None, env, signatures, structs)?;
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
                elem_ty.to_type(),
            )
        }
        ResolvedExprKind::FieldAccess { target, field, field_span } => {
            let target_t = check_expr(target, None, env, signatures, structs)?;
            let struct_id = match target_t.ty {
                Type::Struct(id) => id,
                other => {
                    return Err(TypeError::FieldAccessOnNonStruct {
                        got: other,
                        span: to_source_span(&target.span),
                    });
                }
            };
            let decl = &structs[struct_id.0 as usize];
            let (field_index, field_ty) = decl
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
    };
    let synth = TypedExpr { kind, span: expr.span.clone(), ty };
    // ADR 0014 D3: apply T→?T widening if the expected type is ?T and
    // the synthesized type is T.
    coerce_to_expected(synth, expected, &expr.span)
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
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
        TypeError::GenericStructNotYetSupported { name, span } => (
            "sentinel::types::generic_struct_not_yet_supported",
            format!("generic struct types like `{name}<...>` are not yet supported at C1.7.4a"),
            span.offset()..(span.offset() + span.len()),
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

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-types");
    }

    // ----- positive paths -----

    #[test]
    fn checks_minimal_main() {
        let p = check_ok("fn main() -> i64 { 0 }");
        assert_eq!(p.fns.len(), 1);
        let main = p.main();
        assert_eq!(main.return_type, Type::I64);
        assert_eq!(main.body.ty, Type::I64);
        // Signature table at C1.6: FnId(0)=print, (1)=unwrap_or,
        // (2)=is_some, (3)=len, (4)=main. The generic builtins
        // occupy FnId(1..=3) per ADR 0014 D9 + ADR 0015 D4.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[0].param_types, vec![Type::I64]);
        assert_eq!(p.fn_signatures[1].name, "unwrap_or");
        assert_eq!(p.fn_signatures[2].name, "is_some");
        assert_eq!(p.fn_signatures[3].name, "len");
        assert_eq!(p.fn_signatures[4].name, "main");
        assert!(p.signature(main.id).is_main);
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
    fn nested_array_type_errors() {
        let err = check_err("fn f(x: [[i64]]) -> i64 { 0 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, TypeError::NestedArray { .. }), "got {err:?}");
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
    fn c17_generic_struct_rejected_at_c17_4a() {
        let err = check_err(
            "struct Box<T> { value: T }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::GenericStructNotYetSupported { ref name, .. } if name == "Box"),
            "got {err:?}"
        );
    }

    #[test]
    fn c17_generic_struct_type_in_signature_rejected_at_c17_4a() {
        // `Box<i64>` in type position — even without a `struct Box`
        // decl, the parser produces TypeExprKind::Generic which
        // surfaces as the not-yet diagnostic.
        let err = check_err(
            "fn unbox(b: Box<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        );
        assert!(
            matches!(err, TypeError::GenericStructNotYetSupported { ref name, .. } if name == "Box"),
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
}
