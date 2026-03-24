#!/usr/bin/env bash
set -euo pipefail

# Run a full release: bump versions, tag, push, wait for CI, publish to
# crates.io and update the Homebrew formula.
#
# Usage:
#   ./scripts/release.sh [--dry-run] <version>
#
# Examples:
#   ./scripts/release.sh 3.1.0
#   ./scripts/release.sh --dry-run 3.1.0

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
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
  echo "Usage: release.sh [--dry-run] <version>" >&2
  exit 1
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must be in semver format (e.g. 3.1.0)" >&2
  exit 1
fi

TAG="v$VERSION"

run() {
  if $DRY_RUN; then
    echo "  [dry-run] $*"
  else
    "$@"
  fi
}

confirm() {
  local prompt="$1"
  if $DRY_RUN; then return 0; fi
  read -rp "$prompt [y/N] " answer
  [[ "$answer" =~ ^[Yy]$ ]]
}

# ── Preflight checks ────────────────────────────────────────────────

echo "==> Preflight checks"

if [[ "$(git branch --show-current)" != "main" ]]; then
  echo "Error: not on main branch" >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Error: working tree is dirty" >&2
  exit 1
fi

if ! command -v gh &>/dev/null; then
  echo "Error: gh CLI not found (needed to watch CI)" >&2
  exit 1
fi

if ! command -v cargo &>/dev/null; then
  echo "Error: cargo not found" >&2
  exit 1
fi

if git rev-parse "$TAG" &>/dev/null; then
  echo "Error: tag $TAG already exists" >&2
  exit 1
fi

echo "  All checks passed."
echo ""

# ── Step 1: Bump versions ───────────────────────────────────────────

echo "==> Bumping versions to $VERSION"
if $DRY_RUN; then
  "$ROOT/scripts/version-bump.sh" --dry-run "$VERSION"
else
  "$ROOT/scripts/version-bump.sh" "$VERSION"
fi
echo ""

# ── Step 2: Commit, tag and push ────────────────────────────────────

echo "==> Commit, tag and push"
run git add -A
run git commit -m "$TAG"
run git tag "$TAG"
run git push origin main --tags
echo ""

# ── Step 3: Wait for CI ─────────────────────────────────────────────

echo "==> Waiting for CI (Release Artifacts workflow)"
if $DRY_RUN; then
  echo "  [dry-run] would wait for CI to complete"
else
  echo "  Waiting for workflow run to appear..."
  sleep 5

  # Poll until the workflow run for this tag appears
  for i in $(seq 1 12); do
    RUN_ID=$(gh run list --workflow=release-artifacts.yml --branch="$TAG" --limit=1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)
    if [[ -n "$RUN_ID" ]]; then break; fi
    sleep 5
  done

  if [[ -z "${RUN_ID:-}" ]]; then
    echo "Error: could not find workflow run for $TAG" >&2
    echo "Check https://github.com/juxt/allium-tools/actions manually." >&2
    exit 1
  fi

  echo "  Watching run $RUN_ID..."
  gh run watch "$RUN_ID" --exit-status
fi
echo ""

# ── Step 4: Publish to crates.io ────────────────────────────────────

echo "==> Publishing to crates.io"
run cargo publish -p allium-parser
run cargo publish -p allium-cli
echo ""

# ── Step 5: Update Homebrew formula ─────────────────────────────────

echo "==> Updating Homebrew formula"
if $DRY_RUN; then
  "$ROOT/scripts/update-homebrew-formula.sh" --dry-run "$VERSION"
else
  "$ROOT/scripts/update-homebrew-formula.sh" "$VERSION"
fi
echo ""

# ── Step 6: Push Homebrew tap ───────────────────────────────────────

TAP_PATH="$(cd "$ROOT/.." && pwd)/homebrew-allium"

echo "==> Pushing Homebrew tap"
if [[ ! -d "$TAP_PATH" ]]; then
  echo "  Warning: tap not found at $TAP_PATH, skipping."
  echo "  Push the formula update manually."
else
  run git -C "$TAP_PATH" add -A
  run git -C "$TAP_PATH" commit -m "allium $VERSION"
  run git -C "$TAP_PATH" push
fi
echo ""

# ── Done ─────────────────────────────────────────────────────────────

echo "Release $TAG complete."
echo ""
echo "Published:"
echo "  - GitHub release: https://github.com/juxt/allium-tools/releases/tag/$TAG"
echo "  - crates.io:      https://crates.io/crates/allium-cli/$VERSION"
echo "  - Homebrew:        brew upgrade allium"
