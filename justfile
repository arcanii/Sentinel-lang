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

# Run the full test suite including doctests (nextest skips doctests)
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

# Full pre-commit check: format, lint, the four-check test suite (incl. doctests)
check-all: fmt-check lint test-all

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