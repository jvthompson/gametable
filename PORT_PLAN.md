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
| Name (2026-11-17) | The app is **GameTable** everywhere a user sees it (window, dialogs, executable `GameTable.exe` / `GameTable.app` / `gametable`, release files `gametable-<platform>.zip`, CONTROLS.md, Discord invite text, UPnP mapping, the Rust server's default name). The save folder moves to `%APPDATA%\gametable`; the first launch copies `cardtable` (else `cardflux`) in and repoints the avatar. Kept for compatibility: the `cardflux:` password-hash prefix, `.cardflux_manifest.json`, `cardflux.db`. Installs of `v0.2.0+39` (CardTable names) don't auto-update; they download once by hand |

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
| `sha2` | all | asset hashes, password hash | yes (2026-11-15), in use (client: password hash, asset manifests) |
| `base64` | all | avatars, asset chunks | yes (2026-11-15), in use (server) |
| `rand` | all | shuffling, UUID v4 | yes (2026-11-15) |
| `sdl3` (gpu, ttf, image, build-from-source-static) | client | SDL3 bindings (fallback: `sdl3-sys` family) | yes (2026-11-15), in use (static since Phase 10b) |
| `pdfium-render` | client | PDF rulebooks | yes (2026-11-15), not used: `pdfium.dll` is bound directly with `libloading` (14 functions, as the original's FFI) |
| `ureq` (rustls) | client | HTTPS: GitHub release check/download, ipify | yes (2026-11-15), in use (updater) |
| `zip` | client, server, core | updater extraction, admin game upload/install, `.gamedef` packages | yes (2026-11-15), in use (server and client admin zip check, `deflate` only); added to `gametable-core` 2026-11-21 for the shared `.gamedef` reader |
| `igd-next` | client | UPnP port mapping (alternative: hand-port SSDP/SOAP) | yes (2026-11-15) |
| `libloading` | client | load `discord_partner_sdk.dll` at runtime | yes (2026-11-15) |
| `md-5` | client | avatar cache keys | yes (2026-11-15) |
| `tokio` | server | connections, per-room tasks, timers | yes (2026-11-15), in use |
| `rusqlite` (`bundled`) | server | same SQLite schema/file as the Go server | yes (2026-11-15), in use |
| `bcrypt` | server | password hashes compatible with existing DB rows | yes (2026-11-15), in use |
| `jiff` | server | server-local calendar days / ISO weeks for the token allowance (`store::local_calendar_date`) | yes (2026-11-15), in use |
| `ocrs`, `rten` | client | Card View "Suggest Title → Image (local)": offline text recognition (pure Rust; models downloaded on first use) | yes (2026-11-16), in use |

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
- [x] `proto` (framing, heartbeat, send queue limits) — `connection.rs` (one JSON message per line, bounded send
      queue, heartbeat pings)
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
- [x] `game_loader`, `deck_library`, `deck_store` (local), `save_files`, `image_resolver`, `image_utils` — `data/`.
      `fs.join`/`dirname`/`isSamePath` are string helpers in `data/file_system.rs` (paths in models stay text, as in
      Lua). `deck_store.rs` has the `DeckStore` trait (callback API like the original) with the local store; the
      server store comes with Phase 10. `image_utils` was already `graphics/image_data.rs` + `webp.rs`; only
      `rotateLandscapeImages` (JPEG, editor import) is left for Phase 9. Local time for saves/logs comes from SDL
      (`save_files::local_now`). The real library (11 games + Standard 52) loads with no errors
      (`GAMETABLE_LIBRARY=<folder> cargo test -p gametable --lib real_library -- --ignored --nocapture`).
- [x] Home (profile, join panel, recent servers popup, reconnect banner, wide/narrow layouts, corner tools, build
      label) — `screens/home.rs`; Edit Player Settings (avatar resize to 256 px PNG) — `screens/edit_player_settings.rs`.
      The update banner is wired but stays hidden until the updater (Phase 11). Join/Host/Deck Editor/Game
      Definition Editor open a `NotPortedYet` stand-in naming the phase that brings them.
- [x] Settings, Controls (Markdown renderer `ui/markdown.rs`, shared with the rulebook later), Support dialogs —
      `screens/settings_dialog.rs`, `screens/info_dialogs.rs`. `CONTROLS.md` / `SUPPORT-ME.md` are copied into
      `assets/` (the original packed them into game.love).
- [x] File picker, game picker (+ "Move Existing Files?" prompt), player count, practice select —
      `screens/file_picker.rs`, `screens/game_picker.rs`, `screens/player_count.rs`, `screens/practice_select.rs`.
      Choosing a game / Load a Saved Game leads to the stand-in until Phase 6.
- Checks: unit tests for every data module; screenshots of Home (light/dark, recent popup), Settings, Support,
  Controls, Edit Player and the game grid with the real library. Dev hooks: `GAMETABLE_SCREEN=player_count|
  practice_select|kit_gallery`, `GAMETABLE_HOME_OPEN=settings|support|controls|editplayer|recent`,
  `GAMETABLE_CAPTURE_FRAME=<n>` (default 30). The file picker and the folder-change flow are not yet clicked
  through by hand.

### Phase 6 — The table
- [x] `table_view` (camera, layout, hit-test, felt, playmats, seat mirroring) — `table/view.rs`.
- [x] `card_render` (faces, backs, preview), `selection`, `card_pool_grid` — `table/card_render.rs`,
      `table/selection.rs`, `table/card_pool_grid.rs`.
- [x] `table_screen`: drag/drop rules, hotkeys, context menus — `table/screen.rs` (state, derived data, selection,
      playmats, hotkeys, keyboard), `table/input.rs` (drags, drops, box select, arrows, pan/zoom, clicks),
      `table/menus.rs` (menus as a `MenuAction` enum; dialogs as `TableDialog`).
- [x] Overlays: search, card library, token picker, game log, chat (practice: local /flip and /roll), game menu —
      `table/panels.rs`. The rulebook (Ctrl+R) shows a "not ported yet" snackbar until Phase 8.
- [x] Animations (discard flight, shuffle wiggle, rotation easing, hand lift, drag ghost scaling), drag ghosts, arrows.
- [x] `controller` (Host) — `table/actions.rs`: every controller method is a `TableAction`; `apply_as_host` keeps the
      Host controller's ownership checks; the `TableBackend` trait lets the Phase 7 client send the same actions as
      requests. Practice game — `screens/practice_game.rs`; Save Game (save-name dialog); Load Saved Game + Match
      Returning Players — `screens/load_saved_game.rs`. Pulled forward from Phase 8: the in-game Load Deck dialog and
      Save Deck prompt (`screens/deck_library_dialog.rs`); from Phase 9: the token choosers/stepper
      (`screens/token_choosers.rs`) used by Change Tokens.
- Checks: `tests/table.rs` drives a Standard 52 practice table through real frames (draw with 1, drag hand→table,
  F/E/Q/X, drop onto discard, zone menu → Search, box select + group move). Side-by-side captures against the
  original LÖVE app (`love.exe . --identity <test> --eval ... --shot ... --frames N`) for Standard 52, Lorcana and
  Middle-earth: empty tables, a dealt hand with cards played, Game Menu, Card Library, log + chat, token picker and
  Load Deck — layout identical (pixel diffs only in glyph rasterization and shuffled card faces). Dev hooks:
  `GAMETABLE_SCREEN=practice`, `GAMETABLE_GAME=<id>`, `GAMETABLE_PLAYERS=<n>`, `GAMETABLE_PRACTICE_DECKS=full`,
  `GAMETABLE_TABLE_SETUP=draw,play,menu,library,log,chat,tokens,loaddeck`. Not yet exercised by hand: token bags,
  deck widgets, pack generator, card tokens, scenarios, Load Saved Game's seat matching.

### Phase 7 — Peer multiplayer
- [x] Client `protocol` connection, `host_server`, `game_client`, `controller` (Client)
  (`net/`: std sockets, connect on a helper thread; `table/network.rs`: `NetworkHostBackend`, `ClientBackend`)
- [x] Screens: HostSetup, AssignSeats, HostGameSelect, HostGame, Join, ClientGame (`screens/multiplayer.rs`; the
  Discord tab says Discord isn't available until Phase 11; a CardTable server answer opens a Phase 10 stand-in)
- [x] Reconnect / seat reclaim
- [x] `netinfo` (local + public IP over plain HTTP), UPnP (hand-ported SSDP/SOAP, no `igd-next`)
- [x] Interop: Rust ↔ LÖVE, both host/join directions

### Phase 8 — Decks and documents
- [x] Deck editor (`screens/deck_editor.rs`: game select + editor; the deck library dialog was done in Phase 6)
- [x] Rulebook viewer (`screens/rulebook_viewer.rs`: Markdown TOC, PDF worker/cache in `data/pdf_render.rs` +
  `data/pdf_doc_cache.rs`, zoom, scroll persistence; at the table, in the deck editor; Phase 9's editor reuses it)

### Phase 9 — Game definition editor
- [x] `gamedef_ops` (`data/gamedef_ops.rs`), entry screen (`screens/gamedef/mod.rs`)
- [x] Tabs: General, Playmats, Zones, Sets (folder import, in-process JPEG rotation via SDL3_image's
  `IMG_SaveJPG` in `data/image_utils.rs`), Cards, Tokens, Scenarios (`screens/gamedef/*_tab.rs`)
- [x] Tools dialogs (duplicate ids, merge duplicate images) (`screens/gamedef/dialogs.rs`)

### Phase 10 — Lobby server client
- [x] `server_client` (`net/server_client.rs`, `server_models.rs`, `game_connection.rs`), `asset_sync` + worker
  (`net/asset_sync.rs`, `asset_channel.rs`), server `deck_store` (`data/deck_store.rs`), admin uploads
  (`net/admin_files.rs`)
- [x] Screens: login, lobby, room, decks (the lobby's Deck Editor step), collection, shop, trade (`screens/server/`)
- [x] Admin screen (users, settings, shutdown, games upload/rescan/remove, rooms, files) (`screens/server/admin.rs`)
- [x] Interop: against the Go server and the Rust server (`examples/server_interop.rs`)

### Phase 10b — Cross-platform foundation (Windows, macOS, Linux)
Added 2026-11-16 at the user's request, before Phase 11, so the app builds for most platforms. The Rust code is
mostly portable already; these are the parts that only work on Windows today.
- [x] Shaders: `tools/compile_shaders.sh` compiles `shaders/*.hlsl` to DXIL (Windows SDK dxc, unchanged bytes),
  SPIR-V (Vulkan SDK dxc, `-fvk-use-dx-layout` so uniform layouts match) and MSL (spirv-cross, entry `main0`); all
  three are committed and embedded, and `graphics/gpu_renderer.rs` uses the device's format (logged at startup as
  `graphics: SDL GPU driver …`). The HLSL carries `[[vk::…]]` attributes for SDL's Vulkan bindings. Checked on
  Windows with `SDL_GPU_DRIVER=vulkan`: a practice table with felt, rounded cards and the PDF viewer is identical
  to Direct3D 12. Metal is untested (generated from the same verified SPIR-V).
- [x] Native libraries: `native_libraries.rs` finds pdfium, libwebp/libsharpyuv (and later the Discord SDK) under
  every platform's naming (`pdfium.dll`, `libpdfium.so`, `libpdfium.dylib`, `libwebp-7.dll`, `libwebp.so.7`,
  `libwebp.7.dylib`), beside the executable, in a macOS bundle's `Contents/Frameworks` and `Contents/Resources`, in
  `runtime/<windows|macos|linux>/` for development builds, then the system search. Missing-library messages name the
  platform's file (Windows keeps the original's). Prebuilt sources: pdfium from bblanchon/pdfium-binaries
  (`pdfium-win-x64`, `pdfium-linux-x64`, `pdfium-mac-univ`); libwebp from the distribution on Linux, Homebrew or a
  bundled copy on macOS.
- [x] Platform details: `version.rs` has each platform's release asset and executable name. The default library
  folders are next to the executable on Windows only; on macOS and Linux the app's folder isn't writable, so they
  default to the save folder (SDL pref path: `~/Library/Application Support/cardtable`, `~/.local/share/cardtable`).
  Off Windows, joined paths turn `\` into `/` so Windows-authored image paths resolve (no game in the real library
  uses them today). The file picker and path helpers already work from `/`; the borderless title bar uses SDL
  hit-testing, which SDL supports on macOS, X11 and Wayland (checked on Linux below).
- [x] Static SDL: the `sdl3` crate now uses `build-from-source-static`, so SDL3, SDL_image and SDL_ttf (with its own
  FreeType and HarfBuzz) are inside the executable on every platform; no SDL libraries ship beside it. The three
  `-sys` crates build at `opt-level = 2` in dev builds (unoptimized, CMake used MSVC's debug C runtime, which
  doesn't link with Rust's). `.cargo/config.toml`'s Windows `CMAKE` path only applies when `CMAKE` isn't set, so
  other platforms and CI set `CMAKE=cmake`.
- [x] Linux build and run in WSL (Arch Linux, WSLg): client and server build, and the whole test suite passes,
  including the LÖVE font metrics (only after the switch to static SDL; the system FreeType was a pixel off for one
  Material Icons glyph). The app runs on WSLg's Vulkan (`dzn`) with SPIR-V shaders: Home and a Lorcana practice
  table with full decks and the PDF rulebook (`runtime/linux/libpdfium.so`, PDFium 8165 from
  bblanchon/pdfium-binaries, with its license) render correctly. libwebp comes from the distribution
  (`/usr/lib/libwebp.so`). Not tried by hand: dragging/resizing the borderless window on Linux.
- [ ] macOS: can't be tested on this machine; covered by the Phase 11 CI build matrix.

### Phase 11 — Integrations + packaging
Every item gets Windows, macOS and Linux code paths (see Phase 10b).
- [x] Discord (`platform/discord.rs`): presence (menus, practice, hosting with a join party, joined games), OAuth
  with the saved refresh token, the friends list and invites in Host Setup's "Invite via Discord", and the join
  prompt (`App.showDiscordJoinPrompt`, in `app/mod.rs`). Bound against `cdiscord.h` from Discord Social SDK
  1.10.19337: the strings the header passes by value are declared by value (the original's pointer declarations
  only worked on Win64), and returned strings, spans and callback results are freed/dropped as the SDK's C++
  wrapper does (the original leaked them). The SDK's release libraries are in `runtime/<platform>/` with Discord's
  license notices; the Windows DLL is byte-identical to the original's. Checked: presence publishes on Windows,
  every function binds on Windows and Linux. Not tried by hand: the OAuth consent, friends list and invites.
- [x] Self-updater (`platform/updater.rs`, Home's banner and dialogs): the check, download and zip extraction are
  portable; the install is done by the new executable itself (`--apply-update`) instead of PowerShell + `robocopy`:
  it waits for the app to exit (its standard input closes), mirrors the update in keeping the libraries, and
  relaunches. Tested end to end on Windows against a local server (`GAMETABLE_UPDATE_TEST_URL` +
  `GAMETABLE_HOME_OPEN=update_now`). Releases are zips for every platform (no tarball crate needed).
- [x] Icon/version resource (`crates/gametable/build.rs` runs the Windows SDK's `rc.exe`; release builds have no
  console window), `tools/package.py` (Windows zip, Linux zip with `.desktop`, macOS `.app` with `Info.plist`, an
  `.icns` and ad-hoc signature; `--publish` bumps, tags and pushes), and `.github/workflows/ci.yml` (tests on
  Windows, macOS and Ubuntu 22.04; release packages for `v*` tags). The Windows and Linux packages were checked by
  running them from a fresh unzip. Not yet run: the workflow itself (nothing pushed) and anything on macOS.

### Phase 12 — Parity audit
- [x] Walk `CONTROLS.md` and `docs/*.html` feature-by-feature against the Rust build; fix gaps
  - Inventories (scripts kept out of the repo): every Lua function name has a Rust counterpart except renames,
    Windows-only helpers the port replaced (PowerShell, WinHTTP, Win32 process calls) and inlined helpers; every
    user-facing Lua string is in the Rust code except the same Windows-only text. One wording gap was fixed: admin
    uploads said "Could not connect for upload" where the original says "Could not connect to upload".
  - Keyboard: `T:keypressed` / `T:update` match key for key (players, spectators, rulebook and game menu states).
  - `tests/table_controls.rs` walks the game-table part of `CONTROLS.md` through real frames: click a pile to draw
    its top card, drag to peel one, Alt+drag to move it whole or deal it into the hand, Alt+drop to stack, the board
    menu's Simple Counter (double-click halves, clamped at 0; Increment; Delete), Pass Turn To, Ctrl+click and
    Shift+box selection with Create Pile Here, Tab+drag arrows, Z, and Give to's confirmation.
  - `tests/table_real_games.rs` (ignored; `GAMETABLE_LIBRARY=<library>`, read only) covers what no bundled game
    has: Lorcana's Pack Generator (open a pack from a set, delete) and Deck Widget (create, delete), Arkham Horror
    LCG's scenario load (7 zones, 11 widgets), token bag draw-by-click and return-by-click, and the token picker.
  - Deck Editor and Game Definition Editor controls (add/remove clicks, Ctrl+R), window chrome (hit-test drag,
    double-click maximize, click-through, F6) checked against the code. The unused `NotPortedYet` stand-in was removed.
  - Still only testable by hand: Discord OAuth/friends/invites and running on a Mac (the rest was confirmed by the
    user on 2026-11-17, see the session log).

### Beyond the port — new GameTable features
- [x] Card View **Suggest Title** (`data/title_suggestion/`, `screens/gamedef/title_suggestion_dialogs.rs`): a
  button beside Card Title (and "Suggest Titles..." on the bulk form) reads titles from the ID / image file name
  (text rules), from the image with local OCR (`ocrs`; the largest short line of text), or from the image with
  Claude (Anthropic Messages API, the user's key, Haiku 5.5 by default or Opus 5.5; Settings → "Suggest Title with
  Claude"). Scope: this card, the selection, untitled cards (title still equals ID / image name) or all cards. One
  card is filled in directly; many run in the background (Claude 4 at a time) and end in a review list (old → new,
  editable, per-row checkbox). Tests: unit tests per engine, `tests/title_suggestion.rs` through real frames.
  - Local OCR on samples: Middle Earth and Lorcana titles read correctly (Lorcana only the first line), stylised
    .hack and Spellfire titles mostly misread — Claude is the accurate option. A real Claude call was tested by
    the user.
  - **Title area** (`screens/gamedef/title_area_picker.rs`). It is chosen per run rather than per game, because
    some games move the title between card layouts.
    - With "Image (local)" or "Image (Claude)" picked, the dialog shows the first card in the chosen scope: the
      card open in the form, the first selected, the first untitled, or the first card. Dragging over the
      picture boxes the title; Clear removes the box, and a plain click keeps it.
    - The box is kept as fractions of the image (`TitleArea`) and read in the same place on every card in the
      run.
    - Local OCR crops to the box. Crops under 1000 px are enlarged up to 4× first, so small subtitles read. It
      then joins every wordy line inside the box, top to bottom, with each line in capitals title-cased. On
      Lorcana, a box over the name reads "Ariel On Human Legs" and "Ursula Power Hungry".
    - Claude gets only the cropped picture, with a prompt saying it is the title area.
    - Without a box, both engines read the whole card as before.
    - `ImageCache::put_ready_texture` lets UI tests stand in a texture without a GPU.
  - **Title style** (`data/title_suggestion/title_style.rs`).
    - **Joining a name and subtitle:** with an image engine picked, "Name and subtitle joined with" offers
      Space, Comma, Dash or Colon.
      - It is learned from the game's real titles (placeholders skipped). Only titles that contain a separator
        are counted, since whole sets are often titled from IDs. The winner needs at least 3 uses and twice the
        runner-up.
      - Your choice is remembered per game (`suggest_title_separator_<game id>`) once you run with it.
      - Local OCR joins the lines of a title area with it: "Ariel, On Human Legs" on the real card.
      - Claude is told the separator, with an example.
      - The user's Lorcana library is mixed: The First Chapter uses commas, Hyperia City uses " - " (207 titles)
        and sets 2–13 use none. So it learns Dash there.
    - **Capitals:** `title_case_if_all_caps` turns all-capital titles into title case ("THE BIG BANG" → "The
      Big Bang"). Small words like "of" stay lower case except at the start of a title or subtitle. Mixed-case
      titles are kept, so "The BIG Bang" stays.
      - It is used for every OCR line, and on Claude's answer as a backstop.
      - Claude's prompt states the same rule and gets up to five of the game's titles as style examples, those
        with the separator first.
    - Tests: `TitleArea` maths, line joining, the Claude prompt, and a dialog harness test that draws, keeps and
      clears a box. The ignored `real_card_title` takes `GAMETABLE_OCR_AREA=x,y,width,height`.
- [x] Card View **Suggest Extra Data** (`screens/gamedef/extra_data_suggestion_dialogs.rs`,
  `data/title_suggestion/extra_data.rs`). A "Suggest Extra Data..." button on the bulk form (two or more cards
  selected) reads one extra field from each selected card's picture. The field is picked from the game's existing
  fields (`extra_field_names`) or typed in through "Create new extra data field..."; a typed key that matches an
  existing one ignoring case takes its spelling. It is read with local OCR (every line inside the box, top to bottom;
  numbers and symbols kept, no title-casing) or Claude (the field name plus up to 8 other cards' values as examples;
  answer `{value, found}`). A field area is drawn on the first selected card: required for OCR, optional for Claude.
  It runs on the same `SuggestionJob` (`SuggestionTarget::ExtraField`) and ends in the shared review list
  (`screens/gamedef/suggestion_review.rs`, also used by Suggest Title). Apply updates an existing key in place
  (case-insensitive) or adds it, storing the value as a string like the Extra Data tab. The reader is remembered in
  `suggest_extra_data_source`.
  - With Claude, an optional "Instructions for Claude" box adds the user's own guidance to the prompt ("Additional
    instructions for this field: ..."). They are remembered per game and field
    (`suggest_extra_data_instructions_<game id>_<field key, lower case>`), saved when a run starts, and loaded
    when that field is chosen.
- [x] **Unsaved-changes prompt in the game definition editor** (`screens/gamedef/mod.rs`). The editor keeps the game
  as last loaded or saved (`saved_game`). Back, closing the app, New and Open ask "Unsaved Changes" (Cancel /
  Discard Changes) when the game differs from it. The original had no prompt.
- [x] **UI scale and Text size sliders** (Settings → Display). UI scale is a slider from 50% to 200% in 5% steps
  (the original's dropdown had fixed choices), applied when the slider is released. Text size (`text_scale`,
  75%–150%) enlarges interface text only: `FontLibrary::set_text_scale` multiplies the size of fonts whose
  `FontKey::follows_text_size` is set. `theme::font` follows it; `theme::icon_font` and `theme::table_font` (all
  text drawn in the table scene: card faces, counters, tokens, avatar names) don't, so it still fits what it is
  drawn on. Layout sizes stay the same, so at 150% some tight fields fill their boxes. (A trial that scaled
  everything with monitor resolution, 2560x1440 = 100%, made text too small at 1080p and was dropped.)
- [x] **Card grid zoom** in the deck editor and the game definition editor's Cards tab: a zoom-out button, slider
  (60%–250%) and zoom-in button at the right of the search row (`table/card_pool_grid.rs::CardZoom`). It scales
  each tile's card image and name text (names never below 9pt); each screen remembers its own zoom
  (`deck_editor_card_zoom`, `card_view_card_zoom`), saved when the slider is released. `card_pool_grid` takes the
  zoom and hands each tile its card size, card scale and name position; the other grids (scenario, collection,
  trade, multi-select preview) pass 1.
- [x] **Turned cards don't overlap in card grids**: a left/right card is as wide as a portrait card is tall, so
  `card_pool_grid` takes `has_turned_cards` (from `any_card_turned`) and, when set, uses few enough columns that
  neighbouring turned cards keep at least 5 px apart (`SMALLEST_SPACE_BETWEEN_CARDS`). All-portrait grids are
  unchanged. Every grid caller (deck editor, Cards tab and its multi-select preview, scenario deck pool and placement
  library, collection, trade) passes it.
- [x] **Zones on playmats** (`screens/gamedef/playmat_zone_placement.rs`, `table/playmat_zones.rs`). In the
  Playmats tab, a "Zones on This Mat" row of chips places a zone in the middle of the selected mat. In the table
  preview it can then be dragged around, dragged off the mat, or taken off with its × button. Placed zones are
  drawn as a card back (scenario zones use the scenario back) or as their counter or token bag, with the name as
  a small label above. The label only appears in the editor; at the table the name is the hover tooltip.
  - Each playmat stores its own placements: `PlaymatDefinition.zone_placements`, JSON `"zonePlacements":
    [{"zoneId","x","y"}]`. `x`/`y` are the zone's centre as fractions of the mat, seen the right way up. The key
    is written only when the list isn't empty; the original CardTable ignores it (and drops it if it saves the
    file).
  - Ownership rule (`zone_fits_playmat_type`): individual mats take player-owned zones, and each player's mat
    shows that player's own copy. The shared mat takes shared and scenario zones; a scenario zone appears once
    its scenario is loaded. Placements that no longer fit (the type was switched, or the zone was deleted) are
    shown dimmed with a warning in the editor and ignored at the table.
  - At the table, a placed zone moves from its hand-band or sidebar slot onto the mat and is drawn at the
    table's zoom. It falls back to the slot when the mat is turned off (F12) or a mat without it is chosen. An
    opponent's mat is turned 180° and its zones with it; their zones only show a tooltip, as in the band.
  - Mat rects now come from one function (`TableScreen::active_playmats`). The band, the sidebars and the mats
    all draw zones through `draw_zone_pile`.
  - Mat zones have their own drop lists (`playmat_drop_zones`, `playmat_bag_drop_zones`). They are checked
    after the docked zones and only over the table.
  - Tests: model round-trip; placeable zones; `zones_on_playmat`; an editor harness test (place, drag, drag
    off); `tests/table_playmat_zones.rs` (the discard pile on a shared mat leaves the sidebar, takes a drop,
    and returns after F12 turns the mat off).
  - Dev hooks: `GAMETABLE_TABLE_SETUP=playmats` turns on every seat's first mat. `GAMETABLE_GAMEDEF_TAB=
    playmats,select` selects the first playmat.
  - **Hand areas are see-through.** The hand area has no background, and the zone and avatar area at its end is
    tinted only while some zones are still docked there. The table (mats included) is drawn under both hand
    areas, but an `ItemKind::HandArea` blocker keeps it from being clicked, hovered or hotkeyed there
    (`tests/table.rs`).
  - **Ctrl + drag moves a zone on a playmat at the table.** Players move zones on their own individual mat; on
    the shared mat only the host can (`can_load_scenario` on the client, `owner_id` in `HostEngine`). It works
    for card zones, counters and token bags.
    - Synced to everyone. New request `requestSetPlaymatZonePositions {playmatIndex, positions: [{zoneId, x,
      y}]}` replaces one mat's positions, clamped to 0..1. They are stored as `PlayerInfo.playmatZonePositions`
      or `TableState.sharedPlaymatZonePositions`, `[{playmatIndex, zoneId, x, y}]`, and written only when
      non-empty, so saved games keep them.
    - An original CardTable host or Go server ignores the new request, so there the zones stay where the
      designer put them.
    - Remembered on this computer per game and mat (`playmat_zone_positions_<game>_<index>` in settings) and
      sent again when that mat is picked (F12 or the seed at start).
    - "Reset Zone Positions" in a moved zone's context menu puts back the designer's layout.
    - Tests: `gametable-core/tests/playmat_zone_positions.rs`, plus a Ctrl-drag test in
      `tests/table_playmat_zones.rs`.
  - (Since renamed with playmats: see Game Boards below. The files are now `board_zone_placement.rs`,
    `table/board_zones.rs`, `gametable-core/tests/board_zone_positions.rs` and `tests/table_game_boards.rs`.)
- [x] **Game Boards** (playmats renamed, so they can also be a board game's boards). The editor tab is "Game
  Boards" (`screens/gamedef/game_board_tab.rs`); the layout lives in `table/board_layout.rs`, shared by the table
  and the editor.
  - Each board has its own kind, **player** (every player gets their own copy) or **shared**, replacing the
    game-wide playmat type: `GameBoardDefinition.kind`, JSON `"kind": "player"|"shared"` (player, the default, is
    omitted). Zones placed on a board follow its kind (`zone_fits_board_kind`).
  - JSON: the game writes `"gameBoards"` where `"playmats"` was and no longer writes `"playmatType"`. A game
    without `gameBoards` reads `playmats`, each board taking its kind from the old `playmatType` ("individual" ->
    player). Images stay in `_playmats/` and the settings keys keep their `playmat_` names, so libraries and
    remembered choices carry over. The state and network names are unchanged too (`individualPlaymatOn`,
    `requestSetSharedPlaymat`, `playmatZonePositions`, ...), keeping the Lua host-engine fixtures valid; their
    Rust fields are renamed (`player_boards_shown`, `chosen_player_board`, `shared_boards_shown`,
    `chosen_shared_board`, `board_zone_positions`).
  - **Table Preview** (`table_board_placement.rs`), beside the Board Preview (the old table preview of one
    board, where zones are placed). "Place on Table" chips put a board down, a shared board in the middle and a
    player board in front of the viewer's seat; a 1-6 player switch previews the other seats' copies (dimmed,
    turned across the table). Drag to move; the selected board's toolbar turns it 90 degrees, marks it "Can be
    toggled (F12)", or takes it off the table. Stored as `GameBoardDefinition.table_placement`, JSON
    `"tablePlacement": {x, y, rotation, toggleable}` (rotation 0/90/180/270 and toggleable omitted at their
    defaults). A shared board's `x`/`y` is its centre on the table (world units, as the host sees it); a player
    board's is an offset from its seat origin, the middle of that player's table edge.
  - Seats: each row is split evenly across the table, like the hand areas, and a seat's origin is the middle of
    its share (`seat_origin_xs`). Seats in the right half of the table, as their player sees it, get the
    layout mirrored left to right (offset and quarter turns, `seat_mirrors_its_boards`), so boards sit
    symmetrically around the table: placed toward the middle from your seat, they are toward the middle from
    every seat. A seat in the middle isn't mirrored. The far row's boards are turned 180 degrees about
    the far edge; a mirrored viewer sees shared boards turned too. Zones move with their board's turn; their
    piles are drawn turned only on an upside-down board, so sideways boards keep cards readable.
  - Placed boards are on the table from the start. A toggleable one shows while its owner's flag is on
    (`player_boards_shown`, or for shared boards `shared_boards_shown`, host only); F12 flips it. Toggleable
    placed boards start shown unless this computer last hid them (`boards_shown` now remembers hiding too).
    With every placed board fixed, F12 does nothing.
  - Nothing placed: the original's behaviour. F12 shows each player's chosen player board in the old row layout;
    the table's right-click menu has a **Player Board** submenu (with more than one player board) to pick it,
    and "Show/Hide Boards (F12)". A game with only shared boards keeps the centred shared board (host toggles;
    Shared Board submenu).
  - A zone on several shown boards of one owner sits on the first. Ctrl-drag works per board (the one holding
    the zone), and "Reset Zone Positions" resets that board.
  - The Lua golden model fixtures are compared with the `playmats` -> `gameBoards` rewrite applied
    (`models_golden.rs::as_game_table_saves_it`); `tools/lua_golden/resave_library.sh` notes the same expected
    difference.
  - Tests: model round-trip and migration (`tests/models.rs`), `board_layout` and `board_zones` unit tests (seat
    origins, far-side turning, quarter turns, visibility rules), an editor harness test (place, select, turn,
    toggleable, drag, take off), and `tests/table_game_boards.rs` (a fixed placed board is there from the start
    and untouched by F12; a toggleable one starts shown and F12 hides it; player boards toggle per player).
  - Dev hooks: `GAMETABLE_GAMEDEF_TAB=boards,select,table` opens the tab's Table Preview;
    `GAMETABLE_TABLE_SETUP=boards` (or `playmats`) shows every seat's boards.
  - Editor follow-ups (2026-11-19): the chip rows are gone, to leave the previews more room. Right-clicking the
    Board Preview offers **Add Zone...** and right-clicking the Table Preview **Add To Table...**; both open
    `list_picker_dialog.rs` (a filter field over a scrolling list, for games with many zones). A zone added from
    a right-click on the board lands there, else in the board's middle. Right-clicking a placed board gives
    **Locked to Center** (shared boards), **Turn 90°**, **Can Be Toggled (F12)** and **Take Off Table** (the
    toolbar is gone). Shared boards start locked to the table's middle (`BoardTablePlacement.unlocked_from_center`,
    JSON `"unlockedFromCenter": true` once unlocked); locked, they ignore `x`/`y` and can't be dragged; locking
    again puts them back in the middle. Both previews mark the table's centre with a small white +. The Table
    Preview zooms (wheel at the mouse, zoom buttons, 50%–600%; clicking the percentage fits the table again) and
    pans (drag the felt, or the middle button) via `PreviewCamera`.
  - Player boards per player (2026-11-20). Besides its every-seat placement, a player board can be placed for
    single players: `GameBoardDefinition.player_placements`, JSON `"playerPlacements": [{player, x, y, rotation,
    toggleable}]` (`player` is the 1-based seat; `x`/`y` the board's centre on the table as the first seat sees
    it, like a shared board, so players across the table see it turned with the table). One layout serves every
    player count; a placement for a seat the game doesn't have is simply absent. A player's own placement
    replaces their every-seat copy of that board (`toggleable_for_player`); a board placed only for some players
    isn't shown for the others. In the Table Preview, right-clicking another seat's (dimmed) copy offers **Place
    Separately For Player N** (your own copy's menu offers it for player 1), which keeps it where it was; the
    table's menu has **Add For Player > Player N...** (the board goes where you right-clicked); a separately
    placed board is draggable, labelled with its player, and its menu adds **Belongs To > Player N** and
    "Take Off Table (Player N)". The preview grows to show a player given a board.
  - Moving boards in the game (2026-11-20, `table/board_handle.rs`): right-clicking one of your own player
    boards on the table offers **Unlock Board** / **Lock Board** (kept on this computer for the session), and
    **Reset Board Position** once moved. Unlocked, the board shows a handle: a quarter of its width, 15 screen
    pixels tall, 30 screen pixels beyond its bottom edge as it lies (so it turns with the board). Dragging the
    handle moves the board (and the zones on it); Q / E while dragging turn it 90 degrees. On release the spot
    goes to everyone: `PlayerInfo.moved_boards`, JSON `"movedBoards": [{boardIndex, x, y, rotation}]` (centre as
    table fractions and turn as the first seat sees them) via the new `requestSetMovedBoard` (`{boardIndex, x, y,
    rotation}`; without `x`/`y` it resets). The host only lets a player move their own player boards. A moved
    board overrides the layout in both placed and default modes. Loose cards lying on a board don't move with
    it. The server runs the host engine, so it needs redeploying with this.
  - Boards carry what lies on them (2026-11-20). When a player moves, turns or resets one of their boards,
    the table cards and free counters and tokens whose centre lies on it go with it, keeping their place on
    the board and turning with it (cards and tokens gain the board's quarter turns, the short way round;
    counters only move). Tokens attached to carried cards follow their cards. What lies on the board is
    decided when the drag starts, and the items ride along live while dragging. Other players' cards and
    tokens, docked widgets, arrows, pack generators, deck widgets and token bags stay put. The client sends
    the items and the board's old and new spots as `carried` on `requestSetMovedBoard` (`{fromX, fromY,
    fromRotation, toX, toY, toRotation, cardIds, widgetIds}`, spots as table fractions and degrees as the first
    seat sees them; `BoardCarry`), and the session applies `table_actions::carry_with_board`, skipping
    anything the mover may not move. Being on a board is geometric, not stored. Tests:
    `carry_with_board` unit tests in `tests/table_actions.rs` and the board handle harness test.
- [x] Zone piles without the count badge and shuffle button (2026-11-20). A card zone (and a deck widget's
  sub-zones) no longer draws the black card count bubble; its hover tooltip reads "Name (N cards)" instead, for
  every player's zones. The round shuffle button under a zone is gone: right-clicking a zone with 2+ cards whose
  zone is Shuffleable offers **Shuffle Deck** (same shuffle, sound and wiggle). Table piles keep their badge and
  shuffle button.
- [x] No one-frame snap back on drop (2026-11-20). `TableScreen::draw` copied the session and derived before
  handling input, so the frame that dropped a card (or board, widget...) drew the pre-drop copy with the drag
  already ended: the item flashed at its old spot. `perform` now flags non-preview actions and `draw` re-reads
  the session after input when flagged. Joined clients still wait a round trip for the host's update.
- [x] Scenario Card Placement shows the game boards and zooms like the Table Preview (2026-11-20). The canvas
  draws the placed boards (and the zones on them, and seat labels) read-only under the cards, laid out by the
  table's own `layout_boards` for a chosen Players count (1-6, shown when any board is placed), so cards can be
  set on a board exactly. It uses the Table Preview's `PreviewCamera`: wheel zooms around the cursor, dragging
  bare felt or the middle button pans, zoom buttons and a fit-the-table percentage sit above it. Shared pieces
  live in `table_board_placement.rs` (`draw_placed_boards`, `draw_zoom_controls`). Test: the scenario tab's
  `the_placement_canvas_zooms_and_pans_like_the_table_preview`.
- [x] Scenario zones set on the table (2026-11-20). Right-clicking the Card Placement canvas offers **Add Zone**,
  a submenu of the scenario's zones not yet on the table; the zone lands where the canvas was clicked and drags
  anywhere on the table (Delete takes it off; deleting the zone or duplicating the scenario keeps the spots in
  step). Stored as `Scenario.zone_placements` (`zonePlacements`: `zoneId`, `x`, `y` table fractions as the first
  seat sees them; omitted when empty, so files without it are unchanged). Once the scenario loads, the table
  (`board_zones::scenario_zones_on_table`, `Derived.zones_on_table`) draws the zone at that spot for every viewer
  (flipped with the table for mirrored seats), out of the sidebars and off any shared board that also holds it.
  It takes drops, clicks and hotkeys like a shared zone, but sits on no board, so Ctrl-drag can't move it. No
  protocol change: clients read the spots from the game definition (a lobby server that re-serializes games
  needs the redeploy to keep `zonePlacements`). Tests: `scenario_zones_set_on_the_table_round_trip`,
  `a_scenario_zone_is_added_from_the_right_click_menu_and_dragged_anywhere`,
  `a_scenario_zone_set_on_the_table_sits_there_and_cannot_be_moved`.
- [x] Card View **Shrink Card Art...** (2026-11-21; `screens/gamedef/shrink_card_art_dialog.rs`). A button beside
  Add Card rewrites every card's art (and each existing unique `_BACK` image) so none is taller than 1024 px. Each
  image's width follows from its own aspect ratio (area-average downscale, `RgbaImage::downscaled_to_max_height`);
  images already short enough are untouched. PNG, JPEG and WebP keep their format (JPEG re-encoded at quality 90,
  WebP lossy at Convert to WebP's quality 90); other formats (GIF, BMP) are skipped and counted. A confirmation first, then worker threads with a
  progress dialog and Cancel, then a summary; files are written to a temporary file and renamed over the original
  (`image_utils::shrink_image_to_max_height`). Cached textures reload as each file changes. The game definition
  itself doesn't change. Dev hook: `GAMETABLE_GAMEDEF_TAB=cards,shrinkart`. Tests:
  `downscale_to_max_height_keeps_the_aspect_ratio`, `tall_png_and_jpeg_shrink_to_the_maximum_height`,
  `tall_webp_shrinks_and_stays_webp` (skipped without libwebp),
  `art_paths_cover_fronts_and_existing_unique_backs_once`, `summary_counts_each_kind_of_result`.
- [x] Card View preview shows the selected card's **image resolution** (2026-11-21), e.g. "750 × 1050 px" under the
  image, or "Front · …" / "Back · …" when the back is shown too. Sizes come from the file headers without decoding
  (`data/image_dimensions.rs`: PNG, JPEG incl. progressive and large EXIF, GIF, BMP, WebP), cached per path and
  re-read when the file's modified time changes, so they update after Shrink Card Art. Tests in that module;
  checked by screenshot on Lorcana and Middle Earth against the real files. The label also gives the file size
  ("367 × 512 px · 288 KB"; KB rounded up below 1 MB, then MB to one decimal), re-read when the file's modified time
  or length changes (`ImageFileInfoCache` in `card_view_tab.rs`).
- [x] Card View **Convert to WebP...** (2026-11-21; `screens/gamedef/convert_to_webp_dialog.rs`,
  `data/card_art_files.rs`). Rewrites every card's art, and each card's unique `_BACK` image, as lossy WebP
  (quality 90, `webp::encode_lossy` via the runtime-loaded libwebp's `WebPEncodeRGBA`); PNG, JPEG, GIF and BMP
  convert, already-WebP images are skipped. A front and its unique back convert together or not at all (the back's
  name follows the front's extension). Worker threads write `<name>.webp` beside each original (never over an
  existing `.webp`); then the editor points the cards at the new files, in both the edited game and the saved copy,
  and writes the saved copy straight to `gamedef.json` so the file on disk never names a deleted image while other
  unsaved edits stay unsaved. Only after that save are the originals deleted, except any still used (e.g. by a card
  back definition). The summary gives converted/updated counts, MB before and after, deletions, skips and failures.
  Shrink Card Art and Convert to WebP share `screens/gamedef/card_art_job.rs` (worker threads + progress dialog)
  and `card_art_groups`. Dev hooks: `GAMETABLE_GAMEDEF_TAB=cards,towebp[,start]` (`start` clicks Convert), and
  `GAMETABLE_GAME` may now be a full folder path. Checked end to end on a scratch copy of Megaman: 558 JPEGs, 135.9
  MB -> 67.1 MB, `gamedef.json` identical apart from the 558 `imagePath`s, cards and resolution labels show from the
  WebP files. Players need libwebp to see WebP art (bundled on Windows; from the system on Linux and macOS).
- [x] Sets tab **Add Set...** import options (2026-11-21; `data/set_image_import.rs`). After picking the folder, a
  dialog offers "Shrunk to no taller than 1024 px" and "Converted to WebP (lossy, quality 90)" (unticked by default,
  remembered for the session; WebP greyed out without libwebp). The copy now runs on `CardArtJob` worker threads
  behind a progress dialog with Cancel (cancelled: the images that made it in still get cards). Each image is
  decoded once, turned portrait (PNG/JPEG, as before), shrunk, then written as `<name>.webp` or in its own format;
  `_BACK` art becomes `<name>_BACK.webp`, so unique backs still match. An image that can't be processed is copied
  unchanged and a toast says so. The picked folder is never changed (unless it already is the set's folder in the
  game). Replaces `copy_set_images`. Dev hooks: `GAMETABLE_GAMEDEF_TAB=sets,addset[,importall]` with
  `GAMETABLE_SET_FOLDER`.
- [x] Editor toolbar **Save to Zip** (2026-11-21; `data/game_package.rs`, `screens/gamedef/save_to_zip_dialog.rs`).
  A `folder_zip` button left of Save packs the whole game folder into `<folder name>.gamedef` beside it in the game
  library (`lorcana` -> `<library>/lorcana.gamedef`). It is a plain zip with the folder at its root, the layout
  `validate_game_zip` / the admin game upload expect; images are stored, other files deflated, `.gametable-tmp`
  leftovers skipped, and so are cache/runtime folders (`_cache`, `__pycache__`, `savegames`, `gamelogs`, dot
  folders; Spellfire's `_cache/atlases` was 1.3 GB of old CardTable texture atlases), and `.py` scripts used to build
  a game. `flate2`, `miniz_oxide`,
  `zlib-rs`, `crc32fast` and `zip` build with `opt-level = 3` in dev, since unoptimized Deflate ran at ~3 MB/s. Unsaved changes are saved first (the "Cards Without a Set" warning still applies; Cancel there
  stops the zip). Asks before replacing an existing package; packs on a background thread behind a progress dialog
  with Cancel, writing to a temporary file that only replaces the package once complete. Dev hook:
  `GAMETABLE_GAMEDEF_TAB=savetozip`.
- [x] **Play games straight from `.gamedef` packages** (2026-11-21). A `.gamedef` (zip with the game folder at its
  root, as Save to Zip writes) in the game library is a complete game, played in place without unzipping; loose
  folders still work, and a package wins over a folder with the same id.
  - `gametable-core/src/game_archive.rs`: `GameArchive` (table of contents read once, one zip reader behind a mutex,
    case-insensitive fallback lookups, refuses multi-root or unsafe zips), `split_package_path`, and a shared cache
    (`open_game_archive`, reopened when the file changes; `forget_game_archive` before replacing or deleting one).
  - A packaged game's `folder_path` is the package file, so paths such as `<lib>/lorcana.gamedef/1st/x.webp` are
    unique ImageCache keys. `data/game_files.rs` (`read`, `is_file`, `is_dir`, `list_folder`) reads them; the image
    workers, existence cache, rulebook (PDF + markdown), built-in decks and token folders go through it.
  - `game_loader::load_games_from_folder` loads `*.gamedef` too and skips dot entries. Save games and logs of a
    packaged game go to `<lib>/.gamedata/<id>/` (`writable_game_folder`); adding token images is disabled for one.
  - Home: dropping a `.gamedef` (`Screen::file_dropped`) checks it with `validate_game_zip`, asks Install/Replace
    (notes a same-id folder it will shadow), copies it to `<lib>/<id>.gamedef` on a thread with a progress bar
    (`data/game_install.rs`). Dev hooks: `GAMETABLE_HOME_OPEN=drop:<file>`, `GAMETABLE_GAMES_FOLDER=<folder>` (use
    another library without saving it).
  - Editor: the game list shows packages (".gamedef package"); picking one offers Extract to `<lib>/<id>/`
    (`data/game_extract.rs`, temp folder then rename), then opens it. While a package sits beside an open folder the
    title says "plays from <id>.gamedef; Save to Zip to update it". Dev hook `GAMETABLE_SCREEN=gamedef_entry` with
    `GAMETABLE_GAMEDEF_EXTRACT=<id>`.
  - Lobby downloads reuse files from a local package with the same id (`asset_sync::local_game_folders`).
  - Server: `GameLibrary::scan` loads `<games>/*.gamedef` (`ServerGame::package`), serving `read_file`/`open_file`/
    built-in decks from the zip; the manifest uses the folder walk's rules, so its hash equals the unpacked game's
    (tested). Admin file list/upload/delete refuse packaged games; deleting the game removes the package; admin
    install still unzips.

## Quirks log (original behaviours to preserve or consciously handle)

- GameTable saves a game's boards as `gameBoards` (each with its `kind`), not the original's `playmatType` +
  `playmats`, so the original CardTable shows no playmats for a game saved by GameTable. GameTable still reads
  the old keys.
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
- `CONTROLS.md` lags the code in places: the board menu has its own **Tokens** item (not Widgets > Token), and the
  Card Library stays in the board menu in collection mode. Follow the code.
- Confirmation dialogs (`overlays.confirm`) have no Enter action: Return does nothing, only the button confirms.
- A card's menu offers Give to / Remove Ownership for any card the player can act on, including unownable ones
  (e.g. Standard 52's shared deck); the rules engine then refuses the change silently.
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
  reads local time from SDL (`save_files::local_now`).
- `game_loader` rebuilds each loaded game from a field list that omits `currentTurnColor`, so a game's own
  current-turn colour never takes effect (Settings and the table fall back to the player's/default colour).
  `mergeLocalImagePaths` (client joining a host) likewise drops `tableColor`, `playmatBorderColor` and
  `currentTurnColor`. Both kept as in the original. Check in Phase 9 whether the editor loads through
  `game_loader` (if so, saving from the editor would drop the field).
- Home's avatar: `widgets.avatar` draws its border unless `showBorder == false`, so callers that pass nothing get one.
- Table: the original's controller callbacks (`onLeaveGame`, `onSaveGame`, `onKickPlayer`) become `TableEvent`s the
  hosting screen drains each frame; its global confirm/dialogs are table-owned `TableDialog`s. The table copies the
  session state each frame (the original read it live). A widget's numeric `sizeOverride` is an absolute size in
  world units (the core enum calls it `Multiplier`). Glyphs drawn under a scale (counter values, badges) come from
  SDL_ttf at the unscaled size, so they rasterize a pixel narrower than LÖVE's at some sizes.
- Server client: request callbacks become polled request ids (`take_response`) for screens, with `request_with`
  callbacks kept for the deck store. The lobby (bottom of the server stack) pumps the client in `background_update`
  and drains its events; the "is a Trade screen open" stack scan is a `trade_screen_open` flag set in the Trade
  screen's `enter`/`leave`. Both servers: resuming a token on a new connection drops the account's older connection;
  player names are unique; `adminFileDelete` refuses a non-empty folder ("Delete the files inside it first."); a
  waiting room closes when its owner disconnects. The Go server hashes its library before listening (several seconds
  for a 200 MB game on first start).
- The original game definition editor had no unsaved-changes prompt; GameTable asks before Back, quitting, New
  or Open would drop unsaved edits (a deliberate change requested by the user).

## Build and tooling notes

- SDL3, SDL3_image and SDL3_ttf are built from source by the `sdl3` crate (`build-from-source`). CMake is not on PATH, so
  `.cargo/config.toml` sets `CMAKE` to the copy inside Visual Studio 18 Insiders. The first build takes about 2 minutes.
- `sdl3` crate version 0.22.1 (SDL 3.4.20, SDL_image 3.4.8, SDL_ttf 3.2.2), linked statically.
- Linux builds (WSL Arch, default user root): packages `base-devel git cmake ninja pkgconf clang rustup libx11 libxext
  libxrandr libxcursor libxi libxss libxtst libxkbcommon wayland wayland-protocols libdecor mesa vulkan-icd-loader
  vulkan-swrast vulkan-dzn libpulse alsa-lib freetype2 harfbuzz libwebp`. Build from the Windows checkout with
  `CMAKE=cmake CARGO_TARGET_DIR=$HOME/gametable-target cargo build` (a Linux-disk target folder is much faster than
  `/mnt/f`); the app runs on WSLg's display.
- Shaders: `VULKAN_SDK='F:\Tools\VulcanSDK\1.4.363.1' bash tools/compile_shaders.sh` (the variable is set machine-wide,
  but shells started before the SDK install don't see it).
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
- **Updating the live server: `pwsh tools/deploy-server.ps1`** (`-DryRun` first to trial the new build on the VM
  against a copy of the data on port 51299; `-SkipTests`). Port of the original's `server/tools/build.ps1` +
  `upload-server.bat`: tests, builds the Linux server in WSL, checks its glibc need against the VM's, zips it,
  uploads with PuTTY `pscp`, then `tools/server/install.sh` on the VM backs up `data/cardflux.db*` and the
  programs to `~/backups/<stamp>/`, installs `tools/server/gametable.service` and `server.sh`, runs
  `server.sh update server.zip` and waits for the listening line. Connection details are in the uncommitted
  `tools/deploy-server.local.json` (host, user, PuTTY key). Startup takes about 2.5 minutes (it hashes every
  game file; the Go server took about 3). Rollback to the Go server:
  `sudo systemctl disable --now gametable && sudo systemctl enable --now cardflux` (its binary and service file
  are still there; restore the database from the backup to undo data changes).

## Open questions

- Shader builds for non-Windows platforms: this machine's `dxc` has no SPIR-V backend; Vulkan/Metal builds need
  SPIR-V/MSL versions of `shaders/*.hlsl` (e.g. via SDL_shadercross or a SPIR-V-enabled dxc).

- Release repository for the Rust build's self-updater: **decided 2026-11-16, a new public repo
  `jvthompson/gametable`** for the code, CI and releases (`jvthompson/cardtable` keeps the LÖVE app's history and
  releases; this repo had no remote, and the earlier "same repo" choice rested on a wrong report that it did).
  LÖVE installs don't see Rust releases. To move them over later: publish a final LÖVE release whose updater
  points here, or publish the Rust Windows zip (`cardtable-windows.zip`, build > 38) to `jvthompson/cardtable` too
  (their helper mirrors it in; the settings folder is shared).
- Phase 11 decisions (2026-11-16): the user downloads the Discord Social SDK for macOS and Linux from the developer
  portal; macOS packages are unsigned (ad-hoc) for now; GitHub Actions tests all three platforms on every push and
  builds the three release packages when a release tag is pushed.

- Shader toolchain for Phase 10b: **decided 2026-11-16, the LunarG Vulkan SDK on Windows** (its `dxc.exe` emits
  SPIR-V, and `spirv-cross.exe` turns SPIR-V into MSL). The user installs it; `tools/compile_shaders.sh` will find
  it through `VULKAN_SDK`. Compiled shaders are committed, so building the app never needs the SDK.

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
- 2026-11-16: **Phase 5 complete.** Data modules, the real Home screen (replacing the placeholder), Settings,
  Controls/Support with a Markdown renderer, Edit Player Settings, the in-app file picker, the game picker grid,
  player count and practice select. `Frame` gained `profile`, `hand_settings` and `dpi_scale`. Unported
  destinations open `screens/not_ported.rs`. 85 client unit tests + 10 UI tests pass. Next: Phase 6, the table
  (`table_view`, `card_render`, `selection`, `card_pool_grid`, `table_screen`, Host `controller`, practice game,
  save/load game + player remap).
- 2026-11-16: **Phase 6 complete.** The game table (`table/`), practice games, save/load game with seat matching,
  plus the Load Deck / Save Deck dialogs and token choosers pulled forward. `Session::resume_log_clock` added to the
  core; `Theme` gained `force_dark` for the table's dark panels. Verified side by side against the original LÖVE app
  by driving it with `--eval`/`--shot` under a test identity. 91 client unit tests, 3 table + 10 UI kit interaction
  tests. Next: Phase 7 (peer multiplayer: protocol connection, host server, game client, client backend for
  `TableBackend`, the multiplayer screens).
- 2026-11-16: **Phase 7 complete.** `net/` (connection + heartbeat, connector thread, plain HTTP, netinfo, UPnP,
  host server, game client), network table backends, the six multiplayer screens; Home's Join and Player Count's
  Host now open them. `TableBackend::update` takes the frame index (the host server pumps once a frame).
  `tests/multiplayer.rs` plays host + client over loopback (handshake, roster/avatars, game + state, a draw request,
  seat reclaim, kick). `examples/interop_peer.rs host|join <port>` is a headless peer: checked against the original
  LÖVE app (driven with `--script`, UPnP off) in both directions — handshake, roster, avatars, gameData, fullState,
  requests, game log text and the `hostLeft` disconnect all interoperate; the Rust Host lobby and client table match
  the original side by side. Dev hooks: `GAMETABLE_SCREEN=host|join`, `GAMETABLE_JOIN=<host:port>`,
  `GAMETABLE_NO_UPNP`. Next: Phase 8 (deck editor, rulebook viewer).
- 2026-11-16: **Phase 8 complete.** Deck editor (game select, pool with collection badges, deck slots, open/save
  through the deck library dialogs, Space preview, Ctrl+R) with its deck logic in a testable `DeckBuilder`; the
  documents viewer (Markdown with contents sidebar, PDF pages rendered on a worker thread through `pdfium.dll`, zoom,
  remembered scroll) at the table and in the deck editor. `pdfium.dll` copied into `runtime/windows/`. Markdown images
  load through the image cache rather than the original's synchronous decode; PDF pages upload via the new
  `ImageCache::put_rgba` and can be evicted (they re-render on demand), and a document's worker stops when it's closed
  over. Side by side with the original (real library, read only): the Lorcana deck editor, the Lorcana PDF and the
  Spellfire Markdown rulebook match. Dev hooks: `GAMETABLE_SCREEN=deck_editor|deck_editor_select`,
  `GAMETABLE_DECK_SETUP=add,rulebook`, `GAMETABLE_TABLE_SETUP=rulebook`. Next: Phase 9 (game definition editor).
- 2026-11-16: **Phase 9 complete.** The game definition editor: entry screen (library grid, change folder, New / Open
  Existing), the editor shell (toolbar, tab bar with Tokens/Scenarios only while the game uses them, Ctrl+R
  documents, Tools menu, the "Cards Without a Set" save warning) and all seven tabs. List items that the Lua keyed
  by table identity get `StableKeys` (`screens/gamedef/common.rs`) so widget ids follow items through reorders;
  card keys live on the shared `EditorModel` so the Sets tab, Tools dialogs and Cards tab stay in step. Landscape
  card art turns portrait in-process (PNG and JPEG) instead of through PowerShell/System.Drawing. Side by side with
  the original on the real library (read only, nothing saved): General, Playmats, Zones, Sets, Cards (with a
  selection) for Lorcana, Tokens and Scenarios (Zones, Card Placement, Deck Builder) for Arkham, and the entry screen
  all match (≤2.5% differing samples, all from card-art filtering). Dev hooks: `GAMETABLE_SCREEN=gamedef|gamedef_entry`,
  `GAMETABLE_GAME=<folder>`, `GAMETABLE_GAMEDEF_TAB=<tab>[,select][,placement|deck]`. Not yet exercised in the GUI:
  saving, image/set import, the Tools dialogs, drag-reordering. Next: Phase 10 (lobby server client).
- 2026-11-16: **Phase 10 complete.** The lobby server client: `ServerClient` (polled, fire-and-forget and callback
  requests; events drained by the lobby), the asset-channel game download with local-copy reuse and memoized loads,
  the server deck store, admin uploads (192 KiB chunks; game zips checked with `zip` instead of `tar.exe`), and the
  sign-in, lobby, room, collection, shop, trade and admin screens. The app shell gained `ScreenCommand::PopTo` with
  `Screen::marker` (the original's return-to-lobby) and app-level dialogs (`Ui::app_dialog`, for trade invites on any
  screen); the table takes a `GameConnection` and an optional room chat. `examples/server_interop.rs <host:port>
  <game> <file>` drives two accounts headless through sign-in/resume, chat, download, decks, a room to gameData,
  collection mode, packs, a full trade, admin settings and a file upload/list/delete, then draws every server screen
  on live data: all checks pass against both the Rust server and the Go server (built into scratch). Side by side with
  the original (both resumed into the Go server's lobby): sign-in and lobby match. Not yet exercised in the GUI by
  hand: the Create Game dialog through to the table, the profile dialog, trade dragging, admin dialogs. Next: Phase 11
  (integrations and packaging).
- 2026-11-16: The user wants the app buildable for most platforms, so Phase 10b (cross-platform foundation) was added
  before Phase 11, and Phase 11's items now call for Windows/macOS/Linux code paths. Survey: the Windows-only parts
  are the DXIL-only shaders, the bundled DLLs' names and search paths, and (Phase 11) the original's PowerShell
  updater. WSL with Arch Linux is available for real Linux builds. Next: agree the shader toolchain, then Phase 10b.
- 2026-11-16: **Phase 10b complete (macOS untested).** Shaders in DXIL/SPIR-V/MSL with runtime selection (Vulkan on
  Windows via `SDL_GPU_DRIVER=vulkan` is pixel-identical to Direct3D 12), `native_libraries.rs` for per-platform
  library names and search folders, per-platform release/executable names, writable default library folders off
  Windows, and statically linked SDL. Linux (WSL Arch) builds, passes every test and renders Home, the table and a
  PDF rulebook. Linux builds use `CMAKE=cmake` and a target folder on the Linux disk. Next: Phase 11, with
  Windows/macOS/Linux code paths and a CI matrix (the first macOS build).
- 2026-11-16: **Phase 11 complete (CI and macOS not yet run).** Self-updater with a cross-platform install helper,
  Windows icon/version resource, packaging script, CI workflow, and Discord over the Social SDK for all three
  platforms (see the Phase 11 list). Added `ureq` (approved). `ScreenCommand`-free Discord: the module keeps its own
  state on the main thread; screens publish presence every frame (repeats ignored) and poll `Outcome`s. Next:
  push to run CI (expect fixes for the first macOS build), then Phase 12 (parity audit).
- 2026-11-16: Created the public repo `jvthompson/gametable` and pushed `main`. The first CI run passed on Windows,
  macOS (Apple silicon) and Ubuntu 22.04 with no changes: the full suite builds and passes on macOS, including the
  LÖVE font metrics and binding the macOS Discord library. Still untested: the release packaging jobs (they run
  only for a `v*` tag) and running the app on a Mac. CI warns that `actions/checkout@v4` uses the deprecated
  Node 20. Next: Phase 12 (parity audit).
- 2026-11-16: **Phase 12 complete.** Parity audit by function and string inventories against the Lua source, plus
  two new frame-driven test files covering `CONTROLS.md`'s table interactions and the features only real games use
  (packs, deck widget, scenarios, token bags). Found one wording difference (fixed). The other two failures the
  tests hit were the original's own behaviour (recorded in the quirks log). Every phase of the plan is now checked;
  what remains is hand testing: Discord sign-in and invites, a Mac, and the first tagged release.
- 2026-11-17: **First release `v0.2.0+39`** (`tools/package.py --publish`). The tag's CI run passed: tests on all
  three platforms, then the Windows, Linux and macOS packages were built and attached to the GitHub release
  (`cardtable-windows.zip` 15.6 MB, `cardtable-linux-x86_64.zip` 16.8 MB, `cardtable-macos.zip` 28.8 MB). The Windows
  zip has the expected flat layout. Not yet tried: installing from these downloads, the in-app updater against
  this real release, and opening the macOS app.
- 2026-11-17: **Renamed to GameTable** (see Decisions). The save folder is now `gametable`, filled once from
  `cardtable`/`cardflux` with the avatar path repointed (tested). Because development builds are now also named
  `gametable`, the updater only treats an executable as an install when its packaged `assets` folder is beside it
  (or in `Contents/Resources` on macOS). The Windows package was built and run: title bar, version resource and
  file names all say GameTable. The Home logo (`assets/logo.png` for dark, `logo_light.png` for light) is the
  user's new GameTable artwork (their own transparent PNG, trimmed to 881×813), the same image for both themes
  (checked by screenshot in each). Home now draws it as wide as the content below it when the window is tall
  enough, shrinking to fit, never below the original 220. The Discord application's name is set in Discord's developer
  portal, not in the code.
- 2026-11-17: **The live development server now runs the Rust server.** `tools/deploy-server.ps1` (see Build and
  tooling notes) replaced the Go `cardflux` service with a `gametable` service on the VM (Ubuntu 26.04, glibc
  2.43; the WSL build needs 2.34). The dry run and the switch both opened the live `cardflux.db` and all five games
  with no errors; a `hello` from outside returns the existing server settings (name, MOTD, collection mode). Backup
  of the Go-era database and binary: `~/backups/20261117-030725`. Not yet done: a sign-in by a real player.
- 2026-11-17: **GameTable colour scheme.** The Material green accents are replaced (`ui/theme.rs`): the light
  theme's accent is midnight blue `#191970` with pale neon cyan containers, the dark theme's is neon cyan `#00E5FF`
  with midnight blue containers, and the neutral surfaces have a cool blue tint instead of a green one. Table
  defaults (`data/display_settings.rs`): felt `#191970`, playmat border `#0A0A4A`, current turn `#00E5FF` (a
  game's or player's own colours still win). The fixed status greens are gone: the lobby's "In progress" chip,
  Join's "Connected" message and the shop's NEW badge use the theme accent; the deck editor's unused-copies
  badge is neon cyan. Checked by screenshot (Settings in both themes, the table).
- 2026-11-17: **Suggest Title in the game definition editor** (see "Beyond the port"). New crates `ocrs` + `rten`
  (approved; optimised in dev builds via `[profile.dev.package.*]`, unusable otherwise). The UI harness now records
  region rectangles (`Ui::recorded_regions`, on only in `UiHarness`) so tests can click any widget by id
  (`UiHarness::click_region`, which runs one settling frame first: a dialog's first frame lays out before it knows
  its height). Dev hook: `GAMETABLE_GAMEDEF_TAB=cards,select,suggest`. The Claude engine was tested by the user
  with their own key and works.
- 2026-11-17: **Release `v0.2.0+41`** (Suggest Title, colours, deploy script), the first release published to test
  the self-updater from an earlier GameTable install (`v0.2.0+40`).
- 2026-11-17: Tried scaling the whole UI with monitor resolution on a branch; the user found text too small at
  1080p, so the branch was deleted. Instead: a UI scale slider and a separate Text size slider (see "Beyond the
  port"), checked with screenshots at 100% and 150% text size.
- 2026-11-17: **Zones on playmats** (see "Beyond the port"). Placed in the Playmats tab and drawn on the mat at
  the table instead of the band or sidebar. Checked by screenshot with a scratch copy of Lorcana: the editor
  preview, and a 2-player practice table with both mats on.
- 2026-11-17: Hand areas made see-through (the table shows under them but can't be clicked there), and players can
  Ctrl-drag zones on their playmats, synced, remembered per game and mat (see "Beyond the port").
- 2026-11-17: Playmat hotkeys simplified: F11 (on/off) removed; F12 cycles off → each playmat → off
  (`assets/CONTROLS.md` updated). A mat that was off with a remembered index starts the cycle from "off".
- 2026-11-17: Hand checks confirmed by the user: installing from the published Windows zip and the in-app updater,
  signing in to the live Rust server, the GUI paths (token bags, file picker, folder change), sound playback, and
  dragging/resizing the borderless window on Linux. `proto` ticked (done long ago in `connection.rs`). Left to
  hand-test: Discord sign-in/invites and a Mac.
- 2026-11-17: **Title area for Suggest Title** (see "Beyond the port"). Drawn over the first card of the run and
  used for every card. Checked by screenshot (the dialog on Lorcana) and by real local OCR on two Lorcana cards.
- 2026-11-17: Suggest Title learns the game's name/subtitle separator (remembered per game once chosen), fixes
  all-capital titles to title case, and gives Claude the game's style and example titles.
- 2026-11-18: **Release `v0.2.0+42`** (playmat zones, Ctrl-drag zone positions, see-through hand areas, F12-only
  playmats, Suggest Title areas and title style). The tag's CI run passed on all three platforms and attached the
  Windows (18.8 MB), Linux (20.3 MB) and macOS (34.1 MB) zips. The live server was redeployed with the new
  `requestSetPlaymatZonePositions` handling (dry run first; backup `~/backups/20261118-041542`). The library re-save
  check matched apart from the user's own Lorcana `zonePlacements`, which the original Lua models drop.
- 2026-11-19: **Game Boards** (see "Beyond the port"): playmats renamed, each board a player or shared board,
  boards placed on the table in the editor's new Table Preview (turned in 90-degree steps, optionally
  toggleable), F12 only shows and hides toggleable boards, and with nothing placed a Player Board submenu picks
  the board F12 shows.
- 2026-11-19: Game Boards editor follow-ups (see "Beyond the port"): right-click Add Zone / Add To Table pickers,
  board right-click menu with Locked to Center for shared boards, centre + markers, zoomable Table Preview.
- 2026-11-20: Game Boards: player boards placed per player (Place Separately For Player N, Add For Player,
  Belongs To; `playerPlacements`), and players move and turn their own boards in the game by an unlockable
  handle (Q / E while dragging; `movedBoards`, `requestSetMovedBoard`). Tests: model round-trip, host engine
  permission, layout (own placement replaces the copy, mirrored viewers, moved boards), handle geometry, an
  editor harness test and a table harness test (unlock, drag, turn, reset, lock).
- 2026-11-20: Zones drop the card count badge (count moves into the tooltip) and the shuffle button (now a
  **Shuffle Deck** zone menu item for 2+ cards). Table piles unchanged.
- 2026-11-20: Boards carry the cards, counters and tokens lying on them when moved, turned or reset
  (`carried` on `requestSetMovedBoard`, `carry_with_board`); they ride along live while dragging.
- 2026-11-20: Fixed the one-frame flash of a dropped item at its old spot (table re-read after input).
- 2026-11-20: Scenarios' Card Placement draws the game boards as the table places them (Players count) and
  gets the Table Preview's wheel zoom, felt-drag pan and zoom controls.
- 2026-11-20: Scenario zones can be set at fixed spots on the table from the Card Placement canvas
  (right-click > Add Zone; `zonePlacements`); in the game they sit there and can't be moved.
- 2026-11-20: **Release `v0.2.0+43`** (game boards, boards carrying what lies on them, zone tooltips and Shuffle
  Deck, the drop flash fix, scenario Card Placement with boards, zoom/pan and zones set on the table). The tag's CI
  run passed on all three platforms and attached the Windows (18.0 MB), Linux (19.4 MB) and macOS (32.6 MB) zips.
  The live server was redeployed (dry run first: all six games loaded, no errors; backup
  `~/backups/20261121-042846`), so server games get `requestSetMovedBoard` with `carried` and keep
  `zonePlacements`.
- 2026-11-20: Card grids space their columns for turned (left/right) cards so they no longer overlap: at least 5 px
  between neighbouring card images when any card in the grid is turned (`table/card_pool_grid.rs::grid_columns`).
- 2026-11-20: **Suggest Extra Data** on the Card View bulk form and an **unsaved-changes prompt** in the game
  definition editor (see "Beyond the port"). Fixed `App::handle_event`: a native window close (`Event::Quit`, e.g.
  Alt+F4 with the native frame) was ignored unless a screen cancelled it, and then quit anyway. Not yet tried in
  the running app.
- 2026-11-20: Suggest Extra Data gets an optional "Instructions for Claude" box, remembered per game and field.
- 2026-11-21: **Shrink Card Art...** on the Card View (see "Beyond the port"). Button and confirmation checked by
  screenshot on Lorcana; the shrinking itself is covered by unit tests but not yet run from the app on a real game
  (the real library is read-only).
- 2026-11-21: Card View preview shows the card image's resolution (see "Beyond the port").
- 2026-11-21: **Convert to WebP...** on the Card View (see "Beyond the port"), tested on a scratch copy of Megaman.
- 2026-11-21: The Card View preview's resolution label also shows the image's file size.
- 2026-11-21: Add Set can shrink and convert the art to WebP as it imports (see "Beyond the port"); checked on a
  scratch copy of Megaman with 24 Lorcana PNGs (7.1 MB -> 1.2 MB, 24 cards pointing at the `.webp` files).
- 2026-11-21: **Save to Zip** on the editor toolbar (see "Beyond the port"); checked on the scratch Megaman copy:
  585 files, 68.5 MB `megaman.gamedef`, archive tests clean.
- 2026-11-21: **`.gamedef` packages** (see "Beyond the port"). Checked in the app on scratch libraries only: the
  Install prompt from a dropped file, MegaMan cards and backs played from its package, and a packaged copy of
  Arkham Horror (1 GB) showing its PDF rulebook, game board, tokens and built-in starter deck; the editor's package
  tiles, Extract prompt and the "plays from" title note. The server scanned a package (585 files). Not yet tried: a
  real drag from Explorer, a client downloading a packaged game from the lobby server, and Extract run from the app.
- 2026-11-21: Save to Zip crawled on Spellfire once it reached `_cache/atlases` (76 extension-less files, 1.3 GB,
  deflated by an unoptimized compressor). Those folders are now left out, and the zip crates build optimized in dev:
  116 MB of those atlases now pack in ~4.5 s.
- 2026-11-21: Convert to WebP no longer stalls at the end on big games: checking whether each original was still
  used walked every card of both games per file (quadratic, on the UI thread). Now `images_used_by_game` builds the
  set once, and the originals are deleted on a background thread behind a "Removing the original files: n of m"
  progress dialog (`ConvertToWebpFlow::finish`). Checked on a scratch copy of On The Edge (409 PNGs in four new
  sets, added to the real game's gamedef.json at the user's request; backup kept in the session scratchpad).
- 2026-11-21: **Release `v0.2.0+45`** (`.gamedef` packages and drag-to-install, Save to Zip, Add Set import
  options, Shrink Card Art, Convert to WebP and its responsive finish, image size labels). `v0.2.0+44` was tagged
  but never released: its Linux CI run caught a real server bug (a folder's `GameDef.json` is listed as
  `gamedef.json`, which Linux can't open by that name), now served from the bytes read while scanning. The `+45`
  run passed on all three platforms and attached the Windows (18.3 MB), Linux (19.7 MB) and macOS (33.2 MB) zips.
  The live server was redeployed (dry run first: all six games loaded, no errors; backup
  `~/backups/20261122-004558`), so it can now serve `.gamedef` packages.
- 2026-11-21: Server Administration **Upload New Game** accepts `.gamedef` packages as well as `.zip`: the picker
  shows both, the upload keeps the file's kind in its name (`install_<id>_<time>.gamedef`), and the server keeps a
  `.gamedef` as `<games>/<id>.gamedef` (same checks as a zip: one root folder, id match, safe paths, size limit, no
  existing game) instead of unzipping it. Older servers still unzip it. Tested in `tests/server_admin.rs`.
- 2026-11-21: The admin Users list shows **"Last seen <local date time>"** in Status for users who aren't online
  ("Never signed in" when unknown). The server records sign-ins and disconnects in a new `user_last_seen` table
  (its own table, so the users table keeps the Go server's columns and a rollback still works); users seen before
  it existed fall back to their sessions' last use. `adminUsers` sends `lastSeenAt` (unix seconds).
- 2026-11-21: **Release `v0.2.0+46`** (`.gamedef` admin uploads, last-seen times). The tag's CI run passed on all
  three platforms and attached the Windows (18.3 MB), Linux (19.7 MB) and macOS (33.2 MB) zips. The live server was
  redeployed (dry run first: all six games loaded, no errors; backup `~/backups/20261122-014612`).
- 2026-11-21: **Shrink Card Art** now shrinks WebP art too, re-encoding it lossy at quality 90 (it was written
  before the app had a WebP encoder, so WebP files were skipped). The summary's skip line reads "not PNG, JPEG or
  WebP".
- 2026-11-21: `v0.2.0+47` was tagged but its CI failed on macOS: the new WebP shrink test needed libwebp, which the
  test runner doesn't have. The WebP check moved to its own test that skips without libwebp, like the other WebP
  tests; released as `v0.2.0+48` (the +47 tag is left without a release).
