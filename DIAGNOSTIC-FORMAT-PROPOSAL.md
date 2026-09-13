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
