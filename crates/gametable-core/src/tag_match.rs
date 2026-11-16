//! Card tag matching. Port of `src/core/tag_match.lua`.
//!
//! Card `types` entries are stored as `"<Tag Group Name>|<Tag>"` so same-named tags in different
//! groups stay distinct. A plain `"<Tag>"` (no `|`) is the legacy form and matches by tag name in
//! any group.

/// `"Group|Tag"` -> `(Some("Group"), "Tag")`; a legacy `"Tag"` -> `(None, "Tag")`.
pub fn parse_tag_entry(entry: &str) -> (Option<&str>, &str) {
    match entry.split_once('|') {
        Some((group_name, tag_name)) => (Some(group_name), tag_name),
        None => (None, entry),
    }
}

pub fn encode_tag_entry(group_name: &str, tag_name: &str) -> String {
    format!("{group_name}|{tag_name}")
}

/// True when `entry` refers to `tag_name` in `group_name` (legacy entries match any group).
pub fn tag_entry_matches(entry: &str, group_name: &str, tag_name: &str) -> bool {
    let (entry_group, entry_tag) = parse_tag_entry(entry);
    entry_tag == tag_name && entry_group.is_none_or(|entry_group| entry_group == group_name)
}

pub fn card_has_tag(types: &[String], group_name: &str, tag_name: &str) -> bool {
    types.iter().any(|entry| tag_entry_matches(entry, group_name, tag_name))
}

/// Removes the first entry matching the group and tag. Returns whether one was removed.
pub fn remove_tag(types: &mut Vec<String>, group_name: &str, tag_name: &str) -> bool {
    match types.iter().position(|entry| tag_entry_matches(entry, group_name, tag_name)) {
        Some(position) => {
            types.remove(position);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qualified_and_legacy_entries() {
        assert_eq!(parse_tag_entry("Type|Character"), (Some("Type"), "Character"));
        assert_eq!(parse_tag_entry("Character"), (None, "Character"));
        assert_eq!(encode_tag_entry("Type", "Character"), "Type|Character");
    }

    #[test]
    fn qualified_entries_match_only_their_group_and_legacy_entries_match_any() {
        assert!(tag_entry_matches("Type|Character", "Type", "Character"));
        assert!(!tag_entry_matches("Type|Character", "Faction", "Character"));
        assert!(!tag_entry_matches("Type|Character", "Type", "Vehicle"));
        assert!(tag_entry_matches("Character", "Type", "Character"));
        assert!(tag_entry_matches("Character", "Faction", "Character"));
        assert!(!tag_entry_matches("Character", "Type", "Vehicle"));
        let types = vec!["Type|Character".to_string()];
        assert!(card_has_tag(&types, "Type", "Character"));
        assert!(!card_has_tag(&types, "Faction", "Character"));
    }

    #[test]
    fn remove_tag_removes_the_first_match() {
        let mut types: Vec<String> = ["Faction|Character", "Type|Character", "Common"].map(String::from).to_vec();
        assert!(remove_tag(&mut types, "Type", "Character"));
        assert_eq!(types, ["Faction|Character", "Common"]);
        assert!(remove_tag(&mut types, "Rarity", "Common"));
        assert_eq!(types, ["Faction|Character"]);
        assert!(!remove_tag(&mut types, "Rarity", "Common"));
    }
}
