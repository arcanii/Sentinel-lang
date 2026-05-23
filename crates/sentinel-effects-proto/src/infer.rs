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
use crate::types::{Scheme, Ty, TyVar};
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
}

#[derive(Debug, Default, Clone)]
pub struct TyVarSupply { next: u32 }

impl TyVarSupply {
    pub fn new() -> Self { Self::default() }
    pub fn fresh(&mut self) -> TyVar {
        let v = TyVar(self.next);
        self.next = self.next.checked_add(1).expect("TyVarSupply exhausted");
        v
    }
    pub fn fresh_ty(&mut self) -> Ty { Ty::Var(self.fresh()) }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subst { map: HashMap<TyVar, Ty> }

impl Subst {
    pub fn empty() -> Self { Self::default() }
    pub fn singleton(v: TyVar, t: Ty) -> Self {
        let mut map = HashMap::new();
        map.insert(v, t);
        Subst { map }
    }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
    pub fn len(&self) -> usize { self.map.len() }
    pub fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Int => Ty::Int,
            Ty::Bool => Ty::Bool,
            Ty::Var(v) => match self.map.get(v) {
                Some(bound) => self.apply(bound),
                None => Ty::Var(*v),
            },
            Ty::Fun(a, b) => Ty::Fun(Box::new(self.apply(a)), Box::new(self.apply(b))),
        }
    }
    pub fn apply_scheme(&self, s: &Scheme) -> Scheme {
        let mut filtered = self.map.clone();
        for v in &s.vars { filtered.remove(v); }
        let tmp = Subst { map: filtered };
        Scheme { vars: s.vars.clone(), ty: tmp.apply(&s.ty) }
    }
    pub fn compose(&self, s1: &Subst) -> Subst {
        let mut composed: HashMap<TyVar, Ty> =
            s1.map.iter().map(|(v, t)| (*v, self.apply(t))).collect();
        for (v, t) in &self.map {
            composed.entry(*v).or_insert_with(|| t.clone());
        }
        Subst { map: composed }
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

pub fn unify(s: &Subst, a: &Ty, b: &Ty, span: Span) -> Result<Subst, TypeError> {
    let a = s.apply(a);
    let b = s.apply(b);
    match (a, b) {
        (Ty::Int, Ty::Int) => Ok(s.clone()),
        (Ty::Bool, Ty::Bool) => Ok(s.clone()),
        (Ty::Var(v), t) | (t, Ty::Var(v)) => {
            let extension = bind(v, &t, span, s)?;
            Ok(extension.compose(s))
        }
        (Ty::Fun(a1, b1), Ty::Fun(a2, b2)) => {
            let s1 = unify(s, &a1, &a2, span)?;
            unify(&s1, &b1, &b2, span)
        }
        (expected, found) => Err(TypeError::Mismatch { expected, found, span }),
    }
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
            let arrow = Ty::arrow(s_body.apply(&param_ty), t_body);
            Ok((s_body, arrow))
        }
        ExprKind::App { callee, arg } => {
            let (s1, t_callee) = infer(env, callee, supply)?;
            let env_after_callee = env.apply(&s1);
            let (s2, t_arg) = infer(&env_after_callee, arg, supply)?;
            let result_var = supply.fresh_ty();
            let t_callee_subst = s2.apply(&t_callee);
            let s3 = unify(&Subst::empty(), &t_callee_subst,
                &Ty::arrow(t_arg, result_var.clone()), expr.span)?;
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
            // B1.5 stub: monomorphic. See module docs.
            let t_name = supply.fresh_ty();
            let env_for_value = env.extend(name.clone(), Scheme::mono(t_name.clone()));
            let (s1, t_value) = infer(&env_for_value, value, supply)?;
            let s2 = unify(&s1, &t_name, &t_value, expr.span)?;
            let env_for_body = env.apply(&s2)
                .extend(name.clone(), Scheme::mono(s2.apply(&t_name)));
            let (s3, t_body) = infer(&env_for_body, body, supply)?;
            Ok((s3.compose(&s2), t_body))
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            let (s1, t_cond) = infer(env, cond, supply)?;
            let s2 = unify(&s1, &t_cond, &Ty::Bool, cond.span)?;
            let env_after = env.apply(&s2);
            let (s3, t_then) = infer(&env_after, then_branch, supply)?;
            let s3 = s3.compose(&s2);
            let env_after = env.apply(&s3);
            let (s4, t_else) = infer(&env_after, else_branch, supply)?;
            let s4 = s4.compose(&s3);
            let s5 = unify(&s4, &t_then, &t_else, expr.span)?;
            let ty = s5.apply(&t_then);
            Ok((s5, ty))
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let (s1, t_lhs) = infer(env, lhs, supply)?;
            let env_after = env.apply(&s1);
            let (s2, t_rhs) = infer(&env_after, rhs, supply)?;
            let s2 = s2.compose(&s1);
            if matches!(op, BinOp::Eq) {
                let s3 = unify(&s2, &t_lhs, &t_rhs, expr.span)?;
                Ok((s3, Ty::Bool))
            } else {
                let (expected_lhs, expected_rhs, result_ty) = binop_signature(*op);
                let s3 = unify(&s2, &t_lhs, &expected_lhs, lhs.span)?;
                let s4 = unify(&s3, &t_rhs, &expected_rhs, rhs.span)?;
                Ok((s4, result_ty))
            }
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
        let s = unify(&Subst::empty(), &Ty::Int, &Ty::Int, sp()).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_concrete_mismatch_is_an_error() {
        let err = unify(&Subst::empty(), &Ty::Int, &Ty::Bool, sp()).unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }));
    }

    #[test]
    fn unify_var_binds() {
        let s = unify(&Subst::empty(), &v(0), &Ty::Int, sp()).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
    }

    #[test]
    fn unify_var_with_itself_yields_empty_extension() {
        let s = unify(&Subst::empty(), &v(0), &v(0), sp()).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_arrows_recursively() {
        let left = Ty::arrow(v(0), v(1));
        let right = Ty::arrow(Ty::Int, Ty::Bool);
        let s = unify(&Subst::empty(), &left, &right, sp()).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Bool);
    }

    #[test]
    fn unify_propagates_constraints_across_arms() {
        let left = Ty::arrow(v(0), v(0));
        let right = Ty::arrow(Ty::Int, v(1));
        let s = unify(&Subst::empty(), &left, &right, sp()).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Int);
    }

    #[test]
    fn unify_arrow_with_concrete_is_mismatch() {
        let err = unify(&Subst::empty(),
            &Ty::arrow(Ty::Int, Ty::Int), &Ty::Bool, sp()).unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }));
    }

    #[test]
    fn occurs_check_rejects_self_application() {
        let err = unify(&Subst::empty(), &v(0), &Ty::arrow(v(0), v(1)), sp()).unwrap_err();
        match err {
            TypeError::OccursCheck { var, .. } => assert_eq!(var, TyVar(0)),
            other => panic!("expected OccursCheck, got {other:?}"),
        }
    }

    #[test]
    fn occurs_check_indirect_through_subst() {
        let s = Subst::singleton(TyVar(1), Ty::arrow(v(0), v(2)));
        let err = unify(&s, &v(0), &v(1), sp()).unwrap_err();
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
            (Ty::Fun(a1, b1), Ty::Fun(a2, b2)) => {
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
            Ty::Fun(a, b) => assert_eq!(a, b),
            other => panic!("expected arrow, got {other}"),
        }
    }

    #[test]
    fn infer_const_lambda_arrow() {
        match ty_of("fn(x) => 42") {
            Ty::Fun(_a, b) => assert_eq!(*b, Ty::Int),
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
}
