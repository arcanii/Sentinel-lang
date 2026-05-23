//! Request-pipeline demo: scoped per-request bump arenas, exercising
//! `within_budget` and recorder-based event tracing.
//!
//! Each request opens its own budget scope (capping total bytes the
//! request may allocate). All chunks live inside a single per-request
//! bump arena. At end of main we destroy every arena via the broker.

use sentinel_broker::{Broker, BrokerError, Event, Recorder};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct RequestCtx {
    req_id: u64,
    payload_size: usize,
}

fn handle_request(broker: &Broker, ctx: RequestCtx, budget: usize) -> Result<u64, BrokerError> {
    broker.within_budget(budget, |scope| {
        let arena = scope
            .arena(&format!("req-{}", ctx.req_id))
            .capacity(4096)
            .bump()?;

        // Allocate payload as 128-byte chunks. div_ceil to round up.
        let chunks = ctx.payload_size.div_ceil(128);
        let mut handles = Vec::with_capacity(chunks);
        for i in 0..chunks {
            let buf = [u8::try_from(i % 256).unwrap(); 128];
            let h = arena.alloc(buf)?;
            handles.push(h);
        }

        // Cheap "checksum": sum of first byte of each chunk.
        let mut checksum = 0u64;
        for h in &handles {
            let guard = h.get()?;
            checksum = checksum.wrapping_add(u64::from((*guard)[0]));
        }
        Ok(checksum)
    })
}

fn main() {
    println!("== request_pipeline demo ==");
    let rec = Recorder::with_capacity(1024);
    let broker = Broker::with_recorder(Arc::clone(&rec));

    let reqs = [
        RequestCtx { req_id: 1, payload_size: 256 },
        RequestCtx { req_id: 2, payload_size: 1024 },
        RequestCtx { req_id: 3, payload_size: 2048 },
    ];

    for r in &reqs {
        let budget = 8 * 1024;
        match handle_request(&broker, *r, budget) {
            Ok(ck) => println!("  req {} -> checksum={}", r.req_id, ck),
            Err(e) => println!("  req {} -> error: {:?}", r.req_id, e),
        }
    }

    // Demonstrate budget rejection: request an arena bigger than the
    // budget cap. Budget pre-charges arena capacity, so this is
    // rejected at builder time, before any allocations happen.
    let r = broker.within_budget(8 * 1024, |scope| {
        let _ = scope.arena("req-99-toobig").capacity(16 * 1024).bump()?;
        Ok(())
    });
    match r {
        Err(BrokerError::BudgetExceeded { .. }) => {
            println!("  req 99 -> rejected (budget exceeded, as expected)");
        }
        other => println!("  req 99 -> UNEXPECTED: {other:?}"),
    }

    // Tear down everything via the broker.
    let arenas: Vec<_> = broker.list_arenas().iter().map(|a| a.id).collect();
    let n_destroyed = arenas.len();
    for id in arenas {
        broker.destroy_arena(id).expect("destroy_arena");
    }

    // Inspect the recorded event stream.
    let events = rec.snapshot();
    let n_events = events.len();
    println!("  recorded {n_events} events total");

    let mut n_created = 0;
    let mut n_destroyed_ev = 0;
    let mut n_allocated = 0;
    let mut n_budget_open = 0;
    let mut n_budget_close = 0;
    for e in &events {
        match e {
            Event::ArenaCreated { .. } => n_created += 1,
            Event::ArenaDestroyed { .. } => n_destroyed_ev += 1,
            Event::Allocated { .. } => n_allocated += 1,
            Event::BudgetOpened { .. } => n_budget_open += 1,
            Event::BudgetClosed { .. } => n_budget_close += 1,
            Event::Freed { .. } => {}
        }
    }
    println!(
        "  breakdown: created={n_created} destroyed={n_destroyed_ev} allocated={n_allocated} budget_open={n_budget_open} budget_close={n_budget_close}"
    );
    assert_eq!(n_created, 3, "3 successful requests should create 3 arenas");
    assert_eq!(n_destroyed_ev, n_destroyed);
    assert_eq!(n_budget_open, 4, "4 within_budget calls (3 OK + 1 rejected)");
    assert_eq!(n_budget_close, 4);
    println!("  demo OK");
}
