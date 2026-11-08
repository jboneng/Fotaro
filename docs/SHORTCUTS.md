# Keyboard shortcuts

Generated from `crates/app/src/ui/shortcuts.rs` — do not edit by hand.
Run `UPDATE_DOCS=1 cargo test -p photo_library shortcuts` to regenerate.

`Ctrl` is `Cmd` on macOS. These follow Lightroom Classic's Library module.

## View

| Shortcut | Action | Status |
|---|---|---|
| `G` | Grid view | ✅ |
| `E` | Loupe view | ✅ |
| `Space` | Toggle Grid / Loupe | ✅ |
| `Enter` | Open selected photo in Loupe | ✅ |
| `C` | Compare view | planned (Phase 6) |
| `N` | Survey view | planned (Phase 6) |
| `F` | Full-screen preview | ✅ |
| `Esc` | Exit full screen / back to Grid | ✅ |
| `L` | Cycle Lights Out (dim / off / normal) | ✅ |
| `J` | Cycle Grid cell style | ✅ |
| `I` | Cycle Loupe info overlay | ✅ |
| `Z` | Toggle Loupe zoom (Fit / Navigator zoom level) | ✅ |
| `Ctrl+=` | Zoom in | ✅ |
| `Ctrl++` | Zoom in | ✅ |
| `Ctrl+-` | Zoom out | ✅ |

## Panels

| Shortcut | Action | Status |
|---|---|---|
| `Tab` | Hide / show side panels | ✅ |
| `Shift+Tab` | Hide / show all panels | ✅ |
| `T` | Hide / show toolbar | ✅ |
| `\\` | Hide / show Library Filter bar | ✅ |
| `F5` | Hide / show top panel | ✅ |
| `F6` | Hide / show Filmstrip | ✅ |
| `F7` | Hide / show left panels | ✅ |
| `F8` | Hide / show right panels | ✅ |

## Rating & Flagging

| Shortcut | Action | Status |
|---|---|---|
| `P` | Flag as Pick | ✅ |
| `X` | Flag as Rejected | ✅ |
| `U` | Remove flag | ✅ |
| `0` | Remove star rating | ✅ |
| `1` | Set rating to 1 star | ✅ |
| `2` | Set rating to 2 stars | ✅ |
| `3` | Set rating to 3 stars | ✅ |
| `4` | Set rating to 4 stars | ✅ |
| `5` | Set rating to 5 stars | ✅ |
| `6` | Toggle Red label | ✅ |
| `7` | Toggle Yellow label | ✅ |
| `8` | Toggle Green label | ✅ |
| `9` | Toggle Blue label | ✅ |
| `Shift+P` | Flag as Pick and go to next photo | ✅ |
| `Shift+X` | Flag as Rejected and go to next photo | ✅ |
| `Shift+U` | Remove flag and go to next photo | ✅ |
| `Shift+0` | Remove rating and go to next photo | ✅ |
| `Shift+1` | Rate 1 star and go to next photo | ✅ |
| `Shift+2` | Rate 2 stars and go to next photo | ✅ |
| `Shift+3` | Rate 3 stars and go to next photo | ✅ |
| `Shift+4` | Rate 4 stars and go to next photo | ✅ |
| `Shift+5` | Rate 5 stars and go to next photo | ✅ |
| `Shift+6` | Toggle Red label and go to next photo | ✅ |
| `Shift+7` | Toggle Yellow label and go to next photo | ✅ |
| `Shift+8` | Toggle Green label and go to next photo | ✅ |
| `Shift+9` | Toggle Blue label and go to next photo | ✅ |

## Selection & Navigation

| Shortcut | Action | Status |
|---|---|---|
| `→` | Next photo | ✅ |
| `←` | Previous photo | ✅ |
| `↑` | Photo above (Grid) | ✅ |
| `↓` | Photo below (Grid) | ✅ |
| `Shift+→` | Extend selection to the next photo | ✅ |
| `Shift+←` | Extend selection to the previous photo | ✅ |
| `Shift+↓` | Extend selection down a row (Grid) | ✅ |
| `Shift+↑` | Extend selection up a row (Grid) | ✅ |
| `Home` | First photo | ✅ |
| `End` | Last photo | ✅ |
| `Page Up` | Page up (Grid) | ✅ |
| `Page Down` | Page down (Grid) | ✅ |
| `Ctrl+A` | Select all | ✅ |
| `Ctrl+D` | Select none | ✅ |

## Organizing

| Shortcut | Action | Status |
|---|---|---|
| `Ctrl+Shift+I` | Import photos (Copy, Move or Add) | ✅ |
| `Ctrl+[` | Rotate left (counterclockwise) | ✅ |
| `Ctrl+]` | Rotate right (clockwise) | ✅ |
| `B` | Add to / remove from Quick Collection | ✅ |
| `Ctrl+B` | Show Quick Collection | ✅ |
| `Ctrl+Shift+B` | Clear Quick Collection | ✅ |
| `Ctrl+N` | New collection (with the selected photos) | ✅ |
| `Ctrl+G` | Group into stack | ✅ |
| `Ctrl+Shift+G` | Unstack | ✅ |
| `S` | Collapse / expand stack | ✅ |
| `Shift+S` | Move to top of stack | ✅ |
| `Delete` | Remove from collection (when viewing a collection) | ✅ |

## View

| Shortcut | Action | Status |
|---|---|---|
| `Ctrl+L` | Enable / disable Library filters | ✅ |
| `Ctrl+F` | Find (text filter) | ✅ |

## Organizing

| Shortcut | Action | Status |
|---|---|---|
| `Ctrl+K` | Add keywords (Keywording panel) | ✅ |
| `Ctrl+S` | Save metadata to files (XMP sidecars) | ✅ |
| `Ctrl+E` | Edit in default external editor | planned (Phase 7) |
| `Ctrl+/` | Show keyboard shortcuts | ✅ |

## Auto Advance

With **Caps Lock** on (Windows, macOS) or **Photo ▸ Auto Advance** checked, flagging, rating and labelling move to the next photo, as if Shift were held.

## Loupe mouse

Click the photo to toggle between Fit and the Navigator zoom level at that point; drag (or scroll) to move around while zoomed. Click or drag in the Navigator to choose the visible area.
