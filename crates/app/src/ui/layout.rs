//! Window layout, following Lightroom Classic's Library module:
//!
//! ```text
//! ┌ menu bar ────────────────────────────────────────────────────────────┐
//! ├ identity plate / activity ─────────────────────────── module picker ─┤  F5
//! ├ left panels ─┬ Library Filter bar (\) ─────────────────┬ right panels┤
//! │ Navigator    │                                          │ Histogram   │
//! │ Catalog      │            Grid / Loupe                  │ Keywording  │  F7 / F8, Tab
//! │ Folders      │                                          │ Keyword List│
//! │ Collections  ├ toolbar (T) ─────────────────────────────┤ Metadata    │
//! ├ filmstrip ───┴──────────────────────────────────────────┴─────────────┤  F6
//! └──────────────────────────────────────────────────────────────────────┘
//! ```

use std::time::Duration;

use eframe::egui::{self, Align, Align2, Color32, Layout, RichText};

use super::shortcuts::{Action, SHORTCUTS, Status};
use super::{sidebars, theme, views};
use crate::app::{Command, Dialog, Lights, PhotoLibraryApp, View};
use catalog::Flag;

const TOAST_TIME: Duration = Duration::from_millis(1400);

/// Shortcut label for an action, for display next to menu items.
fn shortcut_for(action: Action) -> Option<String> {
    SHORTCUTS.iter().find(|s| s.action == action).map(|s| s.binding.label())
}

/// A menu item that runs a shortcut action, showing its key binding.
fn menu_action(ui: &mut egui::Ui, app: &mut PhotoLibraryApp, text: &str, action: Action) {
    let planned =
        SHORTCUTS.iter().find(|s| s.action == action).is_some_and(|s| matches!(s.status, Status::Planned(_)));
    let mut button = egui::Button::new(text);
    if let Some(sc) = shortcut_for(action) {
        button = button.shortcut_text(sc);
    }
    let resp = ui.add_enabled(!planned, button);
    let resp = if planned { resp.on_disabled_hover_text("Not available yet") } else { resp };
    if resp.clicked() {
        app.run(Command::Action(action));
        ui.close();
    }
}

fn menu_cmd(ui: &mut egui::Ui, app: &mut PhotoLibraryApp, text: &str, enabled: bool, cmd: Command) {
    if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
        app.run(cmd);
        ui.close();
    }
}

fn menu_toggle(ui: &mut egui::Ui, app: &mut PhotoLibraryApp, text: &str, on: bool, action: Action) {
    let mut button = egui::Button::new(text).selected(on);
    if let Some(sc) = shortcut_for(action) {
        button = button.shortcut_text(sc);
    }
    if ui.add(button).clicked() {
        app.run(Command::Action(action));
        ui.close();
    }
}

fn menu_bar(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let has_catalog = app.session.is_some();
    let active_path = app.session.as_ref().and_then(|s| s.active_photo()).map(|p| p.path.clone());
    let folder_source = app.session.as_ref().and_then(|s| match s.source {
        catalog::Source::Folder { id, .. } => Some(id),
        _ => None,
    });

    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            menu_cmd(ui, app, "New Catalog…", true, Command::NewCatalog);
            menu_cmd(ui, app, "Open Catalog…", true, Command::OpenCatalog);
            ui.separator();
            if has_catalog {
                menu_action(ui, app, "Import Photos…", Action::Import);
                menu_action(ui, app, "Export…", Action::Export);
                menu_action(ui, app, "Export with Previous", Action::ExportWithPrevious);
            }
            ui.separator();
            menu_cmd(ui, app, "Exit", true, Command::Quit);
        });
        ui.menu_button("Edit", |ui| {
            menu_action(ui, app, "Select All", Action::SelectAll);
            menu_action(ui, app, "Select None", Action::SelectNone);
        });
        ui.menu_button("Library", |ui| {
            let sync = folder_source.map_or(Command::Planned("", ""), Command::SyncFolder);
            menu_cmd(ui, app, "Synchronize Folder…", folder_source.is_some(), sync);
            if ui.checkbox(&mut app.ui.include_subfolders, "Show Photos in Subfolders").changed()
                && let Some(s) = &app.session
                && let catalog::Source::Folder { id, .. } = s.source
            {
                let source = catalog::Source::Folder { id, include_subfolders: app.ui.include_subfolders };
                app.run(Command::SetSource(source));
            }
            ui.separator();
            menu_cmd(
                ui,
                app,
                "New Collection…",
                has_catalog,
                Command::CreateCollection { parent: None, kind: catalog::CollectionKind::Collection },
            );
            menu_cmd(
                ui,
                app,
                "New Smart Collection…",
                has_catalog,
                Command::CreateCollection { parent: None, kind: catalog::CollectionKind::Smart },
            );
            menu_cmd(
                ui,
                app,
                "New Collection Set…",
                has_catalog,
                Command::CreateCollection { parent: None, kind: catalog::CollectionKind::Set },
            );
            ui.separator();
            menu_action(ui, app, "Find", Action::Find);
            let filters_on = app.ui.filter.enabled;
            menu_toggle(ui, app, "Enable Filters", filters_on, Action::ToggleFilters);
            ui.separator();
            menu_action(ui, app, "Show Quick Collection", Action::ShowQuickCollection);
            menu_action(ui, app, "Clear Quick Collection", Action::ClearQuickCollection);
            ui.separator();
            let separate =
                app.session.as_ref().is_some_and(|s| s.catalog.separate_raw_jpeg().unwrap_or(false));
            if ui
                .add_enabled(
                    has_catalog,
                    egui::Button::new("Treat JPEG Files Next to Raw Files as Separate Photos")
                        .selected(separate),
                )
                .on_hover_text("Applies to photos added from now on (as in Lightroom Classic)")
                .clicked()
            {
                app.run(Command::ToggleSeparateRawJpeg);
                ui.close();
            }
            ui.separator();
            menu_action(ui, app, "Previous Photo", Action::Previous);
            menu_action(ui, app, "Next Photo", Action::Next);
            ui.separator();
            ui.menu_button("Previews", |ui| {
                let any = app.session.as_ref().is_some_and(|s| !s.selected.is_empty());
                menu_cmd(ui, app, "Build Standard-Sized Previews", any, Command::BuildPreviews);
            });
        });
        ui.menu_button("Photo", |ui| {
            let reveal = active_path.clone().map_or(Command::Planned("", ""), Command::Reveal);
            menu_cmd(ui, app, crate::platform::REVEAL_LABEL, active_path.is_some(), reveal);
            ui.separator();
            ui.menu_button("Set Flag", |ui| {
                menu_action(ui, app, "Flagged", Action::SetFlag { flag: Flag::Picked, advance: false });
                menu_action(ui, app, "Unflagged", Action::SetFlag { flag: Flag::Unflagged, advance: false });
                menu_action(ui, app, "Rejected", Action::SetFlag { flag: Flag::Rejected, advance: false });
            });
            ui.menu_button("Set Rating", |ui| {
                menu_action(ui, app, "None", Action::SetRating { stars: 0, advance: false });
                for (n, label) in
                    ["1 Star", "2 Stars", "3 Stars", "4 Stars", "5 Stars"].into_iter().enumerate()
                {
                    menu_action(ui, app, label, Action::SetRating { stars: n as u8 + 1, advance: false });
                }
            });
            ui.menu_button("Set Color Label", |ui| {
                for (label, name) in
                    [("red", "Red"), ("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue")]
                {
                    menu_action(ui, app, name, Action::ColorLabel { label, advance: false });
                }
            });
            ui.separator();
            menu_action(ui, app, "Create Virtual Copy", Action::CreateVirtualCopy);
            menu_action(ui, app, "Add to Quick Collection", Action::QuickCollection);
            ui.menu_button("Stacking", |ui| {
                menu_action(ui, app, "Group into Stack", Action::Stack);
                menu_action(ui, app, "Unstack", Action::Unstack);
                menu_action(ui, app, "Collapse / Expand Stack", Action::ToggleStack);
                menu_action(ui, app, "Move to Top of Stack", Action::MoveToTopOfStack);
            });
            ui.separator();
            menu_action(ui, app, "Rotate Left (CCW)", Action::RotateLeft);
            menu_action(ui, app, "Rotate Right (CW)", Action::RotateRight);
            ui.separator();
            let auto = app.ui.auto_advance;
            menu_toggle(ui, app, "Auto Advance", auto, Action::ToggleAutoAdvance);
            ui.separator();
            ui.menu_button("Edit In", |ui| {
                menu_action(ui, app, "Edit in Default Editor…", Action::EditIn);
                menu_cmd(ui, app, "Edit in NegPy…", false, Command::Planned("", ""));
                menu_cmd(ui, app, "Edit in Luminar…", false, Command::Planned("", ""));
            });
        });
        ui.menu_button("Metadata", |ui| {
            menu_action(ui, app, "Add Keywords…", Action::Keywording);
            ui.separator();
            menu_action(ui, app, "Save Metadata to Files", Action::SaveMetadata);
            menu_cmd(ui, app, "Read Metadata from Files", has_catalog, Command::ReadMetadata);
            ui.separator();
            let auto = app.ui.auto_write_xmp;
            if ui
                .add(egui::Button::new("Automatically Write Changes into XMP").selected(auto))
                .on_hover_text("Write ratings, labels, keywords and IPTC to XMP sidecars as you change them")
                .clicked()
            {
                app.run(Command::ToggleAutoWriteXmp);
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            menu_toggle(ui, app, "Grid", app.view == View::Grid, Action::GridView);
            menu_toggle(ui, app, "Loupe", app.view == View::Loupe, Action::LoupeView);
            menu_toggle(ui, app, "Compare", app.view == View::Compare, Action::CompareView);
            menu_toggle(ui, app, "Survey", app.view == View::Survey, Action::SurveyView);
            ui.separator();
            menu_action(ui, app, "Toggle Zoom View", Action::ZoomToggle);
            menu_action(ui, app, "Zoom In", Action::ZoomIn);
            menu_action(ui, app, "Zoom Out", Action::ZoomOut);
            ui.separator();
            menu_action(ui, app, "Full Screen Preview", Action::FullScreenPreview);
            ui.menu_button("Lights Out", |ui| {
                for (l, name) in
                    [(Lights::On, "Lights On"), (Lights::Dim, "Lights Dim"), (Lights::Off, "Lights Off")]
                {
                    if ui.add(egui::Button::new(name).selected(app.lights == l)).clicked() {
                        app.run(Command::SetLights(l));
                        ui.close();
                    }
                }
            });
            ui.separator();
            menu_toggle(ui, app, "Show Toolbar", app.ui.show_toolbar, Action::ToggleToolbar);
            menu_toggle(ui, app, "Show Filter Bar", app.ui.show_filter_bar, Action::ToggleFilterBar);
            menu_action(ui, app, "Cycle Grid View Style", Action::CycleGridStyle);
            menu_action(ui, app, "Cycle Loupe Info", Action::CycleInfoOverlay);
        });
        ui.menu_button("Window", |ui| {
            ui.menu_button("Panels", |ui| {
                menu_toggle(ui, app, "Show Module Picker", app.ui.show_top, Action::ToggleTopPanel);
                menu_toggle(ui, app, "Show Filmstrip", app.ui.show_filmstrip, Action::ToggleFilmstrip);
                menu_toggle(ui, app, "Show Left Panels", app.ui.show_left, Action::ToggleLeftPanel);
                menu_toggle(ui, app, "Show Right Panels", app.ui.show_right, Action::ToggleRightPanel);
                ui.separator();
                menu_action(ui, app, "Toggle Side Panels", Action::ToggleSidePanels);
                menu_action(ui, app, "Toggle All Panels", Action::ToggleAllPanels);
            });
        });
        ui.menu_button("Help", |ui| {
            menu_action(ui, app, "Library Module Shortcuts", Action::ShowShortcuts);
        });
    });
}

/// Identity plate (left) with background activity, and the module picker (right).
fn top_panel(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.add_space(14.0);
        let progress = app.session.as_ref().map(|s| s.jobs.progress()).unwrap_or_default();
        if let Some((done, total)) = progress.import {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Importing {done} of {total} photos"))
                            .size(11.0)
                            .color(theme::TEXT),
                    );
                    if ui.small_button("✕").on_hover_text("Stop the import").clicked() {
                        app.run(Command::CancelImport);
                    }
                });
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(220.0)
                        .desired_height(6.0),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        } else if let Some((done, total)) = progress.export {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Exporting {done} of {total} photos"))
                            .size(11.0)
                            .color(theme::TEXT),
                    );
                    if ui.small_button("✕").on_hover_text("Stop the export").clicked() {
                        app.run(Command::CancelExport);
                    }
                });
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(220.0)
                        .desired_height(6.0),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        } else if let Some(label) = &progress.syncing {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new(label).color(theme::TEXT));
        } else if let Some((done, total)) = progress.previews {
            ui.vertical(|ui| {
                ui.label(RichText::new("Building Previews").size(11.0).color(theme::TEXT));
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(200.0)
                        .desired_height(6.0)
                        .text(RichText::new(format!("{done} / {total}")).size(9.0)),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            ui.label(RichText::new("Photo Library").size(20.0).color(theme::TEXT_WEAK));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(18.0);
            // Only the Library module exists; editing happens in external apps.
            ui.label(RichText::new("Library").size(19.0).color(theme::MODULE_ACTIVE));
        });
    });
}

fn welcome(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.25);
        ui.label(RichText::new("Photo Library").size(30.0).color(theme::TEXT_STRONG));
        ui.add_space(6.0);
        ui.label("A catalog keeps track of your photos, ratings and keywords.\nYour original files are never changed.");
        ui.add_space(18.0);
        let default = PhotoLibraryApp::default_catalog_dir();
        let exists = default.join("catalog.db").is_file();
        let label = if exists { "Open Default Catalog" } else { "Create Catalog" };
        if ui.add(egui::Button::new(RichText::new(label).size(14.0)).min_size(egui::vec2(220.0, 30.0))).clicked() {
            app.run(Command::CreateDefaultCatalog);
        }
        ui.label(RichText::new(default.display().to_string()).size(10.5).color(theme::TEXT_WEAK));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 230.0).max(0.0) / 2.0);
            if ui.button("New Catalog…").clicked() {
                app.run(Command::NewCatalog);
            }
            if ui.button("Open Catalog…").clicked() {
                app.run(Command::OpenCatalog);
            }
        });
    });
}

/// Small arrow at a window edge that re-opens a hidden panel (LrC's panel triangles).
fn edge_toggle(ctx: &egui::Context, id: &str, pos: egui::Pos2, anchor: Align2, text: &str) -> bool {
    egui::Area::new(egui::Id::unique(id))
        .order(egui::Order::Foreground)
        .pivot(anchor)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui.add(egui::Button::new(RichText::new(text).size(9.0).color(theme::TEXT_WEAK)).frame(false))
                .on_hover_text("Show panel")
                .clicked()
        })
        .inner
}

fn panel_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill)
}

pub fn draw(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let full = ui.max_rect();
    app.panel_rects.clear();

    let r = egui::Panel::top("menu_bar")
        .frame(panel_frame(theme::TOP_BAR).inner_margin(egui::Margin::symmetric(6, 2)))
        .show(ui, |ui| menu_bar(app, ui));
    app.panel_rects.push(r.response.rect);

    if app.ui.show_top {
        let r = egui::Panel::top("identity_plate")
            .exact_size(46.0)
            .frame(panel_frame(theme::TOP_BAR))
            .show(ui, |ui| top_panel(app, ui));
        app.panel_rects.push(r.response.rect);
    }

    if app.session.is_none() {
        egui::CentralPanel::default().frame(panel_frame(theme::CENTER)).show(ui, |ui| welcome(app, ui));
        overlays(app, &ctx, full);
        return;
    }

    let mut show = app.ui.show_filmstrip;
    if let Some(r) = egui::Panel::bottom("filmstrip")
        .resizable(true)
        .default_size(118.0)
        .size_range(80.0..=220.0)
        .frame(panel_frame(theme::PANEL))
        .show_collapsible(ui, &mut show, |ui| timed("filmstrip", || views::filmstrip(app, ui)))
    {
        app.panel_rects.push(r.response.rect);
    }
    app.ui.show_filmstrip = show;

    let mut show = app.ui.show_left;
    if let Some(r) = egui::Panel::left("left_panels")
        .resizable(true)
        .default_size(250.0)
        .size_range(190.0..=460.0)
        .frame(panel_frame(theme::PANEL))
        .show_collapsible(ui, &mut show, |ui| timed("left panels", || sidebars::left(app, ui)))
    {
        app.panel_rects.push(r.response.rect);
    }
    app.ui.show_left = show;

    let mut show = app.ui.show_right;
    if let Some(r) = egui::Panel::right("right_panels")
        .resizable(true)
        .default_size(270.0)
        .size_range(210.0..=460.0)
        .frame(panel_frame(theme::PANEL))
        .show_collapsible(ui, &mut show, |ui| timed("right panels", || sidebars::right(app, ui)))
    {
        app.panel_rects.push(r.response.rect);
    }
    app.ui.show_right = show;

    let central = egui::CentralPanel::default().frame(panel_frame(theme::CENTER)).show(ui, |ui| {
        if app.view == View::Grid && app.ui.show_filter_bar {
            let r = egui::Panel::top("filter_bar")
                .exact_size(super::filter_bar::height(&app.ui.filter))
                .frame(panel_frame(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 0)))
                .show(ui, |ui| super::filter_bar::show(app, ui));
            app.panel_rects.push(r.response.rect);
        }
        if app.ui.show_toolbar {
            let r = egui::Panel::bottom("toolbar")
                .exact_size(30.0)
                .frame(panel_frame(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 0)))
                .show(ui, |ui| views::toolbar(app, ui));
            app.panel_rects.push(r.response.rect);
        }
        match app.view {
            View::Grid => timed("grid", || views::grid(app, ui)),
            View::Loupe => views::loupe(app, ui),
            View::Compare => views::compare(app, ui),
            View::Survey => views::survey(app, ui),
        }
        ui.max_rect()
    });
    let center_rect = central.inner;

    // Edge arrows for hidden panels.
    let mid_y = full.center().y;
    if !app.ui.show_left
        && edge_toggle(&ctx, "edge_left", egui::pos2(full.left() + 2.0, mid_y), Align2::LEFT_CENTER, "▶")
    {
        app.ui.show_left = true;
    }
    if !app.ui.show_right
        && edge_toggle(&ctx, "edge_right", egui::pos2(full.right() - 2.0, mid_y), Align2::RIGHT_CENTER, "◀")
    {
        app.ui.show_right = true;
    }
    if !app.ui.show_filmstrip
        && edge_toggle(
            &ctx,
            "edge_bottom",
            egui::pos2(full.center().x, full.bottom() - 1.0),
            Align2::CENTER_BOTTOM,
            "▲",
        )
    {
        app.ui.show_filmstrip = true;
    }
    if !app.ui.show_top
        && edge_toggle(
            &ctx,
            "edge_top",
            egui::pos2(full.center().x, full.top() + 22.0),
            Align2::CENTER_TOP,
            "▼",
        )
    {
        app.ui.show_top = true;
    }

    toast(app, &ctx, center_rect);
    overlays(app, &ctx, full);
}

/// LrC-style transient message in the middle of the photo area ("Set Rating to 3 Stars").
fn toast(app: &mut PhotoLibraryApp, ctx: &egui::Context, center: egui::Rect) {
    let Some(t) = &app.toast else { return };
    let age = t.at.elapsed();
    if age > TOAST_TIME {
        app.toast = None;
        return;
    }
    let fade = 1.0 - (age.as_secs_f32() / TOAST_TIME.as_secs_f32()).powi(4);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("toast")));
    let galley = painter.layout_no_wrap(
        t.text.clone(),
        egui::FontId::proportional(17.0),
        theme::TEXT_STRONG.gamma_multiply(fade),
    );
    let rect = egui::Rect::from_center_size(center.center(), galley.size() + egui::vec2(36.0, 20.0));
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha((200.0 * fade) as u8));
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    ctx.request_repaint();
}

fn overlays(app: &mut PhotoLibraryApp, ctx: &egui::Context, full: egui::Rect) {
    // Lights Out: dim (or black out) everything except the photo area.
    if app.lights != Lights::On {
        let alpha = if app.lights == Lights::Dim { 205 } else { 255 };
        let painter =
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("lights_out")));
        for r in &app.panel_rects {
            painter.rect_filled(*r, 0.0, Color32::from_black_alpha(alpha));
        }
    }

    if app.fullscreen {
        views::fullscreen(app, ctx, full);
    }

    if app.show_shortcuts {
        let mut open = true;
        egui::Window::new("Library Module Shortcuts")
            .open(&mut open)
            .default_size([520.0, 560.0])
            .collapsible(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut group = "";
                    for s in SHORTCUTS {
                        if s.group != group {
                            group = s.group;
                            ui.add_space(6.0);
                            ui.label(RichText::new(group).strong().color(theme::TEXT_STRONG));
                            ui.separator();
                        }
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [130.0, 16.0],
                                egui::Label::new(RichText::new(s.binding.label()).monospace()),
                            );
                            let text = RichText::new(s.description);
                            match s.status {
                                Status::Available => ui.label(text),
                                Status::Planned(phase) => {
                                    ui.label(text.color(theme::TEXT_WEAK));
                                    ui.label(
                                        RichText::new(format!("({phase})")).small().color(theme::TEXT_WEAK),
                                    )
                                }
                            };
                        });
                    }
                });
            });
        app.show_shortcuts = open;
    }

    dialogs(app, ctx);
    super::import::window(app, ctx);
    super::export::window(app, ctx);

    if let Some(err) = app.error.clone() {
        let mut open = true;
        egui::Window::new("Something went wrong")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(err);
                if ui.button("OK").clicked() {
                    app.error = None;
                }
            });
        if !open {
            app.error = None;
        }
    }
}

/// Run a drawing section, logging it if it takes long enough to cause a visible hitch.
pub(super) fn timed<R>(name: &str, f: impl FnOnce() -> R) -> R {
    let started = std::time::Instant::now();
    let r = f();
    if started.elapsed() > Duration::from_millis(40) {
        log::warn!("slow section: {name} took {:.0?}", started.elapsed());
    }
    r
}

/// Modal dialogs: Remove folder confirmation, Create Folder and Rename prompts.
/// A keyword name field with validation and an OK button. Returns the name to apply.
/// `Ok(Some(name))` to apply, `Err(())` on Cancel.
fn keyword_name_prompt(ui: &mut egui::Ui, name: &mut String, ok_label: &str) -> Result<Option<String>, ()> {
    ui.label("Keyword Name:");
    let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
    if !resp.has_focus() && !resp.lost_focus() {
        resp.request_focus();
    }
    let trimmed = name.trim();
    let error = if trimmed.is_empty() {
        Some("")
    } else if trimmed.contains([',', ';', '|', '<', '>']) {
        Some("Keyword names cannot contain , ; | < or >")
    } else {
        None
    };
    if let Some(e) = error.filter(|e| !e.is_empty()) {
        ui.label(RichText::new(e).color(theme::MISSING).size(11.0));
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(6.0);
    let mut result = Ok(None);
    ui.horizontal(|ui| {
        let ok = ui.add_enabled(error.is_none(), egui::Button::new(ok_label));
        if (ok.clicked() || enter) && error.is_none() {
            result = Ok(Some(trimmed.to_owned()));
        }
        if ui.button("Cancel").clicked() {
            result = Err(());
        }
    });
    result
}

fn dialogs(app: &mut PhotoLibraryApp, ctx: &egui::Context) {
    let Some(dialog) = app.dialog.as_mut() else { return };
    // Live "N photos match" for the smart collection editor.
    type Counter<'a> = Box<dyn Fn(&catalog::SmartRules) -> i64 + 'a>;
    let app_session_count: Option<Counter> = app.session.as_ref().map(|s| {
        Box::new(move |r: &catalog::SmartRules| {
            s.catalog.count_filtered(catalog::Source::AllPhotographs, &r.criterion()).unwrap_or(0)
        }) as Counter
    });
    let mut close = false;
    let mut run = None;

    let title = match dialog {
        Dialog::ConfirmRemoveFolder { .. } => "Remove Folder",
        Dialog::CreateFolder { .. } => "Create Folder",
        Dialog::RenameFolder { .. } => "Rename Folder",
        Dialog::CreateKeyword { .. } => "Create Keyword Tag",
        Dialog::RenameKeyword { .. } => "Edit Keyword Tag",
        Dialog::ConfirmDeleteKeyword { .. } => "Delete Keyword Tag",
        Dialog::ConfirmReadMetadata { .. } => "Read Metadata from Files",
        Dialog::NewCollection { kind: catalog::CollectionKind::Set, .. } => "Create Collection Set",
        Dialog::NewCollection { kind: catalog::CollectionKind::Smart, .. } => "Create Smart Collection",
        Dialog::NewCollection { .. } => "Create Collection",
        Dialog::RenameCollection { .. } => "Rename",
        Dialog::ConfirmDeleteCollection { .. } => "Delete",
        Dialog::EditSmartCollection { .. } => "Edit Smart Collection",
        Dialog::ConfirmDeleteVirtualCopies { .. } => "Delete Virtual Copies",
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            match dialog {
                Dialog::ConfirmRemoveFolder { id, name, photos } => {
                    ui.label(
                        RichText::new(format!("Remove the folder \"{name}\" from the catalog?")).strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(format!(
                        "{photos} photo{} and any subfolders will be removed from this catalog, \
                         with their ratings, flags and labels.\nThe files on disk are not touched.",
                        if *photos == 1 { "" } else { "s" }
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Remove").clicked() {
                            run = Some(Command::ConfirmRemoveFolder(*id));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::CreateFolder { parent, name } | Dialog::RenameFolder { id: parent, name } => {
                    let renaming = title == "Rename Folder";
                    ui.label(if renaming { "New folder name:" } else { "Folder name:" });
                    let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if !resp.has_focus() && !resp.lost_focus() {
                        resp.request_focus();
                    }
                    let valid = catalog::validate_folder_name(name);
                    if let Err(e) = &valid
                        && !name.is_empty()
                    {
                        ui.label(RichText::new(e.to_string()).color(theme::MISSING).size(11.0));
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let ok = ui.add_enabled(
                            valid.is_ok(),
                            egui::Button::new(if renaming { "Rename" } else { "Create" }),
                        );
                        if (ok.clicked() || enter) && valid.is_ok() {
                            let name = name.clone();
                            run = Some(if renaming {
                                Command::ApplyRenameFolder { id: *parent, name }
                            } else {
                                Command::ApplyCreateFolder { parent: *parent, name }
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::CreateKeyword { parent, name } => match keyword_name_prompt(ui, name, "Create") {
                    Ok(Some(name)) => run = Some(Command::ApplyCreateKeyword { parent: *parent, name }),
                    Ok(None) => {}
                    Err(()) => close = true,
                },
                Dialog::RenameKeyword { id, name } => match keyword_name_prompt(ui, name, "Save") {
                    Ok(Some(name)) => run = Some(Command::ApplyRenameKeyword { id: *id, name }),
                    Ok(None) => {}
                    Err(()) => close = true,
                },
                Dialog::ConfirmDeleteKeyword { id, name, photos } => {
                    ui.label(RichText::new(format!("Delete the keyword \"{name}\"?")).strong());
                    ui.add_space(4.0);
                    ui.label(format!(
                        "It and any keywords inside it will be removed from {photos} photo{}.",
                        if *photos == 1 { "" } else { "s" }
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            run = Some(Command::ConfirmDeleteKeyword(*id));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::NewCollection { parent, kind, name, include_selected, rules } => {
                    ui.label("Name:");
                    let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if !resp.has_focus() && !resp.lost_focus() && name.is_empty() {
                        resp.request_focus();
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    match kind {
                        catalog::CollectionKind::Collection => {
                            ui.checkbox(include_selected, "Include selected photos");
                        }
                        catalog::CollectionKind::Smart => {
                            ui.add_space(6.0);
                            super::collections::rules_editor(ui, rules);
                        }
                        catalog::CollectionKind::Set => {}
                    }
                    ui.add_space(8.0);
                    let valid = !name.trim().is_empty();
                    ui.horizontal(|ui| {
                        let create = ui.add_enabled(valid, egui::Button::new("Create"));
                        if valid && (create.clicked() || (enter && *kind != catalog::CollectionKind::Smart)) {
                            run = Some(Command::ApplyCreateCollection {
                                parent: *parent,
                                kind: *kind,
                                name: name.trim().to_owned(),
                                include_selected: *include_selected,
                                rules: (*kind == catalog::CollectionKind::Smart).then(|| rules.clone()),
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::RenameCollection { id, name } => match keyword_name_prompt(ui, name, "Rename") {
                    Ok(Some(name)) => run = Some(Command::ApplyRenameCollection { id: *id, name }),
                    Ok(None) => {}
                    Err(()) => close = true,
                },
                Dialog::ConfirmDeleteCollection { id, name } => {
                    ui.label(RichText::new(format!("Delete \"{name}\"?")).strong());
                    ui.add_space(4.0);
                    ui.label("Photos stay in the catalog and on disk; only the collection goes.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            run = Some(Command::ConfirmDeleteCollection(*id));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::EditSmartCollection { id, name, rules } => {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    });
                    ui.add_space(6.0);
                    super::collections::rules_editor(ui, rules);
                    if let Some(s) = &app_session_count {
                        let n = s(rules);
                        ui.label(
                            RichText::new(format!("{n} photos match")).color(theme::TEXT_WEAK).size(11.0),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let valid = !name.trim().is_empty();
                        if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                            run = Some(Command::ApplySmartCollection {
                                id: *id,
                                name: name.trim().to_owned(),
                                rules: rules.clone(),
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::ConfirmDeleteVirtualCopies { ids } => {
                    let n = ids.len();
                    ui.label(RichText::new(if n == 1 {
                        "Delete the selected virtual copy?".to_string()
                    } else {
                        format!("Delete {n} virtual copies?")
                    }).strong());
                    ui.add_space(4.0);
                    ui.label("Their ratings, keywords and collections go with them. The file stays: it belongs to the master photo.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            run = Some(Command::DeleteVirtualCopies(ids.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::ConfirmReadMetadata { ids } => {
                    let n = ids.len();
                    ui.label(
                        RichText::new(format!(
                            "Read metadata from {}?",
                            if n == 1 {
                                "the selected photo's file".to_string()
                            } else {
                                format!("{n} files")
                            }
                        ))
                        .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        "Ratings, labels, rotation, keywords, title, caption, creator and copyright in the \
                         catalog will be replaced by the values in the XMP sidecars.\nPhotos without a \
                         sidecar are left unchanged.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Read").clicked() {
                            run = Some(Command::ConfirmReadMetadata(ids.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });

    drop(app_session_count);
    if let Some(cmd) = run {
        app.run(cmd);
        close = true;
    }
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.dialog = None;
    }
}
