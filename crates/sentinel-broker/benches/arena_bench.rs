//! Benchmarks for arena allocation overhead.
//!
//! These set the baseline for what the broker costs versus raw
//! allocation. Used to detect performance regressions in later
//! milestones.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sentinel_broker::Broker;

fn bench_alloc_u64(c: &mut Criterion) {
    let broker = Broker::new();
    c.bench_function("arena_alloc_u64", |b| {
        b.iter(|| {
            let arena = broker.create_arena("bench", 1024 * 1024);
            for i in 0_u64..1000 {
                let _h = arena.alloc(black_box(i)).unwrap();
            }
        });
    });
}

fn bench_alloc_and_read(c: &mut Criterion) {
    c.bench_function("arena_alloc_and_read", |b| {
        b.iter(|| {
            let broker = Broker::new();
            let arena = broker.create_arena("bench", 1024 * 1024);
            let handles: Vec<_> = (0_u64..1000)
                .map(|i| arena.alloc(i).unwrap())
                .collect();
            let mut sum = 0_u64;
            for h in &handles {
                sum = sum.wrapping_add(*h.get().unwrap());
            }
            black_box(sum)
        });
    });
}

criterion_group!(benches, bench_alloc_u64, bench_alloc_and_read);
criterion_main!(benches);