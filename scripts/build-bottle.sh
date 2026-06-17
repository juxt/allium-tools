#!/usr/bin/env bash
#
# Build a Homebrew bottle for the allium CLI from a prebuilt release binary.
#
# Shipping bottles makes `brew install` POUR a prebuilt package instead of
# running the formula's `install` from source in Homebrew's build sandbox. On
# macOS Tahoe that sandbox's deny_read_home step realpaths TCC-protected home
# folders (~/Documents) and aborts when the terminal lacks access
# (juxt/allium#42). Pouring never enters that path.
#
# The bottle is tagged for the runner's OS; because the binary is
# relocation-independent (:any_skip_relocation), Homebrew's OS-version fallback
# pours one arm64 (or x86_64) bottle on any newer macOS of the same arch — so
# one bottle per arch suffices.
#
# Usage:
#   build-bottle.sh <target> <version> <artifact.tar.gz> <root_url> [out_dir]
#
# Example:
#   build-bottle.sh aarch64-apple-darwin 3.4.0 ./allium-aarch64-apple-darwin.tar.gz \
#     https://github.com/juxt/allium-tools/releases/download/v3.4.0
#
# On success prints two lines to stdout:
#   tag=<bottle tag e.g. arm64_sonoma>
#   sha256=<sha of the bottle tarball>
# and writes the pourable bottle file <out_dir>/allium-<version>.<tag>.bottle.tar.gz
set -euo pipefail

TARGET="${1:?target}"; VERSION="${2:?version}"; ARTIFACT="${3:?artifact}"; ROOT_URL="${4:?root_url}"
OUT_DIR="${5:-$PWD}"
ARTIFACT="$(cd "$(dirname "$ARTIFACT")" && pwd)/$(basename "$ARTIFACT")"
TAP="local/alliumbottle"

cleanup() {
  brew uninstall --force allium >/dev/null 2>&1 || true
  brew untap "$TAP" >/dev/null 2>&1 || true
}
trap cleanup EXIT

brew tap-new "$TAP" >/dev/null 2>&1 || true
FDIR="$(brew --repo "$TAP")/Formula"; mkdir -p "$FDIR"
cat > "$FDIR/allium.rb" <<RUBY
class Allium < Formula
  desc "Checker and parser for the Allium specification language"
  homepage "https://github.com/juxt/allium-tools"
  url "file://${ARTIFACT}"
  version "${VERSION}"
  def install
    bin.install "allium"
  end
end
RUBY

brew uninstall --force allium >/dev/null 2>&1 || true
brew install --build-bottle --formula "$TAP/allium" >/dev/null

workdir="$(mktemp -d)"
( cd "$workdir" && brew bottle --json --no-rebuild --root-url="$ROOT_URL" "$TAP/allium" >/dev/null )

json="$(ls "$workdir"/allium--*.bottle.json)"
# brew bottle emits the local (double-dash) tarball; the remote name Homebrew
# fetches from root_url uses a single dash. Read both, plus tag and sha, from
# the JSON manifest so we never guess the OS tag.
read -r tag sha local_name remote_name < <(python3 - "$json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
tags = list(d.values())[0]["bottle"]["tags"]
tag = next(iter(tags))
t = tags[tag]
print(tag, t["sha256"], t["local_filename"], t["filename"])
PY
)

mkdir -p "$OUT_DIR"
cp "$workdir/$local_name" "$OUT_DIR/$remote_name"
rm -rf "$workdir"

echo "tag=$tag"
echo "sha256=$sha"
echo "bottle built: $OUT_DIR/$remote_name (target=$TARGET)" >&2
