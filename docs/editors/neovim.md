# Neovim Setup Guide for Allium

This guide explains how to set up Allium language support in Neovim using `allium-lsp` and `nvim-allium`.

## Prerequisites

- **Neovim** >= 0.9.0
- **lazy.nvim** (recommended plugin manager)
- **nvim-lspconfig** (LSP client configuration)
- **nvim-treesitter** (Syntax highlighting)

## 1. Install Allium LSP Server

The LSP server must be available in your system path as `allium-lsp`.

### From Release Artifacts

1. Download the `allium-lsp-<version>.tar.gz` for your platform from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
2. Extract the archive and move the `allium-lsp` binary to a directory in your `$PATH` (e.g., `/usr/local/bin`).

### From Source

If you have the repository cloned:

```bash
cd packages/allium-lsp
npm install
npm run build
# The binary is in dist/bin.js. You can link it:
ln -s $(pwd)/dist/bin.js /usr/local/bin/allium-lsp
```

## 2. Install nvim-allium Plugin

Add `nvim-allium` to your Neovim configuration. Below is an example using `lazy.nvim`:

```lua
-- Example init.lua configuration
require("lazy").setup({
  {
    "juxt/allium-tools",
    -- Note: During pre-release, you may need to point to the specific subdirectory
    -- or install from a local checkout.
    config = function()
      require("allium").setup({
        -- Custom LSP options
        lsp = {
          cmd = { "allium-lsp", "--stdio" },
        }
      })
    end,
    dependencies = {
      "neovim/nvim-lspconfig",
      "nvim-treesitter/nvim-treesitter",
    },
  }
})
```

## 3. Install Tree-sitter Parser

`nvim-allium` handles the registration of the Allium parser with `nvim-treesitter`. After installing the plugin, you can install the parser by running:

```vim
:TSInstall allium
```

Ensure that you have enabled `highlight` in your `nvim-treesitter` configuration:

```lua
require('nvim-treesitter.configs').setup {
  highlight = {
    enable = true,
  },
}
```

## 4. Verify Setup

Run the following command in Neovim to check the health of the Allium integration:

```vim
:checkhealth allium
```

This will verify that the `allium-lsp` binary is found and that the Tree-sitter parser is correctly installed.

## Quick Isolated Demo

From the monorepo root, you can launch a repo-local Neovim demo session that
does not use your normal Neovim config/state:

```bash
npm run demo:nvim-allium
```

Tree-sitter variant:

```bash
npm run demo:nvim-allium:ts
```

The demo stores runtime data in `.nvim-demo/`.

## Feature Reference

| Feature | Description | Standard Keymap |
| :--- | :--- | :--- |
| **Hover** | Show documentation for symbol | `K` |
| **Go to Definition** | Jump to declaration | `gd` |
| **Find References** | List all usages | `gr` |
| **Rename** | Rename symbol across files | `<leader>rn` |
| **Code Actions** | Apply quick fixes / refactors | `<leader>ca` |
| **Formatting** | Format current buffer | `<leader>f` |
| **Diagnostics** | Show inline errors and warnings | `[d` / `]d` |

## Troubleshooting

- **LSP not starting**: Ensure `allium-lsp` is in your `$PATH`. You can test this by running `allium-lsp --version` in your terminal.
- **No syntax highlighting**: Ensure `nvim-treesitter` is installed and you've run `:TSInstall allium`. Check that the filetype is correctly detected as `allium` with `:set filetype?`.
- **Logs**: Check LSP logs with `:LspLog` for detailed error messages from the server.
