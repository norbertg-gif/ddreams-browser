use super::{AppInfoState, BackgroundDbState, CatalogWriterState, ListingState};
use crate::ai::AiState;
use crate::db;
use std::path::Path;
use tauri::State;

fn path_volume(path: &str) -> String {
    let p = path.replace('/', "\\");
    if p.starts_with("\\\\") {
        let mut parts = p.trim_start_matches('\\').split('\\');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        return format!("\\\\{server}\\{share}");
    }
    if p.as_bytes().get(1) == Some(&b':') {
        return p[..3.min(p.len())].to_string();
    }
    "\\".into()
}

pub(super) fn classify_source_with(
    path: &str,
    cache: &mut std::collections::HashMap<String, bool>,
    mut exists: impl FnMut(&str) -> bool,
) -> (bool, bool) {
    let volume = path_volume(path);
    let reachable = *cache.entry(volume.clone()).or_insert_with(|| exists(&volume));
    if !reachable {
        (true, false)
    } else {
        (false, !exists(path))
    }
}

pub(super) fn classify_source(path: &str, cache: &mut std::collections::HashMap<String, bool>) -> (bool, bool) {
    classify_source_with(path, cache, |p| Path::new(p).exists())
}

pub(super) fn require_missing_source(offline: bool, missing: bool) -> Result<(), String> {
    if offline || missing {
        Ok(())
    } else {
        Err("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie".into())
    }
}

pub(super) fn tree_children_from_listing(disk: Vec<db::Folder>, _catalogue: Vec<db::Folder>) -> Vec<db::Folder> {
    disk
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct CatalogSourceStatus {
    pub path: String,
    pub assets: usize,
    pub offline: bool,
    pub missing: bool,
}

#[derive(Clone)]
pub(super) struct CatalogFolderNode {
    pub(super) id: i64,
    pub(super) parent: Option<i64>,
    pub(super) path: String,
    pub(super) count: usize,
}

pub(super) fn collect_catalog_missing_sources(
    mut nodes: Vec<CatalogFolderNode>,
    mut exists: impl FnMut(&str) -> bool,
) -> Vec<CatalogSourceStatus> {
    nodes.sort_by_key(|n| std::cmp::Reverse(n.path.len()));
    let mut counts = nodes
        .iter()
        .map(|n| (n.id, n.count))
        .collect::<std::collections::HashMap<_, _>>();
    for node in &nodes {
        if let Some(parent) = node.parent {
            let count = counts.get(&node.id).copied().unwrap_or(0);
            *counts.entry(parent).or_default() += count;
        }
    }
    nodes.sort_by_key(|n| n.path.len());
    let mut reachable = std::collections::HashMap::<String, bool>::new();
    let mut status = std::collections::HashMap::<i64, (bool, bool)>::new();
    for node in &nodes {
        let volume = path_volume(&node.path);
        let online = *reachable.entry(volume.clone()).or_insert_with(|| exists(&volume));
        let value = if !online {
            (true, false)
        } else {
            let parent_status = node
                .parent
                .and_then(|id| status.get(&id).copied())
                .unwrap_or((false, false));
            if parent_status.0 || parent_status.1 {
                parent_status
            } else {
                (false, !exists(&node.path))
            }
        };
        status.insert(node.id, value);
    }
    let mut result = Vec::new();
    for node in nodes {
        let Some(&(offline, missing)) = status.get(&node.id) else {
            continue;
        };
        if !offline && !missing {
            continue;
        }
        let parent_bad = node
            .parent
            .and_then(|id| status.get(&id).copied())
            .is_some_and(|(o, m)| o || m);
        if !parent_bad {
            result.push(CatalogSourceStatus {
                path: node.path,
                assets: counts.get(&node.id).copied().unwrap_or(0),
                offline,
                missing,
            });
        }
    }
    result
}

#[tauri::command(async)]
pub fn get_catalog_missing_sources(state: State<'_, BackgroundDbState>) -> Result<Vec<CatalogSourceStatus>, String> {
    let conn = state.0.lock().map_err(|_| "catalog lock poisoned")?;
    let mut stmt=conn.prepare("SELECT f.id,f.parent_id,f.path,count(a.id) FROM folders f LEFT JOIN assets a ON a.folder_id=f.id WHERE f.path IS NOT NULL GROUP BY f.id,f.parent_id,f.path").map_err(|e|e.to_string())?;
    let nodes = stmt
        .query_map([], |r| {
            Ok(CatalogFolderNode {
                id: r.get(0)?,
                parent: r.get(1)?,
                path: r.get(2)?,
                count: r.get::<_, i64>(3)?.max(0) as usize,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    drop(conn);
    Ok(collect_catalog_missing_sources(nodes, |p| Path::new(p).is_dir()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct RemoveCatalogInfo {
    pub assets: usize,
    pub folders: usize,
    pub offline: bool,
    pub missing: bool,
}

#[tauri::command(async)]
pub fn remove_folder_from_catalog_info(
    db: State<'_, BackgroundDbState>,
    folder_path: String,
) -> Result<RemoveCatalogInfo, String> {
    let path = folder_path.trim().trim_end_matches(['\\', '/']);
    let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
    let normal = crate::journal::normalize_fs_path(path);
    let prefix = format!("{}\\", normal);
    let asset_paths = {
        let mut stmt = conn
            .prepare("SELECT path FROM assets WHERE path IS NOT NULL")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    let assets = asset_paths
        .iter()
        .filter(|p| {
            let p = crate::journal::normalize_fs_path(p);
            p == normal || p.starts_with(&prefix)
        })
        .count();
    let all = {
        let mut stmt = conn
            .prepare("SELECT path FROM folders WHERE path IS NOT NULL")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    let prefix = format!("{}\\", path.replace('/', "\\").trim_end_matches('\\'));
    let folders = all
        .iter()
        .filter(|p| p.eq_ignore_ascii_case(path) || p.to_lowercase().starts_with(&prefix.to_lowercase()))
        .count();
    drop(conn);
    let mut cache = std::collections::HashMap::new();
    let (offline, missing) = classify_source(path, &mut cache);
    Ok(RemoveCatalogInfo {
        assets,
        folders,
        offline,
        missing,
    })
}

#[tauri::command(async)]
pub fn remove_folder_from_catalog(
    writer: State<'_, CatalogWriterState>,
    ai: State<'_, AiState>,
    app: State<'_, AppInfoState>,
    listing: State<'_, ListingState>,
    folder_path: String,
) -> Result<crate::journal::RemoveCatalogResult, String> {
    let mut cache = std::collections::HashMap::new();
    let (offline, missing) = classify_source(&folder_path, &mut cache);
    if !offline && !missing {
        return Err("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie".into());
    }
    if app.0.readonly {
        return Err("Katalóg je iba na čítanie".into());
    }
    crate::backup::backup_now(Path::new(&app.0.catalog), Path::new(&app.0.backups))?;
    let result = writer.0.remove_folder_from_catalog(&folder_path)?;
    ai.0.forget(result.asset_ids.clone());
    let _ = listing.0.invalidate_path(&folder_path);
    Ok(result)
}
