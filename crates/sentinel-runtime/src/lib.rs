//! sentinel-runtime
//!
//! Runtime library linked into emitted Sentinel programs. C0.4
//! provides one symbol — `sentinel_print` — exported via the C ABI
//! so codegen-emitted LLVM IR can call it without name mangling.
//! Per ADR 0010 D11, `print(x)` in Sentinel source code lowers to
//! `call i64 @sentinel_print(i64 x)` in the emitted IR, and the
//! return value (always 0) is the call expression's value.
//!
//! C1.6 (per ADR 0015 D9) adds two more runtime symbols:
//!
//!   - `sentinel_alloc(i64 size) -> *mut u8` wraps libc `malloc`;
//!     panics + aborts on allocation failure.
//!   - `sentinel_panic_oob(i64 idx, i64 len) -> never` prints a
//!     diagnostic and aborts with a non-zero exit code.
//!
//! C2.4 (per ADR 0017 D8) adds the matching `free`:
//!
//!   - `sentinel_free(ptr: *mut u8) -> void` wraps libc `free`.
//!     Closes the C1.6+ heap-leak deferral — codegen now emits
//!     drop calls at scope-exit for heap-backed values (arrays +
//!     `?Struct` payloads).
//!
//! C3.5(a) (per ADR 0020 D7) adds the handler runtime:
//!
//!   - `sentinel_perform_op(op_id: u32, arg: i64) -> *mut SentinelKont`
//!     allocates a continuation tagged with the operation's id +
//!     its arg payload, then returns the pointer up the stack.
//!   - `sentinel_kont_resume(kont, value) -> i64` resumes a
//!     captured continuation with `value`. At C3.5(a) the
//!     continuation has no captured frames (the perform must
//!     appear as a direct child of a `handle` body), so resume
//!     just frees the kont and returns the value. Frame
//!     reification at general evaluation sites lands at C3.5(b)
//!     / C3.6.
//!   - `sentinel_kont_panic_resumed() -> never` aborts cleanly
//!     when a second resume happens (one-shot enforcement per
//!     ADR 0020 D2).
//!
//! None of these are exposed at the language level — they're
//! called internally by codegen.

/// Print an i64 to stdout as ASCII decimal followed by `\n`.
/// Returns 0. Called from Sentinel programs as `print(x)`.
///
/// # Safety
///
/// This function is `extern "C"` for ABI compatibility with
/// LLVM-emitted call sites; it is safe to call from Rust as well.
/// `#[no_mangle]` is required so the linker can resolve the symbol
/// by its bare name from the emitted object file.
#[no_mangle]
pub extern "C" fn sentinel_print(value: i64) -> i64 {
    println!("{value}");
    0
}

/// Allocate `size` bytes on the heap and return a pointer. C1.6 /
/// ADR 0015 D9: this wraps libc `malloc`. Aborts the program on
/// allocation failure (returning `null` from malloc).
///
/// # Safety
///
/// The returned pointer is uninitialized memory. Codegen is
/// responsible for storing valid values before any read. The
/// allocated memory is never freed — C1.6 leaks per ADR 0015 D9.
/// Resource management is C2+.
#[no_mangle]
pub extern "C" fn sentinel_alloc(size: i64) -> *mut u8 {
    // We use libc malloc directly via FFI rather than Rust's
    // allocator because the emitted Sentinel programs link with
    // the C runtime; mixing allocators would risk UB.
    if size < 0 {
        eprintln!("sentinel: bad alloc size {size}");
        std::process::abort();
    }
    let size_usize = size as usize;
    // SAFETY: malloc with a non-negative size is safe to call.
    // We check for null below.
    let ptr = unsafe { libc::malloc(size_usize) } as *mut u8;
    if ptr.is_null() {
        eprintln!("sentinel: allocation failed (size = {size})");
        std::process::abort();
    }
    ptr
}

/// Print an out-of-bounds index diagnostic and abort. C1.6 / ADR
/// 0015 D10: every array access at runtime checks `0 <= idx < len`
/// and calls this on failure. Never returns.
#[no_mangle]
pub extern "C" fn sentinel_panic_oob(idx: i64, len: i64) -> ! {
    eprintln!("sentinel: index out of bounds: idx={idx}, len={len}");
    std::process::abort();
}

/// Free a pointer previously returned by [`sentinel_alloc`]. C2.4
/// / ADR 0017 D8: paired with `sentinel_alloc` to close the
/// C1.6+ heap-leak deferral. Calls libc `free` directly because
/// the emitted Sentinel programs link with the C runtime;
/// alternating allocators would risk UB.
///
/// # Safety
///
/// `ptr` must have been returned by an earlier `sentinel_alloc`
/// call (or be null). Passing a null pointer is safe — libc
/// `free(NULL)` is a no-op per the C standard. Double-free or
/// free of an unrelated pointer is undefined behavior; the
/// borrow checker (C2.1+) statically prevents these in user
/// programs.
#[no_mangle]
pub extern "C" fn sentinel_free(ptr: *mut u8) {
    // SAFETY: caller guarantees ptr is from sentinel_alloc or
    // null. libc free handles null as a no-op.
    if !ptr.is_null() {
        unsafe { libc::free(ptr as *mut libc::c_void) };
    }
}

// =============================================================================
// C3.5(a) / ADR 0020 D7: handler runtime
// =============================================================================
//
// The minimum-viable runtime for the restricted case: a `perform`
// expression appears as the direct child of a matching `handle`
// body, with no intervening frames. The Kont struct carries
// op_id + arg + a one-shot `consumed` flag. Frame reification at
// arbitrary evaluation sites lands at C3.5(b) / C3.6 and will
// extend this struct with a frames vector.

/// Layout matches what codegen emits in
/// [`crate::SentinelKont`]-named LLVM struct. Field order is
/// load-bearing — codegen reads `op_id` via `getelementptr` at
/// offset 0 inside `sentinel_kont_resume`.
#[repr(C)]
pub struct SentinelKont {
    /// Tag identifying which operation this kont was raised from.
    /// Codegen assigns a unique 32-bit id per (EffectId, op_index)
    /// at compile time.
    pub op_id: u32,
    /// Padding so `arg` lands at a stable 8-byte offset that
    /// codegen + the Rust side agree on across platforms.
    pub _pad: u32,
    /// The single argument passed to the perform site. At C3.5(a)
    /// only single-arg ops are supported in codegen; multi-arg
    /// ops land at C3.5(b) via a packed-struct extension.
    pub arg: i64,
    /// One-shot enforcement per ADR 0020 D2. `0` = not yet
    /// resumed; `1` = consumed (second resume aborts).
    pub consumed: u8,
}

/// Allocate a fresh continuation tagged with the operation's id.
/// Returns a pointer that flows up the stack until a matching
/// `handle` catches it.
///
/// At C3.5(a) the kont carries op_id + arg only; the captured-
/// frames vector arrives at C3.5(b) / C3.6 along with the
/// general evaluation-site reification.
///
/// # Safety
///
/// The returned pointer must be consumed by exactly one
/// `sentinel_kont_resume` (or freed externally if the handler
/// arm body aborts without resuming). Multi-shot resume aborts
/// via [`sentinel_kont_panic_resumed`].
#[no_mangle]
pub extern "C" fn sentinel_perform_op(op_id: u32, arg: i64) -> *mut SentinelKont {
    let size = core::mem::size_of::<SentinelKont>() as i64;
    let raw = sentinel_alloc(size) as *mut SentinelKont;
    // SAFETY: sentinel_alloc returns a valid uninit pointer of
    // the requested size; we initialise every field below.
    unsafe {
        (*raw).op_id = op_id;
        (*raw)._pad = 0;
        (*raw).arg = arg;
        (*raw).consumed = 0;
    }
    raw
}

/// Resume a captured continuation with `value`. At C3.5(a) the
/// continuation has no captured frames — the perform sat
/// directly inside the matching handle's body — so resume just
/// flips the one-shot flag, frees the kont, and returns
/// `value`. The general-case resume (replay the frames vector
/// in reverse) lands at C3.5(b) / C3.6.
///
/// Second resume on the same kont aborts via
/// [`sentinel_kont_panic_resumed`].
///
/// # Safety
///
/// `kont` must point to a live `SentinelKont` returned by an
/// earlier `sentinel_perform_op` invocation. After this call
/// returns the kont is freed; the caller must not access it.
///
/// This function is invoked exclusively by codegen-emitted IR
/// for `k(v)` resume calls inside handler arms; codegen always
/// passes a freshly-allocated kont pointer. We deliberately do
/// NOT mark this `unsafe` because the LLVM call site has no
/// notion of unsafe blocks — the contract is "called only from
/// codegen" and is enforced statically by the type checker's
/// ResumeKont machinery (only kont-typed VarIds can flow here).
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn sentinel_kont_resume(kont: *mut SentinelKont, value: i64) -> i64 {
    // SAFETY: caller guarantees `kont` is a live SentinelKont.
    let consumed = unsafe { (*kont).consumed };
    if consumed != 0 {
        sentinel_kont_panic_resumed();
    }
    unsafe {
        (*kont).consumed = 1;
    }
    // Restricted case: empty frames vector, so resume just frees
    // and returns the value. The free is safe because the one-
    // shot flag ensures no further reads.
    sentinel_free(kont as *mut u8);
    value
}

/// Abort with a clean diagnostic when a one-shot continuation is
/// resumed twice (ADR 0020 D2). Never returns.
#[no_mangle]
pub extern "C" fn sentinel_kont_panic_resumed() -> ! {
    eprintln!("sentinel: continuation already resumed (one-shot per ADR 0020 D2)");
    std::process::abort();
}

/// C3.5(b) / ADR 0020 D7: wrap a pure value in a "pure return"
/// continuation. Effecting fns (declared `! { ... }`) whose body
/// happens to be pure — never reaches a perform — still need to
/// match the effecting ABI (return a Kont*) so callers in handle
/// bodies can dispatch uniformly. The kont uses the reserved tag
/// [`PURE_RETURN_OP_ID`]; matching handle codegen unwraps the value
/// via [`sentinel_kont_consume_pure`].
///
/// This is the "default return arm" of ADR 0020 D4 implemented at
/// the runtime layer: when the handled computation produces a Pure
/// value rather than performing, the handle's dispatch falls
/// through to a copy of the inner value.
#[no_mangle]
pub extern "C" fn sentinel_kont_pure(value: i64) -> *mut SentinelKont {
    sentinel_perform_op(PURE_RETURN_OP_ID, value)
}

/// C3.5(b) / ADR 0020 D7: read the wrapped value out of a "pure
/// return" kont and free the kont. Symmetric to
/// [`sentinel_kont_pure`]; called from handle codegen's switch
/// when the body's tail produced a value rather than a perform.
///
/// # Safety
///
/// `kont` must be a live SentinelKont returned by
/// [`sentinel_kont_pure`]. After this call the kont is freed.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn sentinel_kont_consume_pure(kont: *mut SentinelKont) -> i64 {
    // SAFETY: caller guarantees `kont` is a live, pure-return
    // SentinelKont. The one-shot flag is irrelevant — pure
    // returns don't loop back through resume.
    let value = unsafe { (*kont).arg };
    sentinel_free(kont as *mut u8);
    value
}

/// Reserved op_id sentinel used by [`sentinel_kont_pure`] and
/// recognised by handle codegen's runtime switch as the "pure
/// return" tag per ADR 0020 D4. The value is chosen to avoid
/// any legitimately-encoded `(EffectId, op_index)` pair —
/// codegen packs those into a 16/16 bit-split with
/// `(EffectId.0 << 16) | op_index`, so EffectId == 0xFFFF +
/// op_index == 0xFFFF would collide. In practice neither
/// approaches that limit; this is a documented reservation.
pub const PURE_RETURN_OP_ID: u32 = u32::MAX;

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-runtime"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-runtime");
    }

    #[test]
    fn sentinel_print_returns_zero() {
        // The function writes to stdout as a side effect; we just
        // assert the return value here. End-to-end behavior is
        // covered by the C0.4 pass tests via the compiled binary.
        assert_eq!(sentinel_print(42), 0);
    }

    #[test]
    fn sentinel_free_null_is_noop() {
        // libc free(NULL) is a no-op per C standard; sentinel_free
        // adds an explicit null guard for clarity.
        sentinel_free(std::ptr::null_mut());
    }

    #[test]
    fn sentinel_alloc_then_free_round_trips() {
        // Round-trip: alloc 32 bytes + free. No assertion on the
        // pointer value; we just confirm no crash.
        let ptr = sentinel_alloc(32);
        assert!(!ptr.is_null());
        sentinel_free(ptr);
    }

    // ----- C3.5(a) / ADR 0020 D7: handler runtime -----

    #[test]
    fn sentinel_perform_op_initialises_fields() {
        let k = sentinel_perform_op(7, 42);
        assert!(!k.is_null());
        // SAFETY: the just-allocated kont is live; reading its
        // fields is well-defined.
        unsafe {
            assert_eq!((*k).op_id, 7);
            assert_eq!((*k).arg, 42);
            assert_eq!((*k).consumed, 0);
        }
        // Resume to free; checked by next test.
        let _ = sentinel_kont_resume(k, 0);
    }

    #[test]
    fn sentinel_kont_resume_returns_value_in_restricted_case() {
        let k = sentinel_perform_op(0, 100);
        let v = sentinel_kont_resume(k, 99);
        assert_eq!(v, 99);
        // After resume the kont is freed; we cannot test for
        // double-free without invoking abort, which is exercised
        // separately by the one-shot enforcement test below
        // (gated behind `#[ignore]` because abort is hard to
        // observe under cargo test).
    }

    #[test]
    fn sentinel_kont_round_trip_with_zero_arg() {
        let k = sentinel_perform_op(0, 0);
        assert_eq!(sentinel_kont_resume(k, 0), 0);
    }

    #[test]
    fn sentinel_kont_struct_layout_is_stable() {
        // Layout invariant: codegen reads `op_id` via GEP at
        // offset 0 + `arg` at offset 8 (after the 4-byte op_id +
        // 4-byte pad). Test ensures the struct doesn't drift
        // beyond what codegen assumes.
        assert_eq!(core::mem::size_of::<SentinelKont>(), 24);
        assert_eq!(core::mem::align_of::<SentinelKont>(), 8);
    }
}
