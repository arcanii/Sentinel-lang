# ADR 0043: Phase D self-host port — (6/N) borrow-check-in-Sentinel

Status: **ACCEPTED** — the sixth sub-phase of the self-host port (ADR 0031 D5 /
ADR 0038 D9), after the lexer (1/N), parser (2/N, ADR 0039), resolve (3/N, ADR 0040),
types (4/N, ADR 0041), and effect-check (5/N, ADR 0042) — all ACCEPTED. **COMPLETE
(A1+A2): `selfhost/borrow.sentinel` matches `snc borrow` byte-for-byte over the ENTIRE
clean-borrow corpus (123/123 fixtures, `sentinel_borrow_checker_matches_oracle_on_corpus`
— the D8 phase-go), leak-free; `snc types` stays byte-identical (the move analysis is a
pure side-pass).** The FIRST stage to REUSE a prior one (the back-half "share the typed
program" foundation). Ports the
**borrow-check** analysis pass to Sentinel: the type-checked program → the **`DropPlan`**
(per fn, the set of **moved-source VarIds** — bindings used in a consuming/move position,
which codegen must NOT drop), differentially validated against a Rust `snc borrow` oracle
over the `tests/pass` + `tests/ui` corpus.

**Owner-chosen as (6/N)** over the transform half (HIR/MIR → codegen): it continues the
ADR 0031 D5 analysis-pass order and has the same clean differential shape as effect-check
(dump a computed structure, skip rejects). But ⚠ **borrow-check marks an ARCHITECTURAL
INFLECTION**: unlike the five self-contained front-end stages (each re-derived a cheap
SUBSET), borrow-check needs the **FULL typed program** — its move analysis classifies
every value Copy-vs-Move via the expression's `Type` (`is_copy_type`). **The owner chose
to REUSE the typed program** (refactor `types.sentinel` to expose its computed output as
a reusable library) over re-deriving inference — setting the back-half's "share the typed
program" foundation. **The REUSE MECHANICS are the central PROBE-GATED question (D3).**

## Amendments

- **A1 — (6a) D3 SCOUTED + SETTLED to FUSE (lean a); the precise build plan.** Studying
  `sentinel-borrow-check` confirmed: `moved_sources` is recorded only at **`Var` nodes in
  a CONSUMING position whose type is non-Copy** (`check_and_record_move(id, ty)`) — never
  at temporaries (a `Call`/struct-lit result), and a field/index/method receiver + a
  `&`/`&mut`/`*` operand are NON-consuming. The branch-merge is a plain **UNION** (moved in
  either arm ⇒ included), so the moved-sources output needs **no per-path state** — far
  simpler than the full borrow checker (the use-after-move state machine is out of scope,
  D5). ⚠ **The move-recording MUST FUSE into types' `dump_texpr` walk** (lean a, not a
  separate re-walk): a `Var`'s VarId is resolved by `sc_lookup` against the scope, and
  match/handler-arm payload binds are assigned + truncated DURING that walk — a second walk
  can't recover them (it'd re-assign different VarIds). So the move-recording rides the one
  walk where scope + VarId + env are all live.
  - **Mechanism (avoids threading a param through all 35 `dump_texpr` sites):** a TyCtx
    field **`consuming: bool`** with SAVE/RESTORE at the ~12 positions that CHANGE it —
    set FALSE around a Field/Index/Method receiver + a `&`/`&mut`/`*` operand + the
    if/while condition; set TRUE around each call/struct-lit/array/enum-construct/perform
    arg (the arg helpers `dump_targs`/`dump_sfields`/`dump_array_elems`/`dump_args_*`) +
    a `let`/assign-RHS + the match scrutinee; INHERIT (leave) at if-branches, block tails,
    match-arm bodies, scope/handle bodies. The `Var` arm: `if (*c).consuming { record_move(c,
    vid) }`. **`dump_texpr` is otherwise UNTOUCHED → `snc types` stays byte-identical by
    construction** (the recording is a pure side-effect; `consuming` never affects the dump).
  - **`record_move(c, vid)`**: push `(curfn, vid)` into a side-table (`mvf`/`mvv`) iff
    `curfn >= 0` (set only in `type_fn` — top-level fn bodies; methods aren't in
    `program.fns`, so `curfn = -1` there → not recorded) AND **`is_move_type(env[vid])`**.
    `is_move_type(h)`: scalars + Ref (kind 2) + Task (kind 12) = Copy; Secret (3) / Nullable
    (4) follow their inner; Array/Struct/Enum/Class/TypeParam/Generic/Vec = Move. (No insert
    dedup needed — the dump iterates VarIds ascending + tests membership, naturally deduped
    + sorted to match the BTreeSet oracle.)
  - **The reuse refactor:** `types.sentinel`'s `main` pipeline → a `pub fn build_program(…)
    -> TyCtx` that runs passes 1/1.5/2 filling the ctx (incl. the type-dump buffer as a new
    `out` FIELD + `mvf`/`mvv`); `main` = `let c = build_program(…); print(c.out)`.
    `borrow.sentinel` (3-deep `use`: borrow→types→parser) = `let c = build_program(…);
    print(dump_moves(c))` (`dump_moves` emits `(fn #<id> <name> #<vid>…)`). ⚠ verify a
    by-value `TyCtx` return works (the ADR 0041 A1 D3 probe verified cross-module pub
    struct/fn; returning a big struct is the new bit — probe if it balks).
  - **⚠ DO IN ORDER:** (1) add the move computation to `types.sentinel` + verify `snc types`
    123/123 byte-identical + leak-free (the side-effect is harmless); (2) the build_program
    refactor (re-verify 123/123); (3) `borrow.sentinel` + `dump_moves` + the differential.

- **A2 — `selfhost/borrow.sentinel` LANDED; ADR 0043 → ACCEPTED, the BORROW-CHECK STAGE
  (6/N) is COMPLETE.** Matches `snc borrow` BYTE-FOR-BYTE over the ENTIRE clean-borrow
  corpus — **123/123 fixtures** (`sentinel_borrow_checker_matches_oracle_on_corpus`, the
  D8 phase-go) + 5 seeds, leak-free (both `types.sentinel` + `borrow.sentinel` 123/123),
  and **`snc types` stays byte-identical** (the move analysis is a pure side-pass)
  (`d21910d`). The A1 fuse plan executed essentially as written — **(6a-i)** the move
  computation, **(6a-ii)** the `run()` refactor, **(6b)** the thin `borrow.sentinel` —
  with two oracle-revealed corrections:
  - **The reuse mechanism is `run(src, mode, result)`** (NOT a by-value `TyCtx` return):
    `types.sentinel`'s `main` pipeline → `pub fn run` that builds the type dump into a
    local + (mode 0) writes it to `result` else (mode 1) writes `dump_moves(ctx)`.
    `main` calls `run(0)`; `borrow.sentinel` is ~10 lines (`use types::run; run(inp, 1,
    &mut result)`). The D.6 chain borrow→types→parser compiles + runs leak-free — sidesteps
    the by-value-struct-return question entirely.
  - **⚠ TWO move-classification corrections from the oracle** (the rest of A1 held): (i)
    **BUILTIN call args are NON-consuming** (`len`/`push`/`vec_to_array`/`print` borrow or
    don't move their args) — only USER-fn args consume, gated by `argcons = fid >= 14` in
    the Call arm; (ii) a **`match` SCRUTINEE is NOT a move** (it's read/inspected — the
    oracle never records it), so the scrutinee walks with `consuming = false` (the arms
    still inherit the match's own consuming context, so a returned arm-payload still
    moves). The `consuming` save/restore lives at ~14 sites; the Var arm records iff
    `consuming && is_move_type(env[vid])`; `curfn` (set in `type_fn` only) keeps method
    bodies out of the dump (methods aren't in `program.fns`).
  - ⚠ idioms confirmed: the move-recording fused cleanly into the typer's walk with ZERO
    change to the type dump (a `bool` field + a side-table); the conservative branch-merge
    needs no per-path state (a plain union); the 3-deep D.6 reuse + `run`-with-`mode` is
    the template for the next back-half stages. **NEXT: (7/N) — HIR/MIR → codegen** (the
    transform half + the bootstrap fixed-point; its own kickoff ADR, reusing `types::run`'s
    typed-program foundation).

## Decision

### D1. Goal.

Port borrow-check's **`DropPlan`** computation. The Rust pass
(`borrow_check(&TypedProgram) -> (DropPlan, Vec<BorrowError>)`) walks each fn tracking
move state; at each Var used in a CONSUMING position whose type is non-Copy
(`is_copy_type` false), it records the VarId into `moved_sources_union` (and, if the
binding was already moved, emits `UseAfterMove`). The `DropPlan.moved_sources:
BTreeMap<FnId, BTreeSet<VarId>>` — the per-fn moved-from set — is **the output we port +
dump**; codegen consults it to skip dropping moved bindings (avoiding double-frees). The
`BorrowError`s (use-after-move, borrow conflicts, returns-local-ref, …) are REJECTIONS,
out of scope (D5/D7 — diagnostic parity has never been ported).

### D2. The oracle — a canonical moved-sources dump (`snc borrow <file>`). [LANDED]

`run_borrow` + `borrow_dump.rs` (mirroring `run_effects` / `effects_dump.rs`): parse →
resolve → check → **borrow_check** → dump `moved_sources`, **one line per user fn in
FnId order**: `(fn #<id> <name> #<vid>…)` (the moved VarIds in ascending BTreeSet order;
an empty set → `(fn #N name)`). Exits NONZERO on ANY error (parse/resolve/type, or a
borrow error) → the corpus differential skips rejects. **123 clean-borrow fixtures** =
the (6b) phase-go target (= the full type-clean set; NO fixture has a borrow error).
4 goldens (`tests/borrow.rs`); verified: `make_pair` moves its struct-lit args, a
field/index read is non-consuming, the if/else merge is conservative (moved in EITHER
branch → included). **Landed `4ef657d`.**

### D3. How the Sentinel stage obtains the typed program (THE central call — PROBE-GATED).

borrow-check's move analysis needs, at each Var USE: (a) its **VarId** (to record into
`moved_sources`) and (b) its **type's Copy/Move-ness** (`is_copy_type(env[vid])`). Both
are exactly what `types.sentinel` already computes — the resolve **scope** (name → VarId,
via `sc_lookup` during the walk) and the **env** (VarId → type handle) + the **interner**
(a handle's kind classifies Copy/Move: scalars/refs/nullable-of-Copy = Copy;
struct/array/Vec/enum/class/string/secret-of-compound = Move). The owner chose **REUSE**
(not re-derive — re-running inference would duplicate ~4,000 lines of `types.sentinel`).

⚠ **`types.sentinel` is today a DUMPER** (its pass-2 `dump_texpr` walks every expr with
the scope + env, emitting the type dump on the fly — the env survives, the per-expr types
do not). Reuse needs it refactored to a reusable **typed-program builder**. The MECHANICS
are the probe (two candidate shapes):
  - **(a) FUSE the move analysis into types' pass-2 walk.** `types.sentinel` already
    walks every expr with the scope + env + each Var's VarId; add "at a consuming Var use,
    record its VarId into a per-fn moved-set" as a side-output, exposed via a `pub`
    accessor. `borrow.sentinel` then re-runs types' build + dumps the moved-sets. PRO:
    ONE walk; the scope + VarIds are already in hand; minimal new code. CON: the move
    analysis lives in (or beside) the types walk — the stage boundary is logical, not
    physical.
  - **(b) SEPARATE re-walk over an exposed TyCtx.** `types.sentinel` exposes its built
    `TyCtx` (env + interner) + a `pub` build entry; `borrow.sentinel` re-parses + re-walks
    the AST doing its OWN move analysis, reading Copy/Move from the env. CON: it must
    REPLAY the scope (name → VarId) to map each Var-use to its VarId — duplicating types'
    scope tracking (the snag that makes (b) heavier than it looks).
  **Lean (a)** — the VarId + scope + env are already threaded through types' walk, so
  recording consuming-Var-uses is a small, faithful addition; `borrow.sentinel` stays
  thin (build + dump). **The probe settles (a) vs (b)** + confirms the D.6 module reuse
  (the ADR 0041 A1 D3 probe verified a `pub struct` + `pub fn` cross-module — this extends
  it to exposing the built `TyCtx`/accessors). ⚠ **The `types.sentinel` refactor MUST be
  behaviour-preserving** — re-run `sentinel_typer_matches_oracle_on_corpus` (123/123) +
  the leak sweep after, BEFORE adding borrow logic (the ADR 0041 (4f) group-order
  discipline).

### D4. The move analysis.

A consuming-`match` walk (the `walk_eff` / `dump_texpr` shape) tracking, per fn, the
moved VarIds. A Var is a moved source at a **consuming use** (a by-value call/method/
construct arg, a `let`/assign RHS that is a bare Var, a returned tail Var, a struct-lit /
array / enum-construct payload Var) **iff its type is non-Copy**. NON-consuming uses
(a field-access / index / method receiver, a `&`/`&mut` borrow, a `*`-deref lvalue) do
NOT move. ⚠ **Branch-merge is conservative** (ADR 0017 D9): a Var moved in EITHER `if`
branch (or any `match` arm) is in `moved_sources` — so the analysis is a simple
UNION over the body (no per-path move-state needed for the *moved-sources* output; the
per-path state is only for the use-after-move ERROR, which is out of scope). This makes
the port markedly simpler than the full Rust borrow checker: **we accumulate the union of
moved non-Copy VarIds, ignoring the error-detection state machine entirely** (D5).

### D5. Error detection — OUT OF SCOPE (happy-path, as every prior stage).

The `BorrowError`s are REJECTIONS; the Sentinel stage does NOT detect/reproduce them
(diagnostic parity has never been ported). `moved_sources` is computed identically with
or without errors (the Rust computes it best-effort even on failure), so on the CLEAN
fixtures the differential compares, the Sentinel dump matches with no error logic. The
oracle's nonzero exit on a borrow error removes the rejecting fixtures from the compared
set — though at the corpus level there are ZERO borrow-error fixtures in the type-clean
set, so all 123 are compared.

### D6. Sub-slicing.

  - **(6a)** the `snc borrow` oracle [LANDED] + the `types.sentinel` → reusable-library
    refactor (behaviour-preserving on the 123 types corpus) — settles the D3 probe.
  - **(6b)** `selfhost/borrow.sentinel`: the move-analysis union over the AST (consuming
    vs non-consuming positions, the Copy/Move classification via the reused interner, the
    conservative branch union) + the dump, then the full-corpus phase-go (D8). Merge/split
    as the build reveals (the 0040/0041/0042 cadence).

### D7. Out of scope.

Diagnostic/error parity (the `BorrowError` messages + spans + the per-path use-after-move
state machine); the lexical-lifetime borrow-conflict analysis (shared/mut borrow
tracking) — none of it affects `moved_sources`; the salsa-tracked `borrow_check_query`;
cross-module (single-file).

### D8. Phase-go.

`sentinel_borrow_checker_matches_oracle_on_corpus` (mirroring the effect-check phase-go):
build the Sentinel borrow-checker, sweep `tests/pass` + `tests/ui`, skip oracle-rejected
fixtures, assert byte-equal `moved_sources` dumps over all 123 clean-borrow fixtures.
Green flips ADR 0043 → ACCEPTED.

## Reasoning

borrow-check's *output* is tiny + clean (a per-fn VarId set) and the *moved-sources* half
of the analysis is a plain union (the conservative branch-merge means no per-path state) —
so the differential method + the proven flat-pool/consuming-`match` idioms apply directly.
What makes it the back-half inflection is the INPUT: it needs the full typed program. The
owner's reuse choice turns `types.sentinel` into the shared typed-program foundation the
remaining stages (HIR/MIR/codegen) will also build on — a one-time refactor that pays off
across the back half. D3 lean (a) (fuse the move-recording into types' existing walk)
exploits that types ALREADY threads the scope + VarIds + env through every expr, so the
addition is small and can't drift from the type computation.

## Consequences

### Positive
- The first REUSE of a prior Sentinel stage — establishes the back-half "share the typed
  program" architecture (vs the front end's self-contained stages).
- The moved-sources union is simpler than the full borrow checker (error state-machine out
  of scope); the output is a clean per-fn VarId set.

### Negative
- The `types.sentinel` refactor touches an ACCEPTED stage — must stay behaviour-preserving
  (the 123 corpus + leak sweep gate it). The D3 probe de-risks the reuse shape first.
- Logical-vs-physical stage boundary under lean (a) (the move-recording rides types' walk).

### Neutral
- Diagnostic parity stays deferred (D5/D7), consistent with every prior stage.
- All 123 type-clean fixtures are borrow-clean → the phase-go set = the types set.

## Revisit

- **D3 (reuse mechanics)**: probe (a) fuse-into-types'-walk vs (b) separate-re-walk;
  confirm the `types.sentinel` library refactor is behaviour-preserving (123/123 +
  leak-free) BEFORE borrow logic. Record the settled shape as an amendment (the
  0041 A3 / 0042 A1 pattern).
- **D6 slice boundaries**: merge/split as the build reveals.
- The moved-sources dump FORMAT (D2) is the oracle's call — pinned by a golden, as the
  prior stages' dumps were.

## Context

The lexer → parser → resolve → types → effect-check stages proved the
**differential-oracle method** + the self-contained front-end pattern. borrow-check is the
next stage in the ADR 0031 D5 order and the FIRST to need the full typed program — opening
the back half, where stages reuse the typed program rather than re-derive a subset. See
ADR 0038 for the port's spine, ADR 0042 for the effect-check precedent (the analysis-pass
differential shape this mirrors), `docs/agent-protocol.md` for the probe discipline, and
the auto-memory `sentinel_selfhost_port` for the running record.
