#!/usr/bin/env bash
#
# Zapcode installer — "I have a cunning plan"
#
# Detects your platform, installs prerequisites, builds the native
# bindings, and makes Zapcode available in your project.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash
#
#   Or with options:
#   curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash -s -- --lang ts
#   curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash -s -- --lang python
#   curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash -s -- --lang rust
#   curl -fsSL https://raw.githubusercontent.com/TheUncharted/zapcode/master/install.sh | bash -s -- --lang wasm

set -euo pipefail

# ── Colors ──────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${BLUE}>${NC} $*"; }
ok()    { echo -e "${GREEN}✓${NC} $*"; }
warn()  { echo -e "${YELLOW}!${NC} $*"; }
fail()  { echo -e "${RED}✗${NC} $*"; exit 1; }

# ── Parse arguments ─────────────────────────────────────────────────
LANG=""
INSTALL_DIR="${ZAPCODE_INSTALL_DIR:-$HOME/.zapcode}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --lang|-l)     LANG="$2"; shift 2 ;;
        --dir|-d)      INSTALL_DIR="$2"; shift 2 ;;
        --help|-h)
            echo "Zapcode installer"
            echo ""
            echo "Usage: install.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --lang, -l <ts|python|rust|wasm>  Target language (default: auto-detect)"
            echo "  --dir, -d <path>                   Install directory (default: ~/.zapcode)"
            echo "  --help, -h                         Show this help"
            exit 0
            ;;
        *) fail "Unknown option: $1" ;;
    esac
done

# ── Detect platform ─────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux*)  PLATFORM="linux" ;;
    Darwin*) PLATFORM="macos" ;;
    MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
    *) fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH="x64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *) fail "Unsupported architecture: $ARCH" ;;
esac

echo ""
echo -e "${BOLD}Zapcode installer${NC} — \"I have a cunning plan\""
echo -e "Platform: ${PLATFORM}-${ARCH}"
echo ""

# ── Auto-detect language from project files ─────────────────────────
if [[ -z "$LANG" ]]; then
    if [[ -f "package.json" ]]; then
        LANG="ts"
    elif [[ -f "pyproject.toml" ]] || [[ -f "setup.py" ]] || [[ -f "requirements.txt" ]]; then
        LANG="python"
    elif [[ -f "Cargo.toml" ]]; then
        LANG="rust"
    else
        info "Could not auto-detect project type."
        info "Run with --lang <ts|python|rust|wasm>"
        echo ""
        echo "  For TypeScript/JavaScript: install.sh --lang ts"
        echo "  For Python:                install.sh --lang python"
        echo "  For Rust:                  install.sh --lang rust"
        echo "  For WebAssembly:           install.sh --lang wasm"
        exit 1
    fi
    ok "Detected project type: ${LANG}"
fi

# ── Check prerequisites ─────────────────────────────────────────────
check_cmd() {
    command -v "$1" &>/dev/null
}

ensure_rust() {
    if check_cmd rustc; then
        ok "Rust toolchain found ($(rustc --version | cut -d' ' -f2))"
    else
        info "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet
        source "$HOME/.cargo/env"
        ok "Rust installed ($(rustc --version | cut -d' ' -f2))"
    fi
}

ensure_git() {
    if check_cmd git; then
        ok "git found"
    else
        fail "git is required but not installed. Install it and try again."
    fi
}

# ── Clone or update Zapcode ────────────────────────────────────────
ensure_git

if [[ -d "$INSTALL_DIR" ]]; then
    info "Updating Zapcode in $INSTALL_DIR..."
    (cd "$INSTALL_DIR" && git pull --quiet)
    ok "Updated"
else
    info "Cloning Zapcode to $INSTALL_DIR..."
    git clone --quiet https://github.com/TheUncharted/zapcode.git "$INSTALL_DIR"
    ok "Cloned"
fi

# ── Install based on language ───────────────────────────────────────
case "$LANG" in
    ts|js|typescript|javascript|node)
        info "Building JavaScript/TypeScript bindings..."
        ensure_rust

        # Check for Node.js
        if check_cmd node; then
            ok "Node.js found ($(node --version))"
        else
            fail "Node.js is required. Install it from https://nodejs.org"
        fi

        # Detect package manager
        if check_cmd bun; then
            PM="bun"
        elif check_cmd pnpm; then
            PM="pnpm"
        elif check_cmd yarn; then
            PM="yarn"
        elif check_cmd npm; then
            PM="npm"
        else
            fail "No package manager found. Install npm, yarn, pnpm, or bun."
        fi
        ok "Using package manager: ${PM}"

        # Build native addon
        (cd "$INSTALL_DIR/crates/zapcode-js" && cargo build --release -p zapcode-js 2>&1 | tail -1)
        ok "Native addon built"

        # Link into current project
        if [[ -f "package.json" ]]; then
            case "$PM" in
                npm)  (cd "$INSTALL_DIR/crates/zapcode-js" && npm link 2>/dev/null) && npm link @zapcode/core 2>/dev/null ;;
                yarn) yarn link "$INSTALL_DIR/crates/zapcode-js" 2>/dev/null ;;
                pnpm) pnpm link "$INSTALL_DIR/crates/zapcode-js" 2>/dev/null ;;
                bun)  bun link "$INSTALL_DIR/crates/zapcode-js" 2>/dev/null ;;
            esac
            ok "Linked @zapcode/core into your project"
        else
            warn "No package.json found in current directory."
            info "To use in your project, run:"
            echo ""
            echo "  cd $INSTALL_DIR/crates/zapcode-js && ${PM} link"
            echo "  cd /your/project && ${PM} link @zapcode/core"
        fi

        echo ""
        ok "Ready! Import in your code:"
        echo ""
        echo "  import { Zapcode } from '@zapcode/core';"
        echo ""
        echo "  const b = new Zapcode('1 + 2 * 3');"
        echo "  const result = b.run();"
        echo "  console.log(result.output);  // 7"
        ;;

    python|py)
        info "Building Python bindings..."
        ensure_rust

        if check_cmd python3; then
            PYTHON="python3"
        elif check_cmd python; then
            PYTHON="python"
        else
            fail "Python is required. Install it from https://python.org"
        fi
        ok "Python found ($(${PYTHON} --version))"

        # Detect Python package manager
        if check_cmd uv; then
            PY_PM="uv"
            ok "Using package manager: uv (Astral)"
        elif check_cmd pip; then
            PY_PM="pip"
        elif check_cmd pip3; then
            PY_PM="pip3"
        else
            fail "No Python package manager found. Install uv (https://docs.astral.sh/uv/) or pip."
        fi

        # Check for maturin
        if ! check_cmd maturin; then
            info "Installing maturin..."
            case "$PY_PM" in
                uv)   uv tool install maturin --quiet ;;
                *)    ${PY_PM} install maturin --quiet ;;
            esac
            ok "maturin installed"
        else
            ok "maturin found"
        fi

        # Build and install
        if [[ "$PY_PM" == "uv" ]]; then
            (cd "$INSTALL_DIR/crates/zapcode-py" && maturin develop --release --uv 2>&1 | tail -1)
        else
            (cd "$INSTALL_DIR/crates/zapcode-py" && maturin develop --release 2>&1 | tail -1)
        fi
        ok "Zapcode installed into current Python environment"

        echo ""
        ok "Ready! Import in your code:"
        echo ""
        echo "  from zapcode import Zapcode"
        echo ""
        echo "  b = Zapcode('1 + 2 * 3')"
        echo "  result = b.run()"
        echo "  print(result['output'])  # 7"
        ;;

    rust|rs)
        info "Setting up Rust dependency..."
        ensure_rust

        if [[ -f "Cargo.toml" ]]; then
            # Check if already added
            if grep -q "zapcode-core" Cargo.toml 2>/dev/null; then
                ok "zapcode-core already in Cargo.toml"
            else
                info "Add to your Cargo.toml [dependencies]:"
                echo ""
                echo "  zapcode-core = { git = \"https://github.com/TheUncharted/zapcode.git\" }"
                echo ""
                echo "  # Or use a local path:"
                echo "  zapcode-core = { path = \"$INSTALL_DIR/crates/zapcode-core\" }"
            fi
        else
            warn "No Cargo.toml found. Create a Rust project first."
        fi

        echo ""
        ok "Ready! Use in your code:"
        echo ""
        echo "  use zapcode_core::{ZapcodeRun, Value, ResourceLimits};"
        echo ""
        echo "  let runner = ZapcodeRun::new("
        echo "      \"1 + 2 * 3\".to_string(),"
        echo "      vec![], vec![], ResourceLimits::default()"
        echo "  )?;"
        echo "  let result = runner.run_simple()?;"
        echo "  assert_eq!(result, Value::Int(7));"
        ;;

    wasm|webassembly)
        info "Building WebAssembly bindings..."
        ensure_rust

        if ! check_cmd wasm-pack; then
            info "Installing wasm-pack..."
            curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
            ok "wasm-pack installed"
        else
            ok "wasm-pack found"
        fi

        (cd "$INSTALL_DIR/crates/zapcode-wasm" && wasm-pack build --target web 2>&1 | tail -1)
        ok "WASM package built at $INSTALL_DIR/crates/zapcode-wasm/pkg/"

        echo ""
        info "Copy the pkg/ directory into your project:"
        echo ""
        echo "  cp -r $INSTALL_DIR/crates/zapcode-wasm/pkg/ ./zapcode-wasm"
        echo ""
        ok "Ready! Import in your code:"
        echo ""
        echo "  import init, { Zapcode } from './zapcode-wasm';"
        echo ""
        echo "  await init();"
        echo "  const b = new Zapcode('1 + 2 * 3');"
        echo "  const result = b.run();"
        echo "  console.log(result.output);  // 7"
        ;;

    *)
        fail "Unknown language: $LANG. Use: ts, python, rust, or wasm"
        ;;
esac

echo ""
echo -e "${GREEN}${BOLD}Done!${NC} Zapcode is installed at ${INSTALL_DIR}"
echo -e "Docs: ${BLUE}https://github.com/TheUncharted/zapcode${NC}"
