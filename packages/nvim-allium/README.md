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

## Using Plugin Functionality

`nvim-allium` uses standard Neovim LSP/diagnostic functions with default
keymaps configured on LSP attach.

After opening an `.allium` file and confirming LSP is attached (`:LspInfo`):

- Hover: `K` or `:lua vim.lsp.buf.hover()`
- Go to definition: `gd` or `:lua vim.lsp.buf.definition()`
- Find references: `gr` or `:lua vim.lsp.buf.references()`
- Rename symbol: `<leader>rn` or `:lua vim.lsp.buf.rename()`
- Code actions: `<leader>ca` or `:lua vim.lsp.buf.code_action()`
- Format buffer: `<leader>f` or `:lua vim.lsp.buf.format({ async = true })`
- Diagnostic navigation: `[d` / `]d`
- Diagnostic location list: `<leader>q`

Built-in plugin options are configured via `require("allium").setup({...})`:

- `lsp.cmd`, `lsp.filetypes`, `lsp.root_dir`, `lsp.settings`
- `keymaps.enabled` and individual keymap overrides

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

## Tests

Run plugin tests from the monorepo root:

```bash
npm run test:nvim
```

These tests run in a headless `-u NONE` Neovim instance and use local stubs for
`nvim-lspconfig` and `nvim-treesitter` to keep test runtime fast.

For isolated integration tests with real Neovim dependencies:

```bash
npm run test:nvim:install
npm run test:nvim:integration
```
