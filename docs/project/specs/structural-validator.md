# Structural validator

Build a structural validator for the Allium parser crate. The validator takes a parsed AST (`Module`) and produces diagnostics (errors and warnings) for violations of the language reference's validation rules.

## Where the code lives

Add a new module `validator.rs` in `crates/allium-parser/src/`. Re-export the public validation function from `lib.rs`. The validator is a pure function: it takes a `&Module` and returns a `Vec<Diagnostic>`.

```rust
// crates/allium-parser/src/lib.rs — add:
pub mod validator;

// crates/allium-parser/src/validator.rs — public API:
pub fn validate(module: &Module) -> Vec<Diagnostic>
```

The CLI (`crates/allium/src/main.rs`) already has a `cmd_check` function that parses files and prints diagnostics. After parsing, call `validate(&result.module)` and append the returned diagnostics to the parser's own diagnostics before printing.

## Existing infrastructure

**AST** (`ast.rs`): `Module` contains `Vec<Decl>`. Declarations are `Use`, `Block`, `Default`, `Variant`, `Deferred`, `OpenQuestion`, `Invariant`. `BlockDecl` has a `kind: BlockKind` (Entity, ExternalEntity, Value, Enum, Given, Config, Rule, Surface, Actor, Contract, Invariant) and `items: Vec<BlockItem>`. Block items are the uniform representation for declaration bodies — fields, clauses, relationships, lets, enum variants, for/if blocks, path assignments, contracts clauses, annotations, invariant blocks, open questions.

**Expressions** (`ast.rs`): The `Expr` enum has 35+ variants covering identifiers, literals, member access, calls, binary/comparison/logical ops, where/with filters, pipes, lambdas, conditionals, for-expressions, transitions_to/becomes, bindings, when-guards, type optionals, let-expressions, qualified names and blocks.

**Diagnostics** (`diagnostic.rs`): `Diagnostic { span: Span, message: String, severity: Severity }` with `Severity::Error` and `Severity::Warning`. Use `Diagnostic::error(span, msg)` and `Diagnostic::warning(span, msg)`.

**Span** (`span.rs`): `Span { start: usize, end: usize }` — byte offset range. Every AST node carries a span.

## Architecture

The validator should build a symbol table in a first pass, then check rules against it. Suggested internal structure:

1. **Symbol collection pass** — walk all declarations and build:
   - Entity map (name → fields, relationships, projections, derived values, status enums, discriminators)
   - Value type map
   - Named enum map
   - Variant map (name → base entity, additional fields)
   - Rule map (name → triggers, requires, ensures)
   - Surface map (name → facing, context, exposes, provides, contracts, guarantees)
   - Actor map
   - Contract map
   - Config map (parameter names, types, defaults)
   - Given bindings
   - External entity set
   - Default instances

2. **Validation pass** — check each rule category below against the symbol table, emitting diagnostics.

Keep the validator in a single file initially. Split into submodules only if it exceeds ~1000 lines.

## Validation rules

Implement these in priority order. Each rule number corresponds to the language reference.

### Tier 1: structural validity (rules 1–6)

These are the foundation. Everything else depends on having accurate entity/field resolution.

1. **All referenced entities and values exist.** When a field type, relationship target, rule trigger, surface facing/context, or expression references an entity or value name, that name must resolve to a declared entity, external entity, value type, or import.

2. **All entity fields have defined types.** Every field assignment in an entity block must have a value expression that resolves to a known type (primitive, entity, value, enum, or inline enum via pipe syntax).

3. **Relationships reference valid entities and include a backreference.** A relationship's type must be a declared entity (singular name). The `with` predicate must reference `this`. `where` is for filtering and must not reference `this`. Emit distinct errors for: unknown entity, missing `with`, `with` without `this` reference, `where` with `this` reference.

4. **Rules have at least one trigger and one ensures.** Check that every rule block contains a `when:` clause and an `ensures:` clause.

5. **Triggers are valid.** The `when:` value must be one of: external stimulus (EntityName.TriggerName with optional params), state transition (field transitions_to value), state becomes (field becomes value), entity creation (EntityName.created), temporal (comparison with `now`), derived (field comparison), or chained (another rule's trigger name).

6. **Shared trigger names use consistent signatures.** When multiple rules reference the same trigger name, they must agree on parameter count and positional types. Parameter binding names may differ.

### Tier 2: state machine validity (rules 7–9)

7. **Status values are reachable.** Every value in a status enum must appear as a target in some rule's ensures clause.

8. **Non-terminal states have exits.** Every status value that is not a terminal (i.e. some rule transitions from it) must have at least one outbound transition. A status value with no outbound transitions is terminal; one with no inbound transitions (other than creation) is unreachable (rule 7).

9. **No undefined states.** Rules cannot set a status field to a value not declared in that field's enum.

### Tier 3: expression validity (rules 10–14)

10. **No circular derived values.** Build a dependency graph of derived values and check for cycles.

11. **Variables bound before use.** Every identifier in an expression must resolve to a field, given binding, let binding, trigger parameter, config parameter, or entity/type name.

12. **Type consistency.** Comparisons and arithmetic must be between compatible types per the language reference's type compatibility table.

13. **Explicit lambdas.** Collection operations (`.any()`, `.all()`, `.map()`, `.filter()`, `.count()`, `.sum()`, `.flatMap()`, `.sortBy()`, `.groupBy()`) must receive lambda expressions with explicit parameters.

14. **No inline enum comparison.** Two fields with inline enum types (pipe-separated lowercase values) cannot be compared, even if they share the same literals. The fix is to extract a named enum.

### Tier 4: sum type validity (rules 15–21)

15. **Discriminators use pipe syntax with capitalised names.** A pipe expression where all branches are capitalised identifiers is a sum type discriminator.

16. **Discriminator names match variant declarations.** Every capitalised name in a discriminator must correspond to a `variant X : BaseEntity` declaration.

17. **All variants listed.** Every variant extending a base entity must appear in that entity's discriminator field.

18. **Variant fields guarded.** Fields specific to a variant are only accessible within type guards (`requires:` or `if` branches that narrow the discriminator).

19. **Base entities with discriminators not directly instantiated.** `.created()` calls must use the variant name, not the base entity name.

20. **Discriminator field names are user-defined.** No reserved name check; just validate that the field exists and uses pipe syntax.

21. **`variant` keyword required.** Variant declarations must use the `variant` keyword.

### Tier 5: given and config validity (rules 22–27)

22. **Given bindings reference declared entity types.**
23. **Given binding names are unique.**
24. **Unqualified instance references resolve.** Must resolve to given binding, let binding, trigger parameter, or default entity instance.
25. **Config parameters have explicit types.** Parameters with defaults must declare them. Parameters without defaults are mandatory.
26. **Config parameter names are unique.**
27. **Config references resolve.** `config.field` must correspond to a declared parameter.

### Tier 6: surface validity (rules 28–35)

28. **Facing types are actors or valid entities.**
29. **Exposed fields are reachable** from surface bindings (facing, context, let) via relationships.
30. **Provided triggers are defined** as external stimulus triggers in rules.
31. **Related surfaces exist** and their context type matches.
32. **Facing and context bindings used consistently.**
33. **When conditions reference valid fields.**
34. **For iterations over collection-typed fields.**
35. **Timeout rule references valid temporal rules.** When a `when` condition is present, verify it matches the referenced rule's temporal trigger.

### Tier 7: contract clause validity (rules 36–39)

36. **`contracts:` entries must use `demands` or `fulfils` followed by a PascalCase contract name.**
37. **Each contract name appears at most once per surface.**
38. **Referenced contract names must resolve to a `contract` declaration in scope (local or imported via `use`).**
39. **Same-named contracts from different modules on the same surface are a structural error.**

### Tier 8: contract validity (rules 40–45)

40. **`contract` declarations must have a PascalCase name followed by a brace-delimited block body.**
41. **Contract bodies may contain only typed signatures and annotations (`@invariant`, `@guidance`).** No entity/value/enum/variant declarations.
42. **Types in contract signatures must be declared at module level or imported via `use`.**
43. **Contract names must be unique at module level.**
44. **`@invariant` annotations within contracts must have a PascalCase name and be followed by at least one indented comment line.**
45. **`@invariant` names must be unique within their contract.**

### Tier 9: config reference validity (rules 46–48)

46. **A qualified config reference in a default expression must resolve to a declared parameter in an imported module's config block.**
47. **The declared type of a parameter with a qualified default must match the referenced parameter's type.**
48. **The config reference graph must be acyclic.**

### Tier 10: config expression validity (rules 49–50)

49. **Expression-form config defaults must use only arithmetic operators (`+`, `-`, `*`, `/`), literal values, local config parameter references and qualified config references.**
50. **Both sides of an arithmetic operator in a config default must resolve to type-compatible operands per the type compatibility table.**

### Tier 11: invariant validity (rules 51–57)

51. **Top-level `invariant` blocks must have a PascalCase name followed by a brace-delimited expression body.**
52. **Entity-level `invariant` blocks must have a PascalCase name followed by a brace-delimited expression body.**
53. **Invariant names must be unique within their scope** (module-level for top-level invariants, entity declaration for entity-level invariants).
54. **Invariant expressions must evaluate to a boolean type.**
55. **Invariant expressions must not contain side-effecting operations** (`.add()`, `.remove()`, `.created()`, trigger emissions).
56. **Invariant expressions must not reference `now`** (volatile; stored timestamp fields are permitted).
57. **Entity collection references in top-level invariants must correspond to declared entity types.**

### Tier 12: annotation validity (rules 58–62)

58. **`@invariant` requires a PascalCase name; names must be unique within their containing construct (contract or surface).**
59. **`@guarantee` requires a PascalCase name; names must be unique within their surface.**
60. **`@guidance` must not have a name; must appear after all structural clauses and after all other annotations in its containing construct.**
61. **All annotations must be followed by at least one indented comment line; unindented comment lines after an annotation are not part of the annotation body.**
62. **Within a construct, `@invariant` and `@guarantee` annotations may appear in any order relative to each other but must appear after all structural clauses; `@guidance` must appear last.**

## Error codes

Error codes and message templates will be defined by the validator implementation. The validation rules above (from the language reference) are the specification; the validator owns the diagnostic wording and code numbering.

## Warnings

The validator should also emit warnings (not errors) for these conditions. Implement warnings after all error rules are working. This list matches the language reference.

- External entities without known governing specification
- Open questions
- Deferred specifications without location hints
- Unused entities or fields
- Rules that can never fire (preconditions always false)
- Temporal rules without guards against re-firing
- Surfaces that reference fields not used by any rule (may indicate dead code)
- Items in `provides` with `when` conditions that can never be true
- Actor declarations that are never used in any surface
- Rules whose ensures creates an entity for a parent, where sibling rules on the same parent don't guard against that entity's existence
- Surface `provides` when-guards weaker than the corresponding rule's requires
- Rules with the same trigger and overlapping preconditions (spec ambiguity)
- Parameterised derived values that reference fields outside the entity (scoping violation)
- Actor `identified_by` expressions that are trivially always-true or always-false
- Rules where all ensures clauses are conditional and at least one execution path produces no effects
- Temporal triggers on optional fields (trigger will not fire when the field is null)
- Surfaces that use a raw entity type in `facing` when actor declarations exist for that entity type (may indicate a missing access restriction)
- `transitions_to` triggers on values that entities can be created with (the rule will not fire on creation; consider `becomes` if the rule should also fire on creation)
- Multiple fields on the same entity with identical inline enum literals (suggests extraction to a named enum; will error if the fields are later compared)
- `@invariant` prose that resembles a formal expression (informational: promote to expression-bearing `invariant Name { expression }` when the assertion is machine-readable)
- Config reference chains deeper than two levels of indirection
- Diamond dependency conflicts in config overrides

## Testing

Follow the testing principles in `AGENTS.md`: meaningful, behaviour-focused tests over raw coverage.

**Unit tests** in `crates/allium-parser/src/validator.rs` (as `#[cfg(test)] mod tests`):
- Write a helper that parses an Allium snippet and runs validation, returning diagnostics.
- For each validation rule, write at least one test for the error case and one for the valid case.
- Group tests by tier (structural, state machine, expression, etc.).
- Test that diagnostics reference the correct rule violations with clear, actionable messages.

**Integration tests** in `crates/allium-parser/tests/`:
- Add fixture `.allium` files that exercise multiple validation rules together.
- Test that the existing fixtures (`comprehensive-edge-cases.allium`, `language-reference-constructs.allium`) produce no unexpected validation errors.

## Implementation guidance

- The parser already handles syntax. The validator handles semantics. Don't re-parse; work from the AST.
- Use `Diagnostic::error(span, msg)` and `Diagnostic::warning(span, msg)` — the infrastructure is ready.
- The `Span` on each AST node gives you the source location for diagnostics.
- Walk declarations to build the symbol table before checking rules. Two passes, not one.
- For cross-entity checks (relationship backreferences, variant-base consistency, trigger signature matching), build indices during the collection pass.
- Start with tier 1. Get entity/field resolution working and tested before moving to state machine or expression checks.
- For type inference (rules 12, 49, 52, 56), start with a simple approach: known primitives (String, Integer, Boolean, Duration, Timestamp), entity types, value types, named enums, inline enums, optionals. Full type inference can be refined later.
- The `BlockItemKind::Clause` variant is where keywords like `when`, `requires`, `ensures`, `facing`, `context`, `exposes`, `provides`, `related`, `timeout`, `contracts` appear. Match on `keyword` string.
- `BlockItemKind::ContractsClause { entries }` holds contract bindings with `demands`/`fulfils` direction markers.
- `BlockItemKind::Annotation(Annotation)` holds `@invariant`, `@guidance` and `@guarantee` prose annotations with comment bodies.
- After completing the validator, update `docs/project/specs/` to reflect the new capability.

## Scope boundaries

- Single-module validation only. Cross-module validation (resolving `use` imports to other files) is out of scope for the first version. Emit a warning for unresolved imports rather than an error.
- Black box functions are opaque. Don't attempt to type-check their bodies or resolve their definitions.
- The `guidance:` block content is opaque. Validate its position (rule 60) but not its content (rule 61).
- Expression-form config defaults (rules 51–52) need basic arithmetic type checking only, not full expression evaluation.
