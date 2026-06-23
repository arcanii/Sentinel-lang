# ADR 0048: Shift operators `<<` / `>>` (with a constant-time secret-amount rule)

Status: **PROPOSED**

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

## Scope — Phase 2 (deferred): the selfhost mirror

The self-hosted `scg` and the `snc llvm` textual oracle (`llvm_dump.rs`) each have their own
parser / Binary-typing / CT-sink copies. They are **not** changed in Phase 1, and the
selfhost differentials stay byte-identical **because no shift program enters the selfhost
corpus** (`tests/pass` + `tests/ui`) — the `bits` lib + ARX example live in `std/` +
`examples/`, compiled by `snc` only. Both bootstrap fixed points hold (the selfhost sources
use no shifts). Phase 2 mirrors shifts into `scg` + the oracle and adds a `tests/pass`
shift fixture so the corpus differential validates `scg == snc llvm` byte-for-byte — at
which point shifts are a fully self-hosted construct.

## Consequences

`std/bits` (rotate/shift) becomes expressible, and the track can build the ChaCha-style ARX
quarter-round — the recognizable branch-free primitive — over `secret` words, rotating by
public constants, all passing the constant-time check. Amendments (A1, …) will record any
deviation once implemented.
