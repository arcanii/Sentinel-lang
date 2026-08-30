# ADR 0058: A 64-bit floating-point type (`f64`) — public-only

Status: **ACCEPTED-WITH-AMENDMENTS (A1–A8).** Implemented `snc`-side in four staged
commits (lexer → `Type::F64` + arithmetic/casts → float literals → `sqrt`) plus a
demonstrator. The design below stands; the amendments at the end record where the
implementation refined it. This was the ADR-first design gate for "math functions" beyond
integers — the second of the two remaining language-gap rocks on the core-libraries
roadmap (the other is the FFI of ADR 0057).

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

## Amendments (at implementation)

- **A1 — `sqrt` is a `UnaryOp::Sqrt` intrinsic, NOT a numbered builtin fn.** Adding a
  builtin (like `read_file` / the TCP ops) would consume an `FnId` and shift EVERY user
  `FnId`, re-blessing every golden resolve/MIR/types dump AND breaking the `scg` selfhost
  differentials (which print `FnId`s by number) — the exact churn ADR 0056's socket
  builtins paid. Instead `sqrt(x)` is recognised by the parser as the reserved name `sqrt`
  with exactly one argument and lowered to a new `UnaryOp::Sqrt` (the existing `Unary`
  node, already threaded through every stage). No `FnId`, so no shift — consistent with
  "`Type::F64` is a variant → no FnId shift" extended to `sqrt`. Wrong arity is a new
  `ParseError::SqrtArity`; the operand must be `f64` (else `Mismatch`). Codegen calls
  `llvm.sqrt.f64` (declared once at setup, stored as `sqrt_f64_fn`).
- **A2 — float literals store IEEE-754 BITS (`u64`), not an `f64`.** `ExprKind` /
  `ResolvedExprKind` / `TypedExprKind` derive `Eq` + `Hash` (Salsa needs them), which
  `f64` does not implement. So `FloatLit(u64)` carries `f64::to_bits`; the parser decodes
  the literal text once, codegen reconstructs via `f64::from_bits`. (The lexer regex
  guarantees a valid literal, so `f64::parse` cannot fail — an out-of-range magnitude
  saturates to ±inf, well-defined, so there is no overflow error, unlike `IntLit`.)
- **A3 — BOTH the `scg` mirror AND a `tests/pass/cNN` fixture are deferred; the
  demonstrator is an `examples/` program (snc-only), exactly as `u128` (ADR 0055) did.**
  The "Self-hosting" section above contemplated deferring the mirror "until a
  `tests/pass/cNN` fixture uses `f64`". In practice the two are coupled: a clean-typing
  `f64` `tests/pass` fixture is auto-scanned by the FRONT-END selfhost differentials
  (`selfhost_types` / `_mir` / …), which run `scg` whenever the Rust oracle (`snc types`)
  *accepts* the fixture — and `snc types` accepts valid `f64`. `scg` does not know `f64`,
  so it would diverge → RED. (Only the *codegen* corpus differential skips it, because the
  `snc llvm` textual oracle Errs on `f64` via its `llvm_ty` catch-all.) So a `tests/pass`
  `f64` fixture is impossible WITHOUT the full `scg` front-end mirror. The demonstrator is
  therefore `examples/math/quadratic.sentinel` (built by the examples harness both
  `--separate` and merged, a free back-end differential), and there is no `f64`
  `tests/pass` fixture this increment. Both bootstrap fixed points + every selfhost
  differential stay byte-identical (no corpus / `selfhost/*.sentinel` source uses `f64`).
- **A4 — two new type errors + one parse error.** `SecretFloat` (the fence) fires at a
  `secret f64` annotation AND at an `x as f64` cast of a `secret` value — no secret float
  can ever exist. `FloatBitwise` rejects `& | ^ << >>` on `f64`. `SqrtArity` (parse)
  rejects `sqrt()` / `sqrt(a, b)`.
- **A5 — `fcmp` predicate mapping is C-style.** `== → oeq`, `< → olt`, `<= → ole`,
  `> → ogt`, `>= → oge` (ordered: any `NaN` operand compares false), but `!= → une`
  (unordered-or-not-equal) so `NaN != NaN` is *true* — matching the ADR's "IEEE 754, so
  `NaN != NaN`" intent and C / Rust semantics (the design said "ordered predicates"
  loosely; `!=` is the one that must be unordered).
- **A6 — `[f64]` / `Vec<f64>` / `?f64` are out of scope this increment** (the
  `ArrayElem` / `VecElem` / `NullableInner` demotes reject `f64`), exactly as `u128`
  deferred `[u128]`. Scalar `f64` only; the demonstrator needs nothing more.
- **A7 — a new `std::math::float` library** (`abs` / `min` / `max` / `sq` / `hypot` /
  `lerp` / `trunc` / `discriminant`) is the demonstrator's public, branch-on-input (so
  public-only) numeric module — the float counterpart to `std::math::num`. The example
  `examples/math/quadratic.sentinel` solves the quadratic formula and a 3-4-5 triangle
  across the module boundary.

- **A8 — A3's deferral is DISCHARGED: the `scg` FRONT-END mirror and the
  `tests/pass` fixture landed together, exactly as A3 said they must.** The mirror is
  `339f437` (the lexer half) + `3eeb34a` (parser / resolve / types / mir), and the
  fixture is **`tests/pass/c58_float_math.sentinel`**, the first `f64` fixture in the
  corpus. What forced the issue was not this ADR but the extended real-program stage
  differentials (`34ffea0`): the eight `examples/` + `sentinel_library/` programs that
  use floats had never been compared, because A3 deferred the fixture along with the
  mirror and the differentials swept only `tests/pass` + `tests/ui`. The first sweep
  showed `2.0` lexing as `IntLit Dot IntLit` and PARSING as a field access —
  `(field (int 2) 0)` — a silent misparse, not a rejection.
  **Scope: FRONT END only, and that is sufficient by construction.** `snc llvm` Errs on
  every float LITERAL it lowers, so the codegen differential skips every float program
  and no codegen mirror is needed — which is also why the fixture is safe. State that
  guard as the LITERAL check and no other: the oracle does NOT refuse `F64` as such
  (`Channel<f64>` and a phantom `f64` generic argument both lower cleanly), and getting
  this wrong cost a real defect during implementation — `cg_mangle_to` had no scalar-4
  arm, so `struct Pair<A, B> { a: A }` used as `Pair<i64, f64>` made `scg` ABORT on a
  program the oracle compiles to `%Pair_i64_ty_F64`. It now mirrors the oracle's own
  defensive `ty_F64`.
  **Two decisions worth carrying forward.** (i) `f64` is a scg SCALAR CODE (4), never a
  new interner KIND: every `h < tbase()` test in the self-hosted typer *means* "scalar"
  (Copy, drop-free, `Fn<T,R>`-eligible, substitution-inert), so a kind would have made
  every f64 value MOVE and polluted the borrow dump. (ii) A1's reserved-name rewrite is
  what makes `sqrt` free of an `FnId` — builtins occupy 0..=41 and user fns are
  `42 + idx`, so a 42nd builtin would shift every user FnId and re-bless every dump.
  **The formatter is the interesting artefact.** The dumps print a float by VALUE
  (Rust's `{:?}`), so the mirror must reproduce shortest-round-trip formatting *from
  decimal text with no `f64` anywhere* — selfhost sources may not contain a float, or
  the oracle would Err on them. It is exact for at most 15 significant digits over a
  normal double (DBL_DIG = 15 means no shorter decimal shares the double), and NOT
  exact beyond that: >15 digits, overflow (`1.8e308` → `inf`) and subnormals all differ,
  each verified to fail as a LOUD unregistered differential mismatch rather than as
  wrong code.
  **Side effect:** the `Channel<f64>` mis-render that HANDOVER carried as a
  known-and-tolerated `scg` over-accept is CLOSED, and `?f64` / `Fn<f64,f64>` now render
  correctly too. **A6 is partly stale:** it says `?f64` is out of scope, but ADR 0066
  M1.2b later added `NullableInner::F64`, so `?f64` is accepted — `[f64]` and `Vec<f64>`
  remain rejected. **Still deferred:** f64 CODEGEN in `scg` (and with it the `snc llvm`
  text backend's own F64 support), `f32`, and float ⇄ string.
