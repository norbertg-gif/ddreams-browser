use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, time::{Duration, SystemTime, UNIX_EPOCH}};

const STATE_FILE: &str = ".last_catalog_backup.json";
const DEFAULT_INTERVAL_DAYS: u32 = 30;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct FileStamp { size: u64, modified_ns: u128 }

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BackupState {
    #[serde(default)] catalog: Option<FileStamp>,
    #[serde(default)] wal: Option<FileStamp>,
    #[serde(default)] backed_up_at: Option<u64>,
    #[serde(default)] last_backup_file: Option<String>,
    #[serde(default = "default_interval")] interval_days: u32,
    #[serde(default)] snoozed_until: Option<u64>,
}

fn default_interval() -> u32 { DEFAULT_INTERVAL_DAYS }
impl Default for BackupState {
    fn default() -> Self { Self { catalog: None, wal: None, backed_up_at: None, last_backup_file: None, interval_days: DEFAULT_INTERVAL_DAYS, snoozed_until: None } }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub last_backup_at: Option<u64>, pub last_backup_file: Option<String>, pub backup_count: usize,
    pub backups_dir: String, pub interval_days: u32, pub snoozed_until: Option<u64>, pub due: bool,
}

fn read_state(backups: &Path) -> BackupState {
    std::fs::read(backups.join(STATE_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn backup_status(_catalog: &Path, backups: &Path) -> BackupStatus {
    let state = read_state(backups);
    let mut files = backup_files(backups);
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    let fallback = files.first().and_then(|path| {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some((modified, path.file_name()?.to_string_lossy().into_owned()))
    });
    let last_backup_at = state.backed_up_at.or_else(|| fallback.as_ref().map(|item| item.0));
    let last_backup_file = state.last_backup_file.or_else(|| fallback.map(|item| item.1));
    let now = unix_now();
    let due = state.interval_days > 0
        && last_backup_at.map_or(true, |last| now.saturating_sub(last) >= u64::from(state.interval_days) * 86_400)
        && state.snoozed_until.map_or(true, |until| now >= until);
    BackupStatus { last_backup_at, last_backup_file, backup_count: files.len(), backups_dir: backups.display().to_string(), interval_days: state.interval_days, snoozed_until: state.snoozed_until, due }
}

pub fn backup_now(catalog: &Path, backups: &Path) -> Result<String, String> {
    if !catalog.is_file() { return Err(format!("catalog not found: {}", catalog.display())); }
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let now = unix_now();
    let base = format!("catalog-{}.sqlite", utc_stamp(now));
    let mut destination = backups.join(&base);
    let mut collision = 1u32;
    while destination.exists() { destination = backups.join(format!("catalog-{}-{collision:02}.sqlite", utc_stamp(now))); collision += 1; }
    let connection = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let destination_sql = destination.to_string_lossy().to_string();
    if let Err(error) = connection.execute("VACUUM INTO ?1", [&destination_sql]) {
        let _ = std::fs::remove_file(&destination);
        return Err(error.to_string());
    }
    let previous = read_state(backups);
    let state = BackupState { catalog: file_stamp(catalog).ok(), wal: file_stamp(&wal_path(catalog)).ok(), backed_up_at: Some(now), last_backup_file: destination.file_name().map(|v| v.to_string_lossy().into_owned()), interval_days: previous.interval_days, snoozed_until: None };
    std::fs::write(backups.join(STATE_FILE), serde_json::to_vec(&state).map_err(|e| e.to_string())?)
        .map_err(|e| format!("backup created but state could not be written: {e}"))?;
    rotate_backups(backups).map_err(|e| e.to_string())?;
    Ok(destination.display().to_string())
}

pub fn set_backup_interval(backups: &Path, days: u32) -> Result<(), String> {
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let mut state = read_state(backups); state.interval_days = days;
    write_state(backups, &state)
}

pub fn snooze_backup(backups: &Path, days: u32) -> Result<(), String> {
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let mut state = read_state(backups); state.snoozed_until = Some(unix_now().saturating_add(u64::from(days) * 86_400));
    write_state(backups, &state)
}

fn write_state(backups: &Path, state: &BackupState) -> Result<(), String> {
    std::fs::write(backups.join(STATE_FILE), serde_json::to_vec(state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

#[tauri::command(async)]
pub fn get_backup_status(state: tauri::State<'_, crate::commands::AppInfoState>) -> BackupStatus {
    backup_status(Path::new(&state.0.catalog), Path::new(&state.0.backups))
}

#[tauri::command]
pub async fn backup_catalog_now(state: tauri::State<'_, crate::commands::AppInfoState>) -> Result<String, String> {
    let catalog = PathBuf::from(&state.0.catalog); let backups = PathBuf::from(&state.0.backups);
    tauri::async_runtime::spawn_blocking(move || backup_now(&catalog, &backups)).await.map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn set_backup_interval_days(state: tauri::State<'_, crate::commands::AppInfoState>, days: u32) -> Result<(), String> {
    set_backup_interval(Path::new(&state.0.backups), days)
}

#[tauri::command(async)]
pub fn snooze_backup_reminder(state: tauri::State<'_, crate::commands::AppInfoState>, days: u32) -> Result<(), String> {
    snooze_backup(Path::new(&state.0.backups), days)
}

pub fn rotate_backups(backups: &Path) -> std::io::Result<()> {
    let mut files = backup_files(backups);
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for stale in files.into_iter().skip(10) { std::fs::remove_file(stale)?; }
    Ok(())
}

fn backup_files(backups: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(backups).into_iter().flatten().filter_map(Result::ok).map(|entry| entry.path())
        .filter(|path| path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("catalog-") && name.ends_with(".sqlite"))).collect()
}

fn wal_path(catalog: &Path) -> PathBuf { let mut value = catalog.as_os_str().to_os_string(); value.push("-wal"); PathBuf::from(value) }
fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let metadata = std::fs::metadata(path)?;
    let modified_ns = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    Ok(FileStamp { size: metadata.len(), modified_ns })
}
fn utc_stamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64; let second_of_day = seconds % 86_400; let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097; let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1; let month = mp + if mp < 10 { 3 } else { -9 }; if month <= 2 { year += 1; }
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", second_of_day / 3600, second_of_day / 60 % 60, second_of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("ddreams-backup-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap(); path
    }
    fn tiny_catalog(path: &Path) { let conn = Connection::open(path).unwrap(); conn.execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES('safe');").unwrap(); }

    #[test]
    fn status_is_due_without_backup_and_not_due_after_recent_backup() {
        let root = temp_root(); let catalog = root.join("catalog.sqlite"); tiny_catalog(&catalog); let backups = root.join("backups");
        assert!(backup_status(&catalog, &backups).due);
        backup_now(&catalog, &backups).unwrap(); assert!(!backup_status(&catalog, &backups).due);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn status_respects_snooze_and_never_interval() {
        let root = temp_root(); let catalog = root.join("catalog.sqlite"); tiny_catalog(&catalog); let backups = root.join("backups");
        snooze_backup(&backups, 7).unwrap(); assert!(!backup_status(&catalog, &backups).due);
        set_backup_interval(&backups, 0).unwrap(); assert!(!backup_status(&catalog, &backups).due);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn backup_now_creates_file_and_updates_status() {
        let root = temp_root(); let catalog = root.join("catalog.sqlite"); tiny_catalog(&catalog); let backups = root.join("backups");
        let created = backup_now(&catalog, &backups).unwrap(); assert!(Path::new(&created).is_file());
        let status = backup_status(&catalog, &backups); assert!(status.last_backup_at.is_some()); assert_eq!(status.backup_count, 1);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn old_state_without_new_fields_still_parses() {
        let state: BackupState = serde_json::from_str(r#"{"catalog":null,"wal":null,"backed_up_at":123}"#).unwrap();
        assert_eq!(state.backed_up_at, Some(123)); assert_eq!(state.interval_days, DEFAULT_INTERVAL_DAYS);
    }
    #[test]
    fn rotation_keeps_the_ten_newest_named_backups() {
        let root = temp_root(); let backups = root.join("backups"); std::fs::create_dir_all(&backups).unwrap();
        for day in 1..=12 { std::fs::write(backups.join(format!("catalog-202601{day:02}-000000.sqlite")), []).unwrap(); }
        rotate_backups(&backups).unwrap(); assert_eq!(backup_files(&backups).len(), 10); assert!(utc_stamp(0).starts_with("19700101-000000"));
        let _ = std::fs::remove_dir_all(root);
    }
}
