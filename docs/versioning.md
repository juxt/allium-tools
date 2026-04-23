# Versioning policy

The Allium language is at version 3. All packages in this repo share a single version that tracks the language.

## Packages

| Package | Manifest | Version source |
|---|---|---|
| allium-parser | `crates/allium-parser/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium (Rust CLI) | `crates/allium/Cargo.toml` | Cargo workspace (`Cargo.toml`) |
| allium-lsp | `packages/allium-lsp/package.json` | Hardcoded |
| allium-vscode | `extensions/allium/package.json` | Hardcoded |

The canonical version lives in two places:

- `Cargo.toml` workspace version (Rust crates)
- `package.json` root version (npm packages)

These two must always agree.

## External editor plugins

Editor plugins in separate repos version independently and note compatible core versions in their README.

| Package | Repo |
|---|---|
| tree-sitter-allium | [juxt/tree-sitter-allium](https://github.com/juxt/tree-sitter-allium) |
| allium-mode | [juxt/allium-mode](https://github.com/juxt/allium-mode) |
| nvim-allium | [juxt/nvim-allium](https://github.com/juxt/nvim-allium) |

## Bumping versions

Use `scripts/version-bump.sh` to update all package versions:

```bash
# Set all packages to 3.1.0
./scripts/version-bump.sh 3.1.0

# Dry run — show what would change without writing
./scripts/version-bump.sh --dry-run 3.1.0
```

## Major language version release checklist

When the Allium language version changes (e.g. v2 → v3):

1. Run `scripts/version-bump.sh <new-version>` to update all manifests.
2. Update the language version statement at the top of this file.
3. Update the "Compatibility" line in each external editor plugin README.
4. After CI publishes the release, run `scripts/update-homebrew-formula.sh <version>` and push the tap repo. See `docs/releasing.md` for the full steps.

## Rules

1. A grammar or language-level change bumps the minor (or major) version.
2. A bugfix bumps the patch version.
3. All packages in this repo share the same version.
4. The two canonical version sources (Cargo workspace, root package.json) must always agree.
