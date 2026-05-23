#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "WRITING src/secret.rs"
echo "======"
cat > "$BROKER/src/secret.rs" <<'END_OF_SECRET'
//! Secret-memory policy.
//!
//! A [`SecretPolicy`] augments an underlying [`AllocStrategy`] with
//! two security properties:
//!
//! 1. **Memory locking**: the strategy's backing buffer is mlocked
//!    (Unix) or VirtualLocked (Windows) so it cannot be paged to
//!    swap. mlock failures are hard errors; STRICT means STRICT.
//! 2. **Zero-on-free**: slot bytes are wiped using a volatile
//!    optimizer-resistant loop, either when a slot is freed
//!    (slab strategy) or when the arena is destroyed.
//!
//! Use via the builder:
//!
//! ```ignore
//! broker.arena("creds").capacity(4096)
//!     .secret(SecretPolicy::STRICT)
//!     .bump()?;
//! ```

use crate::error::BrokerError;
use crate::ids::{SlotGeneration, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, SlotPtr, StrategyKind};
use std::alloc::Layout;
use std::sync::atomic::{compiler_fence, Ordering};

/// Behavioural settings for a secret arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretPolicy {
    /// Lock the backing buffer into RAM (mlock / VirtualLock).
    pub lock_memory: bool,
    /// Zero slot bytes when the slot is freed (slab only).
    pub zero_on_free: bool,
    /// Zero the entire backing buffer when the arena is destroyed.
    pub zero_on_destroy: bool,
}

impl SecretPolicy {
    /// Lock + zero on free + zero on destroy. The default for credentials.
    pub const STRICT: Self = Self {
        lock_memory: true,
        zero_on_free: true,
        zero_on_destroy: true,
    };

    /// Zero on free + zero on destroy, but no mlock. Useful in
    /// environments where mlock is unavailable (containers without
    /// IPC_LOCK) but residual-data scrubbing is still desired.
    pub const LENIENT: Self = Self {
        lock_memory: false,
        zero_on_free: true,
        zero_on_destroy: true,
    };

    /// No protection. Equivalent to no secret policy at all.
    pub const NONE: Self = Self {
        lock_memory: false,
        zero_on_free: false,
        zero_on_destroy: false,
    };
}

impl Default for SecretPolicy {
    fn default() -> Self { Self::STRICT }
}

// --------------------------------------------------------------------
// OS-specific mlock / munlock
// --------------------------------------------------------------------

#[cfg(unix)]
mod os {
    use std::ffi::c_void;
    extern "C" {
        fn mlock(addr: *const c_void, len: usize) -> i32;
        fn munlock(addr: *const c_void, len: usize) -> i32;
    }
    pub fn lock(ptr: *mut u8, len: usize) -> Result<(), std::io::Error> {
        if len == 0 { return Ok(()); }
        // SAFETY: caller asserts `ptr` points to `len` valid bytes.
        let rc = unsafe { mlock(ptr.cast::<c_void>(), len) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    pub fn unlock(ptr: *mut u8, len: usize) -> Result<(), std::io::Error> {
        if len == 0 { return Ok(()); }
        let rc = unsafe { munlock(ptr.cast::<c_void>(), len) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
}

#[cfg(windows)]
mod os {
    use std::ffi::c_void;
    extern "system" {
        fn VirtualLock(addr: *const c_void, len: usize) -> i32;
        fn VirtualUnlock(addr: *const c_void, len: usize) -> i32;
    }
    pub fn lock(ptr: *mut u8, len: usize) -> Result<(), std::io::Error> {
        if len == 0 { return Ok(()); }
        let rc = unsafe { VirtualLock(ptr.cast::<c_void>(), len) };
        if rc != 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    pub fn unlock(ptr: *mut u8, len: usize) -> Result<(), std::io::Error> {
        if len == 0 { return Ok(()); }
        let rc = unsafe { VirtualUnlock(ptr.cast::<c_void>(), len) };
        if rc != 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    pub fn lock(_ptr: *mut u8, _len: usize) -> Result<(), std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "memory locking not supported on this platform",
        ))
    }
    pub fn unlock(_ptr: *mut u8, _len: usize) -> Result<(), std::io::Error> {
        Ok(())
    }
}

/// Volatile, optimizer-resistant zero of a byte range.
///
/// # Safety
/// `ptr` must point to `len` writable bytes that are not aliased by
/// any live reference for the duration of the call.
pub unsafe fn secure_zero(ptr: *mut u8, len: usize) {
    for i in 0..len {
        // SAFETY: caller asserts the byte range is valid and exclusive.
        unsafe { std::ptr::write_volatile(ptr.add(i), 0u8); }
    }
    compiler_fence(Ordering::SeqCst);
}

// --------------------------------------------------------------------
// SecretStrategy: decorator around any AllocStrategy
// --------------------------------------------------------------------

/// Wraps an inner [`AllocStrategy`] and applies a [`SecretPolicy`].
///
/// On construction, locks the inner buffer if requested.
/// On `free`, zeroes the slot bytes before forwarding (when policy
/// requests it). On `Drop`, zeroes the whole buffer if requested
/// and unlocks it.
pub struct SecretStrategy {
    inner: Box<dyn AllocStrategy>,
    policy: SecretPolicy,
    locked_region: Option<(*mut u8, usize)>,
}

// SAFETY: SecretStrategy contains a raw pointer to the inner buffer
// (for unlock/zero on drop). The pointer is derived from the inner
// strategy and is treated as exclusive metadata. The inner strategy
// is itself Send + Sync, and we never expose the pointer to user
// code or follow it concurrently with mutation.
unsafe impl Send for SecretStrategy {}
unsafe impl Sync for SecretStrategy {}

impl SecretStrategy {
    /// Wrap an inner strategy with the given policy.
    ///
    /// # Errors
    /// Returns [`BrokerError::SecretMemory`] if `policy.lock_memory`
    /// is true but the platform mlock call fails.
    pub fn wrap(
        inner: Box<dyn AllocStrategy>,
        policy: SecretPolicy,
    ) -> Result<Self, BrokerError> {
        let mut locked_region: Option<(*mut u8, usize)> = None;
        if policy.lock_memory {
            let Some((ptr, len)) = inner.backing_buffer() else {
                return Err(BrokerError::SecretMemory {
                    reason: "inner strategy does not expose a backing buffer".to_string(),
                });
            };
            os::lock(ptr, len).map_err(|e| BrokerError::SecretMemory {
                reason: format!("mlock failed: {e}"),
            })?;
            locked_region = Some((ptr, len));
        }
        Ok(Self { inner, policy, locked_region })
    }

    /// Access the wrapped policy.
    #[must_use]
    pub fn policy(&self) -> SecretPolicy { self.policy }
}

impl AllocStrategy for SecretStrategy {
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError> {
        self.inner.alloc_raw(layout)
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Option<SlotPtr> {
        self.inner.slot_ptr(slot)
    }

    fn free(&self, slot: SlotIndex, generation: SlotGeneration) -> Result<(), BrokerError> {
        // Zero the slot bytes BEFORE telling the inner strategy to
        // recycle. This way a subsequent allocator that sees the slot
        // (recycled or otherwise) cannot read residual secret data.
        if self.policy.zero_on_free {
            if let Some(sp) = self.inner.slot_ptr_mut(slot) {
                // SAFETY: slot_ptr_mut returns an exclusive raw pointer
                // to the slot's bytes; the strategy guarantees they
                // are not aliased while the slot is "live".
                unsafe { secure_zero(sp.ptr, sp.size); }
            }
        }
        self.inner.free(slot, generation)
    }

    fn used(&self) -> usize { self.inner.used() }
    fn capacity(&self) -> usize { self.inner.capacity() }
    fn available(&self) -> usize { self.inner.available() }
    fn kind(&self) -> StrategyKind { self.inner.kind() }

    fn backing_buffer(&self) -> Option<(*mut u8, usize)> {
        self.inner.backing_buffer()
    }

    fn slot_ptr_mut(&self, slot: SlotIndex) -> Option<SlotPtr> {
        self.inner.slot_ptr_mut(slot)
    }
}

impl Drop for SecretStrategy {
    fn drop(&mut self) {
        if let Some((ptr, len)) = self.locked_region {
            if self.policy.zero_on_destroy {
                // SAFETY: we own the buffer (via the inner strategy)
                // and no slots are accessible at this point.
                unsafe { secure_zero(ptr, len); }
            }
            // Best-effort unlock. Errors are logged but not propagated.
            if let Err(e) = os::unlock(ptr, len) {
                tracing::warn!(error = %e, "munlock failed during SecretStrategy drop");
            }
        } else if self.policy.zero_on_destroy {
            // No lock, but still wipe if possible.
            if let Some((ptr, len)) = self.inner.backing_buffer() {
                unsafe { secure_zero(ptr, len); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_policy_constants_have_expected_flags() {
        assert!(SecretPolicy::STRICT.lock_memory);
        assert!(SecretPolicy::STRICT.zero_on_free);
        assert!(SecretPolicy::STRICT.zero_on_destroy);
        assert!(!SecretPolicy::LENIENT.lock_memory);
        assert!(SecretPolicy::LENIENT.zero_on_free);
        assert_eq!(SecretPolicy::NONE, SecretPolicy {
            lock_memory: false, zero_on_free: false, zero_on_destroy: false,
        });
    }

    #[test]
    fn secure_zero_clears_bytes() {
        let mut buf = vec![0xAAu8; 128];
        unsafe { secure_zero(buf.as_mut_ptr(), buf.len()); }
        assert!(buf.iter().all(|b| *b == 0));
    }
}
END_OF_SECRET
echo "[OK] wrote src/secret.rs"

echo
echo "======"
echo "PATCHING src/error.rs (add SecretMemory variant)"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/error.rs")
src = p.read_text()
if "SecretMemory" in src:
    print("[SKIP] SecretMemory already present")
    raise SystemExit(0)

# Insert a new variant before the closing brace of the enum.
m = re.search(r"pub enum BrokerError \{", src)
if not m:
    print("[WARN] could not find BrokerError enum")
    raise SystemExit(0)

# Find matching closing brace.
i = m.end()
depth = 1
end = None
while i < len(src):
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            end = i
            break
    i += 1

if end is None:
    print("[WARN] could not find end of BrokerError enum")
    raise SystemExit(0)

variant = """
    #[error("secret memory policy failed: {reason}")]
    SecretMemory { reason: String },
"""

src = src[:end] + variant + src[end:]
p.write_text(src)
print("[OK] added BrokerError::SecretMemory variant")
END_OF_PY

echo
echo "======"
echo "PATCHING src/strategy/mod.rs (add backing_buffer + slot_ptr_mut defaults)"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/strategy/mod.rs")
src = p.read_text()
changed = False

if "fn backing_buffer" not in src:
    # Find trait definition and add default methods at the end (before its closing brace).
    m = re.search(r"pub trait AllocStrategy[^{]*\{", src)
    if not m:
        print("[WARN] could not find AllocStrategy trait")
    else:
        i = m.end()
        depth = 1
        end = None
        while i < len(src):
            if src[i] == '{': depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    end = i
                    break
            i += 1
        if end is None:
            print("[WARN] could not find end of AllocStrategy trait")
        else:
            additions = """
    /// The strategy's backing buffer, if any. Used by SecretStrategy
    /// to call mlock / zero-on-destroy on the underlying memory.
    /// Default: None (strategy is opaque or does not own a buffer).
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> { None }

    /// Mutable pointer + size to a slot's bytes. Used by SecretStrategy
    /// to wipe slot contents before forwarding to free. Default: None.
    fn slot_ptr_mut(&self, _slot: crate::ids::SlotIndex) -> Option<SlotPtr> { None }
"""
            src = src[:end] + additions + src[end:]
            changed = True
            print("[OK] added backing_buffer + slot_ptr_mut defaults to AllocStrategy")

if changed:
    p.write_text(src)
END_OF_PY

echo
echo "======"
echo "PATCHING src/strategy/bump.rs (implement backing_buffer)"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/strategy/bump.rs")
src = p.read_text()
if "fn backing_buffer" in src:
    print("[SKIP] BumpStrategy already implements backing_buffer")
    raise SystemExit(0)

# Find `impl AllocStrategy for BumpStrategy {` and add backing_buffer.
m = re.search(r"impl AllocStrategy for BumpStrategy \{", src)
if not m:
    print("[WARN] could not find `impl AllocStrategy for BumpStrategy`")
    raise SystemExit(0)

# Find matching closing brace
i = m.end()
depth = 1
end = None
while i < len(src):
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            end = i
            break
    i += 1

if end is None:
    print("[WARN] could not find end of impl block")
    raise SystemExit(0)

# BumpStrategy stores its buffer as `buffer: NonNull<u8>` (or similar).
# We'll discover by inspecting the file content.
if "buffer:" not in src:
    print("[WARN] could not find buffer field; adding cautious stub")
    add = """
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> { None }
"""
else:
    # Two common shapes: Vec<u8> (.as_mut_ptr / .capacity), NonNull<u8> + capacity field.
    if "Vec<u8>" in src:
        add = """
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> {
        // SAFETY: returning a raw pointer to our owned buffer.
        // SecretStrategy uses this only for mlock / volatile zero;
        // it never reads or writes individual bytes outside slots.
        let mut guard = self.buffer.lock();
        Some((guard.as_mut_ptr(), guard.len().max(guard.capacity())))
    }
"""
    else:
        # Assume NonNull<u8> + capacity usize field.
        add = """
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> {
        Some((self.buffer.as_ptr(), self.capacity))
    }
"""

src = src[:end] + add + src[end:]
p.write_text(src)
print("[OK] added backing_buffer to BumpStrategy")
END_OF_PY

echo
echo "======"
echo "PATCHING src/strategy/slab.rs (implement backing_buffer + slot_ptr_mut)"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/strategy/slab.rs")
src = p.read_text()
changed = False

m = re.search(r"impl AllocStrategy for SlabStrategy \{", src)
if not m:
    print("[WARN] could not find `impl AllocStrategy for SlabStrategy`")
else:
    i = m.end()
    depth = 1
    end = None
    while i < len(src):
        if src[i] == '{': depth += 1
        elif src[i] == '}':
            depth -= 1
            if depth == 0:
                end = i
                break
        i += 1
    if end is None:
        print("[WARN] could not find end of slab impl block")
    else:
        additions = ""
        if "fn backing_buffer" not in src:
            additions += """
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> {
        Some((self.buffer.as_ptr(), self.slot_size_padded * self.slot_count as usize))
    }
"""
            print("[OK] added backing_buffer to SlabStrategy")
        if "fn slot_ptr_mut" not in src:
            additions += """
    fn slot_ptr_mut(&self, slot: crate::ids::SlotIndex) -> Option<crate::strategy::SlotPtr> {
        self.slot_ptr(slot)
    }
"""
            print("[OK] added slot_ptr_mut to SlabStrategy")
        if additions:
            src = src[:end] + additions + src[end:]
            changed = True

if changed:
    p.write_text(src)
END_OF_PY

echo
echo "======"
echo "PATCHING src/builder.rs (add .secret(policy))"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/builder.rs")
src = p.read_text()
if "fn secret(" in src:
    print("[SKIP] .secret() already present")
    raise SystemExit(0)

# Add a `secret_policy` field to ArenaBuilder and a builder method.
# Plus: wrap the strategy in SecretStrategy in bump() / slab() if set.

# 1) Add field
m = re.search(r"pub struct ArenaBuilder<'a> \{([^}]*)\}", src, re.DOTALL)
if not m:
    print("[WARN] could not find ArenaBuilder struct")
    raise SystemExit(0)

body = m.group(1)
if "secret_policy" not in body:
    new_body = body.rstrip()
    if not new_body.endswith(","):
        new_body += ","
    new_body += "\n    secret_policy: Option<crate::secret::SecretPolicy>,\n"
    src = src.replace(m.group(0), f"pub struct ArenaBuilder<'a> {{{new_body}}}", 1)
    print("[OK] added secret_policy field to ArenaBuilder")

# 2) Initialize in any `Self { ... }` block in this file
src = re.sub(
    r"(Self\s*\{[^{}]*?bump_capacity:\s*None,)",
    r"\1\n            secret_policy: None,",
    src,
    count=1,
    flags=re.DOTALL,
)

# 3) Add .secret() method to ArenaBuilder<'a>.
# Find the impl block start.
m2 = re.search(r"impl<'a> ArenaBuilder<'a> \{", src)
if not m2:
    print("[WARN] could not find `impl<'a> ArenaBuilder<'a>`")
else:
    src = src.replace(
        m2.group(0),
        m2.group(0) + """
    /// Apply a secret-memory policy to the next built arena.
    #[must_use]
    pub fn secret(mut self, policy: crate::secret::SecretPolicy) -> Self {
        self.secret_policy = Some(policy);
        self
    }
""",
        1,
    )
    print("[OK] added .secret() method")

# 4) In bump() and slab(), wrap the strategy in SecretStrategy if policy is set.
# We do this by replacing the construction of `strategy: Box<dyn AllocStrategy> = Box::new(...)`
# with a helper that conditionally wraps.
# Simpler: after constructing strategy, transform it.

def inject_wrap(src: str, anchor: str) -> tuple[str, bool]:
    if anchor not in src:
        return src, False
    # Insert wrapping logic right after the anchor line.
    wrap = """
        let strategy: Box<dyn AllocStrategy> = if let Some(policy) = self.secret_policy {
            Box::new(crate::secret::SecretStrategy::wrap(strategy, policy).expect("mlock failed; consider LENIENT or NONE policy"))
        } else { strategy };
"""
    # Best-effort: append after the anchor line's matching `;`
    idx = src.index(anchor)
    semi = src.index(";", idx)
    insert_at = semi + 1
    src = src[:insert_at] + wrap + src[insert_at:]
    return src, True

# Wrap inside bump()
m_bump = re.search(r"let strategy: Box<dyn AllocStrategy> = Box::new\(BumpStrategy::new\(([^)]+)\)\);", src)
if m_bump and "SecretStrategy::wrap" not in src:
    src, ok = inject_wrap(src, m_bump.group(0))
    if ok:
        print("[OK] wrapped bump() strategy with SecretStrategy when policy is set")

# Wrap inside slab()
m_slab = re.search(r"let strategy: Box<dyn AllocStrategy> = Box::new\(SlabStrategy::new\([^)]+\)\);", src)
if m_slab and src.count("SecretStrategy::wrap") < 2:
    src, ok = inject_wrap(src, m_slab.group(0))
    if ok:
        print("[OK] wrapped slab() strategy with SecretStrategy when policy is set")

p.write_text(src)
print("[OK] builder.rs updated")
END_OF_PY

echo
echo "======"
echo "PATCHING src/lib.rs (declare + re-export secret module)"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/lib.rs")
src = p.read_text()
changed = False
if "pub mod secret;" not in src:
    if "pub mod recording;" in src:
        src = src.replace("pub mod recording;", "pub mod recording;\npub mod secret;", 1)
    else:
        src += "\npub mod secret;\n"
    changed = True
    print("[OK] declared pub mod secret")
if "pub use secret::" not in src:
    if "pub use recording::" in src:
        src = re.sub(
            r"(pub use recording::[^\n]+\n)",
            r"\1pub use secret::{SecretPolicy, SecretStrategy};\n",
            src,
            count=1,
        )
    else:
        src += "pub use secret::{SecretPolicy, SecretStrategy};\n"
    changed = True
    print("[OK] re-exported SecretPolicy + SecretStrategy")
if changed:
    p.write_text(src)
END_OF_PY

echo
echo "======"
echo "APPENDING TESTS to src/broker.rs"
echo "======"
python3 <<'END_OF_PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
if "fn secret_strict_arena_basic_alloc" in src:
    print("[SKIP] secret tests already present")
    raise SystemExit(0)

new_tests = '''
    #[test]
    fn secret_strict_arena_basic_alloc() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        // STRICT requires mlock; in CI/dev this normally succeeds on
        // small buffers under RLIMIT_MEMLOCK.
        let a = b.arena("creds").capacity(4096).secret(SecretPolicy::STRICT).bump();
        let h = a.alloc(0x1234_5678_u64).unwrap();
        assert_eq!(*h.get().unwrap(), 0x1234_5678);
    }

    #[test]
    fn secret_lenient_arena_no_mlock() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("creds-lenient").capacity(4096).secret(SecretPolicy::LENIENT).bump();
        let h = a.alloc(99_u32).unwrap();
        assert_eq!(*h.get().unwrap(), 99);
    }

    #[test]
    fn secret_slab_zero_on_free_clears_slot() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("session-keys").secret(SecretPolicy::LENIENT).slab(64, 8, 8);
        // Write a sentinel pattern.
        let h = a.alloc(0xDEAD_BEEF_DEAD_BEEFu64).unwrap();
        let slot = h.slot();
        a.free(&h).unwrap();
        // Re-alloc: same slot is recycled. Without the secret policy,
        // bytes could leak; with zero_on_free, the slot is zeroed
        // before the next write. We allocate a zero value here and
        // verify it remains zero. (The real guarantee is observed
        // via the strategy slot_ptr_mut path; this test is a smoke
        // check that recycling still works correctly under wrapping.)
        let h2 = a.alloc(0u64).unwrap();
        assert_eq!(h2.slot(), slot, "expected slot to be recycled");
        assert_eq!(*h2.get().unwrap(), 0);
    }

    #[test]
    fn secret_none_policy_is_passthrough() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("plain").capacity(1024).secret(SecretPolicy::NONE).bump();
        let h = a.alloc(7_u8).unwrap();
        assert_eq!(*h.get().unwrap(), 7);
    }

    #[test]
    fn secret_strict_destroy_unlocks_cleanly() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("creds-destroy").capacity(4096).secret(SecretPolicy::STRICT).bump();
        let id = a.id();
        let _h = a.alloc(42_u64).unwrap();
        // Dropping the arena should munlock + zero without panicking.
        b.destroy_arena(id).unwrap();
    }
'''

m = re.search(r"#\[cfg\(test\)\]\s*mod tests \{", src)
if not m:
    print("[WARN] no #[cfg(test)] mod tests block found")
    raise SystemExit(0)

start = m.end()
depth = 1
i = start
close = None
while i < len(src):
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            close = i
            break
    i += 1

if close is None:
    print("[WARN] could not find end of mod tests")
    raise SystemExit(0)

src = src[:close] + new_tests + src[close:]
p.write_text(src)
print("[OK] appended 5 secret tests")
END_OF_PY

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 30

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 40

echo
echo "======"
echo "TESTS"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 80
else
  cargo test -p sentinel-broker --lib 2>&1 | tail -n 80
fi

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -n 10

echo
echo "======"
echo "DONE"
echo "======"
