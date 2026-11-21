//! A dialog for picking one entry from a list that may grow long (zones to place on a board,
//! boards to put on the table): a filter field over a scrolling list. Clicking an entry picks it.

use crate::app::Frame;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::ScrollbarStyle;

const ROW_HEIGHT: f32 = 48.0;
const LIST_HEIGHT: f32 = 360.0;
const FIELD_HEIGHT: f32 = 48.0;

/// One entry: what is reported when it is picked, and what is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerEntry {
    pub key: String,
    pub label: String,
    /// A second, smaller line.
    pub detail: Option<String>,
}

impl PickerEntry {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> PickerEntry {
        PickerEntry { key: key.into(), label: label.into(), detail: None }
    }

    pub fn detail(self, detail: impl Into<String>) -> PickerEntry {
        PickerEntry { detail: Some(detail.into()), ..self }
    }

    /// Whether the entry matches the filter text (case-insensitive, label or detail).
    pub fn matches(&self, filter: &str) -> bool {
        let filter = filter.trim().to_lowercase();
        filter.is_empty()
            || self.label.to_lowercase().contains(&filter)
            || self.detail.as_ref().is_some_and(|detail| detail.to_lowercase().contains(&filter))
    }
}

/// What the picker reports this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerResponse {
    Open,
    Picked(String),
    Cancelled,
}

pub struct ListPickerDialog {
    id: String,
    title: String,
    empty_message: String,
    entries: Vec<PickerEntry>,
    filter: String,
}

impl ListPickerDialog {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        empty_message: impl Into<String>,
        entries: Vec<PickerEntry>,
    ) -> ListPickerDialog {
        ListPickerDialog {
            id: id.into(),
            title: title.into(),
            empty_message: empty_message.into(),
            entries,
            filter: String::new(),
        }
    }

    /// The region id of an entry's row (for scripted UI checks).
    pub fn row_id(dialog_id: &str, key: &str) -> String {
        format!("{dialog_id}:row:{key}")
    }

    pub fn show(&mut self, frame: &mut Frame) -> PickerResponse {
        let actions = [DialogAction::text("Cancel")];
        let spec = DialogSpec { title: Some(&self.title), width: 460.0, actions: &actions, ..DialogSpec::default() };
        let id = self.id.clone();
        let entries = &self.entries;
        let filter = &mut self.filter;
        let empty_message = &self.empty_message;
        let mut picked = None;
        let response = frame.dialog(&id, &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let options = TextFieldOptions { hint: Some("Filter"), autofocus: true, ..TextFieldOptions::default() };
            frame.text_field(&format!("{id}:filter"), x, y, width, filter, &options);
            let list_y = y + FIELD_HEIGHT + 12.0;
            let shown: Vec<&PickerEntry> = entries.iter().filter(|entry| entry.matches(filter)).collect();
            if shown.is_empty() {
                let message = if entries.is_empty() { empty_message.as_str() } else { "Nothing matches the filter." };
                let options = TextOptions::new().size(14.0).width(width).color(palette.on_surface_variant);
                frame.text(message, x, list_y + 8.0, options);
                return FIELD_HEIGHT + 12.0 + 40.0;
            }
            let scroll_id = format!("{id}:list");
            let offset = frame.begin_scroll(&scroll_id, x, list_y, width, LIST_HEIGHT);
            let mut row_y = list_y - offset;
            for entry in &shown {
                let region = frame.ui.region(
                    &ListPickerDialog::row_id(&id, &entry.key),
                    x,
                    row_y,
                    width,
                    ROW_HEIGHT,
                    RegionOptions::hand(),
                );
                if region.hovered {
                    frame.fill_rect(x, row_y, width, ROW_HEIGHT, 8.0, with_alpha(palette.on_surface, 0.08));
                }
                let text_width = width - 24.0;
                match &entry.detail {
                    Some(detail) => {
                        let label = TextOptions::new().width(text_width).ellipsis().color(palette.on_surface);
                        frame.text(&entry.label, x + 12.0, row_y + 6.0, label);
                        let detail_options = TextOptions::new()
                            .size(12.0)
                            .width(text_width)
                            .ellipsis()
                            .color(palette.on_surface_variant);
                        frame.text(detail, x + 12.0, row_y + 26.0, detail_options);
                    }
                    None => {
                        let label = TextOptions::new()
                            .width(text_width)
                            .ellipsis()
                            .middle_of(ROW_HEIGHT)
                            .color(palette.on_surface);
                        frame.text(&entry.label, x + 12.0, row_y, label);
                    }
                }
                if region.clicked {
                    picked = Some(entry.key.clone());
                }
                row_y += ROW_HEIGHT;
            }
            frame.end_scroll(&scroll_id, shown.len() as f32 * ROW_HEIGHT, &ScrollbarStyle::default());
            FIELD_HEIGHT + 12.0 + LIST_HEIGHT.min(shown.len() as f32 * ROW_HEIGHT)
        });
        if let Some(key) = picked {
            return PickerResponse::Picked(key);
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => PickerResponse::Open,
            DialogResponse::Action(_) | DialogResponse::Dismissed => PickerResponse::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_matches_label_or_detail_ignoring_case() {
        let entry = PickerEntry::new("walls", "Walls").detail("Scenario: Siege");
        assert!(entry.matches(""));
        assert!(entry.matches("wal"));
        assert!(entry.matches(" SIEGE "));
        assert!(!entry.matches("deck"));
    }
}
