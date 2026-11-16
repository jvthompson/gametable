//! Helpers that read model fields out of a decoded JSON object and write sparse JSON back,
//! reproducing the `requireField` / `enumValue` / `unlessDefault` helpers in `models.lua`.

use super::ModelError;
use crate::color::ArgbColor;
use crate::json::{JsonObject, JsonValue};

/// Implemented by the enums declared with `json_string_enum!`.
pub trait JsonStringEnum: Sized + Copy {
    fn from_json_str(text: &str) -> Option<Self>;
}

/// Reads the fields of one model from a JSON object.
pub struct FieldReader<'json> {
    object: &'json JsonObject,
    model_kind: &'static str,
}

/// Lua indexes any table, so a non-object model value behaves like an empty object.
static EMPTY_OBJECT: std::sync::LazyLock<JsonObject> = std::sync::LazyLock::new(JsonObject::new);

impl<'json> FieldReader<'json> {
    pub fn new(value: &'json JsonValue, model_kind: &'static str) -> Result<Self, ModelError> {
        match value {
            JsonValue::Object(object) => Ok(Self { object, model_kind }),
            // An array decodes to a Lua table too; field lookups on it find nothing.
            JsonValue::Array(_) => Ok(Self { object: &EMPTY_OBJECT, model_kind }),
            _ => Err(ModelError::WrongFieldType { model_kind, field_name: String::new(), expected: "an object" }),
        }
    }

    pub fn object(&self) -> &'json JsonObject {
        self.object
    }

    fn wrong_type(&self, field_name: &str, expected: &'static str) -> ModelError {
        ModelError::WrongFieldType { model_kind: self.model_kind, field_name: field_name.to_string(), expected }
    }

    pub fn optional_value(&self, field_name: &str) -> Option<&'json JsonValue> {
        self.object.get(field_name)
    }

    pub fn required_value(&self, field_name: &'static str) -> Result<&'json JsonValue, ModelError> {
        self.object.get(field_name).ok_or(ModelError::MissingRequiredField { model_kind: self.model_kind, field_name })
    }

    fn as_text(&self, field_name: &str, value: &JsonValue) -> Result<String, ModelError> {
        value.as_str().map(str::to_string).ok_or_else(|| self.wrong_type(field_name, "text"))
    }

    pub fn required_string(&self, field_name: &'static str) -> Result<String, ModelError> {
        let value = self.required_value(field_name)?;
        self.as_text(field_name, value)
    }

    pub fn optional_string(&self, field_name: &str) -> Result<Option<String>, ModelError> {
        self.optional_value(field_name).map(|value| self.as_text(field_name, value)).transpose()
    }

    fn as_number(&self, field_name: &str, value: &JsonValue) -> Result<f64, ModelError> {
        value.as_f64().ok_or_else(|| self.wrong_type(field_name, "a number"))
    }

    pub fn required_number(&self, field_name: &'static str) -> Result<f64, ModelError> {
        let value = self.required_value(field_name)?;
        self.as_number(field_name, value)
    }

    pub fn optional_number(&self, field_name: &str) -> Result<Option<f64>, ModelError> {
        self.optional_value(field_name).map(|value| self.as_number(field_name, value)).transpose()
    }

    fn as_whole_number(&self, field_name: &str, value: &JsonValue) -> Result<i64, ModelError> {
        let number = self.as_number(field_name, value)?;
        if number.fract() != 0.0 || !number.is_finite() {
            return Err(self.wrong_type(field_name, "a whole number"));
        }
        Ok(number as i64)
    }

    pub fn required_whole_number(&self, field_name: &'static str) -> Result<i64, ModelError> {
        let value = self.required_value(field_name)?;
        self.as_whole_number(field_name, value)
    }

    pub fn optional_whole_number(&self, field_name: &str) -> Result<Option<i64>, ModelError> {
        self.optional_value(field_name).map(|value| self.as_whole_number(field_name, value)).transpose()
    }

    fn as_color(&self, field_name: &str, value: &JsonValue) -> Result<ArgbColor, ModelError> {
        let number = self.as_whole_number(field_name, value)?;
        ArgbColor::try_from(number).map_err(|_| self.wrong_type(field_name, "an ARGB colour from 0 to 4294967295"))
    }

    pub fn required_color(&self, field_name: &'static str) -> Result<ArgbColor, ModelError> {
        let value = self.required_value(field_name)?;
        self.as_color(field_name, value)
    }

    pub fn optional_color(&self, field_name: &str) -> Result<Option<ArgbColor>, ModelError> {
        self.optional_value(field_name).map(|value| self.as_color(field_name, value)).transpose()
    }

    /// The Lua idiom `t.flag == true`: anything other than the boolean `true` is false.
    pub fn flag_is_exactly_true(&self, field_name: &str) -> bool {
        self.optional_value(field_name).is_some_and(JsonValue::is_exactly_true)
    }

    pub fn required_bool(&self, field_name: &'static str) -> Result<bool, ModelError> {
        let value = self.required_value(field_name)?;
        value.as_bool().ok_or_else(|| self.wrong_type(field_name, "true or false"))
    }

    /// The Lua `default(t.flag, defaultValue)`: absent means the default.
    pub fn bool_or_default(&self, field_name: &str, default_value: bool) -> Result<bool, ModelError> {
        match self.optional_value(field_name) {
            None => Ok(default_value),
            Some(value) => value.as_bool().ok_or_else(|| self.wrong_type(field_name, "true or false")),
        }
    }

    fn as_enum<E: JsonStringEnum>(&self, qualified_field_name: &str, value: &JsonValue) -> Result<E, ModelError> {
        let parsed = value.as_str().and_then(E::from_json_str);
        parsed.ok_or_else(|| ModelError::InvalidEnumValue {
            qualified_field_name: qualified_field_name.to_string(),
            value_text: lua_tostring_of_json_value(value),
        })
    }

    /// The Lua `enumValue(t.field, allowed, default, "Kind.field")`.
    pub fn enum_or_default<E: JsonStringEnum>(
        &self,
        field_name: &str,
        default_value: E,
        qualified_field_name: &str,
    ) -> Result<E, ModelError> {
        match self.optional_value(field_name) {
            None => Ok(default_value),
            Some(value) => self.as_enum(qualified_field_name, value),
        }
    }

    pub fn required_enum<E: JsonStringEnum>(
        &self,
        field_name: &'static str,
        qualified_field_name: &str,
    ) -> Result<E, ModelError> {
        let value = self.required_value(field_name)?;
        self.as_enum(qualified_field_name, value)
    }

    /// Items of a list field, matching Lua's `ipairs(t.field or {})`: absent means empty, and an
    /// object (which `ipairs` sees as having no array part) is empty too.
    fn list_items(&self, field_name: &str, value: Option<&'json JsonValue>) -> Result<&'json [JsonValue], ModelError> {
        match value {
            None | Some(JsonValue::Object(_)) => Ok(&[]),
            Some(JsonValue::Array(items)) => Ok(items),
            Some(_) => Err(self.wrong_type(field_name, "a list")),
        }
    }

    pub fn list_of<T>(
        &self,
        field_name: &str,
        parse_item: impl Fn(&JsonValue) -> Result<T, ModelError>,
    ) -> Result<Vec<T>, ModelError> {
        self.list_items(field_name, self.optional_value(field_name))?.iter().map(parse_item).collect()
    }

    pub fn required_list_of<T>(
        &self,
        field_name: &'static str,
        parse_item: impl Fn(&JsonValue) -> Result<T, ModelError>,
    ) -> Result<Vec<T>, ModelError> {
        let value = self.required_value(field_name)?;
        self.list_items(field_name, Some(value))?.iter().map(parse_item).collect()
    }

    pub fn string_list(&self, field_name: &str) -> Result<Vec<String>, ModelError> {
        self.list_of(field_name, |item| self.as_text(field_name, item))
    }
}

/// What Lua's `tostring` prints for a decoded JSON value, used in enum error messages.
fn lua_tostring_of_json_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Number(number) => crate::json::format_like_c_percent_g(*number, 14),
        JsonValue::Null => "nil".to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => "table".to_string(),
    }
}

/// Builds a model's JSON object in field order, skipping absent values (Lua's `obj(...)`).
#[derive(Default)]
pub struct SparseObjectWriter {
    object: JsonObject,
}

impl SparseObjectWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Always writes the field.
    pub fn field(mut self, key: &str, value: impl Into<JsonValue>) -> Self {
        self.object.insert(key, value);
        self
    }

    /// Writes the field only when present.
    pub fn optional<V: Into<JsonValue>>(mut self, key: &str, value: Option<V>) -> Self {
        self.object.insert_if_present(key, value);
        self
    }

    /// Writes the field only when it differs from its default (Lua's `unlessDefault`).
    pub fn unless_default<V: Into<JsonValue> + PartialEq>(self, key: &str, value: V, default_value: V) -> Self {
        let value = if value == default_value { None } else { Some(value) };
        self.optional(key, value)
    }

    /// Writes `true` only when the flag is set (Lua's `flag and true or nil`).
    pub fn flag_when_true(self, key: &str, flag: bool) -> Self {
        self.optional(key, flag.then_some(true))
    }

    /// Writes a list only when it is not empty (Lua's `nonEmptyList`).
    pub fn non_empty_list<T>(self, key: &str, items: &[T], to_json: impl Fn(&T) -> JsonValue) -> Self {
        let value = if items.is_empty() { None } else { Some(JsonValue::Array(items.iter().map(to_json).collect())) };
        self.optional(key, value)
    }

    /// Always writes the list, even when empty.
    pub fn list<T>(self, key: &str, items: &[T], to_json: impl Fn(&T) -> JsonValue) -> Self {
        self.field(key, JsonValue::Array(items.iter().map(to_json).collect()))
    }

    pub fn finish(self) -> JsonObject {
        self.object
    }

    pub fn finish_value(self) -> JsonValue {
        JsonValue::Object(self.object)
    }
}

pub fn string_list_to_json(items: &[String]) -> JsonValue {
    JsonValue::Array(items.iter().map(JsonValue::from).collect())
}
