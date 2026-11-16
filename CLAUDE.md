# GameTable

GameTable is a project that aims to create a Rust port of CardTable, an app built using Lua and Love2D to simulate playing various trading card games.

Code should be written so that it is easily understood by readers of the code, with descriptive function names and variable names for everything. 

The original code for CardTable can be found at F:\Projects\cardflux-love if you need to reference anything. Its lobby server (written in Go) is in F:\Projects\cardflux-love\server.

## Port plan

The full plan and progress tracker is in PORT_PLAN.md. At the beginning of each new session, read PORT_PLAN.md to see what has been implemented and what comes next. Update its checkboxes, quirks log and session log as work is completed.

## Cargo crates

Let the user know about any cargo crates you would like to add, and wait for approval, before adding them to any Cargo.toml. Approved crates are recorded in the Crates table in PORT_PLAN.md.

Use SDL3 whenever it makes sense.
