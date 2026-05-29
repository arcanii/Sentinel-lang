# ADR 0027: Bitwise operators (`& | ^`, then `<< >> ~`) for constant-time crypto

Status: ACCEPTED-WITH-AMENDMENTS (C5.3 closed 2026-05-29) — a
concrete-surface ADR (mirroring ADR 0012/0013) that adds integer bitwise
operators, the prerequisite surface for the C5.0 go/no-go's constant-time
`Finished` MAC verify (ADR 0025 D3/D13) and the canonical way to compute
on `secret` values (ADR 0008).

**Amendment A1 (C5.3 close): the `<< >> ~` wave (C5.4) is deferred.**
C5.3 shipped the token-clean, go/no-go-critical trio `& | ^` end-to-end
(lexer → parser → codegen; types / MIR / the D5 pass needed no change —
they handle `Binary` generically). The shift + complement wave (C5.4,
D9) — which carries the `>>`-vs-nested-generic-close split — is a
**follow-on**, taken only if the go/no-go's hash *computation* lands
in-language; its constant-time *compare* (the security core) needs only
`^`/`|`, which shipped. All other D-decisions (D1's `& | ^` half,
D2–D8, the C5.3 rows of D10) landed as specified. **Numbering: the "C5.4"
label this ADR uses for the shift wave is superseded** — the broker
sub-phase took C5.4 / ADR 0028, so the `<< >> ~` wave is an *unnumbered*
deferred follow-on (do it under this ADR's D9 when/if the go/no-go's hash
computation needs shifts).

Date: 2026-05-29
Related:
  - **0025** (Phase C5 kickoff — PROPOSED): the C5.0 resolution pins the
    1.0 go/no-go to a TLS 1.3 handshake whose security core is the
    **constant-time `Finished` MAC verify** — a constant-time equality
    compare, i.e. an XOR-accumulate (`acc |= a ^ b`). That needs `^` and
    `|`, which the surface lacks. This ADR is the scope addition that
    unblocks it.
  - **0026** (HIR/MIR + constant-time — PROPOSED): C5.2b shipped the D5
    verification (`verify_constant_time`) + a `c52_secret_ct` pass
    fixture, but that fixture had to be an *arithmetic* masked select
    (`c*a + (1-c)*b`) because there are no bitwise operators. D5 already
    treats every `Binary` except `Div` as a constant-time non-sink, so it
    needs no change to accept bitwise secret code (D7).
  - **0019** (Phase C3 — ACCEPTED-WITH-AMENDMENTS): C3.1b's
    operator-secret-preserving arithmetic rule (`secret op secret →
    secret`; mixed → `Mismatch`); bitwise typing mirrors it exactly (D5).
  - **0012** (concrete C1 surface — ACCEPTED): the comparison/logical
    operator precedence ladder + the parallel-tree "update every
    exhaustive match together" discipline this reuses.
  - **0008** (secret qualifier + constant-time — ACCEPTED): bitwise ops
    are data-independent-latency — the constant-time primitives `secret`
    code should be written in.
  - **0016** (interner-`Copy`-`Type`): unaffected — bitwise ops add no
    `Type` variants.

## Context

`sentinel_ast::BinOp` is only `{ Add, Sub, Mul, Div }`; there are no
bitwise operators. C5.2b's D5 constant-time verification landed, but the
constant-time primitive it can verify today is limited to arithmetic,
because the canonical constant-time idioms are bitwise:

  - **constant-time equality / MAC verify** — `acc |= a[i] ^ b[i]` over
    the bytes, then `acc == 0` once (the go/no-go's `Finished` verify);
  - **masked select / `cswap`** — `(m & a) | (!m & b)` with a 0/-1 mask;
  - **bit manipulation** generally (hashing, field arithmetic).

None are expressible. So bitwise operators are a prerequisite for the
1.0 go/no-go, and — because they are data-independent-latency (ADR
0008) — they are precisely what `secret` code wants. The existing
arithmetic `Binary` machinery (resolve / types / codegen / the C5.1b MIR
lowering / the C5.2b D5 pass) already handles the *shape*; this ADR adds
the operators to it.

Two lexing facts shape the plan:
  - `&` already exists (`Amp`, the borrow prefix) and `&&` / `||` exist
    (`AmpAmp` / `PipePipe`); `|`, `^`, `<<`, `>>`, `~` do not.
  - A `>>` token would collide with the close of a nested generic
    (`Pair<Pair<i64, i64>, i64>`), which currently lexes as two `>`
    tokens the parser consumes. Adding `>>` therefore needs a deliberate
    parser split (D9); this ADR sequences shifts *after* the token-clean
    `& | ^`.

## Decision

Ten D-numbered decisions. The surface lands per the sub-phase split (D10).

### D1. Operator set + two-wave sequencing.

Target the standard integer bitwise set `& | ^ << >> ~`, landed in two
waves so the go/no-go-critical, token-clean operators ship first:

  - **C5.3 — `& | ^`** (infix bitwise and / or / xor): token-clean and
    exactly the trio the constant-time compare (`^`, `|`) and masked
    select (`&`) need. Fully specified below.
  - **C5.4 — `<< >> ~`** (shifts + complement): `<<` / `>>` require the
    `>>`-vs-generic-close resolution (D9), and shifts are *not* needed
    for the constant-time *compare*; they land in a deliberate follow-on.

### D2. Tokens (C5.3).

Two new logos tokens: `|` → `Pipe`, `^` → `Caret`. The existing `&`
(`Amp`) is **reused** as the infix bitwise-and; the prefix `&` stays the
borrow operator, disambiguated by parser position (D3). Longest-match
keeps `&&` → `AmpAmp` and `||` → `PipePipe` (a single `&` / `|` only
matches when not doubled). **No `<<` / `>>` tokens at C5.3** — deferred
to C5.4 so the nested-generic close-ambiguity is resolved deliberately,
not stumbled into.

### D3. Precedence (C5.3).

Rust-standard, inserted between comparison (looser) and additive
(tighter), with `&` tightest and `|` loosest of the three. The
recursive-descent ladder gains three levels:

    parse_cmp → parse_bitor → parse_bitxor → parse_bitand → parse_add

So `a + b & c` is `(a + b) & c`; `a & b == c` is `(a & b) == c`;
`x ^ y | z` is `(x ^ y) | z`. The infix-vs-prefix `&` split falls out of
the climb for free: a prefix `&x` is consumed by `parse_unary` (deep
inside `parse_add`) before `parse_bitand`'s infix loop ever sees it, so
`&x & &y` parses as `(&x) & (&y)`. (When shifts arrive at C5.4 they slot
between `&` and additive, per Rust.)

### D4. AST + the downstream IRs.

Extend `sentinel_ast::BinOp` with `BitAnd`, `BitOr`, `BitXor`. They join
the arithmetic `Binary` family, so resolve, types, codegen, the C5.1b
MIR lowering, and the C5.2b D5 pass all handle them through their
existing `Binary` paths — **no new `ExprKind` / `TypedExprKind` /
`MirOp` variants**. Per the parallel-tree discipline (ADR 0012 D-style),
every exhaustive `BinOp` match is updated in one coordinated commit.

### D5. Typing — secret-preserving, integer-only.

Mirror the C3.1b arithmetic rule exactly (ADR 0019): both operands must
be the **same integer type** (`i32` / `i64`); the result is that type;
`secret op secret → secret`, public stays public, and mixed
public/secret surfaces as the existing `Mismatch` (the SecretFlow rule).
`bool` operands are rejected as `Mismatch` — `&&` / `||` stay the
boolean operators. **No new `SecretXxx` rejection and no new `TypeError`
variant:** a bitwise op on a secret is data-independent-latency (ADR
0008), i.e. exactly the constant-time computation we want — never a leak.

### D6. Codegen.

`BitAnd` / `BitOr` / `BitXor` lower to LLVM `and` / `or` / `xor` on the
operand integer type. `secret T` continues to lower as its inner `T`
(ADR 0019 D12, unchanged). Emission stays reproducible (lookup-table
maps; source-ordered walks — ADR 0025 D8).

### D7. MIR + the D5 verification — no change needed.

The new `BinOp` variants flow through `MirOp::Binary` in `lower_to_mir`
unchanged (the `Binary` arm is operator-generic). `verify_constant_time`
already treats every `Binary` *except* `Div` as a constant-time,
taint-propagating non-sink, so a `secret` bitwise result correctly stays
tainted (it can't silently leak downstream) while the bitwise op itself
is never flagged. This is the point: bitwise is how you legally compute
on a secret. The C5.2b D5 amendment (type-directed taint) carries over
unchanged.

### D8. Out of scope at C5.3 (deferred / never).

Shifts `<< >>` and complement `~` (→ C5.4, D9); bit-rotate (a future
fn/intrinsic, not an operator); bitwise on `bool`; compound-assignment
(`&= |= ^=`); and `[secret T]` **arrays of secrets** — the
`ArrayElem` / `NullableInner` flat subset (ADR 0015 D6) has no `Secret`
variant, so a secret array is not representable, and the constant-time
fixtures use **scalar** secrets (an arrays-of-secrets surface is its own
deferred follow-on, orthogonal to the operators).

### D9. C5.4 plan — shifts + complement.

`<<` → `Shl`, `>>` → `Shr` (new tokens); `~` → `Tilde` (a new
`UnaryOp::Complement`). Precedence: shifts sit between `&` and additive
(Rust). The `>>`-vs-generic-close ambiguity (`Foo<Bar<T>>`) is resolved
the way Rust does it — **the generic type-argument parser splits a `Shr`
token into two `>`**: on seeing `Shr` where it expects a closing `>`, it
consumes one `>` and leaves a pending `>` for the enclosing argument
list. `~x` lowers to `xor x, -1`. The exact split mechanics are confirmed
at C5.4 open (its own sub-phase commit pair).

### D10. Sub-phase split.

| Sub        | Title                                                        | Risk   | Est.   |
|------------|--------------------------------------------------------------|--------|--------|
| C5.3 (1/N) | lexer: `\|` (`Pipe`) + `^` (`Caret`) tokens                   | low    | ~0.3   |
| C5.3 (2/N) | `& \| ^` surface end-to-end (BinOp + parser precedence +      | medium | ~0.5-1 |
|            | secret-preserving typing + codegen; MIR/D5 already cover it)  |        |        |
|            | + `c53_*` fixtures; ADR 0027 flip.                           |        |        |
| C5.4       | `<< >> ~` (+ the `>>`/generic-close split).                  | medium | ~1     |

Total ~2-2.5 sessions. C5.3 is the go/no-go-critical, low-ambiguity wave;
C5.4 carries the one genuinely fiddly bit (the `>>` split).

## Reasoning

**Why bitwise now.** C5.2b proved the constant-time *verification*
works, but its pass fixture had to fake the primitive (arithmetic masked
select) because the real constant-time idioms — equality compare, masked
select, MAC verify — are bitwise. The go/no-go *is* a constant-time MAC
verify. Bitwise operators are the smallest surface that makes the 1.0
target writable, and they reuse the entire existing `Binary` pipeline.

**Why `& | ^` before `<< >>`.** The constant-time compare needs only
`^` and `|`; masking needs `&`. All three are token-clean. Shifts add the
nested-generic `>>` ambiguity, which deserves its own careful change
rather than riding in on the critical path. Sequencing delivers the
go/no-go value first and isolates the fiddly bit.

**Why no new secret rejection.** The C3.1 rejections (`SecretBranch`,
`SecretDivisor`, `SecretInRefDeref`) target *variable-latency or
control-flow* uses of a secret. Bitwise ops are the opposite — constant
latency — so they are the *sanctioned* secret computation. Adding a
rejection would forbid exactly the code we are enabling. D5 keeps them as
taint-propagating non-sinks so a secret stays tracked without being
flagged.

## Consequences

### Positive
- The 1.0 go/no-go's constant-time `Finished` MAC verify becomes
  writable; `c52_secret_ct` can be upgraded from a faked arithmetic
  select to the real XOR-accumulate compare.
- Reuses the whole `Binary` pipeline (resolve→types→codegen→MIR→D5) —
  small, low-risk, parallel-tree-mechanical.

### Negative
- `&` is now triple-purposed at the lexer (borrow prefix / `&&` logical /
  `&` bitwise infix); position-based disambiguation is correct but is one
  more thing a reader must hold. Documented in the parser.
- C5.4's `>>` split is genuine parser surgery (mitigated by sequencing it
  out of the critical path).

### Neutral
- No `Type` change; `secret` / `declassify` unchanged. Arrays-of-secrets
  remain unrepresentable (D8) — independent of this ADR.

## Alternatives considered

- **Ship the full `& | ^ << >> ~` set in one wave.** Rejected: drags the
  `>>`/generic ambiguity onto the go/no-go critical path for no
  compare-time benefit. Sequenced instead (D1).
- **Add a `>>` token and split it everywhere it's wrong.** That is the
  C5.4 plan (D9) — but only once shifts are actually being added, not now.
- **A constant-time-compare *builtin* instead of operators.** Rejected:
  operators compose (masked select, bit tricks, hashing) and match how
  crypto code is written; a single builtin would be a dead end.
- **A new `secret`-only bitwise rejection.** Rejected (Reasoning): bitwise
  is the *sanctioned* constant-time secret computation.

## Revisit

PROPOSED until C5.3 closes. Per-D triggers:
- **D2/D3**: revisit at C5.3 (1/N)/(2/N) — if the `&` infix/prefix split or
  the precedence insertion misbehaves on real fixtures.
- **D5**: revisit at C5.3 (2/N) — if a constant-time idiom wants a typing
  rule the arithmetic mirror doesn't cover (e.g. shift-amount typing at
  C5.4).
- **D9**: revisit at C5.4 open — the `>>` split mechanics.

## Appendix: estimated implementation footprint

| Workstream                                            | LOC estimate |
|-------------------------------------------------------|--------------|
| lexer (`Pipe` + `Caret` tokens + tests)               | ~40          |
| AST `BinOp` + Display + parser precedence (3 levels)  | ~120         |
| resolve pass-through + types secret-preserving rule   | ~80          |
| codegen (`and`/`or`/`xor`)                            | ~40          |
| fixtures (`c53_bitwise`, `c53_ct_eq`)                 | ~40          |
| **C5.3 total**                                        | **~320**     |
| C5.4 (`<< >> ~` + `>>` split)                         | ~250-400     |

MIR (`lower_to_mir`) and the D5 pass need no changes (D7) — the `Binary`
arm is operator-generic and bitwise ops are non-sinks.
