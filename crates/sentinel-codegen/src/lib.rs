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
use sentinel_borrow_check::DropPlan;
use sentinel_resolve::{
    FnId, StructId, VarId, IS_SOME_FN_ID, LEN_FN_ID, UNWRAP_OR_FN_ID,
};
use sentinel_types::{
    ArrayElem, GenericInstanceData, GenericInstanceId, NullableInner, RefData, Type, TypedBlock,
    TypedExpr, TypedExprKind, TypedFnDef, TypedProgram, TypedStmt, TypedStmtKind,
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
///
/// C2.4: `drop_plan` carries per-fn moved-source info from the
/// borrow checker. Codegen consults it at scope exit to skip
/// dropping bindings that have been moved away (the destination
/// owns + drops them). See [`DropPlan`] in sentinel-borrow-check.
pub fn compile_to_object(
    program: &TypedProgram,
    drop_plan: &DropPlan,
    output: &Path,
) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();

    // C1.7.4b: take a mutable copy of the program's generic-instance
    // table and run the monomorphic worklist BEFORE LLVM struct
    // declarations, since substitution may extend `instances` with
    // new entries (e.g., transitive nested generics). The walk
    // doesn't need LLVM types yet — it just discovers `(FnId,
    // type_args)` tuples and threads through `Type::substitute`.
    // C2 / ADR 0017 D11: same treatment for `refs` — substitution
    // through a ref-of-TypeParam interns a fresh RefId for each
    // concrete instantiation.
    let mut instances: Vec<GenericInstanceData> = program.generic_instances.clone();
    let mut refs: Vec<RefData> = program.refs.clone();
    let instantiations = collect_mono_instantiations(program, &mut instances, &mut refs);
    // Materialise each fn instance as a substituted TypedFnDef so
    // pass 2's body lowering can consume it without TypeParam-
    // awareness. May extend `instances` further as nested generic
    // instances surface during substitution.
    let mono_defs: Vec<((FnId, Vec<Type>), TypedFnDef)> = instantiations
        .iter()
        .map(|(fn_id, args)| {
            let generic_def = program
                .fns
                .iter()
                .find(|f| f.id == *fn_id)
                .expect("collect_mono_instantiations only queues real user-defined fns");
            (
                (*fn_id, args.clone()),
                generic_def.substitute(args, &mut instances, &mut refs),
            )
        })
        .collect();

    // Pass 0: declare every user struct (non-generic + generic
    // instance) as an LLVM struct type. The typed program's struct
    // list is in StructId order. Generic instances get their own
    // LLVM struct types keyed by [`GenericInstanceId`]; their LLVM
    // names are mangled per [`mangle_generic_struct_name`] to avoid
    // collisions across instantiations.
    //
    // Three-step to handle forward references through `?T`-style
    // nullables (C1.5) and `?Struct`/`?GenericInstance` pointer
    // payloads: declare all opaque types first, then set bodies.
    let mut struct_types: HashMap<StructId, StructType> = HashMap::new();
    let mut generic_struct_types: HashMap<GenericInstanceId, StructType> = HashMap::new();
    for sd in &program.structs {
        // Skip the generic struct decl itself — only concrete
        // instances get LLVM types.
        if sd.type_params.is_empty() {
            let st = context.opaque_struct_type(&sd.name);
            struct_types.insert(sd.id, st);
        }
    }
    // Only concrete instances (no TypeParam in args, transitively)
    // get an LLVM struct type. Abstract instances exist in the
    // table because the type checker interned them for generic-fn
    // signatures (e.g., `fst<A, B>(p: Pair<A, B>)` interns a
    // `Pair<TypeParam(0), TypeParam(1)>` shape) but they never
    // materialize at runtime — the monomorphic clones use
    // concrete-arg instances built during substitution.
    let abstract_args = |args: &[Type]| -> bool {
        args.iter().any(|a| arg_contains_typeparam(*a, &instances, &refs))
    };
    for (idx, inst) in instances.iter().enumerate() {
        if abstract_args(&inst.args) {
            continue;
        }
        let name = mangle_generic_struct_name(program, inst);
        let st = context.opaque_struct_type(&name);
        generic_struct_types.insert(GenericInstanceId(idx as u32), st);
    }
    // Set bodies. Non-generic struct fields may reference generic
    // instances (e.g., `struct Cache { items: Box<i64> }`), so the
    // generic instances must already be in `generic_struct_types`
    // before this point — which they are.
    for sd in &program.structs {
        if !sd.type_params.is_empty() {
            continue; // generic decls don't get a runtime body
        }
        let field_tys: Vec<BasicTypeEnum> = sd
            .fields
            .iter()
            .map(|f| llvm_basic_type(&context, f.ty, &struct_types, &generic_struct_types))
            .collect();
        let st = struct_types[&sd.id];
        st.set_body(&field_tys, false);
    }
    for (idx, inst) in instances.iter().enumerate() {
        if abstract_args(&inst.args) {
            continue;
        }
        // The generic struct decl's field types may mention
        // `Type::TypeParam`; substitute them by the instance's args
        // to get concrete LLVM field types.
        let decl = &program.structs[inst.struct_id.0 as usize];
        let field_tys: Vec<BasicTypeEnum> = decl
            .fields
            .iter()
            .map(|f| {
                let args = inst.args.clone();
                let mut local_instances = instances.clone();
                let mut local_refs = refs.clone();
                let concrete =
                    f.ty.substitute(&args, &mut local_instances, &mut local_refs);
                llvm_basic_type(
                    &context,
                    concrete,
                    &struct_types,
                    &generic_struct_types,
                )
            })
            .collect();
        let st = generic_struct_types[&GenericInstanceId(idx as u32)];
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
            .map(|t| llvm_basic_type(&context, *t, &struct_types, &generic_struct_types).into())
            .collect();
        let fn_type = llvm_basic_type(&context, print_sig.return_type, &struct_types, &generic_struct_types)
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
            .map(|t| llvm_basic_type(&context, *t, &struct_types, &generic_struct_types).into())
            .collect();
        let fn_type = if signature.is_main {
            i32_type.fn_type(&param_types, false)
        } else {
            llvm_basic_type(&context, signature.return_type, &struct_types, &generic_struct_types)
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
    // C2.4 / ADR 0017 D8: declare sentinel_free for scope-exit
    // drop emission. Closes the C1.6+ heap-leak deferral.
    let free_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = context.void_type();
        module.add_function(
            "sentinel_free",
            void_ty.fn_type(&[ptr_ty.into()], false),
            None,
        )
    };

    // C1.7.5 / ADR 0016 D7: mono_defs + the `instances` table were
    // already built above before pass 0 (so the LLVM struct types
    // for nested generic instances could be declared). Here we
    // pre-declare each monomorphic LLVM fn with a mangled name so
    // that within pass 2 we can resolve `(FnId, Vec<Type>)` →
    // FunctionValue at every call site (including transitive
    // generic-fn-to-generic-fn calls).
    let mut mono_fns: HashMap<(FnId, Vec<Type>), FunctionValue> = HashMap::new();
    for ((fn_id, args), def) in &mono_defs {
        let mangled = mangle_mono_name(&def.name, args, program);
        let param_types: Vec<_> = def
            .params
            .iter()
            .map(|p| llvm_basic_type(&context, p.ty, &struct_types, &generic_struct_types).into())
            .collect();
        let fn_type = llvm_basic_type(&context, def.return_type, &struct_types, &generic_struct_types)
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
            generic_struct_types,
            alloc_fn,
            panic_oob_fn,
            free_fn,
            current_fn: None,
            current_fn_id: FnId(0), // placeholder; reset in compile_fn
            vars: HashMap::new(),
            scope_stack: Vec::new(),
            drop_plan,
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
struct CodegenCtx<'ctx, 'plan> {
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
    /// C1.7.4b / ADR 0016 D6: per-instance LLVM struct types,
    /// keyed by [`GenericInstanceId`]. Built in pass 0.
    generic_struct_types: HashMap<GenericInstanceId, StructType<'ctx>>,
    /// C1.6: `sentinel_alloc(i64) -> ptr` runtime function. Called
    /// to back array storage and `?Struct` heap payloads.
    alloc_fn: FunctionValue<'ctx>,
    /// C1.6: `sentinel_panic_oob(i64 idx, i64 len) -> void` runtime
    /// function. Called from the bounds-check failure block.
    panic_oob_fn: FunctionValue<'ctx>,
    /// C2.4 / ADR 0017 D8: `sentinel_free(ptr) -> void` runtime
    /// function. Emitted at scope-exit for un-moved heap-backed
    /// bindings (closes the C1.6+ heap-leak deferral).
    free_fn: FunctionValue<'ctx>,
    current_fn: Option<FunctionValue<'ctx>>,
    /// C2.4: the FnId of the fn currently being compiled. Used to
    /// look up the moved-source set from `drop_plan` at scope-exit
    /// drop emission.
    current_fn_id: FnId,
    vars: HashMap<VarId, (PointerValue<'ctx>, Type)>,
    /// C2.4 / ADR 0017 D8: stack of scopes; each scope is the
    /// ordered list of VarIds declared in it. At scope exit
    /// (block-pop, fn return), the codegen iterates these in
    /// reverse to emit drop calls for heap-backed bindings that
    /// weren't moved.
    scope_stack: Vec<Vec<VarId>>,
    /// C2.4: per-fn moved-source sets from the borrow checker.
    /// Codegen looks up `current_fn_id` to determine which
    /// bindings should be skipped at scope-exit drop emission
    /// (the destination of the move owns the value now).
    drop_plan: &'plan DropPlan,
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
    generic_struct_types: &HashMap<GenericInstanceId, StructType<'ctx>>,
) -> BasicTypeEnum<'ctx> {
    match ty {
        Type::Bool => context.bool_type().into(),
        Type::I32 => context.i32_type().into(),
        Type::I64 => context.i64_type().into(),
        Type::Struct(id) => (*struct_types
            .get(&id)
            .expect("struct declared in pass 0"))
        .into(),
        Type::GenericInstance(id) => (*generic_struct_types
            .get(&id)
            .expect("generic instance declared in pass 0"))
        .into(),
        Type::Nullable(inner) => {
            let valid_ty: BasicTypeEnum = context.bool_type().into();
            let payload_ty: BasicTypeEnum = match inner {
                NullableInner::Struct(_) | NullableInner::GenericInstance(_) => {
                    // C1.6 / ADR 0015 D11 + C1.7.4b: `?Struct` and
                    // `?GenericInstance` payloads are pointers to
                    // heap-allocated values (both are nominal
                    // structs at the runtime level).
                    context.ptr_type(inkwell::AddressSpace::default()).into()
                }
                _ => {
                    // `?primitive` stays inline { i1, T }.
                    llvm_basic_type(
                        context,
                        inner.to_type(),
                        struct_types,
                        generic_struct_types,
                    )
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
        Type::Ref(_) => {
            // C2 / ADR 0017 D11: references lower to LLVM opaque
            // pointers. The pointed-to type is recovered from
            // [`TypedProgram::refs`] at deref + borrow-take sites
            // (LLVM 15+ uses opaque pointers, so no LLVM-level
            // pointee tag is needed here).
            context.ptr_type(inkwell::AddressSpace::default()).into()
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
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
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
            instances,
            refs,
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
            instances,
            refs,
            &mut visited,
            &mut order,
            &mut pending,
        );
    }

    order
}

#[allow(clippy::too_many_arguments)]
fn walk_block_for_mono(
    block: &TypedBlock,
    subst: &[Type],
    program: &TypedProgram,
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    visited: &mut HashSet<(FnId, Vec<Type>)>,
    order: &mut Vec<(FnId, Vec<Type>)>,
    pending: &mut Vec<(FnId, Vec<Type>)>,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            TypedStmtKind::Let { value, .. } => walk_expr_for_mono(
                value, subst, program, instances, refs, visited, order, pending,
            ),
            TypedStmtKind::Assign { target, value } => {
                walk_expr_for_mono(
                    target, subst, program, instances, refs, visited, order, pending,
                );
                walk_expr_for_mono(
                    value, subst, program, instances, refs, visited, order, pending,
                );
            }
            TypedStmtKind::Expr(e) => walk_expr_for_mono(
                e, subst, program, instances, refs, visited, order, pending,
            ),
        }
    }
    walk_expr_for_mono(
        &block.tail,
        subst,
        program,
        instances,
        refs,
        visited,
        order,
        pending,
    );
}

#[allow(clippy::too_many_arguments)]
fn walk_expr_for_mono(
    e: &TypedExpr,
    subst: &[Type],
    program: &TypedProgram,
    instances: &mut Vec<GenericInstanceData>,
    refs: &mut Vec<RefData>,
    visited: &mut HashSet<(FnId, Vec<Type>)>,
    order: &mut Vec<(FnId, Vec<Type>)>,
    pending: &mut Vec<(FnId, Vec<Type>)>,
) {
    match &e.kind {
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::Var(_) => {}
        TypedExprKind::WidenToNullable(inner) => walk_expr_for_mono(
            inner, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::Unary(_, inner) => walk_expr_for_mono(
            inner, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_expr_for_mono(l, subst, program, instances, refs, visited, order, pending);
            walk_expr_for_mono(r, subst, program, instances, refs, visited, order, pending);
        }
        TypedExprKind::Block(b) => walk_block_for_mono(
            b, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_expr_for_mono(
                cond, subst, program, instances, refs, visited, order, pending,
            );
            walk_block_for_mono(
                then_branch,
                subst,
                program,
                instances,
                refs,
                visited,
                order,
                pending,
            );
            walk_block_for_mono(
                else_branch,
                subst,
                program,
                instances,
                refs,
                visited,
                order,
                pending,
            );
        }
        TypedExprKind::Call { id, args, type_args, .. } => {
            // Substitute the call's type_args under the active
            // subst to get the concrete instantiation key.
            let concrete_args: Vec<Type> = type_args
                .iter()
                .map(|t| t.substitute(subst, instances, refs))
                .collect();
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
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                walk_expr_for_mono(
                    f, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::FieldAccess { target, .. } => walk_expr_for_mono(
            target, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_expr_for_mono(
                    el, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::Index { target, index, .. } => {
            walk_expr_for_mono(
                target, subst, program, instances, refs, visited, order, pending,
            );
            walk_expr_for_mono(
                index, subst, program, instances, refs, visited, order, pending,
            );
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
        Type::Ref(id) => {
            // C2 / ADR 0017 D11: render `&T` as `ref_T`, `&mut T`
            // as `refmut_T`. Internal-only mangling — debug
            // affordance.
            program
                .refs
                .get(id.0 as usize)
                .map(|d| {
                    let prefix = if d.mutable { "refmut" } else { "ref" };
                    format!("{prefix}_{}", mangle_type(d.inner, program))
                })
                .unwrap_or_else(|| format!("ref{}", id.0))
        }
        // TypeParams shouldn't appear in a monomorphic instance's
        // type-args by construction; if one slips through, render
        // it for debuggability.
        Type::TypeParam(id) => format!("T{}", id.0),
        Type::GenericInstance(id) => {
            // Look up the instance and render its tag recursively.
            // The instance lookup goes through program.generic_instances
            // for the base name; for nested instances during codegen
            // (when `instances` may have been extended), we conservatively
            // fall back to `<gi#N>`.
            if let Some(inst) = program.generic_instances.get(id.0 as usize) {
                let base = program
                    .structs
                    .get(inst.struct_id.0 as usize)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| format!("struct{}", inst.struct_id.0));
                let mut s = base;
                for a in &inst.args {
                    s.push('_');
                    s.push_str(&mangle_type(*a, program));
                }
                s
            } else {
                format!("gi{}", id.0)
            }
        }
    }
}

/// `true` iff `ty` mentions any `Type::TypeParam`, transitively
/// through `Nullable`, `Array`, `GenericInstance`, and `Ref`. Used
/// by codegen pass 0 to filter the abstract (TypeParam-using)
/// instances out of LLVM struct-type emission per ADR 0016 D6 +
/// ADR 0017 D11.
fn arg_contains_typeparam(
    ty: Type,
    instances: &[GenericInstanceData],
    refs: &[RefData],
) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::I64 | Type::I32 | Type::Bool | Type::Struct(_) => false,
        Type::Nullable(ni) => arg_contains_typeparam(ni.to_type(), instances, refs),
        Type::Array(ae) => arg_contains_typeparam(ae.to_type(), instances, refs),
        Type::GenericInstance(id) => instances[id.0 as usize]
            .args
            .iter()
            .any(|a| arg_contains_typeparam(*a, instances, refs)),
        Type::Ref(id) => arg_contains_typeparam(refs[id.0 as usize].inner, instances, refs),
    }
}

/// C1.7.4b / ADR 0016 D6: produce a mangled LLVM struct-type name
/// for a generic instance. The scheme is `{StructName}_{arg1}_{arg2}…`
/// where each arg goes through [`mangle_type`]. Internal-only.
fn mangle_generic_struct_name(
    program: &TypedProgram,
    inst: &GenericInstanceData,
) -> String {
    let base = program
        .structs
        .get(inst.struct_id.0 as usize)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("struct{}", inst.struct_id.0));
    let mut s = base;
    for a in &inst.args {
        s.push('_');
        s.push_str(&mangle_type(*a, program));
    }
    s
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
        Type::GenericInstance(_) => {
            panic!("llvm_int_type called on non-int Type::GenericInstance")
        }
        Type::Ref(_) => panic!("llvm_int_type called on non-int Type::Ref"),
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

impl<'ctx, 'plan> CodegenCtx<'ctx, 'plan> {
    fn llvm_basic_type(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        llvm_basic_type(self.context, ty, &self.struct_types, &self.generic_struct_types)
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
        self.current_fn_id = fn_id;
        self.vars.clear();
        self.scope_stack.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        // C2.4: scope 0 for params (same as compile_fn).
        self.scope_stack.push(Vec::new());

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
            self.scope_stack
                .last_mut()
                .expect("just pushed")
                .push(param.id);
        }

        let body_val = self.lower_block(&def.body, program)?;
        let tail_returned = tail_returned_var(&def.body.tail);
        self.emit_scope_drops(tail_returned)?;
        self.scope_stack.pop();

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
        self.current_fn_id = fn_def.id;
        self.vars.clear();
        self.scope_stack.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        // C2.4: params live at scope depth 0. lower_block on the
        // body pushes scope 1, etc. At fn return, scope 0 (params)
        // gets drop emission too — moved-source set will skip
        // params that were passed-through.
        self.scope_stack.push(Vec::new());

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
            self.scope_stack
                .last_mut()
                .expect("just pushed")
                .push(param.id);
        }

        let body_val = self.lower_block(&fn_def.body, program)?;

        // C2.4: emit drops for params (scope 0). The tail value
        // has already been loaded into body_val by lower_block;
        // any binding consumed by the tail is in the moved-source
        // set and gets skipped.
        let tail_returned = tail_returned_var(&fn_def.body.tail);
        self.emit_scope_drops(tail_returned)?;
        self.scope_stack.pop();

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
                // C2.4: record this binding in the current scope
                // so scope-exit drop emission can find it.
                if let Some(top) = self.scope_stack.last_mut() {
                    top.push(*id);
                }
            }
            TypedStmtKind::Assign { target, value } => {
                // C2 / ADR 0017 D2: lower the RHS, compute the LHS
                // address as a pointer, then store. Lvalue / mut
                // gates already passed at type-check time.
                let v = self.lower_expr(value, program)?;
                let ptr = self.lower_lvalue_ptr(target, program)?;
                self.builder
                    .build_store(ptr, v)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            }
            TypedStmtKind::Expr(e) => {
                let _ = self.lower_expr(e, program)?;
            }
        }
        Ok(())
    }

    /// Compute a pointer to the storage of an lvalue expression.
    /// C2 / ADR 0017 D3 + D4: `&expr` (borrow-take) and assignment
    /// LHS share this code path — both need the address rather
    /// than the value of `expr`. Handles Var (alloca pointer
    /// directly), `*r` (load r's value as the pointer), and
    /// `target.field` (GEP into the struct's field by index, with
    /// recursion into the target).
    fn lower_lvalue_ptr(
        &mut self,
        expr: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        match &expr.kind {
            TypedExprKind::Var(id) => {
                let (ptr, _ty) = *self
                    .vars
                    .get(id)
                    .expect("VarId in scope per resolve invariants");
                Ok(ptr)
            }
            TypedExprKind::Unary(UnaryOp::Deref, inner) => {
                // `*r` as an lvalue: the address is the *value* of
                // r — load r from its alloca to get the underlying
                // pointer.
                let inner_val = self.lower_expr(inner, program)?;
                Ok(inner_val.into_pointer_value())
            }
            TypedExprKind::FieldAccess { target, field_index, .. } => {
                let target_ptr = self.lower_lvalue_ptr(target, program)?;
                let target_struct_ty = match self.llvm_basic_type(target.ty) {
                    BasicTypeEnum::StructType(st) => st,
                    other => {
                        return Err(CodegenError::Builder(format!(
                            "expected struct type for field access target, got {other:?}"
                        )));
                    }
                };
                self.builder
                    .build_struct_gep(
                        target_struct_ty,
                        target_ptr,
                        *field_index as u32,
                        "fieldptr",
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            _ => Err(CodegenError::Builder(
                "lvalue required but expression is an rvalue".to_string(),
            )),
        }
    }

    fn lower_block(
        &mut self,
        block: &TypedBlock,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // C2.4: push a fresh scope for this block's bindings.
        // Drops fire at the bottom (after the tail evaluates)
        // for any heap-backed binding in this scope that wasn't
        // moved away.
        self.scope_stack.push(Vec::new());
        for stmt in &block.stmts {
            self.lower_stmt(stmt, program)?;
        }
        let val = self.lower_expr(&block.tail, program)?;
        // Skip dropping a Var binding that's returned via the
        // tail — the move tracking will mark it in
        // `moved_sources`, but we also conservatively guard here.
        let tail_returned = tail_returned_var(&block.tail);
        self.emit_scope_drops(tail_returned)?;
        self.scope_stack.pop();
        Ok(val)
    }

    /// C2.4 / ADR 0017 D8: emit drop calls for un-moved heap-
    /// backed bindings declared in the current (top-of-stack)
    /// scope. Iterates in reverse declaration order per Rust
    /// convention. Skips:
    ///   - Bindings in [`DropPlan::moved_sources_for`] for the
    ///     current fn (the consumer owns + drops them).
    ///   - The binding returned via the tail expression (if the
    ///     tail is a Var(id)) — passed via `tail_returned`.
    fn emit_scope_drops(
        &mut self,
        tail_returned: Option<VarId>,
    ) -> Result<(), CodegenError> {
        let scope = self
            .scope_stack
            .last()
            .cloned()
            .unwrap_or_default();
        let moved = self.drop_plan.moved_sources_for(self.current_fn_id);
        for &id in scope.iter().rev() {
            if Some(id) == tail_returned {
                continue;
            }
            if moved.contains(&id) {
                continue;
            }
            let (ptr, ty) = match self.vars.get(&id) {
                Some(&v) => v,
                None => continue,
            };
            self.emit_drop_for_binding(ptr, ty)?;
        }
        Ok(())
    }

    /// Emit a drop call for a binding stored at `ptr` with type
    /// `ty`. For heap-backed types, this loads the value from
    /// `ptr` and frees the embedded heap pointer.
    fn emit_drop_for_binding(
        &mut self,
        ptr: PointerValue<'ctx>,
        ty: Type,
    ) -> Result<(), CodegenError> {
        match ty {
            Type::Array(_) => {
                // `[T]` is `{ i64 len, ptr data }`. Load + free
                // the data ptr.
                let llvm_ty = self.llvm_basic_type(ty);
                let val = self
                    .builder
                    .build_load(llvm_ty, ptr, "drop_arr")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_struct_value();
                let data = self
                    .builder
                    .build_extract_value(val, 1, "drop_arr_data")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_pointer_value();
                self.builder
                    .build_call(self.free_fn, &[data.into()], "")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            }
            Type::Nullable(NullableInner::Struct(_))
            | Type::Nullable(NullableInner::GenericInstance(_)) => {
                // `?Struct` / `?GenericInstance` is `{ i1 valid,
                // ptr payload }` per ADR 0015 D11. If valid,
                // free the payload pointer.
                let llvm_ty = self.llvm_basic_type(ty);
                let val = self
                    .builder
                    .build_load(llvm_ty, ptr, "drop_opt")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_struct_value();
                let valid = self
                    .builder
                    .build_extract_value(val, 0, "drop_opt_valid")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_int_value();
                let payload = self
                    .builder
                    .build_extract_value(val, 1, "drop_opt_payload")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_pointer_value();
                let current_fn = self.current_fn.expect("current_fn set");
                let free_block = self.context.append_basic_block(current_fn, "drop_opt_free");
                let after_block = self
                    .context
                    .append_basic_block(current_fn, "drop_opt_after");
                self.builder
                    .build_conditional_branch(valid, free_block, after_block)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder.position_at_end(free_block);
                self.builder
                    .build_call(self.free_fn, &[payload.into()], "")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_unconditional_branch(after_block)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder.position_at_end(after_block);
            }
            Type::Struct(id) => {
                // Recursive drop: load the struct value, then
                // drop each heap-backed field.
                let decl_fields: Vec<_> = self
                    .struct_types
                    .get(&id)
                    .map(|_| {
                        // We don't have direct access to the
                        // TypedStructDecl from CodegenCtx; pass
                        // through via a separate emit_drop_struct.
                        Vec::<()>::new()
                    })
                    .unwrap_or_default();
                let _ = decl_fields; // suppress unused
                self.emit_drop_struct_fields(ptr, ty)?;
            }
            Type::GenericInstance(_) => {
                self.emit_drop_struct_fields(ptr, ty)?;
            }
            Type::Nullable(_) | Type::I64 | Type::I32 | Type::Bool | Type::Ref(_) => {
                // Primitives + refs + nullable-of-primitive: no
                // heap data to free.
            }
            Type::TypeParam(_) => {
                // Unreachable post-monomorphisation; defensive.
            }
        }
        Ok(())
    }

    /// Helper for recursive drop of struct fields. Loads the
    /// struct value from `ptr` and emits drop for each heap-
    /// backed field. Used by both [`Type::Struct`] and
    /// [`Type::GenericInstance`] (concrete instances after
    /// monomorphisation are structurally the same).
    ///
    /// **C2.4 known gap**: struct field drops are deferred —
    /// only direct array bindings + `?Struct` bindings get
    /// dropped at scope exit. A struct containing an array
    /// field (e.g., c16_array_in_struct's `Bag`) would leak the
    /// inner array. Closes via a follow-on commit that threads
    /// `&TypedProgram` access through `emit_scope_drops` so we
    /// can iterate the struct's declared fields by index +
    /// recursively drop each.
    fn emit_drop_struct_fields(
        &mut self,
        _ptr: PointerValue<'ctx>,
        _ty: Type,
    ) -> Result<(), CodegenError> {
        // No-op at C2.4 minimum (see doc comment above).
        Ok(())
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
            TypedExprKind::Unary(UnaryOp::Ref, inner)
            | TypedExprKind::Unary(UnaryOp::RefMut, inner) => {
                // C2 / ADR 0017 D3: `&x` / `&mut x` produces a
                // pointer to x's storage. Mutability is enforced at
                // type-check time; LLVM doesn't distinguish.
                let ptr = self.lower_lvalue_ptr(inner, program)?;
                Ok(ptr.into())
            }
            TypedExprKind::Unary(UnaryOp::Deref, inner) => {
                // C2 / ADR 0017 D4: `*r` loads the inner type from
                // r's pointer value. Look up the ref's pointee type
                // from `program.refs[id]`.
                let ref_ptr = self.lower_expr(inner, program)?.into_pointer_value();
                let ref_id = match inner.ty {
                    Type::Ref(id) => id,
                    _ => unreachable!(
                        "type-check guarantees Deref operand has Type::Ref"
                    ),
                };
                let inner_ty = program.refs[ref_id.0 as usize].inner;
                let llvm_inner = self.llvm_basic_type(inner_ty);
                self.builder
                    .build_load(llvm_inner, ref_ptr, "deref")
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
                // order per check()'s reordering at C1.4. C1.7.4b: pick
                // the LLVM type from `generic_struct_types` when the
                // outer expr resolves to a `GenericInstance`; otherwise
                // use the plain `struct_types` lookup.
                let struct_ty = match expr.ty {
                    Type::GenericInstance(gi_id) => *self
                        .generic_struct_types
                        .get(&gi_id)
                        .expect("generic instance declared in pass 0"),
                    _ => *self
                        .struct_types
                        .get(id)
                        .expect("struct declared in pass 0"),
                };
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

/// C2.4 / ADR 0017 D8 helper: if a block / fn tail expression
/// is `Var(id)`, returns `Some(id)` — codegen should skip
/// dropping that binding at scope exit (it's being returned by
/// move). Recurses through trivial Block wrappers since the
/// type checker rewrites `{ x }` as `Block { tail: Var(x) }`.
fn tail_returned_var(tail: &TypedExpr) -> Option<VarId> {
    match &tail.kind {
        TypedExprKind::Var(id) => Some(*id),
        TypedExprKind::Block(b) => tail_returned_var(&b.tail),
        _ => None,
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
        // C2.4: compile_to_object now takes a DropPlan. For
        // codegen unit tests we use an empty plan — equivalent
        // to "no moves recorded", so every heap-backed binding
        // gets dropped at scope exit (which is the conservative
        // default and exercises the drop emission code path).
        let drop_plan = DropPlan::default();
        compile_to_object(&typed, &drop_plan, &out_path())
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

    // ----- C2.0.2 / ADR 0017 codegen smoke: refs + mut + deref + assign -----

    #[test]
    fn compile_ref_shared_basic() {
        // `&x` as fn arg; callee derefs.
        compile_src(
            "fn read(x: &i64) -> i64 { *x }\nfn main() -> i64 { let v: i64 = 7; read(&v) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_ref_mut_basic() {
        // `&mut x` exclusive borrow; deref-assign.
        compile_src(
            "fn set(x: &mut i64, v: i64) -> i64 { *x = v; *x }\nfn main() -> i64 { let mut a: i64 = 1; set(&mut a, 9) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_let_mut_then_assign() {
        compile_src(
            "fn main() -> i64 { let mut x: i64 = 0; x = 42; x }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_field_assign_via_mut_binding() {
        // p.field = v where p is a let-mut struct.
        compile_src(
            "struct P { x: i64, y: i64 }\nfn main() -> i64 { let mut p: P = P { x: 1, y: 2 }; p.x = 9; p.x + p.y }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_deref_assign_field_through_mut_ref() {
        // Compose deref + field-access on the LHS: `(*r).x = v;`.
        compile_src(
            "struct P { x: i64, y: i64 }\nfn bump(r: &mut P) -> i64 { (*r).x = 99; (*r).x + (*r).y }\nfn main() -> i64 { let mut p: P = P { x: 1, y: 2 }; bump(&mut p) }",
        )
        .expect("compile");
    }

    #[test]
    fn compile_ref_of_array() {
        // `&[i64]` passes through compile, deref reads array.
        compile_src(
            "fn first(xs: &[i64]) -> i64 { (*xs)[0] }\nfn main() -> i64 { let arr: [i64] = [11, 22, 33]; first(&arr) }",
        )
        .expect("compile");
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
