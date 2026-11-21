//! "Shrink Card Art" on the Cards tab (a GameTable addition): rewrites every card's art, and
//! every unique `_BACK` image, so none is taller than [`MAX_CARD_ART_HEIGHT`] pixels. Each image
//! keeps its own aspect ratio and file format; images already short enough are left alone.
//!
//! The flow is a confirmation, then a progress dialog while worker threads shrink the files,
//! then a summary. Card definitions don't change, only the image files on disk.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use gametable_core::models::{GameDefinition, UNIQUE_CARD_BACK_ID};

use crate::app::Frame;
use crate::data::image_resolver::{resolve_bare_image_path, unique_card_back_path_for};
use crate::data::image_utils::{ShrinkOutcome, shrink_image_to_max_height};
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{ConfirmResponse, DialogAction, DialogResponse, DialogSpec};

/// The tallest card art may be after shrinking, in pixels.
pub const MAX_CARD_ART_HEIGHT: u32 = 1024;

/// The most images shrunk at once.
const MAX_WORKER_THREADS: usize = 4;

/// Every image file the game's cards show (fronts, and unique backs that exist), resolved to
/// full paths, each once.
pub fn card_art_paths(folder_path: &str, game: &GameDefinition) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for card in game.cards() {
        let Some(bare_image_path) = &card.image_path else { continue };
        let front_path = resolve_bare_image_path(folder_path, bare_image_path, card.set_id.as_deref());
        if card.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID) {
            let unique_back_path = unique_card_back_path_for(&front_path);
            if Path::new(&unique_back_path).is_file() && seen.insert(unique_back_path.clone()) {
                paths.push(unique_back_path);
            }
        }
        if seen.insert(front_path.clone()) {
            paths.push(front_path);
        }
    }
    paths
}

/// What happened to one image file.
#[derive(Debug, Clone, PartialEq)]
enum FileResult {
    Processed(ShrinkOutcome),
    Missing,
    Failed(String),
}

fn shrink_one_file(path: &str) -> FileResult {
    if !Path::new(path).is_file() {
        return FileResult::Missing;
    }
    match shrink_image_to_max_height(path, MAX_CARD_ART_HEIGHT) {
        Ok(outcome) => FileResult::Processed(outcome),
        Err(error) => FileResult::Failed(error),
    }
}

/// Worker threads taking paths from a shared queue until it is empty or the job is cancelled.
struct ShrinkJob {
    total: usize,
    results: Vec<(String, FileResult)>,
    receiver: Receiver<(String, FileResult)>,
    cancelled: Arc<AtomicBool>,
    /// Threads still running; each sends `()` through `finished_receiver` as it ends.
    running_threads: usize,
    finished_receiver: Receiver<()>,
    /// How many of `results` have had their cached textures dropped.
    results_reloaded: usize,
}

impl ShrinkJob {
    fn start(paths: Vec<String>) -> ShrinkJob {
        let total = paths.len();
        let queue = Arc::new(Mutex::new(VecDeque::from(paths)));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let (finished_sender, finished_receiver) = mpsc::channel();
        let processors = std::thread::available_parallelism().map_or(2, |count| count.get());
        let thread_count = processors.saturating_sub(1).clamp(1, MAX_WORKER_THREADS).min(total.max(1));
        for index in 0..thread_count {
            let queue = queue.clone();
            let cancelled = cancelled.clone();
            let sender = sender.clone();
            let finished_sender = finished_sender.clone();
            std::thread::Builder::new()
                .name(format!("shrink-card-art-{index}"))
                .spawn(move || {
                    while !cancelled.load(Ordering::Relaxed) {
                        let next_path = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                        let Some(path) = next_path else { break };
                        let result = shrink_one_file(&path);
                        if sender.send((path, result)).is_err() {
                            break;
                        }
                    }
                    let _ = finished_sender.send(());
                })
                .expect("spawn card art shrinker");
        }
        ShrinkJob {
            total,
            results: Vec::new(),
            receiver,
            cancelled,
            running_threads: thread_count,
            finished_receiver,
            results_reloaded: 0,
        }
    }

    /// Collects finished files; true once every thread has stopped.
    fn poll(&mut self) -> bool {
        while let Ok(result) = self.receiver.try_recv() {
            self.results.push(result);
        }
        while self.finished_receiver.try_recv().is_ok() {
            self.running_threads -= 1;
        }
        if self.running_threads == 0 {
            // Anything sent between the two drains above.
            while let Ok(result) = self.receiver.try_recv() {
                self.results.push(result);
            }
            return true;
        }
        false
    }
}

/// Counts for the closing summary.
#[derive(Debug, Default, PartialEq)]
struct ShrinkSummary {
    shrunk: usize,
    already_small_enough: usize,
    unsupported_format: usize,
    missing: usize,
    failed: usize,
    first_error: Option<String>,
}

impl ShrinkSummary {
    fn from_results(results: &[(String, FileResult)]) -> ShrinkSummary {
        let mut summary = ShrinkSummary::default();
        for (path, result) in results {
            match result {
                FileResult::Processed(ShrinkOutcome::Shrunk { .. }) => summary.shrunk += 1,
                FileResult::Processed(ShrinkOutcome::AlreadySmallEnough) => summary.already_small_enough += 1,
                FileResult::Processed(ShrinkOutcome::UnsupportedFormat) => summary.unsupported_format += 1,
                FileResult::Missing => summary.missing += 1,
                FileResult::Failed(error) => {
                    summary.failed += 1;
                    summary.first_error.get_or_insert_with(|| format!("{path}: {error}"));
                }
            }
        }
        summary
    }

    fn message(&self, stopped_early: Option<(usize, usize)>) -> String {
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        let mut lines = vec![format!("Shrunk {} image{}.", self.shrunk, plural(self.shrunk))];
        if self.already_small_enough > 0 {
            lines.push(format!(
                "{} image{} already {MAX_CARD_ART_HEIGHT}px tall or less.",
                self.already_small_enough,
                if self.already_small_enough == 1 { " was" } else { "s were" }
            ));
        }
        if self.unsupported_format > 0 {
            lines.push(format!(
                "Skipped {} image{} that {} not PNG or JPEG.",
                self.unsupported_format,
                plural(self.unsupported_format),
                if self.unsupported_format == 1 { "is" } else { "are" }
            ));
        }
        if self.missing > 0 {
            lines.push(format!("{} image file{} could not be found.", self.missing, plural(self.missing)));
        }
        if self.failed > 0 {
            lines.push(format!(
                "{} image{} failed: {}",
                self.failed,
                plural(self.failed),
                self.first_error.as_deref().unwrap_or_default()
            ));
        }
        if let Some((done, total)) = stopped_early {
            lines.push(format!("Stopped after {done} of {total} images."));
        }
        lines.join("\n")
    }
}

enum Stage {
    Confirming { paths: Vec<String> },
    Running { job: ShrinkJob },
    Finished { message: String },
}

pub struct ShrinkCardArtFlow {
    stage: Stage,
}

impl ShrinkCardArtFlow {
    pub fn open(folder_path: &str, game: &GameDefinition) -> ShrinkCardArtFlow {
        ShrinkCardArtFlow { stage: Stage::Confirming { paths: card_art_paths(folder_path, game) } }
    }

    /// Draws the current dialog; false once the flow is over.
    pub fn show(&mut self, frame: &mut Frame) -> bool {
        match &mut self.stage {
            Stage::Confirming { paths } => {
                if paths.is_empty() {
                    return !frame.alert_dialog("shrinkart", "Shrink Card Art", "No cards have art to shrink.");
                }
                let message = format!(
                    "Scale down all {} card image{} so none is taller than {MAX_CARD_ART_HEIGHT}px, keeping \
                     each image's proportions. Images already short enough are left alone.\n\nThis \
                     overwrites the image files in the game folder and can't be undone.",
                    paths.len(),
                    if paths.len() == 1 { "" } else { "s" }
                );
                match frame.confirm_dialog("shrinkart", "Shrink Card Art", &message, "Shrink", true) {
                    ConfirmResponse::Open => true,
                    ConfirmResponse::Cancelled => false,
                    ConfirmResponse::Confirmed => {
                        let paths = std::mem::take(paths);
                        self.stage = Stage::Running { job: ShrinkJob::start(paths) };
                        true
                    }
                }
            }
            Stage::Running { job } => {
                let finished = job.poll();
                // The images on screen reload from the smaller files as each one is rewritten.
                for (path, result) in &job.results[job.results_reloaded..] {
                    if matches!(result, FileResult::Processed(ShrinkOutcome::Shrunk { .. })) {
                        frame.images.invalidate(path);
                    }
                }
                job.results_reloaded = job.results.len();
                let (done, total) = (job.results.len(), job.total);
                let cancelled = job.cancelled.load(Ordering::Relaxed);
                if finished {
                    let stopped_early = (done < total).then_some((done, total));
                    let message = ShrinkSummary::from_results(&job.results).message(stopped_early);
                    self.stage = Stage::Finished { message };
                    return true;
                }
                let status = if cancelled { "Stopping...".to_string() } else { format!("{done} of {total} images") };
                let actions = [DialogAction::text("Cancel").disabled(cancelled)];
                let spec = DialogSpec {
                    title: Some("Shrinking Card Art"),
                    width: 420.0,
                    actions: &actions,
                    dismissible: false,
                    ..DialogSpec::default()
                };
                let progress = if total == 0 { None } else { Some(done as f32 / total as f32) };
                let response = frame.dialog("shrinkartprogress", &spec, |frame, x, y, width| {
                    frame.text(&status, x, y, TextOptions::new().width(width));
                    frame.progress_bar(x, y + 28.0, width, 4.0, progress, None);
                    40.0
                });
                if response == DialogResponse::Action(0) {
                    job.cancelled.store(true, Ordering::Relaxed);
                }
                true
            }
            Stage::Finished { message } => !frame.alert_dialog("shrinkartdone", "Shrink Card Art", message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::CardDefinition;

    #[test]
    fn art_paths_cover_fronts_and_existing_unique_backs_once() {
        let folder = std::env::temp_dir().join(format!("gametable_shrink_paths_{}", std::process::id()));
        std::fs::create_dir_all(folder.join("set1")).unwrap();
        std::fs::write(folder.join("set1").join("dragon_BACK.png"), b"x").unwrap();
        let folder_path = folder.to_string_lossy().into_owned();
        let mut game = GameDefinition::default();
        let card = |title: &str, image: Option<&str>, set: Option<&str>, back: Option<&str>| CardDefinition {
            image_path: image.map(str::to_string),
            set_id: set.map(str::to_string),
            card_back_id: back.map(str::to_string),
            ..CardDefinition::new(format!("id-{title}"), title)
        };
        game.cards_mut().extend([
            card("Dragon", Some("dragon.png"), Some("set1"), Some(UNIQUE_CARD_BACK_ID)),
            card("Dragon again", Some("dragon.png"), Some("set1"), None),
            card("Goblin", Some("goblin.jpg"), None, Some(UNIQUE_CARD_BACK_ID)),
            card("Blank", None, None, None),
        ]);
        // Goblin's unique back doesn't exist, so only its front is listed.
        let expected = vec![
            resolve_bare_image_path(&folder_path, "dragon_BACK.png", Some("set1")),
            resolve_bare_image_path(&folder_path, "dragon.png", Some("set1")),
            resolve_bare_image_path(&folder_path, "goblin.jpg", None),
        ];
        assert_eq!(card_art_paths(&folder_path, &game), expected);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn summary_counts_each_kind_of_result() {
        let results = vec![
            (
                "a.png".to_string(),
                FileResult::Processed(ShrinkOutcome::Shrunk { original_size: (1, 2), new_size: (1, 1) }),
            ),
            ("b.png".to_string(), FileResult::Processed(ShrinkOutcome::AlreadySmallEnough)),
            ("c.webp".to_string(), FileResult::Processed(ShrinkOutcome::UnsupportedFormat)),
            ("d.png".to_string(), FileResult::Missing),
            ("e.png".to_string(), FileResult::Failed("bad data".to_string())),
        ];
        let summary = ShrinkSummary::from_results(&results);
        assert_eq!(
            summary,
            ShrinkSummary {
                shrunk: 1,
                already_small_enough: 1,
                unsupported_format: 1,
                missing: 1,
                failed: 1,
                first_error: Some("e.png: bad data".to_string()),
            }
        );
        let message = summary.message(Some((5, 9)));
        assert!(message.starts_with("Shrunk 1 image."));
        assert!(message.contains("Stopped after 5 of 9 images."));
    }
}
