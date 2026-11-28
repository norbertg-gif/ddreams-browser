use rusqlite::{Connection, OpenFlags};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

#[cfg(test)]
pub fn catalog_path() -> PathBuf {
    crate::paths::Paths::resolve().catalog
}

pub fn open_catalog(path: &Path, readonly: bool) -> rusqlite::Result<Connection> {
    if readonly {
        return open_read_only(path);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -32000)?;
    conn.pragma_update(None, "mmap_size", 268435456)?;
    Ok(conn)
}

pub(super) fn has_category_icon_column(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='icon_key')",
        [table],
        |row| row.get(0),
    )
}

pub(crate) fn migrate_category_icons(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for table in ["categories", "category_roots"] {
        if !has_category_icon_column(&tx, table)? {
            tx.execute(&format!("ALTER TABLE {table} ADD COLUMN icon_key TEXT"), [])?;
        }
    }
    tx.commit()
}

pub fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.pragma_update(None, "cache_size", -32000)?;
    conn.pragma_update(None, "mmap_size", 268435456)?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(4))?;
    Ok(conn)
}
