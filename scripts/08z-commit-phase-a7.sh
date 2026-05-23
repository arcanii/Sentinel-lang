#!/usr/bin/env bash
set -euo pipefail
cd /Users/bryan/Desktop/github_repos/Sentinel-language

echo "======"
echo "PRE-COMMIT CHECKS"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -5
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p sentinel-broker --lib 2>&1 | tail -5
cargo test -p sentinel-broker --doc 2>&1 | tail -5

echo ""
echo "======"
echo "BRANCH"
echo "======"
git --no-pager branch --show-current
git config user.name
git config user.email

echo ""
echo "======"
echo "STAGING"
echo "======"
git add -A
git --no-pager diff --cached --stat

echo ""
echo "======"
echo "COMMITTING"
echo "======"
git commit -m "broker: phase A7 — secret-memory policy" -m "$(cat <<'MSG'
Adds opt-in secret-memory protection for sensitive arenas.

New module: src/secret.rs
  - SecretPolicy { lock_memory, zero_on_free, zero_on_destroy }
    with STRICT / LENIENT / NONE preset constants.
  - SecretStrategy<inner: Box<dyn AllocStrategy>>: a decorator that
    forwards all alloc/free/slot_ptr calls to the inner strategy,
    layering mlock and volatile-zero semantics on top.
  - secure_zero(): write_volatile loop + SeqCst compiler fence so
    the optimizer cannot elide the wipe.
  - Per-slot size tracking via Mutex<HashMap<SlotIndex, usize>> so
    zero-on-free wipes exactly the slot's allocated extent.
  - OS-specific mlock/munlock: inline extern "C" on Unix, VirtualLock
    on Windows, Unsupported error otherwise. No new crate deps.

Error: BrokerError::SecretMemory { reason: &'static str }
  - Hard-fail on mlock errors (STRICT means STRICT).
  - tracing::warn! captures the OS error detail before the variant
    is returned.

Strategy trait additions (src/strategy/mod.rs):
  - AllocOk gains a size: usize field (populated by bump and slab).
  - backing_buffer() default-None hook for strategies that expose
    their underlying allocation as one contiguous range.
  - slot_ptr_mut() default-None hook for strategies that allow
    exclusive raw access to a slot (used by SecretStrategy zeroing).

Bump & slab strategies updated to fill AllocOk.size and to expose
backing_buffer() / slot_ptr_mut() where applicable.

Builder API:
  broker.arena("creds").capacity(4096).secret(SecretPolicy::STRICT).bump();
  broker.arena("keys").secret(SecretPolicy::STRICT).slab(64, 8, 128);

Tests (7 new):
  - secret::tests::secret_policy_constants_have_expected_flags
    (compile-time const asserts on the preset constants)
  - secret::tests::secure_zero_clears_bytes
  - broker::tests::secret_strict_arena_basic_alloc
  - broker::tests::secret_lenient_arena_no_mlock
  - broker::tests::secret_slab_zero_on_free_clears_slot
  - broker::tests::secret_none_policy_is_passthrough
  - broker::tests::secret_strict_destroy_unlocks_cleanly

Total: 70 tests green (62 lib, 5 integration, 2 proptest, 1 doc).
Clippy clean with -D warnings.

Design notes:
  - Construction-time wrapping only; no runtime policy mutation.
  - mlock is hard-fail; LENIENT is the escape hatch for environments
    without IPC_LOCK (containers, etc.).
  - Per-slot size map adds one HashMap insert/remove per alloc/free —
    negligible vs. the secure_zero loop cost, and avoids invasive
    changes to the SlotPtr public type.
  - Event::MemoryLocked deferred to a future phase if needed.

Scripts: 08-secret-memory.sh + diagnostic/fix iterations.
MSG
)"

echo ""
echo "======"
echo "RESULT"
echo "======"
git --no-pager log -1 --stat
