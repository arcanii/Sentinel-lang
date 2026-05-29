//! sentinel-mir — a minimal SSA/CFG mid-level IR.
//!
//! Per ADR 0009 §6.1 the pipeline is `types → hir → mir → codegen`.
//! ADR 0026 D2 stands MIR up as the SSA substrate for the C5.2 D5
//! **constant-time verification**: taint-propagate `secret` through the
//! def-use graph and reject any secret value that reaches a conditional
//! branch, a memory index, or a variable-latency operation.
//!
//! Under the C5.1 escape hatch (ADR 0026 D1/D3 amendment) codegen stays
//! on the typed program; MIR is therefore **analysis-only** at 1.0 — it
//! is lowered from the typed program purely to host that verification
//! (and a future bounds-check elision), not to feed codegen.
//!
//! ## Shape — minimal SSA via block parameters
//!
//! A [`MirFunction`] is a list of basic blocks. Each [`MirBlock`] has
//! SSA *parameters* (the phi-equivalent: predecessors pass matching args
//! on their terminators), a list of [`MirInst`]s each defining exactly
//! one SSA [`MirValue`], and a [`MirTerminator`]. Every value carries its
//! [`Type`], so secrecy is read straight off the type
//! (`Type::Secret(_)`): the D5 taint pass seeds from that, and an op's
//! result is secret iff any operand is — except [`MirOp::Declassify`],
//! the one sink that clears it.
//!
//! Only the *secret-relevant* shapes are modelled precisely (the three
//! D5 sinks — a branch condition, a load index, a division operand —
//! plus `declassify`); everything else funnels through
//! [`MirOp::Opaque`] / [`MirTerminator::Unreachable`], which still carry
//! their operands so taint flows through. This keeps the IR minimal
//! while remaining *sound* for the analysis (no secret can vanish).
//!
//! ## C5.1b (this increment): the data model only
//!
//! This increment defines the IR types + helpers and a hand-built smoke
//! test. The lowering from typed function bodies is C5.1b (2/N); the D5
//! verification pass over this IR is C5.2.

use sentinel_ast::{BinOp, CmpOp, UnaryOp};
use sentinel_types::Type;

/// An SSA value, identified by its index into [`MirFunction::value_tys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirValue(pub u32);

/// A basic block, identified by its index into [`MirFunction::blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirBlockId(pub u32);

/// A lowered program: one [`MirFunction`] per (monomorphic) function.
#[derive(Debug, Clone, Default)]
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
}

/// A function in SSA/CFG form.
#[derive(Debug, Clone)]
pub struct MirFunction {
    pub name: String,
    /// The type of every SSA value, indexed by [`MirValue`]. Block
    /// parameters and instruction results both allocate an entry here.
    pub value_tys: Vec<Type>,
    pub blocks: Vec<MirBlock>,
    /// The entry block; its parameters are the function parameters.
    pub entry: MirBlockId,
    pub ret_ty: Type,
}

impl MirFunction {
    /// The declared type of an SSA value.
    pub fn value_ty(&self, v: MirValue) -> Type {
        self.value_tys[v.0 as usize]
    }

    /// Whether a value is `secret`-typed — the seed for the D5 taint
    /// pass. (Taint then propagates through every op except
    /// [`MirOp::Declassify`].)
    pub fn is_secret(&self, v: MirValue) -> bool {
        matches!(self.value_ty(v), Type::Secret(_))
    }

    /// Borrow a block by id.
    pub fn block(&self, b: MirBlockId) -> &MirBlock {
        &self.blocks[b.0 as usize]
    }
}

/// A basic block: SSA parameters, straight-line instructions, terminator.
#[derive(Debug, Clone)]
pub struct MirBlock {
    /// SSA parameters (phi-equivalent): predecessors supply matching
    /// args on their [`MirTerminator`]. A parameter is `secret` iff any
    /// incoming arg is — the D5 taint pass resolves this by fixpoint.
    pub params: Vec<MirValue>,
    pub insts: Vec<MirInst>,
    pub term: MirTerminator,
}

/// An instruction: defines exactly one SSA value (`dest`) from `op`.
#[derive(Debug, Clone)]
pub struct MirInst {
    pub dest: MirValue,
    pub op: MirOp,
}

/// The value-producing operations.
#[derive(Debug, Clone)]
pub enum MirOp {
    ConstInt(i64),
    ConstBool(bool),
    Unary(UnaryOp, MirValue),
    /// Binary arithmetic. A `secret` operand to a variable-latency op
    /// (integer division / remainder) is a D5 leak.
    Binary(BinOp, MirValue, MirValue),
    Compare(CmpOp, MirValue, MirValue),
    /// `declassify(x)` — the only taint sink: the result is the
    /// non-secret view of `x`.
    Declassify(MirValue),
    /// A load from `base`, optionally at `index`. A `secret` `index` is
    /// a D5 leak (secret-dependent memory address).
    Load {
        base: MirValue,
        index: Option<MirValue>,
    },
    /// A call to a resolved (monomorphic) callee, named for the analysis.
    /// Operands carry taint in; the result's `Type` carries it out.
    Call {
        callee: String,
        args: Vec<MirValue>,
    },
    /// Any other value-producing construct (aggregates, field reads,
    /// widens, …) the minimal lowering doesn't model precisely. Its
    /// operands are listed so taint still flows through.
    Opaque(Vec<MirValue>),
}

/// A block terminator (the control-flow edges out of a block).
#[derive(Debug, Clone)]
pub enum MirTerminator {
    /// Unconditional jump, passing `args` to the target's block params.
    Jump {
        target: MirBlockId,
        args: Vec<MirValue>,
    },
    /// Conditional branch. A `secret` `cond` is a D5 leak (secret-
    /// dependent control flow).
    Branch {
        cond: MirValue,
        then_blk: MirBlockId,
        then_args: Vec<MirValue>,
        else_blk: MirBlockId,
        else_args: Vec<MirValue>,
    },
    /// Return an optional value.
    Return(Option<MirValue>),
    /// A terminator the minimal lowering doesn't model (an unlowered
    /// construct). A sink-free dead end for the analysis.
    Unreachable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_types::SecretId;

    /// Hand-build the SSA for `fn f(s: secret bool) -> i64 { if s { 1 }
    /// else { 0 } }` — shapes only — and confirm the data model
    /// round-trips and the secret seed the D5 pass keys off is readable.
    #[test]
    fn builds_a_function_with_a_secret_branch() {
        let secret = Type::Secret(SecretId(0)); // a secret bool, for the analysis
        let f = MirFunction {
            name: "f".to_string(),
            // v0 = secret param, v1 = const 1, v2 = const 0
            value_tys: vec![secret, Type::I64, Type::I64],
            entry: MirBlockId(0),
            ret_ty: Type::I64,
            blocks: vec![
                // entry(s): branch s ? blk1 : blk2
                MirBlock {
                    params: vec![MirValue(0)],
                    insts: vec![],
                    term: MirTerminator::Branch {
                        cond: MirValue(0),
                        then_blk: MirBlockId(1),
                        then_args: vec![],
                        else_blk: MirBlockId(2),
                        else_args: vec![],
                    },
                },
                // blk1: v1 = 1; return v1
                MirBlock {
                    params: vec![],
                    insts: vec![MirInst {
                        dest: MirValue(1),
                        op: MirOp::ConstInt(1),
                    }],
                    term: MirTerminator::Return(Some(MirValue(1))),
                },
                // blk2: v2 = 0; return v2
                MirBlock {
                    params: vec![],
                    insts: vec![MirInst {
                        dest: MirValue(2),
                        op: MirOp::ConstInt(0),
                    }],
                    term: MirTerminator::Return(Some(MirValue(2))),
                },
            ],
        };

        assert_eq!(f.blocks.len(), 3);
        assert!(f.is_secret(MirValue(0)), "the secret param is tainted");
        assert!(!f.is_secret(MirValue(1)), "an i64 constant is not secret");

        // The branch condition is the value the D5 pass will flag when
        // secret — here it is secret, so this function would be rejected.
        match &f.block(f.entry).term {
            MirTerminator::Branch { cond, .. } => {
                assert!(f.is_secret(*cond), "secret-conditional branch = a D5 leak");
            }
            _ => panic!("entry block should end in a branch"),
        }
    }
}
