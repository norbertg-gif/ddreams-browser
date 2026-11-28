#[test]
fn auto_catalog_adds_listed_files_once() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let listed = |id: i64, name: &str| ListedAsset {
        temporary_id: id,
        name: name.into(),
        ext: "jpg".into(),
        size: Some(5.0),
        file_modified: None,
        path: format!("F:\\Inbox\\{name}"),
        folder_path: "F:\\Inbox".into(),
    };
    let date = "2026-08-31T16:16:00";
    let remaps = writer
        .catalog_listed_assets_with_dates(
            &[listed(-10, "a.jpg"), listed(-11, "b.jpg")],
            &std::collections::HashMap::from([(-10, date.to_owned())]),
        )
        .unwrap();
    assert_eq!(remaps.len(), 2);
    let stored: String = db
        .lock()
        .unwrap()
        .query_row("SELECT exif_date FROM assets WHERE id=?1", [remaps[0].new_id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored, date);
    assert!(
        writer
            .catalog_listed_assets(&[listed(-12, "a.jpg")])
            .unwrap()
            .is_empty(),
        "already catalogued by path"
    );
    let summary: String = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT summary FROM ddreams_changes ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(summary, "Pridané do katalógu: 2 súbory");
}

#[test]
fn backfill_capture_dates_updates_only_missing_values() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    db.lock()
        .unwrap()
        .execute_batch(
            "UPDATE assets SET exif_date='' WHERE id=2;
            UPDATE assets SET exif_date='2025-01-01T00:00:00' WHERE id=3",
        )
        .unwrap();
    let new_date = "2026-08-31T16:16:00";
    let dates = [1, 2, 3].into_iter().map(|id| (id, new_date.to_owned())).collect();
    assert!(
        CatalogWriter::new(db.clone(), false)
            .backfill_capture_dates(&dates)
            .is_err()
    );
    assert_eq!(writer.backfill_capture_dates(&dates).unwrap(), 2);
    assert_eq!(writer.backfill_capture_dates(&dates).unwrap(), 0);
    let conn = db.lock().unwrap();
    for (id, expected) in [(1, new_date), (2, new_date), (3, "2025-01-01T00:00:00")] {
        let stored: String = conn
            .query_row("SELECT exif_date FROM assets WHERE id=?1", [id], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, expected);
    }
}
