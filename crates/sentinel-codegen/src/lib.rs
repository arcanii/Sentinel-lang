//! sentinel-codegen
//!
//! LLVM IR lowering via inkwell. C0.5 takes a [`Program`] of
//! function definitions; pass 1 declares every fn (plus the
//! `print` -> `sentinel_print` runtime mapping), pass 2 emits each
//! fn body. Forward references between fns work because all
//! signatures land in pass 1 before any body is emitted.
//!
//! `main` is required and is given an `i32` return type (the C ABI
//! `main` signature); other fns return `i64`. The exit code is the
//! i32-truncated i64 value of `main`'s tail expression — the
//! "exit-code-is-the-answer" convention from C0.2 still holds.
//! Programs additionally produce stdout via `print(x)` calls.
//!
//! `print` is reserved: user-defined `fn print` collides with the
//! pre-declared runtime symbol and is caught as
//! `CodegenError::RedefinedFunction` in pass 1.
//!
//! Per ADR 0009 D7, sentinel-resolve is deferred to C1; until then
//! codegen continues to do name resolution as a side effect.
//! Variable scoping is flat per function (no shadowing); the vars
//! map is reset at the start of each fn body. C0.5 adds function-
//! name resolution to the same "absorbed into codegen" arrangement.

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
use sentinel_ast::{BinOp, Block, Expr, ExprKind, FnDef, Program, Span, Stmt, StmtKind, UnaryOp};

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
        help("define it with `fn {name}(…) {{ … }}` or use `print` for the runtime builtin")
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

    #[error("function `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::codegen::redefined_function),
        help("each fn name must be unique within a program; `print` is reserved by the runtime")
    )]
    RedefinedFunction {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    #[error("program has no `main` function")]
    #[diagnostic(
        code(sentinel::codegen::missing_main),
        help("add `fn main() {{ … }}` — it is the program entry point")
    )]
    MissingMain,
}

/// Lower a [`Program`] (C0.5 fn-defs) to a native object file at
/// `output`. The emitted module exports `main` (i32-returning, the
/// C ABI entry); the runtime symbol `sentinel_print` is pre-declared
/// in pass 1 and called as `print(x)` from Sentinel source.
pub fn compile_to_object(program: &Program, output: &Path) -> Result<(), CodegenError> {
    let context = Context::create();
    let module = context.create_module("sentinel");
    let builder = context.create_builder();

    let i32_type = context.i32_type();
    let i64_type = context.i64_type();

    // Pass 1: declare every function (including the runtime print).
    let mut fns: HashMap<String, FunctionValue> = HashMap::new();

    // `print(x: i64) -> i64` is the runtime symbol.
    let print_type = i64_type.fn_type(&[i64_type.into()], false);
    let print_fn = module.add_function("sentinel_print", print_type, None);
    fns.insert("print".to_string(), print_fn);

    let mut main_seen = false;
    for fn_def in &program.fns {
        if fns.contains_key(&fn_def.name) {
            return Err(CodegenError::RedefinedFunction {
                name: fn_def.name.clone(),
                span: to_source_span(&fn_def.name_span),
            });
        }
        let return_type = if fn_def.name == "main" {
            main_seen = true;
            i32_type
        } else {
            i64_type
        };
        let param_types: Vec<_> = fn_def.params.iter().map(|_| i64_type.into()).collect();
        let fn_type = return_type.fn_type(&param_types, false);
        let fn_value = module.add_function(&fn_def.name, fn_type, None);
        fns.insert(fn_def.name.clone(), fn_value);
    }

    if !main_seen {
        return Err(CodegenError::MissingMain);
    }

    // Pass 2: emit each function body.
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
            cx.compile_fn(fn_def)?;
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
/// `module.verify()`. The `module` itself is not stored in the
/// ctx because pass 1 declares all functions up-front so pass 2
/// never needs to mutate the module (it only emits IR via the
/// builder against pre-existing FunctionValues). `current_fn` and
/// `vars` are reset at the start of each fn body via
/// [`CodegenCtx::compile_fn`].
struct CodegenCtx<'ctx, 'a> {
    context: &'a Context,
    builder: Builder<'ctx>,
    i64_type: IntType<'ctx>,
    fns: HashMap<String, FunctionValue<'ctx>>,
    current_fn: Option<FunctionValue<'ctx>>,
    vars: HashMap<String, PointerValue<'ctx>>,
}

impl<'ctx, 'a> CodegenCtx<'ctx, 'a> {
    fn compile_fn(&mut self, fn_def: &FnDef) -> Result<(), CodegenError> {
        let fn_value = *self
            .fns
            .get(&fn_def.name)
            .expect("declared in pass 1");
        self.current_fn = Some(fn_value);
        self.vars.clear();

        let entry = self.context.append_basic_block(fn_value, "entry");
        self.builder.position_at_end(entry);

        // Bind parameters: alloca + store the incoming value.
        for (i, param) in fn_def.params.iter().enumerate() {
            if self.vars.contains_key(&param.name) {
                return Err(CodegenError::RedeclaredVariable {
                    name: param.name.clone(),
                    span: to_source_span(&param.span),
                });
            }
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
            self.vars.insert(param.name.clone(), alloca);
        }

        let body_val = self.lower_block(&fn_def.body)?;

        if fn_def.name == "main" {
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
        let then_val = self.lower_block(then_branch)?;
        self.builder
            .build_store(result, then_val)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| CodegenError::Builder(e.to_string()))?;

        self.builder.position_at_end(else_bb);
        let else_val = self.lower_block(else_branch)?;
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
        callee: &str,
        callee_span: &Span,
        args: &[Expr],
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let fn_value = *self
            .fns
            .get(callee)
            .ok_or_else(|| CodegenError::UndefinedFunction {
                name: callee.to_string(),
                span: to_source_span(callee_span),
            })?;
        let expected = fn_value.count_params() as usize;
        if args.len() != expected {
            return Err(CodegenError::ArityMismatch {
                name: callee.to_string(),
                expected,
                got: args.len(),
                span: to_source_span(callee_span),
            });
        }
        let arg_values: Vec<BasicMetadataValueEnum> = args
            .iter()
            .map(|a| self.lower_expr(a).map(|v| v.into()))
            .collect::<Result<Vec<_>, _>>()?;
        let call = self
            .builder
            .build_call(fn_value, &arg_values, &format!("call_{callee}"))
            .map_err(|e| CodegenError::Builder(e.to_string()))?;
        call.try_as_basic_value()
            .left()
            .map(|v| v.into_int_value())
            .ok_or_else(|| {
                CodegenError::Builder(format!("call to `{callee}` returned void unexpectedly"))
            })
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
    use sentinel_ast::{Block, Expr, ExprKind, FnDef, Param, Program, Spanned, Stmt, StmtKind};

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

    fn main_with_body(body: Block) -> FnDef {
        FnDef {
            name: "main".to_string(),
            name_span: 0..4,
            params: vec![],
            body,
            span: 0..50,
        }
    }

    fn main_returning(tail: Expr) -> FnDef {
        let body_span = tail.span.clone();
        let body = Block { stmts: vec![], tail, span: body_span };
        main_with_body(body)
    }

    fn single_fn_program(f: FnDef) -> Program {
        let span = f.span.clone();
        Program { fns: vec![f], span }
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
        let prog = single_fn_program(main_returning(lit(42, 0..2)));
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_main_with_let_program() {
        let body = Block {
            stmts: vec![let_stmt("x", 4..5, lit(5, 8..9), 0..10)],
            tail: var("x", 11..12),
            span: 0..12,
        };
        let prog = single_fn_program(main_with_body(body));
        compile_to_object(&prog, &out_path()).expect("compile");
    }

    #[test]
    fn compile_rejects_undefined_variable() {
        let prog = single_fn_program(main_returning(var("y", 0..1)));
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::UndefinedVariable { ref name, .. } if name == "y"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_redeclared_variable() {
        let body = Block {
            stmts: vec![
                let_stmt("x", 4..5, lit(1, 8..9), 0..10),
                let_stmt("x", 15..16, lit(2, 19..20), 11..21),
            ],
            tail: var("x", 22..23),
            span: 0..23,
        };
        let prog = single_fn_program(main_with_body(body));
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::RedeclaredVariable { ref name, .. } if name == "x"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_missing_main() {
        let f = FnDef {
            name: "foo".to_string(),
            name_span: 0..3,
            params: vec![],
            body: Block { stmts: vec![], tail: lit(0, 0..1), span: 0..1 },
            span: 0..20,
        };
        let prog = Program { fns: vec![f], span: 0..20 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(matches!(err, CodegenError::MissingMain), "got {err:?}");
    }

    #[test]
    fn compile_rejects_redefined_function() {
        let f1 = FnDef {
            name: "double".to_string(),
            name_span: 0..6,
            params: vec![],
            body: Block { stmts: vec![], tail: lit(0, 0..1), span: 0..1 },
            span: 0..20,
        };
        let f2 = FnDef {
            name: "double".to_string(),
            name_span: 30..36,
            params: vec![],
            body: Block { stmts: vec![], tail: lit(1, 0..1), span: 0..1 },
            span: 21..40,
        };
        let main = main_returning(lit(0, 0..1));
        let prog = Program { fns: vec![f1, f2, main], span: 0..50 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::RedefinedFunction { ref name, .. } if name == "double"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_user_redefining_print() {
        // `print` is pre-declared by the runtime; user fns can't take it.
        let user_print = FnDef {
            name: "print".to_string(),
            name_span: 0..5,
            params: vec![Param { name: "x".to_string(), span: 6..7 }],
            body: Block { stmts: vec![], tail: lit(0, 0..1), span: 0..1 },
            span: 0..20,
        };
        let main = main_returning(lit(0, 0..1));
        let prog = Program { fns: vec![user_print, main], span: 0..40 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::RedefinedFunction { ref name, .. } if name == "print"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_multi_fn_with_forward_ref() {
        // fn main() { double(7) }      <- forward-refs double
        // fn double(x) { x * 2 }
        let call_double = Spanned {
            kind: ExprKind::Call {
                callee: "double".to_string(),
                callee_span: 0..6,
                args: vec![lit(7, 0..1)],
            },
            span: 0..9,
        };
        let main = main_returning(call_double);
        let double = FnDef {
            name: "double".to_string(),
            name_span: 0..6,
            params: vec![Param { name: "x".to_string(), span: 0..1 }],
            body: Block {
                stmts: vec![],
                tail: Spanned {
                    kind: ExprKind::Binary(
                        BinOp::Mul,
                        Box::new(var("x", 0..1)),
                        Box::new(lit(2, 0..1)),
                    ),
                    span: 0..5,
                },
                span: 0..5,
            },
            span: 30..50,
        };
        let prog = Program { fns: vec![main, double], span: 0..50 };
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
        let prog = single_fn_program(main_returning(call));
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::UndefinedFunction { ref name, .. } if name == "frobnicate"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_rejects_arity_mismatch() {
        let call = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![lit(1, 6..7), lit(2, 9..10)],
            },
            span: 0..11,
        };
        let prog = single_fn_program(main_returning(call));
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "print"),
            "got {err:?}"
        );
    }

    #[test]
    fn compile_call_to_user_fn_arity_check() {
        // fn one_arg(x) { x }
        // fn main() { one_arg(1, 2) }   <- arity mismatch
        let one_arg = FnDef {
            name: "one_arg".to_string(),
            name_span: 0..7,
            params: vec![Param { name: "x".to_string(), span: 0..1 }],
            body: Block { stmts: vec![], tail: var("x", 0..1), span: 0..1 },
            span: 0..20,
        };
        let bad_call = Spanned {
            kind: ExprKind::Call {
                callee: "one_arg".to_string(),
                callee_span: 0..7,
                args: vec![lit(1, 0..1), lit(2, 0..1)],
            },
            span: 0..15,
        };
        let main = main_returning(bad_call);
        let prog = Program { fns: vec![one_arg, main], span: 0..50 };
        let err = compile_to_object(&prog, &out_path()).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "one_arg"),
            "got {err:?}"
        );
    }
}
