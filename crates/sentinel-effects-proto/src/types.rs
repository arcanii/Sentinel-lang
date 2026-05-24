//! Type representation for Sentinel-Mini.
//!
//! Per ADR 0002 and ADR 0004, B2 widens `Ty::Fun` with a row
//! component and introduces `Row`/`RowVar` as a distinct kind.
//!
//! B2.0 is behaviour-preserving: every arrow gets `Row::Empty`,
//! row unification is a stub that only handles the empty-vs-empty
//! case. B2.1 fills in real row unification.
//!
//! # Identifiers
//!
//! Type variables are dense `u32` ids managed by [`TyVarSupply`] (in
//! `infer.rs`). Row variables use the same supply but a separate
//! counter, so a TyVar and a RowVar with the same numeric id are
//! different entities.

use std::collections::BTreeSet;
use std::fmt;

/// A type variable identifier. Dense and cheap to copy/hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TyVar(pub u32);

impl fmt::Display for TyVar {
    /// Pretty-print as `'a`, `'b`, ... cycling through letters and then
    /// `'a1`, `'b1`, ... for higher indices.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        let letter = (b'a' + (n % 26) as u8) as char;
        let cycle = n / 26;
        if cycle == 0 {
            write!(f, "'{letter}")
        } else {
            write!(f, "'{letter}{cycle}")
        }
    }
}

/// A row variable identifier. Parallel to [`TyVar`] but a distinct
/// kind: a `Subst` cannot bind a `TyVar` to a row or a `RowVar` to a
/// non-row monotype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowVar(pub u32);

impl fmt::Display for RowVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Distinguish from TyVar in diagnostics by prefixing 'r:
        // 'ra, 'rb, ..., 'ra1, ...
        let n = self.0;
        let letter = (b'a' + (n % 26) as u8) as char;
        let cycle = n / 26;
        if cycle == 0 {
            write!(f, "'r{letter}")
        } else {
            write!(f, "'r{letter}{cycle}")
        }
    }
}

/// An effect row.
///
/// ADR 0004 D1: `Cons` carries the effect signature alongside the
/// label so unification does not need an external lookup table.
///
/// B2.0: only `Empty` is ever constructed. `Var` and `Cons` exist
/// so the representation is complete, but infer always passes
/// `Row::Empty` into arrow constructors. B2.1/B2.3 start using the
/// other variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Empty,
    Var(RowVar),
    Cons {
        label: String,
        arg: Box<Ty>,
        ret: Box<Ty>,
        tail: Box<Row>,
    },
}

impl Row {
    /// The closed-empty row. Cheap and common enough that it wants a
    /// name.
    pub fn empty() -> Self {
        Row::Empty
    }

    /// Collect free type variables reachable through cons-cell
    /// payloads.
    pub fn collect_free_vars(&self, acc: &mut BTreeSet<TyVar>) {
        match self {
            Row::Empty | Row::Var(_) => {}
            Row::Cons { arg, ret, tail, .. } => {
                arg.collect_free_vars(acc);
                ret.collect_free_vars(acc);
                tail.collect_free_vars(acc);
            }
        }
    }

    /// Collect free row variables.
    pub fn collect_free_row_vars(&self, acc: &mut BTreeSet<RowVar>) {
        match self {
            Row::Empty => {}
            Row::Var(v) => {
                acc.insert(*v);
            }
            Row::Cons { arg, ret, tail, .. } => {
                arg.collect_free_row_vars(acc);
                ret.collect_free_row_vars(acc);
                tail.collect_free_row_vars(acc);
            }
        }
    }
}

impl fmt::Display for Row {
    /// B2.0: empty row renders as the empty string so existing arrow
    /// display tests are unaffected. Non-empty rows render in the
    /// `{Label | tail}` shape -- not yet exercised in B2.0 but defined
    /// to avoid a second rendering decision in B2.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Row::Empty => Ok(()),
            Row::Var(v) => write!(f, "{{{v}}}"),
            Row::Cons { label, tail, .. } => {
                write!(f, "{{{label}")?;
                let mut cur = tail.as_ref();
                loop {
                    match cur {
                        Row::Empty => break,
                        Row::Var(v) => {
                            write!(f, " | {v}")?;
                            break;
                        }
                        Row::Cons { label, tail, .. } => {
                            write!(f, ", {label}")?;
                            cur = tail.as_ref();
                        }
                    }
                }
                write!(f, "}}")
            }
        }
    }
}

/// A monotype: the language of types without quantification.
///
/// As of B2.0, `Fun` carries an effect row. B2.0 always passes
/// `Row::Empty`; B2.1 (row unification) and B2.3 (effect inference)
/// start using the other Row variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Bool,
    Var(TyVar),
    Fun(Box<Ty>, Row, Box<Ty>),
}

impl Ty {
    /// Convenience: build `arg -> ret` with an empty effect row.
    ///
    /// All existing callers that built arrows pre-B2.0 get the empty
    /// row automatically. B2.3 callers that need a non-empty row
    /// construct `Ty::Fun(..., row, ...)` directly.
    pub fn arrow(arg: Ty, ret: Ty) -> Ty {
        Ty::Fun(Box::new(arg), Row::empty(), Box::new(ret))
    }

    /// Convenience: build an arrow with an explicit row.
    pub fn arrow_with(arg: Ty, row: Row, ret: Ty) -> Ty {
        Ty::Fun(Box::new(arg), row, Box::new(ret))
    }

    /// Collect the set of type variables that appear free in this
    /// type.
    pub fn free_vars(&self) -> BTreeSet<TyVar> {
        let mut acc = BTreeSet::new();
        self.collect_free_vars(&mut acc);
        acc
    }

    /// Collect the set of row variables that appear free in this
    /// type. B2.0: always empty in practice (no constructor yields a
    /// non-empty row), but defined so infer.rs can call it.
    pub fn free_row_vars(&self) -> BTreeSet<RowVar> {
        let mut acc = BTreeSet::new();
        self.collect_free_row_vars(&mut acc);
        acc
    }

    pub(crate) fn collect_free_vars(&self, acc: &mut BTreeSet<TyVar>) {
        match self {
            Ty::Int | Ty::Bool => {}
            Ty::Var(v) => {
                acc.insert(*v);
            }
            Ty::Fun(a, row, b) => {
                a.collect_free_vars(acc);
                row.collect_free_vars(acc);
                b.collect_free_vars(acc);
            }
        }
    }

    pub(crate) fn collect_free_row_vars(&self, acc: &mut BTreeSet<RowVar>) {
        match self {
            Ty::Int | Ty::Bool | Ty::Var(_) => {}
            Ty::Fun(a, row, b) => {
                a.collect_free_row_vars(acc);
                row.collect_free_row_vars(acc);
                b.collect_free_row_vars(acc);
            }
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // On an empty row (B2.0 default), Row's Display emits nothing,
        // so arrows render exactly as they did in B1.
        // On a non-empty row, Row's Display emits `{...}`, and we
        // print it between the arrow and the return type, giving
        // `a -> {Label | tail} b`. B2.1 may refine.
        match self {
            Ty::Int => f.write_str("int"),
            Ty::Bool => f.write_str("bool"),
            Ty::Var(v) => write!(f, "{v}"),
            Ty::Fun(a, row, b) => {
                let row_is_empty = matches!(*row, Row::Empty);
                if matches!(**a, Ty::Fun(_, _, _)) {
                    write!(f, "({a})")?;
                } else {
                    write!(f, "{a}")?;
                }
                if row_is_empty {
                    write!(f, " -> {b}")
                } else {
                    write!(f, " -> {row} {b}")
                }
            }
        }
    }
}

/// A polymorphic type scheme: `forall vars. ty`.
///
/// B2.0: only type variables are quantified. B2.3 may extend this to
/// row variables; that's deferred so generalisation semantics get
/// chosen deliberately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    pub vars: Vec<TyVar>,
    pub ty: Ty,
}

impl Scheme {
    pub fn mono(ty: Ty) -> Self {
        Scheme { vars: Vec::new(), ty }
    }

    pub fn free_vars(&self) -> BTreeSet<TyVar> {
        let mut fvs = self.ty.free_vars();
        for v in &self.vars {
            fvs.remove(v);
        }
        fvs
    }

    pub fn free_row_vars(&self) -> BTreeSet<RowVar> {
        // B2.0: Scheme does not quantify row vars, so all row vars
        // in ty are free.
        self.ty.free_row_vars()
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.vars.is_empty() {
            write!(f, "{}", self.ty)
        } else {
            write!(f, "forall")?;
            for v in &self.vars {
                write!(f, " {v}")?;
            }
            write!(f, ". {}", self.ty)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: u32) -> Ty {
        Ty::Var(TyVar(n))
    }

    #[test]
    fn free_vars_of_concrete_type_is_empty() {
        assert!(Ty::Int.free_vars().is_empty());
        assert!(Ty::Bool.free_vars().is_empty());
    }

    #[test]
    fn free_vars_of_var_is_just_that_var() {
        let fvs = v(7).free_vars();
        assert_eq!(fvs.len(), 1);
        assert!(fvs.contains(&TyVar(7)));
    }

    #[test]
    fn free_vars_of_arrow_combines() {
        let t = Ty::arrow(Ty::arrow(v(0), v(1)), v(0));
        let fvs = t.free_vars();
        assert_eq!(fvs.len(), 2);
        assert!(fvs.contains(&TyVar(0)));
        assert!(fvs.contains(&TyVar(1)));
    }

    #[test]
    fn scheme_free_vars_subtracts_quantifiers() {
        let s = Scheme {
            vars: vec![TyVar(0)],
            ty: Ty::arrow(v(0), v(1)),
        };
        let fvs = s.free_vars();
        assert_eq!(fvs.len(), 1);
        assert!(fvs.contains(&TyVar(1)));
    }

    #[test]
    fn ty_display_arrows_are_right_associative() {
        let t = Ty::arrow(v(0), Ty::arrow(v(1), v(2)));
        assert_eq!(format!("{t}"), "'a -> 'b -> 'c");

        let t = Ty::arrow(Ty::arrow(v(0), v(1)), v(2));
        assert_eq!(format!("{t}"), "('a -> 'b) -> 'c");
    }

    #[test]
    fn tyvar_display_letters_then_indexed() {
        assert_eq!(format!("{}", TyVar(0)), "'a");
        assert_eq!(format!("{}", TyVar(1)), "'b");
        assert_eq!(format!("{}", TyVar(25)), "'z");
        assert_eq!(format!("{}", TyVar(26)), "'a1");
        assert_eq!(format!("{}", TyVar(52)), "'a2");
    }

    #[test]
    fn scheme_display_includes_forall() {
        let s = Scheme {
            vars: vec![TyVar(0), TyVar(1)],
            ty: Ty::arrow(v(0), v(1)),
        };
        assert_eq!(format!("{s}"), "forall 'a 'b. 'a -> 'b");
    }

    // ---------- B2.0: empty-row rendering invariants ----------

    #[test]
    fn b20_empty_row_renders_as_empty_string() {
        assert_eq!(format!("{}", Row::Empty), "");
    }

    #[test]
    fn b20_arrow_with_empty_row_is_unchanged_from_b1() {
        let t = Ty::arrow(v(0), v(1));
        assert_eq!(format!("{t}"), "'a -> 'b");
    }

    #[test]
    fn b20_rowvar_display_uses_r_prefix() {
        assert_eq!(format!("{}", RowVar(0)), "'ra");
        assert_eq!(format!("{}", RowVar(25)), "'rz");
        assert_eq!(format!("{}", RowVar(26)), "'ra1");
    }
}
