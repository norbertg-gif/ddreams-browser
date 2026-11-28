use super::{AssetFilter, assets_page, catalog_path, nodes_page, open_read_only, roots_page};
use crate::formats::FormatTable;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::time::Instant;
static BENCH_LOCK: Mutex<()> = Mutex::new(());
fn db_path() -> PathBuf {
    let p = catalog_path();
    if p.exists() {
        p
    } else {
        PathBuf::from("../.local/catalog.sqlite")
    }
}
#[test]
#[ignore]
fn bench_open_catalog() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let p = db_path();
    let t = Instant::now();
    let c = open_read_only(&p).unwrap();
    println!("open catalog: {:?}", t.elapsed());
    drop(c);
}
#[test]
#[ignore]
fn bench_largest_folder_first_page() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let (f, folder_count) = {
        let c = db.lock().unwrap();
        c.query_row(
            "SELECT folder_id,count(*) n FROM assets WHERE is_listed=1 GROUP BY folder_id ORDER BY n DESC LIMIT 1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    let t = Instant::now();
    let formats = FormatTable::load();
    let p = assets_page(
        &db,
        &AssetFilter {
            kind: "folder".into(),
            id: Some(f),
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
        },
        &formats,
    )
    .unwrap();
    println!(
        "largest folder first page (folder {f}, {} assets, {} rows): {:?}",
        folder_count,
        p.items.len(),
        t.elapsed()
    );
}
#[test]
#[ignore]
fn bench_deep_offset_page() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let f = {
        let c = db.lock().unwrap();
        c.query_row(
            "SELECT folder_id FROM assets WHERE is_listed=1 GROUP BY folder_id ORDER BY count(*) DESC LIMIT 1",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let folder_count = {
        let c = db.lock().unwrap();
        c.query_row(
            "SELECT count(*) FROM assets WHERE folder_id=? AND is_listed=1",
            [f],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let t = Instant::now();
    let formats = FormatTable::load();
    let _ = assets_page(
        &db,
        &AssetFilter {
            kind: "folder".into(),
            id: Some(f),
            root_id: None,
            include_children: false,
            folder_path: None,
            ids: None,
            value: None,
            query: None,
            sort: "name".into(),
            descending: false,
            offset: 3000,
            limit: 100,
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
        },
        &formats,
    )
    .unwrap();
    println!(
        "deep offset page (folder {f}, row count {}, offset 3000): {:?}",
        folder_count,
        t.elapsed()
    );
}
#[test]
#[ignore]
fn bench_category_tree() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let t = Instant::now();
    let _ = roots_page(&db, 0, 100, true);
    let _ = nodes_page(&db, "category", None, Some(1), 0, 500, true).unwrap();
    println!("category tree first level: {:?}", t.elapsed());
}
#[test]
#[ignore]
fn bench_keyword_tree() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let t = Instant::now();
    let _ = nodes_page(&db, "keyword", None, None, 0, 500, true).unwrap();
    println!("keyword tree first level: {:?}", t.elapsed());
}
#[test]
#[ignore]
fn bench_largest_category() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let c = {
        let conn = db.lock().unwrap();
        conn.query_row("SELECT ac.category_id,count(*) n FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1 GROUP BY ac.category_id ORDER BY n DESC LIMIT 1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap()
    };
    let t = Instant::now();
    let formats = FormatTable::load();
    let p = assets_page(
        &db,
        &AssetFilter {
            kind: "category".into(),
            id: Some(c.0),
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
        },
        &formats,
    )
    .unwrap();
    println!(
        "largest category (id {}, {} assets, page {}): {:?}",
        c.0,
        c.1,
        p.items.len(),
        t.elapsed()
    );
}
#[test]
#[ignore]
fn bench_largest_category_root() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let root = roots_page(&db, 0, 500, true)
        .unwrap()
        .into_iter()
        .max_by_key(|r| r.count)
        .unwrap();
    let formats = FormatTable::load();
    let started = Instant::now();
    let page = assets_page(
        &db,
        &AssetFilter {
            kind: "category".into(),
            id: None,
            root_id: Some(root.id),
            include_children: true,
            folder_path: None,
            ids: None,
            value: None,
            query: None,
            sort: "name".into(),
            descending: false,
            offset: 0,
            limit: 100,
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
        },
        &formats,
    )
    .unwrap();
    println!(
        "largest category root (id {}, tree count {}, view total {}, first page {}): {:?}",
        root.id,
        root.count,
        page.total,
        page.items.len(),
        started.elapsed()
    );
}
#[test]
#[ignore]
fn bench_special_items() {
    let _guard = BENCH_LOCK.lock().unwrap();
    let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
    let formats = FormatTable::load();
    for value in ["all_photos", "uncategorized", "unkeyworded", "tagged"] {
        let t = Instant::now();
        let p = assets_page(
            &db,
            &AssetFilter {
                kind: "special".into(),
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
                limit: 100,
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
            },
            &formats,
        )
        .unwrap();
        println!(
            "special {value}: total {}, first page {}, {:?}",
            p.total,
            p.items.len(),
            t.elapsed()
        );
    }
}
