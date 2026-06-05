//! Phase D self-host port (7/N) / ADR 0044 D2: the MIR differential oracle —
//! a canonical pretty-print of the `MirProgram` that `lower_to_mir` produces,
//! which the Sentinel-written MIR stage (`selfhost/mir.sentinel`, via
//! `types::run` mode 2) will reproduce byte-for-byte.
//!
//! One `(fn …)` per top-level user fn, in **FnId order** (= source order — the
//! Sentinel side assigns FnIds source-sequentially). Each fn is its return type
//! then its blocks (in id order); each block its SSA params, instructions, and
//! terminator. SSA values render `v<N>`; a value DEFINITION (a block param or an
//! instruction dest) carries its `Type` as `v<N>:<ty>` (rendered structurally by
//! `sentinel_types::type_display`, **no interner IDs**); a value USE is the bare
//! `v<N>`. Operators use `.symbol()` (the `snc ast` form). **NO raw spans** —
//! they are diagnostic-only (out of scope, ADR 0044 D7) and would make the dump
//! whitespace-fragile.
//!
//! `run_mir` runs parse → resolve → `check` → `lower_to_mir` and prints this.
//! Lowering is TOTAL (it never rejects — unmodelled forms become `Opaque` /
//! `Unreachable`), so the corpus differential skips only the fixtures the
//! upstream pipeline rejects (parse/resolve/type) — the happy-path discipline.
//! The constant-time VERIFIER is validated separately (ADR 0044 D6); this dump is
//! the lowered form regardless of any leak it would flag.

use sentinel_mir::{MirBlock, MirFunction, MirInst, MirOp, MirProgram, MirTerminator, MirValue};
use sentinel_types::{type_display, TypedFnDef, TypedProgram};

/// Canonical MIR dump of `mir` (lowered from `program`).
pub fn dump(mir: &MirProgram, program: &TypedProgram) -> String {
    let mut out = String::new();
    // `lower_to_mir` maps `program.fns` 1:1 onto `mir.functions` (same index);
    // pair them, then emit in FnId order — deterministic and matching the
    // Sentinel side (which walks fns in source order = FnId order).
    let mut paired: Vec<(&TypedFnDef, &MirFunction)> =
        program.fns.iter().zip(mir.functions.iter()).collect();
    paired.sort_by_key(|(f, _)| f.id.0);
    for (_, mf) in paired {
        dump_fn(mf, program, &mut out);
        out.push('\n');
    }
    out
}

fn dump_fn(f: &MirFunction, program: &TypedProgram, out: &mut String) {
    out.push_str("(fn ");
    out.push_str(&f.name);
    out.push(' ');
    out.push_str(&type_display(f.ret_ty, Some(program)));
    // Blocks in id order — a block's id is its index, so position is the id.
    for (id, block) in f.blocks.iter().enumerate() {
        out.push(' ');
        dump_block(f, id, block, program, out);
    }
    out.push(')');
}

fn dump_block(f: &MirFunction, id: usize, block: &MirBlock, program: &TypedProgram, out: &mut String) {
    out.push_str("(block b");
    out.push_str(&id.to_string());
    out.push_str(" (params");
    for &p in &block.params {
        out.push(' ');
        push_def(f, p, program, out);
    }
    out.push(')');
    for inst in &block.insts {
        out.push(' ');
        dump_inst(f, inst, program, out);
    }
    out.push(' ');
    dump_term(&block.term, out);
    out.push(')');
}

/// A value DEFINITION: `v<N>:<ty>` (a block param or an instruction dest).
fn push_def(f: &MirFunction, v: MirValue, program: &TypedProgram, out: &mut String) {
    out.push('v');
    out.push_str(&v.0.to_string());
    out.push(':');
    out.push_str(&type_display(f.value_ty(v), Some(program)));
}

/// A value USE: the bare `v<N>`.
fn push_use(v: MirValue, out: &mut String) {
    out.push('v');
    out.push_str(&v.0.to_string());
}

fn dump_inst(f: &MirFunction, inst: &MirInst, program: &TypedProgram, out: &mut String) {
    out.push('(');
    push_def(f, inst.dest, program, out);
    out.push(' ');
    match &inst.op {
        MirOp::ConstInt(n) => {
            out.push_str("const_int ");
            out.push_str(&n.to_string());
        }
        MirOp::ConstBool(b) => {
            out.push_str("const_bool ");
            out.push_str(if *b { "true" } else { "false" });
        }
        MirOp::Unary(op, x) => {
            out.push_str("unary ");
            out.push_str(op.symbol());
            out.push(' ');
            push_use(*x, out);
        }
        MirOp::Binary(op, l, r) => {
            out.push_str("binop ");
            out.push_str(op.symbol());
            out.push(' ');
            push_use(*l, out);
            out.push(' ');
            push_use(*r, out);
        }
        MirOp::Compare(op, l, r) => {
            out.push_str("cmp ");
            out.push_str(op.symbol());
            out.push(' ');
            push_use(*l, out);
            out.push(' ');
            push_use(*r, out);
        }
        MirOp::Declassify(x) => {
            out.push_str("declassify ");
            push_use(*x, out);
        }
        MirOp::Load { base, index } => {
            out.push_str("load ");
            push_use(*base, out);
            if let Some(idx) = index {
                out.push(' ');
                push_use(*idx, out);
            }
        }
        MirOp::Call { callee, args } => {
            out.push_str("call ");
            out.push_str(callee);
            for a in args {
                out.push(' ');
                push_use(*a, out);
            }
        }
        MirOp::Opaque(args) => {
            out.push_str("opaque");
            for a in args {
                out.push(' ');
                push_use(*a, out);
            }
        }
    }
    out.push(')');
}

fn dump_term(term: &MirTerminator, out: &mut String) {
    out.push_str("(term ");
    match term {
        MirTerminator::Jump { target, args } => {
            out.push_str("jump b");
            out.push_str(&target.0.to_string());
            for a in args {
                out.push(' ');
                push_use(*a, out);
            }
        }
        MirTerminator::Branch {
            cond,
            then_blk,
            then_args,
            else_blk,
            else_args,
            ..
        } => {
            out.push_str("branch ");
            push_use(*cond, out);
            out.push_str(" (b");
            out.push_str(&then_blk.0.to_string());
            for a in then_args {
                out.push(' ');
                push_use(*a, out);
            }
            out.push_str(") (b");
            out.push_str(&else_blk.0.to_string());
            for a in else_args {
                out.push(' ');
                push_use(*a, out);
            }
            out.push(')');
        }
        MirTerminator::Return(v) => {
            out.push_str("return");
            if let Some(v) = v {
                out.push(' ');
                push_use(*v, out);
            }
        }
        MirTerminator::Unreachable => out.push_str("unreachable"),
    }
    out.push(')');
}
