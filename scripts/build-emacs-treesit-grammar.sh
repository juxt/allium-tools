#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$ROOT_DIR/packages/tree-sitter-allium/src"
OUT_DIR="$ROOT_DIR/.emacs-test/tree-sitter"
PARSER_C="$SRC_DIR/parser.c"

if [[ ! -f "$PARSER_C" ]]; then
  echo "Missing parser source: $PARSER_C" >&2
  exit 1
fi

if ! command -v cc >/dev/null 2>&1; then
  echo "C compiler (cc) is required to build tree-sitter grammar" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

SO_EXT="so"
UNAME_S="$(uname -s)"
if [[ "$UNAME_S" == "Darwin" ]]; then
  SO_EXT="dylib"
fi

OBJ_FILE="$OUT_DIR/tree-sitter-allium-parser.o"
LIB_FILE="$OUT_DIR/libtree-sitter-allium.$SO_EXT"

cc -std=c11 -fPIC -I"$SRC_DIR" -c "$PARSER_C" -o "$OBJ_FILE"
cc -shared "$OBJ_FILE" -o "$LIB_FILE"

echo "Built tree-sitter grammar: $LIB_FILE"
