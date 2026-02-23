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

### Parser-backed semantic checks

The analyzer covers a broad set of checks. Remaining areas where deeper parsing would improve confidence:

- Full structural/type validation against the language reference.
- Cross-file type-flow analysis for imported entities.

### Additional editor parity

Diagram preview, rule simulation and test scaffold generation are exposed as LSP custom requests (`allium/getDiagram`, `allium/simulateRule`, `allium/generateScaffold`) so any LSP client can use them. What's missing is documentation and convenience wrappers in the Neovim and Emacs setup guides.

## Engineering approach

- TypeScript strict mode.
- Small composable analysers with unit tests.
- Golden-file fixtures for diagnostics and quick-fix edits.
- Keep checks conservative: avoid noisy false positives.

## Decisions

1. Target `.allium` files only (for now).
2. `open_question` diagnostic severity is `Warning`.
3. Diagnostics are strict by default, with a relaxed mode available.
4. Packaging: one extension (`allium`) with internal basics/tools split.
5. Test scaffold output is framework-configurable via `allium.config.json` ([scaffold framework docs](scaffold-frameworks.md)).

## Risks and mitigations

- Language evolution outpaces regex-based checks — mitigated by parser-backed architecture path; keep early checks minimal and well-tested.
- Scope naming mismatch in grammar themes — mitigated by TextMate scope inspector validation and fixture snapshots.
- Refactorings are unsafe without semantics — mitigated by only offering transformations with strong syntactic confidence.
