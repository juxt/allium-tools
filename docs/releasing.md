# Releasing

Pushing a `v*` tag triggers the release workflow, which builds and publishes all artifacts: Rust binaries, VSIX, npm tarballs, LSP and tree-sitter outputs. Everything ships from a single tag.

Not every artifact needs manual follow-up. The Homebrew formula is the only piece that requires a separate update after CI finishes.

## What goes where

| Artifact | Destination | Needs manual step? |
|---|---|---|
| Rust CLI binaries (4 platforms) | GitHub release | No (CI publishes) |
| VSIX | GitHub release | No (CI publishes) |
| npm tarballs (allium-cli, allium-lsp, tree-sitter) | GitHub release | No (CI publishes) |
| Homebrew formula | homebrew-allium tap | Yes |

## Steps

1. **Bump versions** with `scripts/version-bump.sh`:

   ```bash
   ./scripts/version-bump.sh 3.0.0
   ```

   For major language version bumps, also follow the checklist in `docs/versioning.md`.

2. **Commit, tag and push:**

   ```bash
   git add -A && git commit -m "v3.0.0"
   git tag v3.0.0
   git push origin main --tags
   ```

3. **Wait for CI** to build and attach release artifacts.

4. **Update the Homebrew tap:**

   ```bash
   ./scripts/update-homebrew-formula.sh 3.0.0
   ```

   This downloads the four platform tarballs, computes SHA256 checksums and rewrites the formula. Pass `--dry-run` to preview without modifying the file.

5. **Push the tap repo:**

   ```bash
   cd ~/Code/homebrew-allium
   git add -A && git commit -m "allium 3.0.0"
   git push
   ```

## When to release

All core-tier packages share a major.minor version (see `docs/versioning.md`). Right now, every release cuts a single tag and publishes everything. This means a tree-sitter bugfix produces new Rust binaries even if nothing changed in the Rust crates, and vice versa.

This is fine while the project is small. If the coupling becomes a problem, the natural split is separate tags per artifact group (e.g. `cli-v1.0.1`, `vscode-v0.3.0`), each triggering only its own CI job. That would also let the Homebrew update script run automatically as a post-release workflow step rather than requiring a manual invocation.
