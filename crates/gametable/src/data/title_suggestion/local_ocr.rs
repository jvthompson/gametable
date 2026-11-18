//! "Suggest Title → Image (local)": text recognition on this computer with the `ocrs` engine.
//! Its two models (~10 MB each) are downloaded once into `<app folder>/ocr_models/`. The title is
//! taken to be the card's largest line of text, or, when the user drew a title area, every line of
//! words inside it, top to bottom.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

use super::title_style::{LineSeparator, title_case_if_all_caps};
use super::{TitleArea, load_card_image};

/// The models' published copies (the ocrs project's own hosting).
const DETECTION_MODEL_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx";
const RECOGNITION_MODEL_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx";
const DETECTION_MODEL_FILE: &str = "text-detection.onnx";
const RECOGNITION_MODEL_FILE: &str = "text-recognition.onnx";
/// Card art is shrunk to this long side first: enough for titles, much faster.
const MAX_IMAGE_SIDE: u32 = 1200;
/// A title area whose longer side is under this is enlarged before reading.
const MIN_TITLE_AREA_SIDE: u32 = 1000;
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

/// Reads one card's title from its picture (only inside `title_area` when there is one).
/// `Ok(None)` when no text that looks like a title is found.
/// Lines inside a title area are joined with `line_separator` (", " for "Ariel, On Human Legs").
pub fn suggest_title_with_local_ocr(
    image_path: &str,
    title_area: Option<TitleArea>,
    line_separator: LineSeparator,
    models_folder: &Path,
) -> Result<Option<String>, String> {
    let lines = recognized_lines(image_path, title_area, models_folder)?;
    Ok(if title_area.is_some() { join_title_lines(lines, line_separator) } else { pick_title(lines) })
}

/// Every line of text recognised on the card (or in its title area), with where it is.
pub fn recognized_lines(
    image_path: &str,
    title_area: Option<TitleArea>,
    models_folder: &Path,
) -> Result<Vec<TextLineCandidate>, String> {
    let engine = shared_engine(models_folder)?;
    let image = load_card_image(image_path, title_area)?;
    // A small title area is enlarged so its smaller lines (subtitles) are big enough to read.
    let enlargement = (MIN_TITLE_AREA_SIDE / image.width.max(image.height).max(1)).clamp(1, 4);
    let image = image.upscaled(enlargement).downscaled_to_fit(MAX_IMAGE_SIDE);
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
            .map(|candidate| title_case_if_all_caps(&candidate.text))
    };
    tallest(candidates.iter().filter(is_short).collect()).or_else(|| tallest(candidates.iter().collect()))
}

/// Inside a title area the user drew, the title is all of the wordy lines, top to bottom, joined
/// with the game's separator (a name and its subtitle on two lines become one title).
pub fn join_title_lines(
    candidates: impl IntoIterator<Item = TextLineCandidate>,
    line_separator: LineSeparator,
) -> Option<String> {
    let mut lines: Vec<TextLineCandidate> = candidates
        .into_iter()
        .map(|candidate| TextLineCandidate { text: tidy_line(&candidate.text), ..candidate })
        .filter(|candidate| looks_like_a_title(&candidate.text))
        .collect();
    lines.sort_by(|first, second| first.top.total_cmp(&second.top));
    // Each line is fixed on its own: a name in capitals over a subtitle in mixed case.
    let joined =
        lines.iter().map(|line| title_case_if_all_caps(&line.text)).collect::<Vec<_>>().join(line_separator.joiner());
    (!joined.is_empty()).then_some(joined)
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

    #[test]
    fn inside_a_title_area_every_wordy_line_joins_top_to_bottom() {
        let lines = || vec![line("ON HUMAN LEGS", 60.0, 14.0), line("ARIEL", 20.0, 30.0), line("4", 0.0, 40.0)];
        assert_eq!(join_title_lines(lines(), LineSeparator::Space).as_deref(), Some("Ariel On Human Legs"));
        assert_eq!(join_title_lines(lines(), LineSeparator::Comma).as_deref(), Some("Ariel, On Human Legs"));
        let one_line = vec![line("Lure of Power", 5.0, 20.0)];
        assert_eq!(join_title_lines(one_line, LineSeparator::Comma).as_deref(), Some("Lure of Power"));
        assert_eq!(join_title_lines(vec![line("12", 0.0, 50.0)], LineSeparator::Comma), None);
    }

    /// Reads a real card: `GAMETABLE_OCR_IMAGE=<card image> cargo test -p gametable --release -- --ignored real_card`
    /// (add `GAMETABLE_OCR_AREA=x,y,width,height` to read only a title area).
    #[test]
    #[ignore]
    fn real_card_title() {
        let Ok(image) = std::env::var("GAMETABLE_OCR_IMAGE") else { return };
        // Optional `GAMETABLE_OCR_AREA=x,y,width,height` (fractions of the image) reads only there.
        let area = std::env::var("GAMETABLE_OCR_AREA").ok().map(|text| {
            let parts: Vec<f32> = text.split(',').map(|part| part.trim().parse().unwrap()).collect();
            TitleArea { x: parts[0], y: parts[1], width: parts[2], height: parts[3] }
        });
        // Optional `GAMETABLE_OCR_JOIN=comma|dash|colon` joins a title area's lines with it.
        let separator = match std::env::var("GAMETABLE_OCR_JOIN").as_deref() {
            Ok("comma") => LineSeparator::Comma,
            Ok("dash") => LineSeparator::Dash,
            Ok("colon") => LineSeparator::Colon,
            _ => LineSeparator::Space,
        };
        let folder = models_folder(&std::env::temp_dir().join("gametable-ocr-test"));
        for line in recognized_lines(&image, area, &folder).unwrap() {
            println!("line {:>6.0} {:>4.0} {}", line.top, line.height, line.text);
        }
        println!("{:?}", suggest_title_with_local_ocr(&image, area, separator, &folder));
    }
}
