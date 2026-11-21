//! LrC's Library Filter bar: Text | Attribute | Metadata | None, above the Grid.
//! Click a mode to show only it; Shift-click to combine modes (as in LrC).

use catalog::{FileKind, Flag, MetaColumn, NumOp, TextField, TextOp};
use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke};

use super::{icons, theme};
use crate::app::FotaroApp;
use crate::filter::{COLUMNS, FilterBar};

/// Picks one of the bar's on/off switches.
type ModeSwitch = fn(&mut FilterBar) -> &mut bool;

const ROW: f32 = 28.0;
const META_ROW: f32 = 156.0;

/// Height of the bar for the rows currently open.
pub fn height(f: &FilterBar) -> f32 {
    26.0 + if f.text_on { ROW } else { 0.0 }
        + if f.attribute_on { ROW } else { 0.0 }
        + if f.metadata_on { META_ROW } else { 0.0 }
}

fn mode_button(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let color = if on { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
    ui.add(egui::Button::new(RichText::new(text).color(color)).frame(false).selected(on))
}

pub fn show(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let mut f = app.ui.filter.clone();
    let focus_text = std::mem::take(&mut app.focus_find);
    let shift = ui.input(|i| i.modifiers.shift);

    // Header: Library Filter : Text Attribute Metadata None ... Filters On/Off
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 26.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space((ui.available_width() / 2.0 - 170.0).max(0.0));
            ui.label(RichText::new("Library Filter :").color(theme::TEXT_WEAK));
            let modes: [(&str, ModeSwitch); 3] = [
                ("Text", |f| &mut f.text_on),
                ("Attribute", |f| &mut f.attribute_on),
                ("Metadata", |f| &mut f.metadata_on),
            ];
            for (name, flag) in modes {
                let on = *flag(&mut f);
                if mode_button(ui, name, on).clicked() {
                    if shift {
                        *flag(&mut f) = !on;
                    } else {
                        let only_this = on
                            && [f.text_on, f.attribute_on, f.metadata_on].iter().filter(|b| **b).count() == 1;
                        f.none();
                        *flag(&mut f) = !only_this;
                    }
                    f.enabled = true;
                }
            }
            let none = !(f.text_on || f.attribute_on || f.metadata_on);
            if mode_button(ui, "None", none).clicked() {
                f.none();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, color) = if !f.enabled {
                    ("Filters Off", theme::TEXT_WEAK)
                } else if f.is_filtering() {
                    ("Filters On", theme::TEXT_STRONG)
                } else {
                    ("No Filter", theme::TEXT_WEAK)
                };
                if ui
                    .add(egui::Button::new(RichText::new(label).color(color)).frame(false))
                    .on_hover_text("Enable / disable filters (Ctrl+L)")
                    .clicked()
                {
                    f.enabled ^= true;
                }
            });
        },
    );

    let dim = !f.enabled;
    if f.text_on {
        row(ui, dim, |ui| text_row(ui, &mut f, focus_text));
    }
    if f.attribute_on {
        row(ui, dim, |ui| attribute_row(ui, &mut f));
    }
    if f.metadata_on {
        let bar = f.clone();
        let facets = app.session.as_mut().map(|s| s.facets(&bar).to_vec()).unwrap_or_default();
        ui.allocate_ui(egui::vec2(ui.available_width(), META_ROW), |ui| {
            if dim {
                ui.disable();
            }
            metadata_row(ui, &mut f, &facets);
        });
    }
    app.ui.filter = f;
}

fn row(ui: &mut egui::Ui, dim: bool, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ROW),
        Layout::left_to_right(Align::Center),
        |ui| {
            if dim {
                ui.disable();
            }
            ui.add_space((ui.available_width() / 2.0 - 330.0).max(8.0));
            add(ui);
        },
    );
}

fn text_row(ui: &mut egui::Ui, f: &mut FilterBar, focus: bool) {
    ui.label(RichText::new("Text").color(theme::TEXT_WEAK));
    egui::ComboBox::from_id_salt("text_field").width(150.0).selected_text(f.text_field.label()).show_ui(
        ui,
        |ui| {
            for t in TextField::ALL {
                ui.selectable_value(&mut f.text_field, t, t.label());
            }
        },
    );
    egui::ComboBox::from_id_salt("text_op").width(110.0).selected_text(f.text_op.label()).show_ui(ui, |ui| {
        for o in TextOp::ALL {
            ui.selectable_value(&mut f.text_op, o, o.label());
        }
    });
    let resp = ui.add(egui::TextEdit::singleline(&mut f.text).hint_text("Search").desired_width(220.0));
    if focus {
        resp.request_focus();
    }
    if !f.text.is_empty() && ui.small_button("✕").on_hover_text("Clear").clicked() {
        f.text.clear();
    }
}

fn toggle<T: PartialEq + Clone>(list: &mut Vec<T>, value: T) {
    if let Some(i) = list.iter().position(|v| *v == value) {
        list.remove(i);
    } else {
        list.push(value);
    }
}

fn attribute_row(ui: &mut egui::Ui, f: &mut FilterBar) {
    ui.label(RichText::new("Flag").color(theme::TEXT_WEAK));
    for (flag, tip) in
        [(Flag::Picked, "Flagged"), (Flag::Unflagged, "Unflagged"), (Flag::Rejected, "Rejected")]
    {
        let on = f.flags.contains(&flag);
        let (resp, rect) = icons::icon_button(ui, egui::vec2(20.0, 20.0), on);
        let (fill, outline) = match flag {
            Flag::Picked => (theme::TEXT_STRONG, theme::TEXT_STRONG),
            Flag::Unflagged => (Color32::TRANSPARENT, theme::TEXT),
            Flag::Rejected => (Color32::from_rgb(0x20, 0x20, 0x20), theme::TEXT),
        };
        icons::flag(
            ui.painter(),
            rect.left_center() + egui::vec2(6.0, 0.0),
            12.0,
            fill,
            outline,
            flag == Flag::Rejected,
        );
        if resp.on_hover_text(tip).clicked() {
            toggle(&mut f.flags, flag);
        }
    }

    ui.add_space(14.0);
    ui.label(RichText::new("Rating").color(theme::TEXT_WEAK));
    egui::ComboBox::from_id_salt("rating_op").width(36.0).selected_text(f.rating_op.label()).show_ui(
        ui,
        |ui| {
            for o in NumOp::ALL {
                ui.selectable_value(&mut f.rating_op, o, o.label());
            }
        },
    );
    for stars in 1..=5u8 {
        let (resp, rect) = icons::icon_button(ui, egui::vec2(15.0, 20.0), false);
        let lit = stars <= f.rating;
        icons::star(
            ui.painter(),
            rect.center(),
            5.5,
            lit,
            if lit { theme::TEXT_STRONG } else { theme::TEXT_WEAK },
        );
        if resp.on_hover_text(format!("{} {stars}", f.rating_op.label())).clicked() {
            f.rating = if f.rating == stars { 0 } else { stars };
        }
    }

    ui.add_space(14.0);
    ui.label(RichText::new("Color").color(theme::TEXT_WEAK));
    for label in ["red", "yellow", "green", "blue", "purple"] {
        let on = f.labels.contains(&Some(label.to_owned()));
        let (resp, rect) = icons::icon_button(ui, egui::vec2(16.0, 20.0), on);
        let swatch = egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0));
        ui.painter().rect_filled(swatch, 1.0, theme::label_color(label).unwrap_or_default());
        if resp.on_hover_text(label).clicked() {
            toggle(&mut f.labels, Some(label.to_owned()));
        }
    }
    let on = f.labels.contains(&None);
    let (resp, rect) = icons::icon_button(ui, egui::vec2(16.0, 20.0), on);
    let swatch = egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0));
    ui.painter().rect_stroke(swatch, 1.0, Stroke::new(1.0, theme::TEXT_WEAK), egui::StrokeKind::Inside);
    if resp.on_hover_text("No label").clicked() {
        toggle(&mut f.labels, None);
    }

    ui.add_space(14.0);
    ui.label(RichText::new("Kind").color(theme::TEXT_WEAK));
    for (kind, name) in [(FileKind::Raw, "Raw"), (FileKind::Image, "Other")] {
        let on = f.kinds.contains(&kind);
        if ui.add(egui::Button::new(RichText::new(name).size(11.5)).selected(on)).clicked() {
            toggle(&mut f.kinds, kind);
        }
    }
}

fn metadata_row(ui: &mut egui::Ui, f: &mut FilterBar, facets: &[Vec<catalog::Facet>]) {
    let width = (ui.available_width() - 8.0) / COLUMNS as f32;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for i in 0..COLUMNS {
            ui.allocate_ui_with_layout(
                egui::vec2(width - 2.0, META_ROW - 4.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(width - 2.0);
                    egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x2a, 0x2a)).inner_margin(4.0).show(
                        ui,
                        |ui| {
                            ui.set_min_size(egui::vec2(width - 10.0, META_ROW - 12.0));
                            let column = f.columns[i];
                            egui::ComboBox::from_id_salt(("meta_column", i))
                                .width(width - 20.0)
                                .selected_text(RichText::new(column.label()).strong())
                                .show_ui(ui, |ui| {
                                    for c in MetaColumn::ALL {
                                        if ui.selectable_label(c == column, c.label()).clicked()
                                            && c != column
                                        {
                                            f.columns[i] = c;
                                            f.picks[i].clear();
                                        }
                                    }
                                });
                            let values = facets.get(i).map(Vec::as_slice).unwrap_or_default();
                            egui::ScrollArea::vertical()
                                .id_salt(("meta_scroll", i))
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let all = f.picks[i].is_empty();
                                    if value_row(ui, &format!("All ({})", values.len()), None, all).clicked()
                                    {
                                        f.picks[i].clear();
                                    }
                                    let ctrl = ui.input(|inp| inp.modifiers.command);
                                    for v in values {
                                        let on = f.picks[i].contains(&v.criterion);
                                        if value_row(ui, &v.label, Some(v.count), on).clicked() {
                                            if ctrl {
                                                toggle(&mut f.picks[i], v.criterion.clone());
                                            } else if on && f.picks[i].len() == 1 {
                                                f.picks[i].clear();
                                            } else {
                                                f.picks[i] = vec![v.criterion.clone()];
                                            }
                                            // Columns to the right list what this one leaves.
                                            for later in f.picks.iter_mut().skip(i + 1) {
                                                later.clear();
                                            }
                                        }
                                    }
                                });
                        },
                    );
                },
            );
        }
    });
}

fn value_row(ui: &mut egui::Ui, label: &str, count: Option<i64>, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 17.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x55, 0x55, 0x55));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x3d, 0x3d, 0x3d));
    }
    let color = if selected { theme::TEXT_STRONG } else { theme::TEXT };
    let text_right = rect.right() - 34.0;
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.5), color);
    let clip = egui::Rect::from_min_max(rect.min, egui::pos2(text_right, rect.max.y));
    ui.painter().with_clip_rect(clip).galley(
        rect.left_center() + egui::vec2(4.0, -galley.size().y / 2.0),
        galley,
        color,
    );
    if let Some(n) = count {
        ui.painter().text(
            rect.right_center() - egui::vec2(4.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            egui::FontId::proportional(11.0),
            theme::TEXT_WEAK,
        );
    }
    resp.on_hover_text(label)
}
