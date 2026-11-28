use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::folders::{ensure_folder, load_folder_paths, load_folder_roots, normalize_fs_path};
use super::history::record_change;
use super::values::{AssetValueState, read_asset_values, slovak_count};
use super::{CatalogWriter, ChangeHook, NewCatalogAsset};

#[derive(Clone, Debug)]
pub struct ListedAsset {
    pub temporary_id: i64,
    pub name: String,
    pub ext: String,
    pub size: Option<f64>,
    pub file_modified: Option<String>,
    pub path: String,
    pub folder_path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct AssetIdRemap {
    pub old_id: i64,
    pub new_id: i64,
}

pub(super) fn find_asset_by_path(transaction: &Transaction<'_>, path: &str) -> Result<Option<i64>, String> {
    transaction
        .query_row(
            "SELECT id FROM assets WHERE lower(rtrim(path,'/\\'))=lower(rtrim(?1,'/\\')) ORDER BY id LIMIT 1",
            [path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
}

pub(super) fn insert_asset(transaction: &Transaction<'_>, asset: &NewCatalogAsset) -> Result<(), String> {
    transaction.execute(
        "INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,size,width,height,bpp,caption,notes,author,rating,tagged,label,file_modified,exif_date,acdsee_date,md_dirty,is_orphan,path) VALUES(?1,?2,?3,?4,?5,1,?6,NULL,NULL,NULL,NULL,NULL,NULL,?7,?8,?9,?10,?11,NULL,NULL,NULL,?12)",
        (asset.id, asset.folder_id, &asset.name, &asset.ext, asset.ext.to_lowercase(), asset.size, asset.rating, i64::from(asset.tagged), &asset.label, &asset.file_modified, &asset.exif_date, &asset.path),
    ).map_err(|e| e.to_string())?;
    Ok(())
}

fn track_incoming(tx: &Transaction<'_>, id: i64, path: &str, folder: &str) -> Result<(), String> {
    if folder.is_empty() {
        return Ok(());
    }
    if crate::incoming::is_inside(path, folder) {
        tx.execute(
            "UPDATE assets SET incoming_seen_at=?2 WHERE id=?1 AND incoming_seen_at IS NULL",
            (id, crate::quarantine::now()),
        )
        .map_err(|e| e.to_string())?;
    } else {
        tx.execute(
            "UPDATE assets SET incoming_seen_at=NULL WHERE id=?1 AND incoming_seen_at IS NOT NULL",
            [id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fulfil_edit_with_sibling(tx: &Transaction<'_>, folder_id: i64, candidate: &ListedAsset) -> Result<(), String> {
    let Some(modified) = candidate.file_modified.as_deref() else {
        return Ok(());
    };
    let Some(stem) = Path::new(&candidate.name).file_stem() else {
        return Ok(());
    };
    let stem = stem.to_string_lossy().to_lowercase();
    let mut select = tx
        .prepare_cached(
            "SELECT id,name,ext_lower FROM assets
        WHERE folder_id=?1 AND is_listed=1 AND edit_pending_since IS NOT NULL AND edit_pending_since<unixepoch(?2)",
        )
        .map_err(|e| e.to_string())?;
    let matches = select
        .query_map((folder_id, modified), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let mut clear = tx
        .prepare_cached("UPDATE assets SET edit_pending_since=NULL WHERE id=?1")
        .map_err(|e| e.to_string())?;
    for (id, name, ext) in matches {
        if !ext.is_empty()
            && !ext.eq_ignore_ascii_case(&candidate.ext)
            && Path::new(&name)
                .file_stem()
                .is_some_and(|name| name.to_string_lossy().to_lowercase() == stem)
        {
            clear.execute([id]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn reconcile_incoming(tx: &Transaction<'_>, root: &str, scanned_folder: Option<&str>) -> Result<(), String> {
    let available = Path::new(root).is_dir();
    let now = crate::quarantine::now();
    let filter = if scanned_folder.is_some() {
        "folder_id IN (SELECT id FROM folders WHERE lower(rtrim(replace(path,'/','\\'),'\\'))=?1)"
    } else {
        "(incoming_seen_at IS NOT NULL OR (is_listed=1 AND path IS NOT NULL))"
    };
    let mut select = tx.prepare(&format!(
        "SELECT id,coalesce(path,''),incoming_seen_at,is_listed=1,edit_pending_since FROM assets WHERE {filter} ORDER BY id"
    )).map_err(|e| e.to_string())?;
    let mut update = tx
        .prepare("UPDATE assets SET incoming_seen_at=?2 WHERE id=?1")
        .map_err(|e| e.to_string())?;
    let mut clear = tx
        .prepare("UPDATE assets SET edit_pending_since=NULL WHERE id=?1")
        .map_err(|e| e.to_string())?;
    let mut fulfilled = 0;
    if let Some(folder) = scanned_folder {
        select
            .raw_bind_parameter(1, normalize_fs_path(folder))
            .map_err(|e| e.to_string())?;
    }
    let mut rows = select.raw_query();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let id: i64 = row.get(0).map_err(|e| e.to_string())?;
        let path: String = row.get(1).map_err(|e| e.to_string())?;
        let seen: Option<i64> = row.get(2).map_err(|e| e.to_string())?;
        let listed: Option<bool> = row.get(3).map_err(|e| e.to_string())?;
        let pending: Option<i64> = row.get(4).map_err(|e| e.to_string())?;
        let next = if listed != Some(true) || !crate::incoming::is_inside(&path, root) {
            None
        } else if !available {
            seen
        } else {
            match std::fs::metadata(&path) {
                Ok(info) if info.is_file() => {
                    if scanned_folder.is_none()
                        && pending.is_some_and(|since| {
                            info.modified()
                                .ok()
                                .is_some_and(|mtime| crate::quarantine::modified_after_mark(mtime, since))
                        })
                    {
                        fulfilled += clear.execute([id]).map_err(|e| e.to_string())?;
                    }
                    seen.or(Some(now))
                }
                Ok(_) => None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => seen,
            }
        };
        if next != seen {
            update.execute((id, next)).map_err(|e| e.to_string())?;
        }
    }
    if fulfilled > 0 {
        record_change(
            tx,
            "fulfil_pending_edits",
            &format!("Úpravy zistené v súboroch · {fulfilled}"),
        )?;
    }
    Ok(())
}

pub(super) fn resolve_write_assets(
    tx: &Transaction<'_>,
    asset_ids: &[i64],
    listed: &[ListedAsset],
    insert_missing: bool,
    require_temporary_id: bool,
    track: impl Fn(&Transaction<'_>, i64, &str) -> Result<(), String>,
) -> Result<(Vec<AssetValueState>, Vec<AssetIdRemap>, usize), String> {
    let mut ids = asset_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let candidates: HashMap<_, _> = listed.iter().map(|asset| (asset.temporary_id, asset)).collect();
    let mut folders = None;
    let mut next_asset_id = None;
    let mut states = Vec::new();
    let mut remapped = Vec::new();
    let mut resolved = std::collections::HashSet::new();
    let mut inserted = 0;
    for requested in ids {
        let id = if requested >= 0 {
            requested
        } else {
            let Some(candidate) = candidates.get(&requested) else {
                if require_temporary_id {
                    return Err(format!("Listed file {requested} is no longer available"));
                }
                continue;
            };
            if candidate.ext.trim().is_empty() {
                continue;
            }
            let id = match find_asset_by_path(tx, &candidate.path)? {
                Some(id) => id,
                None if insert_missing => {
                    if folders.is_none() {
                        folders = Some((load_folder_paths(tx)?, load_folder_roots(tx)?));
                    }
                    let (paths, roots) = folders.as_mut().unwrap();
                    let folder_id = ensure_folder(tx, &candidate.folder_path, paths, roots)?;
                    let id = match next_asset_id {
                        Some(id) => id,
                        None => next_id(tx, "assets")?,
                    };
                    next_asset_id = Some(id.checked_add(1).ok_or("Asset ID limit reached")?);
                    insert_asset(
                        tx,
                        &NewCatalogAsset {
                            id,
                            folder_id,
                            name: candidate.name.clone(),
                            ext: candidate.ext.clone(),
                            size: candidate.size,
                            file_modified: candidate.file_modified.clone(),
                            path: candidate.path.clone(),
                            rating: None,
                            label: None,
                            tagged: false,
                            exif_date: None,
                        },
                    )?;
                    track(tx, id, &candidate.path)?;
                    inserted += 1;
                    id
                }
                None => continue,
            };
            remapped.push(AssetIdRemap {
                old_id: requested,
                new_id: id,
            });
            id
        };
        if resolved.insert(id) {
            if let Some(state) = read_asset_values(tx, id)? {
                states.push(state);
            }
        }
    }
    Ok((states, remapped, inserted))
}

pub(super) fn table_exists(tx: &Transaction<'_>, table: &str) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

// Deleted IDs can still be present in the AI index and in listing caches.
pub(crate) fn remember_id(tx: &Transaction<'_>, table: &str, id: i64) -> Result<(), String> {
    let key = format!("ddreams_last_deleted_{table}_id");
    tx.execute("INSERT INTO meta(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE CAST(meta.value AS INTEGER) < ?2",
        (key, id)).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn next_id(tx: &Transaction<'_>, table: &str) -> Result<i64, String> {
    let key = format!("ddreams_last_deleted_{table}_id");
    let (live, deleted): (i64, i64) = tx.query_row(
        &format!("SELECT coalesce((SELECT max(id) FROM {table}),0), coalesce((SELECT CAST(value AS INTEGER) FROM meta WHERE key=?1),0)"),
        [key], |row| Ok((row.get(0)?, row.get(1)?)),
    ).map_err(|e| e.to_string())?;
    live.max(deleted)
        .checked_add(1)
        .ok_or_else(|| format!("ID limit reached for {table}"))
}

impl CatalogWriter {
    pub fn new(db: Arc<Mutex<rusqlite::Connection>>, writable: bool) -> Self {
        Self {
            db,
            writable,
            change_hook: Mutex::new(None),
            rules: crate::folder_rules::FolderRules::default(),
            incoming: Mutex::new(String::new()),
        }
    }

    pub(crate) fn sync_incoming(&self, folder: &str) -> Result<(), String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let mut incoming = self.incoming.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        reconcile_incoming(&tx, folder, None)?;
        tx.commit().map_err(|e| e.to_string())?;
        *incoming = folder.to_owned();
        Ok(())
    }

    pub(crate) fn observe_incoming_folder(&self, folder: &str) -> Result<(), String> {
        if !self.writable {
            return Ok(());
        }
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let incoming = self.incoming.lock().map_err(|e| e.to_string())?;
        if !crate::incoming::contains_folder(folder, &incoming) || !Path::new(folder).is_dir() {
            return Ok(());
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        reconcile_incoming(&tx, &incoming, Some(folder))?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub(super) fn track_incoming(&self, tx: &Transaction<'_>, id: i64, path: &str) -> Result<(), String> {
        let folder = self.incoming.lock().map_err(|e| e.to_string())?;
        track_incoming(tx, id, path, &folder)
    }

    pub(super) fn track_incoming_move(
        &self,
        tx: &Transaction<'_>,
        id: i64,
        from: Option<&str>,
        to: &str,
    ) -> Result<(), String> {
        let folder = self.incoming.lock().map_err(|e| e.to_string())?;
        track_incoming(tx, id, to, &folder)?;
        if !folder.is_empty()
            && from.is_some_and(|path| crate::incoming::is_inside(path, &folder))
            && !crate::incoming::is_inside(to, &folder)
        {
            tx.execute(
                "UPDATE assets SET delete_suggestion=NULL WHERE id=?1 AND delete_suggestion IS NOT NULL",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn set_change_hook(&self, hook: ChangeHook) {
        *self.change_hook.lock().unwrap() = Some(hook);
    }

    pub(super) fn notify_labels_changed(&self, kinds: &[&str]) {
        let hook = self.change_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(kinds);
        }
    }

    /// Adds files found in an included folder in a single catalogue transaction.
    /// Files already catalogued (by path) are skipped. Returns listing id → new catalogue id.
    pub fn catalog_listed_assets(&self, listed: &[ListedAsset]) -> Result<Vec<AssetIdRemap>, String> {
        self.catalog_listed_assets_with_dates(listed, &Default::default())
    }

    pub(crate) fn is_writable(&self) -> bool {
        self.writable
    }

    pub(crate) fn catalog_listed_assets_with_dates(
        &self,
        listed: &[ListedAsset],
        dates: &std::collections::HashMap<i64, String>,
    ) -> Result<Vec<AssetIdRemap>, String> {
        self.require_write()?;
        if listed.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut folder_paths = load_folder_paths(&tx)?;
        let mut folder_roots = load_folder_roots(&tx)?;
        let mut remaps = Vec::new();
        let has_edit_pending = crate::schema::has_asset_column(&tx, "edit_pending_since").map_err(|e| e.to_string())?;
        for candidate in listed {
            if candidate.ext.trim().is_empty() || find_asset_by_path(&tx, &candidate.path)?.is_some() {
                continue;
            }
            let folder_id = ensure_folder(&tx, &candidate.folder_path, &mut folder_paths, &mut folder_roots)?;
            let id = next_id(&tx, "assets")?;
            insert_asset(
                &tx,
                &NewCatalogAsset {
                    id,
                    folder_id,
                    name: candidate.name.clone(),
                    ext: candidate.ext.clone(),
                    size: candidate.size,
                    file_modified: candidate.file_modified.clone(),
                    path: candidate.path.clone(),
                    rating: None,
                    label: None,
                    tagged: false,
                    exif_date: dates.get(&candidate.temporary_id).cloned(),
                },
            )?;
            self.track_incoming(&tx, id, &candidate.path)?;
            if has_edit_pending {
                fulfil_edit_with_sibling(&tx, folder_id, candidate)?;
            }
            remaps.push(AssetIdRemap {
                old_id: candidate.temporary_id,
                new_id: id,
            });
        }
        if remaps.is_empty() {
            return Ok(remaps);
        }
        let summary = format!(
            "Pridané do katalógu: {} {}",
            remaps.len(),
            slovak_count(remaps.len(), "súbor", "súbory", "súborov")
        );
        record_change(&tx, "auto_catalog", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(remaps)
    }

    pub(crate) fn backfill_capture_dates(
        &self,
        dates: &std::collections::HashMap<i64, String>,
    ) -> Result<usize, String> {
        self.require_write()?;
        if dates.is_empty() {
            return Ok(0);
        }
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        {
            let mut update = tx
                .prepare(
                    "UPDATE assets SET exif_date=?2
                WHERE id=?1 AND is_listed=1 AND (exif_date IS NULL OR exif_date='')",
                )
                .map_err(|e| e.to_string())?;
            for (id, date) in dates {
                count += update.execute((id, date)).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub(crate) fn fulfil_edits_from_listing(&self, assets: &[crate::db::Asset]) -> Result<usize, String> {
        if !self.writable {
            return Ok(0);
        }
        let mut modified = assets
            .iter()
            .filter_map(|asset| {
                (asset.id > 0 && !asset.is_directory && !asset.offline && asset.edit_pending_since.is_some())
                    .then_some(asset.file_modified.as_deref())
                    .flatten()
                    .map(|mtime| (asset.id, mtime))
            })
            .peekable();
        if modified.peek().is_none() {
            return Ok(0);
        }
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        {
            let mut update = tx
                .prepare(
                    "UPDATE assets SET edit_pending_since=NULL
                WHERE id=?1 AND is_listed=1 AND unixepoch(?2)>edit_pending_since+?3",
                )
                .map_err(|e| e.to_string())?;
            for (id, mtime) in modified {
                count += update
                    .execute((id, mtime, crate::quarantine::EDIT_MTIME_GRACE))
                    .map_err(|e| e.to_string())?;
            }
        }
        if count > 0 {
            record_change(
                &tx,
                "fulfil_pending_edits",
                &format!("Úpravy zistené v súboroch · {count}"),
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub(super) fn require_write(&self) -> Result<(), String> {
        if self.writable {
            Ok(())
        } else {
            Err("Catalog is in strict read-only mode".into())
        }
    }
}
