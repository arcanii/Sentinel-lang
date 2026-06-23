# ADR 0049: Integer cast expression `x as T`

Status: **PROPOSED**

Adds an integer cast expression `x as T` (T ∈ {`i64`, `i32`, `u8`}). This closes the
"i32 values are unconstructible" gap surfaced by the examples-as-tests track (an integer
literal is `i64`, with no coercion to `i32` and no conversion function), which blocked a
true 32-bit **ChaCha quarter-round** — the headline branch-free primitive the track has
been building toward.

## Decision

### Why a cast, not builtins

The existing width conversions `i64_to_u8` / `u8_to_i64` are *builtin fn-signatures*. Adding
`i64_to_i32` / `i32_to_i64` the same way would **shift every user-fn FnId** (builtins occupy
a contiguous prefix, user fns follow), and FnIds are printed *by number* in the resolve /
MIR / borrow / effects dumps — so it would churn those dumps for every program and force a
lockstep re-bless of ~30 Rust dump-test assertions plus ~27 hardcoded base constants in the
selfhost sources. A cast is a **new expression node**: additive, so existing programs'
dumps and IR are unchanged and nothing needs re-blessing — exactly like the shift operator
(ADR 0048). It also generalizes (any int width) and reads naturally. The `as` keyword
already exists (used in `impl as Trait for Type` heads; the impl `as` is parsed only in item
position, so there is no conflict with a cast in expression position).

### Semantics

`x as T` converts an integer `x` to integer type `T`, by LLVM width op:

- destination **wider** than source → **zero-extend** if the source is `u8` (unsigned),
  else **sign-extend** (`i32`/`i64` are signed);
- destination **narrower** → **truncate**;
- **same width** → no-op (the value flows through, no instruction).

Widths: `u8` = 8, `i32` = 32, `i64` = 64. The operand and target must be integers; a
non-integer cast is a type error (`NonIntegerCast`).

### Secrecy + constant-time

A width conversion is **data-independent** — its latency does not depend on the value — so
casting a `secret` is **constant-time and allowed**, and the cast **preserves secrecy**:
`(secret T) as U` → `secret U`, `T as U` → `U`. There is **no constant-time sink** for a
cast (contrast the shift's `SecretShiftAmount` and the secret divisor): in MIR a cast lowers
to `MirOp::Opaque` carrying the operand (so taint propagates) with the result's type
speaking. This mirrors how `declassify` is a value-level identity — except a cast *keeps*
the secret qualifier rather than stripping it.

### Precedence

`as` binds tighter than every binary operator but looser than unary (Rust-like): `-x as i32`
is `(-x) as i32`, `a * b as i32` is `a * (b as i32)`. A new `parse_cast` level sits between
`parse_mul` and `parse_unary`; left-associative (`x as i32 as u8` chains).

## Implementation

A new `Cast` expression node threaded through every stage — mirroring `Declassify` (a
single-operand wrapper that already threads AST → resolved → typed → codegen → dumps), plus
a target type. The new variant compiler-forces an arm at every exhaustive expr match (the
safety net).

- **Phase 1 — `snc`:** `ExprKind::Cast(Box<Expr>, TypeExpr)` (+ resolved/typed mirrors;
  the typed node carries the resolved target on its `.ty`); `parse_cast`; the `check_expr`
  Cast rule (+ `NonIntegerCast`); the inkwell codegen trunc/sext/zext; MIR → `Opaque`;
  effect-/borrow-check + the codegen helper-walks gain a Cast arm; the ast/resolve/types/
  oracle dumpers emit `(cast …)`. Validated by the ChaCha quarter-round example (built by
  `snc` via the examples harness); the selfhost differentials stay byte-identical because no
  cast program enters the corpus.
- **Phase 2 — selfhost mirror:** an `Expr::Cast(Expr, i64 target-code)` variant + parser +
  the type/MIR/codegen walk + every selfhost dump table, then a `tests/pass` cast fixture so
  the corpus differential validates `scg == snc` byte-for-byte. Both bootstrap fixed points
  hold.

## Consequences

`std/security/ct` gains `ct_rotl32` (the 32-bit twin of `ct_rotl64`), and the track builds a
true **ChaCha quarter-round over `secret i32` words** verifying the RFC 8439 test vector —
rotations by public constants, all branch-free and constant-time. The existing
`i64_to_u8` / `u8_to_i64` builtins are unaffected (the cast is the general successor, but
they remain). Amendments (A1, …) will record any deviation once implemented.
