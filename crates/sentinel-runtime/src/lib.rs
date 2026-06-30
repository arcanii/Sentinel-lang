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
//!   - `sentinel_kont_resume(kont, value) -> *mut SentinelKont`
//!     resumes a captured continuation with `value`. C3.5(e)
//!     widens the return type to a kont* so a resumer that
//!     itself performs can bubble the new perform back to the
//!     enclosing handler (deep-handler re-wrap per ADR 0020 D3).
//!     The caller branches on `op_id`: `PURE_RETURN_OP_ID` means
//!     unwrap to the final i64, anything else means re-dispatch.
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

/// Release a byte buffer a Sentinel `export "C"` function returned to C. ADR
/// 0059 Phase 1b (A7): the public ownership contract for the owned-`[u8]` return
/// ABI — an exported `fn … -> [u8]` hands the C caller a heap buffer (via the
/// `(uint8_t** out_data, int64_t* out_len)` out-params), and the caller frees it
/// with this. It is a thin, deliberately-named alias for [`sentinel_free`] (the
/// returned buffer is `sentinel_alloc`'d, so it is `free`-able): C consumers of a
/// Sentinel library see one clear free symbol and need know nothing about the
/// `sentinel_alloc`/`free` internals. The header generator emits its prototype
/// (`void sentinel_free_bytes(uint8_t* data);`) whenever an export returns bytes.
///
/// # Safety
///
/// `data` must be a pointer an `export "C"` function wrote to `*out_data` (or be
/// null — a no-op). Double-free or free of an unrelated pointer is undefined
/// behavior, exactly as for `sentinel_free`/libc `free`.
#[no_mangle]
pub extern "C" fn sentinel_free_bytes(data: *mut u8) {
    sentinel_free(data);
}

/// Resize a heap buffer previously returned by [`sentinel_alloc`] /
/// [`sentinel_realloc`] (or null) to `new_size` bytes, preserving its
/// contents up to the smaller of the old and new sizes. D.3 / ADR 0034
/// D7: the one new runtime symbol backing growable `Vec<T>` — `push`
/// calls it to grow the data buffer (`realloc` to `max(1, cap*2) *
/// sizeof(T)` bytes) when `len == cap`. Wraps libc `realloc` directly
/// for the same reason as `sentinel_alloc`/`free`: the emitted programs
/// link with the C runtime, so the buffer must be realloc/free-able by
/// the same allocator.
///
/// `realloc(NULL, n)` is defined to behave as `malloc(n)`, so this also
/// serves the *first* `push` (an empty `Vec` has `data == null`, `cap ==
/// 0`): codegen grows `0 -> max(1, 0) == 1` and calls this with a null
/// `ptr`. Aborts on allocation failure or a negative size, matching
/// [`sentinel_alloc`].
///
/// # Safety
///
/// `ptr` must have been returned by an earlier `sentinel_alloc` /
/// `sentinel_realloc` (or be null), and not since freed. The borrow
/// checker keeps the owning `Vec` live across the call.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_realloc(ptr: *mut u8, new_size: i64) -> *mut u8 {
    if new_size < 0 {
        eprintln!("sentinel: bad realloc size {new_size}");
        std::process::abort();
    }
    // SAFETY: ptr is from our allocator or null (realloc(NULL, n) ==
    // malloc(n)); new_size is non-negative. We check for null below.
    let new_ptr = unsafe { libc::realloc(ptr as *mut libc::c_void, new_size as usize) } as *mut u8;
    if new_ptr.is_null() && new_size != 0 {
        eprintln!("sentinel: reallocation failed (new_size = {new_size})");
        std::process::abort();
    }
    new_ptr
}

/// Build an `OsString` path from a length-prefixed Sentinel `[u8]`.
/// D.4 / ADR 0035 D4: Sentinel paths are byte arrays (not NUL-
/// terminated). Aborts on an embedded NUL (a path can't contain one;
/// defensive, not a security model). Portable (ADR 0060 Phase 1): on Unix,
/// `OsStrExt::from_bytes` takes raw bytes, so non-UTF-8 paths are fine; on
/// Windows, where paths are Unicode and `OsString` has no raw-bytes
/// constructor, the bytes are interpreted as UTF-8 and a non-UTF-8 path
/// aborts (it is not representable) — the same panic-on-bad-input stance
/// as the embedded-NUL case above.
///
/// # Safety
/// When `len > 0`, `ptr` must point to `len` readable bytes.
unsafe fn path_from_bytes(ptr: *const u8, len: i64, op: &str) -> std::ffi::OsString {
    let bytes: &[u8] = if len <= 0 {
        &[]
    } else {
        // SAFETY: caller guarantees `len` readable bytes at `ptr`.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    };
    if bytes.contains(&0) {
        eprintln!("sentinel: {op} failed: path contains an embedded NUL byte");
        std::process::abort();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes).to_os_string()
    }
    #[cfg(windows)]
    {
        match std::str::from_utf8(bytes) {
            Ok(s) => std::ffi::OsString::from(s),
            Err(_) => {
                eprintln!("sentinel: {op} failed: path is not valid UTF-8 (required on Windows)");
                std::process::abort();
            }
        }
    }
}

/// Read the entire file at `path` (a `[u8]`) into a fresh heap buffer
/// and return it as a `[u8]` `{ len, data }`: the data pointer is the
/// return value and the byte count is written to `*out_len`. D.4 / ADR
/// 0035 D4/D6 — `read_file(path) -> [u8]`, a runtime builtin (not an
/// algebraic effect; ADR 0035 D2). The buffer is `sentinel_alloc`'d
/// (libc `malloc`) so the caller's scope-exit drop frees it with
/// `sentinel_free`. **Aborts** on any open/read failure (ADR 0035 D5 —
/// panic-on-failure; a recoverable `?[u8]`/`Result` is deferred).
///
/// # Safety
/// `path` must point to `path_len` readable bytes; `out_len` must point
/// to a writable `i64`. Both are guaranteed by codegen's call shape.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_read_file(
    path: *const u8,
    path_len: i64,
    out_len: *mut i64,
) -> *mut u8 {
    // SAFETY: contract above.
    let os_path = unsafe { path_from_bytes(path, path_len, "read_file") };
    let data = match std::fs::read(&os_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "sentinel: read_file failed: {}: {e}",
                std::path::Path::new(&os_path).display()
            );
            std::process::abort();
        }
    };
    let n = data.len();
    // Copy into a libc-malloc'd buffer so scope-exit drop can free it.
    let buf = sentinel_alloc(n as i64);
    if n > 0 {
        // SAFETY: `buf` has `n` bytes (sentinel_alloc aborts on failure);
        // `data` has `n` bytes; the regions don't overlap.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf, n) };
    }
    // SAFETY: caller passed a writable `i64` slot.
    unsafe { *out_len = n as i64 };
    buf
}

/// Write all `data_len` bytes of `data` (a `[u8]`) to the file at
/// `path`, creating or truncating it. D.4 / ADR 0035 D4 —
/// `write_file(path, data) -> i64` (returns 0). **Aborts** on any
/// open/write failure (ADR 0035 D5). `data` is borrowed (the ADR 0033
/// A3 runtime-builtin rule), not freed.
///
/// # Safety
/// `path` / `data` must point to `path_len` / `data_len` readable bytes
/// (guaranteed by codegen's call shape).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_write_file(
    path: *const u8,
    path_len: i64,
    data: *const u8,
    data_len: i64,
) -> i64 {
    // SAFETY: contract above.
    let os_path = unsafe { path_from_bytes(path, path_len, "write_file") };
    let bytes: &[u8] = if data_len <= 0 {
        &[]
    } else {
        // SAFETY: caller guarantees `data_len` readable bytes at `data`.
        unsafe { std::slice::from_raw_parts(data, data_len as usize) }
    };
    if let Err(e) = std::fs::write(&os_path, bytes) {
        eprintln!(
            "sentinel: write_file failed: {}: {e}",
            std::path::Path::new(&os_path).display()
        );
        std::process::abort();
    }
    0
}

/// Write `data`'s bytes to stdout — the byte/string companion to
/// `print` (which writes one i64). D.4 (2/N) / ADR 0035 D4 —
/// `print_bytes(data) -> i64` (returns 0). Writes exactly `data_len`
/// bytes with **no added newline** (the caller includes any); flushes so
/// the bytes are visible before the program exits via its C-ABI `main`
/// return (and interleave correctly with `print`'s `println!`, which
/// shares this stdout). Aborts on a write error.
///
/// # Safety
/// When `data_len > 0`, `data` must point to `data_len` readable bytes
/// (guaranteed by codegen's `{ len, ptr }` call shape).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_print_bytes(data: *const u8, data_len: i64) -> i64 {
    use std::io::Write;
    let bytes: &[u8] = if data_len <= 0 {
        &[]
    } else {
        // SAFETY: caller guarantees `data_len` readable bytes at `data`.
        unsafe { std::slice::from_raw_parts(data, data_len as usize) }
    };
    let mut out = std::io::stdout();
    if out.write_all(bytes).and_then(|()| out.flush()).is_err() {
        eprintln!("sentinel: print_bytes failed: stdout write error");
        std::process::abort();
    }
    0
}

// ---- ADR 0056: a TCP sockets runtime surface ---------------------------------------
//
// The runtime primitives a `std/net` library would wrap as builtins so a Sentinel
// program — e.g. the `std::net::ssh` transport — can run over a REAL connection. They
// follow the file-I/O ABI: byte buffers are `(ptr, len)`, a handle is an opaque `i64`
// (the OS file descriptor). Per ADR 0056 they RETURN a negative `i64` on failure
// instead of aborting (a daemon must survive a dropped connection — the deviation from
// file I/O's panic-on-failure). Blocking sockets; a server spawns one OS-thread task
// per connection (`sentinel_task_spawn` already uses `std::thread`). Unix-only (raw
// libc sockets); the build target is macOS / Linux. NOT yet wired as compiler builtins
// (that needs the FnId registration + the self-hosted-`scg` builtin-table mirror).

/// Create a TCP listener bound to 127.0.0.1:`port` (port 0 = an OS-assigned ephemeral
/// port — query it with `sentinel_tcp_local_port`). Returns the listener handle (fd)
/// ≥ 0, or -1 on error.
#[cfg(unix)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_listen(port: i64) -> i64 {
    // SAFETY: a fixed sequence of libc socket calls on a freshly created fd; the
    // sockaddr is fully initialized (zeroed + the three set fields) before `bind`.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return -1;
        }
        let yes: libc::c_int = 1;
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &yes as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = (port as u16).to_be();
        addr.sin_addr.s_addr = 0x7f00_0001u32.to_be(); // 127.0.0.1, network byte order
        let alen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        if libc::bind(fd, &addr as *const libc::sockaddr_in as *const libc::sockaddr, alen) < 0 {
            libc::close(fd);
            return -1;
        }
        if libc::listen(fd, 16) < 0 {
            libc::close(fd);
            return -1;
        }
        i64::from(fd)
    }
}

/// The local port a listener / connected socket is bound to — for resolving an
/// ephemeral (`tcp_listen(0)`) port. Returns the port (1..=65535), or -1 on error.
#[cfg(unix)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_local_port(handle: i64) -> i64 {
    // SAFETY: `getsockname` writes into the zeroed sockaddr / socklen; handle is an fd.
    unsafe {
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        let mut alen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        if libc::getsockname(
            handle as libc::c_int,
            &mut addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
            &mut alen,
        ) < 0
        {
            return -1;
        }
        i64::from(u16::from_be(addr.sin_port))
    }
}

/// Block for an incoming connection on a listener. Returns a connection handle ≥ 0, or
/// -1 on error.
#[cfg(unix)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_accept(listener: i64) -> i64 {
    // SAFETY: `accept` on a listener fd; null addr/len means "don't report the peer".
    let c = unsafe {
        libc::accept(listener as libc::c_int, std::ptr::null_mut(), std::ptr::null_mut())
    };
    if c < 0 {
        -1
    } else {
        i64::from(c)
    }
}

/// Connect to `host` (a dotted-quad IPv4 string, e.g. `"127.0.0.1"`) on `port`.
/// Returns a connection handle ≥ 0, or -1 on error (no DNS — IPv4 literals only).
#[cfg(unix)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_connect(host: *const u8, host_len: i64, port: i64) -> i64 {
    if host_len <= 0 {
        return -1;
    }
    // SAFETY: caller guarantees `host_len` readable bytes at `host`.
    let bytes = unsafe { std::slice::from_raw_parts(host, host_len as usize) };
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return -1;
    }
    let mut octs = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        match p.parse::<u8>() {
            Ok(v) => octs[i] = v,
            Err(_) => return -1,
        }
    }
    // SAFETY: a fixed libc socket+connect sequence on a freshly created fd.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return -1;
        }
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = (port as u16).to_be();
        addr.sin_addr.s_addr = u32::from_be_bytes(octs).to_be();
        let alen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        if libc::connect(fd, &addr as *const libc::sockaddr_in as *const libc::sockaddr, alen) < 0 {
            libc::close(fd);
            return -1;
        }
        i64::from(fd)
    }
}

/// Block to read up to `max` bytes from a connection into a fresh `sentinel_alloc`'d
/// buffer (the caller's scope-exit drop frees it, like `read_file`). `*out_len` is set
/// to the bytes read (0 = the peer closed / EOF), or -1 on error.
#[cfg(unix)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_read(conn: i64, max: i64, out_len: *mut i64) -> *mut u8 {
    let cap = if max <= 0 { 1 } else { max };
    let buf = sentinel_alloc(cap);
    // SAFETY: `buf` has `cap` writable bytes; `out_len` is a writable i64 slot.
    let n = unsafe { libc::read(conn as libc::c_int, buf as *mut libc::c_void, cap as usize) };
    unsafe {
        *out_len = if n < 0 { -1 } else { n as i64 };
    }
    buf
}

/// Write all `data_len` bytes of `data` to a connection. Returns the bytes written, or
/// -1 on error.
#[cfg(unix)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_write(conn: i64, data: *const u8, data_len: i64) -> i64 {
    if data_len <= 0 {
        return 0;
    }
    // SAFETY: caller guarantees `data_len` readable bytes at `data`.
    let bytes = unsafe { std::slice::from_raw_parts(data, data_len as usize) };
    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: `bytes[off..]` is a valid sub-slice of the caller's buffer.
        let n = unsafe {
            libc::write(
                conn as libc::c_int,
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n <= 0 {
            return -1;
        }
        off += n as usize;
    }
    off as i64
}

/// Close a listener or connection handle. Returns 0, or -1 on error.
#[cfg(unix)]
#[no_mangle]
pub extern "C" fn sentinel_tcp_close(handle: i64) -> i64 {
    // SAFETY: `close` on an fd; double-close is reported as -1, not UB.
    if unsafe { libc::close(handle as libc::c_int) } < 0 {
        -1
    } else {
        0
    }
}

/// Byte-wise equality of two `[u8]` slices — the `str_eq` builtin
/// (D.2 / ADR 0033 D5; the lexer's keyword/identifier matcher). Equal
/// length AND equal bytes. Returns a C `bool`, which Rust's `extern
/// "C"` ABI lowers to `i1 zeroext` — matching codegen's `i1`-returning
/// declaration (the result is used directly as a Sentinel `bool`).
///
/// # Safety
///
/// When the corresponding length is `> 0`, `a` / `b` must point to at
/// least `a_len` / `b_len` readable bytes — guaranteed by the `{ i64
/// len, ptr data }` array representation of any live `[u8]`, whose
/// backing buffer the borrow checker keeps alive across the call. A
/// null pointer is only valid alongside a zero length.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_str_eq(a: *const u8, a_len: i64, b: *const u8, b_len: i64) -> bool {
    if a_len != b_len {
        return false;
    }
    if a_len <= 0 {
        // Equal lengths that are empty (or defensively negative) — two
        // zero-length slices are equal and there is nothing to read.
        return true;
    }
    let n = a_len as usize;
    // SAFETY: lengths are equal and positive, so both pointers are
    // non-null and readable for `n` bytes per the contract above.
    let a_slice = unsafe { std::slice::from_raw_parts(a, n) };
    let b_slice = unsafe { std::slice::from_raw_parts(b, n) };
    a_slice == b_slice
}

// ============================================================================
// C5.4 / ADR 0028: broker-backed scope arenas (the substrate)
// ============================================================================
//
// A process-wide Phase A `Broker` hosts one bump arena per Sentinel scope.
// These C-ABI entry points are *additive* at C5.4 (1/N): codegen still
// emits `sentinel_alloc` / `sentinel_free` (libc) and does NOT call these
// yet, so compiled programs are byte-identical (the ADR 0026 c51 bar).
// C5.4 (2/N) wires codegen to route a scope's non-escaping heap
// allocations (those the borrow-check `DropPlan` frees at that scope exit,
// hence provably non-escaping) into the scope's arena, replacing the N
// per-binding `sentinel_free` calls with one `sentinel_arena_exit` —
// a bump arena reclaims everything at once.

use sentinel_broker::{ArenaHandle, Broker};
use std::sync::OnceLock;

/// Per-scope arena capacity used when codegen passes a non-positive hint.
/// Generous so the substrate's tests + small programs fit; C5.4 (2/N) will
/// size each arena from its scope's actual needs.
const DEFAULT_ARENA_CAPACITY: usize = 1 << 20; // 1 MiB

/// The process-wide broker. Lazily created on first arena use, so programs
/// that never touch the arena path pay nothing.
fn broker() -> &'static Broker {
    static BROKER: OnceLock<Broker> = OnceLock::new();
    BROKER.get_or_init(Broker::new)
}

/// Enter a scope arena: create a bump arena of `capacity` bytes (a
/// non-positive `capacity` uses [`DEFAULT_ARENA_CAPACITY`]) and return an
/// opaque handle to pass to [`sentinel_arena_alloc`] / [`sentinel_arena_exit`].
#[no_mangle]
pub extern "C" fn sentinel_arena_enter(capacity: i64) -> *mut core::ffi::c_void {
    let cap = if capacity <= 0 {
        DEFAULT_ARENA_CAPACITY
    } else {
        capacity as usize
    };
    let handle = broker().create_arena("scope", cap);
    Box::into_raw(Box::new(handle)) as *mut core::ffi::c_void
}

/// Bump-allocate `size` bytes (16-byte aligned, matching libc malloc's
/// guarantee) in the arena. Aborts on a null arena, a negative size, or
/// arena exhaustion — a scope's bump arena has a fixed capacity and does
/// not grow.
///
/// # Safety
/// `arena` must be a live handle from [`sentinel_arena_enter`] that has not
/// yet been passed to [`sentinel_arena_exit`].
#[no_mangle]
pub extern "C" fn sentinel_arena_alloc(arena: *mut core::ffi::c_void, size: i64) -> *mut u8 {
    if arena.is_null() {
        eprintln!("sentinel: arena_alloc on a null arena");
        std::process::abort();
    }
    if size < 0 {
        eprintln!("sentinel: bad arena_alloc size {size}");
        std::process::abort();
    }
    // SAFETY: caller guarantees `arena` is a live handle from
    // sentinel_arena_enter (not yet exited); we only borrow it.
    let handle = unsafe { &*(arena as *const ArenaHandle) };
    let layout = match std::alloc::Layout::from_size_align(size as usize, 16) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("sentinel: bad arena layout (size = {size})");
            std::process::abort();
        }
    };
    match handle.alloc_bytes(layout) {
        Ok(p) => p.as_ptr(),
        Err(e) => {
            eprintln!("sentinel: arena allocation failed (size = {size}): {e}");
            std::process::abort();
        }
    }
}

/// Exit a scope arena: destroy it, bulk-freeing every allocation made in
/// it, and invalidate the handle. A null handle is a no-op.
///
/// # Safety
/// `arena` must be a handle from [`sentinel_arena_enter`] that is exited at
/// most once, and no pointer from [`sentinel_arena_alloc`] on it may be
/// used afterwards.
#[no_mangle]
pub extern "C" fn sentinel_arena_exit(arena: *mut core::ffi::c_void) {
    if arena.is_null() {
        return;
    }
    // SAFETY: caller guarantees `arena` came from sentinel_arena_enter and
    // is exited at most once; reclaim the Box.
    let handle = unsafe { Box::from_raw(arena as *mut ArenaHandle) };
    // Drop the broker's registered Arc, then the handle's Arc; the last
    // drop runs BumpStrategy::drop, freeing the backing buffer.
    let _ = broker().destroy_arena(handle.id());
    drop(handle);
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
    /// Padding so `frames_head` lands at a stable 8-byte offset.
    pub _pad2: [u8; 7],
    /// C3.5(c) / ADR 0020 D7: linked-list head of captured
    /// evaluation frames. NULL when no frames have been pushed
    /// (the C3.5(a)/(b) cases where `perform` is at tail
    /// position with no surrounding context to reify). The list
    /// is ordered innermost-first (head = most-recently-pushed
    /// = closest to the perform site); replay walks head → tail
    /// to evaluate frames in the order their original
    /// computation would have run.
    pub frames_head: *mut SentinelFrame,
}

/// C3.5(c) / ADR 0020 D7: a captured evaluation frame. Created
/// at codegen-emitted `sentinel_kont_push` sites — each "could-
/// be-captured" eval-frame position (let-body, in C3.5(c); more
/// at follow-on sub-phases) emits one. `resumer` is a per-site
/// function the compiler generated; `captured` is the heap-
/// allocated struct holding the values of in-scope variables
/// the resumer needs to re-evaluate its tail; `next` chains
/// outwards from the perform site towards the handle.
///
/// Resumer ABI: takes the resumed value (i64) + the captured
/// state pointer (opaque), returns a `*mut SentinelKont` so the
/// resumer body can itself perform if needed. At C3.5(c) MVP
/// the only resumers we emit are non-performing; they wrap their
/// result via [`sentinel_kont_pure`].
#[repr(C)]
pub struct SentinelFrame {
    pub resumer:
        unsafe extern "C" fn(value: i64, captured: *mut u8) -> *mut SentinelKont,
    pub captured: *mut u8,
    pub next: *mut SentinelFrame,
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
        (*raw)._pad2 = [0; 7];
        (*raw).frames_head = core::ptr::null_mut();
    }
    raw
}

/// C3.5(c) / ADR 0020 D7: push a captured evaluation frame onto
/// the kont's chain. Called from codegen-emitted code at every
/// "could-be-captured" evaluation site (let-stmts at C3.5(c);
/// binops / if-cond / index / etc. at later sub-phases). The
/// `resumer` is a free function the compiler emitted to re-
/// evaluate the surrounding context given the resumed value;
/// `captured` is a heap-allocated struct holding any in-scope
/// values the resumer body references.
///
/// # Safety
///
/// `kont` must point to a live SentinelKont. The (`resumer`,
/// `captured`) pair must match: the resumer must read its
/// captured state through the pointer it expects. Memory
/// management: this fn takes ownership of `captured`. The
/// matching `sentinel_kont_resume` frees the captured pointer
/// after the resumer returns.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn sentinel_kont_push(
    kont: *mut SentinelKont,
    resumer: unsafe extern "C" fn(value: i64, captured: *mut u8) -> *mut SentinelKont,
    captured: *mut u8,
) {
    let frame_size = core::mem::size_of::<SentinelFrame>() as i64;
    let frame = sentinel_alloc(frame_size) as *mut SentinelFrame;
    // SAFETY: sentinel_alloc returns valid uninit memory of the
    // requested size; we initialise every field below.
    unsafe {
        (*frame).resumer = resumer;
        (*frame).captured = captured;
        (*frame).next = (*kont).frames_head;
        (*kont).frames_head = frame;
    }
}

/// Resume a captured continuation with `value`. Walks the kont's
/// frame chain in head→tail order — head is the most-recently-
/// pushed (innermost) frame, which is what would have run first
/// in the original execution. Each frame's resumer is called
/// with the current value + its captured state; the resumer
/// returns a *mut SentinelKont (either a pure-return wrap or an
/// op-perform kont for nested handlers).
///
/// **Return type**: `*mut SentinelKont`. The caller must inspect
/// the result's `op_id` to decide what to do next:
///   - `PURE_RETURN_OP_ID`: the chain drained without any
///     intermediate perform; unwrap with
///     [`sentinel_kont_consume_pure`] to recover the final value.
///   - Any other op id: a resumer in the chain performed; the
///     result kont is a fresh op-perform with the original kont's
///     remaining frames spliced onto its chain tail. Caller's
///     enclosing `handle` site re-dispatches per ADR 0020 D3's
///     deep-handler semantics.
///
/// C3.5(e) / ADR 0020 D7: this is the bubble-aware variant. At
/// C3.5(c)/(d) the runtime assumed every resumer returned a
/// pure-return wrap; C3.5(e) lifts that restriction so chained
/// effecting lets (a `let v = perform Op()` inside a resumer
/// body) work end-to-end.
///
/// Second resume on the same kont aborts via
/// [`sentinel_kont_panic_resumed`].
///
/// # Safety
///
/// `kont` must point to a live `SentinelKont` returned by an
/// earlier `sentinel_perform_op` invocation. After this call
/// returns the original `kont` is freed; the caller must not
/// access it. The returned pointer is a *different* live kont
/// that the caller now owns.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn sentinel_kont_resume(
    kont: *mut SentinelKont,
    value: i64,
) -> *mut SentinelKont {
    // SAFETY: caller guarantees `kont` is a live SentinelKont.
    let consumed = unsafe { (*kont).consumed };
    if consumed != 0 {
        sentinel_kont_panic_resumed();
    }
    unsafe {
        (*kont).consumed = 1;
    }

    // Drain the frame chain. Each frame's resumer is called with
    // the accumulated value; if the resumer returns a non-
    // pure-return kont the chain bubbles up and the remaining
    // frames migrate onto the bubble's chain tail (deep-handler
    // re-wrap).
    let mut current_value = value;
    // SAFETY: read frames_head from the live kont.
    let mut current_frame = unsafe { (*kont).frames_head };
    while !current_frame.is_null() {
        // SAFETY: current_frame is non-null and was allocated by
        // sentinel_kont_push from a live kont.
        let (resumer, captured, next) = unsafe {
            ((*current_frame).resumer, (*current_frame).captured, (*current_frame).next)
        };
        // SAFETY: codegen contract — the resumer reads its
        // captured state through `captured` and returns a live
        // SentinelKont.
        let result_kont = unsafe { resumer(current_value, captured) };
        // SAFETY: result_kont is a live kont returned by the
        // resumer; reading its op_id is well-defined.
        let result_op_id = unsafe { (*result_kont).op_id };
        if result_op_id != PURE_RETURN_OP_ID {
            // C3.5(e): the resumer performed. Splice the
            // remaining frames (current_frame.next onwards) onto
            // result_kont's chain tail so they re-run after the
            // bubble's eventual handler resume.
            sentinel_free(captured);
            sentinel_free(current_frame as *mut u8);
            if !next.is_null() {
                // SAFETY: result_kont's frames_head was just
                // initialised by sentinel_perform_op (null) or
                // augmented by codegen-emitted sentinel_kont_push
                // calls inside the resumer.
                let head = unsafe { (*result_kont).frames_head };
                if head.is_null() {
                    unsafe {
                        (*result_kont).frames_head = next;
                    }
                } else {
                    // Walk to the tail of result_kont's chain
                    // and append `next`.
                    let mut tail = head;
                    loop {
                        // SAFETY: tail is non-null on entry and
                        // we break before stepping to null.
                        let next_in_chain = unsafe { (*tail).next };
                        if next_in_chain.is_null() {
                            break;
                        }
                        tail = next_in_chain;
                    }
                    unsafe {
                        (*tail).next = next;
                    }
                }
            }
            sentinel_free(kont as *mut u8);
            return result_kont;
        }
        // Pure return — unwrap and continue.
        // SAFETY: result_kont is a live pure-return kont.
        let unwrapped = unsafe { (*result_kont).arg };
        sentinel_free(result_kont as *mut u8);
        sentinel_free(captured);
        sentinel_free(current_frame as *mut u8);
        current_value = unwrapped;
        current_frame = next;
    }

    sentinel_free(kont as *mut u8);
    // The chain drained without a bubble. Wrap the final value
    // in a pure-return kont so the caller's uniform unwrap-or-
    // bubble check (op_id == PURE_RETURN_OP_ID) succeeds.
    sentinel_kont_pure(current_value)
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

/// ADR 0065 D6: free an ABANDONED continuation — one allocated by a
/// `perform` (or augmented by `sentinel_kont_push`) but never resumed,
/// because an early `return` crossed its `handle` boundary (a handler arm
/// body, or the handled computation, `return`s instead of resuming `k`).
/// Walks `frames_head` freeing each captured-state block + frame node (the
/// inverse of [`sentinel_kont_push`]), then frees the kont itself.
///
/// The **one-free invariant** (ADR 0020 D2 / ADR 0065 D6): a kont is freed
/// exactly once — by [`sentinel_kont_resume`] on the normal path, or by this
/// on the early-return path, which are mutually exclusive (codegen emits this
/// only on a `return` path that did not resume the kont). `sentinel_free` of
/// a null pointer is a safe no-op, so a frameless kont (the direct-`perform`
/// case, `frames_head == null`) frees cleanly with no guard.
///
/// # Safety
///
/// `kont` must be a live `SentinelKont` that has NOT been resumed — calling
/// this on a resumed (already-freed) kont would double-free. The codegen
/// contract guarantees the mutual exclusion.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn sentinel_kont_free(kont: *mut SentinelKont) {
    // SAFETY: caller guarantees `kont` is a live, un-resumed SentinelKont;
    // read its frame-chain head.
    let mut current_frame = unsafe { (*kont).frames_head };
    while !current_frame.is_null() {
        // SAFETY: current_frame is non-null and was allocated by
        // sentinel_kont_push from a live kont; read its captured ptr + next.
        let (captured, next) =
            unsafe { ((*current_frame).captured, (*current_frame).next) };
        sentinel_free(captured);
        sentinel_free(current_frame as *mut u8);
        current_frame = next;
    }
    sentinel_free(kont as *mut u8);
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

// =============================================================================
// C4.4 / ADR 0024 D6 + D7: structured concurrency runtime
// =============================================================================
//
// At C4.4 minimum we ship a thread-per-spawn implementation: each
// `spawn fn(args)` allocates a Task struct on the heap + spawns an
// OS thread (std::thread::spawn) that runs the wrapper fn. The
// wrapper unpacks args, calls the spawned fn, stores the result in
// the Task. `task.await` joins the thread + reads the result +
// frees the Task. `scope concurrent { ... }` allocates a ScopeCtx
// that tracks spawned-but-not-yet-awaited Tasks; at scope exit
// the runtime auto-awaits each remaining Task per ADR 0024 D9.
//
// Real work-stealing scheduler deferred per ADR 0024 D6 amendment.
// The user-facing surface is identical to what the deferred
// scheduler would provide; only the lowering strategy differs.

/// Type-erased wrapper fn pointer. Each spawn site emits a per-
/// site wrapper with this signature: it unpacks args from
/// `args_storage`, calls the spawned fn, stores the result into
/// the Task's result slot. Codegen emits the wrapper per ADR
/// 0024 D8; the runtime just stores + invokes the pointer.
type WrapperFn = unsafe extern "C" fn(task: *mut SentinelTask, args: *mut u8);

/// Layout of a single in-flight task at C4.4 minimum. Fields:
///   - `result`: where the wrapper writes the spawned fn's return
///     value before signalling done. At C4.4 minimum result_ty is
///     restricted to i64 per ADR 0024 D7 — broader result types
///     are deferred.
///   - `done`: 0 until the wrapper signals completion, then 1.
///     Set by the wrapper before notifying the condvar.
///   - `thread_handle`: opaque OS thread handle wrapped in
///     Option<JoinHandle> for cleanup. Held as
///     `Box<Option<JoinHandle>>` indirectly through the runtime;
///     for FFI we just stash a pointer.
///   - `_pad`: alignment padding so size is a multiple of 8.
///
/// The codegen-side LLVM type is `ptr` (opaque); fields are read /
/// written only through the runtime symbols below.
#[repr(C)]
pub struct SentinelTask {
    pub result: i64,
    pub done: u32,
    /// C4.4 / ADR 0024 D9: ownership flag. `0` = standalone (the
    /// awaiter reclaims the Task struct in `sentinel_task_await`);
    /// `1` = owned by a `scope concurrent` (set by
    /// `sentinel_scope_register`; the scope reclaims the struct at
    /// `sentinel_scope_exit`, so `await` only joins + reads). This
    /// resolves the double-ownership hazard between an explicit
    /// `.await` and the scope's auto-await without changing the
    /// 32-byte layout (it occupies the former `_pad` slot at
    /// offset 12).
    pub owned: u32,
    /// Pointer to a heap-allocated Box<Option<JoinHandle<()>>>.
    /// Wrapped in a struct field so the layout stays C-stable.
    pub join_handle_ptr: *mut JoinHandleBox,
    /// Pointer to a heap-allocated Box<Option<ArgsBoxFreeFn>>
    /// (the free-fn for the args_storage). Called at task
    /// destruction.
    pub args_free_ptr: *mut u8,
}

/// Opaque heap-allocated wrapper holding the OS thread join
/// handle. Box-wrapped so the FFI side just sees a pointer.
pub struct JoinHandleBox {
    pub handle: Option<std::thread::JoinHandle<()>>,
}

/// Per-scope task registry. Tracks Tasks spawned inside the scope
/// so scope_exit can auto-await any not-yet-awaited tasks per ADR
/// 0024 D9.
#[repr(C)]
pub struct SentinelScopeCtx {
    pub registry_ptr: *mut ScopeRegistry,
}

/// Heap-allocated Vec of in-flight Task pointers, owned by a
/// ScopeCtx. Box-wrapped through ScopeCtx's `registry_ptr` for
/// FFI stability.
pub struct ScopeRegistry {
    pub tasks: Vec<*mut SentinelTask>,
}

/// ADR 0024 D7: allocate a Task + spawn an OS thread that runs
/// the wrapper. Returns an opaque Task* — the codegen-emitted
/// LLVM type for `Type::Task(_)` is `ptr` so this matches.
///
/// # Safety
///
/// `wrapper` must be a valid C-ABI fn matching the WrapperFn
/// signature. `args_storage` must be a valid pointer to a heap-
/// allocated buffer of `args_size` bytes — the wrapper unpacks
/// it; the runtime takes ownership and frees it at task
/// destruction.
#[no_mangle]
pub extern "C" fn sentinel_task_spawn(
    wrapper: WrapperFn,
    args_storage: *mut u8,
    _args_size: i64,
) -> *mut SentinelTask {
    let join_box = Box::new(JoinHandleBox { handle: None });
    let join_ptr = Box::into_raw(join_box);
    let task_box = Box::new(SentinelTask {
        result: 0,
        done: 0,
        owned: 0,
        join_handle_ptr: join_ptr,
        args_free_ptr: args_storage,
    });
    let task_ptr = Box::into_raw(task_box);

    // Capture pointer values as usize for Send across thread
    // boundary (raw pointers don't implement Send).
    let task_addr = task_ptr as usize;
    let args_addr = args_storage as usize;
    let handle = std::thread::spawn(move || {
        // SAFETY: task_addr + args_addr are live for the
        // duration of this thread (the spawning thread won't
        // free them until sentinel_task_await joins us).
        unsafe {
            let task = task_addr as *mut SentinelTask;
            let args = args_addr as *mut u8;
            wrapper(task, args);
        }
    });

    // SAFETY: join_ptr is valid (just allocated above); we own it.
    unsafe {
        (*join_ptr).handle = Some(handle);
    }

    task_ptr
}

/// ADR 0024 D7 + D9: join the task's OS thread, read the result,
/// release the thread handle + args buffer. Returns the spawned
/// fn's return value (i64 at C4.4 minimum per the result_ty
/// restriction).
///
/// Idempotent + double-ownership safe per the `owned` flag:
///   - The join handle + args buffer are released on first call
///     and nulled, so a second call (e.g. the scope's auto-await
///     after an explicit `.await`) is a no-op join.
///   - The Task struct is reclaimed here ONLY for standalone tasks
///     (`owned == 0`). A scope-owned task (`owned == 1`, set by
///     `sentinel_scope_register`) is reclaimed by
///     `sentinel_scope_exit` instead — this is what makes an
///     explicit `.await` inside a `scope concurrent { ... }` safe
///     against the scope's exit-time auto-await.
///
/// # Safety
///
/// `task` must be a Task* returned by `sentinel_task_spawn`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_task_await(task: *mut SentinelTask) -> i64 {
    if task.is_null() {
        return 0;
    }
    // SAFETY: task is non-null and live per the caller contract.
    unsafe {
        let join_ptr = (*task).join_handle_ptr;
        if !join_ptr.is_null() {
            let join_box = Box::from_raw(join_ptr);
            if let Some(handle) = join_box.handle {
                let _ = handle.join();
            }
            // JoinHandleBox freed; null the slot so a re-await
            // (scope auto-await) skips the join.
            (*task).join_handle_ptr = std::ptr::null_mut();
        }
        let args = (*task).args_free_ptr;
        if !args.is_null() {
            libc::free(args as *mut libc::c_void);
            (*task).args_free_ptr = std::ptr::null_mut();
        }
        let result = (*task).result;
        if (*task).owned == 0 {
            // Standalone task (no owning scope): reclaim the struct
            // now. Scope-owned tasks are freed by sentinel_scope_exit.
            let _ = Box::from_raw(task);
        }
        result
    }
}

/// ADR 0024 D7: allocate a per-scope task registry. Returned to
/// codegen as an opaque ScopeCtx* (LLVM type is `ptr`).
#[no_mangle]
pub extern "C" fn sentinel_scope_enter() -> *mut SentinelScopeCtx {
    let registry = Box::new(ScopeRegistry { tasks: Vec::new() });
    let registry_ptr = Box::into_raw(registry);
    let scope = Box::new(SentinelScopeCtx { registry_ptr });
    Box::into_raw(scope)
}

/// ADR 0024 D7: register a spawned Task in the enclosing scope.
/// Codegen emits this after each `sentinel_task_spawn` inside a
/// `scope concurrent { ... }` block so the scope-exit can auto-
/// await tasks the user didn't await explicitly.
///
/// # Safety
///
/// `scope` must be a ScopeCtx* returned by `sentinel_scope_enter`;
/// `task` must be a live Task* returned by `sentinel_task_spawn`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_scope_register(
    scope: *mut SentinelScopeCtx,
    task: *mut SentinelTask,
) {
    if scope.is_null() || task.is_null() {
        return;
    }
    // SAFETY: scope.registry_ptr is live per the enter/exit
    // contract; pushing to its Vec is well-defined.
    unsafe {
        // C4.4 / ADR 0024 D9: transfer Task-struct ownership to the
        // scope so an explicit `.await` joins+reads without freeing
        // the struct (the scope frees it at exit). Resolves the
        // double-free between explicit await + auto-await.
        (*task).owned = 1;
        let registry = (*scope).registry_ptr;
        if !registry.is_null() {
            (*registry).tasks.push(task);
        }
    }
}

/// ADR 0024 D7 + D9: exit the scope. The scope OWNS every Task
/// registered in it (`owned == 1`), so this is the single point
/// that reclaims their structs. For each registered task:
///   1. `sentinel_task_await` joins the thread + reads the result
///      if it hasn't already been awaited (idempotent: a second
///      await is a no-op join since the handle slot was nulled).
///      Because the task is `owned`, await does NOT free the
///      struct.
///   2. Then we reclaim the Task struct itself.
///
/// This makes an explicit `.await` inside the scope safe against
/// this exit-time pass: no UAF, no double-free, no leak.
///
/// Cancellation on early exit is DEFERRED per ADR 0024 D9 — at
/// C4.4 minimum this function only runs on the normal-exit path.
///
/// # Safety
///
/// `scope` must be a ScopeCtx* returned by `sentinel_scope_enter`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_scope_exit(scope: *mut SentinelScopeCtx) {
    if scope.is_null() {
        return;
    }
    // SAFETY: reclaim the scope's heap allocation + auto-await +
    // free any tasks the scope owns.
    unsafe {
        let scope_box = Box::from_raw(scope);
        if !scope_box.registry_ptr.is_null() {
            let registry_box = Box::from_raw(scope_box.registry_ptr);
            for task_ptr in registry_box.tasks {
                if task_ptr.is_null() {
                    continue;
                }
                // Join + read if not already awaited (owned, so the
                // struct survives the await), then reclaim it.
                let _ = sentinel_task_await(task_ptr);
                let _ = Box::from_raw(task_ptr);
            }
        }
    }
}

// =============================================================================
// ADR 0066 M1.2: typed message-passing channels (`Channel<i64>` at the minimum).
//
// A `SentinelChannel` wraps a CROSS-PLATFORM `std::sync::mpsc` channel (NOT the
// `#[cfg(unix)]` socket path — channels work on every host). Both the sender
// and the receiver live behind a `Mutex`, so the whole struct is `Sync`: the
// handle is a raw `ptr` shared producer↔consumer by COPYING (ADR 0066 D2 — the
// `Channel` type is `Copy`), and concurrent `send`/`recv` from different threads
// is sound. It is the VALUES that move (an `i64` per `send` at the minimum), not
// the handle.
//
// M1.2-minimum limitations (documented, deferred):
//   * the element is `i64` (`Channel<i64>`); word-scalar / aggregate elements
//     are M1.2b / later.
//   * the struct is LEAKED — a `Copy` handle has no single owner to free it
//     (no `Arc`/refcount yet), so there is nothing safe to free on
//     `channel_close` (another copy may still hold the ptr). Scope-owned
//     channel cleanup is a follow-on (cf. ADR 0024's task ownership).
//   * one `Sender` (not cloned): `channel_close` drops it, signalling recv EOF.

/// A heap-allocated mpsc channel. Codegen holds an opaque `*mut SentinelChannel`
/// (the `Type::Channel` LLVM type is `ptr`). `sender` is `None` after
/// `channel_close` (dropping the `Sender` makes a drained `recv` return EOF).
pub struct SentinelChannel {
    sender: std::sync::Mutex<Option<std::sync::mpsc::Sender<i64>>>,
    receiver: std::sync::Mutex<std::sync::mpsc::Receiver<i64>>,
}

/// ADR 0066 M1.2: allocate a new unbounded mpsc channel; return an opaque
/// `*mut SentinelChannel` (the codegen LLVM type for `Type::Channel` is `ptr`).
#[no_mangle]
pub extern "C" fn sentinel_channel_new() -> *mut SentinelChannel {
    let (tx, rx) = std::sync::mpsc::channel::<i64>();
    let ch = Box::new(SentinelChannel {
        sender: std::sync::Mutex::new(Some(tx)),
        receiver: std::sync::Mutex::new(rx),
    });
    Box::into_raw(ch)
}

/// ADR 0066 M1.2: move `value` into the channel. Returns 0 on success, or -1 if
/// the channel is already closed (the sender was dropped) or the receiver is
/// gone (the value is discarded, like writing to a closed pipe).
///
/// # Safety
///
/// `ch` must be a `*mut SentinelChannel` returned by `sentinel_channel_new`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_channel_send(ch: *mut SentinelChannel, value: i64) -> i64 {
    if ch.is_null() {
        return -1;
    }
    // SAFETY: `ch` is a live SentinelChannel per the caller contract; the
    // struct is `Sync`, so a shared `&` from this (possibly producer) thread
    // is sound.
    let chan = unsafe { &*ch };
    match chan.sender.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(tx) => match tx.send(value) {
                Ok(()) => 0,
                Err(_) => -1,
            },
            None => -1,
        },
        Err(_) => -1,
    }
}

/// ADR 0066 M1.2: block for the next value. On success writes it to `*out` and
/// returns 0; when the channel is closed AND drained (all senders dropped),
/// returns 1 and leaves `*out` untouched. Codegen builds the `?i64`
/// (`{ i1 valid, i64 }`) from the (return == 0, *out) pair (ADR 0066 D4).
///
/// # Safety
///
/// `ch` must be a `*mut SentinelChannel` from `sentinel_channel_new`; `out`
/// must be a valid, writable `*mut i64`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_channel_recv(ch: *mut SentinelChannel, out: *mut i64) -> i64 {
    if ch.is_null() {
        return 1;
    }
    // SAFETY: `ch` is a live SentinelChannel (Sync) per the caller contract.
    let chan = unsafe { &*ch };
    let result = match chan.receiver.lock() {
        Ok(rx) => rx.recv(),
        Err(_) => return 1,
    };
    match result {
        Ok(v) => {
            if !out.is_null() {
                // SAFETY: `out` is a valid writable i64 slot per the contract.
                unsafe {
                    *out = v;
                }
            }
            0
        }
        // All senders dropped + the queue is drained: closed.
        Err(_) => 1,
    }
}

/// ADR 0066 M1.2: drop the channel's sender, so a drained `recv` returns EOF
/// (closed). Idempotent; returns 0. Does NOT free the struct (a `Copy` handle
/// may have other live copies — M1.2-minimum leak, see the module note above).
///
/// # Safety
///
/// `ch` must be a `*mut SentinelChannel` returned by `sentinel_channel_new`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_channel_close(ch: *mut SentinelChannel) -> i64 {
    if ch.is_null() {
        return 0;
    }
    // SAFETY: `ch` is a live SentinelChannel (Sync) per the caller contract.
    let chan = unsafe { &*ch };
    if let Ok(mut guard) = chan.sender.lock() {
        // Drop the Sender (idempotent: a second close finds None).
        *guard = None;
    }
    0
}

// =============================================================================
// ADR 0066 M2.1: subprocess spawn — `Process` over `std::process::Command`.
//
// Cross-platform (std::process::Command abstracts posix_spawn/fork+exec and
// CreateProcess — NOT a `#[cfg(unix)]` path like the sockets). Codegen holds an
// opaque `*mut SentinelProcess` (the `Type::Process` LLVM type is `ptr`). The
// args arrive as a Sentinel `[[u8]]`: `argv` points to `argc` consecutive abi-v1
// array headers `{ i64 len, ptr data }`, each an argument string.
// =============================================================================

/// The abi-v1 array header `{ i64 len, ptr data }` — used to decode the elements
/// of a `[[u8]]` argv at the runtime boundary (the array layout is abi-v1, §5).
#[repr(C)]
struct SentinelArrayHeader {
    len: i64,
    data: *const u8,
}

/// A heap-allocated spawned child process. Codegen holds an opaque
/// `*mut SentinelProcess`. `child` is taken (`None`) after a successful wait, so
/// a second `wait` returns the no-child sentinel rather than blocking again.
pub struct SentinelProcess {
    child: Option<std::process::Child>,
}

/// ADR 0066 M2.1: spawn a child process. `path_ptr`/`path_len` are a Sentinel
/// `[u8]` executable path; `argv`/`argc` describe a Sentinel `[[u8]]` (an array
/// of `[u8]` argument strings, decoded via [`SentinelArrayHeader`]). Returns an
/// opaque `*mut SentinelProcess`, or null on spawn failure (e.g. no such file).
///
/// # Safety
///
/// `path_ptr` must point to `path_len` readable bytes; `argv` must point to
/// `argc` valid [`SentinelArrayHeader`]s, each with `len` readable bytes at
/// `data` — exactly the abi-v1 `[u8]` / `[[u8]]` layout codegen emits.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_process_spawn(
    path_ptr: *const u8,
    path_len: i64,
    argv: *const u8,
    argc: i64,
) -> *mut SentinelProcess {
    if path_ptr.is_null() || path_len < 0 || argc < 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller contract guarantees `path_len` bytes at `path_ptr`.
    let path_bytes = unsafe { std::slice::from_raw_parts(path_ptr, path_len as usize) };
    let path = String::from_utf8_lossy(path_bytes).into_owned();
    let mut cmd = std::process::Command::new(&path);
    // ADR 0066 M2.2: pipe stdin + stdout so the parent can byte-stream with the
    // child (sentinel_process_write / _read). stderr stays inherited (diagnostics
    // flow to the parent's terminal). Piping is harmless for children that ignore
    // their std streams (e.g. M2.1's `exit 42`).
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    if argc > 0 && !argv.is_null() {
        // SAFETY: `argv` points to `argc` abi-v1 array headers (the `[[u8]]`
        // element buffer), each a live `{ i64 len, ptr data }` `[u8]`.
        let headers =
            unsafe { std::slice::from_raw_parts(argv as *const SentinelArrayHeader, argc as usize) };
        for h in headers {
            if h.data.is_null() || h.len < 0 {
                continue;
            }
            let arg_bytes = unsafe { std::slice::from_raw_parts(h.data, h.len as usize) };
            cmd.arg(String::from_utf8_lossy(arg_bytes).into_owned());
        }
    }
    match cmd.spawn() {
        Ok(child) => Box::into_raw(Box::new(SentinelProcess { child: Some(child) })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// ADR 0066 M2.1: wait for the child to exit and return its exit code. Returns
/// -1 on error: a null handle, an already-waited handle, a wait failure, or a
/// child terminated without an exit code (e.g. killed by a signal on Unix).
///
/// # Safety
///
/// `p` must be a `*mut SentinelProcess` from [`sentinel_process_spawn`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_process_wait(p: *mut SentinelProcess) -> i64 {
    if p.is_null() {
        return -1;
    }
    // SAFETY: `p` is a live SentinelProcess per the caller contract.
    let proc = unsafe { &mut *p };
    match proc.child.take() {
        Some(mut child) => match child.wait() {
            Ok(status) => status.code().map(|c| c as i64).unwrap_or(-1),
            Err(_) => -1,
        },
        None => -1,
    }
}

/// ADR 0066 M2.2: write all `data_len` bytes of `data` (a Sentinel `[u8]`) to the
/// child's stdin, then **close** stdin (so a filter child sees EOF and produces
/// its output). Returns 0 on success, -1 on error (null handle, no piped stdin,
/// already-written/closed stdin, or a write failure). The byte-pipe payload is
/// `[u8]` — a PUBLIC type — so a `secret` can never reach it (the cross-process
/// secret fence, ADR 0066 D8: the IPC ABI is public-only, like FFI). `data` is
/// borrowed (the ADR 0033 A3 runtime-builtin rule), not freed.
///
/// One-shot by design: stdin is closed after the write, so the contract is
/// "write the child's full input once, then read its output". Streaming /
/// multi-write IPC is a deferred post-M2.2 upgrade.
///
/// # Safety
///
/// `p` must be a `*mut SentinelProcess` from [`sentinel_process_spawn`]; `data`
/// must point to `data_len` readable bytes (the abi-v1 `[u8]` layout).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_process_write(
    p: *mut SentinelProcess,
    data: *const u8,
    data_len: i64,
) -> i64 {
    use std::io::Write;
    if p.is_null() || data_len < 0 {
        return -1;
    }
    // SAFETY: `p` is a live SentinelProcess per the caller contract.
    let proc = unsafe { &mut *p };
    let child = match proc.child.as_mut() {
        Some(c) => c,
        None => return -1,
    };
    // Take stdin so the drop at end-of-scope closes the pipe (EOF for the child).
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => return -1,
    };
    let bytes = if data_len == 0 || data.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller contract guarantees `data_len` bytes at `data`.
        unsafe { std::slice::from_raw_parts(data, data_len as usize) }
    };
    match stdin.write_all(bytes) {
        Ok(()) => 0,
        Err(_) => -1,
    }
    // `stdin` drops here → the write end of the pipe closes (child sees EOF).
}

/// ADR 0066 M2.2: read the child's stdout to EOF and return it as a Sentinel
/// `[u8]` — the buffer is libc-malloc'd (via [`sentinel_alloc`]) so scope-exit
/// drop can free it, and the byte length is written to `out_len` (exactly the
/// `sentinel_read_file` ABI shape). On any error (null handle, no piped stdout,
/// already-read stdout, or a read failure) returns an empty `[u8]` (`out_len`=0,
/// a non-null zero-length buffer). The payload is `[u8]` (PUBLIC) — the
/// cross-process secret fence keeps the IPC boundary secret-free (ADR 0066 D8).
///
/// # Safety
///
/// `p` must be a `*mut SentinelProcess` from [`sentinel_process_spawn`];
/// `out_len` must point to a writable `i64`.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn sentinel_process_read(p: *mut SentinelProcess, out_len: *mut i64) -> *mut u8 {
    use std::io::Read;
    let empty = || {
        // SAFETY: caller passed a writable `i64` slot; a 0-length sentinel_alloc
        // returns a non-null, never-dereferenced buffer (matches read_file's n=0).
        if !out_len.is_null() {
            unsafe { *out_len = 0 };
        }
        sentinel_alloc(0)
    };
    if p.is_null() {
        return empty();
    }
    // SAFETY: `p` is a live SentinelProcess per the caller contract.
    let proc = unsafe { &mut *p };
    let child = match proc.child.as_mut() {
        Some(c) => c,
        None => return empty(),
    };
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => return empty(),
    };
    let mut data = Vec::new();
    if stdout.read_to_end(&mut data).is_err() {
        return empty();
    }
    let n = data.len();
    // Copy into a libc-malloc'd buffer so scope-exit drop can free it.
    let buf = sentinel_alloc(n as i64);
    if n > 0 {
        // SAFETY: `buf` has `n` bytes (sentinel_alloc aborts on failure); `data`
        // has `n` bytes; the regions don't overlap.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf, n) };
    }
    // SAFETY: caller passed a writable `i64` slot.
    unsafe { *out_len = n as i64 };
    buf
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

    // ---- C5.4 / ADR 0028: broker-backed scope arenas (the substrate) ----

    #[test]
    fn arena_enter_alloc_exit_round_trip() {
        let arena = sentinel_arena_enter(4096);
        assert!(!arena.is_null(), "enter returns a live handle");
        let p = sentinel_arena_alloc(arena, 32);
        assert!(!p.is_null(), "alloc returns usable memory");
        // The bytes are writable + readable while the arena is live.
        unsafe {
            std::ptr::write_bytes(p, 0xCD, 32);
            assert_eq!(*p, 0xCD);
            assert_eq!(*p.add(31), 0xCD);
        }
        sentinel_arena_exit(arena); // bulk-frees the arena's backing buffer
    }

    #[test]
    fn arena_alloc_is_16_byte_aligned() {
        let arena = sentinel_arena_enter(0); // default capacity
        let p = sentinel_arena_alloc(arena, 1);
        assert_eq!(p as usize % 16, 0, "matches libc malloc's alignment guarantee");
        sentinel_arena_exit(arena);
    }

    #[test]
    fn arena_serves_distinct_allocations() {
        let arena = sentinel_arena_enter(4096);
        let a = sentinel_arena_alloc(arena, 16);
        let b = sentinel_arena_alloc(arena, 16);
        assert_ne!(a, b, "successive bump allocations are distinct");
        sentinel_arena_exit(arena);
    }

    #[test]
    fn arena_exit_on_null_is_a_no_op() {
        sentinel_arena_exit(std::ptr::null_mut());
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
        // Resume drains + frees the kont, returning a pure-wrap
        // (since the chain is empty); consume to free the wrap.
        let result = sentinel_kont_resume(k, 0);
        let _ = sentinel_kont_consume_pure(result);
    }

    #[test]
    fn sentinel_kont_resume_returns_value_in_restricted_case() {
        let k = sentinel_perform_op(0, 100);
        // C3.5(e): resume returns a kont*; the empty-chain case
        // wraps the resumed value in a pure-return kont. Consume
        // to unwrap.
        let result = sentinel_kont_resume(k, 99);
        assert!(!result.is_null());
        // SAFETY: result is a live pure-return kont; op_id read
        // is well-defined.
        unsafe {
            assert_eq!((*result).op_id, PURE_RETURN_OP_ID);
        }
        assert_eq!(sentinel_kont_consume_pure(result), 99);
    }

    #[test]
    fn sentinel_kont_round_trip_with_zero_arg() {
        let k = sentinel_perform_op(0, 0);
        let result = sentinel_kont_resume(k, 0);
        assert_eq!(sentinel_kont_consume_pure(result), 0);
    }

    #[test]
    fn sentinel_kont_free_frees_an_abandoned_frameless_kont() {
        // ADR 0065 D6: a kont allocated by a `perform` but never resumed
        // (an early `return` crossed its `handle`). Freeing a frameless kont
        // (frames_head == null — the direct-`perform` case) just frees the
        // kont; it must not crash or double-free.
        let k = sentinel_perform_op(0, 7);
        // SAFETY: k is a live, un-resumed kont.
        unsafe {
            assert!((*k).frames_head.is_null(), "a direct perform has no frames");
        }
        sentinel_kont_free(k);
    }

    #[test]
    fn sentinel_kont_free_walks_and_frees_the_captured_frame_chain() {
        // ADR 0065 D6: an abandoned kont WITH pushed evaluation frames —
        // `sentinel_kont_free` walks `frames_head` freeing each captured block
        // + frame node (the inverse of `sentinel_kont_push`), then the kont.
        // The resumer is never invoked on the free path.
        unsafe extern "C" fn never_called(_v: i64, _c: *mut u8) -> *mut SentinelKont {
            // kont_free frees the chain WITHOUT calling any resumer.
            core::ptr::null_mut()
        }
        let k = sentinel_perform_op(0, 0);
        // Push two frames, each with a heap-allocated captured-state block.
        sentinel_kont_push(k, never_called, sentinel_alloc(16));
        sentinel_kont_push(k, never_called, sentinel_alloc(8));
        // SAFETY: k is live with two pushed frames.
        unsafe {
            assert!(!(*k).frames_head.is_null(), "two frames were pushed");
        }
        // Frees the kont + both frame nodes + both captured blocks; no leak or
        // double-free (a sanitizer build would flag either).
        sentinel_kont_free(k);
    }

    #[test]
    fn sentinel_kont_struct_layout_is_stable() {
        // Layout invariant: codegen reads `op_id` via GEP at
        // offset 0 + `arg` at offset 8 (after the 4-byte op_id +
        // 4-byte pad). `frames_head` follows after `consumed: u8
        // + _pad2: [u8; 7]` at offset 24, but codegen accesses
        // it through sentinel_kont_push so the offset is opaque.
        assert_eq!(core::mem::size_of::<SentinelKont>(), 32);
        assert_eq!(core::mem::align_of::<SentinelKont>(), 8);
    }

    // ===== C5 D7 / ADR 0029: abi-v1 stability =====
    //
    // These pin the runtime↔codegen ABI boundary documented in
    // docs/abi-v1.md §3 + §5. A drift in a struct layout, a field
    // offset, or the runtime-symbol set turns one of these red — by
    // design, so an accidental ABI break is a failing test, not a
    // silent miscompile. Update docs/abi-v1.md in the same commit as
    // any intentional change here.

    #[test]
    fn abi_v1_struct_layouts_are_stable() {
        use core::mem::{align_of, offset_of, size_of};

        // SentinelKont (abi-v1 §3): 32 / 8; codegen GEPs op_id@0, arg@8.
        assert_eq!(size_of::<SentinelKont>(), 32);
        assert_eq!(align_of::<SentinelKont>(), 8);
        assert_eq!(offset_of!(SentinelKont, op_id), 0);
        assert_eq!(offset_of!(SentinelKont, arg), 8);
        assert_eq!(offset_of!(SentinelKont, consumed), 16);
        assert_eq!(offset_of!(SentinelKont, frames_head), 24);

        // SentinelFrame (§3): 3 pointers, 24 / 8.
        assert_eq!(size_of::<SentinelFrame>(), 24);
        assert_eq!(align_of::<SentinelFrame>(), 8);
        assert_eq!(offset_of!(SentinelFrame, resumer), 0);
        assert_eq!(offset_of!(SentinelFrame, captured), 8);
        assert_eq!(offset_of!(SentinelFrame, next), 16);

        // SentinelTask (§3): 32 / 8; `owned` at offset 12 (ADR 0024 D9).
        assert_eq!(size_of::<SentinelTask>(), 32);
        assert_eq!(align_of::<SentinelTask>(), 8);
        assert_eq!(offset_of!(SentinelTask, result), 0);
        assert_eq!(offset_of!(SentinelTask, done), 8);
        assert_eq!(offset_of!(SentinelTask, owned), 12);
        assert_eq!(offset_of!(SentinelTask, join_handle_ptr), 16);
        assert_eq!(offset_of!(SentinelTask, args_free_ptr), 24);

        // SentinelScopeCtx (§3): one pointer, 8 / 8.
        assert_eq!(size_of::<SentinelScopeCtx>(), 8);
        assert_eq!(align_of::<SentinelScopeCtx>(), 8);
        assert_eq!(offset_of!(SentinelScopeCtx, registry_ptr), 0);
    }

    #[test]
    fn abi_v1_runtime_symbol_set() {
        // The exact set of runtime symbols codegen links against
        // (docs/abi-v1.md §5). Referencing each by name forces a
        // compile error if any is renamed or removed — pinning the
        // symbol contract on the definition side. Includes the
        // runtime-internal sentinel_kont_panic_resumed (not codegen-
        // declared, but part of the runtime's link-time contract).
        // Cast through `*const ()` (clippy's preferred fn-pointer form).
        let symbols: &[*const ()] = &[
            sentinel_print as *const (),
            sentinel_alloc as *const (),
            sentinel_free as *const (),
            sentinel_realloc as *const (),
            sentinel_read_file as *const (),
            sentinel_write_file as *const (),
            sentinel_print_bytes as *const (),
            sentinel_str_eq as *const (),
            sentinel_panic_oob as *const (),
            sentinel_arena_enter as *const (),
            sentinel_arena_alloc as *const (),
            sentinel_arena_exit as *const (),
            sentinel_perform_op as *const (),
            sentinel_kont_resume as *const (),
            sentinel_kont_pure as *const (),
            sentinel_kont_consume_pure as *const (),
            sentinel_kont_push as *const (),
            sentinel_kont_free as *const (),
            sentinel_kont_panic_resumed as *const (),
            sentinel_task_spawn as *const (),
            sentinel_task_await as *const (),
            sentinel_scope_enter as *const (),
            sentinel_scope_register as *const (),
            sentinel_scope_exit as *const (),
            // ADR 0066 M1.2: the channel builtins (cross-platform, unlike the
            // `#[cfg(unix)]` socket symbols which are absent from this set).
            sentinel_channel_new as *const (),
            sentinel_channel_send as *const (),
            sentinel_channel_recv as *const (),
            sentinel_channel_close as *const (),
            // ADR 0066 M2.1: the subprocess spawn/wait builtins (cross-platform).
            sentinel_process_spawn as *const (),
            sentinel_process_wait as *const (),
            // ADR 0066 M2.2: the byte-pipe IPC builtins (cross-platform).
            sentinel_process_write as *const (),
            sentinel_process_read as *const (),
        ];
        // 32 symbols: 23 codegen-declared (incl. D.2's sentinel_str_eq,
        // D.3's sentinel_realloc, D.4's sentinel_read_file /
        // sentinel_write_file / sentinel_print_bytes, and ADR 0065 D6's
        // sentinel_kont_free) + sentinel_kont_panic_resumed + ADR 0066 M1.2's
        // 4 sentinel_channel_* + M2.1's 2 sentinel_process_spawn/_wait + M2.2's
        // 2 sentinel_process_write/_read symbols.
        assert_eq!(symbols.len(), 32);
        assert!(symbols.iter().all(|&s| !s.is_null()), "every symbol has an address");
    }

    // ----- C4.4 / ADR 0024 D7: structured concurrency runtime -----

    /// Simple test wrapper: receives no args; writes 42 to the
    /// Task's result slot.
    extern "C" fn test_wrapper_const_42(task: *mut SentinelTask, _args: *mut u8) {
        unsafe {
            (*task).result = 42;
            (*task).done = 1;
        }
    }

    /// Test wrapper that reads a single i64 arg from args_storage,
    /// doubles it, writes the result.
    extern "C" fn test_wrapper_double(task: *mut SentinelTask, args: *mut u8) {
        unsafe {
            let x = *(args as *const i64);
            (*task).result = x * 2;
            (*task).done = 1;
        }
    }

    #[test]
    fn sentinel_task_spawn_then_await_round_trips() {
        let task = sentinel_task_spawn(
            test_wrapper_const_42,
            std::ptr::null_mut(),
            0,
        );
        assert!(!task.is_null());
        let result = sentinel_task_await(task);
        assert_eq!(result, 42);
    }

    #[test]
    fn sentinel_task_spawn_with_args() {
        // Heap-alloc an i64 = 21, hand to spawn, expect 42 back.
        let args_storage = sentinel_alloc(8);
        unsafe {
            *(args_storage as *mut i64) = 21;
        }
        let task = sentinel_task_spawn(test_wrapper_double, args_storage, 8);
        let result = sentinel_task_await(task);
        assert_eq!(result, 42);
    }

    #[test]
    fn sentinel_scope_enter_exit_round_trips() {
        let scope = sentinel_scope_enter();
        assert!(!scope.is_null());
        sentinel_scope_exit(scope);
    }

    #[test]
    fn sentinel_scope_register_then_exit_auto_awaits() {
        // scope_exit should auto-await registered tasks the user
        // didn't await explicitly. After exit, the task is
        // joined and freed.
        let scope = sentinel_scope_enter();
        let task = sentinel_task_spawn(
            test_wrapper_const_42,
            std::ptr::null_mut(),
            0,
        );
        sentinel_scope_register(scope, task);
        sentinel_scope_exit(scope);
        // No way to assert directly that the thread joined, but
        // the test would deadlock or panic if Auto-await missed.
    }

    #[test]
    fn scope_explicit_await_then_exit_is_safe() {
        // The phase-go pattern (ADR 0024 D12): spawn + register +
        // EXPLICIT await + scope_exit. Before the `owned` flag, the
        // scope's exit-time auto-await would UAF / double-free the
        // already-awaited Task. Now await joins+reads without
        // freeing an owned task, and scope_exit reclaims it.
        let scope = sentinel_scope_enter();
        let args = sentinel_alloc(8);
        unsafe {
            *(args as *mut i64) = 21;
        }
        let task = sentinel_task_spawn(test_wrapper_double, args, 8);
        sentinel_scope_register(scope, task);
        let result = sentinel_task_await(task);
        assert_eq!(result, 42);
        // Must not double-free / UAF the explicitly-awaited task.
        sentinel_scope_exit(scope);
    }

    #[test]
    fn sentinel_task_layout_is_stable() {
        // Codegen alloc's Task* opaque (LLVM type is `ptr`); the
        // size assertion catches drift in field layout that
        // would break wrapper-fn ABI.
        // Size = 8 (result i64) + 4 (done u32) + 4 (_pad u32) +
        // 8 (join_handle_ptr) + 8 (args_free_ptr) = 32.
        assert_eq!(core::mem::size_of::<SentinelTask>(), 32);
        assert_eq!(core::mem::align_of::<SentinelTask>(), 8);
    }

    // ---- ADR 0066 M1.2: the channel runtime surface ----

    #[test]
    fn sentinel_channel_send_recv_round_trips() {
        let ch = sentinel_channel_new();
        assert!(!ch.is_null());
        sentinel_channel_send(ch, 42);
        let mut out: i64 = 0;
        let r = sentinel_channel_recv(ch, &mut out as *mut i64);
        assert_eq!(r, 0, "recv of an available value returns 0 (some)");
        assert_eq!(out, 42);
        // After close + drain, recv reports closed (1 = null).
        sentinel_channel_close(ch);
        let r2 = sentinel_channel_recv(ch, &mut out as *mut i64);
        assert_eq!(r2, 1, "recv on a closed+drained channel returns 1 (null)");
    }

    #[test]
    fn sentinel_channel_cross_thread_producer() {
        // The worker pattern (ADR 0066 D2): a producer thread holds a COPY of
        // the handle (the ptr), sends, then closes; the consumer recvs until
        // EOF. Exercises that the channel is Sync (shared across threads) and
        // that close signals recv EOF.
        let ch = sentinel_channel_new();
        let addr = ch as usize; // raw ptr isn't Send; pass the address.
        let producer = std::thread::spawn(move || {
            let ch = addr as *mut SentinelChannel;
            sentinel_channel_send(ch, 10);
            sentinel_channel_send(ch, 32);
            sentinel_channel_close(ch);
        });
        let mut sum: i64 = 0;
        loop {
            let mut out: i64 = 0;
            if sentinel_channel_recv(ch, &mut out as *mut i64) != 0 {
                break;
            }
            sum += out;
        }
        producer.join().unwrap();
        assert_eq!(sum, 42);
    }

    // ---- ADR 0066 M2.1: the subprocess spawn runtime surface ----

    #[test]
    fn sentinel_process_spawn_and_wait_exit_code() {
        // Spawn a cross-platform child that exits 42, then wait for its code.
        // The args arrive as a `[[u8]]` — an array of abi-v1 `{ i64 len, ptr }`
        // headers — exactly as codegen will pass them.
        #[cfg(windows)]
        let (path, arg0, arg1) = ("cmd", "/c", "exit 42");
        #[cfg(not(windows))]
        let (path, arg0, arg1) = ("sh", "-c", "exit 42");

        let arg_bytes = [arg0.as_bytes(), arg1.as_bytes()];
        let headers: Vec<SentinelArrayHeader> = arg_bytes
            .iter()
            .map(|b| SentinelArrayHeader { len: b.len() as i64, data: b.as_ptr() })
            .collect();
        let p = sentinel_process_spawn(
            path.as_ptr(),
            path.len() as i64,
            headers.as_ptr() as *const u8,
            headers.len() as i64,
        );
        assert!(!p.is_null(), "spawn of `{path}` succeeded");
        let code = sentinel_process_wait(p);
        assert_eq!(code, 42, "the child exited 42");
        // A second wait on the now-childless handle returns the -1 sentinel.
        assert_eq!(sentinel_process_wait(p), -1);
    }

    #[test]
    fn sentinel_process_spawn_nonexistent_returns_null() {
        let path = "this_executable_does_not_exist_xyzzy";
        let p = sentinel_process_spawn(path.as_ptr(), path.len() as i64, std::ptr::null(), 0);
        assert!(p.is_null(), "spawning a nonexistent program returns null");
        // wait on a null handle is the -1 sentinel (no panic).
        assert_eq!(sentinel_process_wait(std::ptr::null_mut()), -1);
    }

    // ---- ADR 0066 M2.2: the byte-pipe IPC runtime surface ----

    #[test]
    fn sentinel_process_write_read_filter_round_trips() {
        // The one-shot filter pattern: spawn a child that copies stdin → stdout,
        // write input (which closes stdin → EOF), read the echoed output back.
        // Cross-platform: `cat` on Unix, `findstr "^"` on Windows (matches every
        // line → an identity filter).
        #[cfg(windows)]
        let (path, args): (&str, Vec<&str>) = ("findstr", vec!["x*"]);
        #[cfg(not(windows))]
        let (path, args): (&str, Vec<&str>) = ("cat", vec![]);

        let headers: Vec<SentinelArrayHeader> = args
            .iter()
            .map(|a| SentinelArrayHeader { len: a.len() as i64, data: a.as_ptr() })
            .collect();
        let p = sentinel_process_spawn(
            path.as_ptr(),
            path.len() as i64,
            if headers.is_empty() { std::ptr::null() } else { headers.as_ptr() as *const u8 },
            headers.len() as i64,
        );
        assert!(!p.is_null(), "spawn of `{path}` succeeded");

        let input = b"hello pipe\n";
        assert_eq!(
            sentinel_process_write(p, input.as_ptr(), input.len() as i64),
            0,
            "write to child stdin succeeds (and closes stdin)"
        );
        let mut out_len: i64 = -1;
        let buf = sentinel_process_read(p, &mut out_len as *mut i64);
        assert!(!buf.is_null());
        assert!(out_len >= input.len() as i64, "got at least the echoed bytes");
        // SAFETY: `buf` holds `out_len` readable bytes.
        let got = unsafe { std::slice::from_raw_parts(buf, out_len as usize) };
        // The identity filter echoes our line (findstr may normalize the newline,
        // so assert on the substring rather than byte equality).
        assert!(
            got.windows(b"hello pipe".len()).any(|w| w == b"hello pipe"),
            "child echoed the input line"
        );
        sentinel_free(buf);
        assert_eq!(sentinel_process_wait(p), 0, "the filter exited cleanly");
    }

    #[test]
    fn sentinel_process_write_read_null_handle_is_safe() {
        // Null handle: write returns the -1 sentinel; read returns an empty,
        // non-null `[u8]` (out_len = 0) — neither panics.
        assert_eq!(sentinel_process_write(std::ptr::null_mut(), b"x".as_ptr(), 1), -1);
        let mut out_len: i64 = -1;
        let buf = sentinel_process_read(std::ptr::null_mut(), &mut out_len as *mut i64);
        assert!(!buf.is_null(), "empty read still returns a non-null buffer");
        assert_eq!(out_len, 0);
        sentinel_free(buf);
    }

    // ---- ADR 0056: the TCP sockets runtime surface (loopback) ----

    /// A full loopback exchange over the socket primitives: a server thread accepts
    /// one connection and echoes what it reads; the client connects to the ephemeral
    /// port, sends a message, and reads the echo back. Exercises listen / local_port /
    /// accept / connect / read / write / close end to end.
    #[cfg(unix)]
    #[test]
    fn tcp_loopback_echo() {
        let lfd = sentinel_tcp_listen(0); // 0 = ephemeral port
        assert!(lfd >= 0, "listen failed");
        let port = sentinel_tcp_local_port(lfd);
        assert!(port > 0, "local_port failed");

        let server = std::thread::spawn(move || {
            let conn = sentinel_tcp_accept(lfd);
            assert!(conn >= 0, "accept failed");
            let mut n: i64 = 0;
            let buf = sentinel_tcp_read(conn, 64, &mut n);
            assert!(n > 0, "server read got nothing");
            assert_eq!(sentinel_tcp_write(conn, buf, n), n, "echo write short");
            sentinel_free(buf);
            sentinel_tcp_close(conn);
            sentinel_tcp_close(lfd);
        });

        let host = b"127.0.0.1";
        let conn = sentinel_tcp_connect(host.as_ptr(), host.len() as i64, port);
        assert!(conn >= 0, "connect failed");
        let msg = b"hello sentinel sockets";
        assert_eq!(
            sentinel_tcp_write(conn, msg.as_ptr(), msg.len() as i64),
            msg.len() as i64,
            "client write short"
        );
        let mut rlen: i64 = 0;
        let rbuf = sentinel_tcp_read(conn, 64, &mut rlen);
        assert_eq!(rlen, msg.len() as i64, "echo length");
        // SAFETY: `rbuf` holds `rlen` bytes read from the socket.
        let got = unsafe { std::slice::from_raw_parts(rbuf, rlen as usize) };
        assert_eq!(got, msg, "echo bytes");
        sentinel_free(rbuf);
        sentinel_tcp_close(conn);
        server.join().expect("server thread");
    }
}
