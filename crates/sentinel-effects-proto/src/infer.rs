//! Hindley-Milner inference for Sentinel-Mini.
//!
//! B1.4 shipped the primitives (substitutions, unification,
//! instantiation, generalisation). B1.5a adds the [`infer`] driver
//! that walks the AST. B1.5b will wire [`infer_top`] into
//! [`crate::run`].
//!
//! # `let rec` (B1.5 stub)
//!
//! Currently inferred at a fresh monovar - sufficient for monomorphic
//! recursion (factorial, countdown), insufficient for polymorphic
//! recursion. B1.6 will refine.

use crate::ast::{BinOp, Expr, ExprKind};
use crate::span::Span;
use crate::types::{Row, RowVar, Scheme, Ty, TyVar};
use std::collections::{BTreeSet, HashMap};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypeError {
    #[error("type mismatch: expected {expected}, found {found}")]
    Mismatch { expected: Ty, found: Ty, span: Span },
    #[error("occurs check failed: {var} appears in {ty}")]
    OccursCheck { var: TyVar, ty: Ty, span: Span },
    #[error("unbound variable: {name}")]
    Unbound { name: String, span: Span },
    #[error("row mismatch: expected {expected}, found {found}")]
    RowMismatch { expected: Row, found: Row, span: Span },
    #[error("row occurs check failed: {var} appears in {row}")]
    RowOccursCheck { var: RowVar, row: Row, span: Span },
}

#[derive(Debug, Default, Clone)]
pub struct TyVarSupply {
    next: u32,
    /// B2.0: parallel counter for row variables. Kept separate from
    /// `next` so a TyVar and a RowVar with the same numeric id are
    /// not confusable when stepping through debugger output.
    next_row: u32,
}

impl TyVarSupply {
    pub fn new() -> Self { Self::default() }
    pub fn fresh(&mut self) -> TyVar {
        let v = TyVar(self.next);
        self.next = self.next.checked_add(1).expect("TyVarSupply exhausted");
        v
    }
    pub fn fresh_ty(&mut self) -> Ty { Ty::Var(self.fresh()) }

    /// B2.0: defined for symmetry with `fresh`. Not yet called by
    /// `infer`; B2.3 wires it into lambda introduction.
    pub fn fresh_row_var(&mut self) -> RowVar {
        let v = RowVar(self.next_row);
        self.next_row = self.next_row.checked_add(1).expect("TyVarSupply row exhausted");
        v
    }

    pub fn fresh_row(&mut self) -> Row { Row::Var(self.fresh_row_var()) }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subst {
    map: HashMap<TyVar, Ty>,
    /// B2.0: parallel row-var substitution. Empty in practice until B2.1.
    row_map: HashMap<RowVar, Row>,
}

impl Subst {
    pub fn empty() -> Self { Self::default() }
    pub fn singleton(v: TyVar, t: Ty) -> Self {
        let mut map = HashMap::new();
        map.insert(v, t);
        Subst { map, row_map: HashMap::new() }
    }

    /// B2.0: row-singleton. Not yet exercised by inference; defined so
    /// B2.1's `unify_row` can `bind` a row var without adding API later.
    pub fn singleton_row(v: RowVar, r: Row) -> Self {
        let mut row_map = HashMap::new();
        row_map.insert(v, r);
        Subst { map: HashMap::new(), row_map }
    }
    pub fn is_empty(&self) -> bool { self.map.is_empty() && self.row_map.is_empty() }
    pub fn len(&self) -> usize { self.map.len() + self.row_map.len() }
    pub fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Int => Ty::Int,
            Ty::Bool => Ty::Bool,
            Ty::Var(v) => match self.map.get(v) {
                Some(bound) => self.apply(bound),
                None => Ty::Var(*v),
            },
            Ty::Fun(a, row, b) => Ty::Fun(
                Box::new(self.apply(a)),
                self.apply_row(row),
                Box::new(self.apply(b)),
            ),
        }
    }
    /// Apply this substitution to a row.
    ///
    /// B2.0: only `Row::Empty` is ever produced by the inferer, so this
    /// is exercised only by the empty-row arm. The other arms are
    /// defined so B2.1's unifier and B2.3's effect inference can reuse
    /// the same `apply` infrastructure.
    pub fn apply_row(&self, row: &Row) -> Row {
        match row {
            Row::Empty => Row::Empty,
            Row::Var(v) => match self.row_map.get(v) {
                Some(bound) => self.apply_row(bound),
                None => Row::Var(*v),
            },
            Row::Cons { label, arg, ret, tail } => Row::Cons {
                label: label.clone(),
                arg: Box::new(self.apply(arg)),
                ret: Box::new(self.apply(ret)),
                tail: Box::new(self.apply_row(tail)),
            },
        }
    }

    pub fn apply_scheme(&self, s: &Scheme) -> Scheme {
        let mut filtered = self.map.clone();
        for v in &s.vars { filtered.remove(v); }
        // B2.0: Scheme does not quantify row vars yet, so we keep
        // the full row_map. B2.3 may revisit when generalisation
        // grows row quantifiers.
        let tmp = Subst { map: filtered, row_map: self.row_map.clone() };
        Scheme { vars: s.vars.clone(), ty: tmp.apply(&s.ty) }
    }
    pub fn compose(&self, s1: &Subst) -> Subst {
        let mut composed: HashMap<TyVar, Ty> =
            s1.map.iter().map(|(v, t)| (*v, self.apply(t))).collect();
        for (v, t) in &self.map {
            composed.entry(*v).or_insert_with(|| t.clone());
        }
        let mut composed_rows: HashMap<RowVar, Row> =
            s1.row_map.iter().map(|(v, r)| (*v, self.apply_row(r))).collect();
        for (v, r) in &self.row_map {
            composed_rows.entry(*v).or_insert_with(|| r.clone());
        }
        Subst { map: composed, row_map: composed_rows }
    }
}

fn occurs(v: TyVar, t: &Ty, s: &Subst) -> bool {
    s.apply(t).free_vars().contains(&v)
}

fn bind(v: TyVar, t: &Ty, span: Span, s: &Subst) -> Result<Subst, TypeError> {
    if let Ty::Var(v2) = t {
        if *v2 == v { return Ok(Subst::empty()); }
    }
    let t_resolved = s.apply(t);
    if occurs(v, &t_resolved, &Subst::empty()) {
        return Err(TypeError::OccursCheck { var: v, ty: t_resolved, span });
    }
    Ok(Subst::singleton(v, t_resolved))
}

pub fn unify(
    s: &Subst,
    a: &Ty,
    b: &Ty,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<Subst, TypeError> {
    let a = s.apply(a);
    let b = s.apply(b);
    match (a, b) {
        (Ty::Int, Ty::Int) => Ok(s.clone()),
        (Ty::Bool, Ty::Bool) => Ok(s.clone()),
        (Ty::Var(v), t) | (t, Ty::Var(v)) => {
            let _ = supply;
            let extension = bind(v, &t, span, s)?;
            Ok(extension.compose(s))
        }
        (Ty::Fun(a1, r1, b1), Ty::Fun(a2, r2, b2)) => {
            let s1 = unify(s, &a1, &a2, span, supply)?;
            let s2 = unify(&s1, &b1, &b2, span, supply)?;
            unify_row(&s2, &r1, &r2, span, supply)
        }
        (expected, found) => Err(TypeError::Mismatch { expected, found, span }),
    }
}

/// Unify two effect rows.
///
/// B2.1: full Remy/Leijen rewriting algorithm.
///
/// The interesting case is `(Cons l1 .., Cons l2 ..)` with `l1 != l2`:
/// we search for `l1` inside the second row's tail (via `rewrite_row`),
/// unify the signature, and unify what's left of each side. The search
/// can extend a trailing row variable by minting a fresh row var, which
/// is why this function needs the supply.
pub fn unify_row(
    s: &Subst,
    a: &Row,
    b: &Row,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<Subst, TypeError> {
    let a = s.apply_row(a);
    let b = s.apply_row(b);
    match (a, b) {
        (Row::Empty, Row::Empty) => Ok(s.clone()),

        (Row::Var(v), other) | (other, Row::Var(v)) => {
            if let Row::Var(v2) = &other {
                if *v2 == v {
                    return Ok(s.clone());
                }
            }
            if row_occurs(v, &other, s) {
                return Err(TypeError::RowOccursCheck { var: v, row: other, span });
            }
            let ext = Subst::singleton_row(v, other);
            Ok(ext.compose(s))
        }

        (Row::Cons { label: l1, arg: a1, ret: r1, tail: t1 },
         Row::Cons { label: l2, arg: a2, ret: r2, tail: t2 }) => {
            if l1 == l2 {
                let s1 = unify(s, &a1, &a2, span, supply)?;
                let s2 = unify(&s1, &r1, &r2, span, supply)?;
                unify_row(&s2, &t1, &t2, span, supply)
            } else {
                let rhs_full = Row::Cons {
                    label: l2,
                    arg: a2,
                    ret: r2,
                    tail: t2,
                };
                let (rhs_remaining, s1) =
                    rewrite_row(&l1, &a1, &r1, &rhs_full, span, s, supply)?;
                unify_row(&s1, &t1, &rhs_remaining, span, supply)
            }
        }

        (a, b) => Err(TypeError::RowMismatch { expected: a, found: b, span }),
    }
}

fn rewrite_row(
    label: &str,
    arg: &Ty,
    ret: &Ty,
    row: &Row,
    span: Span,
    s: &Subst,
    supply: &mut TyVarSupply,
) -> Result<(Row, Subst), TypeError> {
    match row {
        Row::Empty => Err(TypeError::RowMismatch {
            expected: Row::Cons {
                label: label.to_string(),
                arg: Box::new(arg.clone()),
                ret: Box::new(ret.clone()),
                tail: Box::new(Row::Empty),
            },
            found: Row::Empty,
            span,
        }),
        Row::Var(v) => {
            let fresh = supply.fresh_row_var();
            let extended = Row::Cons {
                label: label.to_string(),
                arg: Box::new(arg.clone()),
                ret: Box::new(ret.clone()),
                tail: Box::new(Row::Var(fresh)),
            };
            if row_occurs(*v, &extended, s) {
                return Err(TypeError::RowOccursCheck {
                    var: *v,
                    row: extended,
                    span,
                });
            }
            let ext = Subst::singleton_row(*v, extended);
            let s1 = ext.compose(s);
            Ok((Row::Var(fresh), s1))
        }
        Row::Cons { label: l, arg: a, ret: r, tail: t } => {
            if label == l.as_str() {
                let s1 = unify(s, arg, a, span, supply)?;
                let s2 = unify(&s1, ret, r, span, supply)?;
                Ok(((**t).clone(), s2))
            } else {
                let (rest_tail, s1) =
                    rewrite_row(label, arg, ret, t, span, s, supply)?;
                let reconstructed = Row::Cons {
                    label: l.clone(),
                    arg: a.clone(),
                    ret: r.clone(),
                    tail: Box::new(rest_tail),
                };
                Ok((reconstructed, s1))
            }
        }
    }
}

fn row_occurs(v: RowVar, row: &Row, s: &Subst) -> bool {
    let resolved = s.apply_row(row);
    let mut acc = std::collections::BTreeSet::new();
    resolved.collect_free_row_vars(&mut acc);
    acc.contains(&v)
}



pub fn instantiate(scheme: &Scheme, supply: &mut TyVarSupply) -> Ty {
    if scheme.vars.is_empty() { return scheme.ty.clone(); }
    let mut renaming = Subst::empty();
    for v in &scheme.vars {
        let fresh = supply.fresh();
        renaming = renaming.compose(&Subst::singleton(*v, Ty::Var(fresh)));
    }
    renaming.apply(&scheme.ty)
}

pub fn generalize(ty: &Ty, env_free: &BTreeSet<TyVar>) -> Scheme {
    let ty_free = ty.free_vars();
    let vars: Vec<TyVar> = ty_free.difference(env_free).copied().collect();
    Scheme { vars, ty: ty.clone() }
}

// ============================================================================
// Inference driver (new in B1.5a)
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct TypeEnv { bindings: HashMap<String, Scheme> }

impl TypeEnv {
    pub fn empty() -> Self { Self::default() }
    pub fn lookup(&self, name: &str) -> Option<&Scheme> { self.bindings.get(name) }
    pub fn extend(&self, name: String, scheme: Scheme) -> Self {
        let mut next = self.bindings.clone();
        next.insert(name, scheme);
        TypeEnv { bindings: next }
    }
    pub fn apply(&self, s: &Subst) -> Self {
        let bindings = self.bindings.iter()
            .map(|(k, v)| (k.clone(), s.apply_scheme(v))).collect();
        TypeEnv { bindings }
    }
    pub fn free_vars(&self) -> BTreeSet<TyVar> {
        let mut acc = BTreeSet::new();
        for scheme in self.bindings.values() {
            acc.extend(scheme.free_vars());
        }
        acc
    }
}

pub fn infer(env: &TypeEnv, expr: &Expr, supply: &mut TyVarSupply)
    -> Result<(Subst, Ty), TypeError>
{
    match &expr.node {
        ExprKind::Int(_) => Ok((Subst::empty(), Ty::Int)),
        ExprKind::Bool(_) => Ok((Subst::empty(), Ty::Bool)),
        ExprKind::Var(name) => match env.lookup(name) {
            Some(scheme) => Ok((Subst::empty(), instantiate(scheme, supply))),
            None => Err(TypeError::Unbound { name: name.clone(), span: expr.span }),
        },
        ExprKind::Lambda { param, body } => {
            let param_ty = supply.fresh_ty();
            let extended = env.extend(param.clone(), Scheme::mono(param_ty.clone()));
            let (s_body, t_body) = infer(&extended, body, supply)?;
            // B2.0: empty row; B2.3 will mint a fresh row var here.
            let arrow = Ty::arrow(s_body.apply(&param_ty), t_body);
            Ok((s_body, arrow))
        }
        ExprKind::App { callee, arg } => {
            let (s1, t_callee) = infer(env, callee, supply)?;
            let env_after_callee = env.apply(&s1);
            let (s2, t_arg) = infer(&env_after_callee, arg, supply)?;
            let result_var = supply.fresh_ty();
            let t_callee_subst = s2.apply(&t_callee);
            // B2.0: callee's row is empty; B2.3 unifies with ambient row.
            let s3 = unify(&Subst::empty(), &t_callee_subst,
                &Ty::arrow(t_arg, result_var.clone()), expr.span, supply)?;
            let combined = s3.compose(&s2).compose(&s1);
            let result_ty = combined.apply(&result_var);
            Ok((combined, result_ty))
        }
        ExprKind::Let { name, value, body } => {
            let (s1, t_value) = infer(env, value, supply)?;
            let env_after_value = env.apply(&s1);
            let scheme = generalize(&t_value, &env_after_value.free_vars());
            let env_with_name = env_after_value.extend(name.clone(), scheme);
            let (s2, t_body) = infer(&env_with_name, body, supply)?;
            Ok((s2.compose(&s1), t_body))
        }
        ExprKind::LetRec { name, value, body } => {
            // B1.6: proper HM let-rec. The recursive occurrence inside
            // the RHS is monomorphic (sees t_name directly); the body
            // sees a generalized scheme so the binding is polymorphic
            // at use sites.
            let t_name = supply.fresh_ty();
            let env_for_value =
                env.extend(name.clone(), Scheme::mono(t_name.clone()));
            let (s1, t_value) = infer(&env_for_value, value, supply)?;
            // Unify the recursive monovar with the inferred RHS type.
            let s2 = unify(&s1, &t_name, &t_value, expr.span, supply)?;
            // Generalize against the *outer* env (not env_for_value),
            // so the recursive binding itself does not appear in the
            // free-var set we generalize over.
            let env_after = env.apply(&s2);
            let t_name_solved = s2.apply(&t_name);
            let scheme = generalize(&t_name_solved, &env_after.free_vars());
            let env_for_body = env_after.extend(name.clone(), scheme);
            let (s3, t_body) = infer(&env_for_body, body, supply)?;
            Ok((s3.compose(&s2), t_body))
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            let (s1, t_cond) = infer(env, cond, supply)?;
            let s2 = unify(&s1, &t_cond, &Ty::Bool, cond.span, supply)?;
            let env_after = env.apply(&s2);
            let (s3, t_then) = infer(&env_after, then_branch, supply)?;
            let s3 = s3.compose(&s2);
            let env_after = env.apply(&s3);
            let (s4, t_else) = infer(&env_after, else_branch, supply)?;
            let s4 = s4.compose(&s3);
            let s5 = unify(&s4, &t_then, &t_else, expr.span, supply)?;
            let ty = s5.apply(&t_then);
            Ok((s5, ty))
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let (s1, t_lhs) = infer(env, lhs, supply)?;
            let env_after = env.apply(&s1);
            let (s2, t_rhs) = infer(&env_after, rhs, supply)?;
            let s2 = s2.compose(&s1);
            if matches!(op, BinOp::Eq) {
                let s3 = unify(&s2, &t_lhs, &t_rhs, expr.span, supply)?;
                Ok((s3, Ty::Bool))
            } else {
                let (expected_lhs, expected_rhs, result_ty) = binop_signature(*op);
                let s3 = unify(&s2, &t_lhs, &expected_lhs, lhs.span, supply)?;
                let s4 = unify(&s3, &t_rhs, &expected_rhs, rhs.span, supply)?;
                Ok((s4, result_ty))
            }
        }
        // B2.2a scaffolding: Perform inference lands in B2.2b.
        ExprKind::Perform { label, .. } => {
            todo!("B2.2b: Perform inference -- effect {:?}", label)
        }
    }
}

fn binop_signature(op: BinOp) -> (Ty, Ty, Ty) {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => (Ty::Int, Ty::Int, Ty::Int),
        BinOp::Lt | BinOp::Gt => (Ty::Int, Ty::Int, Ty::Bool),
        BinOp::Eq => unreachable!("Eq handled separately"),
    }
}

pub fn infer_top(expr: &Expr) -> Result<Ty, TypeError> {
    let mut supply = TyVarSupply::new();
    let (s, t) = infer(&TypeEnv::empty(), expr, &mut supply)?;
    Ok(s.apply(&t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;
    use crate::{lex, parse};

    fn sp() -> Span { Span::new(0, 0) }
    fn v(n: u32) -> Ty { Ty::Var(TyVar(n)) }
    fn rv(n: u32) -> Row { Row::Var(RowVar(n)) }
    fn cons(label: &str, arg: Ty, ret: Ty, tail: Row) -> Row {
        Row::Cons {
            label: label.to_string(),
            arg: Box::new(arg),
            ret: Box::new(ret),
            tail: Box::new(tail),
        }
    }
    fn unify_t(a: &Ty, b: &Ty) -> Result<Subst, TypeError> {
        let mut supply = TyVarSupply::new();
        unify(&Subst::empty(), a, b, sp(), &mut supply)
    }
    fn unify_r(a: &Row, b: &Row) -> Result<Subst, TypeError> {
        let mut supply = TyVarSupply::new();
        unify_row(&Subst::empty(), a, b, sp(), &mut supply)
    }

    fn ty_of(src: &str) -> Ty {
        let toks = lex(src).expect("lex");
        let expr = parse(&toks).expect("parse");
        infer_top(&expr).expect("infer")
    }
    fn ty_err(src: &str) -> TypeError {
        let toks = lex(src).expect("lex");
        let expr = parse(&toks).expect("parse");
        infer_top(&expr).expect_err("expected type error")
    }

    // ---------- B1.4 primitives ----------

    #[test]
    fn fresh_vars_are_distinct() {
        let mut s = TyVarSupply::new();
        let a = s.fresh(); let b = s.fresh(); let c = s.fresh();
        assert_ne!(a, b); assert_ne!(b, c); assert_ne!(a, c);
    }

    #[test]
    fn apply_replaces_bound_var() {
        let s = Subst::singleton(TyVar(0), Ty::Int);
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), v(1));
    }

    #[test]
    fn apply_recurses_into_arrows() {
        let s = Subst::singleton(TyVar(0), Ty::Int);
        let t = Ty::arrow(v(0), v(0));
        assert_eq!(s.apply(&t), Ty::arrow(Ty::Int, Ty::Int));
    }

    #[test]
    fn compose_applies_right_then_left() {
        let s1 = Subst::singleton(TyVar(0), v(1));
        let s2 = Subst::singleton(TyVar(1), Ty::Int);
        let composed = s2.compose(&s1);
        assert_eq!(composed.apply(&v(0)), Ty::Int);
        assert_eq!(composed.apply(&v(1)), Ty::Int);
    }

    #[test]
    fn apply_scheme_skips_quantified_vars() {
        let s = Subst::singleton(TyVar(0), Ty::Int);
        let scheme = Scheme { vars: vec![TyVar(0)], ty: Ty::arrow(v(0), v(1)) };
        assert_eq!(s.apply_scheme(&scheme), scheme);
        let s = Subst::singleton(TyVar(1), Ty::Bool);
        let after = s.apply_scheme(&scheme);
        assert_eq!(after, Scheme { vars: vec![TyVar(0)], ty: Ty::arrow(v(0), Ty::Bool) });
    }

    #[test]
    fn unify_two_concretes_succeeds_trivially() {
        let s = unify_t(&Ty::Int, &Ty::Int).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_concrete_mismatch_is_an_error() {
        let err = unify_t(&Ty::Int, &Ty::Bool).unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }));
    }

    #[test]
    fn unify_var_binds() {
        let s = unify_t(&v(0), &Ty::Int).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
    }

    #[test]
    fn unify_var_with_itself_yields_empty_extension() {
        let s = unify_t(&v(0), &v(0)).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_arrows_recursively() {
        let left = Ty::arrow(v(0), v(1));
        let right = Ty::arrow(Ty::Int, Ty::Bool);
        let s = unify_t(&left, &right).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Bool);
    }

    #[test]
    fn unify_propagates_constraints_across_arms() {
        let left = Ty::arrow(v(0), v(0));
        let right = Ty::arrow(Ty::Int, v(1));
        let s = unify_t(&left, &right).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Int);
    }

    #[test]
    fn unify_arrow_with_concrete_is_mismatch() {
        let err = unify_t(&Ty::arrow(Ty::Int, Ty::Int), &Ty::Bool).unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }));
    }

    #[test]
    fn occurs_check_rejects_self_application() {
        let err = unify_t(&v(0), &Ty::arrow(v(0), v(1))).unwrap_err();
        match err {
            TypeError::OccursCheck { var, .. } => assert_eq!(var, TyVar(0)),
            other => panic!("expected OccursCheck, got {other:?}"),
        }
    }

    #[test]
    fn occurs_check_indirect_through_subst() {
        let s = Subst::singleton(TyVar(1), Ty::arrow(v(0), v(2)));
        let mut supply = TyVarSupply::new();
        let err = unify(&s, &v(0), &v(1), sp(), &mut supply).unwrap_err();
        assert!(matches!(err, TypeError::OccursCheck { .. }));
    }

    #[test]
    fn instantiate_monomorphic_scheme_is_identity() {
        let scheme = Scheme::mono(Ty::Int);
        let mut supply = TyVarSupply::new();
        assert_eq!(instantiate(&scheme, &mut supply), Ty::Int);
    }

    #[test]
    fn instantiate_polymorphic_scheme_uses_fresh_vars() {
        let scheme = Scheme {
            vars: vec![TyVar(99)],
            ty: Ty::arrow(Ty::Var(TyVar(99)), Ty::Var(TyVar(99))),
        };
        let mut supply = TyVarSupply::new();
        let t1 = instantiate(&scheme, &mut supply);
        let t2 = instantiate(&scheme, &mut supply);
        match (&t1, &t2) {
            (Ty::Fun(a1, _, b1), Ty::Fun(a2, _, b2)) => {
                assert_eq!(a1, b1); assert_eq!(a2, b2); assert_ne!(a1, a2);
            }
            _ => panic!("expected arrows"),
        }
    }

    #[test]
    fn generalize_with_empty_env_quantifies_everything() {
        let ty = Ty::arrow(v(0), v(1));
        let scheme = generalize(&ty, &BTreeSet::new());
        assert_eq!(scheme.vars.len(), 2);
        assert!(scheme.vars.contains(&TyVar(0)));
        assert!(scheme.vars.contains(&TyVar(1)));
    }

    #[test]
    fn generalize_skips_env_free_vars() {
        let ty = Ty::arrow(v(0), v(1));
        let mut env_free = BTreeSet::new();
        env_free.insert(TyVar(0));
        let scheme = generalize(&ty, &env_free);
        assert_eq!(scheme.vars, vec![TyVar(1)]);
    }

    // ---------- B1.5a driver ----------

    #[test]
    fn infer_int_literal() { assert_eq!(ty_of("42"), Ty::Int); }

    #[test]
    fn infer_bool_literal() { assert_eq!(ty_of("true"), Ty::Bool); }

    #[test]
    fn infer_arithmetic_is_int() { assert_eq!(ty_of("1 + 2 * 3"), Ty::Int); }

    #[test]
    fn infer_comparison_is_bool() { assert_eq!(ty_of("1 < 2"), Ty::Bool); }

    #[test]
    fn infer_arith_on_bool_is_mismatch() {
        assert!(matches!(ty_err("1 + true"), TypeError::Mismatch { .. }));
    }

    #[test]
    fn infer_identity_lambda_is_polymorphic_arrow() {
        match ty_of("fn(x) => x") {
            Ty::Fun(a, _, b) => assert_eq!(a, b),
            other => panic!("expected arrow, got {other}"),
        }
    }

    #[test]
    fn infer_const_lambda_arrow() {
        match ty_of("fn(x) => 42") {
            Ty::Fun(_a, _, b) => assert_eq!(*b, Ty::Int),
            other => panic!("expected arrow, got {other}"),
        }
    }

    #[test]
    fn infer_application_returns_result_type() {
        assert_eq!(ty_of("(fn(x) => x + 1)(41)"), Ty::Int);
    }

    #[test]
    fn infer_application_to_wrong_type_is_mismatch() {
        assert!(matches!(ty_err("(fn(x) => x + 1)(true)"), TypeError::Mismatch { .. }));
    }

    #[test]
    fn infer_applying_non_function_is_mismatch() {
        assert!(matches!(ty_err("1(2)"), TypeError::Mismatch { .. }));
    }

    #[test]
    fn let_polymorphism_id_used_at_two_types() {
        // id : forall 'a. 'a -> 'a, used at two different instantiations.
        assert_eq!(ty_of("let id = fn(x) => x in (id(id))(5)"), Ty::Int);
    }

    #[test]
    fn let_monomorphic_use_still_works() {
        assert_eq!(ty_of("let inc = fn(n) => n + 1 in inc(41)"), Ty::Int);
    }

    #[test]
    fn infer_if_returns_branch_type() {
        assert_eq!(ty_of("if true then 1 else 2"), Ty::Int);
        assert_eq!(ty_of("if 1 < 2 then false else true"), Ty::Bool);
    }

    #[test]
    fn if_non_bool_cond_is_mismatch() {
        assert!(matches!(ty_err("if 1 then 2 else 3"), TypeError::Mismatch { .. }));
    }

    #[test]
    fn if_branch_mismatch_is_an_error() {
        assert!(matches!(ty_err("if true then 1 else false"), TypeError::Mismatch { .. }));
    }

    #[test]
    fn unbound_variable_is_reported_at_use_site() {
        match ty_err("oops") {
            TypeError::Unbound { name, .. } => assert_eq!(name, "oops"),
            other => panic!("expected Unbound, got {other:?}"),
        }
    }

    #[test]
    fn self_application_triggers_occurs_check() {
        assert!(matches!(ty_err("fn(x) => x(x)"), TypeError::OccursCheck { .. }));
    }

    #[test]
    fn letrec_factorial_typechecks_at_int_to_int() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact";
        assert_eq!(ty_of(src), Ty::arrow(Ty::Int, Ty::Int));
    }

    #[test]
    fn letrec_application_typechecks_at_int() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact(5)";
        assert_eq!(ty_of(src), Ty::Int);
    }

    // ---------- B2.1 row unification ----------

    #[test]
    fn b21_unify_row_empty_empty_succeeds() {
        let s = unify_r(&Row::Empty, &Row::Empty).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn b21_unify_row_var_with_empty_binds() {
        let s = unify_r(&rv(0), &Row::Empty).unwrap();
        assert_eq!(s.apply_row(&rv(0)), Row::Empty);
    }

    #[test]
    fn b21_unify_row_empty_with_var_binds() {
        let s = unify_r(&Row::Empty, &rv(0)).unwrap();
        assert_eq!(s.apply_row(&rv(0)), Row::Empty);
    }

    #[test]
    fn b21_unify_row_same_label_same_signature_recurses() {
        let a = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let b = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let s = unify_r(&a, &b).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn b21_unify_row_same_label_different_arg_is_type_mismatch() {
        let a = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let b = cons("Print", Ty::Bool, Ty::Bool, Row::Empty);
        let err = unify_r(&a, &b).unwrap_err();
        match err {
            TypeError::Mismatch { .. } => {}
            other => panic!("expected Mismatch on arg type, got {other:?}"),
        }
    }

    #[test]
    fn b21_unify_row_different_labels_swaps_and_unifies() {
        // High-numbered ids dodge supply collision (see note on
        // b21_unify_row_disjoint_with_open_tails_succeeds).
        let a = cons("Print", Ty::Int, Ty::Bool,
                cons("Ask", Ty::Int, Ty::Int, rv(100)));
        let b = cons("Ask", Ty::Int, Ty::Int,
                cons("Print", Ty::Int, Ty::Bool, rv(101)));
        let s = unify_r(&a, &b).expect("rows with permuted labels should unify");
        let ra = s.apply_row(&rv(100));
        let rb = s.apply_row(&rv(101));
        assert_eq!(ra, rb, "tails should resolve identically: {ra:?} vs {rb:?}");
    }

    #[test]
    fn b21_unify_row_var_occurs_in_self_is_error() {
        let a = rv(0);
        let b = cons("Print", Ty::Int, Ty::Bool, rv(0));
        let err = unify_r(&a, &b).unwrap_err();
        match err {
            TypeError::RowOccursCheck { var, .. } => assert_eq!(var, RowVar(0)),
            other => panic!("expected RowOccursCheck, got {other:?}"),
        }
    }

    #[test]
    fn b21_unify_row_var_occurs_in_cons_tail_is_error() {
        let a = rv(0);
        let b = cons("Print", Ty::Int, Ty::Bool,
                cons("Ask", Ty::Int, Ty::Int, rv(0)));
        let err = unify_r(&a, &b).unwrap_err();
        assert!(matches!(err, TypeError::RowOccursCheck { .. }));
    }

    #[test]
    fn b21_unify_row_disjoint_with_open_tails_succeeds() {
        // Use high-numbered row-vars to dodge collision with the
        // supply's fresh_row_var(), which starts at 0. Real inference
        // never hits this because the supply is shared across the
        // whole tree; unit tests have to dodge it manually.
        let a = cons("Print", Ty::Int, Ty::Bool, rv(100));
        let b = cons("Ask", Ty::Int, Ty::Int, rv(101));
        let s = unify_r(&a, &b).expect("disjoint open rows should unify");
        let r100 = s.apply_row(&rv(100));
        let mut labels = Vec::new();
        let mut cur = &r100;
        while let Row::Cons { label, tail, .. } = cur {
            labels.push(label.clone());
            cur = tail.as_ref();
        }
        assert!(labels.contains(&"Ask".to_string()),
            "r100 should resolve to a row including Ask; got labels {labels:?}");
    }

    #[test]
    fn b21_unify_row_label_missing_from_closed_row_is_error() {
        let a = cons("Print", Ty::Int, Ty::Bool, rv(0));
        let b = Row::Empty;
        let err = unify_r(&a, &b).unwrap_err();
        assert!(matches!(err, TypeError::RowMismatch { .. }));
    }

    #[test]
    fn b21_row_display_var_renders_curly_brace_form() {
        let r = rv(0);
        assert_eq!(format!("{r}"), "{'ra}");
    }

    #[test]
    fn b21_row_display_cons_with_var_tail_renders_pipe() {
        let r = cons("Print", Ty::Int, Ty::Bool, rv(0));
        assert_eq!(format!("{r}"), "{Print | 'ra}");
    }

    #[test]
    fn b21_row_display_cons_chain_uses_commas() {
        let r = cons("Print", Ty::Int, Ty::Bool,
                cons("Ask", Ty::Int, Ty::Int, Row::Empty));
        assert_eq!(format!("{r}"), "{Print, Ask}");
    }

    // ----- B1.6 let-rec typing tests -----

    fn infer_source(src: &str) -> Result<Ty, TypeError> {
        let tokens = crate::lexer::lex(src).expect("lex");
        let expr = crate::parser::parse(&tokens).expect("parse");
        let env = TypeEnv::default();
        let mut supply = TyVarSupply::new();
        infer(&env, &expr, &mut supply).map(|(s, t)| s.apply(&t))
    }

    #[test]
    fn b16_letrec_factorial_still_types_as_int_to_int() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact";
        let ty = infer_source(src).expect("factorial should type-check");
        match ty {
            Ty::Fun(a, _, b) => {
                assert!(matches!(*a, Ty::Int), "arg should be Int, got {:?}", a);
                assert!(matches!(*b, Ty::Int), "ret should be Int, got {:?}", b);
            }
            other => panic!("expected Int -> Int, got {:?}", other),
        }
    }

    #[test]
    fn b16_letrec_identity_is_generalized_at_use_sites() {
        // Polymorphic identity used at two different types in the body.
        // Requires generalization of the let-rec binding.
        let src = "let rec id = fn(x) => x in let a = id(1) in let b = id(true) in a";
        let ty = infer_source(src).expect("polymorphic id should type-check");
        assert!(matches!(ty, Ty::Int), "result should be Int, got {:?}", ty);
    }

    #[test]
    fn b16_letrec_recursive_occurrence_is_monomorphic_inside_body() {
        // Inside the RHS, f has type t_name (a monovar). f(true) forces
        // t_name to be Bool -> a; the outer call f(1) forces Int -> b.
        // Conflict -> Mismatch.
        let src = "let rec f = fn(x) => f(true) in f(1)";
        let err = infer_source(src)
            .expect_err("polymorphic recursion inside body must be rejected");
        match err {
            TypeError::Mismatch { .. } => {}
            other => panic!("expected Mismatch, got {:?}", other),
        }
    }

    #[test]
    fn b16_letrec_body_type_error_span_points_into_body() {
        let src = "let rec fact = fn(n) => if n == 0 then 1 else n * fact(n - 1) in fact + true";
        let err = infer_source(src).expect_err("body has fun + Bool");
        let span = match err {
            TypeError::Mismatch { span, .. } => span,
            other => panic!("expected Mismatch, got {:?}", other),
        };
        let rhs_end = src.find(" in ").expect("in keyword") as u32;
        assert!(
            span.start >= rhs_end,
            "error span {}..{} should be inside body (>= {})",
            span.start, span.end, rhs_end
        );
    }
}
