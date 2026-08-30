# ADR 0072: The effecting-fn continuation boundary is an explicit fail-closed list

Status: **ACCEPTED** (2026-08-31) — supersedes an unratified implementation shortcut.
Landed in `5609fb7`: fixtures in place, four-check green, both bootstrap fixed points
byte-identical. ⚠ The five-lens adversarial review was launched and STOPPED before any
lens returned, at the maintainer's call; this rests on the four-check plus the hand
verification recorded under Consequences.
Governs the `Kont*`-ABI lowering of effecting fns first pinned by [ADR 0020](0020-handler-runtime-and-perform-lowering.md) D7/D9.

## Context

An effecting fn returns a `*mut SentinelKont` instead of its declared type. Four shapes
of body are lowered (ADR 0020 D9): a direct `perform` / call-to-effecting tail (c35b), a
single let bound to a kont producer (c35c), an embedded perform in pure context (c35d),
and chained effecting lets (c35e). A body matching none of them was supposed to be
refused by `validate_effecting_fn_body` so the shape could be deferred.

**It was not refused. It fell through to the generic straight-line emitter**, which knows
nothing about the ABI substitution and lowers a call to an effecting fn as though it
returned its declared type. The measured result, on the shipped compiler:

```sentinel
effect Io { read() -> i64; }
fn do_work() -> i64 ! { Io } { perform Io.read() }
fn caller() -> i64 ! { Io } { let s: secret i64 = do_work(); declassify(s) }
fn main() -> i64 { handle caller() with { Io.read(k) => k(42) } }
```

`snc build` accepted this and produced a binary whose **exit status was a raw pointer**,
varying run to run with ASLR (`-1341102038`, `783502784`, `-453424688`). The `let s: i64`
twin returned 42 every time. No diagnostic either way. `snc llvm` showed the mechanism —
`%v0 = call ptr @do_work()` followed by `store i64 %v0` — which `llvm-as` rejects, but
inkwell's `module.verify()` cannot: under LLVM 18 opaque pointers, storing a `ptr` into
an `i64` alloca is well-formed IR. The `Kont*` was simply reinterpreted.

### Why one added qualifier was enough to break it

Two independent guards had to fail, and both did:

1. `detect_let_shape` required `ty == Type::I64` on the let's ANNOTATED type. A comment
   called this an "MVP restriction". **No ADR ratifies it** — ADR 0020 D7 is written
   type-generally ("the value of the return type"), D9's c35c row states no restriction,
   and the only trail-side mention is a line in `HISTORY.md`, which is a log, not
   authority.
2. `tail_produces_kont` did not see through `WidenToSecret`, so the widened RHS was not
   recognised as a kont producer at all.

### The real defect, and why it was so much wider than one type

The guards were only the trigger. The defect is that **the fall-through was silent**, and
the reason it stayed open is a predicate mismatch that is exactly the anti-pattern this
project has already been bitten by once:

> `validate_effecting_fn_body` decided "can we lower this?" by walking for `perform`
> NODES (`expr_performs`), while the `Kont*` ABI substitution that creates the hazard is
> keyed on the CALLEE'S SIGNATURE (`uses_kont_abi`). Two predicates maintained for
> different purposes, intersected into a safety fence.

So **a literal `perform` failed closed everywhere, and a CALL to an effecting fn failed
open everywhere.** A 116-cell audit measured the asymmetry directly: the same 16 programs
with a `perform` RHS gave 15 clean rejections and 0 unsafe outcomes; with a `do_work()`
RHS they gave 1 correct result and 15 unsafe or crashing ones.

Across the audit the single hole had six faces, and only the first was known:

| shape | outcome before |
|---|---|
| `let s: secret i64 = do_work();` | **silent miscompile** — raw pointer as the value |
| `do_work(); 42` (statement position) | **silent** — the effect is DROPPED; a handler's `print` never runs, exit still 42 |
| a captured `u8`/`i32` param | **silent** — `alloca i8` + `load i64`, a 7-byte OOB stack read; exit `-1341102038` for 42 |
| `do_work() + 1`, `if do_work() == 42` | inkwell **PANIC** (`into_int_value` on a `PointerValue`) — an ICE on a legal program |
| `sink(do_work())`, `let s: ?i64 = do_work()` | LLVM verifier abort at build |
| all of the above | `snc llvm` emitted invalid IR while exiting 0 |

Three of these (`secret` let, perform-in-a-call-arg, aggregate tail) also **already
disagreed with the self-hosted `scg`**, which lowers them correctly — a live parity break
sitting behind a missing fixture, invisible only because no corpus program has the shape.

## Decision

### D1. The lowerable set is an EXPLICIT list; everything else is REFUSED

`validate_effecting_fn_body` refuses any effecting-fn body not matched by one of the four
shapes. There is no fall-through to the generic emitter.

**Scope, stated precisely so the claim is not wider than the fix.** This governs effecting
FUNCTIONS. A class or impl METHOD with an effect row is lowered by a different path
(`dump_method`) that never consults `uses_kont_abi` at all — an effecting method is emitted
with the ordinary value ABI while its body performs, so `Box__fetch` is declared to return
`i64` and then `ret i64 %v0` on a `Kont*`. That is a separate, pre-existing hole with a
separate cause: methods do not have the `Kont*` ABI, rather than having it and slipping past a
gate. Giving them one is a feature decision, not a boundary repair, so it is filed rather than
folded in here. No corpus program has an effecting method (zero hits), which is why nothing
observes it today. Note the consequence for D2: `expr_suspends` deliberately does NOT treat
`MethodCall` / `ImplMethodCall` / `QualifiedCall` as suspending — under today's ABI they
genuinely return a value, and marking them otherwise would refuse programs on a premise the
lowering does not share. The diagnostic stays the existing
`CodegenError::EffectingFnBodyNotDirect` code — no new variant — but it now carries a
`reason`, because the generic text was actively WRONG for two of the refusals:
`let s: ?i64 = do_work();` IS "a let bound to a call to another effecting fn", and a narrow
captured param has nothing to do with the body's shape at all. A boundary is only as useful
as the diagnostic that pins it, so the refusal names the rule broken —
"a `let` bound to a suspension must be `i64` or `secret i64`; `?i64` does not fit the
continuation's single i64 slot", or "`n` is captured across the continuation, so it must be
`i64` or `secret i64`; `u8` would be read out of bounds".

The reason is re-derived from the same predicates rather than threaded out of the detectors,
deliberately: a wrong string here is cosmetic, a wrong ANSWER there is a miscompile, and
keeping them apart means the explainer can never widen what is accepted.

### D2. The gates ask "does this SUSPEND", not "does this perform"

`expr_performs` / `stmt_performs` are replaced by `expr_suspends` / `stmt_suspends`,
which take the `TypedProgram` and are true for a `perform`, a `resume`, **or a call to
any fn using the `Kont*` ABI**. The two differ in exactly one match arm.

That replacement turned out to be total: after threading the program through, **not one
caller still wanted the perform-only meaning**, in either back end. Every gate had always
meant "does this suspend"; none could express it.

The walk stays TOTAL over `TypedExprKind` — no wildcard arm. A wildcard is how a new
variant would silently re-open the class.

### D3. `FITS` — the type allow-list for the continuation seam

A reified frame crosses exactly two seams, and both are one `i64` wide:

- `SentinelKont.arg: i64`; `sentinel_kont_pure(value: i64)`; the resumer's LLVM type is
  hard-wired `ptr fn(i64, ptr)`.
- the captured-state struct is a flat `i64[N]`, written and read with 8-byte loads.

So the allow-list is, positively and with a default deny:

```
FITS(T)  ⟺  T = i64  ∨  T = secret U where U = i64
```

- **`secret i64` is IN** because `secret T` lowers IDENTICALLY to `T` (ADR 0019 D12, ADR
  0045 A23) and the secret widen is value-level identity (ADR 0051 A2) — it already IS
  that `i64`. Verified byte-for-byte: the fixed oracle's output for the `secret i64`
  program is identical to its output for the plain-`i64` twin.
- **`?i64` is OUT** — `{ i1, i64 }`, 16 bytes. `WidenToNullable` constructs a value; it
  is not identity. Storing 8 bytes into it leaves the `valid` flag uninitialised.
- **`bool` / `i32` / `u8` are OUT** — narrower than the slot, so an 8-byte load reads
  past the alloca. Measured: a captured `u8` returns `-1341102038` in place of 42.
- **structs, arrays, `f64`, and the pointer-shaped handle types are OUT** — not the
  seam's integer at all.

Widening this list requires widening the SEAM first. It is not a matter of relaxing a
check.

### D4. `FITS` gates the CAPTURES too, not just the let

The let's own type was the only thing ever checked. The captured vars cross the same
`i64[N]` seam and were never checked at all, which is the OOB read above. A capture may
be bound two ways — a fn param, or an earlier let of the same chain — and **both are
listed explicitly**; deriving the answer from `params` alone silently declines legitimate
chained captures, which is the same anti-pattern one level down.

### D5. A `secret` widen is peeled before asking "does this produce a kont"

`strip_secret_widen` is applied to a let's RHS. This is sound precisely because of D3's
reasoning: the widen is value-level identity, so it cannot change what crosses the
continuation — it only hid the `Call` underneath from the detector.

## Consequences

**Programs that gain support.** `let s: secret i64 = <effecting call or perform>;` in an
effecting fn now lowers correctly, single and chained, and returns the right answer. This
is a capability gain, not just a refusal.

**Programs that lose support.** Everything in the table above that used to "compile":
each was already miscompiling, crashing the compiler, or emitting invalid IR. A refusal
is strictly better than any of those outcomes. No program that produced a CORRECT answer
before produces a diagnostic now.

**Self-host parity.** `scg` needs NO change. It never had the `i64` restriction and
already emits the let-shape for these programs, so the fixed oracle converges on what
`scg` was already producing — verified byte-identical. No `selfhost/*.sentinel` source
declares an effect or contains a `perform`, so **neither bootstrap fixed point can move.**

**Does admitting `secret i64` weaken the secret discipline? No — and it was checked, not
assumed.** The guarantee the README states is precise: the MIR pass rejects a `secret`
reaching a branch, a memory index, or a divisor, with the type system as the taint oracle.
Both directions through the new path were constructed and run:

- the RESUMED value — `let s: secret i64 = do_work(); if s == 0 { … }` — is rejected with
  ``snc: `if` on a `secret bool` condition would leak via timing``;
- a CAPTURED secret — `fn caller(base: secret i64)` whose resumer branches on `base` — is
  rejected the same way.

So the taint survives the continuation in both directions. What DOES change is the storage
class: a `secret i64` that crosses a continuation lives briefly in the heap-allocated kont
struct and capture array rather than a stack alloca. That is not a regression against anything
promised — an ordinary `let s: secret i64 = 5;` already lives in an unscrubbed stack slot; the
`mlock` + zero-on-free policy belongs to the broker's arenas and, per ADR 0071 D6.2, to
`Shared`/`Mutex` cells, both of which are opt-in container types rather than a blanket promise
about every secret value. Extending that policy to continuation frames would be its own ADR;
this one neither makes nor weakens that claim.

**What stays open**, deliberately, each filed separately: properly LOWERING the refused
shapes (a wider seam, or per-eval-site reification); `scg`'s own `cg_tailk` sticky flag;
`scg`'s `?i64` let-shape, which stores 8 bytes into a 16-byte slot and — worse than the
oracle's version — assembles cleanly; and the accept/reject drift between inkwell's ADR
0065 stage-3a normalization tier and the text oracle, which has no such tier.

## Amendments

_(none yet.)_
