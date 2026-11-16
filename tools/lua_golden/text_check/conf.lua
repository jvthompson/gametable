local scale = 1
for i, a in ipairs(arg or {}) do if a == "--scale" then scale = tonumber(arg[i + 1]) end end
TEXT_CHECK_SCALE = scale
function love.conf(t)
  t.window.width = math.floor(900 * scale)
  t.window.height = math.floor(260 * scale)
  t.window.title = "LÖVE text check"
  t.window.msaa = 4
  t.modules.audio = false
  t.modules.sound = false
end
