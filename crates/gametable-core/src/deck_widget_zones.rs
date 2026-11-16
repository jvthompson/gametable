//! Synthetic zone ids for cards held inside a deck widget:
//! `"deckWidget:<widgetInstanceId>:<realZoneId>"`. Port of `src/core/deck_widget_zones.lua`.

const DECK_WIDGET_ZONE_PREFIX: &str = "deckWidget:";

pub fn build_deck_widget_zone_id(widget_instance_id: &str, real_zone_id: &str) -> String {
    format!("{DECK_WIDGET_ZONE_PREFIX}{widget_instance_id}:{real_zone_id}")
}

pub fn is_deck_widget_zone_id(zone_id: &str) -> bool {
    zone_id.starts_with(DECK_WIDGET_ZONE_PREFIX)
}

/// The parts of a deck widget zone id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckWidgetZoneId<'text> {
    pub widget_instance_id: &'text str,
    /// May itself contain colons.
    pub real_zone_id: &'text str,
}

/// Splits a deck widget zone id; `None` when it is not one or is malformed.
pub fn parse_deck_widget_zone_id(zone_id: &str) -> Option<DeckWidgetZoneId<'_>> {
    let rest = zone_id.strip_prefix(DECK_WIDGET_ZONE_PREFIX)?;
    let (widget_instance_id, real_zone_id) = rest.split_once(':')?;
    if widget_instance_id.is_empty() || real_zone_id.is_empty() {
        return None;
    }
    Some(DeckWidgetZoneId { widget_instance_id, real_zone_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_including_colons_in_the_real_id() {
        let built = build_deck_widget_zone_id("w1", "weird:zone:id");
        let parsed = parse_deck_widget_zone_id(&built).unwrap();
        assert_eq!(parsed.widget_instance_id, "w1");
        assert_eq!(parsed.real_zone_id, "weird:zone:id");
        assert!(!is_deck_widget_zone_id("main_deck"));
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for text in ["", "deckWidget:", "deckWidget:onlyOnePart", "deckWidget::main_deck", "deckWidget:w1:"] {
            assert_eq!(parse_deck_widget_zone_id(text), None, "{text}");
        }
    }
}
