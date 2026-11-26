# Keyboard shortcuts

Generated from `crates/app/src/ui/shortcuts.rs` and `hotkeys.rs` — do not edit by hand.
Run `UPDATE_DOCS=1 cargo test -p fotaro shortcuts` to regenerate.

These are the keys out of the box. Every one can be changed, and every other slider, button and menu item given one, in **Edit ▸ Keyboard Shortcuts…** (Ctrl+/), where each slider's step is set too.

`Ctrl` is `Cmd` on macOS. These follow Lightroom Classic's Library and Develop modules.
Shortcuts work in both modules unless marked (Library) or (Develop).

## View

| Shortcut | Action |
|---|---|
| `G` | Grid view |
| `E` `Enter` | Loupe view |
| `Space` | Toggle Grid / Loupe |
| `C` | Compare view (Select and Candidate) |
| `N` | Survey view (all selected photos) |
| `F` | Full-screen preview |
| `Esc` | Exit full screen / back to Grid |
| `L` | Cycle Lights Out (dim / off / normal) |
| `J` | Cycle Grid cell style (Library) |
| `I` | Cycle Loupe info overlay |
| `Z` | Toggle Loupe zoom (Fit / Navigator zoom level) |
| `Ctrl+=` `Ctrl++` | Zoom in |
| `Ctrl+-` | Zoom out |

## Panels

| Shortcut | Action |
|---|---|
| `Tab` | Hide / show side panels |
| `Shift+Tab` | Hide / show all panels |
| `T` | Hide / show toolbar |
| `\\` | Hide / show Library Filter bar (Library) |
| `F5` | Hide / show top panel |
| `F6` | Hide / show Filmstrip |
| `F7` | Hide / show left panels |
| `F8` | Hide / show right panels |

## Rating & Flagging

| Shortcut | Action |
|---|---|
| `P` | Flag as Pick |
| `X` | Flag as Rejected |
| `U` | Remove flag |
| `0` | Remove star rating |
| `1` | Set rating to 1 star |
| `2` | Set rating to 2 stars |
| `3` | Set rating to 3 stars |
| `4` | Set rating to 4 stars |
| `5` | Set rating to 5 stars |
| `6` | Toggle Red label |
| `7` | Toggle Yellow label |
| `8` | Toggle Green label |
| `9` | Toggle Blue label |
| `Shift+P` | Flag as Pick and go to next photo |
| `Shift+X` | Flag as Rejected and go to next photo |
| `Shift+U` | Remove flag and go to next photo |
| `Shift+0` | Remove rating and go to next photo |
| `Shift+1` | Rate 1 star and go to next photo |
| `Shift+2` | Rate 2 stars and go to next photo |
| `Shift+3` | Rate 3 stars and go to next photo |
| `Shift+4` | Rate 4 stars and go to next photo |
| `Shift+5` | Rate 5 stars and go to next photo |
| `Shift+6` | Toggle Red label and go to next photo |
| `Shift+7` | Toggle Yellow label and go to next photo |
| `Shift+8` | Toggle Green label and go to next photo |
| `Shift+9` | Toggle Blue label and go to next photo |

## Selection & Navigation

| Shortcut | Action |
|---|---|
| `→` | Next photo |
| `←` | Previous photo |
| `↑` | Photo above (Grid) |
| `↓` | Photo below (Grid) |
| `Shift+→` | Extend selection to the next photo |
| `Shift+←` | Extend selection to the previous photo |
| `Shift+↓` | Extend selection down a row (Grid) |
| `Shift+↑` | Extend selection up a row (Grid) |
| `Home` | First photo |
| `End` | Last photo |
| `Page Up` | Page up (Grid) |
| `Page Down` | Page down (Grid) |
| `Ctrl+A` | Select all |
| `Ctrl+D` | Select none |

## Organizing

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+I` | Import photos (Copy, Move or Add) |
| `Ctrl+Shift+E` | Export the selected photos |
| `Ctrl+Alt+Shift+E` | Export with previous settings |
| `Ctrl+[` | Rotate left (counterclockwise) |
| `Ctrl+]` | Rotate right (clockwise) |
| `B` | Add to / remove from Quick Collection |
| `Ctrl+B` | Show Quick Collection |
| `Ctrl+Shift+B` | Clear Quick Collection |
| `Ctrl+Quote` | Create virtual copy |
| `Ctrl+N` | New collection (with the selected photos) (Library) |
| `Ctrl+G` | Group the selected pictures |
| `Ctrl+Shift+G` | Ungroup |
| `S` | Show / hide the version strip |
| `Shift+S` | Show this member for its stack (make it active) |
| `Alt+←` | Previous member of the stack |
| `Alt+→` | Next member of the stack |
| `Delete` | Remove from collection (when viewing a collection) |
| `Ctrl+Backspace` | Delete rejected photos (remove them from the catalog) |

## View

| Shortcut | Action |
|---|---|
| `Ctrl+L` | Enable / disable Library filters |
| `Ctrl+F` | Find (text filter) |

## Organizing

| Shortcut | Action |
|---|---|
| `Ctrl+K` | Add keywords (Keywording panel) |
| `Ctrl+S` | Save metadata to files (XMP sidecars) |
| `Ctrl+E` | Edit in the primary external editor |
| `Ctrl+Alt+E` | Edit in the secondary external editor |
| `Ctrl+/` | Show keyboard shortcuts |

## Modules

| Shortcut | Action |
|---|---|
| `D` `Ctrl+Alt+2` | Develop module |
| `Ctrl+Alt+1` | Library module |

## Develop

| Shortcut | Action |
|---|---|
| `\\` | Show Before / After (the photo without adjustments) |
| `Ctrl+U` | Auto Tone |
| `Ctrl+Shift+U` | Auto White Balance |
| `W` | White Balance Selector (click a neutral area) |
| `J` | Show clipping (shadows blue, highlights red) |
| `Shift+W` | Masking: open / close |
| `R` | Crop & Straighten: open / close (Enter closes; X swaps portrait / landscape) |
| `Ctrl+Alt+R` | Reset the crop |
| `Q` | Remove tool: Heal and Clone spots (open / close) |
| `/` | Remove tool: another source for the selected spot |
| `A` | Remove tool: Visualize Spots (edges white on black) |
| `K` | New Brush mask (paint on the photo; Alt erases) |
| `M` | New Linear Gradient mask (drag on the photo) |
| `Shift+M` | New Radial Gradient mask (drag on the photo) |
| `O` | Masking: show / hide the overlay |
| `[` | Brush: smaller |
| `]` | Brush: larger |
| `{` | Brush: less feather (Shift+[) |
| `}` | Brush: more feather (Shift+]) |
| `Ctrl+Shift+R` | Reset all settings |
| `Ctrl+Shift+C` | Copy settings (choose which) |
| `Ctrl+Shift+V` | Paste settings (the groups chosen in Copy) |
| `Ctrl+N` | New snapshot (the current settings, named) |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` `Ctrl+Shift+Z` | Redo |

## Develop ▸ Sliders

| Shortcut | Action |
|---|---|
| `Comma` | Pick the previous slider |
| `Period` | Pick the next slider |
| `+` `=` | Raise the picked slider |
| `-` | Lower the picked slider |

## Develop ▸ Basic

| Shortcut | Action |
|---|---|
| `Alt+T` | Temp: Raise |
| `Alt+Shift+T` | Temp: Lower |
| `Alt+I` | Tint: Raise |
| `Alt+Shift+I` | Tint: Lower |
| `Alt+E` | Exposure: Raise |
| `Alt+Shift+E` | Exposure: Lower |
| `Alt+C` | Contrast: Raise |
| `Alt+Shift+C` | Contrast: Lower |
| `Alt+H` | Highlights: Raise |
| `Alt+Shift+H` | Highlights: Lower |
| `Alt+S` | Shadows: Raise |
| `Alt+Shift+S` | Shadows: Lower |
| `Alt+W` | Whites: Raise |
| `Alt+Shift+W` | Whites: Lower |
| `Alt+B` | Blacks: Raise |
| `Alt+Shift+B` | Blacks: Lower |
| `Alt+X` | Texture: Raise |
| `Alt+Shift+X` | Texture: Lower |
| `Alt+L` | Clarity: Raise |
| `Alt+Shift+L` | Clarity: Lower |
| `Alt+Z` | Dehaze: Raise |
| `Alt+Shift+Z` | Dehaze: Lower |
| `Alt+V` | Vibrance: Raise |
| `Alt+Shift+V` | Vibrance: Lower |
| `Alt+A` | Saturation: Raise |
| `Alt+Shift+A` | Saturation: Lower |

## Auto Advance

With **Caps Lock** on (Windows, macOS) or **Photo ▸ Auto Advance** checked, flagging, rating and labelling move to the next photo, as if Shift were held.

## Loupe mouse

Click the photo to toggle between Fit and the Navigator zoom level at that point; drag (or scroll) to move around while zoomed. Click or drag in the Navigator to choose the visible area.
