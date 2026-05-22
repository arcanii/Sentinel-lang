#!/usr/bin/env bash
#
# 00-bootstrap-environment.sh
#
# Idempotent environment bootstrap for the Sentinel language project.
# Sets up the macOS Apple Silicon toolchain needed to begin compiler work.
#
# Safe to run multiple times. Reports what it does and what it skips.
#
# Usage:
#   bash 00-bootstrap-environment.sh
#
# Requirements:
#   - macOS on Apple Silicon (M1/M2/M3/M4)
#   - Homebrew installed at /opt/homebrew
#   - Network access for first-time installs

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
LLVM_VERSION="18"
RUST_CHANNEL="stable"

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

color_reset=$'\033[0m'
color_info=$'\033[1;34m'
color_ok=$'\033[1;32m'
color_skip=$'\033[1;33m'
color_err=$'\033[1;31m'

info()  { printf "%s[INFO]%s  %s\n"  "$color_info" "$color_reset" "$*"; }
ok()    { printf "%s[OK]%s    %s\n"  "$color_ok"   "$color_reset" "$*"; }
skip()  { printf "%s[SKIP]%s  %s\n"  "$color_skip" "$color_reset" "$*"; }
err()   { printf "%s[ERR]%s   %s\n"  "$color_err"  "$color_reset" "$*" >&2; }

# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------

info "Checking preconditions..."

# Verify we're on macOS
if [[ "$(uname -s)" != "Darwin" ]]; then
    err "This script requires macOS. Detected: $(uname -s)"
    exit 1
fi
ok "Running on macOS"

# Verify Apple Silicon
arch_name="$(uname -m)"
if [[ "$arch_name" != "arm64" ]]; then
    err "This script requires Apple Silicon (arm64). Detected: $arch_name"
    err "If you have an Intel Mac, the Sentinel target plan needs adjustment."
    exit 1
fi
ok "Running on Apple Silicon (arm64)"

# Verify Homebrew at the expected location
if [[ ! -x /opt/homebrew/bin/brew ]]; then
    err "Homebrew not found at /opt/homebrew/bin/brew"
    err "Install from https://brew.sh first."
    exit 1
fi
ok "Homebrew found at /opt/homebrew"

# Verify repo directory exists (do NOT create it silently; the user should
# have already cloned the repo)
if [[ ! -d "$SENTINEL_ROOT" ]]; then
    err "Repository directory not found: $SENTINEL_ROOT"
    err "Clone the repository to that location before running this script."
    exit 1
fi
ok "Repository directory exists: $SENTINEL_ROOT"

# Ensure brew is in PATH for the rest of this script
export PATH="/opt/homebrew/bin:$PATH"

# ---------------------------------------------------------------------------
# Homebrew packages
# ---------------------------------------------------------------------------

info "Checking Homebrew packages..."

brew_install_if_missing() {
    local pkg="$1"
    if brew list --formula --versions "$pkg" >/dev/null 2>&1; then
        skip "Homebrew package already installed: $pkg"
    else
        info "Installing Homebrew package: $pkg"
        brew install "$pkg"
        ok "Installed: $pkg"
    fi
}

# Pinned LLVM for inkwell stability
brew_install_if_missing "llvm@${LLVM_VERSION}"

# Build infrastructure
brew_install_if_missing "cmake"
brew_install_if_missing "ninja"
brew_install_if_missing "pkg-config"

# Development tooling
brew_install_if_missing "just"
brew_install_if_missing "ripgrep"
brew_install_if_missing "fd"
brew_install_if_missing "jq"
brew_install_if_missing "git"

# ---------------------------------------------------------------------------
# Rust toolchain
# ---------------------------------------------------------------------------

info "Checking Rust toolchain..."

if ! command -v rustup >/dev/null 2>&1; then
    info "Installing rustup (Rust toolchain manager)..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --default-toolchain "$RUST_CHANNEL" --profile default
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
    ok "Installed rustup and $RUST_CHANNEL toolchain"
else
    skip "rustup already installed"
    # Ensure cargo is in PATH for the remainder of the script
    # shellcheck disable=SC1091
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
fi

# Ensure the stable channel is active and up to date
current_default="$(rustup default 2>/dev/null | awk '{print $1}' | cut -d- -f1 || echo "")"
if [[ "$current_default" != "$RUST_CHANNEL" ]]; then
    info "Setting default Rust toolchain to $RUST_CHANNEL"
    rustup default "$RUST_CHANNEL"
    ok "Default toolchain set to $RUST_CHANNEL"
else
    skip "Default Rust toolchain already $RUST_CHANNEL"
fi

info "Updating Rust toolchain (no-op if current)..."
rustup update "$RUST_CHANNEL" >/dev/null
ok "Rust toolchain is current"

# Required components
ensure_rust_component() {
    local comp="$1"
    if rustup component list --installed 2>/dev/null | grep -q "^${comp}-"; then
        skip "Rust component already installed: $comp"
    else
        info "Installing Rust component: $comp"
        rustup component add "$comp"
        ok "Installed Rust component: $comp"
    fi
}

ensure_rust_component "rustfmt"
ensure_rust_component "clippy"
ensure_rust_component "rust-analyzer"
ensure_rust_component "rust-src"

# Required targets (Apple Silicon native; Intel kept available for cross
# compile but not the default)
ensure_rust_target() {
    local tgt="$1"
    if rustup target list --installed 2>/dev/null | grep -qx "$tgt"; then
        skip "Rust target already installed: $tgt"
    else
        info "Installing Rust target: $tgt"
        rustup target add "$tgt"
        ok "Installed Rust target: $tgt"
    fi
}

ensure_rust_target "aarch64-apple-darwin"

# ---------------------------------------------------------------------------
# Cargo tools
# ---------------------------------------------------------------------------

info "Checking cargo-installed tools..."

cargo_install_if_missing() {
    local crate="$1"
    local binary="${2:-$1}"
    if command -v "$binary" >/dev/null 2>&1; then
        skip "cargo tool already installed: $crate ($binary)"
    else
        info "Installing cargo tool: $crate"
        cargo install --locked "$crate"
        ok "Installed: $crate"
    fi
}

cargo_install_if_missing "cargo-nextest" "cargo-nextest"
cargo_install_if_missing "cargo-insta"   "cargo-insta"
cargo_install_if_missing "cargo-deny"    "cargo-deny"
cargo_install_if_missing "mdbook"        "mdbook"

# ---------------------------------------------------------------------------
# Shell environment additions (idempotent)
# ---------------------------------------------------------------------------

info "Checking shell environment for LLVM and Cargo paths..."

ZSHRC="$HOME/.zshrc"
LLVM_PREFIX="/opt/homebrew/opt/llvm@${LLVM_VERSION}"

ensure_line_in_file() {
    local line="$1"
    local file="$2"
    if [[ ! -f "$file" ]]; then
        touch "$file"
    fi
    if grep -qxF "$line" "$file"; then
        skip "Already in $file: $line"
    else
        printf '\n%s\n' "$line" >> "$file"
        ok "Added to $file: $line"
    fi
}

# Sentinel-managed block marker (so we can tell what we added)
SENTINEL_BLOCK_BEGIN="# >>> sentinel-language environment >>>"
SENTINEL_BLOCK_END="# <<< sentinel-language environment <<<"

if grep -q "$SENTINEL_BLOCK_BEGIN" "$ZSHRC" 2>/dev/null; then
    skip "Sentinel environment block already present in $ZSHRC"
else
    info "Adding Sentinel environment block to $ZSHRC"
    {
        printf '\n%s\n' "$SENTINEL_BLOCK_BEGIN"
        printf '# Managed by Sentinel project bootstrap; safe to edit but keep markers.\n'
        printf 'export PATH="%s/bin:$PATH"\n' "$LLVM_PREFIX"
        printf 'export LLVM_SYS_%s0_PREFIX="%s"\n' "$LLVM_VERSION" "$LLVM_PREFIX"
        printf '[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"\n'
        printf '%s\n' "$SENTINEL_BLOCK_END"
    } >> "$ZSHRC"
    ok "Environment block added to $ZSHRC"
fi

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

info "Verifying installation..."

# Source the new environment for this shell so the verification below works
export PATH="${LLVM_PREFIX}/bin:$PATH"
export LLVM_SYS_${LLVM_VERSION}0_PREFIX="$LLVM_PREFIX"

check_version() {
    local label="$1"
    local cmd="$2"
    if output="$(eval "$cmd" 2>&1)"; then
        ok "$label: $(echo "$output" | head -n1)"
    else
        err "$label: command failed -- $cmd"
        return 1
    fi
}

check_version "rustc"        "rustc --version"
check_version "cargo"        "cargo --version"
check_version "rustup"       "rustup --version"
check_version "clang (llvm)" "${LLVM_PREFIX}/bin/clang --version"
check_version "llvm-config"  "${LLVM_PREFIX}/bin/llvm-config --version"
check_version "cmake"        "cmake --version"
check_version "ninja"        "ninja --version"
check_version "just"         "just --version"
check_version "nextest"      "cargo nextest --version"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

cat <<EOF

${color_ok}========================================${color_reset}
${color_ok}Sentinel environment bootstrap complete.${color_reset}
${color_ok}========================================${color_reset}

Repository:  $SENTINEL_ROOT
Target:      aarch64-apple-darwin (Apple Silicon)
LLVM:        $LLVM_PREFIX
Rust:        $(rustc --version 2>/dev/null || echo "not in PATH for this shell")

If this is the first run, open a new terminal (or run 'source ~/.zshrc')
so the LLVM and Cargo paths are picked up by future shells.

Next step: run the project scaffolding script when it is provided.

EOF

