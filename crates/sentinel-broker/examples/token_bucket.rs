#![allow(clippy::cast_precision_loss)]
//! Token-bucket demo: high-frequency slab allocation with
//! generation recycling and `where_is` lookups.

use sentinel_broker::{Broker};

#[derive(Clone)]
#[allow(dead_code)]
struct Token {
    id: u64,
    payload: [u8; 32],
}

fn main() {
    println!("== token_bucket demo ==");
    let broker = Broker::new();
    let arena = broker.arena("tokens").slab(
        std::mem::size_of::<Token>(),
        std::mem::align_of::<Token>(),
        128,
    );

    // Cycle 128 tokens through the slab 800x -> 102,400 alloc/free pairs.
    let cycles = 800u64;
    let per_cycle = 128u64;
    let mut total_issued = 0u64;
    let start = std::time::Instant::now();

    for cycle in 0..cycles {
        let mut handles = Vec::with_capacity(usize::try_from(per_cycle).unwrap());
        for i in 0..per_cycle {
            let tok = Token {
                id: total_issued,
                payload: [u8::try_from(cycle % 256).unwrap(); 32],
            };
            let h = arena.alloc(tok).expect("slab has room");
            if cycle == 0 && i == 0 {
                if let Some(loc) = broker.where_is(&h) {
                    println!(
                        "  first token: arena={} slot={} gen={}",
                        loc.arena, loc.slot, loc.slot_generation,
                    );
                }
            }
            handles.push(h);
            total_issued += 1;
        }
        for h in &handles {
            arena.free(h).expect("free failed");
        }
    }

    let elapsed = start.elapsed();
    let recycling_ratio = (total_issued as f64) / 128.0;

    println!("  allocations: {total_issued} in {elapsed:.?}");
    println!("  recycling ratio: {recycling_ratio:.1}x (128 slots, reuse factor)");

    let stats = broker.stats();
    println!(
        "  broker stats: live_arenas={} total_allocations={} total_frees={}",
        stats.live_arenas, stats.total_allocations, stats.total_frees,
    );
    assert_eq!(stats.total_allocations, total_issued);
    assert_eq!(stats.total_frees, total_issued);

    broker.destroy_arena(arena.id()).unwrap();
    println!("  demo OK");
}
