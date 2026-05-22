#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

info(){ printf "[INFO] %s\n" "$*"; }
ok(){ printf "[OK]   %s\n" "$*"; }
err(){ printf "[ERR]  %s\n" "$*" >&2; }

# Use python for precise, idempotent in-place edits (sed on macOS is awkward).
python3 - "$BROKER" <<'PYEOF'
import sys, re, pathlib
broker = pathlib.Path(sys.argv[1])

def patch(path, transform, label):
    p = broker / path
    src = p.read_text()
    new = transform(src)
    if new == src:
        print(f"[SKIP] {path}: already patched ({label})")
        return
    p.write_text(new)
    print(f"[OK]   {path}: {label}")

# 1. Add #[derive(Debug)] to HandleRef in src/handle.rs
def fix_handle_ref(src):
    # Match the existing attributes/struct line for HandleRef and ensure Debug is derived.
    pattern = re.compile(r'(#\[derive\(([^)]*)\)\]\s*\n)(pub struct HandleRef\b)')
    m = pattern.search(src)
    if m:
        derives = [d.strip() for d in m.group(2).split(',') if d.strip()]
        if 'Debug' in derives:
            return src
        derives.append('Debug')
        new_attr = f"#[derive({', '.join(derives)})]\n"
        return src[:m.start()] + new_attr + m.group(3) + src[m.end():]
    # No existing derive — insert one above the struct line.
    pattern2 = re.compile(r'(?m)^(pub struct HandleRef\b)')
    return pattern2.sub(r'#[derive(Debug)]\n\1', src, count=1)
patch("src/handle.rs", fix_handle_ref, "derive(Debug) on HandleRef")

# 2. Silence dead_code for SlotInfo::size and ArenaId::raw rather than deleting them
#    (they're part of the public-ish surface we'll use soon).
def allow_dead_size(src):
    # Find `pub size: ...` inside struct SlotInfo and prepend #[allow(dead_code)] once.
    pattern = re.compile(r'(?m)^(\s*)(pub size:\s*[^,\n]+,)')
    def repl(m):
        indent, line = m.group(1), m.group(2)
        # Look one line above for an existing allow attribute.
        return f"{indent}#[allow(dead_code)]\n{indent}{line}"
    # Only patch if not already allowed.
    if "#[allow(dead_code)]\n    pub size:" in src or "#[allow(dead_code)]\n        pub size:" in src:
        return src
    return pattern.sub(repl, src, count=1)
patch("src/arena.rs", allow_dead_size, "allow(dead_code) on SlotInfo::size")

def allow_dead_raw(src):
    pattern = re.compile(r'(?m)^(\s*)(pub(?:\(crate\))?\s+fn raw\(&self\))')
    if re.search(r'#\[allow\(dead_code\)\]\s*\n\s*pub(?:\(crate\))?\s+fn raw\(&self\)', src):
        return src
    def repl(m):
        indent, sig = m.group(1), m.group(2)
        return f"{indent}#[allow(dead_code)]\n{indent}{sig}"
    return pattern.sub(repl, src, count=1)
patch("src/ids.rs", allow_dead_raw, "allow(dead_code) on ArenaId::raw")

# 3. Fix the proptest unwrap_err call site: T = HandleRef must be Debug.
#    The derive above covers it, but also ensure the test imports compile cleanly
#    by switching .unwrap_err() to a match if we ever can't derive Debug. For now,
#    derive(Debug) is sufficient — nothing to change in the test file itself.
PYEOF

info "Re-running broker checks…"
cd "$SENTINEL_ROOT"

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -40 || true

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -30 || \
  cargo test -p sentinel-broker 2>&1 | tail -30

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "FIXUP COMPLETE"
echo "======"
echo "If all four sections above are clean, commit with:"
echo "  cd $SENTINEL_ROOT"
echo "  git add -A"
echo "  git commit -m 'broker: phase A0+A1+A2 foundations and fixups'"
