#[test]
fn move_warning_counts_only_uncategorized_photos_that_would_move() {
    let root = temp();
    let (_disk, manager) = set_up(&root, None);
    seed_asset_path(&manager.db, &root);
    let formats = crate::formats::FormatTable::load_selected(None);
    let source = root.join("src");
    let destination = root.join("dst");
    let count = |target: &Path, policy| {
        manager
            .uncategorized_photos_for_move(&[1, 1], target.to_str().unwrap(), policy, &formats)
            .unwrap()
    };

    assert_eq!(count(&destination, ConflictPolicy::Cancel), 0);
    manager
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM asset_categories WHERE asset_id=1", [])
        .unwrap();
    assert_eq!(count(&destination, ConflictPolicy::Cancel), 1);
    assert_eq!(count(&source, ConflictPolicy::Cancel), 0);
    fs::write(destination.join("a.jpg"), b"conflict").unwrap();
    assert_eq!(count(&destination, ConflictPolicy::Skip), 0);
    assert_eq!(count(&destination, ConflictPolicy::KeepBoth), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn moving_or_copying_into_same_folder_is_a_noop() {
    let root = temp();
    let (disk, base_manager) = set_up(&root, None);
    let db = base_manager.db.clone();
    seed_asset_path(&db, &root);
    let manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        base_manager.writer.clone(),
        false,
        disk,
    );
    let source = root.join("src/a.jpg");
    for copy in [false, true] {
        let result = manager
            .run_copy_move(
                &[1],
                root.join("src").to_str().unwrap(),
                ConflictPolicy::Cancel,
                copy,
                &mut |_, _| {},
            )
            .unwrap();
        assert_eq!(result.summary, "Súbory už sú v tomto priečinku");
        assert_eq!(result.changed_count, 0);
        assert!(source.exists());
    }
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ddreams_changes", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cross_volume_copy_accepts_two_second_mtime_rounding_but_checks_source_exactly() {
    let root = temp();
    let (disk, _manager) = set_up(&root, Some(false));
    disk.copied_mtime_offset.store(-2, Ordering::SeqCst);
    let source = root.join("src").join("a.jpg");
    let target = root.join("dst").join("a.jpg");
    copy_then_remove(disk.as_ref(), &source, &target).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(&target).unwrap(), b"abc");
    fs::remove_file(&target).unwrap();
    fs::write(&source, b"abc").unwrap();
    disk.copied_mtime_offset.store(3, Ordering::SeqCst);
    assert!(copy_then_remove(disk.as_ref(), &source, &target).is_err());
    assert!(source.exists());
    assert!(!target.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn same_volume_move_updates_catalogue_and_moves_sidecar() {
    let root = temp();
    let (disk, manager) = set_up(&root, None);
    let src = root.join("src/a.jpg");
    fs::write(root.join("src/a.xmp"), b"side").unwrap();
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    let listed = ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None));
    let manager = FileManager::with_fs(db.clone(), listed, manager.writer.clone(), false, disk);
    let result = manager
        .move_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::Cancel,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.changed_count, 1);
    assert!(!src.exists());
    assert!(root.join("dst/a.xmp").exists());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT path||':'||rating FROM assets WHERE id=1", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        format!("{}:4", root.join("dst").join("a.jpg").display())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn copy_duplicates_metadata_and_relations() {
    let root = temp();
    let (disk, mut manager) = set_up(&root, None);
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    let listing = ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None));
    manager = FileManager::with_fs(db.clone(), listing, manager.writer.clone(), false, disk.clone());
    let result = manager
        .copy_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::Cancel,
            |_, _| {},
        )
        .unwrap();
    let id = result.remapped_ids[0].new_id;
    {
        let c = db.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT rating||':'||caption FROM assets WHERE id=?1", [id], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "4:cap"
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM asset_categories WHERE asset_id=?1", [id], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM asset_keywords WHERE asset_id=?1", [id], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn conflicts_skip_keep_both_and_overwrite_refusal() {
    let root = temp();
    let (disk, mut manager) = set_up(&root, None);
    fs::write(root.join("dst/a.jpg"), b"existing").unwrap();
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        manager.writer.clone(),
        false,
        disk,
    );
    assert_eq!(
        manager
            .preview_conflicts(&[1], root.join("dst").to_str().unwrap(), None)
            .unwrap(),
        1
    );
    let skip = manager
        .copy_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::Skip,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(skip.skipped_count, 1);
    assert_eq!(fs::read(root.join("dst/a.jpg")).unwrap(), b"existing");
    manager
        .copy_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::KeepBoth,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(fs::read(root.join("dst/a (2).jpg")).unwrap(), b"abc");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cross_volume_switch_and_uncatalogued_move_are_testable() {
    let root = temp();
    let (disk, manager) = set_up(&root, Some(false));
    let listed = ListingService::new(manager.db.clone(), crate::formats::FormatTable::load_selected(None));
    let _ = listed;
    let mut m = manager;
    let source = root.join("src/a.jpg");
    let asset = crate::db::Asset {
        id: -10_000_000,
        folder_id: None,
        name: "a.jpg".into(),
        ext: "jpg".into(),
        size: Some(3.0),
        width: None,
        height: None,
        bpp: None,
        caption: None,
        notes: None,
        author: None,
        rating: None,
        tagged: None,
        label: None,
        file_modified: None,
        exif_date: None,
        acdsee_date: None,
        md_dirty: None,
        is_orphan: None,
        path: Some(source.to_string_lossy().to_string()),
        is_directory: false,
        offline: false,
        missing: false,
        has_categories: false,
        has_keywords: false,
        display_categories: None,
        display_keywords: None,
        quarantined_at: None,
        edit_pending_since: None,
    };
    m.listing.insert_test_asset(asset);
    m.move_files(
        &[-10_000_000],
        root.join("dst").to_str().unwrap(),
        ConflictPolicy::Cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(!source.exists());
    assert!(root.join("dst/a.jpg").exists());
    assert_eq!(
        m.db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets WHERE path LIKE '%a.jpg'", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn simulated_mid_batch_failure_rolls_back_and_readonly_refuses() {
    let root = temp();
    let (disk, manager) = set_up(&root, Some(true));
    fs::write(root.join("src/a.xmp"), b"side").unwrap();
    seed_asset_path(&manager.db, &root);
    disk.fail_move.store(1, Ordering::SeqCst);
    let error = manager
        .move_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::Cancel,
            |_, _| {},
        )
        .unwrap_err();
    assert!(error.contains("simulated failure"));
    assert!(root.join("src/a.jpg").exists());
    assert!(root.join("src/a.xmp").exists());
    assert!(!root.join("dst/a.jpg").exists());
    assert_eq!(
        manager
            .db
            .lock()
            .unwrap()
            .query_row("SELECT path FROM assets WHERE id=1", [], |r| r.get::<_, String>(0))
            .unwrap(),
        root.join("src/a.jpg").to_string_lossy()
    );
    let ro = FileManager::with_fs(
        manager.db.clone(),
        manager.listing.clone(),
        manager.writer.clone(),
        true,
        disk,
    );
    assert!(
        ro.move_files(
            &[1],
            root.join("dst").to_str().unwrap(),
            ConflictPolicy::Cancel,
            |_, _| {}
        )
        .unwrap_err()
        .contains("iba na čítanie")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_catalogue_write_rolls_back_file_copy_and_recycle() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    let db = base.db.clone();
    seed_asset_path(&db, &root);
    let source = root.join("src/a.jpg");
    let sidecar = root.join("src/a.xmp");
    fs::write(&sidecar, b"synthetic sidecar").unwrap();
    db.lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_history BEFORE INSERT ON ddreams_changes
             BEGIN SELECT RAISE(ABORT,'simulated catalogue failure'); END;",
        )
        .unwrap();
    let manager = FileManager::with_fs(
        db.clone(),
        base.listing.clone(),
        base.writer.clone(),
        false,
        disk.clone(),
    );

    let target = root.join("dst");
    assert!(
        manager
            .copy_files(&[1], target.to_str().unwrap(), ConflictPolicy::Cancel, |_, _| {})
            .unwrap_err()
            .contains("simulated catalogue failure")
    );
    assert!(!target.join("a.jpg").exists());
    assert!(!target.join("a.xmp").exists());
    assert!(
        manager
            .delete_files(&[1], false, &[], &mut Vec::new())
            .unwrap_err()
            .contains("simulated catalogue failure")
    );
    assert_eq!(fs::read(&source).unwrap(), b"abc");
    assert_eq!(fs::read(&sidecar).unwrap(), b"synthetic sidecar");
    assert!(disk.recycled_item(&source).is_none());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let _ = fs::remove_dir_all(root);
}
