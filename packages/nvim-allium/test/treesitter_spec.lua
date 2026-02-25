local harness = require("harness")

local function clear_treesitter_modules()
  package.loaded["allium.treesitter"] = nil
  package.loaded["nvim-treesitter.parsers"] = nil
  package.preload["nvim-treesitter.parsers"] = nil
end

harness.test("treesitter.setup registers parser via get_parser_configs API", function()
  clear_treesitter_modules()
  local parser_configs = {}
  package.preload["nvim-treesitter.parsers"] = function()
    return {
      get_parser_configs = function()
        return parser_configs
      end,
    }
  end

  require("allium.treesitter").setup()

  assert(type(parser_configs.allium) == "table", "expected allium parser config")
  assert(parser_configs.allium.filetype == "allium", "expected allium filetype mapping")
  assert(
    parser_configs.allium.install_info.url:match("/packages/tree%-sitter%-allium$"),
    "expected local grammar path under packages/tree-sitter-allium"
  )
end)

harness.test("treesitter.setup does not override existing parser config", function()
  clear_treesitter_modules()
  local parser_configs = {
    allium = {
      filetype = "allium",
      install_info = { url = "/custom/parser/path" },
    },
  }

  package.preload["nvim-treesitter.parsers"] = function()
    return {
      get_parser_configs = function()
        return parser_configs
      end,
    }
  end

  require("allium.treesitter").setup()
  assert(parser_configs.allium.install_info.url == "/custom/parser/path", "expected existing parser config unchanged")
end)

harness.test("treesitter.setup supports new parser table API", function()
  clear_treesitter_modules()
  local parser_module = {}
  package.preload["nvim-treesitter.parsers"] = function()
    return parser_module
  end

  require("allium.treesitter").setup()
  assert(type(parser_module.allium) == "table", "expected parser config on parser module table")
end)

harness.test("treesitter.setup returns cleanly when nvim-treesitter.parsers is unavailable", function()
  clear_treesitter_modules()
  local ok, err = pcall(function()
    require("allium.treesitter").setup()
  end)
  assert(ok, err)
end)

harness.test("treesitter.setup returns cleanly when parser config container is invalid", function()
  clear_treesitter_modules()
  local parser_module = {
    get_parser_configs = function()
      return "invalid"
    end,
  }
  package.preload["nvim-treesitter.parsers"] = function()
    return parser_module
  end

  local ok, err = pcall(function()
    require("allium.treesitter").setup()
  end)
  assert(ok, err)
  assert(parser_module.allium == nil, "expected no parser config when container is invalid")
end)
