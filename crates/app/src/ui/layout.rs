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
use crate::app::{Command, Lights, PhotoLibraryApp, View};
use catalog::Flag;

const TOAST_TIME: Duration = Duration::from_millis(1400);

/// Shortcut label for an action, for display next to menu items.
fn shortcut_for(action: Action) -> Option<String> {
    SHORTCUTS.iter().find(|s| s.action == action).map(|s| s.binding.label())
}

/// A menu item that runs a shortcut action, showing its key binding.
fn menu_action(ui: &mut egui::Ui, app: &mut PhotoLibraryApp, text: &str, action: Action) {
    let planned = SHORTCUTS
        .iter()
        .find(|s| s.action == action)
        .is_some_and(|s| matches!(s.status, Status::Planned(_)));
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
                menu_action(ui, app, "Import Photos (Add Folder)…", Action::Import);
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
            menu_action(ui, app, "Previous Photo", Action::Previous);
            menu_action(ui, app, "Next Photo", Action::Next);
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
                for (n, label) in ["1 Star", "2 Stars", "3 Stars", "4 Stars", "5 Stars"].into_iter().enumerate() {
                    menu_action(ui, app, label, Action::SetRating { stars: n as u8 + 1, advance: false });
                }
            });
            ui.menu_button("Set Color Label", |ui| {
                for (label, name) in [("red", "Red"), ("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue")] {
                    menu_action(ui, app, name, Action::ColorLabel { label, advance: false });
                }
            });
            ui.separator();
            menu_action(ui, app, "Rotate Left (CCW)", Action::RotateLeft);
            menu_action(ui, app, "Rotate Right (CW)", Action::RotateRight);
            ui.separator();
            ui.menu_button("Edit In", |ui| {
                menu_action(ui, app, "Edit in Default Editor…", Action::EditIn);
                menu_cmd(ui, app, "Edit in NegPy…", false, Command::Planned("", ""));
                menu_cmd(ui, app, "Edit in Luminar…", false, Command::Planned("", ""));
            });
        });
        ui.menu_button("Metadata", |ui| {
            menu_action(ui, app, "Add Keywords…", Action::Keywording);
            menu_cmd(ui, app, "Read Metadata from Files", false, Command::Planned("", ""));
            menu_cmd(ui, app, "Save Metadata to Files", false, Command::Planned("", ""));
        });
        ui.menu_button("View", |ui| {
            menu_toggle(ui, app, "Grid", app.view == View::Grid, Action::GridView);
            menu_toggle(ui, app, "Loupe", app.view == View::Loupe, Action::LoupeView);
            menu_action(ui, app, "Compare", Action::CompareView);
            menu_action(ui, app, "Survey", Action::SurveyView);
            ui.separator();
            menu_action(ui, app, "Full Screen Preview", Action::FullScreenPreview);
            ui.menu_button("Lights Out", |ui| {
                for (l, name) in [(Lights::On, "Lights On"), (Lights::Dim, "Lights Dim"), (Lights::Off, "Lights Off")] {
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
        if let Some(label) = &progress.syncing {
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
        .show_collapsible(ui, &mut show, |ui| views::filmstrip(app, ui))
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
        .show_collapsible(ui, &mut show, |ui| sidebars::left(app, ui))
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
        .show_collapsible(ui, &mut show, |ui| sidebars::right(app, ui))
    {
        app.panel_rects.push(r.response.rect);
    }
    app.ui.show_right = show;

    let central = egui::CentralPanel::default().frame(panel_frame(theme::CENTER)).show(ui, |ui| {
        if app.view == View::Grid && app.ui.show_filter_bar {
            let r = egui::Panel::top("filter_bar")
                .exact_size(26.0)
                .frame(panel_frame(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 0)))
                .show(ui, |ui| views::filter_bar(app, ui));
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
            View::Grid => views::grid(app, ui),
            View::Loupe => views::loupe(app, ui),
        }
        ui.max_rect()
    });
    let center_rect = central.inner;

    // Edge arrows for hidden panels.
    let mid_y = full.center().y;
    if !app.ui.show_left && edge_toggle(&ctx, "edge_left", egui::pos2(full.left() + 2.0, mid_y), Align2::LEFT_CENTER, "▶") {
        app.ui.show_left = true;
    }
    if !app.ui.show_right && edge_toggle(&ctx, "edge_right", egui::pos2(full.right() - 2.0, mid_y), Align2::RIGHT_CENTER, "◀") {
        app.ui.show_right = true;
    }
    if !app.ui.show_filmstrip
        && edge_toggle(&ctx, "edge_bottom", egui::pos2(full.center().x, full.bottom() - 1.0), Align2::CENTER_BOTTOM, "▲")
    {
        app.ui.show_filmstrip = true;
    }
    if !app.ui.show_top
        && edge_toggle(&ctx, "edge_top", egui::pos2(full.center().x, full.top() + 22.0), Align2::CENTER_TOP, "▼")
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
    let galley = painter.layout_no_wrap(t.text.clone(), egui::FontId::proportional(17.0), theme::TEXT_STRONG.gamma_multiply(fade));
    let rect = egui::Rect::from_center_size(center.center(), galley.size() + egui::vec2(36.0, 20.0));
    painter.rect_filled(rect, 6.0, Color32::from_black_alpha((200.0 * fade) as u8));
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    ctx.request_repaint();
}

fn overlays(app: &mut PhotoLibraryApp, ctx: &egui::Context, full: egui::Rect) {
    // Lights Out: dim (or black out) everything except the photo area.
    if app.lights != Lights::On {
        let alpha = if app.lights == Lights::Dim { 205 } else { 255 };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("lights_out")));
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
                            ui.add_sized([130.0, 16.0], egui::Label::new(RichText::new(s.binding.label()).monospace()));
                            let text = RichText::new(s.description);
                            match s.status {
                                Status::Available => ui.label(text),
                                Status::Planned(phase) => {
                                    ui.label(text.color(theme::TEXT_WEAK));
                                    ui.label(RichText::new(format!("({phase})")).small().color(theme::TEXT_WEAK))
                                }
                            };
                        });
                    }
                });
            });
        app.show_shortcuts = open;
    }

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
