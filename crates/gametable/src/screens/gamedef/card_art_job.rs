//! Background work over a game's card art files, shared by the Cards tab's "Shrink Card Art"
//! and "Convert to WebP": a few worker threads take items from a queue until it is empty or the
//! user cancels, and a progress dialog shows how far they have got.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use crate::app::Frame;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};

/// The most files worked on at once.
const MAX_WORKER_THREADS: usize = 4;

pub struct CardArtJob<Item, Outcome> {
    total: usize,
    /// Items finished so far (kept when the results are taken).
    completed: usize,
    /// Each finished item with its outcome, in the order they finished.
    results: Vec<(Item, Outcome)>,
    receiver: Receiver<(Item, Outcome)>,
    cancelled: Arc<AtomicBool>,
    /// Threads still running; each sends `()` through `finished_receiver` as it ends.
    running_threads: usize,
    finished_receiver: Receiver<()>,
    /// How many of `results` [`CardArtJob::take_new_results_count`] has already reported.
    results_reported: usize,
}

impl<Item: Send + 'static, Outcome: Send + 'static> CardArtJob<Item, Outcome> {
    /// Starts working through `items` with `work` on background threads.
    pub fn start(items: Vec<Item>, thread_name: &str, work: fn(&Item) -> Outcome) -> CardArtJob<Item, Outcome> {
        let total = items.len();
        let queue = Arc::new(Mutex::new(VecDeque::from(items)));
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
                .name(format!("{thread_name}-{index}"))
                .spawn(move || {
                    while !cancelled.load(Ordering::Relaxed) {
                        let next_item = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                        let Some(item) = next_item else { break };
                        let outcome = work(&item);
                        if sender.send((item, outcome)).is_err() {
                            break;
                        }
                    }
                    let _ = finished_sender.send(());
                })
                .expect("spawn card art worker");
        }
        CardArtJob {
            total,
            completed: 0,
            results: Vec::new(),
            receiver,
            cancelled,
            running_threads: thread_count,
            finished_receiver,
            results_reported: 0,
        }
    }

    /// Collects finished items; true once every thread has stopped.
    pub fn poll(&mut self) -> bool {
        self.collect_results();
        while self.finished_receiver.try_recv().is_ok() {
            self.running_threads -= 1;
        }
        if self.running_threads == 0 {
            // Anything sent between the two drains.
            self.collect_results();
            return true;
        }
        false
    }

    fn collect_results(&mut self) {
        while let Ok(result) = self.receiver.try_recv() {
            self.results.push(result);
            self.completed += 1;
        }
    }

    pub fn results(&self) -> &[(Item, Outcome)] {
        &self.results
    }

    pub fn take_results(&mut self) -> Vec<(Item, Outcome)> {
        std::mem::take(&mut self.results)
    }

    /// The results finished since the last call.
    pub fn new_results(&mut self) -> &[(Item, Outcome)] {
        let start = self.results_reported;
        self.results_reported = self.results.len();
        &self.results[start..]
    }

    /// `Some((done, total))` when the job was cancelled before every item was worked on.
    pub fn stopped_early(&self) -> Option<(usize, usize)> {
        (self.completed < self.total).then_some((self.completed, self.total))
    }

    /// The progress dialog: "<done> of <total> images", a bar and Cancel.
    pub fn show_progress(&mut self, frame: &mut Frame, dialog_id: &str, title: &str) {
        let (done, total) = (self.completed, self.total);
        let cancelled = self.cancelled.load(Ordering::Relaxed);
        let status = if cancelled { "Stopping...".to_string() } else { format!("{done} of {total} images") };
        let actions = [DialogAction::text("Cancel").disabled(cancelled)];
        let spec = DialogSpec {
            title: Some(title),
            width: 420.0,
            actions: &actions,
            dismissible: false,
            ..DialogSpec::default()
        };
        let progress = if total == 0 { None } else { Some(done as f32 / total as f32) };
        let response = frame.dialog(dialog_id, &spec, |frame, x, y, width| {
            frame.text(&status, x, y, TextOptions::new().width(width));
            frame.progress_bar(x, y + 28.0, width, 4.0, progress, None);
            40.0
        });
        if response == DialogResponse::Action(0) {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn double(value: &u32) -> u32 {
        value * 2
    }

    #[test]
    fn every_item_is_worked_on_once() {
        let mut job = CardArtJob::start((1..=20).collect(), "test-job", double);
        while !job.poll() {
            std::thread::yield_now();
        }
        let mut results = job.take_results();
        results.sort();
        assert_eq!(results, (1..=20).map(|value| (value, value * 2)).collect::<Vec<_>>());
        assert_eq!(job.stopped_early(), None);
    }
}
