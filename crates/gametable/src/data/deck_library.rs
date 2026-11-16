//! The local deck library. Port of `src/data/deck_library.lua` (name validation so far; the rest
//! comes with Phase 5).

/// `D.validateName(name, emptyMessage)`: the error for an unusable deck or save name, if any.
/// `name` is already trimmed.
pub fn validate_name(name: &str, empty_message: Option<&str>) -> Option<String> {
    if name.is_empty() {
        return Some(empty_message.unwrap_or("Enter a deck name.").to_string());
    }
    if name == "." || name == ".." {
        return Some("Invalid name.".to_string());
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Some("Name cannot contain: / \\ : * ? \" < > |".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(validate_name("", None).as_deref(), Some("Enter a deck name."));
        assert_eq!(validate_name("", Some("Enter a save name.")).as_deref(), Some("Enter a save name."));
        assert_eq!(validate_name("..", None).as_deref(), Some("Invalid name."));
        assert!(validate_name("a:b", None).is_some());
        assert_eq!(validate_name("My Deck", None), None);
    }
}
