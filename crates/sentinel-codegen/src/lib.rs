//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. C0.2 lowered single expressions;
//! C0.3 lowers full [`Program`]s (let-bindings + variables + tail
//! expression). The emitted module defines a single `main()` whose
//! body computes the program and returns the tail-expression value
//! truncated to i32 — the temporary exit-code-is-the-answer
//! convention; ADR 0010 D11 `print(x)` replaces this at C0.4.
//!
//! Variable storage uses LLVM `alloca` for each `let` binding with
//! `load` / `store` for reads and writes. Per ADR 0009 D7,
//! sentinel-resolve is deferred to C1; until then codegen also
//! does name resolution as a side effect — undefined references
//! and redeclarations both surface as [`CodegenError`] variants.
//! C0.3 uses flat per-function scoping (no shadowing) per the
//! agreed scope at the start of the sub-phase.

use std::collections::HashMap;
use std::path::Path;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::IntType;
use inkwell::values::{FunctionValue, IntValue, PointerValue};
use inkwell::OptimizationLevel;
use sentinel_ast::{BinOp, Expr, ExprKind, Program, Span, Stmt, StmtKind, UnaryOp};

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

    #[error("undefined variable `{name}`")]
    #[diagnostic(
        code(sentinel::codegen::undefined_variable),
        help("declare it with `let {name} = ...;` before this reference")
    )]
    UndefinedVariable {
        name: String,
        #[label("not declared in this scope")]
        span: miette::SourceSpan,
    },

    #[error("variable `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::codegen::redeclared_variable),
        help("C0 scoping is flat per function; pick a different name or remove the earlier binding")
    )]
    RedeclaredVariable {
        name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },
}

/// Lower a [`Program`] to a native object file at `output`. The
/// emitted module defines `main() -> i32` returning the truncated
/// i64 value of the program's trailing expression.
pub fn compile_to_object(program: &Program, output: &Path) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();
    let i64_type = context.i64_type();
    let main_fn_type = i32_type.fn_type(&[], false);
    let main_fn = module.add_function("main", main_fn_type, None);
    let entry = context.append_basic_block(main_fn, "entry");
    builder.position_at_end(entry);

    let mut cx = CodegenCtx {
        builder,
        i64_type,
        _entry_block: entry,
        _main_fn: main_fn,
        vars: HashMap::new(),
    };

    for stmt in &program.stmts {
        cx.lower_stmt(stmt)?;
    }
    let tail = cx.lower_expr(&program.tail)?;

    let exit = cx
        .builder
        .build_int_truncate(tail, i32_type, "exit")
        .map_err(|e| CodegenError::Builder(e.to_string()))?;
    cx.builder
        .build_return(Some(&exit))
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

/// Per-program codegen state. `_entry_block` and `_main_fn` are
/// retained behind `_` prefixes because C0.4 will need them for
/// `if`/`else` block creation; dropping them now would force a
/// re-plumbing of the struct in the next sub-phase.
struct CodegenCtx<'ctx> {
    builder: Builder<'ctx>,
    i64_type: IntType<'ctx>,
    _entry_block: BasicBlock<'ctx>,
    _main_fn: FunctionValue<'ctx>,
    vars: HashMap<String, PointerValue<'ctx>>,
}

impl<'ctx> CodegenCtx<'ctx> {
    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), CodegenError> {
        match &stmt.kind {
            StmtKind::Let { name, name_span, value } => {
                if self.vars.contains_key(name) {
                    return Err(CodegenError::RedeclaredVariable {
                        name: name.clone(),
                        span: to_source_span(name_span),
                    });
                }
                let v = self.lower_expr(value)?;
                let alloca = self
                    .builder
                    .build_alloca(self.i64_type, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.builder
                    .build_store(alloca, v)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                self.vars.insert(name.clone(), alloca);
            }
            StmtKind::Expr(e) => {
                let _ = self.lower_expr(e)?;
            }
        }
        Ok(())
    }

    fn lower_expr(&mut self, expr: &Expr) -> Result<IntValue<'ctx>, CodegenError> {
        match &expr.kind {
            ExprKind::IntLit(n) => Ok(self.i64_type.const_int(*n as u64, true)),
            ExprKind::Var(name) => {
                let ptr = self
                    .vars
                    .get(name)
                    .copied()
                    .ok_or_else(|| CodegenError::UndefinedVariable {
                        name: name.clone(),
                        span: to_source_span(&expr.span),
                    })?;
                let loaded = self
                    .builder
                    .build_load(self.i64_type, ptr, name)
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                Ok(loaded.into_int_value())
            }
            ExprKind::Unary(UnaryOp::Neg, inner) => {
                let v = self.lower_expr(inner)?;
                self.builder
                    .build_int_neg(v, "neg")
                    .map_err(|e| CodegenError::Builder(e.to_string()))
            }
            ExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let result = match op {
                    BinOp::Add => self.builder.build_int_add(l, r, "add"),
                    BinOp::Sub => self.builder.build_int_sub(l, r, "sub"),
                    BinOp::Mul => self.builder.build_int_mul(l, r, "mul"),
                    BinOp::Div => self.builder.build_int_signed_div(l, r, "div"),
                };
                result.map_err(|e| CodegenError::Builder(e.to_string()))
            }
        }
    }
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-codegen"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_ast::{Expr, ExprKind, Program, Spanned, Stmt, StmtKind};

    fn lit(n: i64, span: Span) -> Expr {
        Spanned { kind: ExprKind::IntLit(n), span }
    }

    fn var(name: &str, span: Span) -> Expr {
        Spanned { kind: ExprKind::Var(name.to_string()), span }
    }

    fn let_stmt(name: &str, name_span: Span, value: Expr, span: Span) -> Stmt {
        Spanned {
            kind: StmtKind::Let { name: name.to_string(), name_span, value },
            span,
        }
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
    fn compile_empty_stmts_program() {
        // The C0.2 case still works: pure expression program.
        let prog = Program { stmts: vec![], tail: lit(42, 0..2), span: 0..2 };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_let_program() {
        // `let x = 5; x`
        let prog = Program {
            stmts: vec![let_stmt("x", 4..5, lit(5, 8..9), 0..10)],
            tail: var("x", 11..12),
            span: 0..12,
        };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_rejects_undefined_variable() {
        // `y` is referenced but never bound.
        let prog = Program {
            stmts: vec![],
            tail: var("y", 0..1),
            span: 0..1,
        };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::UndefinedVariable { ref name, .. } if name == "y"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_redeclared_variable() {
        // `let x = 1; let x = 2; x` — flat per-function scoping rejects.
        let prog = Program {
            stmts: vec![
                let_stmt("x", 4..5, lit(1, 8..9), 0..10),
                let_stmt("x", 15..16, lit(2, 19..20), 11..21),
            ],
            tail: var("x", 22..23),
            span: 0..23,
        };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::RedeclaredVariable { ref name, .. } if name == "x"),
            "got {err:?}"
        );
    }
}
