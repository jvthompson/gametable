-- Generates golden table-action output by running CardTable's original table_actions.lua over
-- scripted action sequences.
--
-- Each input `<name>.json` is {"initialState": TableState, "steps": [{"action": name, "args": [...]}]}.
-- JSON null in args means nil. Shuffle actions take a seed in place of the rng. The output
-- `<name>.expected.txt` has one line per step: the compact TableState JSON after that step, followed
-- by `|` and the drawn ids for draw actions.
local json = require("lib.json")
local models = require("src.core.models")
local A = require("src.core.table_actions")

-- Arguments that are model structures, decoded per action.
local structuredArguments = {
  dealNewCards = function(args) args[1] = (function(list)
      local out = {}
      for i, c in ipairs(list) do out[i] = models.CardInstance.fromJson(c) end
      return out
    end)(args[1]) end,
  addWidget = function(args)
    local w = models.BoardWidgetInstance.fromJson(args[1])
    args[1] = w
  end,
  addWidgets = function(args)
    local out = {}
    for i, w in ipairs(args[1]) do out[i] = models.BoardWidgetInstance.fromJson(w) end
    args[1] = out
  end,
}

local function plainList(t)
  local out = {}
  for i, v in ipairs(t) do out[i] = v end
  return out
end

return function(fixturesDirectory, listFile)
  local generated = 0
  for fileName in io.lines(listFile) do
    local baseName = fileName:match("^(.+)%.json$")
    if baseName then
      local inputFile = assert(io.open(fixturesDirectory .. "/inputs/" .. fileName, "rb"))
      local script = json.decode(inputFile:read("*a"))
      inputFile:close()
      local state = models.TableState.fromJson(script.initialState)
      local lines = {}
      for stepIndex, step in ipairs(script.steps) do
        local argumentCount = #step.args
        local args = {}
        for i = 1, argumentCount do
          local v = step.args[i]
          if v == json.null then v = nil end
          if type(v) == "table" and json.isArray(v) and step.action ~= "dealNewCards" and step.action ~= "addWidgets" then
            v = plainList(v)
          end
          args[i] = v
        end
        if structuredArguments[step.action] then structuredArguments[step.action](args) end
        local fn = assert(A[step.action], "unknown action " .. tostring(step.action))
        local ok, newState, drawn = pcall(fn, state, unpack(args, 1, argumentCount))
        if not ok then error("step " .. stepIndex .. " (" .. step.action .. ") failed: " .. tostring(newState)) end
        state = newState
        local line = json.encode(models.TableState.toJson(state))
        if drawn ~= nil then line = line .. "|" .. json.encode(json.array(plainList(drawn))) end
        lines[#lines + 1] = line
      end
      local out = assert(io.open(fixturesDirectory .. "/expected/" .. baseName .. ".expected.txt", "wb"))
      out:write(table.concat(lines, "\n"))
      out:close()
      generated = generated + 1
    end
  end
  print("generated golden output for " .. generated .. " table action scripts")
end
