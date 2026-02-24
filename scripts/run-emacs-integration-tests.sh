#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v emacs >/dev/null 2>&1; then
  echo "Emacs is required to run allium-mode integration tests." >&2
  exit 1
fi

cd "$ROOT_DIR"

npm run --workspace packages/allium-lsp build

emacs -Q --batch \
  -L scripts \
  -L packages/allium-mode \
  -L packages/allium-mode/test \
  -l scripts/emacs-test-bootstrap.el \
  -l packages/allium-mode/test/allium-mode-integration-test-runner.el
