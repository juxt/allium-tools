# Emacs setup guide for Allium

This guide explains how to set up Allium language support in Emacs using `allium-lsp` and `allium-mode`.

## Prerequisites

- **Emacs** >= 28.1 (Emacs 29+ recommended for built-in tree-sitter support).
- **eglot** (built-in since Emacs 29) or **lsp-mode**.

## 1. Install the Allium LSP server

The LSP server must be available in your system path as `allium-lsp`.

### From release artifacts

1. Download the `allium-lsp-<version>.tar.gz` for your platform from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
2. Extract the archive and move the `allium-lsp` binary to a directory in your `$PATH`.

### From source

```bash
cd packages/allium-lsp
npm install
npm run build
# Link the binary to your path
ln -s $(pwd)/dist/bin.js /usr/local/bin/allium-lsp
```

## 2. Install allium-mode

`allium-mode` provides the major mode for editing Allium files. It lives in its own repo: [juxt/allium-mode](https://github.com/juxt/allium-mode).

### Using package-vc (Emacs 29+)

```elisp
(unless (package-installed-p 'allium-mode)
  (let ((inhibit-message t))
    (package-vc-install '(allium-mode :url "https://github.com/juxt/allium-mode"))))

(use-package allium-mode
  :mode "\\.allium\\'")
```

### Using straight.el

```elisp
(use-package allium-mode
  :straight (:host github :repo "juxt/allium-mode")
  :mode "\\.allium\\'")
```

### Manual installation

1. Clone [juxt/allium-mode](https://github.com/juxt/allium-mode) and add the directory to your `load-path`.
2. Add the following to your `init.el`:

```elisp
(require 'allium-mode)
```

## 3. Configure LSP client

### eglot (recommended)

`allium-mode` registers itself with eglot automatically. Use `allium-eglot-ensure` to connect; it checks that the server is installed and shows install instructions if it is missing.

```elisp
(add-hook 'allium-mode-hook 'allium-eglot-ensure)
```

### lsp-mode

`allium-mode` registers itself with lsp-mode automatically.

```elisp
(add-hook 'allium-mode-hook 'lsp-deferred)
```

## 4. Tree-sitter grammar (optional)

On Emacs 29+, `allium-mode` automatically uses tree-sitter when the Allium grammar is installed, giving you more accurate highlighting and richer imenu navigation. Without the grammar, the mode falls back to regex-based highlighting.

To compile and install the grammar:

```
M-x treesit-install-language-grammar RET allium RET
```

When prompted for the repository URL, enter:

```
https://github.com/juxt/tree-sitter-allium
```

Accept the defaults for the remaining prompts. This compiles the grammar and installs the shared library into your `tree-sitter` directory. You need a C compiler available on your system (`cc`).

## Feature reference

| Feature | Command |
| :--- | :--- |
| Hover | `M-x eldoc` (or automatic via ElDoc) |
| Go to definition | `M-.` (`xref-find-definitions`) |
| Find references | `M-?` (`xref-find-references`) |
| Rename | `M-x eglot-rename` or `M-x lsp-rename` |
| Code actions | `M-x eglot-code-actions` or `M-x lsp-execute-code-action` |
| Formatting | `M-x eglot-format-buffer` or `M-x lsp-format-buffer` |
| Outline | `M-x imenu` |

## Troubleshooting

- **Server not found**: Run `M-x executable-find RET allium-lsp RET` to ensure Emacs can see the binary. If using `allium-eglot-ensure`, it will tell you what to do.
- **Connection issues**: Check the `*eglot log*` or `*lsp-log*` buffers for raw JSON-RPC communication and server stderr.
- **Indentation**: Allium uses a 4-space indent by default. Customise via `allium-indent-offset`.
