# PhotoLibrary — Plan

## Context

Goal: a Rust/egui desktop app that recreates the **Lightroom Classic Library module** (catalog, import, folders, collections, grid/loupe/compare/survey, culling, filter bar, metadata/keywords). Photo editing (Develop) is out of scope; editing is delegated later via **Edit In** to **NegPy** and **Luminar**.

Current state: a working scaffold (`photo_library` crate, eframe/egui 0.37, edition 2024) at `C:\Users\jbone\Desktop\PhotoLibrary`:
- `src/main.rs` – window setup; `src/app.rs` – menu, file list, details panel, persisted `Settings`; `src/library.rs` – `is_photo()` + recursive `scan()` with 3 unit tests.
- Note for egui 0.37: `App::ui(&mut self, ui, frame)`, `egui::Panel::{top,bottom,left}(..).show(ui, ..)`, `.default_size()`.

### Decisions made with the user
| Topic | Decision |
|---|---|
| Metadata storage | SQLite catalog is the source of truth, with optional **XMP sidecars** (LrC/Bridge/darktable compatible). Originals are never modified. |
| LrC `.lrcat` migration | Yes, as a later phase. |
| RAW formats | All majors: NEF, CR2/CR3, ARW, RAF (X-Trans), DNG. → **LibRaw** as the RAW decoder. |
| Platforms | Windows + macOS + Linux from day one. |
| Film scans | Camera-scanned RAW and scanner 16-bit TIFFs. Both go to NegPy as originals. |
| Export dialog | In scope, as a later phase. |

## UI/UX priority: it should feel immediately familiar to Lightroom Classic users

This is a top-level requirement, not polish. Every phase is judged against "would a Lightroom Classic user know what to do without thinking?" The goal is a familiar **layout, behaviour and shortcuts**, not a pixel copy: no Adobe names, logos or icon artwork.

**Layout (built as a shell in Phase 0; later phases fill the panels):**
- **Top**: identity plate area on the left, module picker on the right. Only "Library" is active. "Edit In…" lives in the Photo menu, as in LrC.
- **Left panel stack**: Navigator, Catalog (All Photographs, Quick Collection, Previous Import), Folders (volume headers with free space, photo counts), Collections.
- **Right panel stack**: Histogram, Keywording, Keyword List, Metadata (preset dropdown, EXIF/IPTC views). No Quick Develop.
- **Centre**: Grid/Loupe/Compare/Survey views, with the **library filter bar** above (Text | Attribute | Metadata | None) and the **toolbar** below (view buttons, sort order, flag/star/label controls, thumbnail size slider).
- **Bottom**: filmstrip with source breadcrumb and a quick filter.
- **Panel behaviour**: collapsible section headers with ▸/▾ triangles; Solo mode (Alt-click a header to keep only one section open); edge arrows to hide/show each side; resizable side panels; layout remembered per catalog.

**Look:** a dark charcoal theme with LrC-like grey tones (a custom egui `Visuals`), compact typography, grid cells with a lighter "selected" highlight and an index number, cell styles cycled with **J**, and a loupe info overlay toggled with **I**.

**Keyboard shortcut parity (a table kept in `docs/SHORTCUTS.md` and tested):**
- **Views**: G (grid), E (loupe), C (compare), N (survey), F (full-screen preview), L (lights out), Tab / Shift+Tab (hide panels), T (toolbar), \ (filter bar).
- **Culling**: P / X / U (flags), 0–5 (stars), 6–9 (colour labels), Caps Lock (auto-advance), B (Quick Collection).
- **Organising**: Ctrl+G / Ctrl+Shift+G (stack / unstack), Ctrl+[ / Ctrl+] (rotate), Ctrl+Shift+I (import), Ctrl+E (Edit In with the default editor), Ctrl+K (keywording). On macOS, Cmd replaces Ctrl.
- **Selection**: Ctrl+A, Ctrl+D (select none), Shift- and Ctrl-click, arrow keys, Space (Loupe), Z (zoom).

**Interactions:** drag photos onto collections and folders; drag folders to move them; right-click context menus matching LrC's wording, e.g. "Show in Explorer/Finder", "Synchronize Folder…", "Add to Quick Collection".

## Architecture

**Cargo workspace:**
- `crates/catalog`: `rusqlite` (feature `bundled`, so there's no system SQLite on any OS). Schema migrations keyed on `PRAGMA user_version`; folder scanning (moved from `src/library.rs`); incremental sync. No GUI dependencies.
- `crates/imaging`: a `Decoder` trait with `embedded_preview()`, `render(max_px)` and `metadata()`, plus the disk preview cache.
  - `ImageDecoder` uses the `image` crate. 16-bit TIFFs are converted to 8-bit for display; ICC profiles via `lcms2`.
  - `RawDecoder` uses LibRaw: the embedded JPEG for thumbnails and standard previews, a full `dcraw_process` render for 1:1 previews.
  - EXIF: `nom-exif` for non-RAW files, LibRaw metadata for RAW.
- `crates/app`: the egui binary, plus a background job system: worker threads, `crossbeam-channel`, priority queue (visible thumbnails first), progress events, and `ctx.request_repaint()`.

**Catalog layout (mirrors LrC):** a user-chosen catalog directory `MyLibrary.plcat/` containing `catalog.db` and `previews/<xx>/<photo_id>_<size>.jpg`. The cache key includes the source file's mtime and size, so changed files get fresh previews. The default location comes from the `directories` crate, so paths are correct on all three operating systems.

**Schema v1:** `folders(id, path, parent_id)`, `files(id, folder_id, name, ext, kind, size, mtime)`, `photos(id, master_file_id, capture_time, rating, flag, color_label, orientation)`, `exif(photo_id, camera, lens, iso, aperture, shutter, focal_len, width, height, gps)`, `keywords(id, name, parent_id)`, `photo_keywords`, `collections(id, name, parent_id, kind, smart_rules_json)`, `collection_photos`. Stacks, virtual copies and sidecar tracking are added by later migrations.

## Roadmap
| # | Phase | Done when |
|---|---|---|
| 0 | **Foundations**: workspace, catalog + migrations, add-folder sync, job system, preview cache, LibRaw spike | Adding a folder fills the catalog in the background; restart doesn't rescan; thumbnails appear in the cache for every format |
| 1 | **Grid & Folders**: folder tree, virtualized thumbnail grid, filmstrip, GPU texture LRU | 10k photos scroll smoothly |
| 2 | **Loupe & culling**: zoom/1:1, P/X/U, 0–5 stars, 6–9 labels, auto-advance | A full shoot can be culled from the keyboard |
| 3 | **Metadata**: EXIF panel, IPTC editing, keyword tree, XMP sidecar read/write | Ratings/keywords round-trip with darktable/LrC |
| 4 | **Find & group**: filter bar (text/attribute/metadata), collections, smart collections, stacks, RAW+JPEG pairing | Combined filters work |
| 5 | **Import dialog**: add/copy/move, rename templates, duplicate skip, card import | Card → dated folders works |
| 6 | **Compare/Survey**, virtual copies, 1:1 render quality | |
| 7 | **Edit In**: configurable editors (NegPy, Luminar), send original or rendered TIFF, watch for result, auto-stack | Edited file reappears stacked with its original |
| 8 | **Export**: JPEG/TIFF, resize, quality, rename, metadata include/strip | |
| 9 | **LrC `.lrcat` migration**: folders, ratings, flags, labels, keywords, collections | A real LrC catalog imports |

## Phase 0: implementation steps (next)

1. **Write `docs/PLAN.md`** in the repo (this roadmap plus the decisions table) and make the initial git commit of the scaffold.
2. **LibRaw spike, done first because it's the biggest risk.** Test the `rsraw` crate:
   - Does it build with MSVC on Windows (and later on CI for macOS/Linux)?
   - Which LibRaw version does it bundle? It needs to be recent enough for CR3 and current cameras.
   - Can it extract embedded previews and do a full decode?

   If it falls short, write our own thin `crates/libraw-sys`: LibRaw source vendored as a git submodule and built with `cc`, with a small safe wrapper over `libraw_init / open_file / unpack_thumb / dcraw_make_mem_thumb / unpack / dcraw_process / dcraw_make_mem_image / close`. Test files: one public sample per format from raw.pixls.us (NEF, CR2, CR3, ARW, RAF, DNG), kept out of git under `testdata/` with a fetch script.
3. **Convert to a workspace**: move the scaffold into `crates/app`, and move `library.rs` into `crates/catalog::scan`, keeping its tests.
4. **Catalog crate**: open/create the catalog, run migrations, schema v1, `sync_folder(root)` (insert/update/remove based on mtime and size) with tempdir-based tests.
5. **Imaging crate**: the `Decoder` trait, `ImageDecoder` (8/16-bit, ICC), `RawDecoder` (from the spike), and the preview cache with 256 px thumbnail and 2048 px standard sizes.
6. **Job system in the app**: a worker pool for scanning and preview jobs, with a progress display in the status bar.
7. **LrC-style UI shell**, in a new `crates/app/src/ui/` module (`theme.rs`, `panels.rs` for collapsible/solo sections, `layout.rs`, `shortcuts.rs`):
   - The dark theme and the full panel layout described above, with placeholder sections.
   - Hide/show behaviour for Tab, Shift+Tab, T and \, plus a central shortcut dispatcher driven by a table.
   - Wiring: File → New/Open Catalog, Add Folder; the Folders panel and a basic thumbnail list come from the catalog. The real virtualized grid comes in Phase 1.
8. **CI**: a GitHub Actions matrix for Windows, macOS and Linux running `cargo build`, `cargo test` and `cargo clippy`. It runs once a remote exists.

## Verification
- `cargo test --workspace`: migrations apply to an empty database and are idempotent; sync adds, updates and removes files correctly; cache keys change when mtime changes; decoders produce previews for each format in `testdata/`.
- Manual check: `cargo run`, create a catalog, add a folder containing the sample RAWs and a 16-bit TIFF. The UI stays responsive and thumbnails appear in `previews/`. Restart, and photos load from the catalog with no rescan.
- `cargo clippy --workspace -- -D warnings` is clean.
- UX check: unit tests on the shortcut table (no duplicate bindings; every entry in `docs/SHORTCUTS.md` is bound). Manual walkthrough: the layout matches LrC's regions, and Tab, Shift+Tab, T, \ and Alt-click solo mode behave as they do in LrC.

## Open items to verify later
- How NegPy is launched from the command line or by file association (needed for Phase 7).
- How Luminar Neo writes back its result (same folder as the original or a separate export path?).
