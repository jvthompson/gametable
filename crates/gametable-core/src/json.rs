//! Byte-exact port of CardTable's `lib/json.lua`.
//!
//! Every file CardTable writes (`gamedef.json`, saves, decks, `settings.json`, asset manifests) and
//! every network line goes through this module, so its output must match the Lua encoder byte for
//! byte. The rules it reproduces:
//!
//! * Objects keep key insertion order (`json.object()` in Lua). Callers that need the Lua behaviour
//!   for plain tables (sorted keys) call [`JsonObject::sort_keys`].
//! * Integral numbers below 2^53 are written like C's `%d`, so ARGB colours such as `4294967295`
//!   stay plain integers. Other numbers use the shortest of `%.15g`, `%.16g`, `%.17g` that parses
//!   back to the same value.
//! * Strings escape `"`, `\`, `\b`, `\f`, `\n`, `\r`, `\t`; every other control character
//!   (including 0x7F) becomes `\u00xx` in lowercase hex. `/` and non-ASCII text are written raw.
//! * Indented output matches Dart's `JsonEncoder.withIndent('  ')`: `"key": value`, empty `[]`/`{}`
//!   stay inline, `\n` line endings and no trailing newline.
//! * Decoding strips a UTF-8 byte-order mark, drops `null` values inside objects (the key becomes
//!   absent), rejects nesting deeper than 64 levels, combines surrogate-pair escapes, and accepts
//!   the same lenient number syntax as the Lua pattern `-?%d+%.?%d*[eE]?[-+]?%d*`.

use std::fmt;

/// Deepest nesting of arrays/objects the decoder accepts (Lua `MAX_DEPTH`).
pub const MAXIMUM_DECODE_NESTING_DEPTH: usize = 64;

/// Indentation used for every human-readable file CardTable writes.
const INDENT_UNIT: &str = "  ";

/// A decoded or to-be-encoded JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    /// Lua numbers are IEEE doubles, so every JSON number is an `f64`.
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(JsonObject),
}

/// A JSON object that remembers the order its keys were first inserted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JsonObject {
    entries_in_insertion_order: Vec<(String, JsonValue)>,
}

impl JsonObject {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces a value. A replaced key keeps its original position, like Lua's
    /// ordered-object metatable (which only records a key the first time it is assigned).
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<JsonValue>) {
        let key = key.into();
        let value = value.into();
        if let Some(existing_entry) = self.entries_in_insertion_order.iter_mut().find(|(k, _)| *k == key) {
            existing_entry.1 = value;
        } else {
            self.entries_in_insertion_order.push((key, value));
        }
    }

    /// Builder-style [`insert`](Self::insert).
    pub fn with(mut self, key: impl Into<String>, value: impl Into<JsonValue>) -> Self {
        self.insert(key, value);
        self
    }

    /// Inserts the value only when it is `Some`, mirroring Lua's "a nil field is simply absent".
    pub fn insert_if_present<V: Into<JsonValue>>(&mut self, key: impl Into<String>, value: Option<V>) {
        if let Some(value) = value {
            self.insert(key, value);
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.entries_in_insertion_order.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut JsonValue> {
        self.entries_in_insertion_order.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn remove(&mut self, key: &str) -> Option<JsonValue> {
        let position = self.entries_in_insertion_order.iter().position(|(k, _)| k == key)?;
        Some(self.entries_in_insertion_order.remove(position).1)
    }

    pub fn len(&self) -> usize {
        self.entries_in_insertion_order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries_in_insertion_order.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &JsonValue)> {
        self.entries_in_insertion_order.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries_in_insertion_order.iter().map(|(k, _)| k)
    }

    /// Orders keys by byte value, which is how Lua writes a plain (non-ordered) table.
    pub fn sort_keys(&mut self) {
        self.entries_in_insertion_order.sort_by(|(a, _), (b, _)| a.cmp(b));
    }
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(number) => Some(*number),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            JsonValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&JsonObject> {
        match self {
            JsonValue::Object(object) => Some(object),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut JsonObject> {
        match self {
            JsonValue::Object(object) => Some(object),
            _ => None,
        }
    }

    /// Looks up a key when this value is an object.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object().and_then(|object| object.get(key))
    }

    /// True only for the boolean `true`, matching the Lua idiom `t.x == true`.
    pub fn is_exactly_true(&self) -> bool {
        matches!(self, JsonValue::Bool(true))
    }
}

impl From<bool> for JsonValue {
    fn from(flag: bool) -> Self {
        JsonValue::Bool(flag)
    }
}
impl From<f64> for JsonValue {
    fn from(number: f64) -> Self {
        JsonValue::Number(number)
    }
}
impl From<i64> for JsonValue {
    fn from(number: i64) -> Self {
        JsonValue::Number(number as f64)
    }
}
impl From<i32> for JsonValue {
    fn from(number: i32) -> Self {
        JsonValue::Number(number as f64)
    }
}
impl From<u32> for JsonValue {
    fn from(number: u32) -> Self {
        JsonValue::Number(number as f64)
    }
}
impl From<usize> for JsonValue {
    fn from(number: usize) -> Self {
        JsonValue::Number(number as f64)
    }
}
impl From<&str> for JsonValue {
    fn from(text: &str) -> Self {
        JsonValue::String(text.to_string())
    }
}
impl From<String> for JsonValue {
    fn from(text: String) -> Self {
        JsonValue::String(text)
    }
}
impl From<&String> for JsonValue {
    fn from(text: &String) -> Self {
        JsonValue::String(text.clone())
    }
}
impl From<JsonObject> for JsonValue {
    fn from(object: JsonObject) -> Self {
        JsonValue::Object(object)
    }
}
impl<T: Into<JsonValue>> From<Vec<T>> for JsonValue {
    fn from(items: Vec<T>) -> Self {
        JsonValue::Array(items.into_iter().map(Into::into).collect())
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// The only way encoding can fail: Lua refuses NaN and infinities.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonEncodeError;

impl fmt::Display for JsonEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("cannot encode non-finite number")
    }
}

impl std::error::Error for JsonEncodeError {}

/// Encodes without any whitespace (decks, manifests, network lines).
pub fn encode_compact(value: &JsonValue) -> Result<String, JsonEncodeError> {
    let mut output = String::new();
    encode_value_into(value, None, 0, &mut output)?;
    Ok(output)
}

/// Encodes with Dart-style two-space indentation (`gamedef.json`, saves, `settings.json`).
pub fn encode_indented(value: &JsonValue) -> Result<String, JsonEncodeError> {
    let mut output = String::new();
    encode_value_into(value, Some(INDENT_UNIT), 0, &mut output)?;
    Ok(output)
}

fn encode_value_into(
    value: &JsonValue,
    indent_unit: Option<&str>,
    depth: usize,
    output: &mut String,
) -> Result<(), JsonEncodeError> {
    match value {
        JsonValue::Null => output.push_str("null"),
        JsonValue::Bool(flag) => output.push_str(if *flag { "true" } else { "false" }),
        JsonValue::Number(number) => output.push_str(&format_number_like_lua(*number)?),
        JsonValue::String(text) => encode_string_into(text, output),
        JsonValue::Array(items) => {
            if items.is_empty() {
                output.push_str("[]");
                return Ok(());
            }
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                push_newline_and_padding(indent_unit, depth + 1, output);
                encode_value_into(item, indent_unit, depth + 1, output)?;
                if index + 1 < items.len() {
                    output.push(',');
                }
            }
            push_newline_and_padding(indent_unit, depth, output);
            output.push(']');
        }
        JsonValue::Object(object) => {
            if object.is_empty() {
                output.push_str("{}");
                return Ok(());
            }
            output.push('{');
            let key_value_separator = if indent_unit.is_some() { ": " } else { ":" };
            for (index, (key, item)) in object.iter().enumerate() {
                push_newline_and_padding(indent_unit, depth + 1, output);
                encode_string_into(key, output);
                output.push_str(key_value_separator);
                encode_value_into(item, indent_unit, depth + 1, output)?;
                if index + 1 < object.len() {
                    output.push(',');
                }
            }
            push_newline_and_padding(indent_unit, depth, output);
            output.push('}');
        }
    }
    Ok(())
}

fn push_newline_and_padding(indent_unit: Option<&str>, depth: usize, output: &mut String) {
    if let Some(indent_unit) = indent_unit {
        output.push('\n');
        for _ in 0..depth {
            output.push_str(indent_unit);
        }
    }
}

fn encode_string_into(text: &str, output: &mut String) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            // Lua's `%c` class is C `iscntrl`: 0x00-0x1F and 0x7F.
            control if (control as u32) < 0x20 || control as u32 == 0x7F => {
                output.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

/// Formats a number exactly as `encodeNumber` in `lib/json.lua` does.
pub fn format_number_like_lua(number: f64) -> Result<String, JsonEncodeError> {
    if !number.is_finite() {
        return Err(JsonEncodeError);
    }
    const TWO_TO_THE_53: f64 = 9_007_199_254_740_992.0;
    if number.fract() == 0.0 && number.abs() < TWO_TO_THE_53 {
        // `%d` of -0.0 prints "0".
        return Ok(format!("{}", number as i64));
    }
    for significant_digits in 15..=17 {
        let candidate = format_like_c_percent_g(number, significant_digits);
        if candidate.parse::<f64>().ok() == Some(number) {
            return Ok(candidate);
        }
    }
    Ok(format_like_c_percent_g(number, 17))
}

/// Reproduces C's `printf("%.<precision>g", number)` for a finite number.
pub fn format_like_c_percent_g(number: f64, precision: usize) -> String {
    let precision = precision.max(1);
    if number == 0.0 {
        return if number.is_sign_negative() { "-0".to_string() } else { "0".to_string() };
    }
    // Rust's `{:e}` rounds correctly, which gives us C's decimal exponent after rounding.
    let scientific = format!("{:.*e}", precision - 1, number);
    let (mantissa, exponent_text) = scientific.split_once('e').expect("scientific format has an exponent");
    let decimal_exponent: i32 = exponent_text.parse().expect("exponent is an integer");

    if decimal_exponent < -4 || decimal_exponent >= precision as i32 {
        let trimmed_mantissa = strip_trailing_fraction_zeros(mantissa);
        let exponent_sign = if decimal_exponent < 0 { '-' } else { '+' };
        format!("{trimmed_mantissa}e{exponent_sign}{:02}", decimal_exponent.abs())
    } else {
        let digits_after_point = (precision as i32 - 1 - decimal_exponent).max(0) as usize;
        let fixed = format!("{:.*}", digits_after_point, number);
        strip_trailing_fraction_zeros(&fixed).to_string()
    }
}

fn strip_trailing_fraction_zeros(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// A decode failure, formatted like the Lua error: `JSON error at line L (pos P): message`.
/// `byte_position` is 1-based, as in Lua.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonDecodeError {
    pub line: usize,
    pub byte_position: usize,
    pub message: String,
}

impl fmt::Display for JsonDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "JSON error at line {} (pos {}): {}",
            self.line, self.byte_position, self.message
        )
    }
}

impl std::error::Error for JsonDecodeError {}

/// Decodes raw file or socket bytes. Invalid UTF-8 is replaced with U+FFFD (Lua would pass the
/// bytes through untouched; Rust strings must be valid UTF-8).
pub fn decode_bytes(bytes: &[u8]) -> Result<JsonValue, JsonDecodeError> {
    decode(&String::from_utf8_lossy(bytes))
}

/// Decodes a JSON document. `null` inside an object is dropped; a top-level or array `null`
/// decodes to [`JsonValue::Null`].
pub fn decode(text: &str) -> Result<JsonValue, JsonDecodeError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut decoder = Decoder { source: text.as_bytes(), position: 0 };
    let value = decoder.decode_value(1)?;
    decoder.skip_whitespace();
    if decoder.position < decoder.source.len() {
        return Err(decoder.error_at(decoder.position, "trailing garbage"));
    }
    Ok(value)
}

struct Decoder<'source> {
    source: &'source [u8],
    /// 0-based byte offset of the next unread byte.
    position: usize,
}

impl Decoder<'_> {
    fn error_at(&self, zero_based_position: usize, message: impl Into<String>) -> JsonDecodeError {
        // Lua counts newlines in str:sub(1, pos), i.e. up to and including the 1-based position.
        let inclusive_end = (zero_based_position + 1).min(self.source.len());
        let newline_count = self.source[..inclusive_end].iter().filter(|&&b| b == b'\n').count();
        JsonDecodeError {
            line: newline_count + 1,
            byte_position: zero_based_position + 1,
            message: message.into(),
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(&byte) = self.source.get(self.position) {
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                self.position += 1;
            } else {
                break;
            }
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.source.get(self.position).copied()
    }

    fn remaining_starts_with(&self, literal: &[u8]) -> bool {
        self.source[self.position..].starts_with(literal)
    }

    fn decode_value(&mut self, depth: usize) -> Result<JsonValue, JsonDecodeError> {
        self.skip_whitespace();
        match self.peek_byte() {
            Some(b'{') => self.decode_object(depth),
            Some(b'[') => self.decode_array(depth),
            Some(b'"') => Ok(JsonValue::String(self.decode_string()?)),
            _ if self.remaining_starts_with(b"true") => {
                self.position += 4;
                Ok(JsonValue::Bool(true))
            }
            _ if self.remaining_starts_with(b"false") => {
                self.position += 5;
                Ok(JsonValue::Bool(false))
            }
            _ if self.remaining_starts_with(b"null") => {
                self.position += 4;
                Ok(JsonValue::Null)
            }
            _ => self.decode_number(),
        }
    }

    fn decode_object(&mut self, depth: usize) -> Result<JsonValue, JsonDecodeError> {
        if depth > MAXIMUM_DECODE_NESTING_DEPTH {
            return Err(self.error_at(self.position, "too deeply nested"));
        }
        let mut object = JsonObject::new();
        self.position += 1;
        self.skip_whitespace();
        if self.peek_byte() == Some(b'}') {
            self.position += 1;
            return Ok(JsonValue::Object(object));
        }
        loop {
            self.skip_whitespace();
            if self.peek_byte() != Some(b'"') {
                return Err(self.error_at(self.position, "expected key"));
            }
            let key = self.decode_string()?;
            self.skip_whitespace();
            if self.peek_byte() != Some(b':') {
                return Err(self.error_at(self.position, "expected ':'"));
            }
            self.position += 1;
            let value = self.decode_value(depth + 1)?;
            if value != JsonValue::Null {
                object.insert(key, value);
            }
            self.skip_whitespace();
            match self.peek_byte() {
                Some(b'}') => {
                    self.position += 1;
                    return Ok(JsonValue::Object(object));
                }
                Some(b',') => self.position += 1,
                _ => return Err(self.error_at(self.position, "expected ',' or '}'")),
            }
        }
    }

    fn decode_array(&mut self, depth: usize) -> Result<JsonValue, JsonDecodeError> {
        if depth > MAXIMUM_DECODE_NESTING_DEPTH {
            return Err(self.error_at(self.position, "too deeply nested"));
        }
        let mut items = Vec::new();
        self.position += 1;
        self.skip_whitespace();
        if self.peek_byte() == Some(b']') {
            self.position += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.decode_value(depth + 1)?);
            self.skip_whitespace();
            match self.peek_byte() {
                Some(b']') => {
                    self.position += 1;
                    return Ok(JsonValue::Array(items));
                }
                Some(b',') => self.position += 1,
                _ => return Err(self.error_at(self.position, "expected ',' or ']'")),
            }
        }
    }

    fn decode_string(&mut self) -> Result<String, JsonDecodeError> {
        let opening_quote_position = self.position;
        let mut decoded_bytes: Vec<u8> = Vec::new();
        let mut index = self.position + 1;
        loop {
            let Some(&byte) = self.source.get(index) else {
                return Err(self.error_at(opening_quote_position, "unterminated string"));
            };
            match byte {
                b'"' => {
                    self.position = index + 1;
                    // The source was a &str and escapes produce valid UTF-8 (lone surrogates
                    // become U+FFFD), so this cannot fail; be lenient anyway.
                    return Ok(String::from_utf8_lossy(&decoded_bytes).into_owned());
                }
                b'\\' => {
                    let escape_letter = self.source.get(index + 1).copied();
                    if escape_letter == Some(b'u') {
                        let (code_point, next_index) = self.decode_unicode_escape(index)?;
                        let character = char::from_u32(code_point).unwrap_or('\u{FFFD}');
                        let mut utf8_buffer = [0u8; 4];
                        decoded_bytes.extend_from_slice(character.encode_utf8(&mut utf8_buffer).as_bytes());
                        index = next_index;
                    } else {
                        let replacement = match escape_letter {
                            Some(b'b') => b'\x08',
                            Some(b'f') => b'\x0C',
                            Some(b'n') => b'\n',
                            Some(b'r') => b'\r',
                            Some(b't') => b'\t',
                            Some(b'"') => b'"',
                            Some(b'\\') => b'\\',
                            Some(b'/') => b'/',
                            _ => return Err(self.error_at(index, "bad escape")),
                        };
                        decoded_bytes.push(replacement);
                        index += 2;
                    }
                }
                other => {
                    decoded_bytes.push(other);
                    index += 1;
                }
            }
        }
    }

    /// Decodes `\uXXXX` starting at the backslash, combining a following low surrogate.
    /// Returns the code point and the index just past the escape(s).
    fn decode_unicode_escape(&self, backslash_index: usize) -> Result<(u32, usize), JsonDecodeError> {
        let parse_four_hex_digits = |start: usize| -> Option<u32> {
            let digits = self.source.get(start..start + 4)?;
            let digits = std::str::from_utf8(digits).ok()?;
            u32::from_str_radix(digits, 16).ok()
        };
        let Some(mut code_point) = parse_four_hex_digits(backslash_index + 2) else {
            return Err(self.error_at(backslash_index, "bad unicode escape"));
        };
        let mut next_index = backslash_index + 6;
        let is_high_surrogate = (0xD800..=0xDBFF).contains(&code_point);
        if is_high_surrogate && self.source.get(next_index..next_index + 2) == Some(b"\\u") {
            if let Some(low_surrogate) = parse_four_hex_digits(next_index + 2)
                && (0xDC00..=0xDFFF).contains(&low_surrogate)
            {
                code_point = 0x10000 + (code_point - 0xD800) * 0x400 + (low_surrogate - 0xDC00);
                next_index += 6;
            }
        }
        Ok((code_point, next_index))
    }

    /// Matches the Lua pattern `^-?%d+%.?%d*[eE]?[-+]?%d*`, then parses it like `tonumber`.
    fn decode_number(&mut self) -> Result<JsonValue, JsonDecodeError> {
        let start = self.position;
        let mut end = start;
        let byte_at = |index: usize| self.source.get(index).copied();
        if byte_at(end) == Some(b'-') {
            end += 1;
        }
        let integer_digits_start = end;
        while byte_at(end).is_some_and(|b| b.is_ascii_digit()) {
            end += 1;
        }
        if end == integer_digits_start {
            let unexpected = self.source[start..].iter().next().map(|&b| b as char).unwrap_or(' ');
            let shown = if start >= self.source.len() { String::new() } else { unexpected.to_string() };
            return Err(self.error_at(start, format!("unexpected character '{shown}'")));
        }
        if byte_at(end) == Some(b'.') {
            end += 1;
        }
        while byte_at(end).is_some_and(|b| b.is_ascii_digit()) {
            end += 1;
        }
        if matches!(byte_at(end), Some(b'e' | b'E')) {
            end += 1;
        }
        if matches!(byte_at(end), Some(b'-' | b'+')) {
            end += 1;
        }
        while byte_at(end).is_some_and(|b| b.is_ascii_digit()) {
            end += 1;
        }
        let number_text = std::str::from_utf8(&self.source[start..end]).expect("ASCII digits");
        match parse_number_like_lua_tonumber(number_text) {
            Some(number) => {
                self.position = end;
                Ok(JsonValue::Number(number))
            }
            None => Err(self.error_at(start, "bad number")),
        }
    }
}

/// `tonumber` on text already restricted to the decoder's number pattern.
fn parse_number_like_lua_tonumber(text: &str) -> Option<f64> {
    // Rust's float parser accepts everything Lua's does for this restricted alphabet
    // ("01", "1.", "1.e5") and rejects the same malformed forms ("1e", "1+5", "15-").
    text.parse::<f64>().ok().filter(|number| number.is_finite() || text.contains(['e', 'E']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact(value: &JsonValue) -> String {
        encode_compact(value).unwrap()
    }

    #[test]
    fn round_trips_ordinary_nested_data_and_drops_null_inside_objects() {
        let text = r#"{"a":1,"b":["x","y",{"c":true},null],"e":"hé","gone":null}"#;
        let value = decode(text).unwrap();
        assert_eq!(compact(&value), r#"{"a":1,"b":["x","y",{"c":true},null],"e":"hé"}"#);
    }

    #[test]
    fn rejects_deeply_nested_arrays_and_objects() {
        let deep_array = format!("{}1{}", "[".repeat(1000), "]".repeat(1000));
        assert!(decode(&deep_array).is_err());
        let deep_object = format!("{}1{}", r#"{"a":"#.repeat(1000), "}".repeat(1000));
        assert!(decode(&deep_object).is_err());
    }

    #[test]
    fn nesting_limit_is_exactly_sixty_four_levels() {
        let at_limit = format!("{}1{}", "[".repeat(64), "]".repeat(64));
        assert!(decode(&at_limit).is_ok());
        let over_limit = format!("{}1{}", "[".repeat(65), "]".repeat(65));
        assert_eq!(decode(&over_limit).unwrap_err().message, "too deeply nested");
    }

    #[test]
    fn keeps_insertion_order_and_first_position_for_duplicate_keys() {
        let value = decode(r#"{"z":1,"a":2,"z":3}"#).unwrap();
        assert_eq!(compact(&value), r#"{"z":3,"a":2}"#);
    }

    #[test]
    fn sort_keys_orders_by_bytes_like_lua_plain_tables() {
        let mut object = JsonObject::new().with("b", 1).with("B", 2).with("a", 3);
        object.sort_keys();
        assert_eq!(compact(&object.into()), r#"{"B":2,"a":3,"b":1}"#);
    }

    #[test]
    fn formats_numbers_like_lua() {
        // Golden values captured from the original lib/json.lua running under LÖVE 11.5 (LuaJIT).
        let cases: &[(f64, &str)] = &[
            (4294967295.0, "4294967295"),
            (1.0, "1"),
            (-0.0, "0"),
            (-12.0, "-12"),
            (0.1, "0.1"),
            (0.5, "0.5"),
            (1.0 / 3.0, "0.3333333333333333"),
            (2.0 / 3.0, "0.6666666666666666"),
            (1e20, "1e+20"),
            (1.5e-7, "1.5e-07"),
            (0.0001, "0.0001"),
            (123456.789, "123456.789"),
            (9007199254740992.0, "9007199254740992"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e-300, "1e-300"),
            (5e-324, "4.94065645841247e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (100.25, "100.25"),
            (1e15 + 0.5, "1000000000000000.5"),
        ];
        for (number, expected_text) in cases {
            assert_eq!(format_number_like_lua(*number).unwrap(), *expected_text, "formatting {number}");
        }
        assert!(format_number_like_lua(f64::NAN).is_err());
        assert!(format_number_like_lua(f64::INFINITY).is_err());
    }

    #[test]
    fn escapes_strings_like_lua() {
        let value = JsonValue::from("quote\" back\\ \u{8}\u{c}\n\r\t \u{1}\u{1f}\u{7f} /é♠");
        assert_eq!(
            compact(&value),
            "\"quote\\\" back\\\\ \\b\\f\\n\\r\\t \\u0001\\u001f\\u007f /é♠\""
        );
    }

    #[test]
    fn indents_like_dart_json_encoder() {
        let value = JsonValue::from(
            JsonObject::new()
                .with("a", vec![1, 2])
                .with("b", JsonObject::new())
                .with("c", Vec::<JsonValue>::new())
                .with("d", JsonObject::new().with("e", "f")),
        );
        let expected = "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"c\": [],\n  \"d\": {\n    \"e\": \"f\"\n  }\n}";
        assert_eq!(encode_indented(&value).unwrap(), expected);
    }

    #[test]
    fn decodes_escapes_surrogate_pairs_and_byte_order_mark() {
        let value = decode("\u{FEFF}[\"\\u00e9\\ud83c\\udca1\\/\\n\"]").unwrap();
        assert_eq!(value, JsonValue::Array(vec![JsonValue::from("é🂡/\n")]));
        let lone_surrogate = decode(r#""\ud800x""#).unwrap();
        assert_eq!(lone_surrogate, JsonValue::from("\u{FFFD}x"));
    }

    #[test]
    fn accepts_lenient_numbers_and_rejects_malformed_ones() {
        assert_eq!(decode("01").unwrap(), JsonValue::Number(1.0));
        assert_eq!(decode("1.").unwrap(), JsonValue::Number(1.0));
        assert_eq!(decode("1.e2").unwrap(), JsonValue::Number(100.0));
        assert_eq!(decode("-2.5E-1").unwrap(), JsonValue::Number(-0.25));
        assert_eq!(decode("1e").unwrap_err().message, "bad number");
        assert_eq!(decode("1+5").unwrap_err().message, "bad number");
        assert_eq!(decode("-").unwrap_err().message, "unexpected character '-'");
        assert_eq!(decode("1 2").unwrap_err().message, "trailing garbage");
    }

    #[test]
    fn reports_line_and_position_like_lua() {
        let error = decode("{\n  \"a\": x\n}").unwrap_err();
        assert_eq!(error.to_string(), "JSON error at line 2 (pos 10): unexpected character 'x'");
    }
}
