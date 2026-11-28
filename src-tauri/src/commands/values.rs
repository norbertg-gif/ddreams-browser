use super::{CatalogWriterState, DatabaseState, FileManagerState, ListingState};
use crate::journal::{ChangeSummary, ListedAsset, SetAssetValuesResult};
use crate::listing::ListingService;
use serde_json::Value;
use std::path::Path;
use tauri::State;

#[tauri::command(async)]
pub fn get_recent_changes(state: State<'_, CatalogWriterState>, limit: i64) -> Result<Vec<ChangeSummary>, String> {
    state.0.recent_changes(limit)
}

#[tauri::command(async)]
pub fn set_asset_values(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    file_manager: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    changes: serde_json::Map<String, Value>,
) -> Result<SetAssetValuesResult, String> {
    let _operation = changes
        .contains_key("editPending")
        .then(|| file_manager.0.lock_operation())
        .transpose()?;
    let (file_ids, candidates) = listed_asset_candidates(&listing.0, &asset_ids)?;
    let result = state.0.set_asset_values(&file_ids, &candidates, &changes)?;
    for id in asset_ids.iter().copied().filter(|id| *id >= 0) {
        let _ = listing.0.invalidate_asset_parent(id);
    }
    for candidate in candidates {
        let _ = listing.0.invalidate_path(&candidate.folder_path);
    }
    Ok(result)
}

pub(super) fn listed_asset_candidates(
    listing: &ListingService,
    asset_ids: &[i64],
) -> Result<(Vec<i64>, Vec<ListedAsset>), String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let asset = listing
            .asset(id)
            .ok_or_else(|| format!("Listed asset {id} is no longer available"))?;
        if asset.is_directory || asset.ext.is_empty() {
            continue;
        }
        let path = asset
            .path
            .ok_or_else(|| format!("Path for listed asset {id} is unavailable"))?;
        let folder_path = Path::new(&path)
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .ok_or("Parent folder is unavailable")?;
        candidates.push(ListedAsset {
            temporary_id: id,
            name: asset.name,
            ext: asset.ext,
            size: asset.size,
            file_modified: asset.file_modified,
            path,
            folder_path,
        });
        file_ids.push(id);
    }
    Ok((file_ids, candidates))
}

#[tauri::command(async)]
pub fn set_asset_text(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
    field: crate::journal::AssetTextField,
    value: Option<String>,
) -> Result<crate::journal::SetAssetTextResult, String> {
    let (file_ids, candidates) = listed_asset_candidates(&listing.0, &asset_ids)?;
    let result = state.0.set_asset_text(&file_ids, &candidates, field, value)?;
    for id in file_ids.into_iter().filter(|id| *id >= 0) {
        let _ = listing.0.invalidate_asset_parent(id);
    }
    for candidate in candidates {
        let _ = listing.0.invalidate_path(&candidate.folder_path);
    }
    Ok(result)
}

#[tauri::command(async)]
pub fn get_asset_labels(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
) -> Result<Vec<(i64, Option<String>)>, String> {
    let mut labels = Vec::new();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        if let Some(asset) = listing.0.asset(id) {
            labels.push((id, asset.label));
        }
    }
    let conn = state.0.lock().map_err(|_| "catalog lock poisoned")?;
    let listed_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for chunk in listed_ids.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id,label FROM assets WHERE is_listed=1 AND id IN ({marks})");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        labels.extend(
            stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?,
        );
    }
    Ok(labels)
}
