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
    /// B2.2b: `do Label(arg)` is parseable but cannot yet be typed
    /// or evaluated. Real handling lands in B2.3 (inference) and
    /// B3 (handlers).
    /// B2.3b2 (ADR 0005 D9): `do Label(arg)` whose label is not in
    /// the program's effect environment. Replaces the placeholder
    /// `EffectNotYetSupported` once Perform is wired through.
    #[error("unknown effect {label:?} (no matching `effect` declaration in scope)")]
    UnknownEffect { label: String, span: Span },
    /// B2.3a (ADR 0005 D6): residual non-empty row at `infer_top`.
    /// Variant + strict-check landed inert in B2.3a per the divergence
    /// note in ADR 0005 D9; B2.3b makes the check reachable when
    /// Lambda/Perform start populating the row.
    #[error("unhandled effects: residual row {row} at top level")]
    UnhandledEffects { row: Row, span: Span },
    /// B3.1 (ADR 0007 D4): handler arm names a label not present
    /// in the body's effect row.
    #[error("handler arm {label:?} names an effect not present in the row")]
    HandlerLabelNotInRow { label: String, span: Span },
    /// B3.1 (ADR 0007 D2): two arms of one handler share a label.
    /// Parser accepts duplicates; typing rejects them.
    #[error("handler contains two arms for effect {label:?}")]
    DuplicateHandlerArm { label: String, span: Span },
    /// B4.1a (ADR 0008 D3). Raised by `unify` when a `Ty::Secret(_)`
    /// meets a non-secret non-variable type (or vice versa). Carries
    /// both sides so the diagnostic shows the direction of the
    /// disallowed flow. Construction is from the catch-all arm of
    /// `unify`; Var-vs-Secret routes to `SecretEscapesPolymorphism`
    /// instead. Also fires from the `Declassify` typing rule when the
    /// inner expression types as non-secret -- declassify expects a
    /// `Ty::Secret(_)` argument by D5.
    #[error("secret type mismatch: cannot flow {from} into {to}")]
    SecretFlow { from: Ty, to: Ty, span: Span },
    /// B4.1a (ADR 0008 D2 — no-α-leak). Raised by `unify` when a
    /// bare type variable would bind to `Ty::Secret(_)`. Substitutes
    /// for full qualifier polymorphism (shape (c)) at prototype
    /// cost: polymorphic library functions become two-flavored. The
    /// rejected program either needs a secret-aware variant of the
    /// generic function, or an explicit `declassify` to remove the
    /// qualifier first.
    #[error("secret escapes polymorphism: type variable {var} cannot bind to a secret type")]
    SecretEscapesPolymorphism { var: TyVar, span: Span },
    /// B4.1b (ADR 0008 D3). Raised by `infer` on `if` when the
    /// condition types as `Ty::Secret(_)`. Branching on a secret
    /// produces data-dependent timing on real hardware; the program
    /// must use a constant-time selection primitive (Phase C
    /// standard library) or `declassify` the condition first.
    #[error("constant-time violation: cannot branch on a value of secret type")]
    SecretBranch { span: Span },
    /// B4.1b (ADR 0008 D3). Raised by `infer` on `Div`/`Mod` when
    /// the divisor types as `Ty::Secret(_)`. Variable-time division
    /// on hardware is the canonical CT footgun.
    #[error("constant-time violation: cannot divide by a value of secret type")]
    SecretDivisor { span: Span },
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
            // ADR 0008 D1: recurse into Secret. Use Ty::secret
            // (not Ty::Secret(Box::new(...))) so the idempotency
            // invariant survives even if a future Subst maps a
            // var to a Secret type.
            Ty::Secret(inner) => Ty::secret(self.apply(inner)),
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
        Scheme { vars: s.vars.clone(), row_vars: s.row_vars.clone(), ty: tmp.apply(&s.ty) }
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
            // ADR 0008 D2 — no-α-leak. A bare type variable cannot
            // bind to a secret type; rejecting here is the cheap
            // substitute for full qualifier polymorphism (shape (c)).
            // The restriction is forward-compatible: any program that
            // types under this rule also types under qualifier
            // polymorphism if a future ADR adopts it.
            if matches!(t, Ty::Secret(_)) {
                return Err(TypeError::SecretEscapesPolymorphism { var: v, span });
            }
            let _ = supply;
            let extension = bind(v, &t, span, s)?;
            Ok(extension.compose(s))
        }
        (Ty::Fun(a1, r1, b1), Ty::Fun(a2, r2, b2)) => {
            let s1 = unify(s, &a1, &a2, span, supply)?;
            let s2 = unify(&s1, &b1, &b2, span, supply)?;
            unify_row(&s2, &r1, &r2, span, supply)
        }
        // ADR 0008 D1: structural recursion on Secret-Secret. Inner
        // types unify normally; the smart constructor ensures the
        // inner cannot itself be Secret, so recursion terminates.
        (Ty::Secret(a), Ty::Secret(b)) => unify(s, &a, &b, span, supply),
        // ADR 0008 D3 SecretFlow: Secret-vs-non-Secret-non-Var (Var
        // is handled above) is the public/secret unification failure.
        // Split out of the catch-all Mismatch arm so the diagnostic
        // names the qualifier mismatch directly.
        (expected, found) => {
            let either_secret =
                matches!(expected, Ty::Secret(_)) || matches!(found, Ty::Secret(_));
            if either_secret {
                Err(TypeError::SecretFlow { from: expected, to: found, span })
            } else {
                Err(TypeError::Mismatch { expected, found, span })
            }
        }
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

/// B3.1 (ADR 0007 D4): row subtraction. Given a row `rho` and a
/// label `L`, split out L's signature and return the residual row.
///
/// Cases mirror ADR 0007 D4:
/// 1. `Row::Cons{L, a, r, t}` -> `Ok(((a, r), t))`.
/// 2. `Row::Cons{L', a, r, t}` with `L' != L` -> recurse on `t`,
///    reconstruct head on the residual.
/// 3. `Row::Var(rho)` -> mint fresh `alpha`, `beta`, `tail`; bind
///    `rho := Cons{L, alpha, beta, tail}`; return
///    `((alpha, beta), Row::Var(tail))`. This case makes handlers
///    compose with row-polymorphic callers.
/// 4. `Row::Empty` -> `HandlerLabelNotInRow`.
///
/// Distinct from `rewrite_row` (which expects a known signature to
/// unify against; used by `unify_row` for two-sided row matching).
/// `row_split` reads the signature out as an output.
fn row_split(
    s: &Subst,
    row: &Row,
    label: &str,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<((Ty, Ty), Row, Subst), TypeError> {
    let row = s.apply_row(row);
    match row {
        Row::Cons { label: l, arg, ret, tail } => {
            if label == l.as_str() {
                Ok((((*arg).clone(), (*ret).clone()), (*tail).clone(), s.clone()))
            } else {
                let (sig, rest, s1) = row_split(s, &tail, label, span, supply)?;
                let reconstructed = Row::Cons {
                    label: l,
                    arg,
                    ret,
                    tail: Box::new(rest),
                };
                Ok((sig, reconstructed, s1))
            }
        }
        Row::Var(v) => {
            let alpha = supply.fresh_ty();
            let beta = supply.fresh_ty();
            let fresh_tail = supply.fresh_row_var();
            let bound = Row::Cons {
                label: label.to_string(),
                arg: Box::new(alpha.clone()),
                ret: Box::new(beta.clone()),
                tail: Box::new(Row::Var(fresh_tail)),
            };
            if row_occurs(v, &bound, s) {
                return Err(TypeError::RowOccursCheck { var: v, row: bound, span });
            }
            let ext = Subst::singleton_row(v, bound);
            let s1 = ext.compose(s);
            Ok(((alpha, beta), Row::Var(fresh_tail), s1))
        }
        Row::Empty => Err(TypeError::HandlerLabelNotInRow {
            label: label.to_string(),
            span,
        }),
    }
}

fn row_occurs(v: RowVar, row: &Row, s: &Subst) -> bool {
    let resolved = s.apply_row(row);
    let mut acc = std::collections::BTreeSet::new();
    resolved.collect_free_row_vars(&mut acc);
    acc.contains(&v)
}

/// B2.3b1 (ADR 0005 D2): union two row contributions.
///
/// In B2.3b1 the only row contributions are `Row::Empty` (every arm
/// except `Perform`, which is still `Err`). B2.3b2 makes `Perform`
/// contribute `Cons(label, decl_arg, decl_ret, Row::Empty)` -- a
/// closed single-label row. Therefore every union encountered in
/// practice is between two *closed* rows (Empty or Cons-chain
/// terminating in Empty). Open rows live inside arrow *types*; they
/// are not produced as row contributions by any arm.
///
/// The `unreachable!` in the `Var` case is load-bearing: it enforces
/// the closed-contributions invariant. If B3 changes the model and
/// row variables start appearing in contributions, `row_union` must
/// be generalised (and this `unreachable!` is the canary that flags
/// the day it happens).
fn row_union(
    r1: &Row,
    r2: &Row,
    s: &Subst,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<(Row, Subst), TypeError> {
    let r1 = s.apply_row(r1);
    let r2 = s.apply_row(r2);
    match (&r1, &r2) {
        (Row::Empty, _) => Ok((r2, s.clone())),
        (_, Row::Empty) => Ok((r1, s.clone())),
        // B2.3b1 (ADR 0006 D1, extended): a free row variable in a
        // *contribution* represents an unconstrained latent row
        // (typically a callee's ρ_call that nothing concrete bound).
        // Per the default-close policy, treat it as the empty row
        // for union purposes. B3 will revisit when handlers bind row
        // variables that *must* carry through to caller contributions.
        (Row::Var(_), Row::Var(_)) => Ok((Row::Empty, s.clone())),
        (Row::Var(_), _) => Ok((r2, s.clone())),
        (_, Row::Var(_)) => Ok((r1, s.clone())),
        (Row::Cons { .. }, _) => cons_onto(&r1, r2, s, span, supply),
    }
}

fn cons_onto(
    r1: &Row,
    r2: Row,
    s: &Subst,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<(Row, Subst), TypeError> {
    match r1 {
        Row::Empty => Ok((r2, s.clone())),
        // See row_union: row vars in contributions are treated as Empty.
        Row::Var(_) => Ok((r2, s.clone())),
        Row::Cons { label, arg, ret, tail } => {
            let (tail_unioned, s1) = cons_onto(tail, r2, s, span, supply)?;
            cons_or_unify(label, arg, ret, &tail_unioned, &s1, span, supply)
        }
    }
}

fn cons_or_unify(
    label: &str,
    arg: &Ty,
    ret: &Ty,
    into: &Row,
    s: &Subst,
    span: Span,
    supply: &mut TyVarSupply,
) -> Result<(Row, Subst), TypeError> {
    match into {
        Row::Empty => Ok((
            Row::Cons {
                label: label.to_string(),
                arg: Box::new(arg.clone()),
                ret: Box::new(ret.clone()),
                tail: Box::new(Row::Empty),
            },
            s.clone(),
        )),
        Row::Var(_) => Ok((
            Row::Cons {
                label: label.to_string(),
                arg: Box::new(arg.clone()),
                ret: Box::new(ret.clone()),
                tail: Box::new(Row::Empty),
            },
            s.clone(),
        )),
        Row::Cons { label: l, arg: a, ret: r, tail: t } => {
            if label == l.as_str() {
                let s1 = unify(s, arg, a, span, supply)?;
                let s2 = unify(&s1, ret, r, span, supply)?;
                Ok((into.clone(), s2))
            } else {
                let (new_tail, s1) =
                    cons_or_unify(label, arg, ret, t, s, span, supply)?;
                Ok((
                    Row::Cons {
                        label: l.clone(),
                        arg: a.clone(),
                        ret: r.clone(),
                        tail: Box::new(new_tail),
                    },
                    s1,
                ))
            }
        }
    }
}



pub fn instantiate(scheme: &Scheme, supply: &mut TyVarSupply) -> Ty {
    if scheme.vars.is_empty() && scheme.row_vars.is_empty() {
        return scheme.ty.clone();
    }
    let mut renaming = Subst::empty();
    for v in &scheme.vars {
        let fresh = supply.fresh();
        renaming = renaming.compose(&Subst::singleton(*v, Ty::Var(fresh)));
    }
    // B2.3b2-b (ADR 0005 D9): freshen quantified row vars too.
    // This is what makes let-bound effectful functions safely
    // reusable at distinct call sites with distinct latent rows.
    for rv in &scheme.row_vars {
        let fresh = supply.fresh_row_var();
        renaming = renaming.compose(&Subst::singleton_row(*rv, Row::Var(fresh)));
    }
    renaming.apply(&scheme.ty)
}

pub fn generalize(
    ty: &Ty,
    env_free: &BTreeSet<TyVar>,
    env_free_rows: &BTreeSet<RowVar>,
) -> Scheme {
    let ty_free = ty.free_vars();
    let vars: Vec<TyVar> = ty_free.difference(env_free).copied().collect();
    // B2.3b2-b (ADR 0005 D9): quantify free row vars in the type
    // minus those free in the env. Row contributions are *not*
    // considered — they describe the latent effect of the RHS,
    // not the scheme of the binding. Top-level default-close
    // (ADR 0006 D3) still erases unconstrained row vars in the
    // *returned* type; generalization preserves them inside
    // let-bound schemes for sound reuse.
    let ty_free_rows = ty.free_row_vars();
    let row_vars: Vec<RowVar> =
        ty_free_rows.difference(env_free_rows).copied().collect();
    Scheme { vars, row_vars, ty: ty.clone() }
}

// ============================================================================
// Inference driver (new in B1.5a)
// ============================================================================

/// B2.3a (ADR 0005 D6): map from declared effect labels to their
/// (arg, ret) signature. Synthesised empty by `infer_top` and
/// `infer_program` in B2.3a — `Perform` still rejects with
/// `EffectNotYetSupported` regardless of contents. B2.3b will
/// populate it from `Program.effects` and consult it in `Perform`.
pub type EffectEnv = HashMap<String, (Ty, Ty)>;

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
    /// B2.3b2-b (ADR 0005 D9): collect free row vars across all
    /// bound schemes, mirroring `free_vars`. Used by `generalize`
    /// to avoid quantifying row vars that are constrained by the
    /// surrounding environment.
    pub fn free_row_vars(&self) -> BTreeSet<RowVar> {
        let mut acc = BTreeSet::new();
        for scheme in self.bindings.values() {
            acc.extend(scheme.free_row_vars());
        }
        acc
    }
}

pub fn infer(
    env: &TypeEnv,
    eff_env: &EffectEnv,
    expr: &Expr,
    supply: &mut TyVarSupply,
) -> Result<(Subst, Ty, Row), TypeError> {
    // B2.3a: eff_env is threaded but not consulted. B2.3b's `Perform`
    // arm reads it; until then the discard keeps `-D warnings` happy.
    let _ = eff_env;
    match &expr.node {
        ExprKind::Int(_) => Ok((Subst::empty(), Ty::Int, Row::Empty)),
        ExprKind::Bool(_) => Ok((Subst::empty(), Ty::Bool, Row::Empty)),
        ExprKind::Var(name) => match env.lookup(name) {
            Some(scheme) => Ok((Subst::empty(), instantiate(scheme, supply), Row::Empty)),
            None => Err(TypeError::Unbound { name: name.clone(), span: expr.span }),
        },
        ExprKind::Lambda { param, body } => {
            let param_ty = supply.fresh_ty();
            let extended = env.extend(param.clone(), Scheme::mono(param_ty.clone()));
            let (s_body, t_body, r_body) = infer(&extended, eff_env, body, supply)?;
            // B2.3b1 (ADR 0005 D2): mint fresh row var for the arrow,
            // unify body's row contribution against it, build arrow
            // with arrow_with. The closure *value*'s row contribution
            // is Empty -- only *calling* the lambda performs effects.
            let row_var = supply.fresh_row();
            let s_unified = unify_row(&s_body, &r_body, &row_var, body.span, supply)?;
            let arrow = Ty::arrow_with(
                s_unified.apply(&param_ty),
                s_unified.apply_row(&row_var),
                s_unified.apply(&t_body),
            );
            Ok((s_unified, arrow, Row::Empty))
        }
        ExprKind::App { callee, arg } => {
            let (s1, t_callee, r_callee) = infer(env, eff_env, callee, supply)?;
            let env_after_callee = env.apply(&s1);
            let (s2, t_arg, r_arg) = infer(&env_after_callee, eff_env, arg, supply)?;
            let result_var = supply.fresh_ty();
            let t_callee_subst = s2.apply(&t_callee);
            // B2.3b1 (ADR 0005 D2): mint fresh ρ_call, unify callee
            // against arrow_with(t_arg, ρ_call, result_var). App's row
            // contribution is union(r_callee, r_arg, ρ_call_resolved).
            let rho_call = supply.fresh_row();
            let s3 = unify(
                &Subst::empty(),
                &t_callee_subst,
                &Ty::arrow_with(t_arg, rho_call.clone(), result_var.clone()),
                expr.span,
                supply,
            )?;
            let combined = s3.compose(&s2).compose(&s1);
            let result_ty = combined.apply(&result_var);
            let r_callee_resolved = combined.apply_row(&r_callee);
            let r_arg_resolved = combined.apply_row(&r_arg);
            let rho_call_resolved = combined.apply_row(&rho_call);
            let (u1, s4) =
                row_union(&r_callee_resolved, &r_arg_resolved, &combined, expr.span, supply)?;
            let (row_app, s5) = row_union(&u1, &rho_call_resolved, &s4, expr.span, supply)?;
            Ok((s5, result_ty, row_app))
        }
        ExprKind::Let { name, value, body } => {
            let (s1, t_value, r_value) = infer(env, eff_env, value, supply)?;
            let env_after_value = env.apply(&s1);
            let scheme = generalize(
                &t_value,
                &env_after_value.free_vars(),
                &env_after_value.free_row_vars(),
            );
            let env_with_name = env_after_value.extend(name.clone(), scheme);
            let (s2, t_body, r_body) = infer(&env_with_name, eff_env, body, supply)?;
            let combined = s2.compose(&s1);
            let r_value_resolved = combined.apply_row(&r_value);
            let r_body_resolved = combined.apply_row(&r_body);
            let (row, s3) =
                row_union(&r_value_resolved, &r_body_resolved, &combined, expr.span, supply)?;
            Ok((s3, t_body, row))
        }
        ExprKind::LetRec { name, value, body } => {
            // B1.6: proper HM let-rec. The recursive occurrence inside
            // the RHS is monomorphic (sees t_name directly); the body
            // sees a generalized scheme so the binding is polymorphic
            // at use sites.
            let t_name = supply.fresh_ty();
            let env_for_value =
                env.extend(name.clone(), Scheme::mono(t_name.clone()));
            let (s1, t_value, r_value) = infer(&env_for_value, eff_env, value, supply)?;
            let s2 = unify(&s1, &t_name, &t_value, expr.span, supply)?;
            let env_after = env.apply(&s2);
            let t_name_solved = s2.apply(&t_name);
            let scheme = generalize(
                &t_name_solved,
                &env_after.free_vars(),
                &env_after.free_row_vars(),
            );
            let env_for_body = env_after.extend(name.clone(), scheme);
            let (s3, t_body, r_body) = infer(&env_for_body, eff_env, body, supply)?;
            // Per ADR 0005 D2/D4: parser enforces RHS-is-lambda so
            // r_value is the closure-*value* contribution (Empty). The
            // body's row contribution is what surfaces from LetRec.
            let combined = s3.compose(&s2);
            let r_value_resolved = combined.apply_row(&r_value);
            let r_body_resolved = combined.apply_row(&r_body);
            let (row, s4) =
                row_union(&r_value_resolved, &r_body_resolved, &combined, expr.span, supply)?;
            Ok((s4, t_body, row))
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            let (s1, t_cond, r_cond) = infer(env, eff_env, cond, supply)?;
            // ADR 0008 D3 SecretBranch: branching on a secret-typed
            // condition produces data-dependent timing on real
            // hardware. Reject before the Bool unify so the
            // diagnostic names the CT violation directly rather
            // than firing SecretFlow.
            if matches!(s1.apply(&t_cond), Ty::Secret(_)) {
                return Err(TypeError::SecretBranch { span: cond.span });
            }
            let s2 = unify(&s1, &t_cond, &Ty::Bool, cond.span, supply)?;
            let env_after = env.apply(&s2);
            let (s3, t_then, r_then) = infer(&env_after, eff_env, then_branch, supply)?;
            let s3 = s3.compose(&s2);
            let env_after = env.apply(&s3);
            let (s4, t_else, r_else) = infer(&env_after, eff_env, else_branch, supply)?;
            let s4 = s4.compose(&s3);
            let s5 = unify(&s4, &t_then, &t_else, expr.span, supply)?;
            let ty = s5.apply(&t_then);
            let r_cond_resolved = s5.apply_row(&r_cond);
            let r_then_resolved = s5.apply_row(&r_then);
            let r_else_resolved = s5.apply_row(&r_else);
            let (u1, s6) =
                row_union(&r_cond_resolved, &r_then_resolved, &s5, expr.span, supply)?;
            let (row, s7) =
                row_union(&u1, &r_else_resolved, &s6, expr.span, supply)?;
            Ok((s7, ty, row))
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let (s1, t_lhs, r_lhs) = infer(env, eff_env, lhs, supply)?;
            let env_after = env.apply(&s1);
            let (s2, t_rhs, r_rhs) = infer(&env_after, eff_env, rhs, supply)?;
            let s2 = s2.compose(&s1);

            // ADR 0008 D3 SecretDivisor: variable-time division on
            // hardware is the canonical CT footgun. Reject when the
            // divisor types as secret. Checked before unification so
            // the diagnostic is the dedicated SecretDivisor rather
            // than the generic SecretFlow from `unify(Secret(_), Int)`.
            // (Sentinel-Mini has Div but no Mod; ADR D3's `Div | Mod`
            // language is forward-looking.)
            if matches!(op, BinOp::Div)
                && matches!(s2.apply(&t_rhs), Ty::Secret(_))
            {
                return Err(TypeError::SecretDivisor { span: rhs.span });
            }

            // ADR 0008 D4: comparisons on secrets produce Secret<Bool>.
            // When either operand of Eq/Lt/Gt is `Ty::Secret(_)`,
            // unwrap that side to its inner type, unify the other
            // operand against the inner (the (Secret, Secret) arm of
            // `unify` handles the both-secret case naturally via the
            // same code path because both inners become non-Secret
            // here), and produce `Secret(Bool)` as the result type.
            // For Lt/Gt the inner type must additionally be Int per
            // the existing binop_signature; that unify fires after
            // the cross-side unify and produces a SecretFlow if the
            // inner isn't Int (or Mismatch if neither side was secret).
            let is_comparison = matches!(op, BinOp::Eq | BinOp::Lt | BinOp::Gt);
            let t_lhs_resolved = s2.apply(&t_lhs);
            let t_rhs_resolved = s2.apply(&t_rhs);
            let either_secret = matches!(t_lhs_resolved, Ty::Secret(_))
                || matches!(t_rhs_resolved, Ty::Secret(_));

            let (s_final, result_ty) = if is_comparison && either_secret {
                let inner_lhs = match t_lhs_resolved.clone() {
                    Ty::Secret(inner) => *inner,
                    other => other,
                };
                let inner_rhs = match t_rhs_resolved.clone() {
                    Ty::Secret(inner) => *inner,
                    other => other,
                };
                let s3 = unify(&s2, &inner_lhs, &inner_rhs, expr.span, supply)?;
                let s4 = if matches!(op, BinOp::Lt | BinOp::Gt) {
                    unify(&s3, &s3.apply(&inner_lhs), &Ty::Int, lhs.span, supply)?
                } else {
                    s3
                };
                (s4, Ty::secret(Ty::Bool))
            } else if matches!(op, BinOp::Eq) {
                let s = unify(&s2, &t_lhs, &t_rhs, expr.span, supply)?;
                (s, Ty::Bool)
            } else {
                let (expected_lhs, expected_rhs, result_ty) = binop_signature(*op);
                let s3 = unify(&s2, &t_lhs, &expected_lhs, lhs.span, supply)?;
                let s = unify(&s3, &t_rhs, &expected_rhs, rhs.span, supply)?;
                (s, result_ty)
            };

            let r_lhs_resolved = s_final.apply_row(&r_lhs);
            let r_rhs_resolved = s_final.apply_row(&r_rhs);
            let (row, s_done) =
                row_union(&r_lhs_resolved, &r_rhs_resolved, &s_final, expr.span, supply)?;
            Ok((s_done, result_ty, row))
        }
        // B2.3b2 (ADR 0005 D9): Perform looks up the label in the
        // program's effect environment. Unknown labels are a type
        // error; known labels infer the arg, unify it against the
        // declared arg type, and contribute a single-label Cons row.
        ExprKind::Perform { label, arg, label_span, .. } => {
            let (decl_arg, decl_ret) = match eff_env.get(label) {
                Some(pair) => pair.clone(),
                None => {
                    return Err(TypeError::UnknownEffect {
                        label: label.clone(),
                        span: *label_span,
                    });
                }
            };
            let (s_arg, t_arg, r_arg) = infer(env, eff_env, arg, supply)?;
            let s_unif = unify(&s_arg, &t_arg, &decl_arg, arg.span, supply)?;
            let r_arg_resolved = s_unif.apply_row(&r_arg);
            let perform_row = Row::Cons {
                label: label.clone(),
                arg: Box::new(decl_arg.clone()),
                ret: Box::new(decl_ret.clone()),
                tail: Box::new(Row::Empty),
            };
            let (row, s_final) =
                row_union(&r_arg_resolved, &perform_row, &s_unif, expr.span, supply)?;
            Ok((s_final, decl_ret, row))
        }
        // B3.1 (ADR 0007 D3): handler typing rule.
        //
        // Strategy:
        //   1. Infer body, get (t_body, r_body).
        //   2. Check duplicate arm labels.
        //   3. Peel each arm label out of r_body via row_split,
        //      threading substitution. Collect per-arm (arg_ty, ret_ty).
        //      Final residual is r_outer (the row of the handle expr,
        //      modulo additional effects performed by arm bodies).
        //   4. Mint t_result. Type each arm body under env extended
        //      with x : arg_ty and k : ret_ty -> t_result ! r_outer.
        //      Unify each arm-body type with t_result. Union each
        //      arm-body row into r_accumulated.
        //   5. Type the return arm (or default to identity: t_body =
        //      t_result). Union its row into r_accumulated.
        //   6. Return (s_final, t_result, r_accumulated).
        ExprKind::Handle { body, arms, ret_arm } => {
            let (s0, t_body, r_body) = infer(env, eff_env, body, supply)?;

            for i in 0..arms.len() {
                for j in (i + 1)..arms.len() {
                    if arms[i].label == arms[j].label {
                        return Err(TypeError::DuplicateHandlerArm {
                            label: arms[j].label.clone(),
                            span: arms[j].label_span,
                        });
                    }
                }
            }

            let mut s_acc = s0;
            let mut r_current = s_acc.apply_row(&r_body);
            let mut arm_sigs: Vec<(Ty, Ty)> = Vec::with_capacity(arms.len());
            for arm in arms {
                let (sig, residual, s_next) =
                    row_split(&s_acc, &r_current, &arm.label, arm.label_span, supply)?;
                arm_sigs.push(sig);
                r_current = s_next.apply_row(&residual);
                s_acc = s_next;
            }
            let r_outer_initial = r_current.clone();

            let t_result = supply.fresh_ty();
            let mut r_accumulated = r_outer_initial.clone();
            for (arm, (arg_ty, ret_ty)) in arms.iter().zip(arm_sigs.into_iter()) {
                let kont_ty = Ty::arrow_with(
                    ret_ty,
                    r_outer_initial.clone(),
                    t_result.clone(),
                );
                let env_arm = env
                    .apply(&s_acc)
                    .extend(arm.arg.clone(), Scheme::mono(arg_ty))
                    .extend(arm.kont.clone(), Scheme::mono(kont_ty));
                let (s_i, t_arm_body, r_arm_body) =
                    infer(&env_arm, eff_env, &arm.body, supply)?;
                s_acc = s_i;
                s_acc = unify(&s_acc, &t_arm_body, &t_result, arm.body.span, supply)?;
                let r_arm_body_resolved = s_acc.apply_row(&r_arm_body);
                let r_accumulated_resolved = s_acc.apply_row(&r_accumulated);
                let (r_union, s_next) = row_union(
                    &r_accumulated_resolved,
                    &r_arm_body_resolved,
                    &s_acc,
                    arm.body.span,
                    supply,
                )?;
                r_accumulated = r_union;
                s_acc = s_next;
            }

            match ret_arm {
                Some(ra) => {
                    let t_body_resolved = s_acc.apply(&t_body);
                    let env_ret = env
                        .apply(&s_acc)
                        .extend(ra.var.clone(), Scheme::mono(t_body_resolved));
                    let (s_r, t_ret_body, r_ret_body) =
                        infer(&env_ret, eff_env, &ra.body, supply)?;
                    s_acc = s_r;
                    s_acc = unify(&s_acc, &t_ret_body, &t_result, ra.body.span, supply)?;
                    let r_ret_resolved = s_acc.apply_row(&r_ret_body);
                    let r_accumulated_resolved = s_acc.apply_row(&r_accumulated);
                    let (r_union, s_next) = row_union(
                        &r_accumulated_resolved,
                        &r_ret_resolved,
                        &s_acc,
                        ra.body.span,
                        supply,
                    )?;
                    r_accumulated = r_union;
                    s_acc = s_next;
                }
                None => {
                    s_acc = unify(&s_acc, &t_body, &t_result, body.span, supply)?;
                }
            }

            let ty = s_acc.apply(&t_result);
            let row = s_acc.apply_row(&r_accumulated);
            Ok((s_acc, ty, row))
        }
        // ADR 0008 D5: declassify typing rule. `e : Secret<T> ⊢
        // declassify(e) : T`. Implemented by minting a fresh α and
        // unifying `t_inner` against `Ty::Secret(α)`; on success the
        // result type is `s.apply(α)` (the inner of the secret).
        // - inner is `Ty::Secret(t)`: Secret-Secret arm of `unify`
        //   recurses, binds α := t, result is t. ✓
        // - inner is a concrete non-secret type (Int/Bool/Fun): falls
        //   to the catch-all SecretFlow arm with from=non-secret,
        //   to=Secret(α). The diagnostic reads "cannot flow Int into
        //   secret '_a".
        // - inner is a bare `Ty::Var(β)`: Var arm of unify hits D2
        //   and returns SecretEscapesPolymorphism. The diagnostic
        //   reads "secret escapes polymorphism: '_b" -- consistent
        //   with the polymorphism rule, slightly indirect for the
        //   declassify-specific failure but correct per D2.
        // The arg-row r_inner threads through unchanged: declassify
        // performs no effects beyond what its argument performs.
        ExprKind::Declassify { inner, span } => {
            let (s1, t_inner, r_inner) = infer(env, eff_env, inner, supply)?;
            let inner_var = supply.fresh_ty();
            let expected = Ty::secret(inner_var.clone());
            let s2 = unify(&s1, &t_inner, &expected, *span, supply)?;
            let result_ty = s2.apply(&inner_var);
            Ok((s2, result_ty, r_inner))
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
    let eff_env: EffectEnv = HashMap::new();
    let (s, t, r) = infer(&TypeEnv::empty(), &eff_env, expr, &mut supply)?;
    // B2.3a (ADR 0005 D6): strict-residual-row check. In B2.3a every
    // arm returns Row::Empty, so `resolved` is always Row::Empty and
    // this branch is unreachable. The variant + check land inert here
    // to keep B2.3b's diff scoped to semantics only (see commit body).
    let resolved = s.apply_row(&r).close();
    if !matches!(resolved, Row::Empty) {
        return Err(TypeError::UnhandledEffects { row: resolved, span: expr.span });
    }
    // B2.3b1 (ADR 0006 D1/D2): default-close after the residual check.
    Ok(s.apply(&t).close_rows())
}

/// B2.2b: program-level inference entry point.
///
/// In B2.2 the `effects` field of [`crate::ast::Program`] is
/// inert: declarations are accepted by the parser but contribute
/// nothing to the typing environment. B2.3 will thread declared
/// labels through an environment so `do Label(arg)` infers a
/// proper effect row, and handlers in B3 will discharge them.
pub fn infer_program(prog: &crate::ast::Program) -> Result<Ty, TypeError> {
    // B2.3b2 (ADR 0005 D9): populate the EffectEnv from prog.effects
    // so `do Label(arg)` resolves against declared labels. Residual
    // row is still discarded (D6: infer_program is permissive at B2
    // scope; handlers in B3 will discharge rows for real).
    //
    // B4.1b: ADR 0008 D7 -- effect signatures may mention `secret`.
    // The B4.0a/B4.1a placeholder walker that rejected such decls is
    // gone now that D2 (no-α-leak), D3 (SecretBranch/SecretDivisor),
    // and D4 (Secret<Bool> comparisons) collectively ensure
    // secret-typed values reaching inference via `do L(arg)` have
    // nowhere unsafe to flow.
    let mut supply = TyVarSupply::new();
    let mut eff_env: EffectEnv = HashMap::new();
    for decl in &prog.effects {
        eff_env.insert(
            decl.label.clone(),
            (decl.arg.to_ty(), decl.ret.to_ty()),
        );
    }
    let (s, t, _r) = infer(&TypeEnv::empty(), &eff_env, &prog.body, &mut supply)?;
    // B2.3b1 (ADR 0006 D3): same default-close policy as infer_top.
    Ok(s.apply(&t).close_rows())
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

    fn union_r(a: &Row, b: &Row) -> Result<Row, TypeError> {
        let mut supply = TyVarSupply::new();
        let (r, _s) = row_union(a, b, &Subst::empty(), sp(), &mut supply)?;
        Ok(r)
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
        let scheme = Scheme { vars: vec![TyVar(0)], row_vars: Vec::new(), ty: Ty::arrow(v(0), v(1)) };
        assert_eq!(s.apply_scheme(&scheme), scheme);
        let s = Subst::singleton(TyVar(1), Ty::Bool);
        let after = s.apply_scheme(&scheme);
        assert_eq!(after, Scheme { vars: vec![TyVar(0)], row_vars: Vec::new(), ty: Ty::arrow(v(0), Ty::Bool) });
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
            row_vars: Vec::new(),
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
        let scheme = generalize(&ty, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(scheme.vars.len(), 2);
        assert!(scheme.vars.contains(&TyVar(0)));
        assert!(scheme.vars.contains(&TyVar(1)));
    }

    #[test]
    fn generalize_skips_env_free_vars() {
        let ty = Ty::arrow(v(0), v(1));
        let mut env_free = BTreeSet::new();
        env_free.insert(TyVar(0));
        let scheme = generalize(&ty, &env_free, &BTreeSet::new());
        assert_eq!(scheme.vars, vec![TyVar(1)]);
    }

    // ---- B2.3b2-b: row generalization ----

    #[test]
    fn b23b2b_generalize_quantifies_free_row_vars() {
        // Type 'a -> 'a with row 'ra on the arrow.
        let ty = Ty::arrow_with(v(0), rv(0), v(0));
        let scheme = generalize(&ty, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(scheme.vars, vec![TyVar(0)]);
        assert_eq!(scheme.row_vars, vec![RowVar(0)]);
    }

    #[test]
    fn b23b2b_generalize_skips_env_free_row_vars() {
        let ty = Ty::arrow_with(v(0), rv(0), v(0));
        let mut env_free_rows = BTreeSet::new();
        env_free_rows.insert(RowVar(0));
        let scheme = generalize(&ty, &BTreeSet::new(), &env_free_rows);
        assert!(scheme.row_vars.is_empty(),
            "row var bound in env must not be quantified");
    }

    #[test]
    fn b23b2b_instantiate_freshens_row_vars() {
        let scheme = Scheme {
            vars: Vec::new(),
            row_vars: vec![RowVar(99)],
            ty: Ty::arrow_with(Ty::Int, Row::Var(RowVar(99)), Ty::Int),
        };
        let mut supply = TyVarSupply::new();
        let t1 = instantiate(&scheme, &mut supply);
        let t2 = instantiate(&scheme, &mut supply);
        // Two instantiations must produce distinct fresh row vars.
        match (&t1, &t2) {
            (Ty::Fun(_, r1, _), Ty::Fun(_, r2, _)) => {
                assert!(matches!(r1, Row::Var(_)));
                assert!(matches!(r2, Row::Var(_)));
                assert_ne!(r1, r2, "fresh row vars must differ");
            }
            _ => panic!("expected two arrows"),
        }
    }

    #[test]
    fn b23b2b_let_bound_identity_polymorphic_at_two_types() {
        // Regression: row generalization must not break ordinary
        // type-polymorphic let-binding behaviour.
        let src = "let id = fn(x) => x in if id(true) then id(1) else id(2)";
        assert_eq!(ty_of(src), Ty::Int);
    }

    #[test]
    fn b23b2b_let_bound_lambda_used_under_perform() {
        // The let-bound function has a row-polymorphic arrow scheme;
        // calling it inside an effectful context still type-checks.
        let src = "effect Print : Int -> Bool ;                    let id = fn(x) => x in                    do Print(id(1))";
        let toks = crate::lexer::lex(src).unwrap();
        let prog = crate::parser::parse_program(&toks).unwrap();
        assert_eq!(infer_program(&prog).unwrap(), Ty::Bool);
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
        let eff_env: EffectEnv = HashMap::new();
        let mut supply = TyVarSupply::new();
        infer(&env, &eff_env, &expr, &mut supply).map(|(s, t, _r)| s.apply(&t))
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
    // ---- B2.2b: Perform inference rejection ----

    // ---- B2.3b2: Perform inference (replaces B2.2b placeholders) ----

    #[test]
    fn b23b2_perform_undeclared_label_is_unknown_effect() {
        let toks = crate::lexer::lex("do Print(1)").unwrap();
        let expr = crate::parser::parse(&toks).unwrap();
        let err = infer_top(&expr).expect_err("undeclared Print should fail");
        match err {
            TypeError::UnknownEffect { label, .. } => assert_eq!(label, "Print"),
            other => panic!("expected UnknownEffect, got {other:?}"),
        }
    }

    #[test]
    fn b23b2_unknown_effect_span_targets_label_not_do_keyword() {
        let src = "do Print(1)";
        let toks = crate::lexer::lex(src).unwrap();
        let expr = crate::parser::parse(&toks).unwrap();
        let err = infer_top(&expr).expect_err("should fail");
        match err {
            TypeError::UnknownEffect { span, .. } => {
                assert_eq!(&src[span.start as usize .. span.end as usize], "Print");
            }
            other => panic!("expected UnknownEffect, got {other:?}"),
        }
    }

    fn infer_prog(src: &str) -> Result<Ty, TypeError> {
        let toks = crate::lexer::lex(src).unwrap();
        let prog = crate::parser::parse_program(&toks).unwrap();
        infer_program(&prog)
    }

    #[test]
    fn b23b2_single_perform_with_declared_label_infers_ret_ty() {
        let src = "effect Print : Int -> Bool ; do Print(1)";
        assert_eq!(infer_prog(src).unwrap(), Ty::Bool);
    }

    #[test]
    fn b23b2_perform_in_let_body_infers_ret_ty() {
        let src = "effect Ask : Int -> Int ; let x = 1 in do Ask(x)";
        assert_eq!(infer_prog(src).unwrap(), Ty::Int);
    }

    #[test]
    fn b23b2_perform_inside_lambda_infers_arrow() {
        // The lambda is pure-typed at B2 (default-close); we just
        // assert it type-checks and yields an Int -> Bool shape.
        let src = "effect Print : Int -> Bool ; fn(x) => do Print(x)";
        let t = infer_prog(src).unwrap();
        match t {
            Ty::Fun(arg, _row, ret) => {
                assert_eq!(*arg, Ty::Int);
                assert_eq!(*ret, Ty::Bool);
            }
            other => panic!("expected Fun, got {other:?}"),
        }
    }

    #[test]
    fn b23b2_perform_arg_type_mismatch_is_type_error() {
        // Print declared Int -> Bool; passing `true` is a mismatch.
        let src = "effect Print : Int -> Bool ; do Print(true)";
        let err = infer_prog(src).expect_err("Bool != Int should fail");
        assert!(
            matches!(err, TypeError::Mismatch { .. }),
            "expected Mismatch, got {err:?}"
        );
    }

    #[test]
    fn b23b2_two_performs_unioned_in_binop_typecheck() {
        // Both perform Int-returning effects; the sum is Int.
        let src = "effect A : Int -> Int ; effect B : Int -> Int ; do A(1) + do B(2)";
        assert_eq!(infer_prog(src).unwrap(), Ty::Int);
    }

    #[test]
    fn b23b2_perform_then_pure_body_typechecks() {
        // Sequenced via let; binding has effects, body is pure.
        let src = "effect Print : Int -> Bool ; let _b = do Print(1) in 42";
        assert_eq!(infer_prog(src).unwrap(), Ty::Int);
    }

    #[test]
    fn b23b2_unknown_label_in_program_with_other_decls() {
        // `Ask` is declared, `Print` is not.
        let src = "effect Ask : Int -> Int ; do Print(1)";
        let err = infer_prog(src).expect_err("Print is undeclared");
        match err {
            TypeError::UnknownEffect { label, .. } => assert_eq!(label, "Print"),
            other => panic!("expected UnknownEffect, got {other:?}"),
        }
    }


    #[test]
    fn b23b2_perform_with_bool_arg_type_declared() {
        let src = "effect Log : Bool -> Int ; do Log(true)";
        assert_eq!(infer_prog(src).unwrap(), Ty::Int);
    }

    // ---------- B2.3b1: row_union unit tests ----------

    #[test]
    fn b23b1_row_union_empty_empty_is_empty() {
        assert_eq!(union_r(&Row::Empty, &Row::Empty).unwrap(), Row::Empty);
    }

    #[test]
    fn b23b1_row_union_empty_with_cons_is_cons() {
        let r = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        assert_eq!(union_r(&Row::Empty, &r).unwrap(), r);
        let r = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        assert_eq!(union_r(&r, &Row::Empty).unwrap(), r);
    }

    #[test]
    fn b23b1_row_union_two_distinct_labels_contains_both() {
        let a = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let b = cons("Ask", Ty::Int, Ty::Int, Row::Empty);
        let u = union_r(&a, &b).unwrap();
        let mut labels = Vec::new();
        let mut cur = &u;
        while let Row::Cons { label, tail, .. } = cur {
            labels.push(label.clone());
            cur = tail.as_ref();
        }
        labels.sort();
        assert_eq!(labels, vec!["Ask".to_string(), "Print".to_string()]);
    }

    #[test]
    fn b23b1_row_union_same_label_same_signature_is_deduped() {
        let a = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let b = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let u = union_r(&a, &b).unwrap();
        // Exactly one Print label, no duplicate.
        let mut count = 0;
        let mut cur = &u;
        while let Row::Cons { label, tail, .. } = cur {
            if label == "Print" { count += 1; }
            cur = tail.as_ref();
        }
        assert_eq!(count, 1, "Print should appear exactly once after union; got row {u}");
    }

    #[test]
    fn b23b1_row_union_same_label_conflicting_signature_is_mismatch() {
        let a = cons("Print", Ty::Int, Ty::Bool, Row::Empty);
        let b = cons("Print", Ty::Bool, Ty::Bool, Row::Empty);
        let err = union_r(&a, &b).unwrap_err();
        assert!(matches!(err, TypeError::Mismatch { .. }),
            "expected Mismatch on conflicting Print arg type, got {err:?}");
    }

    // ---------- B2.3b1: lambda mints row var; default-close at top ----------

    #[test]
    fn b23b1_infer_top_default_closes_unconstrained_row_var() {
        // After close_rows, fn(x) => x has type 'a -> 'a (empty row).
        let ty = ty_of("fn(x) => x");
        match ty {
            Ty::Fun(a, row, b) => {
                assert_eq!(*a, *b, "identity: arg == ret");
                assert_eq!(row, Row::Empty, "default-close should erase free row var");
            }
            other => panic!("expected arrow, got {other}"),
        }
    }

    #[test]
    fn b23b1_infer_program_default_closes_too() {
        use crate::parser::parse_program;
        // A program with no effects, body is a pure lambda. After
        // infer_program, the returned type should have Row::Empty
        // (not a free row var) on its arrow.
        let toks = crate::lexer::lex("fn(x) => x").unwrap();
        let prog = parse_program(&toks).unwrap();
        let ty = infer_program(&prog).unwrap();
        match ty {
            Ty::Fun(_, row, _) => assert_eq!(row, Row::Empty,
                "infer_program should also default-close per ADR 0006 D3"),
            other => panic!("expected arrow, got {other}"),
        }
    }

    #[test]
    fn b22b_program_with_effect_decl_and_pure_body_infers() {
        use crate::parser::parse_program;
        let toks = crate::lexer::lex("effect Print : Int -> Bool ; 1 + 2").unwrap();
        let prog = parse_program(&toks).unwrap();
        let ty = infer_program(&prog).expect("pure body must infer");
        assert_eq!(ty, Ty::Int);
    }
    #[test]
    fn b31_row_split_at_head_returns_signature_and_tail() {
        let mut supply = TyVarSupply::new();
        let row = Row::Cons {
            label: "Get".to_string(),
            arg: Box::new(Ty::Int),
            ret: Box::new(Ty::Bool),
            tail: Box::new(Row::Empty),
        };
        let (sig, residual, _s) =
            row_split(&Subst::empty(), &row, "Get", Span::point(0), &mut supply).unwrap();
        assert_eq!(sig.0, Ty::Int);
        assert_eq!(sig.1, Ty::Bool);
        assert_eq!(residual, Row::Empty);
    }

    #[test]
    fn b31_row_split_deeper_reconstructs_head() {
        let mut supply = TyVarSupply::new();
        let row = Row::Cons {
            label: "Put".to_string(),
            arg: Box::new(Ty::Int),
            ret: Box::new(Ty::Bool),
            tail: Box::new(Row::Cons {
                label: "Get".to_string(),
                arg: Box::new(Ty::Bool),
                ret: Box::new(Ty::Int),
                tail: Box::new(Row::Empty),
            }),
        };
        let (sig, residual, _s) =
            row_split(&Subst::empty(), &row, "Get", Span::point(0), &mut supply).unwrap();
        assert_eq!(sig.0, Ty::Bool);
        assert_eq!(sig.1, Ty::Int);
        // Put must still be present in the residual.
        match residual {
            Row::Cons { label, tail, .. } => {
                assert_eq!(label, "Put");
                assert_eq!(*tail, Row::Empty);
            }
            other => panic!("expected Put in residual, got {other:?}"),
        }
    }

    #[test]
    fn b31_row_split_on_var_mints_fresh_signature_and_tail() {
        let mut supply = TyVarSupply::new();
        let v = supply.fresh_row_var();
        let row = Row::Var(v);
        let (sig, residual, s1) =
            row_split(&Subst::empty(), &row, "Get", Span::point(0), &mut supply).unwrap();
        // Signature components are fresh type vars.
        assert!(matches!(sig.0, Ty::Var(_)));
        assert!(matches!(sig.1, Ty::Var(_)));
        // Residual is a fresh row var, different from the original.
        match residual {
            Row::Var(rv) => assert_ne!(rv, v),
            other => panic!("expected fresh Row::Var, got {other:?}"),
        }
        // The original var is now substituted to a Cons headed by Get.
        let resolved = s1.apply_row(&Row::Var(v));
        match resolved {
            Row::Cons { label, .. } => assert_eq!(label, "Get"),
            other => panic!("expected Cons binding for v, got {other:?}"),
        }
    }

    #[test]
    fn b31_row_split_on_empty_is_handler_label_not_in_row() {
        let mut supply = TyVarSupply::new();
        let err = row_split(
            &Subst::empty(),
            &Row::Empty,
            "Get",
            Span::point(0),
            &mut supply,
        )
        .unwrap_err();
        match err {
            TypeError::HandlerLabelNotInRow { label, .. } => assert_eq!(label, "Get"),
            other => panic!("expected HandlerLabelNotInRow, got {other:?}"),
        }
    }
    fn type_program(src: &str) -> Result<Ty, TypeError> {
        let toks = crate::lexer::lex(src).expect("lex");
        let prog = crate::parser::parse_program(&toks).expect("parse");
        infer_program(&prog)
    }

    #[test]
    fn b31b_handle_identity_discharges_effect() {
        // Body performs Get; handler discharges it via k(x). Result
        // type is the declared return type of Get.
        let src = "effect Get : Int -> Int ; handle do Get(1) with { Get(x, k) => k(x) }";
        let ty = type_program(src).expect("should type-check");
        assert_eq!(ty, Ty::Int);
    }

    #[test]
    fn b31b_handle_two_arms_discharges_both() {
        let src = "\
            effect Get : Int -> Int ; \
            effect Put : Int -> Int ; \
            handle do Get(1) + do Put(2) with { \
                Get(x, k) => k(x), \
                Put(y, k) => k(y) \
            }";
        let ty = type_program(src).expect("should type-check");
        assert_eq!(ty, Ty::Int);
    }

    #[test]
    fn b31b_handle_return_arm_transforms_result_type() {
        // Body returns Int (from Get); return arm wraps it into Bool
        // via a comparison. Result type should be Bool, not Int.
        let src = "\
            effect Get : Int -> Int ; \
            handle do Get(1) with { \
                Get(x, k) => k(x), \
                return v => v == 0 \
            }";
        let ty = type_program(src).expect("should type-check");
        assert_eq!(ty, Ty::Bool);
    }

    #[test]
    fn b31b_handle_missing_label_in_row_is_error() {
        // Body has only Get; handler arm names Put. row_split should
        // error with HandlerLabelNotInRow on Put.
        let src = "\
            effect Get : Int -> Int ; \
            effect Put : Int -> Int ; \
            handle do Get(1) with { Put(y, k) => k(y) }";
        let err = type_program(src).expect_err("Put not in body's row");
        match err {
            TypeError::HandlerLabelNotInRow { label, .. } => {
                assert_eq!(label, "Put");
            }
            other => panic!("expected HandlerLabelNotInRow, got {other:?}"),
        }
    }

    #[test]
    fn b31b_handle_duplicate_arm_is_error() {
        let src = "\
            effect Get : Int -> Int ; \
            handle do Get(1) with { Get(x, k) => k(x), Get(y, k) => k(y) }";
        let err = type_program(src).expect_err("duplicate Get arm");
        match err {
            TypeError::DuplicateHandlerArm { label, .. } => {
                assert_eq!(label, "Get");
            }
            other => panic!("expected DuplicateHandlerArm, got {other:?}"),
        }
    }

    #[test]
    fn b31b_handle_arm_body_can_perform_residual_effect() {
        // Handler discharges Get but the arm body performs Put. The
        // resulting handle expression's row should contain Put.
        let src = "\
            effect Get : Int -> Int ; \
            effect Put : Int -> Int ; \
            handle do Get(1) with { Get(x, k) => do Put(x) }";
        // We can't easily inspect the row from infer_program (which
        // closes rows per ADR 0006 D6). What we can assert is that
        // this type-checks at all, i.e. the arm body's effect doesn't
        // confuse the rule.
        let ty = type_program(src).expect("should type-check");
        assert_eq!(ty, Ty::Int);
    }

    #[test]
    fn b31b_handle_arm_body_type_must_match_other_arms() {
        // Two arms with different body types should be rejected by
        // unification: arm 1 returns k(x): Int; arm 2 returns 'true':
        // Bool. Both must equal t_result, so one fails.
        let src = "\
            effect Get : Int -> Int ; \
            effect Put : Int -> Int ; \
            handle do Get(1) + do Put(2) with { \
                Get(x, k) => k(x), \
                Put(y, k) => true \
            }";
        let err = type_program(src).expect_err("arm body type mismatch");
        match err {
            TypeError::Mismatch { .. } => {}
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    // ---- B4.1a (ADR 0008 D2/D5): real Declassify typing + D2 no-α-leak ----
    //
    // B4.0a's three `b40a_*` placeholder tests are revised here:
    // the Declassify-expression test moves to `b41a_*` and asserts
    // SecretFlow (the real D5 typing rule's reaction to a non-secret
    // inner). The two effect-decl tests below still assert
    // SecretsNotYetSupported because B4.1a leaves the
    // `infer_program` effect-decl walker in place; B4.1b removes
    // that walker once D3/D4 land.
    //
    // Synthetic AST is still used because the parser-level surface
    // (B4.0c) accepts the keywords but B4.1a inference has no path
    // from source into a `Ty::Secret(_)` -- the effect-decl walker
    // is the only public producer. Synthetic envs cover the typing
    // rule directly.

    #[test]
    fn b41a_declassify_on_non_secret_is_secret_flow() {
        // B4.0a placeholder is gone for Declassify; the real D5 rule
        // unifies the inner against `Secret(α)`. On `declassify(1)`,
        // `1 : Int` cannot unify with `Secret(α)` and the catch-all
        // SecretFlow arm fires.
        use crate::ast::{expr, ExprKind};
        let inner = expr(ExprKind::Int(1), sp());
        let de = expr(
            ExprKind::Declassify { inner: Box::new(inner), span: sp() },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty();
        let eff_env: EffectEnv = HashMap::new();
        let err = infer(&env, &eff_env, &de, &mut supply).expect_err("declassify on Int");
        match err {
            TypeError::SecretFlow { from, .. } => {
                assert_eq!(from, Ty::Int, "from-type should be the non-secret Int");
            }
            other => panic!("expected SecretFlow, got {other:?}"),
        }
    }

    #[test]
    fn b41a_declassify_on_secret_unwraps_the_inner_type() {
        // Positive D5 case. Synthetic env binds `k : Secret<Int>`;
        // declassify(k) types as Int.
        use crate::ast::{expr, ExprKind};
        let k_ref = expr(ExprKind::Var("k".into()), sp());
        let de = expr(
            ExprKind::Declassify { inner: Box::new(k_ref), span: sp() },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("k".to_string(), Scheme::mono(Ty::secret(Ty::Int)));
        let eff_env: EffectEnv = HashMap::new();
        let (_s, t, _r) = infer(&env, &eff_env, &de, &mut supply).expect("declassify k");
        assert_eq!(t, Ty::Int);
    }

    #[test]
    fn b41a_unify_var_vs_secret_is_secret_escapes_polymorphism() {
        // D2 no-α-leak. unify(Var(α), Secret(Int)) must reject; the
        // symmetric direction is handled by the same Var arm via the
        // or-pattern.
        let s = Subst::empty();
        let mut supply = TyVarSupply::new();
        let err = unify(&s, &v(0), &Ty::secret(Ty::Int), sp(), &mut supply)
            .expect_err("Var-vs-Secret");
        match err {
            TypeError::SecretEscapesPolymorphism { var, .. } => {
                assert_eq!(var, TyVar(0));
            }
            other => panic!("expected SecretEscapesPolymorphism, got {other:?}"),
        }
    }

    #[test]
    fn b41a_unify_secret_vs_non_secret_is_secret_flow() {
        // Direct unify test for the catch-all SecretFlow split.
        // unify(Int, Secret(Int)) is non-Var-vs-Secret, catch-all
        // fires SecretFlow rather than the generic Mismatch.
        let s = Subst::empty();
        let mut supply = TyVarSupply::new();
        let err = unify(&s, &Ty::Int, &Ty::secret(Ty::Int), sp(), &mut supply)
            .expect_err("Int-vs-Secret(Int)");
        match err {
            TypeError::SecretFlow { from, to, .. } => {
                assert_eq!(from, Ty::Int);
                assert_eq!(to, Ty::secret(Ty::Int));
            }
            other => panic!("expected SecretFlow, got {other:?}"),
        }
    }

    #[test]
    fn b41a_unify_secret_vs_secret_recurses_on_inner() {
        // Sanity: the (Secret, Secret) arm still works after the
        // SecretFlow upgrade. unify(Secret(Int), Secret(Int)) succeeds.
        let s = Subst::empty();
        let mut supply = TyVarSupply::new();
        let result = unify(&s, &Ty::secret(Ty::Int), &Ty::secret(Ty::Int), sp(), &mut supply);
        assert!(result.is_ok(), "Secret(Int) ~ Secret(Int) should unify: {result:?}");
    }

    #[test]
    fn b41a_unify_secret_int_vs_secret_bool_is_mismatch_on_inner() {
        // unify(Secret(Int), Secret(Bool)) -- the (Secret, Secret) arm
        // recurses to unify(Int, Bool), which falls to the catch-all
        // Mismatch arm (neither side is Secret at that point).
        let s = Subst::empty();
        let mut supply = TyVarSupply::new();
        let err = unify(&s, &Ty::secret(Ty::Int), &Ty::secret(Ty::Bool), sp(), &mut supply)
            .expect_err("Secret(Int) vs Secret(Bool)");
        match err {
            TypeError::Mismatch { expected, found, .. } => {
                assert_eq!(expected, Ty::Int);
                assert_eq!(found, Ty::Bool);
            }
            other => panic!("expected Mismatch on inner types, got {other:?}"),
        }
    }

    // ---- B4.1b: effect-decls with `secret` now accepted (ADR 0008 D7) ----
    //
    // The B4.0a `b40a_effect_decl_with_secret_*` placeholder tests
    // are rewritten here as positive tests now that B4.1b removed
    // the `infer_program` effect-decl walker. The placeholder was
    // safe to drop because D2 (no-α-leak), D3 (SecretBranch /
    // SecretDivisor), and D4 (Secret<Bool> comparisons) collectively
    // ensure secret-typed values produced by `do L(arg)` have
    // nowhere unsafe to flow downstream.

    #[test]
    fn b41b_effect_decl_with_secret_ret_now_type_checks() {
        use crate::ast::{expr, EffectDecl, ExprKind, Program, TyExpr};
        let prog = Program {
            effects: vec![EffectDecl {
                label: "ReadKey".to_string(),
                label_span: sp(),
                arg: TyExpr::Int(sp()),
                ret: TyExpr::Secret(Box::new(TyExpr::Int(sp())), sp()),
                span: sp(),
            }],
            body: expr(ExprKind::Int(0), sp()),
        };
        let ty = infer_program(&prog).expect("effect-decl with secret ret should type-check");
        assert_eq!(ty, Ty::Int);
    }

    #[test]
    fn b41b_effect_decl_with_secret_arg_now_type_checks() {
        use crate::ast::{expr, EffectDecl, ExprKind, Program, TyExpr};
        let prog = Program {
            effects: vec![EffectDecl {
                label: "Sign".to_string(),
                label_span: sp(),
                arg: TyExpr::Secret(Box::new(TyExpr::Int(sp())), sp()),
                ret: TyExpr::Int(sp()),
                span: sp(),
            }],
            body: expr(ExprKind::Int(0), sp()),
        };
        let ty = infer_program(&prog).expect("effect-decl with secret arg should type-check");
        assert_eq!(ty, Ty::Int);
    }

    // ---- B4.1b: D3 SecretBranch / SecretDivisor + D4 comparisons ----

    #[test]
    fn b41b_if_on_secret_bool_is_secret_branch() {
        // Synthetic env: cond : Secret<Bool>. `if cond then 1 else 2`
        // must reject with SecretBranch -- the dedicated diagnostic
        // for D3's CT violation, not the generic SecretFlow.
        use crate::ast::{expr, ExprKind};
        let cond = expr(ExprKind::Var("c".into()), sp());
        let then_b = expr(ExprKind::Int(1), sp());
        let else_b = expr(ExprKind::Int(2), sp());
        let if_expr = expr(
            ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_b),
                else_branch: Box::new(else_b),
            },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("c".to_string(), Scheme::mono(Ty::secret(Ty::Bool)));
        let eff_env: EffectEnv = HashMap::new();
        let err = infer(&env, &eff_env, &if_expr, &mut supply).expect_err("if on secret");
        match err {
            TypeError::SecretBranch { .. } => {}
            other => panic!("expected SecretBranch, got {other:?}"),
        }
    }

    #[test]
    fn b41b_div_by_secret_is_secret_divisor() {
        // Synthetic env: d : Secret<Int>. `1 / d` must reject with
        // SecretDivisor (the canonical CT footgun).
        use crate::ast::{expr, BinOp, ExprKind};
        let lhs = expr(ExprKind::Int(1), sp());
        let rhs = expr(ExprKind::Var("d".into()), sp());
        let div = expr(
            ExprKind::BinOp {
                op: BinOp::Div,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("d".to_string(), Scheme::mono(Ty::secret(Ty::Int)));
        let eff_env: EffectEnv = HashMap::new();
        let err = infer(&env, &eff_env, &div, &mut supply).expect_err("div by secret");
        match err {
            TypeError::SecretDivisor { .. } => {}
            other => panic!("expected SecretDivisor, got {other:?}"),
        }
    }

    #[test]
    fn b41b_eq_on_two_secrets_produces_secret_bool() {
        // D4: `a == b` where both are Secret<Int> produces Secret<Bool>.
        use crate::ast::{expr, BinOp, ExprKind};
        let lhs = expr(ExprKind::Var("a".into()), sp());
        let rhs = expr(ExprKind::Var("b".into()), sp());
        let eq = expr(
            ExprKind::BinOp { op: BinOp::Eq, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("a".to_string(), Scheme::mono(Ty::secret(Ty::Int)))
            .extend("b".to_string(), Scheme::mono(Ty::secret(Ty::Int)));
        let eff_env: EffectEnv = HashMap::new();
        let (_s, t, _r) = infer(&env, &eff_env, &eq, &mut supply).expect("a == b");
        assert_eq!(t, Ty::secret(Ty::Bool));
    }

    #[test]
    fn b41b_eq_secret_vs_public_int_produces_secret_bool() {
        // D4: one side secret, the other plain Int, still works -- the
        // rule unifies the plain Int against the inner (Int) and the
        // result is Secret<Bool>. This is the password-verify shape.
        use crate::ast::{expr, BinOp, ExprKind};
        let lhs = expr(ExprKind::Var("stored".into()), sp());
        let rhs = expr(ExprKind::Int(42), sp());
        let eq = expr(
            ExprKind::BinOp { op: BinOp::Eq, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("stored".to_string(), Scheme::mono(Ty::secret(Ty::Int)));
        let eff_env: EffectEnv = HashMap::new();
        let (_s, t, _r) = infer(&env, &eff_env, &eq, &mut supply).expect("stored == 42");
        assert_eq!(t, Ty::secret(Ty::Bool));
    }

    #[test]
    fn b41b_lt_on_secrets_produces_secret_bool() {
        // D4 + Lt: both sides Secret<Int>, result is Secret<Bool>.
        use crate::ast::{expr, BinOp, ExprKind};
        let lhs = expr(ExprKind::Var("a".into()), sp());
        let rhs = expr(ExprKind::Var("b".into()), sp());
        let lt = expr(
            ExprKind::BinOp { op: BinOp::Lt, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("a".to_string(), Scheme::mono(Ty::secret(Ty::Int)))
            .extend("b".to_string(), Scheme::mono(Ty::secret(Ty::Int)));
        let eff_env: EffectEnv = HashMap::new();
        let (_s, t, _r) = infer(&env, &eff_env, &lt, &mut supply).expect("a < b");
        assert_eq!(t, Ty::secret(Ty::Bool));
    }

    #[test]
    fn b41b_lt_on_secret_bool_rejects_at_inner_unify() {
        // D4 + Lt: Secret<Bool> < Secret<Bool> unwraps to (Bool, Bool),
        // then the Lt's inner-must-be-Int rule rejects with Mismatch
        // (Bool meets Int — neither is Secret at that point).
        use crate::ast::{expr, BinOp, ExprKind};
        let lhs = expr(ExprKind::Var("p".into()), sp());
        let rhs = expr(ExprKind::Var("q".into()), sp());
        let lt = expr(
            ExprKind::BinOp { op: BinOp::Lt, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            sp(),
        );
        let mut supply = TyVarSupply::new();
        let env = TypeEnv::empty()
            .extend("p".to_string(), Scheme::mono(Ty::secret(Ty::Bool)))
            .extend("q".to_string(), Scheme::mono(Ty::secret(Ty::Bool)));
        let eff_env: EffectEnv = HashMap::new();
        let err = infer(&env, &eff_env, &lt, &mut supply).expect_err("Lt on secret bools");
        match err {
            TypeError::Mismatch { .. } => {}
            other => panic!("expected Mismatch on inner Bool-vs-Int, got {other:?}"),
        }
    }
}
