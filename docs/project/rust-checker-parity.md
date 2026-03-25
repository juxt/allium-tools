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

## Current state (commit 559ef01)

128 diagnostics match exactly. 6 are Rust-only (4 correct, 2 false positive). 33 are TypeScript-only (27 legitimate misses, 6 TypeScript false positives).

## Rust false positives to fix (2)

### 1. `rule.undefinedBinding` on clerk.allium:105 and ledger.allium:154

These are `let` bindings inside `for` blocks that reference variables from the `for` binding's `where` clause or prior `let` bindings. The `check_unbound_roots` function doesn't add `for`-block bindings to the scope before checking nested expressions.

**Fix:** In `check_rule_undefined_bindings`, when descending into `ForBlock` items, add the for-binding variable(s) to the `bound` set before checking nested items. Currently the code does this but may not handle all expression shapes within `for` blocks (e.g., `where` clause filter variables, nested `let` bindings inside `for` bodies).

## TypeScript false positives the Rust correctly avoids (6)

### 1. `field.unused` on `terminal` inside `transitions` blocks (5 instances)

The TypeScript regex-based field collector treats `terminal: acknowledged` inside a `transitions status { ... }` block as a field declaration. Per the language reference, `terminal:` is a keyword clause in the transition graph syntax, not a field. The Rust AST represents these as `TransitionsBlock` items, correctly excluding them.

Files: arbiter.allium:50, clerk.allium:57, registrar.allium:54, core.allium:180, core.allium:227

### 2. `field.unused` on value type fields

The TypeScript checker flags fields on value types (`value TimeRange { start: Timestamp }`) as unused when they're not referenced by `.field` access elsewhere. Value types are structural declarations whose fields are part of the type contract. The Rust checker correctly skips `BlockKind::Value` in unused-field checks.

## Legitimate Rust misses to implement (27)

### 1. `rule.unreachableTrigger` — 20 missing

**Root cause:** The Rust emitted-trigger collector (`collect_call_names`) doesn't trace deeply enough into ensures blocks. Specifically:

- Triggers emitted inside `for` blocks within ensures clauses (e.g., `for record in records: ... ClerkFederatesRecord(record)`)
- Triggers emitted inside `if` blocks within ensures clauses
- Triggers emitted as standalone expressions in multi-line ensures blocks (e.g., `ensures:\n    SomeCall()\n    AnotherCall()`)

The TypeScript version collects trigger names using a regex that finds all `PascalCaseName(...)` patterns in rule body text, which naturally captures all nesting depths.

**Fix:** The `collect_call_names` expression walker needs to recurse into more expression types. Currently it handles `Call`, `Block`, `WhenGuard`, and `Conditional`. It should also handle `For`, `Lambda`, and any other expression that can contain nested calls. Additionally, the block-item-level collection from `ForBlock` and `IfBlock` items in the ensures context needs to recurse more deeply.

**Test:** After fixing, the Rust count for `unreachableTrigger` on achronic specs should match or exceed the TypeScript count of 43. Some TypeScript detections may be at slightly different line numbers due to different span reporting.

### 2. `let.duplicateBinding` — 3 missing

**Root cause:** The `check_duplicate_lets_in_items` function correctly descends into `ForBlock` and `IfBlock` items, but the duplicate `let` bindings in the achronic specs are `let` expressions inside `ensures:` clause **expressions** (via `Expr::LetExpr`), not `BlockItemKind::Let` items.

Example from ledger.allium:
```allium
ensures:
    for entity in entities:
        let shard = ShardFor(entity.key)
        let entry = shard.shard_cache.l1_entries.any(...)
```

The parser may represent these as `Expr::LetExpr` inside an `Expr::Block` inside the ensures clause value, not as `BlockItemKind::Let` items.

**Fix:** Walk ensures clause expressions for `Expr::LetExpr` nodes and track their names alongside `BlockItemKind::Let`. The function `check_duplicate_lets_in_items` should also call a companion `check_duplicate_lets_in_expr` that recurses through expression trees looking for `LetExpr`.

### 3. `rule.undefinedBinding` — 3 missing

These fire on rules with invalid triggers (bare entity binding like `when: state: ClerkEventState`). The TypeScript reports both `invalidTrigger` AND `undefinedBinding` because the bare binding doesn't produce usable trigger parameters. The Rust checker already reports `invalidTrigger` for these rules but doesn't also check binding resolution.

**Fix:** When a rule has an invalid trigger form, the binding from the `when` clause may not resolve to a meaningful type. The `check_rule_undefined_bindings` function should still attempt to resolve bindings even when the trigger form is unusual. Specifically, for `Expr::Binding { name, value: Expr::Ident(...) }` (bare entity reference), the binding name should be added to the bound set but subsequent field accesses on it may still flag if the binding type can't be resolved.

### 4. `config.undefinedReference` — 1 missing

**Root cause:** registrar.allium:291 references `config.l2_cache_capacity` inside an `invariant` block expression. The `check_config_undefined_references` function was updated to walk `Decl::Invariant` expressions but may not be matching because the invariant references `config.l2_cache_capacity` in a nested `for` expression.

**Fix:** Verify the `check_config_refs_in_expr` function recurses into `Expr::For` bodies. It currently handles `Block`, `Conditional`, and operator expressions but may be missing `For`.

## Checks where Rust is more correct (4)

The Rust checker finds 4 `type.undefinedReference` errors that the TypeScript misses:
- arbiter.allium:44, 45 — types inside `when`-qualified field declarations
- registrar.allium:44 — same pattern
- warden.allium:45 — same pattern

The TypeScript regex for field type checking uses `/^\s*name\s*:\s*Type\s*$/m` which doesn't match fields with `when` clauses appended. The Rust AST handles `FieldWithWhen` items correctly and checks their type expressions.

Per language reference rule 1: "All referenced entities and values exist." This applies regardless of `when` clauses on the field.

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
| `allium.rule.unreachableTrigger` | info | Partial | Yes |
| `allium.field.unused` | info | Yes | Yes (with false positives) |
| `allium.entity.unused` | warning | Yes | Yes |
| `allium.definition.unused` | warning | Yes | Yes |
| `allium.deferred.missingLocationHint` | warning | Yes | Yes |
| `allium.rule.invalidTrigger` | error | Yes | Yes |
| `allium.rule.undefinedBinding` | error | Partial | Yes |
| `allium.let.duplicateBinding` | error | Partial | Yes |
| `allium.config.undefinedReference` | error | Partial | Yes |
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
