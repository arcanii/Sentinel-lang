//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. Takes a [`ResolvedProgram`] (the
//! output of `sentinel-resolve::resolve`) and produces a native
//! object file. **Pass 1** declares every fn (including the runtime
//! `print` → `sentinel_print` mapping) so forward references work;
//! **pass 2** emits each fn body.
//!
//! `main` is required and is given an `i32` return type (the C ABI
//! `main` signature); other fns return `i64`. The exit code is the
//! i32-truncated i64 value of `main`'s tail expression. Programs
//! additionally produce stdout via `print(x)` calls.
//!
//! **C1.1.2** moved name resolution out of this crate into
//! `sentinel-resolve`. The 6 error variants
//! (`UndefinedVariable`, `RedeclaredVariable`, `UndefinedFunction`,
//! `ArityMismatch`, `RedefinedFunction`, `MissingMain`) all moved
//! with it; what remains here is pure LLVM lowering errors. Per
//! ADR 0011 D4, codegen now consumes a `ResolvedProgram` where
//! every name reference is a stable `VarId` / `FnId` — no more
//! string lookups in this crate.

use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::IntType;
use inkwell::values::{BasicMetadataValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{IntPredicate, OptimizationLevel};
use sentinel_ast::{BinOp, UnaryOp};
use sentinel_resolve::{
    FnId, ResolvedBlock, ResolvedExpr, ResolvedExprKind, ResolvedFnDef, ResolvedProgram,
    ResolvedStmt, ResolvedStmtKind, VarId,
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

/// Lower a [`ResolvedProgram`] to a native object file at `output`.
/// The emitted module exports `main` (i32-returning, the C ABI
/// entry); the runtime symbol `sentinel_print` is pre-declared in
/// pass 1 and called by `FnId(0)` references from resolved
/// programs.
pub fn compile_to_object(program: &ResolvedProgram, output: &Path) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();
    let i64_type = context.i64_type();

    // Pass 1: declare every function in the resolved program's
    // signature table. Signatures are indexed by FnId.0; FnId(0) is
    // always `print` (the runtime symbol) per sentinel-resolve's
    // pre-registration.
    let mut fns: HashMap<FnId, FunctionValue> = HashMap::new();
    for signature in &program.fn_signatures {
        let return_type = if signature.is_main { i32_type } else { i64_type };
        let param_types: Vec<_> =
            (0..signature.arity).map(|_| i64_type.into()).collect();
        let fn_type = return_type.fn_type(&param_types, false);
        // The runtime print symbol lives at "sentinel_print"; user
        // fns get their source name. is_runtime is the routing flag.
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
            i64_type,
            fns,
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

/// Per-function codegen state plus a shared function table. `'ctx`
/// is the LLVM IR lifetime (bound by Context); `'a` is the borrow
/// lifetime — short enough for the ctx to drop before
/// `module.verify()`. `current_fn` and `vars` are reset at the
/// start of each fn body via [`CodegenCtx::compile_fn`].
struct CodegenCtx<'ctx, 'a> {
    context: &'a Context,
    builder: Builder<'ctx>,
    i64_type: IntType<'ctx>,
    fns: HashMap<FnId, FunctionValue<'ctx>>,
    current_fn: Option<FunctionValue<'ctx>>,
    /// Per-function variable map. VarIds are program-global but
    /// only the IDs declared inside `current_fn` are populated
    /// here; the map clears at the start of each fn.
    vars: HashMap<VarId, PointerValue<'ctx>>,
}

impl<'ctx, 'a> CodegenCtx<'ctx, 'a> {
    fn compile_fn(
        &mut self,
        fn_def: &ResolvedFnDef,
        program: &ResolvedProgram,
    ) -> Result<(), CodegenError> {
        let fn_value = *self
            .fns
            .get(&fn_def.id)
            .expect("declared in pass 1");
        self.current_fn = Some(fn_value);
        self.vars.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        // Bind parameters: alloca + store the incoming value. Resolve
        // already caught duplicate-name params; we just allocate by
        // VarId here.
        for (i, param) in fn_def.params.iter().enumerate() {
            let arg = fn_value
                .get_nth_param(i as u32)
                .expect("param exists")
                .into_int_value();
            let alloca = self
                .builder
                .build_alloca(self.i64_type, &param.name)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.builder
                .build_store(alloca, arg)
                .map_err(|e| CodegenError::Builder(e.to_string()))?;
            self.vars.insert(param.id, alloca);
        }

        let body_val = self.lower_block(&fn_def.body, program)?;

        if fn_def.signature(program).is_main {
            let i32_type = self.context.i32_type();
            let exit = self
                .builder
                .build_int_truncate(body_val, i32_type, "exit")
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
        stmt: &ResolvedStmt,
        program: &ResolvedProgram,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            ResolvedStmtKind::Let { id, name, value, .. } => {
                let v = self.lower_expr(value, program)?;
                let alloca = self
                    .builder
                    .build_alloca(self.i64_type, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(alloca, v)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.vars.insert(*id, alloca);
            }
            ResolvedStmtKind::Expr(e) => {
                let _ = self.lower_expr(e, program)?;
            }
        }
        Ok(())
    }

    fn lower_block(
        &mut self,
        block: &ResolvedBlock,
        program: &ResolvedProgram,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        for stmt in &block.stmts {
            self.lower_stmt(stmt, program)?;
        }
        self.lower_expr(&block.tail, program)
    }

    fn lower_if(
        &mut self,
        cond: &ResolvedExpr,
        then_branch: &ResolvedBlock,
        else_branch: &ResolvedBlock,
        program: &ResolvedProgram,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let cond_val = self.lower_expr(cond, program)?;
        let zero = self.i64_type.const_zero();
        let cond_i1 = self
            .builder
            .build_int_compare(IntPredicate::NE, cond_val, zero, "ifcond")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        let current_fn = self.current_fn.expect("current_fn set by compile_fn");
        let then_bb = self.context.append_basic_block(current_fn, "then");
        let else_bb = self.context.append_basic_block(current_fn, "else");
        let merge_bb = self.context.append_basic_block(current_fn, "ifmerge");

        let result = self
            .builder
            .build_alloca(self.i64_type, "ifresult")
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
            .build_load(self.i64_type, result, "ifresult_val")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(loaded.into_int_value())
    }

    fn lower_call(
        &mut self,
        id: FnId,
        args: &[ResolvedExpr],
        program: &ResolvedProgram,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let fn_value = *self
            .fns
            .get(&id)
            .expect("FnId from a resolved program is always in the fn table");
        let signature = program.signature(id);
        let arg_values: Vec<BasicMetadataValueEnum> = args
            .iter()
            .map(|a| self.lower_expr(a, program).map(|v| v.into()))
            .collect::<Result<Vec<_>, _>>()?;
        let call = self
            .builder
            .build_call(fn_value, &arg_values, &format!("call_{}", signature.name))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        call.try_as_basic_value()
            .left()
            .map(|v| v.into_int_value())
            .ok_or_else(|| {
                CodegenError::Builder(format!(
                    "call to `{}` returned void unexpectedly",
                    signature.name
                ))
            })
    }

    fn lower_expr(
        &mut self,
        expr: &ResolvedExpr,
        program: &ResolvedProgram,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match &expr.kind {
            ResolvedExprKind::IntLit(n) => Ok(self.i64_type.const_int(*n as u64, true)),
            ResolvedExprKind::Var(id) => {
                let ptr = *self
                    .vars
                    .get(id)
                    .expect("VarId from a resolved program is always live in its scope");
                // Use the binding's source name as the LLVM SSA name
                // for readability; fall back to a synthetic name if
                // somehow the program tables disagree (shouldn't
                // happen).
                let name = self.var_name(*id, program).unwrap_or("load");
                let loaded = self
                    .builder
                    .build_load(self.i64_type, ptr, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(loaded.into_int_value())
            }
            ResolvedExprKind::Unary(UnaryOp::Neg, inner) => {
                let v = self.lower_expr(inner, program)?;
                self.builder
                    .build_int_neg(v, "neg")
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            ResolvedExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expr(lhs, program)?;
                let r = self.lower_expr(rhs, program)?;
                let result = match op {
                    BinOp::Add => self.builder.build_int_add(l, r, "add"),
                    BinOp::Sub => self.builder.build_int_sub(l, r, "sub"),
                    BinOp::Mul => self.builder.build_int_mul(l, r, "mul"),
                    BinOp::Div => self.builder.build_int_signed_div(l, r, "div"),
                };
                result.map_err(|e| CodegenError::Builder(e.to_string()))
            }
            ResolvedExprKind::Block(b) => self.lower_block(b, program),
            ResolvedExprKind::If { cond, then_branch, else_branch } => {
                self.lower_if(cond, then_branch, else_branch, program)
            }
            ResolvedExprKind::Call { id, args, .. } => self.lower_call(*id, args, program),
        }
    }

    /// Look up a binding's source name for use as an LLVM SSA debug
    /// name. Walks the current fn's params and then its body for a
    /// matching binding. This is purely for IR readability; returns
    /// None if no match (unreachable in practice for well-formed
    /// resolved programs).
    fn var_name<'p>(&self, id: VarId, program: &'p ResolvedProgram) -> Option<&'p str> {
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

fn find_var_name_in_block(block: &ResolvedBlock, id: VarId) -> Option<&str> {
    for stmt in &block.stmts {
        if let ResolvedStmtKind::Let { id: bid, name, value, .. } = &stmt.kind {
            if *bid == id {
                return Some(name);
            }
            if let Some(n) = find_var_name_in_expr(value, id) {
                return Some(n);
            }
        } else if let ResolvedStmtKind::Expr(e) = &stmt.kind {
            if let Some(n) = find_var_name_in_expr(e, id) {
                return Some(n);
            }
        }
    }
    find_var_name_in_expr(&block.tail, id)
}

fn find_var_name_in_expr(expr: &ResolvedExpr, id: VarId) -> Option<&str> {
    match &expr.kind {
        ResolvedExprKind::IntLit(_) | ResolvedExprKind::Var(_) => None,
        ResolvedExprKind::Unary(_, inner) => find_var_name_in_expr(inner, id),
        ResolvedExprKind::Binary(_, lhs, rhs) => {
            find_var_name_in_expr(lhs, id).or_else(|| find_var_name_in_expr(rhs, id))
        }
        ResolvedExprKind::Block(b) => find_var_name_in_block(b, id),
        ResolvedExprKind::If { cond, then_branch, else_branch } => {
            find_var_name_in_expr(cond, id)
                .or_else(|| find_var_name_in_block(then_branch, id))
                .or_else(|| find_var_name_in_block(else_branch, id))
        }
        ResolvedExprKind::Call { args, .. } => {
            args.iter().find_map(|a| find_var_name_in_expr(a, id))
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

    fn compile_src(src: &str) -> Result<(), CodegenError> {
        let prog = parse(src).expect("parse");
        let resolved = resolve(&prog).expect("resolve");
        compile_to_object(&resolved, &out_path())
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
        compile_src("fn main() { 42 }").expect("compile");
    }

    #[test]
    fn compile_main_with_let_program() {
        compile_src("fn main() { let x = 5; x }").expect("compile");
    }

    #[test]
    fn compile_multi_fn_with_forward_ref() {
        compile_src("fn main() { double(7) }\nfn double(x) { x * 2 }").expect("compile");
    }

    #[test]
    fn compile_call_to_print() {
        compile_src("fn main() { print(42) }").expect("compile");
    }

    #[test]
    fn compile_if_else_program() {
        compile_src("fn main() { if 1 { 42 } else { 99 } }").expect("compile");
    }

    #[test]
    fn compile_block_expression() {
        compile_src("fn main() { let r = { let y = 4; y + 1 }; r * 2 }").expect("compile");
    }
}
