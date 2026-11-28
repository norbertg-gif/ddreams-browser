#[test]
fn folder_rewrite_handles_trailing_slashes_and_unicode_prefix_widths() {
    let db = write_fixture();
    {
        let conn = db.lock().unwrap();
        conn.execute("UPDATE folders SET path=?1 WHERE id=2", [r"F:\Kút\"])
            .unwrap();
        conn.execute("UPDATE assets SET path=?1 WHERE id=1", [r"f:/Kút/Sub/Čaj.JPG"])
            .unwrap();
        conn.execute("UPDATE assets SET path=?1 WHERE id=2", [r"F:\Kút_old\Two.jpg"])
            .unwrap();
        conn.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT);")
            .unwrap();
        conn.execute(
            "INSERT INTO ddreams_folder_rules VALUES(?1,'include','now')",
            [r"F:\Kút\Ľudia"],
        )
        .unwrap();
    }
    let writer = CatalogWriter::new(db.clone(), true);
    writer
        .rewrite_folder_paths(r"F:\kút///", r"G:/Nový/", "rename_folder", "Premenovaný priečinok")
        .unwrap();
    let conn = db.lock().unwrap();
    for (sql, expected) in [
        ("SELECT path FROM folders WHERE id=2", r"G:\Nový"),
        ("SELECT name FROM folders WHERE id=2", "Nový"),
        ("SELECT path FROM assets WHERE id=1", r"G:\Nový\Sub\Čaj.JPG"),
        ("SELECT path FROM assets WHERE id=2", r"F:\Kút_old\Two.jpg"),
        ("SELECT path FROM ddreams_folder_rules", r"G:\Nový\Ľudia"),
    ] {
        assert_eq!(conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap(), expected);
    }
}

#[test]
fn exclusion_removes_only_rows_under_the_folder() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    assert_eq!(
        writer.exclusion_impact("f:/photos/").unwrap(),
        ExclusionImpact {
            files: 2,
            with_categories: 1,
            with_keywords: 2,
            with_rating_label_tagged: 2,
            with_color_overrides: 1
        }
    );
    let removed = writer.set_folder_rule("F:\\Photos", Some("exclude")).unwrap();
    assert_eq!(removed, vec![1, 2]);
    let after = dump(&db);
    assert!(
        after.iter().all(|row| !row.starts_with("1:") && !row.starts_with("2:")),
        "{after:?}"
    );
    assert!(
        after.contains(&"4:F:\\Photos_old\\p_1%.jpg:".to_string()),
        "component boundary: Photos_old must stay"
    );
    assert_eq!(
        writer
            .folder_rule_state("F:\\Photos\\sub")
            .unwrap()
            .effective
            .as_deref(),
        Some("exclude")
    );
    assert!(
        writer
            .set_asset_values(
                &[3],
                &[],
                &serde_json::Map::from_iter([("rating".to_string(), serde_json::json!(3))])
            )
            .is_ok()
    );
}

#[test]
fn exclusion_matches_non_ascii_paths() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    assert_eq!(writer.set_folder_rule("f:\\ľudia", Some("exclude")).unwrap(), vec![3]);
}

#[test]
fn excluded_folder_refuses_catalogue_edits() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    writer.set_folder_rule("F:\\Ľudia", Some("include")).unwrap();
    writer.set_folder_rule("F:\\Photos", Some("exclude")).unwrap();
    let listed = ListedAsset {
        temporary_id: -5,
        name: "new.jpg".into(),
        ext: "jpg".into(),
        size: Some(1.0),
        file_modified: None,
        path: "F:\\Photos\\new.jpg".into(),
        folder_path: "F:\\Photos".into(),
    };
    assert_eq!(
        writer.set_asset_categories(&[-5], &[listed], 11, true).unwrap_err(),
        "Priečinok je vylúčený z katalógu."
    );
}

#[test]
fn remove_catalog_source_deletes_subtree_rows_and_never_creates_journal_entry() {
    let db = scope_fixture();
    let writer = CatalogWriter::new(db.clone(), true);
    let result = writer.remove_folder_from_catalog("F:\\Photos").unwrap();
    assert_eq!(result.removed_assets, 2);
    assert_eq!(result.removed_folders, 1);
    assert_eq!(result.change_id, None);
    let conn = db.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM assets WHERE id IN (1,2)", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM asset_categories WHERE asset_id IN (1,2)",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM folders WHERE lower(path) LIKE lower(?1)",
            ["F:\\Photos%"],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM assets WHERE id=4", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
