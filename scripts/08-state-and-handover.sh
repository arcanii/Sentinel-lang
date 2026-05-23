#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

echo "======"
echo "REPO LAYOUT"
echo "======"
ls -la
echo
echo "--- docs/ (if present) ---"
[ -d docs ] && ls -la docs || echo "(no docs/ directory)"

echo
echo "======"
echo "EXISTING HANDOVER.md (if any)"
echo "======"
if   [ -f HANDOVER.md ];      then echo "[found at repo root]"; cat HANDOVER.md
elif [ -f docs/HANDOVER.md ]; then echo "[found at docs/HANDOVER.md]"; cat docs/HANDOVER.md
else echo "(no HANDOVER.md found anywhere obvious)"
fi

echo
echo "======"
echo "EXISTING STATE.md (if any)"
echo "======"
if   [ -f STATE.md ];      then echo "[found at repo root]"; cat STATE.md
elif [ -f docs/STATE.md ]; then echo "[found at docs/STATE.md]"; cat docs/STATE.md
else echo "(no STATE.md found — will create)"
fi

echo
echo "======"
echo "BROKER CRATE — public surface (re-exports)"
echo "======"
sed -n '1,80p' crates/sentinel-broker/src/lib.rs

echo
echo "======"
echo "GIT LOG (last 10)"
echo "======"
git --no-pager log --oneline -n 10

echo
echo "======"
echo "DONE — paste these sections back so I can write STATE.md + revise HANDOVER.md precisely"
echo "======"
