use std::path::{Path, PathBuf};

use super::*;
use crate::{Criterion, NumOp, Source};

struct Fx {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    cat: Catalog,
}

fn fixture(files: &[&str]) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    std::fs::create_dir(&dir).unwrap();
    for f in files {
        std::fs::write(dir.join(f), b"x").unwrap();
    }
    let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
    cat.sync_folder(&dir).unwrap();
    Fx { _tmp: tmp, dir, cat }
}

fn id(cat: &Catalog, name: &str) -> i64 {
    cat.conn
        .query_row(
            "SELECT p.id FROM photos p JOIN files f ON f.id = p.master_file_id
              WHERE f.name = ?1 AND p.virtual_of IS NULL",
            [name],
            |r| r.get(0),
        )
        .unwrap()
}

/// The grid: (shown file name, members in the unit).
fn grid(cat: &Catalog) -> Vec<(String, u32)> {
    cat.photos(Source::AllPhotographs).unwrap().into_iter().map(|p| (p.file_name, p.unit.members)).collect()
}

fn names(members: &[Member]) -> Vec<(&str, Role, bool)> {
    members.iter().map(|m| (m.photo.file_name.as_str(), m.photo.unit.role, m.active)).collect()
}

#[test]
fn passes_and_edits_join_their_original_by_name() {
    let fx = fixture(&[
        "a.NEF",
        "a.jpg",
        "scan.tif",
        "scan_IR.tif",
        "scan_IR_valid.tiff",
        "scan_NegPy.tiff",
        "b.jpg",
        "c_LuminarNeo-edit.tif", // no original: a picture of its own
        "d_IR.jpg",              // IR passes are TIFFs
        "e.png",
        "e_LuminarNeo-edit_0.tif",
    ]);
    let mut g = grid(&fx.cat);
    g.sort();
    assert_eq!(
        g,
        [
            ("a.NEF".to_owned(), 2),
            ("b.jpg".to_owned(), 1),
            ("c_LuminarNeo-edit.tif".to_owned(), 1),
            ("d_IR.jpg".to_owned(), 1),
            ("e_LuminarNeo-edit_0.tif".to_owned(), 2),
            ("scan_NegPy.tiff".to_owned(), 4),
        ],
        "one cell per picture, an edit shown when there is one"
    );
    let scan = fx.cat.unit_members(id(&fx.cat, "scan.tif")).unwrap();
    assert_eq!(
        names(&scan),
        [
            ("scan.tif", Role::Original, false),
            ("scan_IR.tif", Role::Pass, false),
            ("scan_IR_valid.tiff", Role::Pass, false),
            ("scan_NegPy.tiff", Role::Edit, true),
        ]
    );
    assert_eq!(scan[3].derived_from, Some(id(&fx.cat, "scan.tif")));
    assert!(scan.iter().all(|m| m.picture_active));
    assert_eq!(fx.cat.folders().unwrap()[0].photo_count, 11, "every file is a photo");
}

#[test]
fn names_are_matched_like_negpy_does() {
    let c = classify;
    assert_eq!(c("Frame1_ir", "TIF"), Some((Role::Pass, vec!["frame1".into()], true)));
    assert_eq!(c("Frame1_IR", "jpg"), None);
    assert_eq!(c("_ir", "tif"), None, "no name before the suffix");
    assert_eq!(c("roll_3_NegPy", "tiff"), Some((Role::Edit, vec!["roll_3".into(), "roll".into()], false)));
    assert_eq!(c("x_LuminarNeo-edit_12", "tif"), Some((Role::Edit, vec!["x".into()], false)));
    assert_eq!(c("x_LuminarNeo-editor", "tif"), None);
    assert_eq!(c("plain", "tif"), None);
}

#[test]
fn separate_jpegs_stay_pictures_of_their_own() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    std::fs::create_dir(&dir).unwrap();
    for f in ["a.NEF", "a.jpg"] {
        std::fs::write(dir.join(f), b"x").unwrap();
    }
    let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
    cat.set_separate_raw_jpeg(true).unwrap();
    cat.sync_folder(&dir).unwrap();
    assert_eq!(grid(&cat).len(), 2);
}

#[test]
fn the_active_member_is_shown_and_filters_match_any_member() {
    let mut fx = fixture(&["scan.tif", "scan_IR.tif", "scan_NegPy.tiff", "other.tif"]);
    let (scan, ir, edit) =
        (id(&fx.cat, "scan.tif"), id(&fx.cat, "scan_IR.tif"), id(&fx.cat, "scan_NegPy.tiff"));
    fx.cat.set_active(ir).unwrap();
    assert!(grid(&fx.cat).contains(&("scan_IR.tif".into(), 3)));
    fx.cat.set_active(edit).unwrap();

    // Only the original has 5 stars: the unit shows (with its edit), marked.
    fx.cat.set_rating(scan, 5).unwrap();
    let five = Criterion::Rating { op: NumOp::AtLeast, stars: 5 };
    let rows = fx.cat.photos_filtered(Source::AllPhotographs, Default::default(), &five).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id, rows[0].unit.hidden_match), (edit, true));
    fx.cat.set_rating(edit, 5).unwrap();
    let rows = fx.cat.photos_filtered(Source::AllPhotographs, Default::default(), &five).unwrap();
    assert!(!rows[0].unit.hidden_match);
}

#[test]
fn capture_time_sorts_by_the_original() {
    let fx = fixture(&["r1.tif", "r1_NegPy.tif", "r2.tif"]);
    let m = |t| crate::PhotoMetadata { capture_time: Some(t), ..Default::default() };
    fx.cat.set_metadata(id(&fx.cat, "r1.tif"), &m(100)).unwrap();
    fx.cat.set_metadata(id(&fx.cat, "r2.tif"), &m(200)).unwrap();
    // The edit was "taken" much later (its file date), but stays in frame order.
    fx.cat.set_metadata(id(&fx.cat, "r1_NegPy.tif"), &m(9_000)).unwrap();
    let order: Vec<String> = grid(&fx.cat).into_iter().map(|(n, _)| n).collect();
    assert_eq!(order, ["r1_NegPy.tif", "r2.tif"]);
}

#[test]
fn groups_hold_pictures_with_their_members() {
    let mut fx = fixture(&["a.tif", "a_IR.tif", "b.tif", "b_NegPy.tif", "c.tif"]);
    let (a, b, c) = (id(&fx.cat, "a.tif"), id(&fx.cat, "b_NegPy.tif"), id(&fx.cat, "c.tif"));
    fx.cat.group_photos(&[b, a]).unwrap();
    let mut g = grid(&fx.cat);
    g.sort();
    assert_eq!(g, [("b_NegPy.tif".to_owned(), 4), ("c.tif".to_owned(), 1)], "b's picture first: active");
    let members = fx.cat.unit_members(c).unwrap();
    assert_eq!(members.len(), 1);
    let members = fx.cat.unit_members(a).unwrap();
    assert_eq!(
        members.iter().map(|m| (m.photo.file_name.as_str(), m.picture_active)).collect::<Vec<_>>(),
        [("b.tif", true), ("b_NegPy.tif", true), ("a.tif", false), ("a_IR.tif", false)],
        "picture by picture, in group order"
    );
    let row = fx.cat.photo(a).unwrap().unwrap();
    assert_eq!((row.unit.members, row.unit.pictures), (4, 2));

    // Making a member of another picture active makes that picture the group's.
    fx.cat.set_active(id(&fx.cat, "a_IR.tif")).unwrap();
    assert!(grid(&fx.cat).contains(&("a_IR.tif".into(), 4)));

    // Grouping a grouped photo with another merges; ungrouping keeps pictures whole.
    fx.cat.group_photos(&[c, a]).unwrap();
    assert_eq!(grid(&fx.cat), [("c.tif".to_owned(), 5)]);
    assert_eq!(fx.cat.ungroup(&[a]).unwrap(), 1);
    let mut g = grid(&fx.cat);
    g.sort();
    assert_eq!(
        g,
        [("a_IR.tif".to_owned(), 2), ("b_NegPy.tif".to_owned(), 2), ("c.tif".to_owned(), 1)],
        "each picture keeps its active member"
    );
    assert!(fx.cat.group_photos(&[a]).is_err(), "a group needs two pictures");
    let groups: i64 = fx.cat.conn.query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0)).unwrap();
    assert_eq!(groups, 0);
}

#[test]
fn detached_members_stay_detached_and_removal_keeps_pictures_whole() {
    let mut fx = fixture(&["a.NEF", "a.jpg", "b.tif", "b_NegPy.tif"]);
    let jpg = id(&fx.cat, "a.jpg");
    fx.cat.detach(jpg).unwrap();
    assert!(fx.cat.detach(jpg).is_err(), "already alone");
    fx.cat.sync_folder(&fx.dir).unwrap();
    assert_eq!(grid(&fx.cat).len(), 3, "a re-sync doesn't join it again");

    // The shown edit goes: the original is shown again.
    std::fs::remove_file(fx.dir.join("b_NegPy.tif")).unwrap();
    let edit = id(&fx.cat, "b_NegPy.tif");
    fx.cat.conn.execute("DELETE FROM photos WHERE id = ?1", [edit]).unwrap();
    assert!(grid(&fx.cat).contains(&("b.tif".into(), 1)));

    // The original goes: another member takes its place.
    let raw = id(&fx.cat, "a.NEF");
    let copy = fx.cat.create_virtual_copy(raw).unwrap();
    assert!(grid(&fx.cat).contains(&("a.NEF".into(), 2)), "a copy is shown once made");
    fx.cat.conn.execute("DELETE FROM photos WHERE id = ?1", [raw]).unwrap();
    let pictures_without_original: i64 = fx
        .cat
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pictures pi
              WHERE NOT EXISTS (SELECT 1 FROM photos WHERE picture_id = pi.id AND role = 'original')
                 OR active_photo_id NOT IN (SELECT id FROM photos WHERE picture_id = pi.id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pictures_without_original, 0);
    let _ = copy;
}

#[test]
fn edits_and_exports_join_by_hand_too() {
    let mut fx = fixture(&["a.tif", "export.jpg", "edit.tif"]);
    let (a, export, edit) = (id(&fx.cat, "a.tif"), id(&fx.cat, "export.jpg"), id(&fx.cat, "edit.tif"));
    fx.cat.add_to_picture(a, export, Role::Export).unwrap();
    assert_eq!(grid(&fx.cat).iter().find(|(_, n)| *n == 2).unwrap().0, "a.tif", "exports don't take over");
    fx.cat.add_to_picture(a, edit, Role::Edit).unwrap();
    assert_eq!(grid(&fx.cat), [("edit.tif".to_owned(), 3)]);
}

/// A v5 catalog (stacks, a RAW+JPEG pair, a virtual copy, IR and NegPy files) becomes
/// pictures and groups.
#[test]
fn migration_from_v5() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("c.plcat");
    std::fs::create_dir_all(dir.join("previews")).unwrap();
    {
        let conn = rusqlite::Connection::open(dir.join("catalog.db")).unwrap();
        for sql in &crate::schema::MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, path) VALUES (1, '/p');
             INSERT INTO files (id, folder_id, name, ext, kind, size, mtime_ms) VALUES
                 (1, 1, 'a.NEF', 'nef', 'raw', 1, 1),
                 (3, 1, 'scan.tif', 'tif', 'image', 1, 1),
                 (4, 1, 'scan_IR.tif', 'tif', 'image', 1, 1),
                 (5, 1, 'scan_NegPy.tiff', 'tiff', 'image', 1, 1),
                 (6, 1, 'x.tif', 'tif', 'image', 1, 1),
                 (7, 1, 'y.tif', 'tif', 'image', 1, 1);
             INSERT INTO photos (id, master_file_id, imported_at, rating, title) VALUES
                 (1, 1, 0, 4, 'Pier'), (3, 3, 0, 0, NULL), (4, 4, 0, 0, NULL), (5, 5, 0, 0, NULL),
                 (6, 6, 0, 0, NULL), (7, 7, 0, 0, NULL);
             INSERT INTO photos (id, master_file_id, imported_at, virtual_of, copy_name)
                 VALUES (8, 1, 0, 1, 'Copy 1');
             INSERT INTO files (id, folder_id, name, ext, kind, size, mtime_ms, sidecar_of)
                 VALUES (2, 1, 'a.jpg', 'jpg', 'image', 1, 1, 1);
             INSERT INTO keywords (id, name) VALUES (1, 'Film');
             INSERT INTO photo_keywords VALUES (1, 1);
             -- Phase 7 put NegPy edits below their original; a bracket stacked by hand;
             -- a photo stacked with its own copy.
             INSERT INTO stacks (id, collapsed) VALUES (1, 0), (2, 1), (3, 1);
             UPDATE photos SET stack_id = 1, stack_pos = 0 WHERE id = 3;
             UPDATE photos SET stack_id = 1, stack_pos = 1 WHERE id = 5;
             UPDATE photos SET stack_id = 2, stack_pos = 0 WHERE id = 7;
             UPDATE photos SET stack_id = 2, stack_pos = 1 WHERE id = 6;
             UPDATE photos SET stack_id = 3, stack_pos = 0 WHERE id = 1;
             UPDATE photos SET stack_id = 3, stack_pos = 1 WHERE id = 8;",
        )
        .unwrap();
    }
    let cat = Catalog::open(&dir).unwrap();
    assert_eq!(cat.schema_version().unwrap(), crate::SCHEMA_VERSION);
    let mut g = grid(&cat);
    g.sort();
    assert_eq!(
        g,
        [("a.NEF".to_owned(), 3), ("scan_NegPy.tiff".to_owned(), 3), ("y.tif".to_owned(), 2)],
        "pair + copy; scan + IR + edit; the bracket a group with its top shown"
    );
    let a = cat.unit_members(1).unwrap();
    assert_eq!(
        names(&a),
        [("a.NEF", Role::Original, true), ("a.jpg", Role::Pass, false), ("a.NEF", Role::Copy, false)]
    );
    let jpg = &a[1].photo;
    assert_eq!((jpg.rating, jpg.path.clone()), (4, Path::new("/p").join("a.jpg")), "the pair's metadata");
    assert_eq!(cat.photo_keywords(jpg.id).unwrap(), vec![vec!["Film".to_owned()]]);
    assert_eq!(cat.xmp_data(jpg.id).unwrap().title.as_deref(), Some("Pier"));
    let groups: i64 = cat.conn.query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0)).unwrap();
    assert_eq!(groups, 1, "only the bracket is still a group");
}

#[test]
fn marks_belong_to_the_stack() {
    let mut fx = fixture(&["a.tif", "a_IR.tif", "b.tif", "c.tif"]);
    let (a, ir, b, c) =
        (id(&fx.cat, "a.tif"), id(&fx.cat, "a_IR.tif"), id(&fx.cat, "b.tif"), id(&fx.cat, "c.tif"));
    let mut unit = fx.cat.unit_photo_ids(&[ir]).unwrap();
    unit.sort();
    assert_eq!(unit, [a, ir]);

    // A newcomer takes the stack's rating, flag and label.
    fx.cat.set_rating(b, 4).unwrap();
    fx.cat.set_flag(b, crate::Flag::Picked).unwrap();
    fx.cat.set_color_label(b, Some("red")).unwrap();
    std::fs::write(fx.dir.join("b_NegPy.tif"), b"x").unwrap();
    fx.cat.sync_folder(&fx.dir).unwrap();
    let edit = fx.cat.photo(id(&fx.cat, "b_NegPy.tif")).unwrap().unwrap();
    assert_eq!((edit.rating, edit.flag, edit.color_label.as_deref()), (4, crate::Flag::Picked, Some("red")));

    // Grouping gives every picture the first one's marks; the unit spans the group.
    fx.cat.group_photos(&[b, a]).unwrap();
    let all = fx.cat.unit_photo_ids(&[a]).unwrap();
    assert_eq!(all.len(), 4);
    assert!(all.iter().all(|&m| fx.cat.photo(m).unwrap().unwrap().rating == 4));
    assert_eq!(fx.cat.photo(c).unwrap().unwrap().rating, 0, "other stacks are left alone");
}

#[test]
fn rejected_photos_leave_the_catalog_but_not_the_disk() {
    let mut fx = fixture(&["a.tif", "a_IR.tif", "b.tif", "c.tif"]);
    let (a, ir, b) = (id(&fx.cat, "a.tif"), id(&fx.cat, "a_IR.tif"), id(&fx.cat, "b.tif"));
    let copy = fx.cat.create_virtual_copy(b).unwrap();
    for p in [a, ir, copy] {
        fx.cat.set_flag(p, crate::Flag::Rejected).unwrap();
    }
    let rejected = fx.cat.rejected_in(Source::AllPhotographs).unwrap();
    assert_eq!(rejected, [a, ir, copy]);
    let previews = fx.cat.remove_photos(&rejected).unwrap();
    assert_eq!(previews, [a, ir], "the copy shares its master's previews");
    let mut g = grid(&fx.cat);
    g.sort();
    assert_eq!(g, [("b.tif".to_owned(), 1), ("c.tif".to_owned(), 1)]);
    assert!(fx.dir.join("a.tif").exists() && fx.dir.join("a_IR.tif").exists(), "files stay");

    // The files can come back: a sync finds them again.
    let r = fx.cat.sync_folder(&fx.dir).unwrap();
    assert_eq!(r.added, 2);
    assert!(grid(&fx.cat).contains(&("a.tif".into(), 2)), "joined again");
}
