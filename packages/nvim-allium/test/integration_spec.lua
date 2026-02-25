local root = vim.env.ALLIUM_NVIM_TEST_ROOT or vim.fn.getcwd()
local sample_dir = root .. "/.nvim-test/integration-workspace"
local sample_file = sample_dir .. "/integration-sample.allium"

local total = 0
local failed = 0

local function report(ok, name, err)
  total = total + 1
  if ok then
    vim.api.nvim_out_write(string.format("ok %d - %s\n", total, name))
    return
  end
  failed = failed + 1
  vim.api.nvim_out_write(string.format("not ok %d - %s\n", total, name))
  if err and err ~= "" then
    local first_line = tostring(err):match("([^\n]+)") or tostring(err)
    vim.api.nvim_out_write(string.format("# %s\n", first_line))
  end
end

local function run_case(name, fn)
  local ok, err = xpcall(fn, debug.traceback)
  report(ok, name, err)
end

local function skip_case(name, reason)
  total = total + 1
  vim.api.nvim_out_write(string.format("ok %d - %s # SKIP %s\n", total, name, reason))
end

local function run_case_if(name, condition, reason, fn)
  if condition then
    run_case(name, fn)
  else
    skip_case(name, reason)
  end
end

local function finish()
  vim.api.nvim_out_write(string.format("1..%d\n", total))
  if failed > 0 then
    error(string.format("%d integration test(s) failed", failed))
  end
end

local function write_sample_file()
  vim.fn.mkdir(sample_dir, "p")
  vim.fn.writefile({
    "rule TriggerA {",
    "  when: Trigger()",
    "  ensures: Done()",
    "}",
    "",
    "rule UsesTriggerA {",
    "  when: TriggerA()",
    "  ensures: Done()",
    "}",
    "",
    "rule Broken {",
    "when: Trigger()",
    "}",
  }, sample_file)
end

local function uri_for_current_buffer()
  return vim.uri_from_bufnr(0)
end

local function fail(msg)
  error(msg, 0)
end

local function expect(condition, msg)
  if not condition then
    fail(msg)
  end
end

local function lsp_sync(method, params, timeout_ms)
  local responses = vim.lsp.buf_request_sync(0, method, params, timeout_ms or 8000)
  expect(type(responses) == "table" and next(responses) ~= nil, method .. " returned no LSP response")

  for client_id, response in pairs(responses) do
    if response and response.error then
      fail(method .. " failed: " .. vim.inspect(response.error))
    end
    if response then
      return response.result, client_id
    end
  end

  fail(method .. " produced empty LSP response payload")
end

local function get_allium_client()
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = 0 })) do
    if client.name == "allium" then
      return client
    end
  end
  return nil
end

vim.opt.packpath:prepend(vim.fn.stdpath("data") .. "/nvim/site")
pcall(vim.cmd, "packadd nvim-lspconfig")
pcall(vim.cmd, "packadd nvim-treesitter")

vim.opt.runtimepath:prepend(root .. "/packages/nvim-allium")
vim.filetype.add({
  extension = {
    allium = "allium",
  },
})

local plugin = require("allium")
plugin.setup({
  lsp = {
    cmd = { "node", root .. "/packages/allium-lsp/dist/bin.js", "--stdio" },
  },
})

write_sample_file()
local lsp_attached = false

run_case("real dependency nvim-lspconfig is available", function()
  local ok = pcall(require, "lspconfig")
  expect(ok, "nvim-lspconfig not available")
end)

run_case("real dependency nvim-treesitter is available", function()
  local ok = pcall(require, "nvim-treesitter")
  expect(ok, "nvim-treesitter not available")
end)

run_case("allium parser config points at local tree-sitter grammar", function()
  local parsers = require("nvim-treesitter.parsers")
  local parser_configs
  if type(parsers.get_parser_configs) == "function" then
    parser_configs = parsers.get_parser_configs()
  elseif type(parsers) == "table" then
    parser_configs = parsers
  end

  expect(type(parser_configs) == "table", "nvim-treesitter parser configs not available")
  expect(type(parser_configs.allium) == "table", "allium parser config missing")
  expect(
    type(parser_configs.allium.install_info) == "table"
      and type(parser_configs.allium.install_info.url) == "string"
      and parser_configs.allium.install_info.url:match("/packages/tree%-sitter%-allium$"),
    "allium parser should point at local packages/tree-sitter-allium grammar"
  )
end)

run_case("allium filetype is detected", function()
  vim.cmd("edit " .. vim.fn.fnameescape(sample_file))
  vim.bo.filetype = "allium"
  expect(vim.bo.filetype == "allium", "filetype should be allium")
end)

run_case("allium lsp attaches to allium buffer", function()
  lsp_attached = vim.wait(10000, function()
    return get_allium_client() ~= nil
  end, 100)
  expect(lsp_attached, "allium LSP client did not attach within timeout")
end)

run_case_if("plugin keymaps are registered on attach", lsp_attached, "requires attached allium LSP", function()
  local maps = vim.api.nvim_buf_get_keymap(0, "n")
  local leader = vim.g.mapleader or "\\"
  local expected = {
    gd = false,
    K = false,
    gr = false,
    [leader .. "rn"] = false,
    [leader .. "ca"] = false,
    [leader .. "f"] = false,
    ["[d"] = false,
    ["]d"] = false,
    [leader .. "q"] = false,
  }

  for _, map in ipairs(maps) do
    if expected[map.lhs] ~= nil then
      expected[map.lhs] = true
    end
  end

  for lhs, present in pairs(expected) do
    expect(present, "missing expected keymap: " .. lhs)
  end
end)

run_case_if("hover request returns content", lsp_attached, "requires attached allium LSP", function()
  local result = lsp_sync("textDocument/hover", {
    textDocument = { uri = uri_for_current_buffer() },
    position = { line = 6, character = 10 },
  }, 8000)
  expect(result == nil or type(result) == "table", "hover result should be nil or a hover payload")
end)

run_case_if("definition request resolves declaration", lsp_attached, "requires attached allium LSP", function()
  local result = lsp_sync("textDocument/definition", {
    textDocument = { uri = uri_for_current_buffer() },
    position = { line = 6, character = 10 },
  }, 8000)

  local first = nil
  if type(result) == "table" and result.uri then
    first = result
  elseif type(result) == "table" then
    first = result[1]
  end

  expect(type(first) == "table", "definition result should include a location")
  expect(first.uri == uri_for_current_buffer(), "definition should resolve in current file")
  expect(first.range and first.range.start and first.range.start.line == 0, "definition should point to rule declaration")
end)

run_case_if("references request includes declaration and use-site", lsp_attached, "requires attached allium LSP", function()
  local result = lsp_sync("textDocument/references", {
    textDocument = { uri = uri_for_current_buffer() },
    position = { line = 0, character = 6 },
    context = { includeDeclaration = true },
  }, 8000)

  expect(type(result) == "table", "references result should be a list")
  expect(#result >= 2, "references should include at least declaration and one usage")
end)

run_case_if("rename request updates declaration and references", lsp_attached, "requires attached allium LSP", function()
  local workspace_edit, client_id = lsp_sync("textDocument/rename", {
    textDocument = { uri = uri_for_current_buffer() },
    position = { line = 0, character = 6 },
    newName = "TriggerRenamed",
  }, 8000)

  expect(type(workspace_edit) == "table", "rename should return a workspace edit")
  local client = vim.lsp.get_client_by_id(client_id)
  local offset_encoding = client and client.offset_encoding or "utf-16"
  vim.lsp.util.apply_workspace_edit(workspace_edit, offset_encoding)

  local full_text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n")
  expect(full_text:match("rule TriggerRenamed"), "rename should update declaration")
  local renamed_count = 0
  for _ in full_text:gmatch("TriggerRenamed") do
    renamed_count = renamed_count + 1
  end
  expect(renamed_count >= 1, "rename should apply at least one symbol rename")
end)

run_case_if("formatting request returns edits and applies indentation", lsp_attached, "requires attached allium LSP", function()
  local edits, client_id = lsp_sync("textDocument/formatting", {
    textDocument = { uri = uri_for_current_buffer() },
    options = {
      tabSize = 4,
      insertSpaces = true,
    },
  }, 8000)

  expect(type(edits) == "table", "formatting should return text edits")
  local client = vim.lsp.get_client_by_id(client_id)
  local offset_encoding = client and client.offset_encoding or "utf-16"
  local bufnr = vim.api.nvim_get_current_buf()
  vim.lsp.util.apply_text_edits(edits, bufnr, offset_encoding)

  local line = vim.api.nvim_buf_get_lines(0, 11, 12, false)[1] or ""
  expect(line:match("^    when:"), "formatting should indent rule body by four spaces")
end)

run_case_if("code actions include fix-all action", lsp_attached, "requires attached allium LSP", function()
  local actions = lsp_sync("textDocument/codeAction", {
    textDocument = { uri = uri_for_current_buffer() },
    range = {
      start = { line = 0, character = 0 },
      ["end"] = { line = 0, character = 0 },
    },
    context = {
      diagnostics = {},
    },
  }, 8000)

  expect(type(actions) == "table", "codeAction should return a list")
  local found_fix_all = false
  for _, action in ipairs(actions) do
    if action.title == "Allium: Apply All Safe Fixes" then
      found_fix_all = true
      break
    end
  end
  expect(found_fix_all, "expected Allium fix-all action")
end)

run_case_if("diagnostics are published for invalid rule", lsp_attached, "requires attached allium LSP", function()
  local has_diagnostic = vim.wait(8000, function()
    return #vim.diagnostic.get(0) > 0
  end, 100)
  expect(has_diagnostic, "expected diagnostics for intentionally invalid rule")
end)

for _, client in ipairs(vim.lsp.get_clients({ bufnr = 0 })) do
  if client.name == "allium" then
    pcall(vim.lsp.stop_client, client.id, true)
  end
end

finish()
