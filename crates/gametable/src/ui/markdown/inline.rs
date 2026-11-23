//! Inline content: the text inside a paragraph, heading, list item or table cell, as styled
//! spans. Understands Markdown emphasis (`**bold**`, `*italic*`, `` `code` ``, `~~struck~~`),
//! links (`[text](url "title")`, `<https://…>` and bare `https://…` / `www.…` addresses),
//! backslash escapes and hard line breaks, plus the inline HTML tags documents commonly use
//! (`<b> <strong> <i> <em> <u> <s> <del> <code> <kbd> <sup> <sub> <a href> <a name> <br> <img>`),
//! and character entities. Tags it doesn't know are dropped, keeping their text.

use super::html::{HtmlTag, decode_entity, parse_tag};

/// How a span of text is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub underline: bool,
    /// `<sup>`, `<sub>` and `<small>`: a smaller size.
    pub small: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InlineItem {
    /// Text in one style, part of a link when `link` is set (its target as written).
    Text { text: String, style: SpanStyle, link: Option<String> },
    /// `<br>`, two trailing spaces or a trailing backslash.
    LineBreak,
    /// `<a name="…">` or any tag's `id="…"`: a place links can jump to.
    Anchor(String),
}

pub type Inline = Vec<InlineItem>;

/// The text of inline content without its styling (line breaks become spaces).
pub fn plain_text(inline: &[InlineItem]) -> String {
    let mut text = String::new();
    for item in inline {
        match item {
            InlineItem::Text { text: span, .. } => text.push_str(span),
            InlineItem::LineBreak => text.push(' '),
            InlineItem::Anchor(_) => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether inline content shows anything (anchors alone don't).
pub fn has_visible_text(inline: &[InlineItem]) -> bool {
    inline.iter().any(|item| match item {
        InlineItem::Text { text, .. } => !text.trim().is_empty(),
        InlineItem::LineBreak => false,
        InlineItem::Anchor(_) => false,
    })
}

/// Parses inline Markdown and HTML into styled spans.
pub fn parse_inline(text: &str) -> Inline {
    let mut parser = InlineParser { chars: text.chars().collect(), ..InlineParser::default() };
    let length = parser.chars.len();
    parser.run(0, length);
    parser.trim_trailing_space();
    parser.output
}

#[derive(Default)]
struct InlineParser {
    chars: Vec<char>,
    output: Inline,
    markdown_bold: bool,
    markdown_italic: bool,
    markdown_strike: bool,
    html_bold: u32,
    html_italic: u32,
    html_strike: u32,
    html_underline: u32,
    html_code: u32,
    html_small: u32,
    /// Open `<a>` tags: their `href`, or `None` for an anchor-only `<a name>`.
    html_links: Vec<Option<String>>,
    /// The target of the `[text](url)` being read.
    markdown_link: Option<String>,
}

/// URL schemes (and `www.`) that start an address written without brackets.
const BARE_ADDRESS_STARTS: [&str; 3] = ["https://", "http://", "www."];

impl InlineParser {
    fn style(&self) -> SpanStyle {
        SpanStyle {
            bold: self.markdown_bold || self.html_bold > 0,
            italic: self.markdown_italic || self.html_italic > 0,
            code: self.html_code > 0,
            strike: self.markdown_strike || self.html_strike > 0,
            underline: self.html_underline > 0,
            small: self.html_small > 0,
        }
    }

    fn link(&self) -> Option<String> {
        self.markdown_link.clone().or_else(|| self.html_links.iter().rev().find_map(Clone::clone))
    }

    fn push_text_with(&mut self, text: &str, style: SpanStyle, link: Option<String>) {
        if text.is_empty() {
            return;
        }
        if let Some(InlineItem::Text { text: last, style: last_style, link: last_link }) = self.output.last_mut()
            && *last_style == style
            && *last_link == link
        {
            last.push_str(text);
            return;
        }
        self.output.push(InlineItem::Text { text: text.to_string(), style, link });
    }

    fn push_text(&mut self, text: &str) {
        let (style, link) = (self.style(), self.link());
        self.push_text_with(text, style, link);
    }

    fn push_char(&mut self, character: char) {
        let mut buffer = [0u8; 4];
        self.push_text(character.encode_utf8(&mut buffer));
    }

    fn trim_trailing_space(&mut self) {
        if let Some(InlineItem::Text { text, .. }) = self.output.last_mut() {
            let trimmed = text.trim_end_matches(' ').len();
            text.truncate(trimmed);
            if text.is_empty() {
                self.output.pop();
            }
        }
    }

    fn line_break(&mut self) {
        self.trim_trailing_space();
        self.output.push(InlineItem::LineBreak);
    }

    fn previous_char(&self, index: usize, start: usize) -> Option<char> {
        (index > start).then(|| self.chars[index - 1])
    }

    /// Whether `delimiter` appears again in `chars[from..end]`.
    fn has_closer(&self, from: usize, end: usize, delimiter: &[char]) -> bool {
        let mut index = from;
        while index + delimiter.len() <= end {
            if self.chars[index] == '\\' {
                index += 2;
                continue;
            }
            if self.chars[index..index + delimiter.len()] == *delimiter
                && index > from
                && !self.chars[index - 1].is_whitespace()
            {
                return true;
            }
            index += 1;
        }
        false
    }

    fn run(&mut self, start: usize, end: usize) {
        let mut index = start;
        while index < end {
            let character = self.chars[index];
            let next = self.chars.get(index + 1).copied().filter(|_| index + 1 < end);
            match character {
                '\\' => match next {
                    Some('\n') => {
                        self.line_break();
                        index += 2;
                    }
                    Some(escaped) if escaped.is_ascii_punctuation() => {
                        self.push_char(escaped);
                        index += 2;
                    }
                    _ => {
                        self.push_char('\\');
                        index += 1;
                    }
                },
                '\n' => {
                    let hard = index >= start + 2 && self.chars[index - 1] == ' ' && self.chars[index - 2] == ' ';
                    if hard {
                        self.line_break();
                    } else {
                        self.trim_trailing_space();
                        self.push_char(' ');
                    }
                    index += 1;
                    while index < end && self.chars[index] == ' ' {
                        index += 1;
                    }
                }
                '`' => index = self.code_span(index, end),
                '*' | '_' => index = self.emphasis(index, start, end),
                '~' if next == Some('~') => {
                    let delimiter = ['~', '~'];
                    let can_close = self.markdown_strike
                        && self.previous_char(index, start).is_some_and(|previous| !previous.is_whitespace());
                    let can_open = !self.markdown_strike
                        && self.chars.get(index + 2).is_some_and(|after| !after.is_whitespace())
                        && self.has_closer(index + 2, end, &delimiter);
                    if can_close || can_open {
                        self.markdown_strike = !self.markdown_strike;
                    } else {
                        self.push_text("~~");
                    }
                    index += 2;
                }
                '!' if next == Some('[') => match self.link_parts(index + 1, end) {
                    Some(parts) => {
                        // An image inside a line shows its description.
                        let alt: String = self.chars[parts.label_start..parts.label_end].iter().collect();
                        self.push_text(&alt);
                        index = parts.next;
                    }
                    None => {
                        self.push_char('!');
                        index += 1;
                    }
                },
                '[' => match self.link_parts(index, end) {
                    Some(parts) => {
                        let outer_link = self.markdown_link.replace(parts.target);
                        self.run(parts.label_start, parts.label_end);
                        self.markdown_link = outer_link;
                        index = parts.next;
                    }
                    None => {
                        self.push_char('[');
                        index += 1;
                    }
                },
                '<' => index = self.angle_bracket(index, end),
                '&' => match decode_entity(&self.chars, index) {
                    Some((text, length)) if index + length <= end => {
                        self.push_text(&text);
                        index += length;
                    }
                    _ => {
                        self.push_char('&');
                        index += 1;
                    }
                },
                _ if self.link().is_none()
                    && self
                        .previous_char(index, start)
                        .is_none_or(|previous| previous.is_whitespace() || previous == '(')
                    && self.starts_bare_address(index, end) =>
                {
                    index = self.bare_address(index, end);
                }
                _ => {
                    self.push_char(character);
                    index += 1;
                }
            }
        }
    }

    /// `` `code` `` (any run of backticks, closed by the same run); a lone run is literal.
    fn code_span(&mut self, index: usize, end: usize) -> usize {
        let run = (index..end).take_while(|&position| self.chars[position] == '`').count();
        let content_start = index + run;
        let mut position = content_start;
        while position < end {
            if self.chars[position] == '`' {
                let closing = (position..end).take_while(|&inner| self.chars[inner] == '`').count();
                if closing == run {
                    let code: String = self.chars[content_start..position].iter().collect();
                    let code = code.replace('\n', " ");
                    let trimmed = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') {
                        &code[1..code.len() - 1]
                    } else {
                        code.as_str()
                    };
                    let style = SpanStyle { code: true, ..self.style() };
                    let link = self.link();
                    self.push_text_with(trimmed, style, link);
                    return position + closing;
                }
                position += closing;
            } else {
                position += 1;
            }
        }
        let ticks: String = self.chars[index..content_start].iter().collect();
        self.push_text(&ticks);
        content_start
    }

    /// `*`/`_` runs: two toggle bold, one toggles italic, three do both. `_` only counts at a word
    /// boundary, so `snake_case_names` stay as they are.
    fn emphasis(&mut self, index: usize, start: usize, end: usize) -> usize {
        let marker = self.chars[index];
        let run = (index..end).take_while(|&position| self.chars[position] == marker).count();
        let previous = self.previous_char(index, start);
        let after = self.chars.get(index + run).copied().filter(|_| index + run < end);
        let followed_by_text = after.is_some_and(|after| !after.is_whitespace());
        let preceded_by_text = previous.is_some_and(|previous| !previous.is_whitespace());
        let (opens_word, closes_word) = if marker == '_' {
            (
                previous.is_none_or(|previous| !previous.is_alphanumeric()),
                after.is_none_or(|after| !after.is_alphanumeric()),
            )
        } else {
            (true, true)
        };
        let mut remaining = run;
        if remaining >= 2 {
            let delimiter = [marker, marker];
            let close = self.markdown_bold && preceded_by_text && closes_word;
            let open =
                !self.markdown_bold && followed_by_text && opens_word && self.has_closer(index + run, end, &delimiter);
            if close || open {
                self.markdown_bold = !self.markdown_bold;
                remaining -= 2;
            }
        }
        if remaining >= 1 {
            let delimiter = [marker];
            let close = self.markdown_italic && preceded_by_text && closes_word;
            let open = !self.markdown_italic
                && followed_by_text
                && opens_word
                && self.has_closer(index + run, end, &delimiter);
            if close || open {
                self.markdown_italic = !self.markdown_italic;
                remaining -= 1;
            }
        }
        for _ in 0..remaining {
            self.push_char(marker);
        }
        index + run
    }

    /// `<…>`: an autolink, an HTML tag, or a literal `<`.
    fn angle_bracket(&mut self, index: usize, end: usize) -> usize {
        if let Some(close) = (index + 1..end).find(|&position| self.chars[position] == '>') {
            let inside: String = self.chars[index + 1..close].iter().collect();
            let is_address = ["https://", "http://", "mailto:"].iter().any(|scheme| inside.starts_with(scheme))
                && !inside.contains(char::is_whitespace);
            if is_address {
                let style = self.style();
                self.push_text_with(inside.trim_start_matches("mailto:"), style, Some(inside.clone()));
                return close + 1;
            }
        }
        match parse_tag(&self.chars, index).filter(|tag| index + tag.length <= end) {
            Some(tag) => {
                let length = tag.length;
                self.html_tag(&tag);
                index + length
            }
            None => {
                self.push_char('<');
                index + 1
            }
        }
    }

    fn html_tag(&mut self, tag: &HtmlTag) {
        fn adjust(counter: &mut u32, closing: bool) {
            *counter = if closing { counter.saturating_sub(1) } else { *counter + 1 };
        }
        if !tag.closing
            && let Some(id) = tag.attribute("id").filter(|id| !id.is_empty())
        {
            self.output.push(InlineItem::Anchor(id.to_string()));
        }
        let closing = tag.closing;
        match tag.name.as_str() {
            "b" | "strong" => adjust(&mut self.html_bold, closing),
            "i" | "em" | "cite" | "var" | "dfn" => adjust(&mut self.html_italic, closing),
            "u" | "ins" => adjust(&mut self.html_underline, closing),
            "s" | "del" | "strike" => adjust(&mut self.html_strike, closing),
            "code" | "kbd" | "samp" | "tt" => adjust(&mut self.html_code, closing),
            "sup" | "sub" | "small" => adjust(&mut self.html_small, closing),
            "br" => self.line_break(),
            "a" if closing => {
                self.html_links.pop();
            }
            "a" => {
                if let Some(name) = tag.attribute("name").filter(|name| !name.is_empty()) {
                    self.output.push(InlineItem::Anchor(name.to_string()));
                }
                if !tag.self_closing {
                    self.html_links.push(tag.attribute("href").filter(|href| !href.is_empty()).map(str::to_string));
                }
            }
            "img" if !closing => {
                if let Some(alt) = tag.attribute("alt").filter(|alt| !alt.is_empty()) {
                    self.push_text(alt);
                }
            }
            _ => {}
        }
    }

    fn starts_bare_address(&self, index: usize, end: usize) -> bool {
        BARE_ADDRESS_STARTS.iter().any(|start| {
            let start: Vec<char> = start.chars().collect();
            index + start.len() < end
                && self.chars[index..index + start.len()].iter().zip(&start).all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
    }

    /// A bare `https://…` or `www.…` address, up to whitespace, without trailing punctuation.
    fn bare_address(&mut self, index: usize, end: usize) -> usize {
        let mut stop = index;
        while stop < end && !self.chars[stop].is_whitespace() && !matches!(self.chars[stop], '<' | '"') {
            stop += 1;
        }
        while stop > index && matches!(self.chars[stop - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')' | '\'' | '*') {
            stop -= 1;
        }
        let address: String = self.chars[index..stop].iter().collect();
        let style = self.style();
        self.push_text_with(&address, style, Some(address.clone()));
        stop
    }

    /// `[label](target "title")` starting at `chars[index]` (a `[`).
    fn link_parts(&self, index: usize, end: usize) -> Option<LinkParts> {
        let mut depth = 0;
        let mut position = index;
        let label_end = loop {
            if position >= end {
                return None;
            }
            match self.chars[position] {
                '\\' => position += 1,
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        break position;
                    }
                }
                _ => {}
            }
            position += 1;
        };
        if self.chars.get(label_end + 1) != Some(&'(') {
            return None;
        }
        let mut position = label_end + 2;
        while position < end && self.chars[position] == ' ' {
            position += 1;
        }
        let target_start;
        let target_end;
        if self.chars.get(position) == Some(&'<') {
            target_start = position + 1;
            position = (target_start..end).find(|&inner| self.chars[inner] == '>')?;
            target_end = position;
            position += 1;
        } else {
            target_start = position;
            let mut parens = 0;
            while position < end {
                match self.chars[position] {
                    '(' => parens += 1,
                    ')' if parens == 0 => break,
                    ')' => parens -= 1,
                    character if character.is_whitespace() => break,
                    _ => {}
                }
                position += 1;
            }
            target_end = position;
        }
        while position < end && self.chars[position].is_whitespace() {
            position += 1;
        }
        // An optional "title" (shown as a tooltip elsewhere; not used here).
        if let Some(&quote) = self.chars.get(position).filter(|&&quote| quote == '"' || quote == '\'') {
            position = (position + 1..end).find(|&inner| self.chars[inner] == quote)? + 1;
            while position < end && self.chars[position].is_whitespace() {
                position += 1;
            }
        }
        if position >= end || self.chars[position] != ')' {
            return None;
        }
        let target: String = self.chars[target_start..target_end].iter().collect();
        Some(LinkParts { label_start: index + 1, label_end, target, next: position + 1 })
    }
}

struct LinkParts {
    label_start: usize,
    label_end: usize,
    target: String,
    next: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str, style: SpanStyle, link: Option<&str>) -> InlineItem {
        InlineItem::Text { text: text.to_string(), style, link: link.map(str::to_string) }
    }

    const PLAIN: SpanStyle =
        SpanStyle { bold: false, italic: false, code: false, strike: false, underline: false, small: false };
    const BOLD: SpanStyle = SpanStyle { bold: true, ..PLAIN };
    const ITALIC: SpanStyle = SpanStyle { italic: true, ..PLAIN };

    #[test]
    fn emphasis_code_and_strikethrough() {
        assert_eq!(
            parse_inline("a **bold *both*** and `x * y` ~~gone~~"),
            vec![
                text("a ", PLAIN, None),
                text("bold ", BOLD, None),
                text("both", SpanStyle { bold: true, italic: true, ..PLAIN }, None),
                text(" and ", PLAIN, None),
                text("x * y", SpanStyle { code: true, ..PLAIN }, None),
                text(" ", PLAIN, None),
                text("gone", SpanStyle { strike: true, ..PLAIN }, None),
            ]
        );
        // Underscores inside words and lone markers stay as written.
        assert_eq!(parse_inline("snake_case_name 2 * 3"), vec![text("snake_case_name 2 * 3", PLAIN, None)]);
        assert_eq!(parse_inline("_lean_"), vec![text("lean", ITALIC, None)]);
        assert_eq!(parse_inline(r"\*not italic\*"), vec![text("*not italic*", PLAIN, None)]);
    }

    #[test]
    fn markdown_links_autolinks_and_bare_addresses() {
        assert_eq!(
            parse_inline("See **[(FAQ)](#faq)** or [site](https://example.com \"Title\")."),
            vec![
                text("See ", PLAIN, None),
                text("(FAQ)", BOLD, Some("#faq")),
                text(" or ", PLAIN, None),
                text("site", PLAIN, Some("https://example.com")),
                text(".", PLAIN, None),
            ]
        );
        assert_eq!(
            parse_inline("<https://a.org/x> and www.b.com."),
            vec![
                text("https://a.org/x", PLAIN, Some("https://a.org/x")),
                text(" and ", PLAIN, None),
                text("www.b.com", PLAIN, Some("www.b.com")),
                text(".", PLAIN, None),
            ]
        );
        assert_eq!(parse_inline("[broken](no close"), vec![text("[broken](no close", PLAIN, None)]);
        assert_eq!(parse_inline("![Logo](logo.png) x"), vec![text("Logo x", PLAIN, None)]);
    }

    #[test]
    fn inline_html_tags_anchors_and_entities() {
        assert_eq!(
            parse_inline("<strong><a href=\"#guardian\">Guardian</a></strong><br />next &amp; <em>last</em>"),
            vec![
                text("Guardian", BOLD, Some("#guardian")),
                InlineItem::LineBreak,
                text("next & ", PLAIN, None),
                text("last", ITALIC, None),
            ]
        );
        assert_eq!(
            parse_inline("<a name=\"q33\"></a>Q33 <span id=\"s\">x</span> a < b"),
            vec![
                InlineItem::Anchor("q33".to_string()),
                text("Q33 ", PLAIN, None),
                InlineItem::Anchor("s".to_string()),
                text("x a < b", PLAIN, None),
            ]
        );
        assert_eq!(
            parse_inline("x<sup>2</sup> <u>u</u>"),
            vec![
                text("x", PLAIN, None),
                text("2", SpanStyle { small: true, ..PLAIN }, None),
                text(" ", PLAIN, None),
                text("u", SpanStyle { underline: true, ..PLAIN }, None),
            ]
        );
    }

    #[test]
    fn line_breaks_and_joined_lines() {
        assert_eq!(
            parse_inline("one  \ntwo\nthree\\\nfour"),
            vec![
                text("one", PLAIN, None),
                InlineItem::LineBreak,
                text("two three", PLAIN, None),
                InlineItem::LineBreak,
                text("four", PLAIN, None),
            ]
        );
        assert_eq!(plain_text(&parse_inline("**1.** Preparation <em>for</em> Combat")), "1. Preparation for Combat");
    }
}
