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

## Soundness gap: a reference outliving its storage — ✅ CLOSED (ADR 0017 D7)

**Status: closed in `snc`.** Every check below only ever REJECTS. Programs that
relied on the gap stop compiling — that is the point of the section — but no
program keeps compiling and emits different code, so nothing the stage
differentials compare moves and `selfhost/` needs no mirror.

References are second-class (ADR 0017 D7): a reference may not outlive the
storage it points at. C2.1 checked that rule at two places — where a
ref-typed *binding is read*, and where a function *returns* one — and the
type layer refused a `&T` written into a struct field. That left positions
where a reference could reach dead storage without passing either check.

```sentinel
fn read(r: &i64) -> i64 { *r }
fn main() -> i64 {
    read({                        // the operand is bound to nothing, so the
        let v: [i64] = [5, 5];    // binding and return checks never see it
        &v[0]                     // ACCEPTED before this work
    })
}
```

The gap was in WHERE the rule was checked, not in the rule. Each position
below is now checked, and each has its own diagnostic code so the `help`
line can name the fix that applies there:

  - `sentinel::borrow::returns_local_ref` — the exit positions. A
    statement `return`, a `match` / `scope` / method-call / qualified-call
    tail, and the `?&T` and `secret &T` spellings of each.
  - `sentinel::borrow::ref_outlives_binding` — a `let` initialized with a
    reference to storage that is already dead.
  - `sentinel::borrow::ref_outlives_assignment` — a ref binding re-pointed
    at storage that dies before it does. Checking this is what makes the
    strong update on a ref binding sound.
  - `sentinel::borrow::ref_operand_dead` — a reference consumed as a call
    argument or a deref operand, which is bound to nothing and so was
    reached by no other check.
  - `sentinel::borrow::move_while_borrowed` — a place moved out of while a
    reference into it is still live.
  - `sentinel::borrow::ref_stored_into_place` — the fail-closed backstop
    for a reference stored anywhere but a binding (a field, an element,
    through a deref). Unreachable while the type-layer rules below hold.

The type layer closed the matching storage positions, keyed on a new
`carries_ref` predicate that reaches through `?T` and `secret T` where the
old `is_ref()` saw only a bare `&T`: `ref_in_class_field`,
`ref_in_enum_payload`, `ref_in_generic_field` (a type argument that lands
in a field — a *phantom* ref type argument no field uses stays legal), and
the widened `ref_in_struct_field` / `nested_ref` gates.

`secret &T` remains a legal type (ADR 0019 D5) — it is a secret value
behind a public reference, and `carries_ref` reaching through `Secret` does
not ban it. `tests/pass/c21_nullable_secret_ref_passthrough.sentinel` pins
that.

One dead source reports once per function: the same mistake is reachable
from the binding, from every later read, and from every operand the
reference flows into, and the first report wins. Only duplicates are
dropped — a program with a dead reference is still rejected.

## Over-rejection: a block that yields a reference keeps its own borrows

When a block's VALUE carries a reference, every borrow taken inside the block is
kept alive for the enclosing binding's scope — including borrows the yielded
reference does not point at.

```sentinel
fn main() -> i64 {
    let mut v: [i64] = [1, 2, 3];
    let x: i64 = 9;
    let r: &i64 = { let s: &i64 = &v[0]; let t: i64 = *s; &x };
    v[0] = 7;          // REJECTED: cannot assign to `v` while it is borrowed
    *r + v[0]
}
```

Diagnostic: `sentinel::borrow::write_while_borrowed` on the assignment. The same
program with `&x` bound directly (no block) is accepted.

Cause: `FnCtx::pop_scope_yield` cannot tell which of the block's borrows the
yielded reference depends on, so when the block yields one it keeps them all —
`{ let s: &i64 = &v[0]; s }` really does need `v` to stay borrowed, and the
checker has no provenance to distinguish that case from the one above. Keeping
all of them is the sound direction; narrowing it by the yielded value's single
resolved source would drop a place that an if-merged reference still points at.

Workaround: don't take a borrow inside a block whose value is a reference —
split the two.

```sentinel
let t: i64 = { let s: &i64 = &v[0]; *s };   // borrow ends with this block
let r: &i64 = &x;
v[0] = 7;   // accepted
```

Closure: needs per-borrow provenance on the yielded value — the same fact
generator ADR 0018 step .a builds for Polonius.

## Over-rejection: a ref-returning method's receiver, moved in the same call

A method whose result carries a reference keeps its receiver borrowed until the
enclosing statement ends, so the receiver cannot also be moved by that statement.

```sentinel
fn sink(a: i64, k: K) -> i64 { a + k.get() }
fn main() -> i64 {
    let k: K = K::init(7);
    sink(*k.p(), k)   // REJECTED: cannot move out of `k` while it is borrowed
}
```

Diagnostic: `sentinel::borrow::move_while_borrowed` on `k`.

Cause: the receiver's auto-ref (ADR 0022 D3) is registered as a real borrow, and
because `p()` returns a reference the borrow is not transient — it is rooted for
the statement. That the value actually consumed is the dereferenced `i64`, not
the reference, is not tracked.

Workaround: bind the read before the move.

```sentinel
let a: i64 = *k.p();
sink(a, k)   // accepted
```

Closure: as above — this is the "borrow lives past last use" case wearing a
different hat, and ADR 0018's migration closes it.

## Over-rejection: borrowing a field of a temporary

A reference into a value that is not bound to anything is refused, because
there is no binding whose scope could keep the value alive.

```sentinel
struct P { x: i64 }
fn mk() -> P { P { x: 5 } }
fn main() -> i64 {
    let r: &i64 = &mk().x;   // REJECTED: ref_outlives_binding on `<temporary>`
    *r
}
```

Diagnostic: `sentinel::borrow::ref_outlives_binding`, naming the source
`<temporary>`.

Cause: `source_of_expr` is total and fails closed — a base that is not a
place resolves to `BorrowSource::Temporary`, which is never alive. A
flow-sensitive analysis could extend the temporary's lifetime to the
enclosing statement (Rust's temporary-lifetime-extension rules) and accept
some of these.

Workaround: bind the value first, then borrow it.

```sentinel
let p: P = mk();
let r: &i64 = &p.x;   // accepted — `p` is a binding with a scope
*r
```

Closure: temporary lifetime extension is not specified for Sentinel; it
would be its own ADR. Until then this errs on the side of safety.

## Over-rejection: moving an outer binding, or a field of one, inside a `while`

The checker walks a loop's condition and body once and flags any binding
declared outside the loop that they move, whole or by a field. The walk does not
follow the back edge or a `break`, and an assignment does not re-initialize a
moved place, so it refuses some loops whose moved value is never used again.

```sentinel
struct S { a: [i64], b: i64 }
fn consume(v: [i64]) -> i64 { v[0] }
fn main() -> i64 {
    let mut s: S = S { a: [1, 2], b: 1 };
    let mut t: i64 = 0;
    let mut i: i64 = 0;
    while i < 2 {
        t = t + consume(s.a);   // REJECTED: cannot move out of `s` inside a `while` loop
        s.a = [3, 4];
        i = i + 1;
    }
    t
}
```

Diagnostic: `sentinel::borrow::moved_in_loop_body`, on the root `s`. The same
diagnostic refuses a body that assigns the place BEFORE moving it on each
iteration, a move followed by `break`, and each of these for a whole binding.
(A move under a guard, or in a loop condition, is refused too, but that is not
an over-rejection in this page's sense: how often it runs is a run-time
property, which a flow-sensitive checker does not know either.)

Cause: ADR 0036 D8 (whole bindings) and A5 (a field of one) flag any binding
declared outside the loop that the condition or body newly moves, and a
reassignment does not un-move a binding.

Workaround: borrow instead of moving, or move a value declared inside the body.

```sentinel
fn peek(v: &[i64]) -> i64 { (*v)[0] }
// ...
    while i < 2 {
        t = t + peek(&s.a);   // accepted
        s.a = [3, 4];
        i = i + 1;
    }
```

Closure: ADR 0036's Revisit trigger for D8 — a move-state fixpoint over the
body, in which an assignment re-initializes the moved place.

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
| Block yielding a ref keeps its borrows | ADR 0018 step .a fact generator (needs provenance) |
| Ref-returning method's receiver moved in the same call | ADR 0018 step .b / .c (Polonius) |
| Borrow of a field of a temporary    | Temporary-lifetime-extension ADR (unspecified for Sentinel) |
| Move of an outer binding (or field) in a `while`, reassigned or followed by `break` | ADR 0036 Revisit (D8): a move-state fixpoint over the body, an assignment re-initializing the place |
| Partial move + drop unsoundness     | ✅ CLOSED (ADR 0046) — `snc` + `scg` both, differentials byte-identical |
| Reference outliving its storage     | ✅ CLOSED (ADR 0017 D7) — `snc`; rejection-only, so no `scg` mirror is needed |
