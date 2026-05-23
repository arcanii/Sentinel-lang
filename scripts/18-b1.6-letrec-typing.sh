# B1.6: proper HM let-rec typing - generalize the bound name.
# Zsh-paste-safe: no exit, no set -e, no unquoted parens in echoes.

run_b16() {
  cd "$(git rev-parse --show-toplevel)" || { echo 'not in a git repo'; return 1; }

  local INFER='crates/sentinel-effects-proto/src/infer.rs'
  if [ ! -f "$INFER" ]; then
    echo "missing $INFER"; return 1
  fi

  # Write the python patcher to a tempfile, then run it.
  local PY='/tmp/b16_patch.py'
  cat > "$PY" <<'PY_EOF'
import sys, pathlib, re

path = pathlib.Path(sys.argv[1])
src = path.read_text()

# --- Replace the LetRec arm ---------------------------------------------
old = (
    "        ExprKind::LetRec { name, value, body } => {\n"
    "            // B1.5 stub: monomorphic. See module docs.\n"
    "            let t_name = supply.fresh_ty();\n"
    "            let env_for_value = env.extend(name.clone(), Scheme::mono(t_name.clone()));\n"
    "            let (s1, t_value) = infer(&env_for_value, value, supply)?;\n"
    "            let s2 = unify(&s1, &t_name, &t_value, expr.span)?;\n"
    "            let env_for_body = env.apply(&s2)\n"
    "                .extend(name.clone(), Scheme::mono(s2.apply(&t_name)));\n"
    "            let (s3, t_body) = infer(&env_for_body, body, supply)?;\n"
    "            Ok((s3.compose(&s2), t_body))\n"
    "        }\n"
)

new = (
    "        ExprKind::LetRec { name, value, body } => {\n"
    "            // B1.6: proper HM let-rec. The recursive occurrence inside\n"
    "            // the RHS is monomorphic (sees t_name directly); the body\n"
    "            // sees a generalized scheme so the binding is polymorphic\n"
    "            // at use sites.\n"
    "            let t_name = supply.fresh_ty();\n"
    "            let env_for_value =\n"
    "                env.extend(name.clone(), Scheme::mono(t_name.clone()));\n"
    "            let (s1, t_value) = infer(&env_for_value, value, supply)?;\n"
    "            // Unify the recursive monovar with the inferred RHS type.\n"
    "            let s2 = unify(&s1, &t_name, &t_value, expr.span)?;\n"
    "            // Generalize against the *outer* env (not env_for_value),\n"
    "            // so the recursive binding itself does not appear in the\n"
    "            // free-var set we generalize over.\n"
    "            let env_after = env.apply(&s2);\n"
    "            let t_name_solved = s2.apply(&t_name);\n"
    "            let scheme = generalize(&t_name_solved, &env_after.free_vars());\n"
    "            let env_for_body = env_after.extend(name.clone(), scheme);\n"
    "            let (s3, t_body) = infer(&env_for_body, body, supply)?;\n"
    "            Ok((s3.compose(&s2), t_body))\n"
    "        }\n"
)

if old not in src:
    sys.stderr.write('could not find B1.5 LetRec stub to replace\n')
    sys.exit(2)

src = src.replace(old, new, 1)

# --- Append the four B1.6 tests inside mod tests {} ---------------------
mt = src.find('mod tests {')
if mt < 0:
    sys.stderr.write('could not locate mod tests block\n')
    sys.exit(3)

depth = 0
i = mt + len('mod tests ')
close_idx = None
while i < len(src):
    c = src[i]
    if c == '{':
        depth += 1
    elif c == '}':
        depth -= 1
        if depth == 0:
            close_idx = i
            break
    i += 1

if close_idx is None:
    sys.stderr.write('could not find closing brace of mod tests\n')
    sys.exit(4)

tests = r'''
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
            Ty::Fun(a, b) => {
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
'''

src = src[:close_idx] + tests + src[close_idx:]
path.write_text(src)
print('patched', path)
PY_EOF

  python3 "$PY" "$INFER"
  local PATCH_RC=$?
  if [ "$PATCH_RC" != "0" ]; then
    echo "patch failed rc=$PATCH_RC"; return "$PATCH_RC"
  fi

  echo
  echo '====== BUILD ======'
  cargo build -p sentinel-effects-proto
  local BUILD_RC=$?

  echo
  echo '====== CLIPPY ======'
  cargo clippy -p sentinel-effects-proto --all-targets -- -D warnings
  local CLIPPY_RC=$?

  echo
  echo '====== LIB TESTS ======'
  cargo test -p sentinel-effects-proto --lib
  local LIB_RC=$?

  echo
  echo '====== INTEGRATION TESTS ======'
  cargo test -p sentinel-effects-proto --test integration
  local INT_RC=$?

  echo
  echo '====== DOC TESTS ======'
  cargo test -p sentinel-effects-proto --doc
  local DOC_RC=$?

  echo
  echo '====== SUMMARY ======'
  echo "build:       $BUILD_RC"
  echo "clippy:      $CLIPPY_RC"
  echo "lib tests:   $LIB_RC"
  echo "integration: $INT_RC"
  echo "doc tests:   $DOC_RC"

  echo
  echo '====== GIT STATUS ======'
  git status --short

  echo
  echo '====== DONE ======'
  echo 'Expected: lib tests 78 -> 82 (+4 B1.6 tests). Integration still 5. Total 87.'
  echo 'If green, commit: feat mini B1.6 proper HM let-rec typing'
  echo "then say 'go' for scripts/19-b1.7-diagnostics.sh."
}

run_b16
