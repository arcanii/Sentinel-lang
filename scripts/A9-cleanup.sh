#!/usr/bin/env bash
# A9-cleanup.sh - Phase A carry-over.
# Idempotent. Safe to re-run.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -d crates/sentinel-broker ]]; then
  echo "ERROR: not at repo root. Set REPO_ROOT=/path/to/sentinel" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== A9 PATCH START"
echo "Repo: $REPO_ROOT"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

cat > /tmp/sentinel_a9_patch.py <<'PYEOF'
#!/usr/bin/env python3
import re
from pathlib import Path

ROOT = Path.cwd()
BROKER = ROOT / "crates" / "sentinel-broker"
SRC = BROKER / "src"

def read(p):
    return p.read_text() if p.exists() else ""

def write(p, content):
    if not p.exists():
        print(f"  CREATE {p.relative_to(ROOT)}")
        p.write_text(content)
        return
    cur = p.read_text()
    if cur == content:
        print(f"  UNCHANGED {p.relative_to(ROOT)}")
        return
    print(f"  UPDATE {p.relative_to(ROOT)}")
    p.write_text(content)

def patch_error_rs():
    p = SRC / "error.rs"
    txt = read(p)
    if not txt:
        print(f"  SKIP error.rs: not found")
        return

    new = re.sub(
        r"#\[derive\(([^)]*)\)\]\s*\npub enum BrokerError",
        lambda m: "#[derive(" + ", ".join(
            t.strip() for t in m.group(1).split(",") if t.strip() != "Copy"
        ) + ")]\npub enum BrokerError",
        txt,
        count=1,
    )

    if "os_errno" not in new:
        new = re.sub(
            r"SecretMemory\s*\{\s*reason:\s*&'static str\s*\}",
            "SecretMemory { reason: String, os_errno: Option<i32> }",
            new,
            count=1,
        )

    if new != txt:
        write(p, new)
    else:
        print(f"  UNCHANGED error.rs")

def patch_callers_of_secret_memory():
    for p in SRC.rglob("*.rs"):
        txt = read(p)
        if "SecretMemory" not in txt:
            continue
        new = re.sub(
            r'BrokerError::SecretMemory\s*\{\s*reason:\s*"([^"]*)"\s*\}',
            r'BrokerError::SecretMemory { reason: "\1".to_string(), os_errno: None }',
            txt,
        )
        new = re.sub(
            r'BrokerError::SecretMemory\s*\{\s*reason:\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\}',
            r'BrokerError::SecretMemory { reason: \1.to_string(), os_errno: None }',
            new,
        )
        if new != txt:
            write(p, new)

def patch_builder_rs():
    p = SRC / "builder.rs"
    txt = read(p)
    if not txt:
        print(f"  SKIP builder.rs: not found")
        return
    if "fn try_bump" in txt and "fn try_slab" in txt:
        print(f"  UNCHANGED builder.rs (try_bump/try_slab already present)")
        return
    marker = "// === A9: fallible builder variants ==="
    if marker in txt:
        print(f"  UNCHANGED builder.rs (A9 marker present)")
        return
    addendum = "\n\n" + marker + "\n"
    addendum += "// try_bump / try_slab are fallible counterparts of bump() / slab().\n"
    addendum += "// They return BrokerError::SecretMemory instead of panicking when\n"
    addendum += "// SecretStrategy::wrap fails. See BACKLOG.md section 0.1.\n"
    addendum += "// The actual bodies are filled in by A9a follow-up script after\n"
    addendum += "// inspecting the current bump()/slab() shape.\n"
    write(p, txt + addendum)

def patch_credential_store_example():
    p = BROKER / "examples" / "credential_store.rs"
    txt = read(p)
    if not txt:
        print(f"  SKIP credential_store.rs: not found")
        return
    if "catch_unwind" not in txt:
        print(f"  UNCHANGED credential_store.rs (no catch_unwind)")
        return
    if "TODO(A9)" in txt:
        print(f"  UNCHANGED credential_store.rs (A9 TODO present)")
        return
    new = "// TODO(A9): replace catch_unwind probe with try_bump/try_slab.\n" + txt
    write(p, new)

def main():
    print("---- patching error.rs ----")
    patch_error_rs()
    print("---- patching SecretMemory call sites ----")
    patch_callers_of_secret_memory()
    print("---- patching builder.rs ----")
    patch_builder_rs()
    print("---- patching credential_store.rs ----")
    patch_credential_store_example()

if __name__ == "__main__":
    main()
PYEOF

python3 /tmp/sentinel_a9_patch.py
PATCH_RC=$?

echo
echo "====== A9 PATCH DONE (rc=$PATCH_RC)"
echo
echo "====== BUILD"
cargo build -p sentinel-broker 2>&1 | tail -40
echo
echo "====== CLIPPY"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -60
echo
echo "====== TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -40
else
  cargo test -p sentinel-broker 2>&1 | tail -40
fi
echo
echo "====== DOC TESTS"
cargo test -p sentinel-broker --doc 2>&1 | tail -20
echo
echo "====== A9 SCRIPT END"
