# GameTable Controls

## Window

| Key | Effect |
|-----|--------|
| F6 | Toggle between the custom title bar and a standard window frame |

## Game table

### Mouse

| Action | Effect |
|--------|--------|
| Drag a card | Move it. Hold **Alt** to also carry along anything stacked visually on top of it as a group -- other cards, a loose card token, or any tokens attached to them. While dragged, a single card grows to hand size over your hand and shrinks to table size over the table. |
| Hover a card in your hand | Raise it out of the fan so you can read it |
| Click a pile (2+ stacked cards) | Draw the top card into your hand |
| Drag a pile | Peel off just the top card |
| Alt + drag a pile | Move the whole pile; dropped on your hand, every card is dealt into it |
| Drag a card token | Move it like a card -- it shares the same front-to-back ordering as cards, but never attaches to a card and can't be dropped into a hand or zone. Hold **Alt** to carry along anything stacked on top of it (cards or other card tokens) as a group. Only its owner (whoever spawned it) can interact with it at all. |
| Pile's shuffle button | Shuffle the pile and turn every card face-down |
| Hover a zone | Its name and how many cards it holds |
| Click a player's avatar | Practice games: act as that seat |
| Mouse wheel over the table | Zoom in/out around the cursor |
| Middle-button drag on the table | Pan your own camera |
| Tab + drag on the table | Draw an arrow everyone can see (fades after 5 seconds) |
| Drag a game board's handle | Move one of your own player boards, with the zones on it and the cards, counters and tokens lying on it (they keep their place on the board and turn with it; other players' cards stay put); everyone sees where you leave it. Press **Q** / **E** while dragging to turn it a quarter turn counter-clockwise / clockwise. The handle shows after you right-click the board and pick **Unlock Board**; **Lock Board** hides it again and **Reset Board Position** puts the board back |
| Drag on empty table | Box select: every card, pile and token of yours the box touches is selected (tinted in the selection color, set in **Settings > Table**) |
| Shift + drag on empty table | Add to the selection |
| Ctrl + click a card, pile or token | Add it to or remove it from the selection |
| Click empty table | Clear the selection |
| Drag a selected item | Move the whole selection; it drops like a single card (into your hand, onto a zone, Alt onto a card to pile everything onto it) |
| Right-click | Context menu (see below) |

**Dropping a dragged card**

- On your hand (including the part of the fan that overlaps the table): inserts it between the cards it was dropped between, whether it came from the table, a zone or elsewhere in your hand. A whole pile (Alt + drag) goes in at that spot, in pile order.
- On one of your zones: puts it on top of the zone; hold **Alt** to put it on the bottom.
- On a shared pile on the table: stacks onto it.
- Near another loose card with **Alt** held: stacks onto it. Piles never mix owners: your cards only stack onto your own piles and unowned cards onto unowned piles; otherwise the card stays where it was.
- Anywhere else: moves it on the table. Anything that would end up partly off the table is pulled back fully onto it.

A zone a loaded scenario sets on the table works like any shared zone (draw from it, drop onto it, right-click it), but stays where the scenario put it: it can't be moved during the game.

Opponents' cards, piles and zones are read-only: you can hover them (for the Space preview) but not draw, drag, shuffle or flip them.

### Keyboard

Hotkeys act on the card (or token, for Q / E; a card token also responds to Z) under the cursor and fire once per key press (holding a key doesn't repeat). F, Q / E, X and Z on a selected card act on the whole selection.

| Key | Effect | Applies to |
|-----|--------|------------|
| Space (hold) | Large preview of the hovered card, showing exactly what you can see of it | Any card |
| F | Flip face-up/face-down (a flipped token has no backside, so it just goes greyscale) | Your table cards, your hand, a shared zone's top card, your own zone's top card if the editor marked that zone Flippable, or a hovered token |
| Q / E | Rotate 90° counter-clockwise / clockwise (hold Ctrl for 45° steps) | Your cards on the table, or a hovered token |
| X | Send to your discard zone | Your cards on the table, if the game defines a discard pile |
| Z | Send the card's stack to the back, or a hovered card token behind the cards and tokens in front of it | Your cards on the table, or a hovered card token |
| 1–9 | Draw that many cards into your hand (1 on a table card moves just that card to hand) | Your piles, your zones and shared zones |
| W / A / S / D (hold) | Pan your own camera | Only your view moves |
| Home or H | Reset camera pan and zoom | |
| F1 | Show/hide the ownership border on every card | This session only |
| F2 | Toggle zone/hand backgrounds between player-color tint and black | This session only |
| F12 | Show/hide the game boards marked "Can be toggled" on the Game Boards tab (boards placed without it always stay). With no boards placed on the table, shows/hides your player board; pick which one from the table's right-click menu (Player Board). You toggle your own player boards; the host also toggles shared boards | Persists across sessions |
| L | Open/close the Game Log (with a Save Log button) | |
| C | Open/close the chat panel (server games, and practice games for /flip and /roll -- see Chat commands below; not in LAN host/join games) | |
| Ctrl + R | Open/close the Documents viewer (if the game has any PDF/Markdown file in its _documents folder), with a document picker and a collapsible table-of-contents sidebar. Also works in the Deck Editor and the Game Definition Editor. | |
| Esc | Clear the selection; with nothing selected, open/close the Game Menu (save game, settings, controls, kick a player, leave/end game) | |
| Ctrl (while dropping a token) | Duplicate the token instead of moving it | Token widgets |

### Chat commands

Typed into any chat box (lobby, room, or the in-game chat panel) on a GameTable server. /flip and /roll also work in a practice game's chat panel, run locally with no server involved; anything else typed there is just kept as a personal note, since there's no one else at the table to read it.

| Command | Effect |
|---------|--------|
| /flip | Flips a coin and posts "flipped: Heads" or "flipped: Tails" |
| /roll dX | Rolls a die with X sides (e.g. `/roll d6`, `/roll d20`) and posts "rolled dX: &lt;result&gt;" |
| /clear | Lobby chat only, server admins only: clears the lobby chat for everyone |

### Right-click menus

| Target | Menu |
|--------|------|
| Empty table | **Widgets** (Simple Counter, Token, Pack Generator, Deck Widget when the game has deck-building zones), **Card Library**, and **Pass Turn To** a player (marks their avatar as the current player; cosmetic only) |
| Your card | **Send to...** Hand / Discard / a zone (Top, Shuffle Into, Bottom); on the table also **Flip**, **Rotate**, **Give to** a player and **Remove Ownership** (both ask first; not offered for a card in a pile) |
| A selected card (2+ items selected) | The same actions for every selected card, plus **Create Pile Here** (piles them all at the click, in their current order) |
| Your pile | Same as a card, plus **Search...** |
| A zone | **Search...**; **Shuffle Deck** (shuffles it and turns every card face-down) when it holds 2+ cards and can be shuffled; an empty zone that deals a built deck also offers **Load Deck...** |
| Simple Counter | Increment / Decrement / Set Value / Set Colors / Delete |
| Token | Set Color / Delete |
| Pack Generator | Open a pack from one of the game's sets / Delete |
| Your Deck Widget | Delete |

### Your hand

Your hand is drawn as a fan of cards along the bottom of the screen, and opponents' hands as fans of card backs. The shape is adjustable in **Settings > Hand**: card size, card spacing, tilt between cards, maximum fan spread, how far the resting fan sits above the hand area, and how much of a hovered card is shown. Changes apply live and are only on your screen.

### Board widgets

- Drag any widget to move it.
- **Token:** drop it on a card to attach it (it then travels with the card); drop it on empty table to detach it. Hold **Ctrl** while dropping to duplicate it.
- **Simple Counter:** double-click the right half to add 1, the left half to subtract 1.

## Deck Editor

| Action | Effect |
|--------|--------|
| Left-click a card in the pool | Add one copy to the deck |
| Right-click a card in the pool | Remove one copy |
| Click a card in the deck list | Remove one copy |
| Hover | Preview the card |
| Ctrl + R | Open/close the Documents viewer |

## Game Definition Editor

| Action | Effect |
|--------|--------|
| Rulebook toolbar button, or Ctrl + R | Open/close the Documents viewer (if the game has any PDF/Markdown file in its _documents folder) |
| Mouse wheel over a table canvas | Zoom in/out around the cursor (Game Boards' Table Preview and Scenarios' Card Placement); the zoom buttons and percentage sit above the canvas, and clicking the percentage fits the whole table again |
| Drag empty felt, or middle-button drag | Pan a zoomed table canvas |
| Players (Scenarios > Card Placement) | Lays the game boards out for that many seats, exactly where the table puts them for player 1, under the placed cards |
| Right-click the Card Placement canvas > Add Zone | Sets one of the scenario's zones on the table where you clicked. Drag it anywhere on the table; select it and press Delete (or the Delete button) to take it off the table again. Once the scenario is loaded the zone sits at that spot for everyone and can't be moved during the game |
