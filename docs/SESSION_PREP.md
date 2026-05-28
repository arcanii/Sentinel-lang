# SESSION_PREP.md — Paste-ready prompt for the next chat session

Copy everything between the two horizontal rules below into the first
message of a fresh chat. It's a self-contained boot prompt: tells the
agent what's been done, where the code lives, what to read first, and
what's next.

---

Continuing Sentinel-lang work. Repo:
https://github.com/arcanii/Sentinel-lang

Local HEAD: 5209db0 (docs: ADR 0020 PROPOSED — handler runtime +
perform lowering, closes ADR 0019 D8). Last feat commit: 82e859e
(feat(c3.2b): sentinel-effect-check crate + effect_check_query).
Branch in sync with origin/main (verify with `git status` at
session start). Working tree clean.

## Where we are

Phase A (broker) + Phase B (effects-proto) + Phase C0 (bootstrap
MVP) + Phase C1 (full type system, 8 sub-phases) + Phase C2 (refs
+ borrow check + RAII drop, 6 sub-phases) + **Phase C3 typing
layer (6 sub-phases: C3.0(a)/C3.0(b) + C3.1 + C3.1b + C3.2(a) +
C3.2(b) + C3.3) all complete.** **Phase C3 runtime layer (handler
dispatch per ADR 0020) is the next coding work** — sub-phases
C3.4 through C3.7 per ADR 0020 D9. **1008 active workspace tests
+ 1 doctest.**

ADR status:
  - 0001-0014 + 0016 + 0017: ACCEPTED (or ACCEPTED-WITH-AMENDMENTS).
  - 0015 (C1.6 arrays): ACCEPTED-WITH-AMENDMENTS.
  - **0017 (Phase C2): ACCEPTED-WITH-AMENDMENTS.** All 14 D-
    decisions exercised across C2.0.1 → C2.5. Amendments at C2.5
    close.
  - **0018 (Polonius migration plan): PROPOSED.** Documents the
    lexical → flow-sensitive borrow-check migration; no
    migration code yet.
  - **0019 (Phase C3 typing): ACCEPTED-WITH-AMENDMENTS.** All 14
    D-decisions exercised across C3.0 → C3.3. Amendments: A1
    SecretEscapesPolymorphism subsumed by monomorphic generics;
    A2 runtime builtins declared effect-free.
  - **0020 (handler runtime): PROPOSED.** 12 D-decisions; picks
    free-monad reification over CPS / stack-saved; deep + one-
    shot handlers. The pre-flight design ADR for C3.4-C3.7.

Thirteen go/no-go programs run end-to-end via snc:

| Fixture | Sub-phase | Stdout |
|---------|-----------|--------|
| c05     | C0        | "10"   |
| c14     | C1.4      | "7"    |
| c15     | C1.5      | "142"  |
| c16     | C1.6      | "15"   |
| c17     | C1.7      | "42"   |
| c20     | C2.0.2    | "53"   |
| c21     | C2.1      | "168"  |
| c22     | C2.2      | "35"   |
| c23     | C2.3      | "100"  |
| c24     | C2.4      | "160"  |
| c25     | C2.5      | "190"  |
| c31     | C3.1      | "100"  |
| c32     | C3.2      | "42"   |
| c33     | C3.3      | "42"   |

Pipeline at C3.3 close (parse → resolve → check → effect_check →
borrow_check → codegen):

    parse_query → resolve_query → check_query
                → effect_check_query → borrow_check_query → codegen

The full type universe at C3.3 close: `{ I64, I32, Bool,
Struct(StructId), Nullable(NullableInner), Array(ArrayElem),
TypeParam(TypeParamId), GenericInstance(GenericInstanceId),
Ref(RefId), Secret(SecretId) }`. Sixth interner-table ADR
preserving `Type: Copy + Hash`.

Workspace crates:
  - `sentinel-base` — Salsa db + Diagnostic accumulator.
  - `sentinel-syntax` — lexer + parser (274 tests).
  - `sentinel-ast` — AST types (50 tests).
  - `sentinel-resolve` — name resolution (50 tests; produces
    ResolvedProgram with VarId/FnId/StructId/TypeParamId/
    EffectId).
  - `sentinel-types` — type checking (147 tests; produces
    TypedProgram with full type universe).
  - `sentinel-borrow-check` — lexical borrow check (79 tests;
    DropPlan).
  - `sentinel-effect-check` — effect inference + annotation
    check + main-must-be-effect-free (8 tests; new at C3.2(b)).
  - `sentinel-codegen` — LLVM IR via inkwell (62 tests).
  - `sentinel-runtime` — sentinel_alloc/print/panic_oob/free
    (4 tests).
  - `sentinel-driver` — `snc` binary (63 pass-test fixtures).
  - `sentinel-{hir,mir,lsp}` — scaffolds for later phases.

## Read in order (for full context)

1. `docs/HANDOVER.md` §0 in full — the canonical "where the
   codebase is right now" pointer.
2. `docs/STATE.md` — last-updated banner. Source of truth when
   docs disagree.
3. `docs/decisions/0019-phase-c3-kickoff-and-effects-plan.md` —
   ACCEPTED-WITH-AMENDMENTS at C3.3 close. Check the
   "Amendments at C3.3 close" + "Retrospective at C3.3 close"
   sections.
4. `docs/decisions/0020-handler-runtime-and-perform-lowering.md`
   — PROPOSED. The pre-flight design ADR for the next coding
   work (C3.4-C3.7). Twelve D-decisions; check D1's free-monad
   choice + D9's sub-phase split table.
5. `crates/sentinel-effect-check/src/lib.rs` (~470 LOC) — the
   C3.2 effect-check pass. Useful reference for the salsa-query
   pattern + how to add the C3.4 typing changes for effect
   discharge.
6. `crates/sentinel-types/src/lib.rs` — TypedExprKind variants
   (the C3.4 work adds Handle + Perform). Look at how Declassify
   was added at C3.1.

## Sanity check at session start

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # expect 1008 passing
cargo run --bin snc -- build tests/pass/c33_go_no_go.sentinel -o /tmp/c33
/tmp/c33 && echo "exit=$?"              # expect "42" then "exit=0"
```

If everything is green, you're booted.

## Next: C3.4 (handler runtime AST + parser + type-check)

Per ADR 0020 D9, the substantive work splits into four sub-phases:
  - **C3.4** — AST + parser + resolve mirror + effect discharge
    in type-check (1-2 sessions). **Next.**
  - **C3.5** — codegen for `perform` (emit
    `sentinel_perform_op` + frame reification at evaluation
    sites). 2-3 sessions.
  - **C3.6** — codegen for `handle` (dispatch on label;
    resume call; `sentinel_kont_resume` runtime symbol). 2-3
    sessions. The substantive runtime piece.
  - **C3.7** — polish + phase-go program (c37) + STATE.md /
    HANDOVER refresh + ADR 0020 PROPOSED → ACCEPTED flip.
    0-1 sessions.

### C3.4 concrete tasks

1. **AST** (`sentinel-ast`): add three new ExprKind variants
   plus two struct types.

   ```rust
   ExprKind::Handle {
       body: Box<Expr>,
       arms: Vec<HandlerArm>,
       return_arm: Option<ReturnArm>,
       span: Span,
   }
   ExprKind::Perform {
       effect: Spanned<String>,
       op: Spanned<String>,
       args: Vec<Expr>,
       span: Span,
   }
   HandlerArm {
       effect: Spanned<String>,
       op: Spanned<String>,
       param_names: Vec<Spanned<String>>,    // last is the
                                              // continuation
                                              // binding `k`
       body: Expr,
       span: Span,
   }
   ReturnArm {
       value_name: Spanned<String>,
       body: Expr,
       span: Span,
   }
   ```

2. **Parser** (`sentinel-syntax`): `parse_atom` adds `Handle` +
   `Perform` keyword arms. New helpers `parse_handler_arm` +
   `parse_return_arm`. The handle/with/perform lexer keywords
   are already reserved (C3.0(a)).

   Handler arm syntax (`EffectName . OpName ( Ident ( , Ident )* ) => expr`):

       handle do_work() with {
           Io.log(msg, k) => k(0),
           Io.read(k) => k(42),
           return v => v
       }

   The `return v => ret_body` arm is optional. Arms separated
   by `,`; trailing `,` allowed.

3. **Resolve** (`sentinel-resolve`): mirror Handle + Perform.
   Each handler arm's effect+op pair resolves to
   `(EffectId, op_index)` where `op_index` is the position in
   `ResolvedEffectDecl::ops`. Handler-arm param names get
   VarIds from the existing counter; the kont binding is the
   last param.

   New ResolveError variants: `UndefinedHandlerEffect`,
   `UndefinedHandlerOp`, `DuplicateHandlerArm` (same effect+op
   pair listed twice).

4. **Type-check** (`sentinel-types`): add `TypedExprKind::Handle`
   + `TypedExprKind::Perform`. Implement effect discharge in
   `check_expr` per ADR 0020 D6:
   - When checking `handle e with H`, compute the body `e`'s
     inferred row via the existing ADR 0019 D2 mechanism.
   - Compute the set of effects handled by H (union of EffectIds
     in arm Op references).
   - The handle-expr's outer row = body's row MINUS the handled
     set.
   - Each handler arm's body type must equal the handle-expr's
     outer type.

   For `perform Op(args)`: the perform contributes the op's
   effect to the enclosing fn's row. At C3.4 minimum, a
   `perform` outside any `handle` covering that effect raises
   `MissingHandler`. (Strictly: a perform of effect E inside a
   fn unannotated with E would still type-check, but D13
   rejects it at main if E isn't discharged on the way up.)

   New EffectError variants in sentinel-effect-check:
   `MissingHandler`, `OperationArityMismatch`,
   `OperationNotInEffect`, `HandlerArmTypeMismatch`.

5. **Codegen** (`sentinel-codegen`): NO codegen at C3.4. The
   TypedExprKind::Handle + Perform arms in `lower_expr` should
   `panic!("handle/perform codegen lands at C3.5/C3.6")` or
   return a clean CodegenError. The type-checker is supposed to
   reject programs that would reach codegen with Handle/Perform
   active.

   Wait — there's a subtlety. The type-checker accepts the
   surface (it computes the discharge correctly). The
   programs that USE handle/perform need to reach codegen to
   run. So at C3.4 the codegen panic is acceptable: a program
   that uses `handle` or `perform` won't run, but the type
   layer is correct.

   Alternative: emit a `CodegenError::HandlersNotYetSupported`
   so the user gets a clean error message rather than a panic.

6. **Tests + a c34 fixture**: parse + type-check a program that
   uses `handle Op.read() with { Io.read(k) => k(42) }`. UI
   fixtures for the four new EffectError variants.

### Pipeline shape after C3.4

Unchanged from C3.3 — handle/perform are TypedExprKind variants
that flow through `effect_check_query` (which gets a small
update to handle them) and into `borrow_check_query` +
`codegen` (where codegen rejects them until C3.5/C3.6).

## Alternative path: Phase C4 (traits + structured concurrency)

If you want to switch tracks, the next phase per HANDOVER §6.2
is **Phase C4** — classes, traits with named implementations,
delegation, actors. Pre-flight would be **ADR 0021 PROPOSED**
covering trait declaration syntax, impl-block syntax, method
resolution, default impls, generic-bound integration, and the
secret-T / effect-row interaction (e.g., can a trait method
declare an effect? declassify a secret?). "Most of this is
reasonable language design plumbing rather than novel work, but
the volume is significant" per HANDOVER §6.2.

Either path is defensible. The handler-runtime path (C3.4)
completes the Phase B effect-system vision and is the natural
"close what we started" move. The traits path (C4) is larger in
volume but lower per-piece risk.

## Working norms (from HANDOVER §0.1)

- **Trust STATE.md, not the git log.**
- **Small patches, build between each.** Cargo build + cargo
  test + cargo clippy after every meaningful change.
- **ADR-first per phase boundary.** ADR 0020 already PROPOSED;
  C3.4 lands code per its D9 sub-phase split.
- **feat + docs commit pairs per sub-phase.** Code in the feat
  commit; STATE.md + HANDOVER §0 refresh + ADR status update in
  the matching docs commit.
- **cargo clippy --workspace --all-targets -D warnings** is
  part of the four-check suite.
- **Minimal ceremony.** Short replies are the norm.

---

End of paste block.
