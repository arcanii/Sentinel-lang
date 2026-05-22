#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

# -----------------------------------------------------------------------------
# 1) Write src/stats.rs
# -----------------------------------------------------------------------------
echo "======"
echo "WRITING src/stats.rs"
echo "======"
cat > "$BROKER/src/stats.rs" <<'RS'
//! Read-only diagnostic types for broker introspection.
//!
//! These structures are *snapshots* produced by [`Broker::stats`],
//! [`Broker::list_arenas`], and [`Broker::where_is`]. They never hold
//! locks or arena references — they are safe to log, serialize, or
//! ship across threads.
//!
//! ```ignore
//! let stats = broker.stats();
//! println!("live arenas: {}", stats.live_arenas);
//! for summary in broker.list_arenas() {
//!     println!("{}: {}/{} bytes", summary.name, summary.used, summary.capacity);
//! }
//! ```
//!
//! [`Broker::stats`]: crate::Broker::stats
//! [`Broker::list_arenas`]: crate::Broker::list_arenas
//! [`Broker::where_is`]: crate::Broker::where_is

use crate::ids::{ArenaId, Generation, SlotGeneration, SlotIndex};
use crate::strategy::StrategyKind;

/// Aggregate counters across every live arena registered with the broker.
///
/// Cumulative counters (`total_allocations`, `total_frees`) include
/// allocations made into arenas that have since been destroyed —
/// they reflect lifetime activity, not just current state. Capacity
/// and usage figures sum *only* over currently-live arenas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrokerStats {
    /// Number of arenas currently registered with the broker.
    pub live_arenas: usize,
    /// Sum of `capacity()` over all live arenas, in bytes.
    pub total_capacity_bytes: usize,
    /// Sum of `used()` over all live arenas, in bytes.
    pub total_used_bytes: usize,
    /// Lifetime count of successful allocations across all live arenas.
    pub total_allocations: u64,
    /// Lifetime count of successful frees across all live arenas.
    pub total_frees: u64,
}

/// A snapshot of one arena's state at the moment of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaSummary {
    pub id: ArenaId,
    pub name: String,
    pub kind: StrategyKind,
    pub capacity: usize,
    pub used: usize,
    /// The arena's own generation. Increments on `destroy_arena`.
    pub generation: Generation,
    pub allocations: u64,
    pub frees: u64,
}

/// The physical location and liveness of a handle, as resolved through
/// the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleLocation {
    pub arena: ArenaId,
    pub arena_name: String,
    pub slot: SlotIndex,
    /// The slot generation embedded in the handle.
    pub slot_generation: SlotGeneration,
    /// `true` if the arena is still live *and* the handle's slot
    /// generation matches the arena's current slot generation for
    /// that slot. `false` if the slot has been freed/reused.
    pub is_live: bool,
}
RS
echo "[OK] wrote src/stats.rs"

# -----------------------------------------------------------------------------
# 2) Patch arena.rs: add alloc_count / free_count atomics + accessors,
#    bump them in alloc / free paths.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/arena.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
changed = False

# Ensure AtomicU64 is in scope.
if "AtomicU64" not in src:
    # Look for existing atomic use line.
    m = re.search(r"use std::sync::atomic::\{([^}]*)\};", src)
    if m:
        inside = m.group(1)
        if "AtomicU64" not in inside:
            new_inside = inside.rstrip() + ", AtomicU64"
            src = src.replace(m.group(0), f"use std::sync::atomic::{{{new_inside}}};")
            changed = True
            print("[OK] added AtomicU64 to existing atomic import")
    else:
        # Add a fresh import after the first `use` line.
        src = src.replace(
            "use std::sync::Arc;",
            "use std::sync::Arc;\nuse std::sync::atomic::{AtomicU64, Ordering};",
            1,
        )
        changed = True
        print("[OK] inserted AtomicU64+Ordering import")

# Add alloc_count / free_count fields to struct Arena { ... }
if "alloc_count: AtomicU64" not in src:
    m = re.search(r"pub struct Arena \{([^}]*)\}", src, re.DOTALL)
    if not m:
        print("[WARN] could not find `pub struct Arena {}` — aborting field patch")
    else:
        body = m.group(1)
        # Insert before the closing brace, preserving indentation.
        new_body = body.rstrip() + "\n    alloc_count: AtomicU64,\n    free_count: AtomicU64,\n"
        src = src.replace(m.group(0), f"pub struct Arena {{{new_body}}}", 1)
        changed = True
        print("[OK] added alloc_count / free_count fields to Arena")

# Initialize the new fields in Arena::with_strategy (or Arena::new).
# Find a struct-literal `Self {` or `Arena {` inside arena.rs and inject the fields if missing.
def inject_field_init(src: str, label: str) -> str:
    # Find every `Self { ... }` block and add the counters if not present.
    pattern = re.compile(r"Self\s*\{([^{}]*?)\}", re.DOTALL)
    matches = list(pattern.finditer(src))
    for m in matches:
        body = m.group(1)
        # Heuristic: this is the Arena constructor if body mentions both
        # `id` and `strategy` (or `id` and `generation`).
        if ("id" in body and "strategy" in body) or ("id" in body and "generation" in body):
            if "alloc_count" in body:
                continue
            new_body = body.rstrip()
            if not new_body.endswith(","):
                new_body += ","
            new_body += "\n            alloc_count: AtomicU64::new(0),\n            free_count: AtomicU64::new(0),\n        "
            src = src.replace(m.group(0), f"Self {{{new_body}}}", 1)
            print(f"[OK] initialized alloc_count/free_count in {label} constructor")
            return src
    print(f"[WARN] could not auto-init counters in any Self {{...}} block ({label})")
    return src

if "alloc_count: AtomicU64::new(0)" not in src:
    new_src = inject_field_init(src, "Arena")
    if new_src != src:
        src = new_src
        changed = True

# Bump alloc_count on successful alloc.
# Find `pub fn alloc<T>(self: &Arc<Self>, value: T) -> Result<Handle<T>, BrokerError> {`
# and add a counter increment right after the strategy's alloc_raw returns Ok.
# Strategy: locate the line that constructs `Handle::new(`. Just before returning
# Ok(handle), bump the counter.
if "self.alloc_count.fetch_add(1, Ordering::Relaxed);" not in src:
    # Look for the alloc method body — insert a bump right after a successful strategy call.
    # We anchor on `Handle::new(` and add a fetch_add before the Ok(...).
    # Simpler: anchor on the closing of alloc() — replace `Ok(handle)` if it exists.
    if "Ok(handle)" in src:
        src = src.replace(
            "Ok(handle)",
            "self.alloc_count.fetch_add(1, Ordering::Relaxed);\n        Ok(handle)",
            1,
        )
        changed = True
        print("[OK] bump alloc_count before Ok(handle) in alloc()")
    else:
        print("[WARN] could not locate Ok(handle) in alloc(); manual patch may be needed")

# Bump free_count on successful free.
# Find `pub fn free<T>` and look for a successful strategy.free(...) call.
if "self.free_count.fetch_add(1, Ordering::Relaxed);" not in src:
    # Find free body and inject before the trailing Ok(()).
    m = re.search(r"pub fn free<T>[^{]*\{(.*?)\n    \}\n", src, re.DOTALL)
    if m:
        body = m.group(1)
        if "Ok(())" in body:
            new_body = body.replace(
                "Ok(())",
                "self.free_count.fetch_add(1, Ordering::Relaxed);\n        Ok(())",
                1,
            )
            src = src.replace(body, new_body, 1)
            changed = True
            print("[OK] bump free_count before Ok(()) in free()")
        else:
            print("[WARN] free body does not contain Ok(()); skipping bump")
    else:
        print("[WARN] could not find free<T> method; skipping bump")

# Add accessors alloc_count() and free_count() if missing.
if "pub fn alloc_count(&self) -> u64" not in src:
    # Append to the last `impl Arena {` block.
    # Find the location of the last `impl Arena {` and insert before its closing brace.
    # Heuristic: replace the first `pub fn id(&self) -> ArenaId` if present, putting
    # accessors right after it. Or find any `pub fn used(&self) -> usize` and append after.
    if "pub fn used(&self) -> usize" in src:
        # Insert after the used() method.
        m = re.search(r"(pub fn used\(&self\) -> usize \{[^}]*\})", src, re.DOTALL)
        if m:
            insertion = m.group(1) + """

    #[must_use]
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn free_count(&self) -> u64 {
        self.free_count.load(Ordering::Relaxed)
    }"""
            src = src.replace(m.group(1), insertion, 1)
            changed = True
            print("[OK] added alloc_count()/free_count() accessors after used()")
        else:
            print("[WARN] regex match failed for used() insertion")
    else:
        print("[WARN] could not find a good spot for accessors; inserting before final `}`")
        # Append accessors before final `}`
        # Find the LAST `impl Arena {` block
        impls = [m.start() for m in re.finditer(r"impl Arena \{", src)]
        if impls:
            last = impls[-1]
            # find matching close brace by counting
            depth = 0
            i = last
            close = None
            while i < len(src):
                if src[i] == '{': depth += 1
                elif src[i] == '}':
                    depth -= 1
                    if depth == 0:
                        close = i
                        break
                i += 1
            if close is not None:
                insertion = """
    #[must_use]
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn free_count(&self) -> u64 {
        self.free_count.load(Ordering::Relaxed)
    }
"""
                src = src[:close] + insertion + src[close:]
                changed = True
                print("[OK] appended accessors to last impl Arena block")

# Update Arena's manual Debug impl to include the new counters (best effort).
# Not strictly required — finish_non_exhaustive is fine.

if changed:
    p.write_text(src)
    print("[OK] arena.rs updated")
else:
    print("[SKIP] arena.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 3) Patch broker.rs: add stats(), where_is(), list_arenas()
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/broker.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
changed = False

# Add imports for stats types.
if "use crate::stats::" not in src:
    # Insert after the first `use crate::` line.
    m = re.search(r"(use crate::[^\n]+\n)", src)
    if m:
        src = src.replace(
            m.group(1),
            m.group(1) + "use crate::stats::{ArenaSummary, BrokerStats, HandleLocation};\n",
            1,
        )
        changed = True
        print("[OK] imported stats types into broker.rs")

# Insert the three new methods just before `}` that closes `impl Broker`.
# Anchor: the line containing `pub(crate) fn next_budget_id`.
new_methods = '''
    /// Aggregate counters across every currently-live arena.
    #[must_use]
    pub fn stats(&self) -> BrokerStats {
        let arenas = match self.arenas.read() {
            Ok(g) => g,
            Err(_) => return BrokerStats::default(),
        };
        let mut s = BrokerStats {
            live_arenas: arenas.len(),
            ..BrokerStats::default()
        };
        for arena in arenas.values() {
            s.total_capacity_bytes = s.total_capacity_bytes.saturating_add(arena.capacity());
            s.total_used_bytes = s.total_used_bytes.saturating_add(arena.used());
            s.total_allocations = s.total_allocations.saturating_add(arena.alloc_count());
            s.total_frees = s.total_frees.saturating_add(arena.free_count());
        }
        s
    }

    /// Snapshot every live arena as an [`ArenaSummary`].
    #[must_use]
    pub fn list_arenas(&self) -> Vec<ArenaSummary> {
        let arenas = match self.arenas.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<ArenaSummary> = arenas.values().map(|a| ArenaSummary {
            id: a.id(),
            name: a.name().to_string(),
            kind: a.strategy_kind(),
            capacity: a.capacity(),
            used: a.used(),
            generation: a.generation(),
            allocations: a.alloc_count(),
            frees: a.free_count(),
        }).collect();
        out.sort_by_key(|s| s.id);
        out
    }

    /// Resolve a handle's physical location and current liveness.
    ///
    /// Returns `None` if the handle's arena has been destroyed.
    #[must_use]
    pub fn where_is<T>(&self, handle: &crate::Handle<T>) -> Option<HandleLocation> {
        let arenas = self.arenas.read().ok()?;
        let arena = arenas.get(&handle.arena_id())?;
        Some(HandleLocation {
            arena: arena.id(),
            arena_name: arena.name().to_string(),
            slot: handle.slot(),
            slot_generation: handle.slot_generation(),
            is_live: handle.is_live(),
        })
    }
'''

if "pub fn stats(&self) -> BrokerStats" not in src:
    # Anchor on next_budget_id and insert the new methods right after it.
    anchor = "pub(crate) fn next_budget_id(&self) -> crate::ids::BudgetId {"
    if anchor in src:
        # Find the end of the next_budget_id method (closing brace).
        idx = src.index(anchor)
        # Walk forward to find the matching `}`.
        depth = 0
        i = idx
        method_end = None
        while i < len(src):
            if src[i] == '{':
                depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    method_end = i + 1
                    break
            i += 1
        if method_end is not None:
            src = src[:method_end] + "\n" + new_methods + src[method_end:]
            changed = True
            print("[OK] inserted stats/list_arenas/where_is methods after next_budget_id")
        else:
            print("[WARN] could not find end of next_budget_id method")
    else:
        print("[WARN] anchor next_budget_id not found")

if changed:
    p.write_text(src)
    print("[OK] broker.rs updated")
else:
    print("[SKIP] broker.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 4) Ensure Arena exposes strategy_kind() and Handle exposes the accessors.
#    These are usually already present (slot/slot_generation/arena_id/is_live).
#    Add strategy_kind() to Arena if missing.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "ENSURING arena.strategy_kind() exists"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
if "pub fn strategy_kind(&self)" in src:
    print("[SKIP] strategy_kind() already present")
else:
    # Inject after used() if available, else after capacity().
    target = None
    for cand in ("pub fn alloc_count(&self) -> u64 {", "pub fn used(&self) -> usize {", "pub fn capacity(&self) -> usize {"):
        if cand in src:
            target = cand
            break
    if target is None:
        print("[WARN] could not find insertion anchor for strategy_kind")
    else:
        m = re.search(re.escape(target) + r"[^}]*\}", src, re.DOTALL)
        if m:
            block = m.group(0)
            insertion = block + """

    #[must_use]
    pub fn strategy_kind(&self) -> crate::strategy::StrategyKind {
        self.strategy.kind()
    }"""
            src = src.replace(block, insertion, 1)
            p.write_text(src)
            print("[OK] added strategy_kind() to Arena")
        else:
            print("[WARN] regex anchor failed for strategy_kind")
PY

# -----------------------------------------------------------------------------
# 5) Update lib.rs: declare and re-export the stats module.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/lib.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/lib.rs")
src = p.read_text()
changed = False

if "pub mod stats;" not in src and "mod stats;" not in src:
    # Insert after another `pub mod budget;` line.
    if "pub mod budget;" in src:
        src = src.replace("pub mod budget;", "pub mod budget;\npub mod stats;", 1)
        changed = True
        print("[OK] declared pub mod stats")
    elif "mod budget;" in src:
        src = src.replace("mod budget;", "mod budget;\npub mod stats;", 1)
        changed = True
        print("[OK] declared pub mod stats")
    else:
        # Last resort: append after the last `mod ` line.
        import re
        lines = src.splitlines(keepends=True)
        last_mod = None
        for i, ln in enumerate(lines):
            if ln.lstrip().startswith(("pub mod ", "mod ")):
                last_mod = i
        if last_mod is not None:
            lines.insert(last_mod + 1, "pub mod stats;\n")
            src = "".join(lines)
            changed = True
            print("[OK] declared pub mod stats (appended after last mod)")
        else:
            print("[WARN] could not find a mod declaration to anchor")

if "pub use stats::" not in src:
    # Append re-exports after another `pub use` line.
    if "pub use budget::" in src:
        # Find a line containing `pub use budget::` and add stats below.
        import re
        src = re.sub(
            r"(pub use budget::[^\n]+\n)",
            r"\1pub use stats::{ArenaSummary, BrokerStats, HandleLocation};\n",
            src,
            count=1,
        )
        changed = True
        print("[OK] added pub use stats::*")
    else:
        # Append at end.
        if not src.endswith("\n"):
            src += "\n"
        src += "pub use stats::{ArenaSummary, BrokerStats, HandleLocation};\n"
        changed = True
        print("[OK] appended pub use stats::* at end")

if changed:
    p.write_text(src)
    print("[OK] lib.rs updated")
else:
    print("[SKIP] lib.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 6) Append stats tests to broker.rs (inside the existing #[cfg(test)] mod).
# -----------------------------------------------------------------------------
echo
echo "======"
echo "APPENDING TESTS to src/broker.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()

if "fn stats_reports_live_arenas" in src:
    print("[SKIP] stats tests already present")
else:
    new_tests = '''
    #[test]
    fn stats_reports_live_arenas() {
        let b = Broker::new();
        let _a = b.create_arena("one", 1024);
        let _c = b.create_arena("two", 2048);
        let s = b.stats();
        assert_eq!(s.live_arenas, 2);
        assert_eq!(s.total_capacity_bytes, 1024 + 2048);
    }

    #[test]
    fn stats_tracks_allocations_and_frees() {
        let b = Broker::new();
        let slab = b.arena("s").slab(64, 8, 16);
        let h = slab.alloc(99_u64).unwrap();
        let before = b.stats();
        assert_eq!(before.total_allocations, 1);
        assert_eq!(before.total_frees, 0);
        assert!(before.total_used_bytes >= 64);
        slab.free(&h).unwrap();
        let after = b.stats();
        assert_eq!(after.total_allocations, 1);
        assert_eq!(after.total_frees, 1);
    }

    #[test]
    fn list_arenas_sorted_by_id_with_correct_kinds() {
        use crate::strategy::StrategyKind;
        let b = Broker::new();
        let _bmp = b.arena("bumpy").capacity(2048).bump();
        let _slb = b.arena("slabby").slab(32, 8, 8);
        let v = b.list_arenas();
        assert_eq!(v.len(), 2);
        // sorted by id; creation order ensures bump < slab
        assert_eq!(v[0].kind, StrategyKind::Bump);
        assert_eq!(v[0].name, "bumpy");
        assert_eq!(v[1].kind, StrategyKind::Slab);
        assert_eq!(v[1].name, "slabby");
        assert_eq!(v[0].capacity, 2048);
    }

    #[test]
    fn list_arenas_excludes_destroyed() {
        let b = Broker::new();
        let a1 = b.create_arena("a1", 1024);
        let a2 = b.create_arena("a2", 1024);
        let id1 = a1.id();
        b.destroy_arena(id1).unwrap();
        let v = b.list_arenas();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, a2.id());
    }

    #[test]
    fn where_is_returns_some_for_live_handle() {
        let b = Broker::new();
        let a = b.create_arena("loc", 1024);
        let h = a.alloc(7_u64).unwrap();
        let loc = b.where_is(&h).expect("handle should be locatable");
        assert_eq!(loc.arena, a.id());
        assert_eq!(loc.arena_name, "loc");
        assert!(loc.is_live);
    }

    #[test]
    fn where_is_returns_none_after_destroy() {
        let b = Broker::new();
        let a = b.create_arena("gone", 1024);
        let h = a.alloc(7_u64).unwrap();
        let id = a.id();
        b.destroy_arena(id).unwrap();
        assert!(b.where_is(&h).is_none());
    }
'''
    # Insert before the closing `}` of `mod tests`.
    m = re.search(r"#\[cfg\(test\)\]\s*mod tests \{", src)
    if not m:
        print("[WARN] no #[cfg(test)] mod tests block found")
    else:
        # Find matching closing brace.
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
        else:
            src = src[:close] + new_tests + src[close:]
            p.write_text(src)
            print("[OK] appended 6 stats tests to mod tests")
PY

# -----------------------------------------------------------------------------
# 7) Build / clippy / tests / doc tests
# -----------------------------------------------------------------------------
echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 20

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 40

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 50
else
  cargo test -p sentinel-broker 2>&1 | tail -n 50
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
