//! The near-fullscreen viewer for a game's documents. Port of
//! `src/ui/screens/rulebook_viewer.lua`.
//!
//! Every PDF or Markdown file directly in the game's `_documents` folder is offered in a
//! dropdown, with a file named "Rulebook" always first; while a loaded scenario has a guide, a
//! toggle switches to its Scenario Guide. Markdown shows with a table-of-contents sidebar built
//! from its headings; a PDF shows as continuous-scroll page images rendered on a worker thread.
//! Each document's scroll position is remembered in the settings. Opened with Ctrl+R at the
//! table, in the deck editor and in the game definition editor.
//!
//! Markdown images go through the shared image cache (loaded off the main thread) instead of the
//! original's one-off synchronous decode -- the alternative its own comments describe.
//!
//! Links in Markdown documents work (a GameTable addition): `#anchor` links scroll to their
//! heading or `<a name>`, web links ask before opening the browser, and a link to another file in
//! `_documents` (optionally with `#anchor`) switches to that document.

use gametable_core::models::{GameDefinition, Scenario};

use crate::app::Frame;
use crate::data::display_settings::{rulebook_scroll, set_rulebook_scroll};
use crate::data::file_system::{join_path, lowercase_extension, strip_extension};
use crate::data::game_files;
use crate::data::pdf_doc_cache::PdfDocCache;
use crate::graphics::draw_list::Color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::markdown::{MarkdownBlock, MarkdownDocument, MarkdownOptions, follow_link};
use crate::ui::panel::{PANEL_TITLE_BAR_HEIGHT, PanelLeading};
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{Choice, ScrollbarStyle};

const TABLE_OF_CONTENTS_WIDTH: f32 = 260.0;
const TABLE_OF_CONTENTS_GAP: f32 = 16.0;
const TABLE_OF_CONTENTS_INDENT: f32 = 14.0;
const CONTENT_PADDING: f32 = 20.0;
const TABLE_OF_CONTENTS_TEXT_COLOR: Color = Color::rgba(1.0, 1.0, 1.0, 0.85);
const EMPTY_TEXT_COLOR: Color = Color::rgba(1.0, 1.0, 1.0, 0.54);

const PDF_PAGE_GAP: f32 = 16.0;
/// Room on the right of the page column so the scrollbar never sits on a page.
const PDF_SCROLLBAR_GUTTER: f32 = 20.0;
/// US Letter portrait: a PDF panel is sized to show a whole page of this shape at its height.
const PAGE_ASPECT: f32 = 8.5 / 11.0;
/// The panel is always dark, where the default thumb is too dim.
const SCROLL_THUMB_STYLE: ScrollbarStyle =
    ScrollbarStyle { thumb_color: None, thumb_alpha: 0.6, thumb_alpha_hover: 0.85 };
/// Pages are rasterized this many pixels wide once and scaled to the column.
const PDF_RENDER_WIDTH: u32 = 1600;

const DOCUMENT_TOGGLE_HEIGHT: f32 = 36.0;
const DOCUMENT_DROPDOWN_WIDTH: f32 = 240.0;
const ZOOM_WIDTH: f32 = 240.0;

/// Which document the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSlot {
    /// A file from `_documents`.
    Documents,
    /// The loaded scenario's guide.
    ScenarioGuide,
}

impl DocumentSlot {
    fn key(self) -> &'static str {
        match self {
            DocumentSlot::Documents => "rulebook",
            DocumentSlot::ScenarioGuide => "guide",
        }
    }

    fn title(self) -> &'static str {
        match self {
            DocumentSlot::Documents => "Documents",
            DocumentSlot::ScenarioGuide => "Scenario Guide",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentKind {
    Pdf,
    Markdown,
}

fn kind_of(path: &str) -> DocumentKind {
    if lowercase_extension(path) == ".pdf" { DocumentKind::Pdf } else { DocumentKind::Markdown }
}

/// Where each page sits in the continuous scroll.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfLayout {
    /// (top, height) per page.
    pub pages: Vec<(f32, f32)>,
    pub content_height: f32,
}

/// `computePdfLayout`: every page scaled to fill the column width.
pub fn compute_pdf_layout(point_sizes: &[(f64, f64)], column_width: f32, gap: f32) -> PdfLayout {
    let mut pages = Vec::with_capacity(point_sizes.len());
    let mut y = 0.0;
    for &(width, height) in point_sizes {
        let page_height = if width > 0.0 { column_width * (height / width) as f32 } else { 0.0 };
        pages.push((y, page_height));
        y += page_height + gap;
    }
    PdfLayout { pages, content_height: (y - gap).max(0.0) }
}

fn is_rulebook_name(name: &str) -> bool {
    strip_extension(name).to_lowercase() == "rulebook"
}

/// `listDocuments`: the PDF and Markdown files directly in `<folder>/_documents`, "Rulebook"
/// first, then by name ignoring case.
pub fn list_documents(folder: Option<&str>) -> Vec<String> {
    let Some(folder) = folder else { return Vec::new() };
    let documents_folder = join_path(&[folder, "_documents"]);
    let mut names: Vec<String> = game_files::list_folder(&documents_folder)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| !entry.is_folder && matches!(lowercase_extension(&entry.name).as_str(), ".pdf" | ".md"))
        .map(|entry| entry.name)
        .collect();
    names.sort_by(|a, b| {
        is_rulebook_name(b).cmp(&is_rulebook_name(a)).then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    names
}

/// One loaded document.
struct LoadedDocument {
    path: Option<String>,
    kind: DocumentKind,
    has_content: bool,
    markdown: MarkdownDocument,
    pdf: Option<PdfDocCache>,
    pdf_layout: Option<(f32, PdfLayout)>,
    scroll_id: String,
    scroll_key: String,
    pending_restore: Option<f32>,
}

pub struct RulebookViewer {
    game_id: String,
    folder_path: Option<String>,
    open: bool,
    active: DocumentSlot,
    documents: Option<LoadedDocument>,
    guide: Option<LoadedDocument>,
    table_of_contents_visible: bool,
    /// 1 = 100%, 2 = 150%, 3 = 200% of the panel's default width.
    zoom_level: u8,
    selected_document: Option<String>,
    available_documents: Vec<String>,
    /// An anchor to jump to once the document a link switched to has been laid out.
    pending_anchor: Option<String>,
}

impl RulebookViewer {
    pub fn new(game: &GameDefinition) -> RulebookViewer {
        RulebookViewer {
            game_id: game.id.clone(),
            folder_path: game.folder_path.clone(),
            open: false,
            active: DocumentSlot::Documents,
            documents: None,
            guide: None,
            table_of_contents_visible: true,
            zoom_level: 1,
            selected_document: None,
            available_documents: Vec::new(),
            pending_anchor: None,
        }
    }

    /// Follows a game definition that changed (the editor saves, a host resends the game).
    pub fn set_game(&mut self, game: &GameDefinition) {
        if game.id != self.game_id || game.folder_path != self.folder_path {
            *self = RulebookViewer { open: self.open, ..RulebookViewer::new(game) };
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Development hook: opens the panel without a frame (nothing to save on opening).
    pub fn open_for_development(&mut self) {
        self.open = true;
    }

    fn slot(&self, slot: DocumentSlot) -> Option<&LoadedDocument> {
        match slot {
            DocumentSlot::Documents => self.documents.as_ref(),
            DocumentSlot::ScenarioGuide => self.guide.as_ref(),
        }
    }

    /// `RV:toggle()`: closing remembers where the shown document was scrolled to.
    pub fn toggle(&mut self, frame: &mut Frame) {
        self.open = !self.open;
        if !self.open
            && let Some(document) = self.slot(self.active)
        {
            let offset = frame.ui.scroll_offset(&document.scroll_id);
            set_rulebook_scroll(frame.settings, &document.scroll_key, f64::from(offset));
        }
    }

    /// `RV:ensureDoc`: (re)loads a slot when its path changed, restoring its saved scroll.
    fn ensure_document(
        frame: &mut Frame,
        game_id: &str,
        slot_value: &mut Option<LoadedDocument>,
        slot: DocumentSlot,
        path: Option<String>,
        base_folder: Option<String>,
        scroll_key: String,
    ) {
        if slot_value.as_ref().is_some_and(|document| document.path == path) {
            return;
        }
        let kind = path.as_deref().map_or(DocumentKind::Markdown, kind_of);
        let mut document = LoadedDocument {
            path: path.clone(),
            kind,
            has_content: false,
            markdown: MarkdownDocument::default(),
            pdf: None,
            pdf_layout: None,
            scroll_id: format!("rulebook:{}:{game_id}", slot.key()),
            scroll_key,
            pending_restore: None,
        };
        if let Some(path) = &path {
            if kind == DocumentKind::Pdf {
                document.has_content = true;
                document.pdf = Some(PdfDocCache::new(path));
            } else if let Ok(bytes) = game_files::read(path) {
                document.has_content = true;
                document.markdown = MarkdownDocument::parse(&String::from_utf8_lossy(&bytes));
                for block in document.markdown.blocks_mut() {
                    if let (MarkdownBlock::Image { path: image, resolved_path, .. }, Some(base)) =
                        (block, base_folder.as_deref())
                    {
                        *resolved_path = Some(join_path(&[base, image]));
                    }
                }
            }
            let saved = rulebook_scroll(frame.settings, &document.scroll_key) as f32;
            frame.ui.scroll_to(&document.scroll_id, saved);
            document.pending_restore = Some(saved);
        }
        *slot_value = Some(document);
    }

    /// `RV:ensureLoaded()`: refreshes the `_documents` listing, keeps the selection valid
    /// (defaulting to the first file) and loads it.
    fn ensure_documents_loaded(&mut self, frame: &mut Frame) {
        let folder = self.folder_path.clone();
        self.available_documents = list_documents(folder.as_deref());
        if !self.selected_document.as_ref().is_some_and(|selected| self.available_documents.contains(selected)) {
            self.selected_document = self.available_documents.first().cloned();
        }
        let path = match (folder.as_deref(), self.selected_document.as_deref()) {
            (Some(folder), Some(file)) => {
                let path = join_path(&[folder, "_documents", file]);
                game_files::is_file(&path).then_some(path)
            }
            _ => None,
        };
        let base = folder.as_deref().map(|folder| join_path(&[folder, "_documents"]));
        let scroll_key = format!("{}:doc:{}", self.game_id, self.selected_document.as_deref().unwrap_or("nil"));
        let game_id = self.game_id.clone();
        RulebookViewer::ensure_document(
            frame,
            &game_id,
            &mut self.documents,
            DocumentSlot::Documents,
            path,
            base,
            scroll_key,
        );
    }

    /// Follows a link to another of the game's documents: switches to it (and to its anchor),
    /// or says it isn't there.
    fn open_linked_document(&mut self, frame: &mut Frame, file: &str, anchor: Option<String>) {
        let wanted = file.to_lowercase();
        match self.available_documents.iter().find(|name| name.to_lowercase() == wanted).cloned() {
            Some(name) => {
                self.active = DocumentSlot::Documents;
                self.selected_document = Some(name);
                self.ensure_documents_loaded(frame);
                self.pending_anchor = anchor;
            }
            None => frame.ui.toast("That document isn't in this game's documents.", None),
        }
    }

    /// `RV:ensureGuideLoaded(scenario)`: the scenario's guide, or none.
    fn ensure_guide_loaded(&mut self, frame: &mut Frame, scenario: Option<&Scenario>) -> bool {
        let folder = self.folder_path.clone();
        let path = match (folder.as_deref(), scenario.and_then(|scenario| scenario.guide_path.as_deref())) {
            (Some(folder), Some(guide)) => {
                let path = join_path(&[folder, "_scenarios", guide]);
                game_files::is_file(&path).then_some(path)
            }
            _ => None,
        };
        let (Some(path), Some(scenario), Some(folder)) = (path, scenario, folder) else {
            self.guide = None;
            return false;
        };
        let scroll_key = format!("{}:guide:{}", self.game_id, scenario.id);
        let game_id = self.game_id.clone();
        RulebookViewer::ensure_document(
            frame,
            &game_id,
            &mut self.guide,
            DocumentSlot::ScenarioGuide,
            Some(path),
            Some(join_path(&[&folder, "_scenarios"])),
            scroll_key,
        );
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_table_of_contents(
        frame: &mut Frame,
        scroll_target: &str,
        id: &str,
        entries: &[(u8, String, f32)],
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let offset = frame.begin_scroll(id, x, y, width, height);
        let start_y = y - offset;
        let mut row_y = start_y;
        for (index, (level, text, target)) in entries.iter().enumerate() {
            let indent = f32::from(level - 1) * TABLE_OF_CONTENTS_INDENT;
            let row_width = width - indent;
            let row_height = frame.text_height(text, row_width - 8.0, 12.0, Weight::Regular) + 10.0;
            if row_y + row_height > y && row_y < y + height {
                let region = frame.ui.region(
                    &format!("rbtoc:{}", index + 1),
                    x,
                    row_y,
                    width,
                    row_height,
                    RegionOptions::hand(),
                );
                if region.hovered {
                    frame.fill_rect(x, row_y, width, row_height, 4.0, with_alpha(palette.on_surface, 0.08));
                }
                let options = TextOptions::new()
                    .width(row_width - 8.0)
                    .size(12.0)
                    .max_lines(2)
                    .color(TABLE_OF_CONTENTS_TEXT_COLOR);
                frame.text(text, x + indent + 8.0, row_y + 5.0, options);
                if region.pressed {
                    frame.ui.scroll_to(scroll_target, *target);
                }
            }
            row_y += row_height;
        }
        frame.end_scroll(id, row_y - start_y, &SCROLL_THUMB_STYLE);
    }

    /// `RV:drawPdfPages`: only pages within a screen of the view are requested and drawn.
    #[allow(clippy::too_many_arguments)]
    fn draw_pdf_pages(
        frame: &mut Frame,
        document: &mut LoadedDocument,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        offset: f32,
        page_width: f32,
    ) {
        let _ = width;
        let palette = frame.palette();
        let Some((_, layout)) = document.pdf_layout.clone() else { return };
        let Some(cache) = document.pdf.as_mut() else { return };
        let slack = height;
        for (index, &(page_top, page_height)) in layout.pages.iter().enumerate() {
            let top = y + page_top - offset;
            if top + page_height > y - slack && top < y + height + slack {
                cache.request_page(index, PDF_RENDER_WIDTH);
                match cache.page_texture(frame.images, index) {
                    Some(texture) => frame.draw_image(texture, x, top, page_width / texture.width, 1.0),
                    None => {
                        frame.fill_rect(x, top, page_width, page_height, 4.0, with_alpha(palette.on_surface, 0.06));
                        if cache.page_failed(index) {
                            let options = TextOptions::new()
                                .width(page_width)
                                .middle_of(page_height)
                                .centered()
                                .color(EMPTY_TEXT_COLOR);
                            frame.text("Couldn't render this page.", x, top, options);
                        }
                    }
                }
            }
        }
    }

    /// `RV:draw(onClose, scenario)`: draw inside an overlay. `scenario` offers its Scenario
    /// Guide beside the documents. Returns true when the panel asked to close (the X or a click
    /// outside); the host then calls [`RulebookViewer::toggle`].
    pub fn draw(&mut self, frame: &mut Frame, scenario: Option<&Scenario>) -> bool {
        // The panel is always the dark in-game style, so its Markdown uses the dark palette.
        frame.theme.force_dark = true;
        let close = self.draw_panel(frame, scenario);
        frame.theme.force_dark = false;
        close
    }

    fn draw_panel(&mut self, frame: &mut Frame, scenario: Option<&Scenario>) -> bool {
        self.ensure_documents_loaded(frame);
        let guide_available = self.ensure_guide_loaded(frame, scenario);
        if self.active == DocumentSlot::ScenarioGuide && !guide_available {
            self.active = DocumentSlot::Documents;
        }
        for document in [self.documents.as_mut(), self.guide.as_mut()].into_iter().flatten() {
            if let Some(cache) = document.pdf.as_mut() {
                cache.update(frame.images);
            }
        }

        let (screen_width, screen_height) = frame.ui.screen_size();
        let panel_height = (screen_height - frame.ui.content_top - 40.0) * 0.9;
        let kind = self.slot(self.active).map_or(DocumentKind::Markdown, |document| document.kind);
        let mut panel_width = if kind == DocumentKind::Pdf {
            let content_height = panel_height - PANEL_TITLE_BAR_HEIGHT - 2.0 * CONTENT_PADDING;
            let page_width = content_height * PAGE_ASPECT;
            (screen_width - 40.0).min(page_width + PDF_SCROLLBAR_GUTTER + 2.0 * CONTENT_PADDING)
        } else {
            (screen_width - 40.0) * 0.5
        };
        match self.zoom_level {
            2 => panel_width = (screen_width - 40.0).min(panel_width * 1.5),
            3 => panel_width = (screen_width - 40.0).min(panel_width * 2.0),
            _ => {}
        }
        let show_table_of_contents = kind == DocumentKind::Markdown;
        let leading = show_table_of_contents.then_some(if self.table_of_contents_visible {
            PanelLeading { icon: "chevron_left", tooltip: "Hide contents" }
        } else {
            PanelLeading { icon: "chevron_right", tooltip: "Show contents" }
        });
        let response = frame.panel("rulebook", panel_width, panel_height, self.active.title(), None, leading);
        if response.leading_clicked {
            self.table_of_contents_visible = !self.table_of_contents_visible;
        }
        let content = response.content;
        let (x, mut y, width, mut height) = (
            content.x + CONTENT_PADDING,
            content.y + CONTENT_PADDING,
            content.width - 2.0 * CONTENT_PADDING,
            content.height - 2.0 * CONTENT_PADDING,
        );

        let mut row_x = x;
        if guide_available {
            let choices = [
                Choice::new(DocumentSlot::Documents, "Documents"),
                Choice::new(DocumentSlot::ScenarioGuide, "Scenario Guide"),
            ];
            self.active = frame.segmented(
                "rulebook:doctoggle",
                x,
                y,
                280.0,
                &choices,
                self.active,
                Some(DOCUMENT_TOGGLE_HEIGHT),
                false,
            );
            row_x = x + 280.0 + 12.0;
        }
        if self.active == DocumentSlot::Documents && !self.available_documents.is_empty() {
            let choices: Vec<Choice<String>> =
                self.available_documents.iter().map(|name| Choice::new(name.clone(), name.clone())).collect();
            let options = DropdownOptions { height: Some(DOCUMENT_TOGGLE_HEIGHT), ..DropdownOptions::default() };
            let current = self.selected_document.clone().unwrap_or_default();
            let picked =
                frame.dropdown("rulebook:docselect", row_x, y, DOCUMENT_DROPDOWN_WIDTH, &choices, current, &options);
            // `RV:selectDocument`: switches and loads at once.
            if self.selected_document.as_ref() != Some(&picked) {
                self.selected_document = Some(picked);
                self.ensure_documents_loaded(frame);
            }
        }
        let zoom_choices = [Choice::new(1u8, "100%"), Choice::new(2u8, "150%"), Choice::new(3u8, "200%")];
        self.zoom_level = frame.segmented(
            "rulebook:zoom",
            x + width - ZOOM_WIDTH,
            y,
            ZOOM_WIDTH,
            &zoom_choices,
            self.zoom_level,
            Some(DOCUMENT_TOGGLE_HEIGHT),
            false,
        );
        y += DOCUMENT_TOGGLE_HEIGHT + 12.0;
        height -= DOCUMENT_TOGGLE_HEIGHT + 12.0;

        let active = self.active;
        let table_of_contents_visible = self.table_of_contents_visible;
        let pending_anchor = self.pending_anchor.take();
        let document = match active {
            DocumentSlot::Documents => self.documents.as_mut(),
            DocumentSlot::ScenarioGuide => self.guide.as_mut(),
        };
        let Some(document) = document.filter(|document| document.has_content) else {
            let message = if active == DocumentSlot::ScenarioGuide {
                "This scenario doesn't have a guide yet."
            } else {
                "This game doesn't have any documents yet."
            };
            let options = TextOptions::new().width(width).middle_of(height).centered().color(EMPTY_TEXT_COLOR);
            frame.text(message, x, y, options);
            return response.close_requested;
        };

        if document.kind == DocumentKind::Pdf {
            let Some(cache) = document.pdf.as_ref() else { return response.close_requested };
            if let Some(error) = &cache.open_error {
                let message = format!("This document's PDF couldn't be loaded: {error}");
                let options = TextOptions::new().width(width).middle_of(height).centered().color(EMPTY_TEXT_COLOR);
                frame.text(&message, x, y, options);
                return response.close_requested;
            }
            let Some(point_sizes) = cache.point_sizes.clone() else {
                frame.spinner(x + width / 2.0, y + height / 2.0, 14.0, None);
                return response.close_requested;
            };
            let page_width = width - PDF_SCROLLBAR_GUTTER;
            // Rebuilt whenever the column width changes, so pages never gap or overlap.
            if document.pdf_layout.as_ref().is_none_or(|(built_for, _)| *built_for != page_width) {
                document.pdf_layout = Some((page_width, compute_pdf_layout(&point_sizes, page_width, PDF_PAGE_GAP)));
            }
            let scroll_id = document.scroll_id.clone();
            let offset = frame.begin_scroll(&scroll_id, x, y, width, height);
            RulebookViewer::draw_pdf_pages(frame, document, x, y, width, height, offset, page_width);
            let content_height = document.pdf_layout.as_ref().map_or(0.0, |(_, layout)| layout.content_height);
            frame.end_scroll(&scroll_id, content_height, &SCROLL_THUMB_STYLE);
            if let Some(restore) = document.pending_restore.take() {
                frame.ui.scroll_to(&scroll_id, restore);
            }
            return response.close_requested;
        }

        let (document_x, document_width) = if table_of_contents_visible {
            (
                x + TABLE_OF_CONTENTS_WIDTH + TABLE_OF_CONTENTS_GAP,
                width - TABLE_OF_CONTENTS_WIDTH - TABLE_OF_CONTENTS_GAP,
            )
        } else {
            (x, width)
        };
        let scroll_id = document.scroll_id.clone();
        let offset = frame.begin_scroll(&scroll_id, document_x, y, document_width, height);
        let mut options = MarkdownOptions {
            show_title_heading: true,
            id: &scroll_id,
            visible: Some((y, y + height)),
            clicked_link: None,
        };
        let content_height =
            frame.markdown(&mut document.markdown, document_x, y - offset, document_width, &mut options);
        let clicked_link = options.clicked_link.take();
        frame.end_scroll(&scroll_id, content_height, &SCROLL_THUMB_STYLE);
        if let Some(restore) = document.pending_restore.take() {
            frame.ui.scroll_to(&scroll_id, restore);
        }
        if let Some(anchor) = pending_anchor {
            match document.markdown.anchor_offset(&anchor) {
                Some(target) => frame.ui.scroll_to(&scroll_id, (target - 8.0).max(0.0)),
                None => frame.ui.toast("That section isn't in this document.", None),
            }
        }
        let linked_document =
            clicked_link.and_then(|target| follow_link(frame, &document.markdown, Some(&scroll_id), &target));
        let table_of_contents: Vec<(u8, String, f32)> = document.markdown.headings().to_vec();
        if table_of_contents_visible {
            let id = format!("rulebook:toc:{}:{}", active.key(), self.game_id);
            RulebookViewer::draw_table_of_contents(
                frame,
                &scroll_id,
                &id,
                &table_of_contents,
                x,
                y,
                TABLE_OF_CONTENTS_WIDTH,
                height,
            );
        }
        if let Some((file, anchor)) = linked_document {
            self.open_linked_document(frame, &file, anchor);
        }
        response.close_requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_pages_fill_the_column() {
        let layout = compute_pdf_layout(&[(612.0, 792.0), (792.0, 612.0), (0.0, 10.0)], 300.0, 16.0);
        let expected_first = 300.0 * 792.0 / 612.0;
        assert!((layout.pages[0].1 - expected_first).abs() < 0.001);
        assert_eq!(layout.pages[1].0, layout.pages[0].1 + 16.0);
        assert_eq!(layout.pages[2], (layout.pages[1].0 + layout.pages[1].1 + 16.0, 0.0));
        assert_eq!(layout.content_height, layout.pages[2].0);
        assert_eq!(compute_pdf_layout(&[], 300.0, 16.0).content_height, 0.0);
    }

    #[test]
    fn documents_list_rulebook_first() {
        let folder = std::env::temp_dir().join(format!("gametable_rulebook_{}", std::process::id()));
        let documents = folder.join("_documents");
        std::fs::create_dir_all(documents.join("sub.md")).unwrap();
        for name in ["b.pdf", "A.md", "rulebook.PDF", "Rulebook.md", "notes.txt"] {
            std::fs::write(documents.join(name), b"x").unwrap();
        }
        let listed = list_documents(Some(folder.to_str().unwrap()));
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(listed, ["Rulebook.md", "rulebook.PDF", "A.md", "b.pdf"]);
        assert!(list_documents(None).is_empty());
    }
}
