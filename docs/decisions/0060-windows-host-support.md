# ADR 0060: Windows host support — a platform-portable build/link backend

Status: **PROPOSED.** Drafted from an empirical Windows bring-up (2026-06-27): the
compiler **builds** and its **analysis pipeline passes** on `x86_64-pc-windows-msvc`,
but `snc build` (emitting a linked artifact) does not. This ADR scopes the work to close
that gap. It is **not** oracle-moving (see *Self-host* below) and does **not** touch the
constant-time machinery.

## Context

Bringing the toolchain up on Windows (VS 2026 MSVC + a from-source LLVM 18.1.8) established,
empirically, where the platform boundary actually is:

- **Works today on Windows:** `cargo build -p sentinel-driver` (debug **and** release) links
  `snc.exe`; `snc lex` / `ast` / `parse` run; LLVM codegen runs (`snc build` reaches the
  link step, having emitted a valid host object); and the analysis-pipeline unit tests pass —
  **929 lib tests, 0 failures** across `sentinel-{syntax,ast,resolve,types,borrow-check,
  effect-check,mir,hir,base}`.
- **Blocked on Windows:** `snc build` / `--lib` / `--shared` → a linked artifact, and the
  full workspace test build (because `sentinel-runtime` does not compile).

This sharpens the long-standing **macOS-lock-in** concern (external review F5 / action-plan
**P2.4**). The lock-in is **not** in the compiler's analysis or codegen — those are already
platform-clean. It is two host-toolchain assumptions, and the driver says so itself
([`crates/sentinel-driver/src/main.rs:1121`](../../crates/sentinel-driver/src/main.rs):
*"the Sentinel toolchain currently targets macOS"*).

Three couplings, each verified:

1. **`sentinel-runtime` does not compile on Windows.** It uses an ungated
   `use std::os::unix::ffi::OsStrExt;` ([`runtime/src/lib.rs:187`](../../crates/sentinel-runtime/src/lib.rs),
   `OsStr::from_bytes` at :198) and a `unix` submodule. The crate is *partially*
   `#[cfg(unix)]`-gated (lib.rs:324/361/382/…/1540) but has **no Windows arm** and one ungated
   Unix path → `E0433` / `E0599`.
2. **Runtime staticlib name.** The driver hardcodes `libsentinel_runtime.a` (GNU naming);
   the runtime is `crate-type = ["lib", "staticlib"]` ([`runtime/Cargo.toml:18`](../../crates/sentinel-runtime/Cargo.toml)),
   which on MSVC emits `sentinel_runtime.lib`.
3. **Linker / archiver are Unix/macOS.** Linking spawns `cc`
   ([`driver/src/main.rs:1155`](../../crates/sentinel-driver/src/main.rs), and :1403 / :1819);
   the `--lib` archive step spawns `libtool -static` ([:1125](../../crates/sentinel-driver/src/main.rs)).
   Neither `cc` nor `libtool` exists on Windows; even Linux is only half-wired (it would need
   `ar`, as the :1121 comment notes).

Related (in scope for a *complete* port, not a blocker): the broker's STRICT secret-memory
policy is `mlock` + zero-on-free (ADR 0028); the Windows analogue is `VirtualLock`. The
broker already degrades to a LENIENT (no-lock) arena when `mlock` is refused (common on
macOS) — the same fallback covers Windows.

## Decision (proposed)

A **host-platform abstraction** in the driver, plus a **portable runtime**. The emitted IR
and every analysis stage are unchanged; only the host build/link backend and the runtime's
OS layer move.

### 1. Portable `sentinel-runtime`
Give each `#[cfg(unix)]` block a `#[cfg(windows)]` counterpart and replace the ungated
`OsStrExt` path with a portable byte↔`OsString` conversion. Secret-memory hardening uses
`VirtualLock`/`VirtualUnlock` on Windows, mirroring the broker's mlock STRICT/LENIENT
fallback (degrade to LENIENT where the OS refuses the lock — exactly as macOS does). Behavior
is identical where the OS allows it. **Phase 1 deliverable: the crate compiles on Windows**,
which alone unblocks the full `cargo nextest` build on Windows.

### 2. A `HostToolchain` abstraction in the driver
Replace the hardcoded `cc` / `libtool` / `libsentinel_runtime.a` with a small layer that
resolves, per host triple: the **linker driver**, the **archiver**, the **runtime staticlib
filename**, the **object extension**, and the **link-line flavor**.

| Host | Link (exe / shared) | Archive (`--lib`) | Runtime lib |
|------|---------------------|-------------------|-------------|
| macOS | `cc` (unchanged) | `libtool -static` (unchanged) | `libsentinel_runtime.a` |
| Linux | `cc` | `ar` | `libsentinel_runtime.a` |
| Windows (MSVC) | `clang` (VS-bundled) or `lld-link`/`link.exe` | `lib.exe` / `llvm-lib` | `sentinel_runtime.lib` |

On Windows, discover the linker the way Rust's `cc` crate already does (`find-msvc-tools` /
`vswhere`); prefer `clang` as a drop-in `cc` (VS ships it), fall back to `lld-link`. snc
already emits a host-correct object (it produced a valid COFF before failing at link), so
**no codegen change** is required.

### 3. Diagnostics
When the host toolchain is incomplete, emit a clear `miette` diagnostic naming the
platform-specific tool that is missing (extending the existing
`libsentinel_runtime.a not found` message to be platform-aware).

### 4. CI
Add a **Windows** lane and the **P2.4 Linux** lane, `continue-on-error: true` first, promoted
to required once green. The analysis suite is green on Windows today and can gate immediately;
the build-and-run integration tests gate after Phases 1–2.

## Phasing
- **Phase 1 — portable runtime** (cfg the POSIX bits). Low-risk, independently landable;
  unblocks the Windows analysis suite under nextest.
- **Phase 2 — `HostToolchain` link abstraction** (macOS unchanged; add Windows + Linux).
  Delivers `snc build` / `--lib` / `--shared` on Windows + Linux.
- **Phase 3 — CI lanes** (Windows + Linux), gating as they go green.

## Self-host
**Not oracle-moving.** This touches the host build/link backend and the runtime's OS layer —
**not** the emitted LLVM IR, any stage dump, or codegen. Both bootstrap fixed points are
unaffected and `selfhost/*.sentinel` needs no mirror. (Stated explicitly per the repo's
oracle discipline.)

## Constant-time guarantee
**Untouched.** The port does not modify the `secret` type rules, the
`sentinel::mir::secret_leak` pass, or codegen. The CT property constrains the *program*
pre-LLVM and is platform-independent; widening host support does not weaken it.

## Non-goals
- **32-bit `i686` ("Win32" proper)** — needs a separate 32-bit LLVM + target; out of scope.
- **Cross-compilation** (host ≠ target) — this is host-native build on Windows only.
- **MinGW / GNU-ABI Windows** — MSVC ABI only (matches Rust's default Windows target and the
  from-source LLVM built with `/MD`).

## Open questions
- Windows linker: prefer `clang` (cc-like, VS-bundled) vs `lld-link`/`link.exe` (no shim).
  Recommendation: `clang` first, `lld-link` fallback.
- Auto-discover MSVC (vswhere) vs require an active `vcvars` — auto-discover preferred for UX.
- `VirtualLock` working-set limits (Windows may need `SetProcessWorkingSetSize` for large
  locks) — the LENIENT fallback covers the edge.

## References
ADR 0059 (C-ABI export — owns the `cc`/`libtool`/runtime link path), ADR 0057 (FFI import —
also links via `cc`), ADR 0028 (broker integration — the mlock secret-memory policy),
ADR 0037 (separate-compilation link machinery), ADR 0045 (deterministic build);
`docs/REVIEW_ACTION_PLAN.md` F5 / P2.4 (macOS lock-in); `docs/STATE.md`.
