# Neovim Setup Guide for Allium

This guide covers two approaches:

1. **Native setup (Neovim 0.11+)** — uses built-in LSP and filetype APIs, no plugins required beyond `nvim-treesitter`.
2. **Plugin setup (Neovim 0.9+)** — uses the `nvim-allium` plugin with `nvim-lspconfig`.

Both approaches require the `allium-lsp` server to be installed.

## 1. Install Allium LSP Server

The LSP server must be available in your system path as `allium-lsp`.

### From Release Artifacts

1. Download the `allium-lsp-<version>.tar.gz` from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
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

## 2a. Native Setup (Neovim 0.11+)

Neovim 0.11 introduced native LSP configuration via `vim.lsp.config()` / `vim.lsp.enable()` and the `lsp/` config directory. This approach requires no plugin for LSP or filetype detection.

### Filetype Detection

Create `ftdetect/allium.lua` in your Neovim config directory:

```lua
vim.filetype.add({
  extension = {
    allium = "allium",
  },
})
```

### LSP Configuration

Create `lsp/allium_lsp.lua` in your Neovim config directory:

```lua
return {
  cmd = { "allium-lsp", "--stdio" },
  filetypes = { "allium" },
  root_markers = { "allium.config.json", ".git" },
}
```

Then enable it in your `init.lua`:

```lua
vim.lsp.enable("allium_lsp")
```

### Tree-sitter Parser

Register the Allium tree-sitter parser in your `nvim-treesitter` configuration:

```lua
local parser_config = require("nvim-treesitter.parsers").get_parser_configs()
parser_config.allium = {
  install_info = {
    url = "https://github.com/juxt/allium-tools",
    files = { "src/parser.c" },
    location = "packages/tree-sitter-allium",
    branch = "main",
  },
  filetype = "allium",
}
```

Then install the parser:

```vim
:TSInstall allium
```

### Verify Setup

1. Open an `.allium` file.
2. Confirm filetype: `:set filetype?` should show `filetype=allium`.
3. Confirm LSP attached: `:checkhealth lsp` or `:lua print(vim.inspect(vim.lsp.get_clients({ bufnr = 0 })))`.

## 2b. Plugin Setup (Neovim 0.9+)

The `nvim-allium` plugin handles filetype detection, LSP client wiring, tree-sitter parser registration, and default keymaps.

> **Note**: Because `nvim-allium` lives inside the `juxt/allium-tools` monorepo, lazy.nvim will clone the full repository. The plugin code is in `packages/nvim-allium`, so you need to add that subdirectory to the runtimepath.

Add `nvim-allium` to your configuration using `lazy.nvim`:

```lua
{
  "juxt/allium-tools",
  ft = { "allium" },
  config = function(plugin)
    vim.opt.rtp:prepend(plugin.dir .. "/packages/nvim-allium")
    require("allium").setup({
      -- Override defaults (all optional):
      -- lsp = { cmd = { "allium-lsp", "--stdio" } },
      -- keymaps = { enabled = false },
    })
  end,
  init = function(plugin)
    require("lazy.core.loader").ftdetect(plugin.dir .. "/packages/nvim-allium")
  end,
  dependencies = {
    "neovim/nvim-lspconfig",
    "nvim-treesitter/nvim-treesitter",
  },
}
```

Then install the tree-sitter parser:

```vim
:TSInstall allium
```

### Verify Setup

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

## Plugin Tests

Run repo-local Neovim plugin tests in headless mode:

```bash
npm run test:nvim
```

The test suite uses `nvim -u NONE` and stubs external dependencies for fast, deterministic checks.

Install repo-local integration test dependencies once:

```bash
npm run test:nvim:install
```

Run integration tests with real `nvim-lspconfig`, real `nvim-treesitter`, and real `allium-lsp`:

```bash
npm run test:nvim:integration
```

## Feature Reference

All features below work with both the native and plugin setup approaches — they are standard Neovim LSP capabilities:

| Feature | Description | Standard Keymap |
| :--- | :--- | :--- |
| **Hover** | Show documentation for symbol | `K` |
| **Go to Definition** | Jump to declaration | `gd` |
| **Find References** | List all usages | `gr` |
| **Rename** | Rename symbol across files | `<leader>rn` |
| **Code Actions** | Apply quick fixes / refactors | `<leader>ca` |
| **Formatting** | Format current buffer | `<leader>f` |
| **Diagnostics** | Show inline errors and warnings | `[d` / `]d` |

> **Note**: Neovim 0.11+ maps `grn` (rename), `gra` (code action), `grr` (references), and `i_CTRL-S` (signature help) by default when an LSP client attaches. The keymaps above are the conventions used by the `nvim-allium` plugin; your own mappings will take precedence.

## Troubleshooting

- **LSP not starting**: Ensure `allium-lsp` is in your `$PATH`. You can test this by running `allium-lsp --version` in your terminal.
- **No syntax highlighting**: Ensure `nvim-treesitter` is installed and you've run `:TSInstall allium`. Check that the filetype is correctly detected as `allium` with `:set filetype?`.
- **Logs**: Check LSP logs with `:LspLog` for detailed error messages from the server.
- **Plugin not loading (lazy.nvim)**: If `require("allium")` fails, check that the runtimepath includes the `packages/nvim-allium` subdirectory. Run `:echo &rtp` to verify.
