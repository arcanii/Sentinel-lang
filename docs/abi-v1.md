# Sentinel `abi-v1` — the stable ABI of compiled artifacts

Status: **FROZEN at `abi-v1`** (Phase C5 D7, per ADR 0029). This document
is the source-of-truth contract that a Phase-D self-hosted backend or an
independently-built runtime must target. It records **what the bootstrap
codegen already emits** — `abi-v1` documents and freezes the existing
behaviour rather than redesigning it.

**Change discipline.** Pre-1.0 the ABI may still change, but **every
change updates this document AND the stability tests (§7) in the same
commit** — a silent drift is meant to surface as a red test, not a latent
miscompile. Post-1.0, any layout / mangling / symbol change is a
**breaking bump to `abi-v2`**. Each item below links to where it is
realised in the bootstrap so the doc and the code stay cross-checkable.

Targets: x86-64 (SysV AMD64) and aarch64 (AAPCS64) — ADR 0025 D12.

---

## 1. Calling convention

Sentinel adds **no calling convention of its own**: every emitted
function uses the native C ABI via LLVM target lowering (SysV AMD64 /
AAPCS64). Small-struct return/pass is whatever the platform ABI dictates
(LLVM chooses register vs. by-pointer). Realised in
`sentinel-codegen` `compile_fn` / `compile_mono_fn` (the standard path).

| Function kind | Signature shape | Notes |
|---------------|-----------------|-------|
| `main` | `() -> i32` | exit code = the tail value truncated `i64`→`i32` (ADR 0012 D11). |
| Ordinary fn | params + return lowered by their `Type` (§2), returned **by value** | LLVM applies the platform small-struct rules. |
| Effecting fn (`uses_kont_abi`) | returns `ptr` (`*SentinelKont`), **not** the surface type | free-monad ABI (ADR 0020 D7); the surface value is delivered via the kont. |
| Class init | first param is the instance `out_ptr` (`ptr`); writes through it; returns void-ish | ADR 0022 D9. |
| Method / impl method | first param is `self_ptr` (`ptr`) | ADR 0022/0023. |

`TypeParam` / `TraitSelf` never appear at the ABI boundary
(monomorphised / substituted before codegen); `Secret` is stripped to
its inner type at layout entry.

---

## 2. `Type` data layout

The in-memory layout of every `Type` constructor. All layouts are
**LLVM-default-aligned and non-packed**. Source of truth:
`llvm_basic_type` in `sentinel-codegen/src/lib.rs`.

| `Type` | `abi-v1` LLVM layout |
|--------|----------------------|
| `bool` | `i1` |
| `i32` | `i32` |
| `i64` | `i64` |
| `u8` | `i8` (unsigned — signedness lives in the ops: `udiv` / unsigned `icmp`, not the type) — ADR 0033 D4/D6 |
| `Struct(id)` / `GenericInstance(id)` / `Class(id)` | a **named LLVM struct**, fields in **declaration order** (no reordering) |
| `[T]` (`Array`) | `{ i64 len, ptr data }` — ADR 0015 D1 |
| `Vec<T>` (`Vec`) | `{ i64 len, i64 cap, ptr data }` — the `[T]` layout with a capacity field inserted, so `data` is **field 2** (offset 16); 24 bytes, align 8. `data` points at a heap buffer of `cap * sizeof(T)` bytes, the first `len` live; grown by `sentinel_realloc` (§5). `String` = `Vec<u8>` (deferred to D.3 (2/N)). — ADR 0034 D3 |
| `?P` where `P` is primitive | `{ i1 valid, P }` (inline) — ADR 0015 D11 |
| `?Struct` / `?GenericInstance` | `{ i1 valid, ptr payload }`, `payload` points at a heap-allocated value — ADR 0015 D11 |
| `&T` / `&mut T` (`Ref`) | opaque `ptr` (LLVM 15+ opaque pointers) — ADR 0017 D11 |
| `secret T` | **identical layout to `T`** — secrets are register/stack scalars; constant-time codegen does not change layout (ADR 0019 D5/D12) |
| `Kont` | opaque `ptr` to a `SentinelKont` (§3) |
| `Task` | opaque `ptr` to a `SentinelTask` (§3) |
| `Enum(id)` (sum type) | `{ i32 tag, ptr payload }` — `tag` is the variant discriminant (source order); `payload` points at a heap-allocated struct of the active variant's payload fields, or is `null` for a unit variant — ADR 0032 D4 |

**Element-type tracking.** `[T]`, `Vec<T>`, and `?T` track their
element/payload `Type` at the `sentinel-types` level (e.g.
`TypedExprKind::ArrayLit` / `Index`, and a `Vec`-typed binding / the
`push` call's `type_args`), not in the LLVM type — LLVM sees only
`{ i64, ptr }` / `{ i64, i64, ptr }` / the opaque `ptr`. Indexing /
`push` recover the element type from the typed AST.
Likewise an `Enum`'s per-variant payload field types live in
`TypedProgram::enums` (`EnumData`/`VariantData`), not the LLVM type —
LLVM sees only `{ i32, ptr }`. Construction (`EnumConstruct`) and `match`
recover the heap payload's struct shape from the variant's declared
payloads (`enum_payload_struct_type` in codegen) to box / GEP it.

---

## 3. Runtime struct layouts (`#[repr(C)]`)

These structs are the runtime↔codegen boundary. Codegen reads/writes
their fields via `getelementptr` at fixed offsets (directly for
`SentinelKont`; opaquely through runtime symbols for the rest), so their
layout is load-bearing ABI. Source of truth: `sentinel-runtime/src/lib.rs`.

### `SentinelKont` — size **32**, align **8**

| field | type | offset |
|-------|------|--------|
| `op_id` | `u32` | 0 |
| `_pad` | `u32` | 4 |
| `arg` | `i64` | 8 |
| `consumed` | `u8` | 16 |
| `_pad2` | `[u8; 7]` | 17 |
| `frames_head` | `*mut SentinelFrame` | 24 |

Codegen reads `op_id` at offset 0 inside the handler dispatch; `arg` at
offset 8. ADR 0020 D7.

### `SentinelFrame` — size **24**, align **8**

| field | type | offset |
|-------|------|--------|
| `resumer` | `extern "C" fn(i64, *mut u8) -> *mut SentinelKont` | 0 |
| `captured` | `*mut u8` | 8 |
| `next` | `*mut SentinelFrame` | 16 |

### `SentinelTask` — size **32**, align **8**

| field | type | offset |
|-------|------|--------|
| `result` | `i64` | 0 |
| `done` | `u32` | 8 |
| `owned` | `u32` | 12 |
| `join_handle_ptr` | `*mut JoinHandleBox` | 16 |
| `args_free_ptr` | `*mut u8` | 24 |

`owned` occupies the former `_pad` slot at offset 12 (ADR 0024 D9) — the
32-byte layout is unchanged from C4.4.

### `SentinelScopeCtx` — size **8**, align **8**

| field | type | offset |
|-------|------|--------|
| `registry_ptr` | `*mut ScopeRegistry` | 0 |

---

## 4. Name mangling

External symbol names for emitted functions. Source of truth:
`mangle_mono_name` / `mangle_type` + the class/impl name builders in
`sentinel-codegen/src/lib.rs`.

- **Free function**: the **bare source name** (e.g. `fn double` → `double`).
- **Monomorphic instance**: `base__<tag>__<tag>…` — the base name, then
  `__` + each type-arg's tag (`mangle_mono_name`).
- **Class init**: `<Class>__init`.
- **Class method**: `<Class>__<method>`.
- **Impl method**: `<Name>__<Type>__<Trait>__<method>`.

**Type tags** (`mangle_type`):

| `Type` | tag |
|--------|-----|
| `i64` / `i32` / `bool` | `i64` / `i32` / `bool` |
| `Struct` / `Class` | the declared name |
| `?T` (`Nullable`) | `opt_<tag(T)>` |
| `[T]` (`Array`) | `arr_<tag(T)>` |
| `Vec<T>` (`Vec`) | `vec_<tag(T)>` |
| `&T` / `&mut T` | `ref_<tag(T)>` / `refmut_<tag(T)>` |
| `GenericInstance` | `<Base>_<tag(arg)>_<tag(arg)>…` |
| `secret T` | `sec_<tag(T)>` |

`secret T` participates in the monomorphisation key, so `id<i64>` and
`id<secret i64>` get **distinct** symbols (`id__i64` vs `id__sec_i64`).

**Known `abi-v2` soft-spot:** the scheme is not length-prefixed, so
exotic identifiers could in principle collide (e.g. `a__b` vs a type tag
producing `a_` + `_b`). No collision exists in the 1.0 single-file
surface (user-chosen identifiers, no separate compilation); a
length-prefixed scheme is a candidate `abi-v2` hardening (ADR 0029 D8).

---

## 5. Runtime-symbol contract (`sentinel_*`)

Codegen declares these as external; `sentinel-runtime` defines them
(`#[no_mangle] extern "C"`). All pointer params/returns are LLVM opaque
`ptr`. Source of truth: the extern declarations in
`sentinel-codegen/src/lib.rs` + the definitions in `sentinel-runtime`.

| symbol | signature | subsystem |
|--------|-----------|-----------|
| `sentinel_print` | `(i64) -> i64` (prints, returns 0) | I/O |
| `sentinel_alloc` | `(i64 size) -> ptr` | heap (libc malloc) |
| `sentinel_free` | `(ptr) -> void` | heap (libc free) |
| `sentinel_realloc` | `(ptr, i64 new_size) -> ptr` | heap (libc realloc) — grows a `Vec<T>` buffer in `push`; `realloc(null, n) == malloc(n)` serves the first push (ADR 0034 D7) |
| `sentinel_str_eq` | `(ptr a, i64 a_len, ptr b, i64 b_len) -> i1` | strings — `[u8]` byte-equality (ADR 0033 D5) |
| `sentinel_panic_oob` | `(i64 idx, i64 len) -> void` | bounds-check trap |
| `sentinel_arena_enter` | `(i64 capacity) -> ptr` | scope arena (ADR 0028) |
| `sentinel_arena_alloc` | `(ptr arena, i64 size) -> ptr` | scope arena |
| `sentinel_arena_exit` | `(ptr arena) -> void` | scope arena |
| `sentinel_perform_op` | `(i32 op_id, i64 arg) -> ptr` | handlers (ADR 0020) |
| `sentinel_kont_resume` | `(ptr kont, i64 value) -> ptr` | handlers |
| `sentinel_kont_pure` | `(i64 value) -> ptr` | handlers |
| `sentinel_kont_consume_pure` | `(ptr kont) -> i64` | handlers |
| `sentinel_kont_push` | `(ptr kont, ptr resumer, ptr captured) -> void` | handlers |
| `sentinel_task_spawn` | `(ptr wrapper, ptr args, i64 args_size) -> ptr` | concurrency (ADR 0024) |
| `sentinel_task_await` | `(ptr task) -> i64` | concurrency |
| `sentinel_scope_enter` | `() -> ptr` | concurrency |
| `sentinel_scope_register` | `(ptr scope, ptr task) -> void` | concurrency |
| `sentinel_scope_exit` | `(ptr scope) -> void` | concurrency |

**Runtime-internal (not codegen-declared):** `sentinel_kont_panic_resumed`
— the runtime's `sentinel_kont_resume` calls it on the consumed-twice
(multi-shot) path; resolved at the runtime crate's own link time, so it
is part of the runtime contract but never an emitted external reference.

The **resumer ABI** (the function pointer stored in `SentinelFrame.resumer`
and passed to `sentinel_kont_push`) is
`extern "C" fn(value: i64, captured: *mut u8) -> *mut SentinelKont`.

---

## 6. Determinism (reproducible builds)

`abi-v1` artifacts are **byte-identical for identical inputs** (ADR 0025
D8, resolved at C5.0). Codegen's `HashMap`s are lookup-only; emission
walks source-ordered `Vec`s; mach-O objects embed no timestamp; LLVM
lowers identical IR identically. Guarded by
`crates/sentinel-driver/tests/repro.rs` (compile-twice + diff).

---

## 7. Stability enforcement (the tests that pin this doc)

A drift in any layout / mangling / symbol must turn a test **red**:

- **Runtime struct layouts (§3)** — `size_of` / `align_of` / `offset_of!`
  asserts in `sentinel-runtime`'s test module
  (`abi_v1_*_layout_is_stable`).
- **Name mangling (§4)** — golden-string asserts on `mangle_type` /
  `mangle_mono_name` in `sentinel-codegen`'s test module
  (`abi_v1_mangling_*`).
- **Runtime-symbol set (§5)** — a `sentinel-runtime` test that takes the
  address of every documented symbol (`abi_v1_runtime_symbol_set`), so a
  rename/removal is a compile error on the definition side.
- **`Type` data layout (§2)** — `abi_v1_type_layouts_via_datalayout`
  (sentinel-codegen) lowers each `Type` via `llvm_basic_type` and asserts
  its size / alignment / struct-field offsets **and field types** through
  the target `DataLayout` (field types pin the order, which equal-sized
  offsets alone cannot). Verified to turn red on a deliberately-introduced
  field reorder, then reverted.
- **Determinism (§6)** — `repro.rs`.

---

## 8. Out of scope (`abi-v2` / post-1.0)

- The separate-compilation **linker + module surface** (`mod`/`use`,
  per-unit objects keyed to `abi-v1`) — ADR 0025 D9, post-1.0. `abi-v1`
  defines the *contract*; it does not ship the units.
- Cross-architecture beyond x86-64/aarch64 (ADR 0025 D12).
- A **C-header generator** / ABI-compatibility checker for external FFI.
- ABI **migration tooling** (`abi-v1`→`v2` shims).
- A **length-prefixed mangling** scheme (§4 soft-spot).
- Arrays-of-secrets layout (would add a `secret`-carrying layout; a
  deferred surface).
