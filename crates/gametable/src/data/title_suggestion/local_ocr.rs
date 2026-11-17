//! "Suggest Title → Image (local)": text recognition on this computer with the `ocrs` engine.
//! Its two models (~10 MB each) are downloaded once into `<app folder>/ocr_models/`. The title is
//! taken to be the card's largest line of text.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

use crate::graphics::image_data::RgbaImage;

/// The models' published copies (the ocrs project's own hosting).
const DETECTION_MODEL_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx";
const RECOGNITION_MODEL_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx";
const DETECTION_MODEL_FILE: &str = "text-detection.onnx";
const RECOGNITION_MODEL_FILE: &str = "text-recognition.onnx";
/// Card art is shrunk to this long side first: enough for titles, much faster.
const MAX_IMAGE_SIDE: u32 = 1200;
/// A title has at least this many letters.
const MINIMUM_TITLE_LETTERS: usize = 3;

/// The loaded engine, shared by every card and job (loading the models takes a moment).
static ENGINE: Mutex<Option<Arc<OcrEngine>>> = Mutex::new(None);

/// The folder the models live in.
pub fn models_folder(app_folder: &Path) -> PathBuf {
    app_folder.join("ocr_models")
}

/// Whether the models still need downloading (the dialog warns about it).
pub fn models_missing(models_folder: &Path) -> bool {
    !models_folder.join(DETECTION_MODEL_FILE).is_file() || !models_folder.join(RECOGNITION_MODEL_FILE).is_file()
}

/// Reads one card's title from its picture. `Ok(None)` when no text that looks like a title is found.
pub fn suggest_title_with_local_ocr(image_path: &str, models_folder: &Path) -> Result<Option<String>, String> {
    Ok(pick_title(recognized_lines(image_path, models_folder)?))
}

/// Every line of text recognised on the card, with where it is.
pub fn recognized_lines(image_path: &str, models_folder: &Path) -> Result<Vec<TextLineCandidate>, String> {
    let engine = shared_engine(models_folder)?;
    let image = RgbaImage::load(Path::new(image_path))?.downscaled_to_fit(MAX_IMAGE_SIDE);
    let source = ImageSource::from_bytes(&image.pixels, (image.width, image.height))
        .map_err(|error| format!("Could not read the image: {error}"))?;
    let input = engine.prepare_input(source).map_err(|error| error.to_string())?;
    let words = engine.detect_words(&input).map_err(|error| error.to_string())?;
    let line_rects = engine.find_text_lines(&input, &words);
    let lines = engine.recognize_text(&input, &line_rects).map_err(|error| error.to_string())?;
    Ok(lines
        .iter()
        .flatten()
        .map(|line| {
            let rect = line.bounding_rect();
            TextLineCandidate { text: line.to_string(), top: rect.top() as f32, height: rect.height() as f32 }
        })
        .collect())
}

/// One recognised line of text and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLineCandidate {
    pub text: String,
    pub top: f32,
    pub height: f32,
}

/// Titles are short; longer lines are rules text (used only when nothing shorter is found).
const MAXIMUM_TITLE_WORDS: usize = 5;

/// The tallest short line that looks like words (ties go to the higher line), tidied up.
pub fn pick_title(candidates: impl IntoIterator<Item = TextLineCandidate>) -> Option<String> {
    let candidates: Vec<TextLineCandidate> = candidates
        .into_iter()
        .map(|candidate| TextLineCandidate { text: tidy_line(&candidate.text), ..candidate })
        .filter(|candidate| looks_like_a_title(&candidate.text))
        .collect();
    let is_short = |candidate: &&TextLineCandidate| candidate.text.split_whitespace().count() <= MAXIMUM_TITLE_WORDS;
    let tallest = |pool: Vec<&TextLineCandidate>| {
        pool.into_iter()
            .max_by(|first, second| {
                first.height.total_cmp(&second.height).then_with(|| second.top.total_cmp(&first.top))
            })
            .map(|candidate| title_case_if_shouting(&candidate.text))
    };
    tallest(candidates.iter().filter(is_short).collect()).or_else(|| tallest(candidates.iter().collect()))
}

/// `ALPHA ICHIGORO` → `Alpha Ichigoro` (titles are often printed in capitals); mixed case stays.
fn title_case_if_shouting(text: &str) -> String {
    let has_lower = text.chars().any(char::is_lowercase);
    if has_lower {
        return text.to_string();
    }
    text.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapses spaces and trims stray punctuation from the ends.
fn tidy_line(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches(|character: char| !character.is_alphanumeric() && !"!?)'\"".contains(character)).to_string()
}

/// Mostly letters, enough of them, and not just a number or a code.
fn looks_like_a_title(text: &str) -> bool {
    let letters = text.chars().filter(|character| character.is_alphabetic()).count();
    let counted = text.chars().filter(|character| !character.is_whitespace()).count();
    letters >= MINIMUM_TITLE_LETTERS && letters * 4 >= counted * 3
}

fn shared_engine(models_folder: &Path) -> Result<Arc<OcrEngine>, String> {
    let mut slot = ENGINE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(engine) = slot.as_ref() {
        return Ok(engine.clone());
    }
    let detection_path = ensure_model(models_folder, DETECTION_MODEL_FILE, DETECTION_MODEL_URL)?;
    let recognition_path = ensure_model(models_folder, RECOGNITION_MODEL_FILE, RECOGNITION_MODEL_URL)?;
    let detection_model = Model::load_file(&detection_path)
        .map_err(|error| format!("Could not load the text detection model: {error}"))?;
    let recognition_model = Model::load_file(&recognition_path)
        .map_err(|error| format!("Could not load the text recognition model: {error}"))?;
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })
    .map_err(|error| error.to_string())?;
    let engine = Arc::new(engine);
    *slot = Some(engine.clone());
    Ok(engine)
}

/// The model's path, downloading it first if needed (to a temporary name, then renamed, so a
/// broken download never looks complete).
fn ensure_model(models_folder: &Path, file_name: &str, url: &str) -> Result<PathBuf, String> {
    let path = models_folder.join(file_name);
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_folder).map_err(|error| error.to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut response =
        agent.get(url).call().map_err(|error| format!("Could not download the text recognition models: {error}"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_to_vec()
        .map_err(|error| format!("Could not download the text recognition models: {error}"))?;
    let partial = models_folder.join(format!("{file_name}.part"));
    std::fs::write(&partial, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, &path).map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, top: f32, height: f32) -> TextLineCandidate {
        TextLineCandidate { text: text.to_string(), top, height }
    }

    #[test]
    fn the_tallest_wordy_line_is_the_title() {
        let lines = vec![
            line("PC", 10.0, 40.0),
            line("3", 10.0, 60.0),
            line(" ALPHA  ICHIGORO ", 400.0, 34.0),
            line("When Alpha Ichigoro wins a fight,", 600.0, 18.0),
            // A badly merged rules line can measure taller than the title.
            line("you may place a card from your", 650.0, 70.0),
            line("1C1", 900.0, 12.0),
        ];
        assert_eq!(pick_title(lines).as_deref(), Some("Alpha Ichigoro"));
        assert_eq!(
            pick_title(vec![line("only a long line of rules text here", 0.0, 10.0)]).as_deref(),
            Some("only a long line of rules text here")
        );
        assert_eq!(pick_title(vec![line("12", 0.0, 50.0), line("--", 0.0, 50.0)]), None);
        // Equal heights: the higher line wins.
        assert_eq!(
            pick_title(vec![line("Lower Line", 50.0, 20.0), line("Upper Line", 10.0, 20.0)]).as_deref(),
            Some("Upper Line")
        );
        assert_eq!(tidy_line("• Sithicus ."), "Sithicus");
    }

    /// Reads a real card: `GAMETABLE_OCR_IMAGE=<card image> cargo test -p gametable --release -- --ignored real_card`.
    #[test]
    #[ignore]
    fn real_card_title() {
        let Ok(image) = std::env::var("GAMETABLE_OCR_IMAGE") else { return };
        let folder = models_folder(&std::env::temp_dir().join("gametable-ocr-test"));
        for line in recognized_lines(&image, &folder).unwrap() {
            println!("line {:>6.0} {:>4.0} {}", line.top, line.height, line.text);
        }
        println!("{:?}", suggest_title_with_local_ocr(&image, &folder));
    }
}
