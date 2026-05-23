//! Type representation for Sentinel-Mini (B1.4).
//!
//! Per ADR 0002, B1's `Ty` is the textbook Hindley-Milner shape:
//! integers, booleans, type variables, and function arrows. Effect rows
//! are deferred to B2.
//!
//! # Identifiers
//!
//! Type variables are dense `u32` ids managed by [`TyVarSupply`] (in
//! `infer.rs`). They have no source-level name; pretty-printing assigns
//! letters (`a`, `b`, ...) at display time.
//!
//! # Schemes
//!
//! A [`Scheme`] is a (possibly empty) universal quantification over a
//! set of type variables. `let`-generalisation produces schemes;
//! `instantiate` consumes them.

use std::collections::BTreeSet;
use std::fmt;

/// A type variable identifier. Dense and cheap to copy/hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TyVar(pub u32);

impl fmt::Display for TyVar {
    /// Pretty-print as `'a`, `'b`, ... cycling through letters and then
    /// `'a1`, `'b1`, ... for higher indices. Stable per id, suitable for
    /// diagnostics; not unique across processes (which doesn't matter).
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

/// A monotype: the language of types without quantification.
///
/// B2 will widen `Fun` to include an effect row; until then every
/// arrow is implicitly `pure`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Bool,
    Var(TyVar),
    Fun(Box<Ty>, Box<Ty>),
}

impl Ty {
    /// Convenience: build `arg -> ret`.
    pub fn arrow(arg: Ty, ret: Ty) -> Ty {
        Ty::Fun(Box::new(arg), Box::new(ret))
    }

    /// Collect the set of type variables that appear free in this type.
    ///
    /// A `Ty` has no binders, so "free" is just "appears at all". We use
    /// `BTreeSet` for deterministic iteration order, which keeps
    /// generalisation order stable across runs.
    pub fn free_vars(&self) -> BTreeSet<TyVar> {
        let mut acc = BTreeSet::new();
        self.collect_free_vars(&mut acc);
        acc
    }

    fn collect_free_vars(&self, acc: &mut BTreeSet<TyVar>) {
        match self {
            Ty::Int | Ty::Bool => {}
            Ty::Var(v) => {
                acc.insert(*v);
            }
            Ty::Fun(a, b) => {
                a.collect_free_vars(acc);
                b.collect_free_vars(acc);
            }
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Right-associative arrows: a -> b -> c means a -> (b -> c).
        // Left-of-arrow needs parens if it is itself an arrow.
        match self {
            Ty::Int => f.write_str("int"),
            Ty::Bool => f.write_str("bool"),
            Ty::Var(v) => write!(f, "{v}"),
            Ty::Fun(a, b) => {
                if matches!(**a, Ty::Fun(_, _)) {
                    write!(f, "({a}) -> {b}")
                } else {
                    write!(f, "{a} -> {b}")
                }
            }
        }
    }
}

/// A polymorphic type scheme: `forall vars. ty`.
///
/// Produced by `generalize` at `let`-binding sites; consumed by
/// `instantiate` at variable references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    pub vars: Vec<TyVar>,
    pub ty: Ty,
}

impl Scheme {
    /// A scheme with no quantifiers - just a monotype.
    pub fn mono(ty: Ty) -> Self {
        Scheme { vars: Vec::new(), ty }
    }

    /// The free variables of the scheme: those of `ty` minus the bound
    /// vars in `vars`.
    pub fn free_vars(&self) -> BTreeSet<TyVar> {
        let mut fvs = self.ty.free_vars();
        for v in &self.vars {
            fvs.remove(v);
        }
        fvs
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
        // ('a -> 'b) -> 'a
        let t = Ty::arrow(Ty::arrow(v(0), v(1)), v(0));
        let fvs = t.free_vars();
        assert_eq!(fvs.len(), 2);
        assert!(fvs.contains(&TyVar(0)));
        assert!(fvs.contains(&TyVar(1)));
    }

    #[test]
    fn scheme_free_vars_subtracts_quantifiers() {
        // forall 'a. 'a -> 'b   has 'b free
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
        // 'a -> 'b -> 'c   displays without inner parens
        let t = Ty::arrow(v(0), Ty::arrow(v(1), v(2)));
        assert_eq!(format!("{t}"), "'a -> 'b -> 'c");

        // ('a -> 'b) -> 'c   displays WITH parens on the left
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
}
