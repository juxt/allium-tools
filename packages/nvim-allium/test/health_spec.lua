local harness = require("harness")

local function clear_health_modules()
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
