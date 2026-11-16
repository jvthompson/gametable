//! App version. Port of `src/version.lua`. `BUILD` is what the self-updater compares against
//! release tags (`+N`); the release details are used by the updater (Phase 11). Each platform has
//! its own release download and executable name.

pub const VERSION_NAME: &str = "0.2.0";
pub const BUILD: u32 = 38;
pub const RELEASE_REPOSITORY: &str = "jvthompson/cardtable";

/// The release download for this platform (the original only published the Windows one). All
/// are zips; the updater extracts them with the `zip` crate, keeping Unix permissions.
pub const RELEASE_ASSET_NAME: &str = if cfg!(target_os = "windows") {
    "cardtable-windows.zip"
} else if cfg!(target_os = "macos") {
    "cardtable-macos.zip"
} else {
    "cardtable-linux-x86_64.zip"
};

/// The executable inside an install: `CardTable.exe` beside its DLLs on Windows, `CardTable`
/// inside `CardTable.app/Contents/MacOS/` on macOS, `cardtable` on Linux.
pub const EXECUTABLE_NAME: &str = if cfg!(target_os = "windows") {
    "CardTable.exe"
} else if cfg!(target_os = "macos") {
    "CardTable"
} else {
    "cardtable"
};
