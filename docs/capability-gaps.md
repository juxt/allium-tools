# Capability gaps

Behavioural claims our own specs need to make that Allium cannot yet express as
checkable predicates. Where a claim has no expressible form, it lives in a
`@guidance` annotation or a comment rather than a `requires:`/`ensures:` clause,
so the intent stays documented and the spec still checks clean. This file
records the classes so the gaps are visible rather than buried, and so the
guidance can migrate back into checkable clauses if the language later gains the
constructs.

Surfaced while making `docs/project/specs/allium-check-tool-behaviour.allium`
adhere to the v3 checker: of 62 rules, the checkable core of each (which
diagnostic code, at which severity) is expressed as real `ensures:`, while the
conditions and guarantees below could not be and became `@guidance`.

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
