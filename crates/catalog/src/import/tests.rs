use super::*;
use crate::{PhotoMetadata, Source};
use std::fs;

/// 2026-11-08 14:05:09 as naive-local seconds.
const T: i64 = 1_794_146_709;

#[test]
fn calendar_dates() {
    let d = DateParts::from_timestamp(T);
    assert_eq!((d.year, d.month, d.day, d.hour, d.minute, d.second), (2026, 11, 8, 14, 5, 9));
    let leap = DateParts::from_timestamp(951_782_400); // 2000-02-29
    assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
    let before = DateParts::from_timestamp(-1);
    assert_eq!((before.year, before.month, before.day, before.hour), (1969, 12, 31, 23));
}

#[test]
fn templates() {
    let r = |t: &str| render_template(t, "_JMB4583", "Hegra", 7, T);
    assert_eq!(r("{YYYY}{MM}{DD}-{Filename}").unwrap(), "20261108-_JMB4583");
    assert_eq!(r("{Text}-{Seq:3}").unwrap(), "Hegra-007");
    assert_eq!(r("{Text} {OrigNum} {YY}{hh}{mm}{ss}").unwrap(), "Hegra 4583 26140509");
    assert_eq!(r("a/b:c?").unwrap(), "a_b_c_", "unsafe characters are replaced");
    assert!(r("{Nope}").is_err());
    assert!(r("{Seq").is_err());
    assert!(r("{Text}").map(|_| ()).is_ok());
    assert!(render_template("{Text}", "x", "  ", 1, T).is_err(), "empty names are refused");
}

fn candidate(path: &Path, time: i64) -> Candidate {
    Candidate {
        path: path.to_path_buf(),
        kind: file_kind(path).unwrap(),
        size: 1,
        capture_time: Some(time),
        duplicate: false,
    }
}

#[test]
fn plans_dated_renamed_destinations_and_keeps_pairs_together() {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("DCIM");
    let dest = tmp.path().join("Photos");
    fs::create_dir_all(dest.join("2026").join("2026-11-08")).unwrap();
    // Already there: the first pair must move on to "-2" as a pair.
    fs::write(dest.join("2026/2026-11-08/Trip-001.NEF"), b"old").unwrap();
    let files = [
        candidate(&card.join("DSC_1.NEF"), T),
        candidate(&card.join("DSC_1.JPG"), T),
        candidate(&card.join("DSC_2.NEF"), T + 60),
    ];
    let settings = ImportSettings {
        destination: dest.clone(),
        rename: Some("{Text}-{Seq:3}".into()),
        custom_text: "Trip".into(),
        ..Default::default()
    };
    let plan = plan(&files, &settings).unwrap();
    let day = dest.join("2026").join("2026-11-08");
    let dests: Vec<PathBuf> = plan.iter().map(|p| p.dest.clone()).collect();
    assert!(dests.contains(&day.join("Trip-001-2.NEF")), "{dests:?}");
    assert!(dests.contains(&day.join("Trip-001-2.JPG")), "the JPEG follows its raw: {dests:?}");
    assert!(dests.contains(&day.join("Trip-002.NEF")), "{dests:?}");

    let flat =
        ImportSettings { date_folders: DateFolders::None, subfolder: Some("Card 1".into()), ..settings };
    assert!(plan_ok(&files, &flat).iter().all(|p| p.dest.parent() == Some(&dest.join("Card 1"))));
    let bad = ImportSettings { subfolder: Some("a/b".into()), ..flat };
    assert!(super::plan(&files, &bad).is_err());
}

fn plan_ok(files: &[Candidate], s: &ImportSettings) -> Vec<Planned> {
    plan(files, s).unwrap()
}

/// A catalog, a "card" with photos, and a destination folder.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, Catalog) {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("CARD").join("DCIM").join("100NIKON");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("DSC_1.NEF"), b"raw-one").unwrap();
    fs::write(card.join("DSC_1.JPG"), b"jpeg-one").unwrap();
    fs::write(card.join("DSC_1.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"4\"/></rdf:RDF></x:xmpmeta>").unwrap();
    fs::write(card.join("DSC_2.NEF"), b"raw-two!").unwrap();
    let dest = tmp.path().join("Photos");
    fs::create_dir(&dest).unwrap();
    let cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
    (tmp, card, dest, cat)
}

/// Every file dated T (the reader stands in for EXIF).
fn reader() -> impl Fn(&Path, FileKind) -> Option<PhotoMetadata> + Sync {
    |_: &Path, _: FileKind| Some(PhotoMetadata { capture_time: Some(T), ..Default::default() })
}

#[test]
fn copy_import_into_dated_folders() {
    let (tmp, card, dest, mut cat) = setup();
    let read = reader();
    // An older photo of the same destination folder, already in the catalog.
    fs::create_dir_all(dest.join("old")).unwrap();
    fs::write(dest.join("old/a.jpg"), b"a").unwrap();
    cat.sync_folder_opts(&dest, SyncOptions { imported_at: Some(1), ..Default::default() }).unwrap();

    let found =
        scan_source(tmp.path().join("CARD").as_path(), true, Some(&read), &cat.duplicate_keys().unwrap())
            .unwrap();
    assert_eq!(found.len(), 3, "the .xmp is not a photo: {found:?}");
    assert!(found.iter().all(|c| c.capture_time == Some(T) && !c.duplicate));

    let settings = ImportSettings {
        destination: dest.clone(),
        keywords: vec![vec!["Trips".into(), "Hegra".into()]],
        ..Default::default()
    };
    let calls = std::sync::Mutex::new(Vec::new());
    let report = cat
        .import(
            &found,
            &settings,
            Some(&read),
            &|d, t| calls.lock().unwrap().push((d, t)),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        (report.imported, report.failed.len()),
        (3, 0),
        "RAW+JPEG: two photos of one picture: {report:?}"
    );
    assert_eq!(calls.lock().unwrap().last(), Some(&(3, 3)));
    let day = dest.join("2026").join("2026-11-08");
    assert_eq!(fs::read(day.join("DSC_1.NEF")).unwrap(), b"raw-one");
    assert!(day.join("DSC_1.xmp").is_file(), "sidecars come along");
    assert!(card.join("DSC_1.NEF").is_file(), "Copy leaves the card alone");
    assert!(
        !day.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".importing"))
    );

    // The new photos are the Previous Import, rated from the sidecar, keyworded,
    // and their folders hang under the destination folder in the Folders panel.
    let previous = cat.photos(Source::PreviousImport).unwrap();
    assert_eq!(previous.len(), 2);
    let one = previous.iter().find(|p| p.file_name == "DSC_1.NEF").unwrap();
    assert_eq!((one.rating, one.unit.members), (4, 2), "with its JPEG");
    assert_eq!(cat.photo_keywords(one.id).unwrap(), vec![vec!["Trips".to_string(), "Hegra".to_string()]]);
    let folders = cat.folders().unwrap();
    let id_of = |p: &Path| folders.iter().find(|f| f.path == dunce::canonicalize(p).unwrap()).unwrap();
    assert_eq!(id_of(&day).parent_id, Some(id_of(&dest.join("2026")).id));
    assert_eq!(id_of(&dest.join("2026")).parent_id, Some(id_of(&dest).id));

    // Importing the card again: everything is a suspected duplicate.
    let again = scan_source(&card, true, None, &cat.duplicate_keys().unwrap()).unwrap();
    assert!(again.iter().all(|c| c.duplicate), "{again:?}");
}

#[test]
fn move_import_removes_originals_and_add_leaves_files_in_place() {
    let (_tmp, card, dest, mut cat) = setup();
    let found = scan_source(&card, false, None, &HashSet::new()).unwrap();
    let only_two: Vec<Candidate> = found.into_iter().filter(|c| stem(&c.path) == "DSC_2").collect();
    let settings = ImportSettings {
        mode: ImportMode::Move,
        destination: dest.clone(),
        date_folders: DateFolders::None,
        ..Default::default()
    };
    let report = cat.import(&only_two, &settings, None, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.imported, 1);
    assert!(!card.join("DSC_2.NEF").exists() && dest.join("DSC_2.NEF").is_file());
    assert!(card.join("DSC_1.NEF").exists(), "unchosen files stay");

    // Add the rest where they are; only the chosen files join the catalog.
    let rest: Vec<Candidate> = scan_source(&card, false, None, &HashSet::new())
        .unwrap()
        .into_iter()
        .filter(|c| c.path.extension().unwrap() == "NEF")
        .collect();
    let add = ImportSettings { mode: ImportMode::Add, ..Default::default() };
    let report = cat.import(&rest, &add, None, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.imported, 1);
    let all = cat.photos(Source::AllPhotographs).unwrap();
    assert_eq!(all.len(), 2, "the unchosen JPEG was not added: {all:?}");
    assert!(card.join("DSC_1.NEF").exists(), "Add never moves files");

    // Cancelled before starting: nothing happens.
    let cancelled = cat.import(&rest, &settings, None, &|_, _| {}, &AtomicBool::new(true)).unwrap();
    assert!(cancelled.cancelled);
    assert!(card.join("DSC_1.NEF").exists());
}
