# ADR 0051: Implicit public → secret widening

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A5) — adds ergonomic, monotone public→secret widening.
The **operand widen** (binop + cmp) is in `snc` (Phase 1) AND mirrored into the self-hosted
`scg` (Phase 2); `tests/pass/c56_operand_widen` validates `scg == snc` byte-for-byte across all
8 selfhost stage differentials, both bootstrap fixed points hold, and the full nextest is green.
The **call-arg / return** widens were snc-side ergonomics (A1) and are now self-hosted too
(A5); the **array** widen remains snc-side. Amendments below record the deviations from the
PROPOSED plan.

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

## Amendments

- **A1 — the OPERAND widen is fully self-hosted; the call-arg / return / array widens are
  snc-side.** All five widen positions are implemented + validated in `snc` (unit tests +
  probes). Only the **operand widen** (binop + cmp) is mirrored into `scg`
  (`selfhost/types.sentinel`) with the corpus fixture `tests/pass/c56_operand_widen`, because
  the operand widen is the one the crypto code hits constantly. The **call-arg**, **return**,
  and **`[u8] → [secret u8]` array** widens are deliberately left snc-side: no
  `selfhost/*.sentinel` source and no corpus fixture uses them, so `scg` self-hosts and the
  selfhost corpus differentials + both bootstrap fixed points stay **byte-identical without
  mirroring them**. They are sound, additive, codegen-no-op ergonomics; mirroring them is a
  low-value follow-up (they don't change any emitted code). This is the first ADR-feature whose
  self-hosting is *partial by design* — justified because the widen is a pure type-checker
  ergonomic with zero emitted-code impact, so the "byte-identical" property is preserved
  trivially for everything self-hosted.

- **A2 — the scg mirror reused the existing `widen_splice` / `mir_widen` machinery wholesale.**
  The `Expr::Binary` arm now dumps each operand into its own buffer (the widen decision needs
  *both* operand types, known only after both `dump_texpr` returns), then splices the public
  operand back wrapped in `(widen-secret … :secret T)` via `widen_splice` (with the secret type
  as the synthetic expected type, so `widen_kind` returns the widen-secret kind) and wraps its
  MIR value via `mir_widen` (the `Opaque([inner])` the oracle lowers `WidenToSecret` to).
  Codegen needed *no* operand change: a secret widen is value-level identity and `secret i64`
  strips to the same `i64` LLVM type, so the captured operand kinds + the binop/cmp type args
  are already byte-identical. The corpus exercises the secret-left / public-right direction
  (the well-tested MIR `widen_r` path); left-widen is implemented for type/text completeness.

- **A3 — two snc unit tests were repurposed, not deleted.** `c31_mixed_public_secret_arithmetic`
  and `c53_bitwise_secret_public` previously asserted that `secret + public` / `secret ^ public`
  *reject* (Mismatch); they now assert the **widen** (accept, result secret) plus the preserved
  sink exclusions (`secret / public` still Mismatch; a secret shift AMOUNT still
  `SecretShiftAmount`). A new `adr0051_widen_forms_typecheck_and_stay_constant_time` covers all
  five widen positions + re-confirms a widened `secret == public` (now `secret bool`) still
  cannot reach an `if` (`SecretBranch`).

- **A4 — `secret / public` stays a `Mismatch` (Div excluded from the operand widen).** Division
  is special: a secret divisor is variable-time (`SecretDivisor`), so widening a public divisor
  to secret would *trip the sink*. Rather than decouple Div's operand secrecy (like shifts), the
  ADR excludes Div from the widen entirely — `secret_x / 5` remains a `Mismatch` (it is not
  needed by the crypto code). The shift exclusion is the same: the amount is never widened.

- **A5 — the CALL-ARG and RETURN widens are now self-hosted too; only the ARRAY widen stays
  snc-side.** A1 left all three of the call-arg / return / array widens deliberately
  unmirrored, on the grounds that "no `selfhost/*.sentinel` source and no corpus fixture uses
  them" and that mirroring is "a low-value follow-up". Two of those three are now mirrored,
  and A1's cost/benefit is amended accordingly — not contradicted silently.

  **What changed the calculation is that the same missing machinery is NOT `secret`-specific.**
  `scg` threaded no expected type into a call argument, a `return` operand, or an assignment
  right-hand side AT ALL, so the identical gap also dropped ADR 0014 D3's `?T` pushdown, which
  no ADR ever deferred. And the assignment position was not a text divergence: `o = 42` into a
  `?i64` binding emitted `store { i1, i64 } 42, ptr %v0` — an aggregate store of a bare
  integer that `llvm-as` rejects with "integer constant must have integer type". Threading the
  expectation fixes all three positions and both flavours at once; suppressing the `secret`
  half to honour A1 literally would have meant writing extra code to keep a known-wrong
  answer.

  The mechanism is the expectation, not a new widen: the callee's declared param type for an
  argument (`fn_param_ty`), the enclosing fn's return type for a `return`, and the assigned
  place's type for an assignment. `widen_kind` then decides as it always has, so an argument
  whose type already IS the parameter's is not double-wrapped.

  **A5 covers USER-FN arguments only, and the builtin gap it leaves is a live invalid-IR
  divergence, not a design choice.** `fn_param_ty` answers -1 for a builtin, which preserves
  today's behaviour exactly — but today's behaviour is wrong: the oracle DOES widen a
  builtin's argument (`let y: i64 = unwrap_or(5, 0);` emits
  `(widen-null (int 5 :i64) :?i64)`), and scg drops it and emits `extractvalue i64 5, 0`,
  which `llvm-as` rejects. ADR 0052's own text already relied on that widen, describing
  `push(&mut v, i64_to_u8(base + j))` as widening "the public byte at the push argument via
  the ADR 0051 call-arg widen". The builtin arms infer their type argument from the ARGUMENTS
  rather than from an expectation, which is the same shape as the generic user-fn gap, so it
  is filed as its own slice rather than folded in here.

  **The ARRAY widen (`[u8]` → `[secret u8]`) is NOT mirrored and stays A1-deferred**, and it
  is really two different gaps that a single test does not separate:

  - a whole ARRAY VALUE (`let s: [secret u8] = b;` where `b: [u8]`) — `widen_kind` does not
    implement this at all, so it is not a threading question;
  - an array LITERAL (`let s: [secret u8] = [i64_to_u8(1)];`) — here the oracle widens each
    ELEMENT (`(array (widen-secret …) …)`), so `widen_kind` DOES handle `u8` → `secret u8`;
    what is missing is threading the element expectation into the `ArrayLit` arm, which
    makes it a member of the sibling arm family rather than an A1 item.

  Both are unchanged by A5 — verified by building a pre-change stage and diffing, not
  inferred.

  **"Call argument" here means a plain `f(x)` call ONLY.** Every other argument path still
  passes no expectation and still diverges: a METHOD or impl-method argument, a QUALIFIED
  trait call, a CLASS `init`, an enum construct, a `spawn` target, and a GENERIC user fn
  (which routes through `dump_generic_call`). The oracle widens at those too, for a plain
  literal — `C::init(2)` against `init(v: ?i64)` emits a `widen-null` that scg omits. None is
  covered and none is claimed to be.

  **And the residual ARM family is not cosmetic at these positions.** Those arms discard the
  expectation they now receive; at a `let` that costs only the wrapper, but at the ASSIGN and
  RETURN positions the store and the `ret` are rendered from the TARGET type while the operand
  comes back un-widened, so scg emits `store { i1, i64 } %v4` with `%v4 : i64` — invalid IR,
  where the oracle's is clean. Verified PRE-EXISTING against a pre-change stage, so A5 did not
  cause it; but the same invalid-IR severity that justified A5 applies to the residue, and the
  register entry has been corrected to say so rather than calling it a text divergence.

  Pinned by `tests/pass/c19_widen_arg_return_assign`, covering all three positions in both
  flavours. Neither bootstrap fixed point moves: no `selfhost/*.sentinel` source has any of
  these shapes.
