# Releasing

Pushing a `v*` tag triggers the release workflow, which builds and publishes all artifacts: Rust binaries, VSIX, npm tarballs, LSP and tree-sitter outputs. Everything ships from a single tag.

Publishing is automated for every channel except the AUR: CI publishes the GitHub release, pushes the crates to crates.io (`publish-crates` job, via the `CARGO_REGISTRY_TOKEN` secret) and updates the Homebrew formula (`update-homebrew` job, via the `HOMEBREW_TAP_TOKEN` secret). No local crates.io login or tap checkout is needed. The AUR package is bumped by hand once the tag is live — see [AUR](#aur) below.

## What goes where

| Artifact | Destination | Automated? |
|---|---|---|
| Rust CLI binaries (5 targets) | GitHub release | CI |
| Rust crates (allium-parser, allium-cli) | crates.io | CI |
| VSIX | GitHub release | CI |
| npm tarballs (allium-cli, allium-lsp, tree-sitter) | GitHub release | CI |
| Homebrew formula | homebrew-allium tap | CI |
| AUR package | aur.archlinux.org/allium-tools | manual — see [AUR](#aur) |

## Running a release

```bash
./scripts/release.sh 3.1.0
```

This bumps versions, commits, tags, pushes, and watches CI through to completion; CI publishes everything, including crates.io and the Homebrew tap. Pass `--dry-run` to preview each step without making changes.

For major language version bumps, also follow the checklist in `docs/versioning.md`.

Requires `gh` (GitHub CLI) and `cargo` on the PATH. It does not touch the AUR.

## AUR

The AUR package is the one channel neither CI nor `release.sh` publishes, so it goes stale silently until someone bumps it by hand. It builds `allium` from the release source tarball, which means a bump is only ever "point at the new tag, refresh the checksum, build once to check".

`aur/` is a submodule tracking the AUR repo. The procedure lives in `aur/README.md`:

```bash
git submodule update --init aur
```

Pushing needs an AUR account whose SSH key is registered against the package, so the submodule clones read-only over HTTPS; maintainers set the push URL locally.

## Manual steps (if not using the script)

1. Bump versions: `./scripts/version-bump.sh <version>`
2. Commit, tag and push: `git add -A && git commit -m "v<version>" && git tag v<version> && git push origin main --tags`
3. Watch CI build, attach and publish everything
4. Bump the AUR package — see [AUR](#aur). Needed whether or not you used the script.

If the CI publish jobs fail, the fallbacks are `cargo publish -p allium-parser && cargo publish -p allium-cli` (needs a crates.io login) and `./scripts/update-homebrew-formula.sh <version>` plus a push of the `homebrew-allium` tap repo.

## When to release

All core-tier packages share a major.minor version (see `docs/versioning.md`). Right now, every release cuts a single tag and publishes everything. This means a tree-sitter bugfix produces new Rust binaries even if nothing changed in the Rust crates, and vice versa.

This is fine while the project is small. If the coupling becomes a problem, the natural split is separate tags per artifact group (e.g. `cli-v1.0.1`, `vscode-v0.3.0`), each triggering only its own CI job.
