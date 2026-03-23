# Allium Tools

> **Pre-release notice:** This software is currently in a pre-release state. Treat it as unstable, expect breaking changes, and validate it carefully before production use.

Allium Tools is the multi-editor platform for the Allium specification language. It provides rich editor integration, a standalone Language Server (LSP), and a suite of CLI tools for validation, formatting, and analysis.

## What is Allium?

Allium is a domain-specific language for specifying system behavior, rules, and data models. It focuses on clarity, traceability, and automated validation.

### Key Concepts

- **Blocks**: Top-level declarations like `rule`, `entity`, `enum`, `config`, `given`, `surface`, and `actor`.
- **Clauses**: Express requirements and effects using `when:`, `requires:`, and `ensures:`.
- **Traceability**: Built-in support for linking specifications to implementation and tests.

### Brief Syntax Example

```allium
module ordering

enum OrderStatus {
    pending | confirmed | dispatched
}

entity Order {
    status: OrderStatus
    total: Decimal
}

rule PlaceOrder {
    when: OrderSubmitted(basket)
    requires: basket.items.count > 0
    ensures: Order.created(status: pending, total: basket.total)
}

rule DispatchOrder {
    when: order: Order.status becomes confirmed
    requires: order.total > 0
    ensures: order.status = dispatched
}
```

## Editor Support Matrix

| Feature | VS Code | Neovim | Emacs | CLI |
| :--- | :---: | :---: | :---: | :---: |
| Diagnostics (Linting) | ✅ | ✅ | ✅ | ✅ |
| Hover Documentation | ✅ | ✅ | ✅ | - |
| Go to Definition | ✅ | ✅ | ✅ | - |
| Find References | ✅ | ✅ | ✅ | ✅ |
| Rename Refactoring | ✅ | ✅ | ✅ | - |
| Safe Fixes (Autofix) | ✅ | ✅ | ✅ | ✅ |
| Formatting | ✅ | ✅ | ✅ | ✅ |
| Code Lens | ✅ | - | - | - |
| Document Links | ✅ | ✅ | ✅ | - |
| Semantic Highlighting | ✅ | ✅ | ✅ | - |
| Folding | ✅ | ✅ | ✅ | - |
| Completions | ✅ | ✅ | ✅ | - |
| Diagram Preview | ✅ | - | - | ✅ |
| Rule Simulation | ✅ | - | - | - |
| Test Scaffold | ✅ | - | - | - |

## Quick Install

### VS Code

1. Download the latest `allium-vscode-<version>.vsix` from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
2. Install via Command Palette: `Extensions: Install from VSIX...`.
3. See the [VS Code Setup Guide](docs/editors/vscode.md) for details.

### Neovim

Install [nvim-allium](https://github.com/juxt/nvim-allium) with your plugin manager. It handles LSP, tree-sitter highlighting and filetype detection.

### Emacs

Install [allium-mode](https://github.com/juxt/allium-mode) with `straight.el`, Doom or manually. It handles syntax highlighting, indentation and LSP integration via `eglot` or `lsp-mode`. See the [Emacs Setup Guide](docs/editors/emacs.md) for details.

## CLI

The `allium` CLI validates and parses Allium specification files.

- `allium check` — validate specifications and report diagnostics.
- `allium parse` — parse a file and output the syntax tree.

### Install

**Homebrew**

```bash
brew tap juxt/allium && brew install allium
```

**Cargo**

```bash
cargo install allium-cli
```

**From source**

```bash
cargo build --release -p allium-cli
```

Pre-built binaries for Linux and macOS are available on the [GitHub Releases](https://github.com/juxt/allium-tools/releases) page.

## Documentation

- [Architecture Overview](docs/project/architecture.md)
- [Editor Setup Guides](docs/editors/)
- [Contributing Guide](CONTRIBUTING.md)
- [Project Plan & Roadmap](docs/project/plan.md)

## Development

See [AGENTS.md](AGENTS.md) for development rules and the [Project Roadmap](docs/project/plan.md) for build instructions and priorities.

### Git Hooks

This repository uses `pre-commit` hooks for local quality gates.

Install hooks:

```bash
npm install
```

Run the full test matrix locally:

```bash
npm run test:all
```

The `pre-push` hook runs `npm run test:all` before allowing a push.

## Licence

[MIT](LICENSE)
