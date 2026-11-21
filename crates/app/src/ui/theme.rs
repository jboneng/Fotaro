//! Dark charcoal look in the spirit of Lightroom Classic: neutral greys so photos
//! are judged against a colourless surround, low-contrast chrome, compact text.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};

/// Top (identity plate / module picker) bar.
pub const TOP_BAR: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
/// Side panels, filmstrip and toolbars.
pub const PANEL: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// Panel section headers.
pub const SECTION_HEADER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
/// Behind the grid / loupe.
pub const CENTER: Color32 = Color32::from_rgb(0x45, 0x45, 0x45);
/// Grid cells.
pub const CELL: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x5a);
pub const CELL_SELECTED: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const CELL_ACTIVE: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
pub const CELL_INDEX: Color32 = Color32::from_rgba_premultiplied(0x30, 0x30, 0x30, 0x90);
pub const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xec, 0xec, 0xec);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x8c, 0x8c, 0x8c);
pub const MODULE_ACTIVE: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const PICK: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
pub const REJECT: Color32 = Color32::from_rgb(0x15, 0x15, 0x15);
pub const STAR: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// The menu bar's update indicator.
pub const UPDATE: Color32 = Color32::from_rgb(0x6c, 0xb4, 0xe6);
pub const MISSING: Color32 = Color32::from_rgb(0xd0, 0x8a, 0x2a);

pub fn label_color(label: &str) -> Option<Color32> {
    Some(match label {
        "red" => Color32::from_rgb(0xb8, 0x3c, 0x3c),
        "yellow" => Color32::from_rgb(0xc8, 0xb4, 0x3c),
        "green" => Color32::from_rgb(0x5a, 0x9a, 0x50),
        "blue" => Color32::from_rgb(0x46, 0x6e, 0xbe),
        "purple" => Color32::from_rgb(0x8a, 0x5a, 0xb0),
        _ => return None,
    })
}

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(0x2e, 0x2e, 0x2e);
    v.extreme_bg_color = Color32::from_rgb(0x24, 0x24, 0x24);
    v.faint_bg_color = Color32::from_rgb(0x38, 0x38, 0x38);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = TEXT_STRONG;
    v.selection.bg_fill = Color32::from_rgb(0x5c, 0x5c, 0x5c);
    v.selection.stroke = Stroke::new(1.0, TEXT_STRONG);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x1a, 0x1a, 0x1a));
    v.window_corner_radius = CornerRadius::same(3);
    v.menu_corner_radius = CornerRadius::same(2);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x26, 0x26, 0x26));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = Color32::from_rgb(0x42, 0x42, 0x42);
    w.inactive.weak_bg_fill = Color32::from_rgb(0x3c, 0x3c, 0x3c);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = Color32::from_rgb(0x52, 0x52, 0x52);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x4a, 0x4a, 0x4a);
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
    w.active.bg_fill = Color32::from_rgb(0x60, 0x60, 0x60);
    w.active.weak_bg_fill = Color32::from_rgb(0x58, 0x58, 0x58);
    w.active.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(2);
    }
    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(10.0)),
            (TextStyle::Body, FontId::proportional(12.5)),
            (TextStyle::Button, FontId::proportional(12.5)),
            (TextStyle::Heading, FontId::proportional(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 3.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.spacing.interact_size.y = 18.0;
    });
}
