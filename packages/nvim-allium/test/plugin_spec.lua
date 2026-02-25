local harness = require("harness")

harness.test("plugin loader sets global loaded guard", function()
  local root = vim.fn.getcwd()
  local plugin_file = root .. "/packages/nvim-allium/plugin/allium.lua"
  local original = vim.g.loaded_allium
  vim.g.loaded_allium = nil

  local ok, err = pcall(function()
    dofile(plugin_file)
    assert(vim.g.loaded_allium == 1, "expected plugin loader to set vim.g.loaded_allium")
  end)

  vim.g.loaded_allium = original
  assert(ok, err)
end)

harness.test("plugin loader is idempotent when guard is set", function()
  local root = vim.fn.getcwd()
  local plugin_file = root .. "/packages/nvim-allium/plugin/allium.lua"
  local original = vim.g.loaded_allium
  vim.g.loaded_allium = 1

  local ok, err = pcall(function()
    dofile(plugin_file)
    assert(vim.g.loaded_allium == 1, "expected plugin loader guard to remain set")
  end)

  vim.g.loaded_allium = original
  assert(ok, err)
end)
