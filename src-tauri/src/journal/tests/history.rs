#[test]
fn deletion_suggestions_are_batched_and_preserve_keep_decisions() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute("ALTER TABLE assets ADD COLUMN delete_suggestion INTEGER", [])
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let formats = crate::formats::FormatTable::load_selected(None);
    let state = |id| {
        db.lock()
            .unwrap()
            .query_row("SELECT delete_suggestion FROM assets WHERE id=?1", [id], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .unwrap()
    };
    let folder = r"F:\Photos";
    let batch = [(1, true), (2, true)];
    assert!(
        writer
            .suggest_deletions(r"F:\Elsewhere", &batch, &formats)
            .unwrap()
            .change_id
            .is_none()
    );
    assert!(
        writer
            .suggest_deletions(folder, &batch, &formats)
            .unwrap()
            .change_id
            .is_some()
    );
    assert_eq!((state(1), state(2)), (Some(1), Some(1)));
    writer.keep_suggested_assets(Some(&[1])).unwrap();
    assert_eq!((state(1), state(2)), (Some(0), Some(1)));
    assert!(
        writer
            .suggest_deletions(folder, &batch, &formats)
            .unwrap()
            .change_id
            .is_none()
    );
    writer.keep_suggested_assets(None).unwrap();
    assert_eq!((state(1), state(2)), (Some(0), Some(0)));
    assert!(
        CatalogWriter::new(db.clone(), false)
            .keep_suggested_assets(None)
            .is_err()
    );
}
