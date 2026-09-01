# Changelog

Behaviour changes worth knowing before you upgrade. Releases ship from a single version tag, see `docs/releasing.md`.

## Unreleased

Member access is no longer mistaken for an import alias. The reference checker rereads `ident.UppercaseMember` member access as the legacy dotted import form `alias.TypeName`, and since 3.5.3 it errored `allium.reference.undefinedImportedAlias` on every such access whose identifier is not a `use` alias — so `properties.Status.status.name`, reading a capitalised key out of a `properties: Map<String, Any>` field, reddened the gate in a module with zero imports, naming an alias the author never wrote. The dotted reread now stands down when the identifier is a value name the module binds anywhere: a field or `name:` item of any block (entity, external entity, value, `given`, config, surface), a `variant` field, a `default` declaration's name, a `let`, a rule trigger parameter, a surface `context` binding, a lambda parameter or `for` binding — top-level `invariant` bodies included. A dotted qualifier bound nowhere still errors (the real legacy form with a missing `use`), a dotted qualifier that is a declared alias still gets its name checked against what the module offers, and the slash form `alias/Name` is unaffected. A `deferred` declaration's dotted path names a code location, not a value, so it is never exempt: a dangling legacy deferred reference keeps its error even when the module binds a value name spelled the same. This change only removes diagnostics; no previously green gate turns red (#97).

## 3.6.1

Cross-module reference checking, new in 3.5.3, had three gaps. Closing them changes what `allium check` reports, and two of the changes can turn a previously green gate red on upgrade.

Dead local imports are now caught. A reference through a `use` path that resolves neither in the check set nor on disk draws `allium.reference.unresolvedImport` (warning), independent of the `allium.use.unresolvedPath` warning on the use line that a per-line `allium-ignore` can suppress. A spec whose imported file was renamed or deleted, and which previously checked clean, now fails the gate. That is the point of the fix: the old behaviour let one suppressed use-line warning silently disable name checking through the whole alias (#87).

Ambiguity that was always there is now visible. Every emission in an `ensures:` block exports to importers, not just the first statement, and `otherwise:` emissions export too. This removes false `unknownName` and `unreachableTrigger` findings. Where two imported modules both emit the same trigger and one did so after the first statement, the checker can now see the full picture and raise an `allium.use.ambiguousReference` (warning) it previously missed (#88).

Fewer false positives on config and deferred references. `alias/config.param` and `alias/DeferredName` now resolve, so the documented config-reference form no longer warns `unknownName`. Field-level checking of `alias/config.field` is not yet implemented, so a mistyped field name is currently silent rather than flagged (#89).

Import resolution now consults the filesystem, so `allium check` output can differ by platform. A wrong-case local path counts as present on a case-insensitive filesystem (macOS, Windows) and broken on a case-sensitive one (Linux CI). The divergence runs in the safe direction: CI is the stricter of the two.
