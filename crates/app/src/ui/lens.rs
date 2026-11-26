//! The Develop module's Lens Corrections panel (LrC's): Profile (Remove Chromatic
//! Aberration, Enable Profile Corrections with the lens from the Lensfun database, Amount)
//! and Manual (Distortion, Constrain Crop, Defringe, Lens Vignetting).

use eframe::egui::{self, Align, Layout, RichText};
use imaging::develop::{LensSetup, Slider};

use super::develop::{LABEL_W, Spec, slider_row_as, sub_header, weak};
use super::hotkeys::{self, Hot};
use crate::app::{Command, FotaroApp};
use crate::lens::LensTab;

fn spec(id: u8, range: (f32, f32), default: f32) -> Spec {
    Spec { slider: Slider::MaskAmount(id), range, default, log: false, gradient: None, kelvin: false }
}

pub(super) fn lens_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
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
        for (tab, name, id) in [
            (LensTab::Profile, "Profile", "develop.lens.tab.profile"),
            (LensTab::Manual, "Manual", "develop.lens.tab.manual"),
        ] {
            if ui.selectable_label(d.lens.tab == tab, name).hot(id) {
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
            if ui
                .checkbox(&mut ca, "Remove Chromatic Aberration")
                .on_hover_text(tip)
                .hot_flip("develop.lens.ca", &mut ca)
                .changed()
            {
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
            if ui
                .checkbox(&mut on, "Enable Profile Corrections")
                .on_hover_text(tip)
                .hot_flip("develop.lens.profile", &mut on)
                .changed()
            {
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
                profile_box(ui, d, &mut st, &mut done, &ctx, &mut app.ui.recent_lenses);
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
                .hot_flip("develop.lens.constrain", &mut constrain)
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
                let (amount, hue) = if k == 0 {
                    (&mut l.purple_amount, &mut l.purple_hue)
                } else {
                    (&mut l.green_amount, &mut l.green_hue)
                };
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
                if slider_row_as(ui, &spec(206, (0.0, 100.0), 50.0), "Midpoint", &mut l.vignette_midpoint)
                    .done
                {
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

/// The profile: Setup, how it was found, the lens (one list to search), focal length and
/// aperture when the photo doesn't say, and the Amount sliders.
fn profile_box(
    ui: &mut egui::Ui,
    d: &mut crate::develop::DevelopState,
    st: &mut imaging::develop::DevelopSettings,
    done: &mut Option<Option<String>>,
    ctx: &egui::Context,
    recent: &mut Vec<(String, String)>,
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
        for (setup, id) in
            [(LensSetup::Auto, "develop.lens.setup.auto"), (LensSetup::Custom, "develop.lens.setup.custom")]
        {
            if hotkeys::fired(ui.ctx(), id) {
                l.setup = setup;
            }
        }
        egui::ComboBox::from_id_salt("lens_setup")
            .selected_text(match l.setup {
                LensSetup::Auto => "Auto",
                LensSetup::Custom => "Custom",
            })
            .width(110.0)
            .show_ui(ui, |ui| {
                // key: develop.lens.setup.auto
                ui.selectable_value(&mut l.setup, LensSetup::Auto, "Auto")
                    .on_hover_text("The lens the photo says it was taken with");
                // key: develop.lens.setup.custom
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
    // The lens: one list to search, with the photo's candidates and recent choices first.
    let current = l.profile.clone();
    let camera =
        d.lens.photo.as_ref().and_then(|p| db.find_camera(&p.lens.camera_maker, &p.lens.camera_model));
    let mounts = camera.filter(|_| !d.lens.any_mount).map(|c| db.compatible_mounts(&c.mount));
    let fits = |x: &lensdb::Lens| {
        x.kind == lensdb::LensType::Rectilinear
            && x.has_corrections()
            && mounts.as_ref().is_none_or(|m| x.mounts.iter().any(|mm| m.contains(mm)))
            && camera.is_none_or(|c| x.crop <= c.crop * 1.01)
    };
    let mut chosen: Option<usize> = None;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W - 20.0, 18.0], egui::Label::new(weak("Lens:")));
        let shown = current.as_ref().map_or_else(|| "Choose a lens…".to_string(), |p| p.lens.clone());
        let resp = ui.add_sized(
            [ui.available_width().max(120.0), 20.0],
            egui::Button::new(RichText::new(shown).size(11.5)).truncate(), // no hotkey: opens the list of lenses to pick from
        );
        if resp.clicked() {
            d.lens.search.clear();
            d.lens.pick = 0;
            d.lens.focus_search = true;
        }
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(340.0);
            ui.set_max_width(340.0);
            let now = current.as_ref().map(|p| p.lens.as_str());
            chosen = lens_picker(ui, &mut d.lens, &db, &fits, now, recent, camera.is_some());
            if chosen.is_some() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                ui.close();
            }
        });
    });
    if let Some(i) = chosen {
        let lens = &db.lenses[i];
        let (focal, aperture) = current.as_ref().map_or((None, None), |p| (Some(p.focal), Some(p.aperture)));
        let focal = focal.filter(|f| *f >= lens.min_focal && *f <= lens.max_focal);
        l.setup = LensSetup::Custom;
        l.profile = Some(d.lens.resolve_for(&db, lens, focal, aperture));
        d.lens.status = format!("Chosen: {}", lens.model);
        let key = (lens.maker.clone(), lens.model.clone());
        recent.retain(|r| *r != key);
        recent.insert(0, key);
        recent.truncate(8);
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
                    egui::DragValue::new(&mut focal)
                        .range(lens.min_focal..=lens.max_focal)
                        .speed(0.5)
                        .suffix(" mm"),
                );
                changed |= r.changed() && !r.dragged() || r.drag_stopped();
            }
            if photo.aperture.is_none() {
                ui.label(weak("Aperture:"));
                let r =
                    ui.add(egui::DragValue::new(&mut aperture).range(1.0..=32.0).speed(0.05).prefix("f/"));
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
        ui.label(
            weak(format!("Corrects {} at {:.0} mm, f/{:.1}", what.join(", "), p.focal, p.aperture))
                .size(11.0),
        );
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

/// How a lens reads in the list: its model, with the maker in front when the model
/// doesn't already start with it.
fn lens_name(x: &lensdb::Lens) -> String {
    if x.model.to_lowercase().starts_with(&x.maker.to_lowercase()) {
        x.model.clone()
    } else {
        format!("{} {}", x.maker, x.model)
    }
}

/// How well a lens answers a search (`words`, lower case): every word must be somewhere
/// in its maker, model or other names; a word found at the start of one counts double
/// ("50" is better in "50mm" than in "150mm"). `None`: not an answer.
fn search_score(words: &[String], x: &lensdb::Lens) -> Option<u32> {
    let hay = format!("{} {} {}", x.maker, x.model, x.names.join(" ")).to_lowercase();
    let mut score = 0;
    for w in words {
        let mut any = false;
        let mut starts = false;
        for (i, _) in hay.match_indices(w.as_str()) {
            any = true;
            let before = hay[..i].chars().next_back();
            starts |= before.is_none_or(|c| !c.is_alphanumeric() && c != '.');
        }
        if !any {
            return None;
        }
        score += if starts { 2 } else { 1 };
    }
    Some(score)
}

/// The lens list: a search box (type any part of the make or model: "nikon 50 1.8"), the
/// answers best first, with the arrow keys and Enter; without a search, the lenses that
/// fit the photo, those chosen lately, and every maker to open. Returns the lens chosen.
fn lens_picker(
    ui: &mut egui::Ui,
    state: &mut crate::lens::LensState,
    db: &lensdb::Database,
    fits: &dyn Fn(&lensdb::Lens) -> bool,
    current: Option<&str>,
    recent: &[(String, String)],
    has_camera: bool,
) -> Option<usize> {
    let mut chosen = None;
    let edit = ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Search, e.g. nikon 50 1.8")
            .desired_width(f32::INFINITY),
    );
    if std::mem::take(&mut state.focus_search) {
        edit.request_focus();
    }
    if edit.changed() {
        state.pick = 0;
    }
    let row = |ui: &mut egui::Ui, i: usize, lit: bool| {
        let x = &db.lenses[i];
        let on = lit || current == Some(x.model.as_str());
        ui.selectable_label(on, RichText::new(lens_name(x)).size(11.5)) // no hotkey: one per lens
    };
    let words: Vec<String> = state.search.to_lowercase().split_whitespace().map(String::from).collect();
    ui.add_space(2.0);
    if !words.is_empty() {
        let mut found: Vec<(u32, usize)> = db
            .lenses
            .iter()
            .enumerate()
            .filter(|(_, x)| fits(x))
            .filter_map(|(i, x)| search_score(&words, x).map(|s| (s, i)))
            .collect();
        found.sort_by_cached_key(|(s, i)| (std::cmp::Reverse(*s), lens_name(&db.lenses[*i]).to_lowercase()));
        const SHOWN: usize = 80;
        let shown = found.len().min(SHOWN);
        let (down, up) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            )
        });
        if down {
            state.pick += 1;
        }
        if up {
            state.pick = state.pick.saturating_sub(1);
        }
        state.pick = state.pick.min(shown.saturating_sub(1));
        if shown > 0 && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            chosen = Some(found[state.pick].1);
        }
        egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if found.is_empty() {
                ui.label(weak("No lens with all of those words").size(11.0));
            }
            for (k, (_, i)) in found.iter().take(SHOWN).enumerate() {
                let r = row(ui, *i, k == state.pick);
                if k == state.pick && (down || up) {
                    r.scroll_to_me(None);
                }
                if r.clicked() {
                    chosen = Some(*i);
                }
            }
            if found.len() > SHOWN {
                ui.label(
                    weak(format!("{} more: type more to narrow it down", found.len() - SHOWN)).size(11.0),
                );
            }
        });
    } else {
        egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if !state.candidates.is_empty() {
                ui.label(weak("Fits the photo").size(11.0));
                for m in state.candidates.iter().take(8) {
                    if row(ui, m.index, false).clicked() {
                        chosen = Some(m.index);
                    }
                }
                ui.separator();
            }
            let lately: Vec<usize> = recent
                .iter()
                .filter_map(|(maker, model)| {
                    db.lenses.iter().position(|x| x.maker == *maker && x.model == *model)
                })
                .collect();
            if !lately.is_empty() {
                ui.label(weak("Chosen lately").size(11.0));
                for i in lately {
                    if row(ui, i, false).clicked() {
                        chosen = Some(i);
                    }
                }
                ui.separator();
            }
            // Every maker, closed: open one to see its lenses.
            let mut makers: std::collections::BTreeMap<String, (&str, Vec<usize>)> = Default::default();
            for (i, x) in db.lenses.iter().enumerate().filter(|(_, x)| fits(x)) {
                makers.entry(x.maker.to_lowercase()).or_insert((x.maker.as_str(), Vec::new())).1.push(i);
            }
            ui.label(weak("All makes").size(11.0));
            for (maker, mut lenses) in makers.into_values() {
                egui::CollapsingHeader::new(RichText::new(format!("{maker}  ({})", lenses.len())).size(11.5))
                    .id_salt(("lens_maker", maker))
                    .show(ui, |ui| {
                        lenses.sort_by_cached_key(|i| db.lenses[*i].model.to_lowercase());
                        for i in lenses {
                            if row(ui, i, false).clicked() {
                                chosen = Some(i);
                            }
                        }
                    });
            }
        });
    }
    if has_camera {
        ui.separator();
        ui.checkbox(&mut state.any_mount, RichText::new("Lenses for other mounts too").size(11.5)) // no hotkey: in the list of lenses
            .on_hover_text(
                "For a lens used on an adapter: list every lens, not only those for this camera's mount",
            );
    }
    chosen
}

/// The lens database: download it, its date and Update, and the credit.
fn database_line(ui: &mut egui::Ui, d: &mut crate::develop::DevelopState, ctx: &egui::Context) {
    if let Some(p) = &d.lens.downloading {
        let f = *p.lock().unwrap_or_else(|e| e.into_inner());
        ui.add(
            egui::ProgressBar::new(f)
                .desired_height(6.0)
                .text(RichText::new("Downloading the lens database").size(10.5)),
        );
        return;
    }
    if let Some(e) = &d.lens.error {
        ui.label(
            RichText::new(format!("Lens database: {e}"))
                .size(11.0)
                .color(egui::Color32::from_rgb(230, 120, 90)),
        );
    }
    if !d.lens.available() {
        ui.label(weak("Lens profiles come from the Lensfun database (about 0.5 MB to download).").size(11.0));
        if ui.button("Download Lens Database").hot("develop.lens.download") {
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
            if ui
                .small_button("Update")
                .on_hover_text("Download the newest lens database")
                .hot("develop.lens.update")
            {
                d.lens.download(ctx);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lens_search_needs_every_word_and_likes_whole_numbers() {
        let lens = |maker: &str, model: &str| lensdb::Lens {
            maker: maker.into(),
            model: model.into(),
            ..Default::default()
        };
        let words = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
        let fifty = lens("Nikon", "Nikon AF-S Nikkor 50mm f/1.8G");
        let long = lens("Sigma", "Sigma 150mm f/2.8 EX DG");
        assert!(search_score(&words("nikon 50 1.8"), &fifty).is_some());
        assert!(search_score(&words("nikon 50 1.4"), &fifty).is_none(), "every word must be there");
        assert!(search_score(&words("nikon 50"), &long).is_none());
        let (a, b) =
            (search_score(&words("50"), &fifty).unwrap(), search_score(&words("50"), &long).unwrap());
        assert!(a > b, "50 is a better answer in 50mm than in 150mm");
        assert_eq!(lens_name(&fifty), "Nikon AF-S Nikkor 50mm f/1.8G");
        assert_eq!(lens_name(&lens("Nikon", "Nikkor 50mm")), "Nikon Nikkor 50mm");
    }
}
