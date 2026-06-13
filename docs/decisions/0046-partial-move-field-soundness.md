# ADR 0046: Partial-move-through-field soundness (per-(VarId, FieldPath) move state)

Status: **PROPOSED** — flips to ACCEPTED-WITH-AMENDMENTS as the slices land (the borrow
checker first, then the selfhost mirror), recording deviations as numbered amendments.

Closes the **partial-move-through-field-projection double-free** documented in
`docs/borrow-check-limitations.md` (the one *under*-rejection / soundness gap, as opposed
to the over-rejection ergonomics deferred to ADR 0018). It is the review action plan's
**P1.2** and the highest-value post-self-host-port engineering: three of four external
reviewers, and the project's own limitations doc, flag this as a hard blocker for any
"memory-safe" claim.

## Problem

Postfix `.field` on a Move-typed binding is **non-consuming** by design (C2.3) — `p.x +
p.y` is a common shape that consuming `p` on the first field access would break. But with
RAII drop shipped (C2.4/C2.5), passing a **Move-typed field by value** to a fn that drops
it causes a **double-free** at the binding's own drop:

```sentinel
struct Pair { items: [i64], tag: i64 }
fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] }   // drops xs at return
fn main() -> i64 {
    let p: Pair = Pair { items: [10, 20], tag: 7 };
    let used: i64 = consume_arr(p.items);   // p.items's { len, ptr } copied in;
                                            // consume_arr frees it at return
    used + p.tag                            // ACCEPTED today (UB)
}                                           // main drops p → frees p.items AGAIN
```

Confirmed live: the borrow checker **accepts** this program; at runtime the second free
traps (exit 133 / SIGTRAP on current macOS — the allocator no longer silently masks it).

The cause is that `consume_arr(p.items)` reads `p` through a non-consuming projection, so
`p` is never marked Moved and is fully dropped (including `items`) at `main`'s exit. The
fix is **per-field move tracking**, not "consume the whole binding on projection" (which
would reject `p.x + p.y`).

## Decision

### D1. Per-(VarId, field-index) move state.

The borrow checker tracks, in addition to the whole-binding move set, a **partial-move
set** of `(VarId, field_index)` pairs. When a **Move-typed field** `p.field` is read in a
**consuming context** (passed by value to a fn, returned, or otherwise moved), mark
`(p, field_index)` Moved — *not* the whole `p`. A non-consuming read (`p.tag` in
arithmetic, `p.items[i]` as a copy receiver) does **not** move.

### D2. Use-after-(partial-)move.

Any later access of a moved field path — consuming **or** non-consuming, since the field's
heap memory is gone — surfaces `UseAfterMove` keyed on the field. A read of a *different*,
un-moved field (`p.tag`) is accepted. A **whole-binding move** of a partially-moved
binding (`consume(p)` after `p.items` moved) is rejected (`UseAfterMove` on `p`) — a
partially-moved value cannot be moved as a whole (it would double-free the moved field).
A whole-binding move implicitly subsumes all field state (every field goes with `p`).

### D3. DropPlan carries the partial-move set; codegen skips moved fields.

`DropPlan` gains `moved_fields: BTreeMap<FnId, BTreeSet<(VarId, u32)>>` (`u32` =
field index). Codegen's recursive struct-field drop (`emit_drop_struct_fields`) skips a
field whose `(binding-VarId, field-index)` is in the set — so the consumer's drop is the
only free. The whole-binding `moved_sources` path is unchanged (a fully-moved binding is
skipped entirely; partial moves skip only the named fields).

### D4. Branch merging.

A field moved in *either* arm of an `if/else` is conservatively Moved after the merge (the
same rule the whole-binding move state already uses), so codegen never double-frees on
either path. The partial-move *union* (for the DropPlan) grows monotonically within a fn
analysis and is never reset by the branch snapshot/restore.

### D5. Scope (MVP) + refinements.

**In scope:** single-level field projections (`p.field`) on a directly-named binding,
for any Move-typed field (`[T]`, `Vec<T>`, `String`, a Move struct/enum field). This
covers the reproducer + the common shape.

**Deferred refinements** (each sound-by-over-rejection until landed — the checker stays
conservative, never accepts UB):
  - **Deep paths** (`p.a.b`) — a projection through ≥2 fields. MVP: a consuming read of
    `p.a.b` conservatively moves the whole `p.a` field (over-rejects re-use of `p.a.c`),
    or — simplest — only the outermost field is tracked. Recorded as an amendment when a
    fixture demands it.
  - **Index projections** (`xs[i]` of an array binding consumed by value) — element moves
    aren't tracked; consuming `xs[i]` is out of scope (the corpus doesn't do it).
  - **Match-binding field moves** — a `match` arm that moves a payload field.

### D6. The selfhost mirror.

This change **moves the oracle**: the `snc borrow` dump (which dumps `DropPlan`) gains the
partial-move set, and codegen's drop emission changes — so both the **borrow differential**
(`sentinel_borrow_checker_matches_oracle_on_corpus`) and the **codegen differential** must
be re-validated, and `selfhost/borrow.sentinel` + `selfhost/types.sentinel` (the drop
emission) must mirror the new logic under the established lock-step discipline, with both
fixed-point paths preserved. The existing corpus is unaffected (no current fixture consumes
a Move-typed field), so the mirror lands with the new fixtures, not before.

## Sequencing

1. **`snc` borrow checker** (this crate): the per-field move state + `UseAfterMove` on
   field paths + `DropPlan.moved_fields`. Verified by Rust unit tests + ad-hoc fixtures
   (the reproducer now runs correctly — `consume_arr` owns + frees `p.items`, `main` skips
   it → exit 37, no double-free; the use-after-partial-move variant is rejected). The
   existing corpus differentials stay green (no existing fixture triggers the new path).
2. **`snc` codegen** (`sentinel-codegen`): `emit_drop_struct_fields` skips moved fields.
   Verified leak-free / trap-free on the reproducer.
3. **`borrow_dump.rs`**: dump the partial-move set (extends the `snc borrow` oracle).
4. **Corpus fixtures + selfhost mirror** (`borrow.sentinel` + `types.sentinel`): add the
   reproducer (pass, exit 37), the use-after-partial-move (ui reject), and a
   `p.x + p.y`-shape regression (pass, still accepted); mirror the borrow + codegen logic;
   re-bless the differentials; preserve both fixed-point paths.
5. Update `borrow-check-limitations.md` (close the gap), README Status (drop the caveat),
   STATE.md.

## Consequences

- The single live memory-safety **under**-rejection in the borrow checker is closed; the
  remaining limitations are all over-rejections (ergonomics, deferred to ADR 0018) — sound.
- The reproducer transitions from accepted-but-UB to **accepted-and-correct** (the field
  move is real; the consumer owns the field; the producer skips it at drop).
- Codegen's drop emission is now partial-move-aware — the DropPlan is the single source of
  truth for both the routing (which frees happen) and the skip (which don't), so they
  cannot diverge.
