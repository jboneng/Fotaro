//! The Develop module, following Lightroom Classic's:
//!
//! ```text
//! ├ left panels ─┬─────────────────────────────────────────┬ right panels ┤
//! │ Navigator    │                                          │ Histogram    │
//! │ Presets      │         the photo, as developed          │ Basic        │
//! │ Snapshots    │                                          │ (Tone Curve, │
//! │ History      │                                          │  HSL… later) │
//! │ Collections  ├ toolbar ─────────────────────────────────┤              │
//! │ [Copy][Paste]│                                          │      [Reset] │
//! ```

use catalog::PhotoRow;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Vec2};
use imaging::PreviewSize;
use imaging::develop::{
    CurveChannel, CurveFns, DEFAULT_SPLITS, GradeParam, GradeWheel, HslGroup, POINT_PRESETS, Region,
    SWATCHES, Slider, ToneCurve, Treatment, WbPreset,
};
use imaging::develop::{
    DetailParam, EffectParam, MAX_GUIDES, SharpenView, TransformParam, Upright, VignetteStyle,
};

use super::panels::Side;
use super::shortcuts::Action;
use super::theme;
use super::views::{paint_image, rotated_size};
use crate::app::{Command, FotaroApp};
use crate::develop::{DevelopState, GuideDrag, RegionTarget, Slot};

const LEFT: &[&str] = &["Navigator", "Presets", "Snapshots", "History", "Collections"];
const PLANNED_RIGHT: &[&str] = &[];
const RIGHT: &[&str] = &[
    "Histogram",
    "Basic",
    "Tone Curve",
    "HSL / Color",
    "Color Grading",
    "Detail",
    "Lens Corrections",
    "Transform",
    "Effects",
];

pub(super) fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

fn bottom_buttons(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(14.0, 3.0);
        add(ui);
    });
    ui.add_space(2.0);
    ui.separator();
}

pub fn left(app: &mut FotaroApp, ui: &mut egui::Ui) {
    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
        let (has_clip, has_photo) = app
            .session
            .as_ref()
            .map_or((false, false), |s| (s.develop.clipboard.is_some(), s.develop.photo.is_some()));
        bottom_buttons(ui, |ui| {
            if ui
                .add_enabled(has_photo, egui::Button::new("Copy…"))
                .on_hover_text("Copy Settings (Ctrl+Shift+C)")
                .clicked()
            {
                app.run(Command::Action(Action::CopySettings));
            }
            if ui
                .add_enabled(has_clip, egui::Button::new("Paste"))
                .on_hover_text("Paste Settings (Ctrl+Shift+V)")
                .clicked()
            {
                app.run(Command::Action(Action::PasteSettings));
            }
        });
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            // The Navigator stays put above the rest (it previews the preset under the
            // pointer, however far down the list that is).
            let mut stack = std::mem::take(&mut app.ui.dev_left);
            stack.section(ui, Side::Left, "Navigator", LEFT, true, |ui| navigator(app, ui));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                app.presets_ui.hover = None;
                stack.section(ui, Side::Left, "Presets", LEFT, true, |ui| {
                    super::presets::presets_panel(app, ui)
                });
                stack.section(ui, Side::Left, "Snapshots", LEFT, false, |ui| {
                    super::presets::snapshots_panel(app, ui)
                });
                stack.section(ui, Side::Left, "History", LEFT, true, |ui| history(app, ui));
                stack.section(ui, Side::Left, "Collections", LEFT, false, |ui| {
                    super::collections::panel(app, ui)
                });
                app.ui.dev_left = stack;
            });
        });
    });
    // The Navigator previews what is under the pointer (nothing: the photo as it is).
    let hover = app.presets_ui.hover.take();
    if let Some(s) = &mut app.session {
        s.develop.set_preview(hover);
    }
    super::presets::dialogs(app, ui.ctx());
}

/// The Library's Navigator (zoom levels, visible area), showing the photo as developed.
fn navigator(app: &mut FotaroApp, ui: &mut egui::Ui) {
    // A hovered preset or snapshot, once rendered; else the photo as it is.
    let shown = app.session.as_ref().and_then(|s| {
        s.develop.preview_texture().or_else(|| s.develop.textures.get(&Slot::Current)).cloned()
    });
    super::sidebars::navigator(app, ui, shown);
}

/// LrC's History: newest step first; click one to go back to it.
fn history(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let steps = &s.develop.history;
    if steps.is_empty() {
        ui.label(weak("No changes yet."));
        return;
    }
    let mut revert = None;
    let last = steps.len() - 1;
    for (i, step) in steps.iter().enumerate().rev() {
        if super::sidebars::source_row(ui, 8.0, &step.label, None, i == last).clicked() && i != last {
            revert = Some(step.id);
        }
    }
    if let Some(id) = revert {
        app.run(Command::DevelopRevert(id));
    }
}

pub fn right(app: &mut FotaroApp, ui: &mut egui::Ui) {
    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
        let has_photo = app.session.as_ref().is_some_and(|s| s.develop.photo.is_some());
        bottom_buttons(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(has_photo, egui::Button::new("Reset"))
                    .on_hover_text("Reset all settings (Ctrl+Shift+R)")
                    .clicked()
                {
                    app.run(Command::Action(Action::ResetSettings));
                }
            });
        });
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut stack = std::mem::take(&mut app.ui.dev_right);
                stack.section(ui, Side::Right, "Histogram", RIGHT, true, |ui| {
                    let h = app.session.as_ref().and_then(|s| s.develop.histogram.as_deref());
                    let top = ui.cursor().min;
                    super::sidebars::paint_histogram(ui, h);
                    let hist = Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y));
                    clipping_triangles(app, ui, hist);
                    super::sidebars::exposure_summary(app, ui);
                });
                super::masking::masking_button(app, ui);
                // Masking replaces the panels with the selected mask's sliders, as in LrC.
                if app.session.as_ref().is_some_and(|s| s.develop.removing.is_some()) {
                    super::remove::right_panel(app, ui);
                    app.ui.dev_right = stack;
                    return;
                }
                if app.session.as_ref().is_some_and(|s| s.develop.cropping.is_some()) {
                    super::crop::right_panel(app, ui);
                    app.ui.dev_right = stack;
                    return;
                }
                if app.session.as_ref().is_some_and(|s| s.develop.masking.is_some()) {
                    super::masking::right_panel(app, ui);
                    app.ui.dev_right = stack;
                    return;
                }
                stack.section(ui, Side::Right, "Basic", RIGHT, true, |ui| basic(app, ui));
                stack.section(ui, Side::Right, "Tone Curve", RIGHT, false, |ui| tone_curve(app, ui));
                stack.section(ui, Side::Right, "HSL / Color", RIGHT, false, |ui| hsl_panel(app, ui));
                stack.section(ui, Side::Right, "Color Grading", RIGHT, false, |ui| color_grading(app, ui));
                stack.section(ui, Side::Right, "Detail", RIGHT, false, |ui| detail_panel(app, ui));
                stack.section(ui, Side::Right, "Lens Corrections", RIGHT, false, |ui| {
                    super::lens::lens_panel(app, ui)
                });
                for title in PLANNED_RIGHT {
                    stack.section(ui, Side::Right, title, RIGHT, false, |ui| {
                        ui.label(weak("Comes in a later version."));
                    });
                }
                stack.section(ui, Side::Right, "Transform", RIGHT, false, |ui| transform_panel(app, ui));
                stack.section(ui, Side::Right, "Effects", RIGHT, false, |ui| effects_panel(app, ui));
                app.ui.dev_right = stack;
            });
        });
    });
}

/// How a slider row looks and behaves.
pub(super) struct Spec {
    pub(super) slider: Slider,
    pub(super) range: (f32, f32),
    pub(super) default: f32,
    /// Logarithmic track (Kelvin).
    pub(super) log: bool,
    /// Colours at the track's ends (Temp, Tint, Vibrance, Saturation).
    pub(super) gradient: Option<[Color32; 2]>,
    pub(super) kelvin: bool,
}

impl Spec {
    fn t(&self, v: f32) -> f32 {
        let (a, b) = self.range;
        let t = if self.log { (v / a).ln() / (b / a).ln() } else { (v - a) / (b - a) };
        t.clamp(0.0, 1.0)
    }

    fn value(&self, t: f32) -> f32 {
        let (a, b) = self.range;
        let v = if self.log { a * (b / a).powf(t) } else { a + t * (b - a) };
        self.round(v)
    }

    fn round(&self, v: f32) -> f32 {
        let step = 10f32.powi(-(self.slider.decimals() as i32));
        ((v / step).round() * step).clamp(self.range.0, self.range.1)
    }
}

#[derive(Default)]
pub(super) struct Moved {
    pub(super) changed: bool,
    /// The change is finished (slider released, value typed, reset): save it.
    pub(super) done: bool,
    /// Double-clicked (label or track): back to the default.
    pub(super) reset: bool,
    /// The track or value is being dragged right now.
    pub(super) held: bool,
}

pub(super) const LABEL_W: f32 = 74.0;
const VALUE_W: f32 = 48.0;

/// One LrC slider row: label (double-click resets), track, value (drag or type).
pub(super) fn slider_row(ui: &mut egui::Ui, spec: &Spec, value: &mut f32) -> Moved {
    slider_row_as(ui, spec, spec.slider.label(), value)
}

/// [`slider_row`] with its own label (for settings that aren't develop sliders, such as the
/// brush's).
pub(super) fn slider_row_as(ui: &mut egui::Ui, spec: &Spec, text: &str, value: &mut f32) -> Moved {
    let mut out = Moved::default();
    let enabled = ui.is_enabled();
    let id = ui.scope_id().with(("dev_slider", spec.slider));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 21.0), Sense::hover());
    let label_rect = Rect::from_min_size(rect.min, egui::vec2(LABEL_W, rect.height()));
    let value_rect = Rect::from_min_max(egui::pos2(rect.right() - VALUE_W, rect.top() + 1.0), rect.max);
    let track_rect = Rect::from_min_max(
        egui::pos2(label_rect.right() + 8.0, rect.top()),
        egui::pos2(value_rect.left() - 10.0, rect.bottom()),
    );

    let label = ui.interact(label_rect, id.with("label"), Sense::click());
    let text_color = if enabled { theme::TEXT } else { theme::TEXT_WEAK.gamma_multiply(0.7) };
    ui.painter().text(
        label_rect.right_center(),
        Align2::RIGHT_CENTER,
        text,
        FontId::proportional(12.5),
        if label.hovered() { theme::TEXT_STRONG } else { text_color },
    );
    let label = label.on_hover_text("Double-click to reset");
    let mut reset = label.double_clicked();

    // Track: a thin line (or gradient), with a triangle marking the value.
    let track = ui.interact(track_rect, id.with("track"), Sense::click_and_drag());
    let line = Rect::from_center_size(track_rect.center(), egui::vec2(track_rect.width(), 3.0));
    match spec.gradient {
        Some([a, b]) => {
            let (a, b) = if enabled { (a, b) } else { (a.gamma_multiply(0.4), b.gamma_multiply(0.4)) };
            let mut mesh = egui::Mesh::default();
            for (pos, color) in [
                (line.left_top(), a),
                (line.right_top(), b),
                (line.right_bottom(), b),
                (line.left_bottom(), a),
            ] {
                mesh.colored_vertex(pos, color);
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(egui::Shape::mesh(mesh));
        }
        None => {
            ui.painter().rect_filled(line, 1.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
        }
    }
    reset |= track.double_clicked();
    if reset {
        out = Moved { changed: *value != spec.default, done: true, reset: true, held: false };
        *value = spec.default;
    } else if track.is_pointer_button_down_on()
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let v = spec.value((p.x - track_rect.left()) / track_rect.width());
        if v != *value {
            *value = v;
            out.changed = true;
        }
    }
    out.held = track.is_pointer_button_down_on();
    if track.drag_stopped() || track.clicked() {
        out.done = true;
    }
    let x = track_rect.left() + spec.t(*value) * track_rect.width();
    let knob = if track.hovered() || track.dragged() { theme::TEXT_STRONG } else { theme::TEXT };
    let knob = if enabled { knob } else { theme::TEXT_WEAK.gamma_multiply(0.6) };
    let tip = egui::pos2(x, line.bottom());
    ui.painter().add(egui::Shape::convex_polygon(
        vec![tip, tip + egui::vec2(5.0, 8.0), tip + egui::vec2(-5.0, 8.0)],
        knob,
        egui::Stroke::NONE,
    ));

    // The value: drag it, or type a new one.
    let kelvin = spec.kelvin;
    let slider = spec.slider;
    let speed = if spec.log {
        10.0
    } else if slider.decimals() > 0 {
        10f32.powi(-(slider.decimals() as i32))
    } else {
        0.5
    };
    let mut v = *value;
    let resp = ui.put(
        value_rect,
        egui::DragValue::new(&mut v)
            .range(spec.range.0..=spec.range.1)
            .speed(speed)
            .update_while_editing(false)
            .custom_formatter(move |v, _| slider.format(v as f32, kelvin)),
    );
    if resp.changed() {
        let v = spec.round(v);
        if v != *value {
            *value = v;
            out.changed = true;
        }
        if !resp.dragged() {
            out.done = true;
        }
    }
    out.held |= resp.dragged();
    if resp.drag_stopped() {
        out.done = true;
    }
    out
}

pub(super) fn sub_header(ui: &mut egui::Ui, title: &str, add_right: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(12.5).color(theme::TEXT_STRONG));
        ui.with_layout(Layout::right_to_left(Align::Center), add_right);
    });
}

/// Basic's Profile: a 3D LUT (a creative look, after the base tone) and its Amount, as
/// LrC's creative profiles. LUTs are imported (copied to the app's LUT folder) from here.
fn profile_row(
    ui: &mut egui::Ui,
    st: &mut imaging::develop::DevelopSettings,
    done: &mut Option<Option<String>>,
    error: &mut Option<String>,
) {
    use imaging::develop::{LutProfile, import_lut, list_luts, lut_available};
    let mut importing = false;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W, 20.0], egui::Label::new(weak("Profile:")));
        ui.add_space(4.0);
        let shown = st.profile.as_ref().map_or_else(|| "None".to_string(), |p| p.name.clone());
        egui::ComboBox::from_id_salt("dev_profile")
            .selected_text(shown)
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(st.profile.is_none(), "None").clicked() && st.profile.is_some() {
                    st.profile = None;
                    *done = Some(None);
                }
                for l in list_luts() {
                    let on = st.profile.as_ref().is_some_and(|p| p.hash == l.hash);
                    if ui.selectable_label(on, &l.name).clicked() && !on {
                        let amount = st.profile.as_ref().map_or(100.0, |p| p.amount);
                        st.profile = Some(LutProfile { name: l.name, hash: l.hash, amount });
                        *done = Some(None);
                    }
                }
                ui.separator();
                if ui.button("Import LUT…").on_hover_text("3D LUTs (.cube files)").clicked() {
                    importing = true;
                }
            })
            .response
            .on_hover_text(
                "A look from a 3D LUT, applied after the basic tone; the other panels adjust on top",
            );
        if st.profile.as_ref().is_some_and(|p| !lut_available(&p.hash)) {
            ui.label(RichText::new("⚠").color(Color32::from_rgb(230, 160, 40)))
                .on_hover_text("This LUT isn't in the LUT folder: import its .cube file again");
        }
    });
    if importing
        && let Some(files) =
            rfd::FileDialog::new().set_title("Import LUT").add_filter("3D LUT", &["cube"]).pick_files()
    {
        let mut first = None;
        for f in files {
            match import_lut(&f) {
                Ok(l) => first = first.or(Some(l)),
                Err(e) => *error = Some(format!("{}: {e}", f.display())),
            }
        }
        if let Some(l) = first {
            st.profile = Some(LutProfile { name: l.name, hash: l.hash, amount: 100.0 });
            *done = Some(None);
        }
    }
    if let Some(p) = &mut st.profile {
        let spec = Spec {
            slider: Slider::MaskAmount(250),
            range: (0.0, 200.0),
            default: 100.0,
            log: false,
            gradient: None,
            kelvin: false,
        };
        if slider_row_as(ui, &spec, "Amount", &mut p.amount).done {
            *done = Some(None);
        }
    }
}

/// LrC's Basic panel.
fn basic(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let kelvin = d.kelvin();
    let presets: &[WbPreset] = d.info.as_ref().map_or(&WbPreset::RELATIVE, |i| i.presets);
    let resolved = d.wb;
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut toggle_picker = false;
    let picking = d.picking_wb;

    // Treatment
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W, 20.0], egui::Label::new(weak("Treatment:")));
        ui.add_space(4.0);
        for (t, name) in [(Treatment::Color, "Color"), (Treatment::BlackAndWhite, "Black & White")] {
            if ui.selectable_label(st.treatment == t, name).clicked() && st.treatment != t {
                st.treatment = t;
                done = Some(None);
            }
        }
    });
    profile_row(ui, &mut st, &mut done, &mut d.error);
    ui.add_space(4.0);

    // White balance: selector, preset menu, Temp, Tint.
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new(RichText::new("⊹").size(15.0)).selected(picking))
            .on_hover_text("White Balance Selector (W): click a neutral area of the photo")
            .clicked()
        {
            toggle_picker = true;
        }
        ui.label(weak("WB:"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let before = st.wb;
            egui::ComboBox::from_id_salt("dev_wb").selected_text(st.wb.label()).width(110.0).show_ui(
                ui,
                |ui| {
                    for p in presets {
                        ui.selectable_value(&mut st.wb, *p, p.label());
                    }
                },
            );
            if st.wb != before {
                if st.wb == WbPreset::Custom
                    && let Some(r) = resolved
                {
                    (st.temp, st.tint) = (r.temp.round(), r.tint.round());
                }
                done = Some(None);
            }
        });
    });
    let custom = st.wb == WbPreset::Custom;
    for slider in Slider::WB {
        let spec = Spec {
            slider,
            range: slider.range(kelvin),
            default: 0.0,
            log: kelvin && slider == Slider::Temp,
            gradient: Some(match slider {
                Slider::Temp => [Color32::from_rgb(0x4a, 0x7b, 0xd6), Color32::from_rgb(0xe0, 0xc0, 0x40)],
                _ => [Color32::from_rgb(0x4c, 0xaf, 0x50), Color32::from_rgb(0xc0, 0x4c, 0xc0)],
            }),
            kelvin,
        };
        // Presets show the Temp/Tint they resolve to; moving either makes them Custom.
        let shown = match (custom, resolved) {
            (true, _) => slider.get(&st),
            (false, Some(r)) => {
                if slider == Slider::Temp {
                    r.temp
                } else {
                    r.tint
                }
            }
            (false, None) => 0.0,
        };
        // Resetting Temp or Tint goes back to As Shot, as in LrC.
        let mut v = spec.round(shown);
        let spec = Spec { default: v, ..spec };
        let moved = slider_row(ui, &spec, &mut v);
        if moved.reset {
            st.wb = WbPreset::AsShot;
            done = Some(None);
            continue;
        }
        if moved.changed {
            if !custom && let Some(r) = resolved {
                (st.temp, st.tint) = (r.temp.round(), r.tint.round());
            }
            slider.set(&mut st, v);
        }
        if moved.done {
            done = Some(None);
        }
    }

    let mut auto = false;
    sub_header(ui, "Tone", |ui| {
        auto = ui.button("Auto").on_hover_text("Auto Tone (Ctrl+U)").clicked();
    });
    for slider in Slider::TONE.into_iter().chain(Slider::PRESENCE) {
        if slider == Slider::Texture {
            sub_header(ui, "Presence", |_| {});
        }
        let spec = Spec {
            slider,
            range: slider.range(kelvin),
            default: 0.0,
            log: false,
            gradient: match slider {
                Slider::Vibrance | Slider::Saturation => {
                    Some([Color32::from_rgb(0x80, 0x80, 0x80), Color32::from_rgb(0xd0, 0x50, 0x40)])
                }
                _ => None,
            },
            kelvin,
        };
        let mut v = slider.get(&st);
        let moved = slider_row(ui, &spec, &mut v);
        if moved.changed {
            slider.set(&mut st, v);
        }
        if moved.done {
            done = Some(None);
        }
    }

    // Apply: live render now, save when a change is finished.
    if st != d.settings {
        d.set_live(st);
    }
    if toggle_picker {
        d.picking_wb = !d.picking_wb;
        d.picking_detail = false;
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
    if auto {
        app.run(Command::Action(Action::AutoTone));
    }
}

/// LrC's clipping triangles in the histogram's top corners: lit (blue / red) when the
/// render has clipped shadows / highlights; click to toggle showing them (J).
fn clipping_triangles(app: &mut FotaroApp, ui: &mut egui::Ui, hist: Rect) {
    let Some(s) = &app.session else { return };
    let (on, c) = (s.develop.clipping, s.develop.clipped);
    let size = 9.0;
    let corners = [
        (hist.left_top() + egui::vec2(4.0, 4.0), true, c.shadows, Color32::from_rgb(70, 130, 255)),
        (
            hist.right_top() + egui::vec2(-4.0 - size, 4.0),
            false,
            c.highlights,
            Color32::from_rgb(255, 70, 70),
        ),
    ];
    let mut toggle = false;
    for (min, left, share, lit) in corners {
        let r = Rect::from_min_size(min, egui::vec2(size, size));
        let resp = ui.interact(r, ui.scope_id().with(("clip", left)), Sense::click());
        let clipped = share > 0.0001;
        let color = if clipped { lit } else { theme::TEXT_WEAK.gamma_multiply(0.6) };
        let pts = if left {
            vec![r.left_top(), r.right_top(), r.left_bottom()]
        } else {
            vec![r.left_top(), r.right_top(), r.right_bottom()]
        };
        let stroke = if on { egui::Stroke::new(1.0, Color32::WHITE) } else { egui::Stroke::NONE };
        ui.painter().add(egui::Shape::convex_polygon(pts, color, stroke));
        let what = if left { "Shadow" } else { "Highlight" };
        if resp
            .on_hover_text(format!("{what} clipping: {:.2}% of the photo. Click to show (J)", share * 100.0))
            .clicked()
        {
            toggle = true;
        }
    }
    if toggle {
        app.run(Command::Action(Action::ClippingIndicators));
    }
}

/// The center view: the developed photo (or Before), fitted or zoomed (Z, click; drag or
/// scroll to move around, as in the Loupe). Zoomed, the visible part is rendered from the
/// full-size image once the settings settle, over the enlarged proxy. Also the WB
/// Selector's clicks.
pub fn center(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let zoom_level = app.ui.zoom_level;
    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else { return };
    // Render at the size the photo is shown (LrC renders previews at screen size).
    let fitted = rect.shrink(18.0);
    s.develop.set_view_px((fitted.size().max_elem() * ppp).ceil() as u32, &photo);
    let slot = if s.develop.before { Slot::Before } else { Slot::Current };
    let developed = s.develop.textures.get(&slot).cloned();
    let Some(tex) = developed.clone().or_else(|| standard_preview(s, &photo)) else {
        app.zoom.image_rect = None;
        return;
    };
    let painter = ui.painter().with_clip_rect(rect);

    // Size of the developed photo in pixels as displayed (file orientation, then catalog
    // rotation): the file's size, or less with Constrain Crop.
    let full_size = s.develop.output_size().map(|(w, h)| egui::vec2(w as f32, h as f32));
    let img_px = rotated_size(full_size.unwrap_or_else(|| tex.size_vec2()), photo.rotation);
    // Crop & Straighten shows the whole photo (fit), as in LrC.
    let cropping = !s.develop.before && developed.is_some() && s.develop.cropping.is_some();
    // The Remove tool: clicks and drags make and move spots.
    let removing = !s.develop.before && developed.is_some() && s.develop.removing.is_some();
    let zoomed =
        app.zoom.zoomed && !photo.missing && !s.develop.picking_wb && !s.develop.picking_detail && !cropping;
    // Upright Guided: drags on the photo draw guides.
    let guiding = !s.develop.before
        && developed.is_some()
        && s.develop.settings.transform.upright == Upright::Guided
        && !s.develop.picking_wb
        && !s.develop.picking_detail
        && s.develop.masking.is_none()
        && s.develop.cropping.is_none()
        && s.develop.removing.is_none();
    // Masking: drags on the photo draw and shape masks.
    let masking = !s.develop.before && developed.is_some() && s.develop.masking.is_some();
    let brushing = masking && super::masking::brush_active(&s.develop);
    // Space (or Ctrl) held while zoomed in: a drag moves the photo whatever tool is open
    // (as in LrC).
    // A button clicked earlier (Select Subject…) keeps the keyboard focus, and Space would
    // press it again: over the photo, only a text field keeps the focus.
    let typing = ui.ctx().text_edit_focused();
    if resp.hovered() && !typing {
        ui.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    // Pan mode holds through a drag begun while Space was down (or a moment after: a
    // press may arrive just as the key reads as released).
    // Ctrl does the same as Space: some laptops' pointing devices (a ThinkPad's TrackPoint
    // buttons) give no clicks while an ordinary key is held, only with a modifier.
    let space_down = !typing && ui.input(|i| i.key_down(egui::Key::Space) || i.modifiers.command);
    let (now, pressed) = ui.input(|i| (i.time, i.pointer.primary_down()));
    let latch_id = egui::Id::unique("develop_space_pan");
    let (mut last_space, mut latched): (f64, bool) =
        ui.data(|d| d.get_temp(latch_id)).unwrap_or((f64::NEG_INFINITY, false));
    if space_down {
        last_space = now;
    }
    latched = pressed && (latched || now - last_space < 0.4);
    ui.data_mut(|d| d.insert_temp(latch_id, (last_space, latched)));
    let space_pan = zoomed && (space_down || latched);
    let view = if zoomed { rect } else { fitted };
    let fit = (view.width() / img_px.x).min(view.height() / img_px.y);
    let fill = (view.width() / img_px.x).max(view.height() / img_px.y);
    let scale = if zoomed { zoom_level.scale(fill, ppp).max(fit) } else { fit };
    let size = img_px * scale;
    let z = &mut app.zoom;
    if zoomed {
        if let Some((q, offset)) = z.anchor.take() {
            z.center = q - offset / size;
        }
        // The middle button always moves the photo.
        let middle = resp.dragged_by(egui::PointerButton::Middle);
        if middle || (resp.dragged() && (space_pan || (!guiding && !masking && !removing))) {
            z.center -= resp.drag_delta() / size;
        }
        // The wheel pans, unless the brush is out (then it sizes the brush).
        if resp.hovered() && (space_pan || (!brushing && !removing)) {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            if scroll != Vec2::ZERO {
                z.center -= scroll / size;
            }
        }
        z.center = super::views::clamp_center(z.center, view.size() / size);
    }
    let img_rect = if zoomed {
        Rect::from_min_size(view.center() - size * z.center, size)
    } else {
        Rect::from_center_size(view.center(), size)
    };
    let visible = view.intersect(img_rect);
    let visible_q = Rect::from_min_max(
        ((visible.min - img_rect.min) / size).to_pos2(),
        ((visible.max - img_rect.min) / size).to_pos2(),
    );
    z.image_rect = Some(img_rect);
    z.view_rect = Some(view);
    z.visible = zoomed.then_some(visible_q);
    paint_image(&painter, &tex, img_rect, photo.rotation, Color32::WHITE);

    // Zoomed: the visible part at full resolution, drawn over the enlarged proxy.
    let mut sharp = true;
    if zoomed && let Some(full) = full_size {
        // The visible part in the file's orientation and pixels (a pixel of margin).
        let (a, b) = (
            unrotate(visible_q.min.to_vec2(), photo.rotation),
            unrotate(visible_q.max.to_vec2(), photo.rotation),
        );
        let (u0, u1) = (a.0.min(b.0), a.0.max(b.0));
        let (v0, v1) = (a.1.min(b.1), a.1.max(b.1));
        let x0 = (u0 * full.x).floor().max(0.0) as u32;
        let y0 = (v0 * full.y).floor().max(0.0) as u32;
        let x1 = ((u1 * full.x).ceil() + 1.0).min(full.x) as u32;
        let y1 = ((v1 * full.y).ceil() + 1.0).min(full.y) as u32;
        let wanted =
            Region { x: x0, y: y0, w: x1.saturating_sub(x0).max(1), h: y1.saturating_sub(y0).max(1) };
        if !resp.dragged()
            && let Some(wait) = s.develop.want_region(RegionTarget::Zoom, wanted)
        {
            ui.ctx().request_repaint_after(wait);
        }
        match s.develop.region_for(RegionTarget::Zoom) {
            Some((region, region_tex)) => {
                // Its corners in display coordinates (0..1 of the photo as shown).
                let p = |x: u32, y: u32| rotate((x as f32 / full.x, y as f32 / full.y), photo.rotation);
                let (c0, c1) = (p(region.x, region.y), p(region.x + region.w, region.y + region.h));
                let q = Rect::from_two_pos(egui::pos2(c0.0, c0.1), egui::pos2(c1.0, c1.1));
                let r = Rect::from_min_max(
                    img_rect.min + q.min.to_vec2() * size,
                    img_rect.min + q.max.to_vec2() * size,
                );
                paint_image(&painter, region_tex, r, photo.rotation, Color32::WHITE);
                sharp = region == wanted;
            }
            None => sharp = false,
        }
    }

    // While a Transform slider is dragged: a grid to line verticals and horizontals up with.
    if s.develop.transform_grid {
        let area = view.intersect(img_rect);
        let step = (img_rect.width().min(img_rect.height()) / 8.0).max(24.0);
        let stroke = egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 90));
        let mut x = img_rect.left() + step;
        while x < img_rect.right() - 1.0 {
            painter.line_segment([egui::pos2(x, area.top()), egui::pos2(x, area.bottom())], stroke);
            x += step;
        }
        let mut y = img_rect.top() + step;
        while y < img_rect.bottom() - 1.0 {
            painter.line_segment([egui::pos2(area.left(), y), egui::pos2(area.right(), y)], stroke);
            y += step;
        }
    }

    let corner = |text: &str, color: Color32| {
        painter.text(
            rect.left_bottom() + egui::vec2(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            text,
            FontId::proportional(12.5),
            color,
        );
    };
    if let Some(e) = &s.develop.error {
        corner(e, theme::MISSING);
    } else if developed.is_none() {
        corner("Loading…", theme::TEXT_STRONG);
    } else if zoomed && !sharp && s.develop.region_pending(RegionTarget::Zoom) {
        corner("Rendering full resolution…", theme::TEXT_STRONG);
    }
    if s.develop.before {
        painter.text(
            rect.left_top() + egui::vec2(14.0, 10.0),
            Align2::LEFT_TOP,
            "Before",
            FontId::proportional(14.0),
            theme::TEXT_STRONG,
        );
    }

    let over_photo = resp.hover_pos().is_some_and(|p| img_rect.contains(p));
    if space_pan {
        // The hand: the open tool gets no clicks or drags meanwhile.
        let icon = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
        ui.ctx().set_cursor_icon(icon);
        return;
    }
    if s.develop.picking_wb {
        // White Balance Selector: click a neutral area.
        if over_photo {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && img_rect.contains(p)
        {
            let q = (p - img_rect.min) / img_rect.size();
            let (x, y) = unrotate(q, photo.rotation);
            s.develop.pick(x, y);
        }
        return;
    }
    if removing {
        if super::remove::canvas(ui, &resp, &mut s.develop, photo.rotation, img_rect, &painter) {
            app.run(Command::DevelopCommit(None));
        }
        super::masking::ai_consent(app, ui.ctx());
        return;
    }
    if cropping {
        if super::crop::canvas(ui, &resp, &mut s.develop, photo.rotation, img_rect, &painter) {
            app.run(Command::DevelopCommit(None));
        }
        return;
    }
    if masking {
        let commit = super::masking::canvas(ui, &resp, &mut s.develop, photo.rotation, img_rect, &painter);
        super::masking::masks_panel(app, ui.ctx(), rect);
        super::masking::ai_consent(app, ui.ctx());
        if commit {
            app.run(Command::DevelopCommit(None));
        }
        return;
    }
    if guiding {
        if guided_tool(ui, &resp, &mut s.develop, photo.rotation, img_rect, &painter) {
            app.run(Command::DevelopCommit(None));
        }
        return;
    }
    if s.develop.picking_detail {
        // The Detail panel's target: the box follows the pointer; a click keeps the point.
        if let Some(p) = resp.hover_pos().filter(|p| img_rect.contains(*p)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            s.develop.detail_focus = unrotate((p - img_rect.min) / img_rect.size(), photo.rotation);
        }
        if resp.clicked() {
            s.develop.picking_detail = false;
        }
        return;
    }
    // As in the Loupe: a click toggles between Fit and the zoom level, at that point.
    if resp.clicked() && !photo.missing {
        if zoomed {
            app.zoom.zoomed = false;
        } else if let Some(p) = resp.interact_pointer_pos() {
            app.zoom.zoom_in_at_in(p, img_rect, view);
        }
    }
    if zoomed {
        let icon = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
        ui.ctx().set_cursor_icon(icon);
    } else if over_photo && !photo.missing {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
    }
}

/// Upright Guided on the photo: drag to draw a guide (up to four), drag a guide's end to
/// move it, right-click a guide to remove it. Guides are kept in the untransformed photo's
/// coordinates and drawn where the transformed photo shows them. Returns true when the
/// guides changed (to be saved).
fn guided_tool(
    ui: &egui::Ui,
    resp: &egui::Response,
    d: &mut DevelopState,
    rotation: u8,
    img_rect: Rect,
    painter: &egui::Painter,
) -> bool {
    let Some((sw, sh)) = d.info.as_ref().map(|i| i.full_size) else { return false };
    let t = d.settings.transform.clone();
    let map = t.point_map(sw, sh);
    let to_screen = |x: f32, y: f32| -> Option<egui::Pos2> {
        let (u, v) = rotate(map.output_point(x, y)?, rotation);
        Some(img_rect.min + egui::vec2(u, v) * img_rect.size())
    };
    let to_source = |p: egui::Pos2| -> (f32, f32) {
        let (x, y) = unrotate((p - img_rect.min) / img_rect.size(), rotation);
        map.source_point(x, y)
    };
    let screen: Vec<Option<(egui::Pos2, egui::Pos2)>> =
        t.guides.iter().map(|g| Some((to_screen(g[0], g[1])?, to_screen(g[2], g[3])?))).collect();
    if resp.hover_pos().is_some_and(|p| img_rect.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    // A drag starts on a guide's end (moving it) or anywhere else (a new guide).
    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let near = screen.iter().enumerate().find_map(|(i, s)| {
            let (a, b) = (*s)?;
            if a.distance(origin) < 8.0 {
                Some((i, 0))
            } else if b.distance(origin) < 8.0 {
                Some((i, 1))
            } else {
                None
            }
        });
        d.guide_drag = Some(match near {
            Some((index, end)) => GuideDrag::End { index, end },
            None => GuideDrag::New { start: origin },
        });
    }
    let pointer = ui.input(|i| i.pointer.latest_pos());

    // The guides, with the one being moved or drawn following the pointer.
    let color = Color32::from_rgb(255, 205, 60);
    let stroke = egui::Stroke::new(1.5, color);
    for (i, s) in screen.iter().enumerate() {
        let Some((mut a, mut b)) = *s else { continue };
        if let (Some(GuideDrag::End { index, end }), Some(p)) = (d.guide_drag, pointer)
            && index == i
        {
            if end == 0 { a = p } else { b = p }
        }
        painter.line_segment([a, b], stroke);
        painter.circle_filled(a, 3.5, color);
        painter.circle_filled(b, 3.5, color);
    }
    if let (Some(GuideDrag::New { start }), Some(p)) = (d.guide_drag, pointer) {
        painter.line_segment([start, p], stroke);
        painter.circle_filled(start, 3.5, color);
    }

    let mut guides = t.guides.clone();
    if resp.drag_stopped()
        && let Some(drag) = d.guide_drag.take()
        && let Some(p) = pointer
    {
        match drag {
            GuideDrag::New { start } if start.distance(p) > 12.0 && guides.len() < MAX_GUIDES => {
                let (a, b) = (to_source(start), to_source(p));
                guides.push([a.0, a.1, b.0, b.1]);
            }
            GuideDrag::End { index, end } if index < guides.len() => {
                let q = to_source(p);
                guides[index][2 * end] = q.0;
                guides[index][2 * end + 1] = q.1;
            }
            _ => {}
        }
    }
    // Right-click on a guide removes it.
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(i) =
            screen.iter().position(|s| s.is_some_and(|(a, b)| distance_to_segment(p, a, b) < 6.0))
    {
        guides.remove(i);
    }
    if guides != t.guides {
        d.set_guides(guides);
        return true;
    }
    false
}

/// Distance from `p` to the segment `a`–`b`.
fn distance_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A point (0..1) of the photo before the catalog's `rotation` clockwise quarter turns →
/// the same point as displayed (the inverse of [`unrotate`]).
pub(super) fn rotate((u, v): (f32, f32), rotation: u8) -> (f32, f32) {
    match rotation % 4 {
        1 => (1.0 - v, u),
        2 => (1.0 - u, 1.0 - v),
        3 => (v, 1.0 - u),
        _ => (u, v),
    }
}

/// A point (0..1) of the photo as displayed → the same point before the catalog's
/// `rotation` clockwise quarter turns.
pub(super) fn unrotate(q: Vec2, rotation: u8) -> (f32, f32) {
    match rotation % 4 {
        1 => (q.y, 1.0 - q.x),
        2 => (1.0 - q.x, 1.0 - q.y),
        3 => (1.0 - q.y, q.x),
        _ => (q.x, q.y),
    }
}

/// While the Develop render loads: the standard preview (or the thumbnail).
fn standard_preview(s: &mut crate::app::Session, photo: &PhotoRow) -> Option<egui::TextureHandle> {
    if let Some(t) = s.previews.get(photo.id) {
        return Some(t);
    }
    if !photo.missing {
        s.request(photo, PreviewSize::Standard);
    }
    s.thumbs.get(photo.id)
}

pub fn toolbar(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let d = &s.develop;
    let (before, picking, busy) = (d.before, d.picking_wb, d.rendering());
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        if ui
            .add(egui::Button::new("Before / After").selected(before))
            .on_hover_text("Show the photo without adjustments (\\)")
            .clicked()
        {
            app.run(Command::Action(Action::BeforeAfter));
        }
        ui.separator();
        if ui
            .add(egui::Button::new("⊹ WB Selector").selected(picking))
            .on_hover_text("Click a neutral area to set the white balance (W)")
            .clicked()
        {
            app.run(Command::Action(Action::WhiteBalanceSelector));
        }
        if picking {
            ui.label(weak("Click something that should be neutral grey. Esc cancels."));
        }
        // The Remove tool's Visualize Spots, as in LrC's toolbar.
        if app.session.as_ref().is_some_and(|s| s.develop.removing.is_some()) {
            ui.separator();
            if let Some(s) = &mut app.session {
                let d = &mut s.develop;
                let (mut on, mut t) = (d.visualize_spots, d.spot_threshold);
                ui.checkbox(&mut on, "Visualize Spots")
                    .on_hover_text("Show edges as white on black, so dust and blemishes stand out (A)");
                ui.add_enabled_ui(on, |ui| {
                    ui.spacing_mut().slider_width = 140.0;
                    ui.add(egui::Slider::new(&mut t, 0..=100).show_value(false))
                        .on_hover_text("More to the right shows smaller spots");
                });
                if (on, t) != (d.visualize_spots, d.spot_threshold) {
                    d.set_visualize_spots(on, t);
                }
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy {
                ui.add(egui::Spinner::new().size(12.0));
            }
        });
    });
}

/// Distance (points) within which a press picks up a curve point.
const POINT_GRAB: f32 = 9.0;

/// LrC's Tone Curve panel: the curve over the histogram; Parametric (region sliders and
/// split points) or Point (RGB / Red / Green / Blue curves, presets).
fn tone_curve(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;

    // Mode, and in Point mode the channel and the presets.
    ui.horizontal(|ui| {
        if ui.selectable_label(!d.curve_point_mode, "Parametric").clicked() {
            d.curve_point_mode = false;
        }
        if ui.selectable_label(d.curve_point_mode, "Point").clicked() {
            d.curve_point_mode = true;
        }
        if d.curve_point_mode {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                egui::ComboBox::from_id_salt("curve_channel")
                    .selected_text(d.curve_channel.label())
                    .width(64.0)
                    .show_ui(ui, |ui| {
                        for c in CurveChannel::ALL {
                            ui.selectable_value(&mut d.curve_channel, c, c.label());
                        }
                    });
            });
        }
    });
    let (point_mode, ch) = (d.curve_point_mode, d.curve_channel);

    // The graph.
    let w = ui.available_width();
    // Square on a narrow panel; capped so the region sliders stay in view on wide ones.
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, w.min(260.0)), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x22, 0x22));
    if let Some(h) = &d.histogram {
        // Luminance-ish: the channels' average, faint, behind the curve.
        let bins: Vec<f32> = (0..256).map(|i| (h[0][i] + h[1][i] + h[2][i]) as f32 / 3.0).collect();
        let peak = bins[1..255].iter().copied().fold(1.0, f32::max);
        for (i, v) in bins.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let y = rect.bottom() - rect.height() * (v / peak).min(1.0) * 0.9;
            painter.line_segment(
                [egui::pos2(x, rect.bottom()), egui::pos2(x, y)],
                egui::Stroke::new(rect.width() / 255.0 + 0.5, Color32::from_gray(0x38)),
            );
        }
    }
    let grid = egui::Stroke::new(1.0, Color32::from_gray(0x30));
    for k in 1..4 {
        let t = k as f32 / 4.0;
        painter.vline(rect.left() + t * rect.width(), rect.y_range(), grid);
        painter.hline(rect.x_range(), rect.top() + t * rect.height(), grid);
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, Color32::from_gray(0x44)),
    );
    if !point_mode {
        for split in st.curve.splits {
            let x = rect.left() + split / 100.0 * rect.width();
            painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, Color32::from_gray(0x4a)));
        }
    }

    let to_screen =
        |x: f32, y: f32| egui::pos2(rect.left() + x * rect.width(), rect.bottom() - y * rect.height());
    let from_screen =
        |p: egui::Pos2| ((p.x - rect.left()) / rect.width(), (rect.bottom() - p.y) / rect.height());
    let curve_at = |curve: &ToneCurve, x: f32| {
        let f = CurveFns::new(curve);
        match (point_mode, ch) {
            (false, _) => f.master(x),
            (true, CurveChannel::Rgb) => f.master_after_parametric(x),
            (true, CurveChannel::Red) => f.channel(0, x),
            (true, CurveChannel::Green) => f.channel(1, x),
            (true, CurveChannel::Blue) => f.channel(2, x),
        }
    };

    if point_mode {
        let near = |pts: &[[f32; 2]], p: egui::Pos2| {
            pts.iter()
                .enumerate()
                .map(|(i, q)| (i, to_screen(q[0], q[1]).distance(p)))
                .filter(|(_, dist)| *dist <= POINT_GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let press = ui.input(|i| i.pointer.press_origin());
        let pointer = resp.interact_pointer_pos();
        // Press on a point to drag it; elsewhere, a new point on the curve to drag.
        if resp.drag_started()
            && let Some(p) = press
        {
            let mut pts = st.curve.points_or_linear(ch);
            d.curve_drag = near(&pts, p).or_else(|| {
                let x = from_screen(p).0.clamp(0.0, 1.0);
                let point = [x, curve_at(&st.curve, x)];
                let at = pts.partition_point(|q| q[0] < x);
                (at > 0 && at < pts.len()).then(|| {
                    pts.insert(at, point);
                    at
                })
            });
            *st.curve.points_mut(ch) = pts;
        }
        if let (Some(i), Some(p)) = (d.curve_drag, pointer)
            && resp.dragged()
        {
            let mut pts = st.curve.points_or_linear(ch);
            let n = pts.len();
            let end = i == 0 || i == n - 1;
            if !end && (p.y < rect.top() - 30.0 || p.y > rect.bottom() + 30.0) {
                // Dragged off the graph: the point goes (as in LrC).
                pts.remove(i);
                d.curve_drag = None;
            } else if i < n {
                let (x, y) = from_screen(p);
                let lo = if i == 0 { 0.0 } else { pts[i - 1][0] + 0.01 };
                let hi = if i == n - 1 { 1.0 } else { pts[i + 1][0] - 0.01 };
                pts[i] = [x.clamp(lo, hi.max(lo)), y.clamp(0.0, 1.0)];
            }
            *st.curve.points_mut(ch) = pts;
        }
        if resp.drag_stopped() {
            d.curve_drag = None;
            done = Some(None);
        }
        // Click: add a point on the curve; double-click a point: remove it.
        if let Some(p) = pointer {
            let mut pts = st.curve.points_or_linear(ch);
            if resp.double_clicked() {
                if let Some(i) = near(&pts, p).filter(|i| *i != 0 && *i != pts.len() - 1) {
                    pts.remove(i);
                    *st.curve.points_mut(ch) = pts;
                    done = Some(None);
                }
            } else if resp.clicked() && near(&pts, p).is_none() {
                let x = from_screen(p).0.clamp(0.0, 1.0);
                let at = pts.partition_point(|q| q[0] < x);
                if at > 0 && at < pts.len() {
                    pts.insert(at, [x, curve_at(&st.curve, x)]);
                    *st.curve.points_mut(ch) = pts;
                    done = Some(None);
                }
            }
        }
        if resp.hover_pos().is_some_and(|p| near(&st.curve.points_or_linear(ch), p).is_some()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    // The curve, and its points in Point mode.
    let color = match (point_mode, ch) {
        (true, CurveChannel::Red) => Color32::from_rgb(0xe0, 0x50, 0x50),
        (true, CurveChannel::Green) => Color32::from_rgb(0x50, 0xc0, 0x50),
        (true, CurveChannel::Blue) => Color32::from_rgb(0x60, 0x80, 0xf0),
        _ => theme::TEXT_STRONG,
    };
    let f = CurveFns::new(&st.curve);
    let line: Vec<egui::Pos2> = (0..=128)
        .map(|i| {
            let x = i as f32 / 128.0;
            let y = match (point_mode, ch) {
                (false, _) => f.master(x),
                (true, CurveChannel::Rgb) => f.master_after_parametric(x),
                (true, CurveChannel::Red) => f.channel(0, x),
                (true, CurveChannel::Green) => f.channel(1, x),
                (true, CurveChannel::Blue) => f.channel(2, x),
            };
            to_screen(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, color)));
    if point_mode {
        for (i, q) in st.curve.points_or_linear(ch).iter().enumerate() {
            let fill = if d.curve_drag == Some(i) { color } else { Color32::from_gray(0x22) };
            painter.circle(to_screen(q[0], q[1]), 3.5, fill, egui::Stroke::new(1.5, color));
        }
    }
    resp.context_menu(|ui| {
        if point_mode && ui.button(format!("Reset {} Curve", ch.label())).clicked() {
            st.curve.points_mut(ch).clear();
            done = Some(None);
            ui.close();
        }
        if ui.button("Reset Point Curves").clicked() {
            CurveChannel::ALL.iter().for_each(|c| st.curve.points_mut(*c).clear());
            done = Some(None);
            ui.close();
        }
        if ui.button("Reset Parametric Curve").clicked() {
            st.curve = ToneCurve {
                rgb: st.curve.rgb.clone(),
                red: st.curve.red.clone(),
                green: st.curve.green.clone(),
                blue: st.curve.blue.clone(),
                ..Default::default()
            };
            done = Some(None);
            ui.close();
        }
    });

    if point_mode {
        // LrC's Point Curve menu (for the RGB curve).
        ui.horizontal(|ui| {
            ui.label(weak("Point Curve:"));
            let current = POINT_PRESETS
                .iter()
                .find(|(_, p)| st.curve.points_or_linear(CurveChannel::Rgb) == p.to_vec())
                .map_or("Custom", |(name, _)| *name);
            egui::ComboBox::from_id_salt("curve_preset").selected_text(current).width(120.0).show_ui(
                ui,
                |ui| {
                    for (name, points) in POINT_PRESETS {
                        if ui.selectable_label(current == name, name).clicked() {
                            st.curve.rgb = if name == "Linear" { Vec::new() } else { points.to_vec() };
                            done = Some(None);
                        }
                    }
                },
            );
        });
    } else {
        // Split points: drag the triangles under the graph.
        let (strip, _) = ui.allocate_exact_size(egui::vec2(w, 14.0), Sense::hover());
        let mut splits = st.curve.splits;
        for k in 0..3 {
            let x = strip.left() + splits[k] / 100.0 * strip.width();
            let hit = Rect::from_center_size(egui::pos2(x, strip.center().y), egui::vec2(14.0, 14.0));
            let r = ui.interact(hit, ui.scope_id().with(("curve_split", k)), Sense::click_and_drag());
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                splits[k] = ((p.x - strip.left()) / strip.width() * 100.0).round();
                splits = ToneCurve::clamp_splits(splits);
            }
            if r.drag_stopped() {
                done = Some(None);
            }
            if r.double_clicked() {
                splits[k] = DEFAULT_SPLITS[k];
                done = Some(None);
            }
            let c = if r.hovered() || r.dragged() { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
            let tip = egui::pos2(x, strip.top() + 2.0);
            ui.painter().add(egui::Shape::convex_polygon(
                vec![tip, tip + egui::vec2(5.0, 9.0), tip + egui::vec2(-5.0, 9.0)],
                c,
                egui::Stroke::NONE,
            ));
            r.on_hover_text("Drag to move where the regions meet; double-click to reset");
        }
        st.curve.splits = splits;
        ui.add_space(2.0);
        for slider in Slider::CURVE {
            let spec = Spec {
                slider,
                range: (-100.0, 100.0),
                default: 0.0,
                log: false,
                gradient: None,
                kelvin: false,
            };
            let mut v = slider.get(&st);
            let moved = slider_row(ui, &spec, &mut v);
            if moved.changed {
                slider.set(&mut st, v);
            }
            if moved.done {
                done = Some(None);
            }
        }
    }

    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// A colour range's swatch for painting a slider track: linear sRGB, scaled by
/// `brightness`, mixed towards grey by `1 - saturation`.
fn swatch(i: usize, brightness: f32, saturation: f32) -> Color32 {
    let c = SWATCHES[i % 8];
    let grey = 0.5;
    let enc = |v: f32| {
        let v = ((grey + (v - grey) * saturation) * brightness).clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (e * 255.0).round() as u8
    };
    Color32::from_rgb(enc(c[0]), enc(c[1]), enc(c[2]))
}

/// LrC's HSL / Color panel (Hue, Saturation, Luminance or All), or in Black & White its
/// B&W Mix.
fn hsl_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut row = |ui: &mut egui::Ui,
                   st: &mut imaging::develop::DevelopSettings,
                   slider: Slider,
                   gradient: [Color32; 2]| {
        let spec = Spec {
            slider,
            range: (-100.0, 100.0),
            default: 0.0,
            log: false,
            gradient: Some(gradient),
            kelvin: false,
        };
        let mut v = slider.get(st);
        let moved = slider_row(ui, &spec, &mut v);
        if moved.changed {
            slider.set(st, v);
        }
        if moved.done {
            done = Some(None);
        }
    };

    if st.treatment == Treatment::BlackAndWhite {
        sub_header(ui, "Black & White Mix", |_| {});
        for i in 0..8u8 {
            let c = usize::from(i);
            row(ui, &mut st, Slider::BwMix(i), [swatch(c, 0.25, 1.0), swatch(c, 1.0, 1.0)]);
        }
    } else {
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Some(HslGroup::Hue), "Hue"),
                (Some(HslGroup::Saturation), "Saturation"),
                (Some(HslGroup::Luminance), "Luminance"),
                (None, "All"),
            ] {
                if ui.selectable_label(d.hsl_tab == tab, name).clicked() {
                    d.hsl_tab = tab;
                }
            }
        });
        let groups: Vec<HslGroup> = match d.hsl_tab {
            Some(g) => vec![g],
            None => HslGroup::ALL.to_vec(),
        };
        for g in groups {
            if d.hsl_tab.is_none() {
                sub_header(ui, g.label(), |_| {});
            }
            for i in 0..8u8 {
                let c = usize::from(i);
                let gradient = match g {
                    // From the previous range's colour to the next one's.
                    HslGroup::Hue => [swatch(c + 7, 1.0, 1.0), swatch(c + 1, 1.0, 1.0)],
                    HslGroup::Saturation => [swatch(c, 1.0, 0.0), swatch(c, 1.0, 1.0)],
                    HslGroup::Luminance => [swatch(c, 0.2, 1.0), swatch(c, 1.0, 0.6)],
                };
                row(ui, &mut st, Slider::Hsl(g, i), gradient);
            }
        }
    }

    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// A linear sRGB colour for painting.
pub(super) fn paint_color(c: [f32; 3]) -> Color32 {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (e * 255.0).round() as u8
    };
    Color32::from_rgb(enc(c[0]), enc(c[1]), enc(c[2]))
}

/// A colour wheel: hue round the edge, saturation outwards from the neutral centre. Drag
/// the handle (or anywhere) to set both; double-click resets the saturation.
pub(super) fn color_wheel(ui: &mut egui::Ui, id: egui::Id, size: f32, hue: &mut f32, sat: &mut f32) -> Moved {
    let mut out = Moved::default();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let (c, r) = (rect.center(), size / 2.0 - 4.0);
    // The disc: a fan from a grey centre to the wheel's colours at the edge.
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, Color32::from_gray(0x7a));
    let n = 72;
    for i in 0..=n {
        let h = i as f32 * 360.0 / n as f32;
        let a = h.to_radians();
        let edge = imaging::develop::wheel_color(h).map(|v| 0.25 + 0.6 * v);
        mesh.colored_vertex(c + egui::vec2(a.cos(), -a.sin()) * r, paint_color(edge));
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().circle_stroke(c, r, egui::Stroke::new(1.0, Color32::from_gray(0x20)));

    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.drag_started() || resp.clicked())
    {
        let d = p - c;
        let (h, s) = (
            (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0).round(),
            (d.length() / r * 100.0).clamp(0.0, 100.0).round(),
        );
        if (h, s) != (*hue, *sat) {
            (*hue, *sat) = (h, s);
            out.changed = true;
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        out.done = true;
    }
    if resp.double_clicked() {
        *sat = 0.0;
        out = Moved { changed: true, done: true, reset: true, held: false };
    }
    // The handle: where hue and saturation point.
    let a = hue.to_radians();
    let at = c + egui::vec2(a.cos(), -a.sin()) * r * (*sat / 100.0);
    let fill =
        if *sat > 0.0 { paint_color(imaging::develop::wheel_color(*hue)) } else { Color32::from_gray(0x7a) };
    ui.painter().circle(at, 5.0, fill, egui::Stroke::new(1.5, Color32::WHITE));
    resp.on_hover_text("Drag to set hue and saturation; double-click to reset");
    out
}

/// LrC's Color Grading panel: 3-way (Midtones above, Shadows and Highlights below) or one
/// wheel at a time (Shadows, Midtones, Highlights, Global), then Blending and Balance.
fn color_grading(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let note = |m: Moved, done: &mut Option<Option<String>>| {
        if m.done {
            *done = Some(None);
        }
    };

    ui.horizontal(|ui| {
        ui.label(weak("Adjust:"));
        for (view, name) in [
            (None, "3-Way"),
            (Some(GradeWheel::Shadows), "Shadows"),
            (Some(GradeWheel::Midtones), "Midtones"),
            (Some(GradeWheel::Highlights), "Highlights"),
            (Some(GradeWheel::Global), "Global"),
        ] {
            if ui.selectable_label(d.grading_view == view, name).clicked() {
                d.grading_view = view;
            }
        }
    });
    ui.add_space(4.0);

    let w = ui.available_width();
    // A wheel with its name and Luminance slider (and in the one-wheel view, Hue and
    // Saturation sliders too).
    let mut wheel = |ui: &mut egui::Ui,
                     st: &mut imaging::develop::DevelopSettings,
                     which: GradeWheel,
                     size: f32,
                     full: bool| {
        let id = ui.scope_id().with(("grade_wheel", which));
        let mut wh = *st.grading.wheel(which);
        ui.vertical_centered(|ui| {
            let m = color_wheel(ui, id, size, &mut wh.hue, &mut wh.saturation);
            note(m, &mut done);
            ui.label(RichText::new(which.label()).size(12.0).color(theme::TEXT));
        });
        *st.grading.wheel_mut(which) = wh;
        let params: &[GradeParam] = if full {
            &[GradeParam::Hue, GradeParam::Saturation, GradeParam::Luminance]
        } else {
            &[GradeParam::Luminance]
        };
        for p in params {
            let slider = Slider::Grade(which, *p);
            let gradient = match p {
                GradeParam::Hue => None,
                GradeParam::Saturation => {
                    Some([Color32::from_gray(0x80), paint_color(imaging::develop::wheel_color(wh.hue))])
                }
                GradeParam::Luminance => Some([Color32::from_gray(0x20), Color32::from_gray(0xe0)]),
            };
            let spec = Spec {
                slider,
                range: slider.range(false),
                default: 0.0,
                log: false,
                gradient,
                kelvin: false,
            };
            let mut v = slider.get(st);
            let m = slider_row(ui, &spec, &mut v);
            if m.changed {
                slider.set(st, v);
            }
            note(m, &mut done);
        }
    };

    match d.grading_view {
        None => {
            let size = (w * 0.42).clamp(90.0, 160.0);
            wheel(ui, &mut st, GradeWheel::Midtones, size, false);
            ui.add_space(6.0);
            ui.columns(2, |cols| {
                wheel(&mut cols[0], &mut st, GradeWheel::Shadows, size, false);
                wheel(&mut cols[1], &mut st, GradeWheel::Highlights, size, false);
            });
        }
        Some(which) => wheel(ui, &mut st, which, (w * 0.7).clamp(120.0, 230.0), true),
    }
    ui.add_space(6.0);
    for (slider, default) in [(Slider::GradeBlending, 50.0), (Slider::GradeBalance, 0.0)] {
        let spec =
            Spec { slider, range: slider.range(false), default, log: false, gradient: None, kelvin: false };
        let mut v = slider.get(&st);
        let m = slider_row(ui, &spec, &mut v);
        if m.changed {
            slider.set(&mut st, v);
        }
        note(m, &mut done);
    }

    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// LrC's Detail panel: Sharpening (Amount, Radius, Detail, Masking) and Noise Reduction
/// (Luminance with its Detail and Contrast; Color with its Detail and Smoothness).
fn detail_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let rotation = s.active_photo().map_or(0, |p| p.rotation);
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    // LrC's preview box: the target picks the point on the photo; the triangle hides it.
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new(RichText::new("⊹").size(15.0)).selected(d.picking_detail))
            .on_hover_text("Click a point of the photo to show it in the preview")
            .clicked()
        {
            d.picking_detail = !d.picking_detail;
            d.picking_wb = false;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let arrow = if d.detail_box_open { "⏷" } else { "⏴" };
            if ui
                .add(egui::Button::new(arrow).frame(false))
                .on_hover_text(if d.detail_box_open { "Hide the preview" } else { "Show the preview" })
                .clicked()
            {
                d.detail_box_open = !d.detail_box_open;
            }
        });
    });
    if d.detail_box_open {
        detail_box(ui, d, rotation);
    }
    ui.label(weak("Hold Alt or Shift while dragging a Sharpening slider to see what it does."));
    let defaults = d.defaults();
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut preview = None;
    let groups: [(&str, &[DetailParam]); 2] =
        [("Sharpening", &DetailParam::SHARPENING), ("Noise Reduction", &DetailParam::NOISE)];
    for (title, params) in groups {
        sub_header(ui, title, |_| {});
        for p in params {
            // Like LrC, a noise reduction's Detail / Contrast / Smoothness only matter
            // while its amount is above 0.
            let enabled = match p {
                DetailParam::LuminanceDetail | DetailParam::LuminanceContrast => st.detail.luminance > 0.0,
                DetailParam::ColorDetail | DetailParam::ColorSmoothness => st.detail.color > 0.0,
                _ => true,
            };
            let slider = Slider::Detail(*p);
            let spec = Spec {
                slider,
                range: slider.range(false),
                default: p.get(&defaults.detail),
                log: false,
                gradient: None,
                kelvin: false,
            };
            let mut v = slider.get(&st);
            let row = ui.add_enabled_ui(enabled, |ui| slider_row(ui, &spec, &mut v));
            if row.inner.changed {
                slider.set(&mut st, v);
            }
            if row.inner.done {
                done = Some(None);
            }
            if row.inner.held {
                preview = SharpenView::for_param(*p);
            }
        }
    }
    // LrC uses Alt; Shift works too (Alt is easy to hit as AltGr on some keyboards).
    let modifier = ui.input(|i| i.modifiers.alt || i.modifiers.shift);
    d.set_sharpen_view(preview.filter(|_| modifier));
    if d.sharpen_view.is_some() {
        ui.ctx().request_repaint();
    }
    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// The Detail panel's preview box: the photo at 1:1 around the chosen point. Drag to look
/// around.
fn detail_box(ui: &mut egui::Ui, d: &mut crate::develop::DevelopState, rotation: u8) {
    let ppp = ui.ctx().pixels_per_point();
    let side = (ui.available_width() - 24.0).clamp(80.0, 300.0);
    let (outer, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), side + 8.0), Sense::drag());
    let rect = Rect::from_center_size(outer.center(), egui::vec2(side, side));
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(0x60));
    ui.painter().rect_stroke(
        rect.expand(1.0),
        0.0,
        egui::Stroke::new(1.0, Color32::from_gray(0x22)),
        egui::StrokeKind::Outside,
    );
    // The developed photo's size (the box always shows the edit, even in Before).
    let Some(full) = d.info.as_ref().map(|i| d.settings.transform.output_size(i.full_size.0, i.full_size.1))
    else {
        return;
    };
    let (fw, fh) = (full.0.max(1), full.1.max(1));

    // Dragging moves the point the other way, pixel for pixel on screen.
    if resp.dragged() {
        let shown = rotated_size(egui::vec2(fw as f32, fh as f32), rotation);
        let (u, v) = rotate(d.detail_focus, rotation);
        let delta = resp.drag_delta() * ppp;
        let q = egui::vec2(u - delta.x / shown.x, v - delta.y / shown.y);
        let (x, y) = unrotate(q, rotation);
        d.detail_focus = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    // The square of full-size pixels around the point (the box is square, so rotation
    // doesn't change its size).
    let n = (side * ppp).round() as u32;
    let (w, h) = (n.min(fw), n.min(fh));
    let x = ((d.detail_focus.0 * fw as f32) as i64 - i64::from(w / 2)).clamp(0, i64::from(fw - w)) as u32;
    let y = ((d.detail_focus.1 * fh as f32) as i64 - i64::from(h / 2)).clamp(0, i64::from(fh - h)) as u32;
    let wanted = Region { x, y, w, h };
    if let Some(wait) = d.want_region(RegionTarget::DetailBox, wanted) {
        ui.ctx().request_repaint_after(wait);
    }
    match d.region_for(RegionTarget::DetailBox) {
        Some((region, tex)) => {
            let size = rotated_size(egui::vec2(region.w as f32, region.h as f32), rotation) / ppp;
            paint_image(&painter, tex, Rect::from_center_size(rect.center(), size), rotation, Color32::WHITE);
        }
        None => {
            if d.region_pending(RegionTarget::DetailBox) {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Loading…",
                    FontId::proportional(12.5),
                    theme::TEXT_STRONG,
                );
            }
        }
    }
}

/// LrC's Transform panel: the Manual Transform sliders (Vertical, Horizontal, Rotate,
/// Aspect, Scale, X / Y Offset). A grid is shown over the photo while one is dragged.
fn transform_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let defaults = d.defaults();
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut held = false;
    let mut upright = None;

    // Upright: the automatic modes analyse the photo; Guided uses guides drawn on it.
    sub_header(ui, "Upright", |_| {});
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for mode in Upright::ALL {
            let tip = match mode {
                Upright::Off => "No Upright correction",
                Upright::Auto => "Balanced: level, and perspective corrected in moderation",
                Upright::Guided => "Draw guides on the photo along lines that should be straight",
                Upright::Level => "Level the photo",
                Upright::Vertical => "Level, and make converging verticals parallel",
                Upright::Full => "Level, and correct vertical and horizontal perspective",
            };
            let current = st.transform.upright == mode;
            if ui
                .add(egui::Button::new(RichText::new(mode.label()).size(11.5)).selected(current))
                .on_hover_text(tip)
                .clicked()
            {
                upright = Some(mode);
            }
        }
    });
    if st.transform.upright == Upright::Guided {
        let n = st.transform.guides.len();
        ui.label(weak(format!(
            "Drag on the photo to draw a guide along a line that should be vertical or level ({n} of {MAX_GUIDES}). Drag an end to move it; right-click a guide to remove it."
        )));
        if n > 0 && ui.small_button("Clear Guides").clicked() {
            d.set_guides(Vec::new());
            st = d.settings.clone();
            done = Some(None);
        }
    }
    let mut constrain = st.transform.constrain_crop;
    if ui
        .checkbox(&mut constrain, "Constrain Crop")
        .on_hover_text("Crop to the largest part of the frame the transformed photo covers")
        .changed()
    {
        st.transform.constrain_crop = constrain;
        done = Some(None);
    }

    sub_header(ui, "Manual Transform", |_| {});
    for p in TransformParam::ALL {
        let slider = Slider::Transform(p);
        let spec = Spec {
            slider,
            range: slider.range(false),
            default: p.get(&defaults.transform),
            log: false,
            gradient: None,
            kelvin: false,
        };
        let mut v = slider.get(&st);
        let row = slider_row(ui, &spec, &mut v);
        if row.changed {
            slider.set(&mut st, v);
        }
        if row.done {
            done = Some(None);
        }
        held |= row.held;
    }
    d.transform_grid = held;
    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
    if let Some(mode) = upright {
        app.run(Command::DevelopUpright(mode));
    }
}

/// LrC's Effects panel: Post-Crop Vignetting (Style, Amount, Midpoint, Roundness, Feather,
/// Highlights) and Grain (Amount, Size, Roughness). There is no crop yet, so the vignette
/// follows the whole frame.
fn effects_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let defaults = d.defaults();
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;

    sub_header(ui, "Post-Crop Vignetting", |_| {});
    ui.horizontal(|ui| {
        ui.label(weak("Style:"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let before = st.effects.vignette_style;
            egui::ComboBox::from_id_salt("dev_vignette_style")
                .selected_text(before.label())
                .width(140.0)
                .show_ui(ui, |ui| {
                    for style in VignetteStyle::ALL {
                        ui.selectable_value(&mut st.effects.vignette_style, style, style.label());
                    }
                });
            if st.effects.vignette_style != before {
                done = Some(None);
            }
        });
    });
    let groups: [&[EffectParam]; 4] =
        [&EffectParam::VIGNETTE, &EffectParam::GRAIN, &EffectParam::GLOW, &EffectParam::FOG];
    for (n, params) in groups.into_iter().enumerate() {
        match n {
            1 => sub_header(ui, "Grain", |_| {}),
            2 => sub_header(ui, "Glow", |_| {}),
            3 => {
                sub_header(ui, "Fog", |_| {});
                let tip = "See the photo's depth map instead of the photo: white is near, black is far";
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut d.show_depth, "Show Depth").on_hover_text(tip).changed() {
                        d.request_renders();
                    }
                    if st.effects.fog_depth.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let tip = "Make the photo's depth map again";
                            let idle = d.ai_job.is_none();
                            if ui
                                .add_enabled(idle, egui::Button::new("Remake").small())
                                .on_hover_text(tip)
                                .clicked()
                            {
                                d.remake_depth();
                            }
                            if !idle {
                                ui.add(egui::Spinner::new().size(11.0));
                            }
                        });
                    }
                });
                // Fog goes by a depth map of the photo, made by a model the first time.
                if (st.effects.fog_amount != 0.0 || d.show_depth) && st.effects.fog_depth.is_none() {
                    ui.horizontal(|ui| {
                        if d.ai_job.is_some() {
                            ui.add(egui::Spinner::new().size(11.0));
                            ui.label(weak("Working out how far away everything is…").size(11.5));
                        } else {
                            ui.label(weak("Needs a depth map of the photo").size(11.5));
                            if d.fog_declined && ui.small_button("Make It").clicked() {
                                d.fog_declined = false;
                            }
                        }
                    });
                }
            }
            _ => {}
        }
        for p in params {
            // As in LrC, the shape sliders only matter once there is an amount; Highlights
            // only when darkening in a style that protects them.
            let e = &st.effects;
            let enabled = match p {
                EffectParam::VignetteAmount | EffectParam::GrainAmount => true,
                EffectParam::VignetteHighlights => {
                    e.vignette_amount < 0.0 && e.vignette_style != VignetteStyle::PaintOverlay
                }
                EffectParam::GrainSize | EffectParam::GrainRoughness => e.grain_amount > 0.0,
                EffectParam::GlowAmount | EffectParam::FogAmount => true,
                EffectParam::FogDistance
                | EffectParam::FogDensity
                | EffectParam::FogHeight
                | EffectParam::FogWarmth => e.fog_amount != 0.0,
                EffectParam::GlowRange | EffectParam::GlowSpread | EffectParam::GlowWarmth => {
                    e.glow_amount > 0.0
                }
                _ => e.vignette_amount != 0.0,
            };
            let slider = Slider::Effect(*p);
            let spec = Spec {
                slider,
                range: slider.range(false),
                default: p.get(&defaults.effects),
                log: false,
                gradient: match p {
                    EffectParam::VignetteAmount => Some([Color32::from_gray(20), Color32::from_gray(235)]),
                    EffectParam::GlowWarmth | EffectParam::FogWarmth => {
                        Some([Color32::from_rgb(70, 110, 220), Color32::from_rgb(230, 170, 60)])
                    }
                    _ => None,
                },
                kelvin: false,
            };
            let mut v = slider.get(&st);
            let row = ui.add_enabled_ui(enabled, |ui| slider_row(ui, &spec, &mut v));
            if row.inner.changed {
                slider.set(&mut st, v);
            }
            if row.inner.done {
                done = Some(None);
            }
        }
    }
    if st != d.settings {
        d.set_live(st);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_and_unrotate_are_inverses() {
        for r in 0..4 {
            for q in [(0.1f32, 0.7f32), (0.9, 0.2), (0.0, 1.0)] {
                let (x, y) = unrotate(egui::vec2(q.0, q.1), r);
                let back = rotate((x, y), r);
                assert!((back.0 - q.0).abs() < 1e-6 && (back.1 - q.1).abs() < 1e-6, "{r}: {q:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn unrotate_maps_display_corners_back() {
        // Turned a quarter clockwise, the photo's top-left shows at the top right.
        assert_eq!(unrotate(egui::vec2(1.0, 0.0), 1), (0.0, 0.0));
        assert_eq!(unrotate(egui::vec2(0.0, 1.0), 3), (0.0, 0.0));
        assert_eq!(unrotate(egui::vec2(1.0, 1.0), 2), (0.0, 0.0));
        assert_eq!(unrotate(egui::vec2(0.3, 0.6), 0), (0.3, 0.6));
    }

    #[test]
    fn slider_mapping_round_trips() {
        let spec = Spec {
            slider: Slider::Temp,
            range: (2000.0, 50000.0),
            default: 0.0,
            log: true,
            gradient: None,
            kelvin: true,
        };
        let t = spec.t(5500.0);
        assert!((spec.value(t) - 5500.0).abs() <= 1.0);
        let e = Spec { slider: Slider::Exposure, range: (-5.0, 5.0), log: false, ..spec };
        assert_eq!(e.value(0.5), 0.0);
        assert_eq!(e.round(0.123), 0.12);
    }
}
