# ADR 0026: C5.1/C5.2 — the HIR + MIR pipeline middle and constant-time secret codegen

Status: PROPOSED — the C5.1/C5.2 detail ADR under ADR 0025 (Phase C5
kickoff), mirroring how ADR 0022/0023/0024 detailed sub-phases under
ADR 0021. Flips to ACCEPTED(-WITH-AMENDMENTS) as C5.1/C5.2 land,
recording deviations as numbered amendments. **This is the security
core of Phase C5**: it stands up the pipeline middle ADR 0009 §6.1
specified and finally gives `secret` runtime teeth (ADR 0008 →
ADR 0019 D12, twice deferred).

**C5.1 update (2026-05-29): the D3 escape hatch is INVOKED** (decided
with the developer, evidence-grounded — see the D3 tag). For 1.0,
codegen stays on the typed program (via the thin C5.1a HIR seam), MIR
lowers from the typed program for the D5 verification, and constant-time
emission is a codegen pass; the *thick* HIR desugar (D1) and the
codegen-consumes-HIR migration (D3 default) are deferred **post-1.0**.
The 1.0 bar — constant-time `secret` (D4+D5) — is unchanged.

Date: 2026-05-29
Related:
  - **0025** (Phase C5 kickoff — PROPOSED): D2 (stand up HIR + MIR),
    D3 (constant-time secret codegen) are the workstreams detailed
    here; D13's C5.0 resolution (single-process single-file TLS 1.3
    handshake go/no-go) is the forcing function that makes
    constant-time `secret` 1.0-minimum.
  - **0009** (Phase C kickoff — ACCEPTED): §6.1's `types → hir → mir →
    codegen` pipeline. C0–C4 short-circuited `types → codegen`; this
    ADR fills the middle. C1.0c's "codegen stays outside the salsa
    query graph" (LLVM `'ctx` lifetimes) carries forward — `hir_query`
    / `mir_query` are salsa-tracked, `compile_to_object` is not.
  - **0008** (secret qualifier + constant-time — ACCEPTED): specifies
    constant-time operations + speculation safety. C3.1 shipped only
    the type-level rejection; D4/D5 here implement the codegen +
    verification.
  - **0019** (Phase C3 — ACCEPTED-WITH-AMENDMENTS): C3.1's
    `Type::Secret(SecretId)` + the four leak-shape rejections
    (SecretBranch, SecretDivisor, SecretIndex, SecretInRefDeref);
    D12 deferred constant-time codegen to here.
  - **0016 / 0017 / 0020** (interner-`Copy`-`Type`, parallel-tree +
    `DropPlan`, Kont ABI): HIR must preserve the C1.7
    monomorphization, the C2 drop semantics, and the C3 effecting-fn
    `Kont*` ABI through the desugar — the migration is behaviour-
    preserving or it is wrong.
  - **0018** (Polonius migration — PROPOSED): an available C2 follow-on;
    MIR's CFG is the natural home for a future flow-sensitive borrow
    check, but that is not C5.1-gating.

## Context

The pipeline today is `parse → resolve → check → effect_check →
borrow_check → codegen`. `check_query` produces a `TypedProgram` (the
parallel-tree typed AST + interner tables: `fns`, `structs`, `classes`,
`traits`, `impls`, `generic_instances`, `refs`, `secrets`, `konts`,
`tasks`, …); `borrow_check_query` produces a `DropPlan`; and
`compile_to_object(&TypedProgram, &DropPlan, …)` lowers expression
trees straight to LLVM IR. There is no HIR and no MIR — the two
`sentinel-hir` / `sentinel-mir` crates are 20-line stubs.

Two consequences block Phase C5's security core:

  1. **There is nowhere SSA-shaped to host the constant-time
     verification (D5) or bounds-check elision.** These analyses want
     explicit control flow and def-use chains; bolting them onto the
     recursive `TypedProgram` expression tree is fragile (ADR 0025 D2's
     reasoning).
  2. **`secret` has no runtime teeth.** `Type::Secret(s)` lowers
     *identically* to its inner `T` (ADR 0019 D12): no branch-free
     lowering, no speculation barriers, no machine-level proof that a
     secret never reaches a timing-variable operation. The type system
     rejects the four leak shapes at the source level, but nothing
     stops LLVM's optimiser from reintroducing a secret-dependent
     branch, and nothing inserts the ADR 0008 speculation barriers.

`compile_to_object` already does, in an ad-hoc pre-walk, exactly the
work an HIR desugar would hoist: it materialises monomorphic generic
instances (C1.7), it has resolved trait/method/qualified dispatch to
concrete callees (C4.2), and it consumes the `DropPlan` to place drops
(C2.4). The HIR is therefore not a speculative new layer — it is the
*name* for the desugared program codegen already half-computes.

## Decision

Ten D-numbered decisions. Surface + the hard lowering land
incrementally per the sub-phase split; this ADR fixes the shape.

### D1. HIR stage — `sentinel-hir` + `hir_query`.

Introduce a typed, qualifier-resolved, **desugared** HIR produced by a
salsa-tracked `hir_query(db, file) -> Option<HirProgram>` that chains on
`check_query` (for the `TypedProgram`) **and** `borrow_check_query` (for
the `DropPlan`). The desugar:

  - **resolves dispatch to concrete callees** — class methods, default-
    and named-impl trait methods, delegation forwarders, and qualified
    calls all become a uniform `HirCall { callee: HirCallee, args }`
    where `HirCallee` is a concrete `FnId` / `(ImplId, method)` /
    builtin (no dispatch decisions left for codegen);
  - **materialises monomorphic instances** — each `(FnId, type_args)`
    reachable instance from the C1.7 worklist becomes a concrete HIR
    function with a stable mangled identity, so HIR is monomorphic;
  - **makes drops + scope-exits explicit** — the `DropPlan` is folded in
    as explicit `HirStmt::Drop(place)` at scope exits (closing the
    "codegen re-derives drop sites from a side table" coupling);
  - **preserves `secret`** — `Type::Secret(s)` and the interner tables
    are carried verbatim; the desugar is type- and secrecy-preserving.

HIR keeps the C3 effecting-fn `Kont*` ABI shape (ADR 0020) and the
C4.4 task/scope/spawn/await forms — the desugar must round-trip every
C0–C4 program with identical runtime behaviour (D9).

**[C5.1 amendment — the HIR *desugar* (dispatch resolution,
monomorphisation, explicit drops) is DEFERRED post-1.0.** C5.1a (1/N)
shipped only the thin HIR *seam* (codegen consumes an `HirProgram`
bundle via `program()` / `drop_plan()`); the thick desugared HIR above
is post-1.0. See the D3 escape-hatch tag for the evidence + rationale.]

### D2. MIR stage — `sentinel-mir` + `mir_query`.

Lower HIR function bodies to a **minimal SSA/CFG**: basic blocks of
three-address instructions with explicit terminators (branch / switch /
return), value defs in SSA form, and per-value type (carrying the
`secret` bit). `mir_query(db, file) -> Option<MirProgram>` chains on
`hir_query`. **Minimum scope:** exactly enough structure to host (a) the
D5 constant-time verification and (b) a first bounds-check-elision
annotation pass. The full escape-analysis / optimisation suite is
**post-1.0** (D8) — MIR is built for *analysis* and Phase D readiness,
not for an optimiser.

### D3. Codegen boundary — migrate to HIR; MIR is the analysis substrate.

`compile_to_object` migrates to lower from **HIR** rather than
`TypedProgram`. This is a *mechanical re-target*, not an SSA-consuming
rewrite: HIR is shaped like the desugared `TypedProgram` codegen already
computes in its pre-walk (dispatch pre-resolved, instances materialised,
drops explicit), so codegen *sheds* its monomorphisation pre-walk +
dispatch-resolution + drop-derivation logic rather than gaining
complexity. **MIR is the SSA substrate the analyses (D5, bounds-check
elision) run on; its results annotate the HIR/codegen path.** The
ADR 0009 §6.1 "`mir → codegen`" edge — codegen lowering SSA directly —
is realised at 1.0 as "MIR analyses inform codegen-from-HIR"; a full
codegen-consumes-MIR SSA lowering is a **post-1.0 follow-on** (D8).

**Escape hatch (ADR 0025 D2, recorded as the documented fallback):** if
the HIR re-target threatens the C5 budget, keep `compile_to_object` on
`TypedProgram`, lower a thin MIR from `TypedProgram` solely for the D5
verification, and implement constant-time emission (D4) as a codegen
sub-pass. The *capabilities* (D4 + D5) are the 1.0 bar; the pipeline
shape is not. Invoking the hatch is an amendment at C5.1/C5.2 close, not
a silent deviation.

**[C5.1 amendment — ESCAPE HATCH INVOKED (2026-05-29).** Evidence:
codegen couples to the typed tree at ~295 `TypedExprKind` / 342
`TypedExpr` references across 90 signatures (28 expr variants) — a
thick-HIR migration is a multi-session, high-risk rewrite of nearly the
whole 6272-line backend, and it is **not required** for the 1.0
constant-time capability. Decided with the developer: codegen STAYS on
the typed program (reached via the thin C5.1a seam,
`HirProgram::program()`); MIR (D2) lowers from the typed program for the
D5 verification; constant-time emission (D4) is a codegen pass. The
thick-HIR desugar (D1) + codegen-consumes-HIR/MIR migration are
**post-1.0** (they remain Phase-D-valuable). C5.1a closes at the seam;
the remaining C5 work is C5.1b (MIR) → C5.2 (constant-time).]

### D4. Constant-time secret emission (the headline, C5.2).

`secret T` must be constant-time at the machine level, not just type-
rejected:

  - **Secret arithmetic / bitwise ops lower to constant-time
    instructions** and are shielded from timing-variable lowering — no
    secret operand may feed a variable-latency instruction (integer
    division/remainder is the live case; `SecretDivisor` already
    rejects the obvious form, D5 catches the rest).
  - **Branch-free selection.** A `select` on a `secret` condition lowers
    to a branch-free form (`cmov` on x86-64, conditional-select on
    aarch64, or a bitmask `(m & a) | (!m & b)`) — never an LLVM `select`
    the backend may turn back into a branch. The C3.1 `SecretBranch`
    rejection (no `if` on a secret) **stays** at C5.2 minimum — the
    constant-time-by-construction surface is preserved and the branchless
    primitive is the supported way to choose on a secret. (Lifting
    `SecretBranch` into an auto-`select` desugar is a candidate stretch
    item, taken only if the go/no-go program needs secret `if`.)
  - **Speculation barriers (ADR 0008).** Insert an `lfence`-class
    barrier (x86-64 `lfence`; aarch64 `csdb`/`ssbb`) where a secret
    crosses a speculation boundary — notably immediately after a
    bounds check that gates a secret-dependent memory access — so a
    mis-speculated path cannot transmit secret data.

### D5. Constant-time verification pass (belt-and-suspenders, on MIR).

A MIR pass that **proves** the emission achieved constant-time. Taint-
propagate `secret` forward from sources (secret-typed defs, secret op
results) through the SSA def-use graph; `declassify(e)` is the only
taint sink. Assert that **no tainted value is**: the condition of a
conditional branch/switch terminator; the index or address operand of a
memory load/store; or an operand of a variable-latency instruction. On
violation, emit a new what/why/how diagnostic `sentinel::mir::secret_leak`
naming the leaking value + the sink. This catches both anything the
source-level rejections miss and anything LLVM might reintroduce — so it
runs on the MIR that most faithfully reflects the emitted code (run
after the constant-time emission/opts, or with the relevant LLVM passes
pinned; D6/Reasoning). It is the machine-checkable expression of ADR
0008's guarantee.

### D6. Target architecture.

x86-64 + aarch64 — the brew-LLVM18 targets (working norm). Barrier
intrinsic + branch-free-select idiom are selected per target triple.
Other architectures are **post-1.0** (ADR 0025 D12). This makes the
host-arch assumption load-bearing for a *security* property, not just
convenience (ADR 0025 Consequences) — documented honestly.

### D7. Secret tracking representation.

HIR carries `Type::Secret(SecretId)` verbatim (interner-`Copy`
invariant preserved, per ADR 0016). Each MIR SSA value carries an
`is_secret` bit derived from its type; the D5 taint pass seeds from
those bits and propagates through ops (any op with a secret operand
yields a secret result, except `declassify`, which clears it). No new
surface syntax; `secret` / `declassify` are unchanged from C3.1.

### D8. Out of scope at C5.1/C5.2 (post-1.0 or later).

Full escape analysis; the optimisation suite beyond a first bounds-
check-elision annotation; **codegen lowering SSA directly** (the full
`mir → codegen` rewrite — C5.1 keeps codegen on HIR); the Cranelift
debug backend; non-x86/arm targets; constant-time **variable-index**
secret memory access (oblivious access / full-array scan stays the
programmer's job and `SecretIndex` stays rejected); lifting
`SecretBranch` (stretch, D4). Multi-shot continuations and the other
standing C2/C3 follow-ons remain deferred.

### D9. Phase-go + fixtures.

  - **`c51` behaviour-preservation (the HIR/MIR go bar):** the *entire*
    existing pass suite still runs identically through
    `types → hir → mir → codegen`. The migration ships only when no
    `tests/pass/` fixture changes its exit code / stdout and the
    `tests/repro.rs` objects stay byte-identical — the desugar is
    transparent or it is wrong.
  - **`c52_secret_ct` (constant-time go bar):** a branch-free secret
    primitive — e.g. a constant-time MAC/tag comparison (XOR-accumulate
    over two `secret` byte arrays → `secret bool`) and/or a masked
    `cswap` — that compiles, runs, and **passes** the D5 verification;
    inspecting the emitted asm shows no secret-dependent branch.
  - **`c52_secret_leak` (UI):** a program that routes a secret to a
    branch/index the source rejections don't catch (e.g. via a path the
    optimiser could exploit) → the D5 pass rejects it with
    `sentinel::mir::secret_leak` (an `insta` UI snapshot, per C5.0 D11).

### D10. Sub-phase split.

| Sub   | Title                                                           | Risk   | Est.        |
|-------|-----------------------------------------------------------------|--------|-------------|
| C5.1a | HIR *seam* only — `lower_to_hir` + codegen consumes `HirProgram`. | low    | DONE (1/N)  |
|       | Thick HIR *desugar* + codegen migration → post-1.0 (escape hatch;|        |             |
|       | see the D1/D3 amendments).                                       |        |             |
| C5.1b | `sentinel-mir` + `mir_query` — SSA/CFG lowered from the typed     | high   | 1-2 sessions|
|       | program (via the seam) for the D5 verification + bounds-check    |        |             |
|       | elision scaffold.                                               |        |             |
| C5.2a | Constant-time emission (D4): branch-free select, secret-op       | high   | 1-2 sessions|
|       | shielding, ADR 0008 speculation barriers (x86-64 + aarch64).    |        |             |
| C5.2b | D5 verification pass + `secret_leak` diagnostic + phase-go       | medium | 1 session   |
|       | (`c52_secret_ct` + `c52_secret_leak`); ADR 0026 flip.           |        |             |

Total: ~4-7 sessions — consistent with ADR 0025's C5.1 (2-4) + C5.2
(2-3) band. C5.1a (the codegen re-target) is the single riskiest step;
the escape hatch (D3) bounds the downside.

## Reasoning

**Why HIR is lower-risk than it looks.** The desugar hoists work
codegen already does ad-hoc (mono pre-walk, dispatch resolution, drop
placement). Migrating codegen to HIR is a re-target of its *input
shape*, not a new algorithm, and it *removes* code from the 6272-line
`compile_to_object`. The behaviour-preservation bar (D9 `c51`) — every
pass fixture identical, every object byte-identical (the C5.0 D8
`repro.rs` guard) — turns "did the refactor regress anything?" into a
mechanical, suite-wide check.

**Why MIR-as-analysis-substrate, not codegen-from-MIR, at 1.0.** The
1.0 bar is the *capability* (constant-time `secret`, D4+D5), not the
ADR 0009 §6.1 pipeline shape. SSA is genuinely needed for the D5
verification's def-use taint propagation, so MIR is built for real — but
forcing a full SSA-consuming codegen rewrite would put the security
capability hostage to the largest possible refactor. Building MIR for
analysis + keeping codegen on HIR delivers D4/D5 while leaving the full
`mir → codegen` lowering as a clean post-1.0 step (and a Phase D asset).

**Why the verification pass (D5) is the load-bearing security artifact.**
Type-level rejection (C3.1) is a *source*-level guarantee; the optimiser
operates below it. A `select` can become a branch; a masked computation
can be strength-reduced; a bounds check can be hoisted past a barrier.
The only honest "this binary is constant-time" claim comes from checking
the IR that reflects the emitted code. D5 is that check; D4 is what makes
it pass. Shipping D4 without D5 would be the same paper guarantee C3.1
already gives.

**Why run D5 late (post-emission / pinned opts).** Verifying the *pre*-
optimisation MIR would prove nothing about the shipped binary. The pass
must see the constant-time emission and must not be invalidated by a
later LLVM pass — hence D6's pinned/per-arch lowering and running the
verification on the MIR that mirrors the emitted code. This couples the
guarantee to the toolchain, which is why D6 fixes the target set.

## Consequences

### Positive
- The pipeline finally matches ADR 0009 §6.1 (HIR + MIR populated);
  codegen sheds its pre-walk; Phase D self-hosting gains the MIR it
  needs. `secret` gets real runtime teeth — Sentinel's headline promise.
- The D5 pass makes constant-time *machine-checkable*, not aspirational.

### Negative
- C5.1a is a real refactor of working codegen; the behaviour-
  preservation bar is strict but the blast radius is the whole back end.
  The escape hatch is the pressure valve.
- The constant-time guarantee couples to x86-64/aarch64 + the pinned
  LLVM lowering (D6) — a security property now depends on the toolchain
  assumption, not just convenience.

### Neutral
- No C0–C4 surface-syntax change; `secret`/`declassify` are unchanged.
  C5.1 is invisible to user programs (same behaviour, new IR beneath);
  C5.2 is invisible except that more secret code now provably stays
  constant-time (and the D5 pass may reject previously-accepted code that
  leaks below the type level — surfaced as a new diagnostic, not a crash).

## Alternatives considered

- **Codegen consumes MIR (full §6.1 shape) at 1.0.** The "correct" end
  state, but it makes the security capability hostage to the largest
  refactor. Deferred to post-1.0 (D8); D3 builds toward it.
- **Escape hatch as the primary plan (constant-time as a codegen
  sub-pass, no real HIR/MIR).** Tempting for budget, but ADR 0025 D2
  reserved it as fallback, and SSA is genuinely wanted for D5 + Phase D.
  Kept as the documented fallback (D3), not the default.
- **Lift `SecretBranch` to an auto-`select` desugar now.** Rejected for
  the minimum: the branchless primitive already expresses constant-time
  choice, and the go/no-go's needs (MAC compare, `cswap`) are bitwise,
  not branch. A stretch item if the program demands secret `if`.

## Revisit

PROPOSED until C5.2 closes. Per-D triggers:
- **D1/D3**: revisit at C5.1a — if the codegen-to-HIR re-target over-runs
  or threatens the behaviour-preservation bar, invoke the D3 escape
  hatch via amendment.
- **D2**: revisit at C5.1b — if the minimal MIR proves insufficient for
  D5, widen it (still short of the post-1.0 optimiser).
- **D4/D6**: revisit at C5.2a — barrier selection + branch-free idiom per
  arch may need amendment once the emitted asm is inspected.
- **D5**: revisit at C5.2b — taint precision (false positives on
  legitimately-declassified flows) may need the sink set refined.

## Appendix: estimated implementation footprint

Indicative, refined from ADR 0025's appendix:

| Workstream                                   | LOC estimate |
|----------------------------------------------|--------------|
| `sentinel-hir` (types + desugar + `hir_query`) | ~600-1000  |
| codegen re-target to HIR (mostly deletions)  | ~-200 / +400 |
| `sentinel-mir` (SSA/CFG + `mir_query`)         | ~700-1200  |
| Bounds-check-elision annotation (minimal)    | ~150-300     |
| Constant-time emission (D4, per-arch)        | ~400-800     |
| Verification pass (D5) + diagnostic          | ~300-500     |
| Fixtures (`c51` reuse + `c52_*`)               | ~150         |
| **Total**                                    | **~2300-4350** |

Within ADR 0025's HIR/MIR (~1600-3000) + constant-time (~600-1200)
bands. The re-target's deletions partly offset the new IR crates.
