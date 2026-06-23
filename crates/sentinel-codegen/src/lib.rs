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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::{IntPredicate, OptimizationLevel};
use sentinel_ast::{BinOp, CmpOp, LogicOp, UnaryOp};
use sentinel_borrow_check::DropPlan;
use sentinel_hir::HirProgram;
use sentinel_resolve::{
    ClassId, EffectId, EnumId, FnId, ImplId, StructId, VarId, I64_TO_U8_FN_ID, IS_SOME_FN_ID,
    LEN_FN_ID, POP_FN_ID, PRINT_BYTES_FN_ID, PUSH_FN_ID, READ_FILE_FN_ID, STR_EQ_FN_ID,
    U8_TO_I64_FN_ID, UNWRAP_OR_FN_ID, VEC_NEW_FN_ID, VEC_TO_ARRAY_FN_ID, WRITE_FILE_FN_ID,
};
use sentinel_ast::SelfKind;
use sentinel_types::{
    ArrayElem, ClassData, GenericInstanceData, GenericInstanceId, ImplData, NullableInner, RefData,
    SecretData, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedFnSignature,
    TypedHandlerArm, TypedMatchArm, TypedPattern, TypedProgram, TypedReturnArm, TypedStmt,
    TypedStmtKind, TypedStructDecl,
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

    /// C3.4 / ADR 0020 D9: `handle`, `perform`, and `k(...)` resume
    /// calls parse + type-check at C3.4 but codegen lands at
    /// C3.5/C3.6 (perform → C3.5, handle → C3.6). A program that
    /// uses any of these surfaces this error from codegen and exits
    /// cleanly rather than panicking.
    #[error("handler runtime (`handle` / `perform` / continuation resume) is not yet lowered (lands at C3.5/C3.6)")]
    #[diagnostic(
        code(sentinel::codegen::handlers_not_yet_supported),
        help("the type-checker accepts handle/perform but codegen lands at C3.5 (perform) / C3.6 (handle) per ADR 0020 D9")
    )]
    HandlersNotYetSupported,

    /// D.2 / ADR 0033 D6: char/string literals type-check at D.2 (3/N)
    /// (`u8` / `[u8]`) but codegen — the `i8` char constant, the
    /// string-literal global `[N x i8]` + heap copy, `sentinel_str_eq`,
    /// the `u8`↔`i64` conversions — lands at D.2 (4/N). A program that
    /// uses a char/string literal surfaces this from codegen and exits
    /// cleanly rather than panicking.
    #[error("char/string literals are not yet lowered (lands at D.2 (4/N))")]
    #[diagnostic(
        code(sentinel::codegen::string_codegen_not_yet_supported),
        help("the type-checker accepts char/string literals (typed `u8` / `[u8]`) but codegen lands at D.2 (4/N) per ADR 0033 D6")
    )]
    StringCodegenNotYetSupported,

    /// C3.5(a) / ADR 0020 D9: at C3.5(a) the handler codegen
    /// supports only the restricted case where the `handle` body
    /// is a direct `perform Op(args)` expression — no fn-call-
    /// that-performs, no nested handles, no let-bound perform.
    /// At C3.5(b) the surface extends to allow Call-to-effecting-fn
    /// bodies; general-case (let-bound perform, perform-in-binop,
    /// ...) lands at C3.5(c) / C3.6.
    #[error(
        "handle body must be a direct `perform Op(args)` or a call to an effecting fn at C3.5(b); let-bound performs and other inline forms land at C3.5(c)/C3.6"
    )]
    #[diagnostic(
        code(sentinel::codegen::handle_body_not_direct_perform),
        help("rewrite the handle body to inline the perform call or call an effecting fn, or wait for general-case frame reification at C3.5(c)/C3.6 per ADR 0020 D9")
    )]
    HandleBodyNotDirectPerform,

    /// C3.5(b) / ADR 0020 D7: an effecting fn (declared with a
    /// non-empty `! { ... }` row) must have its body produce a
    /// continuation pointer — meaning the tail expression must
    /// be a direct `perform Op(args)` OR a call to another
    /// effecting fn, and no intermediate statement may itself
    /// perform. Per-evaluation-site frame reification lands at
    /// C3.5(c) / C3.6.
    #[error(
        "effecting fn `{fn_name}` body must be a direct `perform` or call to another effecting fn at C3.5(b); let-bound performs and other inline forms land at C3.5(c)/C3.6"
    )]
    #[diagnostic(
        code(sentinel::codegen::effecting_fn_body_not_direct),
        help("simplify the body to inline the perform / effecting call, or wait for general-case frame reification at C3.5(c)/C3.6 per ADR 0020 D9")
    )]
    EffectingFnBodyNotDirect { fn_name: String },
}

/// Lower an [`HirProgram`] to a native object file at `output`.
/// The emitted module exports `main` (i32-returning, the C ABI
/// entry); the runtime symbol `sentinel_print` is pre-declared in
/// pass 1.
///
/// C5.1a / ADR 0026 D3: codegen now consumes the HIR rather than the
/// `TypedProgram` + `DropPlan` separately. At this increment the HIR is
/// a thin bundle of the two (see the `sentinel-hir` crate); codegen
/// still lowers the type-checked program directly via
/// [`HirProgram::program`] and consults the drop plan via
/// [`HirProgram::drop_plan`]. Later C5.1a increments move the desugaring
/// (dispatch resolution, monomorphic materialisation, explicit drops)
/// into the HIR, shrinking these direct reads.
///
/// C2.4: the drop plan carries per-fn moved-source info from the borrow
/// checker. Codegen consults it at scope exit to skip dropping bindings
/// that have been moved away (the destination owns + drops them). See
/// [`DropPlan`] in sentinel-borrow-check.
pub fn compile_to_object(hir: &HirProgram, output: &Path) -> Result<(), CodegenError> {
    compile_to_object_for_module(
        hir,
        &[],
        &HashMap::new(),
        &HashMap::new(),
        &NamedTypeOrigins::default(),
        output,
    )
}

/// Phase D.6 / ADR 0037 D5/D7: compile ONE module of a separately-compiled
/// program to its own object. `module_path` is this module's path (empty
/// for a single-file program — the [`compile_to_object`] case). A LOCAL fn
/// symbol is `mangle_qualified(module_path, name)`; an imported EXTERN
/// (D5.1, `extern_origin` set) is DECLARED external under the DEFINING
/// module's symbol `mangle_qualified(&origin, name)` and left a declaration
/// (it has no body) for the linker to bind. `main` stays the bare C entry.
/// Empty `module_path` → bare names, byte-identical to before the amendment.
///
/// ADR 0037 (2/N): `op_id_base` is the build-wide per-effect op-id base map
/// (effect NAME → a graph-stable index), which decouples the runtime op id
/// from each unit's LOCAL `EffectId` so a `perform` in one unit and a
/// `handle` in another agree on `(base << 16) | op` even when the effect's
/// local id differs between units. EMPTY map (single-file / merged / the
/// corpus) → codegen falls back to the local `EffectId`, byte-identical to
/// before this amendment (the oracle's `encode_op_id` is left untouched).
///
/// ADR 0037 (2/N) `linkonce_odr`: `generic_origins` maps an IMPORTED generic
/// fn's `FnId` → its DEFINING module's path. A mono instance of such a fn over
/// collision-safe type args (primitives — see [`mono_args_dedup_safe`]) is
/// emitted under the ORIGIN-qualified symbol with `linkonce_odr` linkage, so N
/// importers share ONE definition (the linker dedups) instead of each emitting
/// its own importer-qualified copy. EMPTY map (single-file / merged / corpus)
/// → every generic is treated as local and emitted importer-qualified with the
/// default linkage, byte-identical to before.
///
/// ADR 0037 (2/N) point 8: `named_origins` carries the IMPORTED struct/enum →
/// defining-module-path maps. They module-qualify a cross-module named type's
/// tag in a `linkonce_odr` mono key (`id__util$geo$Point`), so a mono instance
/// over a cross-module struct/enum dedups SOUNDLY — two same-named types from
/// different modules no longer alias. EMPTY maps → a named-type arg is treated
/// as not-dedup-safe (kept importer-qualified), byte-identical to before.
pub fn compile_to_object_for_module(
    hir: &HirProgram,
    module_path: &[String],
    op_id_base: &HashMap<String, u32>,
    generic_origins: &HashMap<FnId, Vec<String>>,
    named_origins: &NamedTypeOrigins,
    output: &Path,
) -> Result<(), CodegenError> {
    let program = hir.program();
    let drop_plan = hir.drop_plan();

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
    // C3 / ADR 0019 D5 (C3.1): secrets are cloned from the typed
    // program too. The interner table flows into llvm_basic_type so
    // every Secret(SecretId) lookup can strip to the inner type.
    let secrets: Vec<SecretData> = program.secrets.clone();
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
    // C4.1 / ADR 0022 D9: per-class LLVM struct types. Declared
    // opaque alongside structs so field-type bodies can reference
    // both struct and class names freely.
    let mut class_types: HashMap<ClassId, StructType> = HashMap::new();
    for cd in &program.class_decls {
        let st = context.opaque_struct_type(&cd.name);
        class_types.insert(cd.id, st);
    }
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
            .map(|f| llvm_basic_type(&context, f.ty, &struct_types, &generic_struct_types, &class_types, &secrets))
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
                    &class_types,
                    &secrets,
                )
            })
            .collect();
        let st = generic_struct_types[&GenericInstanceId(idx as u32)];
        st.set_body(&field_tys, false);
    }
    // C4.1 / ADR 0022 D9: set class struct bodies. Field types
    // can reference structs (incl. generic instances) + other
    // classes — all already declared opaque above.
    for cd in &program.class_decls {
        let field_tys: Vec<BasicTypeEnum> = cd
            .fields
            .iter()
            .map(|f| llvm_basic_type(&context, f.ty, &struct_types, &generic_struct_types, &class_types, &secrets))
            .collect();
        let st = class_types[&cd.id];
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
            .map(|t| llvm_basic_type(&context, *t, &struct_types, &generic_struct_types, &class_types, &secrets).into())
            .collect();
        let fn_type = llvm_basic_type(&context, print_sig.return_type, &struct_types, &generic_struct_types, &class_types, &secrets)
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
            .map(|t| llvm_basic_type(&context, *t, &struct_types, &generic_struct_types, &class_types, &secrets).into())
            .collect();
        // C3.5(b) / ADR 0020 D7: effecting fns (non-empty
        // effect_row) return a continuation pointer (Kont*)
        // instead of their declared Sentinel return type. The
        // caller is always inside an enclosing `handle` (per
        // ADR 0019 D13: main is pure), and the handle catches
        // the kont. The fn's declared Sentinel return type is
        // preserved at the type-system level for diagnostics
        // and effect-row reasoning; only the IR shape changes.
        let fn_type = if signature.is_main {
            i32_type.fn_type(&param_types, false)
        } else if uses_kont_abi(signature, program) {
            // Effecting fn: return Kont* (opaque pointer). Async-
            // only fns are excluded (direct-runtime, not handler).
            context
                .ptr_type(inkwell::AddressSpace::default())
                .fn_type(&param_types, false)
        } else {
            llvm_basic_type(&context, signature.return_type, &struct_types, &generic_struct_types, &class_types, &secrets)
                .fn_type(&param_types, false)
        };
        // Phase D.6 / ADR 0037 D7: the LLVM symbol is module-qualified.
        // `main` is the bare C entry. An imported EXTERN (D5.1) is DECLARED
        // under the DEFINING module's symbol (`mangle_qualified(&origin,
        // name)`) — it has no TypedFnDef body, so Pass 2 (which iterates
        // `program.fns`) leaves it a declaration the linker binds. A LOCAL
        // fn is defined under THIS module's symbol. Empty `module_path`
        // (single-file / Path-A) → bare names, byte-identical to before.
        let symbol = if signature.is_main {
            "main".to_string()
        } else if let Some(origin) = &signature.extern_origin {
            mangle_qualified(origin, &signature.name)
        } else {
            mangle_qualified(module_path, &signature.name)
        };
        let fn_value = module.add_function(&symbol, fn_type, None);
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
    // D.2 / ADR 0033 D5: declare `sentinel_str_eq(ptr, i64, ptr, i64)
    // -> i1` for the `str_eq` byte-array-equality builtin.
    let str_eq_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        let bool_ty = context.bool_type();
        module.add_function(
            "sentinel_str_eq",
            bool_ty.fn_type(
                &[ptr_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into()],
                false,
            ),
            None,
        )
    };
    // D.3 / ADR 0034 D7: declare `sentinel_realloc(ptr, i64) -> ptr`
    // for growing a `Vec`'s data buffer in `push` (and the first
    // allocation, since `realloc(null, n) == malloc(n)`).
    let realloc_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_realloc",
            ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    // D.4 / ADR 0035 D6: declare the file-I/O runtime symbols.
    // `sentinel_read_file(path_ptr, path_len, out_len: *i64) -> data_ptr`
    // returns the freshly-allocated `[u8]` buffer + writes its length to
    // `out_len`; `sentinel_write_file(path_ptr, path_len, data_ptr,
    // data_len) -> i64` writes a file. Both abort on I/O failure.
    let read_file_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_read_file",
            ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false),
            None,
        )
    };
    let write_file_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_write_file",
            i64_ty.fn_type(
                &[ptr_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into()],
                false,
            ),
            None,
        )
    };
    // D.4 (2/N) / ADR 0035 D4: `sentinel_print_bytes(data_ptr, data_len)
    // -> i64` writes a `[u8]` to stdout (the byte companion to `print`).
    let print_bytes_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_print_bytes",
            i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    // C5.4 (2/N) / ADR 0028: declare the broker scope-arena symbols.
    // `sentinel_arena_enter(capacity: i64) -> *arena` creates a bump
    // arena (capacity 0 → runtime default); `sentinel_arena_alloc(arena,
    // size: i64) -> *u8` bump-allocates within it; `sentinel_arena_exit(
    // arena)` destroys it, bulk-freeing every allocation at once. These
    // back the scope→arena routing of non-escaping array allocations.
    let arena_enter_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_arena_enter",
            ptr_ty.fn_type(&[i64_ty.into()], false),
            None,
        )
    };
    let arena_alloc_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_arena_alloc",
            ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    let arena_exit_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = context.void_type();
        module.add_function(
            "sentinel_arena_exit",
            void_ty.fn_type(&[ptr_ty.into()], false),
            None,
        )
    };
    // C3.5(a) / ADR 0020 D7: declare the handler-runtime symbols.
    // The Kont struct layout (op_id: u32, _pad: u32, arg: i64,
    // consumed: u8, total 24 bytes / 8-byte aligned) matches
    // sentinel_runtime::SentinelKont and is asserted by a
    // round-trip layout test there.
    let perform_op_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        let i32_ty = context.i32_type();
        module.add_function(
            "sentinel_perform_op",
            ptr_ty.fn_type(&[i32_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    // C3.5(e) / ADR 0020 D7: sentinel_kont_resume returns a kont*
    // (not i64). Either a PURE_RETURN_OP_ID wrap (the chain
    // drained without bubbling — caller unwraps via
    // sentinel_kont_consume_pure) or a fresh op-perform kont
    // (a resumer in the chain performed; caller's enclosing
    // handle re-dispatches per ADR 0020 D3 deep-handler
    // semantics).
    let kont_resume_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_kont_resume",
            ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    // C3.5(b) / ADR 0020 D7: pure-return wrap + unwrap symbols.
    // An effecting fn whose body happens to be a pure expression
    // (the annotation said "may perform Io" but the body doesn't)
    // wraps the value via sentinel_kont_pure so the caller's
    // handle still receives a Kont*. Symmetric sentinel_kont_consume_pure
    // unwraps the value at the handle's pure-return switch case.
    let kont_pure_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_kont_pure",
            ptr_ty.fn_type(&[i64_ty.into()], false),
            None,
        )
    };
    let kont_consume_pure_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        module.add_function(
            "sentinel_kont_consume_pure",
            i64_ty.fn_type(&[ptr_ty.into()], false),
            None,
        )
    };
    // C3.5(c) / ADR 0020 D7: sentinel_kont_push pushes a captured
    // evaluation frame onto a kont's chain. Used at every "could-
    // be-captured" eval site (let-stmt with effecting RHS at
    // C3.5(c); binops + if + index at later sub-phases).
    let kont_push_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = context.void_type();
        module.add_function(
            "sentinel_kont_push",
            void_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
            None,
        )
    };
    // C3.5(a) note: we do NOT declare sentinel_kont_panic_resumed
    // at the module level — codegen never calls it directly. The
    // runtime's `sentinel_kont_resume` dispatches to it internally
    // on the consumed-twice path; that call is resolved at the
    // runtime crate's own link time.

    // C4.4 / ADR 0024 D7: declare the structured-concurrency runtime
    // symbols. Task* / ScopeCtx* are opaque `ptr` to codegen; the
    // layouts live in sentinel-runtime (SentinelTask / SentinelScopeCtx).
    let task_spawn_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        // sentinel_task_spawn(wrapper: ptr, args: ptr, args_size: i64) -> *Task
        module.add_function(
            "sentinel_task_spawn",
            ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), i64_ty.into()], false),
            None,
        )
    };
    let task_await_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = context.i64_type();
        // sentinel_task_await(task: ptr) -> i64
        module.add_function(
            "sentinel_task_await",
            i64_ty.fn_type(&[ptr_ty.into()], false),
            None,
        )
    };
    let scope_enter_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        // sentinel_scope_enter() -> *ScopeCtx
        module.add_function("sentinel_scope_enter", ptr_ty.fn_type(&[], false), None)
    };
    let scope_register_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = context.void_type();
        // sentinel_scope_register(scope: ptr, task: ptr) -> void
        module.add_function(
            "sentinel_scope_register",
            void_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
            None,
        )
    };
    let scope_exit_fn = {
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = context.void_type();
        // sentinel_scope_exit(scope: ptr) -> void
        module.add_function(
            "sentinel_scope_exit",
            void_ty.fn_type(&[ptr_ty.into()], false),
            None,
        )
    };

    // C4.1 / ADR 0022 D9: declare per-class init + method LLVM
    // fns. Init takes `out_ptr: ptr` as the synthetic first arg
    // (so the constructed class doesn't need to be returned by
    // value across the ABI). Methods take `self_ptr: ptr` as the
    // first arg. The mangled fn name is `ClassName__init` /
    // `ClassName__method`.
    let mut class_init_fns: HashMap<ClassId, FunctionValue> = HashMap::new();
    let mut class_method_fns: HashMap<(ClassId, usize), FunctionValue> = HashMap::new();
    let ptr_ty_global = context.ptr_type(inkwell::AddressSpace::default());
    let void_ty_global = context.void_type();
    for cd in &program.class_decls {
        if let Some(init_def) = &cd.init {
            let mut param_tys: Vec<inkwell::types::BasicMetadataTypeEnum> =
                vec![ptr_ty_global.into()];
            for p in &init_def.params {
                param_tys.push(
                    llvm_basic_type(
                        &context,
                        p.ty,
                        &struct_types,
                        &generic_struct_types,
                        &class_types,
                        &secrets,
                    )
                    .into(),
                );
            }
            let fn_type = void_ty_global.fn_type(&param_tys, false);
            // Phase D.6 / ADR 0037 D7: module-qualify the symbol (calls
            // resolve via `class_init_fns` by ClassId, not by name, so the
            // rename is self-contained). Empty path → bare, unchanged.
            let mangled = mangle_qualified(module_path, &format!("{}__init", cd.name));
            let fn_value = module.add_function(&mangled, fn_type, None);
            class_init_fns.insert(cd.id, fn_value);
        }
        for (m_idx, m) in cd.methods.iter().enumerate() {
            let mut param_tys: Vec<inkwell::types::BasicMetadataTypeEnum> =
                vec![ptr_ty_global.into()];
            for p in &m.params {
                param_tys.push(
                    llvm_basic_type(
                        &context,
                        p.ty,
                        &struct_types,
                        &generic_struct_types,
                        &class_types,
                        &secrets,
                    )
                    .into(),
                );
            }
            let fn_type = llvm_basic_type(
                &context,
                m.return_type,
                &struct_types,
                &generic_struct_types,
                &class_types,
                &secrets,
            )
            .fn_type(&param_tys, false);
            // Phase D.6 / ADR 0037 D7: module-qualified (calls via
            // `class_method_fns` by (ClassId, idx); empty path → bare).
            let mangled = mangle_qualified(module_path, &format!("{}__{}", cd.name, m.name));
            let fn_value = module.add_function(&mangled, fn_type, None);
            class_method_fns.insert((cd.id, m_idx), fn_value);
        }
    }

    // C4.2 / ADR 0023 D9: declare per-impl method LLVM fns. The
    // ABI mirrors class methods — `self_ptr: ptr` as the first
    // arg, then the declared params. Mangling: default impls use
    // `default__<Type>__<Trait>__<method>`; named impls use
    // `<ImplName>__<Type>__<Trait>__<method>`.
    let mut impl_method_fns: HashMap<(ImplId, usize), FunctionValue> = HashMap::new();
    for imp in &program.impl_decls {
        for (m_idx, m) in imp.methods.iter().enumerate() {
            let mut param_tys: Vec<inkwell::types::BasicMetadataTypeEnum> =
                vec![ptr_ty_global.into()];
            for p in &m.params {
                param_tys.push(
                    llvm_basic_type(
                        &context,
                        p.ty,
                        &struct_types,
                        &generic_struct_types,
                        &class_types,
                        &secrets,
                    )
                    .into(),
                );
            }
            let fn_type = llvm_basic_type(
                &context,
                m.return_type,
                &struct_types,
                &generic_struct_types,
                &class_types,
                &secrets,
            )
            .fn_type(&param_tys, false);
            let prefix = imp.name.as_deref().unwrap_or("default");
            // Phase D.6 / ADR 0037 D7: module-qualified (calls via
            // `impl_method_fns` by (ImplId, idx); empty path → bare).
            let mangled = mangle_qualified(
                module_path,
                &format!("{prefix}__{}__{}__{}", imp.type_name, imp.trait_name, m.name),
            );
            let fn_value = module.add_function(&mangled, fn_type, None);
            impl_method_fns.insert((imp.id, m_idx), fn_value);
        }
    }

    // C1.7.5 / ADR 0016 D7: mono_defs + the `instances` table were
    // already built above before pass 0 (so the LLVM struct types
    // for nested generic instances could be declared). Here we
    // pre-declare each monomorphic LLVM fn with a mangled name so
    // that within pass 2 we can resolve `(FnId, Vec<Type>)` →
    // FunctionValue at every call site (including transitive
    // generic-fn-to-generic-fn calls).
    let mut mono_fns: HashMap<(FnId, Vec<Type>), FunctionValue> = HashMap::new();
    for ((fn_id, args), def) in &mono_defs {
        // Phase D.6 / ADR 0037 D6/D7. Two emission models for a mono instance:
        //  - DEFAULT (local generic, or an imported one over a named-type arg):
        //    module-qualify by THIS unit's path (`_S4main6id__i64`) with the
        //    default linkage — each importer self-contains its own copy, no
        //    cross-unit clash. Empty `module_path` (single-file / Path-A) →
        //    the bare `id__i64`, byte-identical.
        //  - (2/N) `linkonce_odr` DEDUP: an IMPORTED generic (`generic_origins`
        //    holds its origin) over COLLISION-SAFE args is emitted under the
        //    ORIGIN-qualified symbol with `linkonce_odr` linkage, so N importers
        //    share ONE definition. The mono name renders a cross-module struct
        //    or enum arg ORIGIN-qualified (`id__util$geo$Point`, via
        //    `named_origins`) so two same-named cross-module types don't alias
        //    (ADR point 8).
        let dedup = generic_origins
            .get(fn_id)
            .filter(|_| mono_args_dedup_safe(args, named_origins));
        let mangled = match dedup {
            Some(origin) => mangle_qualified(
                origin,
                &mangle_mono_name_dedup(&def.name, args, program, named_origins),
            ),
            None => mangle_qualified(module_path, &mangle_mono_name(&def.name, args, program)),
        };
        let param_types: Vec<_> = def
            .params
            .iter()
            .map(|p| llvm_basic_type(&context, p.ty, &struct_types, &generic_struct_types, &class_types, &secrets).into())
            .collect();
        let fn_type = llvm_basic_type(&context, def.return_type, &struct_types, &generic_struct_types, &class_types, &secrets)
            .fn_type(&param_types, false);
        let fn_value = module.add_function(&mangled, fn_type, None);
        if dedup.is_some() {
            // Shared across importers → the linker keeps one definition.
            fn_value.set_linkage(Linkage::LinkOnceODR);
        }
        mono_fns.insert((*fn_id, args.clone()), fn_value);
    }

    // Pass 2: emit each user function body. (The runtime `print`
    // has no body — it's defined externally by sentinel-runtime.)
    {
        // C3.5(c) + C3.5(d) / ADR 0020 D7: pre-declare per-fn
        // resumer fns for any user fn whose body matches the
        // let-shape (single let with effecting RHS) OR the
        // embedded-perform shape (stmts.len() == 0 + tail has
        // exactly one perform). The two shapes are disjoint;
        // each fn matches at most one. Each resumer's signature
        // is uniform: `ptr (i64, ptr)` — the resumed value +
        // opaque captured-state pointer; returns a Kont* wrapping
        // the resumer's result. Captured VarIds are captured here
        // so compile_fn can produce both the parent body and the
        // resumer body from the same data.
        let mut let_resumers: HashMap<FnId, (FunctionValue<'_>, Vec<VarId>)> =
            HashMap::new();
        let mut embedded_perform_resumers: HashMap<
            FnId,
            (FunctionValue<'_>, Vec<VarId>, TypedExpr, VarId),
        > = HashMap::new();
        // C3.5(e) / ADR 0020 D7: for chained effecting lets we
        // pre-declare N resumer fns + the captures per resumer
        // so compile_fn can emit each resumer's body in turn.
        let mut chained_lets_resumers: HashMap<
            FnId,
            (Vec<FunctionValue<'_>>, Vec<Vec<VarId>>),
        > = HashMap::new();
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty_local = context.i64_type();
        let resumer_ty = ptr_ty.fn_type(&[i64_ty_local.into(), ptr_ty.into()], false);
        for fn_def in &program.fns {
            if !fn_def.type_params.is_empty() {
                continue;
            }
            if let Some(info) = detect_let_shape(fn_def, program) {
                let captured = collect_captured_vars(info.tail, info.let_id);
                let resumer_name = format!(
                    "__resume_{}_let_{}",
                    fn_def.name, info.let_id.0
                );
                let resumer_fn = module.add_function(&resumer_name, resumer_ty, None);
                let_resumers.insert(fn_def.id, (resumer_fn, captured));
            } else if let Some(info) = detect_embedded_perform_shape(fn_def, program) {
                let resumer_name = format!("__resume_{}_embedded", fn_def.name);
                let resumer_fn = module.add_function(&resumer_name, resumer_ty, None);
                embedded_perform_resumers.insert(
                    fn_def.id,
                    (
                        resumer_fn,
                        info.captured,
                        info.substituted_tail,
                        info.placeholder_id,
                    ),
                );
            } else if let Some(info) = detect_chained_effecting_lets_shape(fn_def, program)
            {
                let mut resumer_fns = Vec::with_capacity(info.lets.len());
                let mut captures_per_resumer = Vec::with_capacity(info.lets.len());
                for (i, (let_id, _let_ty, _rhs)) in info.lets.iter().enumerate() {
                    let resumer_name = format!(
                        "__resume_{}_let_{}",
                        fn_def.name, let_id.0
                    );
                    let resumer_fn =
                        module.add_function(&resumer_name, resumer_ty, None);
                    resumer_fns.push(resumer_fn);
                    captures_per_resumer
                        .push(compute_chained_lets_captures(&info, i));
                }
                chained_lets_resumers
                    .insert(fn_def.id, (resumer_fns, captures_per_resumer));
            }
        }

        // C4.4 / ADR 0024 D8: synthesize one spawn-wrapper per
        // unique spawn-target FnId. This MUST run here — before
        // CodegenCtx is created — because wrapper synthesis needs a
        // `&module` reference + a fresh builder, and CodegenCtx
        // doesn't hold `&module`. Each wrapper is
        //   void __spawn_wrapper_<id>(*Task task, *u8 args)
        // which unpacks i64 args from `args` (8-byte slots), calls
        // the target fn, stores the result into task->result
        // (offset 0) + sets task->done (offset 8). Args are i64 at
        // C4.4 minimum (matches the Task<i64> result restriction).
        let mut spawn_wrappers: HashMap<FnId, FunctionValue> = HashMap::new();
        {
            let mut targets: Vec<FnId> = Vec::new();
            for fn_def in &program.fns {
                if fn_def.type_params.is_empty() {
                    collect_spawn_targets_block(&fn_def.body, &mut targets);
                }
            }
            targets.sort_by_key(|f| f.0);
            targets.dedup();
            let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
            let i64_ty = context.i64_type();
            let i32_ty = context.i32_type();
            let i8_ty = context.i8_type();
            let void_ty = context.void_type();
            for fn_id in targets {
                let target_fn = *fns
                    .get(&fn_id)
                    .expect("spawn target fn declared in pass 1");
                let n_args = program.signature(fn_id).param_types.len();
                let wrapper = module.add_function(
                    &format!("__spawn_wrapper_{}", fn_id.0),
                    void_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false),
                    None,
                );
                let entry = context.append_basic_block(wrapper, "entry");
                let wb = context.create_builder();
                wb.position_at_end(entry);
                let task_ptr = wrapper
                    .get_nth_param(0)
                    .expect("wrapper task param")
                    .into_pointer_value();
                let args_ptr = wrapper
                    .get_nth_param(1)
                    .expect("wrapper args param")
                    .into_pointer_value();
                let mut call_args = Vec::with_capacity(n_args);
                for i in 0..n_args {
                    let offset = i64_ty.const_int((i * 8) as u64, false);
                    let slot = unsafe {
                        wb.build_in_bounds_gep(i8_ty, args_ptr, &[offset], &format!("arg_slot_{i}"))
                            .map_err(|e| CodegenError::Builder(e.to_string()))?
                    };
                    let val = wb
                        .build_load(i64_ty, slot, &format!("arg_{i}"))
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                    call_args.push(val.into());
                }
                let call = wb
                    .build_call(target_fn, &call_args, "spawn_call")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let result = call
                    .try_as_basic_value()
                    .left()
                    .expect("spawn target returns i64");
                // task->result at offset 0.
                wb.build_store(task_ptr, result)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                // task->done = 1 at offset 8.
                let done_off = i64_ty.const_int(8, false);
                let done_slot = unsafe {
                    wb.build_in_bounds_gep(i8_ty, task_ptr, &[done_off], "done_slot")
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                };
                wb.build_store(done_slot, i32_ty.const_int(1, false))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                wb.build_return(None)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                spawn_wrappers.insert(fn_id, wrapper);
            }
        }

        // C5.4 (2/N) / ADR 0028: compute the arena-routing set once, up
        // front, from the typed program + the borrow-check DropPlan
        // (both available here). Drives alloc-routing AND free-skipping.
        let arena_routed = compute_arena_routed(program, drop_plan);

        let mut cx = CodegenCtx {
            context: &context,
            builder,
            fns,
            mono_fns,
            struct_types,
            generic_struct_types,
            class_types,
            class_init_fns,
            class_method_fns,
            impl_method_fns,
            secrets,
            op_id_base: op_id_base.clone(),
            alloc_fn,
            panic_oob_fn,
            free_fn,
            str_eq_fn,
            realloc_fn,
            read_file_fn,
            write_file_fn,
            print_bytes_fn,
            arena_enter_fn,
            arena_alloc_fn,
            arena_exit_fn,
            perform_op_fn,
            kont_resume_fn,
            kont_pure_fn,
            kont_consume_pure_fn,
            kont_push_fn,
            task_spawn_fn,
            task_await_fn,
            scope_enter_fn,
            scope_register_fn,
            scope_exit_fn,
            spawn_wrappers,
            current_scope: None,
            let_resumers,
            embedded_perform_resumers,
            chained_lets_resumers,
            handle_stack: Vec::new(),
            handle_depth: 0,
            current_fn: None,
            current_fn_id: FnId(0), // placeholder; reset in compile_fn
            vars: HashMap::new(),
            scope_stack: Vec::new(),
            drop_plan,
            arena_routed,
            array_route_active: false,
            loop_depth: 0,
            loop_targets: Vec::new(),
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
        // C4.1 / ADR 0022 D9: emit each class's init + method
        // bodies. The init body writes through out_ptr; methods
        // read/write through self_ptr. Both use the standard
        // lower_block machinery with self pre-bound in vars.
        for cd in &program.class_decls {
            cx.compile_class(cd, program)?;
        }
        // C4.2 / ADR 0023 D9: emit each impl's method bodies.
        // Methods take self_ptr as the first param; lower the body
        // identically to a class method.
        for imp in &program.impl_decls {
            cx.compile_impl(imp, program)?;
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

/// C3.5(e) / ADR 0020 D7: per-handle dispatch context recorded on
/// [`CodegenCtx::handle_stack`] while lowering the arms. A `k(v)`
/// call inside an arm body whose resume bubbles (returns a non-
/// PURE_RETURN kont) stores the bubble kont into
/// `current_kont_slot` and branches to `loop_block`, where the
/// handle re-reads `current_kont_slot` + re-dispatches.
///
/// C3.6(a) extension: `return_arm` carries the (optional)
/// non-identity `return v => body` transform. When `k(v)`'s
/// resume drains to a pure value, [`Self::lower_resume_kont`]
/// applies the return arm to that value per Phase B's deep-
/// handler re-wrap semantics (k := `\v. handle (kont.resume v)
/// with H`, where H includes the return arm).
#[derive(Clone)]
struct HandleContext<'ctx> {
    loop_block: inkwell::basic_block::BasicBlock<'ctx>,
    current_kont_slot: PointerValue<'ctx>,
    return_arm: Option<TypedReturnArm>,
}

/// C2.4 / ADR 0017 D8: one lexical scope's drop state. `vars` is the
/// ordered list of VarIds declared in the scope; drop emission walks it
/// in reverse.
///
/// C5.4 (2/N) / ADR 0028: `arena` is the scope's broker bump arena
/// handle (the `*mut c_void` returned by `sentinel_arena_enter`),
/// created *lazily* on the first arena-routed allocation in the scope
/// (`None` if the scope routes nothing, so unused scopes cost nothing
/// and stay byte-identical). At scope exit a single `sentinel_arena_exit`
/// bulk-frees it — replacing the per-binding `sentinel_free`s of exactly
/// the arena-routed bindings (the [`CodegenCtx::arena_routed`] set drives
/// both the routing and the free-skip, so they cannot diverge).
#[derive(Default, Clone)]
struct ScopeFrame<'ctx> {
    vars: Vec<VarId>,
    arena: Option<PointerValue<'ctx>>,
}

impl<'ctx> ScopeFrame<'ctx> {
    /// Record a binding declared in this scope. Keeps every existing
    /// `scope_stack.last_mut().push(id)` call site working unchanged.
    fn push(&mut self, id: VarId) {
        self.vars.push(id);
    }
}

/// Phase D.5 (2/N) / ADR 0036 D9: the branch targets + scope floor of an
/// enclosing `while` loop, for lowering `break` / `continue`. Pushed onto
/// [`CodegenCtx::loop_targets`] when entering a `while` body and popped on
/// exit; `break` / `continue` read the top of the stack — the innermost
/// loop (there are no loop labels at D.5). `Copy` since every field is.
#[derive(Clone, Copy)]
struct LoopTarget<'ctx> {
    /// `loop_cond` — `continue` branches here (re-evaluate the condition).
    cond_bb: inkwell::basic_block::BasicBlock<'ctx>,
    /// `loop_after` — `break` branches here (exit the loop).
    after_bb: inkwell::basic_block::BasicBlock<'ctx>,
    /// `scope_stack.len()` captured the instant before the body scope is
    /// pushed (i.e. the body frame's index). `break` / `continue` drain
    /// every frame at index `>= scope_floor` — the body scope plus any
    /// nested `if` / block scopes between it and the branch — BEFORE
    /// branching, so the per-iteration drops still run on the early-exit
    /// path. Without this a body-scope heap binding live at the `break`
    /// leaks (the load-bearing (2/N) correctness property).
    scope_floor: usize,
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
    /// C4.1 / ADR 0022 D9: per-class LLVM struct types. Built in
    /// pass 0 alongside structs. Each class has exactly one LLVM
    /// struct type; no generic classes at C4.1.
    class_types: HashMap<ClassId, StructType<'ctx>>,
    /// C4.1 / ADR 0022 D9: per-class init fn (FunctionValue) and
    /// per-(class, method_index) method fn. Method index matches
    /// the order in `ClassData::methods`.
    class_init_fns: HashMap<ClassId, FunctionValue<'ctx>>,
    class_method_fns: HashMap<(ClassId, usize), FunctionValue<'ctx>>,
    /// C4.2 / ADR 0023 D9: per-(ImplId, method_index) impl method
    /// fn. The method index matches the order in `ImplData::methods`
    /// (which mirrors the trait's method order after Pass 3d
    /// completeness check). Mangled name:
    /// - default impls: `default__<Type>__<Trait>__<method>`
    /// - named impls:   `<Name>__<Type>__<Trait>__<method>`
    impl_method_fns: HashMap<(ImplId, usize), FunctionValue<'ctx>>,
    /// C3 / ADR 0019 D5 (C3.1): secrets table cloned from
    /// `TypedProgram.secrets` at codegen entry. Used to strip
    /// `Type::Secret(SecretId)` to its inner type — secrets lower
    /// identically to their inner at C3.1 (constant-time codegen
    /// is deferred per ADR 0019 D12).
    secrets: Vec<SecretData>,
    /// ADR 0037 (2/N): the build-wide op-id base map (effect NAME →
    /// a graph-stable index), used by [`CodegenCtx::encode_op_id_ctx`]
    /// to decouple the runtime op id from this unit's LOCAL `EffectId`
    /// so cross-UNIT `perform`/`handle` agree. Empty (single-file /
    /// merged / corpus) → fall back to the local `EffectId.0`,
    /// byte-identical to the standalone [`encode_op_id`].
    op_id_base: HashMap<String, u32>,
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
    /// D.2 / ADR 0033 D5: `sentinel_str_eq(a: ptr, a_len: i64, b: ptr,
    /// b_len: i64) -> i1` — equal length + byte-wise array equality,
    /// the lexer's keyword/identifier matcher.
    str_eq_fn: FunctionValue<'ctx>,
    /// D.3 / ADR 0034 D7: `sentinel_realloc(ptr, i64) -> ptr` — grows a
    /// `Vec`'s data buffer in `push` (and serves the first push, since
    /// `realloc(null, n) == malloc(n)`).
    realloc_fn: FunctionValue<'ctx>,
    /// D.4 / ADR 0035 D6: `sentinel_read_file(path_ptr, path_len,
    /// out_len: *i64) -> data_ptr` — read a whole file into a fresh
    /// `[u8]` buffer (length written to `*out_len`).
    read_file_fn: FunctionValue<'ctx>,
    /// D.4 / ADR 0035 D6: `sentinel_write_file(path_ptr, path_len,
    /// data_ptr, data_len) -> i64` — create/truncate + write a file.
    write_file_fn: FunctionValue<'ctx>,
    /// D.4 (2/N) / ADR 0035 D4: `sentinel_print_bytes(data_ptr, data_len)
    /// -> i64` — write a `[u8]` to stdout (byte companion to `print`).
    print_bytes_fn: FunctionValue<'ctx>,
    /// C5.4 (2/N) / ADR 0028: `sentinel_arena_enter(i64) -> *arena`
    /// creates a per-scope broker bump arena (capacity 0 → default).
    arena_enter_fn: FunctionValue<'ctx>,
    /// C5.4 (2/N) / ADR 0028: `sentinel_arena_alloc(*arena, i64) -> *u8`
    /// bump-allocates an arena-routed binding's heap buffer.
    arena_alloc_fn: FunctionValue<'ctx>,
    /// C5.4 (2/N) / ADR 0028: `sentinel_arena_exit(*arena)` destroys a
    /// scope's arena, bulk-freeing every allocation made in it. Emitted
    /// once per scope that routed ≥1 allocation, replacing that scope's
    /// per-binding `sentinel_free`s.
    arena_exit_fn: FunctionValue<'ctx>,
    /// C3.5(a) / ADR 0020 D7: `sentinel_perform_op(op_id: i32,
    /// arg: i64) -> ptr` allocates a Kont struct tagged with the
    /// op id + its arg, returning the pointer to the caller.
    perform_op_fn: FunctionValue<'ctx>,
    /// C3.5(a) / ADR 0020 D7: `sentinel_kont_resume(kont: ptr,
    /// value: i64) -> i64` resumes a captured continuation. At
    /// C3.5(a) the restricted case means no captured frames —
    /// resume frees the kont and returns `value`.
    kont_resume_fn: FunctionValue<'ctx>,
    /// C3.5(b) / ADR 0020 D7: `sentinel_kont_pure(value: i64)
    /// -> ptr` wraps a pure value in a kont with op_id =
    /// PURE_RETURN_OP_ID. Effecting fns whose body is pure still
    /// have to return a Kont* per the new ABI; this is how.
    kont_pure_fn: FunctionValue<'ctx>,
    /// C3.5(b) / ADR 0020 D7: `sentinel_kont_consume_pure(kont:
    /// ptr) -> i64` reads the wrapped value from a pure-return
    /// kont and frees the kont. Symmetric to [`kont_pure_fn`];
    /// invoked from handle codegen's runtime switch's "pure
    /// return" case.
    kont_consume_pure_fn: FunctionValue<'ctx>,
    /// C3.5(c) / ADR 0020 D7: `sentinel_kont_push(kont, resumer,
    /// captured)` adds a captured evaluation frame to the kont's
    /// chain. Emitted at let-stmts whose RHS produces a kont.
    kont_push_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D7: `sentinel_task_spawn(wrapper, args,
    /// size) -> *Task` spawns an OS thread running `wrapper`.
    task_spawn_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D7: `sentinel_task_await(task) -> i64`
    /// joins + reads the spawned fn's result.
    task_await_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D7: `sentinel_scope_enter() -> *ScopeCtx`.
    scope_enter_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D7: `sentinel_scope_register(scope, task)`
    /// transfers task ownership to the enclosing scope.
    scope_register_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D7: `sentinel_scope_exit(scope)` auto-awaits
    /// + frees the scope's owned tasks.
    scope_exit_fn: FunctionValue<'ctx>,
    /// C4.4 / ADR 0024 D8: per-spawn-target wrapper fns, keyed by
    /// the spawned fn's FnId. One wrapper per unique target (not
    /// per spawn site). Synthesized in `compile_to_object`'s
    /// pre-walk (BEFORE this struct exists — wrapper emission needs
    /// `&module`, which CodegenCtx doesn't hold); looked up at each
    /// spawn site to pass as the `sentinel_task_spawn` fn pointer.
    spawn_wrappers: HashMap<FnId, FunctionValue<'ctx>>,
    /// C4.4 / ADR 0024 D8: the enclosing `scope concurrent`'s
    /// ScopeCtx*, if any. `Some` while lowering a scope body so
    /// each `spawn` inside also calls `sentinel_scope_register`.
    /// Saved + restored around nested scopes.
    current_scope: Option<PointerValue<'ctx>>,
    /// C3.5(c) / ADR 0020 D7: per-fn resumer fn + captured-var
    /// set for effecting fns matching the let-shape. Pre-
    /// declared in compile_to_object's pass 1 so compile_fn can
    /// look them up by FnId; emitted as a side-effect of
    /// compile_fn alongside the parent fn body.
    let_resumers: HashMap<FnId, (FunctionValue<'ctx>, Vec<VarId>)>,
    /// C3.5(d) / ADR 0020 D7: per-fn resumer for the embedded-
    /// perform shape — `stmts.len() == 0` and tail contains
    /// exactly one Perform. The stored TypedExpr is the
    /// substituted tail (perform replaced with Var(placeholder)),
    /// emitted as the resumer body. The VarId is the synthetic
    /// placeholder bound to the resumed value at the resumer's
    /// entry.
    embedded_perform_resumers:
        HashMap<FnId, (FunctionValue<'ctx>, Vec<VarId>, TypedExpr, VarId)>,
    /// C3.5(e) / ADR 0020 D7: per-fn N resumers for the chained-
    /// effecting-lets shape — `stmts.len() >= 2`, every stmt is
    /// `let v: i64 = perform Op(...)` (or call-to-effecting-fn),
    /// tail is pure. Each entry is `(resumer_fns,
    /// captures_per_resumer)` where both vectors have length N
    /// (one entry per let). Resumer-i runs after let-i's RHS
    /// resumes; its body emits `let_{i+1} = perform ...; ...; tail`
    /// (the rest of the chain).
    chained_lets_resumers:
        HashMap<FnId, (Vec<FunctionValue<'ctx>>, Vec<Vec<VarId>>)>,
    /// C3.5(e) / ADR 0020 D7: per-handle dispatch context, pushed
    /// by [`Self::lower_handle`] and consulted by
    /// [`Self::lower_resume_kont`] so a `k(v)` call site whose
    /// resume bubbles can branch back to the enclosing handle's
    /// dispatch loop. `loop_block` is the dispatch switch's parent
    /// block; `current_kont_slot` is an alloca holding the kont
    /// the switch reads next iteration.
    handle_stack: Vec<HandleContext<'ctx>>,
    /// C3.6(b) / ADR 0020 D7: per-fn nesting depth of `handle`
    /// expressions currently being lowered. Incremented at
    /// [`Self::lower_handle`] entry, decremented at exit.
    /// `depth > 1` means the current handle is nested inside
    /// another — in that case the lowering emits Kont*-typed
    /// merge values (arms wrap i64 via sentinel_kont_pure;
    /// switch's default propagates the un-caught op kont to
    /// the merge) so the enclosing outer handle can dispatch
    /// on the result per ADR 0020 D3 deep-handler semantics.
    handle_depth: u32,
    current_fn: Option<FunctionValue<'ctx>>,
    /// C2.4: the FnId of the fn currently being compiled. Used to
    /// look up the moved-source set from `drop_plan` at scope-exit
    /// drop emission.
    current_fn_id: FnId,
    vars: HashMap<VarId, (PointerValue<'ctx>, Type)>,
    /// C2.4 / ADR 0017 D8: stack of scopes; each scope holds the
    /// ordered list of VarIds declared in it. At scope exit
    /// (block-pop, fn return), the codegen iterates these in
    /// reverse to emit drop calls for heap-backed bindings that
    /// weren't moved.
    scope_stack: Vec<ScopeFrame<'ctx>>,
    /// C2.4: per-fn moved-source sets from the borrow checker.
    /// Codegen looks up `current_fn_id` to determine which
    /// bindings should be skipped at scope-exit drop emission
    /// (the destination of the move owns the value now).
    drop_plan: &'plan DropPlan,
    /// C5.4 (2/N) / ADR 0028: the program-wide set of binding VarIds
    /// whose heap allocation is routed into a scope arena instead of
    /// libc `sentinel_alloc`/`free`. Computed once up front by
    /// [`compute_arena_routed`] = exactly the bindings `emit_scope_drops`
    /// would `sentinel_free` (`∉ moved ∧ ≠ tail_returned`), restricted to
    /// `let x = [primitive array literal]` in non-generic, non-effecting
    /// fns. This *one* set drives BOTH the alloc-routing ([`lower_stmt`])
    /// and the free-skip ([`emit_scope_drops`]) so they cannot diverge.
    /// VarIds are globally unique, so a single program-wide set is
    /// unambiguous regardless of which fn is being compiled.
    arena_routed: HashSet<VarId>,
    /// C5.4 (2/N) / ADR 0028: transient flag set by [`lower_stmt`] while
    /// lowering the RHS of an arena-routed `let x = [array literal]`, so
    /// [`Self::lower_array_lit`] routes the data buffer's allocation to
    /// the current scope's arena. Consumed (reset to false) by
    /// `lower_array_lit` before it lowers the elements, so a nested array
    /// literal in an element does not inherit the routing.
    array_route_active: bool,
    /// Phase D.5 / ADR 0036 D4: how many `while` bodies enclose the
    /// current lowering point. When > 0, per-binding allocas are placed
    /// in the function **entry block** (executed once) rather than inline
    /// in the loop body (where they would run — and grow the stack — every
    /// iteration, overflowing at large iteration counts). Bumped around
    /// `lower_block(while-body)`. Zero for non-loop code, so that codegen
    /// is byte-identical to pre-D.5 (the c51 repro bar).
    loop_depth: u32,
    /// Phase D.5 (2/N) / ADR 0036 D9: stack of enclosing `while` loops'
    /// branch targets ([`LoopTarget`]). Pushed entering a `while` body,
    /// popped on exit; `break` / `continue` lower against the top (the
    /// innermost loop). Empty outside any loop — the type checker has
    /// already rejected `break` / `continue` there, so `lower_stmt` can
    /// assume a non-empty stack at those arms.
    loop_targets: Vec<LoopTarget<'ctx>>,
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
///
/// C2.5(a): does the given field type contain anything the drop
/// machinery needs to free? Used by [`emit_drop_struct_fields`]
/// to skip pure-data fields cheaply.
///
///   * `Array(_)` → owns heap memory; drop frees it.
///   * `Nullable(Struct | GenericInstance)` → owns a heap payload
///     conditionally; drop frees it when valid.
///   * `Struct(_)` / `GenericInstance(_)` → recurse only if some
///     transitive field needs drop. Otherwise the struct is pure
///     data and can be skipped.
///   * Everything else (primitives, refs, `?primitive`) → no drop.
fn field_type_needs_drop(ty: Type, program: &TypedProgram) -> bool {
    field_type_needs_drop_inner(ty, program, &mut Vec::new())
}

/// C4.4 / ADR 0024 D8: collect the FnId of every `spawn fn(args)`
/// target reachable in a block. One wrapper is synthesized per
/// unique target (deduped by the caller). The type checker
/// guarantees a `Spawn`'s inner is a `Call`, so non-Call inners
/// are ignored defensively.
fn collect_spawn_targets_block(block: &TypedBlock, acc: &mut Vec<FnId>) {
    for stmt in &block.stmts {
        match &stmt.kind {
            TypedStmtKind::Let { value, .. } => collect_spawn_targets_expr(value, acc),
            TypedStmtKind::Assign { target, value } => {
                collect_spawn_targets_expr(target, acc);
                collect_spawn_targets_expr(value, acc);
            }
            // Phase D.5 / ADR 0036: recurse into the loop's cond + body.
            TypedStmtKind::While { cond, body } => {
                collect_spawn_targets_expr(cond, acc);
                collect_spawn_targets_block(body, acc);
            }
            // D.5 (2/N): payload-free loop control — no spawn target.
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
            TypedStmtKind::Expr(e) => collect_spawn_targets_expr(e, acc),
        }
    }
    collect_spawn_targets_expr(&block.tail, acc);
}

fn collect_spawn_targets_expr(expr: &TypedExpr, acc: &mut Vec<FnId>) {
    match &expr.kind {
        TypedExprKind::Spawn { call, .. } => {
            if let TypedExprKind::Call { id, args, .. } = &call.kind {
                acc.push(*id);
                for a in args {
                    collect_spawn_targets_expr(a, acc);
                }
            }
        }
        TypedExprKind::Scope { body, .. } => collect_spawn_targets_block(body, acc),
        TypedExprKind::Await { task_expr, .. } => collect_spawn_targets_expr(task_expr, acc),
        // Phase D.1 / ADR 0032 (3/N): recurse into construction args +
        // the scrutinee / arm bodies (a `spawn` could nest inside).
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        TypedExprKind::Match { scrutinee, arms, .. } => {
            collect_spawn_targets_expr(scrutinee, acc);
            for arm in arms {
                collect_spawn_targets_expr(&arm.body, acc);
            }
        }
        TypedExprKind::Block(b) => collect_spawn_targets_block(b, acc),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            collect_spawn_targets_expr(cond, acc);
            collect_spawn_targets_block(then_branch, acc);
            collect_spawn_targets_block(else_branch, acc);
        }
        TypedExprKind::Call { args, .. } => {
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => collect_spawn_targets_expr(inner, acc),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            collect_spawn_targets_expr(l, acc);
            collect_spawn_targets_expr(r, acc);
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_spawn_targets_expr(f, acc);
            }
        }
        TypedExprKind::FieldAccess { target, .. } => collect_spawn_targets_expr(target, acc),
        TypedExprKind::ArrayLit { elements, .. } => {
            for e in elements {
                collect_spawn_targets_expr(e, acc);
            }
        }
        TypedExprKind::Index { target, index, .. } => {
            collect_spawn_targets_expr(target, acc);
            collect_spawn_targets_expr(index, acc);
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            collect_spawn_targets_expr(body, acc);
            for arm in arms {
                collect_spawn_targets_expr(&arm.body, acc);
            }
            if let Some(ra) = return_arm {
                collect_spawn_targets_expr(&ra.body, acc);
            }
        }
        TypedExprKind::Perform { args, .. } | TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        TypedExprKind::MethodCall { target, args, .. }
        | TypedExprKind::ImplMethodCall { target, args, .. } => {
            collect_spawn_targets_expr(target, acc);
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        TypedExprKind::ClassInit { args, .. } | TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => {}
    }
}

fn field_type_needs_drop_inner(
    ty: Type,
    program: &TypedProgram,
    seen: &mut Vec<Type>,
) -> bool {
    if seen.contains(&ty) {
        // Cycle guard. Recursive structs go through `?Struct`
        // which we count as needing drop at the `Nullable` arm
        // below, so the recursion terminates without ever
        // re-entering the same nominal struct directly.
        return false;
    }
    match ty {
        Type::Array(_) => true,
        // Phase D.3 / ADR 0034 D7: a `Vec<T>` owns a heap buffer, so it
        // always needs a scope-exit drop (the buffer free), like `[T]`.
        Type::Vec(_) => true,
        Type::Nullable(NullableInner::Struct(_))
        | Type::Nullable(NullableInner::GenericInstance(_)) => true,
        Type::Struct(id) => {
            seen.push(ty);
            let decl = program.struct_decl(id);
            let any = decl
                .fields
                .iter()
                .any(|f| field_type_needs_drop_inner(f.ty, program, seen));
            seen.pop();
            any
        }
        Type::GenericInstance(id) => {
            if (id.0 as usize) >= program.generic_instances.len() {
                return false;
            }
            seen.push(ty);
            let inst = program.generic_instance(id);
            let decl = program.struct_decl(inst.struct_id);
            let mut local_instances = program.generic_instances.clone();
            let mut local_refs = program.refs.clone();
            let any = decl.fields.iter().any(|f| {
                let concrete =
                    f.ty.substitute(&inst.args, &mut local_instances, &mut local_refs);
                field_type_needs_drop_inner(concrete, program, seen)
            });
            seen.pop();
            any
        }
        // Phase D.2 / ADR 0033 D4: a `u8` scalar has no heap payload.
        Type::Nullable(_) | Type::I64 | Type::I32 | Type::U8 | Type::Bool | Type::Ref(_) => false,
        Type::TypeParam(_) => false,
        // C3 / ADR 0019 D5 (C3.1): drop semantics of `secret T`
        // follow the inner — secrets don't introduce new heap
        // payloads. Unwrap and recurse.
        Type::Secret(id) => {
            if (id.0 as usize) >= program.secrets.len() {
                return false;
            }
            field_type_needs_drop_inner(program.secret_data(id).inner, program, seen)
        }
        // C3.4 / ADR 0020 D5: konts never reach codegen at C3.4
        // minimum — Handle/Perform/ResumeKont bail with
        // HandlersNotYetSupported before drop computation. Defensive
        // false here keeps borrow-check's recursive drop walks safe.
        Type::Kont(_) => false,
        // C4.4 / ADR 0024 D8: Task cleanup is the runtime's job
        // (sentinel_task_await / sentinel_scope_exit free the Task);
        // no codegen-emitted drop. A Task value held past its scope
        // is reclaimed at scope_exit.
        Type::Task(_) => false,
        // Phase D.1 / ADR 0032 D6 (4/N): an enum owns its heap-boxed
        // payload, so it needs drop (free the payload box) iff *some*
        // variant carries a payload — a pure-unit enum (every variant
        // `null`-payloaded, C-enum-style) allocates nothing and stays
        // drop-free. No recursion here (so no cycle hazard for
        // recursive enums); the per-variant payload-field recursion is
        // a follow-on (needs synthesized drop fns — see
        // `emit_drop_for_binding`).
        Type::Enum(id) => {
            (id.0 as usize) < program.enums.len()
                && program
                    .enum_data(id)
                    .variants
                    .iter()
                    .any(|v| !v.payloads.is_empty())
        }
        // C4.1 / ADR 0022 D9: class instances follow the same
        // drop-needs rule as structs (recurse into fields).
        Type::Class(id) => {
            if (id.0 as usize) >= program.class_decls.len() {
                return false;
            }
            seen.push(ty);
            let decl = program.class_decl(id);
            let any = decl
                .fields
                .iter()
                .any(|f| field_type_needs_drop_inner(f.ty, program, seen));
            seen.pop();
            any
        }
        // C4.2 / ADR 0023 D7: `Self` never reaches codegen — impl-
        // sig substitution resolves it before bodies type-check.
        Type::TraitSelf(_) => false,
    }
}

fn llvm_basic_type<'ctx>(
    context: &'ctx Context,
    ty: Type,
    struct_types: &HashMap<StructId, StructType<'ctx>>,
    generic_struct_types: &HashMap<GenericInstanceId, StructType<'ctx>>,
    class_types: &HashMap<ClassId, StructType<'ctx>>,
    secrets: &[SecretData],
) -> BasicTypeEnum<'ctx> {
    // C3 / ADR 0019 D5 (C3.1): `secret T` lowers identically to T
    // at C3.1. Constant-time codegen (branch-free `select`/`cmov`,
    // speculation barriers) is deferred to a follow-on ADR per
    // ADR 0019 D12.
    let ty = match ty {
        Type::Secret(id) => secrets[id.0 as usize].inner,
        other => other,
    };
    match ty {
        Type::Bool => context.bool_type().into(),
        Type::I32 => context.i32_type().into(),
        Type::I64 => context.i64_type().into(),
        // Phase D.2 / ADR 0033 D6: `u8` lowers to LLVM `i8` (signedness
        // lives in the ops — `udiv`/unsigned compares — not the type).
        Type::U8 => context.i8_type().into(),
        Type::Struct(id) => (*struct_types
            .get(&id)
            .expect("struct declared in pass 0"))
        .into(),
        Type::GenericInstance(id) => (*generic_struct_types
            .get(&id)
            .expect("generic instance declared in pass 0"))
        .into(),
        // C4.1 / ADR 0022 D9: class instances lower to LLVM
        // struct types — declared in pass 0 alongside structs.
        Type::Class(id) => (*class_types
            .get(&id)
            .expect("class declared in pass 0"))
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
                        class_types,
                        secrets,
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
        Type::Vec(_) => {
            // Phase D.3 / ADR 0034 D3: `Vec<T>` lowers as `{ i64 len,
            // i64 cap, ptr data }` — the `[T]` layout (`{ i64 len, ptr
            // data }`) with a capacity field inserted, so the data
            // pointer is FIELD 2 (not field 1 like an array). Element
            // type is tracked at the Sentinel-types level; LLVM sees an
            // opaque pointer.
            let len_ty: BasicTypeEnum = context.i64_type().into();
            let cap_ty: BasicTypeEnum = context.i64_type().into();
            let data_ty: BasicTypeEnum =
                context.ptr_type(inkwell::AddressSpace::default()).into();
            context.struct_type(&[len_ty, cap_ty, data_ty], false).into()
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
        // Unreachable: the early-return at fn entry strips
        // Type::Secret. If it shows up here, that's a codegen bug.
        Type::Secret(_) => unreachable!("Type::Secret stripped at llvm_basic_type entry"),
        // C3.5(a) / ADR 0020 D7: konts lower to opaque pointers.
        // The underlying SentinelKont struct's fields are read
        // via getelementptr at known offsets — codegen doesn't
        // need an LLVM-level struct type for it.
        Type::Kont(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // C4.4 / ADR 0024 D8: Task lowers to an opaque pointer
        // (*SentinelTask); the runtime owns the struct layout.
        Type::Task(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // Phase D.1 / ADR 0032 D4: an enum lowers to the abi-v1
        // `{ i32 tag, ptr payload }` — a 4-byte discriminant (variant
        // index, source order) + an opaque pointer to a heap-allocated
        // struct of the active variant's payload fields (`null` for a
        // unit variant). Heap-boxing the payload (like `?Struct`) is
        // what makes *recursive* enums representable (an AST node
        // references itself). This lets enum-typed signatures lower at
        // D.1 (3/N); construction + `match` lowering land at (4/N).
        Type::Enum(_) => {
            let tag_ty: BasicTypeEnum = context.i32_type().into();
            let payload_ty: BasicTypeEnum =
                context.ptr_type(inkwell::AddressSpace::default()).into();
            context.struct_type(&[tag_ty, payload_ty], false).into()
        }
        // C4.2: TraitSelf is impossible at codegen — impl-sig
        // substitution resolves it to a concrete type before
        // codegen sees it.
        Type::TraitSelf(_) => {
            unreachable!("Type::TraitSelf must be substituted before codegen")
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
// Reused by the `snc llvm` oracle (`sentinel-driver/src/llvm_dump.rs`) so its
// monomorphic-instance discovery is byte-faithful to the inkwell backend's — pure
// logic over the TypedProgram (no inkwell types), so the textual oracle and the
// production codegen monomorphize the same set in the same order.
pub fn collect_mono_instantiations(
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
            // Phase D.5 / ADR 0036: walk the loop's cond + body so any
            // generic call inside is monomorphised.
            TypedStmtKind::While { cond, body } => {
                walk_expr_for_mono(
                    cond, subst, program, instances, refs, visited, order, pending,
                );
                walk_block_for_mono(
                    body, subst, program, instances, refs, visited, order, pending,
                );
            }
            // D.5 (2/N): payload-free loop control — no generic call.
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
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
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => {}
        TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => walk_expr_for_mono(
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
        // C3.4 / ADR 0020 D5: handle/perform/resume never reach
        // user-fn-call inspection because lower_expr bails before
        // any concrete substitution. Defensive: walk subexpressions
        // so the mono pass remains correct for any generic uses
        // tucked inside arms — even though codegen later rejects.
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            walk_expr_for_mono(
                body, subst, program, instances, refs, visited, order, pending,
            );
            for arm in arms {
                walk_expr_for_mono(
                    &arm.body, subst, program, instances, refs, visited, order, pending,
                );
            }
            if let Some(ra) = return_arm {
                walk_expr_for_mono(
                    &ra.body, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::Perform { args, .. } | TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        // C4.1: classes aren't generic at C4.1; walk children
        // mechanically so any nested generic fn calls in
        // class-instantiation arg expressions are still picked up
        // by the worklist.
        TypedExprKind::MethodCall { target, args, .. } => {
            walk_expr_for_mono(
                target, subst, program, instances, refs, visited, order, pending,
            );
            for a in args {
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::ClassInit { args, .. } => {
            for a in args {
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        // C4.2: impls aren't generic at C4.2 minimum; walk children.
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            walk_expr_for_mono(
                target, subst, program, instances, refs, visited, order, pending,
            );
            for a in args {
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_expr_for_mono(
                    a, subst, program, instances, refs, visited, order, pending,
                );
            }
        }
        // C4.4 / ADR 0024: recurse into concurrency-form children so
        // nested generic-call instantiations are still collected.
        TypedExprKind::Scope { body, .. } => walk_block_for_mono(
            body, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::Spawn { call, .. } => walk_expr_for_mono(
            call, subst, program, instances, refs, visited, order, pending,
        ),
        TypedExprKind::Await { task_expr, .. } => walk_expr_for_mono(
            task_expr, subst, program, instances, refs, visited, order, pending,
        ),
        // Phase D.1 / ADR 0032 (3/N): enums are non-generic at the MVP,
        // but a construction arg / arm body could still contain a
        // generic call to monomorphise — recurse into the children.
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                walk_expr_for_mono(a, subst, program, instances, refs, visited, order, pending);
            }
        }
        TypedExprKind::Match { scrutinee, arms, .. } => {
            walk_expr_for_mono(scrutinee, subst, program, instances, refs, visited, order, pending);
            for arm in arms {
                walk_expr_for_mono(&arm.body, subst, program, instances, refs, visited, order, pending);
            }
        }
    }
}

/// Phase D.6 / ADR 0037 D7 — the module-qualified `abi-v1` amendment.
/// Wrap an item's intra-module symbol (`item`) with its `module_path`
/// to form the cross-unit-unique external symbol.
///
/// - **Empty `module_path`** (a single-file program — the only case
///   until the per-unit separate-compilation back end lands) → the
///   bare `item`, byte-for-byte. Every single-file symbol is therefore
///   exactly what `abi-v1` emitted before this amendment, which is why
///   D7 is an *amendment*, not an `abi-v2` bump.
/// - **Non-empty** → `_S` + a length-prefixed segment per module-path
///   segment + a length-prefixed `item`, each `<decimal-byte-len><bytes>`
///   (Itanium-ish source-name encoding). The encoding is a prefix-free
///   code over the segment sequence `[seg…, item]`, so distinct
///   `(module, item)` pairs never collide; decoding is unambiguous
///   because no Sentinel identifier (nor any [`mangle_type`] tag) begins
///   with a digit. `_S` is reserved for Sentinel-emitted symbols.
///
/// `item` is the existing `abi-v1` §4 symbol — a free fn's source name,
/// [`mangle_mono_name`], or a class/impl method name — wrapped here as
/// ONE length-prefixed blob, so its internal `__` structure is unchanged
/// (this amendment closes the *module* collision surface, not the
/// intra-module one). The entry module's `main` and the `sentinel_*`
/// runtime symbols are passed through by their callers and never reach
/// this function with a non-empty path.
fn mangle_qualified(module_path: &[String], item: &str) -> String {
    if module_path.is_empty() {
        return item.to_string();
    }
    let mut s = String::with_capacity(item.len() + 8 * module_path.len() + 8);
    s.push_str("_S");
    for seg in module_path {
        s.push_str(&seg.len().to_string()); // byte length (identifiers are ASCII)
        s.push_str(seg);
    }
    s.push_str(&item.len().to_string());
    s.push_str(item);
    s
}

/// ADR 0037 (2/N) point 8: per-unit origin maps for cross-module NAMED types,
/// used to module-qualify their tags in `linkonce_odr` mono keys so two
/// distinct same-named types don't alias to one symbol. Empty maps → every
/// named type is treated as "local" → the importer-qualified per-unit
/// emission, byte-identical to before the fix.
#[derive(Default)]
pub struct NamedTypeOrigins {
    /// imported struct `StructId` → its defining module path.
    pub structs: HashMap<StructId, Vec<String>>,
    /// imported enum `EnumId` → its defining module path.
    pub enums: HashMap<EnumId, Vec<String>>,
}

/// ADR 0037 (2/N) `linkonce_odr`: may a mono instance over `args` be deduped
/// across units under an ORIGIN-qualified shared symbol? Yes iff every arg's
/// tag is GLOBALLY UNAMBIGUOUS, so two distinct instances can't alias to one
/// symbol and be wrongly merged:
///   - a PRIMITIVE (`i64` / `i32` / `bool` / `u8`) — no tag ambiguity;
///   - a CROSS-MODULE struct or enum whose origin is known — its tag is
///     module-qualified by [`mangle_type_dedup`] (point 8 fix);
///   - an array / nullable / vec of a safe element (recursively).
///
/// A LOCAL struct/enum (origin unknown — importer-specific) or any other named
/// type (class / generic-instance) is NOT safe and keeps the per-importer
/// emission. Empty origin maps → every named type is "local" → only primitives
/// dedup, matching the pre-point-8 behaviour exactly.
fn mono_args_dedup_safe(args: &[Type], origins: &NamedTypeOrigins) -> bool {
    fn safe(t: Type, origins: &NamedTypeOrigins) -> bool {
        match t {
            Type::I64 | Type::I32 | Type::Bool | Type::U8 => true,
            Type::Struct(id) => origins.structs.contains_key(&id),
            Type::Enum(id) => origins.enums.contains_key(&id),
            Type::Array(e) => safe(e.to_type(), origins),
            Type::Vec(e) => safe(e.to_type(), origins),
            Type::Nullable(inner) => safe(inner.to_type(), origins),
            _ => false,
        }
    }
    args.iter().all(|t| safe(*t, origins))
}

/// ADR 0037 (2/N) point 8: like [`mangle_mono_name`], but renders a
/// CROSS-MODULE named-type arg ORIGIN-qualified (via [`mangle_type_dedup`]) so
/// a `linkonce_odr` mono key is globally unambiguous. Only ever called for
/// dedup-safe args ([`mono_args_dedup_safe`]).
fn mangle_mono_name_dedup(
    base_name: &str,
    type_args: &[Type],
    program: &TypedProgram,
    origins: &NamedTypeOrigins,
) -> String {
    let mut s = String::with_capacity(base_name.len() + 16);
    s.push_str(base_name);
    for t in type_args {
        s.push_str("__");
        s.push_str(&mangle_type_dedup(*t, program, origins));
    }
    s
}

/// ADR 0037 (2/N) point 8: a [`mangle_type`] variant for `linkonce_odr` mono
/// keys that module-qualifies a CROSS-MODULE struct/enum tag (`util$geo$Point`,
/// from `origins`) so two same-named types from different modules don't alias.
/// Only the named-type + element-wrapping arms differ; everything else DELEGATES
/// to [`mangle_type`] (so the two stay in lock-step). The delegation is reached
/// only for primitives in practice — `mono_args_dedup_safe` gates the call to
/// primitives / origin-known named types / arrays-nullables-vecs of those, so no
/// ambiguous tag ever flows here.
fn mangle_type_dedup(ty: Type, program: &TypedProgram, origins: &NamedTypeOrigins) -> String {
    // `$`-join the origin path onto the bare name: a valid-in-identifier
    // separator that can't appear in a bare type name or path segment, so
    // distinct origins never collide. Empty origin → the bare name.
    let qualify = |origin: &[String], name: String| -> String {
        if origin.is_empty() {
            name
        } else {
            format!("{}${name}", origin.join("$"))
        }
    };
    match ty {
        Type::Struct(id) => {
            let name = program
                .structs
                .get(id.0 as usize)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("struct{}", id.0));
            match origins.structs.get(&id) {
                Some(origin) => qualify(origin, name),
                None => name,
            }
        }
        Type::Enum(id) => {
            let name = program
                .enums
                .get(id.0 as usize)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| format!("enum{}", id.0));
            match origins.enums.get(&id) {
                Some(origin) => qualify(origin, name),
                None => name,
            }
        }
        Type::Array(elem) => format!("arr_{}", mangle_type_dedup(elem.to_type(), program, origins)),
        Type::Vec(elem) => format!("vec_{}", mangle_type_dedup(elem.to_type(), program, origins)),
        Type::Nullable(inner) => {
            format!("opt_{}", mangle_type_dedup(inner.to_type(), program, origins))
        }
        _ => mangle_type(ty, program),
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
        // Phase D.2 / ADR 0033 D4: `u8` mangles as `u8` (so `[u8]`
        // mangles `arr_u8` via the Array arm below).
        Type::U8 => "u8".to_string(),
        Type::Struct(id) => program
            .structs
            .get(id.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("struct{}", id.0)),
        Type::Nullable(inner) => format!("opt_{}", mangle_type(inner.to_type(), program)),
        Type::Array(elem) => format!("arr_{}", mangle_type(elem.to_type(), program)),
        Type::Vec(elem) => format!("vec_{}", mangle_type(elem.to_type(), program)),
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
        // C3 / ADR 0019 D5: mangle `secret T` as `sec_T`. Secret
        // wrapping participates in the mono key so `id<secret i64>`
        // and `id<i64>` get distinct LLVM symbols.
        Type::Secret(id) => program
            .secrets
            .get(id.0 as usize)
            .map(|d| format!("sec_{}", mangle_type(d.inner, program)))
            .unwrap_or_else(|| format!("sec{}", id.0)),
        // C3.4 / ADR 0020 D5: konts never participate in
        // monomorphization keys at C3.4 — handlers don't reach
        // codegen until C3.5/C3.6. Defensive label only.
        Type::Kont(id) => format!("kont{}", id.0),
        // C4.4 / ADR 0024: Task<i64> carries no TypeParam so it
        // never appears in a mono key; defensive label only.
        Type::Task(id) => format!("task{}", id.0),
        // C4.1 / ADR 0022 D9: render class types by name.
        Type::Class(id) => program
            .class_decls
            .get(id.0 as usize)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("class{}", id.0)),
        // Phase D.1 / ADR 0032 D5: render enum types by name (so an
        // enum-typed fn signature mangles to a stable, readable symbol;
        // enums are non-generic at the MVP so this never appears in a
        // mono key).
        Type::Enum(id) => program
            .enums
            .get(id.0 as usize)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| format!("enum{}", id.0)),
        // C4.2: TraitSelf doesn't reach mangling.
        Type::TraitSelf(id) => format!("Self_trait{}", id.0),
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
        Type::I64
        | Type::I32
        // Phase D.2 / ADR 0033 D4: `u8` carries no TypeParam.
        | Type::U8
        | Type::Bool
        | Type::Struct(_)
        | Type::Class(_)
        // Phase D.1 / ADR 0032: enums are non-generic at the MVP.
        | Type::Enum(_)
        | Type::TraitSelf(_) => false,
        Type::Nullable(ni) => arg_contains_typeparam(ni.to_type(), instances, refs),
        Type::Array(ae) => arg_contains_typeparam(ae.to_type(), instances, refs),
        Type::Vec(ve) => arg_contains_typeparam(ve.to_type(), instances, refs),
        Type::GenericInstance(id) => instances[id.0 as usize]
            .args
            .iter()
            .any(|a| arg_contains_typeparam(*a, instances, refs)),
        Type::Ref(id) => arg_contains_typeparam(refs[id.0 as usize].inner, instances, refs),
        // C3 / ADR 0019 D5: `secret T` at C3.1 wraps a concrete
        // (non-TypeParam) inner — no recursion. Returns false.
        // Future ADRs may relax for `secret <generic-T>` and need
        // a `secrets: &[SecretData]` parameter here.
        Type::Secret(_) => false,
        // C3.4 / ADR 0020 D5: konts don't appear in mono keys.
        Type::Kont(_) => false,
        // C4.4 / ADR 0024: Task<i64> carries no TypeParam.
        Type::Task(_) => false,
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
        // Phase D.2 / ADR 0033 D6: `u8` is an int type → LLVM `i8`.
        Type::U8 => context.i8_type(),
        Type::Struct(_) => panic!("llvm_int_type called on non-int Type::Struct"),
        Type::Nullable(_) => panic!("llvm_int_type called on non-int Type::Nullable"),
        Type::Array(_) => panic!("llvm_int_type called on non-int Type::Array"),
        Type::Vec(_) => panic!("llvm_int_type called on non-int Type::Vec"),
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
        // C3 / ADR 0019 D5: `secret T` arithmetic operations use
        // the LLVM int type of T. The CodegenCtx wrapper strips
        // secrets before calling this. Reaching here is a bug.
        Type::Secret(_) => panic!(
            "llvm_int_type called on Type::Secret — strip via CodegenCtx::llvm_int_type"
        ),
        Type::Kont(_) => panic!(
            "llvm_int_type called on Type::Kont — handlers not lowered at C3.4 (ADR 0020 D9)"
        ),
        Type::Task(_) => panic!("llvm_int_type called on non-int Type::Task"),
        Type::Class(_) => panic!("llvm_int_type called on non-int Type::Class"),
        Type::Enum(_) => panic!("llvm_int_type called on non-int Type::Enum"),
        Type::TraitSelf(_) => {
            panic!("llvm_int_type called on Type::TraitSelf — must be substituted before codegen")
        }
    }
}

impl<'ctx, 'plan> CodegenCtx<'ctx, 'plan> {
    fn llvm_basic_type(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        llvm_basic_type(
            self.context,
            ty,
            &self.struct_types,
            &self.generic_struct_types,
            &self.class_types,
            &self.secrets,
        )
    }

    /// C3 / ADR 0019 D5 (C3.1): strip one layer of `secret` if
    /// present. Lets codegen treat `secret T` as T for the underlying
    /// LLVM type / arithmetic / control-flow decisions. Constant-
    /// time codegen for secret operations is deferred per ADR 0019
    /// D12.
    fn strip_secret(&self, ty: Type) -> Type {
        match ty {
            Type::Secret(id) => self.secrets[id.0 as usize].inner,
            other => other,
        }
    }

    fn llvm_int_type(&self, ty: Type) -> IntType<'ctx> {
        llvm_int_type(self.context, self.strip_secret(ty))
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
        self.scope_stack.push(ScopeFrame::default());

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
        self.emit_scope_drops(tail_returned, program)?;
        self.scope_stack.pop();

        // Monomorphic instances of `main<T>` are forbidden by ADR
        // 0016 D11 — the type checker rejects them. So no main-
        // truncation special-case is needed here.
        self.builder
            .build_return(Some(&body_val))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(())
    }

    /// C4.1 / ADR 0022 D9: emit a class's init + method bodies.
    /// Init has the synthetic `out_ptr` ABI — the first param is
    /// a pointer to caller-provided storage; field writes inside
    /// the body GEP into that storage. Methods receive `self_ptr`
    /// as the first param; `self.field` reads/writes GEP into it.
    fn compile_class(
        &mut self,
        cd: &ClassData,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        // Init body.
        if let Some(init_def) = &cd.init {
            let fn_value = *self
                .class_init_fns
                .get(&cd.id)
                .expect("declared in pass 1");
            self.current_fn = Some(fn_value);
            // class init bodies don't participate in DropPlan (no
            // FnId entry); use a placeholder current_fn_id. The
            // drop emission paths skip when no entry exists.
            self.vars.clear();
            self.scope_stack.clear();
            let entry = self.context.append_basic_block(fn_value, "entry");
            self.builder.position_at_end(entry);
            self.scope_stack.push(ScopeFrame::default());

            // self_ptr = first arg. Bind self_var_id directly to
            // this pointer (no extra alloca + store) so field
            // access GEPs against the actual class storage.
            let self_ptr = fn_value
                .get_nth_param(0)
                .expect("out_ptr arg present")
                .into_pointer_value();
            self.vars.insert(init_def.self_var_id, (self_ptr, Type::Class(cd.id)));

            // Init params get standard alloca + store treatment.
            for (i, param) in init_def.params.iter().enumerate() {
                let arg = fn_value
                    .get_nth_param((i + 1) as u32)
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

            // Lower stmts only — init bodies use a trailing
            // placeholder `0` per the C4.1 (1/N) workaround
            // (block.tail Option arrives later). We DON'T lower
            // the tail since it's just `0`.
            for stmt in &init_def.body.stmts {
                self.lower_stmt(stmt, program)?;
            }
            self.scope_stack.pop();
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }

        // Method bodies.
        for (m_idx, m) in cd.methods.iter().enumerate() {
            let fn_value = *self
                .class_method_fns
                .get(&(cd.id, m_idx))
                .expect("declared in pass 1");
            self.current_fn = Some(fn_value);
            self.vars.clear();
            self.scope_stack.clear();
            let entry = self.context.append_basic_block(fn_value, "entry");
            self.builder.position_at_end(entry);
            self.scope_stack.push(ScopeFrame::default());

            // self_ptr = first arg. Bind self_var_id directly.
            let self_ptr = fn_value
                .get_nth_param(0)
                .expect("self_ptr present")
                .into_pointer_value();
            self.vars.insert(m.self_var_id, (self_ptr, Type::Class(cd.id)));

            for (i, param) in m.params.iter().enumerate() {
                let arg = fn_value
                    .get_nth_param((i + 1) as u32)
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
            let body_val = self.lower_block(&m.body, program)?;
            self.scope_stack.pop();
            self.builder
                .build_return(Some(&body_val))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            // Silence unused-var warnings on SelfKind for now.
            let _ = m.self_kind;
            let _ = SelfKind::Shared;
        }
        Ok(())
    }

    /// C4.2 / ADR 0023 D9: emit each impl method's body. Mirrors
    /// `compile_class`'s method-emission loop — bind self_ptr +
    /// param allocas, lower the body via the standard machinery,
    /// return the body value. At C4.2 minimum impls aren't
    /// generic and methods aren't effecting, so the per-fn
    /// frame-reification machinery from C3.5(c/d/e) doesn't
    /// activate here.
    fn compile_impl(
        &mut self,
        imp: &ImplData,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let target_ty = match imp.target {
            sentinel_resolve::ImplTarget::Class(cid) => Type::Class(cid),
            sentinel_resolve::ImplTarget::Struct(sid) => Type::Struct(sid),
        };
        for (m_idx, m) in imp.methods.iter().enumerate() {
            let fn_value = *self
                .impl_method_fns
                .get(&(imp.id, m_idx))
                .expect("declared in pass 1");
            self.current_fn = Some(fn_value);
            self.vars.clear();
            self.scope_stack.clear();
            let entry = self.context.append_basic_block(fn_value, "entry");
            self.builder.position_at_end(entry);
            self.scope_stack.push(ScopeFrame::default());

            // self_ptr = first arg; bind directly (no extra
            // alloca + store) so self.field reads/writes GEP
            // straight into the receiver storage.
            let self_ptr = fn_value
                .get_nth_param(0)
                .expect("self_ptr present")
                .into_pointer_value();
            self.vars.insert(m.self_var_id, (self_ptr, target_ty));

            for (i, param) in m.params.iter().enumerate() {
                let arg = fn_value
                    .get_nth_param((i + 1) as u32)
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
            let body_val = self.lower_block(&m.body, program)?;
            self.scope_stack.pop();
            self.builder
                .build_return(Some(&body_val))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let _ = m.self_kind;
        }
        Ok(())
    }

    fn compile_fn(
        &mut self,
        fn_def: &TypedFnDef,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        // C3.5(c) / ADR 0020 D7: special-case the let-shape of an
        // effecting fn body. Single let with effecting RHS + pure
        // tail uses per-let frame reification: codegen emits a
        // resumer fn + captured-state struct so resume can replay
        // the let-body. Fall back to the C3.5(b) path otherwise.
        if let Some(info) = detect_let_shape(fn_def, program) {
            if let Some((resumer_fn, captured)) =
                self.let_resumers.get(&fn_def.id).cloned()
            {
                return self.compile_effecting_fn_with_let(
                    fn_def, &info, resumer_fn, &captured, program,
                );
            }
        }
        // C3.5(d) / ADR 0020 D7: embedded-perform shape — stmts
        // empty and tail contains exactly one perform somewhere
        // (binop, struct-lit field, index, etc). The resumer
        // body is the substituted tail with the perform replaced
        // by Var(placeholder); the parent body lowers JUST the
        // perform expression and pushes a frame.
        if let Some((resumer_fn, captured, substituted_tail, placeholder_id)) =
            self.embedded_perform_resumers.get(&fn_def.id).cloned()
        {
            return self.compile_effecting_fn_with_embedded_perform(
                fn_def,
                resumer_fn,
                &captured,
                &substituted_tail,
                placeholder_id,
                program,
            );
        }
        // C3.5(e) / ADR 0020 D7: chained-effecting-lets shape —
        // stmts.len() >= 2, all effecting lets, pure tail. N
        // resumer fns were pre-declared in pass 1; compile each
        // body in turn (each resumer i emits a let_{i+1} + push
        // resumer_{i+1}; the last resumer wraps the pure tail).
        if let Some(info) = detect_chained_effecting_lets_shape(fn_def, program) {
            if let Some((resumer_fns, captures_per_resumer)) =
                self.chained_lets_resumers.get(&fn_def.id).cloned()
            {
                return self.compile_effecting_fn_with_chained_lets(
                    fn_def,
                    &info,
                    &resumer_fns,
                    &captures_per_resumer,
                    program,
                );
            }
        }

        // C3.5(b) / ADR 0020 D7: validate that effecting fns
        // (non-empty effect_row) have the shape codegen can
        // lower at this sub-phase — body is a direct Perform,
        // a direct Call to another effecting fn, or a block
        // whose tail meets the same constraint with no
        // intermediate performing statement. Other forms need
        // per-evaluation-site frame reification (C3.5(c) / C3.6).
        let signature = program.signature(fn_def.id);
        if uses_kont_abi(signature, program) {
            validate_effecting_fn_body(fn_def, program)?;
        }

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
        self.scope_stack.push(ScopeFrame::default());

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
        self.emit_scope_drops(tail_returned, program)?;
        self.scope_stack.pop();

        let is_main = program.signature(fn_def.id).is_main;
        let is_effecting = uses_kont_abi(program.signature(fn_def.id), program);
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
        } else if is_effecting {
            // C3.5(b) / ADR 0020 D7: effecting fn ABI returns
            // Kont*. If body_val is already a pointer (tail was
            // Perform or Call-to-effecting), return as is. If
            // it's a value (pure tail), wrap via
            // sentinel_kont_pure so the caller's handle still
            // sees a kont (with op_id = PURE_RETURN_OP_ID).
            let ret_val: BasicValueEnum<'ctx> = if body_val.is_pointer_value() {
                body_val
            } else {
                let int_val = body_val.into_int_value();
                let call = self
                    .builder
                    .build_call(self.kont_pure_fn, &[int_val.into()], "pure_kont")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                call.try_as_basic_value()
                    .left()
                    .expect("sentinel_kont_pure returns ptr")
            };
            self.builder
                .build_return(Some(&ret_val))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        } else {
            self.builder
                .build_return(Some(&body_val))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }
        Ok(())
    }

    /// C3.5(c) / ADR 0020 D7: compile an effecting fn whose body
    /// matches the let-shape (single let with effecting RHS +
    /// pure tail). Emits two LLVM fns:
    ///
    /// 1. The parent fn (already declared in pass 1) with body:
    ///    allocate the captured-state struct, store fn-param
    ///    values, evaluate the let's effecting RHS to get a
    ///    Kont*, push a frame onto it referencing the resumer +
    ///    captured ptr, return the Kont*.
    /// 2. The resumer fn (already declared in pass 1) with body:
    ///    allocate stack slots for the let-bound var (filled
    ///    from the resumed value param) and each captured var
    ///    (loaded from the captured struct), lower the tail
    ///    expression, wrap the i64 result in a pure-return
    ///    Kont via sentinel_kont_pure, return the wrapped Kont.
    ///
    /// The captured struct is laid out as a sequence of i64
    /// fields, one per captured VarId, at byte offsets 0, 8,
    /// 16, ... Codegen uses i8-indexed GEP for layout stability.
    #[allow(clippy::too_many_arguments)]
    fn compile_effecting_fn_with_let(
        &mut self,
        fn_def: &TypedFnDef,
        info: &LetShapeInfo<'_>,
        resumer_fn: FunctionValue<'ctx>,
        captured_ids: &[VarId],
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        // --- Compile the resumer fn first ---
        // Save the parent fn's context so we can restore after.
        let saved_fn = self.current_fn;
        let saved_fn_id = self.current_fn_id;
        let saved_vars = std::mem::take(&mut self.vars);
        let saved_scope = std::mem::take(&mut self.scope_stack);

        self.current_fn = Some(resumer_fn);
        self.scope_stack.push(ScopeFrame::default());
        let resumer_entry = self.context.append_basic_block(resumer_fn, "entry");
        self.builder.position_at_end(resumer_entry);

        let i64_ty = self.context.i64_type();
        let i8_ty = self.context.i8_type();
        let value_param = resumer_fn
            .get_nth_param(0)
            .expect("resumer has value param")
            .into_int_value();
        let captured_param = resumer_fn
            .get_nth_param(1)
            .expect("resumer has captured param")
            .into_pointer_value();

        // Bind the let-bound VarId to the resumed value param.
        let v_alloca = self
            .builder
            .build_alloca(i64_ty, "resumed_value")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(v_alloca, value_param)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.vars.insert(info.let_id, (v_alloca, info.let_ty));

        // Load each captured field from the struct + alloca it.
        // The struct is laid out as i64[N] starting at offset 0.
        for (i, cap_id) in captured_ids.iter().enumerate() {
            let offset = i64_ty.const_int((i * 8) as u64, false);
            let elem_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(
                        i8_ty,
                        captured_param,
                        &[offset],
                        &format!("cap_ptr_{}", i),
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            let elem_val = self
                .builder
                .build_load(i64_ty, elem_ptr, &format!("cap_load_{}", i))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let cap_alloca = self
                .builder
                .build_alloca(i64_ty, &format!("cap_{}", i))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(cap_alloca, elem_val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(*cap_id, (cap_alloca, Type::I64));
        }

        // Lower the tail expression. Result is an i64 per the
        // detect_let_shape constraint.
        let tail_val = self.lower_expr(info.tail, program)?.into_int_value();

        // Wrap in a pure-return kont via sentinel_kont_pure.
        let pure_call = self
            .builder
            .build_call(self.kont_pure_fn, &[tail_val.into()], "pure_kont")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let pure_kont = pure_call
            .try_as_basic_value()
            .left()
            .expect("sentinel_kont_pure returns ptr");
        self.builder
            .build_return(Some(&pure_kont))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Restore the parent fn's CodegenCtx state.
        self.current_fn = saved_fn;
        self.current_fn_id = saved_fn_id;
        self.vars = saved_vars;
        self.scope_stack = saved_scope;

        // --- Compile the parent fn body ---
        let parent_fn = *self
            .fns
            .get(&fn_def.id)
            .expect("parent declared in pass 1");
        self.current_fn = Some(parent_fn);
        self.current_fn_id = fn_def.id;
        self.vars.clear();
        self.scope_stack.clear();
        self.scope_stack.push(ScopeFrame::default());

        let parent_entry = self.context.append_basic_block(parent_fn, "entry");
        self.builder.position_at_end(parent_entry);

        // Bind fn params.
        for (i, param) in fn_def.params.iter().enumerate() {
            let arg = parent_fn
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

        // Allocate the captured-state struct (sentinel_alloc). If
        // there are no captured vars, pass a null pointer to
        // sentinel_kont_push — the resumer will receive null and
        // ignore it.
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let captured_ptr: PointerValue<'ctx> = if captured_ids.is_empty() {
            ptr_ty.const_null()
        } else {
            let size = i64_ty.const_int((captured_ids.len() * 8) as u64, false);
            let alloc_call = self
                .builder
                .build_call(self.alloc_fn, &[size.into()], "captured_alloc")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let ptr = alloc_call
                .try_as_basic_value()
                .left()
                .expect("sentinel_alloc returns ptr")
                .into_pointer_value();
            // Store each captured var's current value at the
            // appropriate offset.
            for (i, cap_id) in captured_ids.iter().enumerate() {
                let offset = i64_ty.const_int((i * 8) as u64, false);
                let elem_ptr = unsafe {
                    self.builder
                        .build_in_bounds_gep(
                            i8_ty,
                            ptr,
                            &[offset],
                            &format!("cap_store_ptr_{}", i),
                        )
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                };
                let (cap_alloca, _ty) = *self
                    .vars
                    .get(cap_id)
                    .expect("captured var bound from fn params");
                let cap_val = self
                    .builder
                    .build_load(i64_ty, cap_alloca, &format!("cap_read_{}", i))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(elem_ptr, cap_val)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            }
            ptr
        };

        // Lower the let's RHS (perform OR call-to-effecting-fn).
        // The result is a *mut SentinelKont.
        let kont_val = self.lower_expr(info.rhs, program)?.into_pointer_value();

        // Push the captured frame onto the kont's chain.
        let resumer_ptr = resumer_fn.as_global_value().as_pointer_value();
        self.builder
            .build_call(
                self.kont_push_fn,
                &[kont_val.into(), resumer_ptr.into(), captured_ptr.into()],
                "",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Return the kont so the caller's handle catches it.
        self.builder
            .build_return(Some(&kont_val))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        Ok(())
    }

    /// C3.5(d) / ADR 0020 D7: compile an effecting fn whose body
    /// has shape `stmts.len() == 0` and tail contains exactly one
    /// embedded perform. The resumer fn body is the substituted
    /// tail (perform replaced with Var(placeholder)); the parent
    /// fn body lowers JUST the unique perform expression, pushes
    /// a frame referencing the resumer + captured-state struct,
    /// and returns the resulting Kont*.
    ///
    /// Captured state holds the values of in-scope vars
    /// referenced in the substituted tail (which excludes the
    /// perform's own subtree). MVP-restricted to i64 captures.
    #[allow(clippy::too_many_arguments)]
    fn compile_effecting_fn_with_embedded_perform(
        &mut self,
        fn_def: &TypedFnDef,
        resumer_fn: FunctionValue<'ctx>,
        captured_ids: &[VarId],
        substituted_tail: &TypedExpr,
        placeholder_id: VarId,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        // --- Compile the resumer fn ---
        let saved_fn = self.current_fn;
        let saved_fn_id = self.current_fn_id;
        let saved_vars = std::mem::take(&mut self.vars);
        let saved_scope = std::mem::take(&mut self.scope_stack);

        self.current_fn = Some(resumer_fn);
        self.scope_stack.push(ScopeFrame::default());
        let resumer_entry = self.context.append_basic_block(resumer_fn, "entry");
        self.builder.position_at_end(resumer_entry);

        let i64_ty = self.context.i64_type();
        let i8_ty = self.context.i8_type();
        let value_param = resumer_fn
            .get_nth_param(0)
            .expect("resumer has value param")
            .into_int_value();
        let captured_param = resumer_fn
            .get_nth_param(1)
            .expect("resumer has captured param")
            .into_pointer_value();

        // Bind the placeholder VarId to the resumed value.
        let v_alloca = self
            .builder
            .build_alloca(i64_ty, "resumed_value")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(v_alloca, value_param)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.vars.insert(placeholder_id, (v_alloca, Type::I64));

        // Load each captured field from the struct + alloca it.
        for (i, cap_id) in captured_ids.iter().enumerate() {
            let offset = i64_ty.const_int((i * 8) as u64, false);
            let elem_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(
                        i8_ty,
                        captured_param,
                        &[offset],
                        &format!("cap_ptr_{}", i),
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            let elem_val = self
                .builder
                .build_load(i64_ty, elem_ptr, &format!("cap_load_{}", i))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let cap_alloca = self
                .builder
                .build_alloca(i64_ty, &format!("cap_{}", i))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(cap_alloca, elem_val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(*cap_id, (cap_alloca, Type::I64));
        }

        // Lower the substituted tail. Result is i64 by MVP
        // restriction (only i64-returning ops are accepted, so
        // the placeholder + surrounding context produce i64).
        let tail_val = self.lower_expr(substituted_tail, program)?.into_int_value();

        // Wrap in a pure-return kont.
        let pure_call = self
            .builder
            .build_call(self.kont_pure_fn, &[tail_val.into()], "pure_kont")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let pure_kont = pure_call
            .try_as_basic_value()
            .left()
            .expect("sentinel_kont_pure returns ptr");
        self.builder
            .build_return(Some(&pure_kont))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Restore the parent fn's CodegenCtx state.
        self.current_fn = saved_fn;
        self.current_fn_id = saved_fn_id;
        self.vars = saved_vars;
        self.scope_stack = saved_scope;

        // --- Compile the parent fn body ---
        let parent_fn = *self
            .fns
            .get(&fn_def.id)
            .expect("parent declared in pass 1");
        self.current_fn = Some(parent_fn);
        self.current_fn_id = fn_def.id;
        self.vars.clear();
        self.scope_stack.clear();
        self.scope_stack.push(ScopeFrame::default());

        let parent_entry = self.context.append_basic_block(parent_fn, "entry");
        self.builder.position_at_end(parent_entry);

        // Bind fn params.
        for (i, param) in fn_def.params.iter().enumerate() {
            let arg = parent_fn
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

        // Allocate the captured-state struct.
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let captured_ptr: PointerValue<'ctx> = if captured_ids.is_empty() {
            ptr_ty.const_null()
        } else {
            let size = i64_ty.const_int((captured_ids.len() * 8) as u64, false);
            let alloc_call = self
                .builder
                .build_call(self.alloc_fn, &[size.into()], "captured_alloc")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let ptr = alloc_call
                .try_as_basic_value()
                .left()
                .expect("sentinel_alloc returns ptr")
                .into_pointer_value();
            for (i, cap_id) in captured_ids.iter().enumerate() {
                let offset = i64_ty.const_int((i * 8) as u64, false);
                let elem_ptr = unsafe {
                    self.builder
                        .build_in_bounds_gep(
                            i8_ty,
                            ptr,
                            &[offset],
                            &format!("cap_store_ptr_{}", i),
                        )
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                };
                let (cap_alloca, _ty) = *self
                    .vars
                    .get(cap_id)
                    .expect("captured var bound from fn params");
                let cap_val = self
                    .builder
                    .build_load(i64_ty, cap_alloca, &format!("cap_read_{}", i))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(elem_ptr, cap_val)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            }
            ptr
        };

        // Find + lower the unique perform expression in the
        // original (un-substituted) body. The perform's args
        // reference fn params, which are bound in the env above.
        let original_tail = &fn_def.body.tail;
        let perform_expr = find_unique_perform(original_tail)
            .expect("detect_embedded_perform_shape guarantees a unique perform");
        let (effect_id, op_index, args) = match &perform_expr.kind {
            TypedExprKind::Perform { effect_id, op_index, args, .. } => {
                (*effect_id, *op_index, args.as_slice())
            }
            _ => unreachable!("find_unique_perform returns a Perform"),
        };
        let kont_val = self
            .lower_perform(effect_id, op_index, args, program)?
            .into_pointer_value();

        // Push the captured frame.
        let resumer_ptr = resumer_fn.as_global_value().as_pointer_value();
        self.builder
            .build_call(
                self.kont_push_fn,
                &[kont_val.into(), resumer_ptr.into(), captured_ptr.into()],
                "",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Return the kont so the caller's handle catches it.
        self.builder
            .build_return(Some(&kont_val))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(())
    }

    /// C3.5(e) / ADR 0020 D7: compile an effecting fn whose body
    /// is a chain of N effecting lets followed by a pure tail.
    /// Emits N+1 LLVM fns:
    ///
    /// 1. The parent fn (already declared in pass 1): builds the
    ///    captured-state struct for resumer-0, lowers `lets[0]`'s
    ///    RHS to a Kont*, pushes (resumer-0, captured) onto it,
    ///    returns the Kont*.
    /// 2. Resumer-i (i in 0..N-1, already declared in pass 1):
    ///    binds `let_i` to its `value` param + each captured var
    ///    from the captured struct, builds the captured struct
    ///    for resumer-(i+1) (carrying forward let_i + any prior
    ///    captures), lowers `lets[i+1]`'s RHS to a Kont*, pushes
    ///    (resumer-(i+1), captured) onto it, returns the Kont*.
    /// 3. Resumer-(N-1) (the last resumer, already declared):
    ///    binds let_{N-1} + captures, lowers the pure tail
    ///    expression, wraps via `sentinel_kont_pure`, returns
    ///    the wrap.
    ///
    /// The runtime `sentinel_kont_resume` walks the frame chain
    /// during a handle's `k(v)` call; when an intermediate
    /// resumer returns a non-pure-return kont (a fresh
    /// `perform` from inside resumer-i's emitted body), the
    /// remaining frames migrate onto the new kont and the bubble
    /// re-dispatches per ADR 0020 D3 deep-handler semantics.
    #[allow(clippy::too_many_arguments)]
    fn compile_effecting_fn_with_chained_lets(
        &mut self,
        fn_def: &TypedFnDef,
        info: &ChainedLetsShapeInfo<'_>,
        resumer_fns: &[FunctionValue<'ctx>],
        captures_per_resumer: &[Vec<VarId>],
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let saved_fn = self.current_fn;
        let saved_fn_id = self.current_fn_id;
        let saved_vars = std::mem::take(&mut self.vars);
        let saved_scope = std::mem::take(&mut self.scope_stack);

        let i64_ty = self.context.i64_type();
        let i8_ty = self.context.i8_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());

        // --- Compile each resumer ---
        for level in 0..info.lets.len() {
            let resumer_fn = resumer_fns[level];
            let (let_id, let_ty, _rhs) = info.lets[level];
            let captures_in = &captures_per_resumer[level];

            self.current_fn = Some(resumer_fn);
            self.vars.clear();
            self.scope_stack.clear();
            self.scope_stack.push(ScopeFrame::default());

            let entry = self.context.append_basic_block(resumer_fn, "entry");
            self.builder.position_at_end(entry);

            // Bind let_id to the resumed `value` param.
            let value_param = resumer_fn
                .get_nth_param(0)
                .expect("resumer has value param")
                .into_int_value();
            let captured_param = resumer_fn
                .get_nth_param(1)
                .expect("resumer has captured param")
                .into_pointer_value();

            let v_alloca = self
                .builder
                .build_alloca(i64_ty, "resumed_value")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(v_alloca, value_param)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(let_id, (v_alloca, let_ty));

            // Unpack captures from the struct.
            for (i, cap_id) in captures_in.iter().enumerate() {
                let offset = i64_ty.const_int((i * 8) as u64, false);
                let elem_ptr = unsafe {
                    self.builder
                        .build_in_bounds_gep(
                            i8_ty,
                            captured_param,
                            &[offset],
                            &format!("cap_ptr_{}", i),
                        )
                        .map_err(|e| CodegenError::Builder(e.to_string()))?
                };
                let elem_val = self
                    .builder
                    .build_load(i64_ty, elem_ptr, &format!("cap_load_{}", i))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let cap_alloca = self
                    .builder
                    .build_alloca(i64_ty, &format!("cap_{}", i))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(cap_alloca, elem_val)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.vars.insert(*cap_id, (cap_alloca, Type::I64));
            }

            if level + 1 == info.lets.len() {
                // Last resumer: lower the pure tail + wrap.
                let tail_val =
                    self.lower_expr(info.tail, program)?.into_int_value();
                let pure_call = self
                    .builder
                    .build_call(self.kont_pure_fn, &[tail_val.into()], "pure_kont")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let pure_kont = pure_call
                    .try_as_basic_value()
                    .left()
                    .expect("sentinel_kont_pure returns ptr");
                self.builder
                    .build_return(Some(&pure_kont))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            } else {
                // Non-last resumer: build captures for the next
                // resumer, lower the next let's RHS (perform or
                // call-to-effecting), push the next resumer,
                // return the kont.
                let captures_for_next = &captures_per_resumer[level + 1];
                let next_captured_ptr = self
                    .emit_chained_lets_captures_alloc(captures_for_next, i64_ty, i8_ty, ptr_ty)?;
                let next_rhs = info.lets[level + 1].2;
                let kont_val = self
                    .lower_expr(next_rhs, program)?
                    .into_pointer_value();
                let next_resumer_ptr =
                    resumer_fns[level + 1].as_global_value().as_pointer_value();
                self.builder
                    .build_call(
                        self.kont_push_fn,
                        &[
                            kont_val.into(),
                            next_resumer_ptr.into(),
                            next_captured_ptr.into(),
                        ],
                        "",
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_return(Some(&kont_val))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
            }
        }

        // Restore parent fn state.
        self.current_fn = saved_fn;
        self.current_fn_id = saved_fn_id;
        self.vars = saved_vars;
        self.scope_stack = saved_scope;

        // --- Compile the parent fn body ---
        let parent_fn = *self
            .fns
            .get(&fn_def.id)
            .expect("parent declared in pass 1");
        self.current_fn = Some(parent_fn);
        self.current_fn_id = fn_def.id;
        self.vars.clear();
        self.scope_stack.clear();
        self.scope_stack.push(ScopeFrame::default());

        let parent_entry = self.context.append_basic_block(parent_fn, "entry");
        self.builder.position_at_end(parent_entry);

        // Bind fn params.
        for (i, param) in fn_def.params.iter().enumerate() {
            let arg = parent_fn
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

        // Build captures for resumer-0 + lower the first let's
        // RHS to a kont + push (resumer-0, captures).
        let captures_for_first = &captures_per_resumer[0];
        let captured_ptr = self.emit_chained_lets_captures_alloc(
            captures_for_first,
            i64_ty,
            i8_ty,
            ptr_ty,
        )?;
        let first_rhs = info.lets[0].2;
        let kont_val = self.lower_expr(first_rhs, program)?.into_pointer_value();
        let resumer_0_ptr = resumer_fns[0].as_global_value().as_pointer_value();
        self.builder
            .build_call(
                self.kont_push_fn,
                &[
                    kont_val.into(),
                    resumer_0_ptr.into(),
                    captured_ptr.into(),
                ],
                "",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_return(Some(&kont_val))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(())
    }

    /// C3.5(e) helper: allocate a captured-state struct of i64
    /// slots + populate from the corresponding currently-bound
    /// allocas. Returns null if the capture set is empty (the
    /// resumer ignores its captured param in that case).
    fn emit_chained_lets_captures_alloc(
        &mut self,
        captures: &[VarId],
        i64_ty: inkwell::types::IntType<'ctx>,
        i8_ty: inkwell::types::IntType<'ctx>,
        ptr_ty: inkwell::types::PointerType<'ctx>,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if captures.is_empty() {
            return Ok(ptr_ty.const_null());
        }
        let size = i64_ty.const_int((captures.len() * 8) as u64, false);
        let alloc_call = self
            .builder
            .build_call(self.alloc_fn, &[size.into()], "captured_alloc")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let ptr = alloc_call
            .try_as_basic_value()
            .left()
            .expect("sentinel_alloc returns ptr")
            .into_pointer_value();
        for (i, cap_id) in captures.iter().enumerate() {
            let offset = i64_ty.const_int((i * 8) as u64, false);
            let elem_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(
                        i8_ty,
                        ptr,
                        &[offset],
                        &format!("cap_store_ptr_{}", i),
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            let (cap_alloca, _ty) = *self
                .vars
                .get(cap_id)
                .expect("captured var bound from outer scope");
            let cap_val = self
                .builder
                .build_load(i64_ty, cap_alloca, &format!("cap_read_{}", i))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(elem_ptr, cap_val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }
        Ok(ptr)
    }

    /// Phase D.5 / ADR 0036 D4: allocate a stack slot for a binding /
    /// result value. Inside a `while` body (`loop_depth > 0`) the alloca
    /// is placed at the TOP of the function's entry block — executed once,
    /// the slot reused each iteration — so the stack does not grow per
    /// iteration (a loop body's inline alloca would, overflowing at large
    /// counts). Outside any loop it is built inline, byte-identical to
    /// pre-D.5 codegen.
    fn binding_alloca(
        &self,
        llvm_ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if self.loop_depth == 0 {
            return self
                .builder
                .build_alloca(llvm_ty, name)
                .map_err(|e| CodegenError::Builder(e.to_string()));
        }
        let current_fn = self.current_fn.expect("current_fn set");
        let entry = current_fn
            .get_first_basic_block()
            .expect("fn has an entry block");
        let tmp = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(first) => tmp.position_before(&first),
            None => tmp.position_at_end(entry),
        }
        tmp.build_alloca(llvm_ty, name)
            .map_err(|e| CodegenError::Builder(e.to_string()))
    }

    fn lower_stmt(
        &mut self,
        stmt: &TypedStmt,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            TypedStmtKind::Let { id, name, ty, value, .. } => {
                // C5.4 (2/N) / ADR 0028: if this binding is arena-routed
                // and its RHS is an array literal, signal `lower_array_lit`
                // to bump-allocate the data buffer in the current scope's
                // arena instead of libc. `lower_array_lit` consumes the
                // flag before lowering elements; reset here too defensively.
                self.array_route_active = self.arena_routed.contains(id)
                    && matches!(value.kind, TypedExprKind::ArrayLit { .. });
                let v = self.lower_expr(value, program)?;
                self.array_route_active = false;
                let llvm_ty = self.llvm_basic_type(*ty);
                // D.5 / ADR 0036 D4: hoist to the entry block inside loops.
                let alloca = self.binding_alloca(llvm_ty, name)?;
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
            TypedStmtKind::While { cond, body } => {
                // Phase D.5 / ADR 0036 D4: the first backward CFG branch.
                // Three blocks: loop_cond evaluates the condition and
                // branches to loop_body (true) or loop_after (false);
                // loop_body lowers the body as a scoped block — so its
                // bindings drop PER ITERATION (ADR 0036 D5, the leak-free
                // property) — then branches back to loop_cond (the
                // back-edge). All prior control flow merged forward.
                debug_assert_eq!(cond.ty, Type::Bool);
                let current_fn = self.current_fn.expect("current_fn set by compile_fn");
                let cond_bb = self.context.append_basic_block(current_fn, "loop_cond");
                let body_bb = self.context.append_basic_block(current_fn, "loop_body");
                let after_bb = self.context.append_basic_block(current_fn, "loop_after");

                // Enter the loop: branch into the condition.
                self.builder
                    .build_unconditional_branch(cond_bb)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;

                // Condition block.
                self.builder.position_at_end(cond_bb);
                let cond_i1 = self.lower_expr(cond, program)?.into_int_value();
                self.builder
                    .build_conditional_branch(cond_i1, body_bb, after_bb)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;

                // Body block (its value is discarded; lower_block emits
                // the per-iteration scope drops), then the back-edge.
                // `loop_depth` makes body allocas hoist to the entry block
                // (D4) so the stack does not grow per iteration.
                self.builder.position_at_end(body_bb);
                // D.5 (2/N) / ADR 0036 D9: register this loop's branch
                // targets so `break` / `continue` in the body lower against
                // them. `scope_floor` is the body frame's index — captured
                // NOW, before lower_block pushes it — so break/continue
                // drain every frame `>= scope_floor` before branching.
                self.loop_targets.push(LoopTarget {
                    cond_bb,
                    after_bb,
                    scope_floor: self.scope_stack.len(),
                });
                self.loop_depth += 1;
                let body_result = self.lower_block(body, program);
                self.loop_depth -= 1;
                self.loop_targets.pop();
                let _ = body_result?;
                self.builder
                    .build_unconditional_branch(cond_bb)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;

                // Continue after the loop.
                self.builder.position_at_end(after_bb);
            }
            TypedStmtKind::Break | TypedStmtKind::Continue => {
                // Phase D.5 (2/N) / ADR 0036 D9: branch out of (`break` →
                // `loop_after`) or around (`continue` → `loop_cond`) the
                // innermost enclosing loop. The type checker guarantees we
                // are inside one, so `loop_targets` is non-empty.
                let target = *self
                    .loop_targets
                    .last()
                    .expect("break/continue inside a loop (type-check invariant)");
                let dest_bb = if matches!(stmt.kind, TypedStmtKind::Break) {
                    target.after_bb
                } else {
                    target.cond_bb
                };
                // THE load-bearing (2/N) property: emit the per-iteration
                // drops for every scope frame from the current top down to
                // the loop body BEFORE branching. The branch to loop_after
                // / loop_cond skips lower_block's end-of-body
                // `emit_scope_drops`, so a body-scope heap binding live here
                // would otherwise leak. (This is exactly the body-end exit
                // drop, just emitted early — the fn-return early-exit shape,
                // ADR 0017.)
                self.emit_loop_exit_drops(target.scope_floor, program)?;
                let current_fn = self.current_fn.expect("current_fn set by compile_fn");
                self.builder
                    .build_unconditional_branch(dest_bb)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                // The branch terminates the current block. Any code after
                // `break;` / `continue;` in this block — a statement-only
                // body's synthesised unit tail, a dead trailing statement,
                // the if-arm store/merge in `lower_if` — is unreachable;
                // park the builder on a fresh block so we never append to a
                // terminated block. It has no live predecessors, so LLVM
                // discards it.
                let dead_bb = self.context.append_basic_block(current_fn, "after_loopctl");
                self.builder.position_at_end(dead_bb);
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
            TypedExprKind::Index { target, index, elem_ty } => {
                // `a[i] = v;` (ADR 0050): the lvalue address is the
                // bounds-checked element pointer — the same GEP the read
                // path computes; the `Assign` caller stores through it.
                self.lower_index_elem_ptr(target, index, *elem_ty, program)
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
        self.scope_stack.push(ScopeFrame::default());
        for stmt in &block.stmts {
            self.lower_stmt(stmt, program)?;
        }
        let val = self.lower_expr(&block.tail, program)?;
        // Skip dropping a Var binding that's returned via the
        // tail — the move tracking will mark it in
        // `moved_sources`, but we also conservatively guard here.
        let tail_returned = tail_returned_var(&block.tail);
        self.emit_scope_drops(tail_returned, program)?;
        self.scope_stack.pop();
        Ok(val)
    }

    /// C2.4 / ADR 0017 D8: emit drop calls for the un-moved heap-backed
    /// bindings of the current (top-of-stack) scope. The body lives in
    /// [`Self::emit_frame_drops`] (split out at D.5 (2/N) so `break` /
    /// `continue` can drain several frames at once).
    fn emit_scope_drops(
        &mut self,
        tail_returned: Option<VarId>,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let scope = self
            .scope_stack
            .last()
            .cloned()
            .unwrap_or_default();
        self.emit_frame_drops(&scope, tail_returned, program)
    }

    /// C2.4 / ADR 0017 D8: emit the drop calls for one scope `frame` —
    /// the per-binding `sentinel_free`s (reverse declaration order) plus
    /// the scope's single `sentinel_arena_exit` if it lazily created an
    /// arena. Iterates in reverse declaration order per Rust convention.
    /// Skips:
    ///   - Bindings in [`DropPlan::moved_sources_for`] for the current fn
    ///     (the consumer owns + drops them).
    ///   - The binding returned via the tail expression (if the tail is a
    ///     `Var(id)`) — passed via `tail_returned`.
    ///   - Arena-routed bindings (bulk-freed by the arena exit below).
    ///
    /// C2.5(a): takes `program` so [`emit_drop_struct_fields`] can resolve
    /// struct decls + generic-instance args to recurse through heap-backed
    /// fields.
    fn emit_frame_drops(
        &mut self,
        scope: &ScopeFrame<'ctx>,
        tail_returned: Option<VarId>,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        let moved = self.drop_plan.moved_sources_for(self.current_fn_id);
        // ADR 0046: the partial-move set (Move-typed fields consumed by value); the
        // drop of a partially-moved binding elides these fields (the consumer freed them).
        let moved_fields = self.drop_plan.moved_fields_for(self.current_fn_id);
        for &id in scope.vars.iter().rev() {
            if Some(id) == tail_returned {
                continue;
            }
            if moved.contains(&id) {
                continue;
            }
            // C5.4 (2/N) / ADR 0028: arena-routed bindings (a strict
            // subset of the frees that would happen here — see
            // [`compute_arena_routed`]) had their heap buffer placed in
            // this scope's arena; the single `sentinel_arena_exit` below
            // bulk-frees them, so skip the per-binding `sentinel_free`.
            // Routing and free-skip are driven by the same `arena_routed`
            // set, so they cannot diverge (no leak, no double-free).
            if self.arena_routed.contains(&id) {
                continue;
            }
            let (ptr, ty) = match self.vars.get(&id) {
                Some(&v) => v,
                None => continue,
            };
            // ADR 0046: this binding's partially-moved field indices (empty for the
            // common case) — `emit_drop_struct_fields` skips them.
            let id_moved_fields: BTreeSet<u32> = moved_fields
                .iter()
                .filter_map(|(b, f)| (*b == id).then_some(*f))
                .collect();
            self.emit_drop_for_binding(ptr, ty, program, &id_moved_fields)?;
        }
        // C5.4 (2/N) / ADR 0028: if this scope lazily created an arena
        // (i.e. routed ≥1 allocation), bulk-free it in one call. The
        // handle was captured when the arena was entered, on a block
        // that dominates this exit point, so it is valid here.
        if let Some(handle) = scope.arena {
            self.builder
                .build_call(self.arena_exit_fn, &[handle.into()], "")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }
        Ok(())
    }

    /// Phase D.5 (2/N) / ADR 0036 D9: drop every scope frame from the
    /// current top down to (and including) the loop-body frame at
    /// `scope_floor`, innermost first. Used by `break` / `continue` so the
    /// per-iteration drops still run on the early-exit path: branching to
    /// `loop_after` / `loop_cond` skips `lower_block`'s end-of-body
    /// `emit_scope_drops` for the body scope (and for any nested `if` /
    /// block scopes between it and the branch). No value is returned
    /// through a loop-control branch, so nothing is tail-exempted (`None`).
    ///
    /// The frames are NOT popped here — static lowering of the (now-dead)
    /// remainder of each block still pops them via their own `lower_block`,
    /// emitting a second, unreachable set of drops into the `after_loopctl`
    /// block. Each runtime path therefore frees a given binding exactly
    /// once (the early-exit drop here, or the body-end drop on fall-through
    /// — mutually exclusive blocks).
    fn emit_loop_exit_drops(
        &mut self,
        scope_floor: usize,
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        for i in (scope_floor..self.scope_stack.len()).rev() {
            let frame = self.scope_stack[i].clone();
            self.emit_frame_drops(&frame, None, program)?;
        }
        Ok(())
    }

    /// Emit a drop call for a binding stored at `ptr` with type
    /// `ty`. For heap-backed types, this loads the value from
    /// `ptr` and frees the embedded heap pointer.
    ///
    /// C2.5(a): `program` lets [`emit_drop_struct_fields`] resolve
    /// struct decls + substitute generic-instance args when
    /// recursing through fields.
    fn emit_drop_for_binding(
        &mut self,
        ptr: PointerValue<'ctx>,
        ty: Type,
        program: &TypedProgram,
        // ADR 0046: field indices of THIS binding that were partially moved (and so are
        // owned + freed by the consumer); `emit_drop_struct_fields` skips them. Empty for
        // a fully-live binding and for every nested (recursive) field drop.
        moved_fields: &BTreeSet<u32>,
    ) -> Result<(), CodegenError> {
        // C3 / ADR 0019 D5 (C3.1): unwrap one layer of `secret T`
        // before dispatching on shape. Drop semantics of secrets
        // follow the inner type.
        let ty = self.strip_secret(ty);
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
            Type::Vec(_) => {
                // Phase D.3 / ADR 0034 D7: `Vec<T>` is `{ i64 len, i64
                // cap, ptr data }`. Free the data buffer — the data
                // pointer is FIELD 2 (the capacity field is inserted
                // before it). `sentinel_free(null)` is a safe no-op, so
                // an unpushed `Vec` (data == null, from `vec_new`'s
                // `{ 0, 0, null }`) drops cleanly with no null guard.
                // For a `Vec` of PRIMITIVE elements (`u8`/`i64`/`i32`/
                // `bool` — the D.3 MVP, incl. the 2/N `String` =
                // `Vec<u8>`) this is leak-free: no element owns heap.
                // Per-element recursive drop for droppable elements
                // (`Vec<Struct>` / `Vec<[u8]>`) is deferred (ADR 0034
                // D8, the enum-A1-shaped follow-on).
                let llvm_ty = self.llvm_basic_type(ty);
                let val = self
                    .builder
                    .build_load(llvm_ty, ptr, "drop_vec")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_struct_value();
                let data = self
                    .builder
                    .build_extract_value(val, 2, "drop_vec_data")
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
            Type::Struct(_) | Type::GenericInstance(_) => {
                // C2.5(a): recursive field drop. Per-field GEP +
                // dispatch on the field's type — handles Array,
                // ?Struct/?GenericInstance, nested structs, and
                // nested generic instances.
                self.emit_drop_struct_fields(ptr, ty, program, moved_fields)?;
            }
            Type::Nullable(_)
            | Type::I64
            | Type::I32
            // Phase D.2 / ADR 0033 D4: a `u8` byte has no heap data.
            | Type::U8
            | Type::Bool
            | Type::Ref(_) => {
                // Primitives + refs + nullable-of-primitive: no
                // heap data to free.
            }
            Type::TypeParam(_) => {
                // Unreachable post-monomorphisation; defensive.
            }
            Type::Secret(_) => {
                // Unreachable: stripped at fn entry. Defensive.
            }
            Type::Kont(_) => {
                // C3.4 / ADR 0020 D5: konts never reach drop —
                // Handle/Perform/ResumeKont bail at lower_expr.
                // Defensive.
            }
            Type::Task(_) => {
                // C4.4 / ADR 0024 D8: Task cleanup is the runtime's
                // job (sentinel_task_await / sentinel_scope_exit
                // free the Task). No codegen-emitted drop.
            }
            Type::Class(_) => {
                // C4.1 / ADR 0022 D9: class drop reuses struct
                // recursive field drop machinery. Classes own
                // their fields and follow the standard pattern.
                self.emit_drop_struct_fields(ptr, ty, program, moved_fields)?;
            }
            Type::Enum(_) => {
                // Phase D.1 / ADR 0032 D6 (4/N): an enum owns its
                // heap-boxed payload. Load the `{ i32 tag, ptr payload }`;
                // if the payload is non-null, `sentinel_free` it. This
                // is the `?Struct` drop arm with a null-pointer test in
                // place of the validity bit. Recursive *payload-field*
                // drop (for heap-typed payloads / recursive enums, which
                // would leak nested boxes here) is a measured follow-on:
                // it needs synthesized per-enum drop fns, since inline
                // expansion of a recursive enum's drop is infinite.
                let llvm_ty = self.llvm_basic_type(ty);
                let val = self
                    .builder
                    .build_load(llvm_ty, ptr, "drop_enum")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_struct_value();
                let payload = self
                    .builder
                    .build_extract_value(val, 1, "drop_enum_payload")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_pointer_value();
                let is_null = self
                    .builder
                    .build_is_null(payload, "drop_enum_isnull")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let current_fn = self.current_fn.expect("current_fn set");
                let free_block = self.context.append_basic_block(current_fn, "drop_enum_free");
                let after_block = self.context.append_basic_block(current_fn, "drop_enum_after");
                self.builder
                    .build_conditional_branch(is_null, after_block, free_block)
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
            Type::TraitSelf(_) => {
                // C4.2: unreachable post-substitution; defensive.
            }
        }
        Ok(())
    }

    /// Helper for recursive drop of struct fields. GEPs into the
    /// struct at `ptr` field-by-field and emits drop for each
    /// heap-backed field. Used by both [`Type::Struct`] and
    /// [`Type::GenericInstance`] (concrete instances after
    /// monomorphisation are structurally the same).
    ///
    /// C2.5(a) closes the C2.4 known gap: a struct containing an
    /// array field (e.g., c16_array_in_struct's `Bag`) no longer
    /// leaks the inner array's heap data. For generic instances
    /// the field types are substituted with the instance's
    /// concrete args before checking heap-backedness.
    fn emit_drop_struct_fields(
        &mut self,
        ptr: PointerValue<'ctx>,
        ty: Type,
        program: &TypedProgram,
        // ADR 0046: field indices of the BINDING being dropped that were partially moved
        // (the consumer owns + freed them) — skip their drop. Non-empty only at the top
        // level of a partially-moved binding; a nested struct field carries no moves yet.
        moved_fields: &BTreeSet<u32>,
    ) -> Result<(), CodegenError> {
        // Resolve (decl, concrete_field_types, llvm_struct_ty).
        let (decl, field_tys, struct_llvm_ty): (
            &TypedStructDecl,
            Vec<Type>,
            StructType<'ctx>,
        ) = match ty {
            Type::Struct(id) => {
                let decl = program.struct_decl(id);
                let llvm = *self
                    .struct_types
                    .get(&id)
                    .expect("struct LLVM type declared in pass 0");
                let tys = decl.fields.iter().map(|f| f.ty).collect();
                (decl, tys, llvm)
            }
            Type::GenericInstance(id) => {
                let inst = program.generic_instance(id);
                let decl = program.struct_decl(inst.struct_id);
                let llvm = *self
                    .generic_struct_types
                    .get(&id)
                    .expect("generic instance LLVM type declared in pass 0");
                // Substitute each field's type with the instance's
                // concrete args. Use local clones of the program's
                // interner tables — substitution may push new
                // entries, but those are only needed transiently
                // for the resulting Type itself (we don't recurse
                // into yet-unseen GenericInstance IDs; defensive
                // skip below).
                let mut local_instances = program.generic_instances.clone();
                let mut local_refs = program.refs.clone();
                let tys = decl
                    .fields
                    .iter()
                    .map(|f| f.ty.substitute(&inst.args, &mut local_instances, &mut local_refs))
                    .collect();
                (decl, tys, llvm)
            }
            _ => return Ok(()),
        };

        for (idx, field_ty) in field_tys.iter().enumerate() {
            if !field_type_needs_drop(*field_ty, program) {
                continue;
            }
            // ADR 0046: a field consumed by value (partial move) is owned + freed by the
            // consumer — skip its drop here so we don't double-free.
            if moved_fields.contains(&(idx as u32)) {
                continue;
            }
            // GenericInstance whose ID is past program bounds —
            // arose from local substitution; LLVM type unavailable
            // so we can't GEP into it. Defensive skip; in practice
            // the type checker has already covered every type
            // that appears in the program.
            if let Type::GenericInstance(gid) = field_ty {
                if (gid.0 as usize) >= program.generic_instances.len() {
                    continue;
                }
            }
            let field_ptr = self
                .builder
                .build_struct_gep(struct_llvm_ty, ptr, idx as u32, "drop_fldptr")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            // ADR 0046: nested struct fields carry no tracked moves at the MVP (deep
            // paths deferred — D5), so the recursive drop gets an empty move set.
            self.emit_drop_for_binding(field_ptr, *field_ty, program, &BTreeSet::new())?;
        }
        let _ = decl; // captured for clarity; field types are what we used
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
        // D.5 / ADR 0036 D4: hoist to the entry block inside loops.
        let result = self.binding_alloca(llvm_result_ty, "ifresult")?;

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

    /// Phase D.1 / ADR 0032 D4: the LLVM type of a variant's heap-boxed
    /// payload — an anonymous struct `{ field0, field1, … }` of the
    /// variant's (already-resolved) payload types. The single source of
    /// truth for the payload layout, shared by construction (stores into
    /// it), `match` (GEP/loads out of it), and drop. Empty / zero-field
    /// for a unit variant (which is never heap-allocated).
    fn enum_payload_struct_type(
        &self,
        enum_id: EnumId,
        variant_index: usize,
        program: &TypedProgram,
    ) -> StructType<'ctx> {
        let variant = &program.enum_data(enum_id).variants[variant_index];
        let field_tys: Vec<BasicTypeEnum> =
            variant.payloads.iter().map(|t| self.llvm_basic_type(*t)).collect();
        self.context.struct_type(&field_tys, false)
    }

    /// Phase D.1 / ADR 0032 D4: lower `Enum::Variant(args)` to the abi-v1
    /// `{ i32 tag, ptr payload }` value. The payload is a heap-boxed
    /// struct of the variant's payload fields (`null` for unit variants
    /// — no allocation); the tag is the variant's discriminant (source
    /// order). Heap-boxing (reusing `sentinel_alloc` like `?Struct`)
    /// keeps recursive enums representable.
    fn lower_enum_construct(
        &mut self,
        enum_id: EnumId,
        variant_index: usize,
        args: &[TypedExpr],
        enum_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let enum_struct_ty = self.llvm_basic_type(enum_ty).into_struct_type(); // { i32, ptr }
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());

        // Payload pointer: null for unit variants; otherwise build the
        // payload struct value from the args and heap-box it.
        let payload_ptr: PointerValue<'ctx> = if args.is_empty() {
            ptr_ty.const_null()
        } else {
            let payload_struct_ty =
                self.enum_payload_struct_type(enum_id, variant_index, program);
            let mut agg = payload_struct_ty.get_undef();
            for (i, a) in args.iter().enumerate() {
                let v = self.lower_expr(a, program)?;
                agg = self
                    .builder
                    .build_insert_value(agg, v, i as u32, "enum_payload")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
                    .into_struct_value();
            }
            let size = payload_struct_ty.size_of().expect("payload struct is sized");
            let raw = self.alloc_call(size)?;
            self.builder
                .build_store(raw, agg)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            raw
        };

        let tag = i32_ty.const_int(variant_index as u64, false);
        let agg = enum_struct_ty.get_undef();
        let agg = self
            .builder
            .build_insert_value(agg, tag, 0, "enum_tag")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_struct_value();
        let agg = self
            .builder
            .build_insert_value(agg, payload_ptr, 1, "enum_box")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_struct_value();
        Ok(agg.into())
    }

    /// Phase D.1 / ADR 0032 D5: lower `match scrutinee { arms }` to an
    /// LLVM `switch` on the scrutinee's tag. Each variant arm gets its
    /// own block (binds the payload fields into the pattern bindings'
    /// locals, lowers the arm body); arm results reconcile through a
    /// result alloca at a merge block (the `if`-merge machinery). The
    /// `_` wildcard is the switch default; with no wildcard the default
    /// is `unreachable` — exhaustiveness is a type-check guarantee
    /// (ADR 0032 D2), so the switch needs no runtime fallback.
    fn lower_match(
        &mut self,
        scrutinee: &TypedExpr,
        enum_id: EnumId,
        arms: &[TypedMatchArm],
        result_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let scrut = self.lower_expr(scrutinee, program)?.into_struct_value();
        let tag = self
            .builder
            .build_extract_value(scrut, 0, "match_tag")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let payload_ptr = self
            .builder
            .build_extract_value(scrut, 1, "match_payload")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();

        let current_fn = self.current_fn.expect("current_fn set");
        let i32_ty = self.context.i32_type();
        let llvm_result_ty = self.llvm_basic_type(result_ty);
        // D.5 / ADR 0036 D4: hoist to the entry block inside loops.
        let result = self.binding_alloca(llvm_result_ty, "matchresult")?;
        let merge_bb = self.context.append_basic_block(current_fn, "matchmerge");
        let default_bb = self.context.append_basic_block(current_fn, "matchdefault");

        // One block per variant arm; the wildcard (if any) is the
        // switch default.
        let mut cases: Vec<(IntValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> = Vec::new();
        let mut variant_arms: Vec<(inkwell::basic_block::BasicBlock<'ctx>, &TypedMatchArm)> =
            Vec::new();
        let mut wildcard_arm: Option<&TypedMatchArm> = None;
        for arm in arms {
            match &arm.pattern {
                TypedPattern::Variant { variant_index, .. } => {
                    let bb = self.context.append_basic_block(current_fn, "matcharm");
                    cases.push((i32_ty.const_int(*variant_index as u64, false), bb));
                    variant_arms.push((bb, arm));
                }
                TypedPattern::Wildcard(_) => wildcard_arm = Some(arm),
            }
        }

        self.builder
            .build_switch(tag, default_bb, &cases)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Lower each variant arm: bind payloads, lower body, store, branch.
        for (bb, arm) in &variant_arms {
            self.builder.position_at_end(*bb);
            if let TypedPattern::Variant { variant_index, bindings, .. } = &arm.pattern {
                self.bind_pattern_payloads(payload_ptr, enum_id, *variant_index, bindings, program)?;
            }
            let v = self.lower_expr(&arm.body, program)?;
            self.builder
                .build_store(result, v)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }

        // Default block: the wildcard body, or `unreachable`.
        self.builder.position_at_end(default_bb);
        if let Some(arm) = wildcard_arm {
            let v = self.lower_expr(&arm.body, program)?;
            self.builder
                .build_store(result, v)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        } else {
            self.builder
                .build_unreachable()
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }

        self.builder.position_at_end(merge_bb);
        let loaded = self
            .builder
            .build_load(llvm_result_ty, result, "matchresult_val")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(loaded)
    }

    /// Phase D.1 / ADR 0032 D5: bind a variant pattern's payload fields
    /// into the arm's locals. GEP/loads each field of the heap-boxed
    /// payload into a fresh alloca slot keyed by the binding's `VarId`,
    /// so the arm body's `Var(binding)` reads it. Unit patterns (no
    /// bindings) are a no-op. `_` slots still get a slot (positional —
    /// never read, LLVM elides them).
    fn bind_pattern_payloads(
        &mut self,
        payload_ptr: PointerValue<'ctx>,
        enum_id: EnumId,
        variant_index: usize,
        bindings: &[sentinel_types::TypedPatternBinding],
        program: &TypedProgram,
    ) -> Result<(), CodegenError> {
        if bindings.is_empty() {
            return Ok(());
        }
        let payload_struct_ty = self.enum_payload_struct_type(enum_id, variant_index, program);
        for (i, b) in bindings.iter().enumerate() {
            let field_ptr = self
                .builder
                .build_struct_gep(payload_struct_ty, payload_ptr, i as u32, "enum_field_ptr")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let field_ty = self.llvm_basic_type(b.ty);
            let val = self
                .builder
                .build_load(field_ty, field_ptr, "enum_field")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let slot = self
                .builder
                .build_alloca(field_ty, &b.name)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(slot, val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(b.var_id, (slot, b.ty));
        }
        Ok(())
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

    /// C5.4 (2/N) / ADR 0028: bump-allocate `size` bytes in the current
    /// scope's broker arena, returning the pointer. The arena is created
    /// lazily on first use ([`Self::current_scope_arena`]) and bulk-freed
    /// by the scope's `sentinel_arena_exit` in `emit_scope_drops`.
    fn arena_alloc_call(&mut self, size: IntValue<'ctx>) -> Result<PointerValue<'ctx>, CodegenError> {
        let handle = self.current_scope_arena()?;
        let call = self
            .builder
            .build_call(self.arena_alloc_fn, &[handle.into(), size.into()], "arena_alloc")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let ret = call
            .try_as_basic_value()
            .left()
            .ok_or_else(|| {
                CodegenError::Builder("sentinel_arena_alloc returned void".to_string())
            })?;
        Ok(ret.into_pointer_value())
    }

    /// C5.4 (2/N) / ADR 0028: get-or-create the current (top-of-stack)
    /// scope's broker bump arena handle. Created lazily on the first
    /// arena-routed allocation in the scope — so a scope that routes
    /// nothing emits no `sentinel_arena_enter`/`_exit` and stays
    /// byte-identical — and stored in the scope's [`ScopeFrame`] so
    /// `emit_scope_drops` can exit exactly the same handle. Capacity 0
    /// asks the runtime for its default arena size.
    ///
    /// The enter call is emitted at the first routed alloc, which is a
    /// direct statement on the scope's sequential block spine; the exit
    /// is emitted at the scope's end on that same spine. The enter block
    /// therefore dominates the exit (and every alloc), so the returned
    /// SSA handle is valid at all its uses.
    fn current_scope_arena(&mut self) -> Result<PointerValue<'ctx>, CodegenError> {
        if let Some(frame) = self.scope_stack.last() {
            if let Some(handle) = frame.arena {
                return Ok(handle);
            }
        }
        let cap = self.context.i64_type().const_zero();
        let call = self
            .builder
            .build_call(self.arena_enter_fn, &[cap.into()], "arena_enter")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let handle = call
            .try_as_basic_value()
            .left()
            .ok_or_else(|| {
                CodegenError::Builder("sentinel_arena_enter returned void".to_string())
            })?
            .into_pointer_value();
        if let Some(frame) = self.scope_stack.last_mut() {
            frame.arena = Some(handle);
        }
        Ok(handle)
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
        // C5.4 (2/N) / ADR 0028: consume the routing flag set by
        // `lower_stmt` for an arena-routed `let x = [array]`. Take it
        // before lowering elements so a nested array literal in an
        // element cannot inherit the routing.
        let route_to_arena = std::mem::take(&mut self.array_route_active);

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
        //
        // C5.4 (2/N) / ADR 0028: an arena-routed binding's data buffer
        // is bump-allocated in the current scope's arena (reclaimed by
        // the scope's `sentinel_arena_exit`, which replaces this
        // binding's skipped `sentinel_free`). Otherwise libc as before.
        let data_ptr = if route_to_arena {
            self.arena_alloc_call(total_size)?
        } else {
            self.alloc_call(total_size)?
        };

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

    /// D.2 / ADR 0033 D6: lower a string literal to an owned `[u8]`.
    /// The decoded bytes are **heap-copied** (`sentinel_alloc(N)` + N
    /// constant `i8` stores) into a fresh buffer, so the result is
    /// indistinguishable from any other array literal — drop / move /
    /// escape / arena "just work" via the existing `[T]` paths, with no
    /// global-free hazard. (D6's private global `[N x i8]` + `memcpy` is
    /// a measured optimisation deferred here because `CodegenCtx` holds
    /// no `&Module` to add a global from a lowering method; the
    /// owned-heap-copy semantics — the actual requirement — are
    /// identical, and string literals are short.)
    fn lower_string_lit(
        &self,
        bytes: &[u8],
        array_ty: Type,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let i8_type = self.context.i8_type();
        let n = bytes.len() as u64;
        let len_val = i64_type.const_int(n, false);

        // sizeof(u8) == 1, so the data buffer is exactly N bytes.
        let data_ptr = self.alloc_call(len_val)?;

        for (i, b) in bytes.iter().enumerate() {
            let idx = i64_type.const_int(i as u64, false);
            let byte_ptr = unsafe {
                self.builder
                    .build_gep(i8_type, data_ptr, &[idx], &format!("str_byte_{i}"))
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            self.builder
                .build_store(byte_ptr, i8_type.const_int(u64::from(*b), false))
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
        }

        // Build the { i64 len, ptr data } array struct value (abi-v1 `[T]`).
        let struct_ty = match self.llvm_basic_type(array_ty) {
            BasicTypeEnum::StructType(st) => st,
            _ => unreachable!("[u8] lowers to a {{ i64, ptr }} struct type"),
        };
        let agg = struct_ty.get_undef();
        let with_len = self
            .builder
            .build_insert_value(agg, len_val, 0, "str_with_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let with_data = self
            .builder
            .build_insert_value(with_len, data_ptr, 1, "str_with_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(with_data.into_struct_value().into())
    }

    /// D.2 / ADR 0033 D5: lower `str_eq(a, b)` to a call to the runtime
    /// `sentinel_str_eq(a_ptr, a_len, b_ptr, b_len) -> i1` (equal length
    /// + byte-wise equality). Both args are `[u8]` = `{ i64 len, ptr
    /// data }`; extract the two fields from each struct value.
    fn lower_str_eq(
        &mut self,
        a: &TypedExpr,
        b: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let a_val = self.lower_expr(a, program)?.into_struct_value();
        let b_val = self.lower_expr(b, program)?.into_struct_value();
        let a_len = self
            .builder
            .build_extract_value(a_val, 0, "a_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let a_ptr = self
            .builder
            .build_extract_value(a_val, 1, "a_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let b_len = self
            .builder
            .build_extract_value(b_val, 0, "b_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let b_ptr = self
            .builder
            .build_extract_value(b_val, 1, "b_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let call = self
            .builder
            .build_call(
                self.str_eq_fn,
                &[a_ptr.into(), a_len.into(), b_ptr.into(), b_len.into()],
                "str_eq",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(call
            .try_as_basic_value()
            .left()
            .expect("sentinel_str_eq returns i1"))
    }

    /// D.4 / ADR 0035 D4/D7: lower `read_file(path) -> [u8]`. Extracts
    /// the path's `{ len, ptr }`, calls `sentinel_read_file(path_ptr,
    /// path_len, &out_len)` (which `sentinel_alloc`s the buffer + writes
    /// the byte count to `out_len`, or aborts on failure), and assembles
    /// the owned `[u8]` `{ out_len, data }` struct (freed at scope-exit
    /// drop via the array path).
    fn lower_read_file(
        &mut self,
        path: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());

        let path_val = self.lower_expr(path, program)?.into_struct_value();
        let path_len = self
            .builder
            .build_extract_value(path_val, 0, "path_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let path_ptr = self
            .builder
            .build_extract_value(path_val, 1, "path_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();

        // Out-param slot for the byte count (ADR 0035 D6 ABI (a)).
        let out_len_slot = self
            .builder
            .build_alloca(i64_type, "read_file_len_slot")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let data_ptr = self
            .builder
            .build_call(
                self.read_file_fn,
                &[path_ptr.into(), path_len.into(), out_len_slot.into()],
                "read_file",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .try_as_basic_value()
            .left()
            .expect("sentinel_read_file returns ptr")
            .into_pointer_value();
        let len = self
            .builder
            .build_load(i64_type, out_len_slot, "read_file_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();

        // Assemble the `[u8]` array struct `{ i64 len, ptr data }`.
        let arr_struct_ty = self
            .context
            .struct_type(&[i64_type.into(), ptr_type.into()], false);
        let agg = arr_struct_ty.get_undef();
        let with_len = self
            .builder
            .build_insert_value(agg, len, 0, "rf_with_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let with_data = self
            .builder
            .build_insert_value(with_len, data_ptr, 1, "rf_with_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(with_data.into_struct_value().into())
    }

    /// D.4 / ADR 0035 D4/D7: lower `write_file(path, data) -> i64`.
    /// Extracts `{ len, ptr }` from both `[u8]` args and calls
    /// `sentinel_write_file` (which aborts on failure, else returns 0).
    /// Both args are borrowed (the ADR 0033 A3 runtime-builtin rule).
    fn lower_write_file(
        &mut self,
        path: &TypedExpr,
        data: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let path_val = self.lower_expr(path, program)?.into_struct_value();
        let path_len = self
            .builder
            .build_extract_value(path_val, 0, "path_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let path_ptr = self
            .builder
            .build_extract_value(path_val, 1, "path_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let data_val = self.lower_expr(data, program)?.into_struct_value();
        let data_len = self
            .builder
            .build_extract_value(data_val, 0, "wf_data_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let data_ptr = self
            .builder
            .build_extract_value(data_val, 1, "wf_data_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let call = self
            .builder
            .build_call(
                self.write_file_fn,
                &[
                    path_ptr.into(),
                    path_len.into(),
                    data_ptr.into(),
                    data_len.into(),
                ],
                "write_file",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(call
            .try_as_basic_value()
            .left()
            .expect("sentinel_write_file returns i64"))
    }

    /// D.4 (2/N) / ADR 0035 D4/D7: lower `print_bytes(data) -> i64`.
    /// Extracts the `[u8]`'s `{ len, ptr }` and calls
    /// `sentinel_print_bytes` (write to stdout, return 0). The arg is
    /// borrowed (the ADR 0033 A3 runtime-builtin rule).
    fn lower_print_bytes(
        &mut self,
        data: &TypedExpr,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let data_val = self.lower_expr(data, program)?.into_struct_value();
        let data_len = self
            .builder
            .build_extract_value(data_val, 0, "pb_data_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let data_ptr = self
            .builder
            .build_extract_value(data_val, 1, "pb_data_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let call = self
            .builder
            .build_call(
                self.print_bytes_fn,
                &[data_ptr.into(), data_len.into()],
                "print_bytes",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(call
            .try_as_basic_value()
            .left()
            .expect("sentinel_print_bytes returns i64"))
    }

    /// D.3 / ADR 0034 D7: lower `vec_new() -> Vec<T>` to an empty vector
    /// value `{ i64 len = 0, i64 cap = 0, ptr data = null }`. No
    /// allocation happens until the first `push` (`realloc(null, …)` ==
    /// `malloc`). `vec_ty` is the concrete `Type::Vec(_)` from the call
    /// expression (its element pinned from the binding annotation at
    /// type-check).
    fn lower_vec_new(&self, vec_ty: Type) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let struct_ty = match self.llvm_basic_type(vec_ty) {
            BasicTypeEnum::StructType(st) => st,
            _ => unreachable!("Vec lowers to a {{ i64, i64, ptr }} struct type"),
        };
        let zero = self.context.i64_type().const_zero();
        let null_ptr = self
            .context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        Ok(struct_ty
            .const_named_struct(&[zero.into(), zero.into(), null_ptr.into()])
            .into())
    }

    /// D.3 / ADR 0034 D7: lower `push(&mut v, x)`. Loads the `Vec`'s
    /// `{ len, cap, data }` through the `&mut Vec` pointer; grows the
    /// buffer (`sentinel_realloc` to `max(1, cap*2) * sizeof(T)`) when
    /// `len == cap`; writes `data[len] = x`; bumps `len`. Returns i64 0
    /// (a statement-shaped builtin). `elem_ty` is the concrete element
    /// type (the call's `type_args[0]`), used for `sizeof(T)`.
    ///
    /// The grow path stores the new `cap` + `data` back into the Vec's
    /// fields, so the continuation re-loads `data` from field 2 and gets
    /// the live buffer in both the grow and no-grow paths — no PHI node.
    fn lower_push(
        &mut self,
        vec_ref: &TypedExpr,
        elem: &TypedExpr,
        elem_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());

        // `&mut v` lowers to the pointer to v's `{ len, cap, data }`
        // storage; recover the `Vec<T>` pointee type for the field GEPs.
        let vec_ptr = self.lower_expr(vec_ref, program)?.into_pointer_value();
        let vec_ty = match vec_ref.ty {
            Type::Ref(id) => program.refs[id.0 as usize].inner,
            _ => unreachable!("push's first arg is typed &mut Vec<T>"),
        };
        let vec_struct_ty = match self.llvm_basic_type(vec_ty) {
            BasicTypeEnum::StructType(st) => st,
            _ => unreachable!("Vec lowers to a struct type"),
        };
        let len_ptr = self
            .builder
            .build_struct_gep(vec_struct_ty, vec_ptr, 0, "vec_len_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let cap_ptr = self
            .builder
            .build_struct_gep(vec_struct_ty, vec_ptr, 1, "vec_cap_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let data_field_ptr = self
            .builder
            .build_struct_gep(vec_struct_ty, vec_ptr, 2, "vec_data_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Evaluate the element value before the grow branch (left-to-
        // right arg order: `&mut v` then `x`, then the append logic).
        let elem_val = self.lower_expr(elem, program)?;
        let elem_llvm_ty = self.llvm_basic_type(elem_ty);
        let elem_size = elem_llvm_ty
            .size_of()
            .expect("non-void basic types have a known size");

        let len = self
            .builder
            .build_load(i64_type, len_ptr, "vec_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let cap = self
            .builder
            .build_load(i64_type, cap_ptr, "vec_cap")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();

        // Grow iff len == cap.
        let need_grow = self
            .builder
            .build_int_compare(IntPredicate::EQ, len, cap, "need_grow")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let current_fn = self.current_fn.expect("current_fn set");
        let grow_bb = self.context.append_basic_block(current_fn, "push_grow");
        let cont_bb = self.context.append_basic_block(current_fn, "push_cont");
        self.builder
            .build_conditional_branch(need_grow, grow_bb, cont_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // grow_bb: new_cap = max(1, cap*2); realloc; store cap + data.
        self.builder.position_at_end(grow_bb);
        let old_data = self
            .builder
            .build_load(ptr_type, data_field_ptr, "old_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let cap_x2 = self
            .builder
            .build_int_mul(cap, i64_type.const_int(2, false), "cap_x2")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let cap_is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, cap, i64_type.const_zero(), "cap_is_zero")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let new_cap = self
            .builder
            .build_select(cap_is_zero, i64_type.const_int(1, false), cap_x2, "new_cap")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let new_size = self
            .builder
            .build_int_mul(new_cap, elem_size, "new_size")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let new_data = self
            .builder
            .build_call(self.realloc_fn, &[old_data.into(), new_size.into()], "vec_grow")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .try_as_basic_value()
            .left()
            .expect("sentinel_realloc returns ptr")
            .into_pointer_value();
        self.builder
            .build_store(cap_ptr, new_cap)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(data_field_ptr, new_data)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // cont_bb: data[len] = x; len += 1. Re-load data so we pick up
        // the grown buffer (grow path) or the existing one (no-grow).
        self.builder.position_at_end(cont_bb);
        let data = self
            .builder
            .build_load(ptr_type, data_field_ptr, "vec_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let elem_slot = unsafe {
            self.builder
                .build_gep(elem_llvm_ty, data, &[len], "push_slot")
                .map_err(|e| CodegenError::Builder(e.to_string()))?
        };
        self.builder
            .build_store(elem_slot, elem_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let new_len = self
            .builder
            .build_int_add(len, i64_type.const_int(1, false), "new_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(len_ptr, new_len)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // `push` is statement-shaped (like `print`): yields i64 0.
        Ok(i64_type.const_zero().into())
    }

    /// D.3 (2/N) / ADR 0034 D5: lower `pop(&mut v) -> T`. Loads `len`
    /// from the `&mut Vec`; if the `Vec` is empty (`len == 0`) panics via
    /// `sentinel_panic_oob` (like an out-of-bounds index); otherwise
    /// decrements `len`, loads `data[len-1]`, stores the new `len`, and
    /// returns the element. The buffer is not shrunk (capacity is
    /// retained). `elem_ty` is the concrete element (the call's
    /// `type_args[0]`).
    fn lower_pop(
        &mut self,
        vec_ref: &TypedExpr,
        elem_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());

        let vec_ptr = self.lower_expr(vec_ref, program)?.into_pointer_value();
        let vec_ty = match vec_ref.ty {
            Type::Ref(id) => program.refs[id.0 as usize].inner,
            _ => unreachable!("pop's arg is typed &mut Vec<T>"),
        };
        let vec_struct_ty = match self.llvm_basic_type(vec_ty) {
            BasicTypeEnum::StructType(st) => st,
            _ => unreachable!("Vec lowers to a struct type"),
        };
        let len_ptr = self
            .builder
            .build_struct_gep(vec_struct_ty, vec_ptr, 0, "vec_len_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let data_field_ptr = self
            .builder
            .build_struct_gep(vec_struct_ty, vec_ptr, 2, "vec_data_ptr")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let len = self
            .builder
            .build_load(i64_type, len_ptr, "vec_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        // The popped index is len - 1; if that is < 0 the Vec is empty.
        let new_len = self
            .builder
            .build_int_sub(len, i64_type.const_int(1, false), "pop_idx")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let nonempty = self
            .builder
            .build_int_compare(IntPredicate::SGE, new_len, i64_type.const_zero(), "vec_nonempty")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let current_fn = self.current_fn.expect("current_fn set");
        let ok_bb = self.context.append_basic_block(current_fn, "pop_ok");
        let empty_bb = self.context.append_basic_block(current_fn, "pop_empty");
        self.builder
            .build_conditional_branch(nonempty, ok_bb, empty_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Empty: reuse the OOB trap (idx = -1, len = 0), then unreachable.
        self.builder.position_at_end(empty_bb);
        self.builder
            .build_call(self.panic_oob_fn, &[new_len.into(), len.into()], "")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unreachable()
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // OK: load data[new_len], store the decremented len, return it.
        self.builder.position_at_end(ok_bb);
        let data = self
            .builder
            .build_load(ptr_type, data_field_ptr, "vec_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let elem_llvm_ty = self.llvm_basic_type(elem_ty);
        let elem_ptr = unsafe {
            self.builder
                .build_gep(elem_llvm_ty, data, &[new_len], "pop_slot")
                .map_err(|e| CodegenError::Builder(e.to_string()))?
        };
        let elem_val = self
            .builder
            .build_load(elem_llvm_ty, elem_ptr, "pop_load")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(len_ptr, new_len)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(elem_val)
    }

    /// D.3 (2/N) / ADR 0034 D5: lower `vec_to_array(v) -> [T]` — the
    /// `Vec<T>` -> `[T]` bridge. Copies the live `len` elements of `v`
    /// into a fresh heap buffer and builds an owned `[T]` `{ i64 len,
    /// ptr data }`. Non-consuming: `v` keeps its own buffer (the array
    /// is an independent copy), so both drop their buffers at scope exit
    /// (leak-free). `elem_ty` is the concrete element (`type_args[0]`).
    fn lower_vec_to_array(
        &mut self,
        vec_expr: &TypedExpr,
        elem_ty: Type,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());

        let vec_val = self.lower_expr(vec_expr, program)?.into_struct_value();
        let len = self
            .builder
            .build_extract_value(vec_val, 0, "vec_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let src = self
            .builder
            .build_extract_value(vec_val, 2, "vec_data")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();

        // Allocate len * sizeof(T) and memcpy the live prefix. (Align 1
        // is always valid; an empty Vec copies 0 bytes — the llvm.memcpy
        // intrinsic with length 0 is a no-op even for a null source.)
        let elem_llvm_ty = self.llvm_basic_type(elem_ty);
        let elem_size = elem_llvm_ty
            .size_of()
            .expect("non-void basic types have a known size");
        let total_size = self
            .builder
            .build_int_mul(len, elem_size, "arr_size")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let dest = self.alloc_call(total_size)?;
        self.builder
            .build_memcpy(dest, 1, src, 1, total_size)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Build the `[T]` array struct `{ i64 len, ptr data }` (abi-v1).
        let arr_struct_ty = self
            .context
            .struct_type(&[i64_type.into(), ptr_type.into()], false);
        let agg = arr_struct_ty.get_undef();
        let with_len = self
            .builder
            .build_insert_value(agg, len, 0, "arr_with_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let with_data = self
            .builder
            .build_insert_value(with_len, dest, 1, "arr_with_data")
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
        // The bounds-checked element GEP, then a load. The GEP is
        // factored so index-assignment (`a[i] = v;`, ADR 0050) reuses
        // the identical address computation and stores instead.
        let elem_ptr = self.lower_index_elem_ptr(target, index, elem_ty, program)?;
        let elem_llvm_ty = self.llvm_basic_type(elem_ty.to_type());
        let elem_val = self
            .builder
            .build_load(elem_llvm_ty, elem_ptr, "idx_load")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(elem_val)
    }

    /// Compute a pointer to element `index` of collection `target` —
    /// the bounds-checked element GEP shared by the read path (`a[i]`)
    /// and index-assignment (`a[i] = v;`, ADR 0050). Emits the
    /// `0 <= i < len` check (calling `sentinel_panic_oob` on failure),
    /// leaves the builder positioned in the in-bounds block, and
    /// returns the element pointer for the caller to load / store.
    fn lower_index_elem_ptr(
        &mut self,
        target: &TypedExpr,
        index: &TypedExpr,
        elem_ty: ArrayElem,
        program: &TypedProgram,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let array_val = self.lower_expr(target, program)?.into_struct_value();
        let idx = self.lower_expr(index, program)?.into_int_value();
        // `len` is field 0 for both `[T]` (`{len,data}`) and `Vec<T>`
        // (`{len,cap,data}`). The data pointer is field 1 for an array
        // but field 2 for a `Vec` (the capacity sits between) — D.3
        // (2/N) / ADR 0034 D3. Key on the target's (secret-stripped)
        // type.
        let data_field: u32 = match self.strip_secret(target.ty) {
            Type::Vec(_) => 2,
            _ => 1,
        };
        let len = self
            .builder
            .build_extract_value(array_val, 0, "coll_len")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let data_ptr = self
            .builder
            .build_extract_value(array_val, data_field, "coll_data")
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

        // OK branch: GEP to the element address.
        self.builder.position_at_end(ok_bb);
        let elem_llvm_ty = self.llvm_basic_type(elem_ty.to_type());
        let elem_ptr = unsafe {
            self.builder
                .build_gep(elem_llvm_ty, data_ptr, &[idx], "idx_gep")
                .map_err(|e| CodegenError::Builder(e.to_string()))?
        };
        Ok(elem_ptr)
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
            // D.2 / ADR 0033 D6: a char literal is an `i8` constant of
            // the byte value (no allocation).
            TypedExprKind::CharLit(b) => {
                Ok(self.context.i8_type().const_int(u64::from(*b), false).into())
            }
            // D.2 / ADR 0033 D6: a string literal emits a private global
            // `[N x i8]` constant + heap-copies it so the `[u8]` is owned
            // and drops/moves uniformly with any array.
            TypedExprKind::StringLit(bytes) => self.lower_string_lit(bytes, expr.ty),
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
                // D.2 / ADR 0033 D6: `u8` is unsigned — `/` is `udiv`
                // (add/sub/mul/bitwise are sign-agnostic in two's
                // complement). Both operands share a type (type-checked).
                let is_unsigned = matches!(self.strip_secret(lhs.ty), Type::U8);
                let result = match op {
                    BinOp::Add => self.builder.build_int_add(l, r, "add"),
                    BinOp::Sub => self.builder.build_int_sub(l, r, "sub"),
                    BinOp::Mul => self.builder.build_int_mul(l, r, "mul"),
                    BinOp::Div if is_unsigned => {
                        self.builder.build_int_unsigned_div(l, r, "udiv")
                    }
                    BinOp::Div => self.builder.build_int_signed_div(l, r, "div"),
                    // C5.3 / ADR 0027 D6: bitwise ops lower to the
                    // data-independent LLVM bit instructions (`secret`
                    // still lowers as its inner int — ADR 0019 D12).
                    BinOp::BitAnd => self.builder.build_and(l, r, "and"),
                    BinOp::BitOr => self.builder.build_or(l, r, "or"),
                    BinOp::BitXor => self.builder.build_xor(l, r, "xor"),
                    // ADR 0048: shift left / LOGICAL shift right (zero-fill for
                    // ALL integer types — rotate composition needs `lshr`, not
                    // `ashr`). LLVM requires the amount to share the value's int
                    // type, so coerce it to `l`'s width first (zext/trunc; a
                    // shift count is non-negative, so widening is zero-extend).
                    BinOp::Shl | BinOp::Shr => {
                        let lty = l.get_type();
                        let shamt = match r.get_type().get_bit_width().cmp(&lty.get_bit_width()) {
                            std::cmp::Ordering::Equal => r,
                            std::cmp::Ordering::Greater => self
                                .builder
                                .build_int_truncate(r, lty, "shamt")
                                .map_err(|e| CodegenError::Builder(e.to_string()))?,
                            std::cmp::Ordering::Less => self
                                .builder
                                .build_int_z_extend(r, lty, "shamt")
                                .map_err(|e| CodegenError::Builder(e.to_string()))?,
                        };
                        if matches!(op, BinOp::Shl) {
                            self.builder.build_left_shift(l, shamt, "shl")
                        } else {
                            // `false` = logical (lshr, zero-fill), NOT ashr.
                            self.builder.build_right_shift(l, shamt, false, "lshr")
                        }
                    }
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
                // D.2 / ADR 0033 D6: `u8` is unsigned, so the ordered
                // comparisons use unsigned predicates (a byte ≥ 0x80
                // must compare as large, not negative). Eq/Ne are
                // sign-agnostic; nullable operands only ever use Eq/Ne.
                let is_unsigned = matches!(self.strip_secret(lhs.ty), Type::U8);
                let predicate = match op {
                    CmpOp::Eq => IntPredicate::EQ,
                    CmpOp::Ne => IntPredicate::NE,
                    CmpOp::Lt if is_unsigned => IntPredicate::ULT,
                    CmpOp::Lt => IntPredicate::SLT,
                    CmpOp::Le if is_unsigned => IntPredicate::ULE,
                    CmpOp::Le => IntPredicate::SLE,
                    CmpOp::Gt if is_unsigned => IntPredicate::UGT,
                    CmpOp::Gt => IntPredicate::SGT,
                    CmpOp::Ge if is_unsigned => IntPredicate::UGE,
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
                // D.2 / ADR 0033 D5/D6: the byte-string builtins. The
                // width conversions are a `zext` (u8 → i64) / `trunc`
                // (i64 → u8); `str_eq` calls the runtime byte-compare.
                if *id == U8_TO_I64_FN_ID {
                    let v = self.lower_expr(&args[0], program)?.into_int_value();
                    return self
                        .builder
                        .build_int_z_extend(v, self.context.i64_type(), "u8_to_i64")
                        .map(|x| x.into())
                        .map_err(|e| CodegenError::Builder(e.to_string()));
                }
                if *id == I64_TO_U8_FN_ID {
                    let v = self.lower_expr(&args[0], program)?.into_int_value();
                    return self
                        .builder
                        .build_int_truncate(v, self.context.i8_type(), "i64_to_u8")
                        .map(|x| x.into())
                        .map_err(|e| CodegenError::Builder(e.to_string()));
                }
                if *id == STR_EQ_FN_ID {
                    return self.lower_str_eq(&args[0], &args[1], program);
                }
                // D.3 / ADR 0034 D7: the growable-collection builtins.
                // `vec_new` builds an empty `{ 0, 0, null }`; `push`
                // loads the `&mut Vec` storage, grows it if `len == cap`,
                // writes the element, and bumps `len`. `type_args[0]` is
                // the concrete element type (for `sizeof(T)`).
                if *id == VEC_NEW_FN_ID {
                    return self.lower_vec_new(expr.ty);
                }
                if *id == PUSH_FN_ID {
                    return self.lower_push(&args[0], &args[1], type_args[0], program);
                }
                // D.3 (2/N): pop removes+returns the last element; the
                // bridge copies a Vec's live elements into a fresh [T].
                if *id == POP_FN_ID {
                    return self.lower_pop(&args[0], type_args[0], program);
                }
                if *id == VEC_TO_ARRAY_FN_ID {
                    return self.lower_vec_to_array(&args[0], type_args[0], program);
                }
                // D.4 / ADR 0035: file I/O — read a whole file into a
                // fresh [u8]; write a [u8] to a file. Runtime builtins
                // (like str_eq), backed by sentinel_read_file /
                // sentinel_write_file (which abort on I/O failure).
                if *id == READ_FILE_FN_ID {
                    return self.lower_read_file(&args[0], program);
                }
                if *id == WRITE_FILE_FN_ID {
                    return self.lower_write_file(&args[0], &args[1], program);
                }
                if *id == PRINT_BYTES_FN_ID {
                    return self.lower_print_bytes(&args[0], program);
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
            TypedExprKind::WidenToSecret(inner) | TypedExprKind::Declassify(inner) => {
                // C3 / ADR 0019 D5 + D6 (C3.1): secret wrap +
                // declassify lower as identity at C3.1. The secret
                // qualifier is purely type-level; constant-time
                // codegen (branch-free `select`/`cmov`, speculation
                // barriers) is a follow-on per ADR 0019 D12.
                self.lower_expr(inner, program)
            }
            TypedExprKind::Cast(inner) => {
                // ADR 0049: integer width conversion. Strip secret (layout-
                // free), then trunc / sext / zext by width; same width is a
                // no-op. `u8` is the only UNSIGNED source → zext when widening;
                // `i32`/`i64` sign-extend.
                let v = self.lower_expr(inner, program)?.into_int_value();
                let dst_ty = self.llvm_int_type(expr.ty);
                let src_w = v.get_type().get_bit_width();
                let dst_w = dst_ty.get_bit_width();
                let result = if dst_w == src_w {
                    return Ok(v.into());
                } else if dst_w < src_w {
                    self.builder.build_int_truncate(v, dst_ty, "cast")
                } else if matches!(self.strip_secret(inner.ty), Type::U8) {
                    self.builder.build_int_z_extend(v, dst_ty, "cast")
                } else {
                    self.builder.build_int_s_extend(v, dst_ty, "cast")
                };
                result
                    .map(|x| x.into())
                    .map_err(|e| CodegenError::Builder(e.to_string()))
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
            // C3.5(a) / ADR 0020 D7: lower perform/resume/handle
            // for the restricted case (handle body must be a
            // direct Perform). The general case (fn-call-that-
            // performs, frame reification at arbitrary eval
            // sites) lands at C3.5(b) / C3.6.
            TypedExprKind::Perform { effect_id, op_index, args, .. } => {
                self.lower_perform(*effect_id, *op_index, args, program)
            }
            TypedExprKind::ResumeKont { kont, args, .. } => {
                self.lower_resume_kont(*kont, args, program)
            }
            TypedExprKind::Handle { body, arms, return_arm, .. } => {
                self.lower_handle(body, arms, return_arm.as_deref(), program)
            }
            // C4.1 / ADR 0022 D7: lower a postfix method call.
            // Get the receiver pointer via lower_lvalue_ptr (the
            // typed AST guarantees the target is an lvalue when
            // class-typed; non-lvalue receivers are deferred).
            // Emit a direct call to ClassName__method.
            TypedExprKind::MethodCall { target, class_id, method_index, args, .. } => {
                let self_ptr = self.lower_lvalue_ptr(target, program)?;
                let method_fn = *self
                    .class_method_fns
                    .get(&(*class_id, *method_index))
                    .expect("method fn declared in pass 1");
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![self_ptr.into()];
                for a in args {
                    let v = self.lower_expr(a, program)?;
                    call_args.push(v.into());
                }
                let call_site = self
                    .builder
                    .build_call(method_fn, &call_args, "methodcall")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(call_site
                    .try_as_basic_value()
                    .left()
                    .expect("method returns a value"))
            }
            // C4.1 / ADR 0022 D5 + D9: lower `Name::init(args)`.
            // Allocate an instance of the class struct on the
            // stack, pass its pointer as out_ptr to the init fn,
            // then load the now-constructed value.
            TypedExprKind::ClassInit { id, args, .. } => {
                let class_struct_ty = self.class_types[id];
                let alloca = self
                    .builder
                    .build_alloca(class_struct_ty, "classinit")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let init_fn = *self
                    .class_init_fns
                    .get(id)
                    .expect("init fn declared in pass 1");
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![alloca.into()];
                for a in args {
                    let v = self.lower_expr(a, program)?;
                    call_args.push(v.into());
                }
                self.builder
                    .build_call(init_fn, &call_args, "init")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let loaded = self
                    .builder
                    .build_load(class_struct_ty, alloca, "initval")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(loaded)
            }
            // C4.2 / ADR 0023 D5 Path 1 + D9: receiver-typed
            // dispatch to a default impl method. Get the receiver
            // pointer via lower_lvalue_ptr (same as class
            // MethodCall) + direct call into the impl method's
            // mangled fn.
            TypedExprKind::ImplMethodCall { target, impl_id, method_index, args, .. } => {
                let self_ptr = self.lower_lvalue_ptr(target, program)?;
                let method_fn = *self
                    .impl_method_fns
                    .get(&(*impl_id, *method_index))
                    .expect("impl method fn declared in pass 1");
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![self_ptr.into()];
                for a in args {
                    let v = self.lower_expr(a, program)?;
                    call_args.push(v.into());
                }
                let call_site = self
                    .builder
                    .build_call(method_fn, &call_args, "implmethodcall")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(call_site
                    .try_as_basic_value()
                    .left()
                    .expect("impl method returns a value"))
            }
            // C4.2 / ADR 0023 D5 Path 2 + D9: qualified-named
            // dispatch. args[0] is the receiver expression (a ref
            // — lowered to a pointer); args[1..] are the declared
            // params. Pass args[0]'s lowered ptr as self_ptr.
            TypedExprKind::QualifiedCall { impl_id, method_index, args, .. } => {
                let method_fn = *self
                    .impl_method_fns
                    .get(&(*impl_id, *method_index))
                    .expect("impl method fn declared in pass 1");
                let self_val = self.lower_expr(&args[0], program)?;
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![self_val.into()];
                for a in &args[1..] {
                    let v = self.lower_expr(a, program)?;
                    call_args.push(v.into());
                }
                let call_site = self
                    .builder
                    .build_call(method_fn, &call_args, "qualcall")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(call_site
                    .try_as_basic_value()
                    .left()
                    .expect("qualified call returns a value"))
            }

            // C4.4 / ADR 0024 D8: `scope concurrent { ... }`.
            // scope_enter → lower body (current_scope set so inner
            // spawns register) → scope_exit (auto-await + free owned
            // tasks). The block's tail value is the scope's value.
            TypedExprKind::Scope { body, .. } => {
                let enter = self
                    .builder
                    .build_call(self.scope_enter_fn, &[], "scope_enter")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let scope_ptr = enter
                    .try_as_basic_value()
                    .left()
                    .expect("scope_enter returns *ScopeCtx")
                    .into_pointer_value();
                let prev_scope = self.current_scope;
                self.current_scope = Some(scope_ptr);
                let body_val = self.lower_block(body, program)?;
                self.current_scope = prev_scope;
                self.builder
                    .build_call(self.scope_exit_fn, &[scope_ptr.into()], "scope_exit")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(body_val)
            }

            // C4.4 / ADR 0024 D8: `spawn fn(args)`. Pack args into a
            // heap buffer (one i64 slot each), call sentinel_task_spawn
            // with the per-target wrapper, and (if inside a scope)
            // register the Task so the scope owns + auto-awaits it.
            TypedExprKind::Spawn { call, .. } => {
                let (callee_id, call_args_exprs) = match &call.kind {
                    TypedExprKind::Call { id, args, .. } => (*id, args),
                    _ => unreachable!("type-check guarantees spawn target is a Call"),
                };
                let i64_ty = self.context.i64_type();
                let i8_ty = self.context.i8_type();
                let n = call_args_exprs.len();
                // n*8 bytes (i64 slots); at least 8 so alloc(0) is
                // never requested for a zero-arg target.
                let size_v = i64_ty.const_int((n.max(1) * 8) as u64, false);
                let args_storage = self.alloc_call(size_v)?;
                for (i, arg) in call_args_exprs.iter().enumerate() {
                    let v = self.lower_expr(arg, program)?;
                    let off = i64_ty.const_int((i * 8) as u64, false);
                    let slot = unsafe {
                        self.builder
                            .build_in_bounds_gep(i8_ty, args_storage, &[off], "spawn_arg")
                            .map_err(|e| CodegenError::Builder(e.to_string()))?
                    };
                    self.builder
                        .build_store(slot, v)
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                }
                let wrapper = *self
                    .spawn_wrappers
                    .get(&callee_id)
                    .expect("spawn wrapper synthesized in pre-walk");
                let wrapper_ptr = wrapper.as_global_value().as_pointer_value();
                let spawn_call = self
                    .builder
                    .build_call(
                        self.task_spawn_fn,
                        &[wrapper_ptr.into(), args_storage.into(), size_v.into()],
                        "task_spawn",
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                let task_ptr = spawn_call
                    .try_as_basic_value()
                    .left()
                    .expect("task_spawn returns *Task");
                if let Some(scope) = self.current_scope {
                    self.builder
                        .build_call(
                            self.scope_register_fn,
                            &[scope.into(), task_ptr.into()],
                            "scope_register",
                        )
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                }
                Ok(task_ptr)
            }

            // C4.4 / ADR 0024 D8: `task.await` — join + read result.
            TypedExprKind::Await { task_expr, .. } => {
                let task_val = self.lower_expr(task_expr, program)?;
                let task_ptr = task_val.into_pointer_value();
                let await_call = self
                    .builder
                    .build_call(self.task_await_fn, &[task_ptr.into()], "task_await")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(await_call
                    .try_as_basic_value()
                    .left()
                    .expect("task_await returns i64"))
            }

            // Phase D.1 / ADR 0032 (4/N): enum construction lowers to
            // the `{ i32 tag, ptr payload }` layout (D4); `match` lowers
            // to an LLVM `switch` on the tag (D5).
            TypedExprKind::EnumConstruct { enum_id, variant_index, args, .. } => {
                self.lower_enum_construct(*enum_id, *variant_index, args, expr.ty, program)
            }
            TypedExprKind::Match { scrutinee, enum_id, arms } => {
                self.lower_match(scrutinee, *enum_id, arms, expr.ty, program)
            }
        }
    }

    /// ADR 0037 (2/N): encode an op id consulting the build-wide
    /// op-id base map (effect NAME → a graph-stable index) so a
    /// `perform` in one separately-compiled unit and a `handle` in
    /// another agree on `(base << 16) | op` even when the effect's
    /// LOCAL `EffectId` differs between units (it is BOTH the op-id
    /// basis AND the `effect_decls[]` index, so it can't be shared
    /// directly). Empty map (single-file / merged / corpus) → falls
    /// back to the local `effect_id.0`, byte-identical to the
    /// standalone [`encode_op_id`] the oracle still mirrors. NOTE
    /// (MVP): keyed by NAME, so same-named cross-module effects would
    /// collide — an origin-qualified key is the robust upgrade.
    fn encode_op_id_ctx(
        &self,
        effect_id: EffectId,
        op_index: usize,
        program: &TypedProgram,
    ) -> u32 {
        match program
            .effect_decls
            .get(effect_id.0 as usize)
            .and_then(|d| self.op_id_base.get(&d.name).copied())
        {
            // A graph-stable base for this effect (separate compilation).
            Some(base) => (base << 16) | ((op_index as u32) & 0xFFFF),
            // No base (single-file / merged / corpus) → the local id,
            // delegating to the standalone fn the oracle mirrors so the
            // byte-for-byte equivalence is self-evident.
            None => encode_op_id(effect_id, op_index),
        }
    }

    /// C3.5(a) / ADR 0020 D7: lower `perform Effect.Op(args)` to
    /// a `sentinel_perform_op(op_id, arg)` call. Returns the
    /// continuation pointer; the enclosing handle catches it.
    /// At C3.5(a) only 0- or 1-arg ops are supported by codegen
    /// (the runtime Kont struct's `arg: i64` field carries the
    /// single value); multi-arg ops are flagged at type-check
    /// already via `OperationArityMismatch` if they don't fit.
    fn lower_perform(
        &mut self,
        effect_id: EffectId,
        op_index: usize,
        args: &[TypedExpr],
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let op_id = self.encode_op_id_ctx(effect_id, op_index, program);
        let i32_ty = self.context.i32_type();
        let i64_ty = self.context.i64_type();
        let op_id_v = i32_ty.const_int(op_id as u64, false);
        let arg_v: inkwell::values::IntValue<'ctx> = if args.is_empty() {
            i64_ty.const_int(0, false)
        } else {
            // Single-arg case: lower the arg, expect i64.
            self.lower_expr(&args[0], program)?.into_int_value()
        };
        let call = self
            .builder
            .build_call(
                self.perform_op_fn,
                &[op_id_v.into(), arg_v.into()],
                "kont",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        call.try_as_basic_value()
            .left()
            .ok_or_else(|| {
                CodegenError::Builder(
                    "sentinel_perform_op returned void unexpectedly".to_string(),
                )
            })
    }

    /// C3.5(e) / ADR 0020 D7: lower `k(arg)` (a resume call) to
    /// `sentinel_kont_resume(kont, arg)`. The runtime returns a
    /// kont*; the caller branches on `op_id`:
    ///   - `PURE_RETURN_OP_ID`: the chain drained pure — unwrap
    ///     via `sentinel_kont_consume_pure` and use the i64 as
    ///     the value of the `k(v)` expression.
    ///   - any other op id: a resumer in the chain performed;
    ///     bubble back to the enclosing handle's dispatch loop
    ///     via the [`HandleContext`] pushed by
    ///     [`Self::lower_handle`].
    ///
    /// After this fn returns, the builder is positioned at the
    /// pure-path block and the BasicValueEnum is the unwrapped
    /// i64. The bubble path is terminated by an unconditional
    /// branch back to the loop block — its block doesn't fall
    /// through to anything visible to the caller.
    fn lower_resume_kont(
        &mut self,
        kont: VarId,
        args: &[TypedExpr],
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let (kont_ptr, _kont_ty) = *self
            .vars
            .get(&kont)
            .expect("ResumeKont's VarId is bound by the surrounding Handle codegen");
        // Load the kont pointer (it was stored as a ptr-typed value).
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i32_ty = self.context.i32_type();
        let kont_val = self
            .builder
            .build_load(ptr_ty, kont_ptr, "kont_load")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        // Lower the resume arg (must be i64 per the op's return type
        // at C3.5(a)).
        let arg_v = self.lower_expr(&args[0], program)?.into_int_value();
        let call = self
            .builder
            .build_call(
                self.kont_resume_fn,
                &[kont_val.into(), arg_v.into()],
                "kv_result",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let result_kont = call
            .try_as_basic_value()
            .left()
            .ok_or_else(|| {
                CodegenError::Builder(
                    "sentinel_kont_resume returned void unexpectedly".to_string(),
                )
            })?
            .into_pointer_value();

        // Read result's op_id (i32 at offset 0) — same byte-offset
        // GEP scheme as lower_handle.
        let op_id_val = self
            .builder
            .build_load(i32_ty, result_kont, "kv_op_id")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();
        let pure_id = i32_ty.const_int(u32::MAX as u64, false);
        let is_pure = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                op_id_val,
                pure_id,
                "kv_is_pure",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        let current_fn = self.current_fn.expect("inside compile_fn");
        let pure_block = self.context.append_basic_block(current_fn, "kv_pure");
        let bubble_block = self.context.append_basic_block(current_fn, "kv_bubble");
        self.builder
            .build_conditional_branch(is_pure, pure_block, bubble_block)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Snapshot the enclosing handle's dispatch context before
        // emitting either branch — both reach for it.
        let handle_ctx = self
            .handle_stack
            .last()
            .expect("ResumeKont must be lowered inside a handle arm")
            .clone();

        // Bubble path: the enclosing handle's dispatch loop owns
        // re-dispatching the new kont. Store + branch.
        self.builder.position_at_end(bubble_block);
        self.builder
            .build_store(handle_ctx.current_kont_slot, result_kont)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(handle_ctx.loop_block)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Pure path: unwrap to i64. After this the builder stays
        // positioned in pure_block; the i64 flows back as the
        // value of k(v). If the enclosing handle has a non-
        // identity return arm (ADR 0020 D4 + C3.6(a)), apply it
        // per Phase B's deep-handler re-wrap: k := `\v. handle
        // (kont.resume v) with H` means k(v)'s pure result IS
        // the return arm applied to the resumed value.
        self.builder.position_at_end(pure_block);
        let consume_call = self
            .builder
            .build_call(
                self.kont_consume_pure_fn,
                &[result_kont.into()],
                "kv_pure_val",
            )
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let pure_unwrap = consume_call.try_as_basic_value().left().ok_or_else(|| {
            CodegenError::Builder(
                "sentinel_kont_consume_pure returned void unexpectedly".to_string(),
            )
        })?;
        if let Some(ra) = handle_ctx.return_arm.as_ref() {
            let i64_ty = self.context.i64_type();
            let alloca = self
                .builder
                .build_alloca(i64_ty, &ra.value_name.kind)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(alloca, pure_unwrap.into_int_value())
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(ra.value_var_id, (alloca, Type::I64));
            let ra_val = self.lower_expr(&ra.body, program)?;
            self.vars.remove(&ra.value_var_id);
            Ok(ra_val)
        } else {
            Ok(pure_unwrap)
        }
    }

    /// C3.5(e) + C3.6(b) / ADR 0020 D7: lower `handle body with
    /// { arms }` as a dispatch *loop*. The body produces an
    /// initial kont; the loop reads `current_kont_slot`,
    /// switches on its `op_id`, and runs the matching arm. A
    /// `k(v)` call inside the arm that bubbles (resumer in the
    /// chain performed) stores the bubble kont back into
    /// `current_kont_slot` and branches to the top of the loop —
    /// re-dispatching per ADR 0020 D3 deep-handler semantics.
    ///
    /// **C3.6(b) nested-handle support**: when this handle is
    /// itself the body (or transitively reached from the body)
    /// of an enclosing handle — detected via `handle_depth > 1`
    /// — the lowering shifts to Kont*-typed output: arms wrap
    /// their i64 result via `sentinel_kont_pure`, the pure-
    /// return switch case passes the kont through (or wraps the
    /// return-arm'd value), and the switch's default is a
    /// propagate block that contributes the un-caught kont to
    /// the merge so the outer handle can dispatch on it.
    fn lower_handle(
        &mut self,
        body: &TypedExpr,
        arms: &[TypedHandlerArm],
        return_arm: Option<&TypedReturnArm>,
        program: &TypedProgram,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        self.handle_depth += 1;
        let is_nested = self.handle_depth > 1;

        // C3.7 / ADR 0020 D12: the body can be any expression
        // that produces either a Kont* (direct Perform, call to
        // an effecting fn, nested Handle when nested itself) or
        // an i64 (pure expression — wrapped via
        // sentinel_kont_pure so the dispatch loop sees a
        // uniform Kont* and falls through to the PURE_RETURN
        // case). Earlier sub-phases C3.5(a)/(b) restricted the
        // body shape; ADR 0020 D12's c37_handle_return fixture
        // (`handle 42 with { return v => v * 2 }`) requires the
        // lifted form, which the typing layer already permits.
        let result = self.lower_handle_inner(
            body,
            arms,
            return_arm,
            program,
            is_nested,
        );
        self.handle_depth -= 1;
        result
    }

    fn lower_handle_inner(
        &mut self,
        body: &TypedExpr,
        arms: &[TypedHandlerArm],
        return_arm: Option<&TypedReturnArm>,
        program: &TypedProgram,
        is_nested: bool,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // Lower the body.
        //
        //   - Perform / Call-to-effecting / nested Handle:
        //     produces a Kont* directly.
        //   - Pure expression (i64): wrap via sentinel_kont_pure
        //     so the dispatch loop sees a uniform Kont* (the
        //     switch then matches PURE_RETURN_OP_ID).
        let body_val = self.lower_expr(body, program)?;
        let initial_kont = if body_val.is_pointer_value() {
            body_val.into_pointer_value()
        } else {
            let wrap_call = self
                .builder
                .build_call(
                    self.kont_pure_fn,
                    &[body_val.into_int_value().into()],
                    "body_pure_wrap",
                )
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            wrap_call
                .try_as_basic_value()
                .left()
                .expect("sentinel_kont_pure returns ptr")
                .into_pointer_value()
        };

        let i32_ty = self.context.i32_type();
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let current_fn = self.current_fn.expect("inside compile_fn");

        // Allocate the dispatch slot for the current kont. Each
        // loop iteration re-loads from this slot; bubble paths
        // store the new kont here before branching back.
        let current_kont_slot = self
            .builder
            .build_alloca(ptr_ty, "current_kont_slot")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(current_kont_slot, initial_kont)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        let loop_block = self.context.append_basic_block(current_fn, "handle_loop");
        let merge_block = self.context.append_basic_block(current_fn, "handle_merge");
        // Switch's default destination: a propagate block when
        // nested (contributes the un-caught kont to the merge so
        // the outer handle can dispatch on it), or unreachable
        // at top-level (type-check guarantees full coverage).
        let default_block_name = if is_nested {
            "handle_propagate"
        } else {
            "handle_unreachable"
        };
        let default_block = self
            .context
            .append_basic_block(current_fn, default_block_name);
        let pure_block = self.context.append_basic_block(current_fn, "handle_pure");

        self.builder
            .build_unconditional_branch(loop_block)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Loop block: re-load current kont + read op_id + switch.
        self.builder.position_at_end(loop_block);
        let current_kont = self
            .builder
            .build_load(ptr_ty, current_kont_slot, "current_kont")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_pointer_value();
        let op_id_val = self
            .builder
            .build_load(i32_ty, current_kont, "kont_op_id")
            .map_err(|e| CodegenError::Builder(e.to_string()))?
            .into_int_value();

        // One block per arm + a case-list for the switch. The
        // PURE_RETURN_OP_ID case (u32::MAX) goes to a dedicated
        // pure-return block which unwraps the kont (or, when
        // nested, passes through) and branches to merge.
        let arm_blocks: Vec<_> = arms
            .iter()
            .map(|_| self.context.append_basic_block(current_fn, "handle_arm"))
            .collect();
        let mut cases: Vec<(IntValue<'ctx>, _)> = arms
            .iter()
            .zip(arm_blocks.iter())
            .map(|(a, bb)| {
                let op_id = self.encode_op_id_ctx(a.effect_id, a.op_index, program);
                (i32_ty.const_int(op_id as u64, false), *bb)
            })
            .collect();
        let pure_op_id_const = i32_ty.const_int(u32::MAX as u64, false);
        cases.push((pure_op_id_const, pure_block));

        self.builder
            .build_switch(op_id_val, default_block, &cases)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Pure-return block:
        //
        //   - Top-level: consume_pure → i64; apply return arm
        //     (if any) → i64; flow to merge.
        //   - Nested: if return arm, consume_pure → apply →
        //     re-wrap via sentinel_kont_pure → Kont* (so outer's
        //     pure path can consume_pure). If no return arm,
        //     pass current_kont through unchanged (it's already
        //     a PURE_RETURN-tagged kont).
        self.builder.position_at_end(pure_block);
        let pure_val: BasicValueEnum<'ctx> = if is_nested && return_arm.is_none() {
            // Pass current_kont through.
            current_kont.into()
        } else {
            let pure_call = self
                .builder
                .build_call(
                    self.kont_consume_pure_fn,
                    &[current_kont.into()],
                    "kont_pure_value",
                )
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let pure_unwrap = pure_call
                .try_as_basic_value()
                .left()
                .expect("sentinel_kont_consume_pure returns i64");
            let unwrapped_or_transformed: BasicValueEnum<'ctx> =
                if let Some(ra) = return_arm {
                    // C3.6(a) / ADR 0020 D4: bind the return
                    // arm's value VarId + lower the body.
                    let alloca = self
                        .builder
                        .build_alloca(i64_ty, &ra.value_name.kind)
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                    self.builder
                        .build_store(alloca, pure_unwrap.into_int_value())
                        .map_err(|e| CodegenError::Builder(e.to_string()))?;
                    self.vars.insert(ra.value_var_id, (alloca, Type::I64));
                    let ra_val = self.lower_expr(&ra.body, program)?;
                    self.vars.remove(&ra.value_var_id);
                    ra_val
                } else {
                    pure_unwrap
                };
            if is_nested {
                // Re-wrap the i64 in a PURE_RETURN kont so the
                // outer handle's pure-path can dispatch on it.
                let wrap_call = self
                    .builder
                    .build_call(
                        self.kont_pure_fn,
                        &[unwrapped_or_transformed.into_int_value().into()],
                        "pure_rewrap",
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                wrap_call
                    .try_as_basic_value()
                    .left()
                    .expect("sentinel_kont_pure returns ptr")
            } else {
                unwrapped_or_transformed
            }
        };
        let post_pure_block = self
            .builder
            .get_insert_block()
            .expect("inside pure block");
        self.builder
            .build_unconditional_branch(merge_block)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Push the dispatch context so any k(v) call inside an
        // arm body's lowering can branch back to loop_block on
        // bubble (see [`Self::lower_resume_kont`]). The return
        // arm (if any) is cloned into the context so k(v)'s
        // pure-unwrap path can apply it per Phase B's deep-
        // handler re-wrap semantics.
        self.handle_stack.push(HandleContext {
            loop_block,
            current_kont_slot,
            return_arm: return_arm.cloned(),
        });

        // Lower each arm: bind op-params + kont, lower the
        // arm body, optionally wrap the i64 in pure_kont (when
        // nested) so the merge type is Kont*, then branch to
        // merge. Collect (value, block) pairs for the phi.
        let mut arm_results: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::with_capacity(arms.len());
        for (arm, arm_bb) in arms.iter().zip(arm_blocks.iter()) {
            self.builder.position_at_end(*arm_bb);
            self.bind_handler_arm_params(arm, current_kont)?;
            let arm_val = self.lower_expr(&arm.body, program)?;
            let final_arm_val: BasicValueEnum<'ctx> = if is_nested {
                let wrap_call = self
                    .builder
                    .build_call(
                        self.kont_pure_fn,
                        &[arm_val.into_int_value().into()],
                        "arm_pure_wrap",
                    )
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                wrap_call
                    .try_as_basic_value()
                    .left()
                    .expect("sentinel_kont_pure returns ptr")
            } else {
                arm_val
            };
            let post_arm_block = self
                .builder
                .get_insert_block()
                .expect("inside arm block");
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            arm_results.push((final_arm_val, post_arm_block));
        }

        self.handle_stack.pop();

        // Default block:
        //   - Top-level (!is_nested): unreachable. Type-check
        //     guarantees every op_id matches some arm.
        //   - Nested: propagate the un-caught kont to merge so
        //     the outer handle's dispatch loop catches it.
        self.builder.position_at_end(default_block);
        let propagate_val: Option<(BasicValueEnum<'ctx>, _)> = if is_nested {
            let post_default = self
                .builder
                .get_insert_block()
                .expect("inside default block");
            self.builder
                .build_unconditional_branch(merge_block)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            Some((current_kont.into(), post_default))
        } else {
            self.builder
                .build_unreachable()
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            None
        };

        // Merge: phi over all paths. When nested the type is
        // Kont* (ptr); top-level it's i64 (the handle's outer
        // type per the MVP restriction). Empty arms list (e.g.,
        // `handle X with { return v => body }`) is permitted —
        // we take the type from pure_val.
        self.builder.position_at_end(merge_block);
        let result_ty = if let Some((arm_val, _)) = arm_results.first() {
            arm_val.get_type()
        } else {
            pure_val.get_type()
        };
        let phi = self
            .builder
            .build_phi(result_ty, "handle_result")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        let mut incoming: Vec<(&dyn BasicValue<'ctx>, _)> = arm_results
            .iter()
            .map(|(v, bb)| (v as &dyn BasicValue<'ctx>, *bb))
            .collect();
        incoming.push((&pure_val as &dyn BasicValue<'ctx>, post_pure_block));
        if let Some((v, bb)) = propagate_val.as_ref() {
            incoming.push((v as &dyn BasicValue<'ctx>, *bb));
        }
        phi.add_incoming(&incoming);
        Ok(phi.as_basic_value())
    }

    /// C3.5(b): bind a handler arm's op-param VarIds (via GEP
    /// reads from the kont struct's `arg` field) and the kont
    /// VarId (holding the kont pointer). Allocas land in the
    /// current builder block — the entry-block pattern used
    /// elsewhere doesn't apply here because the switch we
    /// already emitted is entry's terminator, and inserting
    /// after a terminator would surface as an IR verification
    /// failure. LLVM's mem2reg pass copes with non-entry
    /// allocas; correctness is preserved.
    fn bind_handler_arm_params(
        &mut self,
        arm: &TypedHandlerArm,
        kont_ptr: PointerValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let n_op_params = arm.param_var_ids.len() - 1;

        if n_op_params >= 1 {
            // Read kont.arg via GEP at byte offset 8 (after
            // op_id: i32 + _pad: i32). LLVM 15+ opaque
            // pointers — we use i8-indexed GEP for layout
            // stability with the SentinelKont struct.
            let i8_ty = self.context.i8_type();
            let arg_offset = i64_ty.const_int(8, false);
            let arg_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(i8_ty, kont_ptr, &[arg_offset], "kont_arg_ptr")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?
            };
            let arg_val = self
                .builder
                .build_load(i64_ty, arg_ptr, "kont_arg")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            let alloca = self
                .builder
                .build_alloca(i64_ty, "arm_param")
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(alloca, arg_val)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(arm.param_var_ids[0], (alloca, Type::I64));
        }

        // Bind the kont VarId (last param). The store puts
        // kont_ptr into the alloca slot.
        let kont_var_id = *arm
            .param_var_ids
            .last()
            .expect("type-check guarantees the kont VarId is present");
        let kont_alloca = self
            .builder
            .build_alloca(ptr_ty, "kont_var")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_store(kont_alloca, kont_ptr)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.vars
            .insert(kont_var_id, (kont_alloca, Type::Kont(arm.kont_id)));
        Ok(())
    }
}

/// C3.5(a) / ADR 0020 D7: encode `(EffectId, op_index)` as a
/// stable 32-bit op id for runtime tag dispatch. The bit-pack
/// gives ~16M distinct effects and ~64K ops per effect — far
/// beyond any plausible source-level limit.
fn encode_op_id(effect_id: EffectId, op_index: usize) -> u32 {
    (effect_id.0 << 16) | ((op_index as u32) & 0xFFFF)
}

/// C3.5(b) / ADR 0020 D7: at this sub-phase an effecting fn's
/// body must not contain a non-tail `perform` — frame reification
/// at intermediate evaluation sites is not yet implemented.
/// The tail expression can be either:
///   - A `Perform` (lowers to sentinel_perform_op, returns Kont*).
///   - A `Call` to another effecting fn (returns Kont* via the
///     new ABI).
///   - A pure expression (i64-valued; codegen wraps it via
///     sentinel_kont_pure so the fn's effecting ABI is honoured).
///   - A block whose tail recursively meets the same constraint.
///
/// Returns OK for shapes the current codegen can lower;
/// otherwise surfaces `EffectingFnBodyNotDirect`. The general
/// case (let-bound perform + arbitrary frame reification) lands
/// at C3.5(c) / C3.6.
fn validate_effecting_fn_body(
    fn_def: &TypedFnDef,
    program: &TypedProgram,
) -> Result<(), CodegenError> {
    // Statements at the top level (and inside nested blocks
    // reached by the tail) must not themselves perform. The
    // recursive walk in `expr_performs` covers nested blocks
    // automatically.
    for stmt in &fn_def.body.stmts {
        if stmt_performs(&stmt.kind) {
            return Err(CodegenError::EffectingFnBodyNotDirect {
                fn_name: fn_def.name.clone(),
            });
        }
    }
    // Tail can be Perform, Call-to-effecting, or anything pure
    // (no perform/resume inside). Validate accordingly: kont-
    // producing OR fully-pure tails are both lowered (the latter
    // wrapped via sentinel_kont_pure). A tail that mixes perform
    // with surrounding pure context (e.g., `perform Op(...) + 1`)
    // needs frame reification and is rejected.
    let tail = &fn_def.body.tail;
    if tail_produces_kont(tail, program) || !expr_performs(tail) {
        Ok(())
    } else {
        Err(CodegenError::EffectingFnBodyNotDirect {
            fn_name: fn_def.name.clone(),
        })
    }
}

fn stmt_performs(kind: &TypedStmtKind) -> bool {
    match kind {
        TypedStmtKind::Let { value, .. } => expr_performs(value),
        TypedStmtKind::Assign { target, value } => {
            expr_performs(target) || expr_performs(value)
        }
        TypedStmtKind::While { cond, body } => {
            expr_performs(cond)
                || body.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&body.tail)
        }
        // D.5 (2/N): payload-free loop control performs no effect.
        TypedStmtKind::Break | TypedStmtKind::Continue => false,
        TypedStmtKind::Expr(e) => expr_performs(e),
    }
}

fn expr_performs(expr: &TypedExpr) -> bool {
    match &expr.kind {
        TypedExprKind::Perform { .. } | TypedExprKind::ResumeKont { .. } => true,
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            // A handle inside an effecting fn body is a sub-
            // computation that discharges effects internally. We
            // still walk the body in case it itself bubbles
            // unhandled effects through (the arms / return arm
            // type-check at the outer row already).
            expr_performs(body)
                || arms.iter().any(|a| expr_performs(&a.body))
                || return_arm.as_deref().is_some_and(|ra| expr_performs(&ra.body))
        }
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => false,
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => expr_performs(inner),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => expr_performs(l) || expr_performs(r),
        TypedExprKind::Block(b) => {
            b.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&b.tail)
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            expr_performs(cond)
                || then_branch.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&then_branch.tail)
                || else_branch.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&else_branch.tail)
        }
        TypedExprKind::Call { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::StructLit { fields, .. } => fields.iter().any(expr_performs),
        TypedExprKind::FieldAccess { target, .. } => expr_performs(target),
        TypedExprKind::ArrayLit { elements, .. } => elements.iter().any(expr_performs),
        TypedExprKind::Index { target, index, .. } => {
            expr_performs(target) || expr_performs(index)
        }
        TypedExprKind::MethodCall { target, args, .. } => {
            expr_performs(target) || args.iter().any(expr_performs)
        }
        TypedExprKind::ClassInit { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            expr_performs(target) || args.iter().any(expr_performs)
        }
        TypedExprKind::QualifiedCall { args, .. } => args.iter().any(expr_performs),
        // C4.4 / ADR 0024: concurrency forms aren't performs; recurse
        // so a nested perform would still surface (none compose at
        // C4.4 minimum, but the walk stays total + future-proof).
        TypedExprKind::Scope { body, .. } => {
            body.stmts.iter().any(|s| stmt_performs(&s.kind)) || expr_performs(&body.tail)
        }
        TypedExprKind::Spawn { call, .. } => expr_performs(call),
        TypedExprKind::Await { task_expr, .. } => expr_performs(task_expr),
        // Phase D.1 / ADR 0032 (3/N): a perform could nest in a
        // construction arg / scrutinee / arm body.
        TypedExprKind::EnumConstruct { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::Match { scrutinee, arms, .. } => {
            expr_performs(scrutinee) || arms.iter().any(|a| expr_performs(&a.body))
        }
    }
}

/// C4.4 / ADR 0024 D8: does this fn use the effecting-fn `Kont*`
/// ABI? The C3 handler runtime gives any fn with a non-empty
/// effect row a `Kont*`-returning ABI (so a `handle f() with {..}`
/// caller receives a continuation). The `Async` effect is
/// different: it's a direct-runtime marker (spawn/await lower to
/// runtime calls, never `handle`), so a fn declaring ONLY `Async`
/// keeps the plain value-returning ABI. Returns true iff the row
/// contains a perform-effect (any effect other than `Async`).
fn uses_kont_abi(sig: &TypedFnSignature, program: &TypedProgram) -> bool {
    sig.effect_row.iter().any(|eid| {
        program
            .effect_decls
            .get(eid.0 as usize)
            .map(|d| d.name != "Async")
            .unwrap_or(true)
    })
}

/// C3.5(b): does this tail expression produce a Kont* IR value
/// (consistent with the effecting-fn ABI)? True for direct
/// Perform OR Call-to-effecting-fn. False for everything else
/// — those forms need per-eval-site frame reification.
fn tail_produces_kont(tail: &TypedExpr, program: &TypedProgram) -> bool {
    match &tail.kind {
        TypedExprKind::Perform { .. } => true,
        TypedExprKind::Call { id, .. } => {
            let sig = program.signature(*id);
            uses_kont_abi(sig, program)
        }
        // A block whose tail produces a kont is OK as long as its
        // intermediate stmts don't perform. The compile_fn
        // validation above runs stmt_performs on top-level stmts;
        // for nested blocks we recurse.
        TypedExprKind::Block(b) => {
            !b.stmts.iter().any(|s| stmt_performs(&s.kind))
                && tail_produces_kont(&b.tail, program)
        }
        _ => false,
    }
}

/// C3.5(c) / ADR 0020 D7: detect the "single-let-with-effecting-
/// RHS + pure tail" shape of an effecting fn body. This shape is
/// the simplest case where the perform sits in non-tail position
/// — captured into a runtime frame so the kont's resume replays
/// the let-body's evaluation context.
///
/// Returns Some with the let's VarId / Type / RHS expr / tail
/// expr if the shape matches, otherwise None. Other body shapes
/// go through the C3.5(b) "tail-produces-kont OR pure" path.
struct LetShapeInfo<'a> {
    let_id: VarId,
    let_ty: Type,
    rhs: &'a TypedExpr,
    tail: &'a TypedExpr,
}

fn detect_let_shape<'a>(
    fn_def: &'a TypedFnDef,
    program: &TypedProgram,
) -> Option<LetShapeInfo<'a>> {
    let sig = program.signature(fn_def.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    if fn_def.body.stmts.len() != 1 {
        return None;
    }
    let stmt = &fn_def.body.stmts[0];
    let (let_id, value, ty) = match &stmt.kind {
        TypedStmtKind::Let { id, value, ty, .. } => (*id, value, *ty),
        _ => return None,
    };
    // RHS must produce a Kont — i.e., be a Perform or call-to-
    // effecting-fn. The C3.5(b) tail_produces_kont helper checks
    // exactly this.
    if !tail_produces_kont(value, program) {
        return None;
    }
    // Tail must be pure (no nested perform / resume). At MVP
    // we don't yet handle chains of effecting lets.
    if expr_performs(&fn_def.body.tail) {
        return None;
    }
    // MVP restriction: the let-bound type is i64 (the only
    // shape we know how to carry through a SentinelKont's
    // single arg field). Multi-arg ops + non-i64 captures land
    // at a follow-on sub-phase.
    if ty != Type::I64 {
        return None;
    }
    Some(LetShapeInfo {
        let_id,
        let_ty: ty,
        rhs: value,
        tail: &fn_def.body.tail,
    })
}

/// C3.5(e) / ADR 0020 D7: chained effecting lets — `stmts.len() >=
/// 2`, every stmt is `let v: i64 = <produces-kont>`, tail is pure.
/// This is the shape that needs resumers-can-perform: resumer-i's
/// body lowers to `let v_{i+1} = perform Op; ...; tail`, which
/// itself performs and pushes another frame. The runtime
/// `sentinel_kont_resume` bubbles up on non-pure-return result
/// konts to make this work end-to-end.
struct ChainedLetsShapeInfo<'a> {
    lets: Vec<(VarId, Type, &'a TypedExpr)>,
    tail: &'a TypedExpr,
}

fn detect_chained_effecting_lets_shape<'a>(
    fn_def: &'a TypedFnDef,
    program: &TypedProgram,
) -> Option<ChainedLetsShapeInfo<'a>> {
    let sig = program.signature(fn_def.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    // C3.5(c) covers stmts.len() == 1; C3.5(e) is the next step.
    if fn_def.body.stmts.len() < 2 {
        return None;
    }
    let mut lets = Vec::with_capacity(fn_def.body.stmts.len());
    for stmt in &fn_def.body.stmts {
        let (id, value, ty) = match &stmt.kind {
            TypedStmtKind::Let { id, value, ty, .. } => (*id, value, *ty),
            _ => return None,
        };
        // Each let's RHS must produce a Kont — direct Perform or
        // Call to an effecting fn — so we can push the next
        // resumer onto it.
        if !tail_produces_kont(value, program) {
            return None;
        }
        // MVP restriction (same as C3.5(c)/(d)): bound type is i64
        // so the kont's single `arg` slot carries the resumed value.
        if ty != Type::I64 {
            return None;
        }
        lets.push((id, ty, value));
    }
    // Tail must be pure — no perform / resume nested in the final
    // expression. Performs inside the tail would need additional
    // frame reification on top of the chain (a follow-on sub-phase).
    if expr_performs(&fn_def.body.tail) {
        return None;
    }
    Some(ChainedLetsShapeInfo {
        lets,
        tail: &fn_def.body.tail,
    })
}

/// C3.5(e) / ADR 0020 D7: captures needed by resumer-i in a
/// chained-lets fn — the set of VarIds referenced anywhere in the
/// body that resumer-i runs (the suffix `lets[i+1..]`'s RHSes +
/// `tail`), minus the resumed-value let_id (which is the resumer's
/// `value` param) and the future-bound let_ids (which are bound in
/// deeper resumers, not captured from outer scope).
fn compute_chained_lets_captures(
    info: &ChainedLetsShapeInfo<'_>,
    level: usize,
) -> Vec<VarId> {
    let mut refs: Vec<VarId> = Vec::new();
    for j in (level + 1)..info.lets.len() {
        walk_collect_var_refs(info.lets[j].2, &mut refs);
    }
    walk_collect_var_refs(info.tail, &mut refs);
    let value_param_id = info.lets[level].0;
    let mut deeper_let_ids: Vec<VarId> = info
        .lets
        .iter()
        .skip(level + 1)
        .map(|x| x.0)
        .collect();
    deeper_let_ids.push(value_param_id);
    refs.into_iter()
        .filter(|id| !deeper_let_ids.contains(id))
        .collect()
}

/// C3.5(c) / ADR 0020 D7: walk a tail expression collecting the
/// VarIds it references that need to be captured into the runtime
/// frame so the resumer can re-evaluate the tail. Captured set
/// excludes the let-bound VarId (filled from the resumed value at
/// resume time) and excludes builtins / fn ids (those are
/// resolved through fn_table, not vars). MVP-restricted to i64
/// captures by the caller.
fn collect_captured_vars(tail: &TypedExpr, let_id: VarId) -> Vec<VarId> {
    let mut acc: Vec<VarId> = Vec::new();
    walk_collect_var_refs(tail, &mut acc);
    acc.into_iter()
        .filter(|id| *id != let_id)
        .collect()
}

fn walk_collect_var_refs(expr: &TypedExpr, acc: &mut Vec<VarId>) {
    match &expr.kind {
        TypedExprKind::Var(id) => {
            if !acc.contains(id) {
                acc.push(*id);
            }
        }
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_) => {}
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => walk_collect_var_refs(inner, acc),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_collect_var_refs(l, acc);
            walk_collect_var_refs(r, acc);
        }
        TypedExprKind::Block(b) => {
            for s in &b.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&b.tail, acc);
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_collect_var_refs(cond, acc);
            for s in &then_branch.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&then_branch.tail, acc);
            for s in &else_branch.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&else_branch.tail, acc);
        }
        TypedExprKind::Call { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                walk_collect_var_refs(f, acc);
            }
        }
        TypedExprKind::FieldAccess { target, .. } => walk_collect_var_refs(target, acc),
        TypedExprKind::ArrayLit { elements, .. } => {
            for e in elements {
                walk_collect_var_refs(e, acc);
            }
        }
        TypedExprKind::Index { target, index, .. } => {
            walk_collect_var_refs(target, acc);
            walk_collect_var_refs(index, acc);
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            walk_collect_var_refs(body, acc);
            for arm in arms {
                walk_collect_var_refs(&arm.body, acc);
            }
            if let Some(ra) = return_arm.as_deref() {
                walk_collect_var_refs(&ra.body, acc);
            }
        }
        TypedExprKind::Perform { args, .. } | TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::MethodCall { target, args, .. } => {
            walk_collect_var_refs(target, acc);
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::ClassInit { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            walk_collect_var_refs(target, acc);
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        // C4.4 / ADR 0024: recurse into concurrency-form children.
        TypedExprKind::Scope { body, .. } => {
            for s in &body.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&body.tail, acc);
        }
        TypedExprKind::Spawn { call, .. } => walk_collect_var_refs(call, acc),
        TypedExprKind::Await { task_expr, .. } => walk_collect_var_refs(task_expr, acc),
        // Phase D.1 / ADR 0032 (3/N): collect var refs from the
        // construction args / scrutinee / arm bodies.
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::Match { scrutinee, arms, .. } => {
            walk_collect_var_refs(scrutinee, acc);
            for arm in arms {
                walk_collect_var_refs(&arm.body, acc);
            }
        }
    }
}

fn walk_collect_var_refs_stmt(kind: &TypedStmtKind, acc: &mut Vec<VarId>) {
    match kind {
        TypedStmtKind::Let { value, .. } => walk_collect_var_refs(value, acc),
        TypedStmtKind::Assign { target, value } => {
            walk_collect_var_refs(target, acc);
            walk_collect_var_refs(value, acc);
        }
        TypedStmtKind::While { cond, body } => {
            walk_collect_var_refs(cond, acc);
            for s in &body.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&body.tail, acc);
        }
        // D.5 (2/N): payload-free loop control references no var.
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
        TypedStmtKind::Expr(e) => walk_collect_var_refs(e, acc),
    }
}

/// C3.5(d) / ADR 0020 D7: count occurrences of `Perform` inside
/// an expression's subtree. Used by `detect_embedded_perform_shape`
/// to require exactly one perform in the body's tail — when the
/// count is 1 the surrounding context can be reified as a single
/// per-site resumer fn.
fn count_performs(expr: &TypedExpr) -> usize {
    match &expr.kind {
        TypedExprKind::Perform { args, .. } => {
            1 + args.iter().map(count_performs).sum::<usize>()
        }
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => 0,
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => count_performs(inner),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => count_performs(l) + count_performs(r),
        TypedExprKind::Block(b) => {
            b.stmts.iter().map(|s| count_performs_stmt(&s.kind)).sum::<usize>()
                + count_performs(&b.tail)
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            count_performs(cond)
                + then_branch
                    .stmts
                    .iter()
                    .map(|s| count_performs_stmt(&s.kind))
                    .sum::<usize>()
                + count_performs(&then_branch.tail)
                + else_branch
                    .stmts
                    .iter()
                    .map(|s| count_performs_stmt(&s.kind))
                    .sum::<usize>()
                + count_performs(&else_branch.tail)
        }
        TypedExprKind::Call { args, .. } => args.iter().map(count_performs).sum(),
        TypedExprKind::StructLit { fields, .. } => {
            fields.iter().map(count_performs).sum()
        }
        TypedExprKind::FieldAccess { target, .. } => count_performs(target),
        TypedExprKind::ArrayLit { elements, .. } => {
            elements.iter().map(count_performs).sum()
        }
        TypedExprKind::Index { target, index, .. } => {
            count_performs(target) + count_performs(index)
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            count_performs(body)
                + arms.iter().map(|a| count_performs(&a.body)).sum::<usize>()
                + return_arm
                    .as_deref()
                    .map_or(0, |ra| count_performs(&ra.body))
        }
        TypedExprKind::ResumeKont { args, .. } => args.iter().map(count_performs).sum(),
        TypedExprKind::MethodCall { target, args, .. } => {
            count_performs(target) + args.iter().map(count_performs).sum::<usize>()
        }
        TypedExprKind::ClassInit { args, .. } => args.iter().map(count_performs).sum(),
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            count_performs(target) + args.iter().map(count_performs).sum::<usize>()
        }
        TypedExprKind::QualifiedCall { args, .. } => args.iter().map(count_performs).sum(),
        // C4.4 / ADR 0024: recurse into concurrency-form children.
        TypedExprKind::Scope { body, .. } => {
            body.stmts.iter().map(|s| count_performs_stmt(&s.kind)).sum::<usize>()
                + count_performs(&body.tail)
        }
        TypedExprKind::Spawn { call, .. } => count_performs(call),
        TypedExprKind::Await { task_expr, .. } => count_performs(task_expr),
        // Phase D.1 / ADR 0032 (3/N): sum performs across the
        // construction args / scrutinee / arm bodies.
        TypedExprKind::EnumConstruct { args, .. } => args.iter().map(count_performs).sum(),
        TypedExprKind::Match { scrutinee, arms, .. } => {
            count_performs(scrutinee) + arms.iter().map(|a| count_performs(&a.body)).sum::<usize>()
        }
    }
}

fn count_performs_stmt(kind: &TypedStmtKind) -> usize {
    match kind {
        TypedStmtKind::Let { value, .. } => count_performs(value),
        TypedStmtKind::Assign { target, value } => {
            count_performs(target) + count_performs(value)
        }
        TypedStmtKind::While { cond, body } => {
            count_performs(cond)
                + body
                    .stmts
                    .iter()
                    .map(|s| count_performs_stmt(&s.kind))
                    .sum::<usize>()
                + count_performs(&body.tail)
        }
        // D.5 (2/N): payload-free loop control contains no `perform`.
        TypedStmtKind::Break | TypedStmtKind::Continue => 0,
        TypedStmtKind::Expr(e) => count_performs(e),
    }
}

/// C3.5(d) / ADR 0020 D7: locate the first Perform in
/// pre-order. Used in tandem with `count_performs == 1` to find
/// THE unique perform site that's being reified.
fn find_unique_perform(expr: &TypedExpr) -> Option<&TypedExpr> {
    if matches!(expr.kind, TypedExprKind::Perform { .. }) {
        return Some(expr);
    }
    match &expr.kind {
        TypedExprKind::Perform { .. } => unreachable!("handled above"),
        TypedExprKind::IntLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => None,
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => find_unique_perform(inner),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            find_unique_perform(l).or_else(|| find_unique_perform(r))
        }
        TypedExprKind::Block(b) => {
            for s in &b.stmts {
                if let Some(p) = find_unique_perform_stmt(&s.kind) {
                    return Some(p);
                }
            }
            find_unique_perform(&b.tail)
        }
        TypedExprKind::If { cond, then_branch, else_branch } => find_unique_perform(cond)
            .or_else(|| {
                then_branch
                    .stmts
                    .iter()
                    .find_map(|s| find_unique_perform_stmt(&s.kind))
                    .or_else(|| find_unique_perform(&then_branch.tail))
            })
            .or_else(|| {
                else_branch
                    .stmts
                    .iter()
                    .find_map(|s| find_unique_perform_stmt(&s.kind))
                    .or_else(|| find_unique_perform(&else_branch.tail))
            }),
        TypedExprKind::Call { args, .. } => args.iter().find_map(find_unique_perform),
        TypedExprKind::StructLit { fields, .. } => {
            fields.iter().find_map(find_unique_perform)
        }
        TypedExprKind::FieldAccess { target, .. } => find_unique_perform(target),
        TypedExprKind::ArrayLit { elements, .. } => {
            elements.iter().find_map(find_unique_perform)
        }
        TypedExprKind::Index { target, index, .. } => {
            find_unique_perform(target).or_else(|| find_unique_perform(index))
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => find_unique_perform(body)
            .or_else(|| arms.iter().find_map(|a| find_unique_perform(&a.body)))
            .or_else(|| {
                return_arm
                    .as_deref()
                    .and_then(|ra| find_unique_perform(&ra.body))
            }),
        TypedExprKind::ResumeKont { args, .. } => args.iter().find_map(find_unique_perform),
        TypedExprKind::MethodCall { target, args, .. } => {
            find_unique_perform(target).or_else(|| args.iter().find_map(find_unique_perform))
        }
        TypedExprKind::ClassInit { args, .. } => args.iter().find_map(find_unique_perform),
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            find_unique_perform(target).or_else(|| args.iter().find_map(find_unique_perform))
        }
        TypedExprKind::QualifiedCall { args, .. } => args.iter().find_map(find_unique_perform),
        // C4.4 / ADR 0024: recurse into concurrency-form children.
        TypedExprKind::Scope { body, .. } => body
            .stmts
            .iter()
            .find_map(|s| find_unique_perform_stmt(&s.kind))
            .or_else(|| find_unique_perform(&body.tail)),
        TypedExprKind::Spawn { call, .. } => find_unique_perform(call),
        TypedExprKind::Await { task_expr, .. } => find_unique_perform(task_expr),
        // Phase D.1 / ADR 0032 (3/N): search the construction args /
        // scrutinee / arm bodies for the unique embedded perform.
        TypedExprKind::EnumConstruct { args, .. } => args.iter().find_map(find_unique_perform),
        TypedExprKind::Match { scrutinee, arms, .. } => find_unique_perform(scrutinee)
            .or_else(|| arms.iter().find_map(|a| find_unique_perform(&a.body))),
    }
}

fn find_unique_perform_stmt(kind: &TypedStmtKind) -> Option<&TypedExpr> {
    match kind {
        TypedStmtKind::Let { value, .. } => find_unique_perform(value),
        TypedStmtKind::Assign { target, value } => {
            find_unique_perform(target).or_else(|| find_unique_perform(value))
        }
        TypedStmtKind::While { cond, body } => find_unique_perform(cond)
            .or_else(|| body.stmts.iter().find_map(|s| find_unique_perform_stmt(&s.kind)))
            .or_else(|| find_unique_perform(&body.tail)),
        // D.5 (2/N): payload-free loop control contains no `perform`.
        TypedStmtKind::Break | TypedStmtKind::Continue => None,
        TypedStmtKind::Expr(e) => find_unique_perform(e),
    }
}

/// C3.5(d) / ADR 0020 D7: clone an expression tree, replacing
/// the unique `Perform` (caller's responsibility — assumes
/// `count_performs == 1`) with a `Var(placeholder_id)` of the
/// perform's return type. The resumer fn body uses the
/// substituted tail; binding `placeholder_id` to the resumed
/// value reconstitutes the original control flow.
fn substitute_perform_with_var(expr: &TypedExpr, placeholder_id: VarId) -> TypedExpr {
    if matches!(expr.kind, TypedExprKind::Perform { .. }) {
        return TypedExpr {
            kind: TypedExprKind::Var(placeholder_id),
            span: expr.span.clone(),
            ty: expr.ty,
        };
    }
    let new_kind = match &expr.kind {
        TypedExprKind::Perform { .. } => unreachable!("handled above"),
        TypedExprKind::IntLit(n) => TypedExprKind::IntLit(*n),
        TypedExprKind::BoolLit(b) => TypedExprKind::BoolLit(*b),
        TypedExprKind::NullLit => TypedExprKind::NullLit,
        // D.2 / ADR 0033: literal bytes carry no perform — clone as-is.
        TypedExprKind::CharLit(b) => TypedExprKind::CharLit(*b),
        TypedExprKind::StringLit(bytes) => TypedExprKind::StringLit(bytes.clone()),
        TypedExprKind::Var(id) => TypedExprKind::Var(*id),
        TypedExprKind::Unary(op, inner) => TypedExprKind::Unary(
            *op,
            Box::new(substitute_perform_with_var(inner, placeholder_id)),
        ),
        TypedExprKind::WidenToNullable(inner) => TypedExprKind::WidenToNullable(
            Box::new(substitute_perform_with_var(inner, placeholder_id)),
        ),
        TypedExprKind::WidenToSecret(inner) => TypedExprKind::WidenToSecret(
            Box::new(substitute_perform_with_var(inner, placeholder_id)),
        ),
        TypedExprKind::Declassify(inner) => TypedExprKind::Declassify(
            Box::new(substitute_perform_with_var(inner, placeholder_id)),
        ),
        TypedExprKind::Cast(inner) => TypedExprKind::Cast(Box::new(
            substitute_perform_with_var(inner, placeholder_id),
        )),
        TypedExprKind::Binary(op, l, r) => TypedExprKind::Binary(
            *op,
            Box::new(substitute_perform_with_var(l, placeholder_id)),
            Box::new(substitute_perform_with_var(r, placeholder_id)),
        ),
        TypedExprKind::Cmp(op, l, r) => TypedExprKind::Cmp(
            *op,
            Box::new(substitute_perform_with_var(l, placeholder_id)),
            Box::new(substitute_perform_with_var(r, placeholder_id)),
        ),
        TypedExprKind::Logic(op, l, r) => TypedExprKind::Logic(
            *op,
            Box::new(substitute_perform_with_var(l, placeholder_id)),
            Box::new(substitute_perform_with_var(r, placeholder_id)),
        ),
        TypedExprKind::Block(b) => {
            let stmts: Vec<TypedStmt> = b
                .stmts
                .iter()
                .map(|s| TypedStmt {
                    kind: substitute_perform_with_var_stmt(&s.kind, placeholder_id),
                    span: s.span.clone(),
                })
                .collect();
            let tail = substitute_perform_with_var(&b.tail, placeholder_id);
            TypedExprKind::Block(Box::new(TypedBlock {
                stmts,
                tail,
                span: b.span.clone(),
                ty: b.ty,
            }))
        }
        TypedExprKind::If { cond, then_branch, else_branch } => TypedExprKind::If {
            cond: Box::new(substitute_perform_with_var(cond, placeholder_id)),
            then_branch: Box::new(substitute_block(then_branch, placeholder_id)),
            else_branch: Box::new(substitute_block(else_branch, placeholder_id)),
        },
        TypedExprKind::Call { id, callee_span, args, type_args } => TypedExprKind::Call {
            id: *id,
            callee_span: callee_span.clone(),
            args: args
                .iter()
                .map(|a| substitute_perform_with_var(a, placeholder_id))
                .collect(),
            type_args: type_args.clone(),
        },
        TypedExprKind::StructLit { id, name, name_span, fields } => {
            TypedExprKind::StructLit {
                id: *id,
                name: name.clone(),
                name_span: name_span.clone(),
                fields: fields
                    .iter()
                    .map(|f| substitute_perform_with_var(f, placeholder_id))
                    .collect(),
            }
        }
        TypedExprKind::FieldAccess { target, field, field_span, field_index } => {
            TypedExprKind::FieldAccess {
                target: Box::new(substitute_perform_with_var(target, placeholder_id)),
                field: field.clone(),
                field_span: field_span.clone(),
                field_index: *field_index,
            }
        }
        TypedExprKind::ArrayLit { elem_ty, elements } => TypedExprKind::ArrayLit {
            elem_ty: *elem_ty,
            elements: elements
                .iter()
                .map(|e| substitute_perform_with_var(e, placeholder_id))
                .collect(),
        },
        TypedExprKind::Index { target, index, elem_ty } => TypedExprKind::Index {
            target: Box::new(substitute_perform_with_var(target, placeholder_id)),
            index: Box::new(substitute_perform_with_var(index, placeholder_id)),
            elem_ty: *elem_ty,
        },
        TypedExprKind::Handle { .. }
        | TypedExprKind::ResumeKont { .. }
        | TypedExprKind::MethodCall { .. }
        | TypedExprKind::ClassInit { .. }
        | TypedExprKind::ImplMethodCall { .. }
        | TypedExprKind::QualifiedCall { .. }
        // C4.4 / ADR 0024: concurrency forms never embed a
        // substitutable perform at C4.4 minimum (count would
        // exceed 1) — clone them unchanged like the group above.
        | TypedExprKind::Scope { .. }
        | TypedExprKind::Spawn { .. }
        | TypedExprKind::Await { .. }
        // Phase D.1 / ADR 0032 (3/N): an enum construction / `match`
        // never embeds a single substitutable perform at the MVP
        // (count would exceed 1); clone unchanged like the group.
        | TypedExprKind::EnumConstruct { .. }
        | TypedExprKind::Match { .. } => {
            // C3.5(d) MVP: the embedded-perform shape only fires
            // when count_performs(tail) == 1. Substituting a
            // Handle / ResumeKont / MethodCall / ClassInit
            // preserves them as is — they don't have a perform
            // inside (the count would exceed 1). Conservative:
            // clone the kind unchanged.
            return TypedExpr {
                kind: clone_expr_kind(&expr.kind),
                span: expr.span.clone(),
                ty: expr.ty,
            };
        }
    };
    TypedExpr {
        kind: new_kind,
        span: expr.span.clone(),
        ty: expr.ty,
    }
}

fn substitute_perform_with_var_stmt(
    kind: &TypedStmtKind,
    placeholder_id: VarId,
) -> TypedStmtKind {
    match kind {
        TypedStmtKind::Let { id, mutable, name, name_span, ty, value } => {
            TypedStmtKind::Let {
                id: *id,
                mutable: *mutable,
                name: name.clone(),
                name_span: name_span.clone(),
                ty: *ty,
                value: substitute_perform_with_var(value, placeholder_id),
            }
        }
        TypedStmtKind::Assign { target, value } => TypedStmtKind::Assign {
            target: substitute_perform_with_var(target, placeholder_id),
            value: substitute_perform_with_var(value, placeholder_id),
        },
        TypedStmtKind::While { cond, body } => TypedStmtKind::While {
            cond: substitute_perform_with_var(cond, placeholder_id),
            body: Box::new(substitute_block(body, placeholder_id)),
        },
        // D.5 (2/N): payload-free loop control — identity substitution.
        TypedStmtKind::Break => TypedStmtKind::Break,
        TypedStmtKind::Continue => TypedStmtKind::Continue,
        TypedStmtKind::Expr(e) => TypedStmtKind::Expr(substitute_perform_with_var(
            e,
            placeholder_id,
        )),
    }
}

fn substitute_block(b: &TypedBlock, placeholder_id: VarId) -> TypedBlock {
    TypedBlock {
        stmts: b
            .stmts
            .iter()
            .map(|s| TypedStmt {
                kind: substitute_perform_with_var_stmt(&s.kind, placeholder_id),
                span: s.span.clone(),
            })
            .collect(),
        tail: substitute_perform_with_var(&b.tail, placeholder_id),
        span: b.span.clone(),
        ty: b.ty,
    }
}

/// Conservative clone of an expression kind that doesn't need
/// substitution because it contains no Perform.
fn clone_expr_kind(kind: &TypedExprKind) -> TypedExprKind {
    kind.clone()
}

/// C3.5(d) / ADR 0020 D7: detect the unified "single embedded
/// perform" body shape — `stmts.len() == 0` and the tail contains
/// exactly one Perform anywhere in its tree. Covers binops,
/// struct-lit fields, field-access targets, index ops, etc. (any
/// pure surrounding context with a single perform "hole"). The
/// resumer fn substitutes the perform with a placeholder Var and
/// lowers the resulting expression.
///
/// Excludes shapes already handled at C3.5(a)/(b): a tail that
/// IS a direct Perform (the trivial perform-at-tail case) and a
/// tail that's a direct Call to an effecting fn. Both produce a
/// Kont* directly without needing frame reification.
///
/// MVP-restricted to i64-returning ops (the placeholder's type
/// is i64). Non-i64 ops + nested performs land at a follow-on
/// sub-phase.
struct EmbeddedPerformInfo {
    captured: Vec<VarId>,
    substituted_tail: TypedExpr,
    placeholder_id: VarId,
}

fn detect_embedded_perform_shape(
    fn_def: &TypedFnDef,
    program: &TypedProgram,
) -> Option<EmbeddedPerformInfo> {
    let sig = program.signature(fn_def.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    if !fn_def.body.stmts.is_empty() {
        return None;
    }
    let tail = &fn_def.body.tail;
    // Skip shapes already covered by C3.5(a)/(b).
    if matches!(tail.kind, TypedExprKind::Perform { .. }) {
        return None;
    }
    if let TypedExprKind::Call { id, .. } = &tail.kind {
        if uses_kont_abi(program.signature(*id), program) {
            return None;
        }
    }
    // Must have exactly one perform somewhere in the tail.
    if count_performs(tail) != 1 {
        return None;
    }
    let perform = find_unique_perform(tail)?;
    if perform.ty != Type::I64 {
        return None;
    }
    // Placeholder VarId is synthetic — codegen binds it in the
    // resumer's env and consumes it in the substituted tail. A
    // sentinel high value avoids collision with resolve-assigned
    // VarIds (which start at 0 and grow with binding count).
    let placeholder_id = VarId(u32::MAX);
    let substituted_tail = substitute_perform_with_var(tail, placeholder_id);
    // Captured vars = vars referenced in the substituted tail,
    // excluding the placeholder.
    let mut captured: Vec<VarId> = Vec::new();
    walk_collect_var_refs(&substituted_tail, &mut captured);
    let captured: Vec<VarId> =
        captured.into_iter().filter(|id| *id != placeholder_id).collect();
    Some(EmbeddedPerformInfo {
        captured,
        substituted_tail,
        placeholder_id,
    })
}

impl<'ctx, 'plan> CodegenCtx<'ctx, 'plan> {

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
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_) => None,
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Declassify(inner) => {
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
        // C3.4 / ADR 0020 D5: defensive walk into handler bodies +
        // perform args for binding-name lookup. These don't reach
        // codegen at C3.4 (lower_expr bails) but the walk needs to
        // be total.
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            find_var_name_in_expr(body, id)
                .or_else(|| {
                    arms.iter().find_map(|a| find_var_name_in_expr(&a.body, id))
                })
                .or_else(|| {
                    return_arm
                        .as_ref()
                        .and_then(|ra| find_var_name_in_expr(&ra.body, id))
                })
        }
        TypedExprKind::Perform { args, .. } | TypedExprKind::ResumeKont { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
        }
        TypedExprKind::MethodCall { target, args, .. } => {
            find_var_name_in_expr(target, id)
                .or_else(|| args.iter().find_map(|a| find_var_name_in_expr(a, id)))
        }
        TypedExprKind::ClassInit { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
        }
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            find_var_name_in_expr(target, id)
                .or_else(|| args.iter().find_map(|a| find_var_name_in_expr(a, id)))
        }
        TypedExprKind::QualifiedCall { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
        }
        // C4.4 / ADR 0024: recurse into concurrency-form children.
        TypedExprKind::Scope { body, .. } => find_var_name_in_block(body, id),
        TypedExprKind::Spawn { call, .. } => find_var_name_in_expr(call, id),
        TypedExprKind::Await { task_expr, .. } => find_var_name_in_expr(task_expr, id),
        // Phase D.1 / ADR 0032 (3/N): search construction args /
        // scrutinee / arm bodies for the var's source name.
        TypedExprKind::EnumConstruct { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
        }
        TypedExprKind::Match { scrutinee, arms, .. } => find_var_name_in_expr(scrutinee, id)
            .or_else(|| arms.iter().find_map(|a| find_var_name_in_expr(&a.body, id))),
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

/// C5.4 (2/N) / ADR 0028: compute the program-wide set of binding
/// VarIds whose heap allocation is routed into a per-scope broker bump
/// arena (and whose individual `sentinel_free` is therefore skipped, the
/// arena reset reclaiming it instead).
///
/// **Safety bar (the airtight argument).** The routed set is, by
/// construction, a *subset of exactly what [`CodegenCtx::emit_scope_drops`]
/// already frees* at scope exit: a binding `x` is routed iff it is a
/// `let x = [primitive array literal]` (element type i64/i32/bool — so
/// the only heap footprint is the one `{len,data}` data buffer that the
/// `Type::Array` drop frees) AND `x ∉ moved_sources(fn)` AND
/// `Some(x) != tail_returned_var(&B.tail)` for `x`'s declaring block `B`.
/// Those last two are the *same* predicate `emit_scope_drops` uses, so a
/// routed binding is one the borrow checker already proved is dropped
/// (non-escaping) at that exact scope exit. Routing is therefore as safe
/// as today's per-binding free — same bindings, same lifetime, same
/// point, bulk vs individual — and a returned/moved array (which *is*
/// recorded in `moved_sources`; see the note below) is never routed.
///
/// **Note on `moved_sources`.** A tail-returned array such as
/// `fn make() -> [i64] { let a = [1,2,3]; a }` *is* in `moved_sources`:
/// the borrow checker walks the tail `Var(a)` as a consuming move
/// (arrays are Move-typed) before snapshotting the DropPlan, so `∉ moved`
/// alone already excludes it. The `tail_returned` half of the predicate
/// is kept regardless, so the routed set matches `emit_scope_drops`
/// exactly under any reading.
///
/// Scope is deliberately narrow for this first slice: only non-generic
/// (`type_params` empty), non-effecting (`!uses_kont_abi`) free
/// functions, which always lower via the standard `lower_block` +
/// `emit_scope_drops` path. Methods, generics, effecting fns, and
/// non-primitive-element arrays keep the unchanged libc path. VarIds are
/// globally unique, so one program-wide set is queried per binding
/// without ambiguity.
fn compute_arena_routed(program: &TypedProgram, drop_plan: &DropPlan) -> HashSet<VarId> {
    let mut routed = HashSet::new();
    for fn_def in &program.fns {
        if !fn_def.type_params.is_empty() {
            continue;
        }
        if uses_kont_abi(program.signature(fn_def.id), program) {
            continue;
        }
        let moved = drop_plan.moved_sources_for(fn_def.id);
        collect_routed_block(&fn_def.body, moved, &mut routed);
    }
    routed
}

/// Walk one lexical scope (a [`TypedBlock`]) collecting arena-routable
/// `let` bindings, then recurse into nested scopes. Mirrors exactly how
/// `lower_block` + `emit_scope_drops` treat the block: a direct `let`'s
/// binding is freed at *this* block's exit unless it is moved out or is
/// this block's tail value, so the routing predicate is evaluated
/// against this same block's `tail_returned_var(&block.tail)`.
fn collect_routed_block(
    block: &TypedBlock,
    moved: &std::collections::BTreeSet<VarId>,
    routed: &mut HashSet<VarId>,
) {
    let tail_var = tail_returned_var(&block.tail);
    for stmt in &block.stmts {
        match &stmt.kind {
            TypedStmtKind::Let { id, ty, value, .. } => {
                if is_primitive_array_lit(value, *ty)
                    && !moved.contains(id)
                    && Some(*id) != tail_var
                {
                    routed.insert(*id);
                }
                // A nested scope may live inside the RHS (e.g. a block
                // expression); recurse so its routable lets are found.
                collect_routed_expr(value, moved, routed);
            }
            TypedStmtKind::Assign { value, .. } => collect_routed_expr(value, moved, routed),
            // Phase D.5 / ADR 0036: do NOT arena-route bindings inside a
            // loop body. They are allocated + freed PER ITERATION; routing
            // them into a scope arena would risk accumulating one
            // allocation per iteration before the bulk free. The
            // conservative choice keeps the per-iteration libc free
            // (emit_scope_drops) — the routed set stays a subset of the
            // freed set, preserving the C5.4 routing-safety invariant.
            // Recurse into the condition's nested exprs only.
            TypedStmtKind::While { cond, .. } => collect_routed_expr(cond, moved, routed),
            // D.5 (2/N): payload-free loop control routes nothing.
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
            TypedStmtKind::Expr(e) => collect_routed_expr(e, moved, routed),
        }
    }
    collect_routed_expr(&block.tail, moved, routed);
}

/// Recurse into the block-bearing expression forms (`{ ... }` and `if`),
/// where `lower_block` opens further scopes. Other expression kinds are
/// not descended into for this first slice — any array literal buried in
/// them simply stays on the unchanged libc path (conservatively
/// un-routed, never mis-routed).
fn collect_routed_expr(
    expr: &TypedExpr,
    moved: &std::collections::BTreeSet<VarId>,
    routed: &mut HashSet<VarId>,
) {
    match &expr.kind {
        TypedExprKind::Block(b) => collect_routed_block(b, moved, routed),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            collect_routed_expr(cond, moved, routed);
            collect_routed_block(then_branch, moved, routed);
            collect_routed_block(else_branch, moved, routed);
        }
        _ => {}
    }
}

/// Is `value` a `[primitive array literal]` whose binding type is an
/// array of i64/i32/bool? Restricting to primitive element types keeps
/// the routed footprint to exactly one heap buffer (the `{len,data}`
/// data pointer) with exactly one `Type::Array` drop, so the arena reset
/// reclaims precisely what the skipped `sentinel_free` would have.
fn is_primitive_array_lit(value: &TypedExpr, ty: Type) -> bool {
    matches!(value.kind, TypedExprKind::ArrayLit { .. })
        && matches!(
            ty,
            Type::Array(ArrayElem::I64) | Type::Array(ArrayElem::I32) | Type::Array(ArrayElem::Bool)
        )
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
        // C2.4: codegen consults a DropPlan. For codegen unit tests we
        // use an empty plan — equivalent to "no moves recorded", so
        // every heap-backed binding gets dropped at scope exit (the
        // conservative default, which exercises the drop emission path).
        // C5.1a / ADR 0026 D3: codegen now consumes the HIR bundle.
        let drop_plan = DropPlan::default();
        let hir = sentinel_hir::lower_to_hir(&typed, &drop_plan);
        compile_to_object(&hir, &out_path())
    }

    fn out_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sentinel-codegen-test-{}.o", std::process::id()))
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-codegen");
    }

    // ===== C5 D7 / ADR 0029: abi-v1 name-mangling stability =====
    //
    // Golden-string asserts pinning the mangling scheme documented in
    // docs/abi-v1.md §4. A drift in a type tag, the `__` separator, or
    // the mono-name composition turns these red. Update docs/abi-v1.md
    // in the same commit as any intentional change.
    #[test]
    fn abi_v1_mangling_is_stable() {
        // A real TypedProgram so struct-name lookups resolve. `Pair` is
        // the first user struct → StructId(0).
        let prog = parse("struct Pair { a: i64, b: i64 } fn main() -> i64 { 0 }").expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");

        // Type tags (mangle_type).
        assert_eq!(mangle_type(Type::I64, &typed), "i64");
        assert_eq!(mangle_type(Type::I32, &typed), "i32");
        assert_eq!(mangle_type(Type::Bool, &typed), "bool");
        // D.2 / ADR 0033 D4: `u8` mangles `u8`; `[u8]` (the string) → `arr_u8`.
        assert_eq!(mangle_type(Type::U8, &typed), "u8");
        assert_eq!(mangle_type(Type::Struct(StructId(0)), &typed), "Pair");
        // Array tag recurses on the element type.
        assert_eq!(mangle_type(Type::Array(ArrayElem::I64), &typed), "arr_i64");
        assert_eq!(mangle_type(Type::Array(ArrayElem::Bool), &typed), "arr_bool");
        assert_eq!(mangle_type(Type::Array(ArrayElem::U8), &typed), "arr_u8");
        assert_eq!(
            mangle_type(Type::Array(ArrayElem::Struct(StructId(0))), &typed),
            "arr_Pair"
        );
        // Nullable tag recurses on the inner type.
        assert_eq!(mangle_type(Type::Nullable(NullableInner::I64), &typed), "opt_i64");
        assert_eq!(
            mangle_type(Type::Nullable(NullableInner::Struct(StructId(0))), &typed),
            "opt_Pair"
        );

        // Monomorphic-instance composition (mangle_mono_name): base name,
        // then `__` + each type-arg tag, in order.
        assert_eq!(mangle_mono_name("id", &[Type::I64], &typed), "id__i64");
        assert_eq!(
            mangle_mono_name("pick", &[Type::I64, Type::Bool], &typed),
            "pick__i64__bool"
        );
        assert_eq!(
            mangle_mono_name("wrap", &[Type::Array(ArrayElem::I64)], &typed),
            "wrap__arr_i64"
        );
        // Zero type-args → the bare base name (a non-generic fn).
        assert_eq!(mangle_mono_name("main", &[], &typed), "main");
    }

    // ===== D.6 / ADR 0037 D7: module-qualified mangling (abi-v1 amendment) =====
    //
    // The frozen scheme pins the cross-unit symbol surface (docs/abi-v1.md §4).
    // A drift here is a deliberate ABI change and must turn this red.
    #[test]
    fn abi_v1_mangling_qualified_is_stable() {
        // Empty module path → the bare item, byte-for-byte: a single-file
        // program's symbols are UNCHANGED by the amendment (why it is an
        // amendment, not abi-v2). `main` / runtime names pass through here too.
        assert_eq!(mangle_qualified(&[], "double"), "double");
        assert_eq!(mangle_qualified(&[], "id__i64"), "id__i64");
        assert_eq!(mangle_qualified(&[], "main"), "main");

        // Non-empty → `_S` + <len><seg> per module segment + <len><item>.
        assert_eq!(
            mangle_qualified(&["util".to_string(), "math".to_string()], "add"),
            "_S4util4math3add"
        );
        // The item keeps its intra-module `__` scheme as ONE length-prefixed
        // blob: `Token__new` is 10 bytes → `10Token__new`.
        assert_eq!(
            mangle_qualified(&["lex".to_string(), "token".to_string()], "Token__new"),
            "_S3lex5token10Token__new"
        );
        // Distinct module prefixes never collide (the D7 requirement):
        // `parse::Token::new` vs `lex::token::Token::new` above.
        assert_eq!(
            mangle_qualified(&["parse".to_string()], "Token__new"),
            "_S5parse10Token__new"
        );
        // A mono instance under a module: the type-arg tag rides inside the
        // item blob (`id__i64` is 7 bytes → `7id__i64`).
        assert_eq!(
            mangle_qualified(&["util".to_string()], "id__i64"),
            "_S4util7id__i64"
        );
    }

    // ===== D.6 / ADR 0037 (2/N) point 8: linkonce_odr dedup type tags =====
    //
    // The `linkonce_odr` mono key for a generic over a CROSS-MODULE struct or
    // enum must module-qualify that type's tag so two same-named types from
    // different modules don't alias to one symbol. Pins `mangle_type_dedup` /
    // `mangle_mono_name_dedup` + the `mono_args_dedup_safe` gate.
    #[test]
    fn abi_v1_mangling_dedup_qualifies_cross_module_named_type_tags() {
        let prog = parse(
            "struct Point { x: i64, y: i64 } enum Shape { Circle(i64), Square(i64) } \
             fn main() -> i64 { 0 }",
        )
        .expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        let typed = check(&resolved).expect("check");

        let cross = NamedTypeOrigins {
            structs: HashMap::from([(StructId(0), vec!["util".to_string(), "geo".to_string()])]),
            enums: HashMap::from([(EnumId(0), vec!["shape".to_string()])]),
        };
        let empty = NamedTypeOrigins::default();

        // Cross-module struct / enum → origin-qualified tag (`$`-joined path +
        // name); a known origin distinguishes it from any same-named type.
        assert_eq!(mangle_type_dedup(Type::Struct(StructId(0)), &typed, &cross), "util$geo$Point");
        assert_eq!(mangle_type_dedup(Type::Enum(EnumId(0)), &typed, &cross), "shape$Shape");
        // No origin (local type) → the bare name, == `mangle_type`.
        assert_eq!(mangle_type_dedup(Type::Struct(StructId(0)), &typed, &empty), "Point");
        assert_eq!(mangle_type_dedup(Type::Enum(EnumId(0)), &typed, &empty), "Shape");
        // A primitive delegates to `mangle_type` (lock-step), origin maps irrelevant.
        assert_eq!(mangle_type_dedup(Type::I64, &typed, &cross), "i64");
        // Container wrapping a cross-module type recurses.
        assert_eq!(
            mangle_type_dedup(Type::Array(ArrayElem::Struct(StructId(0))), &typed, &cross),
            "arr_util$geo$Point"
        );
        // The mono key composes the qualified tag.
        assert_eq!(
            mangle_mono_name_dedup("id", &[Type::Struct(StructId(0))], &typed, &cross),
            "id__util$geo$Point"
        );
        assert_eq!(
            mangle_mono_name_dedup("id", &[Type::Enum(EnumId(0))], &typed, &cross),
            "id__shape$Shape"
        );

        // The dedup gate: a cross-module struct/enum (origin known) IS safe; a
        // local one (origin unknown) is NOT; primitives always are.
        assert!(mono_args_dedup_safe(&[Type::Struct(StructId(0))], &cross));
        assert!(mono_args_dedup_safe(&[Type::Enum(EnumId(0))], &cross));
        assert!(!mono_args_dedup_safe(&[Type::Struct(StructId(0))], &empty));
        assert!(!mono_args_dedup_safe(&[Type::Enum(EnumId(0))], &empty));
        assert!(mono_args_dedup_safe(&[Type::I64], &empty));
        assert!(mono_args_dedup_safe(&[Type::Array(ArrayElem::Struct(StructId(0)))], &cross));
    }

    // ===== C5 D7 (2/N) / ADR 0029: abi-v1 Type-layout stability =====
    //
    // For each `Type` constructor, lower it via the real `llvm_basic_type`
    // and assert its size / alignment / struct-field offsets through the
    // target `DataLayout` — the concrete byte layout docs/abi-v1.md §2
    // freezes. A reorder / repack / width change turns this red. Both 1.0
    // targets (x86-64, aarch64) are LP64 so the values are identical.
    #[test]
    fn abi_v1_type_layouts_via_datalayout() {
        use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};
        use inkwell::OptimizationLevel;
        use sentinel_types::{KontId, RefId, TaskId, VecElem};

        // Same target setup as `compile_to_object`.
        Target::initialize_native(&InitializationConfig::default()).expect("init native");
        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple).expect("target from triple");
        let tm = target
            .create_target_machine(
                &triple,
                &TargetMachine::get_host_cpu_name().to_string(),
                &TargetMachine::get_host_cpu_features().to_string(),
                OptimizationLevel::None,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .expect("target machine");
        let td = tm.get_target_data();

        let ctx = Context::create();
        let structs = HashMap::new();
        let gi = HashMap::new();
        let classes = HashMap::new();
        let secrets: Vec<SecretData> = Vec::new();
        let lower = |ty: Type| llvm_basic_type(&ctx, ty, &structs, &gi, &classes, &secrets);

        // Scalars (§2).
        assert_eq!(td.get_abi_size(&lower(Type::Bool).into_int_type()), 1);
        assert_eq!(td.get_abi_size(&lower(Type::I32).into_int_type()), 4);
        assert_eq!(td.get_abi_alignment(&lower(Type::I32).into_int_type()), 4);
        assert_eq!(td.get_abi_size(&lower(Type::I64).into_int_type()), 8);
        assert_eq!(td.get_abi_alignment(&lower(Type::I64).into_int_type()), 8);
        // D.2 / ADR 0033 D4: `u8` → LLVM `i8`, size 1, align 1, width 8.
        assert_eq!(lower(Type::U8).into_int_type().get_bit_width(), 8);
        assert_eq!(td.get_abi_size(&lower(Type::U8).into_int_type()), 1);
        assert_eq!(td.get_abi_alignment(&lower(Type::U8).into_int_type()), 1);

        // D.2 / ADR 0033 D3: a string IS a `[u8]` — same `{ i64 len, ptr
        // data }` 16-byte layout as any array (the element width lives in
        // the heap buffer, not the struct).
        let str_arr = lower(Type::Array(ArrayElem::U8)).into_struct_type();
        assert_eq!(str_arr.count_fields(), 2);
        assert_eq!(td.get_abi_size(&str_arr), 16);
        assert_eq!(td.get_abi_alignment(&str_arr), 8);
        assert_eq!(td.offset_of_element(&str_arr, 1), Some(8));

        // `[T]` = `{ i64 len, ptr data }`: 16 bytes, align 8, data @ 8.
        // Field types pin the *order* (len before data) — offsets alone
        // can't, since both fields are 8 bytes.
        let arr = lower(Type::Array(ArrayElem::I64)).into_struct_type();
        assert_eq!(arr.count_fields(), 2);
        assert!(!arr.is_packed());
        assert_eq!(td.get_abi_size(&arr), 16);
        assert_eq!(td.get_abi_alignment(&arr), 8);
        assert_eq!(td.offset_of_element(&arr, 0), Some(0));
        assert_eq!(td.offset_of_element(&arr, 1), Some(8));
        assert_eq!(arr.get_field_type_at_index(0).unwrap().into_int_type().get_bit_width(), 64);
        assert!(matches!(arr.get_field_type_at_index(1).unwrap(), BasicTypeEnum::PointerType(_)));

        // `?primitive` = `{ i1 valid, i64 }`: i64 lands @ 8 by alignment.
        let opt = lower(Type::Nullable(NullableInner::I64)).into_struct_type();
        assert_eq!(opt.count_fields(), 2);
        assert!(!opt.is_packed());
        assert_eq!(td.get_abi_size(&opt), 16);
        assert_eq!(td.offset_of_element(&opt, 0), Some(0));
        assert_eq!(td.offset_of_element(&opt, 1), Some(8));
        assert_eq!(opt.get_field_type_at_index(0).unwrap().into_int_type().get_bit_width(), 1);
        assert_eq!(opt.get_field_type_at_index(1).unwrap().into_int_type().get_bit_width(), 64);

        // `?Struct` = `{ i1 valid, ptr payload }` (payload is a heap ptr).
        let opt_s = lower(Type::Nullable(NullableInner::Struct(StructId(0)))).into_struct_type();
        assert_eq!(opt_s.count_fields(), 2);
        assert_eq!(td.get_abi_size(&opt_s), 16);
        assert_eq!(td.offset_of_element(&opt_s, 1), Some(8));
        assert_eq!(opt_s.get_field_type_at_index(0).unwrap().into_int_type().get_bit_width(), 1);
        assert!(matches!(opt_s.get_field_type_at_index(1).unwrap(), BasicTypeEnum::PointerType(_)));

        // `Enum` = `{ i32 tag, ptr payload }` (ADR 0032 D4): tag @ 0
        // (4 bytes), payload ptr @ 8 (4 bytes padding after the i32 tag),
        // 16 bytes total, align 8. Field types pin the order (i32 tag
        // before ptr payload). `llvm_basic_type` builds this without
        // consulting the enum table, so any EnumId lowers identically.
        let en = lower(Type::Enum(EnumId(0))).into_struct_type();
        assert_eq!(en.count_fields(), 2);
        assert!(!en.is_packed());
        assert_eq!(td.get_abi_size(&en), 16);
        assert_eq!(td.get_abi_alignment(&en), 8);
        assert_eq!(td.offset_of_element(&en, 0), Some(0));
        assert_eq!(td.offset_of_element(&en, 1), Some(8));
        assert_eq!(en.get_field_type_at_index(0).unwrap().into_int_type().get_bit_width(), 32);
        assert!(matches!(en.get_field_type_at_index(1).unwrap(), BasicTypeEnum::PointerType(_)));

        // `Vec<T>` = `{ i64 len, i64 cap, ptr data }` (ADR 0034 D3): the
        // `[T]` layout with a capacity field inserted, so 24 bytes, align
        // 8, and the data pointer is FIELD 2 @ offset 16 (not field 1 @ 8
        // like an array). The element width lives in the heap buffer, so
        // any element type lowers to the same struct. Field types pin the
        // order (i64 len, i64 cap, then ptr data).
        let vec = lower(Type::Vec(VecElem::I64)).into_struct_type();
        assert_eq!(vec.count_fields(), 3);
        assert!(!vec.is_packed());
        assert_eq!(td.get_abi_size(&vec), 24);
        assert_eq!(td.get_abi_alignment(&vec), 8);
        assert_eq!(td.offset_of_element(&vec, 0), Some(0));
        assert_eq!(td.offset_of_element(&vec, 1), Some(8));
        assert_eq!(td.offset_of_element(&vec, 2), Some(16));
        assert_eq!(vec.get_field_type_at_index(0).unwrap().into_int_type().get_bit_width(), 64);
        assert_eq!(vec.get_field_type_at_index(1).unwrap().into_int_type().get_bit_width(), 64);
        assert!(matches!(vec.get_field_type_at_index(2).unwrap(), BasicTypeEnum::PointerType(_)));
        // A `Vec<u8>` (the future `String`) lowers to the same struct —
        // element width is in the heap buffer, not the header.
        let vec_u8 = lower(Type::Vec(VecElem::U8)).into_struct_type();
        assert_eq!(vec_u8.count_fields(), 3);
        assert_eq!(td.get_abi_size(&vec_u8), 24);

        // `&T` / `Kont` / `Task` all lower to an opaque `ptr` (8 bytes).
        let r = lower(Type::Ref(RefId(0)));
        let k = lower(Type::Kont(KontId(0)));
        let t = lower(Type::Task(TaskId(0)));
        assert!(matches!(r, BasicTypeEnum::PointerType(_)));
        assert!(matches!(k, BasicTypeEnum::PointerType(_)));
        assert!(matches!(t, BasicTypeEnum::PointerType(_)));
        assert_eq!(td.get_abi_size(&r.into_pointer_type()), 8);
        assert_eq!(td.get_abi_alignment(&t.into_pointer_type()), 8);

        // Named struct: fields in declaration order, non-packed — a 2×i64
        // struct lays out at {0, 8} / 16 bytes, the shape codegen builds
        // for user structs via `context.struct_type(fields, false)`.
        let pair = ctx.struct_type(&[ctx.i64_type().into(), ctx.i64_type().into()], false);
        assert_eq!(td.get_abi_size(&pair), 16);
        assert_eq!(td.offset_of_element(&pair, 0), Some(0));
        assert_eq!(td.offset_of_element(&pair, 1), Some(8));
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
