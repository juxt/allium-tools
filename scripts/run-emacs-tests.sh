#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v emacs >/dev/null 2>&1; then
  echo "Emacs is required to run allium-mode tests." >&2
  exit 1
fi

cd "$ROOT_DIR"

emacs -Q --batch \
  -L packages/allium-mode \
  -L packages/allium-mode/test \
  -l packages/allium-mode/test/allium-mode-test-runner.el
