# `std/` — Sentinel core libraries

Real, idiomatic Sentinel library modules — the building blocks a programmer
gets instead of starting from scratch. They are written in Sentinel itself, and
they double as feature tests: every module is exercised by a program under
[`../examples/`](../examples/) that is compiled, run, and asserted on by
`cargo nextest` (see [the harness](../crates/sentinel-driver/tests/examples.rs)).

## Layout: functional categories

`std/` is subdivided by **functional category** (each a directory), and a module
is a `.sentinel` file inside one. The module's path is its file path relative to
this corpus root:

```
std/
  security/ct.sentinel   →  module  std::security::ct   (constant-time primitives)
  math/num.sentinel       →  module  std::math::num      (min/max/abs/clamp)
  ...
```

A program imports an item with `use std::<category>::<module>::<Item>;`. Module
discovery roots at the entry file's parent directory and maps
`use a::b::Item` → `<root>/a/b.sentinel` (no parent traversal), so the test
harness assembles a buildable project by dropping this whole `std/` tree next to
the example entry. See the harness doc-comment for the mechanics.

## Categories (current + planned)

| Category    | Module(s)            | Status                                   |
| ----------- | -------------------- | ---------------------------------------- |
| `security`  | `ct`, `siphash`      | ✅ ct scalars + `ct_memcmp` over `[secret u8]` + `ct_rotl64`/`ct_rotl32`; `siphash24` keyed MAC |
| `math`      | `num`                | ✅ min/max/abs/clamp                       |
| `bits`      | `bits`               | ✅ `rotl64`/`rotr64`/`rotl32`/`rotr32`     |
| `bytes`     | `bytes`              | ✅ `eq`/`find`/`contains`/`count`/`starts_with`/`repeat` (over `&[u8]`) |

The list grows as examples force new building blocks.

## The point: find + fix language gaps

A real, idiomatic library hits the language's missing pieces, and finding them is
the most valuable output of this corpus.

- **Fixed — `[secret T]` arrays** (ADR 0047). `ArrayElem` gained a secret form, so
  a variable-length constant-time `memcmp` over a buffer of *secret* bytes
  (`security/ct::ct_memcmp` over `[secret u8]`) is now expressible. Surfaced +
  closed by this corpus (a front-end-only, byte-identical change).
- **Fixed — shift operators `<<` / `>>`** (ADR 0048). Logical right shift, with a
  constant-time rule (a shift by a *secret* amount is rejected like a secret
  divisor; a secret value shifted by a public amount is constant-time). Unblocks
  the `bits` rotate library and the SipHash-style ARX round
  (`examples/security/siphash_round`). (Phase 1 lands shifts in `snc`; the
  self-hosted mirror is Phase 2.)
- **Fixed — the integer cast `x as T`** (ADR 0049). Closes the "i32 values are
  unconstructible" gap (an integer literal is `i64`): `x as i32` truncates / `x as
  i64` sign-extends, preserving secrecy and constant-time (no CT sink). Unblocks a
  true 32-bit **ChaCha quarter-round** over `secret i32` words
  (`examples/security/chacha_qr`, RFC 8439 vector) and makes the `bits` 32-bit
  rotates runnable.
- **Fixed — mutable index assignment `a[i] = v`** (ADR 0050). Lifts the ADR 0017
  D12 deferral so an array / `Vec` element can be written in place through a public
  index (the read path's bounds-checked element GEP + a store). Constant-time is
  preserved with no new sink — a *secret* LHS index is rejected like a secret
  read-index, while a *secret value* stored at a public index is allowed. Unblocks
  a full, idiomatic **ChaCha20 block** that permutes a `[secret i32]` state in place
  (`examples/security/chacha20_block`, RFC 8439 §2.3.2 vector).

All four surfaced gaps are now closed and fully self-hosted (snc + `scg`,
byte-identical, both bootstrap fixed points held). When a gap blocks a genuinely
idiomatic library, that block is the signal to add the feature (ADR-first if it
touches the frozen `abi-v1` contract).
