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

use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue,
};
use inkwell::{IntPredicate, OptimizationLevel};
use sentinel_ast::{BinOp, CmpOp, LogicOp, UnaryOp};
use sentinel_resolve::{FnId, StructId, VarId};
use sentinel_types::{
    Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedProgram, TypedStmt, TypedStmtKind,
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
    let mut struct_types: HashMap<StructId, StructType> = HashMap::new();
    for sd in &program.structs {
        let field_tys: Vec<BasicTypeEnum> = sd
            .fields
            .iter()
            .map(|f| llvm_basic_type(&context, f.ty, &struct_types))
            .collect();
        let st = context.opaque_struct_type(&sd.name);
        st.set_body(&field_tys, false);
        struct_types.insert(sd.id, st);
    }

    // Pass 1: declare every function in the typed program's
    // signature table. Signatures are indexed by FnId.0; FnId(0) is
    // always `print` (the runtime symbol).
    let mut fns: HashMap<FnId, FunctionValue> = HashMap::new();
    for signature in &program.fn_signatures {
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
        let llvm_name = if signature.is_runtime {
            "sentinel_print"
        } else {
            &signature.name
        };
        let fn_value = module.add_function(llvm_name, fn_type, None);
        fns.insert(signature.id, fn_value);
    }

    // Pass 2: emit each user function body. (The runtime `print`
    // has no body — it's defined externally by sentinel-runtime.)
    {
        let mut cx = CodegenCtx {
            context: &context,
            builder,
            fns,
            struct_types,
            current_fn: None,
            vars: HashMap::new(),
        };
        for fn_def in &program.fns {
            cx.compile_fn(fn_def, program)?;
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
    struct_types: HashMap<StructId, StructType<'ctx>>,
    current_fn: Option<FunctionValue<'ctx>>,
    vars: HashMap<VarId, (PointerValue<'ctx>, Type)>,
}

/// Map a Sentinel [`Type`] to its LLVM `BasicTypeEnum`. At C1.4
/// the universe is `{ I64, I32, Bool, Struct }`; primitives map to
/// IntType, structs map to the per-struct StructType cached in
/// pass 0.
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
    }
}

/// Map a Sentinel int type (i1 / i32 / i64) to its LLVM IntType.
/// Panics on `Type::Struct` — callers must gate on `Type::is_int()`
/// first.
fn llvm_int_type<'ctx>(context: &'ctx Context, ty: Type) -> IntType<'ctx> {
    match ty {
        Type::Bool => context.bool_type(),
        Type::I32 => context.i32_type(),
        Type::I64 => context.i64_type(),
        Type::Struct(_) => panic!("llvm_int_type called on non-int Type::Struct"),
    }
}

impl<'ctx> CodegenCtx<'ctx> {
    fn llvm_basic_type(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        llvm_basic_type(self.context, ty, &self.struct_types)
    }

    fn llvm_int_type(&self, ty: Type) -> IntType<'ctx> {
        llvm_int_type(self.context, ty)
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
                let l = self.lower_expr(lhs, program)?.into_int_value();
                let r = self.lower_expr(rhs, program)?.into_int_value();
                let predicate = match op {
                    CmpOp::Eq => IntPredicate::EQ,
                    CmpOp::Ne => IntPredicate::NE,
                    CmpOp::Lt => IntPredicate::SLT,
                    CmpOp::Le => IntPredicate::SLE,
                    CmpOp::Gt => IntPredicate::SGT,
                    CmpOp::Ge => IntPredicate::SGE,
                };
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
            TypedExprKind::Call { id, args, .. } => self.lower_call(*id, args, program),
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
        TypedExprKind::IntLit(_) | TypedExprKind::BoolLit(_) | TypedExprKind::Var(_) => None,
        TypedExprKind::Unary(_, inner) => find_var_name_in_expr(inner, id),
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
