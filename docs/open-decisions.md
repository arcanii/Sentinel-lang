# Open decisions awaiting the maintainer

Filed-defect register items that cannot be actioned by an agent without a call from the
maintainer. Each states its options, costs and risks so the decision can be made from one
page. It is **not** an ADR — nothing here is ratified. When a row is chosen the work
becomes an ordinary slice; the section is then marked DECIDED and kept only while the
remaining rows are still live, and deleted once none are.

**Status:** D36 is open. D47's option A was chosen and has landed; its B and C rows are
still live.

Register entries: [`HANDOVER.md`](HANDOVER.md) menu item 4 (D36, D47).

---

## D36 — a generic fn that RETURNS its container param over-releases in `scg`

`fn ident<T>(x: T) -> T { x }` at `Shared<i64>`: the oracle omits the scope-exit release
in the monomorphised body (correct — ownership moves out with the return value), `scg`
emits it.

### Why it needs a decision rather than a fix

**Polarity is the whole question.** A refcount error has two directions and they are not
symmetric:

| direction | consequence | severity |
|---|---|---|
| one release too many | count reaches zero while a live handle exists | **use-after-free** |
| one release too few | handle never freed | **leak** — and for a `secret` container, the mlock + zero-on-free policy only scrubs on the last drop, so a leaked secret cell is never zeroed and stays resident in locked pages |

D36 is the first direction. Its sibling D31 was the second, on the same machinery.

**Scope it before calling it a security matter.** This is `scg`-only: inkwell and the
oracle are both correct, `snc build` ships inkwell, and `selfhost/*.sentinel` declares no
container-typed binding at all (verified 2026-09-05: no `Shared`/`Mutex`/`Guard`
annotation anywhere; the only textual hits are string literals in `scg`'s own
builtin-name table). So nothing that is built and run links the path. It is a latent
use-after-free in code a back end would *emit* for a user program, in the back end that
is not the shipping one. That makes it a bootstrap-parity matter, not a shipped-binary
one — real, but not urgent, and not private.

**The question splits in two, and the halves have different answers.** This is the part
that makes a single go/no-go the wrong shape:

| gate | locus | oracle | `scg` | is "match the oracle" safe? |
|---|---|---|---|---|
| **field / partial-move** | `record_field_move` call in `dump_te_field`, [`selfhost/types/borrow_arms.sentinel:1179`](../selfhost/types/borrow_arms.sentinel) | net **0** in both the return shape and the forward shape | net **−1** in both | **Yes.** The reference state is already balanced, so aligning removes exactly the surplus release and cannot manufacture a leak. Monotone in the safe direction. |
| **whole-binding** | `record_move`, [`selfhost/types/borrow.sentinel:64`](../selfhost/types/borrow.sentinel) | clone fires only for a named-`Var` argument | — | **No.** Register D35 records that the oracle is the *wrong side* on some of those shapes, so parity here could install the oracle's own leak. |

"Plain parity" is a claim about direction, and it only holds where the reference is known
correct. It holds at the field gate and does not hold at the whole-binding gate.

### Costing corrections (any plan must absorb these)

- **The `let`-local step is not writable as first scoped.** `let y: T = x;` is rejected
  ("unknown type `T`"), so the only reachable form is the unannotated `let y = x`, where
  no declared-type expression exists. Recovering abstractness there means propagating a
  bit through the RHS *expression* type — a taint propagation across the dump walk, not a
  per-declaration flag.
- **The `return` route emits an independent extra release.** A fix validated only on the
  tail-return shape leaves it red, and the byte differential will then fail on far more
  than the lines the fix added. `break` does not route here; the distinction has to be in
  the plan.
- **`is_named_shared_return`** ([`crates/sentinel-types/src/lib.rs:1498`](../crates/sentinel-types/src/lib.rs))
  has **two** holes in the same `_ => false` arm — the generic form, and an `if`-tail
  which needs no generics at all. Closing one and reporting the shape as covered is the
  available mistake.
- ⚠ Anything that *widens* the `SharedReturnNotSupported` guard is gated on a separate
  question tracked privately. Ask before extending it.

### Options

| # | Option | Cost | Risk | Verdict |
|---|---|---|---|---|
| **A** | **Fix the field gate only, as parity** — plus the `return` route, which shares its blast radius | Small: two gates' worth of emission, one `tests/pass` fixture, one sweep | Low. The target state is balanced, so the change can only remove a surplus release | **RECOMMENDED** |
| B | Fix both gates by matching the oracle | Medium | **Installs the oracle's own leak** on named-`Var` shapes (D35). Wrong direction on a secret container | Not without deciding the oracle first |
| C | Oracle-first: decide the correct clone rule, fix the oracle, mirror into `scg` | Large — this is the D35 family, and partly gated privately | Low once decided, but it is a design slice, not a fix | Right eventually; not now |
| D | Defer and document | Zero | Leaves a latent UAF in `scg`-emitted code. Unreachable by the bootstrap, so it can sit | Acceptable if A is not wanted |

**Recommendation: A**, with the whole-binding gate explicitly left open pending C.
Require negative fixtures that pin the gate's *other* conjuncts as well as the positive
case — D31's lesson was that a wrong implementation which drops a conjunct passes every
positive case and the whole corpus.

---

## D47 — a heap-indirect nullable payload cannot be read back, in any emitter

> **▶ DECIDED 2026-09-05: option A was chosen and has LANDED.** `unwrap_or` on a
> `?Struct` / `?GenericInstance` is now refused at the type layer
> (`TypeError::UnwrapOrHeapPayloadNotSupported`), so the compiler states the limit
> instead of crashing on it — the inkwell PANIC is gone. **Options B and C remain
> open**, and the section below is kept because it is the case for them: A bought
> honesty, not the feature. One route is still un-refused, filed as register D53 (a
> generic body sees an abstract `?T`, so the gate cannot decide, and instantiating that
> generic at a struct still reaches the broken lowering).

`unwrap_or` on a `?Struct` or `?GenericInstance` feeds the raw `ptr` into a `select`
typed at the payload struct, with no load through the heap box.

### State, measured

| emitter | behaviour |
|---|---|
| `snc llvm` (text oracle) | exits **0** and emits `select i1 %v5, %Struct.0 %v6, %Struct.0 %v4` with `%v6 : ptr` — IR `llvm-as` rejects. Because the oracle exits 0 the differential *runs*, and post-D39 `scg` is byte-identical to it, so the comparison is **green over IR neither back end can assemble**. The `llvm_rejects` gate is silent by design: it fires only when `scg` is rejected and the oracle is clean |
| inkwell (`snc build`) | **two** stop modes. Bind the result (`let g: P = unwrap_or(o, d);`) → `verify_failed` diagnostic. Use it directly (`unwrap_or(o, d).v`) → **panics** at `inkwell-0.5.0/src/values/enums.rs:333` before verify runs |
| `scg` | reproduces the oracle's invalid IR byte-for-byte |

Two things follow. The panic breaks the project's own no-`panic!`-on-user-program-input
rule, on a program the type checker accepts — that half is a defect independent of the
missing load. And this is not a `?GI` matter: **`?Struct` has been constructible since
C1.6 and its payload has never been readable**, so the whole read-back path is missing,
not a corner of it.

### Why it is not a lowering patch

`unwrap_or(o, d)` returns `T` **by value**.

- Payload owns nothing (`struct P { v: i64 }`) — loading out of the box is trivially
  safe. The copy is a value; the box is freed by the existing scope-exit drop.
- Payload owns heap (`struct Q { xs: [i64] }`) — the copy and the box hold the **same**
  array pointer. Freeing both is a double free.

So the general case is an ownership decision. Related: **D50** — the box's drop is
shallow (2 allocations against 1 free for an owning payload, where the same struct bound
*without* the box is 1 and 1). D50 and D47 interact: closing D47 for owning payloads
without D50 changes which side owns the heap.

### Options

| # | Option | Cost | Risk | Verdict |
|---|---|---|---|---|
| ~~A~~ | ~~Reject, fail-closed~~ | Landed: one slice, no ADR, **three** `tests/ui` fixtures (two axes — payload kind, and bound vs unbound position) | None realised. No corpus program affected; `?&T` / `?Channel<T>` verified still accepted | **DONE 2026-09-05** |
| **B** | **Safe subset behind an explicit allow-list** — load through the box for payloads with no owning fields; reject the rest | Medium; needs an ADR amendment for the ownership rule | Low if the list is explicit and pinned by a rejection fixture. This is the ADR 0072 shape | **RECOMMENDED NEXT** |
| C | Full ownership semantics — move-out-of-box, or return a borrow | Large. Interacts with D50 and with the deferred heap-box widen | Real design risk: it decides what `unwrap_or` on an owning payload *means* | The end state; not a starting point |
| D | Leave it | Zero | A panic on legal input persists, and the differential stays green over invalid IR | Not recommended — A is nearly free |

**A is done. B is the live recommendation**, as its own designed slice. C should not
begin before B has settled the ownership rule, because C is B plus the hard half.
Note that A's cost estimate was low by two fixtures: the review found the panicking
shape (the whole point of A) pinned by nothing, because both original fixtures used the
bound form. Position turned out to be an independent axis from payload kind.

**Note on boundaries:** if B is chosen, the allow-list must be an explicit fail-closed
enumeration pinned by a rejection fixture — not a derived predicate such as "the payload
has no heap fields". This repo has already had a boundary written as a derived
intersection silently widen from another file.
