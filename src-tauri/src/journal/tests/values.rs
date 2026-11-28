#[test]
fn set_asset_text_applies_catalogue_only_and_records_history() {
    let db = write_fixture();
    db.lock()
        .unwrap()
        .execute("DROP TABLE ddreams_change_items", [])
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let initial = writer
        .set_asset_text(&[1, 2], &[], AssetTextField::Caption, Some("Ľúka\r\nJeseň".into()))
        .unwrap();
    assert_eq!(initial.value.as_deref(), Some("Ľúka\nJeseň"));
    assert!(initial.result.change_id.is_some());
    let repeat = writer
        .set_asset_text(&[1, 2], &[], AssetTextField::Caption, initial.value)
        .unwrap();
    assert!(repeat.result.change_id.is_none());
    let cleared = writer
        .set_asset_text(&[1], &[], AssetTextField::Caption, Some(" ".into()))
        .unwrap();
    assert!(cleared.value.is_none());
    let conn = db.lock().unwrap();
    let values = conn
        .query_row(
            "SELECT (SELECT caption FROM assets WHERE id=1),(SELECT caption FROM assets WHERE id=2)",
            [],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(values, (None, "Ľúka\nJeseň".into()));
    let action = conn
        .query_row(
            "SELECT action FROM ddreams_changes WHERE id=?1",
            [cleared.result.change_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(action, "set_asset_text");
}

#[test]
fn set_asset_text_remaps_listed_ids_and_validates_before_writing() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let candidate = ListedAsset {
        temporary_id: -10,
        name: "new.jpg".into(),
        ext: "jpg".into(),
        size: None,
        file_modified: None,
        path: "F:\\Photos\\new.jpg".into(),
        folder_path: "F:\\Photos".into(),
    };
    let result = writer
        .set_asset_text(
            &[-10],
            std::slice::from_ref(&candidate),
            AssetTextField::Author,
            Some("Žanet".into()),
        )
        .unwrap();
    let remap = &result.result.remapped_ids[0];
    assert_eq!(remap.old_id, -10);
    let value = db
        .lock()
        .unwrap()
        .query_row("SELECT author FROM assets WHERE id=?1", [remap.new_id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert_eq!(value, "Žanet");
    assert!(
        writer
            .set_asset_text(&[1], &[], AssetTextField::Author, Some("Ž".repeat(201)))
            .is_err()
    );
    assert!(
        writer
            .set_asset_text(&[1], &[], AssetTextField::Caption, Some("X".repeat(2001)))
            .is_err()
    );
    assert!(
        CatalogWriter::new(db, false)
            .set_asset_text(&[1], &[], AssetTextField::Author, None)
            .is_err()
    );
}

#[test]
fn multi_file_writes_allocate_distinct_ids_and_keep_remaps() {
    for operation in ["values", "category", "keyword", "create_keyword"] {
        let db = keyword_fixture();
        let writer = CatalogWriter::new(db.clone(), true);
        let listed =
            [(-10, "Camera", "a.jpg"), (-11, "Mobile", "b.jpg")].map(|(temporary_id, folder, name)| ListedAsset {
                temporary_id,
                name: name.into(),
                ext: "jpg".into(),
                size: Some(5.0),
                file_modified: None,
                path: format!(r"G:\{folder}\{name}"),
                folder_path: format!(r"G:\{folder}"),
            });
        let ids = [-10, -11];
        let remaps = match operation {
            "values" => {
                writer
                    .set_asset_values(
                        &ids,
                        &listed,
                        &serde_json::Map::from_iter([("rating".into(), Value::from(3))]),
                    )
                    .unwrap()
                    .remapped_ids
            }
            "category" => {
                writer
                    .set_asset_categories(&ids, &listed, 11, true)
                    .unwrap()
                    .remapped_ids
            }
            "keyword" => writer.set_asset_keywords(&ids, &listed, 10, true).unwrap().remapped_ids,
            _ => {
                writer
                    .create_and_assign_keyword(&ids, &listed, None, "Nové")
                    .unwrap()
                    .remapped_ids
            }
        };
        assert_eq!(
            remaps.iter().map(|r| (r.old_id, r.new_id)).collect::<Vec<_>>(),
            [(-11, 3), (-10, 4)],
            "{operation}"
        );
        let conn = db.lock().unwrap();
        let sql = match operation {
            "values" => "SELECT count(*) FROM assets WHERE id IN (3,4) AND rating=3",
            "category" => "SELECT count(*) FROM asset_categories WHERE asset_id IN (3,4) AND category_id=11",
            "keyword" => "SELECT count(*) FROM asset_keywords WHERE asset_id IN (3,4) AND keyword_id=10",
            _ => {
                "SELECT count(*) FROM asset_keywords WHERE asset_id IN (3,4) AND keyword_id=(SELECT id FROM keywords WHERE name='Nové')"
            }
        };
        assert_eq!(
            conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap(),
            2,
            "{operation}"
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM folder_roots WHERE name='G:'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "{operation}"
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(DISTINCT folder_id) FROM assets WHERE id IN (3,4)",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2,
            "{operation}"
        );
    }
}

#[test]
fn label_writes_notify_both_kinds_and_keep_legacy_history_details() {
    let db = keyword_fixture();
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO ddreams_changes VALUES(1,'2000-01-01','old','Starý záznam',1);
             INSERT INTO ddreams_change_items VALUES(1,'assets','1','rating','old','new');",
        )
        .unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = calls.clone();
    writer.set_change_hook(Arc::new(move |kinds| {
        observed
            .lock()
            .unwrap()
            .extend(kinds.iter().map(|kind| kind.to_string()))
    }));
    writer.set_asset_categories(&[1], &[], 11, true).unwrap();
    writer.set_asset_keywords(&[2], &[], 10, true).unwrap();
    writer.set_asset_keywords(&[2], &[], 10, true).unwrap();
    assert_eq!(*calls.lock().unwrap(), ["category", "keyword"]);
    assert_eq!(writer.recent_changes(5).unwrap().len(), 2);
    let conn = db.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM ddreams_change_items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn color_override_updates_only_listed_assets() {
    let db = write_fixture();
    db.lock().unwrap().execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER NOT NULL,color TEXT NOT NULL,state INTEGER NOT NULL,PRIMARY KEY(asset_id,color));").unwrap();
    let writer = CatalogWriter::new(db.clone(), true);
    assert_eq!(
        writer
            .set_asset_colors(&[1, 999], "zelená", Some(true), "Farba: zelená")
            .unwrap(),
        1
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row(
                "SELECT state FROM asset_color_overrides WHERE asset_id=1 AND color='zelená'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn set_values_updates_one_and_many_assets_with_one_journal_change() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let changes =
        serde_json::Map::from_iter([("rating".into(), Value::from(5)), ("label".into(), Value::from("Blue"))]);
    let result = writer.set_asset_values(&[1, 2, 1], &[], &changes).unwrap();
    assert!(result.summary.contains("Hodnotenie 5"));
    assert!(result.summary.contains("2 súbory"));
    let connection = db.lock().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM ddreams_changes", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM assets WHERE rating=5 AND label='Blue'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    drop(connection);
    let single_changes = serde_json::Map::from_iter([("rating".into(), Value::from(1))]);
    let single = writer.set_asset_values(&[1], &[], &single_changes).unwrap();
    assert!(single.summary.contains("1 súbor"));
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT rating FROM assets WHERE id=1", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn set_values_toggle_tags_together_for_a_mixed_selection() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let changes = serde_json::Map::from_iter([("tagged".into(), Value::from("toggle"))]);
    writer.set_asset_values(&[1, 2], &[], &changes).unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets WHERE tagged=1", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    writer.set_asset_values(&[1, 2], &[], &changes).unwrap();
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM assets WHERE tagged=0", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn uncatalogued_listed_asset_is_inserted_with_its_folder() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let candidate = ListedAsset {
        temporary_id: -10,
        name: "new.jpg".into(),
        ext: "jpg".into(),
        size: Some(789.0),
        file_modified: Some("2024-03-04T00:00:00Z".into()),
        path: "G:\\Camera\\new.jpg".into(),
        folder_path: "G:\\Camera".into(),
    };
    let changes = serde_json::Map::from_iter([("rating".into(), Value::from(3))]);
    let result = writer.set_asset_values(&[-10], &[candidate], &changes).unwrap();
    let inserted = result.remapped_ids[0].new_id;
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT name||':'||rating FROM assets WHERE id=?1", [inserted], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "new.jpg:3"
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM folders WHERE path=?1", [r"G:\Camera"], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn readonly_writer_refuses_set_asset_values() {
    let db = write_fixture();
    let writer = CatalogWriter::new(db.clone(), false);
    let changes = serde_json::Map::from_iter([("rating".into(), Value::from(4))]);
    assert!(
        writer
            .set_asset_values(&[1], &[], &changes)
            .unwrap_err()
            .contains("read-only")
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT rating FROM assets WHERE id=1", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
