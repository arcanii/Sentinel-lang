# ADR 0058: A 64-bit floating-point type (`f64`) — public-only

Status: **PROPOSED — design only.** This ADR records the design for adding an IEEE-754
double-precision float type `f64` to the language. No implementation lands with this ADR;
it is the ADR-first design gate for "math functions" beyond integers — the second of the
two remaining language-gap rocks on the core-libraries roadmap (the other is the FFI of
ADR 0057).

## Context

The language is integer-only: `i64`, `u8`, `i32`, and `u128` (ADR 0055). Integer and bit
math is well covered (`std/math/num`, `std/bits/bits`, and the entire constant-time crypto
suite, which is *all* integer field arithmetic). But anything with fractions — `sqrt`,
trigonometry, `exp`/`log`, statistics, geometry, physics, a JSON number like `3.75` — needs
a **floating-point type**. Without one, "math functions" beyond integers simply cannot be
expressed. This is the last *library-blocking* language gap from the owner's big list
(crypto, strings, maps, JSON are shipped; bindings have a design in ADR 0057).

The closest precedent is **`u128`** (ADR 0055): a new numeric `Type` variant
(`crates/sentinel-types/src/lib.rs` — `Type::U128` joins `is_int`, Copy, the width-agnostic
binop/cast codegen) demonstrated by a real consumer, with the `scg` mirror deferred until a
corpus fixture needed it. A float is the same *shape* — a new `Type` variant + LLVM type —
but differs in three load-bearing ways:

1. Float **arithmetic is a different LLVM op family**: `fadd`/`fsub`/`fmul`/`fdiv`/`fneg`
   and `fcmp`, not the integer `add`/`mul`/`sdiv`/`icmp`. The width-agnostic integer binop
   codegen does **not** extend for free; floats need their own lowering.
2. Floats need **float literals** — a new lexical form (`3.14`, `1e9`) distinct from the
   existing integer literals.
3. **Float operations are NOT constant-time.** This is the decisive design issue and the
   reason `f64` differs from `u128` in the secret model (below).

## Decision

Add **`f64`** — IEEE-754 binary64 (`double`) — as a **PUBLIC-ONLY** primitive.

- **Type:** `Type::F64` → LLVM `double`. Copy, like the other scalars; joins a new
  `is_float` predicate (NOT `is_int`).
- **Literals:** a decimal point or an exponent after digits makes a *float* literal —
  `3.14`, `1.0`, `0.5`, `1e9`, `2.5e-3`, `6.022e23`. A bare `42` stays an **integer**
  literal (so existing code is unchanged); `42.0` is `f64`. A leading-digit requirement
  (`0.5`, not `.5`) keeps the lexer unambiguous.
- **Arithmetic:** `+ - * /` on two `f64` lower to `fadd`/`fsub`/`fmul`/`fdiv`; unary `-`
  to `fneg`; the comparisons `== != < <= > >=` to `fcmp` (ordered predicates — IEEE 754,
  so `NaN != NaN`). No `%` (consistent with the language — `%` is unsupported for ints
  too; `frem` is not surfaced). Mixed int/float arithmetic is a type error — the operands
  must be cast explicitly (no implicit promotion), matching the language's existing
  no-implicit-widening stance.
- **Casts** (extending the ADR 0049 `as` machinery): `i64 as f64` (`sitofp`),
  `f64 as i64` (`fptosi`, truncation toward zero), and the `u128`/`i32`/`u8` ↔ `f64`
  pairs (`uitofp`/`fptoui`/`sitofp`/`fptosi`). These are the only way to move between the
  integer and float domains — there is no implicit conversion.
- **`sqrt`** ships with the type via the LLVM `llvm.sqrt.f64` intrinsic (a hardware
  instruction). Other transcendentals (`sin`/`cos`/`exp`/`log`/`pow`) are FOLLOW-UP
  library work (a `std/math/float` module), implemented either as Sentinel polynomial
  approximations or via libm through the ADR 0057 FFI / a couple of runtime builtins.
  Float ⇄ string formatting/parsing (`f64`→text and text→`f64`) are likewise follow-up
  library work — correct float formatting is a real algorithm (Grisú/Ryū), so a first cut
  is a simple fixed-precision formatter, not part of this ADR.

### The decisive rule: there is NO `secret f64`

`secret f64` is a **type error**. Unlike `u128` — where `secret u128` is valid because
integer ops *can* be made constant-time (and the crypto suite relies on exactly that) —
**floating-point operations are not constant-time on real hardware**:

- subnormal/denormal operands make `fadd`/`fmul` take a microcode-slow path on most CPUs
  (a classic, well-documented timing side channel);
- `fdiv` and `sqrt` have data-dependent latency;
- `NaN`/`inf` handling branches in microcode.

So a `secret f64` would silently undermine the machine-checked constant-time guarantee —
it would be a *false* guarantee. The principled boundary is therefore: **the `secret` /
constant-time type system is for constant-time integer crypto; floats are excluded from
it by design.** Floats live entirely in the PUBLIC domain (graphics, statistics, physics,
general numerics); crypto stays integer-only, which it already is — *every* shipped
primitive uses integer field arithmetic. The compiler rejects `secret f64` at the type
level, so no secret float value ever exists, and the existing CT proof (a secret reaching
a branch / index / divisor / shift) is unweakened — there is simply nothing new for it to
police, because the float domain and the secret domain do not overlap.

## Alternatives considered

1. **`f32` first / instead.** `f64` is the universal default for "math functions" (C
   `double`, JSON numbers, Python `float`, most languages); `f32` is a width variant for
   graphics/ML, naturally added later alongside the other widths — not the place to start.
2. **Fixed-point or rational instead of IEEE float.** Deterministic and potentially even
   constant-time, but it does not match what "math functions" means — `sqrt`/trig need
   reals, there is no hardware support, and the ergonomics are poor. Users asking for math
   want hardware doubles.
3. **A `secret f64` with a "constant-time float" discipline.** Not viable: hardware float
   is not constant-time (subnormals, division, NaN), so this would advertise a guarantee
   the hardware cannot keep. Excluding secret floats is the only *sound* choice and is the
   core of this ADR.
4. **Software big-decimal / arbitrary precision.** Overkill for the goal; the language
   wants native hardware doubles for real numerics. A decimal library could later be built
   *on top* if exactness is needed.

## Constant-time interaction (summary)

Floats are public-only (above). The net effect on the language's headline property is
**none**: the constant-time guarantee covers the secret integer domain exactly as before,
and floats are a disjoint public domain. The one thing the design must enforce is the
`secret f64` rejection, so the disjointness holds; everything else (casts move *public*
ints to/from floats; float ops never touch a secret) follows.

## Self-hosting (`scg` mirror)

A front-end + codegen change, like `u128`: a new `Type::F64` variant, a lexer float
literal, the `fadd`/`fcmp`/`sitofp` codegen family, and the `secret f64` rejection.
Following the `u128` precedent (ADR 0055), Phase 1 can be **`snc`-side only** — the `scg`
mirror (`selfhost/*.sentinel`) is deferred until a `tests/pass/cNN` fixture uses `f64`,
because no existing corpus or `selfhost/*.sentinel` source uses floats, so both bootstrap
fixed points stay byte-identical. `Type::F64` is a `Type` variant (not a builtin), so it
causes **no `FnId` shift**.

## Touch-points (Phase 1, `snc`)

- **lexer** — recognise a float literal (`digit+ '.' digit+` and/or `digit+ ('e'|'E')
  ['+'|'-'] digit+`) → a new `FloatLit(f64)` token; an `f64` type keyword.
- **parser** — the `f64` type annotation; the `FloatLit` expression.
- **types** — `Type::F64` + `is_float`; the arithmetic/comparison rules (float ops require
  both operands `f64`, no int/float mixing); the `as` cast rules for every int↔float pair;
  **reject `secret f64`** (the fence).
- **codegen** — the `double` LLVM type; `fadd`/`fsub`/`fmul`/`fdiv`/`fneg`/`fcmp ord`;
  `sitofp`/`fptosi`/`uitofp`/`fptoui`; the `llvm.sqrt.f64` intrinsic; a float-literal
  constant.
- **CT / secret check** — nothing new to police (no secret floats exist); just the
  type-level rejection above.
- **scg mirror** — deferred (snc-only first, like `u128`).

## Scope (what a first increment would land)

`f64` + float literals + `+ - * /` + comparisons + `-` + the int↔float `as` casts +
`sqrt`, `snc`-side, with a demonstrator example (e.g. the quadratic formula, a 2-D vector
length, or a Newton's-method root) and tests, plus the `secret f64` rejection. Deferred to
later increments: the transcendental `std/math/float` library (Sentinel approximations or
libm via ADR 0057), float ⇄ string formatting/parsing, `f32`, and the `scg` mirror.
Downstream beneficiaries once the type lands: `std/data/json` can parse non-integer
numbers as `f64` instead of dropping the fraction, and a real `std/math` grows past
integer min/max/clamp.
