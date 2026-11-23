//! Reading the bits of HTML that Markdown documents commonly use: tags with their attributes,
//! character entities, and `<!-- comments -->` (which are dropped).

/// One HTML tag, e.g. `<a href="#faq">`, `</strong>` or `<br />`.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlTag {
    /// Lower-cased, e.g. `"a"`, `"td"`.
    pub name: String,
    /// `</name>`.
    pub closing: bool,
    /// `<name ... />`.
    pub self_closing: bool,
    /// Lower-cased attribute names with their (entity-decoded) values; a bare attribute has "".
    pub attributes: Vec<(String, String)>,
    /// How many characters the tag takes, from its `<` to its `>`.
    pub length: usize,
}

impl HtmlTag {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(attribute, _)| attribute == name).map(|(_, value)| value.as_str())
    }

    /// `align="center"`, or a `<center>` tag.
    pub fn is_centered(&self) -> bool {
        self.name == "center" || self.attribute("align").is_some_and(|align| align.eq_ignore_ascii_case("center"))
    }
}

/// Reads the tag starting at `chars[start]` (which must be `<`), or `None` when the text there
/// isn't a tag (`a < b`).
pub fn parse_tag(chars: &[char], start: usize) -> Option<HtmlTag> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let mut index = start + 1;
    let closing = chars.get(index) == Some(&'/');
    if closing {
        index += 1;
    }
    let name_start = index;
    while index < chars.len() && (chars[index].is_ascii_alphanumeric() || chars[index] == '-') {
        index += 1;
    }
    if index == name_start || !chars[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name: String = chars[name_start..index].iter().collect::<String>().to_ascii_lowercase();
    let mut attributes = Vec::new();
    let mut self_closing = false;
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        match chars.get(index) {
            None => return None,
            Some('>') => break,
            Some('/') if chars.get(index + 1) == Some(&'>') => {
                self_closing = true;
                index += 1;
                break;
            }
            _ => {}
        }
        let attribute_start = index;
        while index < chars.len() && !chars[index].is_whitespace() && !matches!(chars[index], '=' | '>' | '/') {
            index += 1;
        }
        if index == attribute_start {
            // A stray character (e.g. a lone '/'): skip it.
            index += 1;
            continue;
        }
        let attribute_name: String = chars[attribute_start..index].iter().collect::<String>().to_ascii_lowercase();
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if chars.get(index) == Some(&'=') {
            index += 1;
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            match chars.get(index) {
                Some(&quote) if quote == '"' || quote == '\'' => {
                    index += 1;
                    let value_start = index;
                    while index < chars.len() && chars[index] != quote {
                        index += 1;
                    }
                    if index >= chars.len() {
                        return None;
                    }
                    value = chars[value_start..index].iter().collect();
                    index += 1;
                }
                _ => {
                    let value_start = index;
                    while index < chars.len() && !chars[index].is_whitespace() && chars[index] != '>' {
                        index += 1;
                    }
                    value = chars[value_start..index].iter().collect();
                }
            }
        }
        attributes.push((attribute_name, decode_entities(&value)));
    }
    Some(HtmlTag { name, closing, self_closing, attributes, length: index + 1 - start })
}

/// The tag filling a whole (trimmed) line, e.g. `<hr />` or `</table>`.
pub fn whole_line_tag(line: &str) -> Option<HtmlTag> {
    let chars: Vec<char> = line.trim().chars().collect();
    parse_tag(&chars, 0).filter(|tag| tag.length == chars.len())
}

/// The first tag in a line, when the line starts with one.
pub fn leading_tag(line: &str) -> Option<HtmlTag> {
    let chars: Vec<char> = line.trim_start().chars().collect();
    parse_tag(&chars, 0)
}

/// Decodes the entity starting at `chars[start]` (which must be `&`): its text and how many
/// characters it takes, or `None` when it isn't an entity this knows.
pub fn decode_entity(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'&') {
        return None;
    }
    let end = (start + 1..chars.len().min(start + 12)).find(|&index| chars[index] == ';')?;
    let name: String = chars[start + 1..end].iter().collect();
    let text = if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        char::from_u32(code)?.to_string()
    } else {
        let character = match name.as_str() {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            "mdash" => '—',
            "ndash" => '–',
            "hellip" => '…',
            "copy" => '©',
            "reg" => '®',
            "trade" => '™',
            "times" => '×',
            "divide" => '÷',
            "middot" => '·',
            "bull" => '•',
            "deg" => '°',
            "laquo" => '«',
            "raquo" => '»',
            "lsquo" => '‘',
            "rsquo" => '’',
            "ldquo" => '“',
            "rdquo" => '”',
            "larr" => '←',
            "rarr" => '→',
            "uarr" => '↑',
            "darr" => '↓',
            "frac12" => '½',
            "frac14" => '¼',
            "frac34" => '¾',
            "plusmn" => '±',
            _ => return None,
        };
        character.to_string()
    };
    Some((text, end + 1 - start))
}

/// Every entity in `text` decoded (unknown ones are kept as written).
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut decoded = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if let Some((entity, length)) = decode_entity(&chars, index) {
            decoded.push_str(&entity);
            index += length;
        } else {
            decoded.push(chars[index]);
            index += 1;
        }
    }
    decoded
}

/// `text` without its `<!-- comments -->` (an unclosed comment runs to the end).
pub fn strip_comments(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return kept,
        }
    }
    kept.push_str(rest);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(text: &str) -> Option<HtmlTag> {
        let chars: Vec<char> = text.chars().collect();
        parse_tag(&chars, 0)
    }

    #[test]
    fn tags_read_their_name_attributes_and_length() {
        let link = tag("<a href=\"#faq\" class='x'>FAQ</a>").unwrap();
        assert_eq!(link.name, "a");
        assert_eq!(link.attribute("href"), Some("#faq"));
        assert_eq!(link.attribute("class"), Some("x"));
        assert_eq!(link.length, "<a href=\"#faq\" class='x'>".len());
        let line_break = tag("<BR />").unwrap();
        assert!(line_break.self_closing && line_break.name == "br");
        assert!(tag("</strong>").unwrap().closing);
        let image = tag("<img src=images/a.png width=120 alt=\"A &amp; B\">").unwrap();
        assert_eq!(image.attribute("width"), Some("120"));
        assert_eq!(image.attribute("alt"), Some("A & B"));
        assert!(tag("<p align=\"center\">").unwrap().is_centered());
        assert_eq!(tag("< b"), None);
        assert_eq!(tag("<3"), None);
        assert_eq!(tag("<a href=\"unclosed"), None);
    }

    #[test]
    fn entities_decode_named_and_numeric_forms() {
        assert_eq!(
            decode_entities("Tom &amp; Jerry &lt;3 &#8212; &#x41;&nbsp;&bogus;"),
            "Tom & Jerry <3 — A\u{a0}&bogus;"
        );
    }

    #[test]
    fn comments_are_dropped() {
        assert_eq!(strip_comments("a<!-- hidden -->b\n<!-- -->\nc"), "ab\n\nc");
        assert_eq!(strip_comments("a<!-- never closed"), "a");
    }
}
