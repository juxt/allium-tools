# Emacs Setup Guide for Allium

This guide explains how to set up Allium language support in Emacs using `allium-lsp` and `allium-mode`.

## Prerequisites

- **Emacs** >= 28.1 (Emacs 29+ recommended for built-in tree-sitter support).
- **eglot** (built-in since Emacs 29) or **lsp-mode**.

## 1. Install Allium LSP Server

The LSP server must be available in your system path as `allium-lsp`.

### From Release Artifacts

1. Download the `allium-lsp-<version>.tar.gz` for your platform from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
2. Extract the archive and move the `allium-lsp` binary to a directory in your `$PATH`.

### From Source

```bash
cd packages/allium-lsp
npm install
npm run build
# Link the binary to your path
ln -s $(pwd)/dist/bin.js /usr/local/bin/allium-lsp
```

## 2. Install allium-mode

`allium-mode` provides the major mode for editing Allium files. It lives in its own repo: [juxt/allium-mode](https://github.com/juxt/allium-mode).

### Using use-package and straight.el

```elisp
(use-package allium-mode
  :straight (:host github :repo "juxt/allium-mode")
  :mode "\.allium'")
```

### Manual installation

1. Clone [juxt/allium-mode](https://github.com/juxt/allium-mode) and add the directory to your `load-path`.
2. Add the following to your `init.el`:

```elisp
(require 'allium-mode)
(add-to-list 'auto-mode-alist '("\.allium'" . allium-mode))
```

## 3. Configure LSP Client

### eglot (Recommended)

Eglot is the lightweight, built-in LSP client for Emacs.

```elisp
(use-package eglot
  :ensure t
  :config
  (add-to-list 'eglot-server-programs
               '(allium-mode . ("allium-lsp" "--stdio")))
  :hook (allium-mode . eglot-ensure))
```

### lsp-mode

```elisp
(use-package lsp-mode
  :ensure t
  :hook (allium-mode . lsp-deferred)
  :commands lsp
  :config
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection '("allium-lsp" "--stdio"))
                    :major-modes '(allium-mode)
                    :server-id 'allium-lsp)))
```

## 4. Emacs 29+ Tree-sitter Setup (Optional)

Emacs 29+ includes built-in support for tree-sitter, providing faster and more accurate highlighting and navigation.

1. Ensure the `allium` grammar is installed. You can add it to `treesit-language-source-alist` and run `treesit-install-language-grammar`.
2. Use `allium-ts-mode` instead of the standard `allium-mode`:

```elisp
(add-to-list 'auto-mode-alist '("\.allium'" . allium-ts-mode))
```

## Feature Reference

| Feature | Command / Interaction |
| :--- | :--- |
| **Hover** | `M-x eldoc` (or automatic via ElDoc) |
| **Go to Definition** | `M-.` (`xref-find-definitions`) |
| **Find References** | `M-?` (`xref-find-references`) |
| **Rename** | `M-x eglot-rename` or `M-x lsp-rename` |
| **Code Actions** | `M-x eglot-code-actions` or `s-l a` (lsp-mode) |
| **Formatting** | `M-x eglot-format-buffer` or `M-x lsp-format-buffer` |
| **Outline** | `M-x imenu` |

## What Is Available Today

`allium-mode` currently provides:

- Major mode support for `*.allium` files (`allium-mode`).
- Optional tree-sitter major mode (`allium-ts-mode`) on Emacs 29+ when the grammar is installed.
- Syntax highlighting, indentation, and line comments (`-- ...`).
- Imenu support in tree-sitter mode.
- LSP wiring for both `eglot` and `lsp-mode` with language id `allium`.

To access the language features in a buffer:

1. Open an `.allium` file.
2. Ensure your LSP client is active (`eglot-ensure` or `lsp-deferred`).
3. Use standard Emacs/Xref/LSP commands:
   - Definition: `M-.`
   - References: `M-?`
   - Rename: `M-x eglot-rename` or `M-x lsp-rename`
   - Code actions: `M-x eglot-code-actions` or `M-x lsp-execute-code-action`
   - Format: `M-x eglot-format-buffer` or `M-x lsp-format-buffer`

There are no custom allium-specific interactive commands at the moment; features are accessed through standard `xref`, `eldoc`, and your chosen LSP client commands.

## Troubleshooting

- **Server not found**: Run `M-x executable-find RET allium-lsp RET` to ensure Emacs can see the binary.
- **Connection Issues**: Check the `*eglot log*` or `*lsp-log*` buffers for raw JSON-RPC communication and server stderr.
- **Indentation**: Allium follows a standard 4-space indent. This can be customized via `allium-indent-offset`.
