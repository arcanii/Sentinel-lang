#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "PATCHING arena.rs (with_strategy + with_strategy_and_recorder)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
changed = False

# 1) Replace the entire with_strategy body to (a) init recorder: None and
#    (b) be followed by with_strategy_and_recorder.
old = """    pub(crate) fn with_strategy(id: ArenaId, name: &str, strategy: Box<dyn AllocStrategy>) -> Arc<Self> {
        Arc::new(Self {
            id,
            name: name.into(),
            strategy,
            generation: AtomicU32::new(Generation::INITIAL.raw()),
            alloc_count: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
        })
    }"""

new = """    pub(crate) fn with_strategy(id: ArenaId, name: &str, strategy: Box<dyn AllocStrategy>) -> Arc<Self> {
        Self::with_strategy_and_recorder(id, name, strategy, None)
    }

    /// Same as `with_strategy`, but attaches a recorder for event emission.
    pub(crate) fn with_strategy_and_recorder(
        id: ArenaId,
        name: &str,
        strategy: Box<dyn AllocStrategy>,
        recorder: Option<Arc<Recorder>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            name: name.into(),
            strategy,
            generation: AtomicU32::new(Generation::INITIAL.raw()),
            alloc_count: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
            recorder,
        })
    }"""

if old in src:
    src = src.replace(old, new, 1)
    changed = True
    print("[OK] rewrote with_strategy + added with_strategy_and_recorder")
else:
    print("[WARN] could not find original with_strategy body verbatim; check shape")

if changed:
    p.write_text(src)
    print("[OK] arena.rs updated")
PY

echo
echo "======"
echo "PATCHING broker.rs (emit Event::ArenaCreated in register_arena)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()

if "Event::ArenaCreated" in src:
    print("[SKIP] ArenaCreated already emitted")
else:
    old = """    pub(crate) fn register_arena(&self, arena: Arc<Arena>) {
        let id = arena.id();
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, arena);
        }
        tracing::debug!(arena_id = %id, "arena registered");
    }"""
    new = """    pub(crate) fn register_arena(&self, arena: Arc<Arena>) {
        let id = arena.id();
        let summary = (
            arena.name().to_string(),
            arena.strategy_kind(),
            arena.capacity(),
        );
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, arena);
        }
        tracing::debug!(arena_id = %id, "arena registered");
        if let Some(r) = &self.recorder {
            r.record(Event::ArenaCreated {
                id,
                name: summary.0,
                kind: summary.1,
                capacity: summary.2,
                at_ns: r.now_ns(),
            });
        }
    }"""
    if old in src:
        src = src.replace(old, new, 1)
        p.write_text(src)
        print("[OK] emit Event::ArenaCreated from register_arena")
    else:
        print("[WARN] register_arena body shape unexpected; please paste broker.rs lines 130-150")
PY

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 25

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
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 70
else
  cargo test -p sentinel-broker 2>&1 | tail -n 70
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
