# ADR 0059: A C-ABI export — calling Sentinel from C / C++ / Rust / Python

Status: **PROPOSED — design only.** This ADR records the design for *exporting* Sentinel
functions with a stable C ABI, so programs in other languages can call **into** Sentinel
(the complement of ADR 0057, which is the *import* direction — Sentinel calling out to C).
No implementation lands with this ADR; it is the ADR-first design gate for the
"Python / C / C++ / Rust bindings" item on the core-libraries roadmap.

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
