local M = {}

local tests = {}

function M.test(name, fn)
  tests[#tests + 1] = { name = name, fn = fn }
end

local function reset_allium_modules()
  for module_name, _ in pairs(package.loaded) do
    if module_name:match("^allium") then
      package.loaded[module_name] = nil
    end
  end
end

local function run_test(test_case)
  reset_allium_modules()
  local ok, err = xpcall(test_case.fn, debug.traceback)
  if ok then
    vim.api.nvim_out_write(string.format("ok - %s\n", test_case.name))
    return true
  end

  vim.api.nvim_err_writeln(string.format("not ok - %s", test_case.name))
  vim.api.nvim_err_writeln(err)
  return false
end

function M.run()
  local failures = 0
  for _, test_case in ipairs(tests) do
    if not run_test(test_case) then
      failures = failures + 1
    end
  end

  vim.api.nvim_out_write(string.format("1..%d\n", #tests))
  if failures > 0 then
    error(string.format("%d test(s) failed", failures))
  end
end

return M
