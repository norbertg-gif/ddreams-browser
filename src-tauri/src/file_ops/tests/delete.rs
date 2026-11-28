#[test]
fn delete_recycles_main_and_sidecar_and_removes_rows() {
    let root = temp();
    let (disk, manager) = set_up(&root, None);
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    fs::write(root.join("src/a.xmp"), b"side").unwrap();
    let manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        manager.writer.clone(),
        false,
        disk.clone(),
    );
    let result = manager.delete_files(&[1], false, &[], &mut Vec::new()).unwrap();
    assert_eq!(result.summary, "Odstránené do Koša: 1 súbor");
    assert!(!root.join("src/a.jpg").exists());
    assert!(!root.join("src/a.xmp").exists());
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets WHERE id=1", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_delete_record_reports_all_restore_errors_and_restores_later_files() {
    let root = temp();
    let (disk, manager) = set_up(&root, None);
    seed_asset_path(&manager.db, &root);
    let main = root.join("src/a.jpg");
    let xmp = root.join("src/a.xmp");
    let last = root.join("src/a.jpg.xmp");
    fs::write(&xmp, b"side").unwrap();
    fs::write(&last, b"last").unwrap();
    disk.failed_restores.lock().unwrap().extend([main.clone(), xmp.clone()]);
    fail_history(&manager.db);

    let error = manager.delete_files(&[1], false, &[], &mut Vec::new()).unwrap_err();
    assert!(error.contains("simulated catalogue failure"));
    assert!(error.contains(&main.display().to_string()));
    assert!(error.contains(&xmp.display().to_string()));
    assert_eq!(fs::read(&last).unwrap(), b"last");
    assert!(disk.recycled_item(&main).is_some() && disk.recycled_item(&xmp).is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_permanent_delete_record_reports_disk_deletions_and_keeps_ids_for_ai() {
    let root = temp();
    let (_, manager) = set_up(&root, None);
    seed_asset_path(&manager.db, &root);
    let main = root.join("src/a.jpg");
    let xmp = root.join("src/a.xmp");
    fs::write(&xmp, b"side").unwrap();
    fail_history(&manager.db);
    let mut purged = Vec::new();

    let error = manager.delete_files(&[1], true, &[], &mut purged).unwrap_err();
    assert!(error.contains("už boli natrvalo zmazané z disku"));
    assert!(error.contains(&main.display().to_string()) && error.contains(&xmp.display().to_string()));
    assert!(!main.exists() && !xmp.exists());
    assert_eq!(purged, vec![1]);
    assert_eq!(
        manager
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recycle_info_parser_reads_v1_and_v2() {
    let path = "C:\\Foto\\a.jpg";
    let units: Vec<u8> = path
        .encode_utf16()
        .chain(Some(0))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let mut v2 = Vec::new();
    v2.extend(2u64.to_le_bytes());
    v2.extend(5u64.to_le_bytes());
    v2.extend(77u64.to_le_bytes());
    v2.extend(((units.len() / 2) as u32).to_le_bytes());
    v2.extend(&units);
    assert_eq!(parse_recycle_info(&v2), Some((77, path.to_string())));
    let mut v1 = Vec::new();
    v1.extend(1u64.to_le_bytes());
    v1.extend(5u64.to_le_bytes());
    v1.extend(9u64.to_le_bytes());
    let mut fixed = units.clone();
    fixed.resize(520, 0);
    v1.extend(fixed);
    assert_eq!(parse_recycle_info(&v1), Some((9, path.to_string())));
    assert_eq!(parse_recycle_info(&[0; 10]), None);
}

#[test]
fn drive_without_bin_requires_confirmation_before_purging() {
    let root = temp().join("nobin");
    fs::create_dir_all(&root).unwrap();
    let (disk, manager) = set_up(&root, None);
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    fs::write(root.join("src/a.xmp"), b"side").unwrap();
    let manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        manager.writer.clone(),
        false,
        disk.clone(),
    );
    let info = manager.delete_info(&[1], false).unwrap();
    assert_eq!((info.count, info.sidecars, info.permanent_count), (1, 1, 1));
    assert!(
        manager
            .delete_files(&[1], false, &[], &mut Vec::new())
            .unwrap_err()
            .contains("potvrdenie")
    );
    assert!(root.join("src/a.jpg").exists());
    let mut purged = Vec::new();
    let result = manager
        .delete_files(&[1], false, &info.no_bin_drives, &mut purged)
        .unwrap();
    assert_eq!(result.summary, "Zmazané natrvalo: 1 súbor");
    assert_eq!(purged, vec![1]);
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT action FROM ddreams_changes WHERE id=?1",
                [result.change_id.unwrap()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "purge_files"
    );
    assert!(!root.join("src/a.jpg").exists());
    assert!(!root.join("src/a.xmp").exists());
    assert!(disk.recycled_item(&root.join("src/a.jpg")).is_none(), "not in the bin");
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets WHERE id=1", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn shift_delete_is_permanent_even_with_a_bin() {
    let root = temp();
    let (disk, manager) = set_up(&root, None);
    let db = manager.db.clone();
    seed_asset_path(&db, &root);
    let manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        manager.writer.clone(),
        false,
        disk.clone(),
    );
    assert_eq!(manager.delete_info(&[1], true).unwrap().permanent_count, 1);
    assert_eq!(manager.delete_info(&[1], false).unwrap().permanent_count, 0);
    let result = manager.delete_files(&[1], true, &[], &mut Vec::new()).unwrap();
    assert!(result.summary.starts_with("Zmazané natrvalo"));
    assert!(!root.join("src/a.jpg").exists());
    assert!(disk.recycled_item(&root.join("src/a.jpg")).is_none());
    let _ = fs::remove_dir_all(root);
}
