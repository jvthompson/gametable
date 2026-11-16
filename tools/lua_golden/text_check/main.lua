-- Draws the same block as `cargo run -p gametable --example text_check` and saves a screenshot.
-- Usage: lovec . --scale 1.5   (writes text_check.png to the save folder and prints it)
local s = TEXT_CHECK_SCALE
local function font(file, size)
  local f = love.graphics.newFont(file, size, "normal", s)
  if file:find("^Roboto") then f:setFallbacks(love.graphics.newFont("NotoSansSymbols2-Regular.ttf", size, "normal", s)) end
  return f
end
local paragraph = "Drag cards onto the table, right-click for more options and press F1 for help."
local frames = 0
function love.draw()
  love.graphics.clear(0.973, 0.984, 0.945, 1)
  love.graphics.scale(s)
  local ink = { 0.098, 0.114, 0.09, 1 }
  love.graphics.setColor(ink)
  love.graphics.setFont(font("Roboto-Regular.ttf", 14)); love.graphics.print("The quick brown fox jumps over the lazy dog 0123456789", 10, 10)
  love.graphics.setFont(font("Roboto-Medium.ttf", 16)); love.graphics.print("CardTable \226\128\148 Ace of Spades \226\153\160 \226\153\165 \226\153\166 \226\153\163", 10, 34)
  love.graphics.setFont(font("Roboto-Bold.ttf", 20)); love.graphics.print("Bold heading 24\194\176", 10, 60)
  local utf8 = require("utf8")
  love.graphics.setFont(font("MaterialIcons-Regular.ttf", 24))
  love.graphics.print(utf8.char(0xe5c4, 0xe5cd, 0xe8b8, 0xe51c, 0xe834), 10, 90)
  love.graphics.setFont(font("Roboto-Regular.ttf", 14))
  for i, align in ipairs({ "left", "center", "right", "justify" }) do
    local x = 10 + (i - 1) * 220
    love.graphics.setColor(0.45, 0.47, 0.44, 1)
    love.graphics.rectangle("line", x, 130, 200, 110)
    love.graphics.setColor(ink)
    love.graphics.printf(paragraph, x, 130, 200, align)
  end
  frames = frames + 1
  if frames == 3 then love.graphics.captureScreenshot("text_check.png") end
  if frames == 6 then print(love.filesystem.getSaveDirectory()); love.event.quit() end
end
