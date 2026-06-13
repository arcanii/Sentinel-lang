# Programming in Sentinel

This guide is a tour of the Sentinel language **as it exists today**, written
for someone who wants to read and write Sentinel programs with the bootstrap
compiler (`snc`). Every code block below is real, current syntax — the
examples are drawn from (or verified against) the `tests/pass/` fixtures that
CI compiles and runs on every change.

Sentinel is a memory-safe, capability-bounded systems language. Its
distinctive features — move-checked references with RAII, a `secret` qualifier
with a constant-time check, algebraic effects, classes/traits/delegation, and
structured concurrency — are all covered here. What it is *not* yet (a
production toolchain, a standard library, multi-process) is covered at the end.

> **If this guide disagrees with the compiler, the compiler wins** — and if it
> disagrees with [`STATE.md`](STATE.md), STATE.md wins. Report any discrepancy
> as a docs bug (see [`CONTRIBUTING.md`](../CONTRIBUTING.md)). The language is
> still evolving; treat this as a snapshot, not a spec.

## Contents

- [Setup](#setup)
- [Your first program](#your-first-program)
- [Values, types, and `let`](#values-types-and-let)
- [Operators](#operators)
- [Functions](#functions)
- [Control flow: `if` and `while`](#control-flow-if-and-while)
- [Structs](#structs)
- [Enums and `match`](#enums-and-match)
- [References, moves, and the borrow checker](#references-moves-and-the-borrow-checker)
- [Strings, bytes, and `Vec`](#strings-bytes-and-vec)
- [File I/O](#file-io)
- [Generics](#generics)
- [Nullable `?T`](#nullable-t)
- [`secret` and constant-time](#secret-and-constant-time)
- [Effects and handlers](#effects-and-handlers)
- [Classes, traits, and delegation](#classes-traits-and-delegation)
- [Structured concurrency](#structured-concurrency)
- [Modules and multiple files](#modules-and-multiple-files)
- [A worked example](#a-worked-example)
- [What isn't here yet](#what-isnt-here-yet)
- [Appendix: the C0 language (historical)](#appendix-the-c0-language-historical)

## Setup

You need Rust (stable) and **LLVM 18**. The primary, CI-tested platform is
macOS on Apple Silicon; Linux is possible with `llvm-18` installed and
`LLVM_SYS_180_PREFIX` exported, but is not yet CI-verified.

```bash
brew install llvm@18
echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc
git clone https://github.com/arcanii/Sentinel-lang.git
cd Sentinel-lang
cargo build --workspace
```

The compiler driver lands at `target/debug/snc`. Its main subcommands:

| Command                            | What it does                                              |
| ---------------------------------- | --------------------------------------------------------- |
| `snc build <file> [-o <output>]`   | Compile + link to a native executable (default name = the file stem). |
| `snc parse <file>`                 | Lex, parse, and pretty-print the program.                 |
| `snc ast <file>` / `snc lex <file>`| Dump the canonical AST / token stream (used as self-host oracles). |

A Sentinel source file uses the `.sentinel` extension. Run `snc help` for the
full list.

## Your first program

The smallest valid program is a `main` function returning `i64`:

```sentinel
// hello.sentinel
fn main() -> i64 {
    0
}
```

```bash
snc build hello.sentinel -o hello
./hello; echo "exit=$?"     # exit=0
```

**Exit codes are the answer.** A program's exit code is the value of `main`'s
tail expression, truncated to the C ABI's 32-bit `int`. So `fn main() -> i64 {
42 }` exits with code 42. This "the exit code is the result" convention is what
the test corpus asserts on, and it's the easiest way to see a program's value
without a print.

To write to stdout, use the built-in `print`, which takes an `i64` and writes
it as decimal followed by a newline (and returns 0):

```sentinel
fn main() -> i64 {
    print(42)               // writes "42\n"; print returns 0, so main exits 0
}
```

## Values, types, and `let`

Sentinel is statically typed, and **type annotations are required where a type
can't be inferred** — function parameters and return types always carry them.
The scalar types are:

| Type   | Meaning                                  |
| ------ | ---------------------------------------- |
| `i64`  | 64-bit signed integer (the default int). |
| `i32`  | 32-bit signed integer.                   |
| `u8`   | unsigned byte (also the element of strings — see below). |
| `bool` | `true` / `false`.                        |

`let` introduces a binding. Add `mut` to make it reassignable; an annotation is
optional when the initializer's type is obvious, and required otherwise (e.g.
`vec_new()`, `null`, a `secret`/nullable widen):

```sentinel
fn main() -> i64 {
    let x: i64 = 5;         // annotated
    let y = x + 3;          // inferred as i64
    let mut acc: i64 = 0;   // mutable
    acc = acc + y;          // assignment (no `let`)
    acc = acc + 1;
    acc                     // 9
}
```

Bindings are block-scoped and live until the end of the enclosing block.

## Operators

Arithmetic, comparison, logical, and bitwise operators are all present:

| Group       | Operators                          | Result    |
| ----------- | ---------------------------------- | --------- |
| arithmetic  | `+` `-` `*` `/` and unary `-`      | numeric   |
| comparison  | `==` `!=` `<` `<=` `>` `>=`        | `bool`    |
| logical     | `&&` `\|\|` `!`                    | `bool`    |
| bitwise     | `&` `\|` `^`                       | integer   |

`/` is integer division. `&&` and `||` short-circuit — the right operand is not
evaluated if the left already decides the result:

```sentinel
// `print(99)` never runs: `false &&` short-circuits. stdout stays empty.
fn main() -> i64 {
    if false && print(99) > 0 { 1 } else { 7 }   // exit 7
}
```

Precedence is conventional (unary tightest, then `* /`, then `+ -`, then
comparisons, then `&& ||`); parentheses override it.

## Functions

A function is `fn name(params) -> ReturnType { body }`. The body is a block, and
its tail expression is the return value. Parameter and return types are
mandatory:

```sentinel
fn double(x: i64) -> i64 {
    x * 2
}

fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn main() -> i64 {
    print(double(7));       // 14
    add(3, 4)               // exit 7
}
```

A program is one or more top-level `fn` definitions; exactly one must be named
`main`. Functions may call each other in either direction — the compiler
resolves signatures before bodies, so forward references are fine. `print` is a
reserved built-in; you can't redefine it.

## Control flow: `if` and `while`

`if` is an **expression**: the chosen branch's tail is the value of the whole
`if`. The condition must be a `bool`, and **`else` is mandatory** (both branches
must produce a value):

```sentinel
fn main() -> i64 {
    if 5 > 3 { 12 } else { 0 }    // exit 12
}
```

`while` is a **statement** (a loop has no value). `break` and `continue` work
inside it. Loop-carried state is a `let mut` declared outside the loop:

```sentinel
fn main() -> i64 {
    let mut total: i64 = 0;
    let mut i: i64 = 1;
    while i <= 10 {
        total = total + i;
        i = i + 1;
    }
    total                          // 1+2+...+10 = 55
}
```

A binding declared *inside* the loop body is fresh each iteration (and its heap,
if any, is freed each iteration — see RAII below).

## Structs

A `struct` groups named fields. Construct one with `Name { field: value }` and
read a field with `value.field`:

```sentinel
struct Point { x: i64, y: i64 }

fn manhattan(p: Point) -> i64 {
    p.x + p.y
}

fn main() -> i64 {
    let p: Point = Point { x: 3, y: 4 };
    manhattan(p)                   // exit 7
}
```

Fields can be any type, including other structs and the nullable/collection
types below.

## Enums and `match`

An `enum` is a sum type: a value is exactly one of its variants, and a variant
may carry a positional payload. Construct with `Enum::Variant(...)` and inspect
with `match`, which **must be exhaustive**:

```sentinel
enum Shape {
    Unit,
    Circle(i64),
    Rect(i64, i64),
}

fn area(s: Shape) -> i64 {
    match s {
        Shape::Unit => 0,
        Shape::Circle(r) => r * r * 3,
        Shape::Rect(w, h) => w * h,
    }
}

fn main() -> i64 {
    let c = Shape::Circle(2);      // area 12
    let r = Shape::Rect(5, 6);     // area 30
    area(Shape::Unit) + area(c) + area(r)   // 0 + 12 + 30 = 42
}
```

A `match` arm binds the variant's payload (`r`, `w`, `h` above). The scrutinee
is consumed by the match (it's moved in).

## References, moves, and the borrow checker

Sentinel has ownership, move semantics, references, and RAII drop — the C2
surface.

**References.** `&x` is a shared (read-only) borrow; `&mut x` is a mutable
borrow; `*r` dereferences. A function takes them as `&T` / `&mut T`:

```sentinel
fn add(a: &i64, b: &i64) -> i64 {
    *a + *b
}

fn main() -> i64 {
    let a: i64 = 10;
    let b: i64 = 32;
    add(&a, &b)                    // exit 42
}
```

The borrow checker enforces **shared-XOR-mutable**: at any time a value has
either any number of `&` borrows or exactly one `&mut`, never both.

**Moves.** Non-`Copy` values (structs, arrays, `Vec`, `String`, enums) are
*moved* when passed by value or returned. After a move the original binding is
no longer usable — using it is a compile error. Scalars (`i64`, `bool`, …) and
references are `Copy` (they're duplicated, not moved):

```sentinel
struct Point { x: i64, y: i64 }
fn consume(p: Point) -> i64 { p.x + p.y }

fn main() -> i64 {
    let p: Point = Point { x: 3, y: 4 };
    consume(p)                     // `p` is moved here; it can't be used again
}
```

**RAII drop.** A value that owns heap memory (an array, a `Vec`, a heap-boxed
enum payload, a struct/class containing one) is freed automatically at the end
of its scope — no manual `free`. A value that was moved out is *not* dropped by
the original scope (the new owner drops it), so there's no double-free.

**Known limitations.** The borrow checker is lexical (pre-Polonius), so it is
*conservative*: it sometimes rejects a program that is actually safe — for
example, a borrow whose last use was on the previous line, or a borrow of one
struct field that blocks a write to a different field. Each such case has a
documented workaround in
[`borrow-check-limitations.md`](borrow-check-limitations.md). These are
over-rejections (it errs on the side of safety); the one historical
*under*-rejection (a moved struct field could double-free) is closed.

## Strings, bytes, and `Vec`

**A string is its bytes.** A string literal `"hi"` has type `[u8]` — an array
of unsigned bytes. A character literal `'h'` is a `u8`. `u8` arithmetic is
*unsigned* (a byte ≥ `0x80` is large, not negative). Convert with
`i64_to_u8` / `u8_to_i64`, compare two `[u8]` with `str_eq`, take a length with
`len`, and index with `s[i]` (yielding a `u8`):

```sentinel
fn main() -> i64 {
    let big: u8 = i64_to_u8(200);
    let small: u8 = i64_to_u8(100);
    let q: u8 = big / small;       // unsigned division -> 2
    if big > small && u8_to_i64(q) == 2 { 42 } else { 0 }
}
```

**`Vec<T>` is a growable array.** Create one with `vec_new()` (the element type
comes from the annotation), append with `push(&mut v, x)`, read with `v[i]`,
remove the last with `pop(&mut v)`, measure with `len(v)`, and bridge a
`Vec<T>` to a `[T]` with `vec_to_array(v)`. **`String` is an alias for
`Vec<u8>`** — a growable byte buffer:

```sentinel
fn main() -> i64 {
    let mut nums: Vec<i64> = vec_new();
    push(&mut nums, 10);
    push(&mut nums, 20);
    push(&mut nums, 30);
    let first: i64 = nums[0];        // 10
    let last: i64 = pop(&mut nums);  // 30 (len now 2)

    let mut word: String = vec_new();
    push(&mut word, 'l');
    push(&mut word, 'e');
    push(&mut word, 't');
    let arr: [u8] = vec_to_array(word);   // bridge Vec<u8> -> [u8]
    let kw: [u8] = "let";
    let matched: i64 = if str_eq(arr, kw) { 1 } else { 0 };

    first + last + len(nums) + matched   // 10 + 30 + 2 + 1 = 43
}
```

Every `Vec` / array / string is freed at its owning scope's exit (RAII), so
these programs are leak-free.

## File I/O

File I/O is a set of runtime built-ins (not effects). `write_file(path, bytes)`
writes, `read_file(path) -> [u8]` reads the whole file, and `print_bytes(bytes)`
writes the exact bytes to stdout (no trailing newline). Paths and contents are
both `[u8]`. A failed open/read/write aborts the program (panic-on-failure):

```sentinel
fn main() -> i64 {
    let path: [u8] = "/tmp/sentinel_demo.txt";
    let payload: [u8] = "hello";

    write_file(path, payload);
    let back: [u8] = read_file(path);
    print_bytes(back);             // writes "hello" (no newline)

    if str_eq(back, payload) { len(back) } else { 0 }   // exit 5
}
```

## Generics

Functions and structs can be generic over type parameters, written `<T>`. The
compiler **monomorphizes** — it emits a specialized copy per concrete type used:

```sentinel
fn id<T>(x: T) -> T { x }

fn main() -> i64 {
    id(42)                         // monomorphized to id__i64; exit 42
}
```

A generic struct is `struct Box<T> { value: T }`, instantiated as `Box<i64>`.
Type arguments are inferred at call sites where possible.

## Nullable `?T`

`?T` is a nullable type — a `T` or `null`. A plain `T` widens implicitly to
`?T` at an annotation boundary, and `null` is the absent value. Inspect with
`is_some(x)` (true if present), `unwrap_or(x, default)` (the value or a
fallback), or compare `x == null`:

```sentinel
fn main() -> i64 {
    let x: ?i64 = 42;              // implicit i64 -> ?i64 widen
    unwrap_or(x, 0)                // exit 42
}
```

`?T` is how Sentinel models "maybe absent" without a null-pointer footgun —
you can't use a `?T` as a `T` without going through `unwrap_or` / a check.

## `secret` and constant-time

`secret T` marks a value whose *timing* must not leak. The compiler statically
**rejects** any program where a `secret` value reaches a branch condition, a
memory index/address, or a division divisor — the three classic timing
side-channels. `declassify(s)` is the one sanctioned way to turn a `secret T`
back into a `T` (you're asserting it's safe to branch on now):

```sentinel
fn unwrap(s: secret i64) -> i64 {
    declassify(s)
}

fn main() -> i64 {
    let stored: secret i64 = 42;   // implicit i64 -> secret i64 widen
    let raw: i64 = unwrap(stored);
    print(raw + 8)                 // "50"
}
```

A *branch-free* computation over secrets — using the arithmetic/bitwise
operators, never an `if` on a secret — passes the check. The canonical shape is
constant-time equality: XOR the pairs, OR-reduce, and only `declassify` the
final accumulator. Writing `if (someSecretBool) { ... }`, indexing
`a[someSecret]`, or dividing by a secret is a compile error
(`sentinel::mir::secret_leak`).

**What the guarantee is, precisely.** The check is machine-checked on the
compiler's MIR, with the *type system* as the taint oracle (a value is secret
iff its type says so), and it runs *before* LLVM optimization — so it constrains
the program you wrote, not the optimized machine code. It does not yet *force*
constant-time emission (cmov / speculation barriers / post-codegen assembly
verification are future work). See the README's "headline capability" section
for the full framing.

## Effects and handlers

Sentinel has algebraic effects with deep handlers. An `effect` declares
operations; `perform` invokes one; `handle <body> with { ... }` interprets the
operations the body performs. A handler arm receives a continuation `k` and may
resume it with a value:

```sentinel
effect Io {
    read() -> i64;
}

fn main() -> i64 {
    handle perform Io.read() with {
        Io.read(k) => k(42)        // resume the continuation with 42
    }                              // exit 42
}
```

Effects are part of a function's type: a function that performs an unhandled
effect must declare it in its signature with `! { EffectName }` (you'll see this
on `Async` in the concurrency section). An unhandled effect at `main` is a
compile error — effects are tracked, not implicit.

## Classes, traits, and delegation

A `class` bundles fields with an `init` constructor and methods. `self: &Self`
is a shared receiver, `self: &mut Self` a mutating one. Construct with
`Class::init(...)` and call methods with `instance.method(...)`:

```sentinel
class Point {
    let x: i64;
    pub init(x: i64) {
        self.x = x;
        0
    }
    pub fn get(self: &Self) -> i64 {
        self.x
    }
}

fn main() -> i64 {
    let p: Point = Point::init(7);
    p.get()                        // exit 7
}
```

A `trait` declares method signatures; `impl as Trait for Class { ... }` provides
them, and a call dispatches to the impl:

```sentinel
trait Counter {
    fn tick(self: &mut Self, n: i64) -> i64;
}

class Tally {
    let count: i64;
    pub init() { self.count = 0; 0 }
}

impl as Counter for Tally {
    fn tick(self: &mut Self, n: i64) -> i64 {
        self.count = self.count + n;
        self.count
    }
}

fn main() -> i64 {
    let mut t: Tally = Tally::init();
    t.tick(42)                     // exit 42
}
```

**Delegation** auto-forwards a trait to an inner field. `delegate field: Type to
Trait;` makes the compiler synthesize an `impl as Trait` that routes each method
to `self.field.method(...)` — composition without boilerplate:

```sentinel
class Logger {
    delegate writer: FileSink to Writer;  // FileSink impls Writer
    pub init(w: FileSink) { self.writer = w; 0 }
}
// l.write(42) now routes to l.writer.write(42)
```

## Structured concurrency

`scope concurrent { ... }` opens a concurrency scope; `spawn f(args)` starts a
task (an OS thread at the current minimum), and `task.await` joins it and reads
its result. The scope discharges the `Async` effect, so concurrency stays
visible in the type system but `main` doesn't have to declare it:

```sentinel
fn double(x: i64) -> i64 ! { Async } {
    x * 2
}

fn main() -> i64 {
    let result: i64 = scope concurrent {
        let t = spawn double(21);  // t : Task<i64>
        t.await                    // join + read -> 42
    };
    result                         // exit 42
}
```

A spawned function carries the `Async` effect (`! { Async }`); the `scope`
handles it. Tasks must be awaited within their scope.

## Modules and multiple files

Each file is a module, named by its path relative to the source root (the entry
file's directory). `pub` marks an item as importable; `use path::Item;` brings
it into scope. Given `util.sentinel`:

```sentinel
// util.sentinel
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}
```

and `app.sentinel` next to it:

```sentinel
// app.sentinel
use util::add;

fn main() -> i64 {
    add(40, 2)                     // exit 42
}
```

build the **entry** file — the compiler follows the `use` edges and pulls in the
modules it needs:

```bash
snc build app.sentinel -o app
./app; echo "exit=$?"             # exit=42
```

`pub` applies to any top-level item (fn, struct, enum, trait, class, effect); a
non-`pub` item is private to its module. (The compiler currently merges the
module graph into one unit before codegen; true per-unit separate compilation is
a deferred follow-on — it doesn't change the source-level model above.)

## A worked example

Two fixtures are worth reading in full as end-to-end tours:

- [`tests/pass/c44_go_no_go.sentinel`](../tests/pass/c44_go_no_go.sentinel) —
  the structured-concurrency example above, complete.
- [`tests/pass/c5_go_no_go.sentinel`](../tests/pass/c5_go_no_go.sentinel) — the
  **1.0 acceptance program**: a TLS-1.3-handshake-*shaped* program combining a
  state-machine class, a cipher-suite trait, I/O-as-effects, and a constant-time
  `Finished`-MAC verify — compiled, run, and passing the constant-time check
  end to end.

More broadly, the ~120 fixtures under [`tests/pass/`](../tests/pass/) are each a
small, CI-verified program; reading them by feature prefix (`c14_*` structs,
`c5d1_*` enums, `c35*_*` effects, `c41_*`–`c44_*` classes/concurrency, …) is the
most reliable tour of the surface, because they are exactly what the compiler is
tested against.

## What isn't here yet

Sentinel 1.0 was the *bootstrap-compiler* milestone, and Phase D has since grown
the language to self-host. What remains pending:

- **Not production-ready, not stable.** Every API can change; only the
  `abi-v1` *compiled-artifact* contract is frozen. There is no package manager
  and **no standard library** beyond the handful of built-ins shown here
  (`print`, `len`, `push`, `pop`, `vec_new`, `vec_to_array`, `str_eq`,
  `read_file`, `write_file`, `print_bytes`, `i64_to_u8`, `u8_to_i64`,
  `is_some`, `unwrap_or`, `declassify`).
- **Single-process.** Cross-process capabilities and actors are deferred.
- **The borrow checker over-rejects** some safe programs (see the limitations
  doc); the Polonius-style flow-precise migration is future work.
- **Constant-time *emission* is not forced** — only the MIR-level rejection is
  delivered (see the `secret` section).
- **No floats**, no integer widths beyond `i64`/`i32`/`u8`, no tuples, no
  closures, no `for`/iterators, no block comments (`//` line comments only).
- **Tooling is minimal** — the LSP is a stub; there's no formatter or REPL.

When something you expect to work doesn't, check [`STATE.md`](STATE.md) (the
authoritative feature list) before assuming it's a bug — then, if it really
looks like one, see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Appendix: the C0 language (historical)

The very first bootstrap milestone (**Phase C0**) had exactly one type — `i64` —
and no annotations: `fn double(x) { x * 2 }`. Conditions were C-style truthy
(`0` false, nonzero true), there were no comparison or logical operators, and
`print` was the only built-in. That language is gone — annotations are now
mandatory, conditions are strictly `bool`, and the type system, references,
secrets, effects, generics, classes, collections, and modules described above
all arrived in Phases C1–C5 and D. The C0-era `fn f(x)` examples **no longer
compile**; they're noted here only so old snippets don't confuse you.

If you want the bigger picture of what Sentinel is *for* — the security thesis
behind `secret`, effects-as-capabilities, and the broker — read
[`SENTINEL_SUMMARY.md`](SENTINEL_SUMMARY.md) and
[`SENTINEL_DESIGN2.md`](SENTINEL_DESIGN2.md). [`STATE.md`](STATE.md) tracks what
is actually built, updated at every sub-phase boundary.
