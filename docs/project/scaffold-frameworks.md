# Test scaffold frameworks

The test scaffold command generates test boilerplate from rule declarations in an Allium spec. The output language and test runner depend on a framework configured in `allium.config.json`.

## Configuration

Add a `scaffold` key to your project's `allium.config.json`:

```json
{
  "scaffold": {
    "framework": "vitest"
  }
}
```

Without this key the command will prompt you to configure one.

## Built-in frameworks

| ID | Language | Test runner |
| :--- | :--- | :--- |
| `node:test` | TypeScript | Node.js built-in test runner |
| `jest` | TypeScript | Jest (globals, no import needed) |
| `vitest` | TypeScript | Vitest |
| `pytest` | Python | pytest |
| `junit5` | Java | JUnit 5 |
| `clojure.test` | Clojure | clojure.test |

The output is a scaffold, not a complete test file. You will still need to import the module under test and fill in assertions. JUnit 5 output omits the enclosing class declaration since the class name and package depend on your project structure.

## Adding a framework

Framework templates live in `extensions/allium/src/language-tools/test-scaffold-frameworks.ts`. Each framework implements the `ScaffoldFramework` interface and is registered as an entry in the `frameworks` Map. The Map key is the string users set in `allium.config.json` and must match the `id` field.

```typescript
interface ScaffoldFramework {
  id: string;            // must match the Map key
  languageId: string;    // VS Code language ID for the opened document
  header: string[];      // import lines emitted once at the top of the file
  testOpen: (name: string) => string;   // opening line(s) of each test
  testClose: string;     // closing line of each test (empty string if none)
  comment: (text: string) => string;    // single-line comment syntax
  placeholder: string;   // default assertion or TODO marker
  indent: string;        // indentation within a test body
}
```

### How the generator assembles output

If the spec contains no rule declarations, the generator returns an empty string and the command shows a "no rules found" message.

Otherwise the output is assembled in order:

1. The `header` lines (imports, require statements), followed by a blank line.
2. For each rule declaration:
   - `testOpen(moduleName + " / " + ruleName)` to open the test. If this returns a string containing `\n` (as JUnit 5 does for the `@Test` annotation), the generator splits it into separate lines.
   - An indented comment for each `when`, `requires` and `ensures` clause, via `comment()`.
   - The `placeholder` line, indented.
   - `testClose`, if non-empty.
   - A trailing blank line.

### Name sanitisation

The full test name passed to `testOpen` is `"moduleName / RuleName"`. Frameworks that use the name as a string (node:test, jest, vitest) can pass it through directly. Frameworks that need a valid identifier must sanitise it.

Two helpers are exported from the frameworks module:

- `toSnakeCase(s)` splits camelCase boundaries, replaces non-alphanumeric characters with underscores, trims leading/trailing underscores and lowercases. Suitable for Python and Java identifiers.
- `toKebabCase(s)` does the same but joins with hyphens. Suitable for Clojure symbols.

### Walkthrough: adding an RSpec framework

```typescript
[
  "rspec",
  {
    id: "rspec",
    languageId: "ruby",
    header: [],
    testOpen: (name) => `it "${name}" do`,
    testClose: "end",
    comment: (text) => `# ${text}`,
    placeholder: "expect(true).to eq(true)",
    indent: "  ",
  },
],
```

RSpec uses string-based test names, so `testOpen` passes the name through without sanitisation. A framework that needs an identifier (like pytest's `def test_...`) would call `toSnakeCase(name)` instead.

After adding the entry to the `frameworks` Map:

1. Update the `frameworkIds()` assertion in `extensions/allium/test/test-scaffold.test.ts` to include `"rspec"` in the expected array.
2. Add a test case that verifies the output shape.
3. Run `npm run build` in both `extensions/allium` and `packages/allium-lsp`.
4. Run `npm test` from the workspace root.

### Known limitations

- **No structural wrapping.** The interface produces a flat sequence of test functions. Frameworks that require enclosing structure (a JUnit class, a Go test file with `package` declaration) need the user to add the wrapper. If this becomes a common need, the interface could grow optional `fileOpen`/`fileClose` callbacks.
- **Name collisions.** If two rules produce the same identifier after sanitisation (e.g. `CloseTicket` and `Close_Ticket` both becoming `close_ticket`), the scaffold will contain duplicate function names. This is rare in practice.
