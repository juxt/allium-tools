# Rust checker parity: analysis and remaining work

The Rust CLI (`allium check`) now has a semantic analysis pass in `crates/allium-parser/src/analysis.rs`. It implements 16 diagnostic checks with `-- allium-ignore` suppression. This document describes the remaining gaps against the TypeScript reference implementation and the issues discovered during validation.

## Architecture

The TypeScript analyzer:
- `extensions/allium/src/language-tools/analyzer.ts` — drives the VS Code extension and LSP

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
# Run Rust checker against real-world specs
./target/release/allium check ~/code/achronic/specs/
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

### 6. `deferred.missingLocationHint` predicate unified

The Rust `check_deferred_location_hints` previously emitted the warning for every `deferred` declaration unconditionally — it inspected only the parsed path expression, which drops the trailing comment, so it could never see a hint. It now replays the TypeScript analyzer's line match (`^\s*deferred\s+([A-Za-z_][A-Za-z0-9_.]*)(.*)$`) over the raw source anchored at the declaration's `deferred` keyword, and applies the same predicate as `findDeferredLocationHints` to the suffix between the captured name and the end of its line.

A deferred declaration is treated as carrying a location hint when that suffix includes a quoted path, a URL (`http://`/`https://`), or the `-- see:` comment convention shown in the language reference. The TypeScript predicate was broadened from quoted-path/URL-only to also recognise `-- see:`, so the documented `deferred X -- see: path.allium` form is now accepted by both implementations.

Replaying the match — rather than trusting the parsed path's span — is what keeps the two in step: the deferred path grammar (issue #24) stops before a dangling `.` that the TypeScript capture includes (`deferred Foo.` captures `Foo.`), and a qualified `billing/InvoiceWorkflow` path extends past the flat name the capture stops at — either would move the suffix boundary or change the reported name. With the replayed match, both sides name the warning after the same flat name. Scanning the suffix rather than the whole line matters for the URL markers: a URL glued to the identifier (`deferred Foohttps://x`) is an unspaced path with no hint, and warns in both. See issue #20 and the review on PR #23.

Scope of the guarantee: parity covers every line that parses into a `DeferredDecl`. Since the deferred path grammar landed (issue #24), that is every line whose path starts with an identifier: the path parser stops at the first token it does not accept, the declaration still forms, and the leftover tokens surface as parse errors — identical on both sides, since the extension consumes the same WASM parser's diagnostics (§7). So `deferred Foo.` warns under `Foo.` on both sides plus one shared parse error, and expression-shaped lines like `deferred Foo("x")` suppress on both (the quote lands in the suffix) plus one shared parse error. A line whose path fails at the first token (`deferred (Foo)`) forms no declaration and warns on neither. The replayed match treats `\n`, `\r`, U+2028 and U+2029 as line terminators — the JavaScript `m`-flag set — so CRLF and lone-CR files behave identically on both sides. The remaining divergence: the TypeScript pattern — which runs over raw text with no comment or string awareness — can fire on `deferred`-shaped text the Rust lexer reads as comment or string content (e.g. after a lone `\r` inside a `--` comment). Diagnostic-set parity on such inputs is out of scope for this check.

### 7. Parse diagnostics surfaced (issue #25)

The TypeScript front end discarded the WASM parser's `result.diagnostics`:
`wasmBlocksToParsedBlocks` read only `result.module.declarations`, so malformed
input produced zero parse errors in the extension, the LSP server, and
`check.js`, while `allium check` reported them (e.g. `deferred Foo.` is a parse
error in Rust but was silent in TypeScript).

The parse result now flows through a new `parseAlliumDocument` (in
`extensions/allium/src/language-tools/parser.ts`), and `analyzeAllium` maps each
parse diagnostic into a finding (`allium.parse.error` / `allium.parse.warning`),
matching how `allium check` chains `result.diagnostics` ahead of the analysis
diagnostics. Because all three consumers (`check.js`, the LSP server, and — via
the language client — the VS Code extension) run through `analyzeAllium`, the
fix reaches every surface. Well-formed specs are unaffected: the parser only
emits diagnostics for genuine syntax errors and for files missing the
`-- allium: N` version marker (both of which the Rust CLI also reports).

This closes the "Rust errors, TypeScript silent" direction of the malformed-input
divergence described above. The complementary direction — the regex lanes warning
on `deferred`-shaped text that the Rust front end reads as comment/string content
— is addressed by #28 (see below).

While surfacing parse errors, a latent bug in the temporal-guard autofix was
exposed and fixed: the scaffold emitted `requires: /* add temporal guard */`
(a C-style comment, invalid in Allium) and the `check.js`/`fix-all.ts` paths
inserted it *before* the `when:` clause (invalid clause ordering). It now emits
`requires: TODO() -- add temporal guard` after the `when:` line. Previously these
produced parse errors that nothing surfaced.

### 8. Regex lanes made comment/string aware (issue #28)

The TypeScript lanes in `analyzer.ts` run regexes over raw source text with no
lexer context, so they matched keyword-shaped text inside comments and string
literals that the Rust front end reads as content, not code — e.g. a
`deferred`-shaped token inside a `--` comment produced a spurious
`allium.deferred.missingLocationHint` that `allium check` never emits. This is
the "Rust silent, TypeScript false-positive" direction left open by #25.

`analyzeAllium` now computes a **masked view** of the source via
`maskCommentsAndStrings`, which blanks comment and string/backtick *content* to
spaces while preserving length, offsets, and line breaks. The masker mirrors the
Rust lexer: line comments run from `--` to the next `\n` (a lone `\r` does not
end them), strings honour `\` escapes and terminate at `"`/`\n`, and backtick
literals terminate at `` ` ``/`\n`/`\r`. Block bodies are re-sliced from the
masked text, so every body-based lane inherits the awareness, and the detection
lanes receive the masked text in place of raw text.

Two consumers deliberately keep the **raw** text because they read comment/string
content on purpose: `findDeferredLocationHints` (the `-- see:` / quoted-path /
URL hint — it now detects the `deferred` keyword on the masked text but reads the
hint suffix from raw text) and `applySuppressions` (the `-- allium-ignore`
directive). Delimiters (`"`, `` ` ``) and the `--` of a comment are preserved by
the mask, so lanes that only need to detect that a string or comment is present
(e.g. the type-mismatch operand lanes) still see one.

The status-lifecycle checker (`allium.status.unreachableValue` /
`allium.status.noExit`) was refined in both implementations for issue #18:

- Rule `when:` bindings take their entity type from the surface `provides:`
  declaration of the command they subscribe to, matched positionally
  (`Cancel(admin, sub: Subscription)` types `sub` as `Subscription`). Without
  this, a status value shared by several entities (e.g. two entities both
  declaring `active`) made the binding unresolvable and silently dropped the
  rule's assignments and transitions. Rust:
  `collect_command_param_types`/`augment_binding_types_from_commands`;
  TypeScript: `collectCommandParamTypes`/`augmentBindingTypesFromCommands`.
- A negated `requires: x.status != v` is expanded to the complement of the
  entity's status enum, so every other value gains an exit edge through that
  rule. Previously only `=` comparisons produced exit edges.

One refinement landed as a follow-up regression fix: lanes that compare
string-literal **values** textually (`findNeverFireRuleIssues`,
`findSurfaceImpossibleWhenIssues`) cannot compare masked literals, because
masking collapses distinct same-length literals to identical spaces (`"a"` and
`"b"` both become `" "`) — producing a spurious `rule.neverFires` on satisfiable
requires pairs and missing genuine contradictions. These lanes still *match* on
masked text but re-read string operands from the raw source via
`rawStringOperand`, exploiting the mask's length/offset preservation.

The unreachable-trigger checker (`allium.rule.unreachableTrigger` /
`allium.rule.invalidTrigger`) was refined in both implementations for
issue #19:

- A qualified trigger call (`when: alias/Trigger(...)`) is a valid trigger
  form — the language reference's "Responding to external triggers" section
  documents it. Rust previously rejected it with `rule.invalidTrigger`;
  TypeScript already accepted it.
- Qualified subscriptions are resolved cross-module where possible. The Rust
  CLI builds a per-file alias → provided/emitted-trigger map
  (`collect_trigger_outputs` + `CrossModuleContext.imported_triggers`) and
  flags `alias/Trigger` only when the aliased module is in the check set and
  determinately lacks the trigger; aliases pointing outside the check set are
  never flagged. Unqualified triggers emitted by an imported module also count
  as reachable. The single-file TypeScript analyzer cannot see other modules,
  so it skips qualified subscriptions entirely (a call name preceded by `/`).
- A trigger emitted as the leading call of an `if`/`else if`/`else` branch or
  a `for` iteration inside an `ensures:` value counts as an emission. Rust:
  `collect_leading_ensures_call` recurses into `Conditional` and `For`;
  TypeScript: a branch-call lane scoped to ensures clause extents
  (`ensuresClauseRegions`). Both still collect only the leading call of each
  branch body, consistent with the leading-call-only convention above.

A further refinement landed in both implementations for issue #42: a typed or
named parameter in an external-stimulus / chained trigger call (e.g.
`when: AccountSeen(account: Account)`) is now reported as `rule.invalidTrigger`
on the offending parameter, with a message explaining that such params are bare
names (`?`/`_` permitted) and the `name: value` form is only valid in trigger
emissions. Previously both checkers accepted the malformed call shape and then
emitted a misleading `rule.undefinedBinding` for every body reference to the
param. The name is now also added to the rule's binding scope
(Rust: `collect_bound_names`; TypeScript: the `whenCallPattern` loop) so the
malformed trigger produces exactly one diagnostic, at the trigger, rather than
double-firing on the body.

## Diagnostic codes implemented

| Code | Severity | Rust | TypeScript |
|---|---|---|---|
| `allium.parse.error` | error | Yes (no code) | Yes |
| `allium.parse.warning` | warning | Yes (no code) | Yes |
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
| `allium.list.mixedElementTypes` | error | Yes | Yes |
| `allium.list.emptyListNoElementType` | error | Yes | Yes |
| `allium.default.unknownField` | error | Yes | Yes |
| `allium.surface.unusedPath` | info | Disabled | Yes |

Two language features were added for juxt/allium#43:

- **`[...]` list literals** producing `List<T>`, valid in any expression
  position. Parsing lives in the Rust front end (`parse_list_literal`,
  `Expr::ListLiteral`) and so reaches both implementations through the WASM
  parser; the previous "list literals are not supported" rejection was removed.
  `allium.list.mixedElementTypes` flags a list whose elements are of differing
  literal types (e.g. `["a", 5]`); elements whose type is not determinable
  without a type system (identifiers, calls) are skipped, so there are no false
  positives. The Rust check (`check_list_literal_homogeneity`) and the
  TypeScript check (`findListLiteralHomogeneityIssues`, which walks the
  front-end AST) emit byte-identical messages.
- **Qualified type names in `default` declarations** (`default alias/Type x = ...`).
  Parsing is in the Rust front end (`DefaultDecl.type_alias`) and reaches both
  implementations via WASM; the qualified name resolves cross-module, like
  qualified triggers and config references. Both implementations validate that
  the alias is a known `use` import and emit `allium.default.undefinedImportedAlias`
  otherwise (Rust: `check_qualified_default_aliases`; TypeScript:
  `findDefaultTypeReferenceIssues`). Default literals are validated against the
  declared schema of **local** entity/value types (`check_default_field_schemas`
  / `findDefaultFieldSchemaIssues`): a field the type does not declare is
  reported as `allium.default.unknownField` (recursing into nested object
  literals), and an empty list literal whose target field is not a `List<T>` is
  reported as `allium.list.emptyListNoElementType` (language-reference rule 14c).
  Qualified (imported) default types are **not** validated this way — their field
  schema is not visible to a single-module pass; cross-module drift detection
  would require plumbing imported entity field sets through `CrossModuleContext`
  (as is done for triggers) and would be CLI-multi-file-only, since the
  single-file TypeScript analyzer cannot see other modules.

`allium.surface.requiresWithoutDeferred` is TypeScript-only (no Rust equivalent yet). When porting it, note the deferred-name matching semantics fixed in issue #26: a named requires block matches a deferred declaration by its full name, by a trailing `.`-separated segment, or — for module-qualified declarations like `deferred billing/InvoiceWorkflow` — by the unqualified name after the `alias/` prefix. The alias alone must not satisfy the match.

## Suppression system

Both implementations support `-- allium-ignore code1, code2` comments. The directive suppresses diagnostics on the same line or the next line. The Rust implementation uses `regex-lite` for parsing; the suppression regex must not span blank lines (use `[^\S\n]*` not `\s*` at the start).

## Build and test

```bash
cargo build --release          # Build Rust CLI
cargo test                     # Run Rust tests
npm run build                  # Build TypeScript
npm run test                   # Run TypeScript tests
```
