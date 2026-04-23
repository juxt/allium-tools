#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="${1:-$ROOT_DIR/artifacts}"
CACHE_DIR="$ROOT_DIR/.npm-cache"
mkdir -p "$CACHE_DIR"

mkdir -p "$ARTIFACT_DIR"
rm -f "$ARTIFACT_DIR"/*.vsix "$ARTIFACT_DIR"/*.tar.gz "$ARTIFACT_DIR"/SHA256SUMS.txt

VERSION="$(node -p "require('./extensions/allium/package.json').version")"

echo "Building LSP server..."
npm run --workspace packages/allium-lsp build

echo "Building extension (with bundled LSP binary)..."
npm run --workspace extensions/allium build:release

VSIX_NAME="allium-vscode-${VERSION}.vsix"

echo "Packaging VSIX artifact..."
(
  cd "$ROOT_DIR/extensions/allium"
  npx @vscode/vsce package --allow-missing-repository --no-dependencies --out "$ARTIFACT_DIR/$VSIX_NAME"
)

echo "Packaging allium-lsp binary..."
LSP_TARBALL="allium-lsp-${VERSION}.tar.gz"
(
  cd "$ROOT_DIR/packages/allium-lsp"
  mkdir -p /tmp/allium-lsp-release/allium-lsp
  cp dist/bin.js /tmp/allium-lsp-release/allium-lsp/allium-lsp
  cp dist/allium_wasm.js dist/allium_wasm_bg.wasm /tmp/allium-lsp-release/allium-lsp/
  chmod +x /tmp/allium-lsp-release/allium-lsp/allium-lsp
  tar -czf "$ARTIFACT_DIR/$LSP_TARBALL" -C /tmp/allium-lsp-release allium-lsp
  rm -rf /tmp/allium-lsp-release
)

echo "Generating checksums..."
(
  cd "$ARTIFACT_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum ./*.vsix ./*.tar.gz > SHA256SUMS.txt
  else
    shasum -a 256 ./*.vsix ./*.tar.gz > SHA256SUMS.txt
  fi
)

echo "Release artifacts created in $ARTIFACT_DIR:"
ls -1 "$ARTIFACT_DIR"
