//! Modal dialogs, context menus and snackbars. Port of `src/ui/kit/overlays.lua`.
//!
//! The original kept one global stack of dialogs and menus whose callbacks closed over screen
//! state. Here a screen owns its dialogs and menus and shows them each frame while they are
//! open; they report what happened ([`DialogResponse`], [`MenuResponse`]) and the screen reacts
//! (usually by dropping the dialog). Escape and Enter reach the top dialog or menu through
//! [`Ui::overlay_key_pressed`](crate::ui::context::Ui::overlay_key_pressed).
//!
//! App-level messages that must outlive a screen (an alert shown while returning Home, a
//! confirmation that pushes a screen) and snackbars are kept in the UI itself ([`AppOverlays`])
//! and drawn by the app shell after the screen.

use crate::app::{Frame, ScreenCommand};
use crate::graphics::draw_list::Color;
use crate::ui::context::{OpenOverlay, OverlayKey, Rect, RegionOptions, Ui};
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{BUTTON_HEIGHT, ButtonOptions, ButtonVariant, ScrollbarStyle};

// -- Dialogs -------------------------------------------------------------------------------------

/// One action button of a dialog, laid out right to left from the last.
#[derive(Debug, Clone, Copy)]
pub struct DialogAction<'a> {
    pub label: &'a str,
    pub variant: ButtonVariant,
    pub disabled: bool,
    pub visible: bool,
}

impl<'a> DialogAction<'a> {
    pub fn new(label: &'a str, variant: ButtonVariant) -> DialogAction<'a> {
        DialogAction { label, variant, disabled: false, visible: true }
    }

    pub fn text(label: &'a str) -> DialogAction<'a> {
        DialogAction::new(label, ButtonVariant::Text)
    }

    pub fn filled(label: &'a str) -> DialogAction<'a> {
        DialogAction::new(label, ButtonVariant::Filled)
    }

    pub fn disabled(self, disabled: bool) -> Self {
        DialogAction { disabled, ..self }
    }

    pub fn visible(self, visible: bool) -> Self {
        DialogAction { visible, ..self }
    }
}

/// What a dialog shows (the original's `spec`).
#[derive(Debug, Clone, Copy)]
pub struct DialogSpec<'a> {
    pub title: Option<&'a str>,
    /// Default 400.
    pub width: f32,
    /// The fixed dark panel used in game.
    pub dark: bool,
    /// Escape and scrim clicks dismiss it (default true).
    pub dismissible: bool,
    /// Plain text content above any custom content.
    pub message: Option<&'a str>,
    pub actions: &'a [DialogAction<'a>],
    /// Enter is reported as [`DialogResponse::Enter`] (`spec.onEnter`).
    pub handles_enter: bool,
}

impl Default for DialogSpec<'_> {
    fn default() -> Self {
        DialogSpec {
            title: None,
            width: 400.0,
            dark: false,
            dismissible: true,
            message: None,
            actions: &[],
            handles_enter: false,
        }
    }
}

/// What happened to a dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogResponse {
    Open,
    /// The action at this index was clicked. The original closed the dialog unless the action
    /// asked to keep it open; here the caller decides by not showing it again.
    Action(usize),
    /// Escape or a click on the scrim.
    Dismissed,
    /// Enter, for dialogs with `handles_enter`.
    Enter,
}

#[derive(Debug, Default)]
struct DialogState {
    content_height: f32,
}

/// What a confirm dialog reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResponse {
    Open,
    Confirmed,
    /// Cancel, Escape or a scrim click.
    Cancelled,
}

impl Frame<'_> {
    /// Shows a modal dialog for this frame. `content(frame, x, y, width)` draws custom content
    /// and returns the height it used; the content scrolls when the screen is too short.
    pub fn dialog(
        &mut self,
        id: &str,
        spec: &DialogSpec,
        content: impl FnOnce(&mut Frame, f32, f32, f32) -> f32,
    ) -> DialogResponse {
        self.ui.register_open_overlay(OpenOverlay::Dialog {
            id: id.to_string(),
            dismissible: spec.dismissible,
            handles_enter: spec.handles_enter,
        });
        match self.ui.take_overlay_key(id) {
            Some(OverlayKey::Escape) if spec.dismissible => {
                self.ui.blur();
                return DialogResponse::Dismissed;
            }
            Some(OverlayKey::Enter) if spec.handles_enter => return DialogResponse::Enter,
            _ => {}
        }
        let is_top = self.ui.top_dialog_last_frame().is_none_or(|top| top == id);
        let menu_was_open = self.ui.menu_open();
        let response = self.overlay(|frame| frame.draw_dialog(id, spec, is_top && !menu_was_open, content));
        if response == DialogResponse::Dismissed {
            self.ui.blur();
        }
        response
    }

    fn draw_dialog(
        &mut self,
        id: &str,
        spec: &DialogSpec,
        can_dismiss_by_scrim: bool,
        content: impl FnOnce(&mut Frame, f32, f32, f32) -> f32,
    ) -> DialogResponse {
        let (screen_width, screen_height) = self.ui.screen_size();
        let top = self.ui.content_top;
        self.ui.cover(-1e9, top, 2e9, 2e9);
        self.ui.mark_modal();
        let scrim = Color::rgba(0.0, 0.0, 0.0, if spec.dark { 0.25 } else { 0.32 });
        self.fill_rect(0.0, top, screen_width, screen_height - top, 0.0, scrim);

        let palette = self.palette();
        let foreground = if spec.dark { fixed::WHITE } else { palette.on_surface };
        let width = spec.width.min(screen_width - 32.0);
        let padding = 24.0;
        let title_height = if spec.title.is_some() { 40.0 } else { 0.0 };
        let actions_height = if spec.actions.is_empty() { padding } else { 24.0 + BUTTON_HEIGHT + 24.0 };
        let max_content_height = screen_height - top - 64.0 - title_height - actions_height - padding;
        let stored_height = self.ui.state::<DialogState>(&format!("{id}:dialog")).content_height;
        let content_height = stored_height.min(max_content_height);
        let height = padding + title_height + content_height + actions_height;
        let x = ((screen_width - width) / 2.0).floor();
        let y = (top + (screen_height - top - height) / 2.0).floor();

        self.shadow(x, y, width, height, 28.0, 3.0);
        let panel = if spec.dark { fixed::OVERLAY_PANEL } else { palette.surface_container_high };
        self.fill_rect(x, y, width, height, 28.0, panel);
        if let Some(title) = spec.title {
            let title_options = TextOptions::new().size(22.0).color(foreground).width(width - 2.0 * padding).ellipsis();
            self.text(title, x + padding, y + padding, title_options);
        }

        // Content (scrolls if taller than the screen allows).
        let (content_x, content_y, content_width) = (x + padding, y + padding + title_height, width - 2.0 * padding);
        let scroll_id = format!("dialog:{id}");
        let offset = self.begin_scroll(&scroll_id, content_x - 4.0, content_y, content_width + 8.0, content_height);
        let mut used = 0.0;
        if let Some(message) = spec.message {
            used = self.text_height(message, content_width, 14.0, Weight::Regular);
            let color = if spec.dark { Color::rgba(1.0, 1.0, 1.0, 0.8) } else { palette.on_surface_variant };
            self.text(message, content_x, content_y - offset, TextOptions::new().width(content_width).color(color));
        }
        used += content(self, content_x, content_y - offset + used, content_width);
        self.end_scroll(&scroll_id, used, &ScrollbarStyle::default());
        self.ui.state::<DialogState>(&format!("{id}:dialog")).content_height = used;

        // Actions, right-aligned.
        let mut response = DialogResponse::Open;
        if !spec.actions.is_empty() {
            let mut action_x = x + width - padding;
            let action_y = y + height - 24.0 - BUTTON_HEIGHT;
            for (index, action) in spec.actions.iter().enumerate().rev() {
                if !action.visible {
                    continue;
                }
                let options = ButtonOptions::variant(action.variant);
                let button_width = self.button_width(action.label, &options);
                action_x -= button_width;
                let button_id = format!("{id}:a{}", index + 1);
                let color = (spec.dark && action.variant == ButtonVariant::Text).then_some(fixed::WHITE);
                let options = ButtonOptions { id: Some(&button_id), disabled: action.disabled, color, ..options };
                if self.button(action.label, action_x, action_y, Some(button_width), None, &options) {
                    response = DialogResponse::Action(index);
                }
                action_x -= 8.0;
            }
        }

        if response == DialogResponse::Open
            && can_dismiss_by_scrim
            && spec.dismissible
            && self.ui.mouse_pressed(1)
            && !self.ui.hover_ignoring_layer(x, y, width, height)
        {
            response = DialogResponse::Dismissed;
        }
        response
    }

    /// `overlays.confirm`: Cancel and a confirm button (red with `danger`).
    pub fn confirm_dialog(
        &mut self,
        id: &str,
        title: &str,
        message: &str,
        confirm_label: &str,
        danger: bool,
    ) -> ConfirmResponse {
        let confirm_variant = if danger { ButtonVariant::Danger } else { ButtonVariant::Filled };
        let actions = [DialogAction::text("Cancel"), DialogAction::new(confirm_label, confirm_variant)];
        let spec = DialogSpec {
            title: Some(title),
            message: Some(message),
            width: 420.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        match self.dialog(id, &spec, |_, _, _, _| 0.0) {
            DialogResponse::Action(1) => ConfirmResponse::Confirmed,
            DialogResponse::Open | DialogResponse::Enter => ConfirmResponse::Open,
            _ => ConfirmResponse::Cancelled,
        }
    }

    /// `overlays.alert`: a message with OK. Returns true once it is closed.
    pub fn alert_dialog(&mut self, id: &str, title: &str, message: &str) -> bool {
        let actions = [DialogAction::text("OK")];
        let spec = DialogSpec {
            title: Some(title),
            message: Some(message),
            width: 420.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        self.dialog(id, &spec, |_, _, _, _| 0.0) != DialogResponse::Open
    }

    /// Draws the app-level dialogs and the snackbar (after the screen and title bar).
    pub(crate) fn draw_app_overlays(&mut self) {
        let dialogs = std::mem::take(&mut self.ui.app_overlays.dialogs);
        let mut kept = Vec::new();
        for mut dialog in dialogs {
            let closed = match &dialog.confirm_label {
                None => self.alert_dialog(&dialog.id, &dialog.title, &dialog.message),
                Some(label) => {
                    let actions = [DialogAction::text(&dialog.cancel_label), DialogAction::filled(label)];
                    let spec = DialogSpec {
                        title: Some(&dialog.title),
                        message: Some(&dialog.message),
                        width: 420.0,
                        actions: &actions,
                        ..DialogSpec::default()
                    };
                    match self.dialog(&dialog.id, &spec, |_, _, _, _| 0.0) {
                        DialogResponse::Open | DialogResponse::Enter => false,
                        DialogResponse::Action(1) => {
                            if let Some(on_confirm) = dialog.on_confirm.take() {
                                on_confirm(self.commands);
                            }
                            true
                        }
                        _ => true,
                    }
                }
            };
            if !closed {
                kept.push(dialog);
            }
        }
        // Dialogs added while drawing go on top of the kept ones.
        kept.append(&mut self.ui.app_overlays.dialogs);
        self.ui.app_overlays.dialogs = kept;
        self.overlay(|frame| frame.draw_toasts());
    }

    fn draw_toasts(&mut self) {
        let now = self.ui.time();
        self.ui.app_overlays.toasts.retain(|toast| toast.until > now);
        let Some(toast) = self.ui.app_overlays.toasts.last().cloned() else { return };
        let (screen_width, screen_height) = self.ui.screen_size();
        let text_width = self.text_width(&toast.text, 14.0, Weight::Regular);
        let width = (screen_width - 32.0).min(344.0f32.max(text_width + 32.0));
        let (x, y) = ((screen_width - width) / 2.0, screen_height - 48.0 - 24.0);
        let fade = ((toast.until - now) * 4.0).min(1.0) as f32;
        self.ui.cover(x, y, width, 48.0);
        self.fill_rect(x, y, width, 48.0, 4.0, Color::rgba(0.2, 0.2, 0.2, 0.95 * fade));
        let text_options = TextOptions::new()
            .width(width - 32.0)
            .middle_of(48.0)
            .ellipsis()
            .color(Color::rgba(0.95, 0.95, 0.95, fade));
        self.text(&toast.text, x + 16.0, y, text_options);
    }
}

// -- App-level dialogs and snackbars ---------------------------------------------------------------

/// Runs when an app-level confirmation is accepted; it may push or pop screens.
pub type ConfirmAction = Box<dyn FnOnce(&mut Vec<ScreenCommand>)>;

pub(crate) struct AppDialog {
    id: String,
    title: String,
    message: String,
    /// `None` for an alert with just OK.
    confirm_label: Option<String>,
    cancel_label: String,
    on_confirm: Option<ConfirmAction>,
}

#[derive(Debug, Clone)]
struct Toast {
    text: String,
    until: f64,
}

/// Dialogs and snackbars owned by the app rather than a screen.
#[derive(Default)]
pub struct AppOverlays {
    dialogs: Vec<AppDialog>,
    toasts: Vec<Toast>,
    next_id: u64,
}

impl Ui {
    /// `overlays.toast(text, seconds)`: a snackbar at the bottom (4 seconds by default).
    pub fn toast(&mut self, text: &str, seconds: Option<f64>) {
        let until = self.time() + seconds.unwrap_or(4.0);
        self.app_overlays.toasts.push(Toast { text: text.to_string(), until });
    }

    /// `overlays.alert(title, message)` kept above whatever screen is showing.
    pub fn alert(&mut self, title: &str, message: &str) {
        self.push_app_dialog(title, message, None, "OK", None);
    }

    /// `overlays.confirm` whose action runs screen commands (e.g. the Discord join prompt).
    pub fn confirm(
        &mut self,
        title: &str,
        message: &str,
        confirm_label: &str,
        cancel_label: &str,
        on_confirm: ConfirmAction,
    ) {
        self.push_app_dialog(title, message, Some(confirm_label), cancel_label, Some(on_confirm));
    }

    fn push_app_dialog(
        &mut self,
        title: &str,
        message: &str,
        confirm_label: Option<&str>,
        cancel_label: &str,
        on_confirm: Option<ConfirmAction>,
    ) {
        let overlays = &mut self.app_overlays;
        overlays.next_id += 1;
        overlays.dialogs.push(AppDialog {
            id: format!("app-dialog-{}", overlays.next_id),
            title: title.to_string(),
            message: message.to_string(),
            confirm_label: confirm_label.map(str::to_string),
            cancel_label: cancel_label.to_string(),
            on_confirm,
        });
    }
}

// -- Context menus --------------------------------------------------------------------------------

const ITEM_HEIGHT: f32 = 40.0;

/// One context menu entry.
#[derive(Debug, Clone)]
pub struct MenuItem<A> {
    pub label: String,
    pub icon: Option<&'static str>,
    /// Reported when the item is chosen; `None` just closes the menu.
    pub action: Option<A>,
    /// Non-empty for an item that opens a submenu.
    pub submenu: Vec<MenuItem<A>>,
    pub disabled: bool,
    /// `Some` reserves a check column; `Some(true)` shows the check.
    pub checked: Option<bool>,
    pub divider: bool,
}

impl<A> MenuItem<A> {
    pub fn new(label: impl Into<String>, action: A) -> MenuItem<A> {
        MenuItem {
            label: label.into(),
            icon: None,
            action: Some(action),
            submenu: Vec::new(),
            disabled: false,
            checked: None,
            divider: false,
        }
    }

    pub fn submenu(label: impl Into<String>, items: Vec<MenuItem<A>>) -> MenuItem<A> {
        MenuItem { action: None, submenu: items, ..MenuItem::label_only(label) }
    }

    pub fn divider() -> MenuItem<A> {
        MenuItem { divider: true, ..MenuItem::label_only("") }
    }

    fn label_only(label: impl Into<String>) -> MenuItem<A> {
        MenuItem {
            label: label.into(),
            icon: None,
            action: None,
            submenu: Vec::new(),
            disabled: false,
            checked: None,
            divider: false,
        }
    }

    pub fn icon(self, icon: &'static str) -> Self {
        MenuItem { icon: Some(icon), ..self }
    }

    pub fn disabled(self, disabled: bool) -> Self {
        MenuItem { disabled, ..self }
    }

    pub fn checked(self, checked: bool) -> Self {
        MenuItem { checked: Some(checked), ..self }
    }
}

/// What a menu reports this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuResponse<A> {
    Open,
    Selected(A),
    /// Closed without a choice (outside click, Escape, an item without an action).
    Closed,
}

#[derive(Debug, Clone)]
struct MenuLevel {
    /// Item indexes from the root to this submenu (empty for the root).
    path: Vec<usize>,
    x: f32,
    y: f32,
    /// A submenu that doesn't fit on the right opens left of its parent.
    parent_x: Option<f32>,
    rect: Rect,
}

/// A context menu with nested submenus (`overlays.menu(x, y, items, opts)`).
#[derive(Debug, Clone)]
pub struct ContextMenu<A> {
    id: String,
    items: Vec<MenuItem<A>>,
    pub dark: bool,
    /// Fixed width instead of fitting the labels.
    pub width: Option<f32>,
    levels: Vec<MenuLevel>,
}

impl<A: Clone> ContextMenu<A> {
    pub fn new(id: impl Into<String>, x: f32, y: f32, items: Vec<MenuItem<A>>) -> ContextMenu<A> {
        let root = MenuLevel { path: Vec::new(), x, y, parent_x: None, rect: Rect::default() };
        ContextMenu { id: id.into(), items, dark: false, width: None, levels: vec![root] }
    }

    pub fn dark(self, dark: bool) -> Self {
        ContextMenu { dark, ..self }
    }

    fn items_at(&self, path: &[usize]) -> &[MenuItem<A>] {
        let mut items = self.items.as_slice();
        for &index in path {
            items = &items[index].submenu;
        }
        items
    }

    /// The screen rect of a visible item by label (for scripted UI checks).
    pub fn item_rect(&self, label: &str) -> Option<Rect> {
        for level in self.levels.iter().rev() {
            let mut y = level.rect.y + 8.0;
            for item in self.items_at(&level.path) {
                if item.divider {
                    y += 9.0;
                    continue;
                }
                if item.label == label {
                    return Some(Rect::new(level.rect.x, y, level.rect.width, ITEM_HEIGHT));
                }
                y += ITEM_HEIGHT;
            }
        }
        None
    }

    /// Draws the menu (each level in its own layer) and reports a choice.
    pub fn show(&mut self, frame: &mut Frame) -> MenuResponse<A> {
        frame.ui.register_open_overlay(OpenOverlay::Menu { id: self.id.clone() });
        if frame.ui.take_overlay_key(&self.id) == Some(OverlayKey::Escape) {
            self.levels.pop();
            if self.levels.is_empty() {
                return MenuResponse::Closed;
            }
        }
        let mut level = 0;
        while level < self.levels.len() {
            let response = frame.overlay(|frame| self.draw_level(frame, level));
            if !matches!(response, MenuResponse::Open) {
                return response;
            }
            level += 1;
        }
        MenuResponse::Open
    }

    fn menu_width(&self, frame: &mut Frame, items: &[MenuItem<A>]) -> f32 {
        if let Some(width) = self.width {
            return width;
        }
        let mut width: f32 = 112.0;
        for item in items.iter().filter(|item| !item.divider) {
            let item_width = frame.text_width(&item.label, 14.0, Weight::Regular)
                + 32.0
                + if item.icon.is_some() { 36.0 } else { 0.0 }
                + if item.submenu.is_empty() { 0.0 } else { 28.0 }
                + if item.checked.is_some() { 32.0 } else { 0.0 };
            width = width.max(item_width);
        }
        width.min(420.0)
    }

    fn draw_level(&mut self, frame: &mut Frame, level: usize) -> MenuResponse<A> {
        let path = self.levels[level].path.clone();
        let items: Vec<MenuItem<A>> = self.items_at(&path).to_vec();
        let (screen_width, screen_height) = frame.ui.screen_size();
        let width = self.menu_width(frame, &items);
        let height = 16.0 + items.iter().map(|item| if item.divider { 9.0 } else { ITEM_HEIGHT }).sum::<f32>();
        let (mut x, mut y) = (self.levels[level].x, self.levels[level].y);
        if x + width > screen_width - 4.0 {
            x = self.levels[level].parent_x.map_or(screen_width - 4.0 - width, |parent_x| parent_x - width);
        }
        if y + height > screen_height - 4.0 {
            y = (screen_height - 4.0 - height).max(4.0);
        }
        self.levels[level].rect = Rect::new(x, y, width, height);
        if level == 0 {
            frame.ui.cover(-1e9, -1e9, 2e9, 2e9);
        } else {
            frame.ui.cover(x, y, width, height);
        }
        let palette = frame.palette();
        let background = if self.dark { Color::from_argb(0xFF30_3030) } else { palette.surface_container };
        let foreground = if self.dark { fixed::WHITE } else { palette.on_surface };
        frame.shadow(x, y, width, height, 4.0, 2.0);
        frame.fill_rect(x, y, width, height, 4.0, background);
        let mut item_y = y + 8.0;
        for (index, item) in items.iter().enumerate() {
            if item.divider {
                let color = if self.dark { Color::rgba(1.0, 1.0, 1.0, 0.12) } else { palette.outline_variant };
                frame.divider(x, item_y + 4.0, width, Some(color));
                item_y += 9.0;
                continue;
            }
            let region_id = format!("{}:menu{}:{}:{}", self.id, level + 1, index + 1, item.label);
            let options = if item.disabled { RegionOptions::default() } else { RegionOptions::hand() };
            let region = frame.ui.region(&region_id, x, item_y, width, ITEM_HEIGHT, options);
            let color = if item.disabled { with_alpha(foreground, 0.38) } else { foreground };
            if region.hovered && !item.disabled {
                frame.fill_rect(x, item_y, width, ITEM_HEIGHT, 0.0, with_alpha(foreground, 0.08));
            }
            let mut text_x = x + 16.0;
            if let Some(checked) = item.checked {
                if checked {
                    frame.icon("check", text_x, item_y + 9.0, 22.0, color);
                }
                text_x += 32.0;
            }
            if let Some(icon) = item.icon {
                frame.icon(icon, text_x, item_y + 8.0, 24.0, color);
                text_x += 36.0;
            }
            let has_submenu = !item.submenu.is_empty();
            let label_width = x + width - text_x - if has_submenu { 32.0 } else { 12.0 };
            let label_options = TextOptions::new().middle_of(ITEM_HEIGHT).color(color).width(label_width).ellipsis();
            frame.text(&item.label, text_x, item_y, label_options);
            if has_submenu {
                frame.icon("chevron_right", x + width - 32.0, item_y + 8.0, 24.0, color);
            }
            if !item.disabled {
                let mut item_path = path.clone();
                item_path.push(index);
                if has_submenu && (region.hovered || region.clicked) {
                    if self.levels.get(level + 1).is_none_or(|next| next.path != item_path) {
                        self.levels.truncate(level + 1);
                        self.levels.push(MenuLevel {
                            path: item_path,
                            x: x + width - 4.0,
                            y: item_y - 8.0,
                            parent_x: Some(x + 4.0),
                            rect: Rect::default(),
                        });
                    }
                } else if region.hovered && self.levels.len() > level + 1 && !has_submenu {
                    self.levels.truncate(level + 1);
                }
                if region.clicked && !has_submenu {
                    return match &item.action {
                        Some(action) => MenuResponse::Selected(action.clone()),
                        None => MenuResponse::Closed,
                    };
                }
            }
            item_y += ITEM_HEIGHT;
        }
        if level == 0 && (frame.ui.mouse_pressed(1) || frame.ui.mouse_pressed(2)) {
            // Close when clicking outside every open menu.
            let (mouse_x, mouse_y) = frame.ui.mouse();
            if !self.levels.iter().any(|level| level.rect.contains(mouse_x, mouse_y)) {
                return MenuResponse::Closed;
            }
        }
        MenuResponse::Open
    }
}
