//! The Markdown renderer behind the rulebook viewer and the Controls and Support dialogs
//! (a port of the small renderer in `src/ui/screens/controls_dialog.lua`, much extended).
//!
//! It reads the Markdown that game documents use — `#`–`######` headings, paragraphs with
//! `**bold**`, `*italic*`, `` `code` `` and `~~struck~~` text, nested bullet and numbered lists,
//! quotes, rules, fenced code, pipe tables and images — plus the HTML commonly mixed in
//! (`<table>`, `<strong>`, `<em>`, `<br>`, `<a href>`, `<a name>`, `<img>`, `<p align>`, …),
//! dropping `<!-- comments -->`. Links work: `#anchor` links jump to a heading (by its id, its
//! GitHub-style slug or a compact slug) or an `<a name>`; web links ask before opening the
//! default browser; the host decides about links to its other documents.

mod html;
mod inline;
mod layout;
mod parse;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub use inline::{Inline, InlineItem, SpanStyle, plain_text};
pub use layout::{HeadingEntry, LayoutKey, MarkdownLayout};
pub use parse::{ListMarker, MarkdownBlock, parse_markdown};

use crate::app::Frame;
use crate::ui::theme::{self, Weight};

/// A parsed document and its layout at the width it was last drawn.
#[derive(Debug, Clone, Default)]
pub struct MarkdownDocument {
    blocks: Vec<MarkdownBlock>,
    layout: Option<(LayoutKey, MarkdownLayout)>,
}

impl MarkdownDocument {
    pub fn new(blocks: Vec<MarkdownBlock>) -> MarkdownDocument {
        MarkdownDocument { blocks, layout: None }
    }

    pub fn parse(text: &str) -> MarkdownDocument {
        MarkdownDocument::new(parse_markdown(text))
    }

    pub fn blocks(&self) -> &[MarkdownBlock] {
        &self.blocks
    }

    /// For resolving image paths; the layout is redone afterwards.
    pub fn blocks_mut(&mut self) -> &mut Vec<MarkdownBlock> {
        self.layout = None;
        &mut self.blocks
    }

    /// The headings of the last layout (level, text, offset from the top).
    pub fn headings(&self) -> &[HeadingEntry] {
        self.layout.as_ref().map_or(&[], |(_, layout)| layout.headings.as_slice())
    }

    /// Whether the document has been laid out (so anchors can be found).
    pub fn is_laid_out(&self) -> bool {
        self.layout.is_some()
    }

    /// Where `#target` (or `target`) is, from the top of the document: an explicit id or
    /// `<a name>`, a heading's GitHub-style slug, or a heading whose compact slug matches.
    pub fn anchor_offset(&self, target: &str) -> Option<f32> {
        let (_, layout) = self.layout.as_ref()?;
        find_anchor(&layout.anchors, target)
    }
}

/// Looks a link target up among a layout's anchors.
pub fn find_anchor(anchors: &[(String, f32)], target: &str) -> Option<f32> {
    let wanted = percent_decode(target.trim().trim_start_matches('#'));
    if wanted.is_empty() {
        return None;
    }
    let exact = anchors.iter().find(|(name, _)| *name == wanted);
    let lower = wanted.to_lowercase();
    let ignoring_case = || anchors.iter().find(|(name, _)| name.to_lowercase() == lower);
    let compact = compact_slug(&wanted);
    let by_compact = || anchors.iter().find(|(name, _)| !compact.is_empty() && compact_slug(name) == compact);
    exact.or_else(ignoring_case).or_else(by_compact).map(|(_, offset)| *offset)
}

/// `%20`-style escapes decoded (invalid ones kept).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
            && let Ok(value) = u8::from_str_radix(&text[index + 1..index + 3], 16)
        {
            decoded.push(value);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// GitHub's heading anchor: lower case, punctuation dropped, spaces to hyphens
/// ("1. Preparation for Combat" -> "1-preparation-for-combat").
pub fn github_slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric() || matches!(character, ' ' | '-' | '_'))
        .map(|character| if character == ' ' { '-' } else { character })
        .collect()
}

/// Letters and digits only, lower case ("1. Preparation for Combat" -> "1preparationforcombat"),
/// the form many converted documents link with.
pub fn compact_slug(text: &str) -> String {
    text.to_lowercase().chars().filter(|character| character.is_alphanumeric()).collect()
}

/// What a link in a document points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// A place in this document (`#faq`).
    Anchor(String),
    /// A web or mail address, to open in the default browser after asking.
    External(String),
    /// Another file next to the document (`other.md`, `rules.pdf#page`), with an optional anchor.
    Document { file: String, anchor: Option<String> },
    /// Anything that can't be opened from a document (`file:`, `javascript:`, …).
    Unsupported,
}

pub fn classify_link(target: &str) -> LinkKind {
    let target = target.trim();
    let lower = target.to_lowercase();
    if let Some(anchor) = target.strip_prefix('#') {
        return LinkKind::Anchor(anchor.to_string());
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return LinkKind::External(target.to_string());
    }
    if lower.starts_with("www.") {
        return LinkKind::External(format!("https://{target}"));
    }
    if lower.contains(':') || target.is_empty() {
        return LinkKind::Unsupported;
    }
    let (file, anchor) = match target.split_once('#') {
        Some((file, anchor)) => (file, Some(anchor.to_string())),
        None => (target, None),
    };
    let file = percent_decode(file.trim_start_matches("./"));
    let file = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
    LinkKind::Document { file, anchor }
}

/// Asks before opening a web address in the default browser.
pub fn confirm_external_link(frame: &mut Frame, url: &str) {
    let url = url.to_string();
    let message = format!("This link goes to {url}\n\nOpen it in your web browser?");
    frame.ui.confirm(
        "Open Link?",
        &message,
        "Open in Browser",
        "Cancel",
        Box::new(move |_| {
            let _ = sdl3::url::open_url(&url);
        }),
    );
}

/// Follows a clicked link within one document: jumps to an anchor (scrolling `scroll_id`) or
/// asks about a web address. A link to another document is handed back for the caller to open;
/// anything else says it can't be opened.
pub fn follow_link(
    frame: &mut Frame,
    document: &MarkdownDocument,
    scroll_id: Option<&str>,
    target: &str,
) -> Option<(String, Option<String>)> {
    match classify_link(target) {
        LinkKind::Anchor(anchor) => match (document.anchor_offset(&anchor), scroll_id) {
            (Some(offset), Some(scroll_id)) => frame.ui.scroll_to(scroll_id, (offset - 8.0).max(0.0)),
            _ => frame.ui.toast("That section isn't in this document.", None),
        },
        LinkKind::External(url) => confirm_external_link(frame, &url),
        LinkKind::Document { file, anchor } => return Some((file, anchor)),
        LinkKind::Unsupported => frame.ui.toast("Can't open this link.", None),
    }
    None
}

/// Options for [`Frame::markdown`].
#[derive(Debug, Default)]
pub struct MarkdownOptions<'a> {
    /// Also draw level-1 headings (normally the dialog title names the page).
    pub show_title_heading: bool,
    /// Keeps link regions apart between documents ("md" when empty).
    pub id: &'a str,
    /// Only pieces between these screen y values are drawn (everything when `None`).
    pub visible: Option<(f32, f32)>,
    /// Set to a link's target when it was clicked this frame.
    pub clicked_link: Option<String>,
}

static LOADED_DOCUMENTS: LazyLock<Mutex<HashMap<String, MarkdownDocument>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Draws a bundled Markdown file from the assets folder (parsed once, laid out once per width)
/// and returns the height used and the target of a link clicked this frame.
pub fn draw_bundled_document(
    frame: &mut Frame,
    file_name: &str,
    x: f32,
    y: f32,
    width: f32,
    options: &mut MarkdownOptions,
) -> f32 {
    let mut documents = LOADED_DOCUMENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let assets_folder = frame.assets_folder.to_path_buf();
    let document = documents.entry(file_name.to_string()).or_insert_with(|| load_bundled(&assets_folder, file_name));
    let height = frame.markdown(document, x, y, width, options);
    if let Some(target) = options.clicked_link.clone() {
        follow_link(frame, document, None, &target);
    }
    height
}

fn load_bundled(assets_folder: &Path, file_name: &str) -> MarkdownDocument {
    let text = std::fs::read_to_string(assets_folder.join(file_name))
        .unwrap_or_else(|_| format!("{file_name} is missing from this build."));
    MarkdownDocument::parse(&text)
}

impl Frame<'_> {
    /// `M.drawBlocks(x, y, w, list, opts)`: draws the document at `(x, y)` (laid out again only
    /// when the width, text size or loaded images change); returns the height it takes.
    pub fn markdown(
        &mut self,
        document: &mut MarkdownDocument,
        x: f32,
        y: f32,
        width: f32,
        options: &mut MarkdownOptions,
    ) -> f32 {
        let loaded_images = document
            .blocks
            .iter()
            .filter(|block| {
                matches!(block, MarkdownBlock::Image { resolved_path: Some(path), .. } if self.images.get(path).is_some())
            })
            .count();
        let key = LayoutKey {
            width: width.round(),
            font_family: self.fonts.family(),
            line_height: self.fonts.height(theme::font(13.0, Weight::Regular)),
            show_title_heading: options.show_title_heading,
            loaded_images,
        };
        if document.layout.as_ref().is_none_or(|(laid_out_for, _)| *laid_out_for != key) {
            let layout = self.lay_out_markdown(&document.blocks, width, options.show_title_heading);
            document.layout = Some((key, layout));
        }
        let Some((_, layout)) = &document.layout else { return 0.0 };
        let visible = options.visible.unwrap_or((f32::MIN, f32::MAX));
        let id = if options.id.is_empty() { "md" } else { options.id };
        if let Some(target) = self.draw_markdown_layout(layout, x, y, visible, id) {
            options.clicked_link = Some(target);
        }
        layout.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_github_and_compact_forms() {
        assert_eq!(github_slug("1. Preparation for Combat"), "1-preparation-for-combat");
        assert_eq!(github_slug("Card Conflict Examples (CCE)"), "card-conflict-examples-cce");
        assert_eq!(compact_slug("1. Preparation for Combat"), "1preparationforcombat");
    }

    #[test]
    fn anchors_resolve_by_id_slug_case_and_compact_form() {
        let anchors = vec![
            ("faq".to_string(), 100.0),
            ("1-preparation-for-combat".to_string(), 200.0),
            ("1preparationforcombat".to_string(), 200.0),
            ("Q33".to_string(), 300.0),
        ];
        assert_eq!(find_anchor(&anchors, "#faq"), Some(100.0));
        assert_eq!(find_anchor(&anchors, "#1preparationforcombat"), Some(200.0));
        assert_eq!(find_anchor(&anchors, "#1-preparation-for-combat"), Some(200.0));
        assert_eq!(find_anchor(&anchors, "#1.%20Preparation%20for%20Combat"), Some(200.0));
        assert_eq!(find_anchor(&anchors, "q33"), Some(300.0));
        assert_eq!(find_anchor(&anchors, "#cce"), None);
        assert_eq!(find_anchor(&anchors, "#"), None);
    }

    #[test]
    fn links_are_sorted_into_anchors_web_documents_and_unsupported() {
        assert_eq!(classify_link("#faq"), LinkKind::Anchor("faq".to_string()));
        assert_eq!(classify_link("https://x.org"), LinkKind::External("https://x.org".to_string()));
        assert_eq!(classify_link("mailto:a@b.c"), LinkKind::External("mailto:a@b.c".to_string()));
        assert_eq!(classify_link("www.x.org"), LinkKind::External("https://www.x.org".to_string()));
        assert_eq!(
            classify_link("./Scenario%20Rules.md#setup"),
            LinkKind::Document { file: "Scenario Rules.md".to_string(), anchor: Some("setup".to_string()) }
        );
        assert_eq!(
            classify_link("_documents/rules.pdf"),
            LinkKind::Document { file: "rules.pdf".to_string(), anchor: None }
        );
        assert_eq!(classify_link("javascript:alert(1)"), LinkKind::Unsupported);
        assert_eq!(classify_link("file:///c:/x"), LinkKind::Unsupported);
    }

    #[test]
    fn the_old_supported_subset_still_parses() {
        let text = "# Title\r\n\nSome **bold** text\nwrapped `code`.\n\n## Section\n| Key | Action |\n|---|:--|\n| **A** | Do it |\n- bullet **one**\n**Whole bold**\n![Alt](img/a.png)\n#### Deep\n#nospace";
        let blocks = parse_markdown(text);
        assert_eq!(blocks.len(), 8, "{blocks:#?}");
        assert!(matches!(&blocks[0], MarkdownBlock::Heading { level: 1, .. }));
        assert!(matches!(&blocks[3], MarkdownBlock::Table { rows, .. } if rows.len() == 1));
        // A line right after a bullet continues it, as in standard Markdown.
        assert!(matches!(
            &blocks[4],
            MarkdownBlock::ListItem { marker: ListMarker::Bullet, content, .. } if plain_text(content) == "bullet one Whole bold"
        ));
        assert!(matches!(&blocks[5], MarkdownBlock::Image { path, .. } if path == "img/a.png"));
        assert!(matches!(&blocks[6], MarkdownBlock::Heading { level: 4, .. }));
    }

    #[test]
    fn documents_wrap_record_links_and_anchors_and_report_clicked_links() {
        use crate::app::harness::UiHarness;
        use layout::LaidOut;
        let mut harness = UiHarness::new();
        let mut document = MarkdownDocument::parse(
            "# Rules

Some text with a [link to the FAQ](#faq) that wraps across several lines at this width.

## FAQ {#faq}
Answer",
        );
        let mut clicked = None;
        let mut draw = |frame: &mut Frame| {
            let mut options = MarkdownOptions { show_title_heading: true, ..MarkdownOptions::default() };
            frame.markdown(&mut document, 10.0, 10.0, 160.0, &mut options);
            if let Some(target) = options.clicked_link {
                clicked = Some(target);
            }
        };
        harness.frame(&mut draw);
        let (_, layout) = document.layout.as_ref().expect("laid out");
        let line_tops: std::collections::BTreeSet<i64> = layout
            .items
            .iter()
            .filter_map(|item| match item {
                LaidOut::Text { y, role: layout::TextRole::Body, .. } => Some(*y as i64),
                _ => None,
            })
            .collect();
        assert!(line_tops.len() >= 3, "the paragraph wraps onto several lines: {line_tops:?}");
        let link_index = layout
            .items
            .iter()
            .position(|item| matches!(item, LaidOut::Text { link: Some(_), .. }))
            .expect("a link run");
        assert_eq!(layout.links, ["#faq"]);
        assert!(document.anchor_offset("#faq").is_some_and(|offset| offset > 0.0));
        assert_eq!(
            document.headings().iter().map(|(level, text, _)| (*level, text.as_str())).collect::<Vec<_>>(),
            [(1, "Rules"), (2, "FAQ")]
        );

        let mut clicked = None;
        let mut draw = |frame: &mut Frame| {
            let mut options = MarkdownOptions { show_title_heading: true, ..MarkdownOptions::default() };
            frame.markdown(&mut document, 10.0, 10.0, 160.0, &mut options);
            if let Some(target) = options.clicked_link {
                clicked = Some(target);
            }
        };
        harness.click_region(&format!("md:link:{link_index}"), &mut draw);
        assert_eq!(clicked.as_deref(), Some("#faq"));
    }
}
