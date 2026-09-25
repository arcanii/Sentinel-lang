# ADR 0046: Partial-move-through-field soundness (per-(VarId, FieldPath) move state)

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A4). `snc` (the borrow checker + both codegen
backends) and `scg` (the self-hosted mirror) both close the partial-move-through-field
double-free; the borrow + codegen differentials are byte-identical over the whole corpus
and both bootstrap fixed points hold. Amendments below record the deviations from the
PROPOSED plan. A4 (2026-09-25), below, adds `snc` borrow-checker rules and one `scg` parity fix.

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

## Amendments

**A1 — the `.ll` oracle moved too (not just `borrow_dump.rs`).** D6 named `borrow_dump.rs`
as the oracle that gains the partial-move set, but the codegen differential's oracle is
`snc llvm` (`llvm_dump.rs`), a *separate* textual-`.ll` backend from the inkwell
`sentinel-codegen`. The `snc` feat updated only inkwell, so `llvm_dump.rs` still emitted
the double-free. Closing D6 required mirroring the field-skip into `llvm_dump.rs` too
(`emit_frame_drops` threads each binding's partial-move set into `emit_drop_for_binding`;
the Struct + GenericInstance field walks skip a moved field; nested drops get an empty
set) — so the `.ll` oracle matches inkwell (pinned by `llvm_behaviour_matches_inkwell`)
*and* the Sentinel mode-4 emission. Both oracle dumps move; otherwise re-blessing the
codegen differential would have had nothing to re-bless.

**A2 — the `scg` direct-Var detection is a new `mvbv` channel, not an AST peek.** The
oracle records a partial move only when the field target is `if let Var(base)`. Sentinel
`match` cannot peek the AST node (no catch-all *binding* pattern — only `_`; no nested
patterns; no `&Expr` matching), and the existing "last resolved Var" channels
(`cg_lastvid` / `mir_lastvid`) are written only under their mode guards. So the mirror adds
`TyCtx.mvbv` — reset to -1 at every `dump_texpr` entry, set to the resolved VarId by the
`Var` arm — the mode-independent analogue of `cg_lastvid`. The FieldAccess arm reads it
right after the (forced-non-consuming) target dump: `>= 0` iff the target was a directly-
named binding. Verified exact (not merely conservative) by byte-identical borrow + codegen
differentials over the whole corpus + the self-host fixed point.

**A3 — an existing fixture already exercised the path.** D6 (and the `snc` feat) stated the
existing corpus was unaffected because "no current fixture consumes a Move-typed field."
That held for *whole-binding* moves but missed **returning a field by value**:
`c17_go_no_go`'s `fst<A,B>(p) -> A { p.first }` and `snd -> B { p.second }` are partial
moves of a (generic) field in tail/return position. The borrow oracle now dumps them
(`#2.0` / `#3.1`) and the Sentinel side reproduces them byte-for-byte — so an existing
fixture, not only the new reproducer, validates the mirror. The codegen `.ll` for `c17` is
unchanged (its monomorphised `Pair<i64, i64>` fields are scalar → never dropped → no skip
observable), which is why the codegen differential and both fixed points stayed green at
the `snc` feat before this mirror landed.

**Scope unchanged.** Single-level field projections on a directly-named binding (D5);
deep paths (`p.a.b`), index projections, and match-binding field moves remain deferred (A4
below refuses the first two and treats the third as a move of the scrutinee) —
each sound-by-over-rejection in `snc`, and `scg` mirrors `snc` exactly (it records + dumps
but never rejects; error parity is out of differential scope, ADR 0043 D5/D7) on every
program the differentials compare; registers D116 and D117 record two shapes, found since,
on which the two drop plans differ.

**A4 (2026-09-25) — the moves D5 deferred, and moves through a reference, are refused or
tracked (register D114).** The checker walked a deep path (`p.a.b`) or an index projection
(`xs[i]`) moved by value as a non-consuming read and recorded no move, and it walked a move
out through a reference (`*r`, `(*r).a`), which D5 does not mention, the same way; and moving
a payload bound by value out of `match e` recorded nothing on `e`. So none of them stopped the
same value being moved or read again after it had a new owner.

The checker now, for a Move-typed value taken by value:

- refuses a move out through a reference — `*r`, or a field or element reached through one
  (`sentinel::borrow::move_out_of_borrow`);
- refuses a field more than one level below a named binding
  (`sentinel::borrow::move_out_of_nested_field`); moving the enclosing field into a binding
  first is two tracked moves;
- refuses an element, or a field of one, of a collection a named binding holds
  (`sentinel::borrow::move_out_of_element`); borrowing it is the alternative;
- leaves a projection rooted at a temporary alone, since nothing else owns it;
- treats moving a payload binding — whole, or by a field — as a move out of the `match`'s
  scrutinee: of `e` for `match e`, of the field for `match s.f` (a D1 partial move), a refusal
  as above for a scrutinee reached through a reference, a deeper field or an element, and
  register D61's `move_out_of_self` for one rooted at `self`. The scrutinee is marked moved
  for the use checks only, not in the drop plan: an enum's drop frees its payload box and
  never the payload's own heap (ADR 0032 D6, amendment A1), so the drop plan was already
  right, and landing ADR 0032's deferred recursive payload drop will have to account for a
  payload a `match` moved.

Two payloads of one arm, or two fields of one payload, are different parts of one payload,
and moving both is not a second move of it. Anything else that takes the scrutinee's payload
counts against it: the scrutinee consumed, its payload moved by a nested `match` of it, or
either of those on some path through an earlier `if` or `match` in the arm, since after the
merge the checker cannot tell which path ran. A payload moved or read after that is refused
as a use after a move. Reassigning the scrutinee does not detach the old payload's bindings
from it: the walk cannot tell a reassignment on every path from one on some paths, and
detaching them for the second would let the old payload be moved twice, so it detaches them
for neither (an over-rejection when the reassignment is unconditional;
docs/borrow-check-limitations.md).

Four shapes of the same family, each accepted before, are closed with it:

- a field of a binding, or a `match` payload, moved while the binding is borrowed — the rule
  R14 applies to a whole binding (`sentinel::borrow::move_while_borrowed`), one level down;
- a scrutinee consumed, or its payload moved through a binding of another arm, while a
  payload binding of it is borrowed: the binding holds part of that payload, so a reference
  to it reaches into what the move takes (`move_while_borrowed`, naming the scrutinee);
- a borrow `&s`, or a method called on `s`, after a field of `s` was moved — including by
  the method's own arguments, which run first (`s.m(consume(s.a))`);
- a move of a collection or a receiver in its own index or method arguments (`v[consume(v)]`,
  `s.m(eat(s))`), which runs before the element is read or the method called.

A comparison operand or a discarded expression statement that is a place — a binding, or a
field, element or deref path rooted at one (`(*r).next == null`, `xs[0];`) — only reads that
place, so neither the refusals above nor a payload move apply to the place itself; a payload
binding read that way still needs its scrutinee to hold the payload. A compared or discarded
field of a binding is not recorded as moved out of it for the use checks. So a later use of
that field — compared again, read, borrowed or moved — and a move of the whole binding, each
refused before, are accepted; and A4's own rule against using a partly moved binding does not
count it, so `n.m()` and `&n`, accepted before, still follow `if n.next == null { .. }`.
The drop plan still records that field, as it did before, so it is not dropped with its
binding (register D117). A compared or discarded whole binding is still recorded as moved, as
before, which refuses a later use of it. The operand of a deref that is not itself a place is
a computed value: under `*f(&n, x);` or `*f(&n, x) == 7`, a move in a call's arguments, a block's `let`s or tail, an
`if`'s branches or a struct literal is a move, as it is anywhere else.

As for a whole binding, a payload move is a move of the scrutinee to the other rules too: a
`while` loop that moves the payload of a scrutinee declared outside it is refused by ADR 0036
D8 even when it reassigns the scrutinee, as the state-machine form `st = match st { .. }` does;
and a generic body that takes an element or a `*r` of type `T` by value is refused, since `T`
may be a Move type.

Pinned by ten `tests/ui/c25_*` fixtures (`c25_move_out_of_borrow`,
`c25_move_out_of_nested_field`, `c25_move_out_of_element`, `c25_match_payload_moved_twice`,
`c25_move_under_a_computed_deref`, `c25_payload_used_after_scrutinee_moved`,
`c25_payload_after_conditional_reassign`, `c25_payload_moved_after_a_merge`,
`c25_method_arg_moves_receiver_field`, `c25_borrowed_payload_scrutinee_moved`), the pass
fixture `c25_compared_field_is_only_read`, and twenty-one unit tests in
`sentinel-borrow-check`; twenty-nine mutations of these rules are each caught. Apart from the
one acceptance above, A4 only rejects, and the drop plan's sets do not change, so the emitted
IR of a program the checker accepted before does not move, and `scg` (which records moves and
never rejects, ADR 0043) needs no mirror of the refusals. It does get one parity fix, because the
new `c25_move_out_of_nested_field` fixture exposed it: `snc llvm` compiles a refused program
anyway, and for `s.i.a` `scg` recorded `a`'s field index as a partial move of `s` — its field
arm read a per-node tracker that the nested target `s.i` had left set — where the oracle
records nothing. A target that is itself a field access now records nothing in `scg` either,
and the codegen differential is byte-identical on the fixture. It refuses programs that
compiled before, so it is at least a minor version (ADR 0076 D2); no program that was already
in the corpus is among them. A matched `snc borrow` sweep of all 489 `.sentinel` files changes no result outside
the slice's fourteen new fixtures: eleven of them go from accepted to refused, the pass
fixture from refused to accepted, and two that the loop rule refused before are now reported
first by another rule. A matched `snc build` of the 121 entries `snc borrow` cannot load
(programs, exporting libraries, self-hosted module roots, and library modules through a
program importing them) changes none; a
matched `snc llvm` of every file both compilers emit changes no byte; and each of the ten
self-hosted module roots, merged into one program, passes.
