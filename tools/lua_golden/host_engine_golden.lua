-- Generates golden output for the session + host engine by running CardTable's original
-- session.lua and host_engine.lua over scripted multiplayer scenarios.
--
-- Determinism: uuid.v4 is replaced by a counter ("id-0001", ...), math.random by util.seededRng(seed)
-- and the clock by a script-controlled value starting at 1000 seconds.
--
-- Input `<name>.json`:
--   { "game": GameDefinition, "seed": n, "players": [PlayerInfo], "hostPlayerId": id,
--     "ownerId": id?, "deckConfigsByPlayerId": {id: DeckConfig}?, "initialRoster": [PlayerInfo],
--     "steps": [ {"from": id, "type": t, "payload": {...}} | {"roster": [PlayerInfo]}
--               | {"advanceClock": seconds} | {"tick": true} ] }
-- Output `<name>.expected.txt`: one line per outbound message, `<step>|send|<to>|<type>|<payload>` or
-- `<step>|broadcast|<except or ->|<type>|<payload>` (step 0 is engine start), then `final|<state>`.
local json = require("lib.json")
local models = require("src.core.models")
local util = require("src.core.util")
local uuid = require("src.core.uuid")

return function(fixturesDirectory, listFile)
  local generated = 0
  for fileName in io.lines(listFile) do
    local baseName = fileName:match("^(.+)%.json$")
    if baseName then
      local inputFile = assert(io.open(fixturesDirectory .. "/inputs/" .. fileName, "rb"))
      local script = json.decode(inputFile:read("*a"))
      inputFile:close()

      local idCounter = 0
      uuid.v4 = function()
        idCounter = idCounter + 1
        return string.format("id-%04d", idCounter)
      end
      local seeded = util.seededRng(script.seed)
      math.random = function(n) return seeded(n) end
      local clock = 1000
      local function now() return clock end

      package.loaded["src.game.session"] = nil
      package.loaded["src.game.host_engine"] = nil
      local Session = require("src.game.session")
      local HostEngine = require("src.game.host_engine")

      local game = models.GameDefinition.fromJson(script.game)
      local players = {}
      for i, p in ipairs(script.players) do players[i] = models.PlayerInfo.fromJson(p) end
      local decks
      if script.deckConfigsByPlayerId then
        decks = {}
        for id, d in pairs(script.deckConfigsByPlayerId) do decks[id] = models.DeckConfig.fromJson(d) end
      end
      local sharedDecks
      if script.sharedDeckConfigsByZoneId then
        sharedDecks = {}
        for id, d in pairs(script.sharedDeckConfigsByZoneId) do sharedDecks[id] = models.DeckConfig.fromJson(d) end
      end
      local session = Session.dealFromZones({ game = game, players = players, localPlayerId = script.hostPlayerId,
        deckConfigsByPlayerId = decks, sharedDeckConfigsByZoneId = sharedDecks, now = now })

      local lines = {}
      local currentStep = 0
      local roster = {}
      for i, p in ipairs(script.initialRoster) do roster[i] = models.PlayerInfo.fromJson(p) end
      local server = {}
      function server:roster() return roster end
      function server:sendTo(id, msgType, payload)
        lines[#lines + 1] = currentStep .. "|send|" .. id .. "|" .. msgType .. "|" .. json.encode(payload)
      end
      function server:broadcast(msgType, payload, exceptId)
        lines[#lines + 1] = currentStep .. "|broadcast|" .. (exceptId or "-") .. "|" .. msgType .. "|" .. json.encode(payload)
      end

      local engine = HostEngine.new(session, server, script.hostPlayerId, script.ownerId)
      engine:start()
      for stepIndex, step in ipairs(script.steps) do
        currentStep = stepIndex
        if step.roster then
          roster = {}
          for i, p in ipairs(step.roster) do roster[i] = models.PlayerInfo.fromJson(p) end
          server.onRoster(roster)
        elseif step.advanceClock then
          clock = clock + step.advanceClock
        elseif step.tick then
          session:update()
        else
          server.onIncoming(step.from, { type = step.type, payload = step.payload or json.object() })
        end
      end
      lines[#lines + 1] = "final|" .. json.encode(models.TableState.toJson(session.state))

      local out = assert(io.open(fixturesDirectory .. "/expected/" .. baseName .. ".expected.txt", "wb"))
      out:write(table.concat(lines, "\n"))
      out:close()
      generated = generated + 1
    end
  end
  print("generated golden output for " .. generated .. " host engine scripts")
end
