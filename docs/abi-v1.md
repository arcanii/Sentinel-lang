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
| `Vec<T>` (`Vec`) | `{ i64 len, i64 cap, ptr data }` — the `[T]` layout with a capacity field inserted, so `data` is **field 2** (offset 16); 24 bytes, align 8. `data` points at a heap buffer of `cap * sizeof(T)` bytes, the first `len` live; grown by `sentinel_realloc` (§5). `String` = `Vec<u8>` (D.2/D.3, shipped — ADR 0033). — ADR 0034 D3 |
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

**ADR 0066 M1.1 — generic `Task<T>` result encoding.** The `result` slot stays
`i64` and `sentinel_task_await` still returns `i64`, but for a `Task<T>` whose
`T` is a non-`i64` **word-sized scalar** (`i32`/`u8`/`bool`/`f64`/`ptr`/`Task`)
the slot carries the value **encoded** as an `i64`, not `T` directly: the
per-spawn wrapper writes `zext` (narrow int) / `bitcast` (`f64`) / `ptrtoint`
(pointer) of the result before signalling `done`, and `.await` applies the
inverse (`trunc` / `bitcast` / `inttoptr`) after reading it back. The struct
layout and the symbol signatures are therefore **unchanged** (no ABI break); the
encode/decode is entirely codegen-side, and `Task<i64>` is the identity case
(byte-identical IR to C4.4). Aggregate / `u128` / `secret` results are not yet
supported (rejected at type-check — ADR 0066 D6 / D8).

### `SentinelScopeCtx` — size **8**, align **8**

| field | type | offset |
|-------|------|--------|
| `registry_ptr` | `*mut ScopeRegistry` | 0 |

### `SentinelChannel` — fully opaque (ADR 0066 M1.2)

Unlike `SentinelTask`, codegen **never reads `SentinelChannel`'s fields** — it
only holds the `*mut SentinelChannel` returned by `sentinel_channel_new` and
passes it back to the other `sentinel_channel_*` symbols. The struct's layout
(an mpsc sender/receiver behind `Mutex`es) is therefore a runtime-internal
detail, **not** ABI, and carries no size/offset contract here.

### `SentinelProcess` — fully opaque (ADR 0066 M2.1)

Like `SentinelChannel`, codegen never reads `SentinelProcess`'s fields — it holds
the `*mut SentinelProcess` returned by `sentinel_process_spawn` (the `Type::Process`
LLVM type is `ptr`) and passes it to `sentinel_process_wait` / `_write` / `_read`.
The struct wraps a `std::process::Child` (runtime-internal, not ABI). The argv ABI
*is* contractual: `sentinel_process_spawn(path_ptr, path_len, argv, argc)` decodes
`argv` as `argc` consecutive abi-v1 array headers `{ i64 len, ptr data }` (§5) — the
`[[u8]]` element buffer codegen emits.

**Byte-pipe IPC (M2.2).** `sentinel_process_spawn` pipes the child's stdin + stdout
(stderr inherited). `sentinel_process_write(p, data, data_len)` writes a `[u8]` to the
child's stdin then **closes** it (EOF — one-shot input); `sentinel_process_read(p,
out_len) -> ptr` reads the child's stdout to EOF and returns a libc-malloc'd `[u8]`
(length in `out_len`), exactly the `sentinel_read_file` result shape. The IPC payload
is `[u8]` — a **public** type — so a `secret` can never cross the process boundary (the
cross-process secret fence, ADR 0066 D8). The fence is structural, enforced by the
type system exactly as the FFI boundary is: the builtin's parameter is `[u8]`, and a
`secret`-tainted array is `[secret u8]` (a distinct type — `secret u8 ≠ u8`, with no
implicit secret→public coercion), so it is rejected at the call. `declassify` remains
the only sanctioned way to send formerly-secret data over a pipe.

**Typed framed channel over the pipe (M2.3).** On top of the raw byte pipes,
`sentinel_process_send(p, value)` frames an `i64` as **8 little-endian bytes** to the
child's stdin and flushes, keeping stdin **open** (multi-message, unlike `_write`'s
one-shot close); `sentinel_process_recv(p, out) -> i64` reads exactly one 8-byte LE
frame from the child's stdout into `*out` and returns the status `0` (a value
arrived) / `1` (closed — EOF or short read), from which codegen builds the `?i64`
exactly as `recv` does (`valid = status == 0`). These are the cross-process twins of
`sentinel_channel_send` / `_recv`. `sentinel_process_wait` closes stdin before
reaping so a loop-until-EOF child terminates (idempotent vs `_write`). The framed
element is the **public** `i64`, so the cross-process secret fence (D8) stays
structural: a `secret i64` argument to `process_send` is rejected as a type mismatch.

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

**Type tags** (`mangle_type`). The table is now **complete over every `Type`
variant** (ADR 0016 A1); both Rust matches (inkwell's and the text oracle's) are
exhaustive, so a new variant cannot land without a row here — and `scg`'s
`cg_mangle_to`, which has no exhaustiveness check, answers `UNKNOWN_KIND_<n>`
rather than guessing, so a missing mirror fails the codegen differential loudly. Every tag is **structural** — derived from the type's
own shape, never from an interner index — which is what lets three independent
back ends (inkwell, the `snc llvm` text oracle, the self-hosted `scg`) derive
the same name. They agree on every row **except `u128` and `SealedChannel`**, and
in neither case because the tag differs: `scg` models no `u128` at all and has no
`SealedChannel` arm in `type_of_typeexpr`, so it resolves either annotation to its
`i64` handle and never reaches the row (register item D20, both pre-existing — pre-A1
the oracle said `ty_U128` / `ty_SealedChannel` against the same `i64`). `SealedChannel`
does agree through a generic fn's mono key, which is how
`tests/pass/c16_mono_key_handle_tags.sentinel` pins it; only the type-annotation route
diverges. Read the table as the contract, and those two rows as aspirational on the
annotation route until `scg` catches up:

| `Type` | tag |
|--------|-----|
| `i64` / `i32` / `bool` / `u8` | `i64` / `i32` / `bool` / `u8` |
| `u128` / `f64` / `ptr` | `u128` / `f64` / `ptr` |
| `Struct` / `Class` / `Enum` | the declared name |
| `?T` (`Nullable`) | `opt_<tag(T)>` |
| `[T]` (`Array`) | `arr_<tag(T)>` |
| `Vec<T>` (`Vec`) | `vec_<tag(T)>` |
| `&T` / `&mut T` | `ref_<tag(T)>` / `refmut_<tag(T)>` |
| `GenericInstance` | `<Base>_<tag(arg)>_<tag(arg)>…` |
| `secret T` | `sec_<tag(T)>` |
| `TypeParam` | `T<index>` |
| `Shared<T>` / `Mutex<T>` / `Guard<T>` | `shared_<tag(T)>` / `mutex_<tag(T)>` / `guard_<tag(T)>` |
| `Channel<T>` / `Task<T>` | `chan_<tag(T)>` / `task_<tag(T)>` |
| `Process` / `SealedChannel` | `process` / `sealedchannel` |
| `Fn<P,R>` | `fn_<tag(P)>_<tag(R)>` |
| `Kont` / `TraitSelf` | `kont_<tag(arg)>_<tag(ret)>` / `Self_<Trait>` |

`secret T` participates in the monomorphisation key, so `id<i64>` and
`id<secret i64>` get **distinct** symbols (`id__i64` vs `id__sec_i64`).

Rows **added or changed by the ADR 0016 A1 amendment**: `u128` / `f64` / `ptr`,
`Enum` (`Struct` and `Class` were already specified and are unchanged), the five
handle rows, `Process` / `SealedChannel`, `Fn`, and `Kont` / `TraitSelf`. All were
previously either unspecified here or inconsistent across the three back ends:
inkwell tagged the handle types by their *interner id* (`shared0`, `task0`, …) and
the text oracle fell back to Rust's `Debug` rendering (`ty_Shared(SharedId(0))`),
which is not a legal unquoted LLVM name at all. Neither form is derivable by an
independent implementation, so neither could be an ABI. (`TypeParam` is documented
here for the first time but was already consistent, so it is a doc addition, not a
change.) The amendment is safe as an amendment rather than an `abi-v2` bump because
**no shipped artifact can contain an old tag**: `mangle_type` reaches these variants
only through a phantom generic type argument or a generic fn's mono key, and no
first-party library, example or fixture instantiated a generic at any of them —
verified by sweeping all 339 corpus programs and diffing the emitted names, not
inferred. The `i64`/`i32`/`bool`/`u8`/named-type/wrapper rows are unchanged, so every
existing symbol is byte-identical.

⚠ **Tags share one flat namespace with user type names** — `struct arr_i64 {}`
produces the tag `arr_i64`, the same tag as `[i64]`, so `Pair<arr_i64, i64>` and
`Pair<[i64], i64>` mangle identically (verified: the text oracle emits the same
`%Pair_arr_i64_i64` name twice, with different layouts). This is **pre-existing
and not closed by A1** — it predates the handle tags and applies to `arr_`,
`opt_`, `vec_`, `ref_`, `sec_` alike. It is the same soft spot as the `__`
ambiguity below, and it has the same fix: both `_` and `$` are legal Sentinel
identifier characters (`[A-Za-z_][A-Za-z0-9_$]*`), so no separator built from
them can be unforgeable. Closing it needs a real encoding — length-prefixing, or
`.`, which LLVM accepts in an unquoted name and Sentinel's lexer cannot
produce — and that changes every existing tag, so it is `abi-v2` work
(cf. ADR 0029 D8). The shipping inkwell back end is unaffected in practice: LLVM
uniquifies a duplicate type or function name, so the emitted code is correct
(verified end-to-end); it is the two *printing* back ends that emit invalid IR.

**Module-qualified symbols (D.6 / ADR 0037 D7 — an `abi-v1` amendment).**
Separate compilation makes every cross-unit symbol part of the ABI, so a
symbol must encode its **module path** unambiguously. The unit of mangling
is `(module_path, item)`, where `item` is the intra-module symbol above (a
free fn's source name, a `mangle_mono_name` instance, a class/impl method):

- **Empty module path** — a single-file program (one module, no path) →
  the **bare `item`, byte-for-byte**. Every single-file artifact's symbols
  are therefore exactly the rows above, **unchanged** by this amendment
  (which is why it is an amendment, not an `abi-v2` bump).
- **Non-empty module path** → `_S` + a length-prefixed segment per
  module-path segment + a length-prefixed `item`, each
  `<decimal-byte-len><bytes>` (Itanium-ish source-name encoding); `item`
  is wrapped as **one** length-prefixed blob. E.g. module `util::math` fn
  `add` → `_S4util4math3add`; module `lex::token` method `Token::new` (item
  `Token__new`) → `_S3lex5token10Token__new`; module `parse` method
  `Token::new` → `_S5parse10Token__new` (distinct module prefix → never
  collides). The encoding is a prefix-free code over `[seg…, item]`, so
  distinct `(module, item)` pairs never collide; decoding is unambiguous
  because no Sentinel identifier (nor any type tag) begins with a digit.
- **Exempt:** the entry module's `main` (the C entry) and the `sentinel_*`
  runtime symbols are never module-qualified. **`_S` is reserved** for
  Sentinel-emitted symbols (like `sentinel_*`); user code must not rely on
  a `_S*` name surviving.

Source of truth: `mangle_qualified` in `sentinel-codegen/src/lib.rs`
(realised per unit by the separate-compilation back end, ADR 0037 (a);
single-file / Path-A builds are the empty-module-path case, so they emit
the rows above verbatim). A cross-module **type** carries no symbol (units
agree on layout — ADR 0037 D4), so only fns / methods are module-qualified.

**Cross-unit generic dedup (`linkonce_odr`, ADR 0037 (2/N)).** A mono
instance of an IMPORTED generic fn is emitted under the **origin**-qualified
symbol (the defining module's path, not the importer's) with `linkonce_odr`
linkage, so N importers of `util::id<i64>` share ONE `_S4util4math…id__i64`
definition (the linker dedups; an importer-LOCAL generic stays
importer-qualified with default linkage). The dedup is gated to
**collision-safe** type args, where the mono-key tag is globally unambiguous:
- a **primitive** (`i64` / `i32` / `bool` / `u8`);
- a **cross-module struct or enum** whose origin is known — its tag is then
  **origin-qualified** as `<seg>$…$<Name>` (`$`-joined module path), so
  `id<util::geo::Point>` → `id__util$geo$Point` and a same-named `Point` from
  another module gets a distinct tag (`mangle_type_dedup` /
  `mangle_mono_name_dedup`, keyed by a driver-supplied `StructId`/`EnumId →
  origin` map);

  ⚠ **The dedup GATE — not the separator — is what makes this key unambiguous.**
  Do not widen `mono_args_dedup_safe` on the assumption that the separator
  distinguishes origins. Making the encoding genuinely unambiguous is `abi-v2`
  work (see the type-tag warning in §4). The detailed rationale is tracked
  privately with the maintainer (register D19) — ask before changing this;
- an array / nullable / vec of a safe element (recursively).

A **local** struct/enum (importer-specific, no shared origin) or any **other**
named type (class / generic instance) is NOT collision-safe and keeps the
sound importer-qualified per-unit emission. So `linkonce_odr` dedup now covers
primitives + cross-module **structs and enums**; **class / generic-instance
args + trait/class-method dedup remain the deferred tail.**

**Intra-module `__` soft-spot:** the *item* scheme is not length-prefixed, so a
generated monomorphisation name and a user item can be spelled identically
within a module (e.g. `a__b` vs a type tag producing `a_` + `_b`).

⚠ **This paragraph used to say "no collision exists in the 1.0 surface
(user-chosen identifiers)" and that the item "never affects *cross-module*
uniqueness". Both are FALSE and are corrected here rather than removed, because
a spec that tells an implementer this soft-spot is harmless is worse than one
that says nothing.** Ordinary user-chosen identifiers reach it, and while the
two definitions are intra-module *in source* they are emitted into different
*objects* under `--separate`, where the linker is the only arbiter. Fully
length-prefixing the item is `abi-v2` hardening (ADR 0029 D8). Details are
tracked privately with the maintainer (register D19).

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
| `sentinel_read_file` | `(ptr path, i64 path_len, ptr out_len) -> ptr data` | file I/O — read a whole file into a fresh `[u8]` (data returned, byte count written to `*out_len`); aborts on failure (ADR 0035 D4/D6) |
| `sentinel_write_file` | `(ptr path, i64 path_len, ptr data, i64 data_len) -> i64` | file I/O — create/truncate + write a file; aborts on failure (ADR 0035 D4/D6) |
| `sentinel_print_bytes` | `(ptr data, i64 data_len) -> i64` | file I/O — write a `[u8]` to stdout (no added newline; flushes); the byte companion to `sentinel_print` (ADR 0035 D4) |
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
| `sentinel_kont_free` | `(ptr kont) -> void` | handlers — free an abandoned kont (+ its captured frame chain) when an early `return` crosses a `handle` (ADR 0065 D6) |
| `sentinel_task_spawn` | `(ptr wrapper, ptr args, i64 args_size) -> ptr` | concurrency (ADR 0024) |
| `sentinel_task_await` | `(ptr task) -> i64` | concurrency |
| `sentinel_scope_enter` | `() -> ptr` | concurrency |
| `sentinel_scope_register` | `(ptr scope, ptr task) -> void` | concurrency |
| `sentinel_scope_exit` | `(ptr scope) -> void` | concurrency |
| `sentinel_channel_new` | `() -> ptr` | channels (ADR 0066 M1.2) — a new mpsc `Channel<i64>` |
| `sentinel_channel_send` | `(ptr ch, i64 value) -> i64` | channels — move `value` in; 0 ok / -1 closed |
| `sentinel_channel_recv` | `(ptr ch, ptr out) -> i64` | channels — block; writes `*out`, returns 0 (some) / 1 (closed). Codegen builds the `?i64` |
| `sentinel_channel_close` | `(ptr ch) -> i64` | channels — drop the sender (signals recv EOF) |
| `sentinel_process_spawn` | `(ptr path, i64 path_len, ptr argv, i64 argc) -> ptr` | subprocess (ADR 0066 M2.1) — spawn `path` with `argc` `[u8]` args (`argv` = abi-v1 array headers); null on failure |
| `sentinel_process_wait` | `(ptr p) -> i64` | subprocess — wait; exit code, or -1 (error / no code / already waited) |
| `sentinel_process_write` | `(ptr p, ptr data, i64 data_len) -> i64` | subprocess IPC (ADR 0066 M2.2) — write `[u8]` to child stdin + close it; 0 ok, -1 error |
| `sentinel_process_read` | `(ptr p, ptr out_len) -> ptr` | subprocess IPC — read child stdout to EOF; libc-malloc'd `[u8]` (len → `out_len`) |
| `sentinel_process_send` | `(ptr p, i64 value) -> i64` | typed framed IPC (ADR 0066 M2.3) — frame `value` (8-byte LE) to child stdin, keep open; 0 ok, -1 error |
| `sentinel_process_recv` | `(ptr p, ptr out) -> i64` | typed framed IPC — read one 8-byte LE i64 frame from child stdout; writes `*out`, returns 0 (some) / 1 (closed). Codegen builds the `?i64` |
| `sentinel_shared_new` | `(i64 value) -> ptr` | shared ownership (ADR 0071 M1.4a) — a new refcounted `Shared<T>` cell (rc 1) holding the word-scalar `value`; the first handle that is freed (not leaked) |
| `sentinel_shared_clone` | `(ptr s) -> ptr` | shared ownership — register a new owner (rc++), returns the same ptr; emitted at each duplication of a named `Shared` binding |
| `sentinel_shared_get` | `(ptr s) -> i64` | shared ownership — read the shared value out (immutable at M1.4a; mutation is `Mutex<T>`, M1.4b) |
| `sentinel_shared_release` | `(ptr s) -> void` | shared ownership — drop one owner (rc--), free the cell at zero; emitted at each `Shared` binding's scope-exit drop |
| `sentinel_mutex_new` | `(i64 value) -> ptr` | mutex (ADR 0071 M1.4b) — a new refcounted, lock-protected `Mutex<T>` cell (rc 1, unlocked) holding the word-scalar `value`; reuses the `Shared` co-ownership pattern (freed on the last drop) |
| `sentinel_mutex_clone` | `(ptr m) -> ptr` | mutex — register a new owner (rc++), returns the same ptr; emitted at each duplication of a named `Mutex` binding (mirrors `sentinel_shared_clone`) |
| `sentinel_mutex_release` | `(ptr m) -> void` | mutex — drop one owner (rc--), free the cell at zero; emitted at each `Mutex` binding's scope-exit drop (mirrors `sentinel_shared_release`) |
| `sentinel_mutex_lock` | `(ptr m, ptr out) -> i64` | mutex — acquire with the always-on `LockTimeout` deadline (D5); writes a `*mut i64` to the protected slot into `*out`, returns 0 (acquired) / 1 (timeout, or null `m`/`out`). Codegen builds the `?Guard` like `recv`'s `?i64` |
| `sentinel_mutex_try_lock_for` | `(ptr m, i64 timeout_nanos, ptr out) -> i64` | mutex — bounded acquire (`timeout_nanos ≤ 0` = a non-blocking `try_lock`); same `(status, out-ptr)` contract as `sentinel_mutex_lock` |
| `sentinel_mutex_unlock` | `(ptr m) -> void` | mutex — release the lock held by a prior `lock`/`try_lock_for` (the `Guard`'s scope-exit drop); no refcount change |
| `sentinel_mutex_data` | `(ptr m, i64 valid) -> ptr` | mutex — the protected slot for `*g` reads/writes (`valid` = the `?Guard`'s valid bit). ABORTS on a timed-out/null guard (`valid == 0`) — a deref without the lock would be a data race; the `sentinel_panic_oob` posture. Only sound while the guard's lock is held |

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
  `mangle_mono_name` / `mangle_qualified` (the module-qualified D7 scheme)
  in `sentinel-codegen`'s test module (`abi_v1_mangling_*`, incl.
  `abi_v1_mangling_qualified_is_stable`).
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

- The separate-compilation **linker** for true per-unit objects — **in
  progress** (ADR 0037 (a)). The `use`/`pub` module **surface** shipped
  (D.6), and the **module-qualified mangling** that keys cross-unit symbols
  is now **§4** (an `abi-v1` amendment). Per-unit `linkonce_odr` generic
  dedup is now **partially realised** (§4 — collision-safe primitive-arg
  instances; named-type args await the module-qualified type tag). Still out
  of scope here: incremental caching (ADR 0037 (3/N)).
- Cross-architecture beyond x86-64/aarch64 (ADR 0025 D12).
- An ABI-compatibility checker for external FFI. (The **C-header generator** —
  `snc --emit-header` — has since shipped: ADR 0059.)
- ABI **migration tooling** (`abi-v1`→`v2` shims).
- **Fully** length-prefixing the *item* portion of a symbol (the §4
  intra-module soft-spot). The module path is already length-prefixed (§4).
- (Arrays-of-secrets — `[secret T]` — have since shipped, ADR 0047; they reuse
  the `[T]` layout because `secret T` is layout-identical to `T`, so no `abi-v1`
  change was needed.)
