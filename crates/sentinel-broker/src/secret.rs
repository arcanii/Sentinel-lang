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
use parking_lot::Mutex;
use std::collections::HashMap;

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
    /// Records the size in bytes of each live slot so that `free()`
    /// can zero exactly the right range. Populated on each `alloc_raw`,
    /// drained on each `free`.
    slot_sizes: Mutex<HashMap<SlotIndex, usize>>,
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
                    os_errno: None,
                });
            };
            os::lock(ptr, len).map_err(|e| {
                tracing::warn!(error = %e, "mlock failed");
                BrokerError::SecretMemory { reason: "mlock failed".to_string(), os_errno: None }
            })?;
            locked_region = Some((ptr, len));
        }
        Ok(Self { inner, policy, locked_region, slot_sizes: Mutex::new(HashMap::new()) })
    }

    /// Access the wrapped policy.
    #[must_use]
    pub fn policy(&self) -> SecretPolicy { self.policy }
}

impl AllocStrategy for SecretStrategy {
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError> {
        let ok = self.inner.alloc_raw(layout)?;
        // Round up to layout alignment so we zero the full reserved
        // range, not just the requested size.
        let padded = (layout.size() + layout.align() - 1) & !(layout.align() - 1);
        self.slot_sizes.lock().insert(ok.slot, padded.max(layout.size()));
        Ok(ok)
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<SlotPtr, BrokerError> {
        self.inner.slot_ptr(slot)
    }

    fn free(&self, slot: SlotIndex, generation: SlotGeneration) -> Result<(), BrokerError> {
        // Zero the slot bytes BEFORE telling the inner strategy to
        // recycle. This way a subsequent allocator that sees the slot
        // (recycled or otherwise) cannot read residual secret data.
        let stored_size = self.slot_sizes.lock().remove(&slot);
        if self.policy.zero_on_free {
            if let (Ok(sp), Some(sz)) = (self.inner.slot_ptr(slot), stored_size) {
                // SAFETY: slot_ptr returns a raw pointer to the slot's
                // bytes; the strategy guarantees they are not aliased
                // while the slot is "live" (i.e. before `free` returns).
                unsafe { secure_zero(sp.ptr.as_ptr(), sz); }
            }
        }
        self.inner.free(slot, generation)
    }

    fn used(&self) -> usize { self.inner.used() }
    fn capacity(&self) -> usize { self.inner.capacity() }
    fn available(&self) -> usize { self.inner.available() }
    fn kind(&self) -> StrategyKind { self.inner.kind() }

    fn slot_size_hint(&self) -> Option<usize> { self.inner.slot_size_hint() }

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
        const { assert!(SecretPolicy::STRICT.lock_memory) };
        const { assert!(SecretPolicy::STRICT.zero_on_free) };
        const { assert!(SecretPolicy::STRICT.zero_on_destroy) };
        const { assert!(!SecretPolicy::LENIENT.lock_memory) };
        const { assert!(SecretPolicy::LENIENT.zero_on_free) };
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
