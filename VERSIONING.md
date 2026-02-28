# Versioning policy

The Allium language is at version 1. Tooling versions should align with the language where appropriate.

## Version tiers

### Core tier: major.minor tracks the language

These packages share a major.minor version that tracks the Allium language version. Patch versions may differ between packages (e.g. a parser bugfix doesn't force a CLI release).

| Package | Manifest | Version source |
|---|---|---|
| allium-parser | `crates/allium-parser/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium (Rust CLI) | `crates/allium/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium-cli (Node) | `packages/allium-cli/package.json` | Hardcoded |
| allium-lsp | `packages/allium-lsp/package.json` | Hardcoded |
| tree-sitter-allium | `packages/tree-sitter-allium/package.json` | Hardcoded |

The canonical major.minor lives in two places:

- `Cargo.toml` workspace version (Rust crates)
- `package.json` root version (npm packages)

These two must always agree on major.minor.

### Editor tier: versions independently

Editor plugins are thin integration layers that delegate to the LSP and tree-sitter. They version at their own pace, reflecting their own maturity and feature set.

| Package | Manifest |
|---|---|
| allium-vscode | `extensions/allium/package.json` |
| allium-mode | `packages/allium-mode/allium-mode.el` and `allium-mode-pkg.el` |
| nvim-allium | No version declared (distributed via plugin managers) |

Editor plugins should document which core version they're compatible with in their README.

## Bumping versions

Use `scripts/version-bump.sh` to update core-tier versions:

```bash
# Set all core-tier packages to 1.0.0
./scripts/version-bump.sh 1.0.0

# Dry run — show what would change without writing
./scripts/version-bump.sh --dry-run 1.0.0
```

Editor-tier packages are bumped manually as needed.

## Rules

1. A grammar or language-level change bumps the core-tier minor (or major) version.
2. A bugfix in a single core package bumps only that package's patch version.
3. Editor plugins declare their own versions and note compatible core versions.
4. The two canonical version sources (Cargo workspace, root package.json) must always share the same major.minor.
