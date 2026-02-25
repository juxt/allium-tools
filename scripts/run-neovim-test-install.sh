#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$ROOT_DIR/.nvim-test"
XDG_DATA_HOME="$TEST_ROOT/xdg/data"
LSPCONFIG_DIR="$XDG_DATA_HOME/nvim/site/pack/test/start/nvim-lspconfig"
TREESITTER_DIR="$XDG_DATA_HOME/nvim/site/pack/test/start/nvim-treesitter"
DEMO_LSPCONFIG="$ROOT_DIR/.nvim-demo/xdg/data/nvim/site/pack/demo/start/nvim-lspconfig"
DEMO_TREESITTER="$ROOT_DIR/.nvim-demo/xdg/data/nvim/site/pack/demo/start/nvim-treesitter"

mkdir -p "$XDG_DATA_HOME/nvim/site/pack/test/start"

if [[ ! -d "$LSPCONFIG_DIR" ]]; then
  if [[ -d "$DEMO_LSPCONFIG" ]]; then
    cp -R "$DEMO_LSPCONFIG" "$LSPCONFIG_DIR"
  else
    git clone --depth 1 https://github.com/neovim/nvim-lspconfig "$LSPCONFIG_DIR"
  fi
fi

if [[ ! -d "$TREESITTER_DIR" ]]; then
  if [[ -d "$DEMO_TREESITTER" ]]; then
    cp -R "$DEMO_TREESITTER" "$TREESITTER_DIR"
  else
    git clone --depth 1 https://github.com/nvim-treesitter/nvim-treesitter "$TREESITTER_DIR"
  fi
fi

echo "Installed Neovim test dependencies under $TEST_ROOT"
