# ADR 0033: Phase D.2 — strings + a byte (`u8`) type

Status: PROPOSED — the second Phase D sub-phase ADR under ADR 0031 (Phase D
kickoff) D4 item 2. After sum types (D.1 / ADR 0032), strings + a byte type
are the next self-hosting prerequisite: a compiler's input is **text**, so a
lexer-in-Sentinel cannot start without a way to hold, index, and compare
source bytes. Flips to ACCEPTED(-WITH-AMENDMENTS) as the sub-phase lands.

Date: 2026-05-30
Related:
  - **0031** (Phase D kickoff): D4 item 2 names "strings + a `u8`/byte type +
    string/byte literals — text in, the compiler's input"; D5 sequences the
    self-host port lexer-first, which consumes this.
  - **0015** (arrays / nullable): `[T]` lowers to `{ i64 len, ptr data }`
    with a heap data buffer. **A string is a `[u8]`** at this MVP, so it
    reuses this layout + the array `len`/index/drop/move/escape machinery
    wholesale — the central design lever (D3).
  - **0032** (sum types): the same "new interner-less primitive `Type`
    variant cascades through every exhaustive `Type` match" discipline
    applies to `Type::U8` (mirrors the `Type::Enum` cascade, minus the
    interner table — `u8` is a scalar primitive like `i32`).
  - **0028** (broker scope arenas): a string-literal heap copy is a
    non-escaping primitive `[u8]` buffer, so it is **arena-routable** by the
    existing `compute_arena_routed` pre-pass with no new work (D6).
  - **0019 / 0027** (secret typing; bitwise): `u8` is an integer scalar, so
    it inherits the secret-preserving operator rules + bitwise ops with no
    new typing surface (D4).

**(1/N) update (2026-05-30) — DELIVERED.** The lexer recognises **string
literals** `"…"` and **char literals** `'…'` (logos regexes that handle
`\`-escapes — `\"`/`\'`/`\\`/`\n`/`\xHH` — and exclude raw newlines so an
unterminated literal fails fast; recognise-not-decode, like `IntLit`, so the
byte value is recovered at parse time). Confirmed **`u8` lexes as an
`Ident`** (recognised as a type name at the types layer, like `i64`/`i32`/
`bool` — no keyword token needed), so (1/N) is *only* the two literal
tokens. Additive — the parser consumes them at (2/N); the new `TokenKind`s
cascade nowhere (no exhaustive `TokenKind` match downstream). +7 lexer tests
(1275), four-check green. Next: **(2/N)** AST + parser
(`ExprKind::CharLit`/`StringLit`; `u8` in `TypeExpr`).

**(2/N) update (2026-05-30) — DELIVERED (`f310f15`).** The AST + parser
land. `ExprKind` gains **`CharLit(u8)`** + **`StringLit(Vec<u8>)`** carrying
the *decoded* bytes (a string IS a `[u8]` — D3), with s-expr `Display` arms
`(char N)` / `(string b0 b1 …)`. The parser **decodes the span at parse
time** (like `IntLit`): strip the quotes by byte index, process the escapes
`\n \t \r \0 \\ \' \"` and `\xHH` (two hex digits → one byte) into the bytes;
non-escape bytes (including multi-byte UTF-8) pass through verbatim, so a
string is exactly its UTF-8 source bytes. A char literal must decode to
**exactly one byte** — `''` / `'ab'` / a multi-byte source char are rejected
(`CharLitNotSingleByte`); an unknown escape or a malformed `\x` (non-hex or
< 2 digits) is rejected (`InvalidEscape`). **`u8` needed no type-parser
change** — it already parses as a `TypeExpr` `Ident`, and `[u8]` / `u8` in
param/return position are parse-confirmed by test. Resolve **rejects**
char/string literals with a `CharStringLitNotYet` `NotYet` diagnostic (the
enum/`match` (2/N) shape) — so **`ResolvedExprKind` gains no new variant** and
every downstream typed-tree crate (types/codegen/mir/borrow/effect) is
untouched; the blast radius stays in **ast + syntax + resolve**. Additive
end-to-end. +21 tests (1296), four-check green. Next: **(3/N)** the type
layer — `Type::U8` + the cascade across every exhaustive `Type` match +
`ArrayElem::U8`; char → `u8`, string → `[u8]`; `str_eq` + conversion builtins
typed; resolve → typed mirrors.

## Context

Of the remaining Phase D prerequisites (ADR 0031 D4), **strings + a byte
type** are next: every later compiler stage starts from source text. The
1.0 language has only `i64`/`i32`/`bool`/`null` literals — no way to write or
hold a byte, a character, or a string. This sub-phase adds:

  - a **`u8`** (8-bit unsigned) primitive — the byte;
  - **char/byte literals** `'a'` — a `u8` of the byte value;
  - **string literals** `"abc"` — an immutable **`[u8]`** (byte array);
  - the **core operations** a lexer needs: index a byte (`s[i]`), measure a
    length (`len(s)`), and compare byte sequences (`str_eq`).

The unifying decision is **a string is a `[u8]`** — not a new nominal type.
This is the C-/bootstrap-shaped model: a string is exactly its bytes, and it
reuses *all* of the C1.6 array machinery (codegen `{i64,ptr}`, `len`,
indexing, RAII drop, move/escape, arena routing). The only genuinely new
pieces are the `u8` scalar and the literal-lexing + literal-codegen.

## Decision

### D1. Goal.

Add a `u8` byte type, char/byte literals, and string literals (as `[u8]`),
end to end (lexer → parser → resolve → types → codegen → drop), with byte
indexing, `len`, and a byte-sequence equality builtin — enough to write a
lexer's character classification + keyword matching in Sentinel.

### D2. Surface syntax.

```sentinel
fn is_digit(c: u8) -> bool {
    c >= '0' && c <= '9'          // char literals are u8
}

fn first_byte(s: [u8]) -> u8 {
    s[0]                          // index a string = index a [u8] -> u8
}

fn main() -> i64 {
    let kw: [u8] = "let";         // string literal : [u8]
    let n = len(kw);              // 3 (reuses the array `len` builtin)
    let d = '7' - '0';            // u8 arithmetic -> 7
    n + d                         // exit 10
}
```

  - **`u8`** is a primitive type name (like `i32`), usable anywhere a type
    is: params, returns, `let` annotations, array elements (`[u8]`), struct
    /enum fields.
  - **Char literal** `'x'` is a `u8` whose value is the byte. Escapes:
    `'\n'` `'\t'` `'\r'` `'\0'` `'\\'` `'\''` (and `'\xHH'` — a hex byte).
    Single-byte only (no multi-byte/Unicode escape at MVP — D8).
  - **String literal** `"..."` has type **`[u8]`**; its bytes are the UTF-8
    encoding of the source with the same escapes plus `\"`. It behaves
    exactly like an array literal `[u8]` (owned, heap, droppable, movable).
  - **Operations:** `s[i]` (→ `u8`), `len(s)` (→ `i64`) reuse the array
    machinery; `str_eq(a, b)` (→ `bool`) is a new builtin = byte-wise array
    equality (the lexer's keyword/identifier matcher).

### D3. Representation — **a string IS a `[u8]`**.

No new `String`/`str` `Type` variant. A string literal lowers to a
`Type::Array(ArrayElem::U8)`. This is the load-bearing decision: `len`,
indexing, recursive-free drop (free the data buffer), move/use-after-move,
the returned-`[T]`-escapes path, and `[u8]`-into-a-scope-arena routing all
**already exist** for `[T]` and apply unchanged. The cost is that a
"string" is not a distinct nominal type (a `[u8]` param accepts any byte
array) and there is no growth/capacity — both intentional at MVP; a nominal,
growable `String` is the collections sub-phase (D.3, ADR 0031 D4 item 3).

`ArrayElem` (the array element subset) gains a `U8` case, and `NullableInner`
likewise iff `?u8` is wanted (deferred — D8; `u8` is added to the exhaustive
`Type` matches regardless).

### D4. The `u8` type — an integer scalar.

`Type::U8` is a new **primitive** `Type` variant (no interner table — like
`I32`/`I64`/`Bool`; the next exhaustive-`Type`-match cascade after
`Type::Enum`). It is an 8-bit **unsigned** integer:
  - lowers to LLVM `i8`; `abi-v1` size 1, align 1; mangles `u8`.
  - supports the **same operators** as other integers — arithmetic
    (`+ - * /`), comparison (`== != < <= > >=`), bitwise (`& | ^`) — via the
    existing op-generic `Binary`/`Cmp` pipeline, **no new typing surface**.
    Unsignedness affects codegen (`udiv`/unsigned compares) but not types.
  - inherits the C3.1b **secret-preserving** rules + the C5.3 bitwise rules
    unchanged (`secret u8` is representable + constant-time-able).
  - mixed-width arithmetic stays a type error (no implicit `u8`↔`i64`), as
    today between `i32`/`i64`; an explicit conversion builtin
    (`u8_to_i64` / `i64_to_u8`) is provided since the lexer must turn a
    digit byte into an integer and index with it (D5).

### D5. Operations + conversions (MVP).

  - **`s[i]`** — array indexing, `[u8]` → `u8` (reuses C1.6 bounds-checked
    `Index`; the index is `i64`).
  - **`len(s)`** — reuses the C1.6 `len` builtin (`[T]` → `i64`).
  - **`str_eq(a: [u8], b: [u8]) -> bool`** — a new runtime builtin: equal
    length + byte-wise equality. The lexer matches keywords/identifiers with
    it. (Extending `==` to arrays is out of scope — a builtin keeps the
    operator surface unchanged.)
  - **`u8_to_i64(b: u8) -> i64`** / **`i64_to_u8(n: i64) -> u8`** — explicit
    width conversions (zero-extend / truncate). Needed because mixed-width
    arithmetic is rejected (D4) but a lexer must do `digit = u8_to_i64(c) -
    u8_to_i64('0')` and index buffers with `i64`.
  - **Concatenation / substring / slicing — DEFERRED** (D8): they need an
    allocation/growth strategy and fold into the collections + stdlib
    sub-phases. The lexer works on a fixed source `[u8]` with indices, so it
    does not need them at MVP.

### D6. Codegen — literals + drop + arena.

  - **`u8`**: `i8` everywhere; `udiv` + unsigned `icmp` predicates.
  - **String literal `"abc"`**: emit a private **global `[N x i8]` constant**
    of the bytes, then at the use site **heap-copy** it (`sentinel_alloc(N)`
    + a byte copy) and build `{ len = N, ptr = copy }`. Copy-to-heap makes a
    string-literal `[u8]` **owned + uniformly droppable/movable** like any
    array literal — the global is only the initializer; nothing aliases it,
    so the existing array drop (`sentinel_free(data)`) is correct and there
    is **no global-free hazard**. (An interned/non-owning string-literal
    view is a measured post-MVP optimisation; copy-to-heap is the simple,
    uniform MVP.)
  - **Char literal `'a'`**: an `i8` constant — no allocation.
  - **Drop / move / escape / arena**: entirely the existing `[T]` paths. A
    non-escaping string-literal buffer is `compute_arena_routed`-eligible
    with no change (ADR 0028).
  - **`str_eq`** lowers to a runtime `sentinel_str_eq(ptr,len,ptr,len) ->
    i1` (or an inline length-compare + `memcmp`); the conversions are
    `zext`/`trunc`.

### D7. Pipeline / touch points + sub-phase split.

| Sub        | Title                                                          | Risk   |
|------------|----------------------------------------------------------------|--------|
| D.2 (1/N)  | lexer — char literals `'…'`; string literals `"…"` (+         | low    |
|            | escapes). `u8` lexes as an `Ident` (a type name, like         |        |
|            | `i64`) — **no keyword token**. Additive (parser consumes 2/N). |        |
| D.2 (2/N)  | AST + parser — `ExprKind::CharLit(u8)` + `StringLit(Vec<u8>)`; | medium |
|            | `u8` in `TypeExpr`. Resolve pass-through; types reject (NotYet)|        |
|            | until 3/N.                                                     |        |
| D.2 (3/N)  | types — `Type::U8` (the cascade across every exhaustive `Type` | medium |
|            | match + `ArrayElem::U8`); char-lit → `u8`; string-lit → `[u8]`;|        |
|            | `str_eq` + conversion builtins typed; secret/bitwise inherit.  |        |
| D.2 (4/N)  | codegen + runtime — `i8`; string-literal global + heap copy;   | high   |
|            | `sentinel_str_eq`; conversions; `abi-v1` `u8` entry + tests +  |        |
|            | the `c5d2_strings` phase-go. ADR flip.                         |        |

The `Type::U8` cascade (every exhaustive `Type` match across types / codegen
/ borrow-check / mir) is the same coordinated-arms discipline as the
`Type::Enum` cascade (ADR 0032 (3/N)); `u8` is a non-secret-relevant scalar,
so most arms group it with `I32`/`I64`.

### D8. Out of scope (MVP).

A nominal, growable, owned `String` type (collections / stdlib — D.3); UTF-8
validation + Unicode code points / `char`-as-scalar-value (bytes only —
`'…'` is one **byte**, multi-byte source chars in a literal are their UTF-8
bytes, no `\u{…}`); string concatenation / substring / slicing /
formatting / interpolation; `?u8` / `[u8]` mutation (`s[i] = b` — arrays are
immutable per ADR 0017 D12); extending `==` to byte arrays (use `str_eq`);
`i8` (signed byte — only the unsigned `u8` is added). `secret u8` IS
representable (inherits secret typing) but no `[secret u8]` (the deferred
secret-array surface, ADR 0027 D8).

### D9. Phase-go + fixture.

`tests/pass/c5d2_strings.sentinel`: classify + compare bytes — e.g. a
keyword matcher (`str_eq(src_slice, "let")`) + a digit-value computation
(`u8_to_i64(c) - u8_to_i64('0')`) over a source `[u8]` literal, returning a
computed exit code; verified leak-free via `leaks --atExit` (the literal
heap-copies are array-dropped). Plus a `u8` arithmetic/comparison unit
corpus + a UI fixture (e.g. mixed-width `u8 + i64` → `Mismatch`).

## Reasoning

**Why a string is `[u8]` (not a nominal type).** The bootstrap compiler
needs to *hold source bytes and index/compare them*, not a rich Unicode
string abstraction. `[u8]` delivers exactly that and inherits every array
guarantee already built + tested (drop, move, bounds checks, arena routing,
`abi-v1` layout) — the lowest-risk, fastest path. A nominal growable
`String` (capacity, push, concat) is genuinely different machinery and
belongs with growable collections (D.3), where `Vec<u8>` and `String` share
a design.

**Why copy string literals to the heap.** Uniformity beats cleverness at
MVP: a heap-copied literal is indistinguishable from any other `[u8]`, so
drop/move/escape/arena "just work" with zero new analysis and zero
global-free hazard. The wasted copy per literal use is negligible for a
compiler that reads its input once, and a non-owning interned view is a
clean later optimisation gated on need.

**Why `u8` is a full integer scalar.** A lexer does byte arithmetic
(`c - '0'`) and comparison (`'0' <= c <= '9'`) constantly. Making `u8` a
first-class integer (reusing the op-generic `Binary`/`Cmp`/bitwise pipeline)
costs almost nothing in the type layer and makes lexer code natural. Keeping
it *unsigned* matches byte semantics; rejecting implicit width mixing (with
explicit conversions) matches the existing `i32`/`i64` discipline.

## Consequences

### Positive
- The compiler's input type lands — source text is now expressible — with
  maximal reuse of the proven array machinery (low novelty risk).
- `u8` + bitwise + secret typing compose for free (constant-time byte ops).

### Negative
- A real codegen + runtime addition ((4/N): string-literal globals + heap
  copy + `str_eq`); gated behind the type layer + differential fixtures.
- Another `Type`-variant cascade (the `u8` arms across every exhaustive
  match) — mechanical but wide (the ADR 0032 lesson: land the arms together,
  build, then semantics, then tests).

### Neutral
- No effect on existing programs (additive; `repro.rs` byte-identical for
  pre-D.2 fixtures).

## Revisit

PROPOSED until D.2 closes. Triggers:
- **D3**: if the self-hosted lexer/parser proves `[u8]` too low-level
  (constant slicing/concat churn), bring a nominal growable `String`
  forward from D.3.
- **D6**: if literal heap-copy overhead matters, add the interned
  non-owning string-literal view.
- **D5**: confirm the conversion-builtin surface once real lexer code is
  written against it.
