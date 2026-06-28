# ADR 0062: Conditional compilation — file-level target selection (v1); item-level `#[cfg]` deferred

Status: **PROPOSED → v1 IMPLEMENTING.** Target-conditional compilation so a program can be
built with platform-specific codepaths and libraries (Windows / Linux / macOS). v1 is
**file-level** (the module resolver selects a target-specific *file* per module), which is a
**resolve/driver change — NOT oracle-moving** (it changes only *which file* is read, never
how any file lexes/parses/lowers, so no `selfhost/` mirror, no re-bless, no fixed-point
dance — the same property as ADR 0037 point 12, `--lib-path`). Item-level `#[cfg]` (in-file
conditionals) is the **deferred follow-up** (it needs new syntax → oracle-moving).

Date: 2026-06-28
Related: **0037** (modules / `discover_module_graph` — the resolver this extends; point 12
`--lib-path`), **0060** (Windows host support — codegen is host-only, so the target is the
host by default), **0057 A9** (`extern "C" link(...)` self-linking — a `_<os>` module
declares its own native libs), **0061** (the `keygen` POSIX-`getentropy` gap this unblocks).

## Context

The platform gaps keep surfacing: `std::sys::win32` (user32) is Windows-only; `getentropy`
(the ADR 0061 `keygen_core` entropy source) is POSIX, so it does not link on Windows; the
broker's secret-memory hardening is `mlock` on POSIX and `VirtualLock` on Windows (ADR
0060). Today a program (or a `std` module) cannot offer different code per OS — there is no
conditional compilation. This ADR adds it.

The dominant design axis is **granularity**, because it decides whether the change is
oracle-moving:

- **File-level** — the resolver picks a different *file* per target. A resolve/driver
  concern; **not oracle-moving**.
- **Item-level `#[cfg(...)]`** — Rust-style attributes on individual items. New syntax → the
  lexer/parser change → the `snc lex`/`ast` dumps change → **oracle-moving** (mirror into
  `selfhost/`, re-bless, both fixed points green). Heavier, higher-risk.

Owner-chosen: **file-level now, `#[cfg]` later** — file-level covers the stated need
(platform libraries/bindings/codepaths live in platform files, the Go model) at low risk;
item-level lands as a follow-up ADR when in-function conditionals are actually needed.

## Decision (v1 — file-level)

### D1. Target identity.

A build has an active **target OS**: `windows` | `linux` | `macos` (matching Rust's
`target_os`). It defaults to the **host** OS (`std::env::consts::OS`; codegen is host-only
per ADR 0060). `snc build --target <os>` overrides it — selecting a different platform's
codepaths (useful for checking that, say, the Linux path type-/CT-checks on a Windows box;
it does **not** cross-compile a runnable foreign binary — that needs a cross backend, out of
scope). Light aliases: `win32`→`windows`, `darwin`/`osx`→`macos`.

The family **`unix`** = `linux` ∪ `macos`.

### D2. File-as-module + a target suffix.

A module's file may carry a reserved **`_<os>`** or **`_<family>`** suffix on its filename.
For module `a::b`, the resolver tries, **most-specific first**:

1. `<base>/a/b_<os>.sentinel`        (e.g. `b_windows.sentinel`)
2. `<base>/a/b_<family>.sentinel`    (`b_unix.sentinel`, only when os ∈ {linux, macos})
3. `<base>/a/b.sentinel`             (the portable default / fallback)

The **first existing file wins**. The suffix is a *selector*, not part of the module name:
all three resolve to module **`a::b`**, so an importer always writes `use a::b::item`
regardless of platform. Reserved suffixes: `_windows`, `_linux`, `_macos`, `_unix`.

### D3. Composition with the search path (ADR 0037 point 12).

Resolution stays **base-major** (the entry dir first, then each `--lib-path` / `SNC_LIB_PATH`
dir), and **suffix-minor within a base**. So a local module still shadows a library one (the
shadow rule is unchanged); *within* the winning base, the target-specific variant is
preferred. When no `_<os>`/`_<family>` files exist anywhere (the common case, and **every**
`selfhost/*.sentinel`), each base falls through to `b.sentinel` — **byte-identical** to
pre-0062 resolution. A `ModuleNotFound` lists every candidate tried (suffixed included), so a
missing platform variant is debuggable.

### D4. Reach (v1).

`--target` is wired on `snc build`; every subcommand resolves against the active target
(host by default), so `snc llvm`/`merge`/`build --lib`/`--separate` honor the host target.
(`--target` on those is a trivial follow-up.) Self-linking `extern "C" link(...)` (ADR 0057
A9) composes for free: a `_windows` module declares its own `link("user32")`, so per-target
libraries need no extra flags.

## Self-host

**Not oracle-moving.** No lexer/parser/AST/IR change; only `discover_module_graph`'s
candidate list grows (target-suffixed variants tried before the default). No `selfhost`
file uses a suffix, so selfhost resolution + every differential dump is byte-identical. No
re-bless, no `selfhost/` mirror, both bootstrap fixed points unaffected.

## Constant-time guarantee

**Untouched.** Selecting a different source file does not change the `secret` rules or the
`sentinel::mir::secret_leak` pass; whichever file is compiled is CT-checked exactly as any
module is.

## Non-goals (v1)

- **Item-level `#[cfg(...)]`** (in-function conditionals) — the deferred follow-up (new
  syntax, oracle-moving).
- **Cross-compilation** — `--target` selects codepaths but codegen is host-only (ADR 0060);
  a runnable foreign binary needs a cross backend.
- **Arch / feature cfg** (`x86_64`, endianness, feature flags) — OS only for v1; the suffix
  scheme extends to `_<arch>` later.
- **Boolean cfg expressions** (`not`/`any`/`all`) — the `_<os>`/`_<family>` precedence covers
  the common cases; richer predicates arrive with item-level `#[cfg]`.

## Open questions

- **`win32` vs `windows`** — v1 uses `windows` (Rust's `target_os`); `win32` is accepted as
  an alias. The existing `std::sys::win32` *module* keeps its name (it is the Win32-API
  binding, not a target cfg).
- **Family breadth** — only `unix` for v1. A `posix` synonym or finer families can follow.
- **A portable-default requirement** — should a module without a matching `_<os>`/default
  file be a hard error at *build* time (it already is: `ModuleNotFound`), or only when
  actually imported? v1: the existing `ModuleNotFound` at discovery.

## Follow-ups

1. ✅ **`std::sys::random`** (`_windows` = `RtlGenRandom`/`SystemFunction036` via advapi32,
   `_unix` = `getentropy`) — **DONE** (`c230ff3`), the first real consumer. Closes the ADR
   0061 Windows-`keygen` gap: `keygen_core` (and thus `snc keygen` → `sign` → `verify`) now
   builds + runs on all three targets, proven on Windows.
2. `--target` on `--lib`/`--separate`/`llvm`/`merge`.
3. Item-level `#[cfg(...)]` (a companion ADR; oracle-moving).
