//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. C0.2 lowers the C0.1 expression AST
//! into an LLVM object file. The emitted module defines a single
//! `main()` entry point whose body computes the expression and
//! returns the i64 result truncated to i32 — the program's exit
//! code. This is the temporary "exit-code-is-the-answer"
//! convention; C0.4 will replace it with `print(x)` once function
//! calls land.
//!
//! The driver invokes [`compile_to_object`] to produce a `.o`, then
//! calls the system `cc` to link it into an executable. Linking
//! lives in the driver because it is platform glue, not a compiler
//! responsibility.

use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::values::IntValue;
use inkwell::OptimizationLevel;
use sentinel_ast::{BinOp, Expr, ExprKind, UnaryOp};

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

/// Lower a single C0.1 expression to an LLVM object file at `output`.
///
/// The resulting object exposes a `main()` function whose return
/// value (i32) is the program's exit code; the expression is
/// computed as i64 and truncated.
pub fn compile_to_object(expr: &Expr, output: &Path) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();
    let main_fn_type = i32_type.fn_type(&[], false);
    let main_fn = module.add_function("main", main_fn_type, None);
    let entry = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry);

    let i64_value = lower_expr(expr, &context, &builder)?;
    let i32_value = builder
        .build_int_truncate(i64_value, i32_type, "exit")
        .map_err(|e| CodegenError::Builder(e.to_string()))?;
    builder
        .build_return(Some(&i32_value))
        .map_err(|e| CodegenError::Builder(e.to_string()))?;

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

fn lower_expr<'ctx>(
    expr: &Expr,
    ctx: &'ctx Context,
    builder: &Builder<'ctx>,
) -> Result<IntValue<'ctx>, CodegenError> {
    match &expr.kind {
        ExprKind::IntLit(n) => Ok(ctx.i64_type().const_int(*n as u64, true)),
        ExprKind::Unary(UnaryOp::Neg, inner) => {
            let v = lower_expr(inner, ctx, builder)?;
            builder
                .build_int_neg(v, "neg")
                .map_err(|e| CodegenError::Builder(e.to_string()))
        }
        ExprKind::Binary(op, lhs, rhs) => {
            let l = lower_expr(lhs, ctx, builder)?;
            let r = lower_expr(rhs, ctx, builder)?;
            let result = match op {
                BinOp::Add => builder.build_int_add(l, r, "add"),
                BinOp::Sub => builder.build_int_sub(l, r, "sub"),
                BinOp::Mul => builder.build_int_mul(l, r, "mul"),
                BinOp::Div => builder.build_int_signed_div(l, r, "div"),
            };
            result.map_err(|e| CodegenError::Builder(e.to_string()))
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

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-codegen");
    }

    #[test]
    fn target_init_does_not_panic() {
        // Initializing the native target twice is harmless; this
        // is just a probe that linking against llvm-sys works at all.
        Target::initialize_native(&InitializationConfig::default()).expect("initialize native");
    }
}
