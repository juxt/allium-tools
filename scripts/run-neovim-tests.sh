#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v nvim >/dev/null 2>&1; then
  echo "Neovim is required to run nvim-allium tests." >&2
  exit 1
fi

cd "$ROOT_DIR"

nvim --headless -n -u NONE -i NONE \
  -c "lua local ok = pcall(dofile, 'packages/nvim-allium/test/run.lua'); if not ok then vim.cmd('cq 1'); else vim.cmd('qa!'); end"
