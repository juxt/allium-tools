# Releasing

Pushing a `v*` tag triggers the release workflow, which builds and publishes all artifacts: Rust binaries, VSIX, npm tarballs, LSP and tree-sitter outputs. Everything ships from a single tag.

After CI finishes, two steps need manual action: publishing to crates.io and updating the Homebrew formula. The release script handles the full sequence.

## What goes where

| Artifact | Destination | Automated? |
|---|---|---|
| Rust CLI binaries (4 platforms) | GitHub release | CI |
| Rust crates (allium-parser, allium-cli) | crates.io | Release script |
| VSIX | GitHub release | CI |
| npm tarballs (allium-cli, allium-lsp, tree-sitter) | GitHub release | CI |
| Homebrew formula | homebrew-allium tap | Release script |

## Running a release

```bash
./scripts/release.sh 3.1.0
```

This bumps versions, commits, tags, pushes, waits for CI, publishes to crates.io and updates the Homebrew tap. Pass `--dry-run` to preview each step without making changes.

For major language version bumps, also follow the checklist in `docs/versioning.md`.

Requires `gh` (GitHub CLI), `cargo` logged in to crates.io, and the `homebrew-allium` tap repo as a sibling directory.

## Manual steps (if not using the script)

1. Bump versions: `./scripts/version-bump.sh <version>`
2. Commit, tag and push: `git add -A && git commit -m "v<version>" && git tag v<version> && git push origin main --tags`
3. Wait for CI to build and attach release artifacts
4. Publish to crates.io: `cargo publish -p allium-parser && cargo publish -p allium-cli`
5. Update Homebrew formula: `./scripts/update-homebrew-formula.sh <version>`
6. Push the tap repo

## When to release

All core-tier packages share a major.minor version (see `docs/versioning.md`). Right now, every release cuts a single tag and publishes everything. This means a tree-sitter bugfix produces new Rust binaries even if nothing changed in the Rust crates, and vice versa.

This is fine while the project is small. If the coupling becomes a problem, the natural split is separate tags per artifact group (e.g. `cli-v1.0.1`, `vscode-v0.3.0`), each triggering only its own CI job. That would also let the Homebrew update script run automatically as a post-release workflow step rather than requiring a manual invocation.
