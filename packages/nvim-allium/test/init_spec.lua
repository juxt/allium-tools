local harness = require("harness")

local function clear_setup_modules()
  package.loaded["lspconfig"] = nil
  package.loaded["lspconfig.configs"] = nil
  package.preload["lspconfig"] = nil
  package.preload["lspconfig.configs"] = nil
end

harness.test("setup registers allium lsp server and attaches keymaps", function()
  clear_setup_modules()

  local captured_setup_opts
  local configs = {}
  package.preload["lspconfig.configs"] = function()
    return configs
  end

  package.preload["lspconfig"] = function()
    return {
      allium = {
        setup = function(opts)
          captured_setup_opts = opts
        end,
      },
    }
  end

  local keymaps = {}
  local original_keymap_set = vim.keymap.set
  local original_set_option = vim.api.nvim_buf_set_option
  local original_treesitter_setup = require("allium.treesitter").setup

  vim.keymap.set = function(mode, lhs, rhs, opts)
    keymaps[#keymaps + 1] = { mode = mode, lhs = lhs, rhs = rhs, opts = opts }
  end

  local buf_options = {}
  vim.api.nvim_buf_set_option = function(bufnr, option, value)
    buf_options[#buf_options + 1] = { bufnr = bufnr, option = option, value = value }
  end

  local treesitter_called = false
  require("allium.treesitter").setup = function()
    treesitter_called = true
  end

  local ok, err = pcall(function()
    require("allium").setup({
      lsp = {
        cmd = { "node", "packages/allium-lsp/dist/bin.js", "--stdio" },
      },
    })

    assert(type(configs.allium) == "table", "expected allium lspconfig entry")
    assert(configs.allium.default_config.cmd[1] == "node", "expected configured cmd in server defaults")
    assert(type(captured_setup_opts) == "table", "expected lsp setup call")
    assert(type(captured_setup_opts.on_attach) == "function", "expected on_attach callback")
    assert(treesitter_called, "expected treesitter setup call")

    captured_setup_opts.on_attach({}, 17)
    assert(#keymaps == 9, "expected default LSP keymaps to be registered")
    assert(#buf_options == 2, "expected omnifunc and formatexpr to be set")
  end)

  require("allium.treesitter").setup = original_treesitter_setup
  vim.keymap.set = original_keymap_set
  vim.api.nvim_buf_set_option = original_set_option
  assert(ok, err)
end)

harness.test("setup skips keymap registration when disabled", function()
  clear_setup_modules()

  local captured_setup_opts
  package.preload["lspconfig.configs"] = function()
    return {}
  end

  package.preload["lspconfig"] = function()
    return {
      allium = {
        setup = function(opts)
          captured_setup_opts = opts
        end,
      },
    }
  end

  local keymap_calls = 0
  local original_keymap_set = vim.keymap.set
  local original_set_option = vim.api.nvim_buf_set_option
  local original_treesitter_setup = require("allium.treesitter").setup

  vim.keymap.set = function()
    keymap_calls = keymap_calls + 1
  end
  vim.api.nvim_buf_set_option = function()
  end
  require("allium.treesitter").setup = function()
  end

  local ok, err = pcall(function()
    require("allium").setup({
      keymaps = { enabled = false },
    })

    captured_setup_opts.on_attach({}, 3)
    assert(keymap_calls == 0, "expected no keymaps when keymaps.enabled is false")
  end)

  require("allium.treesitter").setup = original_treesitter_setup
  vim.keymap.set = original_keymap_set
  vim.api.nvim_buf_set_option = original_set_option
  assert(ok, err)
end)
