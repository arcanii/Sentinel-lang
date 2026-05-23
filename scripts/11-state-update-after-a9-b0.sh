#!/usr/bin/env bash
# 11-state-update-after-a9-b0.sh - restructure docs/STATE.md to cover both
# the broker (Phase A through A9) and effects-proto (Phase B0). Two
# top-level crate sections per Option B agreed in the B0 follow-up
# discussion. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f docs/STATE.md ]]; then
  echo "ERROR: docs/STATE.md missing" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== STATE.md UPDATE START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

# Backup the current STATE.md so a botched patch is recoverable.
cp docs/STATE.md docs/STATE.md.bak
echo "  BACKUP docs/STATE.md -> docs/STATE.md.bak"

cat > /tmp/sentinel_state_update.py <<'PYEOF'
#!/usr/bin/env python3
"""Restructure STATE.md to cover broker + effects-proto under two
top-level sections (Option B). Idempotent."""
from pathlib import Path

ROOT = Path.cwd()
STATE = ROOT / "docs" / "STATE.md"

current = STATE.read_text()
# Detection: if the file already contains the "Phase B0" tracker header
# we treat it as already-migrated and exit.
if "## Section B — sentinel-effects-proto" in current:
    print("  UNCHANGED docs/STATE.md (already restructured)")
    raise SystemExit(0)

NEW = """# STATE.md — Sentinel Implementation Status

This document tracks what is actually built. When it disagrees with
HANDOVER.md, STATE.md is the source of truth. New contributors (or
new chat sessions) should be able to read this file and understand
the current state of the workspace without re-reading every commit.

The workspace now has two non-stub crates with very different
purposes: a production-shape memory subsystem (broker) and a
research-grade interpreter (effects-proto). They are tracked in
separate sections below. The remaining workspace members listed in
HANDOVER §3.2 are scaffold-only.

Last updated: phase A9 (broker cleanup) and B0 (effects-proto
scaffold) landed.

---

## Section A — sentinel-broker

Production-shape crate. The broker provides generational arenas,
two allocation strategies, scoped budgets, secret memory, recording,
and diagnostics. Intended for adoption beyond the Sentinel compiler
itself per HANDOVER §4.

### A.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| A0    | Dev dependencies (thiserror, tracing, proptest)    | Done   | 9c7474d |
| A1    | Foundation types (ArenaId, Generation, ...)        | Done   | 9c7474d |
| A2    | Bump arena + generational handles + destroy_arena  | Done   | 9c7474d |
| A3    | Pluggable AllocStrategy (Bump + Slab), builder     | Done   | f606d19 |
| A3.5  | Per-slot generations + slab recycling              | Done   | 37ab02b |
| A4    | Scoped allocation budgets                          | Done   | 493ee7b |
| A5    | Stats, list_arenas, where_is                       | Done   | 15d751c |
| A6    | Recording mode (event log, ring buffer)            | Done   | 2e8fb8b |
| A7    | Secret-memory policy (mlock + zero-on-free)        | Done   | f3170bf |
| A8    | Validation examples / integration demos            | Done   | 683981d |
| A9    | Fallible builders + BrokerError carries OS detail  | Done   | 755e710 |

Test coverage as of A9: 69 tests (62 lib + 5 integration + 2
proptest). The count dropped from 70 → 69 between A8 and A9 because
one test depended on `BrokerError: Copy`, which A9 removed. The
removed test was redundant with `error::tests::error_messages_are_informative`.

Doctests: 1 passing + 6 ignored. Clippy clean under `-D warnings`
across crate and examples.

### A.2 Crate Layout

    crates/sentinel-broker/
      Cargo.toml
      benches/arena_bench.rs
      src/
        lib.rs            crate root + re-exports
        arena.rs          Arena (strategy + recorder + counters)
        broker.rs         Broker, ArenaHandle, within_budget
        budget.rs         Budget, BudgetScope, BudgetArenaBuilder
        builder.rs        ArenaBuilder (bump/slab + try_bump/try_slab as of A9)
        error.rs          BrokerError enum (no longer Copy as of A9)
        handle.rs         Handle<T>, HandleRef<'a, T>
        ids.rs            ArenaId, BudgetId, SlotIndex, Generation,
                          SlotGeneration, monotonic counters
        recording.rs      Recorder, Event (A6)
        secret.rs         SecretPolicy, SecretStrategy (A7)
        stats.rs          BrokerStats, ArenaSummary, HandleLocation (A5)
        strategy/
          mod.rs          AllocStrategy trait + AllocOk/SlotPtr/StrategyKind
          bump.rs         BumpStrategy
          slab.rs         SlabStrategy (freelist + per-slot generations)
      examples/
        token_bucket.rs       high-frequency slab allocation
        request_pipeline.rs   bump-per-request under budgets, with recorder
        credential_store.rs   secret slab with STRICT/LENIENT, raw zero-on-free verification
      tests/
        integration.rs    end-to-end API tests
        proptest.rs       property-based isolation/invalidation tests

### A.3 Public API Surface

All re-exports from `sentinel_broker`:

- Core: `ArenaHandle`, `Broker`, `Arena`, `Handle`, `ArenaBuilder`
- IDs: `ArenaId`, `BudgetId`, `Generation`, `SlotGeneration`, `SlotIndex`
- Strategies: `AllocStrategy`, `StrategyKind`
- Budgets (A4): `Budget`, `BudgetScope`, `BudgetArenaBuilder`
- Stats (A5): `ArenaSummary`, `BrokerStats`, `HandleLocation`
- Recording (A6): `Event`, `Recorder`
- Secret (A7): `SecretPolicy`, `SecretStrategy`
- Errors: `BrokerError`

#### A.3.1 Broker

- `Broker::new()`
- `Broker::with_recorder(Arc<Recorder>)` (A6)
- `broker.create_arena(name, capacity) -> ArenaHandle`
- `broker.arena(name).capacity(n).bump() -> ArenaHandle` (panics on
  misuse; see also `try_bump`)
- `broker.arena(name).slab(slot_size, slot_align, slot_count) -> ArenaHandle`
- `broker.arena(name).secret(policy).bump()/.slab(...)` (A7)
- `broker.arena(name).try_bump() -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.arena(name).try_slab(...) -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.destroy_arena(id)` invalidates all handles
- `broker.live_arena_count()`
- `broker.stats() -> BrokerStats` (A5)
- `broker.list_arenas() -> Vec<ArenaSummary>` (A5, sorted by id)
- `broker.where_is(&handle) -> Option<HandleLocation>` (A5)
- `broker.recorder() -> Option<&Arc<Recorder>>` (A6)
- `broker.within_budget(cap, |scope| { ... })?` (A4, nestable)

#### A.3.2 Arena / ArenaHandle

- `arena.alloc(value) -> Result<Handle<T>, BrokerError>`
- `arena.free(&handle)` (slab only; bump returns `NotImplemented`)
- `handle.get() -> Result<&T, BrokerError>`
- `handle.is_live()`, `arena_id()`, `slot()`, `slot_generation()`
- `arena.__raw_slot_bytes_for_diagnostics(slot)` (A8, doc-hidden,
  forensic tooling only)

#### A.3.3 BrokerError variants

`UseAfterFree`, `UseAfterFreeSlot`, `OutOfMemory`, `InvalidSlot`,
`UnknownArena`, `BrokerPoisoned`, `BudgetExceeded`, `NotImplemented`,
`SecretMemory { reason: String, os_errno: Option<i32> }` (A9 shape),
`BuilderMisuse { reason: &'static str }` (A9).

As of A9, `BrokerError` no longer derives `Copy`. `SecretMemory`
carries the underlying OS error number where available (previously
the OS errno was only logged via `tracing::warn!`).

### A.4 Design Invariants

These are properties the test suite enforces; future changes must
preserve them.

1. Arena destruction invalidates every handle. `destroy_arena(id)`
   removes the broker's `Arc<Arena>` from its map and calls
   `Arena::invalidate()`, which advances generation atomically.
   `Handle::get()` then returns `BrokerError::UseAfterFree`.

2. Per-slot generations defeat ABA. Each slab slot has its own
   generation counter. Reusing a slot increments it, so a handle to
   the prior occupant returns `BrokerError::UseAfterFreeSlot`.

3. Bump strategy never recycles. `BumpStrategy::free` returns
   `NotImplemented`; only `SlabStrategy` supports free. Bump's whole
   point is O(1) bulk free via arena destruction.

4. Budgets pre-charge reserved capacity. `arena("a").capacity(N).bump()`
   inside `within_budget(cap, ...)` charges N to the budget chain
   BEFORE the arena exists. Reservation, not usage, is what counts.
   Nested budgets charge both inner and every ancestor.

5. Budget refunds are atomic. If `try_charge` walks the chain and
   exceeds any cap, all prior charges in that walk are refunded
   before returning `BudgetExceeded`.

6. Recording never affects behaviour. If no recorder is attached,
   the hot path is an `Option::None` branch. If recording fails
   (mutex poisoned), the event is dropped silently — recording is
   observation, not enforcement.

7. All counters use `Ordering::Relaxed`. Snapshots from `stats()`
   may show momentary inconsistency across fields under concurrent
   load. This is expected and acceptable.

8. `Broker::with_recorder` is construction-time only. No runtime
   swap, no `AtomicPtr`. The recorder is set once and read on every
   event-emitting path via `&Option<Arc<Recorder>>`.

9. (A9) Panicking and fallible builders coexist. `.bump()` and
   `.slab()` panic on misuse and are kept for tests and demos that
   want construction-time failure surfacing as a panic. `.try_bump()`
   and `.try_slab()` are exact structural twins returning `Result`.
   New code prefers the fallible variants.

### A.5 Known Limitations / Tech Debt

- `Arena::with_strategy` is `#[allow(dead_code)]` — kept as a
  convenience wrapper but only the recorder-aware variant
  `with_strategy_and_recorder` is used.
- `Recorder` uses `Mutex<Vec<Event>>`; under very high concurrent
  allocation it serializes through one mutex. Acceptable for now.
- Bounded-ring `Recorder::record` uses `Vec::remove(0)` (O(n)) on
  overflow. A `VecDeque` would be better for larger caps.
- No benchmark gate in CI. `benches/arena_bench.rs` exists but is
  not exercised on every PR.
- Doctests are ignored to avoid pulling test-only types into the
  public examples. Should be tagged `no_run` and fleshed out before
  publishing.
- (BACKLOG §0.1 remaining) Bump `slot_size_hint` is `None`; the
  `SlotInfo.size` field exists but is dead-code. Either return it
  from a bump-side override or document diagnostics as slab-only.
- (BACKLOG §0.1 remaining) `Event` variants are not `#[non_exhaustive]`.
  Consumers serializing events would see field additions as
  breaking changes.

---

## Section B — sentinel-effects-proto (Sentinel-Mini)

Research-grade tree-walking interpreter. Built to validate
Sentinel's effect-system design before committing to the Phase C
production compiler per HANDOVER §5. The crate is explicitly
expected to be thrown away or rewritten once its lessons are
absorbed.

### B.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| B0    | Scaffold: lex + parse + eval, no types or effects  | Done   | d090ca1 |
| B1    | HM type inference, letrec, span-tracked errors     | Planned |        |
| B2    | Effect rows and effect declarations                | Planned |        |
| B3    | Effect handlers (handle .. with ..)                | Planned |        |
| B4    | Secret T qualifier and constant-time check         | Planned |        |
| B?    | Broker-as-value-heap integration (bonus)           | Planned |        |

Test coverage as of B0: 23 tests (5 lexer + 5 parser + 8 eval + 5
integration). Clippy clean under `-D warnings`. No doctests yet.

### B.2 Crate Layout

    crates/sentinel-effects-proto/
      Cargo.toml
      src/
        lib.rs            re-exports + `run()` convenience + `MiniError`
        lexer.rs          logos tokeniser, `Token`, `LexError`
        ast.rs            `Expr`, `BinOp` (plain enums, Box-allocated children)
        parser.rs         hand-written recursive descent, precedence climbing,
                          `ParseError`
        eval.rs           tree-walking interpreter, persistent `Env`,
                          `Value`, `EvalError`
      tests/
        integration.rs    end-to-end pipeline tests

### B.3 Language Surface (B0)

Pure expression calculus. Everything is an expression. No
statements, no types, no effects, no `secret`, no recursion.

Grammar (informal):

    expr      := let | if | lambda | compare
    let       := "let" IDENT "=" expr "in" expr
    if        := "if" expr "then" expr "else" expr
    lambda    := "fn" "(" IDENT ")" "=>" expr
    compare   := add ( ("==" | "<" | ">") add )?
    add       := mul ( ("+" | "-") mul )*
    mul       := app ( ("*" | "/") app )*
    app       := atom ( "(" expr ")" )*
    atom      := INT | BOOL | IDENT | "(" expr ")"

Comments are `// to end of line`.

Single-parameter lambdas only. Multi-parameter functions are written
curried (`fn(x) => fn(y) => ...`).

### B.4 Public API Surface

All re-exports from `sentinel_effects_proto`:

- `Expr`, `BinOp` (AST)
- `Token`, `LexError`, `lex(source) -> Result<Vec<Token>, LexError>`
- `ParseError`, `parse(&[Token]) -> Result<Expr, ParseError>`
- `Value`, `EvalError`, `Env`, `eval(&Expr, &Env) -> Result<Value, EvalError>`
- `MiniError`, `run(source) -> Result<Value, MiniError>` (lex+parse+eval)

### B.5 Design Decisions (B0)

1. Hand-written recursive descent over `chumsky` / `lalrpop`. Per
   HANDOVER §3.3 (production compiler) and our B0 reasoning: the
   grammar will churn as effects and qualifiers are added; hand-written
   parsers absorb grammar changes more cheaply than combinator chains.
2. Plain `Box`-allocated AST. No `bumpalo`. The language is small
   enough that heap traffic is irrelevant for a research artifact.
3. Persistent `Arc`-cons-list environment for closures. Standard
   Crafting-Interpreters shape. May be revisited if broker-as-value-heap
   integration lands.
4. Errors are not span-tracked at B0. Spans land with B1 alongside
   the type checker so error highlighting can be meaningful from
   the start.
5. `BrokerError`-style two-flavour API (panicking + fallible) is
   NOT adopted here. Effects-proto is throwaway research code;
   panicking-only is acceptable and simpler. If a panic-free API
   becomes useful for embedding, it lands then.

### B.6 Known Limitations (intentional at B0)

- No recursion. `let f = fn(n) => ... f ...` produces
  `Unbound("f")`. The integration test
  `pipeline_recursion_via_y_combinator_would_need_letrec` documents
  this loudly and flips to a correctness check when `letrec` lands
  in B1.
- No types. Type errors surface at evaluation time, not at compile
  time. (B1 fixes.)
- No effects. The whole reason this crate exists. (B2 onward.)
- No REPL, no driver binary. Library-only.
- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable
  for a research artifact; revisit if body-clone becomes a hot path
  or if the broker becomes the value heap.
- `Value` does not derive `Eq` (closures aren't comparable); the
  error types also drop `Eq` to keep them embeddable in each other.
  They remain `Debug + Clone + PartialEq`, which is what tests use.

---

## Conventions

### Build & Test Commands

The standard check suite for a clean tree, applied per-crate:

    cargo build -p <crate>
    cargo clippy -p <crate> --all-targets -- -D warnings
    cargo nextest run -p <crate>           # or `cargo test -p <crate>`
    cargo test -p <crate> --doc

All four must pass for any commit on `main`. Current expected counts:

  - sentinel-broker:        69 tests + 1 doctest
  - sentinel-effects-proto: 23 tests + 0 doctests

### Script Convention

Every code change lands via a script under `scripts/`:

  - `NN-<phase>.sh`          primary generator/patch for a milestone
  - `NNa-`, `NNb-`, ...      follow-up patches (lint fixes, etc.)
  - `NNz-commit-<phase>.sh`  pre-commit checks + commit creation

Scripts are committed alongside source changes so contributors can
see exactly what was patched, in what order. They are also a useful
debugging aid: re-run the most recent `NNa-` under `set -x` to
inspect a patch.

Output convention: each script prints `======` delimited sections
(BUILD / CLIPPY / TESTS / DOC TESTS). When asking for help, paste
those sections back.

### Working Norms (from HANDOVER §0.1)

- Trust STATE.md (this file), not the git log.
- Terminal heredocs: single-level only. Use `cat > /tmp/script.py`
  then `python3 /tmp/script.py` instead of nested heredocs.
- Avoid `set -e` and bare `exit` in pasted scripts — they can close
  the user's terminal. Use `return 1 2>/dev/null || exit 1` and
  rely on `PIPESTATUS[0]` for return codes.
- Small patches, build between each. Land type/trait changes first,
  build, then implementations, build, then tests.
- Honest disclosure beats confident-but-wrong.
- Examples held to `-D warnings`.
- Check before overwriting docs (`p.exists()` and merge patterns).

---

*End of document. Update on every commit that changes phase
status, public API surface, or invariants.*
"""

STATE.write_text(NEW)
print("  UPDATE docs/STATE.md (restructured under Option B)")
PYEOF

python3 /tmp/sentinel_state_update.py
RC=$?

echo
echo "====== STATE.md UPDATE DONE (rc=$RC)"
echo
echo "====== DIFF SUMMARY"
wc -l docs/STATE.md docs/STATE.md.bak
echo
echo "Use 'diff docs/STATE.md.bak docs/STATE.md | head -80' to inspect, then"
echo "remove the backup with 'rm docs/STATE.md.bak' before committing."
echo
echo "====== STATE.md UPDATE END"
