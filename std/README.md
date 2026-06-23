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
| `security`  | `ct`                 | ✅ constant-time scalar primitives        |
| `math`      | `num`                | ◻ min/max/abs/clamp (next)               |
| `bits`      | `bits`               | ◻ rotate/shift — **blocked on shift ops**|
| `bytes`     | `bytes`              | ◻ `[u8]`/string utils                    |

The list grows as examples force new building blocks.

## The point: find + fix language gaps

A real, idiomatic library hits the language's missing pieces, and finding them is
the most valuable output of this corpus. Two are already tracked:

- **No shift operators** (`<<` / `>>`). Blocks a `bits` rotate library and the
  textbook constant-time primitives that broadcast a sign bit (`x >> (W-1)`).
- **No `[secret T]` arrays.** `ArrayElem` has no secret form, so a
  variable-length constant-time `memcmp` over an array of *secret* bytes is not
  yet expressible — fixed-width compares over `secret` *scalars* are (see
  `security/ct`). This is the flagship's next target.

When a gap blocks a genuinely idiomatic library, that block is the signal to add
the feature (ADR-first if it touches the frozen `abi-v1` contract).
