# Parser roadmap

## Tree-sitter grammar audit

Findings from comparing the tree-sitter grammar (`packages/tree-sitter-allium/grammar.js`) against the language reference.

### What tree-sitter handles

The grammar covers the structural skeleton well: all top-level declarations (rule, entity, external entity, value, enum, given, config, surface, actor, default, variant, deferred, use, module), comments, literal types (string with interpolation, number, boolean, null, duration), expression precedence (boolean, comparison, arithmetic, member access, optional chaining, null coalescing, lambdas, pipe/union, function calls with named arguments), block structure, clause keywords, field assignments, let bindings and open questions.

### Gaps

Constructs present in the language reference but missing or incorrectly modelled in the grammar.

**Major — core language constructs:**

1. **`for ... in ... :` iteration.** Rule-level `for` clauses, ensures-level `for` loops and surface-level `for` iterations are all absent. This is the primary iteration construct in the language.

2. **`if`/`else if`/`else` conditionals.** Both inline (`if condition: value else: value`) and block-form conditionals are missing. Used extensively in ensures clauses and let bindings.

3. **`where` filtering.** Projections (`slots where status = confirmed`), iteration filters (`for user in Users where ...`), surface context (`context assignment: SlotConfirmation where interviewer = viewer`) and actor identification all depend on `where`. Not in the grammar.

4. **`with` relationship declarations.** `slots: InterviewSlot with candidacy = this` — the `with` keyword and its predicate are not modelled. Relationship declarations parse as infix predicate expressions, losing their structural meaning.

5. **`in` / `not in` membership.** `slot in invitation.slots`, `status in {pending, submitted}` — no membership operator.

6. **`exists` / `not exists`.** Entity existence checks and removal assertions are not modelled as keywords or unary operators.

**Medium — type system and modules:**

7. **Qualified names with `/`.** `oauth/Session`, `feedback/Request.created(...)`, `oauth/config.session_duration` — the slash separator for module-qualified names is absent.

8. **Generic types `<T>`.** `Set<Skill>`, `List<Node?>` — angle bracket parameterisation is not in the expression grammar.

9. **Optional type suffix `T?`.** `locked_until: Timestamp?` — the `?` as a type modifier (distinct from `?.` navigation) is missing.

10. **Set literals `{ a, b, c }`.** Bare value sets like `status in {pending, active}` parse as block expressions where the contents fail to match any block_item rule.

**Minor:**

11. **Number literal underscores.** `100_000_000` — the number regex doesn't allow underscores.

12. **Projection arrow `->`.** `confirmations where status = confirmed -> interviewer` — the extraction operator is absent.

13. **Version marker.** `-- allium: 1` is parsed as an ordinary comment.

14. **`context` keyword.** Not a clause keyword; surface context declarations parse as field assignments, losing semantic distinctiveness.

### Implications for the Rust parser

The tree-sitter grammar is a reasonable foundation for syntax highlighting. For a typed AST that drives validation, we need more. Two approaches:

1. **Extend the tree-sitter grammar** to cover all gaps, then build the Rust AST layer on top of the complete CST.
2. **Use tree-sitter for block structure and expression precedence**, supplement with Rust parsing logic for domain-specific constructs.

Option 2 is more pragmatic. Some of Allium's constructs (indentation-significant `for`/`if` blocks, context-sensitive `with`/`where` distinction) sit awkwardly in a context-free grammar. A hand-written recursive descent parser in Rust can handle context-sensitive constructs natively, produce better error messages, and avoids the C dependency that tree-sitter introduces.

The tree-sitter grammar remains valuable for editor integrations (incremental parsing, syntax highlighting). The Rust parser serves a different purpose: producing a typed AST for validation, tooling and MCP consumption.

---

## Implementation phases

### Phase 1: Rust workspace and AST types

Set up a Rust workspace with an `allium-parser` library crate. Define the typed AST covering every construct in the language reference:

- **Top-level:** `Module` containing ordered declarations
- **Declarations:** `Entity`, `ExternalEntity`, `Value`, `Enum`, `Rule`, `Surface`, `Actor`, `Config`, `Default`, `Variant`, `Deferred`, `OpenQuestion`, `Use`, `Given`
- **Entity members:** `Field` (typed), `Relationship`, `Projection`, `DerivedValue` (including parameterised)
- **Rule clauses:** `Trigger` (seven variants: external stimulus, state transition, state becomes, temporal, derived condition, entity creation, chained), `Precondition`, `Postcondition`, `LetBinding`, `ForClause`
- **Expressions:** navigation, join lookups, collection operations, comparisons, arithmetic, boolean logic, conditionals, existence, membership, lambdas, set literals, object literals, function calls
- **Types:** `Primitive` (String, Integer, Decimal, Boolean, Timestamp, Duration), `EntityRef`, `Optional`, `Set`, `List`, `InlineEnum`, `NamedEnum`, `QualifiedRef`
- **Source spans** on every node for error reporting

The AST types are the contract between parsing and validation. Getting them right early saves rework later.

### Phase 2: Recursive descent parser

Hand-written recursive descent parser in Rust. Accepts source text, produces `Result<Module, Vec<Diagnostic>>`.

- Recover from errors where possible and report multiple errors per parse
- Source-span-annotated diagnostics with enough context for an LLM to fix the issue
- Handle all constructs in the language reference
- Start with structural parsing (declarations, block bodies) and extend inward to expressions, type annotations and clause-specific syntax

### Phase 3: Structural validation

Implement the 35 error rules and 16 warning checks from the language reference "Validation rules" section as passes over the AST:

- **Structural:** referenced entities exist, fields have types, relationships valid with backreferences, rules have triggers and ensures, trigger parameter consistency
- **State machine:** status values reachable, non-terminal states have exits, no undefined states
- **Expression:** no circular derived values, variables bound before use, type consistency, explicit lambdas, inline enum comparison prohibition
- **Sum type:** discriminator/variant consistency, type guard requirements
- **Given, config, surface validity:** binding resolution, config types and defaults, surface clause consistency

The TypeScript analyzer (`extensions/allium/src/language-tools/analyzer.ts`) already implements many of these checks and serves as a specification of the expected behaviour.

### Phase 4: CLI

A binary crate wrapping the parser library:

- `allium check <file.allium>` — parse and validate, report errors
- `allium parse <file.allium> --format json` — emit the typed AST as JSON
- Human-readable output with source annotations (via `miette` or `ariadne`)
- JSON output mode for LLM consumption
- Exit codes for CI integration

### Phase 5: MCP server

Wrap the parser as an MCP server for LLM agents:

- `check_syntax` — validate a file and return diagnostics
- `parse_ast` — return the typed AST as JSON
- `list_entities` / `list_rules` / `list_surfaces` — query the AST
- `validate_edit` — check a proposed edit against the spec

This is where the parser becomes a tool that LLMs call after editing `.allium` files to verify correctness.

### Phase 6: Distribution

- GitHub Actions cross-compilation (macOS arm64/x86_64, Linux x86_64/arm64, Windows x86_64)
- Homebrew tap for macOS and Linux
- Pre-built binaries in GitHub releases
- Debian/RPM packages if there's demand

---

## Open questions

- **Hand-written vs tree-sitter-backed parsing.** A hand-written recursive descent parser is simpler to maintain and handles context-sensitive constructs natively. Tree-sitter gives incremental parsing and error recovery for free but adds a C dependency and constrains the grammar. Current recommendation: hand-written parser for the standalone tool, tree-sitter grammar maintained separately for editor integrations.

- **Binary naming.** Should the CLI be `allium` (clean, but may conflict with future top-level tooling) or `allium-parser` (explicit)?

- **TypeScript migration path.** The TypeScript editor tooling could eventually call the Rust parser via stdio or consume a WASM build, replacing the regex-based parser. When to make that transition depends on how quickly the Rust parser reaches feature parity with the TypeScript analyzer.
