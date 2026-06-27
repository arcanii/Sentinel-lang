# ADR 0059: A C-ABI export — calling Sentinel from C / C++ / Rust / Python

Status: **ACCEPTED-WITH-AMENDMENTS (A1–A7).** Implemented `snc`-side: the `export "C"`
annotation, the `--lib` static-archive build mode (no `main`), `--emit-header`, and the
secret-fence. **Phase 1a** = the value-only ABI (`i64`/`f64`) with a constant-time
demonstrator (`ct_choose`). **Phase 1b** (A6) adds the **`&[u8]` INPUT buffer ABI** —
a `&[u8]` export param is presented to C as a `(const uint8_t* data, int64_t len)` pair via
a generated wrapper, demonstrated by `ct_byte_eq` (a verified constant-time byte
comparison) called from C over real buffers. **Phase 1b (A7)** now adds the **owned-`[u8]`
RETURN ABI** — an `export "C" fn … -> [u8]` returns a heap buffer to C via the out-param
pair `(uint8_t** out_data, int64_t* out_len)` plus an exported `sentinel_free_bytes` the C
caller releases it with; demonstrated by `sha256_oneshot` (a verified-constant-time SHA-256
callable from C — the headline) and `repeat_byte` (a variable-length output). Multi-module
libraries and shared objects remain deferred. The design below stands; the amendments at
the end record the re-scoping. This was the ADR-first design gate for the "Python / C /
C++ / Rust bindings" item on the core-libraries roadmap.

## Context

ADR 0057 designs the FFI **import** side (Sentinel declaring + calling native C functions).
This ADR is the **export** side: compile Sentinel code into a *library* (a `.a` archive
first, later `.dylib`/`.so`/`.dll`) that C, C++, Rust, Go, Python, … can link and call.

The compelling case is Sentinel's headline property. The whole `std/security` suite is
**machine-checked constant-time** crypto (X25519, Ed25519, AES-GCM, ChaCha20-Poly1305,
the SHA-2/3 families, …). Exporting it as a C library means *any* language gets a
**drop-in, verified constant-time** primitive: write the primitive once in Sentinel, where
the compiler proves on every build that no secret bit reaches a branch / index / divisor /
shift, and ship it to the whole ecosystem. That is the most valuable thing Sentinel can do
*beyond* Sentinel — and it is the natural payoff of the entire crypto + examples track.

The substrate already exists. `snc` compiles each module to an object file with a C-shaped
ABI (`[u8]` is `{ i64 len, ptr data }`); the runtime is **already** built as a `staticlib`
(`crates/sentinel-runtime/Cargo.toml`, `crate-type = ["lib", "staticlib"]`); and the
driver links objects with `cc` (`link_objects` in `crates/sentinel-driver/src/main.rs`).
What is missing is five things: (1) a way to **mark** functions as C-ABI exports with
stable, un-mangled symbols; (2) a **library build mode** (no `main`, emit `.a`/`.so`
instead of an executable); (3) a generated C **header**; (4) the **memory-ownership
contract** for buffers that cross the boundary; and (5) per-language **binding** wrappers
on top of the C ABI. This ADR reuses ADR 0057's FFI-safe ABI (the `ptr` type, the buffer
marshalling, and — critically — the secret-fence) and adds those five pieces in the export
direction.

## Decision

An **`export "C"`** annotation plus a **library build mode**.

```
// a normal Sentinel function, exported under its un-mangled C symbol:
export "C" fn sha256_oneshot(msg: &[u8]) -> [u8] { … }
```

Six design pieces:

### 1. The `export "C"` annotation (a stable symbol + the C ABI)

An `export "C" fn name(params) -> ret { body }` is an ordinary Sentinel function, except:

- its symbol is the **un-mangled** C name `name` (not the internal `abi-v1` mangling the
  separate-compilation back end uses, ADR 0037) — so C can link it by name; and
- its signature is restricted to the **FFI-safe ABI of ADR 0057** (`i64`, the opaque
  `ptr`, and the buffer marshalling below), validated at the boundary.

Only `export`ed functions are public symbols in the emitted library; everything else stays
internal (`linkonce_odr`/private), so the library has a clean, minimal surface.

### 2. The buffer ABI (idiomatic C `(ptr, len)`)

A Sentinel `&[u8]` parameter is presented to C as the idiomatic **`(const uint8_t* data,
int64_t len)` pair** — the exported wrapper reconstructs the `[u8]` fat pointer internally.
Returning an owned `[u8]` returns it as a `{ uint8_t* data; int64_t len; }` out-struct (or
via out-params), heap-allocated by the Sentinel runtime; the C caller releases it with the
exported `sentinel_free_bytes(uint8_t* data)`. A second supported convention — for
fixed-size outputs (a 32-byte hash, a 64-byte signature) — is **caller-provides-buffer**:
`fn(... , out: ptr /* uint8_t* */)` where C passes a stack buffer and Sentinel writes into
it (no allocation, no free). Both are documented; the fixed-size crypto outputs use the
second, which is the cleanest for callers.

### 3. The secret-fence — and the headline it enables

Exported functions take and return **PUBLIC** types only — a `secret` value cannot cross
the export boundary (the C caller is outside the verified region), exactly the fence ADR
0057 puts on imports. This is not a limitation; it is **the feature**:

> An exported function receives **public** bytes from C, **widens them to `secret`**
> internally (`public→secret`, ADR 0049), runs the **machine-checked constant-time**
> implementation, and **`declassify`s** the result to return. The C/Python/Rust caller
> therefore gets a verified constant-time primitive behind a plain public-bytes API. The
> constant-time proof covers the Sentinel implementation; the boundary is public I/O.

So a Python program doing `sha256(key)` through the exported library inherits Sentinel's
proof that the implementation is constant-time — a guarantee it could not get from a
hand-written C library. This is the export side's reason to exist.

### 4. The library build mode (`--lib` / `--shared`)

A new build mode emits a **library** instead of an executable: `snc build --lib <entry> -o
libfoo.a` (Phase 1, a static archive) — and later `snc build --shared <entry> -o
libfoo.{dylib,so}` (Phase 2, a shared object, with PIC codegen + a different link line).
The mode requires **no `main`** (a library has no entry point), archives the emitted
object(s) together with `libsentinel_runtime.a`, and is deterministic/reproducible like the
existing `build` (ADR 0045). The internal `--separate` machinery (ADR 0037) composes — a
multi-module library is the same per-unit objects, archived rather than linked-to-exe.

### 5. Header generation (`--emit-header`)

`snc build --lib --emit-header foo.h` writes a C header from the exported signatures: the
`export "C"` function declarations (with the `(ptr,len)` buffer expansion), the
`{ uint8_t*; int64_t; }` bytes struct, and `sentinel_free_bytes`. The header is the
contract a C/C++ program `#include`s; it is also the input a per-language generator reads.

### 6. Per-language bindings (on top of the C ABI)

The C header + library is the lingua franca every target language already speaks, so it is
the MVP; language-specific wrappers layer on and are **later phases**:

- **C / C++** — the generated header works directly (optional C++ RAII wrappers over the
  free-function contract).
- **Rust** — a generated `*-sys` crate of `extern "C"` declarations matching the header
  (and optionally a safe wrapper crate).
- **Python** — a `ctypes`/`cffi` wrapper module over the shared library (and later a proper
  C-extension / wheel for packaging).
- **Go / others** — cgo / each language's C-FFI, from the same header.

## Phasing

- **Phase 1** — `export "C"` + `--lib` (static `.a`, no `main`) + the buffer ABI + the
  secret-fence + `--emit-header` + `sentinel_free_bytes`; a demonstrator that exports a
  couple of crypto primitives (e.g. `sha256`, `ed25519_sign`/`verify`) and calls them from a
  tiny C driver (and shows the secret-widen-internally pattern); macOS + Linux.
- **Phase 2** — shared libraries (`.dylib`/`.so`, PIC); struct exports (`#[repr(C)]`); the
  caller-provides-buffer convention fully fleshed out.
- **Phase 3** — the Python (`ctypes` + wheel) and Rust (`-sys` crate) binding generators;
  Win32 `.dll` (riding ADR 0057 Phase 3's PE/COFF toolchain).

## Alternatives considered

1. **Hand-write a C shim per exported function (status quo-ish).** Doesn't scale and is
   error-prone (every export is a manual ABI translation); the `export` annotation +
   header generation automate exactly that.
2. **Export the internal `abi-v1` mangled symbols + publish the mangling.** Fragile and
   not C-callable without a demangler; C wants a stable un-mangled symbol, which is what
   `export "C"` gives.
3. **Static libraries only, never shared.** Static is the right *Phase 1* (simplest link,
   no PIC); shared objects (needed for `ctypes`/`dlopen` and for not statically linking the
   runtime into every consumer) are Phase 2, not dropped.
4. **A full IDL / SWIG-style multi-language generator up front.** A large tooling project.
   The C ABI is the common denominator every language already binds to, so the C header is
   the MVP and per-language generators are an additive Phase 3 — not a prerequisite.
5. **Embed a scripting bridge (e.g., a Python C-extension hand-written against the
   internals).** Couples Sentinel to one language's runtime; the C ABI keeps it
   language-neutral.

## Constant-time interaction

The export boundary is public (the secret-fence, above), so the language's headline
guarantee is **strengthened**, not weakened: the verified constant-time region is the
exported Sentinel implementation, and the public boundary is the only thing the outside
world sees. The widen-internally / declassify-on-return pattern is the sanctioned way a
caller's data enters and leaves the verified region — the same discipline the sockets
boundary (ADR 0056) and the FFI import (ADR 0057) already use, here turned into a product:
constant-time crypto as a drop-in library.

## Self-hosting (`scg` mirror)

A front-end + codegen + driver change: the parser accepts `export "C"` on a fn; resolve
marks it exported and pins its symbol to the C name (resolved by name — **no builtin
`FnId` slot, no `FnId` shift**); types validate the C-ABI signature + the secret-fence;
codegen emits the function with `External` linkage under the un-mangled name + the
`(ptr,len)` buffer lowering, plus the `sentinel_free_bytes` export; the driver gains the
`--lib`/`--emit-header` mode. As with the prior language gaps, the `scg` mirror follows the
first corpus fixture that uses `export` (none exist today, so both bootstrap fixed points
stay byte-identical until then). The `--lib` build mode is driver-only and does not affect
the emitted-object differential (ADR 0045) — the objects are identical; only the final
archive-vs-link step differs.

## Touch-points (Phase 1, `snc`)

- **lexer / parser** — the `export "C"` annotation on a `fn` definition.
- **resolve** — mark the fn exported; pin its symbol to the un-mangled C name (no `FnId`).
- **types** — validate the FFI-safe C-ABI signature; the secret-fence on params/returns;
  reuse ADR 0057's `ptr` / buffer rules.
- **codegen** — emit the exported fn with `External` linkage + the C name + the `(ptr,len)`
  buffer lowering; emit a `sentinel_free_bytes` export; keep everything else internal.
- **driver** — a `--lib` build mode (no `main`; archive the object(s) + the runtime
  staticlib into a `.a`); `--emit-header` (generate the C header from the exported
  signatures); later `--shared` (PIC + shared link).
- **scg mirror** — deferred until the first `export` fixture.

## Scope (what a first increment would land)

`export "C"` + `--lib` (static archive, no `main`) + the `(ptr,len)` buffer ABI + the
secret-fence + `--emit-header` + `sentinel_free_bytes`, on macOS + Linux, with a
demonstrator: export `sha256` (+ maybe `ed25519`) from Sentinel, build `libsentinelcrypto.a`
+ `sentinelcrypto.h`, and call it from a small C program in a harness test — showing that a
foreign caller gets the constant-time primitive over a public-bytes API. Shared libraries,
struct exports, and the Python/Rust generators are deferred to later phases.

## Amendments (at implementation)

- **A1 — Phase 1 is split; the implemented increment (Phase 1a) is the VALUE-ONLY ABI
  (`i64`/`f64`).** An export's params/returns are restricted to public `i64`/`f64` (reusing
  ADR 0057's `is_ffi_safe`), so its signature IS already the C ABI — no wrapper, no buffer
  marshalling. The `(ptr,len)` buffer ABI (for `&[u8]` params + owned `[u8]` returns +
  `sentinel_free_bytes` + the caller-provides-buffer convention) — i.e. the byte-buffer
  crypto exports like `sha256` — is re-scoped to **Phase 1b**, along with multi-module
  libraries (`use`; Phase 1a is SINGLE-FILE) and shared objects (Phase 2). This is the same
  split ADR 0057's import side took (value-only first, `ptr`/buffers second).
- **A2 — export info flows via a `Vec<FnId>`, not a per-signature flag.** `resolve` records
  each `export "C"` fn's id in `ResolvedProgram.exports`; types validates each one's
  signature (FFI-safe + the secret-fence, reusing `ExternFfiType`) and carries the ids to
  `TypedProgram.exports`; codegen + the header generator read that set. An export is an
  ordinary fn (its body is type-/borrow-/CT-checked and codegen'd normally) — `export "C"`
  only pins its symbol and validates its boundary.
- **A3 — the C symbol stays BARE.** `merge_modules` keeps export fn names out of the
  per-module rename map (like the entry's `main`), and codegen emits an export under its
  bare un-mangled name + `External` linkage (a single-file / merged build's empty
  `module_path` already gives bare names; this also pins it for `--separate`). Non-export
  fns staying internal (`linkonce_odr`/private) for a minimal symbol surface is a deferred
  polish — at Phase 1a they remain `External` (extra, harmless public symbols).
- **A4 — `--lib` archives with `libtool -static` (macOS).** The emitted object + the
  runtime staticlib are bundled into one self-contained `.a`. The Linux `ar`-MRI path and
  `--shared` (PIC `.dylib`/`.so`) are deferred. The pipeline (resolve-without-`main` via
  `resolve_module`, then effect-/borrow-/CT-check) is the executable build's, minus the
  link-to-exe step; a library is still fully verified.
- **A5 — the demonstrator is value-granular but shows the full headline.** Since the buffer
  ABI is Phase 1b, the constant-time demonstrator is `ct_choose(cond, a, b)` — a branch-free
  conditional select that widens the public ints to `secret`, runs the machine-checked
  constant-time select, and `declassify`s the result. `examples/export/{ct_select.sentinel,
  driver.c}` + `tests/export.rs` build the `.a` + header, compile a C driver against them,
  run it, and assert exit 42 — a C program getting Sentinel's verified-constant-time
  primitive over a plain C ABI. The byte-buffer crypto export (`sha256` from C) lands with
  Phase 1b's buffer ABI. The `scg` mirror is deferred (no corpus / `selfhost` source uses
  `export` → every differential + both bootstrap fixed points byte-identical).
- **A6 — Phase 1b lands the `&[u8]` INPUT buffer ABI via a generated wrapper.** A `&[u8]`
  export param is now FFI-safe (`is_byte_slice_ref`). When an export takes one, codegen
  emits its real Sentinel-ABI body under an internal `<name>__sentinel_impl` symbol and
  generates a C-ABI **wrapper** under the bare `<name>` (modelled on the existing
  `__spawn_wrapper` post-pass): the wrapper takes each `&[u8]` as a
  `(const uint8_t* data, int64_t len)` pair, rebuilds the Sentinel `{ i64 len, ptr data }`
  fat pointer on the stack, forwards value params straight through, calls the impl, and
  returns the value-ABI result; `--emit-header` expands `&[u8]` to the C pair. Sentinel
  internal calls still resolve to the impl (via the `fns` map). The demonstrator gains
  `ct_byte_eq(a: &[u8], b: &[u8], n: i64) -> i64` — a verified constant-time byte
  comparison (the classic MAC/tag-verification primitive): it widens the first `n` bytes to
  `secret`, XOR-accumulates branch-free, folds to 0/1 in constant time, and declassifies;
  `driver.c` calls it over C buffers (equal → 1, differ → 0) and `tests/export.rs` asserts
  exit 42. STILL deferred (until A7): the owned-`[u8]` RETURN.
- **A7 — Phase 1b lands the owned-`[u8]` RETURN ABI via the OUT-PARAM convention + an
  exported `sentinel_free_bytes`.** An `export "C" fn … -> [u8]` now type-checks (the return
  type may be the PUBLIC owned `[u8]` — `Type::Array(ArrayElem::U8)` — in addition to the
  value ABI; `[secret u8]` / `secret [u8]` are still fenced out). The wrapper machinery
  (A6's `__sentinel_impl` post-pass) is generalized to fire whenever an export takes a
  `&[u8]` param **OR** returns `[u8]` (`export_needs_c_wrapper`). When it returns `[u8]`,
  the C-ABI wrapper takes **two extra trailing out-params** — `(uint8_t** out_data,
  int64_t* out_len)` — and returns `void`: it calls the impl (whose Sentinel-ABI return is
  the `{ i64 len, ptr data }` fat pointer), extracts the two fields, and stores `data` →
  `*out_data`, `len` → `*out_len`. The buffer is `sentinel_alloc`'d (heap, never arena —
  a returned value outlives its scope), and **ownership transfers to the C caller**, who
  releases it with the exported `void sentinel_free_bytes(uint8_t* data)` (a runtime symbol,
  a thin wrapper over the existing `sentinel_free`; declared in the header when any export
  returns bytes). The out-param convention (over a by-value `{ uint8_t*; int64_t; }` struct
  return) is chosen for ABI robustness: a by-value 16-byte struct return relies on LLVM's
  first-class-aggregate lowering matching the platform's small-struct register/`sret`
  coercion (which clang applies on the C side), a fragile coincidence across SysV/AAPCS;
  scalar-only out-params are unambiguous. `--emit-header` renders a `[u8]`-returning export
  as `void name(<inputs…>, uint8_t**, int64_t*);`. The secret-widen-internally /
  declassify-on-return pattern is shown end to end: `examples/export/digest_lib.sentinel`
  exports `sha256_oneshot(msg: &[u8]) -> [u8]` (widen the public bytes to a `[secret u8]`,
  run the machine-checked constant-time SHA-256 — inlined since single-file `--lib` has no
  `use` yet — then declassify the digest byte-by-byte into a public owned `[u8]`) plus
  `repeat_byte(value, count) -> [u8]` (a variable-length output, so the length genuinely
  flows back from Sentinel to C). `examples/export/digest_driver.c` calls both, checks the
  digest against the NIST "abc" vector, frees each buffer with `sentinel_free_bytes`, and
  `tests/export.rs` asserts exit 42. STILL deferred: the caller-provides-buffer convention
  (for fixed-size outputs with no allocation), multi-module export libraries (`--lib` with
  `use`; the demonstrator inlines SHA-256 for now), and shared objects (`--shared`). The
  `scg` mirror stays deferred (no corpus / `selfhost` fixture uses `export` → every
  differential + both bootstrap fixed points byte-identical).
