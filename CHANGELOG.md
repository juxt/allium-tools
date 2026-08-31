# Changelog

Behaviour changes worth knowing before you upgrade. Releases ship from a single version tag, see `docs/releasing.md`.

## Unreleased

Surfaces are now offered to importers. A `related:` entry naming an imported module's surface (`related: alias/Wizard(expr)`) is the documented construct for a cross-module navigation link, but it drew a false `allium.reference.unknownName` because the offered-names set never contained surfaces. It now resolves; a surface name the imported module never declares still warns. The membership check remains a name-existence check, not a kind check, so any qualified reference to a declared surface name stops warning `unknownName` — including positions where a surface makes no sense, such as a `when:` subscription, which previously warned on the name and is now silent there. The same applies to a `default` literal typed to an imported surface (`default alias/Wizard f = { ... }`): the name warning was that construct's only diagnostic, and nothing replaces it — surfaces carry no field schema, so unlike an entity-typed default the literal's fields go unvalidated (deferred-root-typed defaults already behave this way). This change only removes warnings; it cannot turn a previously green gate red (#98).

## 3.6.1

Cross-module reference checking, new in 3.5.3, had three gaps. Closing them changes what `allium check` reports, and two of the changes can turn a previously green gate red on upgrade.

Dead local imports are now caught. A reference through a `use` path that resolves neither in the check set nor on disk draws `allium.reference.unresolvedImport` (warning), independent of the `allium.use.unresolvedPath` warning on the use line that a per-line `allium-ignore` can suppress. A spec whose imported file was renamed or deleted, and which previously checked clean, now fails the gate. That is the point of the fix: the old behaviour let one suppressed use-line warning silently disable name checking through the whole alias (#87).

Ambiguity that was always there is now visible. Every emission in an `ensures:` block exports to importers, not just the first statement, and `otherwise:` emissions export too. This removes false `unknownName` and `unreachableTrigger` findings. Where two imported modules both emit the same trigger and one did so after the first statement, the checker can now see the full picture and raise an `allium.use.ambiguousReference` (warning) it previously missed (#88).

Fewer false positives on config and deferred references. `alias/config.param` and `alias/DeferredName` now resolve, so the documented config-reference form no longer warns `unknownName`. Field-level checking of `alias/config.field` is not yet implemented, so a mistyped field name is currently silent rather than flagged (#89).

Import resolution now consults the filesystem, so `allium check` output can differ by platform. A wrong-case local path counts as present on a case-insensitive filesystem (macOS, Windows) and broken on a case-sensitive one (Linux CI). The divergence runs in the safe direction: CI is the stricter of the two.
