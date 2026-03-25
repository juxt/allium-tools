# Rust checker parity: analysis and remaining work

The Rust CLI (`allium check`) now has a semantic analysis pass in `crates/allium-parser/src/analysis.rs`. It implements 16 diagnostic checks with `-- allium-ignore` suppression. This document describes the remaining gaps against the TypeScript reference implementation and the issues discovered during validation.

## Architecture

The TypeScript analyzer (two copies, kept in sync):
- `extensions/allium/src/language-tools/analyzer.ts` — drives the VS Code extension
- `packages/allium-cli/src/analyzer.ts` — drives the Node CLI (`allium-check`)

The Rust analyzer:
- `crates/allium-parser/src/analysis.rs` — drives the Rust CLI (`allium check`)

The TypeScript version uses regex over raw source text. The Rust version walks the typed AST produced by the parser. The Rust approach is structurally more reliable but needs to handle all AST node shapes the parser produces.

The language reference at `docs/allium-v3-language-reference.md` is the definitive source for language semantics. When the two implementations disagree, consult it.

## Validation test beds

Two real-world spec repos are used for validation:
- `~/code/rorschach/specs/blotter.allium` — trade blotter (1 file)
- `~/code/achronic/specs/` — event sourcing framework (10 files)

Run both checkers and diff:

```bash
# Normalised comparison
(./target/release/allium check ~/code/achronic/specs/ 2>&1 || true) \
  | sed 's|/Users/.*/achronic/||' | grep 'allium\.' \
  | sed -E 's/^([^:]+:[0-9]+):[0-9]+: (error|warning|info) (allium\.[^ ]+).*/\1 \2 \3/' \
  | sort > /tmp/rust.txt

(node packages/allium-cli/dist/src/check.js --no-config ~/code/achronic/specs/ 2>&1 || true) \
  | sed 's|.*/achronic/||' | grep 'allium\.' \
  | sed -E 's/^([^:]+:[0-9]+):[0-9]+ (error|warning|info) (allium\.[^ ]+).*/\1 \2 \3/' \
  | sort > /tmp/ts.txt

comm -23 /tmp/ts.txt /tmp/rust.txt  # TypeScript only
comm -13 /tmp/ts.txt /tmp/rust.txt  # Rust only
```

## Current state

155 diagnostics match exactly. 4 are Rust-only (all correct). 0 are TypeScript-only.

## TypeScript false positives fixed

### `field.unused` on `terminal` inside `transitions` blocks (6 instances)

The TypeScript regex-based field collector was treating `terminal: resolved` inside `transitions status { ... }` blocks as field declarations. Per the language reference, `terminal:` is a keyword clause in the transition graph syntax, not a field. Fixed by excluding `transitions { ... }` sub-block ranges from field scanning in `collectDeclaredEntityFields`.

Files: arbiter.allium:50, clerk.allium:57, registrar.allium:54, core.allium:180, core.allium:227, warden.allium:49

## Checks where Rust is more correct (4)

The Rust checker finds 4 `type.undefinedReference` errors that the TypeScript misses:
- arbiter.allium:44, 45 — types inside `when`-qualified field declarations
- registrar.allium:44 — same pattern
- warden.allium:45 — same pattern

The TypeScript regex for field type checking uses `/^\s*name\s*:\s*Type\s*$/m` which doesn't match fields with `when` clauses appended. The Rust AST handles `FieldWithWhen` items correctly and checks their type expressions.

Per language reference rule 1: "All referenced entities and values exist." This applies regardless of `when` clauses on the field.

## Fixes applied

### 1. `rule.undefinedBinding` false positives fixed

`check_unbound_roots` now accumulates `LetExpr` bindings when walking `Block` expressions, so variables defined by `let` in ensures blocks are in scope for subsequent expressions. Also added `Expr::For` handling (with binding scope) and `Expr::BinaryOp` recursion.

### 2. `rule.unreachableTrigger` — 20 now matching

Replaced the deep expression walker (`collect_call_names`) with `collect_leading_ensures_call` for the emitted trigger set. The new function only collects the first `Call` expression in each ensures clause value, matching the TS regex which captures only the first identifier after `ensures:`. Also restricted collection to `ensures` clauses only (not requires/when).

### 3. `let.duplicateBinding` — 3 now matching

Added `check_duplicate_lets_in_expr` to walk ensures clause expressions for `Expr::LetExpr` nodes, tracking duplicate names alongside `BlockItemKind::Let` items.

### 4. `rule.undefinedBinding` — 3 now matching

Added a targeted check for rules with bare entity bindings (e.g. `when: state: ClerkEventState`). These are invalid trigger forms where the binding doesn't resolve to a meaningful type. The checker now emits one `undefinedBinding` for the first requires/ensures clause referencing the binding.

### 5. `config.undefinedReference` — 1 now matching

Removed early return when no config block exists (the TS checks for `config.xxx` references regardless). Added `Expr::For`, `Expr::LetExpr`, and `Expr::Lambda` handling to `check_config_refs_in_expr`. Changed severity from error to warning to match TS.

## Diagnostic codes implemented

| Code | Severity | Rust | TypeScript |
|---|---|---|---|
| `allium.surface.relatedUndefined` | error | Yes | Yes |
| `allium.sum.v1InlineEnum` | error | Yes | Yes |
| `allium.sum.discriminatorUnknownVariant` | error | Yes | Yes |
| `allium.sum.invalidDiscriminator` | error | Yes | Yes |
| `allium.surface.unusedBinding` | warning | Yes | Yes |
| `allium.status.unreachableValue` | warning | Yes | Yes |
| `allium.status.noExit` | warning | Yes | Yes |
| `allium.externalEntity.missingSourceHint` | warning/info | Yes | Yes |
| `allium.type.undefinedReference` | error | Yes | Yes |
| `allium.rule.undefinedTypeReference` | error | Yes | Yes |
| `allium.rule.unreachableTrigger` | info | Yes | Yes |
| `allium.field.unused` | info | Yes | Yes (with false positives) |
| `allium.entity.unused` | warning | Yes | Yes |
| `allium.definition.unused` | warning | Yes | Yes |
| `allium.deferred.missingLocationHint` | warning | Yes | Yes |
| `allium.rule.invalidTrigger` | error | Yes | Yes |
| `allium.rule.undefinedBinding` | error | Yes | Yes |
| `allium.let.duplicateBinding` | error | Yes | Yes |
| `allium.config.undefinedReference` | warning | Yes | Yes |
| `allium.surface.unusedPath` | info | Disabled | Yes |

## Suppression system

Both implementations support `-- allium-ignore code1, code2` comments. The directive suppresses diagnostics on the same line or the next line. The Rust implementation uses `regex-lite` for parsing; the suppression regex must not span blank lines (use `[^\S\n]*` not `\s*` at the start).

## Build and test

```bash
cargo build --release          # Build Rust CLI
cargo test                     # Run Rust tests (286 in parser, 140 in CLI)
npm run build                  # Build TypeScript
npm run test                   # Run TypeScript tests (284 extension, 19 CLI, 8 LSP)
```
