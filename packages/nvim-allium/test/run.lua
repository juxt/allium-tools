local root = vim.fn.getcwd()
vim.opt.runtimepath:prepend(root .. "/packages/nvim-allium")
package.path = table.concat({
  root .. "/packages/nvim-allium/test/?.lua",
  package.path,
}, ";")

local harness = require("harness")

require("config_spec")
require("treesitter_spec")
require("init_spec")
require("health_spec")
require("plugin_spec")

harness.run()
