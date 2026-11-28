#[test]
fn rename_case_only_keeps_main_and_sidecar() {
    let root = temp();
    let (disk, base_manager) = set_up(&root, None);
    let db = base_manager.db.clone();
    seed_asset_path(&db, &root);
    fs::write(root.join("src/a.xmp"), b"side").unwrap();
    let manager = FileManager::with_fs(
        db.clone(),
        ListingService::new(db.clone(), crate::formats::FormatTable::load_selected(None)),
        base_manager.writer.clone(),
        false,
        disk,
    );
    let result = manager.rename_file(1, "A", |_, _| {}).unwrap();
    assert!(root.join("src/A.jpg").exists());
    assert!(root.join("src/A.xmp").exists());
    assert!(result.summary.contains("a.jpg"));
    let _ = fs::remove_dir_all(root);
}
