# GameTable port plan and progress tracker

Rust port of CardTable (`F:\Projects\cardflux-love`, LÖVE 11.5 / Lua client + Go lobby server).
**Read this file at the start of every session, and update the checkboxes and logs as work lands.**

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done and tested

## Decisions (confirmed with the user, 2026-11-15)

| Topic | Decision |
|---|---|
| Interop | Full protocol-v7 interop: Rust client ↔ Go server / LÖVE hosts, LÖVE clients ↔ Rust server; Rust server reads an existing `cardflux.db` |
| Platforms | Windows first, portable code (SDL3 + portable crates instead of Win32 calls) |
| Integrations | Include Discord, self-updater, UPnP + public IP. Drop dev automation (`--script`, `--eval`, `--shot`, `--frames`) |
| Layout | One Cargo workspace, three crates: `gametable-core`, `gametable` (client), `gametable-server` |
| SDL3 | Use SDL3 wherever it makes sense (window, input, GPU rendering, ttf, image, audio, clipboard, hit-test) |
| Crates | Ask the user before adding any cargo crate |

## Key facts about the original

- All data files are plain JSON (plus images, Markdown, PDF). No game definition contains code; no Lua interpreter is needed.
- The server is Go (`cardflux-love/server/`, ~5.3k LOC) and runs the client's Lua rules engine in gopher-lua. It is fully
  authoritative. In Rust the rules engine is the shared `gametable-core` crate.
- Wire format: newline-delimited JSON `{"type","payload"}` over plain TCP, default port 51234, protocol version 7,
  full-state snapshots filtered per recipient (no deltas).
- No undo/redo, touch, joystick, IME composition, localization, or card-database web APIs.

## Workspace layout

```
Cargo.toml                 workspace
.cargo/config.toml         points the cmake build crate at Visual Studio's CMake
PORT_PLAN.md               this file
crates/gametable-core/     rules engine, models, byte-exact JSON, protocol framing (no SDL)
crates/gametable/          SDL3 client (binary)
crates/gametable-server/   lobby server (binary)
assets/                    fonts, audio, icons, standard_52.json, CONTROLS.md, SUPPORT-ME.md (copied from cardflux-love)
shaders/                   HLSL sources + precompiled SPIR-V/DXIL/MSL
```

## Technology choices

- **Platform layer:** SDL3 for window, input, audio, clipboard, open-URL, threads. Borderless chrome via `SDL_SetWindowHitTest`.
- **Save directory:** same as the original (`%APPDATA%\cardtable` on Windows), same `settings.json` and cache layout.
- **Rendering:** SDL3 GPU API with a small 2D batch renderer: SDF shader for rounded rects/circles/arcs, tessellated
  lines/polygons, textured quads with rounded-corner discard, greyscale shader, procedural felt shader, stencil clipping, MSAA 4,
  mipmaps + anisotropy 4 on card art.
- **Text:** SDL3_ttf (Roboto, NotoSansSymbols2 fallback, Material Icons), rasterised at physical DPI.
- **Images:** SDL3_image on a worker pool, 1024 px area-average downscale, LRU VRAM budget (~768 MB, evict to 80%, 8 uploads/frame).
- **Audio:** SDL3 audio streams, WAV, 4 voices per effect.
- **PDF:** pdfium (same `pdfium.dll` the original ships).
- **Networking:** `std::net` non-blocking sockets on the client; async runtime on the server.
- **RNG:** OS-entropy-seeded RNG for shuffles, packs, bags (the original's sequence is not part of any contract).

## Crates

Nothing is added to a `Cargo.toml` until the user approves it. Record approvals here.

| Crate | Used by | Why | Approved |
|---|---|---|---|
| `serde`, `serde_json` (`preserve_order`) | all | JSON parsing; output via a custom byte-exact formatter | yes (2026-11-15) |
| `sha2` | all | asset hashes, password hash | yes (2026-11-15) |
| `base64` | all | avatars, asset chunks | yes (2026-11-15) |
| `rand` | all | shuffling, UUID v4 | yes (2026-11-15) |
| `sdl3` (gpu, ttf, image, build-from-source) | client | SDL3 bindings (fallback: `sdl3-sys` family) | yes (2026-11-15), in use |
| `pdfium-render` | client | PDF rulebooks | yes (2026-11-15) |
| `ureq` (rustls) | client | HTTPS: GitHub release check/download, ipify | yes (2026-11-15) |
| `zip` | client, server | updater extraction, admin game upload/install | yes (2026-11-15) |
| `igd-next` | client | UPnP port mapping (alternative: hand-port SSDP/SOAP) | yes (2026-11-15) |
| `libloading` | client | load `discord_partner_sdk.dll` at runtime | yes (2026-11-15) |
| `md-5` | client | avatar cache keys | yes (2026-11-15) |
| `tokio` | server | connections, per-room tasks, timers | yes (2026-11-15) |
| `rusqlite` (`bundled`) | server | same SQLite schema/file as the Go server | yes (2026-11-15) |
| `bcrypt` | server | password hashes compatible with existing DB rows | yes (2026-11-15) |

Note: the JSON module turned out simpler to write by hand (`gametable-core::json`), so `serde`/`serde_json` are approved
but not currently used.

## Compatibility contracts (must match exactly)

- **JSON writer** (`lib/json.lua`): insertion-ordered keys per model; integers as `%d` (< 2^53); other numbers as the shortest of
  `%.15g`/`%.16g`/`%.17g` that round-trips; escapes `" \ \b \f \n \r \t`, other control chars (incl. 0x7F) as lowercase `\u00xx`,
  `/` and non-ASCII raw; Dart-style 2-space indent (`"key": value`, empty `[]`/`{}` inline); no trailing newline; `\n` endings.
  Decoder drops `null` object values, strips a UTF-8 BOM, max depth 64, combines surrogate pairs.
  Indented: `gamedef.json`, saves, `settings.json` (sorted keys). Compact: decks, manifests, network lines.
- **Models** (`src/core/models.lua`): field order, defaults omitted on write, required-field errors, enum validation,
  legacy `cardBackImagePath`, `LegacyDeckFormatError`, sets-vs-flat cards, generated `__standard52_<suit>_<rank>` cards.
- **Rules** (`table_actions.lua`, `session.lua`, `host_engine.lua`): every action, revision bump points, permission checks,
  silent failures, exact English game-log strings, `state_filter` hidden-card rule (`__hidden__`), arrow 5 s expiry.
- **Protocol v7:** framing, 4 s ping / 12 s timeout, peer `hello`/`welcome`/`lobbyRosterUpdate`, server `serverInfo` + `rid`
  request/response, ~48 `request*` game messages, drag/arrow previews, asset channel (192 KiB base64 chunks, manifest hash).
- **Server:** SQLite schema, bcrypt-of-client-sha256 passwords, settings row, login rate limiting, collection mode (packs,
  starters, trades, allowance), admin handlers, room lifecycle/idle timeout, spectators (`spectator:<id>`), shutdown flow,
  `--data` / `--port`.
- **Disk layout:** game folder (`gamedef.json`, `<setId>/`, `_tokens`, `_playmats`, `_documents`, `_scenarios`,
  `_decks`|`decks`, `savegames`, `gamelogs`), deck library `<decksDir>/<gameId>/<name>.json`,
  `servers/<host_port>/games/<id>/.cardflux_manifest.json`, `settings.json` keys.

## Phases

Source paths are relative to `F:\Projects\cardflux-love`. A phase is done when its tests pass and this file is updated.

### Phase 0 — Scaffold
- [x] Update `CLAUDE.md` (session-start check, crate approval rule)
- [x] Create `PORT_PLAN.md`
- [x] Crate approval from the user (all proposed crates approved 2026-11-15)
- [x] Workspace + three crates building (stubs)
- [x] Copy assets (fonts, audio, icons, `standard_52.json`, plus `CONTROLS.md` and `SUPPORT-ME.md` for the in-app dialogs)
- [x] SDL3 spike: window + GPU + ttf + image + hit-test working on Windows (`cargo run -p gametable --example sdl3_spike`)

### Phase 1 — `gametable-core`
- [x] `json` (byte-exact encoder/decoder) + port of `tests/json_test.lua`; number golden values captured from real LuaJIT
- [x] `color`, `util`, `uuid`, `geometry`
- [x] `models` + port of `tests/models_test.lua`, plus golden fixtures checked byte-for-byte against the original Lua
      (`crates/gametable-core/tests/models_golden.rs`)
- [x] `tag_match`, `card_filters`, `stack_utils`, `seat_utils`, `deck_widget_zones`, `state_filter`, `save_remap`, `pack_generator`
      (+ ports of `tag_match_test.lua`, `stack_utils_test.lua`, `utils_test.lua` in `tests/rules_helpers.rs`)
- [x] `table_actions` + port of `tests/table_actions_test.lua`, plus an 87-step scripted golden test covering every
      action, matched against the original Lua state after every step (`tests/table_actions_golden.rs`)
- [x] `session` — verified by golden multiplayer scenarios (game-log text, dealing incl. standard deck / shared
      decks / token bags / scenarios / deck widgets, arrow timers) rather than a line-by-line port of `session_test.lua`
- [x] `host_engine` — `tests/host_engine_golden.rs` replays two scripted games (~110 requests, roster changes,
      clock/timer ticks) and matches every outbound message and the final state against the original Lua
- [x] Protocol envelope, line codec, message type constants (`protocol.rs`)
- [x] `compat-check` example (port of `tools/compat_check`, minus image decoding which needs the client):
      0 problems over the real library; `tools/lua_golden/resave_library.sh` re-saves the library with both Lua and
      Rust and diffs them — all 53 files (11 games, built-in decks, saves, library decks) byte-identical

### Phase 2 — `gametable-server`
- [ ] `proto` (framing, heartbeat, send queue limits)
- [x] `store` (schema, settings, users, sessions, decks, collections, token ledger, chat) — same schema as the Go
      server; 6 unit tests. Not yet checked against a real Go-created `cardflux.db` (do that in the interop step).
- [ ] `games` (library scan, manifest, hash cache, built-in `standard_52`)
- [ ] Handshake + dispatch (public / authed / admin / forward-to-room)
- [ ] Auth (register, login, resume, logout, profile, avatars, rate limiting, allowance)
- [ ] Lobby (state pushes, chat, slash commands)
- [ ] Rooms (create/join/spectate/leave/seats/start/close, native engine per room, idle timeout, collection gate)
- [ ] Decks, collection, shop (packs, starter decks)
- [ ] Trading
- [ ] Admin (users, settings, rescan, kick, shutdown, files, game install/delete)
- [ ] Asset channel (download, admin upload)
- [ ] `main` (flags, `server.log`, signals)
- [ ] Integration tests ported from `server_test.go` / `engine_test.go`
- [ ] Interop: original LÖVE client against the Rust server

### Phase 3 — Client foundation
- [ ] App shell (`src/app.lua`): screen stack, title bar, input routing, UI scale/DPI, F6 frame toggle, CLI flags
- [ ] GPU 2D renderer + shaders (rounded, greyscale, felt), stencil, scissor
- [ ] Text, fonts, icons (`theme.lua` font cache)
- [ ] Image cache + decode workers
- [ ] Audio (`src/game/sound.lua`)
- [ ] `fs`, `paths`, `settings`, `profile`, `hand_settings`, `table_settings`, `playmat_settings`, `rulebook_settings`
- [ ] Legacy `cardflux` identity migration

### Phase 4 — UI kit
- [ ] `ui.lua` core (ids, layers, capture, clip stack, scroll views, tooltips)
- [ ] Widgets: button, iconButton, checkbox, switch, segmented, slider, spinner, progressBar, dropdown
- [ ] textField / textArea (selection, word jumps, clipboard, Tab focus, password, filter, max length)
- [ ] `theme` (light/dark), `overlays` (dialogs, context menus, toasts), `widgets`
- [ ] `colorpicker`, `filter_menu`, `card_filter_bar`, `save_name_dialog`, `panel`, `list_detail`

### Phase 5 — Basic screens + data
- [ ] `game_loader`, `deck_library`, `deck_store` (local), `save_files`, `image_resolver`, `image_utils`
- [ ] Home (profile, join panel, recent servers, update banner)
- [ ] Settings, Controls (Markdown renderer), Support dialogs
- [ ] File picker, game picker, player count, practice select

### Phase 6 — The table
- [ ] `table_view` (camera, layout, hit-test, felt, playmats, seat mirroring)
- [ ] `card_render` (faces, backs, preview), `selection`, `card_pool_grid`
- [ ] `table_screen`: drag/drop rules, hotkeys, context menus
- [ ] Overlays: search, card library, token picker, game log, chat, game menu
- [ ] Animations (discard flight, shuffle wiggle), drag ghosts, arrows
- [ ] `controller` (Host), practice game, save game, load saved game + player remap

### Phase 7 — Peer multiplayer
- [ ] Client `protocol` connection, `host_server`, `game_client`, `controller` (Client)
- [ ] Screens: HostSetup, AssignSeats, HostGameSelect, HostGame, Join, ClientGame
- [ ] Reconnect / seat reclaim
- [ ] `netinfo` (local + public IP), UPnP
- [ ] Interop: Rust ↔ LÖVE, both host/join directions

### Phase 8 — Decks and documents
- [ ] Deck editor, deck library dialog
- [ ] Rulebook viewer (Markdown TOC, PDF worker/cache, zoom, scroll persistence)

### Phase 9 — Game definition editor
- [ ] `gamedef_ops`, entry screen
- [ ] Tabs: General, Playmats, Zones, Sets (folder import, in-process JPEG rotation), Cards, Tokens, Scenarios
- [ ] Tools dialogs (duplicate ids, merge duplicate images)

### Phase 10 — Lobby server client
- [ ] `server_client`, `asset_sync` + worker, server `deck_store`
- [ ] Screens: login, lobby, room, decks, collection, shop, trade
- [ ] Admin screens (users, settings, games upload, files)
- [ ] Interop: against the Go server and the Rust server

### Phase 11 — Integrations + packaging
- [ ] Discord (presence, OAuth, friends, invites, join prompt)
- [ ] Self-updater (release repo to confirm with the user)
- [ ] Icon/version resource, build script, CI

### Phase 12 — Parity audit
- [ ] Walk `CONTROLS.md` and `docs/*.html` feature-by-feature against the Rust build; fix gaps

## Quirks log (original behaviours to preserve or consciously handle)

- `PackSetting.count` may be fractional on the client (integer part guaranteed, fraction = chance of a bonus card); the Go
  server declares it `int`. Decide server behaviour when porting `packBuy`.
- Spectator visibility uses only `game.zones[].visibleToAll`; players use effective zones including scenario-loaded zones.
- `chatSend` never replies, even with a `rid`.
- Unknown authenticated message types are forwarded to the room engine rather than rejected.
- Game-request failures are silent; a full peer lobby closes the socket without a `disconnect` message.
- Unowned shared-zone cards and face-down table cards keep their real `definitionId` on the wire.
- JSON `null` inside an object means "absent" (`requestGiveCard.newOwnerId`, `requestSetWidgetSize.sizeOverride`).
- Go lobby messages sort keys alphabetically; engine payloads keep model field order. Interop does not depend on key order.
- The original writes `1` where the older Dart app wrote `1.0`; `compat_check` tolerates that difference.
- `docs/data.html` says game-log lines are `[mm:ss]`; the code writes `[HH:MM:SS]`. Follow the code.
- Client RNG was never seeded in the original; the port seeds from OS entropy.
- JSON: a lone surrogate unicode escape decodes to U+FFFD (Lua produced invalid UTF-8 bytes); invalid UTF-8 in input
  files is replaced with U+FFFD (Lua passed raw bytes through). Rust strings must be valid UTF-8.
- Models are typed: a field holding the wrong JSON type (e.g. a number where an id belongs) is an error, where Lua
  kept the value as-is. `quantity`, `zIndex`, `revision`, `elapsedMs` and playmat indexes must be whole numbers;
  colours must be whole numbers 0..=4294967295. The Dart app that wrote these files could never produce other types.
- `extraFields` keys come out sorted after a load/save cycle (Lua copies them into a plain table). Confirmed with
  golden output. The editor writes them in its own order.
- `table_actions`: `moveToHand` / `drawCard` / `drawFromZone` take a required owner id (Lua accepted nil, which
  left the owner unchanged; no caller passes nil). Shuffles take a `RandomIndexSource` instead of an optional seed.
- Session: listeners are replaced by `Session::notification_count()`; the host engine broadcasts when it changes
  (same effect: a method that errors after changing state does not broadcast, as before). Clock, randomness and
  instance-id generation are injected through `SessionEnvironment`.
- Host engine: malformed requests (missing/non-numeric coordinates, non-text ids, fractional counts or indexes,
  missing colours) are refused. The original sometimes half-applied them (e.g. `requestMove` without `y` moved only x).
  Real clients never send these. `requestSetCurrentPlayer` without a player id still bumps the revision, as before.
- Host engine: `gameData` sends to newly connected players and preview clears for departed players are sent in
  sorted player-id order (the original used Lua hash order). Golden tests sort within each step.
- `isoNow` (save timestamps) needs local time; the core only formats it (`format_iso_local_timestamp`) and the client
  will read local time from SDL.

## Build and tooling notes

- SDL3, SDL3_image and SDL3_ttf are built from source by the `sdl3` crate (`build-from-source`). CMake is not on PATH, so
  `.cargo/config.toml` sets `CMAKE` to the copy inside Visual Studio 18 Insiders. The first build takes about 2 minutes.
- `sdl3` crate version 0.22.1 (SDL 3.4.20, SDL_image 3.4.8, SDL_ttf 3.2.2).
- LÖVE 11.5 is installed at `C:\Program Files\LOVE`. `tools/lua_golden/run.sh <generator>` runs the original Lua code
  (copied from `cardflux-love/lib`, `src/core`, `src/game`) over `crates/gametable-core/tests/fixtures/<generator>/inputs`
  and writes `expected/`. Generators are `tools/lua_golden/<generator>_golden.lua` (so far: `models`, `table_actions`,
  `host_engine`). The `host_engine` generator pins uuid.v4 to `id-0001...`, math.random to `util.seededRng(seed)` and
  the clock to a script-controlled value; the Rust test does the same through `SessionEnvironment`. Use this for every
  engine module: add fixtures, regenerate, and make the Rust test compare against `expected/`.
- **Real game library for testing: `D:\CardfluxLibrary` (`game_library/`, `deck_library/`). READ ONLY — never modify
  anything there.** Lorcana and Middle Earth are the most complete games. Tools must write output elsewhere
  (both `compat_check --resaved-output` and `resave_library.sh` refuse output paths inside the library).
  Re-run `bash tools/lua_golden/resave_library.sh /d/CardfluxLibrary <scratch dir>` after any model/JSON change.
- `rustfmt.toml` sets `max_width = 120`; run `cargo fmt --all` before committing.

## Open questions

- Release repository for the Rust build's self-updater (Phase 11).
- Local time on the server: the daily/weekly token allowance uses server-local calendar days and ISO weeks (Go's
  `time.Local`). `std` has no time-zone support. `store::grant_allowance` takes calendar dates from the caller; the
  server's `main` needs a unix-time -> local date conversion. Options: ask the user to approve a crate (`jiff` or
  `chrono`), or use UTC days (a behaviour change). Ask before Phase 2's `main`.

## Session log

- 2026-11-15: Explored the original, agreed decisions, wrote this plan. Phase 0 started.
- 2026-11-15: All crates approved. Workspace builds; assets copied; SDL3 spike passes; Phase 0 complete.
  Phase 1 started: `gametable-core::json` done (11 tests). Next: `color`, `util`, `uuid`, `geometry`, then `models`.
- 2026-11-15: Git repo set up (LF line endings enforced). `color`, `util` (Park-Miller seeded RNG + shuffle matching
  Lua), `uuid`, `geometry` and `models` done; 40 core tests pass, including golden fixtures from the original Lua.
  Next: `tag_match`, `card_filters`, `stack_utils`, `seat_utils`, `deck_widget_zones`, `state_filter`, `save_remap`,
  `pack_generator`, then `table_actions`.
- 2026-11-15: Rules helpers and `table_actions` done; 97 core tests pass, including the step-by-step golden replay
  against the original Lua. Next: `session` (+ `session_test.lua`, needs a golden generator for game-log strings),
  then `host_engine`, protocol codec, `compat-check`.
- 2026-11-15: `session`, `host_engine` and `protocol` done; 101 core tests pass, including two golden multiplayer
  scenarios matched message for message. Next: `compat-check` example (last Phase 1 item), then Phase 2 (server).
- 2026-11-15: `compat_check` example done; the real library (`D:\CardfluxLibrary`, read-only) re-saves byte-identically
  to the original Lua (53 files). **Phase 1 complete.** Next: Phase 2, the server (start with `proto`, `store`, `games`).
- 2026-11-15: Phase 2 started: server `store` ported (rusqlite, same schema). Next: `games` (library scan, manifest,
  hash cache), then `proto` framing over tokio, then handshake/dispatch and auth.
