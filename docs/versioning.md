# Versioning policy

The Allium language is at version 2. Tooling versions should align with the language where appropriate.

## Version tiers

### Core tier: major.minor tracks the language

These packages share a major.minor version that tracks the Allium language version. Patch versions may differ between packages (e.g. a parser bugfix doesn't force a CLI release).

| Package | Manifest | Version source |
|---|---|---|
| allium-parser | `crates/allium-parser/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium (Rust CLI) | `crates/allium/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium-cli (Node) | `packages/allium-cli/package.json` | Hardcoded |
| allium-lsp | `packages/allium-lsp/package.json` | Hardcoded |
| tree-sitter-allium | Separate repo: [juxt/tree-sitter-allium](https://github.com/juxt/tree-sitter-allium) | Hardcoded |

The canonical major.minor lives in two places:

- `Cargo.toml` workspace version (Rust crates)
- `package.json` root version (npm packages)

These two must always agree on major.minor.

### Editor tier: versions independently

Editor plugins are thin integration layers that delegate to the LSP and tree-sitter. They version at their own pace, reflecting their own maturity and feature set.

| Package | Manifest |
|---|---|
| allium-vscode | `extensions/allium/package.json` |
| allium-mode | Separate repo: [juxt/allium-mode](https://github.com/juxt/allium-mode) |
| nvim-allium | Separate repo: [juxt/nvim-allium](https://github.com/juxt/nvim-allium) |

Editor plugins should document which core version they're compatible with in their README.

## Bumping versions

Use `scripts/version-bump.sh` to update core-tier versions:

```bash
# Set all core-tier packages to 2.0.0
./scripts/version-bump.sh 2.0.0

# Dry run — show what would change without writing
./scripts/version-bump.sh --dry-run 2.0.0
```

Editor-tier packages are bumped manually as needed.

## Major language version release checklist

When the Allium language version changes (e.g. v1 → v2), the following steps are needed beyond a normal version bump.

1. **Core tier.** Run `scripts/version-bump.sh <new-version>` to update all core-tier manifests.
2. **This document.** Update the language version statement at the top of this file.
3. **Editor plugins.** Check whether the VS Code extension has changes on the release branch (`git diff main --stat -- extensions/`). If so, bump its version in `extensions/allium/package.json`. Other editor plugins live in separate repos and are versioned independently:
   - allium-mode: [juxt/allium-mode](https://github.com/juxt/allium-mode)
   - nvim-allium: [juxt/nvim-allium](https://github.com/juxt/nvim-allium)
4. **Compatibility notes.** Update the "Compatibility" line in each editor plugin README to reference the new core version.
5. **Homebrew.** After CI publishes the release, run `scripts/update-homebrew-formula.sh <version>` and push the tap repo. See `docs/releasing.md` for the full steps.

## Rules

1. A grammar or language-level change bumps the core-tier minor (or major) version.
2. A bugfix in a single core package bumps only that package's patch version.
3. Editor plugins declare their own versions and note compatible core versions in their README.
4. The two canonical version sources (Cargo workspace, root package.json) must always share the same major.minor.
