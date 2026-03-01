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

**AST** (`ast.rs`): `Module` contains `Vec<Decl>`. Declarations are `Use`, `Block`, `Default`, `Variant`, `Deferred`, `OpenQuestion`, `Invariant`. `BlockDecl` has a `kind: BlockKind` (Entity, ExternalEntity, Value, Enum, Given, Config, Rule, Surface, Actor, Contract, Invariant) and `items: Vec<BlockItem>`. Block items are the uniform representation for declaration bodies — fields, clauses, relationships, lets, enum variants, for/if blocks, path assignments, expects/offers, invariant blocks, open questions.

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
   - Surface map (name → facing, context, exposes, provides, expects, offers)
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

### Tier 7: obligation block and contract validity (rules 36–47)

36. **Obligation blocks have PascalCase names and brace-delimited bodies.**
37. **Obligation block bodies contain only typed signatures, invariant: declarations and guidance: blocks.** No entity/value/enum/variant declarations.
38. **Obligation block names unique within surface** (across both expects and offers).
39. **Types in obligation block signatures resolve.**
40. **invariant: has PascalCase name and prose description.**
41. **Invariant names unique within obligation block and across blocks in same surface.**
42. **Same-named obligation blocks across composed surfaces error.** (E2)
43. **Contract declarations have PascalCase names and brace bodies.**
44. **Contract bodies contain only typed signatures, invariant: and guidance:.**
45. **Contract names unique at module level.**
46. **Contract references in expects/offers resolve.** (E3)
47. **No inline block and contract reference with same name.** (E4)

### Tier 8: config reference and expression validity (rules 48–52)

48. **Qualified config references resolve.** (E5)
49. **Type matches for config defaults.** (E6)
50. **Config reference graph acyclic.**
51. **Config default expressions use only arithmetic, literals, and config references.** (E7)
52. **Arithmetic operands type-compatible.** (E8)

### Tier 9: invariant validity (rules 53–59)

53. **Top-level invariant blocks have PascalCase name and expression body.**
54. **Entity-level invariant blocks have PascalCase name and expression body.**
55. **Invariant names unique within scope.**
56. **Invariant expressions evaluate to boolean.** (E9)
57. **Invariant expressions contain no side effects.** No `.add()`, `.remove()`, `.created()`, trigger emissions. (E10)
58. **Invariant expressions do not reference `now`.** (E11)
59. **Entity collection references in top-level invariants resolve.**

### Tier 10: guidance validity (rules 60–61)

60. **guidance: appears after all other clauses in a rule.**
61. **guidance: content is opaque** — validate block boundary only, not content.

## Error catalogue

Use these exact error codes and message templates. The code should appear in the diagnostic message.

| Code | Condition | Message template |
|------|-----------|-----------------|
| E1 | Duplicate obligation block name in same surface | "Obligation block '{name}' is already declared in this surface. Obligation block names must be unique within a surface." |
| E2 | Same-named obligation block across composed surfaces | "Obligation block '{name}' is declared in both {surface_a} and {surface_b}. Rename one to resolve the conflict." |
| E3 | Unresolved expects/offers reference | "No contract or obligation block named '{name}' found. Declare it as `contract {name} {{ ... }}` at module level, or define it inline as `expects {name} {{ ... }}` in this surface." |
| E4 | Both inline and contract reference with same name | "Surface declares both an inline obligation block and a contract reference named '{name}'. Use one or the other." |
| E5 | Unresolved qualified config reference | "Config parameter '{param}' not found in module '{alias}'. Check that the parameter name matches and the module is imported via `use`." |
| E6 | Config type mismatch | "Type mismatch: '{param}' is {type_a} in module '{module}', but declared as {type_b} here." |
| E7 | Invalid config default expression | "Config default expressions support arithmetic operators and config references only. '{expr}' is not a valid config default expression." |
| E8 | Incompatible arithmetic operands | "Cannot apply '{op}' to {type_a} and {type_b}. {hint}" |
| E9 | Non-boolean invariant | "Invariant '{name}' must evaluate to a boolean. The expression evaluates to {type}." |
| E10 | Side effect in invariant | "Invariant expressions must be pure assertions. '{construct}' is a side effect and cannot appear in an invariant." |
| E11 | `now` in invariant | "Invariants assert state properties, not temporal conditions. Use a rule with a temporal trigger instead." |

For structural violations without a catalogue code (rules 1–21, 22–35, 60–61), use descriptive messages. Examples:

- "Unknown entity 'Foo'. No entity, external entity, value type or import matches this name."
- "Rule 'Bar' has no ensures clause."
- "Relationship 'slots' must include a `with` predicate referencing `this`."
- "Status value 'archived' on User.status is unreachable: no rule transitions to it."
- "Circular dependency in derived values: x → y → x."
- "Inline enum fields cannot be compared. Extract a named enum to share values across fields."

## Warnings

The validator should also emit warnings (not errors) for these conditions. Implement warnings after all error rules are working.

- External entities without a governing specification comment
- Open questions (flag their existence)
- Deferred specifications without location hints
- Unused entities or fields
- Rules that can never fire (preconditions always false)
- Temporal rules without guards against re-firing
- Surfaces referencing fields not used by any rule
- Provides with when conditions that can never be true
- Actor declarations never used in any surface
- Rules whose ensures creates an entity where sibling rules on the same parent don't guard against that entity's existence
- Surface provides when-guards weaker than the corresponding rule's requires
- Rules with the same trigger and overlapping preconditions
- Parameterised derived values referencing fields outside the entity
- Actor identified_by expressions that are trivially always-true or always-false
- Rules where all ensures clauses are conditional and at least one path produces no effects
- Temporal triggers on optional fields
- Surfaces using raw entity type in facing when actor declarations exist for that entity
- transitions_to triggers on values that entities can be created with (consider becomes)
- Multiple fields on same entity with identical inline enum literals
- Obligation blocks with no invariants
- Invariant descriptions resembling formal expressions (suggest expression-bearing syntax)
- `expects:` or `offers:` with colon and no block name inside a surface
- Config reference chains deeper than two levels
- Diamond dependency conflicts in config overrides

## Testing

Follow the testing principles in `AGENTS.md`: meaningful, behaviour-focused tests over raw coverage.

**Unit tests** in `crates/allium-parser/src/validator.rs` (as `#[cfg(test)] mod tests`):
- Write a helper that parses an Allium snippet and runs validation, returning diagnostics.
- For each validation rule, write at least one test for the error case and one for the valid case.
- Group tests by tier (structural, state machine, expression, etc.).
- Test the error catalogue codes: verify E1–E11 messages appear with the correct code.

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
- The `BlockItemKind::Clause` variant is where keywords like `when`, `requires`, `ensures`, `facing`, `context`, `exposes`, `provides`, `related`, `timeout`, `guarantee`, `guidance` appear. Match on `keyword` string.
- `BlockItemKind::Expects` and `BlockItemKind::Offers` have an optional `items` field: `Some(items)` is an inline obligation block, `None` is a contract reference.
- After completing the validator, update `docs/project/specs/` to reflect the new capability.

## Scope boundaries

- Single-module validation only. Cross-module validation (resolving `use` imports to other files) is out of scope for the first version. Emit a warning for unresolved imports rather than an error.
- Black box functions are opaque. Don't attempt to type-check their bodies or resolve their definitions.
- The `guidance:` block content is opaque. Validate its position (rule 60) but not its content (rule 61).
- Expression-form config defaults (rules 51–52) need basic arithmetic type checking only, not full expression evaluation.
