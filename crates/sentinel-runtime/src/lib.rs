//! sentinel-runtime
//!
//! Runtime library linked into emitted Sentinel programs. C0.4
//! provides one symbol — `sentinel_print` — exported via the C ABI
//! so codegen-emitted LLVM IR can call it without name mangling.
//! Per ADR 0010 D11, `print(x)` in Sentinel source code lowers to
//! `call i64 @sentinel_print(i64 x)` in the emitted IR, and the
//! return value (always 0) is the call expression's value.
//!
//! Later sub-phases will grow this crate with more runtime
//! primitives (allocation, panic handler, eventually broker
//! integration at C5).

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
}
