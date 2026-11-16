-- Re-saves every game definition, built-in deck, saved game and library deck of a CardTable library
-- with the original Lua models, writing the results under an output directory (never the library).
-- Pair with `cargo run -p gametable-core --example compat_check -- <lib> --resaved-output <dir>` and
-- diff the two output directories. Run through tools/lua_golden/resave_library.sh.
--
-- Arguments (via main.lua): <libraryRoot> <listFile>, where listFile lines are
-- "<kind>\t<relative path>\t<output root>" and kind is gamedef | deck | save.
local json = require("lib.json")
local models = require("src.core.models")

local function readFile(path)
  local f = assert(io.open(path, "rb"))
  local text = f:read("*a")
  f:close()
  return text
end

local function writeFile(path, text)
  local f = assert(io.open(path, "wb"), "cannot write " .. path)
  f:write(text)
  f:close()
end

local encoders = {
  gamedef = function(v) return json.encode(models.GameDefinition.toJson(models.GameDefinition.fromJson(v)), { indent = "  " }) end,
  save = function(v) return json.encode(models.SavedGame.toJson(models.SavedGame.fromJson(v)), { indent = "  " }) end,
  deck = function(v) return json.encode(models.DeckConfig.toJson(models.DeckConfig.fromJson(v))) end,
}

return function(libraryRoot, listFile)
  local count = 0
  for line in io.lines(listFile) do
    local kind, relative, outputRoot = line:match("^(%a+)\t(.-)\t(.+)$")
    if kind then
      local text = readFile(libraryRoot .. "/" .. relative)
      local ok, result = pcall(function() return encoders[kind](json.decode(text)) end)
      if ok then
        writeFile(outputRoot .. "/" .. relative, result)
      else
        local message = type(result) == "table" and result.message or tostring(result)
        writeFile(outputRoot .. "/" .. relative:gsub("%.[Jj][Ss][Oo][Nn]$", ".error.txt"), message)
      end
      count = count + 1
    end
  end
  print("re-saved " .. count .. " files with the original Lua models")
end
