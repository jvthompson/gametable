//! The system clipboard (`love.system.get/setClipboardText`), behind a trait so the UI kit can
//! be tested without a window.

use sdl3::clipboard::ClipboardUtil;

pub trait Clipboard {
    /// The clipboard's text, empty when it holds none.
    fn get_text(&self) -> String;
    fn set_text(&mut self, text: &str);
}

/// SDL's clipboard.
pub struct SdlClipboard {
    clipboard: ClipboardUtil,
}

impl SdlClipboard {
    pub fn new(clipboard: ClipboardUtil) -> SdlClipboard {
        SdlClipboard { clipboard }
    }
}

impl Clipboard for SdlClipboard {
    fn get_text(&self) -> String {
        self.clipboard.clipboard_text().unwrap_or_default()
    }

    fn set_text(&mut self, text: &str) {
        let _ = self.clipboard.set_clipboard_text(text);
    }
}

/// A clipboard held in memory (tests, headless tools).
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    pub contents: String,
}

impl Clipboard for MemoryClipboard {
    fn get_text(&self) -> String {
        self.contents.clone()
    }

    fn set_text(&mut self, text: &str) {
        self.contents = text.to_string();
    }
}
