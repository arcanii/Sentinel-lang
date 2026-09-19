# ADR 0073: A reference may not appear in an effect-op signature

Status: **ACCEPTED** (2026-09-19).
Landed with its fixtures and unit tests: four-check green — 1,972 passed with exactly the
18 known Windows failures, doctests and clippy clean, every `selfhost_*` differential green
and both bootstrap fixed points byte-identical. Corpus reach measured at zero (see below).
Closes register item **D85**. Extends the continuation-seam allow-list ratified by
[ADR 0072](0072-effecting-fn-kont-slot-boundary.md) D3 (`FITS`) from the *let* that binds
a suspension to the *operation declaration* that creates one.
Depends on `Type::carries_ref` from the ref-escape slice
([ADR 0017](0017-phase-c2-kickoff-and-region-plan.md) D7).

## Related

- [ADR 0019](0019-phase-c3-kickoff-and-effects-plan.md) D4 — `effect E { op(..) -> T; }`
  declarations; D5 — `secret &T` is a legal type (a secret value behind a public
  reference), which this ADR does **not** overturn.
- [ADR 0020](0020-handler-runtime-and-perform-lowering.md) D7/D9 — the `Kont*` ABI and the
  lowerable body shapes. ⚠ D7 as written still shows the original `ptr` payloads; the
  SHIPPED seam is **one `i64`** — `SentinelKont.arg: i64` and
  `sentinel_perform_op(i32, i64) -> ptr` in [`abi-v1.md`](../abi-v1.md) §3/§5.
- [ADR 0072](0072-effecting-fn-kont-slot-boundary.md) D3/D4 — `FITS`, the explicit
  fail-closed type allow-list for that seam (`i64`, `secret i64`), which D4 applies to the
  captures as well as the let.
- [ADR 0057](0057-foreign-function-interface.md) — the `extern "C"` fence, the other place Sentinel
  refuses a type positionally rather than globally. Same shape of rule.

## Context

An effect operation's parameters are reified into the continuation when a `perform`
suspends, and its return type is what a `k(v)` resume delivers. Both directions cross the
`Kont*` seam, which is **one `i64`** wide: `SentinelKont.arg: i64` and
`sentinel_perform_op(i32 op_id, i64 arg) -> ptr` (`docs/abi-v1.md` §3/§5). ADR 0072 wrote
that constraint down as `FITS` and applied it where a `let` binds a suspension.

Nothing applied it to the operation *declaration*. A reference in an op parameter is
therefore accepted by every front-end stage and reaches codegen, where inkwell aborts the
compiler:

```sentinel
effect Io { write(r: &i64) -> i64; }
fn emit(r: &i64) -> i64 ! { Io } { perform Io.write(r) }
fn main() -> i64 { let x: i64 = 5; handle { emit(&x) } with { Io.write(r, k) => k(0) } }
```

```
thread 'main' panicked at inkwell-0.5.0/src/values/enums.rs:309:
Found PointerValue(... "%r1 = load ptr, ptr %r, align 8" ...) but expected the IntValue variant
```

Measured 2026-09-19 on the shipped compiler. The reference here is perfectly **live** —
`&x` outlives the `handle` — so this is not a lifetime question that the borrow checker
was ever going to answer. It is the seam being unable to carry a pointer. All four
reference spellings abort identically: `&i64`, `&mut i64`, `?&i64` and `secret &i64`.

A compiler panic on ordinary user source also violates the standing project rule that
user-program input must surface a `miette` diagnostic rather than `panic!`.

### Why the RETURN side is already covered, and why it is still in scope

An op *return* of `&T` does not reach codegen today, but only because two unrelated checks
happen to stop it first:

- `fn fetch() -> &i64 ! { Io } { perform Io.get() }` — the borrow layer refuses it, because
  a `perform`'s value has no identifiable source and so fails closed
  (`sentinel::borrow::returns_local_ref`, naming `<anonymous>`).
- `fn use_it() -> i64 ! { Io } { let r: &i64 = perform Io.get(); *r }` — ADR 0072's `FITS`
  refuses it (`sentinel::codegen::effecting_fn_body_not_direct`: "`&i64` does not
  fit the continuation's …").

Both are clean diagnostics, and both are incidental: they fire at a *use*, several lines
from the declaration that made the use impossible, and they would stop firing if either
check were re-scoped for its own reasons. The property "a reference cannot cross the seam"
deserves to be stated once, at the declaration.

### Corpus reach

**Zero**, measured two ways. No `effect` block in the repository contains a `&` (every
`effect … { … }` block extracted from all tracked `.sentinel` files, then grepped). And
with the rule in place, a sweep of all 362 non-`tests/ui` programs — `tests/pass`,
`examples`, `demos`, `sentinel_library`, `selfhost`, `tools` and `crates/**/fixtures` —
produces **no `ref_in_effect_signature` rejection at all**. The second is the stronger
check: it would have caught an effect block the first one's extraction missed.

## Decisions

### D1. A reference type is refused in an effect-op signature — parameters and return.

Keyed on `Type::carries_ref`, so it reaches through `?T` and `secret T` exactly as the
other second-class-reference rules do. New `TypeError::RefInEffectSignature`, code
`sentinel::types::ref_in_effect_signature`, raised at the offending type expression's span
in the `effect` declaration.

The check runs where the op is typed (`sentinel-types`, the effect-decl pass), so the
diagnostic lands on the declaration rather than on a distant `perform`.

### D2. `secret &T` remains a legal type; this ban is POSITIONAL.

ADR 0019 D5 stands: `secret &T` is a secret value behind a public reference and is legal
wherever a type is legal. D1 refuses it in one position, exactly as ADR 0057's
`is_ffi_safe` refuses types in `extern "C"` signatures without making them illegal
generally. A `secret i64` op parameter stays legal — it is in `FITS`.

### D3. The fence does NOT extend to a `handle` expression's value.

A broader rule — "a `handle` may not evaluate to a reference" — was considered and is
**rejected**. It over-rejects the `n5g_handle_ref_retarm` shape, which the ref-escape slice
deliberately keeps accepted, and it is not what the seam constrains: a `handle`'s value is
produced after the computation completes and does not travel through a `Kont*`.

This is the narrow-fence discipline the register already records for boundary predicates:
enumerate the boundary explicitly rather than widening it by association.

### D4. `snc`-only; rejection-only; no `selfhost/` mirror.

The rule only ever rejects, so no program that compiles today changes, no stage dump moves
and no emitted byte changes. `selfhost/` needs no mirror and the differentials need no
re-bless — the same disposition as the rest of the ref-escape work.

### D5. Pinned by a rejection fixture per position.

`tests/ui/c21_ref_in_effect_op_param.sentinel` and
`tests/ui/c21_ref_in_effect_op_return.sentinel`, plus `sentinel-types` unit tests covering
all four spellings (`&T`, `&mut T`, `?&T`, `secret &T`) and a `secret i64` control that
must stay accepted.

## Reasoning

The alternative to a fence is to make the seam carry a pointer. That is a real design —
it is what a multi-word continuation payload would buy — but it is an ABI change to
`abi-v1`, whose evolution policy freezes the 1.0 surface (ADR 0029 D8), and it would
require the reference's lifetime to outlive a suspension that can be resumed arbitrarily later. Nothing in the
lexical borrow checker can establish that today. Refusing the declaration is the honest
minimum, and it is reversible: if the seam ever widens, D1 relaxes with it.

## Consequences

### Positive

- A compiler `panic!` on ordinary source becomes a diagnostic at the declaration.
- The seam's allow-list is stated once, where the obligation is created.
- No corpus reach, so no migration.

### Negative

- An effect that genuinely wants to hand a buffer to a handler must pass it by value or
  behind a handle type, and cannot express "borrow this for the duration of the handler".
  That capability is not available today either — it panics — so nothing is lost, but the
  ADR records it as the shape a future seam widening would unlock.

### Neutral

- `ResumeKont` (`k(&x)`) was already refused by the borrow layer; D1 does not change it.

## Revisit

When the continuation payload stops being a single `i64` — a multi-word seam, or a
region-parameterised reference (ADR 0017 D7's deferred named regions) that could outlive a
suspension. Either would let D1 relax rather than be removed.
