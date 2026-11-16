//! Reading request fields and building response objects the way the Go server does.
//!
//! Go decodes each request into a struct: a missing field, or one of the wrong type, is left at
//! its zero value. [`Fields`] gives the same leniency. Go encodes `map[string]any` payloads with
//! their keys sorted, so [`sorted_object`] does the same; struct payloads (chat lines, settings)
//! keep their declared field order and are built directly.

use gametable_core::json::{JsonObject, JsonValue};

/// Lenient typed access to a request payload.
#[derive(Clone, Copy)]
pub struct Fields<'a>(pub &'a JsonValue);

impl<'a> Fields<'a> {
    pub fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some()
    }

    pub fn raw(&self, key: &str) -> Option<&'a JsonValue> {
        self.0.get(key)
    }

    /// A string field, `""` when absent or not a string.
    pub fn text(&self, key: &str) -> String {
        self.optional_text(key).unwrap_or_default()
    }

    /// A string field, `None` when absent or not a string (Go's `*string`).
    pub fn optional_text(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(JsonValue::as_str).map(str::to_string)
    }

    /// A whole-number field, `0` when absent, fractional or not a number.
    pub fn integer(&self, key: &str) -> i64 {
        self.optional_integer(key).unwrap_or(0)
    }

    /// A whole-number field (Go's `*int64`).
    pub fn optional_integer(&self, key: &str) -> Option<i64> {
        self.0
            .get(key)
            .and_then(JsonValue::as_f64)
            .filter(|number| number.is_finite() && number.fract() == 0.0)
            .map(|number| number as i64)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(JsonValue::as_bool).unwrap_or(false)
    }

    pub fn optional_flag(&self, key: &str) -> Option<bool> {
        self.0.get(key).and_then(JsonValue::as_bool)
    }

    /// A list of strings. Go fails the whole field when any item is not a string.
    pub fn text_list(&self, key: &str) -> Vec<String> {
        let Some(items) = self.0.get(key).and_then(JsonValue::as_array) else { return Vec::new() };
        let texts: Option<Vec<String>> = items.iter().map(|item| item.as_str().map(str::to_string)).collect();
        texts.unwrap_or_default()
    }
}

/// An object with its keys in sorted order, as Go encodes a `map[string]any`.
pub fn sorted_object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut object = JsonObject::new();
    for (key, value) in entries {
        object.insert(key, value);
    }
    object.sort_keys();
    JsonValue::Object(object)
}

/// Sorts an object's keys (for maps built up step by step).
pub fn sorted(mut object: JsonObject) -> JsonValue {
    object.sort_keys();
    JsonValue::Object(object)
}

/// An empty object.
pub fn empty_object() -> JsonValue {
    JsonValue::Object(JsonObject::new())
}

/// A list of strings as a JSON array.
pub fn text_array<S: AsRef<str>>(items: &[S]) -> JsonValue {
    JsonValue::Array(items.iter().map(|item| JsonValue::from(item.as_ref())).collect())
}
