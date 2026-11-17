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
use imaging::develop::{Region, Slider, Treatment, WbPreset};

use super::panels::Side;
use super::shortcuts::Action;
use super::theme;
use super::views::{paint_image, rotated_size};
use crate::app::{Command, PhotoLibraryApp};
use crate::develop::Slot;

const LEFT: &[&str] = &["Navigator", "Presets", "Snapshots", "History", "Collections"];
const PLANNED_RIGHT: &[&str] =
    &["Tone Curve", "HSL / Color", "Color Grading", "Detail", "Lens Corrections", "Transform", "Effects"];
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

fn weak(text: impl Into<String>) -> RichText {
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

pub fn left(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut stack = std::mem::take(&mut app.ui.dev_left);
                stack.section(ui, Side::Left, "Navigator", LEFT, true, |ui| navigator(app, ui));
                stack.section(ui, Side::Left, "Presets", LEFT, false, |ui| {
                    ui.label(weak("Develop presets come in a later version."));
                });
                stack.section(ui, Side::Left, "Snapshots", LEFT, false, |ui| {
                    ui.label(weak("Snapshots come in a later version."));
                });
                stack.section(ui, Side::Left, "History", LEFT, true, |ui| history(app, ui));
                stack.section(ui, Side::Left, "Collections", LEFT, false, |ui| {
                    super::collections::panel(app, ui)
                });
                app.ui.dev_left = stack;
            });
        });
    });
}

/// The Library's Navigator (zoom levels, visible area), showing the photo as developed.
fn navigator(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let shown = app.session.as_ref().and_then(|s| s.develop.textures.get(&Slot::Current).cloned());
    super::sidebars::navigator(app, ui, shown);
}

/// LrC's History: newest step first; click one to go back to it.
fn history(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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

pub fn right(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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
                stack.section(ui, Side::Right, "Basic", RIGHT, true, |ui| basic(app, ui));
                for title in PLANNED_RIGHT {
                    stack.section(ui, Side::Right, title, RIGHT, false, |ui| {
                        ui.label(weak("Comes in a later version."));
                    });
                }
                app.ui.dev_right = stack;
            });
        });
    });
}

/// How a slider row looks and behaves.
struct Spec {
    slider: Slider,
    range: (f32, f32),
    default: f32,
    /// Logarithmic track (Kelvin).
    log: bool,
    /// Colours at the track's ends (Temp, Tint, Vibrance, Saturation).
    gradient: Option<[Color32; 2]>,
    kelvin: bool,
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
        let step = if self.slider.decimals() == 2 { 0.01 } else { 1.0 };
        ((v / step).round() * step).clamp(self.range.0, self.range.1)
    }
}

#[derive(Default)]
struct Moved {
    changed: bool,
    /// The change is finished (slider released, value typed, reset): save it.
    done: bool,
    /// Double-clicked (label or track): back to the default.
    reset: bool,
}

const LABEL_W: f32 = 74.0;
const VALUE_W: f32 = 48.0;

/// One LrC slider row: label (double-click resets), track, value (drag or type).
fn slider_row(ui: &mut egui::Ui, spec: &Spec, value: &mut f32) -> Moved {
    let mut out = Moved::default();
    let enabled = ui.is_enabled();
    let id = ui.scope_id().with(("dev_slider", spec.slider.label()));
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
        spec.slider.label(),
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
        out = Moved { changed: *value != spec.default, done: true, reset: true };
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
    } else if slider.decimals() == 2 {
        0.01
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
    if resp.drag_stopped() {
        out.done = true;
    }
    out
}

fn sub_header(ui: &mut egui::Ui, title: &str, add_right: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(12.5).color(theme::TEXT_STRONG));
        ui.with_layout(Layout::right_to_left(Align::Center), add_right);
    });
}

/// LrC's Basic panel.
fn basic(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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
fn clipping_triangles(app: &mut PhotoLibraryApp, ui: &mut egui::Ui, hist: Rect) {
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
pub fn center(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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

    // Size of the photo in pixels as displayed (file orientation, then catalog rotation).
    let full_size = s.develop.info.as_ref().map(|i| egui::vec2(i.full_size.0 as f32, i.full_size.1 as f32));
    let img_px = rotated_size(full_size.unwrap_or_else(|| tex.size_vec2()), photo.rotation);
    let zoomed = app.zoom.zoomed && !photo.missing && !s.develop.picking_wb;
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
        if resp.dragged() {
            z.center -= resp.drag_delta() / size;
        }
        if resp.hovered() {
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
            && let Some(wait) = s.develop.want_region(wanted)
        {
            ui.ctx().request_repaint_after(wait);
        }
        match s.develop.region_for_view() {
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
    } else if zoomed && !sharp && s.develop.region_pending() {
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

/// A point (0..1) of the photo before the catalog's `rotation` clockwise quarter turns →
/// the same point as displayed (the inverse of [`unrotate`]).
fn rotate((u, v): (f32, f32), rotation: u8) -> (f32, f32) {
    match rotation % 4 {
        1 => (1.0 - v, u),
        2 => (1.0 - u, 1.0 - v),
        3 => (v, 1.0 - u),
        _ => (u, v),
    }
}

/// A point (0..1) of the photo as displayed → the same point before the catalog's
/// `rotation` clockwise quarter turns.
fn unrotate(q: Vec2, rotation: u8) -> (f32, f32) {
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

pub fn toolbar(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy {
                ui.add(egui::Spinner::new().size(12.0));
            }
        });
    });
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
