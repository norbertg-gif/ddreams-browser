use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, time::{Duration, SystemTime, UNIX_EPOCH}};

const STATE_FILE: &str = ".last_catalog_backup.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct FileStamp { size: u64, modified_ns: u128 }

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BackupState { catalog: FileStamp, wal: Option<FileStamp>, backed_up_at: u64 }

pub fn create_startup_backup(catalog: &Path, backups: &Path) -> Result<Option<PathBuf>, String> {
    if !catalog.is_file() { return Err(format!("catalog not found: {}", catalog.display())); }
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let catalog_stamp = file_stamp(catalog).map_err(|e| e.to_string())?;
    let wal_stamp = file_stamp(&wal_path(catalog)).ok();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
    let state_path = backups.join(STATE_FILE);
    if let Ok(text) = std::fs::read_to_string(&state_path) {
        if let Ok(state) = serde_json::from_str::<BackupState>(&text) {
            let age = now.saturating_sub(state.backed_up_at);
            if state.catalog == catalog_stamp && state.wal == wal_stamp && age < 3600 {
                rotate_backups(backups).map_err(|e| e.to_string())?;
                return Ok(None);
            }
        }
    }

    let base = format!("catalog-{}.sqlite", utc_stamp(now));
    let mut destination = backups.join(&base);
    let mut collision = 1u32;
    while destination.exists() {
        destination = backups.join(format!("catalog-{}-{collision:02}.sqlite", utc_stamp(now)));
        collision += 1;
    }
    let connection = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let destination_sql = destination.to_string_lossy().to_string();
    if let Err(error) = connection.execute("VACUUM INTO ?1", [&destination_sql]) {
        let _ = std::fs::remove_file(&destination);
        return Err(error.to_string());
    }
    std::fs::write(&state_path, serde_json::to_vec(&BackupState { catalog: catalog_stamp, wal: wal_stamp, backed_up_at: now }).map_err(|e| e.to_string())?)
        .map_err(|e| format!("backup created but state could not be written: {e}"))?;
    rotate_backups(backups).map_err(|e| e.to_string())?;
    Ok(Some(destination))
}

pub fn rotate_backups(backups: &Path) -> std::io::Result<()> {
    let mut files = std::fs::read_dir(backups)?.filter_map(Result::ok).map(|entry| entry.path())
        .filter(|path| path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("catalog-") && name.ends_with(".sqlite")))
        .collect::<Vec<_>>();
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for stale in files.into_iter().skip(10) { std::fs::remove_file(stale)?; }
    Ok(())
}

fn wal_path(catalog: &Path) -> PathBuf {
    let mut value = catalog.as_os_str().to_os_string();
    value.push("-wal");
    PathBuf::from(value)
}

fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let metadata = std::fs::metadata(path)?;
    let modified_ns = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    Ok(FileStamp { size: metadata.len(), modified_ns })
}

fn utc_stamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 { year += 1; }
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", second_of_day / 3600, second_of_day / 60 % 60, second_of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("ddreams-backup-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn backup_uses_vacuum_snapshot_and_skips_identical_source_for_an_hour() {
        let root = temp_root();
        let catalog = root.join("catalog.sqlite");
        let backups = root.join("backups");
        let conn = Connection::open(&catalog).unwrap();
        conn.execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES('safe');").unwrap();
        drop(conn);
        let created = create_startup_backup(&catalog, &backups).unwrap().unwrap();
        assert!(created.is_file());
        let snapshot = Connection::open_with_flags(created, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(snapshot.query_row("SELECT v FROM t", [], |row| row.get::<_, String>(0)).unwrap(), "safe");
        assert!(create_startup_backup(&catalog, &backups).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rotation_keeps_the_ten_newest_named_backups() {
        let root = temp_root();
        let backups = root.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        for day in 1..=12 { std::fs::write(backups.join(format!("catalog-202601{day:02}-000000.sqlite")), []).unwrap(); }
        rotate_backups(&backups).unwrap();
        let mut kept = std::fs::read_dir(&backups).unwrap().filter_map(Result::ok).map(|v| v.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
        kept.sort();
        assert_eq!(kept.len(), 10);
        assert!(!kept.iter().any(|name| name.contains("20260101") || name.contains("20260102")));
        assert!(utc_stamp(0).starts_with("19700101-000000"));
        let _ = std::fs::remove_dir_all(root);
    }
}
