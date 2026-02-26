# Allium Tools plan

## Product architecture

One extension with an internal modular split:

- `extensions/allium/language-basics` — language registration (`.allium`), TextMate grammar, language configuration, snippets.
- `extensions/allium/src/language-tools` — diagnostics, quick fixes, refactorings, test scaffold, and shared analysis logic consumed by both the LSP and CLI tools.

## Outstanding work

### Formatter depth

Structure-aware indentation, top-level block spacing, trailing whitespace removal and final newline enforcement are implemented. Remaining:

- Alignment rules for multi-line clause values.

### Distribution

- Marketplace publishing for the VS Code extension (VSIX and CLI archive artifacts already ship via GitHub Actions).

### Rust parser and structural validator

A standalone Rust parser that produces a typed AST and enforces the language reference validation rules. Replaces the regex-based parser with a hand-written recursive descent parser, distributed as a native binary via homebrew, apt and GitHub releases. Includes an MCP server for LLM-in-the-loop validation.

See [parser-roadmap.md](parser-roadmap.md) for the detailed plan, tree-sitter grammar audit and implementation phases.

The TypeScript analyzer continues to serve the editor tooling. Over time, the Rust parser can be consumed from TypeScript via WASM or stdio, replacing the regex-based parser and enabling full structural/type validation and cross-file type-flow analysis.

### Additional editor parity

Diagram preview, rule simulation and test scaffold generation are exposed as LSP custom requests (`allium/getDiagram`, `allium/simulateRule`, `allium/generateScaffold`) so any LSP client can use them. What's missing is documentation and convenience wrappers in the Neovim and Emacs setup guides.

## Engineering approach

- TypeScript strict mode for editor tooling and LSP.
- Rust for the standalone parser, CLI and MCP server.
- Small composable analysers with unit tests.
- Golden-file fixtures for diagnostics and quick-fix edits.
- Keep checks conservative: avoid noisy false positives.

## Decisions

1. Target `.allium` files only (for now).
2. `open_question` diagnostic severity is `Warning`.
3. Diagnostics are strict by default, with a relaxed mode available.
4. Packaging: one extension (`allium`) with internal basics/tools split.
5. Test scaffold output is framework-configurable via `allium.config.json` ([scaffold framework docs](scaffold-frameworks.md)).
6. Rust for the standalone parser and MCP server. Hand-written recursive descent, not tree-sitter-backed. The tree-sitter grammar remains for editor syntax highlighting.
7. Parser lives in `crates/` alongside the existing `packages/` and `extensions/` directories.

## Risks and mitigations

- Language evolution outpaces regex-based checks — mitigated by parser-backed architecture path; keep early checks minimal and well-tested.
- Scope naming mismatch in grammar themes — mitigated by TextMate scope inspector validation and fixture snapshots.
- Refactorings are unsafe without semantics — mitigated by only offering transformations with strong syntactic confidence.
- Two parsers (TypeScript + Rust) diverge — mitigated by WASM bridge path and shared test fixtures. The Rust parser targets correctness; the TypeScript analyzer can be simplified once the Rust parser is consumable.
