#!/usr/bin/env bash
# A9b - add BrokerError::BuilderMisuse, try_bump/try_slab on ArenaBuilder,
# and rewrite credential_store.rs to use try_slab. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

echo "====== A9b PATCH START"

cat > /tmp/sentinel_a9b_patch.py <<'PYEOF'
#!/usr/bin/env python3
"""A9b: BuilderMisuse + try_bump/try_slab + credential_store rewrite."""
import re
from pathlib import Path

ROOT = Path.cwd()
BROKER = ROOT / "crates" / "sentinel-broker"
SRC = BROKER / "src"

def patch_error_rs():
    p = SRC / "error.rs"
    txt = p.read_text()
    if "BuilderMisuse" in txt:
        print("  UNCHANGED error.rs (BuilderMisuse already present)")
        return
    # Insert BuilderMisuse just before the closing brace of `pub enum BrokerError`.
    # We look for the enum and inject a new variant before the final `}`.
    m = re.search(r"(pub enum BrokerError\s*\{)([\s\S]*?)(\n\})", txt)
    if not m:
        print("  WARN error.rs: could not locate BrokerError enum body; skipping")
        return
    head, body, tail = m.group(1), m.group(2), m.group(3)
    # Append the new variant. Match the indentation of existing variants.
    # Most variants in the existing enum are #[error("...")] then `Variant { ... },`.
    new_variant = (
        '\n    /// Builder used incorrectly (e.g. .try_bump() without .capacity()).\n'
        '    #[error("builder misuse: {reason}")]\n'
        '    BuilderMisuse { reason: &\'static str },\n'
    )
    # If body already ends with newline, just append; otherwise add one.
    if not body.endswith("\n"):
        body += "\n"
    new_txt = txt[:m.start()] + head + body + new_variant + tail + txt[m.end():]
    p.write_text(new_txt)
    print("  UPDATE error.rs (added BuilderMisuse)")

def patch_builder_rs():
    p = SRC / "builder.rs"
    txt = p.read_text()

    if "fn try_bump" in txt and "fn try_slab" in txt:
        print("  UNCHANGED builder.rs (try_bump/try_slab already present)")
        return

    # Strip the A9 marker comment block (everything from "// === A9:" to EOF).
    marker_re = re.compile(r"\n*// === A9: fallible builder variants ===[\s\S]*\Z")
    txt = marker_re.sub("", txt).rstrip() + "\n"

    # We need to inject try_bump / try_slab into the `impl<'b> ArenaBuilder<'b>`
    # block, just before its closing `}`. The block ends with slab()'s closing
    # brace followed by the impl's closing brace. Locate the LAST `}` that
    # closes the impl by tracking: find `impl<'b> ArenaBuilder<'b> {` and walk
    # to its matching brace.
    impl_start_re = re.search(r"impl<'b> ArenaBuilder<'b>\s*\{", txt)
    if not impl_start_re:
        print("  ERROR builder.rs: could not find impl block")
        return
    start = impl_start_re.end()
    depth = 1
    i = start
    while i < len(txt) and depth > 0:
        c = txt[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                impl_close = i
                break
        i += 1
    else:
        print("  ERROR builder.rs: unbalanced braces in impl block")
        return

    fallible = '''
    /// Fallible counterpart of [`bump`]. Returns
    /// [`BrokerError::BuilderMisuse`] if [`capacity`] was not called,
    /// or [`BrokerError::SecretMemory`] if a secret-memory policy was
    /// requested and [`SecretStrategy::wrap`] failed (e.g. mlock
    /// refused on a host without `IPC_LOCK`).
    pub fn try_bump(self) -> Result<ArenaHandle, crate::error::BrokerError> {
        let cap = self.bump_capacity
            .ok_or(crate::error::BrokerError::BuilderMisuse {
                reason: "ArenaBuilder::try_bump requires .capacity(n) first",
            })?;
        let id = self.broker.next_arena_id();
        let inner: Box<dyn AllocStrategy> = Box::new(BumpStrategy::new(id, cap));
        let strategy: Box<dyn AllocStrategy> = match self.secret_policy {
            Some(p) if p != SecretPolicy::NONE => {
                Box::new(SecretStrategy::wrap(inner, p)?)
            }
            _ => inner,
        };
        let arena = Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc());
        self.broker.register_arena(Arc::clone(&arena));
        Ok(ArenaHandle::from_parts(id, arena))
    }

    /// Fallible counterpart of [`slab`]. Returns
    /// [`BrokerError::SecretMemory`] if a secret-memory policy was
    /// requested and [`SecretStrategy::wrap`] failed.
    pub fn try_slab(
        self,
        slot_size: usize,
        slot_align: usize,
        slot_count: u32,
    ) -> Result<ArenaHandle, crate::error::BrokerError> {
        let id = self.broker.next_arena_id();
        let inner: Box<dyn AllocStrategy> = Box::new(SlabStrategy::new(id, slot_size, slot_align, slot_count));
        let strategy: Box<dyn AllocStrategy> = match self.secret_policy {
            Some(p) if p != SecretPolicy::NONE => {
                Box::new(SecretStrategy::wrap(inner, p)?)
            }
            _ => inner,
        };
        let arena = Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc());
        self.broker.register_arena(Arc::clone(&arena));
        Ok(ArenaHandle::from_parts(id, arena))
    }
'''
    new_txt = txt[:impl_close] + fallible + txt[impl_close:]
    p.write_text(new_txt)
    print("  UPDATE builder.rs (added try_bump/try_slab)")

def patch_credential_store():
    p = BROKER / "examples" / "credential_store.rs"
    txt = p.read_text()

    if "try_slab" in txt and "catch_unwind" not in txt:
        print("  UNCHANGED credential_store.rs (already migrated)")
        return

    # Strip the A9 TODO line at the top.
    txt = re.sub(r"^// TODO\(A9\):.*\n", "", txt)

    # Replace the entire build_vault body with the new implementation.
    # Locate `fn build_vault(broker: &Broker) -> ArenaHandle {` and walk
    # to its matching closing brace.
    fn_start_m = re.search(r"fn build_vault\(broker: &Broker\) -> ArenaHandle \{", txt)
    if not fn_start_m:
        print("  ERROR credential_store.rs: could not find build_vault")
        return
    start = fn_start_m.end()
    depth = 1
    i = start
    while i < len(txt) and depth > 0:
        c = txt[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
        i += 1
    else:
        print("  ERROR credential_store.rs: unbalanced braces in build_vault")
        return

    new_fn = '''fn build_vault(broker: &Broker) -> ArenaHandle {
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
}'''

    new_txt = txt[:fn_start_m.start()] + new_fn + txt[end:]

    # Also fix the stale comment in main() about probe arena counters.
    # The probe arena no longer exists, so total_allocations is just 4
    # (alice, bob, carol, dave). The existing assertion is already 4,
    # but the surrounding comment is now confusing. Clean it up.
    new_txt = re.sub(
        r"    assert_eq!\(stats\.total_allocations, 4,[^;]+;\s*\n"
        r"    // Note: probe arena was destroyed[\s\S]*?// So total_allocations = 3.*?= 4\.\n",
        '    assert_eq!(stats.total_allocations, 4, "alice + bob + carol + dave = 4");\n',
        new_txt,
    )

    p.write_text(new_txt)
    print("  UPDATE credential_store.rs (try_slab migration)")

def main():
    print("---- adding BuilderMisuse to error.rs ----")
    patch_error_rs()
    print("---- adding try_bump/try_slab to builder.rs ----")
    patch_builder_rs()
    print("---- migrating credential_store.rs to try_slab ----")
    patch_credential_store()

if __name__ == "__main__":
    main()
PYEOF

python3 /tmp/sentinel_a9b_patch.py
PATCH_RC=$?

echo
echo "====== A9b PATCH DONE (rc=$PATCH_RC)"
echo
echo "====== BUILD"
cargo build -p sentinel-broker 2>&1 | tail -40
echo
echo "====== CLIPPY"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -50
echo
echo "====== TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -30
else
  cargo test -p sentinel-broker 2>&1 | tail -30
fi
echo
echo "====== DOC TESTS"
cargo test -p sentinel-broker --doc 2>&1 | tail -15
echo
echo "====== EXAMPLE: credential_store"
cargo run -p sentinel-broker --example credential_store 2>&1 | tail -40
echo
echo "====== A9b SCRIPT END"
