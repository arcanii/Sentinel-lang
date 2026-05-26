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
    ResolvedStmt, ResolvedStmtKind, StructId, VarId,
};

// =============================================================================
// Type universe
// =============================================================================

/// Sentinel's type universe. C1.2 shipped only `I64`; C1.3 (per ADR
/// 0012 D5-D8) widened to `I32` and `Bool`; C1.4 (per ADR 0013 D4)
/// adds user-defined struct types tagged by [`StructId`]. Nominal
/// equality — two structs with identical field shapes are distinct
/// types per ADR 0013 D5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    I32,
    Bool,
    Struct(StructId),
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
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::I64 => write!(f, "i64"),
            Type::I32 => write!(f, "i32"),
            Type::Bool => write!(f, "bool"),
            Type::Struct(id) => write!(f, "<struct#{}>", id.0),
        }
    }
}

/// Resolve a surface-level [`TypeExpr`] to a concrete [`Type`].
/// C1.2 recognised only `"i64"`; C1.3 (per ADR 0012 D3 + D5) added
/// `"i32"` and `"bool"`; C1.4 (per ADR 0013 D4) extends to look up
/// user-defined struct names against the struct table. Anything not
/// matching surfaces as [`TypeError::UnknownType`].
fn resolve_type_expr(
    te: &TypeExpr,
    struct_table: &HashMap<String, StructId>,
) -> Result<Type, TypeError> {
    match &te.kind {
        TypeExprKind::Ident(name) => match name.as_str() {
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
        },
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
/// [`TypeExpr`] has been resolved to a concrete [`Type`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedStructDecl {
    pub id: StructId,
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<TypedStructField>,
    pub span: Span,
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
    /// transitively) with no indirection. Lifts when `?T` arrives
    /// at C1.5.
    #[error("recursive struct `{name}` has no representable size")]
    #[diagnostic(
        code(sentinel::types::recursive_struct),
        help("recursive structs need indirection — wait for C1.5's `?T` nullable form")
    )]
    RecursiveStruct {
        name: String,
        /// Names of the structs in the cycle, in order.
        cycle: Vec<String>,
        #[label("recursive struct cycle")]
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
        let mut fields = Vec::with_capacity(sd.fields.len());
        for f in &sd.fields {
            let ty = resolve_type_expr(&f.ty, &struct_table)?;
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
            fields,
            span: sd.span.clone(),
        });
    }

    // Pass 2: cycle detection.
    detect_struct_cycle(&typed_structs)?;

    // Pass 3: fn signatures.
    let mut typed_signatures: Vec<TypedFnSignature> =
        Vec::with_capacity(program.fn_signatures.len());

    // Index 0: runtime print(i64) -> i64.
    let print_sig = &program.fn_signatures[0];
    typed_signatures.push(TypedFnSignature {
        id: print_sig.id,
        name: print_sig.name.clone(),
        name_span: print_sig.name_span.clone(),
        param_types: vec![Type::I64],
        return_type: Type::I64,
        is_main: false,
        is_runtime: true,
    });

    for fn_def in &program.fns {
        let resolved_sig = &program.fn_signatures[fn_def.id.0 as usize];
        let mut param_types = Vec::with_capacity(fn_def.params.len());
        for param in &fn_def.params {
            param_types.push(resolve_type_expr(&param.ty, &struct_table)?);
        }
        let return_type = resolve_type_expr(&fn_def.return_type, &struct_table)?;
        typed_signatures.push(TypedFnSignature {
            id: fn_def.id,
            name: resolved_sig.name.clone(),
            name_span: resolved_sig.name_span.clone(),
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

/// Walk the struct-field graph looking for cycles. Returns
/// [`TypeError::RecursiveStruct`] on the first cycle found. C1.5's
/// `?T` lifts this restriction; at C1.4 a cycle means no
/// representable size.
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

    let body = check_block(&fn_def.body, &mut env, signatures, structs)?;
    let return_type = signature.return_type;

    if body.ty != return_type {
        return Err(TypeError::ReturnTypeMismatch {
            name: fn_def.name.clone(),
            expected: return_type,
            got: body.ty,
            span: to_source_span(&fn_def.body.tail.span),
        });
    }

    Ok(TypedFnDef {
        id: fn_def.id,
        name: fn_def.name.clone(),
        name_span: fn_def.name_span.clone(),
        params,
        return_type,
        body,
        span: fn_def.span.clone(),
    })
}

/// Per-fn type environment: VarId → Type.
type VarTypeEnv = std::collections::HashMap<VarId, Type>;

fn check_block(
    block: &ResolvedBlock,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
) -> Result<TypedBlock, TypeError> {
    let mut stmts = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        stmts.push(check_stmt(stmt, env, signatures, structs)?);
    }
    let tail = check_expr(&block.tail, env, signatures, structs)?;
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
            let value_typed = check_expr(value, env, signatures, structs)?;
            let ty = match ty_annot {
                Some(annot) => {
                    let struct_table = struct_name_table_local(structs);
                    let annotated = resolve_type_expr(annot, &struct_table)?;
                    if annotated != value_typed.ty {
                        return Err(TypeError::Mismatch {
                            expected: annotated,
                            got: value_typed.ty,
                            span: to_source_span(&value.span),
                        });
                    }
                    annotated
                }
                None => value_typed.ty,
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
            TypedStmtKind::Expr(check_expr(e, env, signatures, structs)?)
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

fn check_expr(
    expr: &ResolvedExpr,
    env: &mut VarTypeEnv,
    signatures: &[TypedFnSignature],
    structs: &[TypedStructDecl],
) -> Result<TypedExpr, TypeError> {
    let (kind, ty) = match &expr.kind {
        ResolvedExprKind::IntLit(n) => (TypedExprKind::IntLit(*n), Type::I64),
        ResolvedExprKind::BoolLit(b) => (TypedExprKind::BoolLit(*b), Type::Bool),
        ResolvedExprKind::Var(id) => {
            let ty = *env
                .get(id)
                .expect("resolve guarantees VarId is bound in the current scope");
            (TypedExprKind::Var(*id), ty)
        }
        ResolvedExprKind::Unary(op, inner) => {
            let inner_t = check_expr(inner, env, signatures, structs)?;
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
            let l = check_expr(lhs, env, signatures, structs)?;
            let r = check_expr(rhs, env, signatures, structs)?;
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
            let l = check_expr(lhs, env, signatures, structs)?;
            let r = check_expr(rhs, env, signatures, structs)?;
            // C1.3: comparisons require both operands the same type.
            // C1.4 keeps this as int + bool only (ADR 0013 D6 defers
            // struct equality to C1.5+) — reject struct operands.
            if l.ty.is_struct() {
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
            let l = check_expr(lhs, env, signatures, structs)?;
            let r = check_expr(rhs, env, signatures, structs)?;
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
            let typed_block = check_block(b, env, signatures, structs)?;
            let ty = typed_block.ty;
            (TypedExprKind::Block(Box::new(typed_block)), ty)
        }
        ResolvedExprKind::If { cond, then_branch, else_branch } => {
            let cond_t = check_expr(cond, env, signatures, structs)?;
            // C1.3 step 5: if-condition must be bool.
            if cond_t.ty != Type::Bool {
                return Err(TypeError::Mismatch {
                    expected: Type::Bool,
                    got: cond_t.ty,
                    span: to_source_span(&cond.span),
                });
            }
            let then_t = check_block(then_branch, env, signatures, structs)?;
            let else_t = check_block(else_branch, env, signatures, structs)?;
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
            let signature = &signatures[id.0 as usize];
            let mut typed_args = Vec::with_capacity(args.len());
            for (i, arg) in args.iter().enumerate() {
                let typed_arg = check_expr(arg, env, signatures, structs)?;
                let expected = signature.param_types[i];
                if typed_arg.ty != expected {
                    return Err(TypeError::CallArgMismatch {
                        callee: signature.name.clone(),
                        arg_index: i,
                        expected,
                        got: typed_arg.ty,
                        span: to_source_span(&arg.span),
                    });
                }
                typed_args.push(typed_arg);
            }
            let ty = signature.return_type;
            (
                TypedExprKind::Call {
                    id: *id,
                    callee_span: callee_span.clone(),
                    args: typed_args,
                },
                ty,
            )
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
                let expected = decl.fields[decl_idx].ty;
                let value_t = check_expr(&fi.value, env, signatures, structs)?;
                if value_t.ty != expected {
                    return Err(TypeError::Mismatch {
                        expected,
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
        ResolvedExprKind::FieldAccess { target, field, field_span } => {
            let target_t = check_expr(target, env, signatures, structs)?;
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
    Ok(TypedExpr { kind, span: expr.span.clone(), ty })
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
        // Signature table: FnId(0) is print, FnId(1) is main.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[0].param_types, vec![Type::I64]);
        assert_eq!(p.fn_signatures[1].name, "main");
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
}
