---
paths:
  - "crates/app/src/**"
---

# Every function and slider has a hot key

Every new slider, and every new function (menu item, panel or toolbar button, checkbox,
mode switch, menu choice), must be reachable by a hot key and listed in the shortcuts
editor (Edit ▸ Keyboard Shortcuts…). A default key is optional: give one only when the
function is common. Dialog fields (OK, Cancel, a dialog's inputs) are exempt.

How, by kind (registry in `crates/app/src/ui/hotkeys.rs`):

- **Slider**: draw it with `slider_row` / `slider_row_as` (`ui/develop.rs`). A develop
  setting is a `Slider` value: add it to `sliders()` in `hotkeys.rs` (and, for a new
  parameter enum, to its `ALL` list). A slider that is not a setting uses
  `Slider::MaskAmount(<n>)` with an unused `n` ≥ 200 as its stand-in id, also listed in
  `sliders()`. Give it a sensible `default_step`.
- **Panel control** (drawn only while its panel shows): use the `Hot` trait —
  `.hot("develop.<panel>.<what>")` instead of `.clicked()`, `.hot_flip(id, &mut value)`
  after a checkbox — and add the id and a name to `controls()` in `hotkeys.rs`.
- **Choice inside a menu or popup** (not drawn while closed): check
  `hotkeys::fired(ctx, id)` just outside the menu, and put `// key: <id>` above the
  menu's own widget.
- **Menu item or app-level command** (sort, filter, things that need no panel): add it to
  `items()` and `run()` in `ui/app_keys.rs`; draw menu items with `menu_cmd(.., id, ..)`.
- **Something that is already an `Action`** (in `SHORTCUTS`, `ui/shortcuts.rs`): run the
  action; nothing else is needed.

Ids are saved with the user's keys: never rename one once it has shipped.

The test `every_control_has_a_hot_key` (`hotkeys.rs`) scans `crates/app/src/ui` and fails
on a control without a key. A deliberate exception is marked `// no hotkey: <why>` on or
above its line (one item per keyword, preset, lens…), or a whole dialog function is
marked `// hotkeys: none (<why>)` above it. Run `UPDATE_DOCS=1 cargo test -p fotaro
shortcuts` after changing default keys.
