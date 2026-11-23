//! Markdown (with common HTML) into blocks: headings, paragraphs, lists, quotes, rules, code,
//! tables and images. Inline content in each block is parsed by [`parse_inline`].

use super::html::{HtmlTag, decode_entities, leading_tag, parse_tag, strip_comments, whole_line_tag};
use super::inline::{Inline, InlineItem, SpanStyle, parse_inline};

/// What starts a list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    Bullet,
    Number(u32),
    /// A paragraph that continues the item above it (indented, no marker).
    Continuation,
}

/// One parsed block.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownBlock {
    /// Level 1 to 6, with an explicit `{#id}` / `id="…"` when one was written.
    Heading {
        level: u8,
        content: Inline,
        id: Option<String>,
    },
    Paragraph {
        content: Inline,
        centered: bool,
    },
    /// `depth` 0 for a top-level item, 1 for one nested under it, and so on.
    ListItem {
        depth: u8,
        marker: ListMarker,
        content: Inline,
    },
    Quote(Inline),
    Rule,
    /// A fenced code block, as written.
    Code(String),
    /// `header` is empty for an HTML table without header cells.
    Table {
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    /// `path` is as written; the caller resolves it into `resolved_path` before drawing (nothing
    /// here reads image files). `width` is an HTML `width` in pixels, when given.
    Image {
        alt: String,
        path: String,
        resolved_path: Option<String>,
        width: Option<f32>,
        centered: bool,
    },
}

/// `# Text {#id}` -> (level, text, id); needs whitespace after the hashes. Closing hashes go.
fn parse_heading(line: &str) -> Option<(u8, &str, Option<String>)> {
    let hash_count = line.chars().take_while(|&character| character == '#').count();
    if hash_count == 0 || hash_count > 6 {
        return None;
    }
    let rest = &line[hash_count..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut text = rest.trim();
    let mut id = None;
    if text.ends_with('}')
        && let Some(open) = text.rfind("{#")
    {
        id = Some(text[open + 2..text.len() - 1].trim().to_string());
        text = text[..open].trim_end();
    }
    let without_closing = text.trim_end_matches('#');
    if without_closing.len() < text.len() && (without_closing.is_empty() || without_closing.ends_with(' ')) {
        text = without_closing.trim_end();
    }
    Some((hash_count as u8, text, id))
}

/// `---`, `***` or `___` (three or more, spaces allowed).
fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|character| !character.is_whitespace()).collect();
    compact.len() >= 3 && ['-', '*', '_'].iter().any(|&marker| compact.chars().all(|character| character == marker))
}

/// A list item: (indent in spaces, marker, text).
fn parse_list_item(raw_line: &str) -> Option<(usize, ListMarker, &str)> {
    let indent = leading_indent(raw_line);
    let line = raw_line.trim_start();
    for bullet in ["- ", "* ", "+ ", "-\t", "*\t", "+\t"] {
        if let Some(text) = line.strip_prefix(bullet) {
            return Some((indent, ListMarker::Bullet, text.trim_start()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let after = &line[digits..];
        if let Some(text) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            let number = line[..digits].parse().ok()?;
            return Some((indent, ListMarker::Number(number), text.trim_start()));
        }
    }
    None
}

/// Leading spaces, with a tab counting as four.
fn leading_indent(line: &str) -> usize {
    let mut indent = 0;
    for character in line.chars() {
        match character {
            ' ' => indent += 1,
            '\t' => indent += 4,
            _ => break,
        }
    }
    indent
}

fn split_table_row(line: &str) -> Vec<Inline> {
    let body = line.trim();
    let body = body.strip_prefix('|').unwrap_or(body);
    let body = body.strip_suffix('|').unwrap_or(body);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' if characters.peek() == Some(&'|') => {
                cell.push('|');
                characters.next();
            }
            '|' => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(character),
        }
    }
    cells.push(cell);
    cells.iter().map(|cell| parse_inline(cell.trim())).collect()
}

/// The `|---|:--:|` line under a table header.
fn is_table_separator(line: &str) -> bool {
    line.contains('-')
        && line.chars().all(|character| character.is_whitespace() || matches!(character, '-' | ':' | '|'))
}

/// `![alt](path)` filling the whole line.
fn parse_markdown_image(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let (alt, after_alt) = rest.split_once("](")?;
    let inside = after_alt.strip_suffix(')')?;
    let path = inside.split_once(" \"").map_or(inside, |(path, _)| path).trim();
    Some((alt.to_string(), path.trim_start_matches('<').trim_end_matches('>').to_string()))
}

/// An `<img>` block from its tag.
fn image_from_tag(tag: &HtmlTag, centered: bool) -> Option<MarkdownBlock> {
    let path = tag.attribute("src")?.to_string();
    let width = tag.attribute("width").and_then(|width| width.trim_end_matches("px").parse::<f32>().ok());
    Some(MarkdownBlock::Image {
        alt: tag.attribute("alt").unwrap_or_default().to_string(),
        path,
        resolved_path: None,
        width,
        centered,
    })
}

/// Every `<img>` in a piece of HTML when that's all it holds (apart from wrappers and spaces).
fn images_only(html: &str, centered: bool) -> Option<Vec<MarkdownBlock>> {
    let chars: Vec<char> = html.chars().collect();
    let mut images = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        let tag = parse_tag(&chars, index)?;
        index += tag.length;
        match tag.name.as_str() {
            "img" => images.push(image_from_tag(&tag, centered)?),
            "p" | "div" | "center" | "a" | "br" | "picture" | "source" => {}
            _ => return None,
        }
    }
    (!images.is_empty()).then_some(images)
}

/// The text between a block tag at the start of `lines[start]` and its closing tag (which may
/// be on a later line), and the index of the line after it.
fn collect_until_closing(lines: &[&str], start: usize, name: &str) -> (String, usize) {
    let closing = format!("</{name}");
    let mut collected = String::new();
    let mut index = start;
    while index < lines.len() {
        let line = lines[index];
        if !collected.is_empty() {
            collected.push('\n');
        }
        collected.push_str(line);
        index += 1;
        if line.to_ascii_lowercase().contains(&closing) {
            break;
        }
    }
    (collected, index)
}

/// The element whose opening tag is at `chars[start]`: the tag, where its content starts and
/// ends, and where its closing tag ends (an unclosed element runs to the end).
fn element_at(chars: &[char], start: usize) -> Option<(HtmlTag, usize, usize, usize)> {
    let open = parse_tag(chars, start)?;
    let content_start = start + open.length;
    if open.self_closing {
        return Some((open, content_start, content_start, content_start));
    }
    let mut depth = 1;
    let mut index = content_start;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some(tag) = parse_tag(chars, index)
        {
            if tag.name == open.name && !tag.self_closing {
                if tag.closing {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open, content_start, index, index + tag.length));
                    }
                } else {
                    depth += 1;
                }
            }
            index += tag.length;
            continue;
        }
        index += 1;
    }
    Some((open, content_start, chars.len(), chars.len()))
}

/// The elements named in `names` found in `html` (not looking inside them), in order, each as
/// its opening tag and inner HTML.
fn child_elements(html: &str, names: &[&str]) -> Vec<(HtmlTag, String)> {
    let chars: Vec<char> = html.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some(tag) = parse_tag(&chars, index)
        {
            if !tag.closing
                && names.contains(&tag.name.as_str())
                && let Some((open, content_start, content_end, after)) = element_at(&chars, index)
            {
                found.push((open, chars[content_start..content_end].iter().collect()));
                index = after.max(index + 1);
                continue;
            }
            index += tag.length;
            continue;
        }
        index += 1;
    }
    found
}

/// The first `<name>` element in `html`: its opening tag and inner HTML.
fn inner_html(html: &str, name: &str) -> Option<(HtmlTag, String)> {
    child_elements(html, &[name]).into_iter().next()
}

/// An HTML `<table>`: rows of `<th>`/`<td>` cells; a first row of only `<th>` is the header, and
/// a `<th>` among data cells is shown bold.
fn parse_html_table(html: &str) -> MarkdownBlock {
    let mut header: Vec<Inline> = Vec::new();
    let mut rows: Vec<Vec<Inline>> = Vec::new();
    for (_, row) in child_elements(html, &["tr"]) {
        let cells = child_elements(&row, &["th", "td"]);
        let header_row = !cells.is_empty() && cells.iter().all(|(tag, _)| tag.name == "th");
        let parsed: Vec<Inline> = cells
            .iter()
            .map(|(tag, inner)| {
                let mut content = parse_inline(inner.trim());
                if tag.name == "th" && !header_row {
                    make_bold(&mut content);
                }
                content
            })
            .collect();
        if header_row && header.is_empty() && rows.is_empty() {
            header = parsed;
        } else {
            rows.push(parsed);
        }
    }
    MarkdownBlock::Table { header, rows }
}

fn make_bold(content: &mut Inline) {
    for item in content {
        if let InlineItem::Text { style, .. } = item {
            *style = SpanStyle { bold: true, ..*style };
        }
    }
}

struct BlockParser {
    blocks: Vec<MarkdownBlock>,
    paragraph: Vec<String>,
    paragraph_centered: bool,
    table: Option<(Vec<Inline>, Vec<Vec<Inline>>)>,
    /// The list item being read: its depth, marker and lines.
    list_item: Option<(u8, ListMarker, Vec<String>)>,
    /// The indents of the open list levels, outermost first.
    list_indents: Vec<usize>,
    /// Open `<ul>`/`<ol>` lists: the next number for an `<ol>`.
    html_lists: Vec<Option<u32>>,
    quote: Vec<String>,
    /// Whether the last block ended a list item, so an indented paragraph continues it.
    after_list_item: bool,
}

impl BlockParser {
    fn new() -> BlockParser {
        BlockParser {
            blocks: Vec::new(),
            paragraph: Vec::new(),
            paragraph_centered: false,
            table: None,
            list_item: None,
            list_indents: Vec::new(),
            html_lists: Vec::new(),
            quote: Vec::new(),
            after_list_item: false,
        }
    }

    fn flush(&mut self) {
        if !self.paragraph.is_empty() {
            let content = parse_inline(&self.paragraph.join("\n"));
            self.blocks.push(MarkdownBlock::Paragraph { content, centered: self.paragraph_centered });
            self.paragraph.clear();
            self.paragraph_centered = false;
        }
        if let Some((header, rows)) = self.table.take() {
            self.blocks.push(MarkdownBlock::Table { header, rows });
        }
        if let Some((depth, marker, lines)) = self.list_item.take() {
            self.blocks.push(MarkdownBlock::ListItem { depth, marker, content: parse_inline(&lines.join("\n")) });
            self.after_list_item = true;
        }
        if !self.quote.is_empty() {
            self.blocks.push(MarkdownBlock::Quote(parse_inline(&self.quote.join("\n"))));
            self.quote.clear();
        }
    }

    /// Ends any list: the next item starts a fresh one.
    fn end_list(&mut self) {
        self.list_indents.clear();
        self.after_list_item = false;
    }

    fn push(&mut self, block: MarkdownBlock) {
        self.flush();
        self.end_list();
        self.blocks.push(block);
    }

    /// An `<li>` of the innermost open `<ul>`/`<ol>`.
    fn push_html_list_item(&mut self, inner: &str) {
        let depth = self.html_lists.len().saturating_sub(1).min(5) as u8;
        let marker = match self.html_lists.last_mut() {
            Some(Some(number)) => {
                let marker = ListMarker::Number(*number);
                *number += 1;
                marker
            }
            _ => ListMarker::Bullet,
        };
        self.blocks.push(MarkdownBlock::ListItem { depth, marker, content: parse_inline(inner.trim()) });
    }

    /// The depth of the last list item, which an indented paragraph after it continues.
    fn last_list_depth(&self) -> u8 {
        self.blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                MarkdownBlock::ListItem { depth, .. } => Some(*depth),
                _ => None,
            })
            .unwrap_or(0)
    }

    fn list_depth_for(&mut self, indent: usize) -> u8 {
        while self.list_indents.last().is_some_and(|&last| last > indent) {
            self.list_indents.pop();
        }
        if self.list_indents.last().is_none_or(|&last| last < indent) {
            self.list_indents.push(indent);
        }
        (self.list_indents.len().saturating_sub(1)).min(5) as u8
    }

    /// Handles a line starting with an HTML block tag; returns the index of the next line to read,
    /// or `None` when the line is ordinary text.
    fn html_block(&mut self, lines: &[&str], index: usize) -> Option<usize> {
        let line = lines[index].trim();
        let tag = leading_tag(line)?;
        let name = tag.name.as_str();
        match name {
            "table" if !tag.closing => {
                let (html, next) = collect_until_closing(lines, index, "table");
                self.push(parse_html_table(&html));
                Some(next)
            }
            "hr" => {
                self.push(MarkdownBlock::Rule);
                Some(index + 1)
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !tag.closing => {
                let (html, next) = collect_until_closing(lines, index, name);
                let (open, inner) = inner_html(&html, name)?;
                let level = name[1..].parse().unwrap_or(1);
                let id = open.attribute("id").or_else(|| open.attribute("name")).map(str::to_string);
                self.push(MarkdownBlock::Heading { level, content: parse_inline(inner.trim()), id });
                Some(next)
            }
            "img" if !tag.closing => {
                let images = images_only(line, false)?;
                self.flush();
                self.end_list();
                self.blocks.extend(images);
                Some(index + 1)
            }
            "p" | "div" | "center" if !tag.closing => {
                let (html, next) = collect_until_closing(lines, index, name);
                let centered = tag.is_centered();
                if let Some(images) = images_only(&html, centered) {
                    self.flush();
                    self.end_list();
                    self.blocks.extend(images);
                    return Some(next);
                }
                let inner = inner_html(&html, name).map_or(html.clone(), |(_, inner)| inner);
                // Nested block HTML inside (a table, headings): parse it as its own document.
                let inner_trimmed = inner.trim();
                if leading_tag(inner_trimmed).is_some_and(|inner_tag| {
                    matches!(inner_tag.name.as_str(), "table" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol")
                }) {
                    self.flush();
                    self.end_list();
                    self.blocks.extend(parse_markdown(inner_trimmed));
                    return Some(next);
                }
                self.flush();
                self.end_list();
                self.paragraph.push(inner_trimmed.to_string());
                self.paragraph_centered = centered;
                self.flush();
                Some(next)
            }
            "p" | "div" | "center" | "details" | "tbody" | "thead" => Some(index + 1),
            "summary" => {
                let (html, next) = collect_until_closing(lines, index, "summary");
                let inner = inner_html(&html, "summary").map_or(String::new(), |(_, inner)| inner);
                let mut content = parse_inline(inner.trim());
                make_bold(&mut content);
                self.push(MarkdownBlock::Paragraph { content, centered: false });
                Some(next)
            }
            "ul" | "ol" => {
                self.flush();
                self.end_list();
                if tag.closing {
                    self.html_lists.pop();
                    return Some(index + 1);
                }
                let start = tag.attribute("start").and_then(|start| start.parse().ok()).unwrap_or(1);
                self.html_lists.push((name == "ol").then_some(start));
                // Items on the same line as the list tag (`<ul><li>a</li><li>b</li></ul>`).
                let rest: String = line.chars().skip(tag.length).collect();
                for (_, inner) in child_elements(&rest, &["li"]) {
                    self.push_html_list_item(&inner);
                }
                if rest.contains(&format!("</{name}")) {
                    self.html_lists.pop();
                }
                Some(index + 1)
            }
            "li" if !tag.closing => {
                self.flush();
                let (html, next) = collect_until_closing(lines, index, "li");
                let inner = inner_html(&html, "li").map_or(String::new(), |(_, inner)| inner);
                self.push_html_list_item(&inner);
                Some(next)
            }
            _ if tag.closing && whole_line_tag(line).is_some() => Some(index + 1),
            _ => None,
        }
    }

    fn read(&mut self, lines: &[&str]) {
        let mut index = 0;
        while index < lines.len() {
            let raw_line = lines[index];
            let line = raw_line.trim();
            if line.is_empty() {
                self.flush();
                index += 1;
                continue;
            }
            // Fenced code.
            if line.starts_with("```") || line.starts_with("~~~") {
                let fence = &line[..3];
                let mut code = Vec::new();
                index += 1;
                while index < lines.len() && !lines[index].trim_start().starts_with(fence) {
                    code.push(lines[index]);
                    index += 1;
                }
                self.push(MarkdownBlock::Code(code.join("\n")));
                index += 1;
                continue;
            }
            if line.starts_with('<')
                && let Some(next) = self.html_block(lines, index)
            {
                index = next;
                continue;
            }
            // Setext headings: a paragraph line underlined with === or ---.
            if !self.paragraph.is_empty()
                && self.list_item.is_none()
                && (line.chars().all(|character| character == '=') || line.chars().all(|character| character == '-'))
            {
                let level = if line.starts_with('=') { 1 } else { 2 };
                let content = parse_inline(&self.paragraph.join("\n"));
                self.paragraph.clear();
                self.push(MarkdownBlock::Heading { level, content, id: None });
                index += 1;
                continue;
            }
            if is_rule(line) {
                self.push(MarkdownBlock::Rule);
                index += 1;
                continue;
            }
            if let Some((level, text, id)) = parse_heading(line) {
                self.push(MarkdownBlock::Heading { level, content: parse_inline(text), id });
                index += 1;
                continue;
            }
            if let Some(quoted) = line.strip_prefix('>') {
                if self.quote.is_empty() {
                    self.flush();
                    self.end_list();
                }
                self.quote.push(quoted.trim_start().to_string());
                index += 1;
                continue;
            }
            if line.starts_with('|') {
                if !self.paragraph.is_empty() || self.list_item.is_some() || !self.quote.is_empty() {
                    self.flush();
                }
                self.end_list();
                if !is_table_separator(line) {
                    let cells = split_table_row(line);
                    match &mut self.table {
                        None => self.table = Some((cells, Vec::new())),
                        Some((_, rows)) => rows.push(cells),
                    }
                }
                index += 1;
                continue;
            }
            if self.table.is_some() {
                self.flush();
            }
            if let Some((alt, path)) = parse_markdown_image(line) {
                self.push(MarkdownBlock::Image { alt, path, resolved_path: None, width: None, centered: false });
                index += 1;
                continue;
            }
            if let Some((indent, marker, text)) = parse_list_item(raw_line) {
                self.flush();
                let depth = self.list_depth_for(indent);
                self.list_item = Some((depth, marker, vec![text.to_string()]));
                index += 1;
                continue;
            }
            let indent = leading_indent(raw_line);
            if let Some((_, _, item_lines)) = &mut self.list_item {
                // A lazy continuation of the item.
                item_lines.push(line.to_string());
                index += 1;
                continue;
            }
            if self.paragraph.is_empty() && self.after_list_item && indent >= 2 {
                // An indented paragraph after a blank line continues the list item above.
                let depth = self.last_list_depth();
                let mut item_lines = vec![line.to_string()];
                index += 1;
                while index < lines.len() && !lines[index].trim().is_empty() {
                    item_lines.push(lines[index].trim().to_string());
                    index += 1;
                }
                self.list_item = Some((depth, ListMarker::Continuation, item_lines));
                continue;
            }
            if !self.quote.is_empty() {
                self.quote.push(line.to_string());
                index += 1;
                continue;
            }
            if self.paragraph.is_empty() {
                self.end_list();
            }
            // Keep the raw line ending so two trailing spaces still make a line break.
            self.paragraph.push(raw_line.trim_start().to_string());
            index += 1;
        }
    }

    fn finish(mut self) -> Vec<MarkdownBlock> {
        self.flush();
        self.blocks
    }
}

/// Parses a Markdown document (with common HTML) into blocks.
pub fn parse_markdown(text: &str) -> Vec<MarkdownBlock> {
    let text = strip_comments(&text.replace("\r\n", "\n").replace('\r', "\n"));
    let lines: Vec<&str> = text.split('\n').collect();
    let mut parser = BlockParser::new();
    parser.read(&lines);
    let mut blocks = parser.finish();
    for block in &mut blocks {
        if let MarkdownBlock::Image { alt, .. } = block {
            *alt = decode_entities(alt);
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::markdown::inline::plain_text;

    fn texts(blocks: &[MarkdownBlock]) -> Vec<String> {
        blocks
            .iter()
            .map(|block| match block {
                MarkdownBlock::Heading { level, content, id } => {
                    format!(
                        "h{level}:{}{}",
                        plain_text(content),
                        id.as_ref().map_or(String::new(), |id| format!("#{id}"))
                    )
                }
                MarkdownBlock::Paragraph { content, centered } => {
                    format!("p{}:{}", if *centered { "c" } else { "" }, plain_text(content))
                }
                MarkdownBlock::ListItem { depth, marker, content } => {
                    let marker = match marker {
                        ListMarker::Bullet => "-".to_string(),
                        ListMarker::Number(number) => format!("{number}."),
                        ListMarker::Continuation => "+".to_string(),
                    };
                    format!("li{depth}{marker}:{}", plain_text(content))
                }
                MarkdownBlock::Quote(content) => format!("q:{}", plain_text(content)),
                MarkdownBlock::Rule => "hr".to_string(),
                MarkdownBlock::Code(code) => format!("code:{code}"),
                MarkdownBlock::Table { header, rows } => format!(
                    "table:{}|{}",
                    header.iter().map(|cell| plain_text(cell)).collect::<Vec<_>>().join(","),
                    rows.iter()
                        .map(|row| row.iter().map(|cell| plain_text(cell)).collect::<Vec<_>>().join(","))
                        .collect::<Vec<_>>()
                        .join(";")
                ),
                MarkdownBlock::Image { alt, path, width, .. } => format!("img:{alt}:{path}:{width:?}"),
            })
            .collect()
    }

    #[test]
    fn headings_paragraphs_rules_quotes_and_code() {
        let text = "# Title\r\n\nSome **bold** text\nwrapped `code`.\n\n## Section {#sec}\n### Closed ###\n##### Deep\n#nospace\n\n---\n> quoted\n> more\n\n```\nlet x = 1;\n```\nSetext\n===\n<!-- hidden -->\n<!-- -->";
        assert_eq!(
            texts(&parse_markdown(text)),
            [
                "h1:Title",
                "p:Some bold text wrapped code.",
                "h2:Section#sec",
                "h3:Closed",
                "h5:Deep",
                "p:#nospace",
                "hr",
                "q:quoted more",
                "code:let x = 1;",
                "h1:Setext",
            ]
        );
    }

    #[test]
    fn nested_and_numbered_lists_with_continuations() {
        let text = "- one\n  - nested\n    more of it\n- two\n\n1.  First phase\n2) Second\n\n    *(RB p50)*\n\nAfter";
        assert_eq!(
            texts(&parse_markdown(text)),
            [
                "li0-:one",
                "li1-:nested more of it",
                "li0-:two",
                "li01.:First phase",
                "li02.:Second",
                "li0+:(RB p50)",
                "p:After"
            ]
        );
    }

    #[test]
    fn pipe_tables_and_images() {
        let text = "| Key | Action |\n|---|:--|\n| **A** | Do \\| it |\n\n![Alt](img/a.png)\n<img src=\"b.png\" width=\"120\" alt=\"B\">\n<p align=\"center\"><img src=\"c.png\"></p>";
        assert_eq!(
            texts(&parse_markdown(text)),
            ["table:Key,Action|A,Do | it", "img:Alt:img/a.png:None", "img:B:b.png:Some(120.0)", "img::c.png:None"]
        );
    }

    #[test]
    fn html_tables_from_the_guardians_rulebook() {
        let text = "<table>\n<thead>\n<tr>\n<th>Edition name</th>\n<th>Serial</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td><strong><a href=\"#terrainshield\">Terrain,<br />\nShield</a></strong></td>\n<td>1st basic edition</td>\n</tr>\n</tbody>\n</table>\nAfter";
        let blocks = parse_markdown(text);
        assert_eq!(texts(&blocks), ["table:Edition name,Serial|Terrain, Shield,1st basic edition", "p:After"]);
        let MarkdownBlock::Table { rows, .. } = &blocks[0] else { panic!("a table") };
        assert!(matches!(
            &rows[0][0][0],
            InlineItem::Text { style, link: Some(link), .. } if style.bold && link == "#terrainshield"
        ));
        assert_eq!(rows[0][0][1], InlineItem::LineBreak);
    }

    #[test]
    fn html_blocks_headings_lists_and_paragraphs() {
        let text = "<h2 id=\"rules\">The <em>Rules</em></h2>\n<p align=\"center\">Centred <b>text</b></p>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n<ol start=\"3\">\n<li>c</li>\n</ol>\n<details>\n<summary>More</summary>\nHidden text\n</details>\n<hr />";
        assert_eq!(
            texts(&parse_markdown(text)),
            ["h2:The Rules#rules", "pc:Centred text", "li0-:a", "li0-:b", "li03.:c", "p:More", "p:Hidden text", "hr"]
        );
    }
}
