#[test]
fn failed_folder_record_removes_copy_only_when_source_is_safe() {
    for (copy, same_volume, fail_restore) in [
        (true, true, false),
        (false, false, false),
        (false, false, true),
        (false, true, false),
    ] {
        let root = temp();
        let (disk, manager) = set_up(&root, Some(same_volume));
        let source = root.join("src");
        let parent = root.join("nobin");
        let dest = parent.join("src");
        fs::create_dir(&parent).unwrap();
        fail_history(&manager.db);
        if fail_restore {
            disk.failed_restores.lock().unwrap().insert(source.clone());
        }

        let error = manager
            .transfer_folder(source.to_str().unwrap(), parent.to_str().unwrap(), copy, &mut |_, _| {})
            .unwrap_err();
        assert!(error.contains("simulated catalogue failure"), "{error}");
        assert_eq!(error.contains("simulated restore failure"), fail_restore);
        assert_eq!(source.join("a.jpg").is_file(), !fail_restore);
        assert_eq!(dest.join("a.jpg").is_file(), fail_restore);
        assert_eq!(fs::read_dir(&disk.recycle).unwrap().count(), usize::from(fail_restore));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn folder_rename_rewrites_catalogue_paths_and_rules() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    let db = base.db.clone();
    let source = root.join("src");
    let db_path = source.to_string_lossy().to_string();
    {
        let c = db.lock().unwrap();
        c.execute(
            "UPDATE assets SET path=?1",
            [source.join("a.jpg").to_string_lossy().to_string()],
        )
        .unwrap();
        c.execute("UPDATE folders SET path=?1 WHERE id=2", [&db_path]).unwrap();
        c.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT);")
            .unwrap();
        c.execute(
            "INSERT INTO ddreams_folder_rules VALUES(?1,'include','now')",
            [&db_path],
        )
        .unwrap();
    }
    let manager = FileManager::with_fs(db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    let target = root.join("renamed");
    manager.rename_folder(&db_path, "renamed").unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT path FROM ddreams_folder_rules", [], |r| r.get::<_, String>(0))
            .unwrap(),
        target.to_string_lossy()
    );
    assert!(target.join("a.jpg").exists());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT path FROM assets WHERE id=1", [], |r| r.get::<_, String>(0))
            .unwrap()
            .to_lowercase(),
        crate::journal::normalize_fs_path(&target.join("a.jpg").to_string_lossy())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn folder_rename_refuses_invalid_names_and_existing_targets() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    fs::create_dir(root.join("dst/src")).unwrap();
    let manager = FileManager::with_fs(base.db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    assert!(
        manager
            .rename_folder(root.join("src").to_str().unwrap(), "bad/name")
            .is_err()
    );
    assert!(
        manager
            .rename_folder(root.join("src").to_str().unwrap(), "bad.")
            .is_err()
    );
    assert!(
        manager
            .rename_folder(root.join("src").to_str().unwrap(), "bad ")
            .is_err()
    );
    assert!(manager.rename_folder("C:\\", "x").is_err());
    assert!(
        manager
            .rename_folder(root.join("src").to_str().unwrap(), "dst")
            .is_err()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn folder_delete_recycles_and_removes_rows() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    let db = base.db.clone();
    let source = root.join("src");
    {
        let c = db.lock().unwrap();
        c.execute(
            "UPDATE assets SET path=?1",
            [source.join("a.jpg").to_string_lossy().to_string()],
        )
        .unwrap();
        c.execute(
            "UPDATE folders SET path=?1 WHERE id=2",
            [source.to_string_lossy().to_string()],
        )
        .unwrap();
    }
    let manager = FileManager::with_fs(db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    manager.delete_folder(source.to_str().unwrap()).unwrap();
    assert!(!source.exists());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn folder_copy_has_no_catalogue_rows_and_refuses_conflicts_and_descendants() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    let manager = FileManager::with_fs(base.db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    let source = root.join("src");
    let target = root.join("dst");
    manager
        .copy_folder(source.to_str().unwrap(), target.to_str().unwrap())
        .unwrap();
    assert!(target.join("src/a.jpg").exists());
    assert_eq!(
        base.db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        manager
            .copy_folder(source.to_str().unwrap(), target.to_str().unwrap())
            .is_err()
    );
    assert!(
        manager
            .move_folder(source.to_str().unwrap(), source.join("child").to_str().unwrap())
            .is_err()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn same_volume_folder_move_rewrites_paths() {
    let root = temp();
    let (disk, base) = set_up(&root, None);
    let db = base.db.clone();
    let source = root.join("src");
    let dest = root.join("dst/src");
    {
        let c = db.lock().unwrap();
        c.execute(
            "UPDATE assets SET path=?1",
            [source.join("a.jpg").to_string_lossy().to_string()],
        )
        .unwrap();
        c.execute(
            "UPDATE folders SET path=?1 WHERE id=2",
            [source.to_string_lossy().to_string()],
        )
        .unwrap();
    }
    let manager = FileManager::with_fs(db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    manager
        .move_folder(source.to_str().unwrap(), root.join("dst").to_str().unwrap())
        .unwrap();
    assert!(dest.join("a.jpg").exists());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT path FROM assets WHERE id=1", [], |r| r.get::<_, String>(0))
            .unwrap()
            .to_lowercase(),
        crate::journal::normalize_fs_path(&dest.join("a.jpg").to_string_lossy())
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn folder_delete_refuses_when_recycle_bin_is_unavailable() {
    let root = temp().join("nobin");
    fs::create_dir_all(&root).unwrap();
    let (disk, base) = set_up(&root, None);
    let manager = FileManager::with_fs(base.db.clone(), base.listing.clone(), base.writer.clone(), false, disk);
    assert!(manager.delete_folder(root.join("src").to_str().unwrap()).is_err());
    assert!(root.join("src/a.jpg").exists());
    let _ = fs::remove_dir_all(root.parent().unwrap());
}
