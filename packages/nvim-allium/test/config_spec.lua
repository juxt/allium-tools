local harness = require("harness")

harness.test("config.setup merges defaults with overrides", function()
  local config = require("allium.config")
  config.setup({
    lsp = {
      cmd = { "node", "dist/bin.js", "--stdio" },
    },
    keymaps = {
      enabled = false,
    },
  })

  assert(config.options.lsp.cmd[1] == "node", "expected custom lsp command")
  assert(config.options.lsp.filetypes[1] == "allium", "expected default filetype to remain")
  assert(config.options.keymaps.enabled == false, "expected keymaps override")
  assert(config.options.keymaps.hover == "K", "expected default hover keymap to remain")
end)
