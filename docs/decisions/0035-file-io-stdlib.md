# ADR 0035: Phase D.4 — file I/O via a minimal stdlib (`read_file` / `write_file`)

Status: PROPOSED — the fourth Phase D sub-phase ADR under ADR 0031 (Phase D
kickoff) D4 item 4. After sum types (D.1 / ADR 0032), strings + a byte type
(D.2 / ADR 0033), and growable collections (D.3 / ADR 0034), **file I/O** is
next: a self-hosting compiler must **read its source file** and **write its
output artifact**, and the 1.0 + D.1–D.3 language has no I/O beyond
`sentinel_print` (one i64 to stdout). Flips to ACCEPTED(-WITH-AMENDMENTS) as the
sub-phases land.

Date: 2026-05-31
Related:
  - **0031** (Phase D kickoff): D4 item 4 names "file I/O via a minimal stdlib —
    read source / write artifacts, modelled as effects + handlers over real OS
    syscalls in the runtime." **This ADR amends that framing** (see D2 / the
    Reasoning): the I/O *operations* are **runtime builtins** (like `print`),
    not algebraic-effect ops — the effect/handler machinery (ADR 0020) is for
    *resumable user computations*, which OS side effects are not.
  - **0015** (arrays): `read_file -> [u8]` reuses the `{ i64 len, ptr data }`
    array — a file's bytes are exactly a sized `[u8]`, heap-owned + dropped via
    the existing array machinery (no growth needed: the size is known at read).
  - **0033** (strings + byte type): a path / file content **IS a `[u8]`**. The
    surface is byte-oriented (paths and bytes), matching the "a string is its
    bytes" lever; UTF-8 / `String` ergonomics layer on later.
  - **0020** (handler runtime + perform lowering): the algebraic-effect surface
    (`effect` / `handle` / `perform` / resumable `k`). D2 explains why I/O does
    **not** use it. The *effect-row* system (`! { Io }` purity tracking) is a
    separate, orthogonal concern, deferred (D8).
  - **0029** (stable abi-v1): the new `sentinel_*` syscall-wrapper symbols join
    the §5 runtime-symbol contract + the `abi_v1_runtime_symbol_set` test.

## Context

Of the remaining Phase D prerequisites (ADR 0031 D4), **file I/O** is next. The
1.0 + D.1–D.3 language can compute over text (strings = `[u8]`, growable
`Vec<u8>`) but cannot **acquire** it: the only runtime I/O symbol is
`sentinel_print(i64) -> i64`. A self-hosted lexer's input is a source file; a
self-hosted compiler's output is an artifact (an LLVM-IR `.ll` text, or later an
object). Both are file operations the surface + stdlib do not yet express.

This sub-phase adds a **minimal, byte-oriented file-I/O stdlib**: read a whole
file into a `[u8]`, write a `[u8]` to a file, and (for diagnostics) write a
`[u8]` to stdout. The operations are **runtime builtins** backed by libc
`fopen`/`fread`/`fwrite`/`fclose` (the same "emitted programs link the C
runtime" substrate as `sentinel_alloc`/`free`/`realloc`), joining the `abi-v1`
symbol contract.

The unifying decision is **file I/O is runtime builtins, not algebraic
effects** — `read_file` / `write_file` / `print_bytes` are `sentinel_*` runtime
functions in the `print` mould, typed as builtins (the ADR 0014/0033/0034
builtin-signature machinery), reusing the `[u8]` array representation for both
paths and content. The genuinely new pieces are the **runtime syscall wrappers**
+ their `abi-v1` entries + the builtin signatures/codegen dispatch.

## Decision

### D1. Goal.

Add a minimal file-I/O stdlib, end to end (types → codegen → runtime), with
**`read_file(path) -> [u8]`** (read a whole file), **`write_file(path, data) ->
i64`** (write/overwrite a file), and **`print_bytes(data) -> i64`** (write a
`[u8]` to stdout) — enough for a self-hosted lexer to read its source and a
self-hosted compiler to emit a text artifact + diagnostics.

### D2. File I/O is runtime builtins, NOT algebraic effects.

ADR 0031 D4 sketched "effects + handlers over real OS syscalls." That framing is
**amended here**: the algebraic-effect machinery (ADR 0020 — `effect` / `handle`
/ `perform` with a resumable continuation `k`) models *resumable user
computations* (a handler decides how to resume; the c35/c36 fixtures resume with
computed values). OS I/O is **irreversible side effects**, not resumable
computations — and the effect system forbids an effectful `main` (effects must
be discharged by a user `handle`), so an `Io` effect would force every I/O
program to write a `handle` whose arm **still has to call a runtime syscall
builtin to actually read the file**. The effect adds pure ceremony over a
builtin that is needed regardless.

So I/O operations are **runtime builtins**, exactly as `print` is
(`sentinel_print`): typed via the builtin-signature table, lowered to a call to a
`sentinel_*` runtime function, no Sentinel-source body. This is the lowest-risk,
most-ergonomic path and matches the existing `print` precedent (ADR 0033 noted
`print` is a runtime builtin "treated as effect-free … a future ADR may promote
`print` to carry `Io`").

### D3. Effect-row tracking (`Io` purity) is deferred (D8).

A separate, orthogonal question is whether I/O builtins should carry an `Io`
**effect row** (so the type system tracks which fns touch the outside world). At
the MVP they are **effect-free**, exactly like `print` today — promoting them to
`! { Io }` would force every I/O-using fn (and transitively much of a compiler)
to be annotated `! { Io }` and discharge it, with no runtime handler to discharge
*against* (the effect-check requires `main` effect-free). Promoting `print` +
the I/O builtins to a real `Io` effect — with a top-level/runtime discharge story
— is a coherent later refinement (it pairs with revisiting the C3.2(a)
builtins-are-effect-free amendment), deferred here.

### D4. Surface syntax.

```sentinel
fn main() -> i64 {
    let path: [u8] = "input.txt";
    let src: [u8] = read_file(path);         // whole file as bytes
    let n: i64 = len(src);                    // bytes read

    let out: [u8] = "result.txt";
    write_file(out, src);                     // write/overwrite (truncate)

    print_bytes(src);                         // echo to stdout
    n                                         // exit = byte count
}
```

  - **`read_file(path: [u8]) -> [u8]`** — read the entire file at `path` into a
    fresh heap-owned `[u8]` (`{ len = file size, data }`). The result drops /
    moves via the existing array machinery. **Panics** (abort) if the file
    cannot be opened or read (D5).
  - **`write_file(path: [u8], data: [u8]) -> i64`** — create-or-truncate the
    file at `path` and write all `len` bytes of `data`. Returns `i64` 0
    (statement-shaped, like `print`). **Panics** on open/write failure.
  - **`print_bytes(data: [u8]) -> i64`** — write `data`'s bytes to stdout
    (the byte-oriented companion to `print`'s single i64). Returns 0.
  - **Paths are `[u8]`** — NUL-terminated by the runtime wrapper before the libc
    call (the Sentinel `[u8]` is length-prefixed, not NUL-terminated, so the
    wrapper copies into a NUL-terminated C buffer). A path with an embedded NUL
    is rejected (panic) — defensive, not a security model.

### D5. Error model — panic-on-failure (MVP).

I/O failures (open/read/write errors, a bad path) **abort** with a diagnostic to
stderr, like an out-of-bounds index (`sentinel_panic_oob`) or a failed
allocation (`sentinel_alloc`). A new `sentinel_panic_io(...)` (or a reuse of the
abort path) prints the failing operation + path + errno. A **recoverable** error
model — `read_file(path) -> ?[u8]` (nullable) or `-> Result<[u8], IoError>` — is
**deferred** (D8): a real `Result` wants D.1b generic enums (ADR 0032 A3), and
nullable `?[u8]` wants the `?[T]` deferral lifted (ADR 0015 D6 / C1.6). For a
self-hosting compiler reading a known source path, abort-on-missing is acceptable
at the MVP (the driver validates paths before invoking).

### D6. Representation + runtime.

  - **`read_file`** — the runtime opens + stats + reads the file into a
    `sentinel_alloc`'d buffer and returns it as the `[u8]` `{ len, data }`. Two
    plausible ABIs (settle at (1/N)): (a) `sentinel_read_file(path_ptr, path_len,
    out_len: *mut i64) -> *mut u8` (return the data pointer, write the byte count
    to an out-param; codegen assembles `{ load(out_len), data }`), or (b) return
    a `#[repr(C)] { i64 len, *mut u8 data }` by value. **(a) is recommended** —
    out-param return matches C idioms + the existing `*mut u8`-returning
    `sentinel_alloc`/`realloc` shape (no struct-return ABI subtlety).
  - **`write_file`** — `sentinel_write_file(path_ptr, path_len, data_ptr,
    data_len) -> void` (abort on failure). libc `fopen("wb")` + `fwrite` +
    `fclose`.
  - **`print_bytes`** — `sentinel_print_bytes(data_ptr, data_len) -> void`
    (`fwrite` to stdout).
  - The data buffer `read_file` returns is owned by the caller's `[u8]` binding
    and freed at scope-exit drop (the existing `Type::Array` drop). `write_file`
    / `print_bytes` **borrow** their `[u8]` args (the ADR 0033 A3 runtime-builtin
    non-consuming rule, like `str_eq`): they do not free them.

### D7. Codegen + types.

  - **Types:** `read_file` / `write_file` / `print_bytes` are non-generic builtin
    signatures (concrete `[u8]`), typed in `sentinel-types` like `str_eq`:
    `read_file([u8]) -> [u8]`, `write_file([u8], [u8]) -> i64`, `print_bytes([u8])
    -> i64`. They register in `sentinel-resolve` as the next `FnId`s (the
    now-familiar FnId-shift; fix the hardcoded-FnId test sites).
  - **Codegen:** dispatch in `lower_call` (like `str_eq`); extract `{ len, ptr }`
    from each `[u8]` struct arg, call the `sentinel_*` extern, and for
    `read_file` assemble the returned `{ len, data }` `[u8]` struct.
  - **Borrow-check:** `write_file` / `print_bytes` args are borrowed (the
    existing runtime-builtin rule); `read_file`'s result is a fresh owned `[u8]`.

### D8. Out of scope (MVP).

A **recoverable error model** (`?[u8]` / `Result` — D5); **`Io` effect-row**
promotion (D3); **streaming / incremental** read/write (whole-file only);
**file handles / `open`/`close`/`seek`/`fd`s** (the surface is whole-file
read/write); **stdin** (`read_stdin`) — easy to add but not on the
read-source/write-artifact critical path; **directories / metadata / stat /
remove / rename**; **append mode** (write truncates); **`Vec<u8>`-returning
`read_file`** (`[u8]` suffices — the size is known at read; a `Vec<u8>` variant
is trivial via the D.3 machinery if a grow-after-read need appears); **UTF-8
validation / a nominal `Path`**; **`secret` I/O** / constant-time file access;
**buffering policy / async I/O** (the C4.4 `Async` effect is concurrency, not
file I/O).

### D9. Pipeline / sub-phase split.

| Sub        | Title                                                          | Risk   |
|------------|----------------------------------------------------------------|--------|
| D.4 (1/N)  | `read_file` + `write_file` + the `sentinel_read_file` /        | medium |
|            | `sentinel_write_file` runtime symbols + types/codegen +        |        |
|            | the error/abort path + a **round-trip** phase-go (write a      |        |
|            | file, read it back, compare). The mutually-verifiable core.    |        |
| D.4 (2/N)  | close — `print_bytes` (stdout) + `abi-v1` entries + the        | low    |
|            | symbol-set test count + ADR flip + a richer phase-go (read a   |        |
|            | fixture source, `str_eq` a known prefix).                      |        |

(`read_file` + `write_file` land together in (1/N) because a write-then-read-back
round-trip is the cleanest self-contained, leak-checkable phase-go — neither is
verifiable in isolation without a pre-existing fixture file on disk.)

### D10. Phase-go + fixture.

`tests/pass/c5d4_file_io.sentinel`: `write_file` a known `[u8]` to a temp path,
`read_file` it back, `str_eq` the two (or compare `len` + a few bytes via `v[i]`
/ `s[i]`), returning a computed exit code; **verified leak-free** (`leaks
--atExit` — the `read_file` buffer + any temporaries are freed at scope-exit
drop). The pass harness writes to a unique temp path (under `target/`) so the
test is hermetic + repeatable. Plus a unit corpus (read_file/write_file/
print_bytes typing) and a UI/negative fixture (e.g. `read_file(5)` — a non-`[u8]`
arg → `CallArgMismatch`).

## Reasoning

**Why runtime builtins, not algebraic effects.** Sentinel's effects (ADR 0020)
are *resumable user-defined computations*: a `handle` arm receives the op's args
+ a continuation `k` and decides how (and whether) to resume. That is the right
model for `log` / nondeterminism / cooperative scheduling — computations the
*program* defines and interprets. OS file I/O is an *irreversible side effect*
mediated by the kernel, not a computation the program resumes. Worse, the effect
system forbids an effectful `main` (effects must be discharged by a user
`handle`), and there is no runtime-provided handler — so an `Io` effect would
force every I/O program to wrap calls in a `handle` whose arm must itself call a
runtime syscall builtin. The builtin is needed either way; the effect is
ceremony. `print` already establishes the builtin pattern. The honest reframe of
ADR 0031 D4's "effects + handlers" is: the *runtime* performs the syscalls (true)
and the *effect-row system* may later *track* I/O purity (deferred, D3) — but the
*operation surface* is builtins.

**Why whole-file `[u8]`, panic-on-error.** A lexer wants the whole source as
bytes; `[u8]` already gives owned, sized, droppable bytes with `len` / indexing /
`str_eq`. Streaming + handles + a recoverable error model are real but are
refinements gated on need (and a real `Result` wants D.1b generic enums).
Panic-on-failure matches the existing OOB / bad-alloc aborts and keeps the MVP
surface free of an error type the language can't yet express ergonomically.

**Why libc.** The emitted programs already link the C runtime
(`sentinel_alloc`/`free`/`realloc`/`print`); `fopen`/`fread`/`fwrite`/`fclose`
are the same substrate, portable on the macOS + LLVM 18 target, and avoid a
syscall-ABI layer.

## Consequences

### Positive
- A self-hosting lexer's input (the source file) and a self-hosted compiler's
  output (a text artifact) + diagnostics become expressible — closing the
  ADR 0031 D4 item-4 gap — with maximal reuse of the `print` builtin pattern +
  the `[u8]` array machinery (low novelty risk).
- The first real **outside-world** primitives, established as builtins, leaving
  the door open to a later effect-row promotion (D3) without reworking the
  operation surface.

### Negative
- A real runtime addition (libc file ops + their `abi-v1` entries + a panic-on-IO
  path), and another `FnId`-shift cascade (the now-familiar tax).
- The error model is panic-only at the MVP — a missing/permission-denied file
  aborts the whole program (acceptable for a compiler reading a validated source
  path; a recoverable model is deferred).
- Amends ADR 0031 D4's "effects + handlers" framing (documented above).

### Neutral
- No effect-row change (I/O builtins stay effect-free like `print`), so existing
  programs + an effect-free `main` keep type-checking unchanged.

## Revisit

PROPOSED until D.4 closes. Triggers:
- **D3/D5**: when a self-host stage needs to *recover* from a missing file (e.g.
  a multi-file driver probing include paths), lift the error model to `?[u8]` /
  `Result` (coordinate with D.1b generic enums) and/or promote I/O to an `Io`
  effect row with a top-level discharge.
- **D6**: if whole-file read is too coarse (huge inputs / streaming), add file
  handles + incremental read/write (a separate sub-phase).
- **D8**: bring `read_stdin` / directories / `stat` forward as the self-host
  driver needs them.

## OPEN DESIGN POINTS (settle before D.4 (1/N))

1. **Effects-vs-builtins (D2).** This ADR proposes **runtime builtins**, amending
   ADR 0031 D4's "effects + handlers." Confirm (or, if a runtime-provided `Io`
   handler is wanted, that is a much larger sub-phase — flag it).
2. **Error model (D5).** Proposed: **panic-on-failure** for the MVP. Confirm vs.
   blocking the whole phase on a recoverable `?[u8]` / `Result` (which pulls in
   D.1b).
3. **MVP surface (D4/D8).** Proposed: `read_file` + `write_file` (+ `print_bytes`
   in (2/N)). Confirm whether `read_stdin` belongs in the MVP (a self-host driver
   may read source from a path, not stdin).
