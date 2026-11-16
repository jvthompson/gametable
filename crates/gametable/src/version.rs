//! App version. Port of `src/version.lua`. `BUILD` is what the self-updater compares against
//! release tags (`+N`); the release details are used by the updater (Phase 11).

pub const VERSION_NAME: &str = "0.2.0";
pub const BUILD: u32 = 38;
pub const RELEASE_REPOSITORY: &str = "jvthompson/cardtable";
pub const RELEASE_ASSET_NAME: &str = "cardtable-windows.zip";
pub const EXECUTABLE_NAME: &str = "CardTable.exe";
