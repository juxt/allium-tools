#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-classic}"

if ! command -v nvim >/dev/null 2>&1; then
  echo "Neovim is required to run the nvim-allium demo." >&2
  exit 1
fi

if [[ "$MODE" != "classic" && "$MODE" != "ts" ]]; then
  echo "Usage: $0 [classic|ts]" >&2
  exit 1
fi

cd "$ROOT_DIR"

npm run --workspace packages/allium-lsp build >/dev/null

DEMO_ROOT="$ROOT_DIR/.nvim-demo"
XDG_CONFIG_HOME="$DEMO_ROOT/xdg/config"
XDG_DATA_HOME="$DEMO_ROOT/xdg/data"
XDG_STATE_HOME="$DEMO_ROOT/xdg/state"
XDG_CACHE_HOME="$DEMO_ROOT/xdg/cache"
DEMO_LUA_DIR="$XDG_CONFIG_HOME/nvim"
DEMO_INIT="$DEMO_LUA_DIR/init.lua"
LSPCONFIG_DIR="$XDG_DATA_HOME/nvim/site/pack/demo/start/nvim-lspconfig"
TREESITTER_DIR="$XDG_DATA_HOME/nvim/site/pack/demo/start/nvim-treesitter"

mkdir -p "$DEMO_LUA_DIR" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

if [[ ! -d "$LSPCONFIG_DIR" ]]; then
  git clone --depth 1 https://github.com/neovim/nvim-lspconfig "$LSPCONFIG_DIR" >/dev/null 2>&1
fi

if [[ "$MODE" == "ts" && ! -d "$TREESITTER_DIR" ]]; then
  git clone --depth 1 https://github.com/nvim-treesitter/nvim-treesitter "$TREESITTER_DIR" >/dev/null 2>&1
fi

cat >"$DEMO_INIT" <<'EOF'
local root = vim.env.ALLIUM_DEMO_ROOT
local mode = vim.env.ALLIUM_DEMO_MODE
local lspconfig_dir = vim.env.ALLIUM_DEMO_LSPCONFIG
local treesitter_dir = vim.env.ALLIUM_DEMO_TREESITTER

vim.cmd("filetype plugin indent on")
vim.cmd("syntax enable")
vim.opt.termguicolors = true
vim.opt.packpath:prepend(vim.fn.stdpath("data") .. "/site")

vim.opt.runtimepath:prepend(root .. "/packages/nvim-allium")
vim.opt.runtimepath:prepend(lspconfig_dir)
if treesitter_dir ~= "" and vim.loop.fs_stat(treesitter_dir) then
  vim.opt.runtimepath:prepend(treesitter_dir)
end
pcall(vim.cmd, "packadd nvim-lspconfig")
if mode == "ts" then
  pcall(vim.cmd, "packadd nvim-treesitter")
end

vim.filetype.add({
  extension = {
    allium = "allium",
  },
})
vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  pattern = "*.allium",
  callback = function(args)
    vim.bo[args.buf].filetype = "allium"
  end,
})

require("allium").setup({
  lsp = {
    cmd = { "node", root .. "/packages/allium-lsp/dist/bin.js", "--stdio" },
  },
})

if mode == "ts" then
  pcall(function()
    require("nvim-treesitter.configs").setup({
      highlight = { enable = true },
      indent = { enable = true },
      ensure_installed = {},
      auto_install = false,
    })
    vim.cmd("TSInstallSync allium")
  end)
end

local demo_file = root .. "/docs/project/specs/allium-emacs-mode-behaviour.allium"
vim.cmd("edit " .. vim.fn.fnameescape(demo_file))
vim.bo.filetype = "allium"
EOF

export XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export ALLIUM_DEMO_ROOT="$ROOT_DIR"
export ALLIUM_DEMO_MODE="$MODE"
export ALLIUM_DEMO_LSPCONFIG="$LSPCONFIG_DIR"
export ALLIUM_DEMO_TREESITTER="$TREESITTER_DIR"

nvim -u "$DEMO_INIT"
