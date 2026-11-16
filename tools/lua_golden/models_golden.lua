-- Generates golden model output by running CardTable's original Lua models over fixture inputs.
--
-- Usage (from the workspace root):
--   tools/lua_golden/run.sh models
-- For each `<name>.<Model>.json` in crates/gametable-core/tests/fixtures/models/inputs, this writes
-- `<name>.<Model>.compact.json` and `<name>.<Model>.indented.json` (the model re-encoded with
-- fromJson -> toJson) into ../expected, or `<name>.<Model>.error.txt` when fromJson raises.
local json = require("lib.json")
local models = require("src.core.models")

return function(fixturesDirectory, listFile)
  local inputsDirectory = fixturesDirectory .. "/inputs/"
  local expectedDirectory = fixturesDirectory .. "/expected/"
  local generated = 0
  for fileName in io.lines(listFile) do
    local baseName, modelName = fileName:match("^(.+)%.([%a]+)%.json$")
    if baseName then
      local inputFile = assert(io.open(inputsDirectory .. fileName, "rb"))
      local text = inputFile:read("*a")
      inputFile:close()
      local Model = assert(models[modelName], "unknown model " .. modelName)
      local ok, result = pcall(function() return Model.fromJson(json.decode(text)) end)
      local prefix = expectedDirectory .. baseName .. "." .. modelName
      local function writeFile(path, content)
        local f = assert(io.open(path, "wb"))
        f:write(content)
        f:close()
      end
      if ok then
        writeFile(prefix .. ".compact.json", json.encode(Model.toJson(result)))
        writeFile(prefix .. ".indented.json", json.encode(Model.toJson(result), { indent = "  " }))
      else
        local message = type(result) == "table" and (result.message or "error table") or tostring(result)
        writeFile(prefix .. ".error.txt", message)
      end
      generated = generated + 1
    end
  end
  print("generated golden output for " .. generated .. " fixtures")
end
