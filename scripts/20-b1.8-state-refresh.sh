# B1.8: refresh STATE.md, BACKLOG.md, lib.rs module doc; add ADR 0003.
# Docs-only. Zsh-paste-safe.

run_b18() {
  cd "$(git rev-parse --show-toplevel)" || { echo 'not in a git repo'; return 1; }

  local STATE='docs/STATE.md'
  local BACKLOG='docs/BACKLOG.md'
  local LIB='crates/sentinel-effects-proto/src/lib.rs'
  local ADR='docs/decisions/0003-b1-retrospective.md'

  for f in "$STATE" "$BACKLOG" "$LIB"; do
    if [ ! -f "$f" ]; then echo "missing $f"; return 1; fi
  done
  if [ -f "$ADR" ]; then
    echo "$ADR already exists; refusing to overwrite"; return 1
  fi

  # ---------- patch STATE.md ----------
  local PY='/tmp/b18_state.py'
  cat > "$PY" <<'PY_EOF'
import sys, pathlib

path = pathlib.Path(sys.argv[1])
src = path.read_text()

def must_replace(old, new):
    global src
    if old not in src:
        sys.stderr.write(f'STATE.md: missing snippet:\n{old[:80]!r}\n')
        sys.exit(2)
    src = src.replace(old, new, 1)

# 1) Last-updated line.
must_replace(
    "Last updated: phase A9 (broker cleanup) and B0 (effects-proto\nscaffold) landed.",
    "Last updated: phase B1 complete (HM inference with let-rec\ngeneralization, span-tracked diagnostics, hand-rolled caret\nrenderer). See ADR 0003 for the B1 retrospective.",
)

# 2) B.1 Phase Tracker row for B1.
must_replace(
    "| B1    | HM type inference, letrec, span-tracked errors     | Planned |        |",
    "| B1    | HM type inference, letrec, span-tracked errors     | Done   | e6b06cd |",
)

# 3) B0 test coverage line.
must_replace(
    "Test coverage as of B0: 23 tests (5 lexer + 5 parser + 8 eval + 5\nintegration). Clippy clean under `-D warnings`. No doctests yet.",
    "Test coverage as of B1: 95 tests (8 lexer + 11 parser + 11 eval +\n4 span + 7 types + 41 infer + 7 diag + 6 integration). Clippy\nclean under `-D warnings`. No doctests yet.\n\nB1 landed across five commits: spans + Spanned AST + `let rec`\n(abfb3d9), types scaffold (b3589ea), inference driver wired into\n`run` (24a3db8), proper HM let-rec typing (72c0996), and\nhand-rolled caret diagnostics (e6b06cd).",
)

# 4) Crate layout: add new src files.
must_replace(
    "      src/\n        lib.rs            re-exports + `run()` convenience + `MiniError`\n        lexer.rs          logos tokeniser, `Token`, `LexError`\n        ast.rs            `Expr`, `BinOp` (plain enums, Box-allocated children)\n        parser.rs         hand-written recursive descent, precedence climbing,\n                          `ParseError`\n        eval.rs           tree-walking interpreter, persistent `Env`,\n                          `Value`, `EvalError`",
    "      src/\n        lib.rs            re-exports + `run()` convenience + `MiniError`\n                          (now incl. `MiniError::render(src) -> String`)\n        lexer.rs          logos tokeniser, `Token` (incl. `Rec`), `LexError`\n        ast.rs            `Expr = Spanned<ExprKind>`, `BinOp`, `LetRec` variant\n        parser.rs         hand-written recursive descent, precedence climbing,\n                          `ParseError`, span-threading\n        eval.rs           tree-walking interpreter, persistent `Env`,\n                          `Value`, `EvalError`, `let rec` via `OnceLock`\n        span.rs           `Span { start: u32, end: u32 }`, `Spanned<T>` (B1.1)\n        types.rs          `Ty`, `TyVar`, `Scheme`, free-var sets (B1.4)\n        infer.rs          HM Algorithm W: `Subst`, `unify`, `instantiate`,\n                          `generalize`, `TypeEnv`, `infer`, `infer_top`,\n                          `TypeError` (B1.4-B1.6)\n        diag.rs           `LineCol`, `locate`, `render` -- hand-rolled\n                          rustc-style caret diagnostics (B1.7)",
)

# 5) Grammar: drop "no recursion" sentence, add let rec production.
must_replace(
    "Pure expression calculus. Everything is an expression. No\nstatements, no types, no effects, no `secret`, no recursion.",
    "Pure expression calculus with HM type inference. Everything is an\nexpression. No statements, no effects, no `secret` yet.\nRecursion is supported via `let rec` (B1.3); types are inferred\nwith let-polymorphism and let-rec generalization (B1.5/B1.6).",
)

must_replace(
    "    expr      := let | if | lambda | compare\n    let       := \"let\" IDENT \"=\" expr \"in\" expr",
    "    expr      := let | letrec | if | lambda | compare\n    let       := \"let\" IDENT \"=\" expr \"in\" expr\n    letrec    := \"let\" \"rec\" IDENT \"=\" lambda \"in\" expr",
)

# 6) Public API surface: replace the bullet list.
must_replace(
    "All re-exports from `sentinel_effects_proto`:\n\n- `Expr`, `BinOp` (AST)\n- `Token`, `LexError`, `lex(source) -> Result<Vec<Token>, LexError>`\n- `ParseError`, `parse(&[Token]) -> Result<Expr, ParseError>`\n- `Value`, `EvalError`, `Env`, `eval(&Expr, &Env) -> Result<Value, EvalError>`\n- `MiniError`, `run(source) -> Result<Value, MiniError>` (lex+parse+eval)",
    "All re-exports from `sentinel_effects_proto`:\n\n- AST: `Expr = Spanned<ExprKind>`, `ExprKind`, `BinOp`, `expr` constructor helper\n- Spans: `Span`, `Spanned<T>`\n- Lexer: `Token`, `LexError`,\n  `lex(source) -> Result<Vec<(Token, Span)>, LexError>`\n- Parser: `ParseError`,\n  `parse(&[(Token, Span)]) -> Result<Expr, ParseError>`\n- Eval: `Value`, `EvalError`, `Env`,\n  `eval(&Expr, &Env) -> Result<Value, EvalError>`\n- Types: `Ty`, `TyVar`, `Scheme`\n- Inference: `TypeError`, `TypeEnv`, `Subst`, `TyVarSupply`,\n  `unify`, `instantiate`, `generalize`, `infer`, `infer_top`\n- Top-level: `MiniError`,\n  `run(source) -> Result<Value, MiniError>` (lex+parse+infer+eval),\n  `MiniError::render(&self, source) -> String` for caret diagnostics\n\nThe `diag` module is `pub mod diag` but its items are reached\nthrough `MiniError::render`; they are not re-exported at the\ncrate root in B1.",
)

# 7) Design Decisions: add B1 entries.
must_replace(
    "5. `BrokerError`-style two-flavour API (panicking + fallible) is\n   NOT adopted here. Effects-proto is throwaway research code;\n   panicking-only is acceptable and simpler. If a panic-free API\n   becomes useful for embedding, it lands then.",
    "5. `BrokerError`-style two-flavour API (panicking + fallible) is\n   NOT adopted here. Effects-proto is throwaway research code;\n   panicking-only is acceptable and simpler. If a panic-free API\n   becomes useful for embedding, it lands then.\n6. (B1.1/B1.2) AST nodes carry spans via a `Spanned<T>` wrapper,\n   not an inline `span` field on each variant. Confirmed cheap;\n   parser pattern is `Spanned::new(kind, start_span.merge(end_span))`.\n7. (B1.3) `rec` is a reserved keyword (`Token::Rec`), not a\n   contextual one. `let rec` is the only place it can appear in B1.\n8. (B1.4) Substitutions are eager (`apply` on `bind`), not\n   union-find. Idempotency is maintained by `compose`. Fine at\n   B1 scale; revisit only if profiling demands.\n9. (B1.5/B1.6) Inference is Algorithm W in the textbook shape.\n   `let rec` uses the standard HM treatment: monomorphic recursive\n   occurrence inside the RHS, generalized scheme in the body.\n   Polymorphic recursion is therefore unavailable without\n   annotations -- this is intentional and matches ML/Haskell\n   without explicit type signatures.\n10. (ADR 0002) Function arrows are bare `Fun(Ty, Ty)`. Effect rows\n    are deferred to B2 to keep B1 focused.\n11. (B1.7) Diagnostics are hand-rolled (`diag.rs`, ~110 LoC, no\n    `miette` dependency). Phase C will likely adopt miette; the\n    prototype validates the shape (line/col header, source-line\n    excerpt, caret underline) cheaply. `Display` for `MiniError`\n    stays terse; pretty rendering is opt-in via `.render(src)`.",
)

# 8) Known Limitations: rewrite the whole list.
must_replace(
    "### B.6 Known Limitations (intentional at B0)\n\n- No recursion. `let f = fn(n) => ... f ...` produces\n  `Unbound(\"f\")`. The integration test\n  `pipeline_recursion_via_y_combinator_would_need_letrec` documents\n  this loudly and flips to a correctness check when `letrec` lands\n  in B1.\n- No types. Type errors surface at evaluation time, not at compile\n  time. (B1 fixes.)\n- No effects. The whole reason this crate exists. (B2 onward.)\n- No REPL, no driver binary. Library-only.\n- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable\n  for a research artifact; revisit if body-clone becomes a hot path\n  or if the broker becomes the value heap.\n- `Value` does not derive `Eq` (closures aren't comparable); the\n  error types also drop `Eq` to keep them embeddable in each other.\n  They remain `Debug + Clone + PartialEq`, which is what tests use.",
    "### B.6 Known Limitations (intentional at B1)\n\n- No effects. The whole reason this crate exists. (B2 onward.)\n- No `secret` qualifier or constant-time check. (B4.)\n- No REPL, no driver binary. Library-only.\n- `let rec` RHS must be a syntactic lambda. Parser enforces with\n  `ParseError::LetRecNotLambda`. Relaxing this in B3 (when handlers\n  arrive) is an open question; see ADR 0003.\n- Polymorphic recursion is rejected, as in ML/Haskell without an\n  explicit type signature. Test\n  `b16_letrec_recursive_occurrence_is_monomorphic_inside_body`\n  locks this.\n- Equality (`==`) is polymorphic at the type level (`forall a. a -> a -> Bool`).\n  The evaluator still rejects equality on closures; B2/B3 may\n  refine via type-class-style constraints.\n- `EvalError` variants carry no spans. Eval errors are rare\n  post-type-check (div-by-zero, non-function application on a\n  closure-typed value, the letrec uninitialised internal error)\n  but they render without carets. B2 backlog item.\n- Multi-line spans in `diag::render` clip to the first line. Sentinel-Mini\n  programs are usually one-liners; Phase C diagnostics will handle\n  multi-line ranges properly.\n- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable\n  for a research artifact; revisit if body-clone becomes a hot path\n  or if the broker becomes the value heap.\n- `Value` does not derive `Eq` (closures aren't comparable); the\n  error types also drop `Eq` to keep them embeddable in each other.\n  They remain `Debug + Clone + PartialEq`, which is what tests use.",
)

# 9) Conventions: bump test count.
must_replace(
    "  - sentinel-broker:        69 tests + 1 doctest\n  - sentinel-effects-proto: 23 tests + 0 doctests",
    "  - sentinel-broker:        69 tests + 1 doctest\n  - sentinel-effects-proto: 95 tests + 0 doctests",
)

path.write_text(src)
print('patched', path)
PY_EOF

  python3 "$PY" "$STATE"
  if [ "$?" != "0" ]; then echo "STATE.md patch failed"; return 1; fi

  # ---------- patch BACKLOG.md: add §0.4 ----------
  local PY2='/tmp/b18_backlog.py'
  cat > "$PY2" <<'PY_EOF'
import sys, pathlib

path = pathlib.Path(sys.argv[1])
src = path.read_text()

# Insert §0.4 before §1. Anchor on the "---" that precedes section 1.
marker = "## 1. Privileged-Mode and Bare-Metal Sentinel"
if marker not in src:
    sys.stderr.write('BACKLOG.md: missing section 1 header\n')
    sys.exit(2)

addition = '''### 0.4 Phase B1 carry-over (effects-proto)

Items noticed during B1 implementation that are too small to be ADRs
but worth tracking. None block B2.

- **`EvalError` variants carry no spans.** Eval errors are rare
  post-type-check (div-by-zero, the `LetRecUninitialised` internal
  invariant, application of a Closure to the wrong-typed value once
  the type system gets richer), but when they do fire, `MiniError::render`
  falls back to the terse one-line form. Add `Span` fields to the
  three variants that can plausibly point at source (`Unbound` is
  unreachable post-type-check; `DivByZero` and `Type` should carry
  the offending expression's span; `NotAFunction` likewise; the
  `LetRecUninitialised` is a bug-class error and may stay span-less).
  Originating context: B1.7 dispatch, commit e6b06cd.

- **Multi-line span rendering clips to first line.** `diag::render`
  intentionally degrades on cross-line spans; B2's `effect` and
  `handle` blocks are likely to introduce multi-line constructs
  that make this annoying. Either teach `render` to emit a multi-line
  excerpt with carets on each line, or adopt `miette` here rather
  than waiting for Phase C.

- **`let rec` RHS-must-be-lambda restriction.** Currently a parser
  rule. May need relaxing in B3 when effect handlers arrive
  (handlers as recursive bindings). Re-evaluate when designing the
  handler surface; see ADR 0003.

'''

src = src.replace(marker, addition + "---\n\n" + marker, 1)
path.write_text(src)
print('patched', path)
PY_EOF

  python3 "$PY2" "$BACKLOG"
  if [ "$?" != "0" ]; then echo "BACKLOG.md patch failed"; return 1; fi

  # ---------- patch lib.rs module doc ----------
  local PY3='/tmp/b18_lib.py'
  cat > "$PY3" <<'PY_EOF'
import sys, pathlib

path = pathlib.Path(sys.argv[1])
src = path.read_text()

# Replace the entire "# Status" block.
old = """//! # Status
//!
//! - **B0**: lex + parse + eval.
//! - **B1.1**: span infrastructure.
//! - **B1.2**: AST nodes carry spans.
//! - **B1.3**: `let rec` with `OnceLock` knot-tying.
//! - **B1.4**: types scaffold (Ty, Scheme, Subst, unify, ...).
//! - **B1.5a**: HM inference driver (infer, TypeEnv, infer_top).
//! - **B1.5b** (this commit): pipeline wiring. [`run`] now type-checks
//!   between parse and eval; type errors abort before evaluation.
//!   `let rec` is still typed at a monovar; B1.6 will refine.
//! - B1.6 - B1.8 remaining: letrec generalisation, diagnostic
//!   rendering with carets."""

new = """//! # Status
//!
//! - **B0-B1**: complete. Lex + parse + HM type inference (with
//!   let-polymorphism and let-rec generalization) + eval, all
//!   span-tracked, with hand-rolled caret diagnostics via
//!   [`MiniError::render`].
//! - **B2** (next): effect rows and effect declarations. Per
//!   ADR 0002, B1's function arrows are bare `Fun(Ty, Ty)`; B2
//!   extends this with a row component.
//! - **B3-B4** (planned): effect handlers, `secret` qualifier.
//!
//! See `docs/STATE.md` Section B for the authoritative status."""

if old not in src:
    sys.stderr.write('lib.rs: status block not found\n')
    sys.exit(2)
src = src.replace(old, new, 1)
path.write_text(src)
print('patched', path)
PY_EOF

  python3 "$PY3" "$LIB"
  if [ "$?" != "0" ]; then echo "lib.rs patch failed"; return 1; fi

  # ---------- write ADR 0003 via base64 to dodge backtick collisions ----------
  local B64='/tmp/b18_adr.b64'
  cat > "$B64" <<'B64_EOF'
IyBBRFIgMDAwMzogQjEgUmV0cm9zcGVjdGl2ZQoKLSBTdGF0dXM6IEFjY2VwdGVk
Ci0gRGF0ZTogMjAyNi0wNS0yMwotIFJlbGF0ZWQ6IEFEUiAwMDAxIChzdGFnZWQg
dmFsaWRhdGlvbiksIEFEUiAwMDAyIChlZmZlY3Qgcm93cyBpbiBNaW5pKQoKIyMg
Q29udGV4dAoKUGhhc2UgQjEgb2YgYHNlbnRpbmVsLWVmZmVjdHMtcHJvdG9gIGNv
bXBsZXRlZCBhY3Jvc3MgZml2ZSBjb21taXRzCihhYmNkOSwgYjM1ODllYSwgMjRh
M2RiOCwgNzJjMDk5NiwgZTZiMDZjZCkuIEl0IGFkZGVkOgoKLSBTcGFuLXRyYWNr
ZWQgQVNUIChgU3Bhbm5lZDxFeHByS2luZD5gKSwgcHJlc2VydmVkIHRocm91Z2gK
ICBwYXJzaW5nIGFuZCBhdmFpbGFibGUgdG8gZGlhZ25vc3RpY3MuCi0gYGxldCBy
ZWNgIHdpdGggcnVudGltZSBrbm90LXR5aW5nIHZpYSBgT25jZUxvY2tgIGFuZAog
IHByb3BlciBITSBnZW5lcmFsaXphdGlvbiBpbiB0aGUgYm9keSAobW9ub21vcnBo
aWMKICByZWN1cnNpdmUgb2NjdXJyZW5jZSBpbnNpZGUgdGhlIFJIUykuCi0gQSBI
aW5kbGV5LU1pbG5lciB0eXBlIGluZmVyZW5jZSBkcml2ZXIgKEFsZ29yaXRobSBX
KSB3aXJlZAogIGludG8gYHJ1bigpYCwgc28gdHlwZSBlcnJvcnMgYWJvcnQgYmVm
b3JlIGV2YWx1YXRpb24uCi0gSGFuZC1yb2xsZWQgY2FyZXQgZGlhZ25vc3RpY3Mg
KGBkaWFnLnJzYCwgfjExMCBMb0MsCiAgemVybyBuZXcgZGVwcyksIGV4cG9zZWQg
dmlhIGBNaW5pRXJyb3I6OnJlbmRlcihzcmMpYC4KClRoaXMgZG9jdW1lbnQgY2Fw
dHVyZXMgd2hhdCB3ZSBsZWFybmVkIHRoYXQgYWZmZWN0cyBCMidzCmRlc2lnbiwg
YW5kIHdoYXQgd2UncmUgZGVsaWJlcmF0ZWx5IGNhcnJ5aW5nIGZvcndhcmQuCgoj
IyBPYnNlcnZhdGlvbnMgZnJvbSBCMQoKKipPMS4gVGhlIHN0dWItdGhlbi1nZW5l
cmFsaXplIHNlcXVlbmNpbmcgZm9yIGBsZXQgcmVjYCB3b3JrZWQuKioKQjEuNSBs
YW5kZWQgYSBtb25vbW9ycGhpYyBgbGV0IHJlY2Agc3R1YiAoZmFjdG9yaWFsCnR5
cGVkLCBwb2x5bW9ycGhpYyBpZGVudGl0eSBkaWQgbm90KSBhbmQgQjEuNiByZXBs
YWNlZAp0aGUgc3R1YiB3aXRoIHJlYWwgZ2VuZXJhbGl6YXRpb24gaW4gfjI1IGxp
bmVzIG9mIGNoYW5nZS4KVGhlIGludGVybWVkaWF0ZSBzdGF0ZSB3YXMgcmVsZWFz
YWJsZSAoYWxsIHRlc3RzIGdyZWVuLApydW50aW1lIHNlbWFudGljcyB1bmNoYW5n
ZWQpLiBSZXBlYXQgdGhpcyBzaGFwZSBmb3IgQjI6CmxhbmQgZWZmZWN0LWVtcHR5
LWFsd2F5cyBmaXJzdCwgdGhlbiBlZmZlY3Qgcm93cyB3aXRoIHJlYWwKdW5pZmlj
YXRpb24uCgoqKk8yLiBFYWdlciBzdWJzdGl0dXRpb24gbWFwcyB3ZXJlIGZpbmUu
KioKYFN1YnN0YCBpcyBhIGBIYXNoTWFwPFR5VmFyLCBUeT5gIHdpdGggYGFwcGx5
YCBjYWxsZWQgYXQKYGNvbXBvc2VgIHRpbWUuIE5vIHVuaW9uLWZpbmQsIG5vIHBh
dGgtY29tcHJlc3Npb24uIDQxIGluZmVyCnRlc3RzIHJ1biBpbiB1bmRlciAxbXMg
dG90YWwgaW4gZGV2IGJ1aWxkcy4gRG8gbm90IHJld3JpdGUKZm9yIEIyOyBnYXRl
IGFueSBjaGFuZ2Ugb24gYWN0dWFsIHByb2ZpbGUgZGF0YS4KCioqTzMuIFBlci12
YXJpYW50IGBzcGFuYCBmaWVsZHMgcHJlZmVycmVkIG92ZXIgYSBibGFua2V0CmBT
cGFubmVkPEVycm9yPmAgd3JhcHBlci4qKgpgUGFyc2VFcnJvcjo6VW5leHBlY3Rl
ZEVvZmAgZ2VudWluZWx5IGhhcyBubyB0b2tlbiB0byBwb2ludAphdDsgYEV2YWxF
cnJvcmAgdmFyaWFudHMgY3VycmVudGx5IGhhdmUgbm9uZSAoYmFja2xvZyBpdGVt
LApzZWUgQkFDS0xPRyDCpzAuNCkuIFRoZSBzaGFwZSBgZW51bSBGb29FcnJvciB7
IFggeyBzcGFuOiBTcGFuLCAuLi4gfSwKWVo7IH1gIGtlZXBzIHRoZSBhYnNlbmNl
IGhvbmVzdC4gQjIncyBlZmZlY3QtY2hlY2tlcgpzaG91bGQgZm9sbG93IHRoZSBz
YW1lIHJ1bGUuCgoqKk80LiBgT3B0aW9uPFNwYW4+YCBpbiBgZGlhZzo6cmVuZGVy
YCBpcyB0aGUgcmlnaHQgQVBJLioqClNwYW4tbGVzcyByZW5kZXJpbmcgY29sbGFw
c2VzIHRvIGBzZXZlcml0eTogbWVzc2FnZWAuIE5vCmZha2Ugc3BhbnMsIG5vIHN5
bnRoZXRpYyAiZW5kIG9mIGZpbGUiIGxvY2F0aW9ucy4gS2VlcCB0aGlzCmludmFy
aWFudCBpbiBCMjogaWYgYW4gZWZmZWN0LXJvdyBlcnJvciBjYW4ndCBwb2ludCBh
dCBhCnNwZWNpZmljIG9wZXJhdGlvbiwgZW1pdCBOb25lOyBkb24ndCBmYWJyaWNh
dGUuCgoqKk81LiBIYW5kLXJvbGxlZCBkaWFnbm9zdGljcyBhcmUgY2hlYXAuKioK
YGRpYWcucnNgIGlzIH4xMTAgTG9DIGFuZCBoYXMgNyB0ZXN0cy4gQ29tcGFyZWQg
dG8gYWRvcHRpbmcKbWlldHRlIChhIG5ldyB0b3AtbGV2ZWwgZGVwLCBhIG5ldyBl
cnJvciB0cmFpdCwgYW5kIG5ldyBydW50aW1lCnByaW50aW5nKSB0aGlzIHdhcyB0
aGUgcmlnaHQgY2FsbCBmb3IgYSBwcm90b3R5cGUuIFJlY29uc2lkZXIKbWlldHRl
IGF0IFBoYXNlIEMsIG5vdCBCMi4KCioqTzYuIHpzaC1wYXN0ZS1zYWZlIHNjcmlw
dCBkaXNjaXBsaW5lIG1hdHRlcnMuKioKVHdvIHNjcmlwdHMgaW4gQjEgYWJvcnRl
ZCBvbiB1bnF1b3RlZCBwYXJlbnRoZXNlcyBhbmQgYmFyZQpgZXhpdGAgY29tbWFu
ZHM7IG9uZSBjbG9zZWQgdGhlIHVzZXIncyB0ZXJtaW5hbC4gQWxsCnN1YnNlcXVl
bnQgc2NyaXB0cyB1c2VkOiBubyBgZXhpdGAsIG5vIGB1bnF1b3RlZCBwYXJlbnNg
IGluCmVjaG9lcywgYWxsIGxvZ2ljIGluc2lkZSBhIGZ1bmN0aW9uIGNhbGxlZCBh
dCB0aGUgYm90dG9tLApweXRob24zIGZvciBhbnkgZmlsZSBzdXJnZXJ5IHRoYXQg
ZG9lc24ndCBmaXQgaW4gc2VkLiBNYWtlCnRoaXMgYW4gZXhwbGljaXQgcGF0dGVy
biBpbiBIQU5ET1ZFUiDCpzAuMSBmb3IgQjIuCgojIyBSZWNvbW1lbmRhdGlvbnMg
Zm9yIEIyCgoqKlIxLiBSb3dzIGV4dGVuZCBgVHk6OkZ1bmAuKioKQ3VycmVudCBz
aGFwZTogYEZ1bihCb3g8VHk+LCBCb3g8VHk+KWAuIEIyIHNoYXBlOgpgRnVuKEJv
eDxUeT4sIFJvdywgQm94PFR5PilgIHdoZXJlIGBSb3dgIGlzIGl0c2VsZiBhCmRp
c2NyaW1pbmF0ZWQgdW5pb24gKGNsb3NlZCBlbXB0eSwgb3BlbiB2YXIsIGNvbnMt
Y2VsbCkuClRoZSByZWZhY3RvciB0b3VjaGVzIH4yMC00MCBjYWxsIHNpdGVzIChw
ZXIgQURSIDAwMDIncwplc3RpbWF0ZSk7IG1lYXN1cmUgYWN0dWFsIHRvdWNoIGNv
dW50IGFmdGVyIEIyIHByb3RvdHlwZS4KCioqUjIuIFVuaWZpY2F0aW9uIG9mIHJv
d3MgaXMgYSBzZXBhcmF0ZSBmdW5jdGlvbi4qKgpEbyBub3QgaW5saW5lIHJvdy11
bmlmaWNhdGlvbiBpbnRvIGB1bmlmeWA7IGdpdmUgaXQgaXRzCm93biBmdW5jdGlv
biAoYHVuaWZ5X3JvdyhTdWJzdCwgUm93LCBSb3csIFNwYW4pYCkuIFRoaXMKbWly
cm9ycyB0aGUgS29rYS9FZmZla3QtcGFwZXIgc2hhcGUgYW5kIGtlZXBzIGB1bmlm
eWAKcmVhZGFibGUuCgoqKlIzLiBgVHlwZUVycm9yYCBnYWlucyByb3ctc3BlY2lm
aWMgdmFyaWFudHMuKioKYExhYmVsTWlzbWF0Y2hgLCBgUm93SW5jb21wYXRpYmxl
YCwgZXRjLiBSZWZlcmVuY2UgU3BhbnMgYXQKdGhlIG9wZXJhdGlvbiBzaXRlLCBu
b3QgYXQgdGhlIGZ1bmN0aW9uIHR5cGUuCgoqKlI0LiBLZWVwIGBydW4oKWAncyBz
aGFwZS4qKgpyZW5kZXItYXQtdG9wbGV2ZWwgKGB7IGxleC0+cGFyc2UtPmluZmVy
LT5ldmFsIH1gKSBzaG91bGQgbm90CmNoYW5nZSBmb3IgQjI7IGVmZmVjdHMgYXJl
IGFkZGl0aXZlIGluc2lkZSBpbmZlci4KCioqUjUuIGBsZXQgcmVjYC1taXNzaW5n
LWxhbWJkYSBpcyBhIHBhcnNlciBydWxlLgoqKgpJZiBoYW5kbGVycyAoQjMpIGNh
biBhcHBlYXIgYXMgUkhTLCByZWxheCB0aGVuLiBVbnRpbCBCMyBoYXMKYSBkZXNp
Z24sIGtlZXAgdGhlIHJlc3RyaWN0aW9uLgoKIyMgT3BlbiBxdWVzdGlvbnMgY2Fy
cmllZCBpbnRvIEIyCgotIFNob3VsZCBlcXVhbGl0eSAoYD09YCkgYmUgY29uc3Ry
YWluZWQgYnkgYSB0eXBlIGNsYXNzCiAgKGBFcSBhYCkgaW4gQjEncyBzdWNjZXNz
b3IsIG9yIGRvIHdlIGRlZmVyIHRoZSBjbGFzcwogIHN5c3RlbSBlbnRpcmVseSB0
byBQaGFzZSBDPyBCMSdzIHJ1bnRpbWUtcmVqZWN0aW9uIGlzIGEKICBob2xkLW15
LWJlZXIgc2hhcGU7IGl0J3MgdGltZSB0byBkZWNpZGUuCi0gV2hlcmUgZG9lcyB0
aGUgYnJva2VyIGZpdCBpbnRvIHRoZSBldmFsdWF0b3I/IEhBTkRPVkVSCiAgcGxh
Y2VzIHRoaXMgYXMgYW4gb3B0aW9uYWwgQj8gbWlsZXN0b25lLiBJZiBub3QgaW4g
QjIsCiAgd2hlbj8gQ2FuZGlkYXRlOiBhZnRlciBlZmZlY3QgaGFuZGxlcnMgKEIz
KSwgYmVmb3JlIEM/CgojIyBSZWZlcmVuY2VzCgotIEFEUiAwMDAxOiBzdGFnZWQg
dmFsaWRhdGlvbiAoUGhhc2VzIEEvQi9DL0QpLgotIEFEUiAwMDAyOiBlZmZlY3Qg
cm93cyBkZWZlcnJlZCB0byBCMi4KLSBkb2NzL1NUQVRFLm1kIMKnQjogYXV0aG9y
aXRhdGl2ZSBjcmF0ZSBzdGF0ZS4KLSBjcmF0ZXMvc2VudGluZWwtZWZmZWN0cy1w
cm90by9zcmMvaW5mZXIucnM6IHRoZSBpbmZlcmVuY2UKICBkcml2ZXIgdGhpcyBy
ZXRyb3NwZWN0aXZlIGRpc2N1c3Nlcy4KCipFbmQgb2YgQURSLioK
B64_EOF

  mkdir -p docs/decisions
  base64 -D -i "$B64" -o "$ADR" 2>/dev/null || base64 -d -i "$B64" > "$ADR"
  if [ ! -s "$ADR" ]; then
    echo "ADR write failed (zero bytes)"; return 1
  fi
  echo "wrote $ADR"

  # ---------- sanity build (lib.rs comment block changed) ----------
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
  cargo test -p sentinel-effects-proto --lib 2>&1 | tail -5
  local LIB_RC=${pipestatus[1]:-$?}

  echo
  echo '====== INTEGRATION TESTS ======'
  cargo test -p sentinel-effects-proto --test integration 2>&1 | tail -5
  local INT_RC=${pipestatus[1]:-$?}

  echo
  echo '====== SUMMARY ======'
  echo "build:       $BUILD_RC"
  echo "clippy:      $CLIPPY_RC"
  echo "lib tests:   $LIB_RC"
  echo "integration: $INT_RC"

  echo
  echo '====== ADR PREVIEW (head) ======'
  head -20 "$ADR"
  echo '...'
  echo
  echo '====== GIT STATUS ======'
  git status --short

  echo
  echo '====== DONE ======'
  echo 'Expected: tests still 95 (no source changes). Docs touched:'
  echo '  M docs/STATE.md'
  echo '  M docs/BACKLOG.md'
  echo '  M crates/sentinel-effects-proto/src/lib.rs (module doc only)'
  echo '  ?? docs/decisions/0003-b1-retrospective.md'
  echo 'If green, commit: docs B1.8 retrospective and STATE refresh'
  echo 'B1 is then fully landed. Push to origin at your discretion.'
}

run_b18
