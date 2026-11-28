use crate::{
    ai::{AiService, AiState},
    commands::{AppInfoState, DatabaseState, ListingState, CatalogWriterState},
    db::Db,
    formats::FormatTable,
    journal::DeletionSuggestionChange,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::SystemTime,
};
use tauri::State;

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn read_settings(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn write_setting(path: &Path, key: &str, value: Value) -> Result<(), String> {
    update_settings(path, |settings| {
        settings.insert(key.to_owned(), value);
        Ok(true)
    })
}

pub(crate) fn update_settings(path: &Path, update: impl FnOnce(&mut Map<String, Value>) -> Result<bool, String>) -> Result<(), String> {
    let _lock = SETTINGS_LOCK.lock().map_err(|e| e.to_string())?;
    let mut settings = read_settings(path)?;
    if update(&mut settings)? {
        std::fs::write(path, serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn normalized(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).replace('/', "\\").to_lowercase()
}

pub(crate) fn is_inside(path: &str, folder: &str) -> bool {
    !folder.is_empty() && normalized(path).strip_prefix(&normalized(folder))
        .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn detect_folder(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM (
            SELECT path FROM folders WHERE name='Panasonic' COLLATE NOCASE
            UNION SELECT path FROM folder_roots WHERE name='Panasonic' COLLATE NOCASE
         ) WHERE path IS NOT NULL AND trim(path)<>''",
    )?;
    let mut candidates = Vec::new();
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let path = row?;
        if Path::new(&path).is_dir() {
            let prefix = format!("{}\\", normalized(&path));
            candidates.push((path, prefix, 0usize));
        }
    }
    if candidates.is_empty() { return Ok(None); }

    let mut stmt = conn.prepare("SELECT path FROM assets WHERE is_listed=1 AND path IS NOT NULL")?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let asset_path = normalized(&row?);
        for (_, prefix, count) in &mut candidates {
            if asset_path.starts_with(prefix.as_str()) { *count += 1; }
        }
    }
    Ok(candidates.into_iter().filter(|(_, _, count)| *count > 0)
        .min_by(|(a, _, a_count), (b, _, b_count)| {
            b_count.cmp(a_count).then_with(|| a.chars().count().cmp(&b.chars().count())).then_with(|| a.cmp(b))
        })
        .map(|(path, _, _)| path))
}

pub(crate) fn configured_folder(settings: &Path, db: &Db, writable: bool) -> Result<String, String> {
    let _lock = SETTINGS_LOCK.lock().map_err(|e| e.to_string())?;
    let mut values = read_settings(settings)?;
    match values.get("incomingFolder") {
        // Null marks a deliberate reset. Older versions also saved empty strings after a failed detection.
        Some(Value::Null) => return Ok(String::new()),
        Some(Value::String(path)) if !path.trim().is_empty() => return Ok(path.clone()),
        _ => {}
    }
    let folder = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        detect_folder(&conn).map_err(|e| e.to_string())?
    };
    if let Some(path) = &folder {
        if writable {
            values.insert("incomingFolder".into(), Value::String(path.clone()));
            std::fs::write(settings, serde_json::to_vec_pretty(&values).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(folder.unwrap_or_default())
}

#[tauri::command(async)]
pub fn get_incoming_folder(state: State<'_, AiState>) -> Result<String, String> {
    state.0.incoming_folder()
}

#[tauri::command(async)]
pub fn set_incoming_folder(info: State<'_, AppInfoState>, manager: State<'_, crate::commands::FileManagerState>, path: String) -> Result<String, String> {
    if info.0.readonly { return Err("Katalóg je iba na čítanie".into()); }
    let _operation = manager.0.lock_operation()?;
    let path = path.trim();
    if !path.is_empty() && !Path::new(path).is_dir() {
        return Err("Priečinok neexistuje alebo nie je dostupný.".into());
    }
    let value = if path.is_empty() { Value::Null } else { Value::String(path.to_owned()) };
    update_settings(Path::new(&info.0.settings), |settings| {
        crate::quarantine::validate_incoming_setting(settings, path)?;
        let old_incoming = settings.get("incomingFolder").and_then(Value::as_str).unwrap_or_default();
        let old = crate::quarantine::quarantine_path(settings, old_incoming);
        let new = crate::quarantine::quarantine_path(settings, path);
        manager.0.check_quarantine_relocation(&old, &new)?;
        settings.insert("incomingFolder".into(), value);
        Ok(true)
    })?;
    manager.0.sync_incoming(path)?;
    Ok(path.to_owned())
}

pub(crate) struct Photo {
    pub id: i64,
    pub path: String,
    pub capture: Option<String>,
}

pub(crate) fn photos(db: &Db, formats: &FormatTable) -> Result<Vec<Photo>, String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    let clause = formats.sql_in("ext_lower", &["image", "raw"]);
    let mut stmt = conn.prepare(&format!(
        "SELECT id,path,exif_date FROM assets WHERE id>0 AND is_listed=1 AND path IS NOT NULL AND {clause} ORDER BY id"
    )).map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| Ok(Photo { id: row.get(0)?, path: row.get(1)?, capture: row.get(2)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

pub(crate) fn contains_folder(path: &str, folder: &str) -> bool {
    !folder.is_empty() && (normalized(path) == normalized(folder) || is_inside(path, folder))
}

fn exif_capture(value: &str) -> Option<String> {
    if crate::commands::exif_date_seconds(value).is_some() { return Some(value.to_owned()); }
    let date = value.get(..10)?;
    let tail = value.get(10..)?;
    if !date.is_ascii() { return None; }
    let normalized = format!("{}{tail}", date.replace(':', "-"));
    crate::commands::exif_date_seconds(&normalized).map(|_| normalized)
}

const CAPTURE_CACHE_LIMIT: usize = 50_000;
type CaptureKey = (String, u64, SystemTime);

#[derive(Default)]
struct CaptureCache {
    dates: HashMap<CaptureKey, Option<String>>,
    order: VecDeque<CaptureKey>,
}

impl CaptureCache {
    fn insert(&mut self, key: CaptureKey, value: Option<String>) {
        if self.dates.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        if self.dates.len() > CAPTURE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.dates.remove(&oldest);
            }
        }
    }
}

static CAPTURE_CACHE: LazyLock<Mutex<CaptureCache>> = LazyLock::new(Mutex::default);

pub(crate) fn fill_capture_dates(exif: &crate::exiftool::ExifTool, photos: &mut [Photo], selected: impl Fn(i64) -> bool) {
    fill_cached_capture_dates(&CAPTURE_CACHE, photos, selected, |paths| {
        Ok(crate::native_exif::metadata_batch(exif, paths, &["DateTimeOriginal"]).into_iter().flatten().collect())
    });
}

pub(crate) fn catalog_capture_date(value: &str) -> Option<String> {
    let value = exif_capture(value)?;
    let value = value.get(..19)?;
    crate::commands::exif_date_seconds(value)?;
    Some(format!("{}T{}", &value[..10], &value[11..]))
}

#[cfg(test)]
#[test]
fn catalog_capture_date_uses_local_time_with_t_separator() {
    let expected = Some("2026-08-31T16:16:00");
    assert_eq!(catalog_capture_date("2026:08:31 16:16:00").as_deref(), expected);
    assert_eq!(catalog_capture_date("2026-08-31 16:16:00").as_deref(), expected);
    assert_eq!(catalog_capture_date("2026-08-31T16:16:00+02:00").as_deref(), expected);
    assert_eq!(catalog_capture_date("2026-02-31T16:16:00"), None);
}

fn fill_cached_capture_dates(
    cache: &Mutex<CaptureCache>,
    photos: &mut [Photo],
    selected: impl Fn(i64) -> bool,
    mut read_dates: impl FnMut(&[PathBuf]) -> std::io::Result<Vec<Value>>,
) {
    let mut missing: Vec<_> = photos.iter_mut().filter(|photo| selected(photo.id) &&
        photo.capture.as_deref().and_then(crate::commands::exif_date_seconds).is_none()).collect();
    for chunk in missing.chunks_mut(64) {
        let stamped: Vec<_> = chunk.iter_mut().filter_map(|photo| {
            let meta = std::fs::metadata(&photo.path).ok()?;
            let key = (normalized(&photo.path), meta.len(), meta.modified().ok()?);
            meta.is_file().then_some((photo, key))
        }).collect();
        if stamped.is_empty() { continue; }

        // Serialize each batch so overlapping callers only read each uncached file once.
        let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending = Vec::new();
        for (photo, key) in stamped {
            if let Some(value) = cache.dates.get(&key) {
                photo.capture.clone_from(value);
            } else {
                pending.push((photo, key));
            }
        }
        if pending.is_empty() { continue; }
        let paths: Vec<_> = pending.iter().map(|(photo, _)| PathBuf::from(&photo.path)).collect();
        let Ok(rows) = read_dates(&paths) else { continue; };
        let by_path = rows.iter().filter_map(|row| {
            if row.as_object()?.keys().any(|key| key == "Error" || key.ends_with(":Error")) {
                return None;
            }
            let path = normalized(row["SourceFile"].as_str()?);
            let date = row.as_object()?.iter()
                .find(|(key, _)| key.ends_with(":DateTimeOriginal"))
                .and_then(|(_, value)| value.as_str())
                .and_then(exif_capture);
            Some((path, date))
        }).collect::<HashMap<_, _>>();
        for (photo, key) in pending {
            // An absent date on a successful row is cacheable. Missing/error rows are retried.
            if let Some(value) = by_path.get(&key.0) {
                photo.capture.clone_from(value);
                cache.insert(key, value.clone());
            }
        }
    }
}

pub(crate) fn has_incoming_photo(db: &Db, formats: &FormatTable, incoming: &str, ids: &[i64]) -> Result<bool, String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    let clause = formats.sql_in("ext_lower", &["image", "raw"]);
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT path FROM assets WHERE is_listed=1 AND {clause} AND id IN ({marks}) AND path IS NOT NULL"
        )).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            if is_inside(&row.map_err(|e| e.to_string())?, incoming) { return Ok(true); }
        }
    }
    Ok(false)
}

fn selection_inside(db: &Db, formats: &FormatTable, listing: &crate::listing::ListingService, ids: &[i64], incoming: &str) -> Result<bool, String> {
    if ids.is_empty() || incoming.is_empty() { return Ok(false); }
    let conn = db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT path,ext_lower FROM assets WHERE id=?1 AND is_listed=1")
        .map_err(|e| e.to_string())?;
    for id in ids {
        let asset = if *id > 0 {
            stmt.query_row([id], |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)))
                .optional().map_err(|e| e.to_string())?
        } else {
            listing.asset(*id).filter(|asset| !asset.is_directory).map(|asset| (asset.path, asset.ext))
        };
        let Some((Some(path), ext)) = asset else { return Ok(false); };
        if !is_inside(&path, incoming) || !matches!(formats.group(&ext), Some("image" | "raw")) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Serialize, Deserialize)]
pub struct FolderSuggestion {
    folder: String,
    share: f64,
    #[serde(default, rename = "exampleIds")]
    example_ids: Vec<i64>,
}

#[derive(Serialize)]
pub struct FolderSuggestions {
    eligible: bool,
    folders: Vec<FolderSuggestion>,
}

#[tauri::command(async)]
pub fn suggest_incoming_folders(
    db: State<'_, DatabaseState>,
    formats: State<'_, crate::commands::FormatsState>,
    listing: State<'_, ListingState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
    include_examples: Option<bool>,
) -> Result<FolderSuggestions, String> {
    let folder = ai.0.incoming_folder()?;
    let eligible = selection_inside(&db.0, &formats.0, &listing.0, &asset_ids, &folder)?;
    let mut response = FolderSuggestions { eligible, folders: Vec::new() };
    if eligible && ai.0.vector_queries_available() && asset_ids.iter().any(|id| *id > 0) {
        response.folders = rank_folders(&ai.0, &db.0, &asset_ids, &folder, include_examples.unwrap_or(false))?;
    }
    Ok(response)
}

pub(crate) fn trace_timing(stage: &str, started: std::time::Instant, count: usize) {
    if std::env::var_os("DDREAMS_INCOMING_TIMINGS").is_some() {
        eprintln!("incoming {stage}: {:.1} ms ({count})", started.elapsed().as_secs_f64() * 1000.0);
    }
}

fn resolve_days(ai: &AiService, db: &Db, ids: &[i64], days: &mut HashMap<i64, String>) -> Result<(), String> {
    let mut rows = Vec::new();
    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut stmt = conn.prepare(&format!(
                "SELECT id,path,exif_date FROM assets WHERE is_listed=1 AND path IS NOT NULL AND id IN ({marks})"
            )).map_err(|e| e.to_string())?;
            let photos = stmt.query_map(rusqlite::params_from_iter(chunk), |row| Ok(Photo {
                id: row.get(0)?, path: row.get(1)?, capture: row.get(2)?,
            })).map_err(|e| e.to_string())?;
            rows.extend(photos.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?);
        }
    }
    fill_capture_dates(ai.exif(), &mut rows, |_| true);
    for id in ids { days.entry(*id).or_default(); }
    for photo in rows {
        if let Some(capture) = photo.capture.filter(|date| crate::commands::exif_date_seconds(date).is_some()) {
            days.insert(photo.id, capture[..10].to_owned());
        }
    }
    Ok(())
}

pub(crate) fn rank_folders(ai: &AiService, db: &Db, ids: &[i64], incoming: &str, include_examples: bool) -> Result<Vec<FolderSuggestion>, String> {
    let mut requested = ids.to_vec();
    let mut days = HashMap::new();
    loop {
        let started = std::time::Instant::now();
        resolve_days(ai, db, &requested, &mut days)?;
        trace_timing("folder capture dates", started, requested.len());
        let started = std::time::Instant::now();
        let result = ai.rank_incoming_folders(ids, incoming, &days, include_examples)?;
        trace_timing("folder AI", started, ids.len());
        requested = result["missing_days"].as_array().into_iter().flatten()
            .filter_map(Value::as_i64).filter(|id| !days.contains_key(id)).collect();
        if requested.is_empty() {
            return serde_json::from_value(result["folders"].clone()).map_err(|e| e.to_string());
        }
    }
}

#[tauri::command(async)]
pub fn analyze_incoming_series(ai: State<'_, AiState>) -> Result<DeletionSuggestionChange, String> {
    ai.0.analyze_incoming(&[])
}

pub(crate) fn has_deletion_column(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('assets') WHERE name='delete_suggestion')", [], |row| row.get(0))
}

#[tauri::command(async)]
pub fn get_deletion_suggestions(db: State<'_, DatabaseState>) -> Result<Vec<i64>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if !has_deletion_column(&conn).map_err(|e| e.to_string())? { return Ok(Vec::new()); }
    let mut stmt = conn.prepare("SELECT id FROM assets WHERE is_listed=1 AND delete_suggestion=1")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| row.get(0)).map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn keep_suggested_assets(writer: State<'_, CatalogWriterState>, asset_ids: Option<Vec<i64>>) -> Result<DeletionSuggestionChange, String> {
    writer.0.keep_suggested_assets(asset_ids.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_cache_skips_repeat_reads_including_absent_dates() {
        let dir = std::env::temp_dir().join(format!("ddreams-incoming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = [dir.join("dated.jpg"), dir.join("undated.jpg")];
        for path in &paths { std::fs::write(path, b"photo").unwrap(); }
        let fresh_photos = || paths.iter().enumerate().map(|(i, path)| Photo {
            id: i as i64, path: path.to_str().unwrap().to_owned(), capture: None,
        }).collect::<Vec<_>>();
        let cache = Mutex::new(CaptureCache::default());

        let mut photos = fresh_photos();
        fill_cached_capture_dates(&cache, &mut photos, |_| true, |batch| {
            assert_eq!(batch, &paths[..]);
            Ok(vec![
                serde_json::json!({"SourceFile": batch[0], "ExifIFD:DateTimeOriginal": "2026:11:19 13:27:01"}),
                serde_json::json!({"SourceFile": batch[1]}),
            ])
        });
        assert_eq!(photos[0].capture.as_deref(), Some("2026-11-19 13:27:01"));
        assert_eq!(photos[1].capture, None);

        let mut photos = fresh_photos();
        fill_cached_capture_dates(&cache, &mut photos, |_| true, |_| panic!("Cached files were read again"));
        assert_eq!(photos[0].capture.as_deref(), Some("2026-11-19 13:27:01"));
        assert_eq!(photos[1].capture, None);

        std::fs::write(&paths[1], b"modified photo").unwrap();
        let mut photos = fresh_photos();
        let mut reads = 0;
        fill_cached_capture_dates(&cache, &mut photos, |_| true, |batch| {
            reads += 1;
            assert_eq!(batch, &paths[1..]);
            Ok(vec![serde_json::json!({"SourceFile": batch[0], "ExifIFD:DateTimeOriginal": "2026:11:20 09:00:00"})])
        });
        assert_eq!(reads, 1);
        assert_eq!(photos[1].capture.as_deref(), Some("2026-11-20 09:00:00"));
        for path in paths { std::fs::remove_file(path).unwrap(); }
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn detects_panasonic_with_the_most_listed_assets_and_retries_an_empty_setting() {
        let dir = std::env::temp_dir().join(format!("ddreams-incoming-detect-{}", std::process::id()));
        let stale = dir.join("A").join("Panasonic");
        let shorter = dir.join("B").join("Panasonic");
        let larger = dir.join("LongerPath").join("Panasonic");
        let missing = dir.join("Missing").join("Panasonic");
        for path in [&stale, &shorter, &larger] { std::fs::create_dir_all(path).unwrap(); }
        let db = std::sync::Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let settings = dir.join("settings.json");
        {
            let conn = db.lock().unwrap();
            conn.execute_batch("CREATE TABLE folder_roots(name TEXT,path TEXT);
                CREATE TABLE folders(name TEXT,path TEXT); CREATE TABLE assets(path TEXT,is_listed INTEGER);").unwrap();
            conn.execute("INSERT INTO folder_roots VALUES('panasonic',?)", [shorter.to_str().unwrap()]).unwrap();
            for path in [&stale, &larger, &missing] {
                conn.execute("INSERT INTO folders VALUES('Panasonic',?)", [path.to_str().unwrap()]).unwrap();
            }
            conn.execute("INSERT INTO assets VALUES(?,0)", [stale.join("unlisted.jpg").to_str().unwrap()]).unwrap();
            assert_eq!(detect_folder(&conn).unwrap(), None);
        }
        assert_eq!(configured_folder(&settings, &db, true).unwrap(), "");
        assert!(!settings.exists());
        std::fs::write(&settings, br#"{"incomingFolder":"","other":42}"#).unwrap();
        {
            let conn = db.lock().unwrap();
            for path in [missing.join("one.jpg"), missing.join("two.jpg"), missing.join("three.jpg"),
                shorter.join("one.jpg"), larger.join("one.jpg"), larger.join("sub").join("two.jpg"),
                stale.with_file_name("Panasonic extra").join("one.jpg")] {
                conn.execute("INSERT INTO assets VALUES(?,1)", [path.to_str().unwrap()]).unwrap();
            }
            assert_eq!(detect_folder(&conn).unwrap().as_deref(), larger.to_str());
            conn.execute("INSERT INTO assets VALUES(?,1)", [shorter.join("sub").join("two.jpg").to_str().unwrap()]).unwrap();
            assert_eq!(detect_folder(&conn).unwrap().as_deref(), shorter.to_str());
        }
        assert_eq!(configured_folder(&settings, &db, true).unwrap(), shorter.to_str().unwrap());
        let saved = read_settings(&settings).unwrap();
        assert_eq!(saved["incomingFolder"].as_str(), shorter.to_str());
        assert_eq!(saved["other"], 42);
        write_setting(&settings, "incomingFolder", Value::Null).unwrap();
        assert_eq!(configured_folder(&settings, &db, true).unwrap(), "");
        assert!(read_settings(&settings).unwrap()["incomingFolder"].is_null());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matches_folder_boundaries_and_normalizes_capture_dates() {
        assert!(is_inside(r"F:\Fotky\panasonic\DCIM\a.jpg", "f:/fotky/Panasonic/"));
        assert!(!is_inside(r"F:\Fotky\Panasonic extra\a.jpg", "f:/fotky/Panasonic"));
        assert_eq!(exif_capture("2026:11:19 13:27:01").as_deref(), Some("2026-11-19 13:27:01"));
        assert_eq!(exif_capture("missing"), None);
    }
}
