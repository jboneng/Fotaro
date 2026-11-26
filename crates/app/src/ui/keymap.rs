//! Edit ▸ Keyboard Shortcuts…: every command, the keys it has, and a way to change them.
//!
//! Sections (as in [`hotkeys::commands`]) start collapsed; searching by name or by
//! pressing a combination opens those with matches. A key used by another command where
//! both are in effect asks before it is taken from that one.

use eframe::egui::{self, Key, RichText};

use super::hotkeys::{self, Command, Keymap};
use super::shortcuts::{Binding, binding_for_press};
use super::theme;
use crate::app::FotaroApp;

/// The editor's state between frames.
#[derive(Default)]
pub struct Editor {
    search: String,
    /// Searching by keys: waiting for a combination (`None`) or showing what has it.
    by_keys: Option<Option<Binding>>,
    /// Recording another key for this command.
    recording: Option<&'static Command>,
    /// A recorded key that another command has: (for, key, the others).
    clash: Option<(&'static Command, Binding, Vec<&'static Command>)>,
    /// A search was showing last frame (sections it opened close again after).
    was_filtering: bool,
}

impl Editor {
    /// Keys go to the editor, not to the commands they are bound to.
    pub fn capturing(&self) -> bool {
        self.recording.is_some() || matches!(self.by_keys, Some(None))
    }
}

/// The next key pressed this frame (taken from the queue), or Escape.
fn take_press(ctx: &egui::Context) -> Option<Binding> {
    ctx.input_mut(|input| {
        let at = input.events.iter().position(|e| matches!(e, egui::Event::Key { pressed: true, .. }))?;
        let egui::Event::Key { key, physical_key, modifiers, .. } = input.events.remove(at) else {
            return None;
        };
        Some(binding_for_press(key, physical_key, modifiers))
    })
}

fn is_escape(b: &Binding) -> bool {
    b.key == Key::Escape && !b.ctrl && !b.shift && !b.alt
}

pub fn window(app: &mut FotaroApp, ctx: &egui::Context) {
    if !app.show_shortcuts {
        app.keymap_editor = Editor::default();
        return;
    }
    let mut open = true;
    let mut ed = std::mem::take(&mut app.keymap_editor);
    let mut keymap = app.ui.keymap.clone();

    // Keys pressed for the editor.
    if ed.capturing()
        && let Some(b) = take_press(ctx)
    {
        if is_escape(&b) {
            ed.recording = None;
            ed.by_keys = None;
        } else if let Some(c) = ed.recording.take() {
            let others = keymap.clashes(b, c.scope, &c.id);
            let mut keys = keymap.bindings(c);
            if !keys.contains(&b) {
                if others.is_empty() {
                    keys.push(b);
                    keymap.set(c, &keys);
                } else {
                    ed.clash = Some((c, b, others));
                }
            }
        } else {
            ed.by_keys = Some(Some(b));
        }
    }

    egui::Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .default_size([640.0, 660.0])
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut ed.search)
                        .hint_text("Search by name")
                        .desired_width(220.0),
                );
                if search.changed() {
                    ed.by_keys = None;
                }
                let label = match ed.by_keys {
                    Some(None) => "Press a combination… (Esc)".to_owned(),
                    Some(Some(b)) => format!("Keys: {}", b.label()),
                    None => "Search by Keys".to_owned(),
                };
                let button = ui
                    .add(egui::Button::new(label).selected(ed.by_keys.is_some()))
                    .on_hover_text("Press a combination to see what it does");
                if button.clicked() {
                    ed.by_keys = if ed.by_keys.is_some() { None } else { Some(None) };
                    ed.recording = None;
                    ed.search.clear();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(keymap != Keymap::default(), egui::Button::new("Reset All"))
                        .on_hover_text("Every command back to its keys out of the box")
                        .clicked()
                    {
                        keymap = Keymap::default();
                        ed.clash = None;
                    }
                });
            });
            if let Some((c, b, others)) = ed.clash.clone() {
                let names: Vec<&str> = others.iter().map(|o| o.name.as_str()).collect();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{} is used by {}.", b.label(), names.join(", ")))
                            .color(theme::TEXT_STRONG),
                    );
                    if ui.button(format!("Give it to {}", c.name)).clicked() {
                        for o in &others {
                            let keys: Vec<Binding> =
                                keymap.bindings(o).into_iter().filter(|k| *k != b).collect();
                            keymap.set(o, &keys);
                        }
                        let mut keys = keymap.bindings(c);
                        keys.push(b);
                        keymap.set(c, &keys);
                        ed.clash = None;
                    }
                    if ui.button("Cancel").clicked() {
                        ed.clash = None;
                    }
                });
            }
            ui.separator();

            let needle = ed.search.trim().to_lowercase();
            let keys = ed.by_keys.flatten();
            let filtering = !needle.is_empty() || keys.is_some();
            let shows = |c: &Command| {
                (needle.is_empty() || c.name.to_lowercase().contains(&needle))
                    && keys.is_none_or(|k| keymap.bindings(c).contains(&k))
            };
            let close_all = ed.was_filtering && !filtering;
            ed.was_filtering = filtering;
            // The sections in order, each with the commands that show.
            let mut sections: Vec<(&str, Vec<&'static Command>)> = Vec::new();
            for c in hotkeys::commands().iter().filter(|c| shows(c)) {
                match sections.iter_mut().find(|(s, _)| *s == c.section) {
                    Some((_, members)) => members.push(c),
                    None => sections.push((c.section, vec![c])),
                }
            }

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let any = !sections.is_empty();
                for (section, members) in sections {
                    let open = if filtering {
                        Some(true)
                    } else if close_all {
                        Some(false)
                    } else {
                        None
                    };
                    egui::CollapsingHeader::new(RichText::new(section).strong())
                        .id_salt(("keymap", section))
                        .default_open(false)
                        .open(open)
                        .show(ui, |ui| {
                            egui::Grid::new(("keymap_grid", section)).num_columns(2).striped(true).show(
                                ui,
                                |ui| {
                                    let mut done: Vec<imaging::develop::Slider> = Vec::new();
                                    for c in members {
                                        match c.slider() {
                                            // A slider: one row with both its keys and its step.
                                            Some(s) if done.contains(&s) => continue,
                                            Some(s) => {
                                                done.push(s);
                                                slider_row(ui, &mut ed, &mut keymap, s);
                                            }
                                            None => {
                                                ui.label(&c.name);
                                                ui.horizontal(|ui| key_chips(ui, &mut ed, &mut keymap, c));
                                            }
                                        }
                                        ui.end_row();
                                    }
                                },
                            );
                        });
                }
                if !any {
                    ui.label(RichText::new("Nothing found.").color(theme::TEXT_WEAK));
                }
            });
        });

    if keymap != app.ui.keymap {
        hotkeys::install(&keymap);
        app.ui.keymap = keymap;
    }
    app.keymap_editor = ed;
    if !open {
        app.show_shortcuts = false;
    }
}

/// A command's keys (click one to remove it), + to add one, Reset.
fn key_chips(ui: &mut egui::Ui, ed: &mut Editor, keymap: &mut Keymap, c: &'static Command) {
    let keys = keymap.bindings(c);
    for k in &keys {
        let chip = egui::Button::new(RichText::new(format!("{} ×", k.label())).monospace()).small();
        if ui.add(chip).on_hover_text("Remove this key").clicked() {
            let rest: Vec<Binding> = keys.iter().copied().filter(|o| o != k).collect();
            keymap.set(c, &rest);
        }
    }
    let recording = ed.recording.is_some_and(|r| r.id == c.id);
    if recording {
        ui.label(RichText::new("Press keys… (Esc)").color(theme::TEXT_STRONG));
    } else if ui.small_button("+").on_hover_text("Add a key").clicked() {
        ed.recording = Some(c);
        ed.by_keys = None;
        ed.clash = None;
    }
    if keys != c.defaults
        && ui.small_button("Reset").on_hover_text("Back to the keys out of the box").clicked()
    {
        keymap.set(c, &c.defaults);
    }
}

/// A slider's row: its name, the keys that raise and lower it, and its step.
fn slider_row(ui: &mut egui::Ui, ed: &mut Editor, keymap: &mut Keymap, s: imaging::develop::Slider) {
    let find =
        |up: bool| hotkeys::commands().iter().find(|c| c.run == hotkeys::Run::Slider { slider: s, up });
    let (Some(up), Some(down)) = (find(true), find(false)) else { return };
    ui.label(up.name.trim_end_matches(": Raise"));
    ui.horizontal(|ui| {
        ui.label(RichText::new("Raise").color(theme::TEXT_WEAK));
        key_chips(ui, ed, keymap, up);
        ui.add_space(8.0);
        ui.label(RichText::new("Lower").color(theme::TEXT_WEAK));
        key_chips(ui, ed, keymap, down);
        ui.add_space(8.0);
        ui.label(RichText::new("Step").color(theme::TEXT_WEAK));
        let mut step = keymap.step(s);
        let fine = hotkeys::default_step(s) < 1.0;
        let drag = egui::DragValue::new(&mut step)
            .range(if fine { 0.01..=10.0 } else { 0.1..=1000.0 })
            .speed(if fine { 0.01 } else { 0.5 })
            .max_decimals(2);
        if ui.add(drag).on_hover_text("How much one key press moves the slider").changed() {
            keymap.set_step(s, step);
        }
    });
}
