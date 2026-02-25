local M = {}

function M.setup()
  local ok, parsers = pcall(require, "nvim-treesitter.parsers")
  if not ok then
    return
  end

  local configs
  if type(parsers.get_parser_configs) == "function" then
    configs = parsers.get_parser_configs()
  elseif type(parsers) == "table" then
    -- Newer nvim-treesitter exposes parser configs directly from this module.
    configs = parsers
  end

  if type(configs) ~= "table" then
    return
  end

  if not configs.allium then
    configs.allium = {
      install_info = {
        -- Use local path for pre-release
        url = vim.fn.fnamemodify(debug.getinfo(1).source:sub(2), ":h:h:h:h") .. "/tree-sitter-allium",
        files = { "src/parser.c" },
        branch = "main",
      },
      filetype = "allium",
    }
  end
end

return M
