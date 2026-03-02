# Parser changes: ALP-14, ALP-15, ALP-16

This document describes changes to the parser, AST and validation rules for ALPs 14 through 16. It follows the format established in `parser-changes-v2.md`. ALP-14 was superseded by ALP-15 and requires no parser work.

The language reference is the canonical specification. This document is a focused extraction for the parser team.

## 1. Contracts replace obligation clauses (ALP-15)

ALP-15 removes `expects`/`offers` and their inline obligation block form. Surfaces reference contracts through a single `contracts:` clause with direction markers.

### Removals

**Tokens to remove:**
- `TokenKind::Expects` (lexer.rs)
- `TokenKind::Offers` (lexer.rs)
- `"expects"` and `"offers"` from `classify_keyword()` (lexer.rs)
- `"expects"` and `"offers"` from Display impl (lexer.rs)

**AST nodes to remove:**
- `BlockItemKind::Expects { name, items }` (ast.rs)
- `BlockItemKind::Offers { name, items }` (ast.rs)

**Parser code to remove:**
- `parse_obligation_item()` function (parser.rs)
- `TokenKind::Expects` and `TokenKind::Offers` branches in block item parsing (parser.rs)
- All tests for `expects`/`offers`: `surface_expects_reference`, `surface_offers_reference`, `surface_expects_inline_block`, `surface_offers_inline_block` and similar

### Additions

**New AST nodes:**

```rust
/// Direction marker for contract bindings in surfaces
enum ContractDirection {
    Demands,
    Fulfils,
}

/// A single entry in a contracts: clause
struct ContractBinding {
    direction: ContractDirection,
    name: Ident,
    span: Span,
}

/// The contracts: clause in a surface body
/// In BlockItemKind enum:
ContractsClause {
    entries: Vec<ContractBinding>,
}
```

**Clause keyword registration:**
- Add `"contracts"` to `is_clause_keyword()` (parser.rs)

**Parsing logic:**

The `contracts:` clause uses colon-delimited, indentation-scoped syntax (like `exposes:` and `provides:`). Each entry is a direction marker followed by a PascalCase name:

```
contracts:
    demands DeterministicEvaluation
    fulfils EventSubmitter
```

The parser reads the `contracts:` keyword, then parses indented lines. Each line must start with `demands` or `fulfils` (treated as contextual keywords, not reserved tokens) followed by a PascalCase identifier. `demands` and `fulfils` do not need dedicated token kinds; they can be recognised as identifiers in the `contracts:` parsing context.

### Validation rules

Replace rules 46-47 from ALP-9 with:

46. Each entry in `contracts:` must have a direction modifier (`demands` or `fulfils`) followed by a PascalCase contract name
47. Each contract name appears at most once per surface
48. Referenced contract names must resolve to a `contract` declaration in scope (local or imported via `use`)
49. The same contract may appear with different directions in different surfaces

Note: rules 48-50 (config references) and 51-52 (config expressions) from the previous document shift to 50-54. Renumber accordingly.

### Error codes

Replace E2-E5 with:

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E2 | Entry in `contracts:` missing direction modifier | "Contract reference 'Codec' needs a direction: use `demands Codec` or `fulfils Codec`." |
| E3 | Same contract name appears twice in one surface | "Contract 'Codec' is already referenced in this surface. Each contract may appear at most once per surface." |
| E4 | Contract name does not resolve to a `contract` declaration | "No contract named 'Codec' found. Declare it as `contract Codec { ... }` at module level, or import it via `use`." |
| E5 | `contracts:` entry uses an unrecognised direction modifier | "Unknown direction 'requires' in contracts clause. Use `demands` or `fulfils`." |

### Diagnostic for migration

When the parser encounters the removed `expects` or `offers` tokens, it should emit a helpful error rather than a generic parse failure:

| Trigger | Diagnostic |
|---------|------------|
| `expects` as a keyword in a surface body | "The `expects` keyword was removed. Declare the contract at module level and reference it via `contracts: demands ContractName`." |
| `offers` as a keyword in a surface body | "The `offers` keyword was removed. Declare the contract at module level and reference it via `contracts: fulfils ContractName`." |

To support these diagnostics, keep `expects` and `offers` in `classify_keyword()` mapped to a `TokenKind::Removed` or similar sentinel, rather than removing them entirely. The parser can then check for the sentinel and emit the migration diagnostic.

### Test scenarios

**Should parse:**
- Surface with `contracts:` clause containing one `demands` entry
- Surface with `contracts:` clause containing one `fulfils` entry
- Surface with `contracts:` clause containing both `demands` and `fulfils` entries
- Surface with `contracts:`, `exposes:` and `provides:` together
- Surface with `contracts:` only (purely programmatic boundary)

**Should reject:**
- `contracts:` entry without direction modifier (E2)
- `contracts:` entry with unknown direction modifier (E5)
- Duplicate contract name in same surface (E3)
- Reference to undeclared contract (E4)
- `expects` keyword in surface body (migration diagnostic)
- `offers` keyword in surface body (migration diagnostic)
- `contracts:` entry with inline braced block (`contracts: demands Foo { ... }`)

**Edge cases:**
- `contracts:` clause with a single entry (no indentation ambiguity)
- Contract name that matches an entity name (valid, different namespaces)
- Empty `contracts:` clause (should reject; a contracts clause with no entries is meaningless)

---

## 2. Annotation sigil for prose constructs (ALP-16)

ALP-16 replaces the colon-delimited prose constructs (`invariant:`, `guidance:`, `guarantee:`) with `@`-prefixed annotations.

### Removals

**Clause keywords to remove from `is_clause_keyword()`:**
- `"invariant"` — but only the clause keyword registration. The `invariant` token itself remains for expression-bearing invariants (`invariant Name { expr }`).
- `"guidance"`
- `"guarantee"`

**Parser logic to remove:**
- The `invariant:` colon-form branch in block item parsing. The expression-bearing branch (`invariant Name { expr }`) stays.
- `validate_guidance_ordering()` — replaced by annotation ordering rules.

### Additions

**New token:**
- `TokenKind::At` for the `@` character (lexer.rs)

**New AST nodes:**

```rust
/// Prose annotation kinds
enum AnnotationKind {
    Invariant,
    Guidance,
    Guarantee,
}

/// A prose annotation: @invariant Name, @guidance, @guarantee Name
struct Annotation {
    kind: AnnotationKind,
    name: Option<Ident>,  // required for Invariant and Guarantee, absent for Guidance
    body: Vec<String>,    // indented comment lines
    span: Span,
}

/// In BlockItemKind enum:
Annotation(Annotation),
```

**Parsing logic:**

When the parser encounters `@`, it reads the next token as the annotation keyword (`invariant`, `guidance` or `guarantee`). The keyword is not a reserved token in this context; it follows `@` and is recognised contextually.

```
@invariant PascalCaseName
    -- indented comment line 1
    -- indented comment line 2

@guidance
    -- indented comment line

@guarantee PascalCaseName
    -- indented comment line
```

Rules:
1. `@invariant` and `@guarantee` require a PascalCase name after the keyword.
2. `@guidance` must not have a name.
3. The annotation body is one or more comment lines indented relative to the `@` sigil.
4. Unindented comment lines after an annotation are not part of the annotation body.

The parser must track indentation depth to determine where the annotation body ends. This is the same indentation-scoping mechanism used for `ensures:` and `exposes:` blocks, not a new parsing strategy.

### Where annotations appear

| Annotation | Valid contexts |
|------------|---------------|
| `@invariant` | contract bodies |
| `@guidance` | contract bodies, rule bodies, surface bodies |
| `@guarantee` | surface bodies |

### Ordering rules

Within any construct:
1. All structural clauses come first (typed signatures in contracts; `when`, `requires`, `ensures`, `let`, `for` in rules; `facing`, `context`, `exposes`, `provides`, `contracts:`, `related`, `timeout` in surfaces).
2. `@invariant` and `@guarantee` annotations may appear in any order relative to each other, but after all structural clauses.
3. `@guidance` must appear last, after all other annotations.

### Invariant form distinction

The parser must now distinguish three cases when it encounters `invariant`:

| Token sequence | Form | Action |
|----------------|------|--------|
| `@ invariant Name` | Prose annotation | Parse as `Annotation { kind: Invariant, name, body }` |
| `invariant Name {` | Expression-bearing (entity-level or top-level) | Parse as `InvariantBlock { name, body }` (unchanged) |
| `invariant Name {` at declaration level | Top-level declaration | Parse as `Decl::Invariant` (unchanged) |

The old `invariant: Name` (colon form) no longer parses. Emit a migration diagnostic.

### Validation rules

Replace rules 60-61 (ALP-7 guidance) with:

58. `@invariant` requires a PascalCase name; names must be unique within their containing construct (contract or surface)
59. `@guarantee` requires a PascalCase name; names must be unique within their surface
60. `@guidance` must not have a name; must appear after all structural clauses and after all other annotations
61. Annotations must be followed by at least one indented comment line
62. Within a construct, `@invariant` and `@guarantee` annotations may appear in any order but must appear after all structural clauses; `@guidance` must appear last

Contract body validity rule (43 from ALP-9) becomes:

43. Contract bodies may contain only typed signatures and annotations (`@invariant`, `@guidance`)

### Error codes

| Code | Trigger | Diagnostic |
|------|---------|------------|
| E13 | `@invariant` or `@guarantee` with lowercase name | "Annotation names must be PascalCase. Did you mean `@invariant CamelCaseName`?" |
| E14 | `@guidance` followed by a name | "`@guidance` does not take a name. Remove the name after `@guidance`." |
| E15 | Annotation with no comment body | "Annotations must be followed by at least one indented comment line." |
| E16 | `@invariant` or `@guarantee` with duplicate name in scope | "Annotation 'Determinism' is already declared in this contract. Annotation names must be unique within their containing construct." |
| E17 | `@guidance` not in final position | "`@guidance` must appear after all other clauses and annotations." |

### Diagnostics for migration

| Trigger | Diagnostic |
|---------|------------|
| `invariant:` (colon form) inside a contract body | "`invariant:` syntax was replaced by `@invariant`. Use `@invariant Name` followed by indented comment lines." |
| `guidance:` inside a rule, contract or surface body | "`guidance:` syntax was replaced by `@guidance`. Use `@guidance` followed by indented comment lines." |
| `guarantee:` inside a surface body | "`guarantee:` syntax was replaced by `@guarantee`. Use `@guarantee Name` followed by indented comment lines." |

To support these diagnostics, the parser should recognise the old colon forms and emit the migration message rather than a generic parse error. Since `invariant`, `guidance` and `guarantee` remain in the keyword table (invariant for expression-bearing forms; the other two can be kept as removed-keyword sentinels), the parser can check for the colon after these keywords and branch to the diagnostic.

### Test scenarios

**Should parse:**
- Contract with `@invariant Name` and indented comment body
- Contract with multiple `@invariant` annotations
- Contract with `@invariant` followed by `@guidance`
- Rule with `@guidance` as final element
- Surface with `@guarantee Name` and indented comment body
- Surface with `@guarantee` followed by `@guidance`
- Surface with `contracts:`, `@guarantee` and `@guidance` together
- Annotation body with multiple comment lines
- Annotation body with blank lines between comments (indented blank lines are part of body)

**Should reject:**
- `@invariant` with lowercase name (E13)
- `@guidance` with a name (E14)
- `@invariant` with no comment body (E15)
- Duplicate `@invariant` names in the same contract (E16)
- `@guidance` before `@invariant` in the same construct (E17)
- `@guarantee` in a contract body (wrong context)
- `@invariant` in a rule body (wrong context)
- Old `invariant:` colon form (migration diagnostic)
- Old `guidance:` colon form (migration diagnostic)
- Old `guarantee:` colon form (migration diagnostic)
- `@` followed by an unrecognised keyword (`@note`, `@warning`)

**Edge cases:**
- `@invariant` in a contract vs `invariant Name { expr }` at top level: both valid, syntactically disjoint
- Surface containing both `@guarantee` and expression-bearing `invariant Name { expr }` (valid, different constructs)
- Annotation immediately followed by another annotation (no blank line between)
- Annotation at end of file with no trailing newline

---

## Validation rule renumbering

The complete validation rule ranges after ALP-14 through ALP-16:

| Range | Topic | ALP |
|-------|-------|-----|
| 1-42 | Existing rules | — |
| 43 | Contract body contents (updated) | ALP-9, ALP-16 |
| 44-45 | Contract naming and uniqueness | ALP-9 |
| 46-49 | Contract references in surfaces | ALP-15 |
| 50-52 | Config reference validity | ALP-10 |
| 53-54 | Config expression validity | ALP-13 |
| 55-57 | Invariant expression validity | ALP-11 |
| 58-62 | Annotation validity | ALP-16 |

## Error catalogue additions

| Code | Topic | ALP |
|------|-------|-----|
| E2 | Missing direction modifier in contracts clause | ALP-15 |
| E3 | Duplicate contract reference in surface | ALP-15 |
| E4 | Unresolved contract reference | ALP-15 |
| E5 | Unknown direction modifier | ALP-15 |
| E13 | Annotation name not PascalCase | ALP-16 |
| E14 | `@guidance` with unexpected name | ALP-16 |
| E15 | Annotation with empty body | ALP-16 |
| E16 | Duplicate annotation name in scope | ALP-16 |
| E17 | `@guidance` not in final position | ALP-16 |

## AST changes summary

**Removed:**
- `BlockItemKind::Expects`
- `BlockItemKind::Offers`

**Added:**
- `ContractDirection` enum (`Demands`, `Fulfils`)
- `ContractBinding` struct (`direction`, `name`, `span`)
- `BlockItemKind::ContractsClause { entries: Vec<ContractBinding> }`
- `AnnotationKind` enum (`Invariant`, `Guidance`, `Guarantee`)
- `Annotation` struct (`kind`, `name`, `body`, `span`)
- `BlockItemKind::Annotation(Annotation)`

**Modified:**
- `is_clause_keyword()`: remove `invariant`, `guidance`, `guarantee`; add `contracts`
- `classify_keyword()`: remove or sentinel `expects`, `offers`; keep `invariant` for expression-bearing form
