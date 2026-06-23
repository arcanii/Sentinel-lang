# ADR 0048: Shift operators `<<` / `>>` (with a constant-time secret-amount rule)

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A5) — shifts are in `snc` (Phase 1) AND mirrored
into the self-hosted `scg` (Phase 2); the corpus differential validates `scg == snc llvm`
byte-for-byte on a shift fixture, both bootstrap fixed points hold, and the full nextest is
green. Amendments below record the deviations from the PROPOSED plan.

Adds bit-shift operators `<<` (shift left) and `>>` (shift right) to the language. This is
the next language gap on the examples-as-tests + core-library track
(`sentinel_examples_and_corelibs`): a `std/bits` library (rotate / shift) and a
recognizable branch-free primitive — a **ChaCha-style ARX quarter-round** — both need bit
shifts, which Sentinel lacks (`BinOp` is `Add Sub Mul Div BitAnd BitOr BitXor` only).

## Decision

### Semantics

- `x << n` — shift `x` left by `n` bits (LLVM `shl`).
- `x >> n` — **logical** (zero-fill) shift right (LLVM `lshr`) for **all** integer types,
  including signed `i64`/`i32`. This is deliberate: a rotate composes as
  `rotl(x, n) = (x << n) | (x >> (W - n))`, which is only correct with a zero-filling
  right shift (an arithmetic `ashr` would smear the sign bit into the rotated-in bits).
  Sentinel has no unsigned 32/64-bit type, so a ChaCha quarter-round on `i32` words relies
  on logical `>>`. (An arithmetic-shift-right operator is a possible future addition; it is
  not needed by the crypto use cases driving this.)
- Operands are integers (`i64` / `i32` / `u8`). The result type is the **left** (value)
  operand's type. The amount may be any integer width; codegen coerces it (zext/trunc) to
  the value's width (LLVM requires both shift operands to share a type), and a shift amount
  is a non-negative count so the widening is zero-extend.

### Precedence

Shifts slot **between additive and the bitwise ops** — tighter than `& ^ |`, looser than
`+ -` (and, since the bitwise ops already sit below comparison in Sentinel's ladder,
shifts are tighter than comparison too). The ladder becomes:

```
|| · && · comparison · | · ^ · & · << >> · + - · * / · unary
```

So `(x << 16) | (x >> 16)` (the rotate shape) parses without parentheses, and
`a + b << c` is `(a + b) << c` (conventional C/Rust ordering). Left-associative.

### Lexing — no new tokens (the `>>` / generics rule)

`<<` and `>>` are **not** lexed as tokens. `>` lexes as `Gt`, and nested generics close
one `Gt` at a time — `Vec<Box<i64>>` ends in two adjacent `Gt` (the self-hosted parser
documents this invariant). A longest-match `>>` token would mis-lex that and break every
nested generic. Instead the **parser** reconstructs a shift from two **span-adjacent**
single tokens (`Lt`+`Lt` → `<<`, `Gt`+`Gt` → `>>`, requiring `tok1.span.end ==
tok2.span.start`), and only in expression position (the type-argument parser keeps
consuming single `Gt`). This is unambiguous: Sentinel has no turbofish and explicit
generic args don't appear in expression position, so two adjacent `<`/`>` in an expression
can only be a shift. `<<` has no generics ambiguity either way, but both operators are
handled uniformly by the two-token rule.

### Constant-time: the secret-amount rule

The novel part. A shift's **value** (left) may be `secret`: a shift by a *public/constant*
amount has data-independent latency, so `secret i32 << 16` is constant-time and must be
**accepted**, typing to `secret i32`. But a shift by a **secret** amount is a variable-time
operation — a timing side channel — exactly like a secret divisor in `a / secret_b`, and
must be **rejected**.

This forces a deliberate exception to the ordinary binary-op secrecy rule. Today
`secret ^ public` is rejected because the `Binary` arm compares full types (`l.ty != r.ty`,
the `Type::Secret` wrapper included) — there is no separate equal-secrecy predicate. For
shifts we **bypass** that identity check:

- require each operand's stripped inner type to be an integer (the two widths need not
  match — the amount is coerced in codegen);
- the result's secrecy is the **left** operand's secrecy alone (`secret i32 << anything` →
  `secret i32`; `i32 << anything` → `i32`); the amount's secrecy is ignored for typing;
- a **secret amount** is rejected — a new `TypeError::SecretShiftAmount` at type-check
  (mirroring the `SecretDivisor` sink) **and** a new `SinkKind::ShiftAmount` in the MIR
  `verify_constant_time` pass (`Binary(Shl|Shr, _, amount) if is_secret(amount)`), so the
  guarantee holds the same way the secret-divisor rejection does — the type system is the
  taint oracle, the MIR pass is the backstop. The `_` on the value operand means a secret
  *value* is never flagged.

So: `secret i32 << 16` ✅ (→ `secret i32`, constant-time); `secret i32 << secret_n` ❌
(`SecretShiftAmount` / `ShiftAmount` leak); `i32 << secret_n` ❌ (same — a secret amount is
a leak regardless of value secrecy).

## Implementation (Phase 1 — `snc`, the inkwell backend)

A new `BinOp::Shl` / `Shr` (in `sentinel-ast`) drives compiler-checked exhaustiveness —
every `match` on `BinOp` (parser, types, codegen, MIR) is forced to gain an arm. Sites
(all "small", confirmed by reading each stage):

- **AST** (`sentinel-ast/src/lib.rs`): `BinOp::Shl`/`Shr` + `symbol()` arms.
- **Parser** (`sentinel-syntax/src/parser.rs`): a `peek2` helper; a new `parse_shift` level
  between `parse_bitand` and `parse_add` (two-adjacent-token detection); rewire
  `parse_bitand`'s operands to `parse_shift`. **No lexer change.**
- **Types** (`sentinel-types/src/lib.rs`): special-case `Shl`/`Shr` in the `Binary` arm
  *before* the `l.ty != r.ty` check (the secrecy exception above); `SecretShiftAmount`
  error + its diagnostic rendering.
- **Codegen** (`sentinel-codegen/src/lib.rs`): `Shl → build_left_shift`, `Shr →
  build_right_shift(.., false /* logical */, ..)`, with the amount coerced to the value's
  width.
- **MIR** (`sentinel-mir/src/lib.rs`): `SinkKind::ShiftAmount` + Display + the
  `verify_constant_time` arm. Lowering is op-agnostic (shifts ride `MirOp::Binary`) — no
  change.

Validation: a `std/bits` library + an ARX quarter-round example, exercised by the examples
harness (`snc build`, both `--separate` and merge). The full nextest is the gate.

## Scope — Phase 2: the selfhost mirror (DONE)

The self-hosted `scg` and the `snc llvm` textual oracle (`llvm_dump.rs`) each have their own
parser / Binary-typing / CT-sink copies. Phase 1 left them unchanged (the selfhost
differentials stayed byte-identical because no shift program was in the corpus). Phase 2
mirrors shifts into `scg` + the oracle and adds `tests/pass/c53_shift` so the corpus
differential validates `scg == snc llvm` byte-for-byte — making shifts a fully self-hosted
construct.

## Consequences

`std/bits` (rotate/shift) becomes expressible, and the track built a SipHash-style ARX
round — the recognizable branch-free primitive — over `secret` words, rotating by public
constants, all passing the constant-time check.

## Amendments (as implemented)

- **A1 — lexing, as proposed (no tokens).** Shifts are reconstructed from two span-adjacent
  `<`/`>` tokens. In `snc` via a `peek2` helper + a `parse_shift` level between bitwise-and
  and additive; in `scg` (Phase 2) the same, reading the parser's parallel token-span vecs
  (`(*e)[cur] == (*s)[cur+1]`). The `>` stays `Gt`, so nested generics still close one `>`
  at a time. `cur+1` is read only when the current token is `<`/`>` (always followed by ≥
  the EOF token), preserving panic-freedom.
- **A2 — the constant-time secret-amount rule, as proposed.** A new `TypeError::SecretShiftAmount`
  (rejecting a secret amount at type-check) + a new MIR `SinkKind::ShiftAmount` backstop;
  the result takes the LEFT operand's secrecy (a secret value shifted by a public amount is
  accepted and stays secret). Mirrored in `scg` (the typer `resty` widened to include
  shifts; the `mir_verify` binop arm flags a secret amount as leak-kind 4 "ShiftAmount").
  The secret-amount path is type-rejected before MIR, so the MIR sink is a backstop
  exercised by a Rust unit test (and mirror-correct in `scg`).
- **A3 — codegen, as proposed.** `shl` / `lshr` (logical, all int types). `snc`'s inkwell
  backend coerces a mismatched-width amount (zext/trunc); the corpus fixture uses
  matching-width `i64` shifts so the oracle and `scg` emit a bare `shl i64`/`lshr i64` and
  stay byte-identical (the oracle has no coercion path — a deferred refinement, harmless
  since no mismatched-width shift is in the corpus).
- **A4 — the Phase 2 mirror found a real bug.** `scg`'s Binary arm had a `bop >= 14`
  short-circuit fork (for `&&`/`||`) that also caught shifts (op-codes 16/17 ≥ 14), lowering
  `x << n` as a *branch* on `x`. Narrowing both forks (the MIR fork and the codegen fork) to
  `bop == 14 || bop == 15` fixed it — the MIR then matched the oracle byte-for-byte. The
  symbol tables in four selfhost files (`parser` / `resolve` / `merge` / `types`, each
  feeding a stage differential) all needed the `<<`/`>>` rows.
- **A5 — ChaCha → SipHash.** ChaCha is 32-bit, but i32 values are unconstructible today (an
  int literal is i64, no `i64_to_i32`), so the headline ARX primitive is a 64-bit SipHash
  round instead. The i32-construction gap is documented as the next gap.
