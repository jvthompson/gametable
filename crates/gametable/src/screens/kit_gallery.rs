//! A development screen that shows every UI kit widget, for checking the kit against the
//! original by eye. Not part of the original app. `GAMETABLE_GALLERY_OPEN` (a comma-separated
//! list of `dialog`, `confirm`, `menu`, `picker`, `save`, `panel`) opens overlays at startup
//! for screenshots.

use gametable_core::models::{CardDefinition, GameDefinition, GameSet, TagGroup};

use crate::app::{Frame, Screen};
use crate::ui::card_filter_bar::{CardFilterState, LeadingButton};
use crate::ui::color_picker::{Hsv, PickerResponse};
use crate::ui::components::{AvatarSource, BannerKind, ChipOptions, Flow, Reorder, ToolbarAction, move_item};
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{
    ConfirmResponse, ContextMenu, DialogAction, DialogResponse, DialogSpec, MenuItem, MenuResponse,
};
use crate::ui::panel::{Pane, PanelLeading, clamp_selection};
use crate::ui::save_name_dialog::{ExistingName, SaveNameDialog, SaveNameResponse};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::widgets::{
    ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle, SliderOptions, ToggleOptions,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum MenuChoice {
    Rename,
    Delete,
    Color(u32),
}

pub struct KitGallery {
    name: String,
    password: String,
    notes: String,
    checked: bool,
    switched: bool,
    segment: u32,
    slider: f32,
    pick: Option<String>,
    show_dialog: bool,
    dialog_name: String,
    show_confirm: bool,
    menu: Option<ContextMenu<MenuChoice>>,
    picker: Option<Hsv>,
    save_dialog: Option<SaveNameDialog>,
    show_panel: bool,
    color: u32,
    tab: usize,
    chips: [bool; 3],
    order: Vec<String>,
    selected_item: Option<String>,
    game: GameDefinition,
    filters: CardFilterState,
    last_event: String,
}

fn sample_game() -> GameDefinition {
    let mut game = GameDefinition::new("sample", "Sample");
    for (id, name) in [("core", "Core Set"), ("expansion", "Expansion")] {
        game.sets.push(GameSet { id: id.into(), name: name.into(), pack_settings: Vec::new() });
    }
    game.tag_groups.push(TagGroup {
        id: "type".into(),
        name: "Type".into(),
        tags: ["Character", "Action", "Item", "Location"].map(String::from).to_vec(),
    });
    for (index, title) in ["Alpha", "Beta", "Gamma", "Delta"].iter().enumerate() {
        let mut card = CardDefinition::new(format!("c{index}"), *title);
        card.set_id = Some(if index % 2 == 0 { "core" } else { "expansion" }.into());
        card.extra_fields.insert("Cost", index as f64);
        card.extra_fields.insert("Strength", 2.0);
        game.cards_mut().push(card);
    }
    game
}

fn save_dialog() -> SaveNameDialog {
    SaveNameDialog::new(
        "gallery:save",
        "Save Game",
        "Save name",
        vec![
            ExistingName { name: "Friday night".into(), trailing: Some("2026-11-14 21:40".into()) },
            ExistingName { name: "Turn 3".into(), trailing: Some("2026-11-12 18:02".into()) },
        ],
        "Existing saves \u{2014} tap to reuse a name",
        "No saves yet",
        "Overwrite Save?",
        Box::new(|name| format!("A save named \"{name}\" already exists. Overwrite it?")),
        Box::new(|name| name == "Turn 3"),
    )
}

impl Default for KitGallery {
    fn default() -> Self {
        let game = sample_game();
        let filters = CardFilterState::new(&game);
        let open = std::env::var("GAMETABLE_GALLERY_OPEN").unwrap_or_default();
        let opens = |name: &str| open.split(',').any(|item| item.trim() == name);
        let menu = opens("menu").then(|| gallery_menu(560.0, 620.0));
        KitGallery {
            name: "Player One".to_string(),
            password: "secret".to_string(),
            notes: "A longer piece of text that wraps across several lines inside the text area so the caret can move up and down between them.".to_string(),
            checked: true,
            switched: false,
            segment: 2,
            slider: 0.4,
            pick: Some("lorcana".to_string()),
            show_dialog: opens("dialog"),
            dialog_name: String::new(),
            show_confirm: opens("confirm"),
            menu,
            picker: opens("picker").then(|| Hsv::from_argb(0xFF3B_6939)),
            save_dialog: opens("save").then(save_dialog),
            show_panel: opens("panel"),
            color: 0xFF3B_6939,
            tab: 0,
            chips: [true, false, false],
            order: ["First", "Second", "Third"].map(String::from).to_vec(),
            selected_item: None,
            game,
            filters,
            last_event: "Nothing yet".to_string(),
        }
    }
}

fn gallery_menu(x: f32, y: f32) -> ContextMenu<MenuChoice> {
    ContextMenu::new(
        "gallery:menu",
        x,
        y,
        vec![
            MenuItem::new("Rename", MenuChoice::Rename).icon("edit"),
            MenuItem::submenu(
                "Color",
                vec![
                    MenuItem::new("Red", MenuChoice::Color(0xFFF4_4336)).checked(true),
                    MenuItem::new("Blue", MenuChoice::Color(0xFF21_96F3)).checked(false),
                ],
            ),
            MenuItem::divider(),
            MenuItem::new("Delete", MenuChoice::Delete).icon("delete_outline"),
            MenuItem::new("Disabled", MenuChoice::Delete).disabled(true),
        ],
    )
}

impl KitGallery {
    fn draw_basics(&mut self, frame: &mut Frame, left: f32, top: f32) -> f32 {
        let mut row = top;
        frame.text("Buttons", left, row, TextOptions::new().size(18.0).bold());
        row += 32.0;
        let mut button_x = left;
        for (label, variant) in [
            ("Filled", ButtonVariant::Filled),
            ("Tonal", ButtonVariant::Tonal),
            ("Outlined", ButtonVariant::Outlined),
            ("Text", ButtonVariant::Text),
            ("Danger", ButtonVariant::Danger),
        ] {
            let options = ButtonOptions::variant(variant).tooltip("A tooltip after half a second");
            let button_width = frame.button_width(label, &options);
            if frame.button(label, button_x, row, Some(button_width), None, &options) {
                self.last_event = format!("Clicked {label}");
                frame.ui.toast(&format!("Clicked {label}"), None);
            }
            button_x += button_width + 8.0;
        }
        let disabled = ButtonOptions::default().icon("save").disabled(true);
        frame.button("Disabled", button_x, row, None, None, &disabled);
        button_x += frame.button_width("Disabled", &disabled) + 16.0;
        if frame.icon_button("settings", button_x, row, &IconButtonOptions::tooltip("Settings")) {
            self.last_event = "Settings icon".to_string();
        }
        frame.icon_button("delete_outline", button_x + 48.0, row, &IconButtonOptions::default().disabled(true));
        row += 64.0;

        frame.text("Toggles", left, row, TextOptions::new().size(18.0).bold());
        row += 40.0;
        self.checked = frame.checkbox("gallery:check", left, row, self.checked, &ToggleOptions::label("Checkbox"));
        frame.checkbox("gallery:check2", left + 160.0, row, true, &ToggleOptions::label("Disabled").disabled(true));
        self.switched =
            frame.switch("gallery:switch", left + 320.0, row - 7.0, self.switched, &ToggleOptions::default());
        row += 40.0;
        let choices = [Choice::new(1, "One"), Choice::new(2, "Two"), Choice::new(3, "Three")];
        self.segment = frame.segmented("gallery:segment", left, row, 360.0, &choices, self.segment, None, false);
        row += 56.0;
        let format = |value: f32| format!("{:.0}%", value * 100.0);
        (self.slider, _) = frame.slider_row(
            "gallery:slider",
            "Slider",
            left,
            row,
            360.0,
            self.slider,
            0.0,
            1.0,
            &SliderOptions::default(),
            Some(&format),
        );
        row += 72.0;
        frame.spinner(left + 16.0, row + 16.0, 14.0, None);
        frame.progress_bar(left + 48.0, row + 14.0, 200.0, 4.0, Some(self.slider), None);
        frame.progress_bar(left + 268.0, row + 14.0, 200.0, 4.0, None, None);
        row += 56.0;

        frame.text("Text fields", left, row, TextOptions::new().size(18.0).bold());
        row += 40.0;
        frame.text_field("gallery:name", left, row, 300.0, &mut self.name, &TextFieldOptions::label("Name"));
        let password = TextFieldOptions { password: true, ..TextFieldOptions::label("Password") };
        frame.text_field("gallery:password", left + 316.0, row, 240.0, &mut self.password, &password);
        row += 72.0;
        let error = TextFieldOptions {
            error: Some("Something is wrong"),
            hint: Some("Hint text"),
            ..TextFieldOptions::label("With error")
        };
        let mut error_text = String::new();
        frame.text_field("gallery:error", left, row, 300.0, &mut error_text, &error);
        (self.color, _) = frame.color_field("gallery:color", left + 316.0, row, 240.0, self.color, Some("Color"), None);
        row += 80.0;
        frame.text_area("gallery:notes", left, row, 400.0, 100.0, &mut self.notes, &TextFieldOptions::label("Notes"));
        let games = [
            Choice::new(Some("lorcana".to_string()), "Lorcana"),
            Choice::new(Some("middle_earth".to_string()), "Middle Earth"),
            Choice::new(None, "None"),
        ];
        let dropdown_options = DropdownOptions { label: Some("Game"), ..DropdownOptions::default() };
        self.pick =
            frame.dropdown("gallery:dropdown", left + 416.0, row, 240.0, &games, self.pick.clone(), &dropdown_options);
        row += 124.0;

        frame.text("Overlays", left, row, TextOptions::new().size(18.0).bold());
        row += 32.0;
        let mut flow = Flow::new(left, row, 680.0, 40.0, 8.0);
        let mut overlay_button = |frame: &mut Frame, label: &str, variant: ButtonVariant| {
            let options = ButtonOptions::variant(variant);
            let width = frame.button_width(label, &options);
            let (button_x, button_y) = flow.place(width);
            frame.button(label, button_x, button_y, Some(width), None, &options)
        };
        if overlay_button(frame, "Open dialog", ButtonVariant::Filled) {
            self.show_dialog = true;
        }
        if overlay_button(frame, "Confirm", ButtonVariant::Outlined) {
            self.show_confirm = true;
        }
        if overlay_button(frame, "Menu", ButtonVariant::Tonal) {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            self.menu = Some(gallery_menu(mouse_x, mouse_y));
        }
        if overlay_button(frame, "Save dialog", ButtonVariant::Tonal) {
            self.save_dialog = Some(save_dialog());
        }
        if overlay_button(frame, "Panel", ButtonVariant::Tonal) {
            self.show_panel = true;
        }
        if overlay_button(frame, "Toast", ButtonVariant::Text) {
            frame.ui.toast("This is a snackbar message", None);
        }
        row = flow.bottom() + 16.0;
        let palette = frame.palette();
        let status = format!("Last event: {}", self.last_event);
        frame.text(&status, left, row, TextOptions::new().color(palette.on_surface_variant));
        row + 40.0
    }

    fn draw_components(&mut self, frame: &mut Frame, left: f32, top: f32, width: f32) -> f32 {
        let mut row = top;
        frame.text("Components", left, row, TextOptions::new().size(18.0).bold());
        row += 32.0;
        let actions = [
            ToolbarAction { icon: "save", tooltip: "Save", id: None, disabled: false },
            ToolbarAction { icon: "refresh", tooltip: "Reload", id: None, disabled: true },
        ];
        if let Some(index) = frame.editor_toolbar(left, row, width, "Editor toolbar", &actions) {
            self.last_event = format!("Toolbar action {index}");
        }
        row += 64.0;
        self.tab = frame.tab_bar("gallery:tabs", left, row, width, &["General", "Cards", "Tokens"], self.tab);
        row += 60.0;
        let mut chip_x = left;
        for (index, label) in ["Selected", "Plain", "With icon"].iter().enumerate() {
            let options = ChipOptions { icon: (index == 2).then_some("style"), ..ChipOptions::default() };
            let (clicked, chip_width) = frame.chip(label, chip_x, row, self.chips[index], &options);
            if clicked {
                self.chips[index] = !self.chips[index];
            }
            chip_x += chip_width + 8.0;
        }
        row += 48.0;
        let (banner_height, _) = frame.banner(
            "A warning banner that wraps when the text is long enough to need it.",
            left,
            row,
            width,
            BannerKind::Warning,
            false,
        );
        row += banner_height + 8.0;
        let (banner_height, clicked) =
            frame.banner("An info banner you can click.", left, row, width, BannerKind::Info, true);
        if clicked {
            self.last_event = "Banner".to_string();
        }
        row += banner_height + 16.0;
        frame.avatar(left, row, 100.0, 0xFF45_5A64, None, true);
        frame.avatar(left + 116.0, row, 48.0, 0xFFE9_1E63, None, true);
        let icon_path = frame.assets_folder.join("icon.png").to_string_lossy().into_owned();
        frame.avatar(left + 180.0, row, 48.0, 0xFF21_96F3, Some(AvatarSource::Path(&icon_path)), true);
        let mut avatar = |frame: &mut Frame, x: f32, y: f32| frame.avatar(x, y, 32.0, 0xFF4C_AF50, None, true);
        let (_, clicked) =
            frame.header_pill("gallery:pill", left + width, row, "Header pill", Some("Profile"), Some(&mut avatar));
        if clicked {
            self.last_event = "Header pill".to_string();
        }
        row += 116.0;
        frame.heading("Drag to reorder", left, row, None);
        row += 24.0;
        let reorder = Reorder::begin(frame, "gallery:reorder");
        for (index, label) in self.order.iter().enumerate() {
            let row_y = row + index as f32 * 44.0;
            frame.card(left, row_y, 300.0, 40.0, Some(8.0), None, true);
            reorder.handle(frame, index, left, row_y, row_y, 40.0);
            frame.text(label, left + 44.0, row_y, TextOptions::new().middle_of(40.0));
        }
        if let Some((from, to)) = reorder.finish(frame, left, 300.0) {
            move_item(&mut self.order, from, to);
        }
        row += self.order.len() as f32 * 44.0 + 16.0;

        frame.heading("Card filter bar", left, row, None);
        row += 12.0;
        let leading =
            [LeadingButton { label: "Add", icon: Some("add"), id: "gallery:add", variant: ButtonVariant::Tonal }];
        let (bottom, _) =
            frame.card_filters("gallery:filters", &mut self.filters, &self.game, left, row, width, true, &leading);
        row = bottom + 12.0;
        let background = frame.palette().surface;
        row = frame.card_search(
            "gallery:filters",
            &mut self.filters,
            &self.game,
            left,
            row,
            width,
            Some(background),
            None,
        );
        row += 8.0;
        let visible: Vec<String> =
            self.filters.visible(&self.game).iter().map(|card| card.card_title.clone()).collect();
        frame.text(&format!("Visible: {}", visible.join(", ")), left, row, TextOptions::new().width(width));
        row += 32.0;

        frame.heading("List / detail", left, row, None);
        row += 24.0;
        let items: Vec<String> = ["Zones", "Sets", "Tokens"].map(String::from).to_vec();
        clamp_selection(&mut self.selected_item, &items);
        let selected = self.selected_item.clone();
        let mut picked = None;
        frame.card(left, row, width, 140.0, Some(0.0), None, true);
        frame.list_detail(left, row, width, 140.0, 160.0, selected.is_some(), "Nothing selected", |frame, pane| {
            match pane {
                Pane::List(area) => {
                    for (index, item) in items.iter().enumerate() {
                        let item_y = area.y + index as f32 * 40.0;
                        let region = frame.ui.region(
                            &format!("gallery:item{index}"),
                            area.x,
                            item_y,
                            area.width,
                            40.0,
                            Default::default(),
                        );
                        if selected.as_ref() == Some(item) {
                            let color = crate::ui::theme::with_alpha(frame.palette().primary, 0.12);
                            frame.fill_rect(area.x, item_y, area.width, 40.0, 0.0, color);
                        }
                        frame.text(item, area.x + 16.0, item_y, TextOptions::new().middle_of(40.0));
                        if region.clicked {
                            picked = Some(item.clone());
                        }
                    }
                }
                Pane::Detail(area) => {
                    let detail = format!("Details of {}", selected.clone().unwrap_or_default());
                    frame.text(&detail, area.x + 16.0, area.y + 16.0, TextOptions::new().size(16.0));
                }
            }
        });
        if picked.is_some() {
            self.selected_item = picked;
        }
        row + 160.0
    }

    fn draw_overlays(&mut self, frame: &mut Frame) {
        if self.show_dialog {
            let actions = [DialogAction::text("Cancel"), DialogAction::filled("Save")];
            let spec = DialogSpec {
                title: Some("Name something"),
                message: Some("Dialogs scroll their content when the window is too short."),
                actions: &actions,
                handles_enter: true,
                ..DialogSpec::default()
            };
            let dialog_name = &mut self.dialog_name;
            let response = frame.dialog("gallery:dialog", &spec, |frame, x, y, width| {
                let options = TextFieldOptions {
                    autofocus: true,
                    label_background: Some(frame.palette().surface_container_high),
                    ..TextFieldOptions::label("Name")
                };
                frame.text_field("gallery:dialog:name", x, y + 16.0, width, dialog_name, &options);
                72.0
            });
            match response {
                DialogResponse::Open => {}
                DialogResponse::Action(1) | DialogResponse::Enter => {
                    self.last_event = format!("Saved \"{}\"", self.dialog_name);
                    self.show_dialog = false;
                }
                _ => {
                    self.last_event = "Dialog dismissed".to_string();
                    self.show_dialog = false;
                }
            }
        }
        if self.show_confirm {
            match frame.confirm_dialog(
                "gallery:confirm",
                "Delete everything?",
                "This cannot be undone.",
                "Delete",
                true,
            ) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Confirmed => {
                    self.last_event = "Confirmed".to_string();
                    self.show_confirm = false;
                }
                ConfirmResponse::Cancelled => {
                    self.last_event = "Cancelled".to_string();
                    self.show_confirm = false;
                }
            }
        }
        if let Some(picker) = &mut self.picker {
            match frame.color_picker_dialog("gallery:picker", picker) {
                PickerResponse::Open => {}
                PickerResponse::Picked(argb) => {
                    self.color = argb;
                    self.picker = None;
                }
                PickerResponse::Cancelled => self.picker = None,
            }
        }
        if let Some(dialog) = &mut self.save_dialog {
            match dialog.show(frame, None) {
                SaveNameResponse::Open => {}
                SaveNameResponse::Save(name) => {
                    self.last_event = format!("Save as \"{name}\"");
                    self.save_dialog = None;
                }
                SaveNameResponse::Cancelled => self.save_dialog = None,
            }
        }
        if self.show_panel {
            let response = frame.overlay(|frame| {
                let leading = PanelLeading { icon: "menu_book", tooltip: "Contents" };
                let response =
                    frame.panel("gallery:panel", 640.0, 420.0, "Game Log", Some("42 entries"), Some(leading));
                let content = response.content;
                let text = "Panels are the dark full-screen chrome used in game.";
                frame.text(
                    text,
                    content.x + 16.0,
                    content.y + 16.0,
                    TextOptions::new().color(crate::ui::theme::fixed::WHITE),
                );
                response
            });
            if response.close_requested {
                self.show_panel = false;
            }
        }
        if let Some(menu) = &mut self.menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Selected(choice) => {
                    self.last_event = format!("Menu: {choice:?}");
                    self.menu = None;
                }
                MenuResponse::Closed => self.menu = None,
            }
        }
    }
}

impl Screen for KitGallery {
    fn title(&self) -> String {
        "UI Kit Gallery".to_string()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let offset = frame.begin_scroll("gallery", x, y, width, height);
        let left_bottom = self.draw_basics(frame, x + 32.0, y + 24.0 - offset);
        let right_left = x + 32.0 + 720.0;
        let right_width = (width - 720.0 - 64.0).max(320.0);
        let right_bottom = self.draw_components(frame, right_left, y + 24.0 - offset, right_width);
        frame.end_scroll("gallery", left_bottom.max(right_bottom) + offset - y, &ScrollbarStyle::default());
        self.draw_overlays(frame);
    }
}
