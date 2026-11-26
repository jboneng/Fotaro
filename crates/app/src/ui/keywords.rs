//! LrC's Keywording and Keyword List panels.

use std::collections::HashMap;

use catalog::Keyword;
use eframe::egui::{self, Align, Layout, RichText, TextEdit};

use crate::app::{Command, FotaroApp};

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(super::theme::TEXT_WEAK)
}

/// A text box that edits a value and reports it once, when the user is done (focus
/// lost or Enter). While focused it keeps its own buffer, so the value behind it can
/// refresh without fighting the typing. Returns `(before, after)` when changed.
pub fn commit_on_blur(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: &str,
    make: impl FnOnce(&mut String) -> TextEdit<'_>,
) -> Option<(String, String)> {
    let stored: Option<(String, String)> = ui.data(|d| d.get_temp(id));
    let mut text = stored.as_ref().map_or_else(|| current.to_owned(), |(_, b)| b.clone());
    let resp = ui.add(make(&mut text).id(id));
    // Enter commits even in multi-line fields (LrC behaviour); Shift+Enter is a new line.
    if text.contains('\n') && !ui.input(|i| i.modifiers.shift) {
        text = text.replace('\n', " ").trim().to_owned();
        resp.surrender_focus();
        let original = stored.as_ref().map_or_else(|| current.to_owned(), |(o, _)| o.clone());
        ui.data_mut(|d| d.remove::<(String, String)>(id));
        return (text != original).then_some((original, text));
    }
    if resp.has_focus() {
        let original = stored.map_or_else(|| current.to_owned(), |(o, _)| o);
        ui.data_mut(|d| d.insert_temp(id, (original, text)));
        None
    } else if let Some((original, buffer)) = stored {
        ui.data_mut(|d| d.remove::<(String, String)>(id));
        (buffer != original).then_some((original, buffer))
    } else {
        None
    }
}

pub fn keywording(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let view = app.view;
    let focus_entry = std::mem::take(&mut app.focus_keywording);
    let Some(s) = &mut app.session else { return };
    if s.active.is_none() {
        ui.label(weak("No photo selected"));
        return;
    }
    let meta = s.selection_meta(view).clone();
    let n = meta.ids.len();
    let mut commands = Vec::new();

    ui.label(
        weak(if n > 1 { format!("Keyword Tags  ({n} photos)") } else { "Keyword Tags".to_owned() })
            .size(11.0),
    );
    let tags_id = egui::Id::unique("keyword_tags");
    if let Some((before, after)) = commit_on_blur(ui, tags_id, &meta.tags_text(), |t| {
        TextEdit::multiline(t)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text("No keywords. Type here, separated by commas.")
    }) {
        commands.push(Command::SetKeywordTags { before, after });
    }
    if meta.tags.iter().any(|(_, _, all)| !all) {
        ui.label(weak("* only on some of the selected photos").size(10.5));
    }

    ui.add_space(4.0);
    let entry_id = egui::Id::unique("keyword_entry");
    let mut entry: String = ui.data(|d| d.get_temp(entry_id)).unwrap_or_default();
    let resp = ui.add(
        TextEdit::singleline(&mut entry)
            .id(entry_id.with("edit"))
            .desired_width(f32::INFINITY)
            .hint_text("Click here to add keywords"),
    );
    if focus_entry {
        resp.request_focus();
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        if !entry.trim().is_empty() {
            commands.push(Command::AddKeywords(std::mem::take(&mut entry)));
        }
        // Stay in the field for the next keyword, as LrC does.
        resp.request_focus();
    }
    ui.data_mut(|d| d.insert_temp(entry_id, entry.clone()));
    // Suggest existing keywords while typing the last item.
    let typed = entry.rsplit([',', ';']).next().unwrap_or("").trim().to_lowercase();
    if typed.len() >= 2 && resp.has_focus() {
        let matches: Vec<&Keyword> =
            s.keywords.iter().filter(|k| k.name.to_lowercase().starts_with(&typed)).take(6).collect();
        if !matches.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for k in matches {
                    let path = s.keyword_path(k.id).join(" > ");
                    if ui.small_button(&k.name).on_hover_text(&path).clicked() {
                        // no hotkey: one per keyword
                        let keep = entry.rfind([',', ';']).map_or("", |i| &entry[..=i]).to_owned();
                        ui.data_mut(|d| d.insert_temp(entry_id, String::new()));
                        commands.push(Command::AddKeywords(format!("{keep} {path}")));
                    }
                }
            });
        }
    }

    ui.add_space(6.0);
    ui.label(weak("Recent Keywords").size(11.0));
    if s.recent_keywords.is_empty() {
        ui.label(weak("Keywords you add appear here.").size(10.5));
    } else {
        let width = ((ui.available_width() - 8.0) / 3.0).max(40.0);
        egui::Grid::new("recent_keywords").num_columns(3).spacing([4.0, 4.0]).show(ui, |ui| {
            for (i, &id) in s.recent_keywords.clone().iter().enumerate() {
                let Some(k) = s.keywords.iter().find(|k| k.id == id) else { continue };
                let on_all = meta.keyword_counts.get(&id).is_some_and(|&c| c == n);
                let button = egui::Button::selectable(on_all, RichText::new(&k.name).size(11.0)).truncate();
                if ui.add_sized([width, 20.0], button).on_hover_text(s.keyword_path(id).join(" > ")).clicked()
                {
                    commands.push(Command::SetKeyword { id, on: !on_all });
                }
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    }

    for c in commands {
        app.run(c);
    }
}

pub fn keyword_list(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let view = app.view;
    let Some(s) = &mut app.session else { return };
    let mut commands = Vec::new();

    let filter_id = egui::Id::unique("keyword_filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        if ui.small_button("+").on_hover_text("Create Keyword Tag…").clicked() {
            // key: library.keywords.create
            commands.push(Command::CreateKeyword(None));
        }
        ui.add(TextEdit::singleline(&mut filter).hint_text("Filter Keywords").desired_width(f32::INFINITY));
    });
    ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    ui.add_space(4.0);

    if s.keywords.is_empty() {
        ui.label(weak("No keywords yet. Add some in the Keywording panel."));
    } else {
        let has_selection = s.active.is_some();
        let (counts, n) = if has_selection {
            let m = s.selection_meta(view);
            (m.keyword_counts.clone(), m.ids.len())
        } else {
            (HashMap::new(), 0)
        };
        let mut children: HashMap<Option<i64>, Vec<&Keyword>> = HashMap::new();
        for k in &s.keywords {
            children.entry(k.parent_id).or_default().push(k);
        }
        // With a filter, show matching keywords and the path down to them, expanded.
        let filter = filter.trim().to_lowercase();
        let visible: Option<std::collections::HashSet<i64>> = (!filter.is_empty()).then(|| {
            let mut keep = std::collections::HashSet::new();
            for k in s.keywords.iter().filter(|k| k.name.to_lowercase().contains(&filter)) {
                let mut next = Some(k.id);
                while let Some(id) = next {
                    keep.insert(id);
                    next = s.keywords.iter().find(|k| k.id == id).and_then(|k| k.parent_id);
                }
            }
            keep
        });
        let mut toggles = Vec::new();
        let mut stack: Vec<(&Keyword, usize)> =
            children.get(&None).into_iter().flatten().rev().map(|k| (*k, 0)).collect();
        while let Some((k, depth)) = stack.pop() {
            if visible.as_ref().is_some_and(|v| !v.contains(&k.id)) {
                continue;
            }
            let kids = children.get(&Some(k.id));
            let expanded = visible.is_some() || !s.collapsed_keywords.contains(&k.id);
            let count = counts.get(&k.id).copied().unwrap_or(0);
            let resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(depth as f32 * 14.0);
                    if kids.is_some() {
                        let tri = if expanded { "▼" } else { "▶" };
                        if ui.add(egui::Button::new(RichText::new(tri).size(9.0)).frame(false)).clicked() {
                            // no hotkey: one per keyword
                            toggles.push(k.id);
                        }
                    } else {
                        ui.add_space(14.0);
                    }
                    if has_selection {
                        let mut on = count > 0 && count == n;
                        let box_ =
                            egui::Checkbox::without_text(&mut on).indeterminate(count > 0 && count < n);
                        let tip = if count == n {
                            "Remove this keyword from the selected photos"
                        } else {
                            "Add this keyword to the selected photos"
                        };
                        if ui.add(box_).on_hover_text(tip).clicked() {
                            commands.push(Command::SetKeyword { id: k.id, on: count < n });
                        }
                    }
                    ui.label(RichText::new(&k.name).size(12.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(weak(k.photo_count.to_string()).size(11.0));
                    });
                })
                .response;
            resp.context_menu(|ui| {
                if ui.button(format!("Create Keyword Tag inside \"{}\"…", k.name)).clicked() {
                    // no hotkey: a keyword's own menu
                    commands.push(Command::CreateKeyword(Some(k.id)));
                    ui.close();
                }
                if ui.button("Edit Keyword Tag…").clicked() {
                    // no hotkey: a keyword's own menu
                    commands.push(Command::RenameKeyword(k.id));
                    ui.close();
                }
                if ui.button(format!("Delete \"{}\"…", k.name)).clicked() {
                    // no hotkey: a keyword's own menu
                    commands.push(Command::DeleteKeyword(k.id));
                    ui.close();
                }
            });
            if expanded {
                for child in kids.into_iter().flatten().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
        for id in toggles {
            if !s.collapsed_keywords.remove(&id) {
                s.collapsed_keywords.insert(id);
            }
        }
    }
    for c in commands {
        app.run(c);
    }
}
