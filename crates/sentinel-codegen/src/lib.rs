//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. C0.2 lowered single expressions;
//! C0.3 added let-bindings and variable references; C0.4 adds
//! `if`/`else`, block expressions, and function calls (the only
//! callable in C0.4 is `print`, which the runtime provides as
//! `sentinel_print`). The emitted module defines `main()` whose
//! body computes the program and returns the tail-expression value
//! truncated to i32 — the temporary exit-code-is-the-answer
//! convention. C0.4 now also produces stdout via `print(x)` calls
//! to the runtime.
//!
//! Variable storage uses LLVM `alloca` per ADR 0009 D1a. `if`
//! lowering uses **alloca-based result** (one stack slot for the
//! if-expression's value, stores from both branches, load at the
//! merge block) rather than LLVM `phi` nodes — the choice was
//! pinned at C0.4 start because alloca is simpler and -O0 doesn't
//! care; later sub-phases enabling optimization will get phi nodes
//! via mem2reg promotion of these allocas. C-style truthy
//! condition per ADR 0010 D9: condition is i64, compared `!= 0`.
//!
//! Per ADR 0009 D7, sentinel-resolve is deferred to C1; until then
//! codegen also does name resolution as a side effect — undefined
//! variables, redeclarations, undefined function names, and arity
//! mismatches all surface as [`CodegenError`] variants. C0.4
//! continues with flat per-function scoping: variables declared
//! inside `if` branches are visible across the whole function,
//! and the same name in both branches is a [`CodegenError::
//! RedeclaredVariable`] at codegen time even though only one
//! branch executes at runtime.

use std::collections::HashMap;
use std::path::Path;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::IntType;
use inkwell::values::{FunctionValue, IntValue, PointerValue};
use inkwell::{IntPredicate, OptimizationLevel};
use sentinel_ast::{BinOp, Block, Expr, ExprKind, Program, Span, Stmt, StmtKind, UnaryOp};

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

    #[error("undefined function `{name}`")]
    #[diagnostic(
        code(sentinel::codegen::undefined_function),
        help("the only callable in C0.4 is `print`; `fn` definitions arrive at C0.5")
    )]
    UndefinedFunction {
        name: String,
        #[label("no such function")]
        span: miette::SourceSpan,
    },

    #[error("`{name}` takes {expected} argument(s), got {got}")]
    #[diagnostic(code(sentinel::codegen::arity_mismatch))]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of arguments")]
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

    // Scope CodegenCtx so its borrow of `module` ends before
    // `module.verify()` and `target_machine.write_to_file(&module, ...)`
    // run. Inkwell's lifetimes can't see through the side-effects on
    // module that codegen performs via cx; the explicit scope makes
    // the end-of-borrow trivially obvious to the borrow checker.
    {
        let mut cx = CodegenCtx {
            context: &context,
            module: &module,
            builder,
            i64_type,
            _entry_block: entry,
            main_fn,
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

/// Borrowed codegen state. `'ctx` is the context/module/IR
/// lifetime; `'a` is the (shorter) borrow lifetime of the references
/// the ctx holds — short enough that the ctx is dropped before
/// `module.verify()` and `target_machine.write_to_file(&module, …)`
/// run in the outer scope.
struct CodegenCtx<'ctx, 'a> {
    context: &'a Context,
    module: &'a Module<'ctx>,
    builder: Builder<'ctx>,
    i64_type: IntType<'ctx>,
    _entry_block: BasicBlock<'ctx>,
    main_fn: FunctionValue<'ctx>,
    vars: HashMap<String, PointerValue<'ctx>>,
}

impl<'ctx, 'a> CodegenCtx<'ctx, 'a> {
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

    fn lower_block(&mut self, block: &Block) -> Result<IntValue<'ctx>, CodegenError> {
        for stmt in &block.stmts {
            self.lower_stmt(stmt)?;
        }
        self.lower_expr(&block.tail)
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Block,
        else_branch: &Block,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let cond_val = self.lower_expr(cond)?;
        let zero = self.i64_type.const_zero();
        let cond_i1 = self
            .builder
            .build_int_compare(IntPredicate::NE, cond_val, zero, "ifcond")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        let then_bb = self.context.append_basic_block(self.main_fn, "then");
        let else_bb = self.context.append_basic_block(self.main_fn, "else");
        let merge_bb = self.context.append_basic_block(self.main_fn, "ifmerge");

        // ADR 0009 D6 + C0.4 plan: alloca-based result. mem2reg
        // promotes this to phi nodes when optimization is enabled.
        let result = self
            .builder
            .build_alloca(self.i64_type, "ifresult")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder
            .build_conditional_branch(cond_i1, then_bb, else_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Then branch
        self.builder.position_at_end(then_bb);
        let then_val = self.lower_block(then_branch)?;
        self.builder
            .build_store(result, then_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Else branch
        self.builder.position_at_end(else_bb);
        let else_val = self.lower_block(else_branch)?;
        self.builder
            .build_store(result, else_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        // Merge: load the chosen branch's stored result
        self.builder.position_at_end(merge_bb);
        let loaded = self
            .builder
            .build_load(self.i64_type, result, "ifresult_val")
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        Ok(loaded.into_int_value())
    }

    fn lower_call(
        &mut self,
        callee: &str,
        callee_span: &Span,
        args: &[Expr],
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match callee {
            "print" => {
                if args.len() != 1 {
                    return Err(CodegenError::ArityMismatch {
                        name: "print".to_string(),
                        expected: 1,
                        got: args.len(),
                        span: to_source_span(callee_span),
                    });
                }
                let arg = self.lower_expr(&args[0])?;
                let fn_value = self.get_or_declare_sentinel_print();
                let call = self
                    .builder
                    .build_call(fn_value, &[arg.into()], "call_print")
                    .map_err(|e| CodegenError::Builder(e.to_string()))?;
                call.try_as_basic_value()
                    .left()
                    .map(|v| v.into_int_value())
                    .ok_or_else(|| {
                        CodegenError::Builder(
                            "sentinel_print returned void unexpectedly".to_string(),
                        )
                    })
            }
            _ => Err(CodegenError::UndefinedFunction {
                name: callee.to_string(),
                span: to_source_span(callee_span),
            }),
        }
    }

    fn get_or_declare_sentinel_print(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("sentinel_print") {
            return f;
        }
        let fn_type = self
            .i64_type
            .fn_type(&[self.i64_type.into()], false);
        self.module.add_function("sentinel_print", fn_type, None)
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
            ExprKind::Block(b) => self.lower_block(b),
            ExprKind::If { cond, then_branch, else_branch } => {
                self.lower_if(cond, then_branch, else_branch)
            }
            ExprKind::Call { callee, callee_span, args } => {
                self.lower_call(callee, callee_span, args)
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
    use sentinel_ast::{Block, Expr, ExprKind, Program, Spanned, Stmt, StmtKind};

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

    fn block_lit(n: i64, span: Span) -> Block {
        Block { stmts: vec![], tail: lit(n, span.clone()), span }
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
        let prog = Program { stmts: vec![], tail: lit(42, 0..2), span: 0..2 };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_let_program() {
        let prog = Program {
            stmts: vec![let_stmt("x", 4..5, lit(5, 8..9), 0..10)],
            tail: var("x", 11..12),
            span: 0..12,
        };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_rejects_undefined_variable() {
        let prog = Program { stmts: vec![], tail: var("y", 0..1), span: 0..1 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::UndefinedVariable { ref name, .. } if name == "y"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_redeclared_variable() {
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

    #[test]
    fn compile_if_expression() {
        // tail = if 1 { 7 } else { 8 }
        let if_expr = Spanned {
            kind: ExprKind::If {
                cond: Box::new(lit(1, 3..4)),
                then_branch: Box::new(block_lit(7, 5..8)),
                else_branch: Box::new(block_lit(8, 14..17)),
            },
            span: 0..18,
        };
        let prog = Program { stmts: vec![], tail: if_expr, span: 0..18 };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_block_expression() {
        // tail = { 99 }
        let blk = Spanned {
            kind: ExprKind::Block(Box::new(block_lit(99, 1..3))),
            span: 0..4,
        };
        let prog = Program { stmts: vec![], tail: blk, span: 0..4 };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_call_to_print() {
        // tail = print(5)
        let call = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![lit(5, 6..7)],
            },
            span: 0..8,
        };
        let prog = Program { stmts: vec![], tail: call, span: 0..8 };
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_rejects_undefined_function() {
        let call = Spanned {
            kind: ExprKind::Call {
                callee: "frobnicate".to_string(),
                callee_span: 0..10,
                args: vec![lit(5, 11..12)],
            },
            span: 0..13,
        };
        let prog = Program { stmts: vec![], tail: call, span: 0..13 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::UndefinedFunction { ref name, .. } if name == "frobnicate"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_arity_mismatch_too_many() {
        let call = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![lit(1, 6..7), lit(2, 9..10)],
            },
            span: 0..11,
        };
        let prog = Program { stmts: vec![], tail: call, span: 0..11 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "print"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_arity_mismatch_too_few() {
        let call = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![],
            },
            span: 0..7,
        };
        let prog = Program { stmts: vec![], tail: call, span: 0..7 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::ArityMismatch { ref name, expected: 1, got: 0, .. } if name == "print"),
            "got {err:?}"
        );
    }
}
