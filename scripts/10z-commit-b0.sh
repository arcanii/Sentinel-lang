#!/usr/bin/env bash
# 10z-commit-b0.sh - commit Phase B0 (sentinel-effects-proto scaffold).
# Runs full check suite, stages, commits. Does NOT push.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -d crates/sentinel-effects-proto ]]; then
  echo "ERROR: sentinel-effects-proto missing; run B0 scaffold first" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== B0 COMMIT START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

echo "====== PRE-COMMIT BUILD (effects-proto)"
cargo build -p sentinel-effects-proto 2>&1 | tail -6
B_RC=${PIPESTATUS[0]}
echo "build rc=$B_RC"

echo
echo "====== PRE-COMMIT CLIPPY (effects-proto)"
cargo clippy -p sentinel-effects-proto --all-targets -- -D warnings 2>&1 | tail -6
C_RC=${PIPESTATUS[0]}
echo "clippy rc=$C_RC"

echo
echo "====== PRE-COMMIT TESTS (effects-proto)"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-effects-proto 2>&1 | tail -8
  T_RC=${PIPESTATUS[0]}
else
  cargo test -p sentinel-effects-proto 2>&1 | tail -8
  T_RC=${PIPESTATUS[0]}
fi
echo "tests rc=$T_RC"

echo
echo "====== PRE-COMMIT REGRESSION (broker unaffected)"
cargo build -p sentinel-broker 2>&1 | tail -4
BB_RC=${PIPESTATUS[0]}
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -6
  TB_RC=${PIPESTATUS[0]}
else
  cargo test -p sentinel-broker 2>&1 | tail -6
  TB_RC=${PIPESTATUS[0]}
fi
echo "broker build rc=$BB_RC tests rc=$TB_RC"

if [[ $B_RC -ne 0 || $C_RC -ne 0 || $T_RC -ne 0 || $BB_RC -ne 0 || $TB_RC -ne 0 ]]; then
  echo
  echo "ERROR: pre-commit checks failed. Aborting commit."
  return 1 2>/dev/null || exit 1
fi

echo
echo "====== GIT STATUS"
git status --short

echo
echo "====== STAGING"
git add \
  Cargo.toml \
  crates/sentinel-effects-proto/ \
  scripts/10-b0-effects-proto-scaffold.sh \
  scripts/10a-b0-fix-eq.sh \
  scripts/10z-commit-b0.sh
git status --short

# Write the commit message body to /tmp to avoid quoting headaches.
cat > /tmp/sentinel_b0_commit_msg.txt <<'MSGEOF'
B0: sentinel-effects-proto scaffold (Sentinel-Mini lex+parse+eval)

First milestone of Phase B per HANDOVER.md section 5. Establishes the
end-to-end pipeline for the tree-walking interpreter that will validate
Sentinel's effect-system design before committing to the Phase C
production compiler.

New crate: crates/sentinel-effects-proto (added to workspace members).

Language surface at B0 (pure expression calculus, no types yet):

  - integer and boolean literals
  - identifiers, let .. in .., if .. then .. else ..
  - single-parameter lambdas (fn(x) => body) and application
  - arithmetic (+ - * /) and comparison (== < >)
  - parenthesised grouping
  - // line comments

Modules:

  - lexer.rs   logos-based tokenizer, returns Vec<Token>
  - ast.rs     plain enum AST (Box-allocated children)
  - parser.rs  hand-written recursive descent, precedence climbing
  - eval.rs    tree-walking evaluator over a persistent Arc-cons-list
               environment; closures capture by sharing structure
  - lib.rs     re-exports + a top-level run() convenience and MiniError

Tests: 23 green (5 lexer + 5 parser + 8 eval + 5 integration). Clippy
clean under -D warnings. Doctests pass (none written yet).

Deliberately deferred to later B milestones:

  - HM type inference                   (B1)
  - effect rows and effect declarations (B2)
  - effect handlers (handle .. with ..) (B3)
  - secret T qualifier                  (B4)
  - letrec / recursion                  (B1 alongside types)
  - broker integration as value heap    (bonus, post-B1)
  - REPL / driver binary                (when useful)
  - span tracking + rich diagnostics    (B1)

One test (pipeline_recursion_via_y_combinator_would_need_letrec) is
asserting an unbound-variable error rather than a correct result, to
document the recursion gap loudly. It flips to a correctness check
when letrec lands.

Honest notes:

  - First scaffold pass had Token deriving only PartialEq while
    ParseError derived Eq, which fails to compile. Fix landed in
    10a-b0-fix-eq.sh: dropped Eq from ParseError and EvalError.
    Both error types remain Debug + Clone + PartialEq, which is
    what tests actually use.
  - Closures clone the body Expr into an Arc<Expr>. Acceptable for a
    research artifact; will revisit if/when bodies get large or if
    the broker becomes the value heap.
MSGEOF

echo
echo "====== COMMIT"
git commit -F /tmp/sentinel_b0_commit_msg.txt
COMMIT_RC=$?
echo "commit rc=$COMMIT_RC"

echo
echo "====== POST-COMMIT GIT LOG"
git log --oneline -6

echo
echo "====== B0 COMMIT END"
echo "NOTE: commit is local only. Review with 'git show HEAD' and push when ready."
