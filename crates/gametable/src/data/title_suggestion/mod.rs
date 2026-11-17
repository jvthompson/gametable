//! "Suggest Title" in the game definition editor's Card View: guesses a card's title from its ID
//! (text rules) or from its picture (local text recognition, or Claude through the Anthropic API).
//! Suggestions run on background threads; the editor polls [`SuggestionJob`] each frame.

pub mod claude_vision;
pub mod from_id;
pub mod local_ocr;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use gametable_core::models::CardDefinition;

use claude_vision::{ClaudeCardRequest, ClaudeModel, suggest_title_with_claude};

/// Where a suggestion comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    FromId,
    FromImage(ImageEngine),
}

/// What reads a card's picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageEngine {
    /// Text recognition on this computer (free, offline).
    LocalOcr,
    /// Claude through the Anthropic API (the user's key).
    Claude,
}

/// Claude settings for a job that uses it.
#[derive(Debug, Clone)]
pub struct ClaudeAccess {
    pub api_key: String,
    pub model: ClaudeModel,
}

/// One card to suggest a title for; `key` is the editor's stable card key.
#[derive(Debug, Clone)]
pub struct CardToSuggest {
    pub key: u64,
    pub card: CardDefinition,
    /// The image's full path, when the card has one.
    pub image_path: Option<String>,
}

/// One card's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionOutcome {
    pub key: u64,
    /// `Ok(None)`: nothing found. `Err`: the engine failed for this card.
    pub suggestion: Result<Option<String>, String>,
}

/// Whether a card still has a placeholder title: empty, or the same as its ID or image file name
/// (what "cards from an image folder" sets).
pub fn is_untitled(card: &CardDefinition) -> bool {
    let title = card.card_title.trim();
    if title.is_empty() || title.eq_ignore_ascii_case(card.id.trim()) {
        return true;
    }
    card.image_path.as_deref().is_some_and(|path| {
        let image_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        title.eq_ignore_ascii_case(&from_id::file_stem(path)) || title.eq_ignore_ascii_case(image_name)
    })
}

/// Everything an engine needs besides the card.
#[derive(Debug, Clone)]
pub struct SuggestionContext {
    pub game_name: String,
    pub claude: Option<ClaudeAccess>,
    /// Where the local text recognition models are (or get downloaded to).
    pub ocr_models_folder: std::path::PathBuf,
}

/// One card's suggestion, run on the calling thread.
pub fn suggest_title(
    item: &CardToSuggest,
    source: TitleSource,
    context: &SuggestionContext,
) -> Result<Option<String>, String> {
    match source {
        TitleSource::FromId => Ok(from_id::suggest_title_from_id(&item.card)),
        TitleSource::FromImage(engine) => {
            let Some(image_path) = &item.image_path else { return Ok(None) };
            match engine {
                ImageEngine::Claude => {
                    let Some(access) = &context.claude else {
                        return Err("Add a Claude API key in Settings first.".to_string());
                    };
                    suggest_title_with_claude(&ClaudeCardRequest {
                        api_key: access.api_key.clone(),
                        model: access.model,
                        game_name: context.game_name.clone(),
                        card_id: item.card.id.clone(),
                        image_path: image_path.clone(),
                    })
                }
                ImageEngine::LocalOcr => {
                    local_ocr::suggest_title_with_local_ocr(image_path, &context.ocr_models_folder)
                }
            }
        }
    }
}

/// Suggestions for many cards on background threads. Poll [`SuggestionJob::poll`] each frame.
pub struct SuggestionJob {
    receiver: Receiver<SuggestionOutcome>,
    cancelled: Arc<AtomicBool>,
    total: usize,
    pub outcomes: Vec<SuggestionOutcome>,
}

impl SuggestionJob {
    /// Starts the work: Claude runs a few requests at once; the local engines one card at a time.
    pub fn start(cards: Vec<CardToSuggest>, source: TitleSource, context: SuggestionContext) -> SuggestionJob {
        let total = cards.len();
        let worker_count = match source {
            TitleSource::FromImage(ImageEngine::Claude) => 4.min(total.max(1)),
            _ => 1,
        };
        let queue = Arc::new(Mutex::new(VecDeque::from(cards)));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel();
        for _ in 0..worker_count {
            let (queue, cancelled, sender, context) =
                (queue.clone(), cancelled.clone(), sender.clone(), context.clone());
            std::thread::spawn(move || run_worker(&queue, &cancelled, &sender, source, &context));
        }
        SuggestionJob { receiver, cancelled, total, outcomes: Vec::new() }
    }

    /// Collects finished cards; true once every card is done (or the job was cancelled and the
    /// workers have stopped).
    pub fn poll(&mut self) -> bool {
        loop {
            match self.receiver.try_recv() {
                Ok(outcome) => self.outcomes.push(outcome),
                Err(std::sync::mpsc::TryRecvError::Empty) => return self.outcomes.len() >= self.total,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    /// Stops after the cards already in progress.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

impl Drop for SuggestionJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_worker(
    queue: &Mutex<VecDeque<CardToSuggest>>,
    cancelled: &AtomicBool,
    sender: &Sender<SuggestionOutcome>,
    source: TitleSource,
    context: &SuggestionContext,
) {
    while !cancelled.load(Ordering::Relaxed) {
        let Some(item) = queue.lock().ok().and_then(|mut queue| queue.pop_front()) else { return };
        let suggestion = suggest_title(&item, source, context);
        if sender.send(SuggestionOutcome { key: item.key, suggestion }).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_titles_count_as_untitled() {
        let mut card = CardDefinition::new("hack1033", "hack1033");
        assert!(is_untitled(&card));
        card.card_title = "  ".into();
        assert!(is_untitled(&card));
        card = CardDefinition::new("TW-59", "LureofPower");
        card.image_path = Some("LureofPower.jpg".into());
        assert!(is_untitled(&card));
        card.card_title = "Lure of Power".into();
        assert!(!is_untitled(&card));
    }

    #[test]
    fn a_job_suggests_every_card_from_ids() {
        let cards = ["059-ursula-power-hungry", "hack1033", "080-magical-maneuvers"]
            .iter()
            .enumerate()
            .map(|(index, id)| CardToSuggest {
                key: index as u64,
                card: CardDefinition::new(*id, *id),
                image_path: None,
            })
            .collect();
        let context =
            SuggestionContext { game_name: "Test".into(), claude: None, ocr_models_folder: std::env::temp_dir() };
        let mut job = SuggestionJob::start(cards, TitleSource::FromId, context);
        while !job.poll() {
            std::thread::yield_now();
        }
        let mut outcomes = job.outcomes.clone();
        outcomes.sort_by_key(|outcome| outcome.key);
        let suggestions: Vec<_> = outcomes.into_iter().map(|outcome| outcome.suggestion).collect();
        assert_eq!(
            suggestions,
            vec![Ok(Some("Ursula Power Hungry".to_string())), Ok(None), Ok(Some("Magical Maneuvers".to_string()))]
        );
    }

    #[test]
    fn claude_without_a_key_or_image_reports_why() {
        let item = CardToSuggest { key: 1, card: CardDefinition::new("a", "a"), image_path: Some("a.png".into()) };
        let context =
            SuggestionContext { game_name: "Test".into(), claude: None, ocr_models_folder: std::env::temp_dir() };
        assert!(suggest_title(&item, TitleSource::FromImage(ImageEngine::Claude), &context).is_err());
        let no_image = CardToSuggest { image_path: None, ..item };
        assert_eq!(suggest_title(&no_image, TitleSource::FromImage(ImageEngine::Claude), &context), Ok(None));
    }
}
