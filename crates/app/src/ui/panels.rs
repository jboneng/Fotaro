//! LrC-style collapsible panel sections with Solo Mode.
//!
//! - Click a header to expand/collapse it.
//! - Alt-click (Option-click on macOS) a header to toggle Solo Mode: only one
//!   section is open at a time, as in Lightroom Classic.
//! - Right-click a header for "Solo Mode" and "Expand/Collapse All".

use std::collections::BTreeMap;

use eframe::egui::{self, Align2, Sense};
use serde::{Deserialize, Serialize};

use super::theme;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionStack {
    /// Section title -> expanded. Missing entries use the section's default.
    open: BTreeMap<String, bool>,
    pub solo: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl SectionStack {
    pub fn is_open(&self, title: &str, default_open: bool) -> bool {
        self.open.get(title).copied().unwrap_or(default_open)
    }

    fn set_open(&mut self, title: &str, open: bool, all_titles: &[&str]) {
        if open && self.solo {
            for t in all_titles {
                self.open.insert((*t).to_owned(), false);
            }
        }
        self.open.insert(title.to_owned(), open);
    }

    /// Draw one section. `all_titles` lists every section in this stack (for Solo Mode).
    pub fn section(
        &mut self,
        ui: &mut egui::Ui,
        side: Side,
        title: &str,
        all_titles: &[&str],
        default_open: bool,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        let open = self.is_open(title, default_open);

        // Fixed-height painted header: triangle + title, mirrored on the right side.
        let (rect, header) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
        let painter = ui.painter_at(rect);
        let bg =
            if header.hovered() { theme::SECTION_HEADER.gamma_multiply(1.25) } else { theme::SECTION_HEADER };
        painter.rect_filled(rect, 0.0, bg);
        let (tri_x, text_pos, align) = match side {
            Side::Left => {
                (rect.left() + 12.0, rect.left_center() + egui::vec2(24.0, 0.0), Align2::LEFT_CENTER)
            }
            Side::Right => {
                (rect.right() - 12.0, rect.right_center() - egui::vec2(24.0, 0.0), Align2::RIGHT_CENTER)
            }
        };
        let dir = match (open, side) {
            (true, _) => super::icons::Dir::Down,
            (false, Side::Left) => super::icons::Dir::Right,
            (false, Side::Right) => super::icons::Dir::Left,
        };
        super::icons::triangle(&painter, egui::pos2(tri_x, rect.center().y), 4.0, dir, theme::TEXT_WEAK);
        painter.text(text_pos, align, title, egui::FontId::proportional(13.5), theme::TEXT_STRONG);

        if header.clicked() {
            if ui.input(|i| i.modifiers.alt) {
                self.solo = !self.solo;
            } else {
                self.set_open(title, !open, all_titles);
            }
        }
        header.context_menu(|ui| {
            if ui.checkbox(&mut self.solo, "Solo Mode").changed() && self.solo {
                for t in all_titles {
                    self.open.insert((*t).to_owned(), *t == title);
                }
            }
            ui.separator();
            if ui.button("Expand All").clicked() {
                self.solo = false;
                for t in all_titles {
                    self.open.insert((*t).to_owned(), true);
                }
                ui.close();
            }
            if ui.button("Collapse All").clicked() {
                for t in all_titles {
                    self.open.insert((*t).to_owned(), false);
                }
                ui.close();
            }
        });

        if self.is_open(title, default_open) {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                add_contents(ui);
            });
        }
        ui.add_space(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_mode_keeps_only_one_section_open() {
        let titles = ["A", "B", "C"];
        let mut s = SectionStack::default();
        s.set_open("A", true, &titles);
        s.set_open("B", true, &titles);
        assert!(s.is_open("A", false) && s.is_open("B", false));

        s.solo = true;
        s.set_open("C", true, &titles);
        assert!(!s.is_open("A", true) && !s.is_open("B", true) && s.is_open("C", false));
    }
}
