//! The Develop module's Lens Corrections panel (LrC's): Profile (Remove Chromatic
//! Aberration, Enable Profile Corrections with the lens from the Lensfun database, Amount)
//! and Manual (Distortion, Constrain Crop, Defringe, Lens Vignetting).

use eframe::egui::{self, Align, Layout, RichText};
use imaging::develop::{LensSetup, Slider};

use super::develop::{LABEL_W, Spec, slider_row_as, sub_header, weak};
use crate::app::{Command, PhotoLibraryApp};
use crate::lens::LensTab;

fn spec(id: u8, range: (f32, f32), default: f32) -> Spec {
    Spec { slider: Slider::MaskAmount(id), range, default, log: false, gradient: None, kelvin: false }
}

pub(super) fn lens_panel(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    if d.photo.is_none() {
        ui.label(weak("Select a photo to develop."));
        return;
    }
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut measure = false;
    ui.horizontal(|ui| {
        for (tab, name) in [(LensTab::Profile, "Profile"), (LensTab::Manual, "Manual")] {
            if ui.selectable_label(d.lens.tab == tab, name).clicked() {
                d.lens.tab = tab;
            }
        }
    });
    ui.add_space(4.0);
    match d.lens.tab {
        LensTab::Profile => {
            let l = &mut st.transform.lens;
            let mut ca = l.remove_ca;
            let tip = "Measure the colour fringes along edges towards the corners (lateral \
                       chromatic aberration) and line the colours up. Works without a profile";
            if ui.checkbox(&mut ca, "Remove Chromatic Aberration").on_hover_text(tip).changed() {
                l.remove_ca = ca;
                if ca {
                    l.ca = None;
                    measure = true;
                } else {
                    done = Some(Some("Remove Chromatic Aberration: Off".into()));
                }
            }
            if d.lens.measuring {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.add(egui::Spinner::new().size(11.0));
                    ui.label(weak("Measuring…").size(11.5));
                });
            }
            let mut on = l.profile_enable;
            let tip = "Correct the lens's distortion, vignetting and colour fringes from its profile";
            if ui.checkbox(&mut on, "Enable Profile Corrections").on_hover_text(tip).changed() {
                l.profile_enable = on;
                if on {
                    d.lens.load(&ctx);
                    if let Some(db) = d.lens.db.clone() {
                        l.profile = d.lens.wanted(&db, l);
                        d.lens.checked = d.photo.map(|id| (id, l.clone()));
                    }
                }
                done = Some(Some(format!("Lens Profile Corrections: {}", if on { "On" } else { "Off" })));
            }
            if l.profile_enable {
                profile_box(ui, d, &mut st, &mut done, &ctx);
            }
            ui.add_space(6.0);
            database_line(ui, d, &ctx);
        }
        LensTab::Manual => {
            let l = &mut st.transform.lens;
            sub_header(ui, "Transform", |_| {});
            if slider_row_as(ui, &spec(202, (-100.0, 100.0), 0.0), "Distortion", &mut l.distortion).done {
                done = Some(None);
            }
            let mut constrain = st.transform.constrain_crop;
            if ui
                .checkbox(&mut constrain, "Constrain Crop")
                .on_hover_text("Crop to the part of the frame the corrected photo covers")
                .changed()
            {
                st.transform.constrain_crop = constrain;
                done = Some(None);
            }
            let l = &mut st.transform.lens;
            sub_header(ui, "Defringe", |_| {});
            let rows: [(u8, &str, (f32, f32), f32); 2] =
                [(203, "Purple Amount", (0.0, 20.0), 0.0), (204, "Green Amount", (0.0, 20.0), 0.0)];
            for (k, (id, name, range, default)) in rows.into_iter().enumerate() {
                let (amount, hue) = if k == 0 { (&mut l.purple_amount, &mut l.purple_hue) } else { (&mut l.green_amount, &mut l.green_hue) };
                if slider_row_as(ui, &spec(id, range, default), name, amount).done {
                    done = Some(None);
                }
                let label = if k == 0 { "Purple Hue" } else { "Green Hue" };
                let base = if k == 0 { [30.0, 70.0] } else { [40.0, 60.0] };
                ui.add_enabled_ui(*amount > 0.0, |ui| {
                    if slider_row_as(ui, &spec(id + 10, (0.0, 100.0), base[0]), label, &mut hue[0]).done {
                        done = Some(None);
                    }
                    if slider_row_as(ui, &spec(id + 20, (0.0, 100.0), base[1]), "to", &mut hue[1]).done {
                        done = Some(None);
                    }
                    if hue[0] > hue[1] - 10.0 {
                        hue[0] = (hue[1] - 10.0).max(0.0);
                    }
                });
            }
            sub_header(ui, "Lens Vignetting", |_| {});
            if slider_row_as(ui, &spec(205, (-100.0, 100.0), 0.0), "Amount", &mut l.vignette_amount).done {
                done = Some(None);
            }
            ui.add_enabled_ui(l.vignette_amount != 0.0, |ui| {
                if slider_row_as(ui, &spec(206, (0.0, 100.0), 50.0), "Midpoint", &mut l.vignette_midpoint).done {
                    done = Some(None);
                }
            });
        }
    }
    if st != d.settings {
        d.set_live(st);
    }
    if measure {
        d.measure_ca();
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// The profile: Setup, how it was found, Make / Model, focal length and aperture when the
/// photo doesn't say, and the Amount sliders.
fn profile_box(
    ui: &mut egui::Ui,
    d: &mut crate::develop::DevelopState,
    st: &mut imaging::develop::DevelopSettings,
    done: &mut Option<Option<String>>,
    ctx: &egui::Context,
) {
    let Some(db) = d.lens.db.clone() else {
        d.lens.load(ctx);
        if d.lens.busy() {
            ui.label(weak("Loading the lens database…").size(11.5));
        }
        return;
    };
    let l = &mut st.transform.lens;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W - 20.0, 18.0], egui::Label::new(weak("Setup:")));
        let before = l.setup;
        egui::ComboBox::from_id_salt("lens_setup")
            .selected_text(match l.setup {
                LensSetup::Auto => "Auto",
                LensSetup::Custom => "Custom",
            })
            .width(110.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut l.setup, LensSetup::Auto, "Auto")
                    .on_hover_text("The lens the photo says it was taken with");
                ui.selectable_value(&mut l.setup, LensSetup::Custom, "Custom")
                    .on_hover_text("A lens chosen here");
            });
        if l.setup != before {
            if l.setup == LensSetup::Auto {
                l.profile = d.lens.wanted(&db, l);
            }
            *done = Some(Some("Lens Profile Setup".into()));
        }
    });
    if !d.lens.status.is_empty() {
        ui.label(weak(d.lens.status.as_str()).size(11.5));
    }
    // Make and Model: the photo's candidates first; any lens of a maker by hand.
    let current = l.profile.clone();
    let camera = d.lens.photo.as_ref().and_then(|p| db.find_camera(&p.lens.camera_maker, &p.lens.camera_model));
    let mounts = camera.map(|c| db.compatible_mounts(&c.mount));
    let fits = |x: &lensdb::Lens| {
        x.kind == lensdb::LensType::Rectilinear
            && x.has_corrections()
            && mounts.as_ref().is_none_or(|m| x.mounts.iter().any(|mm| m.contains(mm)))
            && camera.is_none_or(|c| x.crop <= c.crop * 1.01)
    };
    let maker = d.lens.maker.clone().or_else(|| current.as_ref().map(|p| p.maker.clone()));
    let mut chosen: Option<usize> = None;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W - 20.0, 18.0], egui::Label::new(weak("Make:")));
        egui::ComboBox::from_id_salt("lens_make")
            .selected_text(maker.clone().unwrap_or_else(|| "—".into()))
            .width(150.0)
            .height(400.0)
            .show_ui(ui, |ui| {
                let mut makers: Vec<&str> = db.lenses.iter().filter(|x| fits(x)).map(|x| x.maker.as_str()).collect();
                makers.sort_by_key(|m| m.to_lowercase());
                makers.dedup();
                for m in makers {
                    if ui.selectable_label(maker.as_deref() == Some(m), m).clicked() {
                        d.lens.maker = Some(m.to_string());
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W - 20.0, 18.0], egui::Label::new(weak("Model:")));
        let shown = current.as_ref().map_or_else(|| "None".to_string(), |p| p.lens.clone());
        egui::ComboBox::from_id_salt("lens_model")
            .selected_text(RichText::new(shown).size(11.5))
            .width(ui.available_width().max(120.0))
            .height(420.0)
            .show_ui(ui, |ui| {
                // The photo's candidates, then every lens of the chosen maker.
                if !d.lens.candidates.is_empty() {
                    ui.label(weak("Fits the photo").size(11.0));
                    for m in d.lens.candidates.iter().take(12) {
                        let x = &db.lenses[m.index];
                        let on = current.as_ref().is_some_and(|p| p.lens == x.model);
                        if ui.selectable_label(on, &x.model).clicked() {
                            chosen = Some(m.index);
                        }
                    }
                    ui.separator();
                }
                if let Some(mk) = &maker {
                    let mut all: Vec<usize> =
                        (0..db.lenses.len()).filter(|&i| db.lenses[i].maker == *mk && fits(&db.lenses[i])).collect();
                    all.sort_by_key(|&i| db.lenses[i].model.to_lowercase());
                    for i in all {
                        let x = &db.lenses[i];
                        let on = current.as_ref().is_some_and(|p| p.lens == x.model);
                        if ui.selectable_label(on, &x.model).clicked() {
                            chosen = Some(i);
                        }
                    }
                } else if d.lens.candidates.is_empty() {
                    ui.label(weak("Choose the Make first").size(11.0));
                }
            });
    });
    if let Some(i) = chosen {
        let lens = &db.lenses[i];
        let (focal, aperture) = current.as_ref().map_or((None, None), |p| (Some(p.focal), Some(p.aperture)));
        let focal = focal.filter(|f| *f >= lens.min_focal && *f <= lens.max_focal);
        l.setup = LensSetup::Custom;
        l.profile = Some(d.lens.resolve_for(&db, lens, focal, aperture));
        d.lens.maker = Some(lens.maker.clone());
        d.lens.status = format!("Chosen: {}", lens.model);
        *done = Some(Some(format!("Lens Profile: {}", lens.model)));
    }
    // What the photo doesn't say: focal length (zooms) and aperture.
    let photo = d.lens.photo.clone().unwrap_or_default();
    if let Some(p) = l.profile.clone()
        && let Some(lens) = db.lenses.iter().find(|x| x.model == p.lens && x.maker == p.maker)
        && (photo.lens.focal.is_none() || photo.aperture.is_none())
    {
        let (mut focal, mut aperture) = (p.focal, p.aperture);
        let mut changed = false;
        ui.horizontal(|ui| {
            if photo.lens.focal.is_none() && lens.is_zoom() {
                ui.label(weak("Focal length:"));
                let r = ui.add(
                    egui::DragValue::new(&mut focal).range(lens.min_focal..=lens.max_focal).speed(0.5).suffix(" mm"),
                );
                changed |= r.changed() && !r.dragged() || r.drag_stopped();
            }
            if photo.aperture.is_none() {
                ui.label(weak("Aperture:"));
                let r = ui.add(egui::DragValue::new(&mut aperture).range(1.0..=32.0).speed(0.05).prefix("f/"));
                changed |= r.changed() && !r.dragged() || r.drag_stopped();
            }
        });
        if changed {
            l.profile = Some(d.lens.resolve_for(&db, lens, Some(focal), Some(aperture)));
            *done = Some(Some("Lens Profile".into()));
        }
    }
    if let Some(p) = &l.profile {
        let what: Vec<&str> = [
            p.distortion.map(|_| "distortion"),
            p.vignetting.map(|_| "vignetting"),
            p.tca.map(|_| "colour fringes"),
        ]
        .into_iter()
        .flatten()
        .collect();
        ui.label(weak(format!("Corrects {} at {:.0} mm, f/{:.1}", what.join(", "), p.focal, p.aperture)).size(11.0));
    }
    sub_header(ui, "Amount", |_| {});
    ui.add_enabled_ui(l.profile.as_ref().is_some_and(|p| p.distortion.is_some()), |ui| {
        if slider_row_as(ui, &spec(200, (0.0, 200.0), 100.0), "Distortion", &mut l.distortion_amount).done {
            *done = Some(None);
        }
    });
    ui.add_enabled_ui(l.profile.as_ref().is_some_and(|p| p.vignetting.is_some()), |ui| {
        if slider_row_as(ui, &spec(201, (0.0, 200.0), 100.0), "Vignetting", &mut l.vignetting_amount).done {
            *done = Some(None);
        }
    });
}

/// The lens database: download it, its date and Update, and the credit.
fn database_line(ui: &mut egui::Ui, d: &mut crate::develop::DevelopState, ctx: &egui::Context) {
    if let Some(p) = &d.lens.downloading {
        let f = *p.lock().unwrap_or_else(|e| e.into_inner());
        ui.add(egui::ProgressBar::new(f).desired_height(6.0).text(RichText::new("Downloading the lens database").size(10.5)));
        return;
    }
    if let Some(e) = &d.lens.error {
        ui.label(RichText::new(format!("Lens database: {e}")).size(11.0).color(egui::Color32::from_rgb(230, 120, 90)));
    }
    if !d.lens.available() {
        ui.label(weak("Lens profiles come from the Lensfun database (about 0.5 MB to download).").size(11.0));
        if ui.button("Download Lens Database").clicked() {
            d.lens.download(ctx);
        }
        return;
    }
    ui.horizontal(|ui| {
        let when = crate::lens::dir()
            .and_then(lensdb::store::timestamp)
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        ui.label(weak(format!("Lens data: Lensfun {when}")).size(10.5))
            .on_hover_text("lensfun.github.io, licensed CC BY-SA 3.0");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Update").on_hover_text("Download the newest lens database").clicked() {
                d.lens.download(ctx);
            }
        });
    });
}
