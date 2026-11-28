use crate::db::{self, AssetFilter};
use assets::{rank_similarity_ids, similarity_page};

#[cfg(test)]
#[test]
fn similarity_order_preserves_unranked_ids_and_directory_prefix() {
    let mut ids = [-1, 2, 3, 1, 4];
    rank_similarity_ids(&mut ids[1..], &[3, 4]);
    assert_eq!(ids, [-1, 3, 4, 2, 1]);
}

#[cfg(test)]
#[test]
fn similarity_cache_reuses_order_and_invalidates_when_rows_change() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);
                              CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER);
                              CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER);
                              CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT);
                              CREATE TABLE keywords(id INTEGER PRIMARY KEY,name TEXT);").unwrap();
    let measure = std::env::var_os("DDREAMS_BENCH_SIMILARITY_SYNTHETIC").is_some();
    let count = if measure { 71_000 } else { 1_200 };
    connection.execute("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?)
                        INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) SELECT id,printf('%06d.jpg',id),'jpg','jpg',1,100 FROM n", [count]).unwrap();
    let db = Arc::new(std::sync::Mutex::new(connection));
    let formats = crate::formats::FormatTable::load_selected(None);
    let listing = ListingService::new(db.clone(), formats.clone());
    let filter: AssetFilter = serde_json::from_value(serde_json::json!({"kind":"special","value":"all_photos","sort":"similarity","descending":false,"offset":0,"limit":120})).unwrap();
    let status: crate::ai::AiStatus = crate::ai::AiStatus {
        available: true,
        reason: None,
        state: "ready".into(),
        model_state: "ready".into(),
        waiting: 0,
        indexed: count as u64,
        queued: 0,
        done: 0,
        failed: 0,
        offline: 0,
        total: count as u64,
        rate_per_s: 0.0,
        eta_s: None,
        last_error: None,
    };
    let ranks = std::cell::Cell::new(0);
    let rank = |mut ids: Vec<i64>| {
        ranks.set(ranks.get() + 1);
        ids.reverse();
        Ok(ids)
    };
    let first = similarity_page(&db, &listing, &formats, &filter, &status, rank).unwrap();
    assert_eq!(first.items[0].id, count);
    let mut next = filter.clone();
    next.offset = 120;
    let after_start = std::time::Instant::now();
    let second = similarity_page(&db, &listing, &formats, &next, &status, rank).unwrap();
    let after = after_start.elapsed();
    assert_eq!(second.items[0].id, count - 120);
    assert_eq!(ranks.get(), 1);
    if measure {
        // Old warm-page path: read every Asset row, then apply the cached AI order to all rows.
        let mut base = filter.clone();
        base.sort = "name".into();
        base.limit = 500;
        let before_start = std::time::Instant::now();
        let mut all = Vec::new();
        for offset in (0..count).step_by(500) {
            base.offset = offset;
            all.extend(db::assets_page(&db, &base, &formats).unwrap().items);
        }
        let positions: std::collections::HashMap<_, _> = (1..=count)
            .rev()
            .enumerate()
            .map(|(position, id)| (id, position))
            .collect();
        all.sort_by_key(|asset| positions[&asset.id]);
        assert_eq!(all[120].id, second.items[0].id);
        eprintln!(
            "similarity synthetic warm page: old {:.2?}, new {:.2?}, assets {count}",
            before_start.elapsed(),
            after
        );
    }
    db.lock()
        .unwrap()
        .execute("UPDATE assets SET is_listed=0 WHERE id=?", [count])
        .unwrap();
    let refreshed = similarity_page(&db, &listing, &formats, &filter, &status, rank).unwrap();
    assert_eq!(refreshed.items[0].id, count - 1);
    assert_eq!(ranks.get(), 2);
}
