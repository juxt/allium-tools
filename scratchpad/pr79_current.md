This started as a testing experiment — build the *shape* of the problem, confirm it catches #76/#77/#78, and see whether it surfaces unreported bugs. It did both.

## The harness (first commit)

Two systematic sweeps in `crates/allium/tests/witness_matrix.rs`:

- **Witness matrix** — one valid `pending -> done` witness, expressed through every binding-type source (subscription, `becomes`/`transitions_to`, importer context, context+`where`, inline annotation, rule emission), rendered single-file and split. Property: single-file (a valid witness) is clean, and the split equals it. Catches #76 (both `where` variants) and #77; the five known-good sources pass, so it is not crying wolf.
- **Alias-anchoring sweep** — an undeclared-alias qualifier (`nosuch/`) at every site it can appear. Property: it is diagnosed at the reference.

The alias sweep found the reported #78 (`when:` subscription) **plus five sites nobody had reported**: `when:` transition-trigger subject, surface `context`, inline provides parameter, field type, and `.created(...)`. Each verified genuinely silent.

## The fixes — two unified passes, not eight patches

The nine failing cells were two systemic gaps, both "ad-hoc per site" instead of unified:

- **Binding-type sources (#76, #77):** `qualified_context_binding` now unwraps a `where` clause (#76); `collect_emitted_event_param_types` adds rule emissions as a third type source, alongside imported and importer surfaces (#77). The witness matrix is green.
- **Alias validation (#78 + the five):** one pass, `check_undefined_import_aliases`, walks *every* qualified reference (with spans) and flags an undeclared alias at its site — single-file and multi-file. It replaces the per-site `provides` and `default` alias checks. The alias sweep is green.

## Code consolidation (please sanity-check)

The per-site `allium.provides.undefinedImportedAlias` (shipped in 3.5.2) and `allium.default.undefinedImportedAlias` are retired in favour of one **`allium.reference.undefinedImportedAlias`** for all sites, message-anchored. This matches the "one unified resolution" shape and the convention that a code is a self-describing LLM label. If you would rather keep per-site codes, that is a small change — flagging it because it retires a two-day-old code. The trigger-existence half keeps `allium.provides.unknownTrigger`.

## Widened the sweeps — and they found one more

Broadened the generators (a `transitions_to` emission, a multi-hop lifecycle, an if/else-branch target across a module) and added five more alias sites (`requires`, `ensures` status, invariant, contract `fulfils`, value field type). The five new alias sites all pass, confirming the unified pass is genuinely comprehensive. The witness widening surfaced **one more unreported bug**: when the witnessing rule assigns the target inside an `if`/`else` branch, the reverse channel skipped it — the #58 nested-block traversal gap recurring in `collect_witnessed_transition`, which the original #58 fix hadn't reached. Fixed with the same `for_each_rule_clause` helper.

## Premortem, then found it

Did a premortem ("what will the reporter catch in 24h that we missed?"). Top prediction: we unified *alias*-existence but never *name*-existence — a valid alias with a nonexistent name (`dom/Ghost`) is only checked at provides triggers (#72) and default fields (#47), nowhere else. A third sweep (`name_existence_sweep`) confirmed it: five sites silent (`context`, field type, `.created`, transition subject, inline provides param). Fixed by widening `collect_referenced_trigger_names` to all names a module *offers* (declared types + referenced triggers) and adding a name check to the same unified pass — which subsumes #72's provides-specific `allium.provides.unknownTrigger` into `allium.reference.unknownName`.

## The second prediction did reproduce, across five passes

An earlier version of this note said the second premortem prediction, the #58 branch-traversal gap lurking in another pass, did not reproduce. That was wrong: the first probe used a mis-built case and looked clean. Rather than guess pass by pass, I audited every rule-body loop that reads `requires`, `ensures` or emissions and asked which still walked only the top level. The gap recurred in five more passes, each a genuinely silent miss confirmed by a before-and-after probe.

- Qualified creation in the reverse channel: a `dom/Job.created(...)` nested in a branch was not credited, so the split spec raised a false `unreachableValue` and `deadlock`.
- Emission parameter typing (`collect_emitted_event_param_types`): a branch-nested emission left the event's parameters untyped, which broke cross-module witness typing.
- Undefined-binding detection (`check_rule_undefined_bindings`): a branch-nested reference to an unbound name was accepted in silence, though the identical top-level reference is a hard error. The fix walks recursively and scopes branch-local `let`s, so it adds no false positive.
- Type-reference checking (`check_type_references`): an undeclared type used only in a branch went unflagged.
- Conflict and determinism analysis: the pass the first probe had wrongly cleared. A conflicting `ensures` nested in an `if`/`else` made the conflict finding vanish.

Each fix routes through the same `for_each_rule_clause` helper or an equivalent recursive walk, and ships with a standing guard: new witness-matrix cells (`created_in_branch`, `emission_in_branch`), branch-invariance property tests for undefined-binding and type-reference detection, and a conflict-in-branch unit test.

A sixth site is left unfixed by design and flagged with a code NOTE. The bare-entity malformed-trigger anchor scans only top-level clauses, so a binding referenced solely in a branch loses a secondary `undefinedBinding` anchor. This is benign: the malformed trigger still raises `invalidTrigger`, so the rule is never silently accepted. Making it branch-aware needs a span-carrying traversal, which is not worth it for a redundant diagnostic.

## A second vein: type-refinement unwrapping

The same "fixed at one site, forgotten at its siblings" shape turned up again around #76. #76 was that a `where` refinement wraps a binding's type as `Expr::Where { source }`, and the resolver matched `QualifiedName` without unwrapping. The fix was applied in one resolver (`qualified_context_binding`) and only for `where`. Two blind spots remained: other resolvers never unwrapped at all, and even the fixed one ignored the sibling refinements `with` and `?` (`TypeOptional`).

A cross-module probe confirmed the live bug: `provides: Ready(b: dom/Job where status = pending)` fails to type `b`, so the witness cannot credit `pending -> done` and the split raises a false `noExit` and `unreachableValue`. `facing b: dom/Job with …` was broken the same way.

The fix is one shared helper, `unwrap_type_refinement`, that peels `Where`, `With` and `TypeOptional`, routed through every binding-type resolver (`qualified_context_binding`, the cross-module inline-param path, and the local inline-param path). Four new witness-matrix cells guard it: inline+`where`, inline+`with`, `context`+`with`, `facing`+`with`. All four fail against the pre-fix code and pass after.

## A third vein: temporal triggers across a module boundary

Extending the split-invariance probe past the lifecycle path turned up a third gap. A transition witnessed by a temporal or relational trigger, `m: dom/E.due_at <= now  requires: m.status = X  ensures: m.status = Y`, survives fine in one file but breaks when split: the imported entity looks stuck, raising a false `deadlock`, `noExit` and `unreachableValue`. The `becomes`/`transitions_to` triggers work across a split because the reverse channel resolves them explicitly; a temporal trigger hit no handler, so the transition was never credited back.

Two fixes, both in the reverse channel:
- `qualified_temporal_trigger_entity` resolves the imported entity a temporal trigger observes and types the binding, with no implicit `from` state (the `requires` clause supplies it). A separate probe confirmed a genuinely-stuck imported entity still deadlocks, so this credits only real transitions.
- Qualified field references (`dom/E.due_at`) are now collected alias-scoped and credited, so a field whose only reference lives in the importer is not reported as unused across the split. The collector is deliberately alias-aware; a name-based version broke the alias-scoping invariant the cross-module aggregation relies on.

Guarded by a `temporal_trigger` witness-matrix cell (fails pre-fix, passes after) and two reverse-channel unit tests.

Known limitation, flagged not fixed: cross-module *conflict detection* does not fire. When two rules that conflict both live in the importer and reference the imported entity, the split no longer raises the false deadlock, but it also does not report the conflict the single-file form does. Detecting it needs the importer's conflict pass to attribute rules to imported entities via their status vocabulary, which is a larger piece of work than this PR. It is a false negative (a missed hint), not a false positive.

## Test coverage: from hand cells to generative properties

The hand-written witness cells guard specific arrangements; this pass adds generative properties that explore the space, so a regression in an untested combination surfaces on its own.

- Branch-nesting invariance: wrapping a rule's clauses in identical `if/else` to a random depth (1–3) is a semantic no-op, so the report set must be unchanged. Faults (an undefined binding, an undeclared type) are injected so it also checks that a diagnostic fires whether its clause is flat or deeply nested.
- Split-invariance, generatively: random entity and state names, a witnessing form drawn from all six binding-type sources, and a random `where`/`with` refinement, each rendered single-file and split. Extended to multiple entities, and to a multi-importer merge where a multi-hop lifecycle is witnessed one transition at a time across several consumer modules.
- A combined fuzzer over every valid dimension at once (hop count, per-transition form, branch nesting, module split).
- Declaration-order invariance: shuffling a spec's top-level declarations must not change the reports, guarding against order-dependent aggregation.
- CLI smoke tests for the `check`/`analyse`/`parse` exit-code and JSON-envelope contract.

The generative properties run clean, which is the point: the veins closed above hold across a far wider surface than the fixed cells reach. A latent test-harness bug surfaced and was fixed on the way (the shared temp directory was keyed on the process id alone, so parallel tests clobbered each other).

## True-positive detection: the split hides no fault but one

The properties above check that a valid witness stays clean across a split. The complement is that a genuine fault is not silently dropped. Using the single file as the oracle over deliberately faulty specs — witnesses omitted at random (stuck states), witnesses assigned random undeclared target edges (a tangle of undeclaredTransition / noExit / unreachable / deadlock) — the split reports exactly the same set the single file does, over hundreds of seeds. A probe battery confirmed the same for nonexistent qualified names, nonexistent fields and undeclared transition edges: each is detected identically single-file and split.

The one exception the exploration found — cross-module **conflict** detection — is now fixed here. Two importer rules that can both fire in the same state and set conflicting statuses were reported single-file but not across a split: the importer's conflict pass built its entity view from the local module only, so it never learned the imported entity's status vocabulary and could not attribute either rule to it. The fix threads imported entity status vocabularies (`collect_entity_status_schemas`) through the CLI driver's cross-module context and the parser API into the process-findings pass, where the conflict pass merges them into its lookup for that pass only — the lifecycle checks stay local, so imported entities are not re-analysed in the importer. Conflict attribution itself was already there (`resolve_binding_entity` infers the entity from the status values a rule reads and writes); it only needed the vocabulary in scope.

Guarded from both sides: the split now detects the conflict identically to the single file, and an actor-choice case (two rules on the same imported entity firing on different triggers) is confirmed not to invent one. The single-file corpus is byte-identical to before, so the plumbing adds no regression.

Full workspace suite green (the sweeps, the generative properties and the new guards), no new clippy warnings, behaviour spec updated. Everything the sweeps and the audits surfaced is fixed here rather than filed separately, as agreed.

## Code consolidation summary (for the reviewer)

Retired in favour of unified codes on the same pass:
- `allium.provides.undefinedImportedAlias`, `allium.default.undefinedImportedAlias` -> `allium.reference.undefinedImportedAlias` (undeclared alias, any site)
- `allium.provides.unknownTrigger` -> `allium.reference.unknownName` (valid alias, nonexistent name, any site)

Closes #76
Closes #77
Closes #78


