#[test]
fn keyword_assignment_partial_unassign_and_noop() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let added = writer.set_asset_keywords(&[1, 2], &[], 10, true).unwrap();
    assert_eq!(added.changed_count, 1);
    assert_eq!(added.summary, "Pridané kľúčové slovo Vianoce · 1 súbor");
    let after = keyword_rows(&db);
    assert_eq!(after, vec![(1, 10), (2, 10), (2, 12)]);
    let changes = db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let noop = writer.set_asset_keywords(&[1, 2], &[], 10, true).unwrap();
    assert_eq!(noop.change_id, None);
    assert_eq!(noop.changed_count, 0);
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        changes
    );
    let removed = writer.set_asset_keywords(&[1], &[], 10, false).unwrap();
    assert_eq!(removed.summary, "Odobrané kľúčové slovo Vianoce · 1 súbor");
    assert_eq!(keyword_rows(&db), vec![(2, 10), (2, 12)]);
    assert_eq!(
        writer.set_asset_keywords(&[1, 2], &[], 20, true).unwrap().summary,
        "Pridané kľúčové slovo Rodina · 2 súbory"
    );
}

#[test]
fn keyword_create_rename_move_collisions_and_cycles() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    assert_eq!(
        writer.create_keyword(None, " Výlety ").unwrap().summary,
        "Nové kľúčové slovo Výlety"
    );
    let nested = writer.create_keyword(Some(10), "Deti");
    assert!(nested.is_err());
    let nested = writer.create_keyword(Some(10), "Oslava").unwrap();
    let nested_id = nested.keyword_id.unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT parent_id FROM keywords WHERE id=?1", [nested_id], |r| r
                .get::<_, Option<i64>>(0))
            .unwrap(),
        Some(10)
    );
    let renamed = writer.rename_keyword(10, "Sviatky").unwrap();
    assert_eq!(renamed.summary, "Premenované Vianoce → Sviatky");
    assert!(
        writer
            .create_keyword(None, " rodina ")
            .unwrap_err()
            .contains("už existuje kľúčové slovo")
    );
    assert!(
        writer
            .create_keyword(Some(10), " dEtI ")
            .unwrap_err()
            .contains("už existuje kľúčové slovo")
    );
    assert!(writer.move_keyword(10, Some(12)).unwrap_err().contains("podriadeného"));
    writer.move_keyword(11, None).unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT parent_id FROM keywords WHERE id=11", [], |r| r
                .get::<_, Option<i64>>(0))
            .unwrap(),
        None
    );
    writer.move_keyword(20, Some(10)).unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT parent_id FROM keywords WHERE id=20", [], |r| r
                .get::<_, Option<i64>>(0))
            .unwrap(),
        Some(10)
    );
}

#[test]
fn keyword_delete_refuses_descendant_links_then_removes_subtree() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let info = writer.keyword_delete_info(10).unwrap();
    assert_eq!(info.assigned_files, 2);
    assert_eq!(info.subkeywords, 2);
    assert!(writer.delete_keyword(10).unwrap_err().contains("obsahuje 2 súbory"));
    db.lock().unwrap().execute("DELETE FROM asset_keywords", []).unwrap();
    let deleted = writer.delete_keyword(10).unwrap();
    assert_eq!(deleted.deleted_keyword_ids.len(), 3);
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT id FROM keywords", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        20
    );
}

#[test]
fn create_and_assign_keyword_is_one_change() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let count_before = db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let result = writer.create_and_assign_keyword(&[1, 2], &[], None, "Rodinné").unwrap();
    assert!(
        result
            .summary
            .starts_with("Nové kľúčové slovo Rodinné a priradené · 2 súbory")
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        count_before + 1
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM asset_keywords WHERE keyword_id=?1",
                [result.keyword_id.unwrap()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn uncatalogued_keyword_assignment_creates_asset_and_link() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let candidate = ListedAsset {
        temporary_id: -10,
        name: "new.jpg".into(),
        ext: "jpg".into(),
        size: Some(789.0),
        file_modified: None,
        path: "G:\\Camera\\new.jpg".into(),
        folder_path: "G:\\Camera".into(),
    };
    let result = writer.set_asset_keywords(&[-10], &[candidate], 10, true).unwrap();
    let id = result.remapped_ids[0].new_id;
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM asset_keywords WHERE asset_id=?1 AND keyword_id=10",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn readonly_writer_refuses_keyword_mutations() {
    let db = keyword_fixture();
    let writer = CatalogWriter::new(db.clone(), false);
    assert!(
        writer
            .set_asset_keywords(&[1], &[], 10, true)
            .unwrap_err()
            .contains("read-only")
    );
    assert!(writer.create_keyword(None, "Test").unwrap_err().contains("read-only"));
    assert!(writer.rename_keyword(10, "Test").unwrap_err().contains("read-only"));
    assert!(writer.move_keyword(11, None).unwrap_err().contains("read-only"));
    assert!(writer.delete_keyword(10).unwrap_err().contains("read-only"));
    assert_eq!(keyword_rows(&db), vec![(1, 10), (2, 12)]);
}
