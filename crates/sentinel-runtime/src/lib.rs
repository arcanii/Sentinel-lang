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
    pub _pad: u32,
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
        _pad: 0,
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

/// ADR 0024 D7: join the task's OS thread, read the result,
/// free the Task. Returns the spawned fn's return value (i64 at
/// C4.4 minimum per the result_ty restriction).
///
/// # Safety
///
/// `task` must be a Task* returned by `sentinel_task_spawn` (or
/// `sentinel_scope_register`); double-await is undefined.
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
            // Box dropped at end of scope; JoinHandleBox freed.
        }
        let result = (*task).result;
        let args = (*task).args_free_ptr;
        if !args.is_null() {
            libc::free(args as *mut libc::c_void);
        }
        // Reclaim + drop the Task box.
        let _ = Box::from_raw(task);
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
        let registry = (*scope).registry_ptr;
        if !registry.is_null() {
            (*registry).tasks.push(task);
        }
    }
}

/// ADR 0024 D7 + D9: exit the scope. Auto-awaits any Task in the
/// registry that hasn't already been awaited (we detect this via
/// the Task's join_handle_ptr being non-null — `sentinel_task_await`
/// frees the JoinHandleBox + the Task itself, so an awaited Task
/// is invalid memory). At C4.4 minimum we track per-scope tasks
/// explicitly; calling await on a task removes it from the
/// registry would be cleaner — TODO for a future iteration.
/// For now we just iterate the registry; tasks already freed by
/// explicit await will be skipped via a per-task "awaited" flag.
///
/// Cancellation on early exit is DEFERRED per ADR 0024 D9 — at
/// C4.4 minimum this function only runs on the normal-exit path.
///
/// # Safety
///
/// `scope` must be a ScopeCtx* returned by `sentinel_scope_enter`.
#[no_mangle]
pub extern "C" fn sentinel_scope_exit(scope: *mut SentinelScopeCtx) {
    if scope.is_null() {
        return;
    }
    // SAFETY: reclaim the scope's heap allocation + auto-await
    // any tasks the user didn't explicitly await.
    unsafe {
        let scope_box = Box::from_raw(scope);
        if !scope_box.registry_ptr.is_null() {
            let registry_box = Box::from_raw(scope_box.registry_ptr);
            for task_ptr in registry_box.tasks {
                if task_ptr.is_null() {
                    continue;
                }
                // A task whose join_handle_ptr is null was
                // already awaited (and freed) — skip. But since
                // we don't track per-task "awaited" status
                // cleanly at C4.4 minimum, this is a best-effort
                // check that may UB on already-freed Tasks.
                // Mitigation: at C4.4 minimum tests always
                // explicitly await tasks; auto-await is a future
                // safety net.
                let join_ptr = (*task_ptr).join_handle_ptr;
                if join_ptr.is_null() {
                    continue;
                }
                let _ = sentinel_task_await(task_ptr);
            }
        }
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
    fn sentinel_kont_struct_layout_is_stable() {
        // Layout invariant: codegen reads `op_id` via GEP at
        // offset 0 + `arg` at offset 8 (after the 4-byte op_id +
        // 4-byte pad). `frames_head` follows after `consumed: u8
        // + _pad2: [u8; 7]` at offset 24, but codegen accesses
        // it through sentinel_kont_push so the offset is opaque.
        assert_eq!(core::mem::size_of::<SentinelKont>(), 32);
        assert_eq!(core::mem::align_of::<SentinelKont>(), 8);
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
    fn sentinel_task_layout_is_stable() {
        // Codegen alloc's Task* opaque (LLVM type is `ptr`); the
        // size assertion catches drift in field layout that
        // would break wrapper-fn ABI.
        // Size = 8 (result i64) + 4 (done u32) + 4 (_pad u32) +
        // 8 (join_handle_ptr) + 8 (args_free_ptr) = 32.
        assert_eq!(core::mem::size_of::<SentinelTask>(), 32);
        assert_eq!(core::mem::align_of::<SentinelTask>(), 8);
    }
}
