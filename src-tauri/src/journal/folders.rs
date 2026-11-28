use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use super::history::{iso_now, mark_catalog_modified, record_change};
use super::values::{slovak_count, update_column};
use super::writer::{AssetIdRemap, find_asset_by_path, insert_asset, next_id, remember_id, table_exists};
use super::{CatalogWriter, NewCatalogAsset};

#[derive(Clone, Debug)]
pub struct FileCatalogMove {
    pub asset_id: i64,
    pub target_path: String,
    pub target_folder_path: String,
    pub name: String,
    pub ext: String,
    pub size: f64,
    pub file_modified: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileCatalogCopy {
    pub source_asset_id: i64,
    pub target_path: String,
    pub target_folder_path: String,
    pub name: String,
    pub ext: String,
    pub size: f64,
    pub file_modified: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct ExclusionImpact {
    pub files: usize,
    pub with_categories: usize,
    pub with_keywords: usize,
    pub with_rating_label_tagged: usize,
    pub with_color_overrides: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct RemoveCatalogResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub removed_assets: usize,
    pub removed_folders: usize,
    pub asset_ids: Vec<i64>,
}

fn remove_asset_rows(tx: &Transaction<'_>, ids: &[i64], permanent: bool) -> Result<(), String> {
    if let Some(&max_id) = ids.iter().max() {
        remember_id(tx, "assets", max_id)?;
    }
    let tables = [
        "asset_categories",
        "asset_keywords",
        "asset_color_overrides",
        "ai_suggestion_rejections",
    ];
    for table in tables
        .into_iter()
        .take(if permanent { tables.len() } else { tables.len() - 1 })
    {
        if table_exists(tx, table)? {
            for chunk in ids.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                tx.execute(
                    &format!("DELETE FROM {table} WHERE asset_id IN ({marks})"),
                    rusqlite::params_from_iter(chunk),
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        tx.execute(
            &format!("DELETE FROM assets WHERE id IN ({marks})"),
            rusqlite::params_from_iter(chunk),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Catalogue asset ids whose file lies in `folder` or below (whole path components, case-insensitive).
/// Matched in Rust: SQLite's lower() only folds ASCII (Ľudia ≠ ľudia) and LIKE would treat `_` as a wildcard.
fn assets_under(conn: &rusqlite::Connection, folder: &str) -> Result<Vec<i64>, String> {
    let prefix = normalize_fs_path(folder) + "\\";
    let mut stmt = conn
        .prepare("SELECT id,path FROM assets WHERE path IS NOT NULL ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|(_, path)| normalize_fs_path(path).starts_with(&prefix))
        .map(|(id, _)| id)
        .collect())
}

pub(super) fn load_folder_paths(transaction: &Transaction<'_>) -> Result<HashMap<String, i64>, String> {
    let mut statement = transaction
        .prepare("SELECT id,path FROM folders WHERE path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    for row in rows {
        let (id, path) = row.map_err(|e| e.to_string())?;
        result.insert(normalize_fs_path(&path), id);
    }
    Ok(result)
}

pub(super) fn load_folder_roots(transaction: &Transaction<'_>) -> Result<Vec<(i64, String)>, String> {
    let mut statement = transaction
        .prepare("SELECT id,path FROM folder_roots WHERE path IS NOT NULL")
        .map_err(|e| e.to_string())?;
    statement
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

pub(super) fn ensure_folder(
    transaction: &Transaction<'_>,
    target: &str,
    folders: &mut HashMap<String, i64>,
    roots: &mut Vec<(i64, String)>,
) -> Result<i64, String> {
    let target_key = normalize_fs_path(target);
    if let Some(id) = folders.get(&target_key) {
        return Ok(*id);
    }
    if target.trim().is_empty() {
        return Err("Folder path is empty".into());
    }
    let root = roots
        .iter()
        .filter(|(_, path)| is_path_prefix(path, target))
        .max_by_key(|(_, path)| path.len())
        .cloned();
    let (root_id, root_path) = if let Some(root) = root {
        root
    } else {
        let (name, path) = derive_path_root(target);
        let id = next_id(transaction, "folder_roots")?;
        transaction
            .execute(
                "INSERT INTO folder_roots(id,name,path) VALUES(?1,?2,?3)",
                (id, &name, &path),
            )
            .map_err(|e| e.to_string())?;

        roots.push((id, path.clone()));
        (id, path)
    };
    let root_key = normalize_fs_path(&root_path);
    let mut parent_id = if let Some(id) = folders.get(&root_key) {
        *id
    } else {
        let id = next_id(transaction, "folders")?;
        transaction
            .execute(
                "INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,NULL,?2,'\\',?3)",
                (id, root_id, &root_path),
            )
            .map_err(|e| e.to_string())?;

        folders.insert(root_key.clone(), id);
        id
    };
    let target_parts = path_parts(target);
    let root_parts = path_parts(&root_path);
    let skip = root_parts.len();
    let mut current = PathBuf::from(&root_path);
    for part in target_parts.into_iter().skip(skip) {
        current.push(&part);
        let path = current.to_string_lossy().to_string();
        let key = normalize_fs_path(&path);
        if let Some(id) = folders.get(&key) {
            parent_id = *id;
            continue;
        }
        let id = next_id(transaction, "folders")?;
        transaction
            .execute(
                "INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,?2,?3,?4,?5)",
                (id, parent_id, root_id, &part, &path),
            )
            .map_err(|e| e.to_string())?;

        folders.insert(key, id);
        parent_id = id;
    }
    Ok(parent_id)
}

fn derive_path_root(path: &str) -> (String, String) {
    let normalized = path.replace('/', "\\");
    if normalized.len() >= 3 && normalized.as_bytes()[1] == b':' && normalized.as_bytes()[2] == b'\\' {
        return (normalized[..2].to_owned(), normalized[..3].to_owned());
    }
    let parts = path_parts(&normalized);
    if normalized.starts_with("\\\\") && parts.len() >= 2 {
        let root = format!("\\\\{}\\{}\\", parts[0], parts[1]);
        return (format!("{}\\{}", parts[0], parts[1]), root);
    }
    let first = parts.first().cloned().unwrap_or_else(|| normalized.clone());
    (first.clone(), first)
}

fn path_parts(path: &str) -> Vec<String> {
    path.split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn normalize_fs_path(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase()
}

fn is_path_prefix(root: &str, target: &str) -> bool {
    let root = normalize_fs_path(root);
    let target = normalize_fs_path(target);
    target == root
        || target
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

// Map the normalized prefix to the separator in the original path, whose UTF-8 width can differ.
fn matched_path_suffix<'a>(path: &'a str, old_normalized: &str) -> Option<&'a str> {
    let normalized = normalize_fs_path(path);
    if normalized == old_normalized {
        return Some("");
    }
    if !normalized.strip_prefix(old_normalized)?.starts_with('\\') {
        return None;
    }
    let separators = old_normalized.matches('\\').count();
    let (boundary, _) = path.match_indices(['\\', '/']).nth(separators)?;
    Some(&path[boundary..])
}

pub(super) fn delete_row(tx: &Transaction<'_>, table: &str, id: i64) -> Result<(), String> {
    remember_id(tx, table, id)?;
    tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn remove_folder_rows(tx: &Transaction<'_>, folders: &[(i64, String, Option<i64>)]) -> Result<(), String> {
    let ids = folders.iter().map(|row| row.0).collect::<Vec<_>>();
    if let Some(&max_id) = ids.iter().max() {
        remember_id(tx, "folders", max_id)?;
    }
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        tx.execute(
            &format!("DELETE FROM folders WHERE id IN ({marks})"),
            rusqlite::params_from_iter(chunk),
        )
        .map_err(|e| e.to_string())?;
    }
    let roots = folders
        .iter()
        .filter_map(|row| row.2)
        .collect::<std::collections::HashSet<_>>();
    for root in roots {
        remember_id(tx, "folder_roots", root)?;
        tx.execute(
            "DELETE FROM folder_roots WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM folders WHERE root_id=?1)",
            [root],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl CatalogWriter {
    pub fn folder_rules(&self) -> Result<Vec<crate::folder_rules::FolderRule>, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        self.rules.list(&conn)
    }

    pub fn folder_rule_state(&self, path: &str) -> Result<crate::folder_rules::FolderRuleState, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        self.rules.state(&conn, path)
    }

    pub fn folder_catalogue_states(
        &self,
        paths: Vec<String>,
    ) -> Result<std::collections::HashMap<String, crate::folder_rules::FolderCatalogueState>, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        self.rules.catalogue_states(&conn, paths)
    }

    /// Effective rule for a folder path: Some("include") / Some("exclude") / None (legacy, no rule).
    pub fn effective_folder_mode(&self, path: &str) -> Option<String> {
        self.folder_rule_state(path).ok().and_then(|s| s.effective)
    }

    /// How much catalogue data lives under `path` (shown before excluding the folder).
    pub fn exclusion_impact(&self, path: &str) -> Result<ExclusionImpact, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let ids = assets_under(&conn, path)?;
        let mut impact = ExclusionImpact {
            files: ids.len(),
            ..Default::default()
        };
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT coalesce(sum(EXISTS(SELECT 1 FROM asset_categories c WHERE c.asset_id=a.id)),0),
                        coalesce(sum(EXISTS(SELECT 1 FROM asset_keywords k WHERE k.asset_id=a.id)),0),
                        coalesce(sum(coalesce(a.rating,0)<>0 OR coalesce(a.label,'')<>'' OR coalesce(a.tagged,0)<>0),0),
                        coalesce(sum(EXISTS(SELECT 1 FROM asset_color_overrides c WHERE c.asset_id=a.id)),0)
                 FROM assets a WHERE a.id IN ({marks})"
                ))
                .map_err(|e| e.to_string())?;
            let (categories, keywords, values, colors) = stmt
                .query_row(rusqlite::params_from_iter(chunk), |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            impact.with_categories += categories as usize;
            impact.with_keywords += keywords as usize;
            impact.with_rating_label_tagged += values as usize;
            impact.with_color_overrides += colors as usize;
        }
        Ok(impact)
    }

    pub fn remove_folder_from_catalog(&self, path: &str) -> Result<RemoveCatalogResult, String> {
        self.require_write()?;
        let path = path.trim().trim_end_matches(['\\', '/']).replace('/', "\\");
        if path.is_empty() {
            return Err("Cesta priečinka je prázdna".into());
        }
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let normal = normalize_fs_path(&path);
        let prefix = format!("{normal}\\");
        let ids = {
            let mut stmt = tx
                .prepare("SELECT id,path FROM assets WHERE path IS NOT NULL ORDER BY id")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|(_, p)| {
                    let p = normalize_fs_path(p);
                    p == normal || p.starts_with(&prefix)
                })
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        let folder_rows = {
            let mut stmt = tx
                .prepare("SELECT id,path,root_id FROM folders WHERE path IS NOT NULL")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
        };
        let folders = folder_rows
            .into_iter()
            .filter(|(_, p, _)| {
                normalize_fs_path(p) == normalize_fs_path(&path) || normalize_fs_path(p).starts_with(&prefix)
            })
            .collect::<Vec<_>>();
        remove_asset_rows(&tx, &ids, false)?;
        remove_folder_rows(&tx, &folders)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !ids.is_empty() {
            self.notify_labels_changed(&["category", "keyword"]);
        }
        Ok(RemoveCatalogResult {
            change_id: None,
            summary: format!(
                "Odstránené z katalógu: {} súborov a {} priečinkov",
                ids.len(),
                folders.len()
            ),
            removed_assets: ids.len(),
            removed_folders: folders.len(),
            asset_ids: ids,
        })
    }

    /// Saves (or clears, `mode=None`) the rule for `path`. Excluding removes every catalogue row under the
    /// folder, including category, keyword and colour links. Returns the removed ids for the AI index.
    pub fn set_folder_rule(&self, path: &str, mode: Option<&str>) -> Result<Vec<i64>, String> {
        self.require_write()?;
        let path = path.trim().trim_end_matches(['\\', '/']).replace('/', "\\");
        if path.is_empty() {
            return Err("Cesta priečinka je prázdna".into());
        }
        if !matches!(mode, None | Some("include") | Some("exclude")) {
            return Err("Neplatné pravidlo priečinka".into());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS ddreams_folder_rules(path TEXT PRIMARY KEY COLLATE NOCASE,mode TEXT NOT NULL CHECK(mode IN ('include','exclude')),created TEXT NOT NULL);").map_err(|e|e.to_string())?;
        let mut removed = Vec::new();
        if mode == Some("exclude") {
            removed = assets_under(&tx, &path)?;
            remove_asset_rows(&tx, &removed, false)?;
            let summary = format!(
                "Vylúčené z katalógu: {path} ({} {})",
                removed.len(),
                slovak_count(removed.len(), "súbor", "súbory", "súborov")
            );
            if !removed.is_empty() {
                record_change(&tx, "exclude_folder", &summary)?;
            }
        }
        match mode {
            Some(mode) => {
                tx.execute("INSERT INTO ddreams_folder_rules(path,mode,created) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET mode=excluded.mode",(&path,mode,iso_now())).map_err(|e|e.to_string())?;
            }
            None => {
                tx.execute("DELETE FROM ddreams_folder_rules WHERE path=?1", [&path])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.rules.reload(&connection);
        if !removed.is_empty() {
            self.notify_labels_changed(&["category", "keyword"]);
        }
        Ok(removed)
    }

    pub fn record_file_change(
        &self,
        action: &str,
        summary: &str,
        moved: &[FileCatalogMove],
        copied: &[FileCatalogCopy],
        created_folders: &[String],
    ) -> Result<(Option<i64>, Vec<AssetIdRemap>), String> {
        self.record_file_transfer(action, summary, moved, copied, created_folders, None)
    }

    pub(crate) fn record_quarantine_move(
        &self,
        summary: &str,
        moved: &FileCatalogMove,
        source: &str,
        entering: bool,
    ) -> Result<Option<i64>, String> {
        let action = if entering {
            "move_to_quarantine"
        } else {
            "restore_from_quarantine"
        };
        self.record_file_transfer(
            action,
            summary,
            std::slice::from_ref(moved),
            &[],
            &[],
            Some((source, entering)),
        )
        .map(|r| r.0)
    }

    fn record_file_transfer(
        &self,
        action: &str,
        summary: &str,
        moved: &[FileCatalogMove],
        copied: &[FileCatalogCopy],
        created_folders: &[String],
        quarantine: Option<(&str, bool)>,
    ) -> Result<(Option<i64>, Vec<AssetIdRemap>), String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut paths = load_folder_paths(&tx)?;
        let mut roots = load_folder_roots(&tx)?;
        let mut remapped = Vec::new();
        let regular_move = action == "move_files"
            && crate::schema::has_asset_column(&tx, "quarantined_at").map_err(|e| e.to_string())?;
        for item in moved {
            if let Some((source, _)) = quarantine {
                let expected = (item.asset_id > 0).then_some(item.asset_id);
                if find_asset_by_path(&tx, source)? != expected {
                    return Err("Zdrojová cesta v katalógu sa zmenila".into());
                }
                if find_asset_by_path(&tx, &item.target_path)?.is_some() {
                    return Err(format!("Cieľová cesta už existuje v katalógu: {}", item.target_path));
                }
            }
            let folder = ensure_folder(&tx, &item.target_folder_path, &mut paths, &mut roots)?;
            let mut previous_path = None;
            let id = if item.asset_id < 0 && matches!(quarantine, Some((_, true))) {
                let id = next_id(&tx, "assets")?;
                insert_asset(
                    &tx,
                    &NewCatalogAsset {
                        id,
                        folder_id: folder,
                        name: item.name.clone(),
                        ext: item.ext.clone(),
                        size: Some(item.size),
                        file_modified: item.file_modified.clone(),
                        path: item.target_path.clone(),
                        rating: None,
                        label: None,
                        tagged: false,
                        exif_date: None,
                    },
                )?;
                id
            } else {
                previous_path = tx
                    .query_row(
                        "SELECT path FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>''",
                        [item.asset_id],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Súbor katalógu {} sa nenašiel", item.asset_id))?;
                if regular_move {
                    tx.execute("UPDATE assets SET quarantined_at=NULL,quarantine_origin=NULL WHERE id=?1 AND quarantined_at IS NOT NULL",
                        [item.asset_id]).map_err(|e| e.to_string())?;
                }
                for (column, value) in [
                    ("folder_id", SqlValue::Integer(folder)),
                    ("path", SqlValue::Text(item.target_path.clone())),
                    ("name", SqlValue::Text(item.name.clone())),
                    ("ext", SqlValue::Text(item.ext.clone())),
                    ("ext_lower", SqlValue::Text(item.ext.to_ascii_lowercase())),
                    ("size", SqlValue::Real(item.size)),
                    (
                        "file_modified",
                        item.file_modified.clone().map(SqlValue::Text).unwrap_or(SqlValue::Null),
                    ),
                ] {
                    update_column(&tx, "assets", item.asset_id, column, value)?;
                }
                item.asset_id
            };
            if let Some((source, entering)) = quarantine {
                self.track_incoming(&tx, id, &item.target_path)?;
                tx.execute("UPDATE assets SET quarantined_at=?2,quarantine_origin=?3,delete_suggestion=0,edit_pending_since=NULL WHERE id=?1",
                    (id, entering.then(crate::quarantine::now), entering.then_some(source))).map_err(|e| e.to_string())?;
            } else {
                self.track_incoming_move(&tx, id, previous_path.as_deref(), &item.target_path)?;
            }
        }
        for item in copied {
            let folder = ensure_folder(&tx, &item.target_folder_path, &mut paths, &mut roots)?;
            let source_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>'')",
                    [item.source_asset_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !source_exists {
                continue;
            }
            let id = next_id(&tx, "assets")?;
            tx.execute("INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,size,width,height,bpp,caption,notes,author,rating,tagged,label,file_modified,exif_date,acdsee_date,md_dirty,is_orphan,path) SELECT ?1,?2,?3,?4,lower(?4),is_listed,?5,width,height,bpp,caption,notes,author,rating,tagged,label,?6,exif_date,acdsee_date,md_dirty,is_orphan,?7 FROM assets WHERE id=?8",(id,folder,&item.name,&item.ext,item.size,&item.file_modified,&item.target_path,item.source_asset_id)).map_err(|e|e.to_string())?;
            self.track_incoming(&tx, id, &item.target_path)?;
            remapped.push(AssetIdRemap {
                old_id: item.source_asset_id,
                new_id: id,
            });
            for (table, column) in [("asset_categories", "category_id"), ("asset_keywords", "keyword_id")] {
                tx.execute(&format!("INSERT OR IGNORE INTO {table}(asset_id,{column}) SELECT ?1,{column} FROM {table} WHERE asset_id=?2"),
                    (id, item.source_asset_id)).map_err(|e| e.to_string())?;
            }
        }
        for folder_path in created_folders {
            ensure_folder(&tx, folder_path, &mut paths, &mut roots)?;
        }
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !remapped.is_empty() {
            self.notify_labels_changed(&["category", "keyword"]);
        }
        Ok((change_id, remapped))
    }

    pub fn rewrite_folder_paths(
        &self,
        old: &str,
        new: &str,
        action: &str,
        summary: &str,
    ) -> Result<Option<i64>, String> {
        self.require_write()?;
        let old = normalize_fs_path(old);
        let new = new.trim_end_matches(['\\', '/']).replace('/', "\\");
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        for table in ["assets", "folders", "folder_roots"] {
            let mut stmt = tx
                .prepare(&format!("SELECT id,path FROM {table} WHERE path IS NOT NULL"))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            for (id, path) in rows {
                if let Some(suffix) = matched_path_suffix(&path, &old) {
                    let target = format!("{new}{}", suffix.replace('/', "\\"));
                    update_column(&tx, table, id, "path", &target)?;
                    if table == "assets" {
                        self.track_incoming_move(&tx, id, Some(&path), &target)?;
                    }
                    if table == "folders" && suffix.is_empty() {
                        update_column(&tx, table, id, "name", new.rsplit('\\').next().unwrap_or(&new))?;
                    }
                }
            }
        }
        if table_exists(&tx, "ddreams_folder_rules")? {
            let mut stmt = tx
                .prepare("SELECT path FROM ddreams_folder_rules")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            for path in rows {
                if let Some(suffix) = matched_path_suffix(&path, &old) {
                    let target = format!("{new}{}", suffix.replace('/', "\\"));
                    tx.execute(
                        "UPDATE ddreams_folder_rules SET path=?1 WHERE path=?2",
                        (&target, &path),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.rules.reload(&conn);
        Ok(change_id)
    }

    pub fn record_folder_delete(&self, path: &str, summary: &str) -> Result<Option<i64>, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let ids = assets_under(&tx, path)?;
        let old = normalize_fs_path(path);
        let folders = {
            let mut stmt = tx
                .prepare("SELECT id,path,root_id FROM folders WHERE path IS NOT NULL")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(_, path, _)| matched_path_suffix(path, &old).is_some())
            .collect::<Vec<_>>()
        };
        remove_asset_rows(&tx, &ids, true)?;
        remove_folder_rows(&tx, &folders)?;
        let change_id = record_change(&tx, "delete_folder", summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !ids.is_empty() {
            self.notify_labels_changed(&["category", "keyword"]);
        }
        Ok(change_id)
    }

    pub fn purge_assets(&self, asset_ids: &[i64]) -> Result<(), String> {
        self.require_write()?;
        if asset_ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        remove_asset_rows(&tx, asset_ids, true)?;
        mark_catalog_modified(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category", "keyword"]);
        Ok(())
    }

    pub fn record_file_delete(&self, asset_ids: &[i64], summary: &str) -> Result<Option<i64>, String> {
        self.record_named_file_delete(asset_ids, "delete_files", summary)
    }

    pub(crate) fn record_named_file_delete(
        &self,
        asset_ids: &[i64],
        action: &str,
        summary: &str,
    ) -> Result<Option<i64>, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        remove_asset_rows(&tx, asset_ids, true)?;
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !asset_ids.is_empty() {
            self.notify_labels_changed(&["category", "keyword"]);
        }
        Ok(change_id)
    }

    pub(crate) fn record_file_intent(&self, action: &str, summary: &str) -> Result<(), String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())
    }
}
