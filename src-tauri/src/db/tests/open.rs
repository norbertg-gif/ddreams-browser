use super::open_catalog;
use rusqlite::Connection;

use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn writable_catalog_uses_wal_busy_timeout_and_foreign_keys() {
    let root = std::env::temp_dir().join(format!(
        "ddreams-open-catalog-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("catalog.sqlite");
    Connection::open(&path).unwrap();
    let connection = open_catalog(&path, false).unwrap();
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    let busy_timeout: i64 = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(foreign_keys, 1);
    assert_eq!(busy_timeout, 5_000);
    drop(connection);
    let _ = std::fs::remove_dir_all(root);
}
