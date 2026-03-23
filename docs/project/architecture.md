# Allium Tools Architecture

Allium Tools follows a layered architecture designed for multi-editor support, high performance, and robustness.

## High-Level Overview

```text
+-------------------------------------------------------------+
|                      Editor Clients                         |
|  (VS Code, Neovim, Emacs, or standard LSP clients)          |
+-------------------------------------------------------------+
          |                               |
          | JSON-RPC (stdio)              | Tree-sitter Queries
          v                               v
+-----------------------+       +-----------------------------+
|      allium-lsp       |       |     tree-sitter-allium      |
|  (Language Server)    |       |  (Incremental Parser)       |
+-----------+-----------+       +-----------------------------+
            |                           | Highlighting, Folds,
            | Function Calls            | Indentation, Navigation
            v                           v
+-------------------------------------------------------------+
|                      language-tools                         |
|  (Core Engine: Analysis, Definitions, Refactors, Formatting) |
+-------------------------------------------------------------+
            ^
            | Function Calls
            |
+-----------+-----------+
|      allium-cli       |
|  (Check, Format, etc) |
+-----------------------+
```

## Core Components

### 1. language-tools (Shared Engine)

Located in `extensions/allium/src/language-tools/`, this is the heart of Allium. It is a pure TypeScript library with **zero dependencies** on editor APIs (like `vscode`).

- **Input**: Plain text (the Allium specification).
- **Output**: Plain data structures (finding lists, edit plans, markdown documentation).
- **Features**: Diagnostic logic, jump-to-definition resolution, workspace-wide rename planning, and automated refactoring logic.

### 2. allium-lsp (Language Server)

Located in `packages/allium-lsp/`, this package provides an LSP-compliant wrapper around `language-tools`.

- **Responsibility**: It manages the JSON-RPC lifecycle, document synchronization, and multi-file workspace indexing.
- **Protocol**: Communicates over `stdio` using standard Language Server Protocol.
- **Extensibility**: It exposes custom requests for features like diagram generation and rule simulation that go beyond the standard LSP.

### 3. tree-sitter-allium (Structural Grammar)

Located in its own repo ([juxt/tree-sitter-allium](https://github.com/juxt/tree-sitter-allium)), this provides a high-performance incremental parser.

- **Usage**: Editors use Tree-sitter for fast syntax highlighting, code folding, smart indentation, and structural navigation (like "jumping to the next rule").
- **Artifacts**: Produces a native `.node` library for Node.js and a `.wasm` file for web/browser use.

### 4. Editor Clients

- **VS Code**: A thin wrapper using `vscode-languageclient` to launch the LSP server and `tree-sitter-allium.wasm` for highlighting.
- **Neovim**: Integrates via `nvim-lspconfig` and `nvim-treesitter`.
- **Emacs**: Integrates via `eglot` or `lsp-mode` and built-in `treesit.el` (Emacs 29+).

### 5. CLI Tools (allium-cli)

Standalone command-line tools that consume `language-tools` directly. They are optimized for speed and machine-readable output (JSON/SARIF), making them ideal for CI/CD pipelines.

## Data Flow

1. **User Edits**: The editor sends the updated text to the LSP server.
2. **Analysis**: `allium-lsp` calls the appropriate function in `language-tools`.
3. **Diagnostics**: `language-tools` returns a list of findings, which `allium-lsp` translates into LSP `publishDiagnostics` notifications.
4. **Interactive Actions**: When a user requests a rename or a refactor, the request flows through LSP to `language-tools`, which returns a `WorkspaceEdit` that the editor then applies to the file system.
