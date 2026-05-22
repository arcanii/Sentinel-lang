#!/usr/bin/env bash
#
# 01-scaffold-workspace.sh
#
# Idempotent scaffolding for the Sentinel language Cargo workspace.
# Creates the directory structure, workspace manifest, member crate
# stubs, justfile, rust-toolchain.toml, and supporting files described
# in HANDOVER.md Section 3.2.
#
# Safe to run multiple times. Existing files are not overwritten unless
# they are unchanged-stub files we created previously.

set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
RUST_TOOLCHAIN="stable"
RUST_EDITION="2021"

# Output helpers
color_reset=$'\033[0m'
color_info=$'\033[1;34m'
color_ok=$'\033[1;32m'
color_skip=$'\033[1;33m'
color_err=$'\033[1;31m'
info()  { printf "%s[INFO]%s  %s\n"  "$color_info" "$color_reset" "$*"; }
ok()    { printf "%s[OK]%s    %s\n"  "$color_ok"   "$color_reset" "$*"; }
skip()  { printf "%s[SKIP]%s  %s\n"  "$color_skip" "$color_reset" "$*"; }
err()   { printf "%s[ERR]%s   %s\n"  "$color_err"  "$color_reset" "$*" >&2; }

if [[ ! -d "$SENTINEL_ROOT" ]]; then
    err "Repository directory not found: $SENTINEL_ROOT"
    exit 1
fi

cd "$SENTINEL_ROOT"
ok "Working in $SENTINEL_ROOT"

# ---------------------------------------------------------------------------
# Idempotent file writer: writes only if missing or differs from intended
# ---------------------------------------------------------------------------

write_if_needed() {
    local path="$1"
    local content="$2"
    local dir
    dir="$(dirname "$path")"
    mkdir -p "$dir"
    if [[ -f "$path" ]]; then
        if [[ "$(cat "$path")" == "$content" ]]; then
            skip "Unchanged: ${path#$SENTINEL_ROOT/}"
            return 0
        else
            # Existing file differs from scaffold; do NOT overwrite user edits
            skip "Already exists (user-modified, not overwriting): ${path#$SENTINEL_ROOT/}"
            return 0
        fi
    fi
    printf "%s" "$content" > "$path"
    ok "Created: ${path#$SENTINEL_ROOT/}"
}

ensure_dir() {
    local d="$1"
    if [[ -d "$d" ]]; then
        skip "Directory exists: ${d#$SENTINEL_ROOT/}"
    else
        mkdir -p "$d"
        ok "Created directory: ${d#$SENTINEL_ROOT/}"
    fi
}

# ---------------------------------------------------------------------------
# Top-level directories
# ---------------------------------------------------------------------------

info "Creating top-level directory structure..."
ensure_dir "$SENTINEL_ROOT/crates"
ensure_dir "$SENTINEL_ROOT/docs"
ensure_dir "$SENTINEL_ROOT/docs/decisions"
ensure_dir "$SENTINEL_ROOT/tests/ui"
ensure_dir "$SENTINEL_ROOT/tests/pass"
ensure_dir "$SENTINEL_ROOT/tests/snapshots"
ensure_dir "$SENTINEL_ROOT/examples"
ensure_dir "$SENTINEL_ROOT/.github/workflows"
ensure_dir "$SENTINEL_ROOT/scripts"

# ---------------------------------------------------------------------------
# rust-toolchain.toml
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/rust-toolchain.toml" "$(cat <<'EOF'
# Pinned toolchain for the Sentinel language project.
# Every contributor builds with this exact channel and components.

[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-analyzer", "rust-src"]
targets = ["aarch64-apple-darwin"]
profile = "default"
EOF
)"

# ---------------------------------------------------------------------------
# .gitignore
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/.gitignore" "$(cat <<'EOF'
# Build artifacts
/target/
**/target/
**/*.rs.bk
*.pdb

# Cargo lock for libraries is committed; this is a workspace with
# the snc binary, so Cargo.lock IS committed at the workspace root.

# IDE
.vscode/
.idea/
*.swp
*.swo
.DS_Store

# Test snapshots that are pending review
*.snap.new

# Local environment overrides
.env
.env.local

# Generated documentation builds
/book/
EOF
)"

# ---------------------------------------------------------------------------
# Workspace Cargo.toml
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/Cargo.toml" "$(cat <<'EOF'
# Sentinel language workspace manifest.
#
# Member crates are listed below. Dependency versions are pinned in
# [workspace.dependencies] and inherited by member crates via
# `package.workspace = true` to prevent version drift.

[workspace]
resolver = "2"
members = [
    "crates/sentinel-broker",
    "crates/sentinel-syntax",
    "crates/sentinel-ast",
    "crates/sentinel-resolve",
    "crates/sentinel-types",
    "crates/sentinel-hir",
    "crates/sentinel-mir",
    "crates/sentinel-codegen",
    "crates/sentinel-driver",
    "crates/sentinel-runtime",
    "crates/sentinel-lsp",
]

[workspace.package]
edition      = "2021"
rust-version = "1.80"
version      = "0.0.1"
authors      = ["Sentinel Language Project"]
license      = "Apache-2.0 OR MIT"
repository   = "https://github.com/bryan/Sentinel-language"

[workspace.dependencies]
# Lexing and parsing
logos      = "0.14"

# Query engine
salsa      = "0.18"

# LLVM bindings (matches LLVM 18 pinned in 00-bootstrap-environment.sh)
inkwell    = { version = "0.5", features = ["llvm18-0"] }

# Fast debug codegen
cranelift              = "0.111"
cranelift-module       = "0.111"
cranelift-object       = "0.111"

# Arena allocation for AST/IR nodes
bumpalo       = "3.16"
typed-arena   = "2.0"

# Collections and utilities
indexmap   = "2.5"
rustc-hash = "2.0"
smallvec   = "1.13"

# Diagnostics
miette     = { version = "7.2", features = ["fancy"] }
thiserror  = "1.0"

# Tracing/logging
tracing             = "0.1"
tracing-subscriber  = { version = "0.3", features = ["env-filter"] }

# Testing
insta      = "1.40"

# Serialization (used by tooling, manifest parsing)
serde      = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml       = "0.8"

[profile.release]
lto              = "thin"
codegen-units    = 1
opt-level        = 3
debug            = "line-tables-only"

[profile.dev]
opt-level        = 0
debug            = "full"

[profile.test]
opt-level        = 1
EOF
)"

# ---------------------------------------------------------------------------
# justfile
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/justfile" "$(cat <<'EOF'
# Sentinel language project task runner.
# Run `just` with no arguments to see available recipes.

default:
    @just --list

# Build all workspace members
build:
    cargo build --workspace

# Build in release mode
build-release:
    cargo build --workspace --release

# Run the full test suite via nextest (faster than cargo test)
test:
    cargo nextest run --workspace

# Run tests including snapshot review
test-all:
    cargo nextest run --workspace
    cargo test --workspace --doc

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Lint with clippy
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the Sentinel compiler driver
snc *args:
    cargo run --bin snc -- {{args}}

# Bless updated snapshots after a test run
bless:
    INSTA_UPDATE=always cargo nextest run --workspace

# Full pre-commit check: format, lint, test
check-all: fmt-check lint test

# Clean build artifacts
clean:
    cargo clean

# Display project status
status:
    @echo "Sentinel language project"
    @echo "Workspace root: $(pwd)"
    @echo "Rust:           $(rustc --version)"
    @echo "Cargo:          $(cargo --version)"
    @echo ""
    @echo "Workspace members:"
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | sort | sed 's/^/  /'
EOF
)"

# ---------------------------------------------------------------------------
# Member crate stubs
# ---------------------------------------------------------------------------

# Each crate gets:
#   crates/<name>/Cargo.toml
#   crates/<name>/src/lib.rs  (with a placeholder)

create_crate_stub() {
    local name="$1"
    local description="$2"
    local crate_dir="$SENTINEL_ROOT/crates/$name"
    local kind="${3:-lib}"   # lib or bin
    ensure_dir "$crate_dir/src"

    local cargo_toml
    cargo_toml="$(cat <<EOF
[package]
name        = "$name"
description = "$description"

edition.workspace      = true
rust-version.workspace = true
version.workspace      = true
authors.workspace      = true
license.workspace      = true
repository.workspace   = true

[dependencies]
tracing    = { workspace = true }
thiserror  = { workspace = true }

[lints.rust]
unsafe_code = "deny"
EOF
)"

    # The driver crate is a binary; runtime is allowed unsafe; broker is allowed unsafe
    case "$name" in
        sentinel-driver)
            cargo_toml="${cargo_toml/\[dependencies\]/[[bin]]
name = \"snc\"
path = \"src/main.rs\"

[dependencies]}"
            ;;
        sentinel-runtime|sentinel-broker|sentinel-codegen)
            cargo_toml="${cargo_toml/unsafe_code = \"deny\"/unsafe_code = \"allow\"}"
            ;;
    esac

    write_if_needed "$crate_dir/Cargo.toml" "$cargo_toml"

    if [[ "$name" == "sentinel-driver" ]]; then
        write_if_needed "$crate_dir/src/main.rs" "$(cat <<EOF
//! snc: the Sentinel compiler driver.
//!
//! Wires the query-based pipeline together and exposes the
//! command-line interface. See HANDOVER.md Section 6.1.

fn main() {
    println!("snc: Sentinel compiler (scaffold stub)");
    println!("crate: $name");
    println!("description: $description");
    std::process::exit(0);
}
EOF
)"
    else
        write_if_needed "$crate_dir/src/lib.rs" "$(cat <<EOF
//! $name
//!
//! $description
//!
//! Scaffold stub. Real implementation begins in the phase described
//! in HANDOVER.md Section 6.2.

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "$name"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "$name");
    }
}
EOF
)"
    fi
}

info "Creating member crate stubs..."

create_crate_stub "sentinel-broker"  "Runtime memory broker: arenas, generational handles, budgets, recording"
create_crate_stub "sentinel-syntax"  "Lexer, parser, and concrete syntax tree for Sentinel source"
create_crate_stub "sentinel-ast"     "Abstract syntax tree types after lowering from CST"
create_crate_stub "sentinel-resolve" "Name resolution, module graph, and import handling"
create_crate_stub "sentinel-types"   "Type, region, nullability, secrecy, and effect checking"
create_crate_stub "sentinel-hir"     "Typed high-level IR with all qualifiers resolved"
create_crate_stub "sentinel-mir"     "SSA-form mid-level IR with optimizations"
create_crate_stub "sentinel-codegen" "LLVM IR lowering via inkwell"
create_crate_stub "sentinel-driver"  "snc compiler driver and command-line interface" "bin"
create_crate_stub "sentinel-runtime" "Runtime library linked into emitted Sentinel programs"
create_crate_stub "sentinel-lsp"     "Language server using the same query engine as the compiler"

# ---------------------------------------------------------------------------
# CI workflow stub
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/.github/workflows/ci.yml" "$(cat <<'EOF'
# Sentinel language project CI.
# Runs the full check-all suite on macOS Apple Silicon, which is the
# primary development target. Other targets are added later.

name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  check-all:
    name: check-all on macOS aarch64
    runs-on: macos-14
    steps:
      - uses: actions/checkout@v4

      - name: Install LLVM 18
        run: |
          brew install llvm@18
          echo "/opt/homebrew/opt/llvm@18/bin" >> "$GITHUB_PATH"
          echo "LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18" >> "$GITHUB_ENV"

      - name: Install just and nextest
        run: |
          brew install just
          cargo install --locked cargo-nextest

      - name: Format check
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Test
        run: cargo nextest run --workspace
EOF
)"

# ---------------------------------------------------------------------------
# First Architecture Decision Record
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/docs/decisions/0001-staged-validation.md" "$(cat <<'EOF'
# ADR 0001: Staged validation before full bootstrap

## Status
Accepted, 2026-05.

## Context
Sentinel is an ambitious language with several novel pillars (runtime
memory broker, region-based safety with second-class refs, algebraic
effects, secret qualifier, signature infrastructure). Building all of
it in one push risks years of work before the foundational ideas are
validated.

## Decision
Follow the four-phase plan in HANDOVER.md:

  - Phase A: prototype the broker as a standalone Rust crate (3-6 mo)
  - Phase B: prototype the effects system as a research compiler (6-9 mo)
  - Phase C: build the production bootstrap compiler (12-18 mo)
  - Phase D: self-host (9-12 mo)

Each phase has a defined go/no-go criterion. Phase C does not begin
until Phase A and Phase B have validated their core ideas.

## Consequences
Slower path to a complete language; faster path to validated ideas.
Phase A and Phase B produce standalone value (a usable broker crate,
a publishable research artifact) even if Sentinel never proceeds.
EOF
)"

# ---------------------------------------------------------------------------
# README stub
# ---------------------------------------------------------------------------

write_if_needed "$SENTINEL_ROOT/README.md" "$(cat <<'EOF'
# Sentinel

A security-first systems programming language for the threats of the 2030s.

Sentinel is in very early development. The design documents in `docs/`
describe the language. The implementation is being bootstrapped in Rust
following the staged plan in `docs/HANDOVER.md`.

## Building

Requirements:
- macOS on Apple Silicon (the primary development target)
- LLVM 18 (`brew install llvm@18`)
- Rust stable toolchain
- `just` task runner

To bootstrap the environment from scratch:

    bash scripts/00-bootstrap-environment.sh

To build and test:

    just build
    just test

## Layout

- `crates/`  — compiler workspace members
- `docs/`    — design documents
- `tests/`   — UI and execution tests
- `scripts/` — idempotent setup and build scripts

## License

Dual-licensed under Apache 2.0 OR MIT, at your option.
EOF
)"

# ---------------------------------------------------------------------------
# Verify the workspace builds
# ---------------------------------------------------------------------------

info "Running cargo check to verify the workspace builds..."
if cargo check --workspace --quiet 2>&1 | tail -20; then
    ok "Workspace builds cleanly"
else
    err "Workspace failed to build; see output above"
    exit 1
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo ""
echo "======"
echo "SCAFFOLD COMPLETE"
echo "======"
echo ""
echo "Repository: $SENTINEL_ROOT"
echo "Members:"
ls -1 "$SENTINEL_ROOT/crates" | sed 's/^/  /'
echo ""
echo "Next: cd $SENTINEL_ROOT && just status"
echo ""

