//! LrC's Collections panel, the smart collection rule editor, and photo drag & drop
//! onto collections.

use std::collections::HashMap;

use catalog::{
    Collection, CollectionKind, Criterion, FileKind, Flag, NumOp, SmartRules, Source, TextField, TextOp,
};
use eframe::egui::{self, Color32, RichText, Stroke};

use super::theme;
use crate::app::{Command, FotaroApp};

/// Drag payload: photos dragged from the grid or filmstrip.
#[derive(Debug, Clone)]
pub struct PhotoDrag(pub Vec<i64>);

/// Small painted icons distinguish sets, collections and smart collections (as LrC does).
fn paint_kind_icon(painter: &egui::Painter, center: egui::Pos2, kind: CollectionKind) {
    let color = theme::TEXT_WEAK;
    let stroke = Stroke::new(1.0, color);
    match kind {
        CollectionKind::Set => {
            let r = egui::Rect::from_center_size(center + egui::vec2(0.0, 1.0), egui::vec2(11.0, 8.0));
            painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [r.left_top() + egui::vec2(1.0, -2.0), r.left_top() + egui::vec2(5.0, -2.0)],
                stroke,
            );
        }
        CollectionKind::Collection => {
            for d in [-2.0, 1.0] {
                let r = egui::Rect::from_center_size(center + egui::vec2(d, -d), egui::vec2(8.0, 8.0));
                painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
            }
        }
        CollectionKind::Smart => {
            let r = egui::Rect::from_center_size(center, egui::vec2(10.0, 9.0));
            painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
            painter.circle_filled(r.center(), 2.0, color);
        }
    }
}

/// A row that accepts dropped photos; returns them if some were dropped on it.
pub fn accept_photos(resp: &egui::Response, ui: &egui::Ui) -> Option<Vec<i64>> {
    if resp.dnd_hover_payload::<PhotoDrag>().is_some() {
        ui.painter().rect_stroke(
            resp.rect,
            2.0,
            Stroke::new(1.5, theme::TEXT_STRONG),
            egui::StrokeKind::Inside,
        );
    }
    resp.dnd_release_payload::<PhotoDrag>().map(|p| p.0.clone())
}

pub fn panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let mut commands = Vec::new();
    ui.horizontal(|ui| {
        ui.menu_button(RichText::new("+").size(13.0), |ui| {
            for (label, kind) in [
                ("Create Collection…", CollectionKind::Collection),
                ("Create Smart Collection…", CollectionKind::Smart),
                ("Create Collection Set…", CollectionKind::Set),
            ] {
                if ui.button(label).clicked() {
                    commands.push(Command::CreateCollection { parent: None, kind });
                    ui.close();
                }
            }
        });
    });
    let Some(s) = &mut app.session else {
        for c in commands {
            app.run(c);
        }
        return;
    };
    let collections: Vec<Collection> = s.collections_fresh().to_vec();
    if collections.is_empty() {
        ui.label(RichText::new("No collections yet. Use + to create one.").color(theme::TEXT_WEAK));
    }
    let mut children: HashMap<Option<i64>, Vec<&Collection>> = HashMap::new();
    for c in &collections {
        children.entry(c.parent_id).or_default().push(c);
    }
    // Sets first, then collections, each by name (as LrC lists them).
    for list in children.values_mut() {
        list.sort_by_key(|c| (c.kind != CollectionKind::Set, c.name.to_lowercase()));
    }
    let selected = match s.source {
        Source::Collection { id } => Some(id),
        _ => None,
    };
    let mut toggles = Vec::new();
    let mut stack: Vec<(&Collection, usize)> =
        children.get(&None).into_iter().flatten().rev().map(|c| (*c, 0)).collect();
    while let Some((c, depth)) = stack.pop() {
        let kids = children.get(&Some(c.id));
        let expanded = !s.collapsed_collections.contains(&c.id);
        let indent = 4.0 + depth as f32 * 14.0;
        let tri = match (c.kind, expanded) {
            (CollectionKind::Set, true) => "▼",
            (CollectionKind::Set, false) => "▶",
            _ => "",
        };
        let count = (c.kind != CollectionKind::Set).then_some(c.count);
        let resp = super::sidebars::source_row(ui, indent + 30.0, &c.name, count, selected == Some(c.id));
        let painter = ui.painter();
        painter.text(
            resp.rect.left_center() + egui::vec2(indent, 0.0),
            egui::Align2::LEFT_CENTER,
            tri,
            egui::FontId::proportional(9.0),
            theme::TEXT_WEAK,
        );
        paint_kind_icon(painter, resp.rect.left_center() + egui::vec2(indent + 20.0, 0.0), c.kind);
        if resp.clicked() {
            let on_triangle =
                resp.interact_pointer_pos().is_some_and(|p| p.x < resp.rect.left() + indent + 12.0);
            if c.kind == CollectionKind::Set && on_triangle {
                toggles.push(c.id);
            } else {
                commands.push(Command::SetSource(Source::Collection { id: c.id }));
            }
        }
        if resp.double_clicked() && c.kind == CollectionKind::Set {
            toggles.push(c.id);
        }
        if c.kind == CollectionKind::Collection
            && let Some(photos) = accept_photos(&resp, ui)
        {
            commands.push(Command::AddToCollection { id: c.id, photos: Some(photos) });
        }
        let resp = match c.kind {
            CollectionKind::Smart => resp.on_hover_text("Smart collection: photos matching its rules"),
            CollectionKind::Collection => resp.on_hover_text("Drag photos here to add them"),
            CollectionKind::Set => resp,
        };
        resp.context_menu(|ui| {
            let mut item = |ui: &mut egui::Ui, text: &str, cmd: Command| {
                if ui.button(text).clicked() {
                    commands.push(cmd);
                    ui.close();
                }
            };
            match c.kind {
                CollectionKind::Set => {
                    item(
                        ui,
                        "Create Collection…",
                        Command::CreateCollection { parent: Some(c.id), kind: CollectionKind::Collection },
                    );
                    item(
                        ui,
                        "Create Smart Collection…",
                        Command::CreateCollection { parent: Some(c.id), kind: CollectionKind::Smart },
                    );
                    item(
                        ui,
                        "Create Collection Set…",
                        Command::CreateCollection { parent: Some(c.id), kind: CollectionKind::Set },
                    );
                    ui.separator();
                }
                CollectionKind::Smart => {
                    item(ui, "Edit Smart Collection…", Command::EditSmartCollection(c.id));
                    ui.separator();
                }
                CollectionKind::Collection => {
                    item(ui, "Add Selected Photos", Command::AddToCollection { id: c.id, photos: None });
                    ui.separator();
                }
            }
            item(ui, "Rename…", Command::RenameCollection(c.id));
            item(ui, "Delete…", Command::DeleteCollection(c.id));
        });
        if expanded {
            for k in kids.into_iter().flatten().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    for id in toggles {
        if !s.collapsed_collections.remove(&id) {
            s.collapsed_collections.insert(id);
        }
    }
    for c in commands {
        app.run(c);
    }
}

/// "Add to Collection" items for a photo context menu.
pub fn add_to_collection_menu(ui: &mut egui::Ui, app: &mut FotaroApp) {
    let Some(s) = &app.session else { return };
    let targets: Vec<(i64, String)> = s
        .collections
        .iter()
        .filter(|c| c.kind == CollectionKind::Collection)
        .map(|c| (c.id, c.name.clone()))
        .collect();
    ui.menu_button("Add to Collection", |ui| {
        for (id, name) in targets {
            if ui.button(name).clicked() {
                app.run(Command::AddToCollection { id, photos: None });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("New Collection…").clicked() {
            app.run(Command::CreateCollection { parent: None, kind: CollectionKind::Collection });
            ui.close();
        }
    });
}

// ---- Smart collection rules ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Rating,
    Flag,
    Label,
    Text(TextField),
    FileType,
    CaptureDate,
    Other,
}

impl RuleKind {
    fn all() -> Vec<RuleKind> {
        let mut v = vec![RuleKind::Rating, RuleKind::Flag, RuleKind::Label];
        v.extend(TextField::ALL.into_iter().map(RuleKind::Text));
        v.extend([RuleKind::FileType, RuleKind::CaptureDate]);
        v
    }

    fn label(self) -> &'static str {
        match self {
            Self::Rating => "Rating",
            Self::Flag => "Pick Flag",
            Self::Label => "Label Color",
            Self::Text(f) => f.label(),
            Self::FileType => "File Type",
            Self::CaptureDate => "Capture Date",
            Self::Other => "Other",
        }
    }

    fn of(c: &Criterion) -> Self {
        match c {
            Criterion::Rating { .. } => Self::Rating,
            Criterion::Flag(_) => Self::Flag,
            Criterion::Label(_) => Self::Label,
            Criterion::Text { field, .. } => Self::Text(*field),
            Criterion::Kind(_) => Self::FileType,
            Criterion::CapturedWithinDays(_) => Self::CaptureDate,
            _ => Self::Other,
        }
    }

    fn default_criterion(self) -> Criterion {
        match self {
            Self::Rating => Criterion::Rating { op: NumOp::AtLeast, stars: 3 },
            Self::Flag => Criterion::Flag(Flag::Picked),
            Self::Label => Criterion::Label(Some("red".into())),
            Self::Text(field) => Criterion::Text { field, op: TextOp::ContainsAll, value: String::new() },
            Self::FileType => Criterion::Kind(FileKind::Raw),
            Self::CaptureDate | Self::Other => Criterion::CapturedWithinDays(30),
        }
    }
}

const LABELS: [(Option<&str>, &str); 6] = [
    (Some("red"), "Red"),
    (Some("yellow"), "Yellow"),
    (Some("green"), "Green"),
    (Some("blue"), "Blue"),
    (Some("purple"), "Purple"),
    (None, "None"),
];

/// LrC's smart collection editor: "Match [all|any] of the following rules".
pub fn rules_editor(ui: &mut egui::Ui, rules: &mut SmartRules) {
    ui.horizontal(|ui| {
        ui.label("Match");
        egui::ComboBox::from_id_salt("smart_match")
            .selected_text(if rules.match_all { "all" } else { "any" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut rules.match_all, true, "all");
                ui.selectable_value(&mut rules.match_all, false, "any");
            });
        ui.label("of the following rules:");
    });
    ui.add_space(4.0);
    let mut remove = None;
    for (i, rule) in rules.rules.iter_mut().enumerate() {
        ui.push_id(i, |ui| {
            ui.horizontal(|ui| {
                let kind = RuleKind::of(rule);
                egui::ComboBox::from_id_salt("kind").width(150.0).selected_text(kind.label()).show_ui(
                    ui,
                    |ui| {
                        for k in RuleKind::all() {
                            if ui.selectable_label(k == kind, k.label()).clicked() && k != kind {
                                *rule = k.default_criterion();
                            }
                        }
                    },
                );
                rule_value(ui, rule);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("−").on_hover_text("Remove this rule").clicked() {
                        remove = Some(i);
                    }
                });
            });
        });
    }
    if let Some(i) = remove {
        rules.rules.remove(i);
    }
    if ui.small_button("+ Add Rule").clicked() {
        rules.rules.push(RuleKind::Rating.default_criterion());
    }
}

fn rule_value(ui: &mut egui::Ui, rule: &mut Criterion) {
    match rule {
        Criterion::Rating { op, stars } => {
            egui::ComboBox::from_id_salt("op").width(48.0).selected_text(op.label()).show_ui(ui, |ui| {
                for o in NumOp::ALL {
                    ui.selectable_value(op, o, o.label());
                }
            });
            ui.add(egui::Slider::new(stars, 0..=5).suffix(" ★").show_value(true));
        }
        Criterion::Flag(flag) => {
            ui.label("is");
            let name = |f: Flag| match f {
                Flag::Picked => "Flagged",
                Flag::Unflagged => "Unflagged",
                Flag::Rejected => "Rejected",
            };
            egui::ComboBox::from_id_salt("flag").selected_text(name(*flag)).show_ui(ui, |ui| {
                for f in [Flag::Picked, Flag::Unflagged, Flag::Rejected] {
                    ui.selectable_value(flag, f, name(f));
                }
            });
        }
        Criterion::Label(label) => {
            ui.label("is");
            let current = LABELS.iter().find(|(l, _)| *l == label.as_deref()).map_or("None", |(_, n)| n);
            egui::ComboBox::from_id_salt("label").selected_text(current).show_ui(ui, |ui| {
                for (l, n) in LABELS {
                    if ui.selectable_label(label.as_deref() == l, n).clicked() {
                        *label = l.map(str::to_owned);
                    }
                }
            });
        }
        Criterion::Text { op, value, .. } => {
            egui::ComboBox::from_id_salt("op").width(110.0).selected_text(op.label()).show_ui(ui, |ui| {
                for o in TextOp::ALL {
                    ui.selectable_value(op, o, o.label());
                }
            });
            ui.add(egui::TextEdit::singleline(value).desired_width(170.0));
        }
        Criterion::Kind(kind) => {
            ui.label("is");
            let name = |k: FileKind| if k == FileKind::Raw { "Raw" } else { "Other (JPEG, TIFF, PNG…)" };
            egui::ComboBox::from_id_salt("kind_value").selected_text(name(*kind)).show_ui(ui, |ui| {
                for k in [FileKind::Raw, FileKind::Image] {
                    ui.selectable_value(kind, k, name(k));
                }
            });
        }
        Criterion::CapturedWithinDays(days) => {
            ui.label("is in the last");
            ui.add(egui::DragValue::new(days).range(1..=36_500).suffix(" days"));
        }
        _ => {
            ui.label(RichText::new("(set in another app version)").color(Color32::GRAY));
        }
    }
}

/// Paint the Quick Collection marker (a dot, filled when the photo is in it).
pub fn paint_quick_marker(painter: &egui::Painter, center: egui::Pos2, filled: bool) {
    if filled {
        painter.circle_filled(center, 4.0, theme::TEXT);
    } else {
        painter.circle_stroke(center, 4.0, Stroke::new(1.0, theme::TEXT_WEAK));
    }
}
