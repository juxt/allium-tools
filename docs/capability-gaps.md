# Capability gaps

Behavioural claims our own specs make that Allium can express only
descriptively. Every predicate in these specs is written as a `requires:`/
`ensures:` clause that parses, but for the classes below the clause is a named
predicate call (e.g. `DiagnosticsOrderedBySourcePositionThenCode()`) that
carries the intent without the checker being able to confirm it: the checker
validates a spec's form, not the truth of its predicates, and nothing in the
language models the checker's own internals to make these decidable. This file
records the classes so the limit stays visible. If Allium later gains the
constructs to reason about them, these predicates can be given real backing.

Surfaced while migrating `docs/project/specs/` to the v3 checker: each rule's
checkable core (which diagnostic code, at which severity) is a real `ensures:`
observation, while the conditions and guarantees below are nominal predicate
calls of the kinds catalogued here.

## Meta-claims about output ordering and determinism

`ensures: diagnostics are ordered by source position then code`, and that the
diagnostics array is identical across repeated runs. Allium expresses what a run
produces, not a total order over its output or a stability guarantee across
invocations.

## Set reasoning over derived or aggregate sets

`requires: the aliased module offers no declared type or referenced trigger
named Name`, and `ensures: findings below the minimum severity are filtered
out`. The "offered set" and the filtered finding set are computed collections
with no declared type in the spec, so membership over them is not expressible
without inventing a domain model the spec does not otherwise carry.

## Filesystem and process effects

`ensures: FileWritten(path)`, `ensures: FileUnchanged(path)`, and use-path
resolution that depends on what is present on disk. These are effects on the
world outside the spec, which Allium describes only as named outcome events, not
as verifiable state.

## Structural predicates over the analysed spec's own AST

`requires: entity declares field`, `requires: a rule assigns the status value in
an ensures clause inside an if or else branch`. The checker's behaviour is
defined over the shape of the spec it analyses. Allium has no reflective
vocabulary for "a rule", "a field", or "an if branch" as subjects, so these
conditions can only be stated in prose.
