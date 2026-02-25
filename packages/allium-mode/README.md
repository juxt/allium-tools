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

## Quick Demo (Isolated `-Q` Session)

You can try allium-mode quickly in a clean Emacs session without touching your
normal Emacs config.

First-time setup (installs repo-local test/demo dependencies into `.emacs-test`):

```bash
npm run test:emacs:install
```

Launch classic mode demo:

```bash
npm run demo:allium-mode
```

Launch tree-sitter mode demo:

```bash
npm run demo:allium-mode:ts
```

These commands run Emacs with `-Q` and repository-local package state, so they
do not modify your normal `~/.emacs.d` setup.

## Testing

Run the allium-mode ERT suite from the monorepo root:

```bash
npm run test:emacs
```

This runs Emacs in `-Q --batch` mode against deterministic unit tests for:
- core major mode behavior
- `eglot` registration
- `lsp-mode` client registration

Install integration-test dependencies into a repo-local Emacs test home:

```bash
npm run test:emacs:install
```

This installs packages into `.emacs-test/elpa` (gitignored), so batch runs
with `-Q` can still load required packages deterministically.
It also builds the repository's `tree-sitter-allium` grammar into
`.emacs-test/tree-sitter` for real `allium-ts-mode` grammar tests.

Run live integration tests (real `allium-lsp` process):

```bash
npm run test:emacs:integration
```

Integration tests run against:
- `eglot` (required for that test)
- `lsp-mode` (executed when installed; skipped otherwise)

## LSP Configuration

### eglot

```elisp
(add-hook 'allium-mode-hook 'eglot-ensure)
```

### lsp-mode

```elisp
(add-hook 'allium-mode-hook 'lsp-deferred)
```
