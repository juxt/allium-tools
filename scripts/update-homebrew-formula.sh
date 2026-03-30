#!/usr/bin/env bash
set -euo pipefail

# Update the Homebrew formula with checksums from a GitHub release.
# CI handles this automatically during normal releases (see
# .github/workflows/release-artifacts.yml). This script is a manual
# fallback if the CI job fails.
#
# Usage:
#   ./scripts/update-homebrew-formula.sh [--dry-run] <version> [tap-path]
#
# Examples:
#   ./scripts/update-homebrew-formula.sh 1.0.0
#   ./scripts/update-homebrew-formula.sh --dry-run 0.1.4
#   ./scripts/update-homebrew-formula.sh 1.0.0 ~/Code/homebrew-allium

REPO="juxt/allium-tools"
TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-unknown-linux-gnu
  x86_64-unknown-linux-gnu
)

DRY_RUN=false
VERSION=""
TAP_PATH=""

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
    *)
      if [[ -z "$VERSION" ]]; then
        VERSION="$arg"
      else
        TAP_PATH="$arg"
      fi
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "Usage: update-homebrew-formula.sh [--dry-run] <version> [tap-path]" >&2
  exit 1
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must be in semver format (e.g. 1.0.0)" >&2
  exit 1
fi

# Default tap path: sibling directory to this repo
if [[ -z "$TAP_PATH" ]]; then
  TAP_PATH="$(cd "$(dirname "$0")/../.." && pwd)/homebrew-allium"
fi

FORMULA="$TAP_PATH/Formula/allium.rb"

if [[ ! -f "$FORMULA" ]]; then
  echo "Error: formula not found at $FORMULA" >&2
  exit 1
fi

echo "Fetching checksums for v$VERSION..."

declare -A CHECKSUMS
for target in "${TARGETS[@]}"; do
  url="https://github.com/$REPO/releases/download/v$VERSION/allium-$target.tar.gz"
  echo "  $target"
  sha=$(curl -fsSL "$url" | shasum -a 256 | awk '{print $1}')
  CHECKSUMS[$target]="$sha"
done

echo ""
echo "Checksums:"
for target in "${TARGETS[@]}"; do
  echo "  $target: ${CHECKSUMS[$target]}"
done
echo ""

if $DRY_RUN; then
  echo "(dry run — formula not modified)"
  exit 0
fi

# Update version
sed -i '' "s/^  version \".*\"/  version \"$VERSION\"/" "$FORMULA"

# Update checksums using the placeholder/previous values
for target in "${TARGETS[@]}"; do
  # Convert target to the placeholder name: hyphens to underscores, uppercase
  placeholder=$(echo "$target" | tr '-' '_' | tr '[:lower:]' '[:upper:]')
  sha="${CHECKSUMS[$target]}"
  # Replace either a placeholder or a previous 64-char hex hash on the line following the matching URL
  sed -i '' "/allium-${target}\.tar\.gz/{ n; s/sha256 \"[^\"]*\"/sha256 \"${sha}\"/; }" "$FORMULA"
done

echo "Updated $FORMULA to v$VERSION"
