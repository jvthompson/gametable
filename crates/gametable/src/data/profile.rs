//! Player profile, reconnect, Discord-token and theme settings. Port of `src/data/profile.lua`
//! (the avatar file operations come with the image utilities).

use gametable_core::json::{JsonObject, JsonValue};

use super::settings::{Settings, keys};

/// How many recently joined hosts/servers are remembered.
const MAX_RECENT_SERVERS: usize = 5;

/// A host or server the player joined.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentServer {
    pub host: String,
    pub port: f64,
}

pub fn name(settings: &Settings) -> Option<&str> {
    settings.text(keys::PLAYER_NAME)
}

pub fn set_name(settings: &mut Settings, name: &str) {
    settings.set(keys::PLAYER_NAME, name);
}

/// The player's colour as stored (an ARGB number).
pub fn color(settings: &Settings) -> Option<f64> {
    settings.get(keys::PLAYER_COLOR).and_then(JsonValue::as_f64)
}

pub fn set_color(settings: &mut Settings, argb: f64) {
    settings.set(keys::PLAYER_COLOR, argb);
}

pub fn avatar_path(settings: &Settings) -> Option<&str> {
    settings.text(keys::PLAYER_AVATAR_PATH)
}

pub fn set_avatar_path(settings: &mut Settings, path: Option<&str>) {
    match path {
        Some(path) => settings.set(keys::PLAYER_AVATAR_PATH, path),
        None => settings.remove(keys::PLAYER_AVATAR_PATH),
    }
}

/// The player id a host assigned last time, if `host_key` is the most recent host.
pub fn reconnect_player_id_for<'a>(settings: &'a Settings, host_key: &str) -> Option<&'a str> {
    if settings.text(keys::RECONNECT_HOST_KEY) != Some(host_key) {
        return None;
    }
    settings.text(keys::RECONNECT_PLAYER_ID)
}

pub fn save_reconnect(settings: &mut Settings, host_key: &str, player_id: &str) {
    settings.set(keys::RECONNECT_HOST_KEY, host_key);
    settings.set(keys::RECONNECT_PLAYER_ID, player_id);
}

/// Recently joined hosts/servers, most recent first. Malformed entries are skipped.
pub fn recent_servers(settings: &Settings) -> Vec<RecentServer> {
    let Some(JsonValue::Array(entries)) = settings.get(keys::RECENT_SERVERS) else { return Vec::new() };
    entries
        .iter()
        .filter_map(|entry| {
            let host = entry.get("host")?.as_str()?.to_string();
            let port = match entry.get("port")? {
                JsonValue::Number(number) => *number,
                JsonValue::String(text) => text.trim().parse().ok()?,
                _ => return None,
            };
            Some(RecentServer { host, port })
        })
        .collect()
}

fn save_recent_servers(settings: &mut Settings, servers: &[RecentServer]) {
    let entries = servers
        .iter()
        .map(|server| JsonValue::Object(JsonObject::new().with("host", server.host.as_str()).with("port", server.port)))
        .collect();
    settings.set(keys::RECENT_SERVERS, JsonValue::Array(entries));
}

/// Puts a server first in the recent list (keeping at most five).
pub fn add_recent_server(settings: &mut Settings, host: &str, port: f64) {
    let mut servers = vec![RecentServer { host: host.to_string(), port }];
    for server in recent_servers(settings) {
        if servers.len() >= MAX_RECENT_SERVERS {
            break;
        }
        if !(server.host == host && server.port == port) {
            servers.push(server);
        }
    }
    save_recent_servers(settings, &servers);
}

pub fn remove_recent_server(settings: &mut Settings, host: &str, port: f64) {
    let servers: Vec<RecentServer> =
        recent_servers(settings).into_iter().filter(|server| !(server.host == host && server.port == port)).collect();
    save_recent_servers(settings, &servers);
}

pub fn discord_refresh_token(settings: &Settings) -> Option<&str> {
    settings.text(keys::DISCORD_REFRESH_TOKEN)
}

pub fn save_discord_refresh_token(settings: &mut Settings, token: &str) {
    settings.set(keys::DISCORD_REFRESH_TOKEN, token);
}

pub fn clear_discord_refresh_token(settings: &mut Settings) {
    settings.remove(keys::DISCORD_REFRESH_TOKEN);
}

/// Dark mode. The original returned the stored value as-is and callers tested it for truth, so
/// anything other than a missing value or `false` counts as on.
pub fn dark_mode(settings: &Settings) -> bool {
    match settings.get(keys::DARK_MODE) {
        None | Some(JsonValue::Bool(false)) | Some(JsonValue::Null) => false,
        Some(_) => true,
    }
}

pub fn set_dark_mode(settings: &mut Settings, enabled: bool) {
    settings.set(keys::DARK_MODE, enabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_servers_keep_five_most_recent_first() {
        let mut settings = Settings::in_memory();
        for index in 0..7 {
            add_recent_server(&mut settings, &format!("host{index}"), 51234.0);
        }
        add_recent_server(&mut settings, "host3", 51234.0);
        let hosts: Vec<String> = recent_servers(&settings).into_iter().map(|server| server.host).collect();
        assert_eq!(hosts, ["host3", "host6", "host5", "host4", "host2"]);
        remove_recent_server(&mut settings, "host5", 51234.0);
        assert_eq!(recent_servers(&settings).len(), 4);
        let encoded = gametable_core::json::encode_compact(settings.get(keys::RECENT_SERVERS).unwrap()).unwrap();
        assert!(encoded.starts_with(r#"[{"host":"host3","port":51234},{"host":"host6""#), "{encoded}");
    }

    #[test]
    fn reconnect_only_matches_the_last_host() {
        let mut settings = Settings::in_memory();
        assert_eq!(reconnect_player_id_for(&settings, "a:1"), None);
        save_reconnect(&mut settings, "a:1", "p1");
        assert_eq!(reconnect_player_id_for(&settings, "a:1"), Some("p1"));
        assert_eq!(reconnect_player_id_for(&settings, "b:1"), None);
    }

    #[test]
    fn profile_fields() {
        let mut settings = Settings::in_memory();
        assert!(!dark_mode(&settings));
        set_dark_mode(&mut settings, true);
        assert!(dark_mode(&settings));
        set_name(&mut settings, "Ann");
        set_color(&mut settings, 4294278144.0);
        set_avatar_path(&mut settings, Some("C:\\a.png"));
        assert_eq!(
            (name(&settings), color(&settings), avatar_path(&settings)),
            (Some("Ann"), Some(4294278144.0), Some("C:\\a.png"))
        );
        set_avatar_path(&mut settings, None);
        assert_eq!(avatar_path(&settings), None);
        save_discord_refresh_token(&mut settings, "t");
        clear_discord_refresh_token(&mut settings);
        assert_eq!(discord_refresh_token(&settings), None);
    }
}
