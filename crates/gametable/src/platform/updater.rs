//! The self-updater. Port of `src/platform/updater.lua` and `update_worker.lua`: GitHub Releases
//! are checked once per launch, and on request the release package is downloaded and extracted
//! on a worker thread, then installed by a helper while the app is closed.
//!
//! The original's helper was a detached PowerShell script that ran `robocopy /MIR`. Here the
//! *new* executable itself is the helper, on every platform: it is started from the extracted
//! update with `--apply-update`, waits for the running app to exit (its standard input closes),
//! mirrors the new files over the install (leaving the game and deck libraries alone), starts the
//! updated app and exits. Nothing touches the install until the app has quit, so a failure before
//! that leaves the current install intact.
//!
//! Install layouts: Windows and Linux are a folder holding the executable; macOS is
//! `CardTable.app` (the executable is `CardTable.app/Contents/MacOS/CardTable`). Release packages
//! are zips of the same layout.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gametable_core::json::{self, JsonValue};

use crate::version::{BUILD, EXECUTABLE_NAME, RELEASE_ASSET_NAME, RELEASE_REPOSITORY};

/// Temporary folders for downloaded updates start with this.
const TEMPORARY_FOLDER_PREFIX: &str = "cardtable_update_";
/// The command-line switch that runs the install helper.
pub const APPLY_UPDATE_SWITCH: &str = "--apply-update";
/// GitHub rejects API requests without a User-Agent. Generic on purpose.
const USER_AGENT: &str = "CardTable-updater";
/// See [`start_check`]. The helper clears it (and `GAMETABLE_HOME_OPEN`) for the relaunched app.
const TEST_URL_VARIABLE: &str = "GAMETABLE_UPDATE_TEST_URL";

// ---------------------------------------------------------------------------------------------
// Release selection (pure)
// ---------------------------------------------------------------------------------------------

/// A release newer than this build, with this platform's package.
#[derive(Debug, Clone, PartialEq)]
pub struct AvailableRelease {
    pub tag: String,
    pub url: String,
    pub size: Option<u64>,
}

/// `U.buildFromTag`: the build number from a release tag like `v0.1.0+49`.
pub fn build_from_tag(tag: &str) -> Option<u32> {
    let (_, build) = tag.rsplit_once('+')?;
    if build.is_empty() || !build.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    build.parse().ok()
}

/// `U.pickRelease`: the release from a `/releases/latest` response when it is newer than
/// `local_build` and carries `asset_name`.
pub fn pick_release(release: &JsonValue, local_build: u32, asset_name: &str) -> Option<AvailableRelease> {
    let tag = release.get("tag_name")?.as_str()?;
    let remote_build = build_from_tag(tag)?;
    if remote_build <= local_build {
        return None;
    }
    release.get("assets")?.as_array()?.iter().find_map(|asset| {
        if asset.get("name")?.as_str()? != asset_name {
            return None;
        }
        let url = asset.get("browser_download_url")?.as_str()?;
        Some(AvailableRelease {
            tag: tag.to_string(),
            url: url.to_string(),
            size: asset.get("size").and_then(JsonValue::as_f64).map(|size| size as u64),
        })
    })
}

/// `U.pathsInside`: the candidate folders that sit inside `install_folder` (so the mirror step
/// leaves them alone). Compared ignoring case on Windows.
pub fn paths_inside(install_folder: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let comparable = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_string();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    let root = comparable(install_folder);
    candidates
        .iter()
        .filter(|candidate| {
            let candidate = comparable(candidate);
            candidate.len() > root.len() + 1 && candidate.starts_with(&root) && candidate[root.len()..].starts_with('/')
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Install layout
// ---------------------------------------------------------------------------------------------

/// The executable inside an install (or an extracted package) rooted at `root`.
pub fn executable_in(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("CardTable.app").join("Contents").join("MacOS").join(EXECUTABLE_NAME)
    } else {
        root.join(EXECUTABLE_NAME)
    }
}

/// The folder an update replaces: the executable's folder, or the `.app` bundle on macOS.
fn install_root_of(executable: &Path) -> Option<PathBuf> {
    let folder = executable.parent()?;
    if cfg!(target_os = "macos") {
        // CardTable.app/Contents/MacOS/CardTable -> CardTable.app
        let bundle = folder.parent()?.parent()?;
        return (bundle.extension().is_some_and(|extension| extension == "app")).then(|| bundle.to_path_buf());
    }
    Some(folder.to_path_buf())
}

/// `U.supported()`: only a packaged build checks for or installs updates (a development run's
/// executable is `gametable` inside `target/`). The packaged executable is named
/// [`EXECUTABLE_NAME`].
pub fn is_packaged_install() -> bool {
    let Ok(executable) = std::env::current_exe() else { return false };
    let named_like_a_package =
        executable.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(EXECUTABLE_NAME));
    named_like_a_package && install_root_of(&executable).is_some()
}

// ---------------------------------------------------------------------------------------------
// The update check
// ---------------------------------------------------------------------------------------------

static AVAILABLE: OnceLock<Mutex<Option<AvailableRelease>>> = OnceLock::new();

fn available_slot() -> &'static Mutex<Option<AvailableRelease>> {
    AVAILABLE.get_or_init(|| Mutex::new(None))
}

/// The release found by [`start_check`], if any (read by Home's banner).
pub fn available_release() -> Option<AvailableRelease> {
    available_slot().lock().ok().and_then(|slot| slot.clone())
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn fetch_latest_release() -> Result<Option<AvailableRelease>, String> {
    let url = format!("https://api.github.com/repos/{RELEASE_REPOSITORY}/releases/latest");
    let mut response =
        http_agent().get(&url).header("Accept", "application/vnd.github+json").call().map_err(|error| match error {
            ureq::Error::StatusCode(status) => format!("HTTP {status}"),
            other => other.to_string(),
        })?;
    let body = response.body_mut().read_to_string().map_err(|error| error.to_string())?;
    let release = json::decode(&body).map_err(|error| format!("bad release JSON: {error:?}"))?;
    Ok(pick_release(&release, BUILD, RELEASE_ASSET_NAME))
}

/// `U.check(force)`: looks for a newer release in the background (best effort). `force` checks
/// even in a development run (`--update-check`).
pub fn start_check(force: bool) {
    // Development hook: `GAMETABLE_UPDATE_TEST_URL=<zip URL>` offers that package as release
    // v9.9.9+9999 (for testing the whole update flow against a local server).
    if let Ok(url) = std::env::var(TEST_URL_VARIABLE) {
        if let Ok(mut slot) = available_slot().lock() {
            *slot = Some(AvailableRelease { tag: "v9.9.9+9999".to_string(), url, size: None });
        }
        return;
    }
    if !force && !is_packaged_install() {
        return;
    }
    let _ = std::thread::Builder::new().name("update-check".to_string()).spawn(|| match fetch_latest_release() {
        Ok(found) => {
            if let Ok(mut slot) = available_slot().lock() {
                *slot = found;
            }
        }
        Err(message) => eprintln!("Update check: {message}"),
    });
}

// ---------------------------------------------------------------------------------------------
// Downloading and staging an update
// ---------------------------------------------------------------------------------------------

/// How an update is going (`phase`, `progress` in the original).
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateProgress {
    /// The fraction downloaded, when the size is known.
    Downloading(Option<f32>),
    Extracting,
    Verifying,
    LaunchingInstaller,
    /// The helper is running: quit now so it can replace the files.
    ReadyToQuit,
    Failed(String),
}

/// A running `U.apply`.
pub struct UpdateJob {
    messages: Receiver<UpdateProgress>,
    last: UpdateProgress,
}

impl UpdateJob {
    /// `U.apply`: downloads and stages `release`, then launches the helper. `keep_folders` are
    /// the game and deck libraries, kept when they live inside the install.
    pub fn start(release: AvailableRelease, keep_folders: Vec<PathBuf>) -> UpdateJob {
        let (sender, messages) = mpsc::channel();
        let failure = sender.clone();
        let spawned = std::thread::Builder::new().name("update-apply".to_string()).spawn(move || {
            if let Err(message) = download_and_launch(&release, &keep_folders, &sender) {
                let _ = sender.send(UpdateProgress::Failed(message));
            }
        });
        if let Err(error) = spawned {
            let _ = failure.send(UpdateProgress::Failed(error.to_string()));
        }
        UpdateJob { messages, last: UpdateProgress::Downloading(Some(0.0)) }
    }

    pub fn poll(&mut self) -> UpdateProgress {
        loop {
            match self.messages.try_recv() {
                Ok(progress) => self.last = progress,
                Err(TryRecvError::Empty) => return self.last.clone(),
                Err(TryRecvError::Disconnected) => {
                    if !matches!(self.last, UpdateProgress::ReadyToQuit | UpdateProgress::Failed(_)) {
                        self.last = UpdateProgress::Failed("The update stopped unexpectedly.".to_string());
                    }
                    return self.last.clone();
                }
            }
        }
    }
}

fn download(url: &str, expected_size: Option<u64>, sender: &mpsc::Sender<UpdateProgress>) -> Result<Vec<u8>, String> {
    let mut response = http_agent().get(url).call().map_err(|error| match error {
        ureq::Error::StatusCode(status) => format!("Update download failed with HTTP {status}."),
        other => format!("Update download failed: {other}"),
    })?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .or(expected_size);
    let mut reader = response.body_mut().as_reader();
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut last_report = Instant::now();
    loop {
        let count = reader.read(&mut chunk).map_err(|error| format!("Update download failed: {error}"))?;
        if count == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..count]);
        if last_report.elapsed() > Duration::from_millis(100) {
            last_report = Instant::now();
            let fraction = total.filter(|total| *total > 0).map(|total| data.len() as f32 / total as f32);
            let _ = sender.send(UpdateProgress::Downloading(fraction.map(|fraction| fraction.min(1.0))));
        }
    }
    Ok(data)
}

/// Extracts a zip into `destination`, keeping Unix permissions (executables) where recorded.
pub fn extract_zip(bytes: &[u8], destination: &Path) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| format!("Bad update package: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| format!("Bad update package: {error}"))?;
        // `enclosed_name` rejects absolute paths and `..` components.
        let Some(relative) = entry.enclosed_name() else { continue };
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = std::fs::File::create(&target).map_err(|error| format!("{}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut file).map_err(|error| format!("{}: {error}", target.display()))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o7777));
        }
    }
    Ok(())
}

fn download_and_launch(
    release: &AvailableRelease,
    keep_folders: &[PathBuf],
    sender: &mpsc::Sender<UpdateProgress>,
) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let install_root = install_root_of(&executable).ok_or("Updates can only be installed from a packaged build.")?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let temporary_folder = std::env::temp_dir().join(format!("{TEMPORARY_FOLDER_PREFIX}{now}"));
    std::fs::create_dir_all(&temporary_folder)
        .map_err(|error| format!("Couldn't create {}: {error}", temporary_folder.display()))?;

    let _ = sender.send(UpdateProgress::Downloading(Some(0.0)));
    let bytes = download(&release.url, release.size, sender)?;

    let _ = sender.send(UpdateProgress::Extracting);
    let staged = temporary_folder.join("staged");
    extract_zip(&bytes, &staged)?;
    drop(bytes);

    let _ = sender.send(UpdateProgress::Verifying);
    let staged_executable = executable_in(&staged);
    if !staged_executable.is_file() {
        return Err(format!("Downloaded update package is missing {EXECUTABLE_NAME} -- aborting."));
    }
    // macOS: the package holds CardTable.app; mirror that bundle over the installed one.
    let staged_root = if cfg!(target_os = "macos") { staged.join("CardTable.app") } else { staged.clone() };

    let _ = sender.send(UpdateProgress::LaunchingInstaller);
    let mut command = std::process::Command::new(&staged_executable);
    command
        .arg(APPLY_UPDATE_SWITCH)
        .arg("--staged")
        .arg(&staged_root)
        .arg("--install")
        .arg(&install_root)
        .arg("--log")
        .arg(temporary_folder.join("apply_update.log"));
    for folder in paths_inside(&install_root, keep_folders) {
        command.arg("--keep").arg(folder);
    }
    // The helper waits for its standard input to close, which happens when this process exits.
    command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let child = spawn_detached(&mut command).map_err(|error| format!("Failed to launch the update helper: {error}"))?;
    // Keep the pipe open for the rest of this process's life.
    if let Some(stdin) = child.stdin {
        std::mem::forget(stdin);
    }
    let _ = sender.send(UpdateProgress::ReadyToQuit);
    Ok(())
}

/// Starts a process that outlives this one.
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Leave any job object the app runs in (a launcher may kill it with the app); not every
        // job allows that, so fall back to staying in it.
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if let Ok(child) = command.spawn() {
            return Ok(child);
        }
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        command.spawn()
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a terminal's Ctrl+C or hang-up doesn't reach it.
        command.process_group(0);
        command.spawn()
    }
}

/// Removes update folders left in the temporary folder by earlier updates (the helper can't
/// delete the folder it runs from). Best effort.
pub fn remove_leftover_update_folders() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with(TEMPORARY_FOLDER_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The install helper (`--apply-update`)
// ---------------------------------------------------------------------------------------------

/// What the helper was asked to do.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HelperRequest {
    pub staged: PathBuf,
    pub install: PathBuf,
    pub keep: Vec<PathBuf>,
    pub log: Option<PathBuf>,
}

/// Parses `--apply-update --staged S --install I [--keep K]... [--log L]`. `None` when the
/// arguments aren't a helper request.
pub fn parse_helper_request(arguments: &[String]) -> Option<HelperRequest> {
    if arguments.first().map(String::as_str) != Some(APPLY_UPDATE_SWITCH) {
        return None;
    }
    let mut request = HelperRequest::default();
    let mut rest = arguments[1..].iter();
    while let Some(name) = rest.next() {
        let value = PathBuf::from(rest.next()?);
        match name.as_str() {
            "--staged" => request.staged = value,
            "--install" => request.install = value,
            "--keep" => request.keep.push(value),
            "--log" => request.log = Some(value),
            _ => return None,
        }
    }
    (!request.staged.as_os_str().is_empty() && !request.install.as_os_str().is_empty()).then_some(request)
}

/// Retries a file operation that can fail briefly while the old app's files are still locked.
fn with_retries<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() > deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn is_kept(path: &Path, keep: &[PathBuf]) -> bool {
    let comparable = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    let path = comparable(path);
    keep.iter().any(|kept| comparable(kept) == path)
}

/// `robocopy staged install /MIR /XD keep...`: makes `install` match `staged`, copying every
/// file over and deleting what the new version doesn't have, except the kept folders.
pub fn mirror_folder(staged: &Path, install: &Path, keep: &[PathBuf], log: &mut dyn Write) -> std::io::Result<()> {
    std::fs::create_dir_all(install)?;
    let mut staged_names = std::collections::HashSet::new();
    for entry in std::fs::read_dir(staged)? {
        let entry = entry?;
        let name = entry.file_name();
        staged_names.insert(name.clone());
        let (from, to) = (entry.path(), install.join(&name));
        if is_kept(&to, keep) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            if to.is_file() {
                with_retries(|| std::fs::remove_file(&to))?;
            }
            mirror_folder(&from, &to, keep, log)?;
        } else {
            if to.is_dir() {
                with_retries(|| std::fs::remove_dir_all(&to))?;
            }
            with_retries(|| std::fs::copy(&from, &to))?;
            let _ = writeln!(log, "copied {}", to.display());
        }
    }
    for entry in std::fs::read_dir(install)? {
        let entry = entry?;
        let path = entry.path();
        if staged_names.contains(&entry.file_name()) || is_kept(&path, keep) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            with_retries(|| std::fs::remove_dir_all(&path))?;
        } else {
            with_retries(|| std::fs::remove_file(&path))?;
        }
        let _ = writeln!(log, "removed {}", path.display());
    }
    Ok(())
}

/// Runs the install helper: waits for the app to exit, mirrors the update in, starts the
/// updated app. Returns the process exit code.
pub fn run_helper(request: &HelperRequest) -> i32 {
    let mut log: Box<dyn Write> = match request.log.as_ref().and_then(|path| std::fs::File::create(path).ok()) {
        Some(file) => Box::new(file),
        None => Box::new(std::io::sink()),
    };
    let _ = writeln!(log, "update helper: {request:?}");
    // The app holds our standard input open until it exits.
    let mut buffer = [0u8; 64];
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    while matches!(input.read(&mut buffer), Ok(count) if count > 0) {}
    let _ = writeln!(log, "the app has exited");
    if let Err(error) = mirror_folder(&request.staged, &request.install, &request.keep, &mut log) {
        let _ = writeln!(log, "mirror failed: {error}");
        return 1;
    }
    let executable = if cfg!(target_os = "macos") {
        request.install.join("Contents").join("MacOS").join(EXECUTABLE_NAME)
    } else {
        request.install.join(EXECUTABLE_NAME)
    };
    let mut command = std::process::Command::new(&executable);
    command.current_dir(&request.install).env_remove(TEST_URL_VARIABLE).env_remove("GAMETABLE_HOME_OPEN");
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match spawn_detached(&mut command) {
        Ok(_) => {
            let _ = writeln!(log, "started {}", executable.display());
            0
        }
        Err(error) => {
            let _ = writeln!(log, "couldn't start {}: {error}", executable.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_release_selection() {
        assert_eq!(build_from_tag("v0.1.0+49"), Some(49));
        assert_eq!(build_from_tag("v0.1.0"), None);
        assert_eq!(build_from_tag("v0.1.0+x"), None);
        let release = json::decode(
            r#"{"tag_name":"v0.3.0+40","assets":[{"name":"other.zip","browser_download_url":"u1","size":1},
            {"name":"cardtable-windows.zip","browser_download_url":"https://x/y.zip","size":1234}]}"#,
        )
        .unwrap();
        assert_eq!(
            pick_release(&release, 38, "cardtable-windows.zip"),
            Some(AvailableRelease { tag: "v0.3.0+40".into(), url: "https://x/y.zip".into(), size: Some(1234) })
        );
        assert_eq!(pick_release(&release, 40, "cardtable-windows.zip"), None, "not newer");
        assert_eq!(pick_release(&release, 1, "cardtable-macos.zip"), None, "no asset for this platform");
    }

    #[test]
    fn helper_arguments() {
        let arguments: Vec<String> =
            ["--apply-update", "--staged", "/t/s", "--install", "/a", "--keep", "/a/game_library", "--log", "/t/l"]
                .map(String::from)
                .to_vec();
        let request = parse_helper_request(&arguments).unwrap();
        assert_eq!(request.staged, PathBuf::from("/t/s"));
        assert_eq!(request.keep, vec![PathBuf::from("/a/game_library")]);
        assert!(parse_helper_request(&["--identity".to_string(), "x".to_string()]).is_none());
        assert!(parse_helper_request(&["--apply-update".to_string(), "--staged".to_string()]).is_none());
    }

    #[test]
    fn libraries_inside_the_install_are_kept() {
        let root = PathBuf::from(if cfg!(windows) { "C:\\Apps\\CardTable" } else { "/apps/CardTable" });
        let inside = root.join("game_library");
        let outside = PathBuf::from(if cfg!(windows) { "D:\\Games" } else { "/games" });
        let sibling = PathBuf::from(if cfg!(windows) { "C:\\Apps\\CardTable2" } else { "/apps/CardTable2" });
        assert_eq!(paths_inside(&root, &[inside.clone(), outside, sibling]), vec![inside]);
    }

    #[test]
    fn mirror_replaces_files_and_keeps_libraries() {
        let root = std::env::temp_dir().join(format!("gametable-mirror-{}", gametable_core::uuid::new_v4_uuid()));
        let (staged, install) = (root.join("staged"), root.join("install"));
        std::fs::create_dir_all(staged.join("assets")).unwrap();
        std::fs::write(staged.join("app.bin"), "new").unwrap();
        std::fs::write(staged.join("assets").join("a.png"), "a2").unwrap();
        std::fs::create_dir_all(install.join("assets")).unwrap();
        std::fs::create_dir_all(install.join("game_library").join("g")).unwrap();
        std::fs::write(install.join("app.bin"), "old").unwrap();
        std::fs::write(install.join("old.dll"), "x").unwrap();
        std::fs::write(install.join("assets").join("gone.png"), "x").unwrap();
        std::fs::write(install.join("game_library").join("g").join("gamedef.json"), "{}").unwrap();
        let keep = vec![install.join("game_library")];
        mirror_folder(&staged, &install, &keep, &mut std::io::sink()).unwrap();
        assert_eq!(std::fs::read_to_string(install.join("app.bin")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(install.join("assets").join("a.png")).unwrap(), "a2");
        assert!(!install.join("old.dll").exists());
        assert!(!install.join("assets").join("gone.png").exists());
        assert!(install.join("game_library").join("g").join("gamedef.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zips_extract_safely() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            writer.start_file("CardTable", options).unwrap();
            writer.write_all(b"binary").unwrap();
            writer.start_file("assets/fonts/a.ttf", zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(b"font").unwrap();
            writer.start_file("../escape.txt", zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(b"no").unwrap();
            writer.finish().unwrap();
        }
        let root = std::env::temp_dir().join(format!("gametable-unzip-{}", gametable_core::uuid::new_v4_uuid()));
        extract_zip(buffer.get_ref(), &root.join("out")).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("out").join("assets").join("fonts").join("a.ttf")).unwrap(),
            "font"
        );
        assert!(!root.join("escape.txt").exists(), "paths outside the folder are skipped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("out").join("CardTable")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "executable bit kept");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
