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

1. Install `allium-lsp` via your favorite plugin manager or from release artifacts.
2. Configure `nvim-lspconfig` to point to the `allium-lsp` binary.
3. See the [Neovim Setup Guide](docs/editors/neovim.md) for details.

### Emacs

1. Install `allium-mode` and configure it to use `allium-lsp` with `eglot` or `lsp-mode`.
2. See the [Emacs Setup Guide](docs/editors/emacs.md) for details.

## CLI Tools

The Allium CLI suite provides standalone tools for CI/CD and automation.

- `allium-check`: Validate specifications and apply automatic fixes.
- `allium-format`: Format Allium files according to standard style.
- `allium-diagram`: Generate D2 or Mermaid diagrams from specs.
- `allium-trace`: Check traceability between specs and tests.
- `allium-drift`: Detect coverage drift between specs and implementation.

Install the CLI tools globally:

```bash
npm install -g @allium/cli
```

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

The `pre-push` hook runs the full test matrix (`npm run test`, `npm run test:emacs`, and `npm run test:emacs:integration`) before allowing a push.

## Licence

[MIT](LICENSE)
