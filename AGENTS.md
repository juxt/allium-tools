# AGENTS.md

Guidance for human and AI agents working on this codebase.

The goals are:

* High-quality, maintainable, professional code
* Strong, meaningful tests (quality over quantity)
* Safe, predictable collaboration between humans and LLMs

> **Important:** The `README.md` is the initial guide to the key concepts in the app.
> Always read or skim `README.md` at the **start of every session**.

## 1. Coding principles

* **Clarity over cleverness**: prefer simple, readable solutions.
* **Minimal, focused changes**: make the smallest change that solves the problem well.
* **Consistency with existing patterns**: match naming, structure, and design already used in the repo.
* **Preserve existing behavior unless explicitly changing it**.
* **Respect architecture**: extend existing abstractions rather than invent new ones.
* **Error handling**: follow established error-handling conventions; avoid silent failures.
* **Dependencies**: avoid adding new libraries unless absolutely necessary and justified.

---

## 2. Testing principles

The application must be **well tested**. We value **meaningful, behavior-focused tests**, not raw coverage numbers.

* **Unit tests**: all non-trivial logic (utilities, domain rules, data transformations) must have high-quality unit tests.
* **Integration tests**: use integration tests where components, services, or layers interact — particularly around data flows, API boundaries, and LSP protocol boundaries.
* Test key paths, edge cases, and important error conditions.
* Avoid breaking real behaviour just to satisfy tests.
* Prefer realistic tests that reflect actual usage.
* Follow established testing conventions in the repository.
* When adding new features, create or update tests in a way that protects important logic.

---

## 3. LLM editing rules for code and tests

When modifying code or tests:

* Make targeted, minimal changes.
* Preserve formatting and style.
* Avoid refactoring unless explicitly asked.
* Do not modify large unrelated regions of code.
* Do not use `git commit --no-verify` unless the user gives explicit permission in the current conversation.
* After completing any new feature, update the Allium specs in `docs/project/specs/` so they reflect the current system behaviour before finishing the work.

### Tests

* Prioritize meaningful coverage: critical branches, behaviors, and edge cases.
* Avoid brittle tests tied to internal implementation details.
* Do not degrade real behavior to satisfy tests.
* Fix incorrect tests where appropriate.

## 4. Security, privacy, and sensitive data

* Never commit or write secrets, tokens, passwords, or user data.
* When in doubt, anonymize.
* Any suspected sensitive data must be replaced with `[REDACTED]`.

## 5. Sources of truth

* **Allium language semantics and syntax:** https://juxt.github.io/allium/language (authoritative for any language-level behaviour in this repository)
* **Architecture & key concepts:** `README.md` and `docs/project/architecture.md`.
* **Versioning policy:** `VERSIONING.md` — defines which packages share versions and how to bump them.

---

## 6. Monorepo structure

This is an npm workspace monorepo. Each package has its own `package.json`, `tsconfig.json`, and test suite.

```text
extensions/allium/          VS Code extension (LSP client launcher)
packages/allium-lsp/        Language Server Protocol server (wraps language-tools/)
packages/allium-cli/        Standalone CLI package (allium-check, allium-format, etc.)
packages/tree-sitter-allium/ Tree-sitter grammar for the Allium language
packages/allium-mode/       Emacs major mode package
```

> **Note:** The Neovim plugin lives in its own repo: [juxt/nvim-allium](https://github.com/juxt/nvim-allium).

### Architecture

The core language intelligence lives in `extensions/allium/src/language-tools/` — pure TypeScript with **zero editor API dependencies**. All functions accept plain text and return plain data.

```text
language-tools/   Pure analysis engine: parser, analyzer, hover, definitions, rename, refactors, etc.
     |
     ├── packages/allium-lsp/      LSP server: wraps language-tools/ over JSON-RPC (stdio)
     |        |
     |        ├── extensions/allium/   VS Code launcher
     |        └── packages/allium-mode/ Emacs major mode
     |
     └── packages/allium-cli/      CLI tools: direct consumers of language-tools/

packages/tree-sitter-allium/   Structural grammar (used by Neovim, Emacs, GitHub, etc.)
```

### Development setup

```bash
# Install all workspace dependencies
npm install

# Build all workspaces
npm run build

# Run all tests
npm run test

# Build a specific workspace
npm run --workspace packages/allium-lsp build

# Run tests for a specific workspace
npm run --workspace extensions/allium test
```

### Build order

1. **`packages/tree-sitter-allium`**: Generate the parser first.
2. **`packages/allium-lsp`**: Build the server.
3. **`extensions/allium`**: The VS Code extension bundles the LSP server binary.
4. **`packages/allium-cli`**: Shares logic from the extension source.

### Testing Workflow

- **Unit tests**: Run `npm run test` in the relevant package.
- **Corpus tests**: Run `tree-sitter test` in `packages/tree-sitter-allium`.
- **Integration tests**: Run `npm run test` in `extensions/allium` to test LSP-to-engine interactions.

### Adding new editor integrations

New editor integrations live under `packages/`. Each should:
1. Point its LSP client at the `allium-lsp` binary.
2. Use tree-sitter query files from `packages/tree-sitter-allium/queries/`.
3. Include a README with setup instructions.
4. Provide a `docs/editors/<name>.md` guide.
