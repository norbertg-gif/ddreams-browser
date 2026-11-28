#[test]
fn category_assignment_is_partial_idempotent_and_removable() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    db.lock()
        .unwrap()
        .execute("INSERT INTO asset_categories VALUES(1,11)", [])
        .unwrap();
    let assigned = writer.set_asset_categories(&[1, 2], &[], 11, true).unwrap();
    assert_eq!(assigned.changed_count, 1);
    assert_eq!(assigned.summary, "Zaradené do Rodina › Deti · 1 súbor");
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM asset_categories WHERE category_id=11", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
    let journal_count = db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let no_op = writer.set_asset_categories(&[1, 2], &[], 11, true).unwrap();
    assert_eq!(no_op.change_id, None);
    assert_eq!(no_op.changed_count, 0);
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        journal_count
    );
    let removed = writer.set_asset_categories(&[1, 2], &[], 11, false).unwrap();
    assert_eq!(removed.changed_count, 2);
    assert!(removed.summary.contains("Vyradené z Rodina › Deti · 2 súbory"));
}

#[test]
fn category_assignment_multi_summary_has_correct_slovak_plural_and_one_journal_row() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let result = writer.set_asset_categories(&[1, 2], &[], 10, true).unwrap();
    assert_eq!(result.summary, "Zaradené do Rodina · 2 súbory");
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn category_assignment_of_uncatalogued_file_creates_link() {
    let db = write_fixture();
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
    let result = writer.set_asset_categories(&[-10], &[candidate], 11, true).unwrap();
    let id = result.remapped_ids[0].new_id;
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM asset_categories WHERE asset_id=?1 AND category_id=11",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn readonly_writer_refuses_category_changes() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), false);
    assert!(
        writer
            .set_asset_categories(&[1], &[], 11, true)
            .unwrap_err()
            .contains("read-only")
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM asset_categories", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn category_summary_uses_slovenian_plural_forms() {
    assert_eq!(slovak_count(1, "súbor", "súbory", "súborov"), "súbor");
    assert_eq!(slovak_count(2, "súbor", "súbory", "súborov"), "súbory");
    assert_eq!(slovak_count(4, "súbor", "súbory", "súborov"), "súbory");
    assert_eq!(slovak_count(5, "súbor", "súbory", "súborov"), "súborov");
}

#[test]
fn create_top_level_and_subcategory_stores_hierarchy() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let top = writer.create_category(None, "Nové", None).unwrap();
    let top_id = top.category_id.unwrap();
    let root_id = top.root_id.unwrap();
    assert_eq!(top.summary, "Nová kategória Nové");
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT name FROM category_roots WHERE id=?1", [root_id], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Nové"
    );
    let sub = writer.create_category(Some(10), " Vnúčatá ", None).unwrap();
    let sub_id = sub.category_id.unwrap();
    assert_eq!(sub.summary, "Nová kategória Rodina › Vnúčatá");
    assert_eq!(sub.root_id, Some(1));
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT parent_id FROM categories WHERE id=?1", [sub_id], |r| r
                .get::<_, Option<i64>>(0))
            .unwrap(),
        Some(10)
    );
    assert!(top_id > 0);
}

#[test]
fn category_names_are_trimmed_validated_and_unique_case_insensitively() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    assert!(
        writer
            .create_category(Some(10), " dEtI ", None)
            .unwrap_err()
            .contains("už kategória")
    );
    assert!(writer.create_category(None, "rodina", None).is_err());
    assert!(
        writer
            .create_category(Some(10), "  ", None)
            .unwrap_err()
            .contains("nesmie byť prázdny")
    );
    assert!(
        writer
            .create_category(None, &"x".repeat(101), None)
            .unwrap_err()
            .contains("100 znakov")
    );
    let created = writer.create_category(Some(10), "  Nové meno  ", None).unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT name FROM categories WHERE id=?1",
                [created.category_id.unwrap()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Nové meno"
    );
}

#[test]
fn renaming_top_level_keeps_root_pair_consistent() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let result = writer.rename_category(10, "Domov").unwrap();
    assert_eq!(result.summary, "Premenované Rodina → Domov");
    {
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT name FROM categories WHERE id=10", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "Domov"
        );
        assert_eq!(
            conn.query_row("SELECT name FROM category_roots WHERE id=1", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "Domov"
        );
    }
}

#[test]
fn moving_subcategory_to_top_creates_root_and_updates_entire_subtree() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",
            [],
        )
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let result = writer.move_category(11, None).unwrap();
    let new_root = result.root_id.unwrap();
    let conn = db.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT parent_id FROM categories WHERE id=11", [], |r| r
            .get::<_, Option<i64>>(0))
            .unwrap(),
        None
    );
    assert_eq!(
        conn.query_row("SELECT root_id FROM categories WHERE id=11", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        new_root
    );
    assert_eq!(
        conn.query_row("SELECT root_id FROM categories WHERE id=12", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        new_root
    );
    assert_eq!(
        conn.query_row("SELECT name FROM category_roots WHERE id=?1", [new_root], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Deti"
    );
    drop(conn);
}

#[test]
fn category_icons_set_clear_and_keep_root_pair() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let icon = |table: &str, id: i64| {
        db.lock()
            .unwrap()
            .query_row(&format!("SELECT icon_key FROM {table} WHERE id=?1"), [id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .unwrap()
    };
    assert!(
        writer
            .set_category_icon(11, false, Some("camera"))
            .unwrap()
            .change_id
            .is_some()
    );
    assert_eq!(icon("categories", 11).as_deref(), Some("camera"));
    assert!(
        writer
            .set_category_icon(11, false, Some("camera"))
            .unwrap()
            .change_id
            .is_none()
    );
    writer.set_category_icon(11, false, None).unwrap();
    assert_eq!(icon("categories", 11), None);

    writer.set_category_icon(1, true, Some("people")).unwrap();
    assert_eq!(icon("category_roots", 1).as_deref(), Some("people"));
    assert_eq!(icon("categories", 10).as_deref(), Some("people"));

    db.lock()
        .unwrap()
        .execute("INSERT INTO category_roots(id,name) VALUES(2,'Miesta')", [])
        .unwrap();
    writer.set_category_icon(2, true, Some("places")).unwrap();
    assert_eq!(icon("category_roots", 2).as_deref(), Some("places"));

    let readonly = CatalogWriter::new(db.clone(), false);
    assert!(
        readonly
            .set_category_icon(11, false, Some("camera"))
            .unwrap_err()
            .contains("read-only")
    );
    assert!(writer.set_category_icon(11, false, Some("unknown")).is_err());
}

#[test]
fn create_category_stores_icon_and_delete_removes_empty_root() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let created = writer.create_category(None, "Výlety", Some("travel")).unwrap();
    let category_id = created.category_id.unwrap();
    let root_id = created.root_id.unwrap();
    let root_icon = || {
        db.lock()
            .unwrap()
            .query_row("SELECT icon_key FROM category_roots WHERE id=?1", [root_id], |row| {
                row.get::<_, String>(0)
            })
    };
    assert_eq!(root_icon().unwrap(), "travel");
    writer.delete_category(category_id).unwrap();
    assert!(matches!(root_icon(), Err(rusqlite::Error::QueryReturnedNoRows)));
}

#[test]
fn moving_top_level_under_another_category_removes_empty_root() {
    let db = write_fixture();
    db.lock().unwrap().execute_batch("INSERT INTO category_roots(id,name) VALUES(2,'Priatelia');INSERT INTO categories(id,parent_id,root_id,name) VALUES(20,NULL,2,'Priatelia'),(21,20,2,'Blízki'),(12,11,1,'Mladí');").unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    writer.move_category(10, Some(21)).unwrap();
    {
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT parent_id||':'||root_id FROM categories WHERE id=10", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "21:2"
        );
        assert_eq!(
            conn.query_row("SELECT root_id FROM categories WHERE id=12", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM category_roots WHERE id=1", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn category_move_rejects_self_and_descendant_cycles() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",
            [],
        )
        .unwrap();
    let writer = CatalogWriter::new(db, true);
    assert!(writer.move_category(11, Some(11)).unwrap_err().contains("seba"));
    assert!(writer.move_category(10, Some(12)).unwrap_err().contains("podkategórie"));
}

#[test]
fn delete_refuses_assigned_files_in_descendants_and_reports_count() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute("INSERT INTO asset_categories VALUES(1,11)", [])
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let info = writer.category_delete_info(10).unwrap();
    assert_eq!(info.assigned_files, 1);
    assert_eq!(info.subcategories, 1);
    let error = writer.delete_category(10).unwrap_err();
    assert!(error.contains("obsahuje 1 súbor ("));
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM categories WHERE id IN (10,11)", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn delete_subtree_keeps_parent_category() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",
            [],
        )
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let deleted = writer.delete_category(11).unwrap();
    assert_eq!(deleted.deleted_category_ids.len(), 2);
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM categories WHERE id IN (11,12)", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn readonly_writer_refuses_category_structure_changes() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), false);
    assert!(
        writer
            .create_category(None, "Test", None)
            .unwrap_err()
            .contains("read-only")
    );
    assert!(writer.rename_category(10, "Test").unwrap_err().contains("read-only"));
    assert!(writer.move_category(11, None).unwrap_err().contains("read-only"));
    assert!(writer.delete_category(11).unwrap_err().contains("read-only"));
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM categories", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
