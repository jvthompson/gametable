-- Prints LÖVE's font metrics, widths, wraps and glyph data as one JSON-ish line per case.
local fonts = { "Roboto-Regular.ttf", "Roboto-Medium.ttf", "Roboto-Bold.ttf", "MaterialIcons-Regular.ttf", "NotoSansSymbols2-Regular.ttf" }
local sizes = { 10, 11, 12, 13, 14, 16, 18, 20, 22, 24, 28, 32 }
local scales = { 1, 1.25, 1.5, 1.75, 2 }
local samples = { "Hello, World!", "CardTable", "The quick brown fox jumps over the lazy dog", "Ace of Spades \226\153\160", "iiiWWW 0123456789" }
local wrapText = "The quick brown fox jumps over the lazy dog and keeps running far away"
local out = {}
local function p(s) out[#out + 1] = s end
for _, file in ipairs(fonts) do
  for _, size in ipairs(sizes) do
    for _, scale in ipairs(scales) do
      local f = love.graphics.newFont(file, size, "normal", scale)
      -- Like theme.font: the Roboto faces fall back to Noto Sans Symbols 2.
      if file:find("^Roboto") then f:setFallbacks(love.graphics.newFont("NotoSansSymbols2-Regular.ttf", size, "normal", scale)) end
      local widths = {}
      for _, s in ipairs(samples) do widths[#widths + 1] = f:getWidth(s) end
      local wraps = {}
      for _, limit in ipairs({ 60, 120, 200 }) do
        local mw, lines = f:getWrap(wrapText, limit)
        wraps[#wraps + 1] = mw .. ":" .. #lines .. ":" .. table.concat(lines, "|")
      end
      p(string.format("F %s %d %g height=%d ascent=%d baseline=%g widths=%s wraps=%s", file, size, scale,
        f:getHeight(), f:getAscent(), f:getBaseline(), table.concat(widths, ","), table.concat(wraps, "#")))
      if scale == 1.5 and (size == 14 or size == 24) then
        local r = love.font.newTrueTypeRasterizer(file, size, "normal", scale)
        for _, ch in ipairs({ "A", "g", "W", " ", ".", "\226\153\160" }) do
          local gd = r:getGlyphData(ch)
          p(string.format("G %s %d %g %q adv=%d bx=%d by=%d w=%d h=%d", file, size, scale, ch,
            gd:getAdvance(), gd:getBearing(), select(2, gd:getBearing()), gd:getWidth(), gd:getHeight()))
        end
      end
    end
  end
end
love.filesystem.write("golden.txt", table.concat(out, "\n") .. "\n")
print(love.filesystem.getSaveDirectory())
love.event.quit()
