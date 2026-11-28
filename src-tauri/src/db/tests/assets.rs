#[cfg(test)]
mod feature_tests {
    use super::{
        Asset, AssetFilter, AssignmentFilter, CategoryPath, Db, asset_catalog_links, asset_filter_matches,
        asset_ids_page, assets_page, catalog_tree_search, compare_assets, folder_previews, folders_page,
        keyword_search_page, locate_asset, migrate_category_icons, nodes_page, roots_page, valid_date_sql, values_page,
    };
    use crate::formats::FormatTable;
    use rusqlite::{Connection, params};
    use std::sync::{Arc, Mutex};

    use std::cmp::Ordering;
    fn asset(id: i64, name: &str, size: Option<f64>, rating: Option<i64>) -> Asset {
        Asset {
            id,
            folder_id: Some(1),
            name: name.into(),
            ext: "jpg".into(),
            size,
            width: Some(10),
            height: Some(10),
            bpp: None,
            caption: None,
            notes: None,
            author: None,
            rating,
            tagged: Some(0),
            label: None,
            file_modified: None,
            exif_date: None,
            acdsee_date: None,
            md_dirty: None,
            is_orphan: None,
            path: None,
            is_directory: false,
            offline: false,
            missing: false,
            has_categories: false,
            has_keywords: false,
            display_categories: None,
            display_keywords: None,
            quarantined_at: None,
            edit_pending_since: None,
        }
    }
    #[test]
    fn filter_logic_supports_modes_and_groups() {
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let unrated = asset(1, "a.jpg", None, None);
        let rated = asset(2, "b.jpg", Some(10.0), Some(4));
        let mut f = AssetFilter {
            kind: "folder".into(),
            id: None,
            root_id: None,
            include_children: false,
            folder_path: None,
            ids: None,
            value: None,
            query: None,
            sort: "name".into(),
            descending: false,
            offset: 0,
            limit: 100,
            rating_mode: Some("atLeast".into()),
            rating_value: Some(3),
            label_filter: None,
            tagged_only: false,
            category_filter: None,
            keyword_filter: None,
            include_categories: false,
            include_keywords: false,
            date_from: None,
            date_to: None,
            date_source: None,
            date_no_date: false,
            file_group: Some("images".into()),
        };
        assert!(asset_filter_matches(&rated, &f, &formats));
        assert!(!asset_filter_matches(&unrated, &f, &formats));
        f.rating_mode = Some("unrated".into());
        assert!(asset_filter_matches(&unrated, &f, &formats));
        assert!(!asset_filter_matches(&rated, &f, &formats));
    }

    #[test]
    fn assignment_filters_agree_between_catalogue_and_listed_assets() {
        use AssignmentFilter::{With, Without};
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO assets(id,name,ext,ext_lower,is_listed,tagged,size) VALUES
                (1,'1.jpg','jpg','jpg',1,1,10),(2,'2.jpg','jpg','jpg',1,0,10),
                (3,'3.jpg','jpg','jpg',1,1,10),(4,'4.jpg','jpg','jpg',1,0,10);
             INSERT INTO asset_categories VALUES (1,10),(2,10);
             INSERT INTO asset_keywords VALUES (1,20),(3,20);",
            )
            .unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut filter = test_filter("special", "all_photos");
        let catalogue = assets_page(&db, &filter, &formats).unwrap().items;
        let mut listed = catalogue.clone();
        listed.push(asset(-1, "new.jpg", Some(10.0), None));
        for (category, keyword, tagged, catalogue_ids, includes_new) in [
            (Some(With), None, false, &[1, 2][..], false),
            (Some(Without), None, false, &[3, 4], true),
            (None, Some(With), false, &[1, 3], false),
            (None, Some(Without), false, &[2, 4], true),
            (Some(With), Some(Without), false, &[2], false),
            (Some(Without), Some(Without), false, &[4], true),
            (Some(Without), Some(With), true, &[3], false),
        ] {
            filter.category_filter = category;
            filter.keyword_filter = keyword;
            filter.tagged_only = tagged;
            let page = assets_page(&db, &filter, &formats).unwrap();
            assert_eq!(page.total as usize, catalogue_ids.len());
            assert_eq!(page.items.iter().map(|item| item.id).collect::<Vec<_>>(), catalogue_ids);
            assert_eq!(asset_ids_page(&db, &filter, &formats, 0, 10).unwrap(), catalogue_ids);
            let matches = listed
                .iter()
                .filter(|item| asset_filter_matches(item, &filter, &formats))
                .map(|item| item.id)
                .collect::<Vec<_>>();
            let expected = catalogue_ids
                .iter()
                .copied()
                .chain(includes_new.then_some(-1))
                .collect::<Vec<_>>();
            assert_eq!(matches, expected);
        }
    }
    #[test]
    fn sort_keeps_empty_values_last_in_both_directions() {
        let empty = asset(1, "empty", None, None);
        let full = asset(2, "full", Some(5.0), Some(4));
        for desc in [false, true] {
            assert_eq!(compare_assets(&full, &empty, "size", desc), Ordering::Less);
        }
    }
    #[test]
    fn date_sorts_prefer_capture_time_to_newer_mtime() {
        let mut later = asset(1, "a.jpg", None, None);
        later.exif_date = Some("2026-08-31T17:00:00".into());
        let mut copied = asset(2, "b.dng", None, None);
        copied.file_modified = Some("2026-11-24T00:00:00Z".into());
        copied.exif_date = crate::incoming::catalog_capture_date("2026-08-31 16:16:00");
        for sort in ["date", "dateTaken", "captureTime"] {
            assert_eq!(compare_assets(&copied, &later, sort, false), Ordering::Less);
            assert_eq!(compare_assets(&copied, &later, sort, true), Ordering::Greater);
        }
        copied.exif_date = Some(String::new());
        assert_eq!(compare_assets(&copied, &later, "date", false), Ordering::Greater);
        assert_eq!(compare_assets(&copied, &later, "captureTime", true), Ordering::Greater);
    }
    fn test_db() -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);CREATE TABLE categories(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT);CREATE TABLE category_roots(id INTEGER PRIMARY KEY,name TEXT);CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));CREATE INDEX idx_asset_categories_category_asset ON asset_categories(category_id,asset_id);CREATE INDEX idx_asset_keywords_keyword_asset ON asset_keywords(keyword_id,asset_id);CREATE INDEX idx_categories_parent_id ON categories(parent_id);CREATE INDEX idx_keywords_parent_id ON keywords(parent_id);").unwrap();
        Arc::new(Mutex::new(c))
    }
    #[test]
    fn invalid_calendar_dates_are_included_in_no_date() {
        let conn = Connection::open_in_memory().unwrap();
        let valid = valid_date_sql("?1");
        for (date, expected) in [
            (Some("2020-02-29T12:00:00"), true),
            (Some("2020-02-31"), false),
            (Some("1900-02-29"), false),
            (Some("2020-04-31"), false),
            (Some("2020-2x-01"), false),
            (None, false),
        ] {
            let undated = conn
                .query_row(&format!("SELECT NOT ({valid})"), [date], |row| row.get::<_, bool>(0))
                .unwrap();
            assert_eq!(undated, !expected, "{date:?}");
        }
    }

    #[test]
    fn locate_catalog_asset_uses_full_filtered_order() {
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO assets(id,name,ext,ext_lower,is_listed,exif_date,file_modified) VALUES
            (1,'keep.dng','dng','dng',1,'2026-08-31T16:16:00','2026-11-24T00:00:00Z'),
            (2,'keep.jpg','jpg','jpg',1,'2026-09-01T12:00:00',NULL),
            (3,'skip.jpg','jpg','jpg',1,'2026-08-01T00:00:00',NULL)",
            )
            .unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut filter = test_filter("special", "all_photos");
        filter.query = Some("keep".into());
        filter.sort = "dateTaken".into();
        filter.offset = 1;
        filter.limit = 1;
        for descending in [false, true] {
            filter.descending = descending;
            assert_eq!(
                locate_asset(&db, &filter, &formats, 1).unwrap(),
                Some(usize::from(descending))
            );
            assert_eq!(locate_asset(&db, &filter, &formats, 3).unwrap(), None);
            let page = assets_page(&db, &filter, &formats).unwrap();
            assert_eq!(locate_asset(&db, &filter, &formats, page.items[0].id).unwrap(), Some(1));
        }
    }

    #[test]
    fn folder_previews_use_direct_images_then_descendants() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,path,file_modified) VALUES (1,'one.jpg','jpg','jpg',1,'C:/photos/one.jpg','2025-01-01'),(2,'two.png','png','png',1,'C:/photos/empty/inner/two.png','2025-01-02'),(3,'clip.mp4','mp4','mp4',1,'C:/photos/clip.mp4',NULL),(4,'gone.jpg','jpg','jpg',0,'C:/photos/gone.jpg',NULL),
            (5,'ba.jpg','jpg','jpg',1,'C:/photos/empty/other/ba.jpg',NULL),
            (6,'a.jpg','jpg','jpg',1,'C:/photos/empty/inner/deep/a.jpg',NULL),
            (7,'zz.jpg','jpg','jpg',1,'C:/photos/empty/other/zz.jpg',NULL),
            (8,'09.jpg','jpg','jpg',1,'C:/photos/empty/x/y/09.jpg',NULL)").unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let result = folder_previews(&db, &["C:\\photos".into(), "C:\\empty".into()], &formats).unwrap();
        assert_eq!(result["C:\\photos"].iter().map(|a| a.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(result["C:\\empty"].len(), 0);
        let descendants = folder_previews(&db, &["C:\\photos\\empty".into()], &formats).unwrap();
        assert_eq!(
            descendants["C:\\photos\\empty"]
                .iter()
                .map(|a| a.id)
                .collect::<Vec<_>>(),
            vec![8, 6, 5, 2]
        );
    }
    fn test_filter(kind: &str, value: &str) -> AssetFilter {
        AssetFilter {
            kind: kind.into(),
            id: None,
            root_id: None,
            include_children: false,
            folder_path: None,
            ids: None,
            value: Some(value.into()),
            query: None,
            sort: "name".into(),
            descending: false,
            offset: 0,
            limit: 10,
            rating_mode: None,
            rating_value: None,
            label_filter: None,
            tagged_only: false,
            category_filter: None,
            keyword_filter: None,
            include_categories: false,
            include_keywords: false,
            date_from: None,
            date_to: None,
            date_source: None,
            date_no_date: false,
            file_group: None,
        }
    }
    #[test]
    fn catalogue_search_treats_sql_wildcards_and_backslash_as_literals() {
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO keywords VALUES(1,NULL,'A%_\\B'),(2,NULL,'Axx\\B');
             INSERT INTO categories VALUES(1,NULL,NULL,'A%_\\B'),(2,NULL,NULL,'Axx\\B');",
            )
            .unwrap();
        for query in ["%", "_", r"%_\"] {
            assert_eq!(
                keyword_search_page(&db, query, 0, 20)
                    .unwrap()
                    .iter()
                    .map(|n| n.id)
                    .collect::<Vec<_>>(),
                [1]
            );
            for kind in ["category", "keyword"] {
                for children in [false, true] {
                    assert_eq!(
                        catalog_tree_search(&db, kind, query, 0, 20, children)
                            .unwrap()
                            .iter()
                            .map(|n| n.id)
                            .collect::<Vec<_>>(),
                        [1]
                    );
                }
            }
        }
        assert_eq!(keyword_search_page(&db, r"\", 0, 20).unwrap().len(), 2);
    }

    #[test]
    fn ai_view_pages_past_500_ids_in_requested_order() {
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<620)
             INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) SELECT id,'a.jpg','jpg','jpg',1,1 FROM n;",
            )
            .unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut filter = test_filter("ai", "");
        filter.ids = Some((1..=620).rev().collect());
        filter.offset = 500;
        filter.limit = 4;
        let page = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!((page.total, page.total_size), (620, 620.0));
        assert_eq!(
            page.items.iter().map(|a| a.id).collect::<Vec<_>>(),
            [120, 119, 118, 117]
        );
        assert_eq!(
            asset_ids_page(&db, &filter, &formats, 500, 4).unwrap(),
            [120, 119, 118, 117]
        );
    }

    #[test]
    fn asset_id_pages_match_grid_for_every_view_kind() {
        let db = test_db();
        db.lock().unwrap().execute_batch(
            "ALTER TABLE assets ADD COLUMN delete_suggestion INTEGER;
             INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,rating,label,tagged,exif_date,delete_suggestion) VALUES
               (1,9,'a_1.jpg','jpg','jpg',1,4,'Red',1,'2025-01-01',1),
               (2,9,'b.jpg','jpg','jpg',1,4,'Red',0,'2025-01-02',NULL),
               (3,10,'c.jpg','jpg','jpg',1,NULL,NULL,0,NULL,NULL),
               (4,9,'d.jpg','jpg','jpg',0,4,'Red',1,'2025-01-01',1);
             INSERT INTO category_roots VALUES(10,'Root');
             INSERT INTO categories VALUES(100,NULL,10,'Top'),(101,100,10,'Child');
             INSERT INTO keywords VALUES(200,NULL,'Top'),(201,200,'Child');
             INSERT INTO asset_categories VALUES(1,100),(2,101);
             INSERT INTO asset_keywords VALUES(1,200),(2,201);
             CREATE TABLE asset_color_overrides(asset_id INTEGER,color TEXT,state INTEGER);
             INSERT INTO asset_color_overrides VALUES(1,'modrá',1),(2,'modrá',0),(3,'modrá',1);
             ATTACH DATABASE ':memory:' AS ai;
             CREATE TABLE ai.colors(asset_id INTEGER, auto TEXT);
             INSERT INTO ai.colors VALUES(2,'[\"modrá\",\"biela\"]'),(3,'[\"biela\"]');
             CREATE TABLE ai.quality(asset_id INTEGER,version INTEGER,sharp REAL,hi REAL,mean REAL);
             INSERT INTO ai.quality VALUES(1,1,10,0,0.5),(2,1,90,0,0.5);
             ATTACH DATABASE ':memory:' AS auto_exif;
             CREATE TABLE auto_exif.exif_index(asset_id INTEGER,camera TEXT);
             INSERT INTO auto_exif.exif_index VALUES(1,'Canon'),(2,'Nikon'),(3,'Canon');"
        ).unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut cases = vec![
            (test_filter("auto", "camera\tCanon"), vec![1, 3]),
            (test_filter("rating", "4"), vec![1, 2]),
            (test_filter("rating", "unrated"), vec![3]),
            (test_filter("label", "Red"), vec![1, 2]),
            (test_filter("color", "modrá"), vec![1, 3]),
            (test_filter("color", "modrá, biela"), vec![3]),
            (test_filter("color", "invalid"), vec![]),
            (test_filter("quality", "blur"), vec![1]),
            (test_filter("special", "all_photos"), vec![1, 2, 3]),
            (test_filter("special", "uncategorized"), vec![3]),
            (test_filter("special", "unkeyworded"), vec![3]),
            (test_filter("special", "tagged"), vec![1]),
            (test_filter("special", "delete_suggested"), vec![1]),
            (test_filter("unknown", ""), vec![]),
        ];
        for (kind, id) in [("folder", 9), ("category", 100), ("keyword", 200)] {
            let mut filter = test_filter(kind, "");
            filter.id = Some(id);
            cases.push((filter.clone(), if kind == "folder" { vec![1, 2] } else { vec![1] }));
            if kind == "folder" {
                filter.ids = Some(vec![9, 10]);
            } else {
                filter.include_children = true;
            }
            cases.push((filter, if kind == "folder" { vec![1, 2, 3] } else { vec![1, 2] }));
        }
        let mut root = test_filter("category", "");
        root.root_id = Some(10);
        cases.push((root.clone(), vec![1]));
        root.include_children = true;
        cases.push((root, vec![1, 2]));
        let mut date = test_filter("date", "");
        date.date_source = Some("taken".into());
        date.date_from = Some("2025-01-01".into());
        date.date_to = Some("2025-01-02".into());
        cases.push((date, vec![1, 2]));
        for kind in ["ai", "series", "people"] {
            let mut filter = test_filter(kind, "");
            filter.ids = Some(vec![3, 1, 3, 4, -1, 2]);
            cases.push((filter, vec![3, 1, 2]));
        }
        let mut combined = test_filter("color", "modrá");
        combined.query = Some("_".into());
        combined.rating_mode = Some("atLeast".into());
        combined.rating_value = Some(4);
        combined.label_filter = Some("Red".into());
        combined.tagged_only = true;
        combined.file_group = Some("images".into());
        cases.push((combined, vec![1]));

        for (mut filter, expected) in cases {
            for (offset, limit) in [(0, 10), (1, 1), (10, 2)] {
                filter.offset = offset;
                filter.limit = limit;
                let page = assets_page(&db, &filter, &formats).unwrap();
                let grid = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
                assert_eq!(
                    grid,
                    expected
                        .iter()
                        .copied()
                        .skip(offset as usize)
                        .take(limit as usize)
                        .collect::<Vec<_>>(),
                    "{} {:?}",
                    filter.kind,
                    filter.value
                );
                assert_eq!(
                    asset_ids_page(&db, &filter, &formats, offset, limit).unwrap(),
                    grid,
                    "{}",
                    filter.kind
                );
            }
        }
        db.lock().unwrap().execute_batch("DETACH DATABASE ai;").unwrap();
        let filter = test_filter("color", "modrá");
        assert_eq!(asset_ids_page(&db, &filter, &formats, 0, 10).unwrap(), vec![1, 3]);
        assert_eq!(
            assets_page(&db, &filter, &formats)
                .unwrap()
                .items
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn folder_pages_slice_after_merging_equal_names() {
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);
             INSERT INTO folders VALUES (1,50,1,'A',NULL),(2,51,2,'a',NULL),(3,50,1,'B',NULL),
                                       (4,51,2,'C',NULL),(5,50,1,'D',NULL);
             INSERT INTO assets(id,folder_id,is_listed) VALUES(1,1,1),(2,2,1),(3,3,1),(4,4,1),(5,5,1);",
            )
            .unwrap();
        let page = |offset| folders_page(&db, Some(50), Some(vec![50, 51]), offset, 2).unwrap();
        let first = page(0);
        assert_eq!(
            (first[0].name.as_str(), first[0].count, first[0].folder_ids.as_slice()),
            ("A", 2, &[1, 2][..])
        );
        assert_eq!(first[1].name, "B");
        assert_eq!(
            page(2).iter().map(|folder| folder.name.as_str()).collect::<Vec<_>>(),
            ["C", "D"]
        );
        assert!(page(4).is_empty());
    }

    #[test]
    fn cached_catalog_pages_keep_order_totals_and_invalidate_on_write() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES (1,'z.jpg','jpg','jpg',1,10),(2,'a.jpg','jpg','jpg',1,20),(3,'m.jpg','jpg','jpg',1,NULL),(4,'b.jpg','jpg','jpg',1,40),(5,'hidden.jpg','jpg','jpg',0,100)").unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut filter = test_filter("special", "all_photos");
        filter.limit = 2;
        for (offset, expected) in [(0, vec![2, 4]), (2, vec![3, 1]), (3, vec![1]), (9, vec![])] {
            filter.offset = offset;
            let page = assets_page(&db, &filter, &formats).unwrap();
            assert_eq!(page.items.iter().map(|a| a.id).collect::<Vec<_>>(), expected);
            assert_eq!((page.total, page.total_size), (4, 70.0));
        }
        db.lock()
            .unwrap()
            .execute_batch("UPDATE assets SET name='0.jpg',size=5 WHERE id=1; UPDATE assets SET is_listed=0 WHERE id=4")
            .unwrap();
        filter.offset = 0;
        let page = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(page.items.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!((page.total, page.total_size), (3, 25.0));
    }
    #[test]
    fn auto_category_view_filters_and_pages_catalog() {
        let db = test_db();
        {
            let c = db.lock().unwrap();
            c.execute_batch("ATTACH DATABASE ':memory:' AS auto_exif; CREATE TABLE auto_exif.exif_index(asset_id INTEGER PRIMARY KEY,camera TEXT); INSERT INTO assets(id,name,ext,ext_lower,is_listed,path) VALUES(1,'one.jpg','jpg','jpg',1,'one'),(2,'two.jpg','jpg','jpg',1,'two'),(3,'three.jpg','jpg','jpg',1,'three'); INSERT INTO auto_exif.exif_index VALUES(1,'Canon'),(2,'Nikon'),(3,'Canon');").unwrap();
        }
        let formats = FormatTable::load_selected(None);
        let mut filter = test_filter("auto", "camera\tCanon");
        filter.limit = 1;
        let page = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(asset_ids_page(&db, &filter, &formats, 0, 100).unwrap().len(), 2);
    }
    #[test]
    fn special_items_use_listed_photos_and_relationships() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,tagged,rating) VALUES(1,'a.jpg','jpg','jpg',1,10,1,4),(2,'b.doc','doc','doc',1,20,0,5),(3,'gone.jpg','jpg','jpg',0,30,1,3),(4,'d.png','png','png',1,40,0,NULL);INSERT INTO asset_categories VALUES(1,9),(2,9);INSERT INTO asset_keywords VALUES(1,8);").unwrap();
        for (kind, count) in [
            ("all_photos", 2),
            ("uncategorized", 1),
            ("unkeyworded", 1),
            ("tagged", 1),
        ] {
            assert_eq!(
                assets_page(
                    &db,
                    &test_filter("special", kind),
                    &FormatTable::parse(include_str!("../../../formats.json")).unwrap()
                )
                .unwrap()
                .total,
                count,
                "{kind}"
            );
        }
        assert_eq!(
            values_page(
                &db,
                "special",
                0,
                10,
                &FormatTable::parse(include_str!("../../../formats.json")).unwrap()
            )
            .unwrap()
            .iter()
            .find(|v| v.value == "unkeyworded")
            .unwrap()
            .count,
            1
        );
    }
    #[test]
    fn keyword_subquery_is_only_selected_when_requested() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'a.jpg','jpg','jpg',1,10);INSERT INTO keywords(id,name) VALUES(1,'Rodina'),(2,'Príroda');INSERT INTO asset_keywords VALUES(1,1),(1,2);").unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut f = test_filter("special", "all_photos");
        let without = assets_page(&db, &f, &formats).unwrap();
        assert_eq!(without.items[0].display_keywords, None);
        f.include_keywords = true;
        let with = assets_page(&db, &f, &formats).unwrap();
        assert_eq!(with.items[0].display_keywords.as_deref(), Some("Príroda, Rodina"));
    }
    #[test]
    fn rating_query_uses_selected_value_and_excludes_unrated() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating) VALUES(1,'one.jpg','jpg','jpg',1,1,1),(2,'three.jpg','jpg','jpg',1,1,3),(3,'five.jpg','jpg','jpg',1,1,5),(4,'none.jpg','jpg','jpg',1,1,NULL);").unwrap();
        for (mode, value, expected) in [("equal", 3, 1), ("atLeast", 3, 2), ("atMost", 3, 2)] {
            let mut f = test_filter("special", "all_photos");
            f.rating_mode = Some(mode.into());
            f.rating_value = Some(value);
            assert_eq!(
                assets_page(
                    &db,
                    &f,
                    &FormatTable::parse(include_str!("../../../formats.json")).unwrap()
                )
                .unwrap()
                .total,
                expected,
                "{mode}"
            );
        }
    }
    #[test]
    fn descendant_views_are_distinct_optional_and_include_roots() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(10,'Root'),(20,'Cycle');
            INSERT INTO categories VALUES(100,NULL,10,'Top'),(101,100,10,'Child'),(102,101,10,'Grandchild'),(200,201,20,'Cycle A'),(201,200,20,'Cycle B');
            INSERT INTO keywords VALUES(500,NULL,'Top'),(501,500,'Child'),(502,501,'Grandchild');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'a.jpg','jpg','jpg',1,10),(2,'b.jpg','jpg','jpg',1,20),(3,'c.jpg','jpg','jpg',1,30);
            INSERT INTO asset_categories VALUES(1,100),(2,101),(2,102),(3,102),(1,200),(2,201);
            INSERT INTO asset_keywords VALUES(1,500),(2,501),(2,502),(3,502);").unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut category = test_filter("category", "");
        category.id = Some(101);
        let direct = assets_page(&db, &category, &formats).unwrap();
        assert_eq!(direct.total, 1);
        assert_eq!(direct.total_size, 20.0);
        category.include_children = true;
        let descendants = assets_page(&db, &category, &formats).unwrap();
        assert_eq!(descendants.total, 2);
        assert_eq!(descendants.total_size, 50.0);
        assert_eq!(asset_ids_page(&db, &category, &formats, 0, 20).unwrap(), vec![2, 3]);
        let mut root = test_filter("category", "");
        root.root_id = Some(10);
        root.include_children = true;
        let root_page = assets_page(&db, &root, &formats).unwrap();
        assert_eq!(root_page.total, 3);
        assert_eq!(root_page.total_size, 60.0);
        root.include_children = false;
        assert_eq!(assets_page(&db, &root, &formats).unwrap().total, 1);
        let mut keyword = test_filter("keyword", "");
        keyword.id = Some(501);
        assert_eq!(assets_page(&db, &keyword, &formats).unwrap().total, 1);
        keyword.include_children = true;
        assert_eq!(assets_page(&db, &keyword, &formats).unwrap().total, 2);
        assert_eq!(asset_ids_page(&db, &keyword, &formats, 0, 20).unwrap(), vec![2, 3]);
        let mut cycle = test_filter("category", "");
        cycle.id = Some(200);
        cycle.include_children = true;
        assert_eq!(assets_page(&db, &cycle, &formats).unwrap().total, 2);
        let roots_on = roots_page(&db, 0, 20, true).unwrap();
        assert_eq!(roots_on.iter().find(|r| r.id == 10).unwrap().count, 3);
        let roots_off = roots_page(&db, 0, 20, false).unwrap();
        assert_eq!(roots_off.iter().find(|r| r.id == 10).unwrap().count, 1);
        let top_on = nodes_page(&db, "category", None, Some(10), 0, 20, true).unwrap();
        assert_eq!(top_on[0].count, 3);
        let top_off = nodes_page(&db, "category", None, Some(10), 0, 20, false).unwrap();
        assert_eq!(top_off[0].count, 1);
        let child_on = nodes_page(&db, "category", Some(100), Some(10), 0, 20, true).unwrap();
        assert_eq!(child_on[0].count, 2);
        let keywords_on = nodes_page(&db, "keyword", None, None, 0, 20, true).unwrap();
        assert_eq!(keywords_on[0].count, 3);
    }
    #[test]
    fn category_icon_migration_is_additive_and_idempotent() {
        let db = test_db();
        {
            let conn = db.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO category_roots VALUES(1,'Miesta'),(2,'Príroda');
                INSERT INTO categories VALUES(10,NULL,1,'Miesta'),(11,10,1,'Výlety'),(20,NULL,2,'Les');
                INSERT INTO asset_categories VALUES(99,11);",
            )
            .unwrap();
            conn.pragma_update(None, "query_only", "ON").unwrap();
        }
        assert!(
            roots_page(&db, 0, 10, false)
                .unwrap()
                .iter()
                .all(|root| root.icon_key.is_none())
        );
        assert!(
            nodes_page(&db, "category", None, Some(1), 0, 10, true).unwrap()[0]
                .icon_key
                .is_none()
        );
        {
            let mut conn = db.lock().unwrap();
            conn.pragma_update(None, "query_only", "OFF").unwrap();
            let changes = conn.total_changes();
            migrate_category_icons(&mut conn).unwrap();
            assert_eq!(conn.total_changes(), changes);
            for table in ["categories", "category_roots"] {
                assert_eq!(
                    conn.query_row(&format!("SELECT count(icon_key) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
            conn.execute_batch(
                "UPDATE categories SET icon_key='travel' WHERE id=11;
                UPDATE category_roots SET icon_key='places' WHERE id=1;
                UPDATE category_roots SET icon_key='nature' WHERE id=2;",
            )
            .unwrap();
            let changes = conn.total_changes();
            migrate_category_icons(&mut conn).unwrap();
            assert_eq!(conn.total_changes(), changes);
            assert_eq!(
                conn.query_row("SELECT category_id FROM asset_categories WHERE asset_id=99", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
                11
            );
        }
        for include_children in [false, true] {
            let roots = roots_page(&db, 0, 10, include_children).unwrap();
            assert_eq!(roots[0].merged_category_id, Some(10));
            assert_eq!(roots[0].icon_key.as_deref(), Some("places"));
            assert_eq!(roots[1].merged_category_id, None);
            assert_eq!(roots[1].icon_key.as_deref(), Some("nature"));
            let children = nodes_page(&db, "category", None, Some(1), 0, 10, include_children).unwrap();
            assert_eq!(children[0].name, "Výlety");
            assert_eq!(children[0].icon_key.as_deref(), Some("travel"));
            let children = nodes_page(&db, "category", None, Some(2), 0, 10, include_children).unwrap();
            assert_eq!(children[0].name, "Les");
            assert!(children[0].icon_key.is_none());
        }
    }

    #[test]
    fn merged_category_roots_expand_count_and_search_without_duplicate_level() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(1,'Miesta'),(2,'Príroda'),(3,'Bez kategórií');
            INSERT INTO categories VALUES(10,NULL,1,' miesta '),(11,10,1,'Dieťa'),(12,NULL,1,'Extra'),(20,NULL,2,'Krajina');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'root.jpg','jpg','jpg',1,1),(2,'child.jpg','jpg','jpg',1,1),(3,'extra.jpg','jpg','jpg',1,1),(4,'nature.jpg','jpg','jpg',1,1);
            INSERT INTO asset_categories VALUES(1,10),(2,11),(3,12),(4,20);").unwrap();
        let roots_on = roots_page(&db, 0, 10, true).unwrap();
        let places = roots_on.iter().find(|r| r.id == 1).unwrap();
        assert_eq!(places.merged_category_id, Some(10));
        assert_eq!(places.count, 2);
        assert!(places.has_children);
        let places_off = roots_page(&db, 0, 10, false)
            .unwrap()
            .into_iter()
            .find(|r| r.id == 1)
            .unwrap();
        assert_eq!(places_off.count, 1);
        let children = nodes_page(&db, "category", None, Some(1), 0, 20, true).unwrap();
        assert_eq!(children.iter().map(|n| n.id).collect::<Vec<_>>(), vec![11, 12]);
        assert!(children.iter().any(|n| n.id == 11 && n.count == 1));
        let path = catalog_tree_search(&db, "category", "dieťa", 0, 20, true).unwrap();
        assert_eq!(path[0].name, "Miesta > Dieťa");
        let nature = roots_page(&db, 0, 10, true)
            .unwrap()
            .into_iter()
            .find(|r| r.id == 2)
            .unwrap();
        assert_eq!(nature.merged_category_id, None);
        assert_eq!(nature.count, 1);
        assert_eq!(
            nodes_page(&db, "category", None, Some(2), 0, 20, true).unwrap()[0].id,
            20
        );
        let empty = roots_page(&db, 0, 10, true)
            .unwrap()
            .into_iter()
            .find(|r| r.id == 3)
            .unwrap();
        assert_eq!(empty.merged_category_id, None);
        assert_eq!(empty.count, 0);
        assert!(!empty.has_children);
    }
    #[test]
    fn asset_catalog_links_include_direct_links_ancestors_and_root_ids() {
        let db = test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(1,'Miesta');
            INSERT INTO categories VALUES(10,NULL,1,'Miesta'),(11,10,1,'Dieťa'),(12,11,1,'Vnuk');
            INSERT INTO keywords VALUES(20,NULL,'Téma'),(21,20,'Podtéma'),(22,21,'List');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed) VALUES(1,'linked.jpg','jpg','jpg',1),(2,'uncatalogued.jpg','jpg','jpg',1);
            INSERT INTO asset_categories VALUES(1,12);INSERT INTO asset_keywords VALUES(1,22);").unwrap();
        let links = asset_catalog_links(&db, 1).unwrap().unwrap();
        assert_eq!(links.category_ids, vec![12]);
        assert_eq!(
            links.category_paths,
            vec![CategoryPath {
                id: 12,
                path: "Miesta › Dieťa › Vnuk".into()
            }]
        );
        assert_eq!(links.category_ancestor_ids, vec![10, 11]);
        assert_eq!(links.keyword_ids, vec![22]);
        assert_eq!(links.keyword_ancestor_ids, vec![20, 21]);
        assert_eq!(links.root_ids, vec![1]);
        assert!(asset_catalog_links(&db, 2).unwrap().unwrap().category_ids.is_empty());
        assert!(asset_catalog_links(&db, 99).unwrap().is_none());
    }

    #[test]
    fn ai_view_preserves_order_applies_filters_and_skips_unknown_ids() {
        let db = test_db();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating,tagged) VALUES
            (1,'plain.jpg','jpg','jpg',1,10,4,0),(2,'z-last.jpg','jpg','jpg',1,20,4,1),
            (3,'offline-row.jpg','jpg','jpg',0,30,5,1),(4,'a-first.png','png','png',1,40,3,1);",
            )
            .unwrap();
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let filter = AssetFilter {
            kind: "ai".into(),
            id: None,
            root_id: None,
            include_children: false,
            folder_path: None,
            ids: Some(vec![4, 2, 404, 3, 1, 2]),
            value: None,
            query: None,
            sort: "name".into(),
            descending: true,
            offset: 0,
            limit: 500,
            rating_mode: Some("atLeast".into()),
            rating_value: Some(3),
            label_filter: None,
            tagged_only: true,
            category_filter: None,
            keyword_filter: None,
            file_group: Some("images".into()),
            include_categories: false,
            include_keywords: false,
            date_from: None,
            date_to: None,
            date_source: None,
            date_no_date: false,
        };
        let page = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(page.items.iter().map(|asset| asset.id).collect::<Vec<_>>(), vec![4, 2]);
        assert_eq!(page.total, 2);
        assert_eq!(page.total_size, 60.0);
        let mut second = filter.clone();
        second.offset = 1;
        second.limit = 1;
        let page = assets_page(&db, &second, &formats).unwrap();
        assert_eq!(page.items.iter().map(|asset| asset.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(page.total, 2);
        assert_eq!(page.total_size, 60.0);
    }

    #[test]
    fn people_view_preserves_order_and_exceeds_ai_cap() {
        let db = test_db();
        {
            let conn = db.lock().unwrap();
            for id in 1..=501 {
                conn.execute(
                    "INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(?1,?2,'jpg','jpg',1,1)",
                    params![id, format!("{id:04}.jpg")],
                )
                .unwrap();
            }
        }
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let ids = (1..=501).rev().collect::<Vec<_>>();
        let filter = AssetFilter {
            kind: "people".into(),
            id: None,
            root_id: None,
            include_children: false,
            folder_path: None,
            ids: Some(ids.clone()),
            value: None,
            query: None,
            sort: "name".into(),
            descending: true,
            offset: 0,
            limit: 500,
            rating_mode: None,
            rating_value: None,
            label_filter: None,
            tagged_only: false,
            category_filter: None,
            keyword_filter: None,
            file_group: Some("images".into()),
            include_categories: false,
            include_keywords: false,
            date_from: None,
            date_to: None,
            date_source: None,
            date_no_date: false,
        };
        let first = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(first.total, 501);
        assert_eq!(first.items.len(), 500);
        assert_eq!(first.items.first().unwrap().id, 501);
        let mut next = filter.clone();
        next.offset = 500;
        let page = assets_page(&db, &next, &formats).unwrap();
        assert_eq!(page.items.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(asset_ids_page(&db, &filter, &formats, 499, 2).unwrap(), vec![2, 1]);
    }

    #[test]
    fn color_view_uses_effective_colors_and_normal_paging_filters() {
        let db = test_db();
        let conn = db.lock().unwrap();
        conn.execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER,color TEXT,state INTEGER,PRIMARY KEY(asset_id,color));ATTACH DATABASE ':memory:' AS ai;CREATE TABLE ai.colors(asset_id INTEGER PRIMARY KEY,auto TEXT);INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating) VALUES(1,'a.jpg','jpg','jpg',1,10,4),(2,'b.jpg','jpg','jpg',1,20,2),(3,'c.jpg','jpg','jpg',1,30,5),(4,'hidden.jpg','jpg','jpg',0,40,5);INSERT INTO ai.colors VALUES(1,'[\"červená\"]'),(2,'[]'),(3,'[\"červená\"]');INSERT INTO asset_color_overrides VALUES(2,'červená',1),(3,'červená',0);").unwrap();
        drop(conn);
        let formats = FormatTable::parse(include_str!("../../../formats.json")).unwrap();
        let mut filter = test_filter("color", "červená");
        filter.limit = 1;
        filter.rating_mode = Some("atLeast".into());
        filter.rating_value = Some(3);
        let first = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(first.total, 1);
        assert_eq!(first.total_size, 10.0);
        assert_eq!(first.items[0].id, 1);
        filter.rating_mode = None;
        filter.offset = 1;
        let second = assets_page(&db, &filter, &formats).unwrap();
        assert_eq!(second.total, 2);
        assert_eq!(second.items[0].id, 2);
    }
}
