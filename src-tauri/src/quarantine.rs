use crate::{
    ai::AiState,
    commands::{
        AppInfoState, DatabaseState, ExifState, FileManagerState, FileProgress, FormatsState,
    },
    db::Db,
    file_ops::{ConflictPolicy, DeleteInfo},
    formats::FormatTable,
    incoming,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, State};

const SETTINGS_KEY: &str = "quarantine";
pub(crate) const DAY: i64 = 86_400;
pub(crate) const EDIT_MTIME_GRACE: i64 = 60;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preferences {
    folder: String,
    pub move_after_days: u32,
    pub delete_after_days: u32,
    #[serde(default = "default_edit_after_days")]
    pub edit_after_days: u32,
    pub manage_from: String,
}

fn default_edit_after_days() -> u32 { 90 }

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    #[serde(flatten)]
    preferences: Preferences,
    incoming_folder: String,
    quarantine_folder: String,
}

impl Settings {
    pub fn edit_after_days(&self) -> u32 {
        self.preferences.edit_after_days
    }

    pub fn incoming(&self) -> &Path {
        Path::new(&self.incoming_folder)
    }
    pub fn quarantine(&self) -> &Path {
        Path::new(&self.quarantine_folder)
    }
    pub fn manages(&self, photo: &incoming::Photo) -> bool {
        photo
            .capture
            .as_deref()
            .and_then(|date| date.get(..10))
            .is_some_and(|day| day >= self.preferences.manage_from.as_str())
    }

    #[cfg(test)]
    pub fn fixture(incoming: &Path, quarantine: &Path) -> Self {
        make_settings(
            incoming.to_string_lossy().into_owned(),
            Preferences {
                folder: quarantine.to_string_lossy().into_owned(),
                move_after_days: 60,
                delete_after_days: 30,
                edit_after_days: default_edit_after_days(),
                manage_from: "2024-06-01".into(),
            },
        )
    }
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) fn modified_after_mark(modified: SystemTime, since: i64) -> bool {
    modified.duration_since(UNIX_EPOCH).ok()
        .is_some_and(|age| age.as_secs() as i64 > since.saturating_add(EDIT_MTIME_GRACE))
}

fn default_folder(incoming_folder: &str) -> String {
    Path::new(incoming_folder)
        .parent()
        .filter(|p| p.is_absolute())
        .map(|p| p.join("_Karanténa").to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn make_settings(incoming_folder: String, preferences: Preferences) -> Settings {
    let quarantine_folder = if preferences.folder.is_empty() {
        default_folder(&incoming_folder)
    } else {
        preferences.folder.clone()
    };
    Settings {
        preferences,
        incoming_folder,
        quarantine_folder,
    }
}

fn configured_path(values: &Map<String, Value>) -> &str {
    values
        .get("incomingFolder")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn load(path: &Path, db: &Db, writable: bool) -> Result<Settings, String> {
    let incoming_folder = incoming::configured_folder(path, db, writable)?;
    let today = db
        .lock()
        .map_err(|e| e.to_string())?
        .query_row("SELECT date('now','localtime')", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let default = Preferences {
        folder: String::new(),
        move_after_days: 60,
        delete_after_days: 30,
        edit_after_days: default_edit_after_days(),
        manage_from: today,
    };
    let mut result = None;
    incoming::update_settings(path, |values| {
        let current = values.get(SETTINGS_KEY);
        let preferences = match current {
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
            None => default,
        };
        let initialize = writable && current.is_none();
        if initialize {
            values.insert(
                SETTINGS_KEY.into(),
                serde_json::to_value(&preferences).map_err(|e| e.to_string())?,
            );
        }
        let source = if values.contains_key("incomingFolder") {
            configured_path(values).to_owned()
        } else {
            incoming_folder
        };
        result = Some(make_settings(source, preferences));
        Ok(initialize)
    })?;
    result.ok_or_else(|| "Nastavenia karantény sa nedajú načítať".into())
}

pub(crate) fn initialize(
    path: &Path,
    db: &Db,
    writer: &crate::journal::CatalogWriter,
) -> Result<(), String> {
    let settings = load(path, db, true)?;
    writer.sync_incoming(&settings.incoming_folder)
}

pub(crate) fn quarantine_path(values: &Map<String, Value>, input: &str) -> String {
    let custom = values
        .get(SETTINGS_KEY)
        .and_then(|v| v.get("folder"))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    custom
        .map(str::to_owned)
        .unwrap_or_else(|| default_folder(input))
}

pub(crate) fn validate_incoming_setting(
    values: &Map<String, Value>,
    input: &str,
) -> Result<(), String> {
    let quarantine = quarantine_path(values, input);
    if !input.is_empty() && quarantine.is_empty() {
        return Err(
            "Importovací priečinok musí mať nadradený priečinok alebo nastavenú karanténu".into(),
        );
    }
    validate_roots(Path::new(input), Path::new(&quarantine))
}

/// Reject traversal, device paths, alternate streams, symlinks and junctions. Canonicalize
/// the deepest existing ancestor so aliases (including Windows short names) share a scope.
pub(crate) fn checked_path(path: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    let normal_prefix = matches!(path.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::UNC(..)));
    #[cfg(not(windows))]
    let normal_prefix = true;
    if !normal_prefix
        || !path.is_absolute()
        || path.components().any(|part| match part {
            Component::ParentDir => true,
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                name.contains(':') || name.ends_with(['.', ' '])
            }
            _ => false,
        })
    {
        return Err(format!(
            "Nepovolená cesta pre karanténu: {}",
            path.display()
        ));
    }
    let mut existing = None;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if reparse || metadata.file_type().is_symlink() {
                    return Err(format!(
                        "Karanténa nepoužíva odkazy ani pripájacie body: {}",
                        ancestor.display()
                    ));
                }
                existing.get_or_insert(ancestor);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", ancestor.display())),
        }
    }
    let existing = existing.ok_or_else(|| format!("Cesta nie je dostupná: {}", path.display()))?;
    let canonical = existing
        .canonicalize()
        .map_err(|e| format!("{}: {e}", existing.display()))?;
    Ok(canonical.join(path.strip_prefix(existing).map_err(|e| e.to_string())?))
}

pub(crate) fn inside(path: &Path, root: &Path) -> bool {
    incoming::is_inside(&path.to_string_lossy(), &root.to_string_lossy())
}

pub(crate) fn validate_roots(source: &Path, quarantine: &Path) -> Result<(), String> {
    let resolve = |path: &Path| {
        if path.as_os_str().is_empty() {
            return Ok(None);
        }
        if path.exists() && !path.is_dir() {
            return Err(format!("Cesta nie je priečinok: {}", path.display()));
        }
        checked_path(path).map(Some)
    };
    let source = resolve(source)?;
    let quarantine = resolve(quarantine)?;
    if let (Some(source), Some(quarantine)) = (source, quarantine) {
        if inside(&quarantine, &source)
            || inside(&source, &quarantine)
            || crate::journal::normalize_fs_path(&source.to_string_lossy())
                == crate::journal::normalize_fs_path(&quarantine.to_string_lossy())
        {
            return Err("Importovací priečinok a karanténa sa nesmú prekrývať".into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Move,
    Delete,
    StaleEdit,
}

impl Stage {
    pub fn root(self, settings: &Settings) -> &Path {
        match self {
            Self::Move | Self::StaleEdit => settings.incoming(),
            Self::Delete => settings.quarantine(),
        }
    }
}

pub(crate) fn due_candidates(
    conn: &Connection,
    settings: &Settings,
    formats: &FormatTable,
    stage: Stage,
    at: i64,
    id: Option<i64>,
) -> Result<Vec<incoming::Photo>, String> {
    let days = match stage {
        Stage::Move => settings.preferences.move_after_days,
        Stage::Delete => settings.preferences.delete_after_days,
        Stage::StaleEdit => settings.preferences.edit_after_days,
    };
    if days == 0 {
        return Ok(Vec::new());
    }
    let action_filter = match stage {
        Stage::Move => format!(
            "incoming_seen_at <= ?1 AND quarantined_at IS NULL AND edit_pending_since IS NULL
             AND coalesce(rating,0)=0 AND trim(coalesce(label,''))='' AND coalesce(tagged,0)=0
             AND NOT EXISTS(SELECT 1 FROM asset_categories c WHERE c.asset_id=a.id)
             AND {}",
            formats.sql_in("ext_lower", &["image", "raw"])
        ),
        Stage::Delete => "quarantined_at <= ?1 AND quarantine_origin IS NOT NULL".into(),
        Stage::StaleEdit => format!(
            "edit_pending_since <= ?1 AND quarantined_at IS NULL AND {}",
            formats.sql_in("ext_lower", &["image", "raw"])
        ),
    };
    let capture = match stage {
        Stage::Move => {
            "CASE WHEN date(substr(exif_date,1,10),'+0 days')=substr(exif_date,1,10) THEN substr(exif_date,1,10) END"
        }
        Stage::Delete | Stage::StaleEdit => "NULL",
    };
    let id_filter = if id.is_some() { " AND id=?2" } else { "" };
    let mut stmt = conn.prepare(&format!(
        "SELECT id,path,{capture} FROM assets a WHERE is_listed=1 AND id>0 AND path IS NOT NULL AND {action_filter}{id_filter} ORDER BY id"
    )).map_err(|e| e.to_string())?;
    stmt.raw_bind_parameter(1, at - i64::from(days) * DAY)
        .map_err(|e| e.to_string())?;
    if let Some(id) = id {
        stmt.raw_bind_parameter(2, id).map_err(|e| e.to_string())?;
    }
    let root = stage.root(settings);
    stmt.raw_query()
        .mapped(|r| {
            Ok(incoming::Photo {
                id: r.get(0)?,
                path: r.get(1)?,
                capture: r.get(2)?,
            })
        })
        .filter(|row| {
            row.as_ref().is_err()
                || row
                    .as_ref()
                    .is_ok_and(|photo| inside(Path::new(&photo.path), root))
        })
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())
}

#[derive(Debug)]
pub(crate) struct Review {
    pub settings: Settings,
    pub moves: Vec<Snapshot>,
    pub deletes: Vec<Snapshot>,
    pub stale_edits: Vec<Snapshot>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub id: i64,
    pub path: PathBuf,
    pub files: Vec<(PathBuf, u64, SystemTime, Option<SystemTime>)>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewList {
    pub ids: Vec<i64>,
    pub bytes: u64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewPreview {
    pub moves: ReviewList,
    pub deletes: ReviewList,
    pub stale_edits: ReviewList,
    pub edit_after_days: u32,
    pub no_bin_drives: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewSelection {
    pub moves: bool,
    pub deletes: bool,
    pub stale_edits: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outcome {
    pub moved: usize,
    pub restored: usize,
    pub deleted: usize,
    pub failures: Vec<String>,
    pub affected_folder_paths: Vec<String>,
    #[serde(skip)]
    pub purged: Vec<i64>,
}

#[tauri::command(async)]
pub(crate) fn get_quarantine_settings(
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
) -> Result<Settings, String> {
    load(Path::new(&info.0.settings), &db.0, !info.0.readonly)
}

#[tauri::command(async)]
pub(crate) fn set_quarantine_settings(
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    mut preferences: Preferences,
) -> Result<Settings, String> {
    if info.0.readonly {
        return Err("Katalóg je iba na čítanie".into());
    }
    let _operation = manager.0.lock_operation()?;
    preferences.folder = preferences.folder.trim().to_owned();
    let date = &preferences.manage_from;
    let valid: bool =
        db.0.lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT coalesce(length(?1)=10 AND date(?1,'+0 days')=?1,0)",
                [date],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
    if !valid {
        return Err("Zadajte platný dátum snímania".into());
    }
    incoming::update_settings(Path::new(&info.0.settings), |values| {
        let old = quarantine_path(values, configured_path(values));
        values.insert(
            SETTINGS_KEY.into(),
            serde_json::to_value(&preferences).map_err(|e| e.to_string())?,
        );
        validate_incoming_setting(values, configured_path(values))?;
        manager
            .0
            .check_quarantine_relocation(&old, &quarantine_path(values, configured_path(values)))?;
        Ok(true)
    })?;
    load(Path::new(&info.0.settings), &db.0, true)
}

#[tauri::command(async)]
pub(crate) fn preview_quarantine_review(
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>,
) -> Result<ReviewPreview, String> {
    if info.0.readonly {
        return Ok(ReviewPreview::default());
    }
    let _operation = manager.0.lock_operation()?;
    let settings = load(Path::new(&info.0.settings), &db.0, true)?;
    manager
        .0
        .prepare_quarantine_review(settings, &formats.0, Some(&exif.0))
}

fn progress(app: &AppHandle, operation: &str, done: usize, total: usize) {
    let _ = app.emit(
        "file-operation-progress",
        FileProgress {
            operation: operation.into(),
            done,
            total,
        },
    );
}

#[tauri::command(async)]
pub(crate) fn execute_quarantine_review(
    app: AppHandle,
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>,
    ai: State<'_, AiState>,
    confirmed_no_bin_drives: Vec<String>,
    selection: ReviewSelection,
) -> Result<Outcome, String> {
    let result = {
        let _operation = manager.0.lock_operation()?;
        let settings = load(Path::new(&info.0.settings), &db.0, false)?;
        manager.0.execute_quarantine_review(
            &settings,
            &formats.0,
            Some(&exif.0),
            selection,
            &confirmed_no_bin_drives,
            |done, total| progress(&app, "Spracúvam karanténu", done, total),
        )?
    };
    ai.0.forget(result.purged.clone());
    Ok(result)
}

#[tauri::command(async)]
pub(crate) fn move_to_quarantine(
    app: AppHandle,
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    formats: State<'_, FormatsState>,
    asset_ids: Vec<i64>,
    conflict: ConflictPolicy,
    suggestions_only: bool,
) -> Result<Outcome, String> {
    let _operation = manager.0.lock_operation()?;
    let settings = load(Path::new(&info.0.settings), &db.0, false)?;
    manager.0.move_to_quarantine(
        &asset_ids,
        &settings,
        &formats.0,
        conflict,
        suggestions_only,
        |done, total| progress(&app, "Presúvam do karantény", done, total),
    )
}

#[tauri::command(async)]
pub(crate) fn restore_from_quarantine(
    app: AppHandle,
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
) -> Result<Outcome, String> {
    let _operation = manager.0.lock_operation()?;
    let settings = load(Path::new(&info.0.settings), &db.0, false)?;
    manager
        .0
        .restore_from_quarantine(&asset_ids, &settings, |done, total| {
            progress(&app, "Vraciam z karantény", done, total)
        })
}

#[tauri::command(async)]
pub(crate) fn get_quarantine_delete_info(
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
) -> Result<DeleteInfo, String> {
    let _operation = manager.0.lock_operation()?;
    let settings = load(Path::new(&info.0.settings), &db.0, false)?;
    manager.0.quarantine_delete_info(&asset_ids, &settings)
}

#[tauri::command(async)]
pub(crate) fn delete_from_quarantine(
    app: AppHandle,
    info: State<'_, AppInfoState>,
    db: State<'_, DatabaseState>,
    manager: State<'_, FileManagerState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
    confirmed_no_bin_drives: Vec<String>,
) -> Result<Outcome, String> {
    let result = {
        let _operation = manager.0.lock_operation()?;
        let settings = load(Path::new(&info.0.settings), &db.0, false)?;
        manager.0.delete_from_quarantine(
            &asset_ids,
            &settings,
            &confirmed_no_bin_drives,
            |done, total| progress(&app, "Mažem z karantény", done, total),
        )?
    };
    ai.0.forget(result.purged.clone());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_preferences_default_to_ninety_days() {
        let old = serde_json::json!({"folder":"","moveAfterDays":60,"deleteAfterDays":30,"manageFrom":"2024-06-01"});
        let preferences: Preferences = serde_json::from_value(old).unwrap();
        assert_eq!(preferences.edit_after_days, 90);
    }

    #[test]
    fn stale_edits_are_due_at_the_cutoff_only_in_import_and_can_be_disabled() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,path TEXT,is_listed INTEGER,ext_lower TEXT,
            exif_date TEXT,incoming_seen_at INTEGER,quarantined_at INTEGER,edit_pending_since INTEGER,
            rating INTEGER,label TEXT,tagged INTEGER);
            CREATE TABLE asset_categories(asset_id INTEGER);").unwrap();
        let root = std::env::temp_dir();
        let mut settings = Settings::fixture(&root.join("import"), &root.join("quarantine"));
        let at = 1_800_000_000;
        let cutoff = at - 90 * DAY;
        for (id, folder, pending, quarantined, listed) in [
            (1, "import", Some(cutoff), None, 1),
            (2, "import", Some(cutoff + 1), None, 1),
            (3, "import-old", Some(cutoff - 1), None, 1),
            (4, "import", Some(cutoff - 1), Some(at), 1),
            (5, "import", Some(cutoff - 1), None, 0),
            (6, "import", None, None, 1),
        ] {
            let path = root.join(folder).join(format!("{id}.jpg"));
            db.execute("INSERT INTO assets(id,path,is_listed,ext_lower,exif_date,incoming_seen_at,quarantined_at,edit_pending_since)
                VALUES(?1,?2,?3,'jpg','2020-01-01',?4,?5,?6)",
                (id, path.to_string_lossy().as_ref(), listed, cutoff, quarantined, pending)).unwrap();
        }
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let ids = |settings: &Settings, stage| due_candidates(&db, settings, &formats, stage, at, None)
            .unwrap().into_iter().map(|photo| photo.id).collect::<Vec<_>>();
        assert_eq!(ids(&settings, Stage::Move), vec![6]);
        assert_eq!(ids(&settings, Stage::StaleEdit), vec![1]);
        settings.preferences.edit_after_days = 0;
        assert!(ids(&settings, Stage::StaleEdit).is_empty());
    }
}
