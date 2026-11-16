-- Writes LÖVE 11.5's glyph advances (physical pixels) for the Roboto faces at every pixel size
-- the UI can ask for, for Latin text and common punctuation. The Rust client uses this table so
-- text widths and wrapping match LÖVE exactly (its bundled FreeType hints Roboto's advances
-- differently from SDL_ttf's).
local faces = { "Roboto-Regular.ttf", "Roboto-Medium.ttf", "Roboto-Bold.ttf" }
local codepoints = {}
for c = 32, 126 do codepoints[#codepoints + 1] = c end
for c = 160, 255 do codepoints[#codepoints + 1] = c end
for _, c in ipairs({ 0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2026, 0x2122, 0x2190, 0x2191, 0x2192, 0x2193, 0x2212 }) do
  codepoints[#codepoints + 1] = c
end
local utf8 = require("utf8")
local out = { "# LÖVE 11.5 Roboto advances: face pixel_size advance... (codepoints on the next line)" }
local cps = {}
for i, c in ipairs(codepoints) do cps[i] = string.format("%X", c) end
out[#out + 1] = "codepoints " .. table.concat(cps, ",")
for _, face in ipairs(faces) do
  for pixels = 4, 128 do
    local r = love.font.newTrueTypeRasterizer(face, pixels, "normal", 1)
    local advances = {}
    for i, c in ipairs(codepoints) do advances[i] = r:getGlyphData(utf8.char(c)):getAdvance() end
    out[#out + 1] = face .. " " .. pixels .. " " .. table.concat(advances, ",")
  end
end
love.filesystem.write("advances.txt", table.concat(out, "\n") .. "\n")
print(love.filesystem.getSaveDirectory())
love.event.quit()
