# ADR 0057: A foreign-function interface (`extern "C"`) for native OS bindings

Status: **ACCEPTED-WITH-AMENDMENTS (A1–A10).** Phase 1 is implemented `snc`-side as a
**value-only ABI** (public `i64` + `f64`), which already unlocks the libc identity calls
and the whole libm math family. **Phase 1b (A6)** adds the **`ptr` opaque type +
`ptr_of`/`ptr_of_mut`**, so a Sentinel buffer's data pointer can cross to a pointer-taking
libc call — demonstrated by `getentropy` (OS randomness into a Sentinel buffer) and
`strlen` (over a NUL-terminated `cstr`). **Phase 1b (A7)** completes the import buffer ABI
with the C-string READ-back: an `is_null(ptr) -> bool` intrinsic + a `cstr_read(ptr) -> [u8]`
helper (null-safe `strlen` + `memcpy`, both libc externs), so `getenv` is callable from
Sentinel (the value bytes copied out of the returned `char*`). The design below stands; the
amendments at the end record where the implementation refined or re-scoped it. This was the
ADR-first design gate for the "native bindings" item on the core-libraries roadmap.

## Context

Sentinel can today call exactly the **~31 hard-coded `extern "C"` runtime builtins** in
`sentinel-runtime` — file I/O (`read_file` / `write_file` / `print_bytes`, ADR 0035),
the TCP sockets (ADR 0056), `alloc` / `free` / `realloc`, and the structured-concurrency
primitives (ADR 0024). Each one is wired through the whole compiler: a `FnId` constant +
signature in `sentinel-resolve`, a typed signature in `sentinel-types`, an LLVM `declare`
+ dispatch arm in `sentinel-codegen`, and a mirror in the self-hosted `scg`
(`selfhost/*.sentinel`). Adding one is a multi-crate change, and because the builtins
occupy a contiguous `FnId` range (`0..=20`) that every user fn is offset past, **adding a
builtin shifts every user-fn `FnId`** and forces a byte-exact re-mirror of `scg` (the
sockets ADR's "FnId-shift crux").

That mechanism does not scale to "native bindings". There are thousands of libc / OS
functions; a real program wants `getenv`, `stat`, `gettimeofday`, `mmap`, `ioctl`,
`getrandom`, … — and which ones differ per platform. Hard-coding each as a builtin is a
compiler change per call. The crypto, data, text, collections, and net libraries are
shipped; the remaining big item — calling the host OS — needs a **general** way to
declare and call an arbitrary C-ABI function from Sentinel source.

Two facts make this tractable:

1. **The link step already pulls in libc.** `snc` links the generated object with
   `libsentinel_runtime.a` using `cc` (`crates/sentinel-driver/src/main.rs`), and `cc`
   links the platform C library (libSystem on macOS, glibc on Linux) by default. So a
   symbol like `getpid` is *already resolvable at link time* — the generated object just
   has to reference it. No new linking machinery is needed for the common case.
2. **The `[u8]` ABI is already C-shaped.** A Sentinel array is `{ i64 len, ptr data }`;
   the `data` field is exactly the `char*` / `void*` a C buffer API wants.

The hard parts are therefore not "how to call C" but: the **type/ABI marshalling**
(C `int` vs `i64`, `char*` vs `[u8]`, structs, NUL-terminated strings, raw pointers), the
**constant-time / `secret` interaction** (an FFI call is the single biggest threat to the
machine-checked `secret` guarantee), **safety** (the borrow checker can't see into C),
and **cross-platform** reach (Win32 is a different object format + linker + ABI).

## Decision

Add a user-declarable **`extern "C"`** FFI: a Sentinel source file may declare C-ABI
functions (no body), and the compiler emits an LLVM `declare` for each so `cc` resolves
them at link. Declarations are confined by convention to platform `std/sys/*` wrapper
modules that re-export *safe, idiomatic* Sentinel APIs; user code calls the wrappers.

```
// declaration (no body) — resolved against libc at link time:
extern "C" {
    fn getpid() -> i64;
    fn getenv(name: ptr) -> ptr;          // char* getenv(const char*)
    fn write(fd: i64, buf: ptr, count: i64) -> i64;
}
```

Five design pillars:

### 1. A restricted, explicit FFI ABI (no silent marshalling)

`extern` params and returns are limited to **FFI-safe types**, so the lowering is a
direct, predictable C ABI with no hidden copies:

- `i64` ↔ C `long` / `intptr_t` (the native machine word — the safe default).
- `ptr` — a NEW opaque raw-pointer type: a machine-word address that is **not
  dereferenceable in Sentinel**. It is produced only by (a) an FFI return, or (b)
  `ptr_of(&[u8])` (the buffer's `data` field) / `ptr_of_mut(&mut [u8])`, and is consumed
  only by passing it back to an `extern` call. This models C handles (a `FILE*`, an
  `mmap` result, a `char*`) as opaque tokens — Sentinel never reads through them, so it
  stays memory-safe at its own level.
- (Phase 2) width-annotated `i32` / `u32` for C `int` / `unsigned`, and `f64` (gated on
  ADR-for-floats) for C `double`.

A `&[u8]` is **not** auto-coerced to `ptr` — the caller writes `ptr_of(&buf)` explicitly,
and passes the length as a separate `i64` argument (exactly as libc APIs take a `void*` +
`size_t`). A `cstr(&[u8]) -> [u8]` helper (copy + append `\0`) produces a NUL-terminated
buffer for `char*` APIs; `ptr_of` of *that* is the `const char*`. There is **no automatic
struct or string marshalling in Phase 1** — `sockaddr`-style by-pointer structs are
Phase 2 (built from a `[u8]` scratch buffer the caller lays out, or a future `#[repr(C)]`
struct).

### 2. The `secret` / constant-time fence (the critical rule)

An FFI call jumps into code the compiler cannot see — it could branch on, index by, or
time-vary with its arguments. So FFI is fenced out of the secret domain: **`extern`
functions may take and return only PUBLIC FFI-safe types; a `secret` value reaching an
`extern` argument is a compile error.** To pass secret bytes to C, the caller must
`declassify` them first — making every secret-crossing an explicit, auditable boundary,
exactly as the socket boundary (ADR 0056) and the SSH `mpint` length leak already work.
This keeps the machine-checked constant-time proof **sound for all all-Sentinel code**:
the only way secret data leaves the verified region is an explicit `declassify`, FFI
included. (A crypto library therefore never FFIs out with key material; it stays in the
secret domain and the build remains the CT proof.)

### 3. Safety: unsafe leaves, wrapped in `std/sys`

An `extern` call is an unverified leaf — no Sentinel body for the borrow checker / CT
checker / drop planner to walk, and the C side's memory ownership is opaque. Raw
`extern` declarations live **only** in `std/sys/*` modules, which wrap them in safe
Sentinel APIs that own the marshalling, bounds, and lifetime contract (e.g.,
`sys::env::get(name: &[u8]) -> [u8]` builds the `cstr`, calls `getenv`, and copies the
result out of the returned `ptr` into an owned `[u8]` of a bounded length). User code
calls the wrapper, never the raw `extern`. (A future `unsafe`-block marker could make
direct `extern` calls in user code explicit; deferred — the `std/sys` convention is the
Phase-1 boundary.)

### 4. Linking: default libc now, `-l` / `dlopen` later

Phase 1 resolves symbols against the already-linked platform C library via `cc` — no new
flags. Phase 2 adds a way to name extra libraries to link (`extern "C" from "z"` or a
build flag threading `-lz`) and, for plugins, a `dlopen`/`dlsym` runtime path. The
reproducible-object discipline (ADR 0045) is preserved: an `extern` lowers to a
deterministic `declare` + `call`; only the link line changes.

### 5. Cross-platform: one mechanism, per-platform bindings

The MECHANISM (declare any C-ABI symbol, marshal the restricted ABI, link) is
platform-agnostic. The SPECIFIC bindings are per-platform `std/sys` modules selected by a
target cfg:

- **macOS + Linux first** — both are POSIX/libc over `cc`, Mach-O / ELF; the same
  `extern` declarations (`getpid`, `stat`, `clock_gettime`, `getrandom`/`getentropy`, …)
  work on both, modulo a few symbol/struct differences isolated in `std/sys/{macos,linux}`.
- **Win32 later (Phase 3)** — a bigger lift: the PE/COFF object format, a different linker
  and CRT, importing from `kernel32.dll`/`ws2_32.dll` via an import library, and (on
  32-bit) `stdcall` vs `cdecl`. The FFI mechanism is *designed to extend* to it (the ABI
  string `"C"` could gain `"stdcall"`/`"system"`, and the codegen target/triple already
  exists), but the toolchain port is out of scope for the first implementation. The
  current project targets macOS / LLVM 18; Linux is the natural second target, Win32 the
  third.

## Phasing (what each increment lands)

- **Phase 1** — `extern "C" { fn … ; }` declarations; the `ptr` opaque type +
  `ptr_of`/`ptr_of_mut`/`cstr`; the `i64`+`ptr` ABI; the secret-fence; LLVM `declare` +
  call; default-libc linking; a `std/sys/posix` module wrapping ~6 libc calls
  (`getpid`, `getppid`, `getuid`, `time`, `getenv`, `getentropy`/`getrandom`) on
  macOS+Linux; a demonstrator example (e.g., print the pid + a `$VAR` + 32 OS-random
  bytes). No `scg` mirror needed until a corpus fixture uses `extern` (then mirror it,
  per the language-gap discipline).
- **Phase 2** — width-annotated `i32`/`u32` (C `int`); `f64` (if floats land); by-pointer
  structs via a laid-out `[u8]` scratch (unblocks `stat`/`gettimeofday`/`sockaddr`);
  extra-library link flags + `dlopen`/`dlsym`.
- **Phase 3** — Win32: the PE/COFF + import-library toolchain, `std/sys/win32`, a target
  cfg, and the `"system"` ABI string.

## Alternatives considered

1. **Keep hard-coding runtime builtins (status quo).** Does not scale — every OS call is
   a five-crate change plus the `FnId`-shift + `scg` re-mirror. Fine for the handful of
   primitives the runtime *owns* (alloc, sockets, concurrency), wrong for open-ended OS
   bindings. The FFI explicitly does **not** disturb the existing builtins: `extern` fns
   are user-declared and resolved **by symbol name**, so they do **not** occupy the
   `0..=20` builtin `FnId` range and do **not** shift any user-fn `FnId` — the
   FnId-shift pain that every prior builtin caused is **avoided by construction**.
2. **A single generic `syscall(n, …)` builtin.** Works for raw Linux syscalls but not
   macOS (no stable public syscall ABI) or Win32; throws away type checking and can't
   express struct/pointer args. Useful as a *niche* addition, not the general mechanism.
3. **libffi (dynamic FFI at runtime).** A heavyweight runtime dependency and slower
   per-call; still needs the same type-marshalling design. The static
   declare-and-link approach reuses the existing `cc` link and stays zero-runtime-cost.
4. **A C-header importer (bindgen-style).** The ergonomic end state, but a large tooling
   project (a C parser + layout engine). The manual `extern "C"` declaration is the MVP
   that a header importer could later *generate* — so this ADR is a prerequisite, not a
   competitor.

## Self-hosting (`scg` mirror)

This is a front-end + codegen change, NOT a runtime change. The parser gains an `extern`
block; resolve registers each declaration as a new **`ExternFn`** item (kind distinct
from runtime builtins and user fns; its identity is the C symbol name, with **no builtin
`FnId` slot**); types validate the restricted FFI ABI and enforce the secret-fence;
codegen emits `module.add_function(cname, ty, External)` + an ordinary `call`. Because
`extern` is opt-in and the existing corpus + the `selfhost/*.sentinel` sources do not use
it, the bootstrap fixed point is undisturbed until a fixture exercises it — at which point
the `scg` parser/resolve/types/codegen mirror lands alongside that fixture and is
validated byte-for-byte against the `snc` oracle (the established language-gap cadence:
ADR-first, fixture, mirror, four-check). The `ptr` type adds one `Type` variant (like
`u128`, ADR 0055 — a variant, so no `FnId` shift).

## Touch-points (Phase 1, `snc`)

- **lexer** — `extern` keyword + the `"C"` ABI string literal.
- **parser** — an `extern "C" { fn name(params) -> ret; … }` top-level block → a list of
  body-less `ExternFn` decls.
- **resolve** — register each `ExternFn` (symbol = the C name; resolved by name, no
  `FnId`-builtin slot); add the `ptr` primitive + `ptr_of`/`ptr_of_mut`/`cstr` helpers.
- **types** — restrict `extern` params/returns to the FFI-safe set; **reject `secret`**
  reaching an `extern` argument (the fence); type `ptr` as opaque (no index/deref).
- **codegen** — `add_function(cname, fn_ty, External)`; lower an `extern` call as a plain
  `call`; `ptr_of` = extract the array's `data` field; `cstr` = copy + `\0`.
- **borrow / CT / drop** — an `extern` call is a leaf (no body to walk); `ptr` is Copy and
  owns nothing (the C side owns whatever it points at — the `std/sys` wrapper documents
  the contract).
- **driver/link** — unchanged for libc (already linked by `cc`); Phase 2 threads `-l`.
- **`std/sys/posix`** — the first safe wrappers + a demonstrator example, dual-built on
  both back ends like every other example.

## Scope (what a first increment would land)

`extern "C"` declarations + the `ptr` type + `ptr_of`/`cstr` + the secret-fence + the
LLVM declare/call + `std/sys/posix` wrapping ~6 libc calls on macOS+Linux + a
demonstrator, with the `scg` mirror following the first fixture. Everything beyond
(`i32`/structs/`dlopen`/Win32) is explicitly deferred to later phases. The headline
payoff: the OS-bindings item becomes *library* work (declare + wrap a symbol) instead of
a compiler change per call — the same shift the data/text libraries made for their
domain.

## Amendments (at implementation)

- **A1 — Phase 1 is split; the implemented increment is the VALUE-ONLY ABI (`i64` +
  `f64`).** A new `is_ffi_safe` admits only public `i64` / `f64`. This already covers the
  argument-less libc identity calls (`getpid`/`getppid`/`getuid`/`getgid`) AND the entire
  libm math family (`sin`/`cos`/`exp`/`log`/`pow`/`floor`/`ceil`/… are all `f64`-scalar) —
  so it is high-value on its own. The `ptr` opaque type + `ptr_of`/`ptr_of_mut`/`cstr`
  (and the libc calls that need them — `getenv`, `getentropy`, …) are re-scoped to a
  **Phase 1b** and deferred. `getentropy`-style read-back also needs a runtime
  cstr/buffer-read helper (a `ptr` is not dereferenceable in Sentinel), which is the bulk
  of the remaining Phase-1b work. The design's Phase-2 `f64` gate is moot now that floats
  ship (ADR 0058), so `f64` is in this first increment.
- **A2 — `extern "C"` fns reuse the cross-module-import "body-less fn" machinery, not a
  brand-new `ExternFn` runtime kind.** An `extern` is registered as a `FnSignature` with a
  new `is_extern_c` flag in the USER `FnId` range (after the builtins — exactly like a D.6
  cross-module import, so NO builtin `FnId` shift) + a `fn_table` entry; its param/return
  type-exprs ride in a `ResolvedExternFn`; types builds a `TypedFnSignature` and records
  the id in `TypedProgram.externs`; codegen declares it `External` under its BARE name and
  Pass 2 (which iterates `program.fns`) leaves it a declaration. So the ADR's "resolved by
  symbol name, no builtin FnId slot" is realised as the import machinery + a flag.
- **A3 — the C symbol stays BARE in every path.** `merge_modules` collects externs without
  adding them to the per-module rename map (so calls stay un-qualified) and dedups by name
  (a C symbol declared in several modules is one symbol); codegen forces the bare name for
  any `TypedProgram.externs` id (the merge path's empty `module_path` already gave bare
  names; this fixes `--separate`, where the unit's `module_path` would otherwise mangle it).
  Externs do NOT cross the separate-compilation import boundary — only their safe `pub fn`
  wrappers do (via the existing import path) — so no new cross-unit extern export was
  needed.
- **A4 — the fence is one error, `ExternFfiType`.** A non-FFI-safe `extern` param/return
  type (including any `secret` type) is rejected at the declaration. Combined with the
  no-narrowing rule (a `secret` value cannot be passed where a public type is expected),
  this enforces "a `secret` reaching an `extern` argument is a compile error" without a
  separate call-site check.
- **A5 — libm wrappers are named to avoid the C-symbol collision.** A `pub fn` cannot
  share a name with the `extern` it forwards to (both would occupy the `fn_table` slot), so
  `std/math/float` exposes `sine`/`cosine`/`tangent`/`exp_e`/`ln`/`powf`/`round_down`/
  `round_up`/`angle_of` over the raw `sin`/`cos`/… externs. An `extern`-symbol-aliasing
  form (`fn name = "c_sym"(…)`) that would let a wrapper reuse the idiomatic name is a
  natural Phase-1b/2 ergonomic. On macOS libm is in libSystem (linked by `cc`); on Linux
  `-lm` is needed — deferred until the build threads extra `-l` flags (ADR pillar 4).

Demonstrators: `std/sys/posix` + `examples/sys/process_ids` (i64 identity calls);
`std/math/float` libm bindings + `examples/math/transcendental` (f64). The `scg` mirror is
deferred (no corpus / `selfhost/*.sentinel` source uses `extern` → every differential +
both bootstrap fixed points byte-identical).

- **A6 — Phase 1b lands the `ptr` opaque type + `ptr_of`/`ptr_of_mut` (no runtime change,
  no `FnId` shift).** `Type::Ptr` is a new primitive variant (like `u128`/`f64` — a variant,
  so NO `FnId` shift), lowering to an LLVM opaque pointer; it is opaque (not indexable, not
  arithmetic — opacity falls out of `ptr` being absent from the numeric / array type sets),
  Copy, and owns nothing (no drop). `is_ffi_safe` admits it, so `extern` fns may take /
  return `ptr`. It is produced only by an FFI return or by `ptr_of(&[u8]) -> ptr` /
  `ptr_of_mut(&mut [u8]) -> ptr` — two single-argument INTRINSICS modelled on `sqrt`
  (`UnaryOp::PtrOf` / `UnaryOp::PtrOfMut`, recognised by reserved name in the parser, so
  NO `FnId`); each extracts the `data` field (field 1) of the borrowed `{ i64 len, ptr data }`
  slice. `ptr_of_mut` requires a `&mut [u8]` (so C may write through it under an exclusive
  borrow); `ptr_of` accepts a shared or mutable `&[u8]`. The fence holds: `ptr_of` rejects a
  `&[secret u8]` (it requires public bytes), and `ptr` is public, so a secret never crosses.
  `cstr(&[u8]) -> [u8]` (copy + append `\0`) is a PURE Sentinel `std/sys` helper (no compiler
  support needed). NO runtime change is needed for this slice: `getentropy` / `strlen` are
  libc symbols `cc` already links. Demonstrators: `std/sys/ffi` (`random_bytes(n)` via
  `getentropy` + `ptr_of_mut`; `cstr` + `cstr_len(&[u8])` via `strlen` + `ptr_of`) +
  `examples/sys/ffi_buffers` (asserts `strlen("hello") == 5` and that two `getentropy` draws
  of 32 bytes differ — proving the OS wrote randomness). STILL deferred (until A7): the
  C-string read-BACK. STILL deferred after A7: `i32`/`u32` widths, by-pointer structs,
  extra-library `-l`, `dlopen`, Win32. The `scg` mirror stays deferred (no corpus /
  `selfhost` fixture uses `ptr_of` / `extern`).

- **A7 — Phase 1b completes the import buffer ABI with the C-string READ-back (`is_null` +
  `cstr_read`), no runtime change / no `FnId` shift.** A `ptr` returned from C (e.g.
  `char* getenv(const char*)`) is opaque — Sentinel can't deref it — so reading its bytes
  back needs (1) null detection and (2) a byte copy out. Both are done WITHOUT a runtime
  builtin (which would shift `FnId`s): (1) a new `is_null(ptr) -> bool` INTRINSIC
  (`UnaryOp::IsNull`, recognised by reserved name like `ptr_of`/`sqrt`; codegen
  `build_is_null`); (2) the copy uses libc `strlen` + `memcpy` declared as ordinary
  `extern`s. The read-back is pure Sentinel: `cstr_read(p)` = `let n = if is_null(p) { 0 }
  else { strlen(p) }` (Sentinel has no `return`, so the if-expression idiom), build an
  `n`-byte `[u8]`, then `if n > 0 { memcpy(ptr_of_mut(&mut buf), p, n); 0 } else { 0 }` —
  guarding `n > 0` so `memcpy` never sees a NULL `src`. `std/sys/ffi` gains `getenv` +
  `memcpy` externs + `cstr_read` + `env_get(name: &[u8]) -> [u8]` (cstr the name, `getenv`,
  read back; an unset OR empty var → an empty `[u8]`, documented). `examples/sys/ffi_buffers`
  gains: `env_get("PATH")` is non-empty (the present-var read-back path) and a bogus name is
  empty (the NULL path). `is_null`'s operand must be `Type::Ptr` (else a `Mismatch` —
  reusing the existing error, no new variant). STILL deferred: distinguishing unset from
  empty (a `?[u8]` return), `i32`/`u32` widths, by-pointer structs, extra-library `-l`,
  `dlopen`, Win32. The `scg` mirror stays deferred (still no corpus / `selfhost` fixture
  uses `extern` / the `ptr` intrinsics).
- **A8 — extra-library linking (`--link`) + the first native-GUI calls (Windows).** ADR 0060
  (Windows host support) ported the toolchain (PE/COFF + `link.exe`), removing the Phase-3
  Win32 blocker this ADR deferred. On top, `snc build <file> --link <lib>` (repeatable,
  order-free) threads an extra native library into the link — pillar 4's "extra-library
  `-l`", now implemented in the ADR 0060 `HostToolchain` backend (`<lib>.lib` on MSVC,
  `-l<lib>` on Unix). That is already enough to call native **GUI** functions: `demos/win32/`
  declares `extern "C"` `GetSystemMetrics` + `MessageBoxA` (user32) — `HWND`/`UINT` ride the
  `i64` register ABI (`NULL`/`MB_OK` = 0), strings cross as `const char*` via a `cstr` +
  `ptr_of`. Verified on Windows: `snc build … --link user32` links user32, `screen_metrics`
  prints the live primary-screen size (2560×1440) from `GetSystemMetrics`, and `messagebox`
  pops a real Win32 message box. STILL deferred for richer GUI: `i32`/`u32` widths, by-pointer
  structs (`WNDCLASS`/`MSG`), and **function-pointer callbacks** (`WndProc` event loops — the
  next design gate). `--separate` does not thread `--link` yet. Implemented `snc`-side on
  branch `adr-0060-phase1-portable-runtime`; the `scg` mirror stays deferred (no `selfhost`
  fixture uses `extern`); pending the macOS four-check.
- **A9 — self-linking extern blocks (`extern "C" link("lib") { … }`).** Complements A8's
  `--link` CLI flag with a source-level annotation: an extern block declares the native
  libraries its symbols come from, so a *consumer* of the module needs no `--link`. The
  parser parses `link("a"[, "b"]*)` after the ABI string (`link` is contextual — still a
  valid identifier elsewhere); the libs ride on each `ExternFnDecl` as pure link metadata
  (resolve/types/codegen ignore it); the driver unions them with the CLI `--link` set
  (deduped) from the parsed/merged program's externs and passes them to the linker. This
  makes binding libraries self-contained: `std::sys::win32` declares `link("user32")`, so a
  program that `use`s it builds with a plain `snc build prog.sentinel` — no flag. Verified on
  Windows: a `use std::sys::win32` program with NO `--link` builds + runs (live screen size
  via `GetSystemMetrics`). NOT oracle-moving — the annotation never reaches the emitted IR or
  the `snc ast`/`lex` dumps, so both fixed points are unaffected and no `selfhost` mirror is
  needed. Implemented on branch `std-c-cstring`; pending the macOS four-check.

- **A10 — the Phase 1b INTRINSIC mirror landed, paired with its fixture; the FEATURE
  mirror is still deferred.** The "Self-hosting" section says the `scg`
  parser/resolve/types/codegen mirror "lands alongside that fixture". Two increments of
  that have now happened, and it is worth separating them: `extern` blocks were mirrored
  with `tests/pass/c57_extern_call.sentinel` (`5c911f5`, plus the `export "C"` dispatcher
  fix in `e2e5ee5`), and **`ptr_of` / `ptr_of_mut` / `is_null` + the `ptr` TYPE** are
  mirrored here with **`tests/pass/c57_ptr_of.sentinel`**.
  **What this half actually required** was smaller than "parser/resolve/types/codegen",
  because ADR 0058 A1 had already established the shape: these three are RESERVED NAMES
  the oracle rewrites at a CALL SITE into a unary node, which is precisely how they avoid
  an `FnId` (builtins occupy 0..=41, user fns are `42 + idx`, so a 42nd builtin would
  shift every user FnId). With `sqrt` they are the complete family of four, at scg unary
  op-codes 6..=9. `ptr` became scg SCALAR CODE 5 — a scalar, never an interner kind,
  because every `h < tbase()` test in the self-hosted typer *means* "scalar", and this
  ADR's own note that `ptr` "adds one `Type` variant … so no `FnId` shift" has the same
  spirit. The oracle's `is_copy_type` lists `Type::Ptr` with the integers, which is what
  makes the scalar treatment correct rather than merely convenient.
  **NO codegen half was needed, and the reason is precise:** `snc llvm` Errs on all three
  ("ptr_of / is_null not ported (ADR 0057 snc-only)") and on a `ptr` TYPE ("type not yet
  ported: Ptr"), so the codegen differential skips every program that uses them. That is
  the same structure ADR 0058 A8 records for floats. It does NOT mean `ptr` cannot reach
  scg's codegen at all: a PHANTOM generic argument does (`struct Pair<A, B> { a: A }` used
  as `Pair<i64, ptr>`), so `cg_mangle_to` carries a real scalar-5 arm — added pre-emptively
  here after the identical trap one scalar code over was found by review during the float
  mirror.
  ⚠ **The TAG changed at ADR 0016 A1**: it was `ty_Ptr` (copied from the oracle's
  `Debug`-leaking catch-all) and is now `ptr`. The old spelling made the oracle and scg
  agree with each OTHER while both disagreed with inkwell, which always said `ptr` — so the
  codegen differential was green on precisely the arm that was wrong. A1 moved all three to
  inkwell's spelling.
  **What the gap cost while it was open**, recorded because the registry understated it:
  scg did not merely emit `(call ptr_of …)`. The name resolved to nothing, so `resolve`
  printed `(call # …)` with an EMPTY FnId and `mir` printed `call print_bytes` — an
  unrelated builtin reached by indexing a name table at a negative callee.
  **STILL DEFERRED:** f64/ptr CODEGEN in `scg` (and the `snc llvm` text backend's own
  support for both), and the caller-provides-buffer convention of A7.
