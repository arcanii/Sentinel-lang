//! Hindley-Milner inference primitives for Sentinel-Mini (B1.4).
//!
//! This module ships the *machinery* of HM - substitutions, fresh
//! variable supply, occurs check, unification, instantiation, and
//! generalisation - but not yet the `infer(expr) -> Ty` driver. That
//! lands in B1.5 once the primitives below are exercised in isolation.
//!
//! # Substitution shape
//!
//! Substitutions are eager (apply-on-bind) rather than lazy
//! (union-find-with-find-on-lookup). The textbook tradeoff: eager
//! substitutions are simpler to reason about and adequate for a research
//! interpreter; union-find is faster for large programs. Sentinel-Mini
//! programs are tiny, so we take the simple road.
//!
//! # Errors
//!
//! Inference errors carry the offending [`Span`] from B1.2 so B1.7's
//! diagnostic renderer can point at source. The span is the span of the
//! AST node that triggered unification; the inference driver in B1.5
//! threads it in.

use crate::span::Span;
use crate::types::{Scheme, Ty, TyVar};
use std::collections::{BTreeSet, HashMap};
use thiserror::Error;

/// Errors produced by the inference primitives.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypeError {
    /// Unification failed: two distinct concrete shapes don't agree.
    /// `span` is the location whose typing triggered the unification.
    #[error("type mismatch: expected {expected}, found {found}")]
    Mismatch {
        expected: Ty,
        found: Ty,
        span: Span,
    },
    /// The occurs check rejected a unification that would create an
    /// infinite type (e.g. `'a ~ 'a -> 'b`).
    #[error("occurs check failed: {var} appears in {ty}")]
    OccursCheck {
        var: TyVar,
        ty: Ty,
        span: Span,
    },
}

/// A fresh-type-variable supply. Hand one of these to the inferer and
/// it produces a stream of unique `TyVar`s.
#[derive(Debug, Default, Clone)]
pub struct TyVarSupply {
    next: u32,
}

impl TyVarSupply {
    pub fn new() -> Self {
        Self::default()
    }

    /// Produce a fresh [`TyVar`] that has not been seen before from this
    /// supply.
    pub fn fresh(&mut self) -> TyVar {
        let v = TyVar(self.next);
        self.next = self
            .next
            .checked_add(1)
            .expect("TyVarSupply exhausted u32::MAX type variables");
        v
    }

    /// Produce a fresh [`Ty::Var`]. Sugar for `Ty::Var(self.fresh())`.
    pub fn fresh_ty(&mut self) -> Ty {
        Ty::Var(self.fresh())
    }
}

/// A substitution from type variables to types.
///
/// Invariant: substitutions are *idempotent* - applying a substitution
/// to its own range produces the range unchanged. The `compose` and
/// `bind` helpers maintain this; raw `HashMap::insert` does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subst {
    map: HashMap<TyVar, Ty>,
}

impl Subst {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Construct a singleton substitution `{ v -> t }`. Does not perform
    /// the occurs check; callers must have verified it.
    pub fn singleton(v: TyVar, t: Ty) -> Self {
        let mut map = HashMap::new();
        map.insert(v, t);
        Subst { map }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Apply this substitution to a type, recursively.
    pub fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Int => Ty::Int,
            Ty::Bool => Ty::Bool,
            Ty::Var(v) => match self.map.get(v) {
                // Re-apply to the bound term in case of chains; idempotency
                // means this terminates in at most one extra step.
                Some(bound) => self.apply(bound),
                None => Ty::Var(*v),
            },
            Ty::Fun(a, b) => Ty::Fun(Box::new(self.apply(a)), Box::new(self.apply(b))),
        }
    }

    /// Apply this substitution to a scheme: walk inside, but do *not*
    /// touch the quantified variables.
    pub fn apply_scheme(&self, s: &Scheme) -> Scheme {
        // Build a temporary subst that omits the scheme's quantified
        // vars, so we don't accidentally substitute them.
        let mut filtered = self.map.clone();
        for v in &s.vars {
            filtered.remove(v);
        }
        let tmp = Subst { map: filtered };
        Scheme {
            vars: s.vars.clone(),
            ty: tmp.apply(&s.ty),
        }
    }

    /// Compose two substitutions: `(s2.compose(&s1)).apply(t) ==
    /// s2.apply(&s1.apply(t))`.
    ///
    /// This is left-to-right composition: apply `s1` first, then `s2`.
    pub fn compose(&self, s1: &Subst) -> Subst {
        // First, apply self to every binding in s1.
        let mut composed: HashMap<TyVar, Ty> = s1
            .map
            .iter()
            .map(|(v, t)| (*v, self.apply(t)))
            .collect();
        // Then, add bindings from self that aren't already in s1.
        for (v, t) in &self.map {
            composed.entry(*v).or_insert_with(|| t.clone());
        }
        Subst { map: composed }
    }
}

/// Check whether `v` appears anywhere in `t` after applying `s`. Used
/// by `unify` to refuse infinite types.
fn occurs(v: TyVar, t: &Ty, s: &Subst) -> bool {
    s.apply(t).free_vars().contains(&v)
}

/// Bind `v` to `t` after performing the occurs check. The returned
/// substitution is `{ v -> t }` if the bind is admissible.
fn bind(v: TyVar, t: &Ty, span: Span, s: &Subst) -> Result<Subst, TypeError> {
    // Identity bind: 'a ~ 'a is fine and produces no constraint.
    if let Ty::Var(v2) = t {
        if *v2 == v {
            return Ok(Subst::empty());
        }
    }
    let t_resolved = s.apply(t);
    if occurs(v, &t_resolved, &Subst::empty()) {
        return Err(TypeError::OccursCheck { var: v, ty: t_resolved, span });
    }
    Ok(Subst::singleton(v, t_resolved))
}

/// Unify two types under an existing substitution. Returns a new
/// substitution that extends `s` to make the types equal.
///
/// `span` is attached to any error this unification produces.
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

/// Instantiate a scheme by replacing each quantified variable with a
/// fresh type variable from `supply`.
pub fn instantiate(scheme: &Scheme, supply: &mut TyVarSupply) -> Ty {
    if scheme.vars.is_empty() {
        return scheme.ty.clone();
    }
    let mut renaming = Subst::empty();
    for v in &scheme.vars {
        let fresh = supply.fresh();
        renaming = renaming.compose(&Subst::singleton(*v, Ty::Var(fresh)));
    }
    renaming.apply(&scheme.ty)
}

/// Generalise a type to a scheme by quantifying over variables that are
/// free in `ty` but NOT free in the surrounding environment.
///
/// `env_free` should be the union of free vars across the type
/// environment in scope at this binding site - the inference driver
/// in B1.5 will compute this.
pub fn generalize(ty: &Ty, env_free: &BTreeSet<TyVar>) -> Scheme {
    let ty_free = ty.free_vars();
    let vars: Vec<TyVar> = ty_free.difference(env_free).copied().collect();
    Scheme { vars, ty: ty.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn sp() -> Span {
        // Tests don't care about the actual span; just need a value.
        Span::new(0, 0)
    }

    fn v(n: u32) -> Ty {
        Ty::Var(TyVar(n))
    }

    // ---------- TyVarSupply ----------

    #[test]
    fn fresh_vars_are_distinct() {
        let mut s = TyVarSupply::new();
        let a = s.fresh();
        let b = s.fresh();
        let c = s.fresh();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    // ---------- Subst ----------

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
        // s1: 'a -> 'b
        // s2: 'b -> int
        // composed.apply('a) should give Int (chained through 'b).
        let s1 = Subst::singleton(TyVar(0), v(1));
        let s2 = Subst::singleton(TyVar(1), Ty::Int);
        let composed = s2.compose(&s1);
        assert_eq!(composed.apply(&v(0)), Ty::Int);
        assert_eq!(composed.apply(&v(1)), Ty::Int);
    }

    #[test]
    fn apply_scheme_skips_quantified_vars() {
        // Subst: 'a -> Int
        // Scheme: forall 'a. 'a -> 'b
        // Result: forall 'a. 'a -> 'b  (unchanged; 'a is bound)
        let s = Subst::singleton(TyVar(0), Ty::Int);
        let scheme = Scheme {
            vars: vec![TyVar(0)],
            ty: Ty::arrow(v(0), v(1)),
        };
        let after = s.apply_scheme(&scheme);
        assert_eq!(after, scheme);

        // But if we substitute the FREE var:
        let s = Subst::singleton(TyVar(1), Ty::Bool);
        let after = s.apply_scheme(&scheme);
        assert_eq!(
            after,
            Scheme {
                vars: vec![TyVar(0)],
                ty: Ty::arrow(v(0), Ty::Bool),
            }
        );
    }

    // ---------- unify ----------

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
        // 'a ~ 'a should add no new bindings.
        let s = unify(&Subst::empty(), &v(0), &v(0), sp()).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn unify_arrows_recursively() {
        // ('a -> 'b) ~ (Int -> Bool)  =>  'a := Int, 'b := Bool
        let left = Ty::arrow(v(0), v(1));
        let right = Ty::arrow(Ty::Int, Ty::Bool);
        let s = unify(&Subst::empty(), &left, &right, sp()).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Bool);
    }

    #[test]
    fn unify_propagates_constraints_across_arms() {
        // ('a -> 'a) ~ (Int -> 'b)  =>  'a := Int, 'b := Int
        let left = Ty::arrow(v(0), v(0));
        let right = Ty::arrow(Ty::Int, v(1));
        let s = unify(&Subst::empty(), &left, &right, sp()).unwrap();
        assert_eq!(s.apply(&v(0)), Ty::Int);
        assert_eq!(s.apply(&v(1)), Ty::Int);
    }

    #[test]
    fn unify_arrow_with_concrete_is_mismatch() {
        let err = unify(
            &Subst::empty(),
            &Ty::arrow(Ty::Int, Ty::Int),
            &Ty::Bool,
            sp(),
        )
        .unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }));
    }

    // ---------- occurs check ----------

    #[test]
    fn occurs_check_rejects_self_application() {
        // Classic: 'a ~ 'a -> 'b  must fail.
        let err = unify(&Subst::empty(), &v(0), &Ty::arrow(v(0), v(1)), sp()).unwrap_err();
        match err {
            TypeError::OccursCheck { var, .. } => assert_eq!(var, TyVar(0)),
            other => panic!("expected OccursCheck, got {other:?}"),
        }
    }

    #[test]
    fn occurs_check_indirect_through_subst() {
        // Build a subst where 'b is bound to ('a -> 'c), then try 'a ~ 'b.
        // After applying the subst, 'a ~ ('a -> 'c), which must fail.
        let s = Subst::singleton(TyVar(1), Ty::arrow(v(0), v(2)));
        let err = unify(&s, &v(0), &v(1), sp()).unwrap_err();
        assert!(matches!(err, TypeError::OccursCheck { .. }));
    }

    // ---------- instantiate ----------

    #[test]
    fn instantiate_monomorphic_scheme_is_identity() {
        let scheme = Scheme::mono(Ty::Int);
        let mut supply = TyVarSupply::new();
        assert_eq!(instantiate(&scheme, &mut supply), Ty::Int);
    }

    #[test]
    fn instantiate_polymorphic_scheme_uses_fresh_vars() {
        // forall 'a. 'a -> 'a   instantiated twice should yield two
        // distinct fresh vars.
        let scheme = Scheme {
            vars: vec![TyVar(99)],
            ty: Ty::arrow(Ty::Var(TyVar(99)), Ty::Var(TyVar(99))),
        };
        let mut supply = TyVarSupply::new();
        let t1 = instantiate(&scheme, &mut supply);
        let t2 = instantiate(&scheme, &mut supply);
        // Each instantiation is well-formed as 'fresh -> 'fresh
        // (same var on both sides).
        match (&t1, &t2) {
            (Ty::Fun(a1, b1), Ty::Fun(a2, b2)) => {
                assert_eq!(a1, b1);
                assert_eq!(a2, b2);
                assert_ne!(a1, a2);
            }
            _ => panic!("expected arrows: {t1:?}, {t2:?}"),
        }
    }

    // ---------- generalize ----------

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
        // 'a is "in the environment"; only 'b gets quantified.
        let ty = Ty::arrow(v(0), v(1));
        let mut env_free = BTreeSet::new();
        env_free.insert(TyVar(0));
        let scheme = generalize(&ty, &env_free);
        assert_eq!(scheme.vars, vec![TyVar(1)]);
    }
}
