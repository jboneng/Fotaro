//! Hot keys: every function (and, later, slider) the keyboard can reach, with the keys
//! it has out of the box, and the keys the user has given it instead.
//!
//! [`commands`] lists them; [`Keymap`] holds the user's changes (saved with the app's
//! settings, not the catalog). The dispatcher in [`super::shortcuts`] looks keys up
//! through [`current`], the keymap in use.

use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock, RwLock};

use eframe::egui::Key;
use serde::{Deserialize, Serialize};

use super::shortcuts::{Action, Binding, SHORTCUTS, Scope};

/// Something the keyboard can do.
pub struct Command {
    /// Stable: the user's keys are saved under it.
    pub id: String,
    pub name: String,
    /// Where it is listed (and, for a panel's control, the panel).
    pub section: &'static str,
    pub scope: Scope,
    pub action: Action,
    /// The keys it has out of the box.
    pub defaults: Vec<Binding>,
}

/// An action's id: its name and values, as written in the code ("SetRating.stars=3.advance=false").
fn action_id(action: Action) -> String {
    format!("{action:?}").replace(" { ", ".").replace(" }", "").replace(": ", "=").replace(", ", ".")
}

/// Every command, in the order they are listed.
pub fn commands() -> &'static [Command] {
    static ALL: OnceLock<Vec<Command>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out: Vec<Command> = Vec::new();
        for s in SHORTCUTS {
            // One action with several keys (Redo: Ctrl+Y and Ctrl+Shift+Z) is one command.
            if let Some(c) = out.iter_mut().find(|c| c.action == s.action && c.scope == s.scope) {
                c.defaults.push(s.binding);
                continue;
            }
            out.push(Command {
                id: action_id(s.action),
                name: s.description.to_owned(),
                section: s.group,
                scope: s.scope,
                action: s.action,
                defaults: vec![s.binding],
            });
        }
        out
    })
}

/// A key combination as saved: the key by its name, so that the file reads well and
/// survives changes to the key list's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBinding {
    pub key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
}

impl From<Binding> for SavedBinding {
    fn from(b: Binding) -> Self {
        SavedBinding { key: b.key.name().to_owned(), ctrl: b.ctrl, shift: b.shift, alt: b.alt }
    }
}

impl SavedBinding {
    /// `None` for a key this version does not know.
    pub fn binding(&self) -> Option<Binding> {
        Some(Binding { key: Key::from_name(&self.key)?, ctrl: self.ctrl, shift: self.shift, alt: self.alt })
    }
}

/// The user's changes to the keys: per command id the keys it has instead of its
/// defaults (empty: none).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    pub bindings: BTreeMap<String, Vec<SavedBinding>>,
}

impl Keymap {
    /// The keys `c` has.
    pub fn bindings(&self, c: &Command) -> Vec<Binding> {
        match self.bindings.get(&c.id) {
            Some(saved) => saved.iter().filter_map(SavedBinding::binding).collect(),
            None => c.defaults.clone(),
        }
    }

    /// Give `c` these keys (its defaults: forget the change).
    pub fn set(&mut self, c: &Command, keys: &[Binding]) {
        if keys == c.defaults.as_slice() {
            self.bindings.remove(&c.id);
        } else {
            self.bindings.insert(c.id.clone(), keys.iter().map(|b| SavedBinding::from(*b)).collect());
        }
    }

    /// The commands that `key` would also run where `scope` is (other than `except`).
    pub fn clashes(&self, key: Binding, scope: Scope, except: &str) -> Vec<&'static Command> {
        commands()
            .iter()
            .filter(|c| c.id != except && c.scope.overlaps(scope) && self.bindings(c).contains(&key))
            .collect()
    }
}

static CURRENT: LazyLock<RwLock<Keymap>> = LazyLock::new(|| RwLock::new(Keymap::default()));

/// The keymap in use.
pub fn current() -> Keymap {
    CURRENT.read().map(|k| k.clone()).unwrap_or_default()
}

/// Use `keymap` from now on.
pub fn install(keymap: &Keymap) {
    if let Ok(mut k) = CURRENT.write() {
        *k = keymap.clone();
    }
}

/// The first key `action` has, as shown in menus and tooltips.
pub fn label_for(action: Action) -> Option<String> {
    let keymap = CURRENT.read().ok()?;
    commands()
        .iter()
        .filter(|c| c.action == action)
        .find_map(|c| keymap.bindings(c).first().map(Binding::label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shortcuts::Status;

    #[test]
    fn every_command_has_its_own_id_and_every_shortcut_a_command() {
        let all = commands();
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| b.id != a.id), "id {} twice", a.id);
        }
        for s in SHORTCUTS.iter().filter(|s| s.status == Status::Available) {
            assert!(
                all.iter().any(|c| c.action == s.action && c.defaults.contains(&s.binding)),
                "{} has no command",
                s.description
            );
        }
        assert!(all.iter().any(|c| c.id == "SetRating.stars=3.advance=false"));
        let redo = all.iter().find(|c| c.action == Action::Redo).unwrap();
        assert_eq!(redo.defaults.len(), 2);
    }

    #[test]
    fn every_key_is_saved_and_read_back_by_name() {
        for key in Key::ALL {
            let b = Binding { key: *key, ctrl: true, shift: false, alt: true };
            let saved = SavedBinding::from(b);
            assert_eq!(saved.binding(), Some(b), "{}", key.name());
        }
        assert_eq!(
            SavedBinding { key: "NoSuchKey".into(), ctrl: false, shift: false, alt: false }.binding(),
            None
        );
    }

    #[test]
    fn a_users_keys_replace_the_defaults_and_are_kept() {
        let flag = commands().iter().find(|c| c.id == "SetFlag.flag=Picked.advance=false").unwrap();
        let mut keymap = Keymap::default();
        assert_eq!(keymap.bindings(flag), flag.defaults);
        let other = Binding { key: Key::F9, ctrl: false, shift: true, alt: false };
        keymap.set(flag, &[other]);
        assert_eq!(keymap.bindings(flag), vec![other]);
        // Saved and read back.
        let json = serde_json::to_string(&keymap).unwrap();
        let back: Keymap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, keymap);
        assert!(!json.contains("\"ctrl\""), "only what is on is written: {json}");
        // None at all is kept as none; the defaults again forget the change.
        keymap.set(flag, &[]);
        assert!(keymap.bindings(flag).is_empty());
        keymap.set(flag, &flag.defaults);
        assert_eq!(keymap, Keymap::default());
    }

    #[test]
    fn a_key_already_used_where_it_would_clash_is_found() {
        let keymap = Keymap::default();
        let p = Binding { key: Key::P, ctrl: false, shift: false, alt: false };
        let names: Vec<&str> =
            keymap.clashes(p, Scope::Develop, "").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Flag as Pick"]);
        // `\` is the Filter bar in Library and Before / After in Develop: no clash between them.
        let backslash = Binding { key: Key::Backslash, ctrl: false, shift: false, alt: false };
        let before_after = commands().iter().find(|c| c.action == Action::BeforeAfter).unwrap();
        assert!(keymap.clashes(backslash, Scope::Develop, &before_after.id).is_empty());
        assert_eq!(keymap.clashes(backslash, Scope::All, "").len(), 2);
    }
}
