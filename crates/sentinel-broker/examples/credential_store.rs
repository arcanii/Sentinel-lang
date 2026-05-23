//! Credential-store demo: secret slab arena with mlock + zero-on-free.
//!
//! Demonstrates Sentinel's secret-memory policy end-to-end:
//!   1. Try STRICT (mlock on); fall back to LENIENT if the host
//!      lacks `IPC_LOCK` capability (typical on macOS dev machines).
//!   2. Allocate three fake credentials into a slab arena.
//!   3. Hex-dump slot 0's raw bytes (via the diagnostic accessor)
//!      to show the credential content sitting in memory.
//!   4. Free that credential.
//!   5. Hex-dump the *same* slot again to prove the bytes are now zero.
//!   6. Re-allocate to demonstrate slot recycling.
//!
//! The unsafe raw-pointer read uses
//! [`ArenaHandle::__raw_slot_bytes_for_diagnostics`], a hidden,
//! research-only accessor that bypasses the generation check. It is
//! NOT part of the stable public API.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::single_match_else)]

use sentinel_broker::{ArenaHandle, Broker, SecretPolicy};

/// Width (in bytes) of each credential slot.
const SLOT_BYTES: usize = 64;
/// Number of credential slots in the vault.
const SLOT_COUNT: u32 = 8;

/// Print a labeled hex+ASCII dump of the first `SLOT_BYTES` bytes
/// pointed to by `ptr`. The caller is responsible for ensuring the
/// pointer is valid for at least `len` bytes (see the unsafe block
/// at the call site).
fn hex_dump(label: &str, bytes: &[u8]) {
    print!("    {label:20} ");
    for (i, b) in bytes.iter().enumerate() {
        if i == 16 {
            print!(" ");
        }
        print!("{b:02x} ");
        if i == 31 {
            break;
        }
    }
    print!(" |");
    for b in bytes.iter().take(32) {
        let c = if (0x20..=0x7e).contains(b) { *b as char } else { '.' };
        print!("{c}");
    }
    println!("|");
}

/// Pack a credential (label + secret) into a fixed-width 64-byte buffer
/// padded with zeros. Returns a stack-allocated array suitable for slab
/// allocation.
fn pack_credential(user: &str, secret: &str) -> [u8; SLOT_BYTES] {
    let mut buf = [0u8; SLOT_BYTES];
    let combined = format!("{user}:{secret}");
    let take = combined.len().min(SLOT_BYTES);
    buf[..take].copy_from_slice(&combined.as_bytes()[..take]);
    buf
}

/// Try to construct a STRICT secret slab arena. If mlock is refused
/// (e.g., the dev machine has no `IPC_LOCK`), narrate the fallback and
/// retry with LENIENT.
fn build_vault(broker: &Broker) -> ArenaHandle {
    // Try STRICT first (mlock + zero_on_free + zero_on_destroy). If
    // SecretStrategy::wrap reports an mlock failure (typical on macOS
    // dev machines without `ulimit -l unlimited`), narrate the fallback
    // and retry with LENIENT.
    match broker
        .arena("vault")
        .secret(SecretPolicy::STRICT)
        .try_slab(SLOT_BYTES, 8, SLOT_COUNT)
    {
        Ok(h) => {
            println!("  policy: STRICT (mlock active)");
            h
        }
        Err(e) => {
            println!("  policy: STRICT unavailable ({e}); falling back to LENIENT");
            println!("          (this is expected on macOS dev machines without `ulimit -l unlimited`)");
            broker
                .arena("vault")
                .secret(SecretPolicy::LENIENT)
                .try_slab(SLOT_BYTES, 8, SLOT_COUNT)
                .expect("LENIENT slab construction should never fail")
        }
    }
}

fn main() {
    println!("== credential_store demo ==");
    let broker = Broker::new();
    let vault = build_vault(&broker);
    println!(
        "  vault arena: id={} capacity={} bytes ({} slots x {} bytes)",
        vault.id(),
        vault.capacity(),
        SLOT_COUNT,
        SLOT_BYTES,
    );

    // Phase 1: store three credentials.
    let h_alice = vault.alloc(pack_credential("alice", "hunter2")).expect("alloc alice");
    let h_bob = vault.alloc(pack_credential("bob", "letmein")).expect("alloc bob");
    let h_carol = vault.alloc(pack_credential("carol", "p@ssw0rd!"))
        .expect("alloc carol");

    println!(
        "\n  stored 3 credentials: alice@{}, bob@{}, carol@{}",
        h_alice.slot(),
        h_bob.slot(),
        h_carol.slot(),
    );

    // Phase 2: dump alice's slot from raw memory to show the bytes are really there.
    let alice_slot = h_alice.slot();
    println!("\n  ── alice's slot BEFORE free (raw memory) ──");
    {
        // SAFETY: Four conditions:
        //   (1) `vault` (and its inner Arc<Arena>) is alive for the
        //       duration of this block, so the returned pointer is
        //       valid.
        //   (2) We hold no aliasing mutable reference to the slot
        //       (we only have `h_alice`, which is a generational
        //       handle, not a raw pointer).
        //   (3) No other thread is allocating into this slot
        //       concurrently — this demo is single-threaded.
        //   (4) We only read, never write, through this pointer.
        let (ptr, len) = vault
            .__raw_slot_bytes_for_diagnostics(alice_slot)
            .expect("diagnostic accessor returned None");
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        hex_dump("alice", bytes);
    }

    // Phase 3: free alice's credential. Zero-on-free policy will wipe the slot.
    println!("\n  freeing alice's credential...");
    vault.free(&h_alice).expect("free alice");

    // Phase 4: dump the same slot. With zero_on_free, every byte must be zero.
    println!("\n  ── alice's slot AFTER free (raw memory) ──");
    let all_zero = {
        // SAFETY: same as above — the arena still owns the backing
        // memory (free returns the slot to the freelist; it does
        // not deallocate the backing buffer).
        let (ptr, len) = vault
            .__raw_slot_bytes_for_diagnostics(alice_slot)
            .expect("diagnostic accessor returned None");
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        hex_dump("alice (post-free)", bytes);
        bytes.iter().all(|&b| b == 0)
    };

    if all_zero {
        println!("\n  ✓ VERIFIED: all {SLOT_BYTES} bytes of the freed slot are zero.");
        println!("    Zero-on-free policy is enforced.");
    } else {
        println!("\n  ✗ FAILURE: freed slot still contains non-zero bytes!");
        std::process::exit(1);
    }

    // Phase 5: re-allocate to demonstrate slot recycling.
    let h_dave = vault.alloc(pack_credential("dave", "correctbatterystaple"))
        .expect("alloc dave");
    println!(
        "\n  re-allocated: dave -> {} (same slot index as alice: {})",
        h_dave.slot(),
        h_dave.slot() == alice_slot,
    );
    assert_eq!(h_dave.slot(), alice_slot, "slab should recycle alice's slot");

    // Phase 6: stats summary.
    let stats = broker.stats();
    println!(
        "\n  broker stats: live_arenas={} allocations={} frees={}",
        stats.live_arenas, stats.total_allocations, stats.total_frees,
    );
    assert_eq!(stats.total_allocations, 4, "alice + bob + carol + dave = 4");

    // Cleanup.
    broker.destroy_arena(vault.id()).expect("destroy vault");
    let _ = (h_bob, h_carol, h_dave); // suppress unused warnings post-destroy

    println!("\n  demo OK");
}
