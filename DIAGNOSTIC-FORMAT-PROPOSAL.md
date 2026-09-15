# CLI diagnostic format — proposal for an LLM consumer

The primary caller of `allium check`/`analyse` is an LLM that will read the JSON and then EDIT the
spec. The format should be optimised for that act, not for a human scanning an editor gutter. Three
problems with the current output, and a proposed schema.

> **Empirical update (diag-format eval, allium-trials).** We tested whether format changes fix
> SUCCESS: 3 seeded defects, real diagnostics rendered raw vs located vs actionable, a tool-using
> fixer, checker re-run as oracle, Opus + Sonnet. Result: **100% fix-success across all three formats,
> both models** — a null. On a normal-size spec a capable fixer opens the file and fixes the defect
> regardless of how the diagnostic is shaped. So do NOT justify this redesign by "the LLM fixes it
> better". The justifications that survive: (1) **programmatic consumption** — a stable `code` and one
> consistent schema let skills/prompts branch deterministically; (2) **correctness/consistency bugs**
> found along the way (see below); (3) plausibly **efficiency at scale** (a self-contained diagnostic
> avoids a read), which the eval did not test. Treat the schema below as serving (1) and (2).

## What's wrong now

1. **Byte spans are near-useless to an LLM.** `"span": {"start": 13, "end": 202}` forces the model
   to re-derive text from offsets it doesn't hold indexed. An LLM anchors on NAMES and STRUCTURE
   ("component `Account`", "invariant `cap`") and on the SOURCE TEXT itself, not on offsets.
2. **Two inconsistent shapes.** `check` emits `{code, severity:"warning", location:{file,line,col}}`;
   v4 analyse findings emit `{message, severity:"Warning", span:{start,end}}` (raw serde — note the
   capitalised severity and byte span). Same tool, two schemas, one of them the poor one.
3. **The message does two jobs in one run-on sentence, in an essay voice** (em-dashes, rhetorical
   asides). "…never what it must achieve — it is satisfiable by an implementation that does nothing.
   State an `objective`…" mixes *what's wrong* with *how to fix* and reads like generated prose.

## What is most actionable for an LLM — the design principles

- **Name the construct.** kind + name (`component Account`, `invariant cap`) is the anchor a model
  edits against. This is the single biggest win over a span.
- **Carry the source excerpt.** Return the verbatim text of the flagged construct so the diagnostic
  is self-contained — the model acts on the JSON without a second read (fewer round-trips).
- **Separate WHAT from HOW.** `problem` (one plain sentence: what is wrong) and `fix` (one
  imperative sentence: what to do). This is more actionable AND fixes the voice problem.
- **A stable, documented `code`.** So skills/prompts can branch deterministically on a known
  diagnostic, and so each code gets a doc page. (`check` has this; findings don't.)
- **Structured `detail` per code.** The code-specific payload a model can act on without parsing
  prose — the conflicting core, the offending edge, the missing field. Generalise what
  `dead_transition`/`missing_producer` already do.
- **Say whether it blocks.** `blocking: true/false` — severity alone doesn't tell the model whether
  the gate fails.
- **Keep line/col/file** as secondary (cheap, good for humans and round-trip); DROP raw byte offsets
  from the headline (optionally retain under `span` for editor integrations).

## Proposed schema (one shape for every diagnostic AND finding)

```json
{
  "code": "vacuous-component",
  "severity": "warning",
  "blocking": false,
  "target": {
    "kind": "component",
    "name": "Account",
    "file": "account.allium",
    "line": 2,
    "excerpt": "component Account\n  observable state balance : Money\n  invariant non_negative means balance >= 0"
  },
  "problem": "Account has invariants and faults but no objective, budget or requirement. It constrains what must not happen and requires nothing to happen, so an implementation that does nothing satisfies it.",
  "fix": "Add an objective stating what Account must achieve and by when, e.g. `objective settled within eod`.",
  "detail": { "has": ["invariant", "fault", "action"], "missing_any_of": ["objective", "budget", "requirement"] }
}
```

Before → after for the pasted example: the byte span becomes `target` (name + line + excerpt); the
run-on message splits into `problem` + `fix`; a stable `code` appears; severity lowercases.

Run-level envelope gains a summary so a model can branch before scanning:

```json
{ "command": "analyse", "summary": {"errors": 0, "warnings": 1, "blocking": 0}, "diagnostics": [ … ] }
```

## Message drafting rules (apply to every `problem`/`fix` string)

- `problem`: one sentence, plain, declarative. Name the construct. No em-dashes, no rhetorical
  asides, no "it is satisfiable by…" essay register. British plain style.
- `fix`: one sentence, imperative, concrete, with a minimal syntax example where it helps.
- No dashes as connectors; use full stops. Say it once.

## Scope of change (if greenlit)

1. Unify serialization: one `finding_to_json` producing the schema above; route both v4 analyse
   findings and parser findings through it. Kill the raw-serde path (fixes the "Warning"/byte-span
   inconsistency).
2. Give every finding a `code` and split its message into `problem`/`fix`. Start with the ones the
   evals exercise (vacuous-component, contradictory-invariants, dead-transition, missing-producer,
   deadlock).
3. Add `target.excerpt` from the span via the existing `SourceMap` (cap ~8 lines / 400 chars, with a
   `truncated` flag).
4. Rewrite the message strings to the drafting rules; snapshot-test them so the voice can't drift.

## Open questions for the human

- Excerpt cap: whole construct, or header line + line-range for large components?
- Keep raw `span:{start,end}` at all (editor use), or drop entirely?
- `suggested_edit` (machine-applicable patch) for high-confidence codes — worth it, or is
  imperative `fix` prose enough for an LLM that will edit anyway?

## Implemented (v4 branch)

Structural unification, real locations, dedup, and em-dash removal are done and test-locked
(`crates/allium/tests/cli_smoke.rs`). Stable `code` slugs assigned so far (others still emit
`code: null`, a consistent shape, pending an incremental sweep):

| code | command | has structural `fix` |
|---|---|---|
| `undeclared-name` | check | yes |
| `vacuous-component` | analyse | yes |
| `contradictory-invariants` | analyse | yes |
| `contradictory-rules` | analyse | yes |
| `nonlinear-not-checked` | analyse | (remedy in message) |
| `invariant-not-entailed` | analyse | (remedy in message) |
| `objective-unbounded` | analyse | (remedy in message) |
| `action-never-enabled` | analyse | (remedy in message) |
| `enum-value-unreachable` | analyse | (remedy in message) |
| `stuck-state` | analyse | (remedy in message) |
| `contract-not-satisfied` | analyse | (remedy in message) |
| `case-split-not-disjoint` | analyse | (remedy in message) |
| `case-split-incomplete` | analyse | (remedy in message) |

Remaining hardening (tracked, not yet done): assign codes to the rest of the ~72 v4 diagnostic
sites; the malformed-predicate checker gap (a `= =` predicate parses without a diagnostic) is a
separate parser bug, not a formatting issue.

## Further silent-acceptance gaps found by probing (verified, tracked)

Fixed: `malformed-predicate` (dangling operator / stray token / unknown operator / empty body) now
surfaced. Remaining "green means nothing" gaps found by probing the v4 check, each reproduced:

1. **Undeclared entity in a state sort passes silently.** FIXED: `observable state x(Ghost)` with no
   `entity Ghost` now emits `undeclared-entity`. Verified: 0 false positives across the real v4
   corpus; cli_smoke test locks it.
2. **Empty component passes silently.** `component C end` (no items) → 0 diagnostics. Debatable
   whether to flag, low priority.
3. **`parse` and `model` on v4 specs.** FIXED + now fully supported. `parse` runs the v4 grammar and
   dumps the v4 AST (with unified diagnostics); `model` extracts a v4 component-centric domain model
   (entities, observable states with sort + value type, givens, actions, role-tagged predicates,
   objectives) via the new `domain_model_v4` module. `plan` already had its v4 emitter. cli_smoke
   tests lock both. No command now runs the v3 grammar on a v4 spec.

These are checker-correctness follow-ups beyond the diagnostic-format feature; recorded here so they
are not lost.

## Advisory tier + coverage field (implemented)

Several v4 `analyse` lines were informational or advisory but emitted as `Warning`, so they blended
with real problems and read as noise. Fixed by separating signal from advice:

- **New `info` severity tier.** Advisories never affect the exit code and are filterable, so
  `warning` now means "a real problem to fix" (CONTRADICTORY, undeclared-name, malformed-predicate,
  undeclared-entity) and `info` means "transparency or a design suggestion". Ladder: `error` blocks,
  `warning` is a problem, `info` is advice.
- **Coverage moved to its own `coverage` field.** "what was verified, in which tier" is run metadata,
  not a per-problem diagnostic. `analyse` now emits it in a top-level `coverage` array, out of the
  `diagnostics`/warning stream. It stays available as the antidote to "green means nothing".
- **Advisories demoted to `info`** (kept, with codes, so a skill like `elicit` can still act on them):
  `vacuous-component`, `objective-design-time`, `objective-monitored`, `budget-monitored`. The
  invalid-measure case (`objective-measure-invalid`) stays a `warning` because it is a real defect.
- **Messages rewritten** in plain, Simplified-Technical-English style (short active sentences,
  present tense, no em-dashes) per the project writing guidance.

Codes now assigned: undeclared-name, undeclared-entity, malformed-predicate, vacuous-component,
contradictory-invariants, contradictory-rules, nonlinear-not-checked, invariant-not-entailed,
objective-unbounded, objective-measure-invalid, objective-design-time, objective-monitored,
budget-monitored, action-never-enabled, enum-value-unreachable, stuck-state, contract-not-satisfied,
case-split-not-disjoint, case-split-incomplete, coverage.

## Positive-result sweep (implemented)

`warning` is now reserved strictly for problems. The checker's positive and neutral verification
results were also emitted as warnings, which read oddly (a success reported as a problem). They are
now `info` with codes: `objective-discharged`, `objective-terminates`, `invariant-inductive`,
`invariant-preserved`, `conservation-preserved`, `invariant-safe`, `invariant-entailed`,
`rely-discharged`, `rules-satisfiable`, `case-split-disjoint`, and the conditional-preservation
caveat `preservation-conditional`. The problem cases in the same code paths (an action that CAN
break an invariant, a rely NOT discharged, a case-split NOT disjoint or incomplete) stay warnings.
Net: a well-formed, fully-specified component produces zero warnings; its results and advice are
info, and coverage is in the `coverage` field.
