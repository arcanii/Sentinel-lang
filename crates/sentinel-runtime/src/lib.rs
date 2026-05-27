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
}
