//! The small Markdown renderer of `src/ui/screens/controls_dialog.lua`, shared by the Controls
//! and Support dialogs and the rulebook viewer.
//!
//! It understands the subset the bundled documents use: `#`/`##`/`###` headings, pipe tables,
//! `- ` bullets, paragraphs, `**bold**` (shown as plain text, or as a bold line when it is the
//! whole line) and `![alt](path)` images on a line of their own.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use crate::app::Frame;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Weight, with_alpha};

/// One parsed block.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownBlock {
    /// Heading level 1 to 3 (deeper headings count as 3).
    Heading {
        level: u8,
        text: String,
    },
    /// A line that is entirely `**bold**`.
    BoldLine(String),
    Paragraph(String),
    Bullet(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// `path` is as written in the Markdown; the caller resolves it into `resolved_path` before
    /// drawing (nothing here reads image files).
    Image {
        alt: String,
        path: String,
        resolved_path: Option<String>,
    },
}

/// `**` and backticks removed.
fn plain(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

fn split_table_row(line: &str) -> Vec<String> {
    let body = line.trim();
    let body = body.strip_prefix('|').unwrap_or(body);
    let body = body.strip_suffix('|').unwrap_or(body);
    body.split('|').map(|cell| plain(cell.trim())).collect()
}

/// The `|---|:--:|` line under a table header.
fn is_table_separator(line: &str) -> bool {
    line.len() > 1
        && line[1..].chars().all(|character| character.is_whitespace() || matches!(character, '-' | ':' | '|'))
}

/// `# Text` -> (1, "Text"); needs whitespace after the hashes.
fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hash_count = line.chars().take_while(|&character| character == '#').count();
    if hash_count == 0 {
        return None;
    }
    let rest = &line[hash_count..];
    let text = rest.trim_start();
    (text.len() < rest.len()).then(|| (hash_count.min(3) as u8, text))
}

/// `**whole line**` with no other `**` inside.
fn parse_bold_line(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("**")?.strip_suffix("**")?;
    (!inner.contains("**")).then_some(inner)
}

/// `![alt](path)` filling the whole line.
fn parse_image(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("![")?;
    let (alt, after_alt) = rest.split_once("](")?;
    let path = after_alt.strip_suffix(')')?;
    Some((alt, path))
}

/// `M.parse(text)`.
pub fn parse_markdown(text: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Option<String> = None;
    let mut table: Option<(Vec<String>, Vec<Vec<String>>)> = None;
    let flush = |blocks: &mut Vec<MarkdownBlock>,
                 paragraph: &mut Option<String>,
                 table: &mut Option<(Vec<String>, Vec<Vec<String>>)>| {
        if let Some(text) = paragraph.take() {
            blocks.push(MarkdownBlock::Paragraph(text));
        }
        if let Some((header, rows)) = table.take() {
            blocks.push(MarkdownBlock::Table { header, rows });
        }
    };
    for raw_line in text.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut table);
        } else if line.starts_with('|') {
            if paragraph.is_some() {
                flush(&mut blocks, &mut paragraph, &mut table);
            }
            if !is_table_separator(line) {
                let cells = split_table_row(line);
                match &mut table {
                    None => table = Some((cells, Vec::new())),
                    Some((_, rows)) => rows.push(cells),
                }
            }
        } else {
            if table.is_some() {
                flush(&mut blocks, &mut paragraph, &mut table);
            }
            if let Some((level, text)) = parse_heading(line) {
                flush(&mut blocks, &mut paragraph, &mut table);
                blocks.push(MarkdownBlock::Heading { level, text: plain(text) });
            } else if let Some(text) = line.strip_prefix("- ") {
                flush(&mut blocks, &mut paragraph, &mut table);
                blocks.push(MarkdownBlock::Bullet(plain(text)));
            } else if let Some(text) = parse_bold_line(line) {
                flush(&mut blocks, &mut paragraph, &mut table);
                blocks.push(MarkdownBlock::BoldLine(text.to_string()));
            } else if let Some((alt, path)) = parse_image(line) {
                flush(&mut blocks, &mut paragraph, &mut table);
                blocks.push(MarkdownBlock::Image { alt: alt.to_string(), path: path.to_string(), resolved_path: None });
            } else {
                let text = plain(line);
                paragraph = Some(match paragraph.take() {
                    Some(existing) => format!("{existing} {text}"),
                    None => text,
                });
            }
        }
    }
    flush(&mut blocks, &mut paragraph, &mut table);
    blocks
}

static LOADED_DOCUMENTS: LazyLock<Mutex<HashMap<String, Vec<MarkdownBlock>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `M.loadBlocks(source)`: a bundled Markdown file from the assets folder, parsed once.
pub fn load_bundled_document(assets_folder: &Path, file_name: &str) -> Vec<MarkdownBlock> {
    let mut documents = LOADED_DOCUMENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    documents
        .entry(file_name.to_string())
        .or_insert_with(|| {
            let text = std::fs::read_to_string(assets_folder.join(file_name))
                .unwrap_or_else(|_| format!("{file_name} is missing from this build."));
            parse_markdown(&text)
        })
        .clone()
}

/// Called with a heading's level, text and offset from the top of the content.
pub type HeadingCallback<'a> = &'a mut dyn FnMut(u8, &str, f32);

/// Options for [`Frame::markdown`].
#[derive(Default)]
pub struct MarkdownOptions<'a> {
    /// Also draw level-1 headings (normally the dialog title names the page).
    pub show_title_heading: bool,
    /// Called for each drawn heading with its level, text and offset from the top of the
    /// content, before moving past it.
    pub on_heading: Option<HeadingCallback<'a>>,
}

/// Column widths as fractions of the table width, by column count.
fn column_fractions(column_count: usize) -> Vec<f32> {
    match column_count {
        1 => vec![1.0],
        2 => vec![0.34, 0.66],
        3 => vec![0.24, 0.46, 0.30],
        _ => vec![1.0 / column_count.max(1) as f32; column_count],
    }
}

const CELL_PADDING: f32 = 8.0;

impl Frame<'_> {
    #[allow(clippy::needless_range_loop)]
    fn markdown_table(&mut self, header: &[String], rows: &[Vec<String>], x: f32, y: f32, width: f32) -> f32 {
        let palette = self.palette();
        let column_count = header.len();
        let fractions = column_fractions(column_count);
        let widths: Vec<f32> = fractions.iter().map(|fraction| width * fraction).collect();
        let mut column_x = Vec::with_capacity(column_count);
        let mut next_x = x;
        for column_width in &widths {
            column_x.push(next_x);
            next_x += column_width;
        }
        let mut row_y = y;
        for (row_index, cells) in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            let is_header = row_index == 0;
            let weight = if is_header { Weight::Bold } else { Weight::Regular };
            let mut row_height: f32 = 0.0;
            for column in 0..column_count {
                let cell = cells.get(column).map_or("", String::as_str);
                row_height = row_height.max(self.text_height(cell, widths[column] - 2.0 * CELL_PADDING, 13.0, weight));
            }
            row_height += 2.0 * CELL_PADDING;
            if is_header {
                self.fill_rect(x, row_y, width, row_height, 0.0, palette.surface_container_highest);
            }
            let color = if is_header { palette.on_surface } else { palette.on_surface_variant };
            for column in 0..column_count {
                let cell = cells.get(column).map_or("", String::as_str);
                let options = TextOptions::new()
                    .width(widths[column] - 2.0 * CELL_PADDING)
                    .size(13.0)
                    .weight(weight)
                    .color(color);
                self.text(cell, column_x[column] + CELL_PADDING, row_y + CELL_PADDING, options);
            }
            self.divider(x, row_y + row_height - 1.0, width, Some(with_alpha(palette.outline, 0.4)));
            row_y += row_height;
        }
        row_y - y
    }

    /// `M.drawBlocks(x, y, w, list, opts)`: returns the height used.
    pub fn markdown(
        &mut self,
        blocks: &[MarkdownBlock],
        x: f32,
        y: f32,
        width: f32,
        options: &mut MarkdownOptions,
    ) -> f32 {
        let palette = self.palette();
        let mut block_y = y;
        let mut first = true;
        for block in blocks {
            match block {
                MarkdownBlock::Heading { level: 1, text } => {
                    if options.show_title_heading {
                        if !first {
                            block_y += 24.0;
                        }
                        if let Some(on_heading) = options.on_heading.as_mut() {
                            on_heading(1, text, block_y - y);
                        }
                        self.text(text, x, block_y, TextOptions::new().size(22.0).bold().color(palette.on_surface));
                        block_y += 36.0;
                    }
                }
                MarkdownBlock::Heading { level: 2, text } => {
                    if !first {
                        block_y += 20.0;
                    }
                    if let Some(on_heading) = options.on_heading.as_mut() {
                        on_heading(2, text, block_y - y);
                    }
                    self.text(text, x, block_y, TextOptions::new().size(18.0).bold().color(palette.primary));
                    block_y += 30.0;
                }
                MarkdownBlock::Heading { level, text } => {
                    block_y += 12.0;
                    if let Some(on_heading) = options.on_heading.as_mut() {
                        on_heading(*level, text, block_y - y);
                    }
                    self.text(text, x, block_y, TextOptions::new().size(15.0).bold().color(palette.on_surface));
                    block_y += 26.0;
                }
                MarkdownBlock::BoldLine(text) => {
                    block_y += 8.0;
                    self.text(text, x, block_y, TextOptions::new().bold().color(palette.on_surface));
                    block_y += 24.0;
                }
                MarkdownBlock::Bullet(text) => {
                    self.text("•", x + 4.0, block_y, TextOptions::new().size(13.0).color(palette.on_surface_variant));
                    let options = TextOptions::new().width(width - 20.0).size(13.0).color(palette.on_surface_variant);
                    self.text(text, x + 20.0, block_y, options);
                    block_y += self.text_height(text, width - 20.0, 13.0, Weight::Regular) + 6.0;
                }
                MarkdownBlock::Paragraph(text) => {
                    let options = TextOptions::new().width(width).size(13.0).color(palette.on_surface_variant);
                    self.text(text, x, block_y, options);
                    block_y += self.text_height(text, width, 13.0, Weight::Regular) + 10.0;
                }
                MarkdownBlock::Table { header, rows } => {
                    block_y += self.markdown_table(header, rows, x, block_y, width) + 12.0;
                }
                MarkdownBlock::Image { alt, resolved_path, .. } => {
                    block_y += 8.0;
                    let texture = resolved_path.as_deref().and_then(|path| self.images.get(path));
                    if let Some(texture) = texture {
                        let scale = width / texture.width;
                        self.draw_image(texture, x, block_y, scale, 1.0);
                        block_y += texture.height * scale + 12.0;
                    } else {
                        // Not loaded (yet), failed, or never resolved: show the alt text so the gap
                        // isn't silently blank.
                        let placeholder_height = 72.0;
                        let fill = with_alpha(palette.on_surface_variant, 0.15);
                        self.fill_rect(x, block_y, width, placeholder_height, 4.0, fill);
                        let label = if alt.is_empty() { "(image)" } else { alt.as_str() };
                        let options = TextOptions::new()
                            .width(width)
                            .middle_of(placeholder_height)
                            .centered()
                            .color(palette.on_surface_variant);
                        self.text(label, x, block_y, options);
                        block_y += placeholder_height + 12.0;
                    }
                }
            }
            let is_hidden_title =
                matches!(block, MarkdownBlock::Heading { level: 1, .. }) && !options.show_title_heading;
            if !is_hidden_title {
                first = false;
            }
        }
        block_y - y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_supported_subset() {
        let text = "# Title\r\n\nSome **bold** text\nwrapped `code`.\n\n## Section\n| Key | Action |\n|---|:--|\n| **A** | Do it |\n- bullet **one**\n**Whole bold**\n![Alt](img/a.png)\n#### Deep\n#nospace";
        let blocks = parse_markdown(text);
        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::Heading { level: 1, text: "Title".into() },
                MarkdownBlock::Paragraph("Some bold text wrapped code.".into()),
                MarkdownBlock::Heading { level: 2, text: "Section".into() },
                MarkdownBlock::Table {
                    header: vec!["Key".into(), "Action".into()],
                    rows: vec![vec!["A".into(), "Do it".into()]],
                },
                MarkdownBlock::Bullet("bullet one".into()),
                MarkdownBlock::BoldLine("Whole bold".into()),
                MarkdownBlock::Image { alt: "Alt".into(), path: "img/a.png".into(), resolved_path: None },
                MarkdownBlock::Heading { level: 3, text: "Deep".into() },
                MarkdownBlock::Paragraph("#nospace".into()),
            ]
        );
    }

    #[test]
    fn bold_lines_need_a_single_pair() {
        assert_eq!(parse_bold_line("**a** and **b**"), None);
        assert_eq!(parse_bold_line("**ab**"), Some("ab"));
        assert_eq!(parse_bold_line("***"), None);
    }
}
