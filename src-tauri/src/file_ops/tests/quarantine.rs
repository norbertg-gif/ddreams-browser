#[test]
fn quarantine_review_excludes_protected_old_undated_outside_and_nonphotos() {
    let root = temp();
    let (_, manager, settings) = quarantine_fixture(&root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let now = crate::quarantine::now();
    let aged = now - 61 * crate::quarantine::DAY;
    {
        let conn = manager.db.lock().unwrap();
        conn.execute_batch("DELETE FROM assets; DELETE FROM asset_categories;")
            .unwrap();
        for (id, ext, date, rating, label, tagged, base, seen) in [
            (1, "jpg", Some("2024-06-01T12:30:00"), None, None, None, "src", aged),
            (2, "rw2", Some("2024-06-02"), Some(0), Some(""), Some(0), "src", aged),
            (3, "jpg", Some("2024-07-01"), Some(4), None, None, "src", aged),
            (4, "jpg", Some("2024-07-01"), None, Some("Red"), None, "src", aged),
            (5, "jpg", Some("2024-07-01"), None, None, Some(1), "src", aged),
            (6, "jpg", Some("2024-07-01"), None, None, None, "src", aged),
            (7, "jpg", Some("2024-05-31"), None, None, None, "src", aged),
            (8, "jpg", None, None, None, None, "src", aged),
            (9, "jpg", Some("2024-07-01"), None, None, None, "src-other", aged),
            (10, "mp4", Some("2024-07-01"), None, None, None, "src", aged),
            (11, "jpg", Some("2024-07-01"), None, None, None, "src", now),
            (12, "jpg", Some("2024-02-31"), None, None, None, "src", aged),
        ] {
            let name = format!("{id}.{ext}");
            let folder = root.join(base);
            fs::create_dir_all(&folder).unwrap();
            let path = folder.join(&name);
            fs::write(&path, b"test").unwrap();
            conn.execute("INSERT INTO assets(id,name,ext,ext_lower,is_listed,path,exif_date,rating,label,tagged,incoming_seen_at)
                    VALUES(?1,?2,?3,?3,1,?4,?5,?6,?7,?8,?9)",
                (id, name, ext, path.to_str().unwrap(), date, rating, label, tagged, seen)).unwrap();
        }
        conn.execute("INSERT INTO asset_categories VALUES(6,1)", []).unwrap();
    }
    let preview = manager
        .prepare_quarantine_review(settings.clone(), &formats, None)
        .unwrap();
    assert_eq!(preview.moves.ids, vec![1, 2]);
    assert_eq!(preview.moves.bytes, 8);
    assert!(preview.deletes.ids.is_empty());
    assert!(root.join("src/1.jpg").exists());
    manager
        .db
        .lock()
        .unwrap()
        .execute("UPDATE assets SET rating=5 WHERE id=1", [])
        .unwrap();
    let result = manager
        .execute_quarantine_review(
            &settings,
            &formats,
            None,
            crate::quarantine::ReviewSelection {
                moves: true,
                deletes: true,
                stale_edits: true,
            },
            &[],
            |_, _| {},
        )
        .unwrap();
    assert_eq!((result.moved, result.failures.len()), (1, 1));
    assert!(root.join("src/1.jpg").exists());
    assert!(root.join("dst/2.rw2").exists());
    assert!(
        manager
            .execute_quarantine_review(
                &settings,
                &formats,
                None,
                crate::quarantine::ReviewSelection {
                    moves: true,
                    deletes: true,
                    stale_edits: true
                },
                &[],
                |_, _| {}
            )
            .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_edit_batch_preserves_first_mark_and_lists_oldest_first() {
    let root = temp();
    let (_, manager, settings) = quarantine_fixture(&root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let path = settings.incoming().join("b.jpg");
    {
        let conn = manager.db.lock().unwrap();
        conn.execute("UPDATE assets SET edit_pending_since=123 WHERE id=1", [])
            .unwrap();
        conn.execute(
            "INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,path)
                VALUES(2,2,'b.jpg','jpg','jpg',1,?1)",
            [path.to_str().unwrap()],
        )
        .unwrap();
    }
    let change = |value| {
        manager
            .writer
            .set_asset_values(
                &[1, 2],
                &[],
                &serde_json::Map::from_iter([("editPending".into(), value)]),
            )
            .unwrap()
    };
    let marked = change(serde_json::json!("toggle"));
    assert_eq!(marked.assets[0].edit_pending_since, Some(123));
    assert!(marked.assets[1].edit_pending_since.unwrap() > 123);
    assert!(change(serde_json::json!(true)).change_id.is_none());
    let filter = serde_json::from_value(serde_json::json!({
        "kind":"special","value":"edit_pending","sort":"name","descending":true,"offset":0,"limit":50
    }))
    .unwrap();
    assert_eq!(
        crate::db::asset_ids_page(&manager.db, &filter, &formats, 0, 50).unwrap(),
        vec![1, 2]
    );
    let page = crate::db::assets_page(&manager.db, &filter, &formats).unwrap();
    assert_eq!(page.items.iter().map(|asset| asset.id).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(page.items[0].edit_pending_since, Some(123));
    let counts = crate::db::values_page(&manager.db, "special", 0, 50, &formats).unwrap();
    assert_eq!(
        counts.iter().find(|item| item.value == "edit_pending").unwrap().count,
        2
    );
    assert!(
        change(serde_json::json!("toggle"))
            .assets
            .iter()
            .all(|asset| asset.edit_pending_since.is_none())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_edit_clears_only_when_listing_mtime_passes_the_grace_period() {
    let root = temp();
    let (disk, manager, settings) = quarantine_fixture(&root);
    let src = settings.incoming();
    let file = src.join("a.jpg");
    let since = 1_700_000_000_i64;
    {
        let conn = manager.db.lock().unwrap();
        conn.execute("UPDATE folders SET path=?1 WHERE id=2", [src.to_str().unwrap()])
            .unwrap();
        conn.execute("UPDATE assets SET edit_pending_since=?1 WHERE id=1", [since])
            .unwrap();
    }
    let filter = serde_json::from_value(serde_json::json!({
        "kind":"folder","id":2,"folderPath":src,"sort":"name","descending":false,"offset":0,"limit":50
    }))
    .unwrap();
    for (seconds, expected) in [(60, 0), (61, 1)] {
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs((since + seconds) as u64);
        disk.set_modified(&file, modified).unwrap();
        manager.listing.invalidate_path(src.to_str().unwrap()).unwrap();
        let (_, rows) = manager.listing.listed_folder(&filter).unwrap();
        assert_eq!(manager.writer.fulfil_edits_from_listing(&rows).unwrap(), expected);
    }
    let pending: Option<i64> = manager
        .db
        .lock()
        .unwrap()
        .query_row("SELECT edit_pending_since FROM assets WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_edit_clears_for_a_newer_listed_sibling_in_the_same_folder() {
    let root = temp();
    let (_, manager, settings) = quarantine_fixture(&root);
    let src = settings.incoming();
    let dst = settings.quarantine();
    {
        let conn = manager.db.lock().unwrap();
        conn.execute("UPDATE folders SET path=?1 WHERE id=2", [src.to_str().unwrap()])
            .unwrap();
        conn.execute("UPDATE folders SET path=?1 WHERE id=3", [dst.to_str().unwrap()])
            .unwrap();
        conn.execute(
            "UPDATE assets SET edit_pending_since=unixepoch('2024-01-02') WHERE id=1",
            [],
        )
        .unwrap();
    }
    let listed = |id, folder: &Path, name: &str, ext: &str, modified: &str| crate::journal::ListedAsset {
        temporary_id: id,
        name: name.into(),
        ext: ext.into(),
        size: Some(4.0),
        file_modified: Some(modified.into()),
        path: folder.join(name).to_string_lossy().into_owned(),
        folder_path: folder.to_string_lossy().into_owned(),
    };
    let pending = || {
        manager
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT edit_pending_since IS NOT NULL FROM assets WHERE id=1",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
    };
    let old = listed(-2, src, "A.tif", "tif", "2024-01-01T00:00:00Z");
    for sibling in [old.clone(), listed(-3, dst, "A.psd", "psd", "2024-01-03T00:00:00Z")] {
        manager.writer.catalog_listed_assets(&[sibling]).unwrap();
        assert!(pending());
    }
    let touched = crate::journal::ListedAsset {
        file_modified: Some("2024-01-03T00:00:00Z".into()),
        ..old
    };
    assert!(manager.writer.catalog_listed_assets(&[touched]).unwrap().is_empty());
    assert!(pending());
    manager
        .writer
        .catalog_listed_assets(&[listed(-4, src, "A.PSD", "psd", "2024-01-03T00:00:00Z")])
        .unwrap();
    assert!(!pending());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_edit_review_can_be_skipped_and_rechecks_scope_before_moving() {
    use crate::quarantine::{DAY, ReviewSelection};
    let root = temp();
    let (disk, manager, settings) = quarantine_fixture(&root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let source = settings.incoming().join("a.jpg");
    let since = crate::quarantine::now() - 91 * DAY;
    disk.set_modified(
        &source,
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(since as u64),
    )
    .unwrap();
    let mark = |timestamp| {
        manager
            .db
            .lock()
            .unwrap()
            .execute("UPDATE assets SET edit_pending_since=?1 WHERE id=1", [timestamp])
            .unwrap()
    };
    let preview = || {
        manager
            .prepare_quarantine_review(settings.clone(), &formats, None)
            .unwrap()
    };
    let execute = |stale_edits| {
        manager
            .execute_quarantine_review(
                &settings,
                &formats,
                None,
                ReviewSelection {
                    moves: true,
                    deletes: true,
                    stale_edits,
                },
                &[],
                |_, _| {},
            )
            .unwrap()
    };
    mark(Some(since));
    assert_eq!(preview().stale_edits.ids, vec![1]);
    assert_eq!(execute(false).moved, 0);
    assert!(source.exists());
    assert_eq!(preview().stale_edits.ids, vec![1]);
    mark(None);
    let outcome = execute(true);
    assert_eq!((outcome.moved, outcome.failures.len()), (0, 1));
    mark(Some(since));
    assert_eq!(preview().stale_edits.ids, vec![1]);
    assert_eq!(execute(true).moved, 1);
    let (pending, quarantined): (Option<i64>, Option<i64>) = manager
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT edit_pending_since,quarantined_at FROM assets WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(pending.is_none() && quarantined.is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantine_round_trip_keeps_sidecars_and_both_conflicting_photos() {
    let root = temp();
    let (_, manager, settings) = quarantine_fixture(&root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let source = root.join("src/a.jpg");
    let sidecar = root.join("src/a.xmp");
    fs::write(&sidecar, b"sidecar").unwrap();
    fs::write(
        root.join("dst/a.rw2"),
        b"existing quarantine file with the same XMP stem",
    )
    .unwrap();
    let skipped = manager
        .move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::Skip, false, |_, _| {})
        .unwrap();
    assert_eq!((skipped.moved, skipped.failures.len()), (0, 1));
    let moved = manager
        .move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::KeepBoth, false, |_, _| {})
        .unwrap();
    assert_eq!((moved.moved, moved.failures.len()), (1, 0));
    assert!(!source.exists() && !sidecar.exists());
    assert_eq!(fs::read(root.join("dst/a (2).xmp")).unwrap(), b"sidecar");
    assert!(root.join("dst/a.rw2").exists());
    {
        let conn = manager.db.lock().unwrap();
        let (path, origin, seen, stamp): (String, String, Option<i64>, i64) = conn
            .query_row(
                "SELECT path,quarantine_origin,incoming_seen_at,quarantined_at FROM assets WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(Path::new(&path), root.join("dst/a (2).jpg"));
        assert_eq!(origin, source.to_string_lossy());
        assert_eq!(seen, None);
        assert!(stamp > 0);
    }
    fs::write(&source, b"new original-name file").unwrap();
    let restored = manager.restore_from_quarantine(&[1], &settings, |_, _| {}).unwrap();
    assert_eq!((restored.restored, restored.failures.len()), (1, 0));
    assert_eq!(fs::read(&source).unwrap(), b"new original-name file");
    assert_eq!(fs::read(root.join("src/a (2).jpg")).unwrap(), b"abc");
    assert_eq!(fs::read(root.join("src/a (2).xmp")).unwrap(), b"sidecar");
    assert!(
        manager
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT incoming_seen_at IS NOT NULL AND
            quarantined_at IS NULL AND quarantine_origin IS NULL FROM assets WHERE id=1",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantine_delete_uses_bin_and_requires_drive_confirmation_for_purge() {
    for bin in [true, false] {
        let temp_dir = temp();
        let root = temp_dir.join(if bin { "fixed" } else { "nobin" });
        let (disk, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        fs::write(root.join("src/a.jpg.xmp"), b"sidecar").unwrap();
        let moved = manager
            .move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::KeepBoth, false, |_, _| {})
            .unwrap();
        assert_eq!(moved.moved, 1);
        let photo = root.join("dst/a.jpg");
        let sidecar = root.join("dst/a.jpg.xmp");
        let info = manager.quarantine_delete_info(&[1], &settings).unwrap();
        assert_eq!(
            (info.count, info.sidecars, info.permanent_count),
            (1, 1, usize::from(!bin))
        );
        if !bin {
            let refused = manager.delete_from_quarantine(&[1], &settings, &[], |_, _| {}).unwrap();
            assert!(refused.failures[0].contains("potvrdenie"));
            assert!(photo.exists() && sidecar.exists());
            manager.quarantine_delete_info(&[1], &settings).unwrap();
        }
        let deleted = manager
            .delete_from_quarantine(&[1], &settings, &info.no_bin_drives, |_, _| {})
            .unwrap();
        assert_eq!((deleted.deleted, deleted.failures.len()), (1, 0));
        assert!(!photo.exists() && !sidecar.exists());
        assert_eq!(disk.recycled_item(&photo).is_some(), bin);
        assert_eq!(disk.recycled_item(&sidecar).is_some(), bin);
        let conn = manager.db.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM assets WHERE id=1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT action FROM ddreams_changes ORDER BY id DESC LIMIT 1", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            if bin {
                "delete_from_quarantine"
            } else {
                "purge_from_quarantine"
            }
        );
        fs::remove_dir_all(temp_dir).unwrap();
    }
}

#[test]
fn incoming_seen_is_initialized_for_existing_and_new_assets_and_cleared_on_move() {
    let root = temp();
    let (_, manager, settings) = quarantine_fixture(&root);
    let src = settings.incoming();
    let seen = |id| {
        manager
            .db
            .lock()
            .unwrap()
            .query_row("SELECT incoming_seen_at FROM assets WHERE id=?1", [id], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .unwrap()
    };
    assert!(seen(1).is_some());
    manager
        .db
        .lock()
        .unwrap()
        .execute("UPDATE assets SET incoming_seen_at=1 WHERE id=1", [])
        .unwrap();
    manager.sync_incoming(src.to_str().unwrap()).unwrap();
    assert_eq!(seen(1), Some(1));
    let path = src.join("b.jpg");
    fs::write(&path, b"test").unwrap();
    let new = manager
        .writer
        .catalog_listed_assets(&[crate::journal::ListedAsset {
            temporary_id: -2,
            name: "b.jpg".into(),
            ext: "jpg".into(),
            size: Some(4.0),
            file_modified: None,
            path: path.to_string_lossy().into_owned(),
            folder_path: src.to_string_lossy().into_owned(),
        }])
        .unwrap()[0]
        .new_id;
    assert!(seen(new).unwrap() > 1);
    manager
        .db
        .lock()
        .unwrap()
        .execute("UPDATE assets SET delete_suggestion=1 WHERE id IN (1,?1)", [new])
        .unwrap();
    manager
        .move_files(
            &[1],
            settings.quarantine().to_str().unwrap(),
            ConflictPolicy::Cancel,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(seen(1), None);
    let suggestion = |id| {
        manager
            .db
            .lock()
            .unwrap()
            .query_row("SELECT delete_suggestion FROM assets WHERE id=?1", [id], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .unwrap()
    };
    assert_eq!(suggestion(1), None);
    assert_eq!(suggestion(new), Some(1));
    fs::remove_file(path).unwrap();
    manager.sync_incoming(src.to_str().unwrap()).unwrap();
    assert_eq!(seen(new), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantine_rejects_import_folder_and_its_descendants() {
    let root = temp();
    let source = root.join("Panasonic");
    fs::create_dir(&source).unwrap();
    for target in [
        &source,
        &source.join("nested"),
        &root,
        &root.join("other/../Panasonic/nested"),
    ] {
        assert!(crate::quarantine::validate_roots(&source, target).is_err());
    }
    assert!(crate::quarantine::validate_roots(&source, &root.join("_Karanténa")).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_quarantine_review_deletes_only_previously_reviewed_unchanged_expired_files() {
    let root = temp();
    let (disk, manager, settings) = quarantine_fixture(&root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let old = crate::quarantine::now() - 31 * crate::quarantine::DAY;
    let fresh = crate::quarantine::now();
    {
        let conn = manager.db.lock().unwrap();
        for (id, folder, age) in [(2, "dst", old), (3, "dst", fresh), (4, "src", old), (5, "dst", old)] {
            let name = format!("{id}.jpg");
            let path = root.join(folder).join(&name);
            fs::write(&path, b"before").unwrap();
            conn.execute(
                "INSERT INTO assets(id,path,name,ext,ext_lower,is_listed,quarantined_at,quarantine_origin)
                    VALUES(?1,?2,?3,'jpg','jpg',1,?4,?5)",
                (
                    id,
                    path.to_str().unwrap(),
                    &name,
                    age,
                    root.join("src").join(&name).to_str().unwrap(),
                ),
            )
            .unwrap();
        }
    }
    let preview = manager
        .prepare_quarantine_review(settings.clone(), &formats, None)
        .unwrap();
    assert_eq!(preview.deletes.ids, vec![2, 5]);
    assert_eq!(preview.deletes.bytes, 12);
    assert!(root.join("dst/2.jpg").exists());
    fs::write(root.join("dst/5.jpg"), b"replacement").unwrap();
    let result = manager
        .execute_quarantine_review(
            &settings,
            &formats,
            None,
            crate::quarantine::ReviewSelection {
                moves: true,
                deletes: true,
                stale_edits: true,
            },
            &[],
            |_, _| {},
        )
        .unwrap();
    assert_eq!((result.deleted, result.failures.len()), (1, 1));
    assert!(disk.recycled_item(&root.join("dst").join("2.jpg")).is_some());
    assert!(root.join("dst/3.jpg").exists());
    assert!(root.join("src/4.jpg").exists());
    assert_eq!(fs::read(root.join("dst/5.jpg")).unwrap(), b"replacement");
    fs::remove_dir_all(root).unwrap();
}
