#!/usr/bin/env bash
set -euo pipefail

# Bump package versions across the monorepo.
# See docs/versioning.md for the versioning policy.
#
# Usage:
#   ./scripts/version-bump.sh [--dry-run] <version>
#
# Examples:
#   ./scripts/version-bump.sh 1.0.0
#   ./scripts/version-bump.sh --dry-run 1.0.0

DRY_RUN=false
VERSION=""

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
    *) VERSION="$arg" ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "Usage: version-bump.sh [--dry-run] <version>" >&2
  exit 1
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must be in semver format (e.g. 1.0.0)" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

update_json_version() {
  local file="$1"
  local full_path="$ROOT/$file"

  if [[ ! -f "$full_path" ]]; then
    echo "  SKIP  $file (not found)"
    return
  fi

  local current
  current=$(grep -m1 '"version"' "$full_path" | sed 's/.*"version": *"\([^"]*\)".*/\1/')

  if [[ "$current" == "$VERSION" ]]; then
    echo "  OK    $file (already $VERSION)"
    return
  fi

  if $DRY_RUN; then
    echo "  WOULD $file: $current -> $VERSION"
  else
    sed -i '' "s/\"version\": *\"$current\"/\"version\": \"$VERSION\"/" "$full_path"
    echo "  SET   $file: $current -> $VERSION"
  fi
}

update_cargo_version() {
  local file="$1"
  local full_path="$ROOT/$file"

  if [[ ! -f "$full_path" ]]; then
    echo "  SKIP  $file (not found)"
    return
  fi

  local current
  current=$(grep -m1 '^version' "$full_path" | sed 's/.*"\([^"]*\)".*/\1/')

  if [[ "$current" == "$VERSION" ]]; then
    echo "  OK    $file (already $VERSION)"
    return
  fi

  if $DRY_RUN; then
    echo "  WOULD $file: $current -> $VERSION"
  else
    sed -i '' "s/^version = \"$current\"/version = \"$VERSION\"/" "$full_path"
    echo "  SET   $file: $current -> $VERSION"
  fi
}

echo "Version bump -> $VERSION"
if $DRY_RUN; then
  echo "(dry run — no files will be modified)"
fi
echo ""

# Cargo workspace
update_cargo_version "Cargo.toml"

# npm packages
update_json_version "package.json"
update_json_version "packages/allium-lsp/package.json"
update_json_version "extensions/allium/package.json"
echo ""
echo "Done."
