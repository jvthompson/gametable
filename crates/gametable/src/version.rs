//! App version. Port of `src/version.lua`. `BUILD` is what the self-updater compares against
//! release tags (`+N`); the release details are used by the updater (Phase 11). Each platform has
//! its own release download and executable name.

pub const VERSION_NAME: &str = "0.2.0";
pub const BUILD: u32 = 45;
/// The Rust build's own repository (the LÖVE app's releases are on `jvthompson/cardtable`).
pub const RELEASE_REPOSITORY: &str = "jvthompson/gametable";

/// The release download for this platform (the original only published the Windows one). All
/// are zips; the updater extracts them with the `zip` crate, keeping Unix permissions.
/// (Releases up to `v0.2.0+39` used the CardTable names: `cardtable-windows.zip` etc.)
pub const RELEASE_ASSET_NAME: &str = if cfg!(target_os = "windows") {
    "gametable-windows.zip"
} else if cfg!(target_os = "macos") {
    "gametable-macos.zip"
} else {
    "gametable-linux-x86_64.zip"
};

/// The executable inside an install: `GameTable.exe` beside its DLLs on Windows, `GameTable`
/// inside `GameTable.app/Contents/MacOS/` on macOS, `gametable` on Linux.
pub const EXECUTABLE_NAME: &str = if cfg!(target_os = "windows") {
    "GameTable.exe"
} else if cfg!(target_os = "macos") {
    "GameTable"
} else {
    "gametable"
};
