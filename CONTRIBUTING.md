# Contributing New Editor Integrations

Thank you for your interest in expanding Allium support to more editors! This guide explains how to build and contribute a new editor integration.

## 1. What Allium Provides

Allium's core intelligence is exposed via two primary mechanisms:

### Language Server (`allium-lsp`)

The LSP server implements the following capabilities:

- **Diagnostics**: Real-time error and warning reporting.
- **Hover**: Documentation and type information.
- **Go to Definition**: Cross-file symbol navigation.
- **Find References**: Project-wide usage discovery.
- **Rename**: Safe, workspace-aware renaming.
- **Formatting**: Document-wide style enforcement.
- **Code Actions**: Quick fixes and automated refactorings.
- **Document Symbols**: Hierarchical file outline.
- **Workspace Symbols**: Global symbol search.
- **Semantic Tokens**: Rich, syntax-aware highlighting.
- **Folding Ranges**: Logical block collapsing.
- **Completions**: Keyword and configuration key suggestions.

### Tree-sitter Grammar ([tree-sitter-allium](https://github.com/juxt/tree-sitter-allium))

For fast, local syntax highlighting and structural navigation, Allium provides:

- **parser.c**: For native integrations (Emacs, Neovim).
- **tree-sitter-allium.wasm**: For web-based editors (VS Code).
- **Query Files**: Standard `highlights.scm`, `indents.scm`, and `folds.scm`.

## 2. Integration Strategy

Your integration should typically be a thin wrapper that:

1.  **Registers the `allium` filetype** (associated with `*.allium`).
2.  **Configures an LSP client** to launch `allium-lsp --stdio`.
3.  **Wires up Tree-sitter** for highlighting and indentation (if the editor supports it).

## 3. Repository Conventions

- **New Repository**: Create a new repo (e.g., `allium-<editor>` or `<editor>-allium`).
- **README**: Include a `README.md` explaining installation and setup.
- **Documentation**: Add a setup guide to `docs/editors/<editor>.md` in this repo, linking to the new repo.

## 4. Testing Your Integration

Before submitting a PR, verify the following features:

- Does the LSP server start automatically when an `.allium` file is opened?
- Are errors and warnings displayed correctly?
- Do "Go to Definition" and "Find References" work across multiple files?
- Is syntax highlighting accurate (test with `docs/project/specs/`)?
- Does formatting (`Ctrl+Shift+I` or equivalent) work?

## 5. Submission Checklist

- [ ] New repo created for the editor plugin.
- [ ] LSP client correctly configured for `allium-lsp`.
- [ ] Tree-sitter queries integrated (if applicable).
- [ ] `README.md` added to the new package.
- [ ] User guide added to `docs/editors/`.
- [ ] Integration tested with real Allium specifications.
- [ ] `AGENTS.md` and `README.md` at the root updated to include the new editor.
- [ ] All CI checks passing.
