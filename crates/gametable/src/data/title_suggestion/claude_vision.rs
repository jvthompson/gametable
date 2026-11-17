//! "Suggest Title → From Image" with Claude: sends the card picture to the Anthropic Messages API
//! and asks for the printed title. Needs the user's own API key (Settings → Claude API key); API
//! use is billed to that key's account, separately from any Claude subscription.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{self, JsonObject, JsonValue, encode_compact};

use crate::graphics::image_data::RgbaImage;

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
/// The server-side refusal fallback; only sent for Opus (Haiku has none).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Card art is shrunk to this long side before sending: plenty to read a title, fewer tokens.
const MAX_IMAGE_SIDE: u32 = 1024;
/// Room for the model's (always-on) thinking plus the short JSON answer.
const MAX_OUTPUT_TOKENS: u32 = 2048;
/// Attempts for rate limits, overload and server errors.
const MAX_ATTEMPTS: u32 = 4;

/// Which Claude model reads the cards (Settings → Claude model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaudeModel {
    /// Claude Haiku 5.5: much cheaper, reads printed titles well.
    #[default]
    Haiku,
    /// Claude Opus 5.5: most accurate, for stylised or worn card art.
    Opus,
}

impl ClaudeModel {
    pub fn model_id(self) -> &'static str {
        match self {
            ClaudeModel::Haiku => "claude-haiku-5-5",
            ClaudeModel::Opus => "claude-opus-5-5",
        }
    }

    /// The value stored in settings.
    pub fn setting_value(self) -> &'static str {
        match self {
            ClaudeModel::Haiku => "haiku",
            ClaudeModel::Opus => "opus",
        }
    }

    pub fn from_setting_value(value: Option<&str>) -> ClaudeModel {
        if value == Some("opus") { ClaudeModel::Opus } else { ClaudeModel::Haiku }
    }
}

/// What one card's request needs.
#[derive(Debug, Clone)]
pub struct ClaudeCardRequest {
    pub api_key: String,
    pub model: ClaudeModel,
    pub game_name: String,
    pub card_id: String,
    pub image_path: String,
}

/// Reads one card's title from its picture. `Ok(None)` when Claude finds no title (or declines).
pub fn suggest_title_with_claude(request: &ClaudeCardRequest) -> Result<Option<String>, String> {
    let image = RgbaImage::load(Path::new(&request.image_path))?.downscaled_to_fit(MAX_IMAGE_SIDE);
    let png_base64 = BASE64.encode(image.encode_png()?);
    let body = build_request_body(request.model, &png_base64, &request.game_name, &request.card_id);
    let body_text = encode_compact(&body).map_err(|_| "Could not build the request.".to_string())?;
    let response = post_with_retries(&request.api_key, request.model, &body_text)?;
    parse_title_response(&response)
}

/// The Messages API request body for one card.
pub fn build_request_body(model: ClaudeModel, png_base64: &str, game_name: &str, card_id: &str) -> JsonValue {
    let instructions = format!(
        "This picture is a card from the game \"{game_name}\" (its ID in the game files is \"{card_id}\"). \
         Read the card's title exactly as it is printed on the card, keeping its capitalisation and punctuation. \
         The title is usually the largest text near the top of the card. If the card has no readable title, \
         set found to false and title to an empty string."
    );
    let image_block = JsonObject::new().with("type", "image").with(
        "source",
        JsonObject::new().with("type", "base64").with("media_type", "image/png").with("data", png_base64),
    );
    let text_block = JsonObject::new().with("type", "text").with("text", instructions);
    let message = JsonObject::new().with("role", "user").with("content", vec![image_block, text_block]);
    let schema = JsonObject::new()
        .with("type", "object")
        .with(
            "properties",
            JsonObject::new()
                .with("title", JsonObject::new().with("type", "string"))
                .with("found", JsonObject::new().with("type", "boolean")),
        )
        .with("required", vec!["title", "found"])
        .with("additionalProperties", false);
    let output_config = JsonObject::new()
        .with("effort", "low")
        .with("format", JsonObject::new().with("type", "json_schema").with("schema", schema));
    let mut body = JsonObject::new()
        .with("model", model.model_id())
        .with("max_tokens", MAX_OUTPUT_TOKENS)
        .with("output_config", output_config)
        .with("messages", vec![message]);
    if model == ClaudeModel::Opus {
        body.insert("fallbacks", "default");
    }
    JsonValue::Object(body)
}

/// The title from a Messages API response.
pub fn parse_title_response(response: &JsonValue) -> Result<Option<String>, String> {
    match response.get("stop_reason").and_then(JsonValue::as_str) {
        Some("refusal") => return Ok(None),
        Some("max_tokens") => return Err("Claude's answer was cut off.".to_string()),
        _ => {}
    }
    let text: String = response
        .get("content")
        .and_then(JsonValue::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(JsonValue::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(JsonValue::as_str))
                .collect()
        })
        .unwrap_or_default();
    let answer = json::decode(&text).map_err(|_| "Claude's answer was not in the expected format.".to_string())?;
    let found = answer.get("found").and_then(JsonValue::as_bool).unwrap_or(false);
    let title = answer.get("title").and_then(JsonValue::as_str).unwrap_or("").trim().to_string();
    Ok((found && !title.is_empty()).then_some(title))
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Posts the request, retrying rate limits (429), overload (529) and server errors with backoff.
fn post_with_retries(api_key: &str, model: ClaudeModel, body: &str) -> Result<JsonValue, String> {
    let agent = http_agent();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut request = agent
            .post(MESSAGES_URL)
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION);
        if model == ClaudeModel::Opus {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }
        let mut response = match request.send(body) {
            Ok(response) => response,
            Err(_) if attempt < MAX_ATTEMPTS => {
                std::thread::sleep(backoff(attempt, None));
                continue;
            }
            Err(error) => return Err(format!("Could not reach the Anthropic API: {error}")),
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let text = response.body_mut().read_to_string().map_err(|error| error.to_string())?;
        if status == 200 {
            return json::decode(&text).map_err(|_| "The Anthropic API sent an unreadable answer.".to_string());
        }
        let retryable = status == 429 || status == 529 || status >= 500;
        if retryable && attempt < MAX_ATTEMPTS {
            std::thread::sleep(backoff(attempt, retry_after));
            continue;
        }
        return Err(api_error_message(status, &text));
    }
}

fn backoff(attempt: u32, retry_after_seconds: Option<u64>) -> Duration {
    match retry_after_seconds {
        Some(seconds) => Duration::from_secs(seconds.min(60)),
        None => Duration::from_millis(1000 * 2u64.pow(attempt - 1)),
    }
}

/// A short message for a failed request (never includes the key).
fn api_error_message(status: u16, body: &str) -> String {
    let detail = json::decode(body).ok().and_then(|value| {
        value.get("error").and_then(|error| error.get("message")).and_then(JsonValue::as_str).map(str::to_string)
    });
    match status {
        401 => "The Claude API key was not accepted (check Settings).".to_string(),
        402 | 403 => format!(
            "The Anthropic API refused the request{}",
            detail.map(|text| format!(": {text}")).unwrap_or_default()
        ),
        _ => format!(
            "The Anthropic API returned HTTP {status}{}",
            detail.map(|text| format!(": {text}")).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_has_image_schema_and_model() {
        let body = build_request_body(ClaudeModel::Haiku, "AAAA", "Spellfire", "booster_9_13245");
        assert_eq!(body.get("model").and_then(JsonValue::as_str), Some("claude-haiku-5-5"));
        assert!(body.get("fallbacks").is_none());
        let content = body.get("messages").and_then(JsonValue::as_array).unwrap()[0].get("content").unwrap();
        let image = &content.as_array().unwrap()[0];
        assert_eq!(image.get("source").and_then(|source| source.get("data")).and_then(JsonValue::as_str), Some("AAAA"));
        let text = content.as_array().unwrap()[1].get("text").and_then(JsonValue::as_str).unwrap();
        assert!(text.contains("Spellfire") && text.contains("booster_9_13245"));
        let format = body.get("output_config").and_then(|config| config.get("format")).unwrap();
        assert_eq!(format.get("type").and_then(JsonValue::as_str), Some("json_schema"));
        let opus = build_request_body(ClaudeModel::Opus, "AAAA", "g", "c");
        assert_eq!(opus.get("model").and_then(JsonValue::as_str), Some("claude-opus-5-5"));
        assert_eq!(opus.get("fallbacks").and_then(JsonValue::as_str), Some("default"));
    }

    #[test]
    fn responses_become_titles() {
        let answer = |text: &str, stop: &str| {
            json::decode(&format!(
                r#"{{"stop_reason":"{stop}","content":[{{"type":"thinking","thinking":""}},{{"type":"text","text":{}}}]}}"#,
                encode_compact(&JsonValue::from(text)).unwrap()
            ))
            .unwrap()
        };
        assert_eq!(
            parse_title_response(&answer(r#"{"title":" Sithicus ","found":true}"#, "end_turn")),
            Ok(Some("Sithicus".to_string()))
        );
        assert_eq!(parse_title_response(&answer(r#"{"title":"","found":false}"#, "end_turn")), Ok(None));
        assert_eq!(parse_title_response(&answer("", "refusal")), Ok(None));
        assert!(parse_title_response(&answer("Sithicus", "end_turn")).is_err());
        assert!(parse_title_response(&answer("", "max_tokens")).is_err());
    }

    #[test]
    fn model_setting_round_trips() {
        assert_eq!(ClaudeModel::from_setting_value(Some("opus")), ClaudeModel::Opus);
        assert_eq!(ClaudeModel::from_setting_value(None), ClaudeModel::Haiku);
        assert_eq!(ClaudeModel::from_setting_value(Some(ClaudeModel::Haiku.setting_value())), ClaudeModel::Haiku);
        assert!(api_error_message(401, "{}").contains("key"));
    }
}
