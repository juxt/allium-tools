# allium-mode

Emacs major mode for the Allium specification language.

## Installation

### Using straight.el

```elisp
(use-package allium-mode
  :straight (:host github :repo "juxt/allium-tools"
             :files ("packages/allium-mode/*.el"))
  :mode "\.allium'")
```

### Using Doom Emacs

In `packages.el`:

```elisp
(package! allium-mode
  :recipe (:host github :repo "juxt/allium-tools"
           :files ("packages/allium-mode/*.el")))
```

In `config.el`:

```elisp
(use-package! allium-mode
  :mode "\.allium'")
```

### Manual Installation

1. Clone this repository.
2. Add the `packages/allium-mode` directory to your `load-path`.
3. Add `(require 'allium-mode)` to your configuration.

## Features

- Syntax highlighting (regex-based or tree-sitter).
- Indentation support.
- LSP integration via `eglot` or `lsp-mode`.
- Tree-sitter support for Emacs 29+.

## LSP Configuration

### eglot

```elisp
(add-hook 'allium-mode-hook 'eglot-ensure)
```

### lsp-mode

```elisp
(add-hook 'allium-mode-hook 'lsp-deferred)
```
