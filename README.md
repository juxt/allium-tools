# Allium Tools

Allium is an LLM-native language for specifying what systems should do. You describe entities, rules, transitions and invariants; your LLM reads the spec when generating code, catches contradictions you missed and generates tests from the behavioural model.

Allium Tools is the parser, CLI, LSP server and editor integrations that make this work.

> **Pre-release notice:** This software is in a pre-release state. Expect breaking changes and validate carefully before production use.

## Why a specification language?

Within a session, meaning drifts: by prompt ten or twenty, the model is pattern-matching on its own outputs rather than the original intent. Across sessions, knowledge evaporates. Allium gives behavioural intent a durable form that persists across sessions and surfaces implications the developer never mentioned.

See the [Allium language site](https://juxt.github.io/allium/) for the full rationale, worked examples and the [v3 language guide](https://juxt.github.io/allium/v3/).

## A brief example

```allium
-- allium: 3

entity Order {
    status: pending | confirmed | shipped | delivered | cancelled
    tracking_number: String when status = shipped | delivered
    shipped_at: Timestamp when status = shipped | delivered

    transitions status {
        pending -> confirmed
        confirmed -> shipped
        shipped -> delivered
        pending -> cancelled
        confirmed -> cancelled
        terminal: delivered, cancelled
    }

    invariant NonNegativeTotal { this.total >= 0 }
}

rule ShipOrder {
    when: ShipOrder(order, tracking)
    requires: order.status = confirmed
    ensures:
        order.status = shipped
        order.tracking_number = tracking
        order.shipped_at = now
}

rule CancelOrder {
    when: CustomerCancels(order)
    requires: order.status in {pending, confirmed}
    ensures:
        order.status = cancelled
        order.cancelled_at = now
}
```

State-dependent fields (`when`), transition graphs and expression-bearing invariants are new in v3. The checker enforces that any rule transitioning into a `when` set must set the field, and that undeclared transitions have no rule that could cause them. See [what's new in v3](https://juxt.github.io/allium/v3/).

## Install the CLI

The `allium` CLI validates, parses and analyses specification files.

**Homebrew**

```bash
brew tap juxt/allium && brew install allium
```

**Cargo**

```bash
cargo install allium-cli
```

Pre-built binaries for Linux and macOS are available on the [releases page](https://github.com/juxt/allium-tools/releases).

### Commands

- `allium check` — validate specifications and report diagnostics.
- `allium analyse` — run structural checks plus process-level analysis: data flow, reachability, deadlock and conflict detection, invariant verification.
- `allium parse` — parse a file and output the syntax tree.
- `allium plan` — derive test obligations from a specification.
- `allium model` — extract the domain model as structured data.

With the CLI installed, your LLM validates every `.allium` file after writing or editing it, catching structural errors before they accumulate.

## Editor support

| Feature | VS Code | Neovim | Emacs | CLI |
| :--- | :---: | :---: | :---: | :---: |
| Diagnostics | ✅ | ✅ | ✅ | ✅ |
| Hover | ✅ | ✅ | ✅ | - |
| Go to definition | ✅ | ✅ | ✅ | - |
| Find references | ✅ | ✅ | ✅ | - |
| Rename | ✅ | ✅ | ✅ | - |
| Autofix | ✅ | ✅ | ✅ | ✅ |
| Formatting | ✅ | ✅ | ✅ | ✅ |
| Semantic highlighting | ✅ | ✅ | ✅ | - |
| Folding | ✅ | ✅ | ✅ | - |
| Completions | ✅ | ✅ | ✅ | - |
| Document links | ✅ | ✅ | ✅ | - |
| Code lens | ✅ | - | - | - |
| Rule simulation | ✅ | - | - | - |
| Test scaffold | ✅ | - | - | - |
| Diagram preview | ✅ | - | - | ✅ |

### VS Code

Download the latest `.vsix` from [releases](https://github.com/juxt/allium-tools/releases) and install via Command Palette: `Extensions: Install from VSIX...`. See the [VS Code setup guide](docs/editors/vscode.md).

### Neovim

Install [nvim-allium](https://github.com/juxt/nvim-allium) with your plugin manager. It handles LSP, tree-sitter highlighting and filetype detection.

### Emacs

Install [allium-mode](https://github.com/juxt/allium-mode) via `straight.el`, Doom or manually. It provides syntax highlighting, indentation and LSP integration through `eglot` or `lsp-mode`. See the [Emacs setup guide](docs/editors/emacs.md).

## Documentation

- [Allium language](https://juxt.github.io/allium/) — rationale, usage examples, language reference
- [Architecture overview](docs/project/architecture.md)
- [Editor setup guides](docs/editors/)
- [Contributing guide](CONTRIBUTING.md)

## Development

See [AGENTS.md](AGENTS.md) for development guidance and the [project roadmap](docs/project/plan.md) for priorities.

```bash
cargo build              # build the CLI and parser
cargo test               # run Rust tests
npm install              # install Node dependencies (LSP, VS Code extension)
npm run build            # build Node workspaces
npm run test             # run Node tests
```

## Licence

[MIT](LICENSE)
