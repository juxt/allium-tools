Unifies qualified-reference resolution across the analyser, closing #76, #77 and #78 along with several related gaps in the same area. The common thread is consistency: a check or resolution that applied at one site is made to apply at all of its siblings, single-file and across a `use` edge alike.

## Qualified-reference resolution

One pass walks every qualified reference `alias/Name` and validates it at the reference site, single-file and multi-file:

- An undeclared alias is flagged wherever it appears: `when` triggers, transition subjects, surface `context`, inline parameters, field types, `.created(...)`, provides entries, defaults and contract clauses. Previously only provides and default sites were checked.
- With a valid alias, a name the target module does not offer is flagged the same way.

This replaces the per-site `provides` and `default` alias checks with a single resolution.

## Binding-type resolution

A witnessing rule's binding is typed from several sources, each now resolved consistently:

- `where`, `with` and `?` refinements on a binding's type are unwrapped by one shared helper in every resolver, so `Ready(b: dom/Job where status = pending)` and `facing b: dom/Job with …` type `b` exactly as the bare form does.
- Rule emissions count as a binding-type source alongside imported and importer surfaces.
- Temporal and relational triggers (`m: dom/E.due_at <= now`) resolve across a module boundary, and the qualified field references they read are credited so the field is not reported unused.

## Branch traversal

Several passes walked only the top level of a rule body, silently skipping clauses nested in `if`/`else` or `for`. All now descend through a shared helper: qualified creation and witnessed transitions in the reverse channel, emission parameter typing, undefined-binding detection, type-reference checking, and conflict and determinism effect analysis. Undefined-binding detection scopes branch-local `let`s, so it adds no false positive.

## Cross-module conflict detection

Two rules in an importer that can both fire in the same state of an imported entity and set conflicting statuses are now reported, matching the single-file behaviour. The importer's conflict pass is given the imported entity's status vocabulary (threaded through the existing cross-module context), so it can attribute both rules to the entity; the lifecycle checks stay local to each module. An actor's choice, where the rules fire on different triggers, is still not a conflict.

## Diagnostic-code consolidation

Retired in favour of one code per behaviour, message-anchored at the reference:

- `allium.provides.undefinedImportedAlias` and `allium.default.undefinedImportedAlias` become `allium.reference.undefinedImportedAlias` (undeclared alias, any site).
- `allium.provides.unknownTrigger` becomes `allium.reference.unknownName` (valid alias, nonexistent name, any site).

`allium.provides.undefinedImportedAlias` shipped in 3.5.2, so flagging its retirement in case you would rather keep per-site codes.

## Tests

- A metamorphic split-invariance harness: a valid witness is clean in one file and reports identically when split across a `use` edge. Generative variants cover random names, every binding-type source, refinements, multiple entities, multi-importer merges, deep branch nesting and a combined fuzzer.
- The same single-file-as-oracle applied to deliberately faulty specs, confirming a genuine fault survives the split rather than being silently dropped.
- Branch-nesting, declaration-order and refinement invariance properties.
- CLI smoke tests for the `check`, `analyse` and `parse` exit-code and JSON-envelope contract.

Full workspace suite green, no new clippy warnings, behaviour specs updated.

Closes #76
Closes #77
Closes #78
