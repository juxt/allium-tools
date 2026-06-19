# allium-lsp

Language Server Protocol (LSP) server for the [Allium](https://github.com/juxt/allium) behavioural specification language.

It provides live diagnostics, go-to-definition and hover for `.allium` files in any LSP-compatible editor or agent.

## Install

```sh
npm install -g allium-lsp
```

This puts the `allium-lsp` binary on your `PATH`. Requires Node.js ≥ 20.

## Usage

The server communicates over stdio. Point your editor's LSP client at:

- **Command:** `allium-lsp --stdio`
- **File association:** `*.allium`

See the [editor setup guides](https://github.com/juxt/allium-tools/tree/main/docs/editors) for Neovim, Emacs, Helix and others, and the [Claude Code plugin](https://github.com/juxt/allium) which wires this server automatically.

## Licence

MIT — see [LICENSE](./LICENSE).
