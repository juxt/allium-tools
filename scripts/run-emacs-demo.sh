#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-classic}"

if ! command -v emacs >/dev/null 2>&1; then
  echo "Emacs is required to run the allium-mode demo." >&2
  exit 1
fi

if [[ "$MODE" != "classic" && "$MODE" != "ts" ]]; then
  echo "Usage: $0 [classic|ts]" >&2
  exit 1
fi

cd "$ROOT_DIR"

if [[ ! -d ".emacs-test/elpa" ]]; then
  cat >&2 <<'EOF'
Missing .emacs-test package state.
Run `npm run test:emacs:install` once, then retry.
EOF
  exit 1
fi

npm run --workspace packages/allium-lsp build >/dev/null

EMACS_ARGS=(
  -Q
  -L scripts
  -L packages/allium-mode
  -l scripts/emacs-test-bootstrap.el
  --eval "(setq allium-demo-root default-directory)"
  --eval "(require 'allium-mode)"
  --eval "(setq allium-lsp-server-command (list \"node\" (expand-file-name \"packages/allium-lsp/dist/bin.js\" allium-demo-root) \"--stdio\"))"
  --eval "(unless (require 'eglot nil t) (error \"eglot is unavailable; run npm run test:emacs:install\"))"
  --eval "(add-hook 'allium-mode-hook #'eglot-ensure)"
  --eval "(find-file (expand-file-name \"docs/project/specs/allium-emacs-mode-behaviour.allium\" allium-demo-root))"
)

if [[ "$MODE" == "ts" ]]; then
  EMACS_ARGS+=(
    --eval "(add-to-list 'treesit-extra-load-path (expand-file-name \".emacs-test/tree-sitter\" allium-demo-root))"
    --eval "(allium-ts-mode)"
  )
else
  EMACS_ARGS+=(--eval "(allium-mode)")
fi

emacs "${EMACS_ARGS[@]}"
