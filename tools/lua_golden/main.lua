-- LÖVE entry point for golden-output generators. See run.sh.
-- Arguments: <generator name> <fixtures directory> <file listing input names>
function love.load(arguments)
  local generatorName, fixturesDirectory, listFile = arguments[1], arguments[2], arguments[3]
  local ok, err = pcall(function()
    local generator = require(generatorName .. "_golden")
    generator(fixturesDirectory, listFile)
  end)
  if not ok then
    io.stderr:write("golden generation failed: " .. tostring(err) .. "\n")
  end
  love.event.quit(ok and 0 or 1)
end
