//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. Takes a [`TypedProgram`] (the
//! output of `sentinel-types::check`) and produces a native
//! object file. **Pass 0** declares every user struct as an LLVM
//! struct type (so fn signatures + struct literals can reference
//! them); **pass 1** declares every fn (including the runtime
//! `print` → `sentinel_print` mapping) so forward references work;
//! **pass 2** emits each fn body.
//!
//! `main` is required and is given an `i32` return type (the C ABI
//! `main` signature); other fns return their declared type. The
//! exit code is the i32-truncated i64 value of `main`'s tail
//! expression. Programs additionally produce stdout via `print(x)`
//! calls.
//!
//! **C1.1.2** moved name resolution out of this crate into
//! `sentinel-resolve`. **C1.2.4** swapped the input shape from
//! `ResolvedProgram` to `TypedProgram`. **C1.3** activated the
//! `Type` field: lowering picks between `i1` (bool), `i32`, and
//! `i64` storage based on the expression's typed annotation.
//! **C1.4** widens the codegen value type from `IntValue<'ctx>` to
//! `BasicValueEnum<'ctx>` so struct values can flow through the
//! same machinery. Struct literals lower via `build_insert_value`
//! chains starting from `undef`; field access lowers via
//! `build_extract_value`. Pass-by-value through fn args is
//! transparent — LLVM's ABI lowering handles the small-struct vs
//! by-pointer choice.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::{IntPredicate, OptimizationLevel};
use sentinel_ast::{BinOp, CmpOp, LogicOp, UnaryOp};
use sentinel_resolve::{
    FnId, StructId, VarId, IS_SOME_FN_ID, LEN_FN_ID, UNWRAP_OR_FN_ID,
};
use sentinel_types::{
    ArrayElem, NullableInner, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedProgram,
    TypedStmt, TypedStmtKind,
};

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum CodegenError {
    #[error("LLVM IR verification failed: {0}")]
    #[diagnostic(code(sentinel::codegen::verify_failed))]
    VerifyFailed(String),

    #[error("LLVM target initialization failed: {0}")]
    #[diagnostic(code(sentinel::codegen::target_init))]
    TargetInit(String),

    #[error("LLVM target machine creation failed")]
    #[diagnostic(code(sentinel::codegen::target_machine))]
    TargetMachine,

    #[error("failed to write object file `{path}`: {message}")]
    #[diagnostic(code(sentinel::codegen::write_failed))]
    WriteFailed { path: String, message: String },

    #[error("LLVM builder error: {0}")]
    #[diagnostic(code(sentinel::codegen::builder))]
    Builder(String),

    /// C1.7.4a: calls to user-defined generic fns can't be lowered
    /// until monomorphization arrives at C1.7.5. Generic builtins
    /// (`unwrap_or`, `is_some`, `len`) keep their existing special-
    /// case lowering per ADR 0016 D8b.
    #[error("calls to user-defined generic fn `{name}` are not yet supported at C1.7.4a")]
    #[diagnostic(
        code(sentinel::codegen::generic_call_not_yet_supported),
        help("generic-fn monomorphization arrives at C1.7.5; for now only the builtin generics (unwrap_or, is_some, len) lower")
    )]
    GenericCallNotYetSupported { name: String },
}

/// Lower a [`TypedProgram`] to a native object file at `output`.
/// The emitted module exports `main` (i32-returning, the C ABI
/// entry); the runtime symbol `sentinel_print` is pre-declared in
/// pass 1.
pub fn compile_to_object(program: &TypedProgram, output: &Path) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();

    // Pass 0: declare every user struct as an LLVM struct type. The
    // typed program's struct list is in StructId order; we mirror
    // that order so the StructId.0 indexes the struct_types vec.
    //
    // Two-step to handle forward references via `?T`-style nullables
    // (C1.5): first declare all opaque struct types, then set their
    // bodies. Without this, `struct Node { next: ?Node }` would
    // panic when llvm_basic_type tries to look up Node before it's
    // inserted.
    let mut struct_types: HashMap<StructId, StructType> = HashMap::new();
    for sd in &program.structs {
        let st = context.opaque_struct_type(&sd.name);
        struct_types.insert(sd.id, st);
    }
    for sd in &program.structs {
        let field_tys: Vec<BasicTypeEnum> = sd
            .fields
            .iter()
            .map(|f| llvm_basic_type(&context, f.ty, &struct_types))
            .collect();
        let st = struct_types[&sd.id];
        st.set_body(&field_tys, false);
    }

    // Pass 1: declare every function in the typed program's
    // signature table. Signatures are indexed by FnId.0; FnId(0) is
    // always `print` (the actual runtime symbol). FnId(1)/(2)/(3)
    // are the generic builtins `unwrap_or` / `is_some` / `len`
    // which codegen INLINES — no LLVM symbol needed for them. So
    // we only emit an LLVM declaration for the real runtime fn
    // (print) and the user fns. The inlined builtins still occupy
    // FnId slots so the fns HashMap entries are populated with the
    // print function value as a placeholder (never read; defensive).
    let mut fns: HashMap<FnId, FunctionValue> = HashMap::new();
    let print_fn_value = {
        let print_sig = &program.fn_signatures[0];
        debug_assert!(print_sig.is_runtime && print_sig.name == "print");
        let param_types: Vec<_> = print_sig
            .param_types
            .iter()
            .map(|t| llvm_basic_type(&context, *t, &struct_types).into())
            .collect();
        let fn_type = llvm_basic_type(&context, print_sig.return_type, &struct_types)
            .fn_type(&param_types, false);
        module.add_function("sentinel_print", fn_type, None)
    };
    fns.insert(FnId(0), print_fn_value);
    // The other inline builtins map to print_fn_value as a dummy —
    // codegen never reads these entries.
    for signature in program.fn_signatures.iter().skip(1) {
        if signature.is_runtime {
            fns.insert(signature.id, print_fn_value);
            continue;
        }
        // C1.7.4a / ADR 0016 D7: skip declaring user-defined
        // generic fns. Their param / return types contain
        // `Type::TypeParam` which has no LLVM representation;
        // monomorphic instances will be emitted at call sites
        // when C1.7.5 lands. Insert a dummy mapping so any
        // accidental call-site lookup gets a clear (and unused)
        // FunctionValue — actual call lowering catches the
        // generic case via `GenericCallNotYetSupported` below.
        if !signature.type_params.is_empty() {
            fns.insert(signature.id, print_fn_value);
            continue;
        }
        let param_types: Vec<_> = signature
            .param_types
            .iter()
            .map(|t| llvm_basic_type(&context, *t, &struct_types).into())
            .collect();
        let fn_type = if signature.is_main {
            i32_type.fn_type(&param_types, false)
        } else {
            llvm_basic_type(&context, signature.return_type, &struct_types)
                .fn_type(&param_types, false)
        };
        let fn_value = module.add_function(&signature.name, fn_type, None);
        fns.insert(signature.id, fn_value);
    }

    // C1.6 / ADR 0015 D9: declare the heap runtime symbols. These
    // are external; sentinel-runtime supplies the implementations.
    let alloc_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_alloc",
            ptr_ty.fn_type(&[i64_ty.into()], false),
            None,
        )
    };
    let panic_oob_fn = {
        let i64_ty = context.i64_type();
        let void_ty = context.void_type();
        let panic_type = void_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function("sentinel_panic_oob", panic_type, None)
    };

    // C1.7.5 / ADR 0016 D7: collect generic-fn instantiations
    // reachable from non-generic fn bodies (the seed) and any
    // transitive calls discovered while walking already-queued
    // instances. Each unique `(FnId, Vec<Type>)` becomes one
    // monomorphic LLVM function.
    let instantiations = collect_mono_instantiations(program);
    // Materialise each instance as a substituted TypedFnDef so
    // pass 2's body lowering can consume it without TypeParam-
    // awareness. The substituted defs are owned here and held
    // alive across pass 2.
    let mono_defs: Vec<((FnId, Vec<Type>), TypedFnDef)> = instantiations
        .iter()
        .map(|(fn_id, args)| {
            let generic_def = program
                .fns
                .iter()
                .find(|f| f.id == *fn_id)
                .expect("collect_mono_instantiations only queues real user-defined fns");
            ((*fn_id, args.clone()), generic_def.substitute(args))
        })
        .collect();
    // Pre-declare each monomorphic LLVM fn with a mangled name so
    // that within pass 2 we can resolve `(FnId, Vec<Type>)` →
    // FunctionValue at every call site (including transitive
    // generic-fn-to-generic-fn calls).
    let mut mono_fns: HashMap<(FnId, Vec<Type>), FunctionValue> = HashMap::new();
    for ((fn_id, args), def) in &mono_defs {
        let mangled = mangle_mono_name(&def.name, args, program);
        let param_types: Vec<_> = def
            .params
            .iter()
            .map(|p| llvm_basic_type(&context, p.ty, &struct_types).into())
            .collect();
        let fn_type = llvm_basic_type(&context, def.return_type, &struct_types)
            .fn_type(&param_types, false);
        let fn_value = module.add_function(&mangled, fn_type, None);
        mono_fns.insert((*fn_id, args.clone()), fn_value);
    }

    // Pass 2: emit each user function body. (The runtime `print`
    // has no body — it's defined externally by sentinel-runtime.)
    {
        let mut cx = CodegenCtx {
            context: &context,
            builder,
            fns,
            mono_fns,
            struct_types,
            alloc_fn,
            panic_oob_fn,
            current_fn: None,
            vars: HashMap::new(),
        };
        for fn_def in &program.fns {
            // C1.7.4a / ADR 0016 D7: skip generic fn bodies; their
            // monomorphic images are emitted below.
            if !fn_def.type_params.is_empty() {
                continue;
            }
            cx.compile_fn(fn_def, program)?;
        }
        // C1.7.5: emit each monomorphic instance. The substituted
        // `TypedFnDef` carries concrete types; compile_fn lowers
        // it identically to a non-generic fn.
        for ((fn_id, args), def) in &mono_defs {
            cx.compile_mono_fn(*fn_id, args, def, program)?;
        }
    }

    module
        .verify()
        .map_err(|e| CodegenError::VerifyFailed(e.to_string()))?;

    Target::initialize_native(&InitializationConfig::default())
        .map_err(CodegenError::TargetInit)?;

    let triple = TargetMachine::get_default_triple();
    let target =
        Target::from_triple(&triple).map_err(|e| CodegenError::TargetInit(e.to_string()))?;
    let cpu = TargetMachine::get_host_cpu_name().to_string();
    let features = TargetMachine::get_host_cpu_features().to_string();
    let target_machine = target
        .create_target_machine(
            &triple,
            &cpu,
            &features,
            OptimizationLevel::None,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or(CodegenError::TargetMachine)?;

    target_machine
        .write_to_file(&module, FileType::Object, output)
        .map_err(|e| CodegenError::WriteFailed {
            path: output.display().to_string(),
            message: e.to_string(),
        })?;

    Ok(())
}

/// Per-function codegen state. See C1.1.2 docs in commit 9374edf
/// for the lifetime / dropping rationale. C1.3 stores both the
/// alloca pointer AND its [`Type`] per binding so `build_load` can
/// pick the right element type. C1.4 adds the struct-type cache
/// (StructId → LLVM StructType) — struct types are declared in
/// pass 0 before either fn signatures or fn bodies. The `'ctx`
/// lifetime covers both the borrowed Context and the LLVM derived
/// values (Builder, FunctionValue, etc.) — they all live and die
/// together.
struct CodegenCtx<'ctx> {
    context: &'ctx Context,
    builder: Builder<'ctx>,
    fns: HashMap<FnId, FunctionValue<'ctx>>,
    /// C1.7.5 / ADR 0016 D7: monomorphic instances of generic user
    /// fns, keyed by `(FnId, type_args)`. Each entry's
    /// FunctionValue has a mangled LLVM name and a substituted
    /// signature. Call-site lowering consults this map when the
    /// callee is a user-defined generic fn.
    mono_fns: HashMap<(FnId, Vec<Type>), FunctionValue<'ctx>>,
    struct_types: HashMap<StructId, StructType<'ctx>>,
    /// C1.6: `sentinel_alloc(i64) -> ptr` runtime function. Called
    /// to back array storage and `?Struct` heap payloads.
    alloc_fn: FunctionValue<'ctx>,
    /// C1.6: `sentinel_panic_oob(i64 idx, i64 len) -> void` runtime
    /// function. Called from the bounds-check failure block.
    panic_oob_fn: FunctionValue<'ctx>,
    current_fn: Option<FunctionValue<'ctx>>,
    vars: HashMap<VarId, (PointerValue<'ctx>, Type)>,
}

/// Map a Sentinel [`Type`] to its LLVM `BasicTypeEnum`. C1.5
/// shipped the universe `{ I64, I32, Bool, Struct, Nullable }` with
/// `?T` as flat `{ i1, T }`. C1.6 / ADR 0015 D11 splits the
/// nullable lowering: `?primitive` stays flat inline, but
/// `?Struct(id)` becomes `{ i1 valid, ptr payload }` where the
/// payload is a heap-allocated `Struct(id)`. This breaks recursive
/// struct cycles (the cycle goes through a pointer-sized field
/// instead of an infinite-sized inline field).
///
/// C1.6 also adds `Type::Array(ArrayElem)` lowering to
/// `{ i64 len, ptr data }` per ADR 0015 D1.
fn llvm_basic_type<'ctx>(
    context: &'ctx Context,
    ty: Type,
    struct_types: &HashMap<StructId, StructType<'ctx>>,
) -> BasicTypeEnum<'ctx> {
    match ty {
        Type::Bool => context.bool_type().into(),
        Type::I32 => context.i32_type().into(),
        Type::I64 => context.i64_type().into(),
        Type::Struct(id) => (*struct_types
            .get(&id)
            .expect("struct declared in pass 0"))
        .into(),
        Type::Nullable(inner) => {
            let valid_ty: BasicTypeEnum = context.bool_type().into();
            let payload_ty: BasicTypeEnum = match inner {
                NullableInner::Struct(_) => {
                    // C1.6 / ADR 0015 D11: `?Struct` payload is a
                    // pointer to a heap-allocated struct.
                    context.ptr_type(inkwell::AddressSpace::default()).into()
                }
                _ => {
                    // `?primitive` stays inline { i1, T }.
                    llvm_basic_type(context, inner.to_type(), struct_types)
                }
            };
            context
                .struct_type(&[valid_ty, payload_ty], false)
                .into()
        }
        Type::Array(_) => {
            // `[T]` lowers as `{ i64 len, ptr data }` per ADR 0015
            // D1. The element type is tracked at the Sentinel-types
            // level (in TypedExprKind::ArrayLit / Index); LLVM uses
            // opaque pointers since LLVM 15.
            let len_ty: BasicTypeEnum = context.i64_type().into();
            let data_ty: BasicTypeEnum =
                context.ptr_type(inkwell::AddressSpace::default()).into();
            context.struct_type(&[len_ty, data_ty], false).into()
        }
        // C1.7 / ADR 0016 D7: TypeParams are abstract; codegen
        // requires substitution at the monomorphic instantiation
        // boundary. Reaching here is a codegen bug. Until C1.7.5
        // implements monomorphization, generic-fn bodies are not
        // emitted (see the skip in pass 0/1), so this branch is
        // unreachable for source-level programs.
        Type::TypeParam(_) => {
            panic!("llvm_basic_type called on Type::TypeParam — generic fn body must be monomorphised first per ADR 0016 D7")
        }
    }
}

// =============================================================================
// C1.7.5 monomorphization helpers per ADR 0016 D7.
// =============================================================================

/// Walk the [`TypedProgram`] starting from non-generic fn bodies
/// and collect every reachable `(FnId, Vec<Type>)` instantiation
/// of a user-defined generic fn. Transitive cases — a generic fn's
/// body calling another generic fn — are handled by repeatedly
/// processing newly-discovered instances with their type-arg
/// substitution applied. Returns instances in a deterministic
/// (insertion) order for stable LLVM output.
fn collect_mono_instantiations(
    program: &TypedProgram,
) -> Vec<(FnId, Vec<Type>)> {
    let mut visited: HashSet<(FnId, Vec<Type>)> = HashSet::new();
    let mut order: Vec<(FnId, Vec<Type>)> = Vec::new();
    let mut pending: Vec<(FnId, Vec<Type>)> = Vec::new();

    // Seed: scan non-generic fn bodies for calls to generic fns.
    let no_subst: Vec<Type> = Vec::new();
    for fn_def in &program.fns {
        if !fn_def.type_params.is_empty() {
            continue;
        }
        walk_block_for_mono(
            &fn_def.body,
            &no_subst,
            program,
            &mut visited,
            &mut order,
            &mut pending,
        );
    }

    // Worklist: each pending instance is itself a generic-fn body
    // we now walk under its concrete type-args.
    while let Some((fn_id, subst)) = pending.pop() {
        let generic_def = program
            .fns
            .iter()
            .find(|f| f.id == fn_id)
            .expect("collect: queued instance must reference an existing fn");
        walk_block_for_mono(
            &generic_def.body,
            &subst,
            program,
            &mut visited,
            &mut order,
            &mut pending,
        );
    }

    order
}

fn walk_block_for_mono(
    block: &TypedBlock,
    subst: &[Type],
    program: &TypedProgram,
    visited: &mut HashSet<(FnId, Vec<Type>)>,
    order: &mut Vec<(FnId, Vec<Type>)>,
    pending: &mut Vec<(FnId, Vec<Type>)>,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            TypedStmtKind::Let { value, .. } => {
                walk_expr_for_mono(value, subst, program, visited, order, pending)
            }
            TypedStmtKind::Expr(e) => {
                walk_expr_for_mono(e, subst, program, visited, order, pending)
            }
        }
    }
    walk_expr_for_mono(&block.tail, subst, program, visited, order, pending);
}

fn walk_expr_for_mono(
    e: &TypedExpr,
    subst: &[Type],
    program: &TypedProgram,
    visited: &mut HashSet<(FnId, Vec<Type>)>,
    order: &mut Vec<(FnId, Vec<Type>)>,
    pending: &mut Vec<(FnId, Vec<Type>)>,
) {
    match &e.kind {
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::Var(_) => {}
        TypedExprKind::WidenToNullable(inner) => {
            walk_expr_for_mono(inner, subst, program, visited, order, pending)
        }
        TypedExprKind::Unary(_, inner) => {
            walk_expr_for_mono(inner, subst, program, visited, order, pending)
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_expr_for_mono(l, subst, program, visited, order, pending);
            walk_expr_for_mono(r, subst, program, visited, order, pending);
        }
        TypedExprKind::Block(b) => {
            walk_block_for_mono(b, subst, program, visited, order, pending)
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_expr_for_mono(cond, subst, program, visited, order, pending);
            walk_block_for_mono(then_branch, subst, program, visited, order, pending);
            walk_block_for_mono(else_branch, subst, program, visited, order, pending);
        }
        TypedExprKind::Call { id, args, type_args, .. } => {
            // Substitute the call's type_args under the active
            // subst to get the concrete instantiation key.
            let concrete_args: Vec<Type> =
                type_args.iter().map(|t| t.substitute(subst)).collect();
            let signature = program.signature(*id);
            // Only enqueue user-defined generic fns. Builtins are
            // inlined; print and other runtime fns don't need
            // monomorphic copies.
            if !signature.is_runtime && !signature.type_params.is_empty() {
                let key = (*id, concrete_args);
                if visited.insert(key.clone()) {
                    order.push(key.clone());
                    pending.push(key);
                }
            }
            for a in args {
                walk_expr_for_mono(a, subst, program, visited, order, pending);
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                walk_expr_for_mono(f, subst, program, visited, order, pending);
            }
        }
        TypedExprKind::FieldAccess { target, .. } => {
            walk_expr_for_mono(target, subst, program, visited, order, pending)
        }
        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_expr_for_mono(el, subst, program, visited, order, pending);
            }
        }
        TypedExprKind::Index { target, index, .. } => {
            walk_expr_for_mono(target, subst, program, visited, order, pending);
            walk_expr_for_mono(index, subst, program, visited, order, pending);
        }
    }
}

/// C1.7.5 / ADR 0016 D7: produce a mangled LLVM symbol name for a
/// monomorphic instance of a generic fn. The scheme is
/// `{name}__{type1}_{type2}_...` where each type is rendered via
/// [`mangle_type`]. Stable across runs given the same input
/// program — handy for LLVM IR inspection.
fn mangle_mono_name(
    base_name: &str,
    type_args: &[Type],
    program: &TypedProgram,
) -> String {
    let mut s = String::with_capacity(base_name.len() + 16);
    s.push_str(base_name);
    for t in type_args {
        s.push_str("__");
        s.push_str(&mangle_type(*t, program));
    }
    s
}

/// Render a [`Type`] as a mangling-friendly tag. `i64` → `"i64"`,
/// `Pair<i64, bool>` (when generic structs land) → something like
/// `"Pair_i64_bool"`, etc. The format is internal-only — anyone
/// who wants a human display should use [`type_display`].
fn mangle_type(ty: Type, program: &TypedProgram) -> String {
    match ty {
        Type::I64 => "i64".to_string(),
        Type::I32 => "i32".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Struct(id) => program
            .structs
            .get(id.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("struct{}", id.0)),
        Type::Nullable(inner) => format!("opt_{}", mangle_type(inner.to_type(), program)),
        Type::Array(elem) => format!("arr_{}", mangle_type(elem.to_type(), program)),
        // TypeParams shouldn't appear in a monomorphic instance's
        // type-args by construction; if one slips through, render
        // it for debuggability.
        Type::TypeParam(id) => format!("T{}", id.0),
    }
}

/// Map a Sentinel int type (i1 / i32 / i64) to its LLVM IntType.
/// Panics on non-int types — callers must gate on
/// `Type::is_int()` first.
fn llvm_int_type<'ctx>(context: &'ctx Context, ty: Type) -> IntType<'ctx> {
    match ty {
        Type::Bool => context.bool_type(),
        Type::I32 => context.i32_type(),
        Type::I64 => context.i64_type(),
        Type::Struct(_) => panic!("llvm_int_type called on non-int Type::Struct"),
        Type::Nullable(_) => panic!("llvm_int_type called on non-int Type::Nullable"),
        Type::Array(_) => panic!("llvm_int_type called on non-int Type::Array"),
        // C1.7 / ADR 0016 D7: TypeParams must be substituted to a
        // concrete Type at the monomorphic instantiation boundary
        // before codegen sees them. Reaching here is a codegen bug
        // (e.g., trying to lower a generic fn body without an
        // active substitution).
        Type::TypeParam(_) => {
            panic!("llvm_int_type called on Type::TypeParam — generic fn body must be monomorphised first per ADR 0016 D7")
        }
    }
}

impl<'ctx> CodegenCtx<'ctx> {
    fn llvm_basic_type(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        llvm_basic_type(self.context, ty, &self.struct_types)
    }

    fn llvm_int_type(&self, ty: Type) -> IntType<'ctx> {
        llvm_int_type(self.context, ty)
    }

    /// C1.7.5 / ADR 0016 D7: emit a monomorphic instance of a
    /// generic fn. The `def` is the substituted [`TypedFnDef`]
    /// (with `Type::TypeParam` replaced by concrete types per
    /// `type_args`); `fn_id` and `type_args` are the keys into
    /// `mono_fns` where the pre-declared LLVM fn lives.
    ///
    /// Body lowering reuses [`compile_fn`]'s machinery — the
    /// substituted def looks no different from a non-generic fn
    /// to the per-fn lowering path. The only special case is
    /// resolving the `current_fn` against `mono_fns` rather than
    /// the regular `fns` table at the start.
    fn compile_mono_fn(
        &mut self,
        fn_id: FnId,
        type_args: &[Type],
        def: &TypedFnDef,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let fn_value = *self
            .mono_fns
            .get(&(fn_id, type_args.to_vec()))
            .expect("declared in monomorphic pre-pass");
        self.current_fn = Some(fn_value);
        self.vars.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        for (i, param) in def.params.iter().enumerate() {
            let arg = fn_value
                .get_nth_param(i as u32)
                .expect("param exists");
            let llvm_ty = self.llvm_basic_type(param.ty);
            let alloca = self
                .builder
                .build_alloca(llvm_ty, &param.name)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(alloca, arg)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(param.id, (alloca, param.ty));
        }

        let body_val = self.lower_block(&def.body, program)?;
        // Monomorphic instances of `main<T>` are forbidden by ADR
        // 0016 D11 — the type checker rejects them. So no main-
        // truncation special-case is needed here.
        self.builder
            .build_return(Some(&body_val))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(())
    }

    fn compile_fn(
        &mut self,
        fn_def: &TypedFnDef,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let fn_value = *self
            .fns
            .get(&fn_def.id)
            .expect("declared in pass 1");
        self.current_fn = Some(fn_value);
        self.vars.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        for (i, param) in fn_def.params.iter().enumerate() {
            let arg = fn_value
                .get_nth_param(i as u32)
                .expect("param exists");
            let llvm_ty = self.llvm_basic_type(param.ty);
            let alloca = self
                .builder
                .build_alloca(llvm_ty, &param.name)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(alloca, arg)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(param.id, (alloca, param.ty));
        }

        let body_val = self.lower_block(&fn_def.body, program)?;

        let is_main = program.signature(fn_def.id).is_main;
        if is_main {
            // main is required to return i64 per ADR 0012 D11; the
            // typed AST guarantees body_val is i64. Truncate to i32
            // for the C ABI return.
            let i32_type = self.context.i32_type();
            let body_int = body_val.into_int_value();
            let exit = self
                .builder
                .build_int_truncate(body_int, i32_type, "exit")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_return(Some(&exit))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        } else {
            self.builder
                .build_return(Some(&body_val))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }
        Ok(())
    }

    fn lower_stmt(
        &mut self,
        stmt: &TypedStmt,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            TypedStmtKind::Let { id, name, ty, value, .. } => {
                let v = self.lower_expr(value, program)?;
                let llvm_ty = self.llvm_basic_type(*ty);
                let alloca = self
                    .builder
                    .build_alloca(llvm_ty, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(alloca, v)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.vars.insert(*id, (alloca, *ty));
            }
            TypedStmtKind::Expr(e) => {
                let _ = self.lower_expr(e, program)?;
            }
        }
        Ok(())
    }

    fn lower_block(
        &mut self,
        block: &TypedBlock,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        for stmt in &block.stmts {
            self.lower_stmt(stmt, program)?;
        }
        self.lower_expr(&block.tail, program)
    }

    fn lower_if(
        &mut self,
        cond: &TypedExpr,
        then_branch: &TypedBlock,
        else_branch: &TypedBlock,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // ADR 0010 D9 retired at C1.3 step 5: the type checker
        // guarantees cond.ty == Bool, so the lowered value is already
        // i1 and feeds straight into build_conditional_branch.
        debug_assert_eq!(cond.ty, Type::Bool);
        let cond_i1 = self.lower_expr(cond, program)?.into_int_value();

        let current_fn = self.current_fn.expect("current_fn set by compile_fn");
        let then_bb = self.context.append_basic_block(current_fn, "then");
        let else_bb = self.context.append_basic_block(current_fn, "else");
        let merge_bb = self.context.append_basic_block(current_fn, "ifmerge");

        // Both arms produce the same type per check(); use that for
        // the merged-result alloca. C1.4 widens this to any basic
        // type (struct + primitives).
        let result_ty = then_branch.ty;
        let llvm_result_ty = self.llvm_basic_type(result_ty);
        let result = self
            .builder
            .build_alloca(llvm_result_ty, "ifresult")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder
            .build_conditional_branch(cond_i1, then_bb, else_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(then_bb);
        let then_val = self.lower_block(then_branch, program)?;
        self.builder
            .build_store(result, then_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(else_bb);
        let else_val = self.lower_block(else_branch, program)?;
        self.builder
            .build_store(result, else_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(merge_bb);
        let loaded = self
            .builder
            .build_load(llvm_result_ty, result, "ifresult_val")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(loaded)
    }

    /// Short-circuit lowering for `lhs && rhs`. Evaluates `lhs`; if
    /// false, the result is `lhs` (i.e. false) and `rhs` is skipped;
    /// otherwise the result is `rhs`. Implemented with one branching
    /// basic block and a PHI at the merge.
    fn lower_logic_and(
        &mut self,
        lhs: &TypedExpr,
        rhs: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let l = self.lower_expr(lhs, program)?.into_int_value();
        let lhs_end_bb = self
            .builder
            .get_insert_block()
            .expect("builder positioned inside a basic block");
        let current_fn = self.current_fn.expect("current_fn set by compile_fn");
        let rhs_bb = self.context.append_basic_block(current_fn, "and_rhs");
        let merge_bb = self.context.append_basic_block(current_fn, "and_merge");
        self.builder
            .build_conditional_branch(l, rhs_bb, merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(rhs_bb);
        let r = self.lower_expr(rhs, program)?.into_int_value();
        let rhs_end_bb = self
            .builder
            .get_insert_block()
            .expect("builder positioned inside a basic block");
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(self.context.bool_type(), "and_result")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        phi.add_incoming(&[(&l, lhs_end_bb), (&r, rhs_end_bb)]);
        Ok(phi.as_basic_value())
    }

    /// Short-circuit lowering for `lhs || rhs`. If `lhs` is true,
    /// skip `rhs` and the result is `lhs`; otherwise the result is
    /// `rhs`.
    fn lower_logic_or(
        &mut self,
        lhs: &TypedExpr,
        rhs: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let l = self.lower_expr(lhs, program)?.into_int_value();
        let lhs_end_bb = self
            .builder
            .get_insert_block()
            .expect("builder positioned inside a basic block");
        let current_fn = self.current_fn.expect("current_fn set by compile_fn");
        let rhs_bb = self.context.append_basic_block(current_fn, "or_rhs");
        let merge_bb = self.context.append_basic_block(current_fn, "or_merge");
        self.builder
            .build_conditional_branch(l, merge_bb, rhs_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(rhs_bb);
        let r = self.lower_expr(rhs, program)?.into_int_value();
        let rhs_end_bb = self
            .builder
            .get_insert_block()
            .expect("builder positioned inside a basic block");
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(self.context.bool_type(), "or_result")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        phi.add_incoming(&[(&l, lhs_end_bb), (&r, rhs_end_bb)]);
        Ok(phi.as_basic_value())
    }

    /// Call `sentinel_alloc(size)` and return the resulting pointer.
    /// Used by ArrayLit (D2) and WidenToNullable for `?Struct` (D11).
    fn alloc_call(&self, size: IntValue<'ctx>) -> Result<PointerValue<'ctx>, CodegenError> {
        let call = self
            .builder
            .build_call(self.alloc_fn, &[size.into()], "sentinel_alloc")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let ret = call
            .try_as_basic_value()
            .left()
            .ok_or_else(|| CodegenError::Builder("sentinel_alloc returned void".to_string()))?;
        Ok(ret.into_pointer_value())
    }

    /// Lower `is_some(x: ?T) -> bool` per ADR 0014 D9. Inline:
    /// extract the discriminator field (i1 valid) from the `?T`
    /// struct value.
    fn lower_is_some(
        &mut self,
        arg: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let nullable_val = self.lower_expr(arg, program)?.into_struct_value();
        let valid = self
            .builder
            .build_extract_value(nullable_val, 0, "is_some_valid")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(valid)
    }

    /// Lower `len(a: [T]) -> i64` per ADR 0015 D4. Inline: extract
    /// the length field (i64 at index 0) of the `{ i64, ptr }`
    /// array struct.
    fn lower_len(
        &mut self,
        arg: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let array_val = self.lower_expr(arg, program)?.into_struct_value();
        let len = self
            .builder
            .build_extract_value(array_val, 0, "len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(len)
    }

    /// Lower an array literal `[e1, e2, ...]` per ADR 0015 D2.
    /// Allocates `n * sizeof(T)` bytes on the heap, stores each
    /// element via GEP+store, and builds the `{ i64 len, ptr data }`
    /// struct value.
    fn lower_array_lit(
        &mut self,
        elem_ty: ArrayElem,
        elements: &[TypedExpr],
        array_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let n = elements.len() as u64;
        let len_val = i64_type.const_int(n, false);

        let elem_llvm_ty = self.llvm_basic_type(elem_ty.to_type());
        // Compute total size = n * sizeof(elem).
        // size_of() for primitive types returns Some(IntValue); for
        // structs it also returns Some. We use the build_int_mul
        // path for safety.
        let elem_size = elem_llvm_ty
            .size_of()
            .expect("non-void basic types have a known size");
        let total_size = self
            .builder
            .build_int_mul(len_val, elem_size, "array_size")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Allocate. For an empty array, total_size is 0; malloc(0)
        // is implementation-defined but our sentinel_alloc treats
        // it as a normal allocation (may return null or a unique
        // pointer per the C standard). We pass it through; reading
        // from an empty array would already be a bounds-check
        // failure.
        let data_ptr = self.alloc_call(total_size)?;

        // Store each element: GEP to elem-i, store.
        for (i, elem) in elements.iter().enumerate() {
            let idx = i64_type.const_int(i as u64, false);
            let elem_ptr = unsafe {
                self.builder
                    .build_gep(elem_llvm_ty, data_ptr, &[idx], &format!("arr_elem_{i}"))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            let elem_val = self.lower_expr(elem, program)?;
            self.builder
                .build_store(elem_ptr, elem_val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }

        // Build the { i64 len, ptr data } struct value.
        let struct_ty = match self.llvm_basic_type(array_ty) {
            BasicTypeEnum::StructType(st) => st,
            _ => unreachable!("Array lowers to a struct type"),
        };
        let agg = struct_ty.get_undef();
        let with_len = self
            .builder
            .build_insert_value(agg, len_val, 0, "arr_with_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let with_data = self
            .builder
            .build_insert_value(with_len, data_ptr, 1, "arr_with_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(with_data.into_struct_value().into())
    }

    /// Lower an array indexing `target[index]` per ADR 0015 D3 with
    /// bounds checking per D10. Emits the conditional branch on
    /// `0 <= idx < len`; the false branch calls
    /// `sentinel_panic_oob(idx, len)` and falls through to
    /// `unreachable`; the true branch does GEP+load.
    fn lower_index(
        &mut self,
        target: &TypedExpr,
        index: &TypedExpr,
        elem_ty: ArrayElem,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let array_val = self.lower_expr(target, program)?.into_struct_value();
        let idx = self.lower_expr(index, program)?.into_int_value();
        let len = self
            .builder
            .build_extract_value(array_val, 0, "arr_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let data_ptr = self
            .builder
            .build_extract_value(array_val, 1, "arr_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();

        // Bounds check: 0 <= idx < len.
        let i64_type = self.context.i64_type();
        let zero = i64_type.const_zero();
        let ge_zero = self
            .builder
            .build_int_compare(IntPredicate::SGE, idx, zero, "idx_ge_zero")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let lt_len = self
            .builder
            .build_int_compare(IntPredicate::SLT, idx, len, "idx_lt_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let in_bounds = self
            .builder
            .build_and(ge_zero, lt_len, "idx_in_bounds")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        let current_fn = self.current_fn.expect("current_fn set");
        let ok_bb = self.context.append_basic_block(current_fn, "idx_ok");
        let oob_bb = self.context.append_basic_block(current_fn, "idx_oob");
        self.builder
            .build_conditional_branch(in_bounds, ok_bb, oob_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // OOB branch: call sentinel_panic_oob then unreachable.
        self.builder.position_at_end(oob_bb);
        self.builder
            .build_call(self.panic_oob_fn, &[idx.into(), len.into()], "")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unreachable()
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // OK branch: GEP + load.
        self.builder.position_at_end(ok_bb);
        let elem_llvm_ty = self.llvm_basic_type(elem_ty.to_type());
        let elem_ptr = unsafe {
            self.builder
                .build_gep(elem_llvm_ty, data_ptr, &[idx], "idx_gep")
                .map_err(|e| CodegenError::Builder(e.to_string()))?
        };
        let elem_val = self
            .builder
            .build_load(elem_llvm_ty, elem_ptr, "idx_load")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(elem_val)
    }

    /// Lower `unwrap_or(x: ?T, default: T) -> T` per ADR 0014 D9.
    /// Inline: extract the valid bit; conditional select between the
    /// payload (when valid) and the default (when null). Uses
    /// `build_select` rather than basic-block control flow because
    /// both operands are already evaluated by their caller.
    fn lower_unwrap_or(
        &mut self,
        x: &TypedExpr,
        default: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let nullable_val = self.lower_expr(x, program)?.into_struct_value();
        let default_val = self.lower_expr(default, program)?;
        let valid = self
            .builder
            .build_extract_value(nullable_val, 0, "unwrap_valid")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let payload = self
            .builder
            .build_extract_value(nullable_val, 1, "unwrap_payload")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        // build_select on BasicValueEnum requires both arms to be
        // the same BasicValueEnum variant — payload and default are
        // both T, so they match.
        let selected = self
            .builder
            .build_select(valid, payload, default_val, "unwrap_or_result")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(selected)
    }

    fn lower_call(
        &mut self,
        id: FnId,
        args: &[TypedExpr],
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let fn_value = *self
            .fns
            .get(&id)
            .expect("FnId from a typed program is always in the fn table");
        let signature = program.signature(id);
        let arg_values: Vec<BasicMetadataValueEnum> = args
            .iter()
            .map(|a| self.lower_expr(a, program).map(|v| v.into()))
            .collect::<Result<Vec<_>, _>>()?;
        let call = self
            .builder
            .build_call(fn_value, &arg_values, &format!("call_{}", signature.name))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        call.try_as_basic_value().left().ok_or_else(|| {
            CodegenError::Builder(format!(
                "call to `{}` returned void unexpectedly",
                signature.name
            ))
        })
    }

    /// C1.7.5 / ADR 0016 D7: lower a call to a user-defined
    /// generic fn. The pre-pass already declared an LLVM fn for
    /// `(id, type_args)`; this method resolves it from `mono_fns`
    /// and emits the call. If the lookup misses (which would
    /// indicate the pre-pass collector missed an instantiation —
    /// a codegen bug), surface as `GenericCallNotYetSupported` so
    /// the failure is at least diagnostic-friendly rather than a
    /// panic.
    fn lower_mono_call(
        &mut self,
        id: FnId,
        type_args: &[Type],
        args: &[TypedExpr],
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let signature = program.signature(id);
        let key = (id, type_args.to_vec());
        let fn_value = match self.mono_fns.get(&key).copied() {
            Some(fv) => fv,
            None => {
                return Err(CodegenError::GenericCallNotYetSupported {
                    name: signature.name.clone(),
                });
            }
        };
        let arg_values: Vec<BasicMetadataValueEnum> = args
            .iter()
            .map(|a| self.lower_expr(a, program).map(|v| v.into()))
            .collect::<Result<Vec<_>, _>>()?;
        let call_name = format!("call_{}", signature.name);
        let call = self
            .builder
            .build_call(fn_value, &arg_values, &call_name)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        call.try_as_basic_value().left().ok_or_else(|| {
            CodegenError::Builder(format!(
                "call to `{}` returned void unexpectedly",
                signature.name
            ))
        })
    }

    fn lower_expr(
        &mut self,
        expr: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match &expr.kind {
            TypedExprKind::IntLit(n) => {
                let llvm_ty = self.llvm_int_type(expr.ty);
                Ok(llvm_ty.const_int(*n as u64, true).into())
            }
            TypedExprKind::BoolLit(b) => Ok(self
                .context
                .bool_type()
                .const_int(u64::from(*b), false)
                .into()),
            TypedExprKind::Var(id) => {
                let (ptr, ty) = *self
                    .vars
                    .get(id)
                    .expect("VarId from a typed program is always live in its scope");
                let name = self.var_name(*id, program).unwrap_or("load");
                let llvm_ty = self.llvm_basic_type(ty);
                let loaded = self
                    .builder
                    .build_load(llvm_ty, ptr, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(loaded)
            }
            TypedExprKind::Unary(UnaryOp::Neg, inner) => {
                let v = self.lower_expr(inner, program)?.into_int_value();
                self.builder
                    .build_int_neg(v, "neg")
                    .map(|v| v.into())
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            TypedExprKind::Unary(UnaryOp::Not, inner) => {
                // `!b` for bool b ≡ `b XOR 1`. The type checker
                // guarantees inner.ty == Bool, so the constant is i1.
                let v = self.lower_expr(inner, program)?.into_int_value();
                let one = self.context.bool_type().const_int(1, false);
                self.builder
                    .build_xor(v, one, "not")
                    .map(|v| v.into())
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            TypedExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expr(lhs, program)?.into_int_value();
                let r = self.lower_expr(rhs, program)?.into_int_value();
                let result = match op {
                    BinOp::Add => self.builder.build_int_add(l, r, "add"),
                    BinOp::Sub => self.builder.build_int_sub(l, r, "sub"),
                    BinOp::Mul => self.builder.build_int_mul(l, r, "mul"),
                    BinOp::Div => self.builder.build_int_signed_div(l, r, "div"),
                };
                result
                    .map(|v| v.into())
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            TypedExprKind::Cmp(op, lhs, rhs) => {
                // ADR 0014 D7: `x == null` / `x != null` compares the
                // discriminator (i1 valid bit) rather than the int
                // payload. The type checker only allows Eq / Ne on
                // nullable operands, so other predicates are
                // unreachable here for nullable values.
                let lhs_is_nullable = lhs.ty.is_nullable();
                let rhs_is_nullable = rhs.ty.is_nullable();
                let predicate = match op {
                    CmpOp::Eq => IntPredicate::EQ,
                    CmpOp::Ne => IntPredicate::NE,
                    CmpOp::Lt => IntPredicate::SLT,
                    CmpOp::Le => IntPredicate::SLE,
                    CmpOp::Gt => IntPredicate::SGT,
                    CmpOp::Ge => IntPredicate::SGE,
                };
                if lhs_is_nullable || rhs_is_nullable {
                    // Extract the valid bits from both sides and
                    // compare them.
                    let l_struct = self.lower_expr(lhs, program)?.into_struct_value();
                    let r_struct = self.lower_expr(rhs, program)?.into_struct_value();
                    let l_valid = self
                        .builder
                        .build_extract_value(l_struct, 0, "lhs_valid")
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                        .into_int_value();
                    let r_valid = self
                        .builder
                        .build_extract_value(r_struct, 0, "rhs_valid")
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                        .into_int_value();
                    return self
                        .builder
                        .build_int_compare(predicate, l_valid, r_valid, "cmp_null")
                        .map(|v| v.into())
                        .map_err(|e| CodegenError::Builder(e.to_string()));
                }
                let l = self.lower_expr(lhs, program)?.into_int_value();
                let r = self.lower_expr(rhs, program)?.into_int_value();
                self.builder
                    .build_int_compare(predicate, l, r, "cmp")
                    .map(|v| v.into())
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            TypedExprKind::Logic(LogicOp::And, lhs, rhs) => {
                self.lower_logic_and(lhs, rhs, program)
            }
            TypedExprKind::Logic(LogicOp::Or, lhs, rhs) => {
                self.lower_logic_or(lhs, rhs, program)
            }
            TypedExprKind::Block(b) => self.lower_block(b, program),
            TypedExprKind::If { cond, then_branch, else_branch } => {
                self.lower_if(cond, then_branch, else_branch, program)
            }
            TypedExprKind::Call { id, args, type_args, .. } => {
                // ADR 0014 D9 + ADR 0015 D4 builtins: lower inline
                // rather than calling an external runtime function.
                // Per ADR 0016 D8b these stay special-cased at C1.7
                // because their bodies can't be expressed in
                // Sentinel-1.7 source (force-unwrap / pattern
                // matching / runtime-metadata access are absent).
                if *id == UNWRAP_OR_FN_ID {
                    return self.lower_unwrap_or(&args[0], &args[1], program);
                }
                if *id == IS_SOME_FN_ID {
                    return self.lower_is_some(&args[0], program);
                }
                if *id == LEN_FN_ID {
                    return self.lower_len(&args[0], program);
                }
                // C1.7.5 / ADR 0016 D7: user-defined generic-fn
                // calls route to the monomorphic instance emitted
                // in the pre-pass. Non-generic calls take the
                // existing path.
                let signature = program.signature(*id);
                if !signature.type_params.is_empty() {
                    return self.lower_mono_call(*id, type_args, args, program);
                }
                self.lower_call(*id, args, program)
            }
            TypedExprKind::NullLit => {
                // ADR 0014 D2: NullLit lowers to `{ i1 false, undef payload }`.
                // C1.6 / ADR 0015 D11: for `?Struct`, the payload is
                // a pointer (heap-indirected); the null case uses a
                // null pointer.
                let nullable_inner = match expr.ty {
                    Type::Nullable(ni) => ni,
                    _ => unreachable!("type-check guarantees NullLit.ty is Nullable"),
                };
                let struct_ty = match self.llvm_basic_type(expr.ty) {
                    BasicTypeEnum::StructType(st) => st,
                    _ => unreachable!("Nullable lowers to a struct type"),
                };
                let valid = self.context.bool_type().const_int(0, false);
                let payload: BasicValueEnum = match nullable_inner {
                    NullableInner::Struct(_) => {
                        // null pointer
                        self.context
                            .ptr_type(inkwell::AddressSpace::default())
                            .const_null()
                            .into()
                    }
                    _ => {
                        let payload_ty = self.llvm_basic_type(nullable_inner.to_type());
                        payload_ty.const_zero()
                    }
                };
                Ok(struct_ty.const_named_struct(&[valid.into(), payload]).into())
            }
            TypedExprKind::WidenToNullable(inner) => {
                // ADR 0014 D3: lower the inner T value and wrap.
                // For primitives: `{ i1 true, T payload }` inline.
                // C1.6 / ADR 0015 D11: for Struct, allocate the
                // payload on the heap and wrap as `{ i1 true, ptr }`.
                let nullable_inner = match expr.ty {
                    Type::Nullable(ni) => ni,
                    _ => unreachable!("WidenToNullable.ty is Nullable"),
                };
                let struct_ty = match self.llvm_basic_type(expr.ty) {
                    BasicTypeEnum::StructType(st) => st,
                    _ => unreachable!("Nullable lowers to a struct type"),
                };
                let payload_val = self.lower_expr(inner, program)?;
                let payload_in_struct: BasicValueEnum = match nullable_inner {
                    NullableInner::Struct(_) => {
                        // Heap-allocate the inner struct, store the
                        // value, use the pointer as the payload.
                        let inner_ty = self.llvm_basic_type(inner.ty);
                        let size = inner_ty
                            .size_of()
                            .expect("struct types have a known size");
                        let raw_ptr = self.alloc_call(size)?;
                        self.builder
                            .build_store(raw_ptr, payload_val)
                            .map_err(|e| CodegenError::Builder(e.to_string()))?;
                        raw_ptr.into()
                    }
                    _ => payload_val,
                };
                let agg = struct_ty.get_undef();
                let valid = self.context.bool_type().const_int(1, false);
                let with_valid = self
                    .builder
                    .build_insert_value(agg, valid, 0, "widen_valid")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let with_payload = self
                    .builder
                    .build_insert_value(with_valid, payload_in_struct, 1, "widen_payload")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(with_payload.into_struct_value().into())
            }
            TypedExprKind::StructLit { id, fields, .. } => {
                // Build the struct value via a chain of build_insert_value
                // starting from undef. Field-order is already declaration
                // order per check()'s reordering at C1.4.
                let struct_ty = *self
                    .struct_types
                    .get(id)
                    .expect("struct declared in pass 0");
                let mut agg = struct_ty.get_undef();
                for (i, fv) in fields.iter().enumerate() {
                    let val = self.lower_expr(fv, program)?;
                    let inserted = self
                        .builder
                        .build_insert_value(agg, val, i as u32, "structlit")
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                    agg = inserted.into_struct_value();
                }
                Ok(agg.into())
            }
            TypedExprKind::FieldAccess { target, field_index, .. } => {
                let target_val = self.lower_expr(target, program)?;
                let struct_val = target_val.into_struct_value();
                let field_val = self
                    .builder
                    .build_extract_value(struct_val, *field_index as u32, "field")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(field_val)
            }
            TypedExprKind::ArrayLit { elem_ty, elements } => {
                self.lower_array_lit(*elem_ty, elements, expr.ty, program)
            }
            TypedExprKind::Index { target, index, elem_ty } => {
                self.lower_index(target, index, *elem_ty, program)
            }
        }
    }

    /// Look up a binding's source name for use as an LLVM SSA debug
    /// name. See C1.1.2 commit 9374edf for the rationale.
    fn var_name<'p>(&self, id: VarId, program: &'p TypedProgram) -> Option<&'p str> {
        let current_fn_value = self.current_fn?;
        let current_fn_id = *self.fns.iter().find_map(|(fn_id, value)| {
            if *value == current_fn_value {
                Some(fn_id)
            } else {
                None
            }
        })?;
        let fn_def = program.fns.iter().find(|f| f.id == current_fn_id)?;
        if let Some(param) = fn_def.params.iter().find(|p| p.id == id) {
            return Some(&param.name);
        }
        find_var_name_in_block(&fn_def.body, id)
    }
}

fn find_var_name_in_block(block: &TypedBlock, id: VarId) -> Option<&str> {
    for stmt in &block.stmts {
        if let TypedStmtKind::Let { id: bid, name, value, .. } = &stmt.kind {
            if *bid == id {
                return Some(name);
            }
            if let Some(n) = find_var_name_in_expr(value, id) {
                return Some(n);
            }
        } else if let TypedStmtKind::Expr(e) = &stmt.kind {
            if let Some(n) = find_var_name_in_expr(e, id) {
                return Some(n);
            }
        }
    }
    find_var_name_in_expr(&block.tail, id)
}

fn find_var_name_in_expr(expr: &TypedExpr, id: VarId) -> Option<&str> {
    match &expr.kind {
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::Var(_) => None,
        TypedExprKind::Unary(_, inner) | TypedExprKind::WidenToNullable(inner) => {
            find_var_name_in_expr(inner, id)
        }
        TypedExprKind::Binary(_, lhs, rhs)
        | TypedExprKind::Cmp(_, lhs, rhs)
        | TypedExprKind::Logic(_, lhs, rhs) => {
            find_var_name_in_expr(lhs, id).or_else(|| find_var_name_in_expr(rhs, id))
        }
        TypedExprKind::Block(b) => find_var_name_in_block(b, id),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            find_var_name_in_expr(cond, id)
                .or_else(|| find_var_name_in_block(then_branch, id))
                .or_else(|| find_var_name_in_block(else_branch, id))
        }
        TypedExprKind::Call { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
        }
        TypedExprKind::StructLit { fields, .. } => {
            fields.iter().find_map(|f| find_var_name_in_expr(f, id))
        }
        TypedExprKind::FieldAccess { target, .. } => find_var_name_in_expr(target, id),
        TypedExprKind::ArrayLit { elements, .. } => {
            elements.iter().find_map(|e| find_var_name_in_expr(e, id))
        }
        TypedExprKind::Index { target, index, .. } => {
            find_var_name_in_expr(target, id).or_else(|| find_var_name_in_expr(index, id))
        }
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-codegen"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_resolve::resolve;
    use sentinel_syntax::parse;
    use sentinel_types::check;

    fn compile_src(src: &str) -> Result<(), CodegenError> {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");
        compile_to_object(&typed, &out_path())
    }

    fn out_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sentinel-codegen-test-{}.o", std::process::id()))
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-codegen");
    }

    #[test]
    fn target_init_does_not_panic() {
        Target::initialize_native(&InitializationConfig::default()).expect("initialize native");
    }

    #[test]
    fn compile_main_with_int_lit() {
        compile_src("fn main() -> i64 { 42 }").expect("compile");
    }

    #[test]
    fn compile_main_with_let_program() {
        compile_src("fn main() -> i64 { let x = 5; x }").expect("compile");
    }

    #[test]
    fn compile_multi_fn_with_forward_ref() {
        compile_src("fn main() -> i64 { double(7) }\nfn double(x: i64) -> i64 { x * 2 }")
            .expect("compile");
    }

    #[test]
    fn compile_call_to_print() {
        compile_src("fn main() -> i64 { print(42) }").expect("compile");
    }

    #[test]
    fn compile_if_else_program() {
        // C1.3: if-condition must be Bool (ADR 0010 D9 retired).
        compile_src("fn main() -> i64 { if true { 42 } else { 99 } }").expect("compile");
    }

    #[test]
    fn compile_block_expression() {
        compile_src("fn main() -> i64 { let r = { let y = 4; y + 1 }; r * 2 }").expect("compile");
    }

    // ----- C1.3 codegen smoke -----

    #[test]
    fn compile_bool_literal_program() {
        compile_src("fn yes() -> bool { true }\nfn main() -> i64 { 0 }").expect("compile");
    }

    #[test]
    fn compile_comparison_program() {
        compile_src("fn gt(x: i64) -> bool { x > 0 }\nfn main() -> i64 { 0 }").expect("compile");
    }

    #[test]
    fn compile_all_six_comparisons() {
        for op in ["==", "!=", "<", "<=", ">", ">="] {
            let src = format!(
                "fn cmp(x: i64, y: i64) -> bool {{ x {op} y }}\nfn main() -> i64 {{ 0 }}"
            );
            compile_src(&src).unwrap_or_else(|e| panic!("op {op} failed: {e:?}"));
        }
    }

    #[test]
    fn compile_logical_and() {
        compile_src(
            "fn band(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_logical_or() {
        compile_src(
            "fn bor(a: bool, b: bool) -> bool { a || b }\nfn main() -> i64 { 0 }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_unary_not() {
        compile_src("fn neg(b: bool) -> bool { !b }\nfn main() -> i64 { 0 }").expect("compile");
    }

    #[test]
    fn compile_if_with_bool_condition() {
        compile_src(
            "fn main() -> i64 { let b: bool = true; if b { 1 } else { 2 } }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_if_with_comparison_condition() {
        compile_src("fn main() -> i64 { if 5 > 0 { 1 } else { 2 } }").expect("compile");
    }

    #[test]
    fn compile_c13_phasego_program() {
        // The C1.3 phase-go program per ADR 0012 appendix — bool
        // flow end-to-end through is_positive, pick, comparisons.
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
        compile_src(src).expect("compile");
    }

    #[test]
    fn compile_i32_signature_program() {
        // i32 type resolves and codegen handles it. Integer literals
        // default to i64 so direct i32 arithmetic still requires
        // future casting infrastructure (C1.5+); this fixture
        // exercises i32 propagation across fn boundaries.
        compile_src("fn echo32(x: i32) -> i32 { x }\nfn main() -> i64 { 0 }").expect("compile");
    }

    // ----- C1.4 codegen smoke: struct decl + literal + field access -----

    #[test]
    fn compile_struct_decl_only() {
        compile_src("struct Empty { }\nfn main() -> i64 { 0 }").expect("compile");
    }

    #[test]
    fn compile_struct_literal_and_field_access() {
        compile_src(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let p = P { x: 3, y: 4 }; p.x + p.y }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_struct_by_value_through_fn() {
        compile_src(
            "struct P { x: i64, y: i64 }\nfn sum(p: P) -> i64 { p.x + p.y }\nfn main() -> i64 { sum(P { x: 1, y: 2 }) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_struct_with_bool_field() {
        compile_src(
            "struct F { v: bool }\nfn get(f: F) -> bool { f.v }\nfn main() -> i64 { 0 }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_chained_field_access() {
        compile_src(
            "struct Inner { x: i64 }\nstruct Outer { inner: Inner }\nfn main() -> i64 { let o = Outer { inner: Inner { x: 7 } }; o.inner.x }",
        )
        .expect("compile");
    }

    // ----- C1.5: nullable types + null literal + builtins -----

    #[test]
    fn compile_nullable_let_with_null() {
        compile_src("fn main() -> i64 { let x: ?i64 = null; 0 }").expect("compile");
    }

    #[test]
    fn compile_nullable_let_with_widening() {
        // `42` widens to ?i64.
        compile_src("fn main() -> i64 { let x: ?i64 = 42; 0 }").expect("compile");
    }

    #[test]
    fn compile_unwrap_or_builtin() {
        compile_src(
            "fn main() -> i64 { let x: ?i64 = 42; unwrap_or(x, 0) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_is_some_builtin() {
        compile_src(
            "fn main() -> i64 { let x: ?i64 = null; if is_some(x) { 1 } else { 0 } }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_cmp_against_null() {
        compile_src(
            "fn main() -> i64 { let x: ?i64 = null; if x == null { 1 } else { 0 } }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_nullable_in_fn_return() {
        compile_src(
            "fn maybe(c: bool) -> ?i64 { if c { 42 } else { null } }\nfn main() -> i64 { unwrap_or(maybe(true), 0) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_nullable_struct_field() {
        // Non-recursive struct with a nullable field.
        compile_src(
            "struct Pair { first: ?i64, second: i64 }\nfn main() -> i64 { let p = Pair { first: 1, second: 2 }; p.second }",
        )
        .expect("compile");
    }

    // ----- C1.6 codegen smoke: arrays + indexing + len + heap ?Struct -----

    #[test]
    fn compile_array_literal() {
        compile_src("fn main() -> i64 { let xs = [1, 2, 3]; 0 }").expect("compile");
    }

    #[test]
    fn compile_array_index() {
        compile_src("fn main() -> i64 { let xs = [42]; xs[0] }").expect("compile");
    }

    #[test]
    fn compile_len_builtin() {
        compile_src("fn main() -> i64 { let xs = [1, 2, 3]; len(xs) }").expect("compile");
    }

    #[test]
    fn compile_empty_array_with_annotation() {
        compile_src("fn main() -> i64 { let xs: [i64] = []; 0 }").expect("compile");
    }

    #[test]
    fn compile_array_as_fn_arg() {
        compile_src(
            "fn first(xs: [i64]) -> i64 { xs[0] }\nfn main() -> i64 { first([7, 8, 9]) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_linked_list_struct_via_nullable() {
        // ADR 0014 D10 unlock via ADR 0015 D11: recursive struct
        // through `?Node` heap indirection.
        compile_src(
            "struct Node { value: i64, next: ?Node }\nfn main() -> i64 { let n = Node { value: 42, next: null }; n.value }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_nullable_struct_widening() {
        // `?Foo` heap-allocates the payload per ADR 0015 D11.
        compile_src(
            "struct Foo { x: i64 }\nfn maybe(c: bool) -> ?Foo { if c { Foo { x: 1 } } else { null } }\nfn main() -> i64 { 0 }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_array_of_struct() {
        compile_src(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let ps = [P { x: 1, y: 2 }, P { x: 3, y: 4 }]; ps[0].x + ps[1].y }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_c16_phasego_sum_program() {
        let src = "\
fn sum_from(a: [i64], i: i64) -> i64 {
    if i == len(a) { 0 } else { a[i] + sum_from(a, i + 1) }
}
fn main() -> i64 {
    let arr: [i64] = [1, 2, 3, 4, 5];
    sum_from(arr, 0)
}
";
        compile_src(src).expect("compile");
    }

    #[test]
    fn compile_c15_phasego_value_program() {
        // ADR 0014 phase-go 1: value flow with ?T + null + unwrap_or
        // + implicit widening.
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
        compile_src(src).expect("compile");
    }

    #[test]
    fn compile_c14_phasego_program() {
        let src = "\
struct Point { x: i64, y: i64 }
fn manhattan(p: Point) -> i64 { p.x + p.y }
fn main() -> i64 {
    let p = Point { x: 3, y: 4 };
    print(manhattan(p))
}
";
        compile_src(src).expect("compile");
    }
}
