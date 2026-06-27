# The constant-time model: what `verify_constant_time` actually checks

This document is the **contract** for Sentinel's constant-time (CT) guarantee:
exactly which language constructs the MIR-level CT verifier models *precisely*,
which it models *conservatively*, and which are *not on the 1.0 CT path*. It is
the artifact to read first when auditing the `secret` guarantee, and the
specification the secret-flow conformance suite
([`tests/ui/c52_secret_*`](../tests/ui/) + the accept-side
[`tests/pass/c52_secret_through_constructs_ok`](../tests/pass/c52_secret_through_constructs_ok.sentinel))
tests against.

It calibrates a claim, not a marketing line. The README's "headline capability"
section states the guarantee in prose; this is the mechanical detail behind it.
When this document and [`STATE.md`](STATE.md) disagree, STATE.md wins; when it
and the code (`crates/sentinel-mir/src/lib.rs`) disagree, the code wins —
report the drift as a bug.

## The guarantee, precisely

`snc build` lowers each function's type-checked body to a minimal SSA MIR
([`lower_to_mir`]) and runs [`verify_constant_time`], which **rejects** the
program (`sentinel::mir::secret_leak`) if a `secret`-typed value reaches any of
**four sinks**:

| Sink                       | MIR site                              | Source shape |
| -------------------------- | ------------------------------------- | ------------ |
| `SinkKind::Branch`         | a `Branch` terminator's condition     | `if s { … }`, `s && …`, `s \|\| …` |
| `SinkKind::MemoryIndex`    | a `Load`'s index operand              | `a[s]` |
| `SinkKind::MemoryAddress`  | a `Load`'s base (pointer) operand     | `*p` where `p` is secret |
| `SinkKind::Division`       | a `Binary(Div)`'s divisor operand     | `x / s` |

An empty result means the program is **constant-time at the MIR level**.
`declassify(e)` is the one sanctioned escape: it produces a non-secret-typed
value, so anything downstream of it may branch/index/divide freely (you are
asserting the value is safe to treat as public).

### Two boundaries (the same ones the README states)

1. **The taint oracle is the type system.** A value is `secret` iff its `Type`
   says so. The type checker's operator-secret-preserving rules already
   computed the taint fixpoint (a result is secret iff a source-reachable
   operand was; `declassify` clears it; function-signature boundaries are
   respected), so the verifier **reads taint straight off the type**
   (`MirFunction::is_secret`) — there is no independent dataflow pass. **The
   verifier is therefore exactly as sound as the type checker's secret
   propagation.** That is the assumption the conformance suite exists to test.

2. **It runs before LLVM optimization.** It constrains the program you wrote,
   not the optimized machine code, and it does not *force* constant-time
   emission (no `cmov` forcing, no speculation barriers, no post-codegen
   assembly verification — all future work). A standalone forward taint
   propagation only becomes necessary once MIR is lowered from
   *post-optimization* code; that is a recorded post-1.0 amendment to ADR 0026
   D5.

## Per-construct modeling

Every typed expression lowers to exactly one MIR shape. The shape determines
whether a secret flowing *through* that construct can still be *seen* at a sink.
Three tiers:

- **Precise** — the construct lowers to the exact op the verifier inspects (a
  sink, or a straight-line op that carries its operand's type). A secret here is
  caught directly.
- **Conservative (Opaque / Call)** — the construct funnels through `MirOp::Opaque`
  (or `MirOp::Call`), which **carries its operands** and whose **result type is
  the type checker's verdict**. The verifier reads secrecy off that result type;
  the carried operands are there for the future post-optimization pass. Soundness
  here rests entirely on the type checker propagating `secret` correctly through
  the construct — see "What conservative means" below.
- **Off the 1.0 CT path** — not lowered, so not analyzed.

| Construct (`TypedExprKind`)                         | MIR lowering                          | Tier |
| --------------------------------------------------- | ------------------------------------- | ---- |
| `IntLit` / `BoolLit`                                | `ConstInt` / `ConstBool`              | Precise (never secret) |
| `Var`                                               | the var's current SSA value (typed)   | Precise (taint by type) |
| `Unary(Deref)` (`*p`)                               | `Load { base, index: None }`          | **Precise sink** (MemoryAddress) |
| `Unary(_)` (`-`, `!`)                               | `Unary`                               | Precise |
| `Binary(_)`                                         | `Binary` — **Div divisor is a sink**  | **Precise sink** (Division) |
| `Cmp(_)`                                            | `Compare`                             | Precise |
| `Logic(&&, \|\|)`                                   | `Branch` (short-circuit)              | **Precise sink** (Branch) |
| `If`                                                | `Branch` + merge                      | **Precise sink** (Branch) |
| `Index` (`a[i]`)                                    | `Load { base, index: Some }`          | **Precise sink** (MemoryIndex) |
| `Declassify`                                        | `Declassify` (clears the secret bit)  | Precise (the taint clear) |
| `Block` / `Scope`                                   | lowered inline (transparent)          | Precise |
| `CharLit` / `StringLit` / `NullLit`                 | empty `Opaque` (public constant)      | Conservative (never secret) |
| `WidenToSecret` / `WidenToNullable`                 | `Opaque(operand)` (identity widen)    | Conservative (type carries the bit) |
| `Call`                                              | `Call { callee, args }`               | Conservative (result secrecy = signature return type) |
| `StructLit` / `FieldAccess`                         | `Opaque(operands)`                    | Conservative |
| `ArrayLit`                                          | `Opaque(elements)`                    | Conservative |
| `MethodCall` / `ImplMethodCall` / `QualifiedCall` / `ClassInit` | `Opaque(receiver?, args)` | Conservative |
| `EnumConstruct` / `Match`                           | `Opaque(args / scrutinee + arm bodies)` | Conservative |
| `Perform` / `ResumeKont` / `Spawn` / `Await`        | `Opaque(operands)`                    | Conservative |
| `Handle` **arm bodies**                             | **not lowered** (only the handle body is) | **Off the 1.0 CT path** |

## What "conservative" means for soundness

The conservative tier is sound **iff the type checker propagates `secret`
correctly through the construct**. The MIR does not re-derive secrecy through an
`Opaque`; it trusts the result's `Type`. Two consequences:

- A construct whose *result type* correctly inherits a secret operand's secrecy
  is sound: if `f(s)` returns `secret i64`, then `if f(s) { … }` reaches a
  `Branch` with a secret condition and is rejected, even though `f`'s call
  funneled through `Call`. The **secret-flow conformance suite** routes a secret
  through each conservative construct into each sink and asserts the leak is
  *still* rejected — turning "trust the type checker" into a test.
- A construct that *loses* secrecy at the type level would be a real
  under-rejection. That is precisely what the conformance suite is built to
  catch; any such case is a soundness bug in the type checker, not in this pass.

This is why the calibrated README claim says the guarantee is "as sound as the
type checker's secret propagation" rather than "proven": the pass is a faithful
reader of a verdict computed elsewhere.

## Known gaps (stated honestly)

- **Handler arm bodies are not on the 1.0 CT path.** `Handle` lowers only its
  body, not its arms (the arms bind handler-scoped variables this minimal
  lowering does not model). A secret used *inside a handler arm body* is not
  CT-checked. Effects-carrying secret handling is out of the 1.0 CT scope.
- **The type checker is the single point of trust.** Per the boundary above, a
  secret-propagation bug in the type checker would false-negative this pass. The
  conformance suite is the mitigation; an independent secret-dataflow oracle is
  post-1.0 work.
- **Pre-optimization.** The check constrains the program, not the optimized
  machine code; constant-time *emission* is future work (README, ADR 0026 D5).
- **No `secret enum` at the MVP.** `[secret T]` arrays now exist (ADR 0047) and
  are covered — a secret array element reaching a branch, index, or divisor is
  rejected like any other secret (the variable-length constant-time `memcmp`
  over secret bytes relies on exactly this). What remains not-yet-expressible is
  a **`secret enum`** (a secret enum *payload*): that "secret through an enum
  aggregate" flow cannot be written, so it cannot leak. The conformance suite
  tests the flows the language admits today (secret scalars and secret array
  elements through calls, fields, `match`, arithmetic, `&&`/`||`, into each
  sink), and records the not-yet-expressible ones as deferred.
- **`lookup_var` robustness.** An unbound MIR variable (a resolver/lowering bug)
  would emit a taint-free `Opaque` and could mask a secret; a debug-build
  assertion now makes that loud (review F4), so it cannot pass CI silently.

## References

- `crates/sentinel-mir/src/lib.rs` — `lower_to_mir`, `verify_constant_time`,
  `MirFunction::is_secret` (the implementation; the source of truth).
- [`decisions/0026-hir-mir-pipeline-and-constant-time-secret-codegen.md`](decisions/0026-hir-mir-pipeline-and-constant-time-secret-codegen.md)
  — ADR 0026 D5 (the CT verification design) + the type-as-oracle /
  pre-optimization amendments.
- The README "headline capability" section — the prose statement of the same
  guarantee.
- The secret-flow conformance suite — `tests/ui/c52_secret_via_{call,field,match}`
  + `c52_secret_or_leak` (taint survives the conservative funnels → caught by the
  MIR pass), `c52_secret_{in_if,array_index,divisor}` (the type checker's
  source-level sink rejections), and `tests/pass/c52_secret_through_constructs_ok`
  (the accept side) — plus `tests/ui/c52_secret_leak` and the `tests/pass/c5*`
  constant-time go/no-go programs. The behavioral evidence.
