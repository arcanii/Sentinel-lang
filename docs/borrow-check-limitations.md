# Borrow checker limitations at C2.5 close

This document tracks known imprecision in `sentinel-borrow-check`
at C2.5 close. Two flavours:

  - **Over-rejection** — the checker rejects a program that flow-
    sensitive analysis (NLL / Polonius) would accept. Workaround
    exists; the program author has to rewrite. Migration plan in
    ADR 0018 closes most of these.
  - **Under-rejection (soundness gap)** — the checker accepts a
    program that has UB. The author cannot tell from the
    diagnostic surface that there's a problem. These are bugs and
    will be closed by a follow-on sub-phase or ADR before any
    work that depends on borrow-check soundness lands.

Each entry below: a reproducer, the verdict, the underlying
cause, and the closure plan.

## Over-rejection: borrow lives past last use

The canonical NLL case.

```sentinel
fn main() -> i64 {
    let mut x: i64 = 5;
    let r: &i64 = &x;
    let snapshot: i64 = *r;   // last use of r
    x = 10;                   // REJECTED
    print(snapshot + x)
}
```

Diagnostic at C2.5: `sentinel::borrow::write_while_borrowed` on
`x = 10;`.

Cause: `FnCtx.places[x]` carries an `UntilScope(depth)` shared
borrow rooted at `let r = &x;`. The borrow stays alive until the
enclosing scope pops. Lexical analysis can't see that `r`'s last
use was the previous line.

Workaround: wrap the borrow + use in an inner block.

```sentinel
let snapshot: i64 = {
    let r: &i64 = &x;
    *r
};
x = 10;  // accepted — r's scope ended at the closing brace
```

Closure: ADR 0018 D5's three-step Polonius migration. Step .b
ships precision behind a flag; step .c flips the default.

## Over-rejection: field-disjoint borrows

Borrowing one field blocks any mutation through the parent.

```sentinel
struct Pair { x: i64, y: i64 }
fn main() -> i64 {
    let mut p: Pair = Pair { x: 1, y: 2 };
    let rx: &i64 = &p.x;
    let _u: i64 = *rx;
    p.y = 99;     // REJECTED
    print(p.y)
}
```

Diagnostic at C2.5: `sentinel::borrow::write_while_borrowed` on
`p.y = 99;`.

Cause: place tracking is binding-precise, not field-precise.
`&p.x` records a shared borrow keyed by `p`'s VarId; the
write-conflict check looks up `p` and sees the active borrow.
Polonius supports field-precise places (a place is a sequence
of projections from a base local); Sentinel's fact generator
will start with binding-precise places and refine in a follow-
on sub-phase (ADR 0018 D6 out-of-scope).

Workaround: split the borrow scope or rebind via copy.

```sentinel
let xv: i64 = p.x;   // copy out
p.y = 99;            // now no &p.x is alive
print(p.y + xv)
```

Closure: ADR 0018 step .a fact generator + post-Polonius field-
precise places ADR.

## Soundness gap: partial move through field projection + drop — ✅ CLOSED (ADR 0046)

**Status: fully closed — `snc` (the Rust bootstrap compiler) AND `scg`
(the self-hosted compiler) by ADR 0046** (per-(VarId, field-index)
partial-move state). `snc` closed it in both the borrow checker and
both codegen backends (the inkwell `sentinel-codegen` and the `snc
llvm` `.ll` oracle); `scg` mirrors it in `selfhost/types.sentinel`
(the move recorder + the mode-1 dump + the mode-4 drop field-skip;
`selfhost/borrow.sentinel` is a thin wrapper). The borrow + codegen
differentials are byte-identical over the whole corpus and both
bootstrap fixed points hold. The write-up below is retained as the
original gap description.

The under-rejection case. C2.3's docstring noted "Partial moves
through field projection — `let inner = p.x` doesn't consume p.
This is slightly unsound for non-Copy fields but benign at C2.3
since drop hasn't shipped." Drop shipped at C2.4 + C2.5(a). The
gap is no longer benign.

```sentinel
struct Pair { items: [i64], tag: i64 }
fn consume_arr(xs: [i64]) -> i64 { xs[0] + xs[1] }
fn main() -> i64 {
    let p: Pair = Pair { items: [10, 20], tag: 7 };
    let used: i64 = consume_arr(p.items);   // p.items pointer
                                            // passed by value;
                                            // consume_arr drops
                                            // its xs param at
                                            // return → free
    print(used + p.tag)                     // ACCEPTED
}
```

What actually happens at runtime:

  1. `p.items`'s `{ len, ptr }` fat pointer is copied into
     `consume_arr`'s `xs` param.
  2. `consume_arr` reads `xs`; at fn return, `xs` is dropped →
     `sentinel_free(xs.ptr)` fires.
  3. Back in `main`, `p` is still Live. `p.items.ptr` now
     references freed memory.
  4. `p.tag` is a primitive; reading it is fine.
  5. At main return, `p` is dropped. Recursive field drop walks
     `p.items` and calls `sentinel_free(p.items.ptr)` —
     **double-free** of the same pointer.

Empirically the program exits 0 on macOS — the platform allocator
doesn't abort on double-free of small allocations. Other
allocators (jemalloc, glibc with `MALLOC_CHECK_=3`) would abort.
This is undefined behavior under the C standard regardless.

The corollary use-after-free (reading `p.items[0]` after
`consume_arr` returns) similarly compiles, doesn't currently
crash, and is UB.

Cause: postfix `.field` on a Move-typed binding is non-consuming
in C2.3's design. The choice was deliberate — `p.x + p.y` is a
common shape that consuming `p` on first field-access would
break. The fix is per-field move tracking, not "consume on
projection."

**Closure (DONE in `snc` — ADR 0046):** per-(VarId, field-index)
move state. On `consume_arr(p.items)`:

  - Mark `(p, items)` as Moved (NOT the whole `p`) —
    `FnCtx.moved_fields` + the `DropPlan.moved_fields` union.
  - At main's drop, `emit_drop_struct_fields` skips the `items`
    field (it's in the partial-move set).
  - On any later read of `p.items[i]` (consuming or not), surface
    `BorrowError::UseAfterMove`.
  - On any later read of `p.tag`, accept (the tag field is not
    moved). A whole-binding move of a partially-moved binding is
    rejected (can't move a partial).

The reproducer above is now **accepted and correct** (exit 37,
leak-free: `consume_arr` owns + frees `p.items`, `main` skips it).
MVP scope = single-level field projections on a directly-named
binding; deep paths (`p.a.b`), index projections, and match-binding
field moves are deferred refinements (each sound-by-over-rejection;
ADR 0046 D5). This was roughly half the work of the Polonius
migration's fact generator, conceptually independent and shipped on
its own.

**`scg` mirror (DONE — ADR 0046 D6):** `selfhost/types.sentinel` now
records the partial move (a Move-typed field consumed by value on a
directly-named base — the direct-Var base detected via the new
`mvbv` channel), dumps the `#<vid>.<field>` set, and elides the field
in the mode-4 recursive drop; the `snc borrow` + `snc llvm` oracle
dumps emit the same set. Three corpus fixtures (the reproducer at
exit 37, a non-consuming-read regression, a use-after-partial-move
reject) plus the pre-existing `c17_go_no_go` (which returns a generic
field by value) exercise it. The self-hosted `scg` no longer has the
gap.

## Out of scope at this doc

- Closures, async, traits, lifetime parameters — none of these
  exist in Sentinel at C2.5. Their borrow-check semantics are
  defined when the features land.
- Effects + secrets — Phase B's surface; no borrow-check
  interaction yet at the C2 type system.
- `unsafe` blocks + raw pointers + `Cell`/`RefCell` — ADR 0017
  D12 out of scope.

## Tracking

Each row here gets closed by a specific ADR or sub-phase:

| Limitation                          | Closes at                       |
|-------------------------------------|----------------------------------|
| Borrow past last use                | ADR 0018 step .b / .c (Polonius) |
| Field-disjoint borrows              | Post-Polonius field-precise places ADR |
| Partial move + drop unsoundness     | ✅ CLOSED (ADR 0046) — `snc` + `scg` both, differentials byte-identical |
