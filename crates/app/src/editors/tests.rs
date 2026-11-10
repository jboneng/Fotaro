use super::*;

#[test]
fn launch_args_carry_the_recipe_and_the_files_last() {
    let prefs = EditInPrefs::default();
    let files = [PathBuf::from("C:/scans/a.NEF"), PathBuf::from("C:/scans/b.tif")];
    let args: Vec<String> = launch_args(&files, Path::new("C:/scans"), &prefs, true)
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--format",
            "tiff",
            "--bit-depth",
            "16",
            "--color-space",
            "adobe-rgb",
            "--output-dir",
            "C:/scans",
            "--filename",
            "{{ original_name }}-NegPy",
            "--quit-after-export",
            "C:/scans/a.NEF",
            "C:/scans/b.tif",
        ]
    );
    let jpeg =
        EditInPrefs { format: EditFormat::Jpeg, quality: 80, color_space: EditColorSpace::Srgb, ..prefs };
    let args: Vec<String> = launch_args(&files[..1], Path::new("o"), &jpeg, false)
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[..6], ["--format", "jpeg", "--quality", "80", "--color-space", "srgb"]);
    assert!(!args.contains(&"--quit-after-export".to_owned()));
}

#[test]
fn results_are_recognised_by_name() {
    let yes = ["scan_NegPy.tif", "SCAN_negpy.TIFF", "scan_NegPy.jpg", "scan_2_NegPy.png"];
    for n in yes {
        assert!(is_result_of(Path::new(n), "scan"), "{n}");
    }
    let no = [
        "scan.tif",
        "scan_NegPy.xmp",
        "scan2_NegPy.tif",
        "scan_x_NegPy.tif",
        "other_NegPy.tif",
        "scan_NegPy_2.tif",
    ];
    for n in no {
        assert!(!is_result_of(Path::new(n), "scan"), "{n}");
    }
    assert!(is_result_of(Path::new("my scan (10)_NegPy.tif"), "my scan (10)"), "names are kept verbatim");
}

#[test]
fn versions() {
    assert_eq!(parse_version("0.68.0\n"), Some((0, 68, 0)));
    assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
    assert_eq!(parse_version("0.69.0-rc1"), Some((0, 69, 0)));
    assert_eq!(parse_version("dev"), None);
}

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
}

#[test]
fn source_checkouts_need_launch_support_and_a_venv() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("NegPy");
    touch(&repo.join("desktop.py"));
    std::fs::create_dir_all(repo.join("negpy/desktop")).unwrap();
    std::fs::write(repo.join("VERSION"), "0.68.0\n").unwrap();
    assert!(at_location(&repo).unwrap_err().contains("can't open files"));
    touch(&repo.join("negpy/desktop/launch.py"));
    assert!(at_location(&repo).unwrap_err().contains("uv sync"));
    let python = repo.join(if cfg!(windows) { ".venv/Scripts/pythonw.exe" } else { ".venv/bin/python3" });
    touch(&python);
    let n = at_location(&repo).unwrap();
    assert_eq!(n.launcher, Launcher::Source { python, repo: repo.clone() });
    assert_eq!(n.version.as_deref(), Some("0.68.0"));
    assert!(n.describe().contains("from source"));
}

#[test]
fn releases_need_a_version_that_can_open_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("NegPy");
    touch(&dir.join("NegPy.exe"));
    touch(&dir.join("_internal/VERSION"));
    std::fs::write(dir.join("_internal/VERSION"), "0.68.0").unwrap();
    assert!(at_location(&dir).unwrap_err().contains("0.68.0 is installed"));
    std::fs::write(dir.join("_internal/VERSION"), "0.69.0").unwrap();
    let n = at_location(&dir.join("NegPy.exe")).unwrap();
    assert_eq!(n.launcher, Launcher::App { exe: dir.join("NegPy.exe") });
    assert!(at_location(&tmp.path().join("nothing")).is_err());
}

#[test]
fn scanning_finds_new_results_only() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    std::fs::write(dir.join("a_NegPy.tif"), b"old").unwrap();
    let since = SystemTime::now() + std::time::Duration::from_secs(5);
    let w = watch(7, &dir, "a", since);
    assert!(scan(std::slice::from_ref(&w)).is_empty(), "made before the edit started");
    let w = EditWatch { since: SystemTime::now() - std::time::Duration::from_secs(5), ..w };
    std::fs::write(dir.join("b_NegPy.tif"), b"other photo").unwrap();
    let found = scan(&[w]);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].0, found[0].1.clone(), found[0].2.0), (7, dir.join("a_NegPy.tif"), 3));
}

#[test]
fn negpy_launches_go_per_folder_and_single_photos_close_it() {
    let negpy = NegPy {
        launcher: Launcher::Source { python: PathBuf::from("py"), repo: PathBuf::from("/src/NegPy") },
        version: None,
    };
    let prefs = EditInPrefs::default();
    let one = negpy.launches(&[(1, PathBuf::from("/a/x.NEF"))], &prefs);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].program, PathBuf::from("py"));
    assert_eq!(one[0].args[0], PathBuf::from("/src/NegPy").join("desktop.py").into_os_string());
    assert!(one[0].args.contains(&"--quit-after-export".into()));
    assert!(!one[0].wait_for_exit);
    let many = negpy.launches(
        &[
            (1, PathBuf::from("/a/x.NEF")),
            (2, PathBuf::from("/b/y.tif")),
            (3, PathBuf::from("/a/z.NEF")),
            (4, PathBuf::from("/a/x.NEF")), // a virtual copy: same file, opened once
        ],
        &prefs,
    );
    assert_eq!(
        many.iter().map(|l| (l.results[0].1.clone(), l.results.len())).collect::<Vec<_>>(),
        vec![(PathBuf::from("/a"), 2), (PathBuf::from("/b"), 1)]
    );
    assert!(
        many.iter().all(|l| !l.args.contains(&"--quit-after-export".into())),
        "several photos: it stays open"
    );
    assert_eq!(many[0].results[0].2, ResultName::NegPy { stem: "x".into() });
}

#[test]
fn results_are_taken_once_written_and_again_when_re_exported() {
    let mut t = EditTracker::default();
    let p = PathBuf::from("/a/x_NegPy.tif");
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let v1 = (10, at);
    assert!(t.checked(vec![(1, p.clone(), (5, at))]).is_empty(), "still being written");
    assert!(t.checked(vec![(1, p.clone(), v1)]).is_empty(), "grew since");
    assert_eq!(t.checked(vec![(1, p.clone(), v1)]), vec![(1, p.clone())], "unchanged twice: done");
    assert!(t.checked(vec![(1, p.clone(), v1)]).is_empty(), "taken already");
    let v2 = (12, at + Duration::from_secs(60));
    assert!(t.checked(vec![(1, p.clone(), v2)]).is_empty());
    assert_eq!(t.checked(vec![(1, p.clone(), v2)]), vec![(1, p)], "exported again from NegPy");
}

#[test]
fn checks_wait_for_watches_and_each_other() {
    let mut t = EditTracker::default();
    assert!(t.due_check().is_none(), "nothing to watch");
    t.watches.push(watch(1, Path::new("/a"), "x", SystemTime::now()));
    assert_eq!(t.due_check().map(|w| w.len()), Some(1));
    assert!(t.due_check().is_none(), "one look at a time");
    t.checked(Vec::new());
    assert!(t.due_check().is_none(), "and not more often than every few seconds");
}

#[test]
fn results_dated_like_their_original_are_still_found() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let since = SystemTime::now() - Duration::from_secs(2);
    let result = dir.join("scan_NegPy.tif");
    std::fs::write(&result, b"pixels").unwrap();
    // NegPy stamps its export with the original's (old) modified time.
    let old = SystemTime::now() - Duration::from_secs(400 * 24 * 3600);
    std::fs::File::options().write(true).open(&result).unwrap().set_modified(old).unwrap();
    let w = watch(1, &dir, "scan", since);
    let found = scan(std::slice::from_ref(&w));
    assert_eq!(found.len(), 1, "found by its change time, not its modified time");
    let first = found[0].2;

    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(&result, b"pixels").unwrap(); // exported again: same size …
    std::fs::File::options().write(true).open(&result).unwrap().set_modified(old).unwrap(); // … and date
    let again = scan(&[w])[0].2;
    assert_ne!(again, first, "a new version all the same");
}

fn watch(original: i64, dir: &Path, stem: &str, since: SystemTime) -> EditWatch {
    EditWatch {
        original,
        editor: Editor::NegPy,
        dir: dir.to_path_buf(),
        name: ResultName::NegPy { stem: stem.into() },
        since,
        ended: None,
    }
}

#[test]
fn base64_matches_lightroom() {
    assert_eq!(luminar::base64(b""), "");
    assert_eq!(luminar::base64(b"f"), "Zg==");
    assert_eq!(luminar::base64(b"fo"), "Zm8=");
    assert_eq!(luminar::base64(b"foo"), "Zm9v");
    assert_eq!(luminar::base64(b"foobar"), "Zm9vYmFy");
    assert_eq!(luminar::base64(r"C:\Bilder\Ø.NEF".as_bytes()), "QzpcQmlsZGVyXMOYLk5FRg==", "UTF-8");
}

#[test]
fn luminar_gets_one_photo_and_a_result_path_per_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.NEF"), b"x").unwrap();
    std::fs::write(dir.join("a_LuminarNeo-edit.tif"), b"earlier edit").unwrap();
    std::fs::write(dir.join("b.tif"), b"x").unwrap();
    let lum =
        Luminar { exe: PathBuf::from("C:/Program Files/Skylum/Luminar Neo/Luminar Neo.exe"), version: None };
    let launches = lum.launches(&[(1, dir.join("a.NEF")), (2, dir.join("b.tif")), (3, dir.join("a.NEF"))]);
    assert_eq!(launches.len(), 2, "one per photo; the virtual copy's file only once");
    assert!(launches.iter().all(|l| l.wait_for_exit && l.results.len() == 1));
    let a = &launches[0];
    assert_eq!(a.results[0].2, ResultName::Exact(PathBuf::from("a_LuminarNeo-edit_0.tif")), "name taken");
    assert_eq!(launches[1].results[0].2, ResultName::Exact(PathBuf::from("b_LuminarNeo-edit.tif")));
    let path = dir.join("a.NEF");
    assert_eq!(
        a.args,
        vec![
            OsString::from("lightroom"),
            luminar::base64(path.to_string_lossy().as_bytes()).into(),
            "-MPResPath".into(),
            luminar::base64(dir.join("a_LuminarNeo-edit_0.tif").to_string_lossy().as_bytes()).into(),
        ]
    );
    assert!(
        ResultName::Exact(PathBuf::from("b_LuminarNeo-edit.tif")).matches(&dir.join("B_LUMINARNEO-EDIT.TIF"))
    );
    assert!(
        !ResultName::Exact(PathBuf::from("b_LuminarNeo-edit.tif"))
            .matches(&dir.join("b_LuminarNeo-edit_0.tif"))
    );
}

#[test]
fn luminar_launches_wait_for_the_previous_to_close_and_missing_edits_are_reported() {
    // A stand-in "editor" that exits at once without writing anything.
    let exe = if cfg!(windows) { PathBuf::from("cmd") } else { PathBuf::from("true") };
    let args: Vec<OsString> = if cfg!(windows) { vec!["/C".into(), "exit 0".into()] } else { Vec::new() };
    let tmp = tempfile::tempdir().unwrap();
    let launch = |id: i64| Launch {
        editor: Editor::Luminar,
        program: exe.clone(),
        args: args.clone(),
        cwd: None,
        results: vec![(
            id,
            tmp.path().to_path_buf(),
            ResultName::Exact(PathBuf::from(format!("{id}_LuminarNeo-edit.tif"))),
        )],
        wait_for_exit: true,
        then: Vec::new(),
        clear_first: Vec::new(),
        finish: Vec::new(),
    };
    let mut t = EditTracker::default();
    t.enqueue(vec![launch(1), launch(2)]);
    assert!(t.tick().is_empty());
    assert_eq!(t.watches.len(), 1, "the second waits for the first to close");
    let started = Instant::now();
    while t.watches.len() < 2 && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
        t.tick();
    }
    assert_eq!(t.watches.len(), 2, "started once the first closed");
    assert!(t.watches[0].ended.is_some(), "the first is known to have closed");

    // Long after closing without a result: given up, and said so.
    t.watches[0].ended = Some(Instant::now() - AFTER_EXIT - Duration::from_secs(1));
    let messages = t.tick();
    assert!(messages.iter().any(|m| m.contains("closed without saving")), "{messages:?}");
    assert_eq!(t.watches.len(), 1);
}

#[test]
fn darktable_opens_everything_at_once_and_renders_what_changed() {
    let dt = Darktable {
        exe: PathBuf::from("C:/Program Files/darktable/bin/darktable.exe"),
        cli: PathBuf::from("C:/Program Files/darktable/bin/darktable-cli.exe"),
        version: Some("5.6.2".into()),
    };
    let prefs = EditInPrefs { darktable_color_space: EditColorSpace::ProPhotoRgb, ..Default::default() };
    let a = PathBuf::from("C:/scans/a.NEF");
    let b = PathBuf::from("C:/scans/b.tif");
    let launches = dt.launches(&[(1, a.clone()), (2, b.clone()), (3, a.clone())], &prefs);
    assert_eq!(launches.len(), 1, "one window");
    let gui = &launches[0];
    assert_eq!(gui.args, [a.as_os_str(), b.as_os_str()], "a virtual copy's file only once");
    assert!(gui.wait_for_exit);
    assert_eq!(gui.results[0].2, ResultName::Exact(PathBuf::from("a_darktable.tif")));
    assert_eq!(gui.then.len(), 2);
    assert_eq!(gui.then[0].if_changed, PathBuf::from("C:/scans/a.NEF.xmp"), "darktable's own sidecar");

    let render = &gui.then[1].launch;
    let args: Vec<String> = render.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(render.program, dt.cli);
    assert_eq!(args[..2], ["C:/scans/b.tif", "C:/scans/b.tif.xmp"]);
    assert!(!args[2].contains('\\'), "darktable-cli takes \\ as an escape: {}", args[2]);
    assert!(args.windows(2).any(|w| w == ["--icc-type", "PROPHOTO_RGB"]));
    assert!(args.contains(&"plugins/imageio/format/tiff/bpp=16".to_owned()));
    assert_eq!(render.finish[0].1, PathBuf::from("C:/scans/b_darktable.tif"));
    assert_eq!(render.clear_first, [render.finish[0].0.clone()], "the temporary render is cleared first");
}

/// A stand-in program: `script` run by the system shell.
fn shell(script: &str) -> (PathBuf, Vec<OsString>) {
    if cfg!(windows) {
        (PathBuf::from("cmd"), vec!["/C".into(), script.into()])
    } else {
        (PathBuf::from("sh"), vec!["-c".into(), script.into()])
    }
}

fn run_until(t: &mut EditTracker, done: impl Fn(&EditTracker) -> bool) -> Vec<String> {
    let started = Instant::now();
    let mut messages = Vec::new();
    while !done(t) && started.elapsed() < Duration::from_secs(15) {
        messages.extend(t.tick());
        std::thread::sleep(Duration::from_millis(50));
    }
    messages
}

#[test]
fn follow_ups_run_only_for_changed_files_and_replace_the_result() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let sidecar = dir.join("a.NEF.xmp");
    std::fs::write(&sidecar, "before").unwrap();
    let result = dir.join("a_darktable.tif");
    std::fs::write(&result, "old render").unwrap();
    let temp = dir.join("render.tmp");
    let (program, args) = shell(&format!("echo rendered> {}", temp.display()));
    let render = Launch {
        editor: Editor::Darktable,
        program,
        args,
        cwd: None,
        results: vec![(1, dir.clone(), ResultName::Exact(PathBuf::from("a_darktable.tif")))],
        wait_for_exit: true,
        then: Vec::new(),
        clear_first: vec![temp.clone()],
        finish: vec![(temp.clone(), result.clone())],
    };
    let (program, args) = shell("exit 0");
    let gui = Launch {
        editor: Editor::Darktable,
        program,
        args,
        cwd: None,
        results: render.results.clone(),
        wait_for_exit: true,
        then: vec![FollowUp { original: 1, if_changed: sidecar.clone(), launch: render }],
        clear_first: Vec::new(),
        finish: Vec::new(),
    };

    // Edited: the sidecar changes while "darktable" runs; the render replaces the result.
    let mut t = EditTracker::default();
    t.enqueue(vec![gui.clone()]);
    t.tick();
    std::fs::write(&sidecar, "after").unwrap();
    let messages =
        run_until(&mut t, |_| std::fs::read_to_string(&result).is_ok_and(|s| s.contains("rendered")));
    assert!(std::fs::read_to_string(&result).unwrap().contains("rendered"), "{messages:?}");
    assert!(!temp.exists(), "moved, not copied");
    assert_eq!(t.watches.len(), 1, "still watched: the result comes back to the catalog");

    // Not edited: no render, and said so.
    let mut t = EditTracker::default();
    t.enqueue(vec![gui]);
    let messages = run_until(&mut t, |t| t.is_idle());
    assert!(messages.iter().any(|m| m.contains("darktable closed")), "{messages:?}");
    assert!(t.watches.is_empty());
}
