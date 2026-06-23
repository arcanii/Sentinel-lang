# ADR 0051: Implicit public → secret widening

Status: **PROPOSED** — adds ergonomic, monotone public→secret widening so a public value can
combine with / flow into a `secret` value without a `let`-boundary pre-bind. Phase 1 implements
it in the type checker (`snc`); Phase 2 mirrors it into the self-hosted `scg` with a corpus
fixture so the differential validates `scg == snc` byte-for-byte. Amendments (A1…) will record
deviations from this plan.

Removes the pervasive friction the examples-as-tests track keeps hitting: mixing a `secret`
value with a public value in operand position is rejected (`type mismatch: expected secret,
found i64`), so every public constant that meets a secret must be pre-bound `secret` at a `let`
first (e.g. `let ones: secret i64 = 0 - 1;` in `std/security/ct`, and one `let`-widen *per
message word* in SipHash / ChaCha20 / Poly1305). This ADR makes the widen implicit.

## Decision

### What widens

A **public `T`** is implicitly widened to **`secret T`** (the result is `secret T`) in these
positions, when the inner types otherwise match:

1. **Binary operand** — `secret_x OP public` (and `public OP secret_x`) for the *symmetric*
   integer ops `+ - * & | ^`. The public operand is widened; the result is `secret`.
2. **Comparison operand** — `secret_x CMP public` for `== != < <= > >=`. The result is
   `secret bool`.
3. **Call argument** — `f(public)` where the parameter is `secret T` (extends the existing
   bidirectional expected-type pushdown — already done for `?T` — to `secret T`).
4. **Return value** — `fn f() -> secret T { public_expr }` (pushes the expected return type
   into the body, like a `let` annotation).
5. **Array** — a public `[u8]` flowing into a `[secret u8]` context (a `let`-annotation, a
   `secret [u8]` parameter, or a `secret [u8]` return). A runtime no-op (see below).

It is implemented by inserting the existing **`WidenToSecret`** node (ADR 0019 D5) — the same
node a `let pw: secret i64 = 42;` already inserts — in these new positions. For the array case
the node re-tags `[u8]` as `[secret u8]`.

### What does NOT widen (the constant-time sinks stay intact)

The widen is deliberately excluded where it would defeat a constant-time guarantee:

- **Shift amount** (`x << n` / `x >> n`, ADR 0048) — the amount is *not* widened. Shifts are
  already asymmetric (a secret *value* by a public *amount* is constant-time; a secret *amount*
  is rejected). Widening a public amount to secret would wrongly trip `SecretShiftAmount`.
- **Divisor** (`a / b`, ADR 0019 D7) — `/` is excluded from the operand widen. A secret divisor
  is variable-time and rejected; widening a public divisor to secret would wrongly trip
  `SecretDivisor`. (`secret / public` therefore stays a `Mismatch` — out of scope; the crypto
  code does not need it.)
- **`&&` / `||`** (logical short-circuit, a secret-branch sink) — deferred (low value; the
  crypto code does not mix public/secret booleans). A `secret bool` still cannot reach an `if`
  / `while` / `&&` / `||`.

Different integer *widths* (`secret i32 + i64`) still mismatch — the widen only fires when the
stripped inner types are equal.

### Constant-time soundness

Public→secret widening is **monotone**: it only *adds* secrecy (a type-level tag), never
removes it, and the bits are identical. It therefore cannot create a secret→public leak:

- The widen only fires on combinations that were previously **rejected** (a public operand met
  a secret one). It changes a public operand's *type tag*; it introduces no new value and no new
  data path.
- Every constant-time sink — `SecretBranch` (a secret `if`/`while`), `SecretShiftAmount`,
  `SecretDivisor`, `IndexNotInt` (a secret memory index), the `&&`/`||` secret rejection — fires
  on the **secret type after** inference and is *orthogonal* to the widen. A value that is now
  secret because it was widened is rejected at those sinks exactly as a natively-secret value
  is. (Indeed the shift-amount and divisor exclusions above exist precisely so the widen does
  not *suppress* a sink by making a public operand look acceptably matched.)
- No `secret` is ever narrowed to public (only `declassify` does that, unchanged).

So the widen strictly *grows* the set of accepted programs, and every newly-accepted program is
one whose secret flow is still fully constrained by the unchanged sinks. The MIR `secret_leak`
pass — the authoritative constant-time oracle — is unchanged.

### ABI / additive

**Purely additive, zero codegen change.** `WidenToSecret` already lowers as **identity** in all
three backends (inkwell, the `snc llvm` oracle, and `scg`'s `cg_widen` for the secret kind) —
secrecy is a type tag and `[u8]` / `[secret u8]` share the same `{i64 len, ptr data}` layout, so
the widen emits no instruction. Every existing program type-checks unchanged (the widen only
fires on previously-*rejected* code), so its typed AST / MIR / IR and all dumps are byte-
identical; the frozen `abi-v1`, both bootstrap fixed points, and the selfhost corpus differential
are unaffected. A new fixture exercises the widen (Phase 2) and `scg` mirrors it.

### Stages touched

- **Types** (`sentinel-types`): a `widen_operand_to_secret` helper (wraps a public operand in
  `WidenToSecret`); applied in the Binary (symmetric ops) + Cmp arms; the call-arg expected-type
  pushdown extended from `is_nullable` to also `is_secret`; the return-type pushdown likewise;
  `coerce_to_expected` gains a `[u8] → [secret u8]` array arm. No new `TypeError`, no new node.
- **Codegen** (inkwell + oracle): **none** (`WidenToSecret` is already identity).
- **MIR / CT**: **none** (the sinks are unchanged; the widen is orthogonal).
- **Selfhost** (`selfhost/types.sentinel`, Phase 2): mirror the operand widen (the `widen_splice`
  / `mir_widen` / `cg_widen` machinery from ADR 0049 A3) in the binop/cmp arms + the coercion
  sites; add `tests/pass/c56_*` exercising the widen so the corpus differential validates it.

### Alternatives considered

- **Keep the explicit `let`-bind** (status quo) — rejected: it is real, repetitive friction
  (the MAC examples each carry ~10 widen-only `let`s), and the implicit widen is sound + free.
- **Widen at the operand always, including shifts/div** — rejected: it would *suppress*
  constant-time sinks (a secret shift amount / divisor). The exclusions are load-bearing.
- **A `widen(x)` builtin / explicit cast to secret** — rejected: noisier than implicit, and the
  `as` cast already preserves secrecy without changing it (ADR 0049); a secrecy-*adding* implicit
  coercion is the natural dual of the existing `let`-widen.

## Consequences

- `secret_x + 5`, `secret_x == k`, `f(public)` into a `secret` param, and `[secret u8]` buffers
  built from public bytes all type-check directly — the crypto libraries lose their widen-only
  `let` ceremony.
- The constant-time guarantee is unchanged (the sinks are untouched; the exclusions keep the
  shift/divisor sinks intact).
- No ABI / codegen / dump change; both fixed points and the corpus differential stay byte-
  identical.
- Deferred: `secret / public` (divisor decoupling), `&&` / `||` public/secret mixing, and the
  general `[T] → [secret T]` array widen (scoped to `[u8]` first, matching ADR 0047).
