#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "PATCHING arena.rs (alloc + free counters)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
changed = False

# 1) alloc: insert bump before `Ok(Handle::new(`
if "self.alloc_count.fetch_add(1, Ordering::Relaxed);" not in src:
    needle = "        Ok(Handle::new(\n            self.id,\n            allocated.slot,\n            self.generation(),\n            allocated.generation,\n            Arc::downgrade(self),\n        ))"
    if needle in src:
        replacement = "        self.alloc_count.fetch_add(1, Ordering::Relaxed);\n" + needle
        src = src.replace(needle, replacement, 1)
        changed = True
        print("[OK] inserted alloc_count bump before Ok(Handle::new(...))")
    else:
        print("[WARN] could not find Ok(Handle::new(...)) block")

# 2) free: replace tail call so it bumps free_count on Ok.
if "self.free_count.fetch_add(1, Ordering::Relaxed);" not in src:
    old = "        self.strategy.free(handle.slot, handle.slot_generation)\n    }"
    new = """        let r = self.strategy.free(handle.slot, handle.slot_generation);
        if r.is_ok() {
            self.free_count.fetch_add(1, Ordering::Relaxed);
        }
        r
    }"""
    if old in src:
        src = src.replace(old, new, 1)
        changed = True
        print("[OK] wrapped strategy.free to bump free_count on success")
    else:
        print("[WARN] could not find strategy.free tail call")

if changed:
    p.write_text(src)
    print("[OK] arena.rs updated")
else:
    print("[SKIP] arena.rs unchanged")
PY

echo
echo "======"
echo "PATCHING broker.rs (let-else)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
changed = False

# Fix 1: stats() — match -> let Ok(arenas) = ... else return default
old1 = """        let arenas = match self.arenas.read() {
            Ok(g) => g,
            Err(_) => return BrokerStats::default(),
        };"""
new1 = """        let Ok(arenas) = self.arenas.read() else {
            return BrokerStats::default();
        };"""
if old1 in src:
    src = src.replace(old1, new1, 1)
    changed = True
    print("[OK] rewrote stats() match as let-else")
else:
    print("[WARN] could not find stats() match block")

# Fix 2: list_arenas() — match -> let Ok(arenas) = ... else return Vec::new()
old2 = """        let arenas = match self.arenas.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };"""
new2 = """        let Ok(arenas) = self.arenas.read() else {
            return Vec::new();
        };"""
if old2 in src:
    src = src.replace(old2, new2, 1)
    changed = True
    print("[OK] rewrote list_arenas() match as let-else")
else:
    print("[WARN] could not find list_arenas() match block")

if changed:
    p.write_text(src)
    print("[OK] broker.rs updated")
else:
    print("[SKIP] broker.rs unchanged")
PY

echo
echo "======"
echo "ARENA.RS — alloc + free (post-fix preview)"
echo "======"
sed -n '60,110p' "$BROKER/src/arena.rs"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 15

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 30

echo
echo "======"
echo "TESTS"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 60
else
  cargo test -p sentinel-broker 2>&1 | tail -n 60
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
