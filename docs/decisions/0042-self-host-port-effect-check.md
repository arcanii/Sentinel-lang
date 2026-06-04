# ADR 0042: Phase D self-host port — (5/N) effect-check-in-Sentinel

Status: **PROPOSED** — the fifth sub-phase of the self-host port (ADR 0031 D5 /
ADR 0038 D9), after the lexer (1/N), parser (2/N, ADR 0039 → ACCEPTED), resolve
(3/N, ADR 0040 → ACCEPTED), and types (4/N, ADR 0041 → ACCEPTED). Ports the
**effect-check** analysis pass to Sentinel: the type-checked program → each fn's
**effective effect row** (the union of the effects it performs, after handler/scope
discharge), with the annotation-as-constraint + `main`-must-be-empty invariants —
differentially validated against a Rust `snc effects` oracle over the `tests/pass` +
`tests/ui` corpus.

It is the **smallest stage since the lexer** — `crates/sentinel-effect-check/src/lib.rs`
is **758 lines** (~7% of types). It was the owner's pick for (5/N) over borrow-check
(2227), HIR/MIR (1134), and codegen (8263): most-self-contained-first (ADR 0031 D5
sequences "types (+ borrow / effect checks) → HIR/MIR → codegen"), and the cleanest
output to differential-dump (a per-fn effect-row map, not a transform).

⚠ **Two design questions are PROBE-GATED (the ADR-0040/0041 discipline) — settle
before the (5a) consumer build:** **D3** (how the Sentinel stage obtains its
resolved/typed input) and **D4** (the effect-row representation + the fixed-point
WITHOUT `Vec` index-assign). See ## Revisit.

## Amendments

- **A1 — the D4 probe SETTLED + the `snc effects` oracle LANDED (5a, first half).**
  **D4 (the bitmask fixed-point) is EMPIRICALLY VERIFIED** (`/tmp/probe_ec`, orchestrator-run
  — compiled, ran, leak-checked clean): effect rows as i64 BITMASKS with the
  rebuild-per-sweep fixed-point driven by **RECURSION** (`fixpoint(rows: Vec<i64>) ->
  Vec<i64>` — take the rows array by value, `sweep` into a FRESH `Vec<i64>` pushed in
  FnId order, element-wise `rows_eq` compare, converge or recurse) compiles + runs +
  is **leak-free** (the `Vec<i64>` by-value-through-recursion auto-drops via RAII; no
  index-assign, no loop-reassignment of a Move binding). The bit ops confirmed: union
  `|`, discharge via `x & (mask ^ (0 - 1))` (bit-not as `^ -1` — avoids `~`, which the
  1.0 surface lacks). The probe returned the expected `106` (a 3-fn graph: an annotated
  callee's row propagating through the call graph to `main`, plus the discharge check).
  ⚠ **`vec_to_array` is the `Vec<u8>`→`[u8]` bridge ONLY** — it type-errors on
  `Vec<i64>`, which simply auto-drops (no explicit reclaim needed). **D3 (input
  acquisition) stays lean-(a) self-contained** — to be confirmed at the Sentinel build
  (the proven types pattern; lower risk).
  - **The `snc effects` oracle landed** (`run_effects` + `effects_dump.rs`, `8b55a5f`):
    parse → resolve → check → `effect_check`, dumping `effective_rows` one line per user
    fn in FnId order (`(fn #<id> <name> <effect-name>…)`), exiting NONZERO on any error
    (incl. effect errors) so the differential skips rejects. 4 goldens (`tests/effects.rs`).
    **Corpus characterised: 122 clean-effect fixtures** (the (5b) phase-go target — types
    ok + effects ok), 1 types-ok-but-effect-error (`c37_perform_outside_handle`, skipped),
    18 type-rejected. Verified on c44 (`double Async` / `main` discharges via scope),
    c35/c37 handlers, and call-graph inference (an unannotated caller picks up a callee's
    row). **NEXT: the Sentinel `selfhost/effects.sentinel` (5a, second half)** — the
    self-contained fn/effect tables + the annotation re-scan + the bitmask fixed-point +
    the dump, on the simple call-graph + annotation cases.

## Decision

### D1. Goal.

Port the effect-check stage. The Rust pass (`effect_check(&TypedProgram) ->
(EffectCheckedProgram, Vec<EffectError>)`) computes, for every fn, its **effective
effect row** = (its annotation, if any, else) the union of its callees' effective
rows + its own `perform`/`spawn`/`.await` effects, minus the effects discharged by an
enclosing `handle` / `scope concurrent`. It then validates two invariants (ADR 0019
D2 + D13): an annotated fn's inferred row ⊆ its annotation (`EffectAnnotationMismatch`),
and `fn main`'s row is empty (`UnhandledEffect`). The **output we port + dump is the
`effective_rows` map** (`BTreeMap<FnId, BTreeSet<EffectId>>`); the errors are out of
scope (D5/D7 — diagnostic parity has never been ported).

### D2. The oracle — a canonical effect-row dump (`snc effects <file>`).

A new `run_effects` driver subcommand + an `effects_dump.rs` module (mirroring
`run_types` / `types_dump.rs`): parse → resolve → check → **effect_check** → dump.
The dump is the `EffectCheckedProgram.effective_rows`, **one line per fn in FnId
order** (= source order for user fns; builtins #0–#13 are effect-free and omitted —
only fns actually in `program.fns` are dumped, as the Rust map keys are):

```
(fn #14 double Async)
(fn #15 main)
```

— `(fn #<FnId> <name>` then each effect NAME in **EffectId order** (the `BTreeSet`
order — deterministic, no interner-order obligation, like the type render), then `)`.
A fn with an empty row dumps `(fn #N name)`. Effects render by their source name
(`effect_decls[eid].name`); the built-in `Async` (auto-registered, id = user-effect
count) renders `Async`.

**Exit code = the skip signal:** `snc effects` exits NONZERO if ANY pass errors —
parse / resolve / type errors (as today) OR an effect error (annotation-mismatch /
unhandled-effect). So the corpus differential SKIPS every rejected fixture (the
happy-path discipline, D7), exactly as the parser/resolve/types differentials skip
their rejects. On a clean fixture it dumps `effective_rows` + exits 0. Golden-tested
(`tests/effects.rs`) + corpus-differential (`sentinel_effect_checker_matches_oracle_on_corpus`).

### D3. How the Sentinel stage obtains resolved/typed input (PROBE-GATED).

effect-check consumes the *typed* program, but it needs only a SUBSET: the fn table
(FnIds + names + per-fn annotation row), the effect table (EffectIds + names, incl.
the appended `Async`), and the per-fn body walked for its effect-contributing nodes
(Call → callee FnId; Perform → EffectId; Spawn/Await → Async; Scope/Handle →
discharge sets). It does NOT need full types (no inference, no interner).

Candidates (probe settles, as types refined D3 to self-contained):
  - **(a) self-contained** — import only `parser.sentinel`; re-derive the fn table +
    effect table (the types/resolve pass-1 scan) + each fn's annotation (re-scan the
    `! { … }` row the parser currently SKIPS) + walk the AST for the call graph. **Lean
    (a):** types proved a self-contained `TyCtx` is cleaner than threading two contexts
    across a module boundary, and effect-check needs even less shared state.
  - **(b) reuse types.sentinel / resolve.sentinel as a D.6 module** — get the resolved
    Call→FnId / Perform→EffectId disambiguation for free. Heavier coupling.

The call-graph disambiguation (Call vs ResumeKont; the Qcall→enum-construct/qcall-impl
split) matters only insofar as it changes which node contributes effects — the
effect-checker can replay resolve's lightweight rules (a callee that resolves to an
in-scope var is a ResumeKont, contributing no own effect, etc.). **Probe (a) end-to-end
before the build.**

### D4. The effect-row representation + the fixed-point (PROBE-GATED — the central call).

**An effect row = an i64 BITMASK** (bit `i` set ⟺ EffectId `i` is in the row). Effects
are FEW (the whole corpus has a handful; ≪ 64), so one i64 holds any row. This makes
every operation a bit op: **union = `|`**, **annotation ⊇ inferred = `(inferred & ~annotation) == 0`**,
**discharge** (scope removes Async, handle removes its handled set) = **`row & ~mask`**.
Far simpler than the typer's hash-consed interner.

⚠ **The fixed-point must NOT use `Vec` index-assign** (`rows[fid] = newrow` is
`IndexAssignNotSupported`, ADR 0041 A1). The Rust pass updates a `HashMap<FnId, row>`
in place across sweeps. The Sentinel port **REBUILDS the rows array each sweep**: a
sweep reads the previous `Vec<i64>` of per-fn row masks and pushes a fresh `Vec<i64>`
(one mask per fn, in FnId order) — annotated fns push their annotation mask, unannotated
push `collect_inferred(body, oldrows)`. Iterate until a sweep changes nothing
(element-wise compare). ⚠ **Drive the sweep loop by RECURSION, not a `while` that
reassigns the rows binding** — reassigning an outer Move binding (`oldrows = newrows`)
in a loop trips the moved-in-loop rule (the ADR-0039 finding); a recursive
`fixpoint(rows) -> rows` that returns the converged Vec sidesteps it. The corpus call
graphs are tiny, so O(fns²) sweep cost is negligible. **Probe the rebuild-per-sweep
fixed-point (Vec-by-value recursion + element compare) before the build.**

`collect_inferred(body, rows)` is the walk: a consuming recursive `match` over the AST
(the typer's `dump_texpr` shape, but ACCUMULATING a mask instead of emitting a dump) —
Call → `mask | rows[callee]`, Perform → `mask | (1 << effect_id)`, Spawn/Await →
`mask | (1 << async_id)`, Scope → `walk(body) & ~(1 << async_id)`, Handle → `(walk(body)
& ~handled_mask) | walk(arms) | walk(return_arm)`, everything else structural.

### D5. Error detection — OUT OF SCOPE (happy-path, as every prior stage).

The two `EffectError`s (annotation-mismatch, unhandled-effect) are REJECTIONS. The
Sentinel stage does NOT detect or reproduce them — diagnostic parity has never been
ported (ADR 0039/0040/0041 D7). The `effective_rows` are computed identically whether
or not the oracle found errors (errors don't stop the analysis), so on the CLEAN
fixtures the differential compares, the Sentinel dump matches without any error logic.
The oracle's nonzero exit on an effect error is what removes the rejecting fixtures
from the compared set (D2).

### D6. Sub-slicing.

A small stage — likely **two slices**:
  - **(5a)** the `snc effects` oracle + a minimal Sentinel effect-checker: the fn +
    effect tables, the bitmask fixed-point over the call graph (Call edges only), the
    per-fn dump. Covers effect-free programs + fns whose rows come purely from calls +
    annotations. (Settles the D3/D4 probes end-to-end.)
  - **(5b)** the full effect walk — `perform` / `spawn` / `.await` contributions +
    `scope` / `handle` discharge — then the full-corpus phase-go (D8). (Merge/split as
    the build reveals, per the 0040/0041 cadence A2–A13.)

### D7. Out of scope.

Diagnostic/error parity (the `EffectError` messages + spans); the salsa-tracked
`effect_check_query`; method / impl-method / class-init effect propagation (the Rust
pass leaves these as follow-ons — a `MethodCall` walks receiver + args but does NOT
union the method's own row; the port matches that, so it stays faithful); cross-module
(single-file, as resolve/types).

### D8. Phase-go.

`sentinel_effect_checker_matches_oracle_on_corpus` (mirroring
`sentinel_typer_matches_oracle_on_corpus`): build the Sentinel effect-checker, sweep
`tests/pass` + `tests/ui`, skip oracle-rejected fixtures, assert byte-equal
`effective_rows` dumps over the entire clean-effect-checking set. Green flips ADR 0042
→ ACCEPTED.

## Reasoning

effect-check is the right (5/N): it is the **next stage in the pipeline order** (ADR
0031 D5), the **smallest remaining** (758 lines), and its output is a **clean
structural map** that the established differential method dumps directly — no new
oracle philosophy. Its analysis-pass nature (it REJECTS rather than transforms) is
neutralised by the happy-path discipline: we dump the `effective_rows` (always
computed) and let the oracle's exit code skip the rejecting fixtures, exactly as
type-error fixtures are skipped today. The genuinely new bit is the **fixed-point**,
which the bitmask-rebuild-per-sweep model makes both simple (bit ops) and
index-assign-free (the one Sentinel constraint it must respect) — hence the D4 probe.
The flat-pool fn/effect tables + the consuming-`match` AST walk are proven idioms from
resolve/types; the port reuses them wholesale.

## Consequences

### Positive
- The smallest port stage since the lexer; the bitmask model is markedly simpler than
  the typer's interner.
- Completes the front-end analysis trio (resolve + types + effect-check) — after this,
  the remaining stages (borrow-check, HIR/MIR, codegen) are the back-end half.
- The effect fixed-point is a reusable pattern for any future whole-program analysis.

### Negative
- The fixed-point is a new control shape (rebuild-per-sweep + recursion) not used in
  prior stages — the D4 probe de-risks it.
- An analysis pass dumps less "interesting" output than a transform; the differential
  still fully exercises the call-graph walk + discharge logic.

### Neutral
- Diagnostic parity stays deferred (D5/D7), consistent with every prior stage.
- Builtins are effect-free and omitted from the dump (they're not in `program.fns`).

## Revisit

- **D3 (input acquisition)**: probe self-contained re-derivation (lean a) vs a
  types/resolve module reuse; record the choice as an amendment (the ADR-0041 A3
  pattern, where D3 refined to self-contained at the build).
- **D4 (bitmask fixed-point)**: probe the rebuild-per-sweep recursion (Vec-by-value
  return + element-wise convergence compare) + the annotation re-scan (mapping the
  `! { Name, … }` row names to EffectId masks) leak-free, before the (5a) build.
- **D6 slice boundaries**: merge/split as the build reveals (resolve/types did, A2–A13).
- The effect-row dump FORMAT (D2) is the oracle's call — finalise it when `run_effects`
  lands (a golden pins it), as `snc resolve`/`snc types` did.

## Context

The lexer (1/N), parser (2/N), resolve (3/N), and types (4/N) proved the
**differential-oracle method** end-to-end: a canonical `snc <stage>` dump the Sentinel
stage reproduces byte-for-byte, diffed over the corpus, rejecting fixtures skipped.
effect-check is the next stage in the ADR 0031 D5 self-host order. The Rust pass it
ports — `crates/sentinel-effect-check` — is documented inline (ADR 0019 D2 + D13, ADR
0020 handlers, ADR 0024 D5 concurrency discharge). See ADR 0038 for the port's spine,
`docs/agent-protocol.md` for the probe discipline, and the auto-memory
`sentinel_selfhost_port` for the running stage-by-stage record.
