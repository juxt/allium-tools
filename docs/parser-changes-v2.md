# Parser changes: ALP-7, ALP-9, ALP-10, ALP-11, ALP-13

This document describes all language changes adopted by the design review committee that affect the parser, structural validator and checker. Each section specifies the grammar change, validation rules, error codes and test scenarios.

The language reference is the canonical specification. This document is a focused extraction for the parser team.

## 1. `guidance:` as a rule clause (ALP-7)

### Grammar change

`guidance:` is now a valid clause in rule bodies. It must appear after all other clauses. The clause sequence becomes: `when`, `for`, `let`, `requires`, `ensures`, `guidance`.

```
rule RuleName {
    when: trigger
    requires: precondition
    ensures: postcondition

    guidance:
        -- Non-normative implementation advice.
}
```

`guidance:` uses colon-delimited, indentation-scoped syntax (same as `ensures:`, `exposes:`). It is not brace-delimited. Content is opaque prose.

### Validation rules

60. `guidance:` in a rule must appear after all other clauses
61. `guidance:` content is opaque; the checker does not parse or validate it beyond recognising the block boundary

### Test scenarios

**Should parse:**
- Rule with `guidance:` after `ensures:`
- Rule with `guidance:` containing multiple comment lines
- Rule with `guidance:` containing empty lines between comments
- Rule with all clause types present including `guidance:` last

**Should reject:**
- `guidance:` appearing before `ensures:`
- `guidance:` appearing before `requires:`
- `guidance:` appearing between two `ensures:` clauses
- Two `guidance:` blocks in the same rule

**Should not affect:**
- `guidance:` in surfaces (already valid, unchanged)
- `guidance:` in obligation blocks (already valid, unchanged)
- `guidance:` in contracts (valid, same syntax as obligation blocks)

---

## 2. `contract` declarations (ALP-9)

### Grammar change

New top-level declaration keyword: `contract`. Declares a named obligation block at module level.

```
contract Codec {
    serialize: (value: Any) -> ByteArray
    deserialize: (bytes: ByteArray) -> Any

    invariant: Roundtrip
        -- Prose description.

    guidance:
        -- Non-normative advice.
}
```

Contracts appear in a new section between Value Types and Enumerations in the file structure.

Contract bodies permit exactly the same contents as inline obligation blocks: typed signatures, `invariant:` declarations (colon-delimited, prose-only) and `guidance:` blocks.

### Surface referencing

Surfaces can reference contracts by name without braces:

```
surface Example {
    facing party: ActorType

    expects Codec                    -- contract reference (no braces)
    expects InlineBlock {            -- inline obligation block (with braces)
        operation: (param: Type) -> ReturnType
    }

    offers EventSubmitter            -- contract reference
}
```

The parser must distinguish:
- `expects Name` (no braces) — contract reference, resolves to a `contract` declaration
- `expects Name { ... }` (with braces) — inline obligation block

### Validation rules

43. `contract` declarations must have a PascalCase name followed by a brace-delimited block body
44. Contract bodies may contain only typed signatures, `invariant:` declarations and `guidance:` blocks
45. Contract names must be unique at module level
46. A surface `expects`/`offers` clause referencing a contract name must resolve to a `contract` declaration in scope (local or imported via `use`)
47. A surface may not have both an inline obligation block and a contract reference with the same name

### Error codes

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E2 | Two obligation blocks (whether both `expects`, both `offers`, or one of each) with the same name in the same surface | "Obligation block 'Foo' is already declared in this surface. Obligation block names must be unique within a surface." |
| E3 | Two surfaces that are composed or referenced together both declare an obligation block with the same name | "Obligation block 'Foo' is declared in both SurfaceA and SurfaceB. Rename one to resolve the conflict." |
| E4 | `expects Foo` or `offers Foo` where `Foo` is not an in-scope `contract` or inline obligation block | "No contract or obligation block named 'Foo' found. Declare it as `contract Foo { ... }` at module level, or define it inline as `expects Foo { ... }` in this surface." |
| E5 | A surface declares both `expects Foo { ... }` (inline) and `expects Foo` (reference) | "Surface declares both an inline obligation block and a contract reference named 'Foo'. Use one or the other." |

### Contract identity

Identity is by module-qualified name. Two contracts are the same iff they resolve to the same module-qualified declaration. Composed surfaces referencing the same module-qualified contract are not in conflict. Surfaces referencing identically named contracts from different modules are a structural error (extends validation rule 42).

### Import semantics

Contracts are importable via `use`. Imports are atomic (whole contract only, no partial imports). A contract imported as `other/Codec` is the same identity as `Codec` declared in the `other` module.

### Test scenarios

**Should parse:**
- `contract` declaration with signatures only
- `contract` declaration with signatures, `invariant:` and `guidance:`
- Surface with `expects ContractName` (no braces)
- Surface with `offers ContractName` (no braces)
- Surface mixing contract references and inline blocks
- Contract imported via `use` and referenced in a surface
- Contract with `Any` type in signatures

**Should reject:**
- `contract` with lowercase name (rule 43)
- `contract` with entity/value/enum declarations inside (rule 44)
- Two `contract` declarations with the same name in the same module (rule 45)
- Two obligation blocks with the same name in the same surface (E2)
- Same-named obligation block across two composed surfaces (E3)
- `expects UnknownContract` where no contract or inline block matches (E4)
- Surface with both `expects Foo { ... }` and `expects Foo` (E5)
- `contract` with colon-delimited body instead of braces (rule 43)

**Edge cases:**
- Contract name coincides with an entity name in a different module (valid, accessed via qualified names)
- Two composed surfaces both `expects Codec` where `Codec` resolves to the same module-qualified contract (valid, not a conflict)
- Two composed surfaces both `expects Codec` where `Codec` resolves to different contracts in different modules (structural error)

---

## 3. Cross-module config parameter references (ALP-10)

### Grammar change

Config parameter defaults now accept qualified references to parameters in imported modules:

```
use "./core.allium" as core

config {
    max_batch_size: Integer = core/config.max_batch_size
    publish_delay: Duration = core/config.publish_delay
}
```

The parser must recognise `alias/config.param` in config default position. The `/config.` infix is the structural signal distinguishing qualified config references from other identifiers.

### Resolution semantics

1. Explicit override wins
2. Else follow the qualified reference (using the referenced parameter's resolved value)
3. Else use the referenced parameter's own default

Chains resolve transitively. Cycles are prohibited.

### Validation rules

48. A qualified config reference in a default expression must resolve to a declared parameter in an imported module's config block
49. The declared type of a parameter with a qualified default must match the referenced parameter's type
50. The config reference graph must be acyclic

### Error codes

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E6 | `alias/config.param` where `param` is not declared in the referenced module's config block | "Config parameter 'param' not found in module 'alias'. Check that the parameter name matches and the module is imported via `use`." |
| E7 | Declared type does not match referenced parameter's type | "Type mismatch: 'max_retries' is Integer in module 'core', but declared as Duration here." |

### Warnings

| Code | Trigger | Diagnostic |
|------|---------|------------|
| W1 | Config reference chain exceeds two levels of indirection | "Config parameter 'field' resolves through N levels of indirection. Consider referencing the source parameter directly." |

### Test scenarios

**Should parse:**
- Config parameter with qualified reference default
- Config parameter with qualified reference and explicit type
- Multiple parameters referencing different modules
- Parameter referencing a parameter that itself has a qualified default (chain)

**Should reject:**
- Reference to non-existent parameter in imported module (E6)
- Type mismatch between local declaration and referenced parameter (E7)
- Cyclic config references: A defaults to B, B defaults to A (rule 50)
- Indirect cycle through three modules (rule 50)
- Reference to a module not imported via `use` (E6)

**Should warn:**
- Chain of length 3 (A -> B -> C -> literal) (W1)
- Diamond dependency: two modules override the same parameter in a shared dep

**Edge cases:**
- Renaming permitted: `publish_delay: Duration = core/config.default_delay` (valid)
- Override takes precedence over qualified reference default
- Override of a referenced parameter propagates through the chain

---

## 4. Expression-form config defaults (ALP-13)

### Grammar change

Config parameter defaults can be arithmetic expressions combining qualified references, local config references and literal values:

```
config {
    base_timeout: Duration = core/config.base_timeout
    extended_timeout: Duration = core/config.base_timeout * 2
    buffer_size: Integer = core/config.batch_size + 10
    retry_limit: Integer = max_attempts - 1
}
```

Operators: `+`, `-`, `*`, `/` with standard precedence. Parenthesised sub-expressions are permitted. Expression-form defaults are evaluated once at config resolution time, after all overrides have been applied. They are not re-evaluated dynamically.

The config default production becomes: `default_value := literal | qualified_ref | local_ref | expression`.

### Type compatibility table

| Left | Operator | Right | Result |
|------|----------|-------|--------|
| Integer | `+` `-` `*` `/` | Integer | Integer |
| Duration | `+` `-` | Duration | Duration |
| Duration | `*` `/` | Integer | Duration |
| Integer | `*` | Duration | Duration |
| Decimal | `+` `-` `*` `/` | Decimal | Decimal |
| Decimal | `*` `/` | Integer | Decimal |
| Integer | `*` | Decimal | Decimal |

Integer division uses truncation toward zero. All other combinations are type errors. `Duration * Decimal` and `Decimal * Duration` are type errors; duration scaling uses Integer multipliers only. Commutative rows are listed for scalar multiplication only; addition and subtraction require matching types (Integer with Integer, Duration with Duration, Decimal with Decimal).

### Validation rules

51. Expression-form config defaults must use only arithmetic operators, literal values, local config parameter references and qualified config references
52. Both sides of an arithmetic operator must resolve to type-compatible operands per the type compatibility table

### Error codes

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E8 | Expression uses an operator or construct beyond arithmetic and config references | "Config default expressions support arithmetic operators and config references only. 'slots.count' is not a valid config default expression." |
| E9 | Arithmetic operator applied to type-incompatible operands | "Cannot apply '*' to Duration and Duration. Duration can be multiplied by Integer, not by another Duration." |

### Acyclicity

Local references in expressions create dependency edges within the same config block. The acyclicity rule (rule 50) applies uniformly to both cross-module and local edges.

### Test scenarios

**Should parse:**
- `param: Integer = other_param + 1` (local reference)
- `param: Duration = core/config.timeout * 2` (qualified reference with arithmetic)
- `param: Integer = (base + 1) * factor` (parenthesised sub-expression)
- `param: Duration = core/config.a + core/config.b` (two qualified refs)
- `param: Decimal = price * 1.5` (decimal literal)
- `param: Duration = timeout * 2 + 1.minute` (mixed operators)

**Should reject:**
- `param: Boolean = core/config.flag and local_flag` (boolean expression, E8)
- `param: Duration = core/config.timeout * core/config.timeout` (Duration * Duration, E9)
- `param: Duration = core/config.timeout * 1.5` (Duration * Decimal, E9)
- `param: Integer = slots.count` (not a config reference, E8)
- `param: String = "hello" + "world"` (string concatenation not supported, E8)
- Cycle through local references: `a = b + 1`, `b = a - 1` (rule 50)

**Edge cases:**
- `param: Integer = 5` (literal only, already supported, must still work)
- `param: Integer = core/config.x` (bare reference, ALP-10 syntax, must still work)
- Integer division truncates toward zero: `7 / 2 = 3`
- Operator precedence: `a + b * c` means `a + (b * c)`

---

## 5. Expression-bearing invariants and `implies` (ALP-11)

### Grammar change: `implies`

New boolean operator: `implies`. `a implies b` is equivalent to `not a or b`.

Precedence (lowest to highest): `implies`, `or`, `and`, `not`.

Available in all expression contexts (rules, derived values, invariants, surfaces), not only invariants.

```
account.status = closed implies account.balance = 0
```

### Grammar change: top-level `invariant`

New top-level declaration. Appears in the Invariants section after Rules and before Actor Declarations.

```
invariant NonNegativeBalance {
    for account in Accounts:
        account.balance >= 0
}
```

PascalCase name, brace-delimited body, body is an expression that evaluates to boolean.

### Grammar change: entity-level `invariant`

New declaration within entity bodies:

```
entity Account {
    balance: Decimal
    credit_limit: Decimal

    invariant SufficientFunds {
        balance >= -credit_limit
    }
}
```

Field names resolve to the enclosing entity's fields without qualification. `this` refers to the entity instance.

### Syntactic distinction from prose-only invariants

Two distinct forms:

- `invariant: Name` (colon, then indented prose) — prose-only, in obligation blocks and contracts
- `invariant Name { expression }` (no colon, braces) — expression-bearing, at top-level and entity-level

The parser must distinguish these by the presence or absence of the colon after `invariant`.

### Expression language

Invariant expressions use the existing expression language:
- Navigation, optional navigation, null coalescing
- Comparisons, `in` / `not in`
- Boolean logic including `implies`
- Arithmetic
- Collection operations (`.count`, `.any()`, `.all()`)
- `for x in Collection: expression` (universal quantifier)
- `exists entity`, `not exists entity`
- `let` bindings (must be pure)

### Purity constraints

- No side effects: `.add()`, `.remove()`, `.created()`, trigger emissions are prohibited
- No `now`: volatile temporal reference is prohibited. Stored timestamp fields (`created_at`) are permitted.
- `let` bindings must be pure

### Quantification semantics

`for x in Collection: expression` in an invariant body is a universal quantifier (all elements must satisfy). Same syntax as rule-level `for` but with assertion semantics, not ensures semantics.

### Validation rules

53. Top-level `invariant` blocks must have a PascalCase name followed by a brace-delimited expression body
54. Entity-level `invariant` blocks must have a PascalCase name followed by a brace-delimited expression body
55. Invariant names must be unique within their scope (module-level for top-level, entity declaration for entity-level)
56. Invariant expressions must evaluate to a boolean type
57. Invariant expressions must not contain side-effecting operations
58. Invariant expressions must not reference `now` (volatile; stored timestamp fields are permitted)
59. Entity collection references in top-level invariants must correspond to declared entity types

### Error codes

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E10 | Invariant body evaluates to non-boolean type | "Invariant 'NonNegativeBalance' must evaluate to a boolean. The expression evaluates to Integer." |
| E11 | Invariant body contains side effects | "Invariant expressions must be pure assertions. '.created()' is a side effect and cannot appear in an invariant." |
| E12 | Invariant body references `now` | "Invariants assert state properties, not temporal conditions. Use a rule with a temporal trigger instead." |

### Test scenarios: `implies`

**Should parse:**
- `a implies b` in a `requires:` clause
- `a implies b` in an `ensures:` conditional
- `a implies b` in a derived value
- `a and b implies c` (binds as `(a and b) implies c`)
- `a implies b implies c` (right-associative or left-to-right, specify)
- `not a implies b` (binds as `(not a) implies b`)
- `a implies b or c` (binds as `a implies (b or c)`)
- `a or b implies c` (binds as `(a or b) implies c`)

**Should reject:**
- `implies` used as an identifier name

### Test scenarios: top-level invariants

**Should parse:**
- Simple boolean expression: `invariant X { account.balance >= 0 }`
- Single `for` quantifier: `invariant X { for a in Accounts: a.balance >= 0 }`
- Nested `for` quantifiers: `invariant X { for a in As: for b in Bs: ... }`
- `implies` in invariant body
- `let` binding in invariant body
- Collection operations (`.any()`, `.all()`, `.count`)
- `exists` and `not exists`
- Optional navigation (`parent?.field`)
- Null coalescing (`field ?? default`)

**Should reject:**
- Invariant with lowercase name (rule 53)
- Invariant body that evaluates to Integer (E10)
- Invariant body containing `.created()` (E11)
- Invariant body containing `.add()` or `.remove()` (E11)
- Invariant body containing trigger emission (E11)
- Invariant body referencing `now` (E12)
- Invariant referencing undeclared entity collection (rule 59)
- Two top-level invariants with the same name (rule 55)
- Invariant with colon-delimited body at top level (syntactic confusion with prose form)

### Test scenarios: entity-level invariants

**Should parse:**
- Simple field comparison: `invariant X { balance >= 0 }`
- Reference to enclosing entity's fields without qualification
- `this` reference to the entity instance
- Relationship traversal from entity fields
- `implies` in entity-level invariant

**Should reject:**
- Two entity-level invariants with the same name in the same entity (rule 55)
- Entity-level invariant with lowercase name (rule 54)
- Side effects in entity-level invariant (E11)
- `now` in entity-level invariant (E12)

**Edge cases:**
- Entity-level invariant name can duplicate a top-level invariant name (different scopes, rule 55)
- `for` inside entity-level invariant iterates over entity's own collections
- `let` binding in entity-level invariant body

---

## Complete validation rule numbering

| Range | Topic | ALP |
|-------|-------|-----|
| 1-42 | Existing rules | — |
| 43-47 | Contract validity | ALP-9 |
| 48-50 | Config reference validity | ALP-10 |
| 51-52 | Config expression validity | ALP-13 |
| 53-59 | Invariant validity | ALP-11 |
| 60-61 | Rule guidance validity | ALP-7 |

## Complete error catalogue

| Code | Topic | ALP |
|------|-------|-----|
| E2 | Duplicate obligation block name within a surface | ALP-1 |
| E3 | Same-named obligation block across composed surfaces | ALP-1 |
| E4 | Unresolved contract reference | ALP-9 |
| E5 | Name collision (inline block vs contract ref) | ALP-9 |
| E6 | Unresolved config reference | ALP-10 |
| E7 | Type mismatch in config reference | ALP-10 |
| E8 | Invalid config default expression | ALP-13 |
| E9 | Type-incompatible config default expression | ALP-13 |
| E10 | Non-boolean invariant expression | ALP-11 |
| E11 | Side effect in invariant | ALP-11 |
| E12 | Temporal reference in invariant | ALP-11 |

## Complete warning catalogue additions

| Code | Topic | ALP |
|------|-------|-----|
| W1 | Deep config reference chain (> 2 levels) | ALP-10 |

Existing warning updated:
- "Invariant descriptions that resemble formal expressions" now suggests `invariant Name { expression }` syntax rather than mentioning a future version.

## AST additions

The parser's typed AST needs these new node types:

### `ContractDeclaration`
- `name`: PascalCase identifier
- `signatures`: list of typed signatures (same structure as obligation block signatures)
- `invariants`: list of prose-only invariant declarations
- `guidance`: optional guidance block

### `ContractReference`
- `name`: PascalCase identifier (resolved to a `ContractDeclaration`)
- `direction`: `expects` | `offers`
- Context: appears in surface bodies

### `InvariantDeclaration` (expression-bearing)
- `name`: PascalCase identifier
- `body`: expression node (must evaluate to boolean)
- `scope`: `top_level` | `entity_level`
- Context: top-level (after rules) or inside entity declarations

### `ConfigReference` (in config defaults)
- `module_alias`: identifier (the `use` alias)
- `parameter_name`: snake_case identifier
- Context: config parameter default expressions

### `ConfigExpression` (in config defaults)
- `operator`: `+` | `-` | `*` | `/`
- `left`: config reference | local reference | literal | config expression
- `right`: config reference | local reference | literal | config expression
- Context: config parameter default position

### `ImpliesExpression`
- `left`: expression node
- `right`: expression node
- Precedence: lowest of all boolean operators

### `GuidanceClause` (in rules)
- `content`: opaque text (list of comment lines)
- Context: rule body, must be final clause
