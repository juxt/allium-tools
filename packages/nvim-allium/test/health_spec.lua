local harness = require("harness")

local function clear_health_modules()
  package.loaded["allium.health"] = nil
  package.loaded["lspconfig"] = nil
  package.loaded["nvim-treesitter"] = nil
  package.loaded["nvim-treesitter.parsers"] = nil
  package.preload["lspconfig"] = nil
  package.preload["nvim-treesitter"] = nil
  package.preload["nvim-treesitter.parsers"] = nil
end

harness.test("health.check reports healthy setup when deps and parser are available", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      has_parser = function(name)
        return name == "allium"
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health

  local records = { start = {}, ok = {}, warn = {}, error = {} }
  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function(cmd)
    if cmd == "allium-lsp" then
      return 1
    end
    return 0
  end
  vim.fn.exepath = function(cmd)
    if cmd == "allium-lsp" then
      return "/usr/bin/allium-lsp"
    end
    return ""
  end

  vim.health = {
    start = function(msg)
      records.start[#records.start + 1] = msg
    end,
    ok = function(msg)
      records.ok[#records.ok + 1] = msg
    end,
    warn = function(msg)
      records.warn[#records.warn + 1] = msg
    end,
    error = function(msg)
      records.error[#records.error + 1] = msg
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    assert(records.start[1] == "allium", "expected allium health section")
    assert(#records.ok >= 5, "expected health checks to report success")
    assert(#records.warn == 0, "expected no warnings for healthy setup")
    assert(#records.error == 0, "expected no errors for healthy setup")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check reports error when allium-lsp command is missing", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      has_parser = function()
        return true
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_health = vim.health
  local records = { error = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 0
  end
  vim.health = {
    start = function()
    end,
    ok = function()
    end,
    warn = function()
    end,
    error = function(msg)
      records.error[#records.error + 1] = msg
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    assert(#records.error >= 1, "expected an error for missing allium-lsp command")
    assert(records.error[1]:match("allium%-lsp binary not found"), "expected missing lsp binary message")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  assert(ok, err)
end)

harness.test("health.check reports error on unsupported Neovim version", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      has_parser = function()
        return true
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health
  local records = { error = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 0
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 1
  end
  vim.fn.exepath = function()
    return "/usr/bin/allium-lsp"
  end
  vim.health = {
    start = function()
    end,
    ok = function()
    end,
    warn = function()
    end,
    error = function(msg)
      records.error[#records.error + 1] = msg
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    local found = false
    for _, msg in ipairs(records.error) do
      if msg:match("Neovim version < 0%.9%.0") then
        found = true
      end
    end
    assert(found, "expected unsupported Neovim version error")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check reports missing nvim-lspconfig dependency", function()
  clear_health_modules()

  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      has_parser = function()
        return true
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health
  local records = { error = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 1
  end
  vim.fn.exepath = function()
    return "/usr/bin/allium-lsp"
  end
  vim.health = {
    start = function()
    end,
    ok = function()
    end,
    warn = function()
    end,
    error = function(msg)
      records.error[#records.error + 1] = msg
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    local found = false
    for _, msg in ipairs(records.error) do
      if msg:match("nvim%-lspconfig not found") then
        found = true
      end
    end
    assert(found, "expected nvim-lspconfig missing error")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check reports missing nvim-treesitter dependency", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health
  local records = { error = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 1
  end
  vim.fn.exepath = function()
    return "/usr/bin/allium-lsp"
  end
  vim.health = {
    start = function()
    end,
    ok = function()
    end,
    warn = function()
    end,
    error = function(msg)
      records.error[#records.error + 1] = msg
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    local found = false
    for _, msg in ipairs(records.error) do
      if msg:match("nvim%-treesitter not found") then
        found = true
      end
    end
    assert(found, "expected nvim-treesitter missing error")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check detects parser with get_parser_configs API", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      get_parser_configs = function()
        return {
          allium = {},
        }
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health
  local records = { ok = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 1
  end
  vim.fn.exepath = function()
    return "/usr/bin/allium-lsp"
  end
  vim.health = {
    start = function()
    end,
    ok = function(msg)
      records.ok[#records.ok + 1] = msg
    end,
    warn = function()
    end,
    error = function()
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    local found = false
    for _, msg in ipairs(records.ok) do
      if msg:match("tree%-sitter parser installed/configured") then
        found = true
      end
    end
    assert(found, "expected parser configured message via get_parser_configs")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check detects parser with parser table fallback API", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      allium = {},
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_exepath = vim.fn.exepath
  local original_health = vim.health
  local records = { ok = {} }

  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function()
    return 1
  end
  vim.fn.exepath = function()
    return "/usr/bin/allium-lsp"
  end
  vim.health = {
    start = function()
    end,
    ok = function(msg)
      records.ok[#records.ok + 1] = msg
    end,
    warn = function()
    end,
    error = function()
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    local found = false
    for _, msg in ipairs(records.ok) do
      if msg:match("tree%-sitter parser installed/configured") then
        found = true
      end
    end
    assert(found, "expected parser configured message via table fallback")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  vim.fn.exepath = original_exepath
  assert(ok, err)
end)

harness.test("health.check warns when parser is unavailable", function()
  clear_health_modules()

  package.preload["lspconfig"] = function()
    return {}
  end
  package.preload["nvim-treesitter"] = function()
    return {}
  end
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      has_parser = function()
        return false
      end,
    }
  end

  local original_has = vim.fn.has
  local original_executable = vim.fn.executable
  local original_health = vim.health

  local records = { warn = {} }
  vim.fn.has = function(feature)
    if feature == "nvim-0.9" then
      return 1
    end
    return original_has(feature)
  end
  vim.fn.executable = function(cmd)
    if cmd == "allium-lsp" then
      return 1
    end
    return 0
  end
  vim.health = {
    start = function()
    end,
    ok = function()
    end,
    warn = function(msg)
      records.warn[#records.warn + 1] = msg
    end,
    error = function()
    end,
  }

  local ok, err = pcall(function()
    local config = require("allium.config")
    config.setup({})
    require("allium.health").check()
    assert(#records.warn == 1, "expected exactly one warning")
    assert(records.warn[1]:match("parser not installed"), "expected parser warning message")
  end)

  vim.health = original_health
  vim.fn.has = original_has
  vim.fn.executable = original_executable
  assert(ok, err)
end)
