local M = {}

local function check_lsp()
  local config = require("allium.config")
  local lsp_cmd = config.options.lsp and config.options.lsp.cmd and config.options.lsp.cmd[1] or "allium-lsp"
  if vim.fn.executable(lsp_cmd) == 1 then
    vim.health.ok(string.format("allium-lsp found at %s", vim.fn.exepath(lsp_cmd)))
  else
    vim.health.error("allium-lsp binary not found", "Ensure allium-lsp is in your PATH or configure lsp.cmd in setup()")
  end
end

local function check_version()
  if vim.fn.has("nvim-0.9") == 1 then
    vim.health.ok("Neovim version >= 0.9.0")
  else
    vim.health.error("Neovim version < 0.9.0", "Upgrade to Neovim 0.9.0 or newer for full Allium support")
  end
end

local function check_dependencies()
  if pcall(require, "lspconfig") then
    vim.health.ok("nvim-lspconfig available")
  else
    vim.health.error("nvim-lspconfig not found", "Install nvim-lspconfig to enable LSP features")
  end

  if pcall(require, "nvim-treesitter") then
    vim.health.ok("nvim-treesitter available")
    local ok, parsers = pcall(require, "nvim-treesitter.parsers")
    local has_parser = false
    if ok and type(parsers) == "table" then
      if type(parsers.has_parser) == "function" then
        has_parser = parsers.has_parser("allium")
      elseif type(parsers.get_parser_configs) == "function" then
        local parser_configs = parsers.get_parser_configs()
        has_parser = type(parser_configs) == "table" and parser_configs.allium ~= nil
      else
        has_parser = parsers.allium ~= nil
      end
    end

    if has_parser then
      vim.health.ok("allium tree-sitter parser installed/configured")
    else
      vim.health.warn("allium tree-sitter parser not installed", "Run :TSInstall allium")
    end
  else
    vim.health.error("nvim-treesitter not found", "Install nvim-treesitter for syntax highlighting")
  end
end

function M.check()
  vim.health.start("allium")
  check_version()
  check_lsp()
  check_dependencies()
end

return M
