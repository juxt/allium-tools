#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v emacs >/dev/null 2>&1; then
  echo "Emacs is required to install allium-mode test dependencies." >&2
  exit 1
fi

cd "$ROOT_DIR"

emacs -Q --batch \
  -L scripts \
  -l scripts/emacs-test-install.el

"$ROOT_DIR/scripts/build-emacs-treesit-grammar.sh"
