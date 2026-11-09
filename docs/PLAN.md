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
- [ ] Later: export presets (user-named), 16-bit TIFF, colour spaces other than sRGB, output
      sharpening, watermarks, "Limit File Size To", GPS / Remove Location Info (needs GPS
      import first), Export as Catalog.
