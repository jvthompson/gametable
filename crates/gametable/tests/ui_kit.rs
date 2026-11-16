//! Drives the UI kit through real frames with simulated mouse and keyboard input, the way the
//! app shell routes SDL events: text editing, focus, dialogs, menus, dropdowns and scrolling.

use gametable::app::Frame;
use gametable::app::harness::UiHarness;
use gametable::ui::dropdown::DropdownOptions;
use gametable::ui::overlays::{ContextMenu, DialogAction, DialogResponse, DialogSpec, MenuItem, MenuResponse};
use gametable::ui::text_field::{TextFieldOptions, TextFieldResponse};
use gametable::ui::widgets::{Choice, ScrollbarStyle, ToggleOptions};

/// Moves there, presses and releases the left button, one frame each.
fn click(harness: &mut UiHarness, x: f32, y: f32, draw: &mut dyn FnMut(&mut Frame)) {
    harness.move_mouse(x, y);
    harness.frame(|frame| draw(frame));
    harness.press(x, y, 1, 1);
    harness.frame(|frame| draw(frame));
    harness.release(x, y, 1);
    harness.frame(|frame| draw(frame));
}

fn field(frame: &mut Frame, id: &str, y: f32, text: &mut String, options: &TextFieldOptions) -> TextFieldResponse {
    frame.text_field(id, 100.0, y, 300.0, text, options)
}

#[test]
fn text_field_editing() {
    let mut harness = UiHarness::new();
    let mut text = "hello world".to_string();
    let mut submitted = false;
    let options = TextFieldOptions::label("Name");
    let mut draw = |frame: &mut Frame| submitted |= field(frame, "name", 100.0, &mut text, &options).submitted;
    // Clicking right of the text focuses the field with the caret at the end.
    click(&mut harness, 390.0, 124.0, &mut draw);
    assert_eq!(harness.ui.focus.as_deref(), Some("name"));

    assert!(harness.type_text(" again"));
    harness.frame(&mut draw);
    harness.hold("lctrl");
    harness.key("backspace");
    harness.frame(&mut draw);
    harness.key("a");
    harness.frame(&mut draw);
    harness.key("c");
    harness.frame(&mut draw);
    harness.let_go("lctrl");
    assert_eq!(harness.clipboard.contents, "hello world ");

    // Typing replaces the selection; Shift+Left selects back one character.
    harness.type_text("abc");
    harness.frame(&mut draw);
    harness.hold("lshift");
    harness.key("left");
    harness.frame(&mut draw);
    harness.let_go("lshift");
    harness.type_text("Z");
    harness.frame(&mut draw);
    harness.key("home");
    harness.frame(&mut draw);
    harness.key("delete");
    harness.frame(&mut draw);
    harness.key("return");
    harness.frame(&mut draw);
    harness.key("escape");
    harness.frame(&mut draw);
    assert!(submitted);
    assert_eq!(text, "bZ");
    assert_eq!(harness.ui.focus, None, "Escape blurs");
}

#[test]
fn paste_max_length_and_filter() {
    let mut harness = UiHarness::new();
    let mut digits = String::new();
    let only_digits = |inserted: &str| inserted.chars().all(|character| character.is_ascii_digit());
    let options = TextFieldOptions { max_length: Some(4), filter: Some(&only_digits), ..TextFieldOptions::default() };
    let mut draw = |frame: &mut Frame| {
        field(frame, "digits", 100.0, &mut digits, &options);
    };
    click(&mut harness, 200.0, 124.0, &mut draw);
    harness.type_text("12");
    harness.type_text("x");
    harness.frame(&mut draw);
    harness.clipboard.contents = "345\r\n6".to_string();
    harness.hold("lctrl");
    harness.key("v");
    harness.frame(&mut draw);
    harness.let_go("lctrl");
    harness.clipboard.contents = "789".to_string();
    harness.hold("rctrl");
    harness.key("v");
    harness.frame(&mut draw);
    harness.let_go("rctrl");
    // "345 6" fails the filter (it has a space); "789" is cut to the 2 characters left.
    assert_eq!(digits, "1278");
}

#[test]
fn tab_moves_focus_and_selects_the_next_field() {
    let mut harness = UiHarness::new();
    let (mut first, mut second) = ("one".to_string(), "two".to_string());
    let options = TextFieldOptions::default();
    let mut draw = |frame: &mut Frame| {
        field(frame, "first", 100.0, &mut first, &options);
        field(frame, "second", 200.0, &mut second, &options);
    };
    click(&mut harness, 200.0, 124.0, &mut draw);
    harness.key("tab");
    harness.frame(&mut draw);
    assert_eq!(harness.ui.focus.as_deref(), Some("second"));
    harness.type_text("2");
    harness.frame(&mut draw);
    harness.hold("lshift");
    harness.key("tab");
    harness.frame(&mut draw);
    harness.let_go("lshift");
    assert_eq!(harness.ui.focus.as_deref(), Some("first"));
    assert_eq!(second, "2", "Tab selected the whole field, so typing replaced it");
    // Clicking outside every field unfocuses.
    let mut draw = |frame: &mut Frame| {
        field(frame, "first", 100.0, &mut first, &options);
    };
    click(&mut harness, 900.0, 600.0, &mut draw);
    assert_eq!(harness.ui.focus, None);
}

#[test]
fn checkbox_toggles_on_click() {
    let mut harness = UiHarness::new();
    let mut checked = false;
    let mut draw =
        |frame: &mut Frame| checked = frame.checkbox("check", 100.0, 100.0, checked, &ToggleOptions::label("Check"));
    click(&mut harness, 108.0, 108.0, &mut draw);
    assert!(checked);
}

fn show_dialog(frame: &mut Frame, response: &mut Option<DialogResponse>) {
    let actions = [DialogAction::text("Cancel"), DialogAction::filled("Save")];
    let spec = DialogSpec { title: Some("Title"), actions: &actions, handles_enter: true, ..DialogSpec::default() };
    let shown = frame.dialog("dialog", &spec, |_, _, _, _| 0.0);
    if shown != DialogResponse::Open {
        *response = Some(shown);
    }
}

#[test]
fn dialog_keys_scrim_and_actions() {
    // Escape dismisses, Enter is reported.
    for (key, expected) in [("escape", DialogResponse::Dismissed), ("return", DialogResponse::Enter)] {
        let mut harness = UiHarness::new();
        let mut response = None;
        harness.frame(|frame| show_dialog(frame, &mut response));
        assert!(harness.key(key), "an open dialog takes the key");
        harness.frame(|frame| show_dialog(frame, &mut response));
        assert_eq!(response, Some(expected));
    }

    // The dialog is 400×152, centred: (440, 284). Its last action ends 24 left of the edge.
    let mut harness = UiHarness::new();
    let mut response = None;
    let mut draw = |frame: &mut Frame| show_dialog(frame, &mut response);
    click(&mut harness, 806.0, 392.0, &mut draw);
    assert_eq!(response, Some(DialogResponse::Action(1)));

    let mut harness = UiHarness::new();
    let mut response = None;
    let mut draw = |frame: &mut Frame| show_dialog(frame, &mut response);
    click(&mut harness, 100.0, 100.0, &mut draw);
    assert_eq!(response, Some(DialogResponse::Dismissed), "a scrim click dismisses");
}

#[test]
fn a_dialog_blocks_the_screen_below() {
    let mut harness = UiHarness::new();
    let mut checked = false;
    let mut draw = |frame: &mut Frame| {
        checked = frame.checkbox("check", 100.0, 100.0, checked, &ToggleOptions::default());
        let spec = DialogSpec { dismissible: false, ..DialogSpec::default() };
        frame.dialog("modal", &spec, |_, _, _, _| 0.0);
    };
    click(&mut harness, 108.0, 108.0, &mut draw);
    assert!(!checked);
}

#[derive(Debug, Clone, PartialEq)]
enum Choice2 {
    Rename,
    Red,
}

#[test]
fn context_menu_submenu_and_escape() {
    let items = || {
        vec![
            MenuItem::new("Rename", Choice2::Rename),
            MenuItem::submenu("Color", vec![MenuItem::new("Red", Choice2::Red)]),
        ]
    };
    let mut harness = UiHarness::new();
    let mut menu = ContextMenu::new("menu", 100.0, 100.0, items());
    let mut response = MenuResponse::Open;
    // Hovering "Color" opens its submenu to the right (the root is 112 wide: x 208).
    harness.move_mouse(150.0, 160.0);
    harness.frame(|frame| response = menu.show(frame));
    harness.frame(|frame| response = menu.show(frame));
    let red = menu.item_rect("Red").expect("submenu open");
    assert_eq!((red.x, red.y), (208.0, 148.0));
    let mut draw = |frame: &mut Frame| {
        if let MenuResponse::Open = response {
            response = menu.show(frame);
        }
    };
    click(&mut harness, 240.0, 160.0, &mut draw);
    assert_eq!(response, MenuResponse::Selected(Choice2::Red));

    // Escape closes the submenu first, then the menu.
    let mut menu = ContextMenu::new("menu", 100.0, 100.0, items());
    harness.move_mouse(150.0, 160.0);
    harness.frame(|frame| response = menu.show(frame));
    harness.frame(|frame| response = menu.show(frame));
    harness.key("escape");
    harness.frame(|frame| response = menu.show(frame));
    assert_eq!(response, MenuResponse::Open);
    harness.move_mouse(150.0, 120.0);
    harness.frame(|frame| response = menu.show(frame));
    harness.key("escape");
    harness.frame(|frame| response = menu.show(frame));
    assert_eq!(response, MenuResponse::Closed);

    // A click outside closes it.
    let mut menu = ContextMenu::new("menu", 100.0, 100.0, items());
    let mut response = MenuResponse::Open;
    let mut draw = |frame: &mut Frame| {
        if let MenuResponse::Open = response {
            response = menu.show(frame);
        }
    };
    click(&mut harness, 700.0, 500.0, &mut draw);
    assert_eq!(response, MenuResponse::Closed);
}

#[test]
fn dropdown_picks_an_option() {
    let mut harness = UiHarness::new();
    let choices = [Choice::new(1, "One"), Choice::new(2, "Two"), Choice::new(3, "Three")];
    let mut selected = 1;
    let mut draw = |frame: &mut Frame| {
        selected = frame.dropdown("pick", 100.0, 100.0, 200.0, &choices, selected, &DropdownOptions::default());
    };
    click(&mut harness, 150.0, 120.0, &mut draw);
    // The popup starts at y 150; option 2 spans 198..238.
    click(&mut harness, 150.0, 210.0, &mut draw);
    harness.frame(&mut draw);
    assert_eq!(selected, 2);
}

#[test]
fn wheel_scrolls_a_hovered_view() {
    let mut harness = UiHarness::new();
    let mut draw = |frame: &mut Frame| {
        frame.begin_scroll("list", 100.0, 100.0, 300.0, 200.0);
        frame.end_scroll("list", 1000.0, &ScrollbarStyle::default());
    };
    harness.move_mouse(200.0, 200.0);
    harness.frame(&mut draw);
    harness.ui.on_wheel(0.0, -2.0);
    for _ in 0..30 {
        harness.frame(&mut draw);
    }
    assert_eq!(harness.ui.scroll_offset("list"), 120.0);
    harness.ui.on_wheel(0.0, -100.0);
    for _ in 0..30 {
        harness.frame(&mut draw);
    }
    assert_eq!(harness.ui.scroll_offset("list"), 800.0, "clamped to content minus view");
}

#[test]
fn app_alerts_and_toasts() {
    let mut harness = UiHarness::new();
    harness.ui.alert("Disconnected", "The host left.");
    harness.ui.toast("Saved", Some(1.0));
    harness.frame(|_| {});
    assert!(harness.ui.any_overlay_open());
    assert!(harness.key("escape"));
    harness.frame(|_| {});
    harness.frame(|_| {});
    assert!(!harness.ui.any_overlay_open(), "Escape closed the alert");
}
