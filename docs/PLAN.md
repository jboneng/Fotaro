# PhotoLibrary — Plan

## Context

Goal: a Rust/egui desktop app that recreates the **Lightroom Classic Library module** (catalog, import, folders, collections, grid/loupe/compare/survey, culling, filter bar, metadata/keywords). Editing was first delegated to external apps through **Edit In** (NegPy, Luminar, darktable). A **Develop** module of our own is now being added, starting with LrC's Basic panel (see "Design: Develop module" below); Edit In stays.

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
- ~~How NegPy is launched from the command line~~: `NegPy <files> --format … --output-dir … --filename …
  [--quit-after-export]` (NegPy's launch support, in its source; releases after 0.68.0).
- ~~How Luminar Neo writes back its result~~: as its Lightroom plug-in launches it, `Luminar Neo.exe
  lightroom <base64 photo> -MPResPath <base64 result>`; per the plug-in, Apply writes the TIFF there and Luminar exits.

## Progress (Phase 0)
- [x] 1. Plan + initial commit
- [x] 2. LibRaw: `rsraw` rejected (refuses MSVC, LibRaw 0.21.3, needs libclang). Own `crates/libraw` (vendored 0.22.3 + C++ shim + safe wrapper) builds on MSVC. Verified on real samples (see "Real-file check" below); CR2/CR3/ARW/RAF still untested for lack of files.
- [x] 3. Workspace (`app`, `catalog`, `libraw`)
- [x] 4. Catalog: schema v1 + migrations, `sync_folder` (add/update/missing/restore; missing files keep metadata), folder & photo queries
- [x] 5. Imaging crate: RAW (embedded preview → half-size render fallback), 8/16-bit images, ICC → sRGB (lcms2), EXIF; preview cache (256 px + 2048 px JPEG, fingerprinted)
- [x] 6. Job system: worker pool, Visible (LIFO, capped) / Normal / Background lanes, per-photo build lock, progress in identity plate
- [x] 7. LrC-style UI shell: menus, identity plate + module picker, collapsible/solo panel sections, Navigator/Catalog/Folders/Collections, Histogram/Metadata, Grid (3 cell styles) / Loupe / full screen, filmstrip, toolbar, filter bar, Lights Out, toasts, shortcut table (docs/SHORTCUTS.md). Planned features show which phase delivers them.
- [x] 8. CI: GitHub Actions — fmt check; clippy (-D warnings) + tests on Windows, macOS, Linux

## Progress (Phase 1)
Baseline measured with a generated 10,000-photo library (100 folders, 1600 px JPEGs), release build,
16-thread laptop:

| | First run | After fixes |
|---|---|---|
| Import (sync) | 0.4 s, no metadata | 1.2 s **with** capture dates/camera data |
| Build all previews | 315 s | 222 s |
| Frames over 16.7 ms while building | ~10% | 0.4% |
| Frames over 100 ms | many (one 13 s) | 0 |
| Scrolling after build | 5 ms/frame | 5 ms/frame |

Fixes: metadata read in parallel during sync (one transaction, no reload churn, no SQLite write
contention from workers); SIMD `jpeg-encoder` for previews; free-space queries off the UI thread;
folder tree computed once per change; workers at lowest OS priority; mimalloc allocator.
Measure again with `PHOTOLIB_PERF=1` (frame stats) and the `imaging` `bench` example.

- [x] 10k-photo performance pass
- [x] Sort options (Capture Time, Added Order, Pick, Label Color, Rating, File Name, Extension, Aspect Ratio) + direction
- [x] Home / End / Page Up / Page Down
- [x] Folder operations (LrC context menu): missing-folder "?" (checked off the UI thread), Create Folder inside…, Rename… (disk + catalog, undone on failure), Remove… (catalog only, purges previews), Synchronize Folder…, Show Parent Folder, Show in Explorer/Finder
- [x] Library ▸ Previews ▸ Build Standard-Sized Previews
- [x] "Find Missing Folder…" (missing folders) / "Update Folder Location…" (present ones): repoint a folder and its subfolders, keeping ratings etc.; refuses a location holding none of its photos or one that collides with catalog folders; re-syncs afterwards
- [x] Orphaned-preview cleanup on catalog open (removed photos, old versions, `.tmp` leftovers; previews newer than the catalog snapshot are kept; skipped for an empty catalog)

### Real-file check (2026-11-08)
A 15-file sample of the user's own library lives in `testdata/real/` (git-ignored, 1.3 GB):
D750 NEFs, a D750-converted DNG, SilverFast/Plustek OpticFilm 8200i raw-negative DNGs, a
"SilverScan Studio" linear DNG, 16-bit scanner TIFFs (incl. the `_IR` infrared channel), NegPy
`.NEF.tiff` output, and PNG/JPEG exports. Decode them all with
`cargo run -p imaging --example decode --release -- <out dir> testdata/real/*/*`.

All 15 decode with correct orientation and colour (negatives display as negatives, as in LrC).
Fixes from this pass:
- Resizing with `fast_image_resize` (SIMD) instead of `image`: a 24 MP NEF standard preview went
  from 1.4 s to 0.20 s, thumbnails from 32 ms to 11 ms.
- Embedded RAW previews must reach 75% of the requested size; the 1024 px previews inside converted
  DNGs looked soft in the loupe, so those are rendered instead (as LrC does).
- Files without a capture date (scans, exports) use the file's modification time, like LrC.
- Lens names for Nikon NEFs, built from the makernote focal/aperture range ("60.0 mm f/2.8").
- Image files are memory-mapped for decoding (TIFF decoding seeks a lot; a BufReader was ~2x slower).

Still slowish: 184 MB 16-bit TIFFs take 1–2.5 s and SilverFast raw DNGs ~2 s per standard preview
(timings were noisy while Windows Defender scanned the freshly copied files). Fine for background
building; revisit with 1:1 previews in Phase 2.

## Progress (Phase 2)
- [x] Loupe zoom: Z / click toggles Fit ↔ the Navigator level (FILL, 1:1, 2:1, 3:1), anchored at the
      pointer; Ctrl+= / Ctrl+- step through levels; drag or scroll to pan; zoom position kept when
      moving between photos (checking focus across a burst). 1:1 = one image pixel per screen pixel.
- [x] 1:1 previews (`f` in the preview cache), built on demand: RAWs use the full-size embedded JPEG
      when present (same colours as the standard preview), else a full LibRaw render. The enlarged
      standard preview shows with "Loading…" meanwhile; the next photo's 1:1 is prefetched.
      Unused 1:1 previews are discarded after 30 days (LrC default). Measured: D750 NEF 2 s,
      SilverScan linear DNG 5 s (debug build).
- [x] Navigator: zoom level picker, visible-area rectangle, click/drag to move the view.
- [x] Rotate Left / Right (Ctrl+[ / Ctrl+], Photo menu, context menu): catalog only, stored as an
      EXIF orientation code in `photos.orientation` (ready for XMP in Phase 3); applied at draw time
      everywhere (grid, filmstrip, Navigator, Loupe, full screen) and to displayed dimensions.
      On Nordic layouts the keys in the US `[` `]` positions (Å, ¨) work via egui's physical-key fallback.
- [x] Auto Advance: Photo ▸ Auto Advance, or Caps Lock (Windows, macOS; Linux has the menu only).
- [ ] Later: rotate buttons on grid cells (hover), Fill/ratio dropdown with more levels, tiled
      rendering for images larger than the GPU texture limit (currently scaled to fit the limit).

## Progress (Phase 3)
- [x] `crates/xmp`: reads and updates XMP in place, keeping everything it doesn't manage (LrC develop
      settings, darktable history, xpacket wrappers, unknown namespaces); namespace-aware (any prefix).
      Managed: xmp:Rating, xmp:Label, tiff:Orientation, dc:subject + lr:hierarchicalSubject,
      dc:title/description/creator/rights, and our own pick flag (LrC keeps flags in its catalog only).
- [x] Sidecar naming: RAW `IMG.xmp` (LrC/Bridge/Capture One; darktable reads it on import), other
      files `IMG.tif.xmp` (darktable/digiKam), so RAW+JPEG pairs never share a sidecar. Originals
      are never modified (LrC would embed metadata in JPEG/TIFF/DNG; we don't).
- [x] Schema v2: IPTC fields, `metadata_dirty`, `xmp_mtime_ms`, file orientation, unique root keywords.
- [x] Save Metadata to Files (Ctrl+S), Read Metadata from Files (confirmed), Automatically Write
      Changes into XMP (Metadata menu; off by default as in LrC). "Needs saving" badge on grid cells.
      Rotation is written as the displayed orientation (file orientation + catalog rotation).
- [x] Adding photos reads existing sidecars (rating, label, keywords, IPTC, rotation); a broken
      sidecar is logged and skipped, never overwritten.
- [x] Keywording panel: Keyword Tags (editable, `*` = on some photos), entry field with LrC syntax
      (`Child < Parent`, also `Parent > Child`), suggestions while typing, Recent Keywords; Ctrl+K.
- [x] Keyword List: tree, filter, tri-state checkboxes for the selection, counts, create inside /
      edit / delete (with confirmation).
- [x] Metadata panel presets Default / EXIF / IPTC with editable Title, Caption, Creator, Copyright
      (applied to all target photos; `< mixed >` when they differ).
- [ ] Later: "metadata changed on disk" conflict badge (xmp_mtime_ms is tracked for it), keyword
      synonyms and export flags, more IPTC fields (location, headline), GPS.

## Progress (Phase 4)
- [x] One criteria language (`catalog::query`) compiled to SQL for both the Library Filter and smart
      collections; Unicode case-insensitive text matching (custom `plfold` SQL function, so
      Å/å and Ø/ø match), parameters always bound (no injection, `%`/`_` are plain text).
- [x] Library Filter bar: Text (field + Contains All/Contains/Doesn't Contain/Starts/Ends With),
      Attribute (flags, rating with ≥/≤/=, color labels incl. "no label", raw/other), Metadata
      (4 cascading columns: Date, Camera, Lens, Keyword, File Type, Label, with counts). Click a mode
      for only it, Shift-click to combine; None; Filters On/Off (Ctrl+L); Find (Ctrl+F). Filtered
      views and smart collections follow edits immediately.
- [x] Collections panel: collection sets, collections, smart collections (rule editor with a live
      match count), rename/delete, drag photos onto a collection, Add to Collection menu, New
      Collection with the selection (Ctrl+N), Delete removes from the shown collection.
- [x] Quick Collection: B toggles (all in -> remove, else add), Ctrl+B shows, Ctrl+Shift+B clears,
      drag onto it; dot badge on cells.
- [x] Stacks: Ctrl+G (merges existing stacks; top = first selected), Ctrl+Shift+G, S collapse/expand,
      Shift+S move to top; stacks stay together in any sort; badges "3" / "2/3".
- [x] RAW+JPEG pairs: a new JPEG next to a raw file with the same name becomes part of that photo
      ("NEF+JPG"), unless Library ▸ "Treat JPEG Files Next to Raw Files as Separate Photos".
- [x] Shift+arrows extend the selection (LrC); 2x2 supersampling when drawing images smaller than
      their texture (egui-wgpu has no mipmaps: thin lines like whiskers aliased into dots).
- [ ] Later: filter presets and per-source filter locking, target collection (B adds to it),
      Metadata column hierarchies (year > month > day), dragging collections into sets.

## Progress (Phase 5)
- [x] `catalog::import`: scan a source (capture times read on all cores; suspected duplicates =
      same file name and size as a catalog file), plan destinations, transfer, add to the catalog
      as one import (Previous Import), apply keywords. Unit tested end to end (copy into dated
      folders, move, add, cancel, duplicates, pairs) plus a job-system test of the Import button path.
- [x] Copy (default) / Move / Add. Copies go through a temp file, are size-checked, keep the file
      date, and never overwrite (collisions get `-2`, `-3`); Move deletes an original only after its
      copy is verified (or renames on the same drive). XMP sidecars travel with their photos.
- [x] Organize by date (2026/2026-11-08, 2026/11/08, 2026/2026-11, 2026-11-08, 2026) or into one
      folder, optional "Into Subfolder"; date folders appear under the destination in Folders.
- [x] Renaming templates with presets ({Filename} {Text} {Seq:N} {OrigNum} {YYYY} {MM} {DD} {hh}
      {mm} {ss}), live sample and errors; RAW+JPEG pairs share a sequence number and suffix.
- [x] Import dialog (Ctrl+Shift+I, File ▸ Import Photos…, Folders ▸ Add Folder… opens it in Add
      mode): drives with cards (DCIM) first and auto-selected, Choose Folder…, recent source,
      Include Subfolders; thumbnail grid with check boxes, Check All / Uncheck All, duplicates
      marked; settings remembered; progress with cancel in the identity plate; shows Previous Import
      when done. Closes only with Cancel / Esc. `--import <folder>` opens it from the command line.
- [ ] Later: eject card after import, import presets, video files, "Make a Second Copy To",
      building standard/1:1 previews as an import option, date-range/thumbnail size in the grid.

## Progress (Phase 6)
- [x] Compare (C): Select | Candidate panes sharing one zoom (Z / click / Navigator) and position;
      the active pane (white frame) gets ratings, flags and labels; click the other pane to
      activate it; ←/→ step the Candidate, ↑ Make Select, ↓ Swap; filmstrip picks the Candidate.
- [x] Survey (N): all selected photos laid out as large as possible; click to activate, ✕ to
      drop from the survey, arrows move between them; Auto Advance stays inside the view.
- [x] Virtual copies (Ctrl+', Photo menu, context menu): schema v4 rebuilds `photos` (one file may
      back several photos) with foreign keys off and a foreign-key check, keeping all ids; copies
      take keywords, label, IPTC and rotation, start unrated, stack after their source (open),
      share the master's previews, never write XMP, and Delete removes them after confirmation.
- [x] 1:1 quality: request the GPU's real texture limit (32768 px here instead of egui's 8192),
      so very large scans show at full resolution.
- [ ] Later: Compare zoom unlinking (LrC's lock), Survey drag-reordering, renaming virtual copies.

## Progress (Phase 7: NegPy)
- [x] Photo ▸ Edit In ▸ Edit in NegPy… (Ctrl+E, also in the photo context menu) opens the selected
      originals in NegPy with an export recipe: TIFF / JPEG / PNG, 8 or 16 bits, colour space,
      exported next to the original as `<name>_NegPy`. One launch per folder; later launches hand
      their files to the running NegPy. A single photo closes NegPy after its export.
- [x] NegPy is found without setup: a release newer than 0.68.0 (registry, Program Files,
      /Applications, PATH), else a source checkout with launch support and a `.venv` in the usual
      development folders (or `NEGPY_SOURCE`). Edit ▸ External Editing Settings… shows what was found
      and lets you choose a folder or program instead.
- [x] Results are found by name and by their change time (NegPy stamps exports with the original's
      modified time), taken once two looks see the same version, added to the catalog with the
      original's rating, label, IPTC and keywords, and stacked right below it. Exporting again from
      NegPy updates the photo, its metadata and its previews. NegPy launch failures are reported.
- [x] Checked end to end with NegPy from source: launch, recipe, both results added and stacked.
- [x] Luminar Neo (Photo ▸ Edit In ▸ Edit in Luminar Neo…): found in the registry / Program Files,
      launched like its Lightroom plug-in does, one photo at a time (the next opens when Luminar
      closes). Luminar opens the original, RAW included; Apply writes `<name>_LuminarNeo-edit.tif`
      (`_0`, `_1`… when taken) next to it, which is added and stacked like NegPy's. Closing without
      Apply is reported. Ctrl+E opens the primary editor (External Editing settings), Ctrl+Alt+E the
      other.
- [x] Checked: Luminar Neo 1.29 opens a NEF in its plug-in mode (Cancel / Apply) from these arguments.
- [x] darktable (Photo ▸ Edit In ▸ Edit in darktable…): found in the registry / Program Files (with
      darktable-cli). The originals open in one darktable window; when it closes, each photo whose
      darktable sidecar (`photo.NEF.xmp`) changed is rendered with darktable-cli (16 or 8 bit TIFF,
      sRGB / Adobe RGB / ProPhoto / Display P3) to `<name>_darktable.tif`, through a temporary file
      (darktable-cli never overwrites; output paths need forward slashes). Unchanged photos and a
      darktable that closes at once (already open) are reported. Settings: primary editor (Ctrl+E)
      and secondary editor (Ctrl+Alt+E), as in LrC.
- [x] Checked: darktable-cli 5.6.2 renders a NEF to 16-bit TIFF in each of the four colour spaces.
- [ ] To check by hand: editing in the darktable window, closing it, the render coming back.
- [ ] To check by hand: Apply in Luminar writes the result and it comes back stacked.

## Progress (Phase 8)
- [x] Export dialog (File ▸ Export…, Ctrl+Shift+E, photo context menu) laid out like LrC's:
      Export Location (specific folder or the original's folder, subfolder, Add to This Catalog,
      Add to Stack, Existing Files: new name / overwrite / skip), File Naming (the import rename
      templates, with an example), File Settings, Image Sizing, Metadata, Post-Processing.
      Settings are remembered; Export with Previous (Ctrl+Alt+Shift+E) skips the dialog.
- [x] Formats: JPEG (quality), TIFF 8-bit (none or ZIP with predictor, strips compressed in
      parallel), PNG, all sRGB with an embedded ICC profile; Original copies the file byte for
      byte plus an XMP sidecar holding the catalog metadata (and rotation).
- [x] Pixels: full-size decode (RAW: full-size embedded JPEG or LibRaw render), catalog rotation
      applied, Lanczos3 resize to Width & Height / Dimensions / Long Edge / Short Edge /
      Megapixels / Percentage, Don't Enlarge, resolution (ppi) written to the file.
- [x] Metadata: Copyright Only / Copyright & Contact Info Only / All Except Camera Info / All
      Metadata. EXIF (camera, lens, exposure, capture time, artist, copyright, caption) and XMP
      (title, caption, keywords with hierarchy, rating, label) in JPEG APP1, TIFF tags and PNG chunks.
- [x] Runs as a background job, a few photos at a time on low-priority threads, with progress
      and ✕ in the identity plate; writes through `.part` files so nothing half-written is left.
      Exports added to the catalog are never paired as RAW+JPEG with their original.
- [x] Real files (release build): a 24 MP NEF renders in 0.1 s; JPEG 0.2 s, TIFF-ZIP 0.6 s,
      PNG 0.2 s.
- [x] Presets: a preset column in the dialog with built-in presets (Full-Sized JPEGs, For Web,
      For Email, 16-bit TIFF for Editing, Print) that set file, size, sharpening and metadata but
      keep location and naming, and user presets (Add, Remove, right-click Update with Current
      Settings / Delete) holding every setting. File ▸ Export with Preset and the context menu
      export without the dialog. Presets are app-wide, like LrC's.
- [x] 16 bits per channel for TIFF and PNG. 16-bit scans and TIFFs keep their precision; RAWs
      export their full-size camera preview (what the Library shows), and only RAWs without one
      are rendered by LibRaw at 16 bits (linear ProPhoto, converted to the output space).
- [x] Colour spaces: sRGB, Display P3, Adobe RGB (1998), ProPhoto RGB, with the matching ICC
      profile embedded (and EXIF ColorSpace). Sources are converted from their own profile,
      in parallel (lcms2).
- [x] Output Sharpening: Screen / Matte Paper / Glossy Paper, Low / Standard / High: an unsharp
      mask on luminance after resizing; paper radii grow with the ppi.
- [x] Watermarking: Simple Copyright Watermark (each photo's copyright), Text or Graphic
      (PNG/JPEG with transparency); opacity, size, inset, 9 anchors, white/black, shadow.
- [x] JPEG Limit File Size To: the best quality that fits, by bisection.
- [x] GPS: read at import (LibRaw for RAWs, EXIF otherwise), stored in the catalog (schema v5),
      shown in the Metadata panel; older catalogs are read in the background. Exports carry it
      in EXIF unless Remove Location Info is ticked or the level leaves it out.
- [x] Real files: a 16-bit scan TIFF (4932×3309) exports as 16-bit ProPhoto TIFF in ~3 s; a
      silverfast DNG renders at 16 bits in ~1 s.
- [ ] Later: Export as Catalog, PSD/DNG/JPEG XL formats, Remove Person Info (no people yet),
      a watermark editor with a live preview.

## Design: Stacks, pictures and the version strip (next)

LrC's stacks use one mechanism for different relationships (parts of one capture, versions
of one picture, photos grouped by hand), and a collapsed stack hides photos unpredictably.
This replaces them. Decided with the user:

- Sidecar images (IR pass, its mask, the JPEG of a RAW+JPEG pair) are **members of the
  stack**, not separate grid cells and not hidden parts.
- A stack has an **active member**: it is what the grid shows and what is rated, exported…
  The newest edit becomes active automatically.
- Manual grouping stays.
- Members are reached through a **version strip**, not by expanding the stack in the grid.
- **Two levels**: a group (made by hand) holds pictures; a picture holds its members.
- A filter matches a stack when **any member** matches.
- Later (separate brainstorm): NegPy renders positive previews of negatives at import.

### Model

- **Picture**: one capture and everything made from it. Every photo belongs to exactly
  one picture; a lone photo is a picture of one.
- **Member roles** within a picture:

  | Role | Examples | How it joins |
  |---|---|---|
  | Original | the scan, the camera RAW | the photo the picture is built around |
  | Pass | `<name>_IR.tif(f)`, `<name>_IR_VALID.tif(f)` beside `<name>.tif(f)` (NegPy's rule, case-insensitive); the JPEG of a RAW+JPEG pair | by name, at import / sync / add |
  | Edit | `<name>_NegPy.*`, `<name>_LuminarNeo-edit[_n].tif` | Edit In, or by name (edits made outside the app) |
  | Copy | virtual copies | Create Virtual Copy |
  | Export | exports added with "Add to Stack" | Export |

  Edits, copies and exports record the member they were made **from** (for "compare with
  original", whatever the order).
- **Group**: pictures put together by hand (bursts, brackets, panorama frames). Optional:
  most pictures are in no group.
- **Unit**: what the grid shows as one cell — a group, or a picture in no group.
- **Active**: each picture has an active member, each group an active picture. The unit's
  cell shows the active member of the active picture.
  - A new edit becomes the active member of its picture, and its picture the active one
    of its group.
  - Otherwise the original is active. The user changes it from the strip.

### Behaviour

- **Grid / filmstrip**: one cell per unit, always (no expanded stacks). Badge: member count
  (and pictures, for a group); a marker when a filter matched a member that isn't shown.
- **Filters and collections**: a unit is shown when any of its members matches / is in the
  collection.
- **Sorting**: by the active picture's original (keeps film rolls in frame order).
- **Actions** (rating, flag, label, keywords, Edit In, export, compare) apply to the photo
  shown: the active member in Grid, the member being viewed in Loupe.
- **Version strip**: a row above the filmstrip whenever the current photo's unit has more
  than one member. Members laid out by picture (dividers between pictures), each with its
  role label (Original, IR, JPEG, NegPy, Luminar, Copy 1, Export); the active one marked.
  - Click: view that member (Loupe shows it; the cover does not change).
  - Double-click or Shift+S: make it active.
  - Alt+← / Alt+→: previous / next member; arrows alone still move between units.
  - "Compare with original": Compare view, original vs the viewed member.
  - S: show / hide the strip.
- **Grouping**: Ctrl+G groups the selected units (groups merge). Ctrl+Shift+G ungroups:
  the pictures become units again; a picture's members stay together. "Detach from
  Picture" makes a member a picture of its own (role Original).
- **RAW+JPEG**: the JPEG becomes its own photo (role Pass) in the RAW's picture, with its
  own metadata. Import's "treat JPEGs next to raws as separate photos" makes it a picture
  of its own instead.

### Catalog (schema v6)

- `pictures (id, group_id → groups, group_pos, active_photo_id)`.
- `groups (id, active_picture_id)`.
- `photos`: `picture_id` (not null), `role`, `derived_from → photos`, `member_pos`.
- Drop `stacks`, `photos.stack_id / stack_pos`, `files.sidecar_of` (pair JPEGs become photos).
- Migration from v5, with tests on a v5 catalog containing pairs, virtual copies, stacks
  and IR / NegPy / Luminar names:
  - every photo gets its own picture;
  - virtual copies join their master's picture (role Copy);
  - pair JPEGs become Pass photos of the RAW's picture (keeping the photo's metadata);
  - `_IR`, `_IR_VALID`, `_NegPy`, `_LuminarNeo-edit` files join their original's picture;
  - existing stacks become groups (their top photo's picture active).

### Steps

1. Schema v6, migration and its tests.
2. Catalog API: pictures, groups, roles, active members; the joining rules in sync,
   import, add_files, Edit In, export and virtual copies; unit queries with any-member
   filters and sorting (checked against the 10k-photo performance catalog).
3. Grid and filmstrip: one cell per unit, badges; actions on the shown photo.
4. Version strip: layout, viewing, make active, Alt+arrows, compare with original.
5. Grouping commands and Detach from Picture; shortcuts, menus, `docs/SHORTCUTS.md`.
6. Remove the old stack code; real-file check with scans + IR passes + NegPy edits.

### Progress (stacks)
- [x] Schema v6 and its migration (tested on a v5 catalog with pairs, copies, stacks, IR and
      NegPy files); triggers keep pictures and groups whole when photos come and go.
- [x] Catalog: roles, joining by name at sync / import / add (`_IR`, `_IR_valid`, `_NegPy`,
      `_<n>_NegPy`, `_LuminarNeo-edit[_n]`, RAW+JPEG), active members, groups, detach,
      `unit_members`; the grid query returns one row per unit and matches any member.
      10,000 photos: 40 ms (two simple queries and the sort in Rust; the one-query version
      took 110–140 ms).
- [x] Edit In, Export (Add to Stack, now from any folder) and virtual copies join pictures;
      a new edit or copy becomes what the unit shows.
- [x] Grid / filmstrip: one cell per unit; badge with the member count (two cards for a
      group), the shown member's role when it isn't the original, a dot when a filter
      matched another member. Selection follows a unit whose shown photo changes.
- [x] Version strip above the filmstrip (S shows / hides): members picture by picture with
      their roles; click shows one (ratings etc. then go to it), double-click / Shift+S makes
      it the one shown, Alt+← / Alt+→ step, right-click: Detach from Picture, Compare with
      Original (Before / After, zoom linked).
- [x] Photo ▸ Stacking: Group Pictures (Ctrl+G), Ungroup (Ctrl+Shift+G), Show for the Stack,
      Previous / Next Member, Compare with Original, Detach from Picture, Show Version Strip.
- [x] Rating, flag and colour label belong to the stack: set on every member of the target
      stacks (all pictures of a group); a newcomer (edit, pass, copy) takes the stack's; grouping
      gives every picture the first one's. Keywords and IPTC stay per photo.
- [x] Version strip: View ▸ Show Version Strip, Photo ▸ Stacking, S, a Hide button on the strip,
      and "Stack: n ▴" in the filmstrip header while it is hidden.
- [x] Photo ▸ Delete Rejected Photos… (Ctrl+Backspace): the current source's rejected photos
      leave the catalog after asking; files stay on disk (and come back with a sync or import).
- [ ] To check by hand: the version strip and Compare with Original in the app (automated
      keystrokes were stopped while the PC was in use).

## Design: Develop module (Basic panel)

Decided with the user (2026-11-17): a Develop module like LrC's, starting with **Basic**.
Our renderer is our own: the sliders behave as an LrC user expects, but the look will not
match Adobe's exactly.

- **Controls**: Treatment (Color / B&W); WB (As Shot, Auto, Daylight, Cloudy, Shade,
  Tungsten, Fluorescent, Flash, Custom; Temp, Tint; eyedropper); Exposure, Contrast,
  Highlights, Shadows, Whites, Blacks, Auto; Texture, Clarity, Dehaze; Vibrance, Saturation.
- **Storage**: the catalog (`develop` + `develop_history`, schema v7) is the source of truth;
  settings are also written to the XMP sidecar as `photolibrary:Develop` (JSON, in the app's own namespace; LrC and
  darktable ignore it). No `crs:` fields: our numbers don't mean what Adobe's do.
- **Reach**: edited photos get re-rendered thumbnails and previews (the settings hash is part
  of the preview key) and Export applies the edits. Unedited photos keep the camera's
  embedded previews. Virtual copies have their own settings.
- **Engine** (`imaging::develop`): LibRaw demosaics to linear camera RGB with unity white
  balance; we apply WB (Kelvin/tint via Robertson isotemperature lines and the camera matrix),
  so Temp/Tint are interactive. Working space linear ProPhoto, f32, rayon by rows; an
  interactive proxy the size the photo is shown at (as LrC renders at screen size;
  reloaded larger if the window grows) and a full-size render for export and 1:1.
- **UI**: module picker Library | Develop (D, G, E, Ctrl+Alt+1/2); left Navigator + History
  (Presets, Snapshots planned); right Histogram + Basic (other panels planned); toolbar
  Before/After (\), Copy / Paste settings, Reset. Shortcuts become per module.

### Steps
1. Docs.
2. LibRaw camera-space render + colour info.
3. `imaging::develop`: settings, WB, working image, global pipeline.
4. Catalog v7, XMP namespace, settings copied to virtual copies.
5. App: module switch, Develop layout, Basic sliders, render worker, histogram,
   Before/After, Reset, Copy/Paste, History/Undo.
6. Highlights/Shadows (local), Auto Tone, Auto WB, eyedropper, clipping (J).
7. Texture, Clarity, Dehaze.
8. Previews + grid badge, Export with edits.
9. 1:1 full-resolution render in Develop.

### Progress (Develop)
- [x] 1. Docs
- [x] 2. LibRaw camera-space render (half size ~0.2 s, full ~5 s on a D750 NEF) + colour data
- [x] 3. `imaging::develop`: settings, Kelvin/tint WB, working image, global pipeline. A 2560 px
      proxy renders in ~22 ms. RAW base look tuned against the camera JPEG of a D750 sample
      from raw.pixls.us (`testdata/web/`); our own RAW samples are film scans.
- [x] 4. Catalog v7 (`develop`, `develop_history` with an "Import" first step, revert to a step),
      `photolibrary:Develop` in sidecars (read at sync / Read Metadata; never cleared by a sidecar
      without it, since other apps may drop our property; left out of exports), copies inherit
      the settings of the photo they are made from.
- [x] 5. Develop module: module picker (D, G/E/C/N, Ctrl+Alt+1/2), render thread (latest
      settings win while dragging), Basic panel with LrC-style sliders (double-click resets;
      Temp/Tint back to As Shot), WB presets and Selector (W), Treatment, histogram, History
      (click to go back; Ctrl+Z / Ctrl+Y), Before / After (\), Copy / Paste, Reset, Settings
      menu. Shortcuts are per module. Highlights, Shadows, Texture, Clarity, Dehaze and Auto
      are shown disabled until steps 6-7. Checked in the app with the D750 sample.
- [x] 6. Highlights / Shadows from an edge-preserving brightness map (self-guided filter at
      ~384 px, gain per map cell; ~40 ms per proxy render with them on), Auto Tone (Ctrl+U,
      the Auto button) from luminance percentiles, clipping overlay (J; red / blue) and the
      histogram's clipping triangles (click to toggle). Auto WB and the WB Selector came in step 5.
- [x] 7. Texture (fine detail against a small blur, radius ∝ image size, noise threshold),
      Clarity (mid-scale detail against a second edge-preserving map, mostly midtones),
      Dehaze (dark channel prior: haze map at ~256 px and airlight; negative adds haze).
      Speed: local adjustments are computed into one gain-per-pixel buffer before the main
      pass, with fast log2/exp2; and the proxy now follows the view size instead of a fixed
      2560 px. At a 1280 px view: plain render ~7 ms, all local effects at once ~50 ms.
- [x] 8. Edited photos' thumbnails, standard and 1:1 previews are rendered with their
      settings, stored under the photo's negated id with the settings hash in the fingerprint
      (an edited master and its unedited virtual copy each keep theirs; a reset finds the plain
      previews still there); the orphan purge knows both. After each saved change the photo's
      thumbnail and preview reload (the old ones stay up until then). ± badge in Grid cells.
      Export develops the photo at full size (16-bit through the colour conversion, also for
      8-bit files). Checked in the app: B&W + Auto showed in Grid, filmstrip and Navigator.
- [x] 9. 1:1 in Develop: Z / click zooms (Navigator levels, drag / scroll to pan, the
      Library's Navigator with the visible area). The enlarged proxy shows at once; once the
      settings have been still for 200 ms, the visible region is rendered from the full-size
      image (loaded on its own thread the first time: ~4.4 s for the D750 NEF; then ~10 ms per
      view). Regions render exactly like that part of a full render (the local maps come from
      the whole image; Texture uses the region plus a margin); tested.

All nine steps of the Basic panel plan are done.

### Fixes after the Basic panel (2026-11-17)
- [x] Symbol shortcuts typed with AltGr (Ctrl+Alt on Windows) match their plain binding
      (`\` on Nordic layouts). Injected key events can't reproduce AltGr; to confirm by hand.
- [x] "Building Previews" counted all background jobs (reading GPS too): only preview builds
      count now, and "done" never passes the total.
- [x] Develop settings changed by a sidecar (Read Metadata, sync) reload the photo's previews,
      and the Develop module if it shows that photo.

## Develop: Tone Curve
- [x] Parametric curve (Highlights, Lights, Darks, Shadows; three split points to drag under
      the graph, double-click resets one) and point curves (RGB, Red, Green, Blue: click to
      add, drag, drag off or double-click to remove, right-click to reset; Linear / Medium /
      Strong Contrast presets). Monotone cubic splines (no overshoot). Applied after the
      Basic tone on perceptual values, folded into the per-channel tone tables (no per-pixel
      cost). Stored in the settings JSON (`curve`); older settings load with a neutral curve.

## Develop: HSL / Color
- [x] Hue, Saturation, Luminance for eight ranges (Red, Orange, Yellow, Green, Aqua, Blue,
      Purple, Magenta) on Hue / Saturation / Luminance / All tabs, with tinted slider tracks.
      Worked in OkLCh (perceptual): ranges centred on reference colours' hues and eased between
      neighbours; hue +100 goes three quarters of the way to the next range; neutrals are left
      alone. In Black & White the panel is the B&W Mix (how light each colour turns). About
      25 ms per proxy render when on (Oklab per pixel); nothing when everything is at 0.
- [x] Fix found while checking it: a render requested before the photo's load reached the
      worker in another batch was dropped, leaving "Loading…" forever (timing-dependent,
      since step 5). Requests now wait for their photo.

## Develop: Color Grading
- [x] Shadows, Midtones, Highlights and Global wheels (hue 0–360 on the colour wheel,
      saturation 0–100, luminance ±100), Blending (how far the ranges overlap) and Balance
      (moves them towards shadows or highlights). 3-Way view (Midtones above, Shadows and
      Highlights below, Luminance under each) or one wheel with Hue / Saturation / Luminance
      sliders. Drag a wheel's handle; double-click resets. Tints are added in Oklab after HSL
      and after B&W, so it split-tones black & white photos. ~23 ms per proxy render when on.

## Library previews of RAWs match Develop (2026-11-18)
- [x] Unedited RAWs' thumbnails, standard and 1:1 previews (and exports) are rendered by the
      Develop engine at default settings, as LrC's Standard previews are, instead of the
      camera's embedded JPEG, so a photo looks the same in Library and Develop. Decided with
      the user. The embedded JPEG shows as a placeholder while the rendered preview is built
      (~0.3–0.5 s per RAW instead of ~0.05 s). Existing caches rebuild once (new key); old
      embedded previews are removed as their replacements are written.

## Develop: Detail
- [x] Sharpening (Amount, Radius, Detail, Masking) and Noise Reduction (Luminance with Detail
      and Contrast; Color with Detail and Smoothness), done last on the rendered image's Oklab
      lightness and colour. Sharpening is an unsharp mask on L with soft halo limiting (Detail)
      and an edge mask (Masking); luminance NR is an edge-preserving guided filter on L;
      colour NR smooths a/b guided by L, computed on 2–4 px blocks and applied at full
      resolution (colour noise is coarse). Radii are in full-size pixels, so a fit view shows
      a scaled-down effect and 1:1 is exact, as in LrC; 1:1 regions render with a margin on
      the block grid and match the whole image exactly. Defaults per file type, as in LrC:
      RAWs get Sharpening 40 and Color NR 25, rendered files nothing; Reset uses them, and
      unedited RAWs' previews are rendered with them. Proxy render (1280 px) with the RAW
      defaults ~20–30 ms, everything on ~35–45 ms. The develop example now uses the app's
      allocator (the system one made big buffers page-fault and doubled the timings).

## Develop: Effects
- [x] Post-Crop Vignetting: Style (Highlight Priority: an exposure change before the tone
      curve, so highlights roll off; Color Priority: one gain per pixel after the tone
      curve, keeping hues; Paint Overlay: blended towards black or white), Amount, Midpoint,
      Roundness (−100 follows the frame's rectangle, 0 its ellipse, +100 a circle), Feather
      and Highlights (keeps bright areas bright when darkening). No crop exists yet, so it
      follows the whole frame. The shape's per-column and per-row terms are precomputed.
- [x] Grain: Amount, Size (grains 1–5 full-size pixels), Roughness (more of a finer,
      irregular layer). Hashed value noise in image coordinates, so 1:1 regions and exports
      match; grains smaller than a rendered pixel average out (weaker, finer grain in a fit
      view, as in LrC). Applied last, after Detail, so noise reduction doesn't remove it and
      sharpening doesn't sharpen it; strongest in the midtones. Auto Tone ignores Effects.
- [x] Glow (as in Lightroom 2027 / Camera Raw): Amount, Range (only the brightest
      highlights .. most light tones), Spread (0.5–8% of the long side), Warmth. The
      developed photo's bright parts on a ~384-cell map of the whole photo (without the
      region-only local gains and masks, so every region agrees), blurred, tinted, and
      screened over after Color Grading. About 25 ms on a proxy render.

## Develop: Detail preview box and sharpening previews
- [x] The Detail panel's preview box, as in LrC: the photo at 1:1 around a point (the
      centre at first), rendered from the full-size image; drag inside it to look around;
      the target button, then hovering over the photo, moves the point (a click keeps it);
      the triangle hides the box. Renders through the same full-resolution region path as
      the zoomed view (a second region target), following a slider about 60 ms after it
      pauses.
- [x] Holding Alt (as in LrC) or Shift while dragging a Sharpening slider shows its
      greyscale preview in the view and the box: Amount the sharpened image, Radius and
      Detail the edges being enhanced on grey, Masking the mask (white where sharpening
      applies). The histogram keeps the photo's.

## Develop: Transform
- [x] Stage 1, Manual Transform: Vertical and Horizontal (keystone: the photo is turned
      as if by a camera with a ~35 mm lens, centre kept in place; Vertical below 0 widens
      the top, as in LrC), Rotate (±10°), Aspect, Scale (50–150%), X / Y Offset. A grid
      shows over the photo while a slider is dragged. The transformed image is made first
      (bilinear) and cached with the source until the Transform changes, so everything
      after it (local adjustments, Detail, Effects, 1:1 regions, the Detail box, previews,
      export) works unchanged; uncovered parts render white, as LrC shows them before a
      crop. The White Balance Selector samples through the Transform. ~7–11 ms to warp a
      1280 px proxy.
- [x] Stage 2, Upright Level / Vertical / Full / Auto: straight edges are found (edge pixels
      grown into regions of one direction, long thin ones kept as segments, on a ~900 px
      copy), then the camera turn (pitch, yaw, roll) is fitted by robust least squares so
      near-verticals come out vertical and near-horizontals level. Level fits the roll only,
      Vertical adds pitch, Full adds yaw, Auto weighs horizontals less and resists big turns.
      A turn needs two lines of its kind; a result that straightens under 30% of the lines
      (whiskers, branches) falls back to levelling, then to nothing. The angles are stored in
      the settings, so renders never analyse again; the Manual sliders apply on top.
      ~100–200 ms per analysis.
- [x] Stage 3, Upright Guided: drag on the photo to draw up to four guides (kept in the
      untransformed photo's coordinates), drag an end to move one, right-click to remove;
      two verticals fix pitch and roll, two horizontals yaw, one guide only levels; solved
      exactly on the UI thread.
- [x] Constrain Crop: the output becomes the largest rectangle of the photo's shape inside
      both the frame and the transformed photo (searched over centres), so no white is left.
      The crop is part of the Transform's mapping: the developed photo is really smaller
      (previews, 1:1, export), and the Loupe and Develop size it as developed. A crop tool
      can build on it.
- [x] Fix found on the way: sliders with one decimal (Detail's Radius) rounded to whole
      numbers; they now step by their own precision.

## Design: Masking (Brush, Linear Gradient, Radial Gradient)
Local adjustments as in LrC's Masks; AI masks (sky, subject from a segmentation model) come
later as another component kind.
- A mask has components combined in order (the first adds; then Add / Subtract /
  Intersect, each invertible), an Amount, visibility, and LrC's local sliders (Temp, Tint,
  Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Texture, Clarity, Dehaze, Hue,
  Saturation, Sharpness, Noise, Color). Stored in the settings JSON (`masks`); shapes in the
  untransformed photo's coordinates, so masks follow Transform and Constrain Crop.
- Rendering: per render, every active mask's value per output pixel (output → source
  through the Transform's mapping): linear and radial analytically, brushes as dabs along
  each stroke rasterized in source pixels (cached with the source image by strokes and
  area). The local sliders act inside the per-pixel pipeline at the matching step.
- UI (decided with the user): a floating Masks panel over the photo as in LrC; the right
  panel shows the selected mask's sliders while masking. Shift+W, K, M, Shift+M, O, [ ].

### Steps
1. [x] Engine: masks, components, brush rasterizing, mask values per pixel; local Temp,
   Tint, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Hue, Saturation; the
   overlay image; History labels ("Add Brush Mask", "Mask 1: Exposure +0.50").
2. [x] Masking mode: floating Masks panel, Linear and Radial tools with pins and handles,
   local sliders, overlay (O), shortcuts. The overlay is the mask rendered on the worker
   from the proxy (red, half strength), asked for alongside renders. Esc drops the tool,
   then leaves masking; Delete removes the selected component.
3. [x] Brush: K, paint (a stroke per drag, live), erase (Alt or the Erase box), Size /
   Feather / Flow / Density, a two-circle cursor, [ ] for size and { } (Shift+[ ]) for
   feather; a click paints one dab, and a click with the radial tool makes a default one.
   A Shift-typed symbol now also matches its plain binding (US Shift+[ arrives as {).
   UI pass (2026-11-18): the Masks panel and the masking tool are painted like the rest of
   the app (tool glyphs, rows, an on/off switch per mask instead of an eye, pill buttons,
   a + menu with keys), the right panel is a section with a header, the brush sliders
   use the standard slider rows, outlines stay on the photo, pins are rings, and Esc
   with a menu open only closes the menu.
4. [x] Texture, Clarity, Dehaze (per-pixel amounts: the global value plus each mask's
   times its value, through the same maps; the haze map keeps the raw dark channel so
   any strength applies per pixel). Sharpness (unsharp mask, negative blurs) and Noise
   (edge-preserving smoothing) on the lightness after Detail, per pixel; masks using
   them take the Detail path. Color: a swatch with Color Grading's wheel, an Oklab tint
   (eased off in deep shadows). Thumbnails (each mask white on black, from a 96 px copy
   of the photo in the worker), Rename (double-click or the menu) and Duplicate
   ("Mask 1 Copy"). An Edit button left of the tools goes back to the panels.
5. [ ] Auto Mask (edge-aware brush).

## Develop: Crop & Straighten (R)
- [x] The crop is part of the Transform's mapping (`transform.crop`: a rectangle as 0..1 of
      the frame the Transform shows, after Constrain Crop, and an angle turning the photo
      clockwise about the frame's centre), so previews, 1:1, export, masks, the White
      Balance Selector and Upright guides all follow it. Old settings load uncropped.
- [x] The crop tool (R, or the crop icon in the tool strip) shows the whole turned photo,
      dims what is cropped away, draws thirds, corner and edge handles; drag an edge or
      corner to resize (Shift-free lock: the Aspect lock keeps the shape), inside to move,
      outside to turn the photo. Aspect presets (Original, 1 x 1, 4 x 5, 8.5 x 11, 5 x 7,
      2 x 3, 4 x 3, 16 x 9, 16 x 10, Custom) follow the crop's orientation; X swaps
      portrait / landscape. Angle slider (±45°), the Straighten tool (draw a line that should
      be level or plumb), Auto (levels from the photo's lines, as Upright Level does, in the
      frame the Transform shows). Constrain to Image keeps the crop on the photo (no white
      corners) while turning, resizing and moving. Enter or R closes it, Esc puts the
      Straighten tool away and then closes; Ctrl+Alt+R resets the crop.

## Develop: Remove (Q): Heal and Clone
- [x] LrC's spot removal without the AI mode (Content-Aware Remove comes with the
      segmentation work). A click makes a round spot, a drag brushes one; each takes its
      pixels from the same shape moved by an offset. The source is found automatically
      nearby: candidates around the spot are scored by how well the ring around them
      matches the ring around the spot (inside the photo, nearer preferred); / picks
      another. Clone copies the source; Heal multiplies it by the ratio of the
      destination's and the source's local average colour, both measured on the ring
      only (a normalised convolution, so the blemish never counts), which matches it to
      the surroundings. Spots are in the untransformed photo's coordinates and applied to
      its own pixels first (cached until they change), so Transform, crop and masks follow.
- [x] The tool: outlines for all spots, the selected one with its source and an arrow;
      drag a spot (its source stays) or its source to move it, a round spot's edge to
      resize; the wheel and [ ] size the selected spot (or new ones); Heal / Clone, Size,
      Feather, Opacity for the selected spot or new ones; New Source, Delete, Reset; Q,
      Esc, Delete, / keys. In the tool strip between Crop and Masking.
- [x] Visualize Spots (A, or the toolbar checkbox while the Remove tool is open): what is
      shown, as its edges in white on black (the difference from a 1.5-pixel blur, scaled by
      the threshold slider), so dust and blemishes stand out; the spot outlines stay on top.
- [x] Content-Aware Remove (AI): see "Design: Remove (content-aware and generative)".

- [x] UI pass (2026-11-19): a spot's outline is traced from its area (marching squares on
      the signed distance to its strokes, cached per shape and relative to its first
      point, so moving is free): one clean line however a stroke went, instead of offset
      polylines that crossed. Clicks within a few points of an outline hit it (thin
      strokes). A spot's area can be edited: Area Add / Subtract in the panel (or
      Shift+drag / Ctrl+drag) paints strokes into the selected spot (`Spot::strokes`,
      `erase` for subtracting); Remove spots fill again. Heal / Clone sources are found
      at distances by the whole spot's size (a long stroke's source used to land on the
      stroke itself); the help text says where the source outline is and that `/` picks
      another.

## Design: AI masks (Subject, Sky, Background, Objects)
Decided with the user (2026-11-18): PhotoLibrary will never be a commercial product, so
non-commercial model weights are fine; DirectML from the start; ~400–500 MB of models
downloaded on first use is acceptable; benchmark before choosing.

### Benchmark (2026-11-18, `cargo run -p segment --example segbench --release`)
i7-1260P laptop (4 P + 8 E cores, Iris Xe), photos at 1600 px. DirectML's masks were
identical to the CPU's for every model that ran.

| Use | Model (ONNX) | DirectML | CPU | Notes |
|---|---|---|---|---|
| Subject / Background | **RMBG-1.4** (BRIA, 176 MB, non-commercial) | **0.6 s** | 5 s | good; a little generous (sleeve, sign, person) |
| | BiRefNet-lite (MIT, 224 MB, fixed 1024 input) | out of memory / 10–19 s | 24 s | best quality; needs a GPU with its own memory |
| Sky (later Landscape) | **SegFormer-B2 ADE20K** (110 MB, non-commercial) | **0.4 s** | 3 s | clean sky, through trees |
| | SegFormer-B0 (15 MB) | 0.4 s | 0.25–0.7 s | the CPU-only fallback |
| Objects | **SAM 2.1-tiny** (Apache-2.0, 149 MB) | **encoder 0.54 s, click 0.21 s** | 5–10 s, 0.3–1.2 s | best for quality |
| | SlimSAM-77 (Apache-2.0, 40 MB) | encoder 0.73 s, click 0.08 s | 5.5 s, 0.3 s | fast enough for live hover |

Findings:
- ONNX Runtime is loaded at run time (`ort` 2.0.0-rc.15, `load-dynamic`): Microsoft's
  DirectML build (onnxruntime 1.24.4, ~17 MB) and DirectML.dll (1.15.4, ~18 MB) ship next
  to the app. pyke's static build needs a newer MSVC than ours and isn't needed.
- ONNX Runtime's default of a thread per logical core is ~5× too slow on hybrid laptop
  chips (fast + slow cores): half the logical cores, at most 8.
- Models must see the developed photo (what the user sees), not raw data: a film
  negative is seen inverted otherwise.
- Expect 2–4× faster than Iris Xe on Core Ultra (Arc) / Ryzen 8840HS (780M) laptops;
  BiRefNet should fit on a P16s with an RTX 500 Ada.
- Linux / macOS: the same models; acceleration via CUDA / OpenVINO / ROCm (Linux) or
  CoreML (macOS), CPU otherwise; the runtime library name differs per OS.

### Plan
1. [x] Groundwork: model manager (download on first use with consent, SHA-256 check,
   stored in the user's local app data), the runtime downloaded from Microsoft's NuGet
   packages on Windows (or taken from next to the app), device choice (DirectML with CPU
   fallback; "Use GPU for AI" in the Masks panel, remembered), a thread per job with
   progress in the Masks panel.
2. [x] Raster mask component (`Shape::Raster`): the model's mask stored as a PNG in the
   catalog's `masks` folder with what made it (`selects`, `model`), in the untransformed
   photo's file orientation; edges refined against the photo at render size with the
   guided filter (Detail's; radius four picture pixels), cached like brush rasters, and
   regions still render exactly like the whole; Add / Subtract / Intersect / Invert as
   other components. A missing PNG selects nothing.
3. [x] Select Subject / Sky / Background in the Masks panel's Create, Add, Subtract and
   Intersect menus (Background is the subject's picture inverted). The model sees the
   photo developed but without geometry, effects or masks, turned upright. On the
   i7-1260P (debug build) the first Subject takes ~7 s with DirectML's session setup,
   later ones ~3 s.
4. [x] Objects (SAM 2.1-tiny): Object in the Masks panel's menus, a tool with LrC's two
   modes: Brush (points along the stroke; a stroke's box would confine the object to
   the brush's width) or Rectangle (a box), and a click for one point. The encoder runs
   on the GPU when it can, the decoder always on the CPU (its inputs change shape with
   the prompt, which made DirectML rebuild it every time and once reset the GPU). The
   photo's encoding is kept, so more objects on it take ~0.5 s. Masks are cut to the box
   (with a margin). On battery the Iris Xe's encoder was erratic (3–8 s against 3.3 s on
   the CPU); "Use GPU for AI" lets the user choose.
5. [ ] Later: Landscape classes (SegFormer), BiRefNet as a high-quality option on capable
   GPUs, Content-Aware Remove (LaMa inpainting), People.

## Design: Remove (content-aware and generative)
LrC's Remove tool also has **Remove** (content-aware fill) with a "Use Generative AI"
option (3 variations). Local models, downloaded on first use like the AI masks.

### Benchmark (2026-11-19, `cargo run -p segment --example inpaintbench --release`)
ThinkPad P16s Gen 1 (i7-1260P, Intel UHD + **NVIDIA T550 4 GB**). DirectML adapter 0 is
the Intel GPU, 1 the T550 (`PL_DML_DEVICE`; until now every DirectML run, the masks'
included, used the Intel GPU). Five removals (a person, a sticker, flags against the sky,
a cat's eye, a whole ball), cut as the app will: a square around the area, at 512².

| Model | Size | CPU | DirectML (Intel / T550) | Quality |
|---|---|---|---|---|
| LaMa fp32 / int8 (Carve export) | 208 / 37 MB | 11–15 s | fails (a 5-D MatMul) | best |
| LaMa int8 "r4" (4-D rewrite) | 37 MB | 6–14 s | runs (0.7 s) but returns broken pixels | best (on the CPU) |
| **LaMa-Dilated** (Qualcomm export) | 183 MB | 4 s | **0.37 s** (load 4 s) | good on small areas; smudges on large ones |
| MI-GAN 512 | 28 MB | 1.3–1.8 s | 0.16 s | weaker: ghosts on large areas, specks |
| SD 1.5 inpainting fp16 (UNet + VAE) | 1.9 GB | 30 s/step (cfg 5) | 15–22 / 6.6 s a step (cfg 5); 3.5 s (cfg 1) | at 12 steps without guidance: paints new figures, a grey disc on the dark cat; flags fine |

- Content-aware: first LaMa r4, but on DirectML it returns broken pixels (found in the
  app); now LaMa-Dilated on the GPU, big-LaMa on the CPU as "Best quality" (see below).
- Generative: SD 1.5 needs ~20 guided steps for usable fills: ~2–2.5 min per variation
  on the T550, using 3.6 of its 4 GB (one device-removed error after four runs). Not
  practical as is; see the decision below.
- Prompts: CLIP embeddings made once ("background, seamless, empty, natural, photo" /
  "object, person, text, logo, blurry"), in `sd_prompts.bin`.
- **Decision (2026-11-19):** content-aware Remove with LaMa, one result; no
  generative and no variations for now (MAT, which gives variations per seed, has no
  ONNX release). The SD pipeline (`segment::sd`) and the benchmark stay for later.

### Plan
1. [x] Benchmark (above).
2. [x] GPU choice: DirectML on the adapter with the most dedicated memory (DXGI; the
   T550 here), for the masks too (`segment::gpu`; `PL_DML_DEVICE` overrides). The mask
   models are no faster on the T550 than on the Intel GPU (overhead-bound); heavy models
   are.
3. [x] Engine: `SpotMode::Remove` with `Spot::fill` (`Fill`: picture id, area, colour
   gains, the spot shape it was made for, model). The picture (PNG in the catalog's
   `masks` folder) is in the colours the model saw, `T(p) = sRGB(p · gains)` (white
   balance, the crop's 99th percentile to 0.9), and goes back through T⁻¹ onto the
   photo's own pixels, feathered, at the spot's opacity. A moved or resized spot's fill
   is ignored until it is made again.
4. [x] In the app: Remove first among Remove / Heal / Clone (and the default). A new or
   changed Remove spot is filled on its own, one at a time: the full-size photo (loaded as
   for 1:1) around the spot → 512² → the model → back (at most 1024 px) → saved, a
   History step "Remove". Spinner on the spot, progress in the panel, consent before the
   first download, the model loaded when the tool opens.
   - Found in testing: the int8 big-LaMa "runs" on DirectML but returns broken (black)
     pixels on both GPUs. So the default is **LaMa-Dilated** (183 MB, BSD-3, Qualcomm AI
     Hub's export: dilated convolutions instead of Fourier units): 0.37 s on DirectML,
     loads in 2–4 s. "Best quality (slower)" uses **big-LaMa on the CPU** (37 MB): ~5 s
     a fill, ~13 s to load the first time.
   - On a grainy film scan (a child in the sea), LaMa-Dilated left a pale blur; big-LaMa
     continued the water cleanly. LaMa-Dilated is fine on small spots; consider making
     best quality the default.
5. [x] Big-LaMa on the GPU (2026-11-19). Comparing every intermediate tensor between
   DirectML and the CPU (Python `onnx` + `onnxruntime-directml` 1.24.4, the app's version)
   showed the network itself is fine on DirectML: only its int4/int8 weights
   (`DequantizeLinear` → `Cast`, node 1 onwards) decode wrongly, so the hole came out ~0.
   Fix: decode them once on the CPU (ONNX Runtime constant folding at the basic level with
   `session.disable_quant_qdq`, saved as `lama-r4.folded.onnx`, 206 MB; the download stays
   37 MB) and run that on DirectML. Identical to the CPU's output; on the T550 **0.58 s a
   fill** (CPU 5–12 s). First use ~30 s (decoding ~15 s once, then loading 15–25 s each
   session, started when the Remove tool opens).
   - With this, big-LaMa is as fast per fill as LaMa-Dilated and clearly better: worth
     making "Best quality" the default. Done: "Best quality" (big-LaMa) is on by default;
     unticked uses LaMa-Dilated (smaller to load, for machines without a good GPU).

## Design: Remove → Distractions (Sensor Dust, Dust & Scratches)

LrC's Distractions has Dust (sensor dust). Film scans need more, so two parts:

- **Sensor Dust** (digital captures): Find turns soft, round, slightly darker blobs in
  smooth areas into Heal spots marked `found` (differences of blurs at five scales in log
  lightness at ~1500 px, a local minimum, a smooth ring around it, no fine detail around
  it, not in deep shadow). Each spot gets a source from `find_source`. Find again replaces
  the found spots; moving, resizing, painting or changing one makes it the user's own.
  Clear Found deletes the found ones. On the sample digital captures it finds nothing
  false; on film scans it flags some soft round picture details (it is not meant for them).
- **Dust & Scratches** (film scans): one layer (`DevelopSettings.dust`) with Find in IR /
  Picture / Both, Sensitivity and Max Size. The found mask is kept as a picture in the
  catalog's mask store (made on a thread from the full-size photo, ~2–8 s for 20–24 MP)
  and filled at render time by a push-pull pyramid before the spots. A History step
  "Dust & Scratches" when the mask arrives. Show found draws it in red.
  - **IR**: the infrared pass (`<name>_IR`, with NegPy's `<name>_IR_valid`, found in the
    photo's unit) lightly smoothed, its dips under the local background. Scanners see a
    little of the dyes in IR (about a tenth of the picture's own dips on the test scan:
    dense picture detail passed for dust): that share is measured per scan (the median
    ratio where the picture departs strongly) and taken away. Not for silver B&W film.
  - **Picture**: black-hat and white-hat (closing/opening, van Herk min/max) of the lightly
    smoothed lightness against the local usual level (clipped, so a hair doesn't raise its
    own), hysteresis from clear seeds into fainter rest, thin parts only, long ones only
    when strong and not straight (frames, edges and the film border are straight; hairs
    curl, so straight scratches are left to spots). Plus a **hair finder** for heavy grain:
    a 5 px black/white-hat averaged along short lines in 8 directions at half size, its
    score against the local spread, hysteresis along the hair, long, thin and curling.
    Finds the faint hairs on the B&W test scan while leaving the soft drawn outlines.
  - **Both**: what IR finds, plus picture finds the IR confirms (its dip averaged over the
    find); adds the small specks IR only hints at, never wires or edges. The default with
    an IR pass.
- Not done: Reflections and People (LrC's other Distractions).

### Picture-only Dust & Scratches, measured (2026-11-20)

Measured against **FILM-AA** (Ivanova et al. 2023, CC BY 4.0: ten blank 35 mm frames from a
Plustek 7600i with 12,135 hand-marked defects). Also used: composites of its defects on a
digital photo, clean digital photos (everything found is false), and the user's SilverFast
Kodak Gold scans. The colour scan's IR pass served as a reference for real dust.

| Picture detector | FILM-AA recall | SilverFast scan: flagged px/MP | 17–24 MP |
|---|---|---|---|
| Before (dark and light) | 42–67% | 4,373 | ~2 s |
| Classroom top-hat pipeline (the linked repo) | 26% | 18,929 (blank film) | 0.7 s |
| **Now: Light only, recalibrated** | **77–91%** | 3,769 | **1.3–2.5 s** |
| **AI: Microsoft's scratch U-Net at ½ scale + Light** | **85%** | **328** | 45–55 s (T550) |

- **Polarity.** On scans of negatives, dust comes out light: 89% of finds on FILM-AA, 86%
  of IR-confirmed dust on the colour scan. The false finds on the Kodak Gold scan were 68%
  dark. **Defects: Light / Dark / Both**, Light by default; Dark is for slides. Looking one
  way needs stricter factors (the usual level halves).
- **Tried and dropped:**
  - a "carries on past the find" test: little gain, and it cost scratches;
  - colour and edge sharpness: no separation in the IR-checked data.
- **Use AI (slower, cleaner)**, for Picture: Microsoft's U-Net, "Bringing Old Photos Back
  to Life" (MIT), a 151 MB download with consent.
  - It runs once a photo: grey, ½ scale, 512 px tiles. Its map is kept (`ai_map`), so
    Sensitivity, Max Size and Defects only threshold it.
  - At ⅓ scale it takes half the time, but recall drops to 77%.
  - It takes about 1.8 s per 512² tile on the T550 (6–8 s on the CPU). No faster path
    found.
  - The user chose it as an option, not the default.
- **Sensor Dust's Find found nothing.**
  - Its thresholds needed blobs much darker than sensor dust (0/40 synthetic 2–6% blobs,
    even at 100).
  - Now relative to the local noise: 19/40 at the default with no false finds on clean
    digital photos, 27/40 at 70.
  - The panel says what it found (or that it found nothing); the History step is
    "Sensor Dust: n spots".
- **Speed.** Masks are saved with fast PNG compression. Each stage of the dust job is
  logged (read, found, saved; and the full-size load).

## Design: Remove → Distractions → People (2026-11-20)

LrC finds the people in a photo and removes them. Ours: **Find**, then pick on the photo,
then **Remove (n)**.
- Found people are outlined. Orange ones will be removed; finds below 0.5 confidence start
  faint and unpicked. A click switches one. All / None.
- Remove makes each picked person a **Remove spot whose area is their outline** (a picture
  in the mask store). Area Add/Subtract, moving and deleting work as for other spots.
  Big-LaMa fills them one by one.

How it finds them:
- **Detector:** RT-DETR R18 (PekingU, Apache-2.0, COCO), onnx-community's int8 export
  (22 MB). It runs on the whole photo and on overlapping 1280 px tiles, so small, distant
  people (20–40 px on a 4600 px scan) are found too.
  - Boxes cut by a tile's inner edge are dropped (another tile holds them whole), the rest
    merged.
  - Belongings (bag, backpack, umbrella, suitcase, ball, kite, skateboard, surfboard) go
    with the person their box overlaps most (≥30%).
  - About 13–20 s for 16–21 views on the T550 (fp32 about the same; int8 is the smaller
    download).
- **Outlines:** SAM 2.1 tiny with the box, on a crop three times its size (at least
  256 px), nearby people sharing a crop up to 1024 px.
  - On the **CPU**: its image encoder takes 2.4 s there against 12 s on DirectML on the
    T550.
  - Lifeguard photo: 12 found and outlined in under a minute.
- **Fills:** a person's crop gets less room around it (0.75 radius, not 1.5), keeping
  detail at the model's 512 px.
  - Fills enlarged more than 1.25× get **grain** matched to the spot's surroundings (the
    ring's fine-detail spread, per channel, by how much detail the enlargement lost).
  - Without it, a large person's fill (2055 px crop at 512) looked smooth on grainy film.

Seen in testing:
- floats and buoys in the water can come up as low-confidence "people" (unpicked by
  default);
- a tower base came up as a "bench" (benches are not kept).

Findings the app may want:
- SAM's encoder is 5× faster on the CPU than on the T550, so Select Object's GPU default
  may be the slower choice there.
