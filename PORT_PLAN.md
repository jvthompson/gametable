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
| `base64` | all | avatars, asset chunks | yes (2026-11-15), in use (server) |
| `rand` | all | shuffling, UUID v4 | yes (2026-11-15) |
| `sdl3` (gpu, ttf, image, build-from-source) | client | SDL3 bindings (fallback: `sdl3-sys` family) | yes (2026-11-15), in use |
| `pdfium-render` | client | PDF rulebooks | yes (2026-11-15) |
| `ureq` (rustls) | client | HTTPS: GitHub release check/download, ipify | yes (2026-11-15) |
| `zip` | client, server | updater extraction, admin game upload/install | yes (2026-11-15), in use (server, `deflate` only) |
| `igd-next` | client | UPnP port mapping (alternative: hand-port SSDP/SOAP) | yes (2026-11-15) |
| `libloading` | client | load `discord_partner_sdk.dll` at runtime | yes (2026-11-15) |
| `md-5` | client | avatar cache keys | yes (2026-11-15) |
| `tokio` | server | connections, per-room tasks, timers | yes (2026-11-15), in use |
| `rusqlite` (`bundled`) | server | same SQLite schema/file as the Go server | yes (2026-11-15), in use |
| `bcrypt` | server | password hashes compatible with existing DB rows | yes (2026-11-15), in use |
| `jiff` | server | server-local calendar days / ISO weeks for the token allowance (`store::local_calendar_date`) | yes (2026-11-15), in use |

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
      server; 6 unit tests. Checked both ways: the Rust server reads a Go-written `cardflux.db` (logins, admin flag,
      tokens, decks, collections, settings, chat) and the Go server reads a Rust-written one; it also opens a copy of
      `cardflux-love/server/devdata/cardflux.db`.
- [x] `games` (library scan, manifest, hash cache, built-in `standard_52`) — unit test + ignored read-only scan of
      the real library (`cargo test -p gametable-server -- --ignored real_library`, ~4.5 min) with 0 errors.
      Server card counts differ from the client's (VTES 4149 vs 8545, Lorcana 3423 vs 3433) because the server keeps
      the first card per id and skips cards Go can't unmarshal — confirmed identical to the real Go server: `gameList`
      and every manifest (12,088 files, same hashes) match for copies of `vtes` and `lorcana`.
- [x] `proto` → `connection.rs` (tokio; line framing, 64 MiB line cap, 4096-line send queue that drops slow peers,
      4 s ping / 12 s timeout, 200 ms close grace, `send_waiting` for bulk transfers). Crate is now lib + bin so
      integration tests can start a real server.
- [x] Handshake + dispatch (public / authed / admin / forward-to-room) — `server/mod.rs`. Handlers are blocking
      functions run with `block_in_place`; one state mutex like Go's `srv.mu`; clients are `ClientId`s.
- [x] Auth (register, login, resume, logout, profile, avatars, rate limiting, allowance) — `server/auth.rs`; bcrypt
      written in Go's `$2a$` format, base64 decoded like Go's `StdEncoding`.
- [x] Lobby (state pushes, chat incl. trade chat, slash commands, allowance loop) — `server/lobby.rs`.
- [x] Rooms (create/join/spectate/leave/seats/start/close, native engine per room, idle timeout, collection gate and
      `requestLoadDeckIntoZone` collection check) — `server/rooms.rs`. Each room's engine runs on its own thread
      (replaces gopher-lua VM + room goroutine; no VM pool or call timeout needed).
- [x] Decks, collection, shop (packs via the core pack generator, starter decks) — `server/decks.rs`.
- [x] Trading (invites with 30 s expiry, offers clamped to what the giver owns, confirm/execute, leave, disconnect)
      — `server/trade.rs`.
- [x] Admin (users, settings, rescan, kick, clear collection, files, game install/delete) — `server/admin.rs`,
      `server/shutdown.rs`.
- [x] Asset channel (chunked download with `send_waiting`, admin chunked upload incl. staging uploads) —
      `server/assets.rs`.
- [x] `main` (flags `--data`/`--port` in Go flag style, `server.log`, Ctrl+C) — log lines use Go's `LstdFlags` format.
- [x] Integration tests ported from `server_test.go` / `engine_test.go`: `tests/server_lobby.rs` (10),
      `tests/server_collection.rs` (Decks, CollectionMode, StarterDecks, BuiltInDecksPrefersUnderscoreFolder),
      `tests/server_admin.rs` (Admin, Assets, AdminFiles, AdminGames, Trading + chunked large download),
      `tests/server_room_engine.rs` (RoomDealDrawAndFilter, TwoRoomsShuffleDifferently, LargeGame,
      GameDataRoundTripsSameOrientationForAll, MessageRejectsDeeplyNestedJSON, RequestLoadScenarioOwnerOnly);
      OpenPack and AllowancePeriods are unit tests. The VM-pool/timeout engine tests have no Lua counterpart.
      Collection deck check and trade clamp mutation-checked.
- [x] Interop: Go server vs Rust server — the same scripted 2-player session (accounts, chat, decks, shop, admin,
      trading, rooms + engine, files, asset channel, error paths, kick, logout) gives byte-identical transcripts
      (210 lines) apart from random pack contents. Scripts: see session log.
- [x] Interop: original LÖVE client against the Rust server — checked by the user on 2026-11-15 (connects and works).

### Phase 3 — Client foundation
- [x] App shell (`src/app.lua`) — `app/mod.rs`: screen stack with push/pop/replace/popToHome commands, `Screen`
      trait, `Frame` context (draw list, fonts, images, settings, sounds, keyboard, theme), UI scale = display content
      scale (rounded to 0.01) × setting, capped by the desktop; min window size; input in logical units with LÖVE key
      names (`app/input.rs`) and LÖVE mouse button numbers; F6 native-frame toggle; borderless window with SDL hit
      test (`app/window_chrome.rs`); CLI flags `--identity`, `--dark`, `--light`, `--no-discord`, `--update-check`.
      The title bar is drawn with simple hover/click handling until the Phase 4 UI kit's icon buttons replace it.
      Dev hook: `GAMETABLE_CAPTURE=<file.png>` saves frame 30 and exits. Home is a placeholder until Phase 5.
- [x] GPU 2D renderer + shaders (rounded, greyscale, felt), stencil, scissor — `graphics/`: shapes tessellated with
      LÖVE 11's own algorithms (`tessellation.rs`), `draw_list.rs` records love.graphics-style calls,
      `gpu_renderer.rs` (SDL3 GPU, D3D12/DXIL, 4× MSAA, offscreen frame + capture). HLSL in `shaders/`, compiled by
      `tools/compile_shaders.sh` into committed `assets/shaders/*.dxil`. `examples/render_check.rs` draws every feature.
- [x] Text, fonts, icons — `graphics/text.rs` + `text_layout.rs`, `ui/theme.rs`. Matches LÖVE exactly in 300
      font/size/scale cases (`tests/love_font_metrics.rs` vs `tests/fixtures/love_fonts.txt`, regenerated by
      `tools/lua_golden/fonts/run.sh`). `examples/text_check.rs` vs `tools/lua_golden/text_check` renders side by side.
- [x] Image cache + decode workers — `graphics/image_cache.rs`, `image_data.rs` (area-average downscale, rotate,
      PNG encode), `webp.rs` (libwebp loaded at runtime like the original). Not yet exercised under eviction pressure.
- [x] Audio (`src/game/sound.lua`) — `sound.rs`, SDL3 audio streams, 4 voices per effect. Playback not yet heard
      in-app (no screen plays sounds until Phase 5/6).
- [x] `fs`, `paths`, `settings`, `profile`, `hand_settings`, `table_settings`, `playmat_settings`, `rulebook_settings`
      — `data/`. Avatar file operations (resize/save) come with the profile UI.
- [x] Legacy `cardflux` identity migration — `AppPaths::migrate_legacy_identity`.

### Phase 4 — UI kit
- [x] `ui.lua` core (ids, layers, capture, clip stack, scroll views, tooltips) — `ui/context.rs` (`Ui`), `ui/draw.rs`
      (text/icon/rect/shadow helpers and the tooltip as `Frame` methods). Overlays draw in place in a new layer
      (`Frame::overlay`); each layer has its own draw list, merged in layer order at the end of the frame.
- [x] Widgets: button, iconButton, checkbox, switch, segmented, slider, spinner, progressBar, dropdown — `ui/widgets.rs`,
      `ui/dropdown.rs`.
- [x] textField / textArea (selection, word jumps, clipboard, Tab focus, password, filter, max length) — `ui/text_field.rs`;
      clipboard behind `app::Clipboard` (SDL or in-memory).
- [x] `theme` (light/dark), `overlays` (dialogs, context menus, toasts), `widgets` — `ui/overlays.rs`, `ui/components.rs`.
      The title bar now uses the kit's icon buttons (tooltips, hover); all mouse input goes through the UI as in `app.lua`.
- [x] `colorpicker`, `filter_menu`, `card_filter_bar`, `save_name_dialog`, `panel`, `list_detail` — `ui/color_picker.rs`,
      `ui/filter_menu.rs`, `ui/card_filter_bar.rs`, `ui/save_name_dialog.rs`, `ui/panel.rs` (panel + list/detail).
- Checks: `tests/ui_kit.rs` drives real frames through `app::harness::UiHarness` (text editing, paste/filter/max length,
  Tab focus, dialogs incl. Escape/Enter/scrim/actions, modal blocking, nested menus, dropdown, wheel scrolling, app
  alerts). `screens/kit_gallery.rs` shows every widget (`GAMETABLE_SCREEN=kit_gallery`, overlays opened with
  `GAMETABLE_GALLERY_OPEN=dialog,confirm,menu,picker,save,panel`); checked by screenshot in light and dark.
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

- `PackSetting.count` may be fractional on the client (integer part guaranteed, fraction = chance of a bonus card). The Go
  server hands the set's raw `packSettings` JSON to the Lua pack generator, so fractions work there too; the Rust
  server does the same through the core pack generator.
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
- Server JSON: Go's `encoding/json` escapes `<`, `>`, `&`, U+2028 and U+2029 as `<` etc. The Rust server does
  the same for every message it builds (`connection::encode_message`) but not for engine output (`gameData`,
  `fullState`, relays — `encode_engine_message`), which Go passed through as raw JSON. Map payloads are key-sorted.
- Server JSON: Go encodes empty nil slices as `null`: a room's chat history on `roomJoin`/`roomSpectate`, a trade's
  `chat` before anyone speaks, and `gameErrors` with no scan errors. The Rust server sends `null` in the same places.
- Server: Go's `json.Unmarshal` into `int` refuses `3.0`; the Rust server accepts any whole number (the Lua client
  always writes `3`). Same for deck `quantity` validation in `deckSave` and the collection check.
- Server decks: Go stored the deck exactly as the client sent it and embedded the stored text verbatim; the Rust
  server re-encodes the parsed deck compactly (identical for the Lua client's compact output; a `null` inside a
  deck would be dropped). Purchased starter decks are re-encoded with top-level keys sorted, as Go's map did.
- Server admin paths: `\` counts as a path separator on every OS (Go only did so on Windows), and zip entries with a
  `..` component behind either slash, or a drive letter, are refused as unsafe (Go on Windows would follow
  backslashes out of the folder).
- Client fonts: LÖVE's bundled FreeType auto-hints Roboto's advances (digits mostly) differently from SDL_ttf's, so
  `assets/fonts/love_roboto_advances.txt` (LÖVE's own advances, Latin + punctuation, 4-128 px) overrides SDL_ttf's.
  The fonts have no `kern` table, so LÖVE never kerned; neither does the port. Glyph bitmaps are SDL_ttf's (a hair
  lighter than LÖVE's). Ascent rounds up, descent down, height to nearest (checked against LÖVE).
- Client rendering: LÖVE's smooth-line overdraw is reproduced from memory of `Polyline.cpp`; fills rely on 4× MSAA
  like the original. The felt noise uses `sin`-hash precision of the GPU, as before.
- Client: SDL keycodes are read unmodified (`SDL_KEYCODE_OPTIONS=unmodified,latin_letters`) so key names don't change
  with Shift, as with LÖVE's SDL2.
- UI kit: dialogs and context menus belong to the screen that shows them and report a response each frame
  (`DialogResponse`, `MenuResponse`) instead of global callbacks; the screen closes them by not showing them again.
  App-level alerts/confirmations (which outlive a screen) and snackbars live in `Ui`. Escape/Enter reach the top
  dialog or menu one frame later (by id) rather than inside the key callback.
- UI kit: a focused text field that is not drawn in a frame loses focus (the original blurred when a dialog closed;
  without this a field of a vanished dialog would keep swallowing keys).
- UI kit: wheel steps are SDL3's integer deltas (`integer_y`), matching LÖVE 11's whole-number `wheelmoved`.
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

- Shader builds for non-Windows platforms: this machine's `dxc` has no SPIR-V backend; Vulkan/Metal builds need
  SPIR-V/MSL versions of `shaders/*.hlsl` (e.g. via SDL_shadercross or a SPIR-V-enabled dxc).

- Release repository for the Rust build's self-updater (Phase 11).

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
- 2026-11-15: `jiff` approved and used for local allowance dates; `games` library scanner ported. Next: `proto`
  framing over tokio (add tokio), handshake/dispatch, auth (add bcrypt).
- 2026-11-15: Added tokio, bcrypt, base64. Ported connection framing, handshake/dispatch, auth, lobby/chat, rooms
  (native engine per room thread), shutdown and `main`; 17 server unit tests + 10 integration tests pass (two
  mutation-checked). Next: `decks.go` (decks, collection, packs, starters, collection deck check), then
  `trade.go`, `admin.go`, `admin_files.go`, `admin_games.go` (needs `zip`), `assets.go`, remaining Go tests, interop.
- 2026-11-15: Added `zip`. Ported decks/collection/shop, trading (incl. trade chat), admin (users, settings, rescan,
  files, game install/delete) and the asset channel; 16 more integration tests (36 total) and 7 more unit tests.
  Interop with the real Go server (built from `cardflux-love/server` into a scratch folder with `go build`):
  `tools/server_interop/compare_servers.py <go-server.exe> <rust-server.exe> <scratch dir>` runs one scripted
  session against both and diffs normalized transcripts — byte-identical apart from random packs after matching Go's
  HTML escaping and `null` empty slices. `swap_databases.py` (same arguments, run after) opens each server's DB with
  the other. Real-game manifests for `vtes`/`lorcana` (copied to scratch) match the Go server exactly.
  **Phase 2 is complete except a manual check with the original LÖVE client.** Next: Phase 3 (client foundation).
- 2026-11-15: User confirmed the LÖVE client works against the Rust server (Phase 2 done). Phase 3 done: data
  modules, LÖVE-exact tessellation + SDL3 GPU renderer, LÖVE-exact text, image cache with libwebp, audio, app shell
  (placeholder Home). `runtime/windows/` holds libwebp-7.dll + libsharpyuv-0.dll copied from the original build.
  Added `libloading`. Next: Phase 4, the UI kit (`ui.lua` first).
- 2026-11-16: **Phase 4 complete.** The whole UI kit is ported (see the Phase 4 list); the app shell routes input
  through it and its title bar uses icon buttons. `Frame` now carries `ui` (layers, input) and `clipboard` instead
  of a draw list. `data/deck_library.rs` has `validate_name` (rest in Phase 5). 10 interaction tests + unit tests.
  Next: Phase 5 (`game_loader`, `deck_library`, `deck_store`, `save_files`, `image_resolver`, `image_utils`,
  then Home, Settings, Controls, Support, file/game pickers, player count, practice select).
