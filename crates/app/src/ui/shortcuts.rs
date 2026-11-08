//! Keyboard shortcuts, modelled on Lightroom Classic's Library module.
//!
//! [`SHORTCUTS`] is the single source of truth: the dispatcher, the in-app
//! shortcut window (Ctrl+/) and `docs/SHORTCUTS.md` are all generated from it.
//! `Ctrl` means Cmd on macOS.

use catalog::Flag;
use eframe::egui::{self, Key};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    GridView,
    LoupeView,
    ToggleLoupe,
    CompareView,
    SurveyView,
    FullScreenPreview,
    Escape,
    CycleLightsOut,
    ToggleSidePanels,
    ToggleAllPanels,
    ToggleToolbar,
    ToggleFilterBar,
    ToggleTopPanel,
    ToggleFilmstrip,
    ToggleLeftPanel,
    ToggleRightPanel,
    CycleGridStyle,
    CycleInfoOverlay,
    SetFlag { flag: Flag, advance: bool },
    SetRating { stars: u8, advance: bool },
    ColorLabel { label: &'static str, advance: bool },
    Next,
    Previous,
    Up,
    Down,
    SelectAll,
    SelectNone,
    ZoomToggle,
    RotateLeft,
    RotateRight,
    QuickCollection,
    Stack,
    Unstack,
    Import,
    EditIn,
    Keywording,
    ShowShortcuts,
}

/// Is the action wired up yet, or does it arrive in a later roadmap phase?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

const fn k(key: Key) -> Binding {
    Binding { key, ctrl: false, shift: false, alt: false }
}
const fn ctrl(key: Key) -> Binding {
    Binding { key, ctrl: true, shift: false, alt: false }
}
const fn shift(key: Key) -> Binding {
    Binding { key, ctrl: false, shift: true, alt: false }
}
const fn ctrl_shift(key: Key) -> Binding {
    Binding { key, ctrl: true, shift: true, alt: false }
}

pub struct Shortcut {
    pub binding: Binding,
    pub action: Action,
    pub group: &'static str,
    pub description: &'static str,
    pub status: Status,
}

const fn s(binding: Binding, action: Action, group: &'static str, description: &'static str) -> Shortcut {
    Shortcut { binding, action, group, description, status: Status::Available }
}
const fn planned(
    binding: Binding,
    action: Action,
    group: &'static str,
    description: &'static str,
    phase: &'static str,
) -> Shortcut {
    Shortcut { binding, action, group, description, status: Status::Planned(phase) }
}

const VIEW: &str = "View";
const PANELS: &str = "Panels";
const CULL: &str = "Rating & Flagging";
const SELECT: &str = "Selection & Navigation";
const ORG: &str = "Organizing";

use Action as A;
use Key as K;

pub const SHORTCUTS: &[Shortcut] = &[
    // View modes
    s(k(K::G), A::GridView, VIEW, "Grid view"),
    s(k(K::E), A::LoupeView, VIEW, "Loupe view"),
    s(k(K::Space), A::ToggleLoupe, VIEW, "Toggle Grid / Loupe"),
    s(k(K::Enter), A::LoupeView, VIEW, "Open selected photo in Loupe"),
    planned(k(K::C), A::CompareView, VIEW, "Compare view", "Phase 6"),
    planned(k(K::N), A::SurveyView, VIEW, "Survey view", "Phase 6"),
    s(k(K::F), A::FullScreenPreview, VIEW, "Full-screen preview"),
    s(k(K::Escape), A::Escape, VIEW, "Exit full screen / back to Grid"),
    s(k(K::L), A::CycleLightsOut, VIEW, "Cycle Lights Out (dim / off / normal)"),
    s(k(K::J), A::CycleGridStyle, VIEW, "Cycle Grid cell style"),
    s(k(K::I), A::CycleInfoOverlay, VIEW, "Cycle Loupe info overlay"),
    planned(k(K::Z), A::ZoomToggle, VIEW, "Toggle Loupe zoom (fit / 1:1)", "Phase 2"),
    // Panels
    s(k(K::Tab), A::ToggleSidePanels, PANELS, "Hide / show side panels"),
    s(shift(K::Tab), A::ToggleAllPanels, PANELS, "Hide / show all panels"),
    s(k(K::T), A::ToggleToolbar, PANELS, "Hide / show toolbar"),
    s(k(K::Backslash), A::ToggleFilterBar, PANELS, "Hide / show Library Filter bar"),
    s(k(K::F5), A::ToggleTopPanel, PANELS, "Hide / show top panel"),
    s(k(K::F6), A::ToggleFilmstrip, PANELS, "Hide / show Filmstrip"),
    s(k(K::F7), A::ToggleLeftPanel, PANELS, "Hide / show left panels"),
    s(k(K::F8), A::ToggleRightPanel, PANELS, "Hide / show right panels"),
    // Rating & flagging
    s(k(K::P), A::SetFlag { flag: Flag::Picked, advance: false }, CULL, "Flag as Pick"),
    s(k(K::X), A::SetFlag { flag: Flag::Rejected, advance: false }, CULL, "Flag as Rejected"),
    s(k(K::U), A::SetFlag { flag: Flag::Unflagged, advance: false }, CULL, "Remove flag"),
    s(k(K::Num0), A::SetRating { stars: 0, advance: false }, CULL, "Remove star rating"),
    s(k(K::Num1), A::SetRating { stars: 1, advance: false }, CULL, "Set rating to 1 star"),
    s(k(K::Num2), A::SetRating { stars: 2, advance: false }, CULL, "Set rating to 2 stars"),
    s(k(K::Num3), A::SetRating { stars: 3, advance: false }, CULL, "Set rating to 3 stars"),
    s(k(K::Num4), A::SetRating { stars: 4, advance: false }, CULL, "Set rating to 4 stars"),
    s(k(K::Num5), A::SetRating { stars: 5, advance: false }, CULL, "Set rating to 5 stars"),
    s(k(K::Num6), A::ColorLabel { label: "red", advance: false }, CULL, "Toggle Red label"),
    s(k(K::Num7), A::ColorLabel { label: "yellow", advance: false }, CULL, "Toggle Yellow label"),
    s(k(K::Num8), A::ColorLabel { label: "green", advance: false }, CULL, "Toggle Green label"),
    s(k(K::Num9), A::ColorLabel { label: "blue", advance: false }, CULL, "Toggle Blue label"),
    s(
        shift(K::P),
        A::SetFlag { flag: Flag::Picked, advance: true },
        CULL,
        "Flag as Pick and go to next photo",
    ),
    s(
        shift(K::X),
        A::SetFlag { flag: Flag::Rejected, advance: true },
        CULL,
        "Flag as Rejected and go to next photo",
    ),
    s(
        shift(K::U),
        A::SetFlag { flag: Flag::Unflagged, advance: true },
        CULL,
        "Remove flag and go to next photo",
    ),
    s(shift(K::Num0), A::SetRating { stars: 0, advance: true }, CULL, "Remove rating and go to next photo"),
    s(shift(K::Num1), A::SetRating { stars: 1, advance: true }, CULL, "Rate 1 star and go to next photo"),
    s(shift(K::Num2), A::SetRating { stars: 2, advance: true }, CULL, "Rate 2 stars and go to next photo"),
    s(shift(K::Num3), A::SetRating { stars: 3, advance: true }, CULL, "Rate 3 stars and go to next photo"),
    s(shift(K::Num4), A::SetRating { stars: 4, advance: true }, CULL, "Rate 4 stars and go to next photo"),
    s(shift(K::Num5), A::SetRating { stars: 5, advance: true }, CULL, "Rate 5 stars and go to next photo"),
    s(
        shift(K::Num6),
        A::ColorLabel { label: "red", advance: true },
        CULL,
        "Toggle Red label and go to next photo",
    ),
    s(
        shift(K::Num7),
        A::ColorLabel { label: "yellow", advance: true },
        CULL,
        "Toggle Yellow label and go to next photo",
    ),
    s(
        shift(K::Num8),
        A::ColorLabel { label: "green", advance: true },
        CULL,
        "Toggle Green label and go to next photo",
    ),
    s(
        shift(K::Num9),
        A::ColorLabel { label: "blue", advance: true },
        CULL,
        "Toggle Blue label and go to next photo",
    ),
    // Selection & navigation
    s(k(K::ArrowRight), A::Next, SELECT, "Next photo"),
    s(k(K::ArrowLeft), A::Previous, SELECT, "Previous photo"),
    s(k(K::ArrowUp), A::Up, SELECT, "Photo above (Grid)"),
    s(k(K::ArrowDown), A::Down, SELECT, "Photo below (Grid)"),
    s(ctrl(K::A), A::SelectAll, SELECT, "Select all"),
    s(ctrl(K::D), A::SelectNone, SELECT, "Select none"),
    // Organizing
    s(ctrl_shift(K::I), A::Import, ORG, "Import photos (add folder)"),
    planned(ctrl(K::OpenBracket), A::RotateLeft, ORG, "Rotate left (counterclockwise)", "Phase 2"),
    planned(ctrl(K::CloseBracket), A::RotateRight, ORG, "Rotate right (clockwise)", "Phase 2"),
    planned(k(K::B), A::QuickCollection, ORG, "Add to / remove from Quick Collection", "Phase 4"),
    planned(ctrl(K::G), A::Stack, ORG, "Group into stack", "Phase 4"),
    planned(ctrl_shift(K::G), A::Unstack, ORG, "Unstack", "Phase 4"),
    planned(ctrl(K::K), A::Keywording, ORG, "Add keywords", "Phase 3"),
    planned(ctrl(K::E), A::EditIn, ORG, "Edit in default external editor", "Phase 7"),
    s(ctrl(K::Slash), A::ShowShortcuts, ORG, "Show keyboard shortcuts"),
];

pub fn key_name(key: Key) -> &'static str {
    match key {
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::Backslash => "\\",
        Key::Slash => "/",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Escape => "Esc",
        other => other.name(),
    }
}

impl Binding {
    /// Platform-specific label, e.g. "Ctrl+Shift+I" (or "Cmd+Shift+I" on macOS).
    pub fn label(&self) -> String {
        self.label_with(if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" })
    }

    fn label_with(&self, ctrl_name: &str) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push(ctrl_name);
        }
        if self.alt {
            parts.push(if cfg!(target_os = "macos") { "Option" } else { "Alt" });
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(key_name(self.key));
        parts.join("+")
    }

    fn modifiers_match(&self, m: egui::Modifiers) -> bool {
        m.command == self.ctrl && m.shift == self.shift && m.alt == self.alt
    }

    fn matches(&self, key: Key, physical: Option<Key>, m: egui::Modifiers) -> bool {
        if !self.modifiers_match(m) {
            return false;
        }
        // Shift+digit may arrive as a symbol ("!" for Shift+1), so fall back to the
        // physical key for Shift+digit bindings only. (Never otherwise: arrow keys can
        // report the numpad digit as their physical key.)
        key == self.key
            || (self.shift && is_digit(self.key) && !is_logical_key_bound(key) && physical == Some(self.key))
    }
}

/// Is `key` bound (with any modifiers) as a logical key? Such keys never fall back to physical matching.
fn is_logical_key_bound(key: Key) -> bool {
    SHORTCUTS.iter().any(|s| s.binding.key == key)
}

fn is_digit(key: Key) -> bool {
    matches!(
        key,
        Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9
    )
}

/// Take this frame's shortcut presses out of the input queue (so widgets don't
/// also react, e.g. Tab moving focus). Does nothing while a text field has focus.
pub fn take_actions(ctx: &egui::Context) -> Vec<&'static Shortcut> {
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|ev| {
            let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = ev else {
                return true;
            };
            match resolve(*key, *physical_key, *modifiers) {
                Some(s) => {
                    log::debug!("shortcut {}: {}", s.binding.label(), s.description);
                    hits.push(s);
                    false
                }
                None => true,
            }
        });
    });
    hits
}

/// The shortcut for a key press: exact logical matches win over the physical fallback.
fn resolve(key: Key, physical: Option<Key>, m: egui::Modifiers) -> Option<&'static Shortcut> {
    SHORTCUTS
        .iter()
        .find(|s| s.binding.key == key && s.binding.modifiers_match(m))
        .or_else(|| SHORTCUTS.iter().find(|s| s.binding.matches(key, physical, m)))
}

/// Markdown reference, written to `docs/SHORTCUTS.md` (kept in sync by a test).
#[cfg_attr(not(test), allow(dead_code))]
pub fn markdown() -> String {
    let mut out = String::from(
        "# Keyboard shortcuts\n\n\
         Generated from `crates/app/src/ui/shortcuts.rs` — do not edit by hand.\n\
         Run `UPDATE_DOCS=1 cargo test -p photo_library shortcuts` to regenerate.\n\n\
         `Ctrl` is `Cmd` on macOS. These follow Lightroom Classic's Library module.\n",
    );
    let mut group = "";
    for s in SHORTCUTS {
        if s.group != group {
            group = s.group;
            out.push_str(&format!("\n## {group}\n\n| Shortcut | Action | Status |\n|---|---|---|\n"));
        }
        let status = match s.status {
            Status::Available => "✅".to_string(),
            Status::Planned(phase) => format!("planned ({phase})"),
        };
        let label = s.binding.label_with("Ctrl").replace('\\', "\\\\").replace('|', "\\|");
        out.push_str(&format!("| `{label}` | {} | {status} |\n", s.description));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_binding_is_used_twice() {
        let mut seen = HashSet::new();
        for s in SHORTCUTS {
            let b = s.binding;
            assert!(
                seen.insert((b.key, b.ctrl, b.shift, b.alt)),
                "duplicate binding {} ({})",
                b.label(),
                s.description
            );
        }
    }

    #[test]
    fn shift_digit_matches_by_physical_key() {
        let b = shift(Key::Num3);
        let m = egui::Modifiers { shift: true, ..Default::default() };
        assert!(b.matches(Key::Num3, Some(Key::Num3), m));
        assert!(b.matches(Key::Exclamationmark, Some(Key::Num3), m));
        assert!(!b.matches(Key::Num3, Some(Key::Num3), egui::Modifiers::NONE));
        // Letters follow the logical layout (AZERTY etc.), not the physical position.
        assert!(!k(Key::P).matches(Key::O, Some(Key::P), egui::Modifiers::NONE));
    }

    #[test]
    fn arrow_keys_win_over_numpad_physical_keys() {
        // Windows reports Right arrow (and numpad arrows) with physical key Num6.
        let s = resolve(Key::ArrowRight, Some(Key::Num6), egui::Modifiers::NONE).unwrap();
        assert_eq!(s.action, Action::Next);
        let s = resolve(Key::ArrowLeft, Some(Key::Num4), egui::Modifiers::NONE).unwrap();
        assert_eq!(s.action, Action::Previous);
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let s = resolve(Key::Exclamationmark, Some(Key::Num1), shift).unwrap();
        assert_eq!(s.action, Action::SetRating { stars: 1, advance: true });
    }

    #[test]
    fn shortcuts_doc_is_up_to_date() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/SHORTCUTS.md");
        let expected = markdown();
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(
            actual == expected,
            "docs/SHORTCUTS.md is out of date; run UPDATE_DOCS=1 cargo test -p photo_library shortcuts"
        );
    }
}
