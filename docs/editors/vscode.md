# VS Code Setup Guide for Allium

The Allium VS Code extension provides a rich development environment for the Allium specification language, powered by the Allium Language Server.

## Installation

### From Marketplace (Coming Soon)

Search for "Allium" in the VS Code Extensions view and click **Install**.

### From Release Artifacts

1. Download the `allium-vscode-<version>.vsix` file from the [GitHub Releases](https://github.com/juxt/allium-tools/releases) page.
2. In VS Code, open the Extensions view (`Ctrl+Shift+X`).
3. Click the **...** (Views and More Actions) menu in the top-right corner.
4. Select **Install from VSIX...**.
5. Choose the downloaded `.vsix` file.

## Features

### Language Support

- **Syntax Highlighting**: Rich coloring for Allium constructs using Tree-sitter.
- **Snippets**: Quick scaffolds for `rule`, `entity`, `enum`, etc.
- **Formatting**: Automated document formatting via `Ctrl+Shift+I`.

### Navigation & Discovery

- **Go to Definition**: Jump to the declaration of any symbol.
- **Find References**: List all usages of a symbol across your workspace.
- **Document Symbols**: Navigate your file using the Outline view.
- **Workspace Symbols**: Search for any declared symbol using `Ctrl+T`.
- **Code Lenses**: Quick links above declarations to find references or see test coverage.

### Diagnostics & Quick Fixes

The extension identifies common errors and provides automated fixes:

- Missing `ensures:` clauses.
- Undefined configuration references.
- Circular dependencies in derived values.
- **Quick Fixes**: Click the lightbulb icon or press `Ctrl+.` to apply suggested fixes.

### Refactorings

- **Rename**: Safely rename symbols across your entire workspace (`F2`).
- **Extract Literal**: Convert hardcoded strings or numbers into `config` keys.
- **Extract Enum**: Move inline enum fields to top-level `enum` declarations.

### Advanced Tools

- **Diagram Preview**: Generate visual representations of your specifications. Run `Allium: Generate Diagram`.
- **Rule Simulation**: Test rule logic with sample data. Run `Allium: Preview Rule Simulation`.
- **Test Scaffold**: Automatically generate boilerplate for rule tests. Run `Allium: Generate Rule Test Scaffold`.

## Configuration

| Setting | Default | Description |
| :--- | :--- | :--- |
| `allium.diagnostics.mode` | `strict` | `strict` or `relaxed` validation. |
| `allium.profile` | `custom` | Preset profiles (e.g., `strict-authoring`, `legacy-migration`). |
| `allium.format.indentWidth` | `4` | Number of spaces for indentation. |

## Troubleshooting

- **Activation**: Ensure you have an `.allium` file open. The extension activates on the `allium` language ID.
- **Output Channel**: Check the `Allium Language Server` channel in the **Output** tab for logs and errors.
- **Re-check**: If diagnostics seem stale, run the `Allium: Run Checks` command.
