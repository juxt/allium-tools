# Allium Tools for VS Code

Rich language support for the **Allium** specification language, powered by the Allium Language Server.

**Compatibility:** Allium language versions 1, 2 and 3

## Features

### 🌈 Syntax Highlighting & Snippets
Intelligent colorization for all Allium constructs including v3 features: transition graphs, when-qualified fields, backtick-quoted enum literals and expression-bearing invariants. Includes snippets to quickly scaffold specifications.

### 🔍 Navigation & Discovery
- **Go to Definition**: Jump instantly to any declared symbol or imported module.
- **Find References**: See where your rules and entities are used across the entire workspace.
- **Symbol Search**: Use `Ctrl+T` to search for symbols globally or `Ctrl+Shift+O` for an outline of the current file.
- **Code Lenses**: Quick actions above declarations to find references or jump to related tests.

### 🛠 Diagnostics & Quick Fixes
Real-time feedback on your specifications. Allium catches logic errors, missing clauses, and undefined references as you type.
- **Safe Fixes**: Automated remediation for missing `ensures` blocks and temporal guards.
- **Suppression**: Easy insertion of ignore directives for specific findings.

### 📐 Advanced Specification Tools
- **Diagram Preview**: Visualise your specifications as D2 or Mermaid diagrams.
- **Rule Simulation**: Dry-run your rule logic with JSON sample data.
- **Test Scaffolding**: Automatically generate rule-test boilerplate from your specifications.

## Quick Start

1. Open an `.allium` file.
2. The extension will activate automatically.
3. Start typing! Use `Ctrl+Space` for completions.
4. Run `Ctrl+Shift+P` and type `Allium` to see all available commands.

## Configuration

The extension can be customized to fit your workflow:

| Setting | Default | Description |
| :--- | :--- | :--- |
| `allium.diagnostics.mode` | `strict` | `strict` (all checks) or `relaxed` (suppresses temporal warnings). |
| `allium.profile` | `custom` | Choose a preset profile like `legacy-migration` or `doc-writing`. |
| `allium.format.indentWidth` | `4` | Customize the indentation of your specifications. |

## Documentation

- [Full Allium Documentation](https://juxt.github.io/allium/language)
- [Architecture Overview](https://github.com/juxt/allium-tools/blob/main/docs/project/architecture.md)
- [Editor Setup Guides](https://github.com/juxt/allium-tools/tree/main/docs/editors)

---

**Allium Tools** is part of the Allium multi-editor platform. Join us on [GitHub](https://github.com/juxt/allium-tools).
