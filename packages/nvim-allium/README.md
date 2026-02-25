# nvim-allium

Neovim integration for the Allium language, providing LSP and Tree-sitter support.

## Installation

### Using lazy.nvim

```lua
{
  "juxt/allium-tools",
  dir = "~/path/to/allium-tools/packages/nvim-allium", -- Local development path
  dependencies = {
    "neovim/nvim-lspconfig",
    "nvim-treesitter/nvim-treesitter",
  },
  opts = {
    -- Optional: Override defaults
    lsp = {
      cmd = { "allium-lsp", "--stdio" },
    },
  },
  config = function(_, opts)
    require("allium").setup(opts)
  end,
}
```

## Features

- LSP support via `allium-lsp`.
- Tree-sitter syntax highlighting, indents, and folds.
- Diagnostic reporting and quick fixes.

## Quick Demo (Isolated Neovim Session)

You can try `nvim-allium` in an isolated Neovim environment that does not touch
your normal Neovim config or state.

Classic mode demo:

```bash
npm run demo:nvim-allium
```

Tree-sitter mode demo:

```bash
npm run demo:nvim-allium:ts
```

The demo script stores everything under `.nvim-demo/` (repo-local) and launches
Neovim with repo-local config, data, state, and cache directories.
