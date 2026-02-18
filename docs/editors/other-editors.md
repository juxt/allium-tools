# Generic LSP Setup for Other Editors

Allium provides a standard Language Server Protocol (LSP) server, `allium-lsp`, which can be used with any LSP-compatible editor.

## 1. Install Allium LSP Server

The LSP server binary must be available on your system.

### From Release Artifacts

1. Download the `allium-lsp-<version>.tar.gz` for your platform from [GitHub Releases](https://github.com/juxt/allium-tools/releases).
2. Extract the archive and move the `allium-lsp` binary to a directory in your `$PATH`.

### From Source

```bash
cd packages/allium-lsp
npm install
npm run build
# Link the binary to your path
ln -s $(pwd)/dist/bin.js /usr/local/bin/allium-lsp
```

## 2. Generic Configuration

The server communicates via `stdio`. Most editors require two pieces of information:

- **Command**: `allium-lsp --stdio`
- **File Association**: `*.allium`

## 3. Editor-Specific Snippets

### Helix

Add the following to your `languages.toml`:

```toml
[[language]]
name = "allium"
scope = "source.allium"
injection-regex = "allium"
file-types = ["allium"]
roots = ["allium.config.json", ".git"]
language-servers = [ "allium-lsp" ]

[language-server.allium-lsp]
command = "allium-lsp"
args = ["--stdio"]
```

### Zed

Add the following to your `settings.json`:

```json
{
  "lsp": {
    "allium-lsp": {
      "binary": {
        "path": "allium-lsp",
        "arguments": ["--stdio"]
      }
    }
  },
  "file_types": {
    "Allium": ["allium"]
  }
}
```

### Sublime Text (LSP Package)

If you use the `LSP` package, add this to `LSP.sublime-settings`:

```json
{
  "clients": {
    "allium-lsp": {
      "command": ["allium-lsp", "--stdio"],
      "enabled": true,
      "selector": "source.allium"
    }
  }
}
```

## Tree-sitter Support

Editors like Helix and Zed use Tree-sitter for highlighting. To enable this, you will need to compile the Allium grammar. See [packages/tree-sitter-allium/README.md](../../packages/tree-sitter-allium/README.md) for build instructions.
