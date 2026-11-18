//! Keyboard shortcuts, modelled on Lightroom Classic's Library module.
//!
//! [`SHORTCUTS`] is the single source of truth: the dispatcher, the in-app
//! shortcut window (Ctrl+/) and `docs/SHORTCUTS.md` are all generated from it.
//! `Ctrl` means Cmd on macOS.

use catalog::Flag;

use crate::app::Module;
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
    SetFlag {
        flag: Flag,
        advance: bool,
    },
    SetRating {
        stars: u8,
        advance: bool,
    },
    ColorLabel {
        label: &'static str,
        advance: bool,
    },
    Next,
    Previous,
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    SelectAll,
    SelectNone,
    ZoomToggle,
    ZoomIn,
    ZoomOut,
    ToggleAutoAdvance,
    RotateLeft,
    RotateRight,
    QuickCollection,
    Stack,
    Unstack,
    Import,
    Export,
    ExportWithPrevious,
    EditIn,
    EditInOther,
    Keywording,
    ShowShortcuts,
    SaveMetadata,
    ShowQuickCollection,
    ClearQuickCollection,
    ToggleStack,
    MoveToTopOfStack,
    PreviousMember,
    NextMember,
    DetachFromPicture,
    CompareWithOriginal,
    ToggleFilters,
    Find,
    RemoveFromCollection,
    DeleteRejected,
    NewCollection,
    /// Shift+arrows: extend the selection (by this many photos).
    ExtendSelection(isize),
    CreateVirtualCopy,
    LibraryModule,
    DevelopModule,
    BeforeAfter,
    AutoTone,
    AutoWhiteBalance,
    WhiteBalanceSelector,
    ClippingIndicators,
    NewBrushMask,
    BrushSize(i8),
    BrushFeather(i8),
    Masking,
    NewLinearMask,
    NewRadialMask,
    MaskOverlay,
    ResetSettings,
    CopySettings,
    PasteSettings,
    Undo,
    Redo,
}

/// Is the action wired up yet, or does it arrive in a later roadmap phase?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    // Every shortcut is available today; kept for features announced before they land.
    #[allow(dead_code)]
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
const fn alt(key: Key) -> Binding {
    Binding { key, ctrl: false, shift: false, alt: true }
}
const fn ctrl_alt(key: Key) -> Binding {
    Binding { key, ctrl: true, shift: false, alt: true }
}
const fn ctrl_alt_shift(key: Key) -> Binding {
    Binding { key, ctrl: true, shift: true, alt: true }
}

/// Where a shortcut works: a key can mean different things in Library and Develop
/// (`\` is the Filter bar in one and Before/After in the other, as in LrC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Library,
    Develop,
}

impl Scope {
    pub fn includes(self, module: Module) -> bool {
        match self {
            Scope::All => true,
            Scope::Library => module == Module::Library,
            Scope::Develop => module == Module::Develop,
        }
    }

    #[cfg(test)]
    fn overlaps(self, other: Scope) -> bool {
        self == Scope::All || other == Scope::All || self == other
    }
}

pub struct Shortcut {
    pub binding: Binding,
    pub action: Action,
    pub group: &'static str,
    pub description: &'static str,
    pub status: Status,
    pub scope: Scope,
}

const fn s(binding: Binding, action: Action, group: &'static str, description: &'static str) -> Shortcut {
    Shortcut { binding, action, group, description, status: Status::Available, scope: Scope::All }
}
/// A Library-only shortcut.
const fn lib(binding: Binding, action: Action, group: &'static str, description: &'static str) -> Shortcut {
    Shortcut { binding, action, group, description, status: Status::Available, scope: Scope::Library }
}
/// A Develop-only shortcut.
const fn dev(binding: Binding, action: Action, description: &'static str) -> Shortcut {
    Shortcut {
        binding,
        action,
        group: DEVELOP,
        description,
        status: Status::Available,
        scope: Scope::Develop,
    }
}
#[allow(dead_code)]
const fn dev_planned(
    binding: Binding,
    action: Action,
    description: &'static str,
    phase: &'static str,
) -> Shortcut {
    Shortcut {
        binding,
        action,
        group: DEVELOP,
        description,
        status: Status::Planned(phase),
        scope: Scope::Develop,
    }
}

const VIEW: &str = "View";
const PANELS: &str = "Panels";
const CULL: &str = "Rating & Flagging";
const SELECT: &str = "Selection & Navigation";
const ORG: &str = "Organizing";
const MODULES: &str = "Modules";
const DEVELOP: &str = "Develop";

use Action as A;
use Key as K;

pub const SHORTCUTS: &[Shortcut] = &[
    // View modes
    s(k(K::G), A::GridView, VIEW, "Grid view"),
    s(k(K::E), A::LoupeView, VIEW, "Loupe view"),
    s(k(K::Space), A::ToggleLoupe, VIEW, "Toggle Grid / Loupe"),
    s(k(K::Enter), A::LoupeView, VIEW, "Open selected photo in Loupe"),
    s(k(K::C), A::CompareView, VIEW, "Compare view (Select and Candidate)"),
    s(k(K::N), A::SurveyView, VIEW, "Survey view (all selected photos)"),
    s(k(K::F), A::FullScreenPreview, VIEW, "Full-screen preview"),
    s(k(K::Escape), A::Escape, VIEW, "Exit full screen / back to Grid"),
    s(k(K::L), A::CycleLightsOut, VIEW, "Cycle Lights Out (dim / off / normal)"),
    lib(k(K::J), A::CycleGridStyle, VIEW, "Cycle Grid cell style"),
    s(k(K::I), A::CycleInfoOverlay, VIEW, "Cycle Loupe info overlay"),
    s(k(K::Z), A::ZoomToggle, VIEW, "Toggle Loupe zoom (Fit / Navigator zoom level)"),
    s(ctrl(K::Equals), A::ZoomIn, VIEW, "Zoom in"),
    s(ctrl(K::Plus), A::ZoomIn, VIEW, "Zoom in"),
    s(ctrl(K::Minus), A::ZoomOut, VIEW, "Zoom out"),
    // Panels
    s(k(K::Tab), A::ToggleSidePanels, PANELS, "Hide / show side panels"),
    s(shift(K::Tab), A::ToggleAllPanels, PANELS, "Hide / show all panels"),
    s(k(K::T), A::ToggleToolbar, PANELS, "Hide / show toolbar"),
    lib(k(K::Backslash), A::ToggleFilterBar, PANELS, "Hide / show Library Filter bar"),
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
    s(shift(K::ArrowRight), A::ExtendSelection(1), SELECT, "Extend selection to the next photo"),
    s(shift(K::ArrowLeft), A::ExtendSelection(-1), SELECT, "Extend selection to the previous photo"),
    s(shift(K::ArrowDown), A::ExtendSelection(isize::MAX), SELECT, "Extend selection down a row (Grid)"),
    s(shift(K::ArrowUp), A::ExtendSelection(isize::MIN), SELECT, "Extend selection up a row (Grid)"),
    s(k(K::Home), A::First, SELECT, "First photo"),
    s(k(K::End), A::Last, SELECT, "Last photo"),
    s(k(K::PageUp), A::PageUp, SELECT, "Page up (Grid)"),
    s(k(K::PageDown), A::PageDown, SELECT, "Page down (Grid)"),
    s(ctrl(K::A), A::SelectAll, SELECT, "Select all"),
    s(ctrl(K::D), A::SelectNone, SELECT, "Select none"),
    // Organizing
    s(ctrl_shift(K::I), A::Import, ORG, "Import photos (Copy, Move or Add)"),
    s(ctrl_shift(K::E), A::Export, ORG, "Export the selected photos"),
    s(ctrl_alt_shift(K::E), A::ExportWithPrevious, ORG, "Export with previous settings"),
    s(ctrl(K::OpenBracket), A::RotateLeft, ORG, "Rotate left (counterclockwise)"),
    s(ctrl(K::CloseBracket), A::RotateRight, ORG, "Rotate right (clockwise)"),
    s(k(K::B), A::QuickCollection, ORG, "Add to / remove from Quick Collection"),
    s(ctrl(K::B), A::ShowQuickCollection, ORG, "Show Quick Collection"),
    s(ctrl_shift(K::B), A::ClearQuickCollection, ORG, "Clear Quick Collection"),
    s(ctrl(K::Quote), A::CreateVirtualCopy, ORG, "Create virtual copy"),
    s(ctrl(K::N), A::NewCollection, ORG, "New collection (with the selected photos)"),
    s(ctrl(K::G), A::Stack, ORG, "Group the selected pictures"),
    s(ctrl_shift(K::G), A::Unstack, ORG, "Ungroup"),
    s(k(K::S), A::ToggleStack, ORG, "Show / hide the version strip"),
    s(shift(K::S), A::MoveToTopOfStack, ORG, "Show this member for its stack (make it active)"),
    s(alt(K::ArrowLeft), A::PreviousMember, ORG, "Previous member of the stack"),
    s(alt(K::ArrowRight), A::NextMember, ORG, "Next member of the stack"),
    s(k(K::Delete), A::RemoveFromCollection, ORG, "Remove from collection (when viewing a collection)"),
    s(ctrl(K::Backspace), A::DeleteRejected, ORG, "Delete rejected photos (remove them from the catalog)"),
    s(ctrl(K::L), A::ToggleFilters, VIEW, "Enable / disable Library filters"),
    s(ctrl(K::F), A::Find, VIEW, "Find (text filter)"),
    s(ctrl(K::K), A::Keywording, ORG, "Add keywords (Keywording panel)"),
    s(ctrl(K::S), A::SaveMetadata, ORG, "Save metadata to files (XMP sidecars)"),
    s(ctrl(K::E), A::EditIn, ORG, "Edit in the primary external editor"),
    s(ctrl_alt(K::E), A::EditInOther, ORG, "Edit in the secondary external editor"),
    s(ctrl(K::Slash), A::ShowShortcuts, ORG, "Show keyboard shortcuts"),
    // Modules
    s(k(K::D), A::DevelopModule, MODULES, "Develop module"),
    s(ctrl_alt(K::Num1), A::LibraryModule, MODULES, "Library module"),
    s(ctrl_alt(K::Num2), A::DevelopModule, MODULES, "Develop module"),
    // Develop
    dev(k(K::Backslash), A::BeforeAfter, "Show Before / After (the photo without adjustments)"),
    dev(ctrl(K::U), A::AutoTone, "Auto Tone"),
    dev(ctrl_shift(K::U), A::AutoWhiteBalance, "Auto White Balance"),
    dev(k(K::W), A::WhiteBalanceSelector, "White Balance Selector (click a neutral area)"),
    dev(k(K::J), A::ClippingIndicators, "Show clipping (shadows blue, highlights red)"),
    dev(shift(K::W), A::Masking, "Masking: open / close"),
    dev(k(K::K), A::NewBrushMask, "New Brush mask (paint on the photo; Alt erases)"),
    dev(k(K::M), A::NewLinearMask, "New Linear Gradient mask (drag on the photo)"),
    dev(shift(K::M), A::NewRadialMask, "New Radial Gradient mask (drag on the photo)"),
    dev(k(K::O), A::MaskOverlay, "Masking: show / hide the overlay"),
    dev(k(K::OpenBracket), A::BrushSize(-1), "Brush: smaller"),
    dev(k(K::CloseBracket), A::BrushSize(1), "Brush: larger"),
    dev(k(K::OpenCurlyBracket), A::BrushFeather(-1), "Brush: less feather (Shift+[)"),
    dev(k(K::CloseCurlyBracket), A::BrushFeather(1), "Brush: more feather (Shift+])"),
    dev(ctrl_shift(K::R), A::ResetSettings, "Reset all settings"),
    dev(ctrl_shift(K::C), A::CopySettings, "Copy settings"),
    dev(ctrl_shift(K::V), A::PasteSettings, "Paste settings"),
    dev(ctrl(K::Z), A::Undo, "Undo"),
    dev(ctrl(K::Y), A::Redo, "Redo"),
    dev(ctrl_shift(K::Z), A::Redo, "Redo"),
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
        Key::OpenCurlyBracket => "{",
        Key::CloseCurlyBracket => "}",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Escape => "Esc",
        Key::PageUp => "Page Up",
        Key::PageDown => "Page Down",
        Key::Equals => "=",
        Key::Plus => "+",
        Key::Minus => "-",
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
pub fn take_actions(ctx: &egui::Context, module: Module) -> Vec<&'static Shortcut> {
    if ctx.egui_wants_keyboard_input() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|ev| {
            let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = ev else {
                return true;
            };
            match resolve(*key, *physical_key, *modifiers, module) {
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
fn resolve(key: Key, physical: Option<Key>, m: egui::Modifiers, module: Module) -> Option<&'static Shortcut> {
    let active = || SHORTCUTS.iter().filter(move |s| s.scope.includes(module));
    let find = |m: egui::Modifiers| {
        active()
            .find(|s| s.binding.key == key && s.binding.modifiers_match(m))
            .or_else(|| active().find(|s| s.binding.matches(key, physical, m)))
    };
    find(m)
        .or_else(|| {
            // Symbols typed with AltGr (`\` is AltGr+'+' on Nordic layouts, `[` AltGr+8 on
            // German ones) arrive as Ctrl+Alt+symbol on Windows: match the plain binding.
            (m.ctrl && m.alt && !m.shift && is_symbol(key))
                .then(|| find(egui::Modifiers { ctrl: false, alt: false, command: false, ..m }))
                .flatten()
        })
        .or_else(|| {
            // A symbol typed with Shift (`{` is Shift+[ on US layouts) already is the
            // shifted character: match its binding without Shift.
            (m.shift && !m.ctrl && !m.alt && is_symbol(key))
                .then(|| find(egui::Modifiers { shift: false, ..m }))
                .flatten()
        })
}

/// Punctuation keys, which some layouts only reach through AltGr.
fn is_symbol(key: Key) -> bool {
    matches!(
        key,
        Key::Backslash
            | Key::Pipe
            | Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Equals
            | Key::Plus
            | Key::Minus
            | Key::Slash
            | Key::Questionmark
            | Key::Quote
            | Key::Semicolon
            | Key::Colon
            | Key::Comma
            | Key::Period
            | Key::Backtick
            | Key::Exclamationmark
    )
}

/// Markdown reference, written to `docs/SHORTCUTS.md` (kept in sync by a test).
#[cfg_attr(not(test), allow(dead_code))]
pub fn markdown() -> String {
    let mut out = String::from(
        "# Keyboard shortcuts\n\n\
         Generated from `crates/app/src/ui/shortcuts.rs` — do not edit by hand.\n\
         Run `UPDATE_DOCS=1 cargo test -p photo_library shortcuts` to regenerate.\n\n\
         `Ctrl` is `Cmd` on macOS. These follow Lightroom Classic's Library and Develop modules.\n\
         Shortcuts work in both modules unless marked (Library) or listed under Develop.\n",
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
        let only = if s.scope == Scope::Library { " (Library)" } else { "" };
        out.push_str(&format!("| `{label}` | {}{only} | {status} |\n", s.description));
    }
    out.push_str(
        "\n## Auto Advance\n\n\
         With **Caps Lock** on (Windows, macOS) or **Photo ▸ Auto Advance** checked, flagging, rating \
         and labelling move to the next photo, as if Shift were held.\n\n\
         ## Loupe mouse\n\n\
         Click the photo to toggle between Fit and the Navigator zoom level at that point; drag \
         (or scroll) to move around while zoomed. Click or drag in the Navigator to choose the \
         visible area.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_binding_is_used_twice_in_a_module() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                assert!(
                    a.binding != b.binding || !a.scope.overlaps(b.scope),
                    "duplicate binding {} ({} / {})",
                    a.binding.label(),
                    a.description,
                    b.description
                );
            }
        }
    }

    #[test]
    fn altgr_symbols_match_plain_bindings() {
        // Windows reports AltGr as Ctrl+Alt: Nordic `\` is AltGr+'+'.
        let altgr = egui::Modifiers { ctrl: true, alt: true, command: true, ..Default::default() };
        let s = resolve(Key::Backslash, Some(Key::Minus), altgr, Module::Develop).unwrap();
        assert_eq!(s.action, Action::BeforeAfter);
        let s = resolve(Key::Backslash, Some(Key::Minus), altgr, Module::Library).unwrap();
        assert_eq!(s.action, Action::ToggleFilterBar);
        // Letters and digits with Ctrl+Alt keep their own bindings.
        let s = resolve(Key::E, None, altgr, Module::Library).unwrap();
        assert_eq!(s.action, Action::EditInOther);
        assert!(resolve(Key::P, None, altgr, Module::Library).is_none());
        // Brush keys: `[` is AltGr+8 and `{` AltGr+7 on Nordic layouts; `{` is Shift+[ on US.
        let s = resolve(Key::OpenBracket, Some(Key::Num8), altgr, Module::Develop).unwrap();
        assert_eq!(s.action, Action::BrushSize(-1));
        let s = resolve(Key::OpenCurlyBracket, Some(Key::Num7), altgr, Module::Develop).unwrap();
        assert_eq!(s.action, Action::BrushFeather(-1));
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let s = resolve(Key::CloseCurlyBracket, Some(Key::CloseBracket), shift, Module::Develop).unwrap();
        assert_eq!(s.action, Action::BrushFeather(1));
        // Ctrl+[ still rotates.
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        assert_eq!(
            resolve(Key::OpenBracket, None, ctrl, Module::Develop).unwrap().action,
            Action::RotateLeft
        );
    }

    #[test]
    fn keys_follow_the_module() {
        let none = egui::Modifiers::NONE;
        let lib = resolve(Key::Backslash, None, none, Module::Library).unwrap();
        assert_eq!(lib.action, Action::ToggleFilterBar);
        let dev = resolve(Key::Backslash, None, none, Module::Develop).unwrap();
        assert_eq!(dev.action, Action::BeforeAfter);
        // Rating works in both.
        assert!(resolve(Key::Num3, None, none, Module::Develop).is_some());
        assert!(resolve(Key::W, None, none, Module::Library).is_none());
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
        let s = resolve(Key::ArrowRight, Some(Key::Num6), egui::Modifiers::NONE, Module::Library).unwrap();
        assert_eq!(s.action, Action::Next);
        let s = resolve(Key::ArrowLeft, Some(Key::Num4), egui::Modifiers::NONE, Module::Library).unwrap();
        assert_eq!(s.action, Action::Previous);
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let s = resolve(Key::Exclamationmark, Some(Key::Num1), shift, Module::Library).unwrap();
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
