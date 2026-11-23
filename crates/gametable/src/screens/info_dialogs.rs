//! The Controls reference and Support the Author dialogs, both rendering a bundled Markdown file.
//! Ports of `src/ui/screens/controls_dialog.lua` (`M.open`) and `support_dialog.lua`.

use crate::app::Frame;
use crate::ui::markdown::{MarkdownOptions, draw_bundled_document};
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};

const CONTROLS_DOCUMENT: &str = "CONTROLS.md";
const SUPPORT_DOCUMENT: &str = "SUPPORT-ME.md";
pub const KOFI_URL: &str = "https://ko-fi.com/jamesvthompson";

/// The Controls dialog (opened from the Game Menu); false once closed.
pub fn show_controls_dialog(frame: &mut Frame) -> bool {
    let actions = [DialogAction::filled("Done")];
    let spec = DialogSpec { title: Some("Controls"), width: 820.0, actions: &actions, ..DialogSpec::default() };
    let response = frame.dialog("controls", &spec, |frame, x, y, width| {
        let mut options = MarkdownOptions { id: "controls:md", ..MarkdownOptions::default() };
        draw_bundled_document(frame, CONTROLS_DOCUMENT, x, y, width, &mut options)
    });
    response == DialogResponse::Open
}

/// The Support the Author dialog (opened from Home); false once closed.
pub fn show_support_dialog(frame: &mut Frame) -> bool {
    let actions = [DialogAction::text("Maybe Later"), DialogAction::filled("Visit Ko-fi.com")];
    let spec =
        DialogSpec { title: Some("Support The Author"), width: 460.0, actions: &actions, ..DialogSpec::default() };
    let response = frame.dialog("support", &spec, |frame, x, y, width| {
        let mut options = MarkdownOptions { id: "support:md", ..MarkdownOptions::default() };
        draw_bundled_document(frame, SUPPORT_DOCUMENT, x, y, width, &mut options)
    });
    if response == DialogResponse::Action(1) {
        let _ = sdl3::url::open_url(KOFI_URL);
    }
    response == DialogResponse::Open
}
