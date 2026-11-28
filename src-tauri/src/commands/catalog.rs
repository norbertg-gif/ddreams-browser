use super::sources::{classify_source, tree_children_from_listing};
use super::{BackgroundDbState, CalendarState, CatalogWriterState, DatabaseState, FormatsState, ListingState};
use crate::ai::AiState;
use crate::calendar::{CalendarAggregate, DateSource};
use crate::db::{self, AssetFilter};
use crate::journal::{
    CategoryDeleteInfo, CategoryMutationResult, KeywordDeleteInfo, KeywordMutationResult, ListedAsset,
    SetAssetCategoriesResult, SetAssetKeywordsResult,
};
use serde::Serialize;
use std::path::Path;
use tauri::State;

#[tauri::command(async)]
pub fn set_asset_categories(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
    category_id: i64,
    assign: bool,
) -> Result<SetAssetCategoriesResult, String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let Some(asset) = listing.0.asset(id) else {
            continue;
        };
        if asset.is_directory || asset.ext.is_empty() {
            continue;
        }
        let path = asset
            .path
            .ok_or_else(|| format!("Listed file {id} has no resolved path"))?;
        let folder_path = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
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
    let result = state
        .0
        .set_asset_categories(&file_ids, &candidates, category_id, assign)?;
    for id in file_ids.iter().copied().filter(|id| *id >= 0) {
        let _ = listing.0.invalidate_asset_parent(id);
    }
    for candidate in candidates {
        let _ = listing.0.invalidate_path(&candidate.folder_path);
    }
    Ok(result)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySelectionCounts {
    pub total_files: usize,
    pub assigned: std::collections::HashMap<i64, usize>,
}

#[tauri::command(async)]
pub fn get_category_selection_counts(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
) -> Result<CategorySelectionCounts, String> {
    let mut positive = Vec::new();
    let mut total = 0usize;
    for id in asset_ids {
        if id < 0 {
            if listing
                .0
                .asset(id)
                .is_some_and(|a| !a.is_directory && !a.ext.is_empty())
            {
                total += 1;
            }
        } else {
            positive.push(id);
        }
    }
    let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
    for chunk in positive.chunks(500) {
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT count(*) FROM assets WHERE is_listed=1 AND ext_lower<>'' AND id IN ({marks})");
        total += conn
            .query_row(&sql, rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as usize;
    }
    let mut assigned = std::collections::HashMap::new();
    for chunk in positive.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT ac.category_id,count(DISTINCT ac.asset_id) FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1 AND a.ext_lower<>'' AND ac.asset_id IN ({marks}) GROUP BY ac.category_id"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, count) = row.map_err(|e| e.to_string())?;
            *assigned.entry(id).or_insert(0) += count as usize;
        }
    }
    Ok(CategorySelectionCounts {
        total_files: total,
        assigned,
    })
}

#[tauri::command(async)]
pub fn get_keyword_selection_counts(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
) -> Result<CategorySelectionCounts, String> {
    let mut positive = Vec::new();
    let mut total = 0usize;
    for id in asset_ids {
        if id < 0 {
            if listing
                .0
                .asset(id)
                .is_some_and(|a| !a.is_directory && !a.ext.is_empty())
            {
                total += 1;
            }
        } else {
            positive.push(id);
        }
    }
    let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
    for chunk in positive.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT count(*) FROM assets WHERE is_listed=1 AND ext_lower<>'' AND id IN ({marks})");
        total += conn
            .query_row(&sql, rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as usize;
    }
    let mut assigned = std::collections::HashMap::new();
    for chunk in positive.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT ak.keyword_id,count(DISTINCT ak.asset_id) FROM asset_keywords ak JOIN assets a ON a.id=ak.asset_id WHERE a.is_listed=1 AND a.ext_lower<>'' AND ak.asset_id IN ({marks}) GROUP BY ak.keyword_id"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, count) = row.map_err(|e| e.to_string())?;
            *assigned.entry(id).or_insert(0) += count as usize;
        }
    }
    Ok(CategorySelectionCounts {
        total_files: total,
        assigned,
    })
}

#[tauri::command(async)]
pub fn create_category(
    state: State<'_, CatalogWriterState>,
    parent_id: Option<i64>,
    name: String,
    icon_key: Option<String>,
) -> Result<CategoryMutationResult, String> {
    state.0.create_category(parent_id, &name, icon_key.as_deref())
}

#[tauri::command(async)]
pub fn set_category_icon(
    state: State<'_, CatalogWriterState>,
    id: i64,
    is_root: bool,
    icon_key: Option<String>,
) -> Result<CategoryMutationResult, String> {
    state.0.set_category_icon(id, is_root, icon_key.as_deref())
}

#[tauri::command(async)]
pub fn rename_category(
    state: State<'_, CatalogWriterState>,
    id: i64,
    name: String,
) -> Result<CategoryMutationResult, String> {
    state.0.rename_category(id, &name)
}

#[tauri::command(async)]
pub fn move_category(
    state: State<'_, CatalogWriterState>,
    id: i64,
    new_parent_id: Option<i64>,
) -> Result<CategoryMutationResult, String> {
    state.0.move_category(id, new_parent_id)
}

#[tauri::command(async)]
pub fn get_category_delete_info(state: State<'_, CatalogWriterState>, id: i64) -> Result<CategoryDeleteInfo, String> {
    state.0.category_delete_info(id)
}

#[tauri::command(async)]
pub fn delete_category(state: State<'_, CatalogWriterState>, id: i64) -> Result<CategoryMutationResult, String> {
    state.0.delete_category(id)
}

#[tauri::command(async)]
pub fn set_asset_keywords(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
    keyword_id: i64,
    assign: bool,
) -> Result<SetAssetKeywordsResult, String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let Some(asset) = listing.0.asset(id) else { continue };
        if asset.is_directory || asset.ext.is_empty() {
            continue;
        }
        let path = asset
            .path
            .ok_or_else(|| format!("Listed file {id} has no resolved path"))?;
        let folder_path = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
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
    let result = state.0.set_asset_keywords(&file_ids, &candidates, keyword_id, assign)?;
    for id in file_ids.iter().copied().filter(|id| *id >= 0) {
        let _ = listing.0.invalidate_asset_parent(id);
    }
    for candidate in candidates {
        let _ = listing.0.invalidate_path(&candidate.folder_path);
    }
    Ok(result)
}

#[tauri::command(async)]
pub fn create_and_assign_keyword(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
    parent_id: Option<i64>,
    name: String,
) -> Result<SetAssetKeywordsResult, String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let Some(asset) = listing.0.asset(id) else { continue };
        if asset.is_directory || asset.ext.is_empty() {
            continue;
        }
        let path = asset
            .path
            .ok_or_else(|| format!("Listed file {id} has no resolved path"))?;
        let folder_path = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
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
    let result = state
        .0
        .create_and_assign_keyword(&file_ids, &candidates, parent_id, &name)?;
    for id in file_ids.iter().copied().filter(|id| *id >= 0) {
        let _ = listing.0.invalidate_asset_parent(id);
    }
    for candidate in candidates {
        let _ = listing.0.invalidate_path(&candidate.folder_path);
    }
    Ok(result)
}

#[tauri::command(async)]
pub fn create_keyword(
    state: State<'_, CatalogWriterState>,
    parent_id: Option<i64>,
    name: String,
) -> Result<KeywordMutationResult, String> {
    state.0.create_keyword(parent_id, &name)
}

#[tauri::command(async)]
pub fn rename_keyword(
    state: State<'_, CatalogWriterState>,
    id: i64,
    name: String,
) -> Result<KeywordMutationResult, String> {
    state.0.rename_keyword(id, &name)
}

#[tauri::command(async)]
pub fn move_keyword(
    state: State<'_, CatalogWriterState>,
    id: i64,
    new_parent_id: Option<i64>,
) -> Result<KeywordMutationResult, String> {
    state.0.move_keyword(id, new_parent_id)
}

#[tauri::command(async)]
pub fn get_keyword_delete_info(state: State<'_, CatalogWriterState>, id: i64) -> Result<KeywordDeleteInfo, String> {
    state.0.keyword_delete_info(id)
}

#[tauri::command(async)]
pub fn delete_keyword(state: State<'_, CatalogWriterState>, id: i64) -> Result<KeywordMutationResult, String> {
    state.0.delete_keyword(id)
}

#[tauri::command(async)]
pub fn search_keyword_options(
    state: State<'_, BackgroundDbState>,
    query: String,
) -> Result<Vec<db::KeywordOption>, String> {
    db::keyword_options(&state.0, &query).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_calendar_aggregate(state: State<'_, CalendarState>, source: String) -> Result<CalendarAggregate, String> {
    state
        .0
        .aggregate(DateSource::parse(&source)?)
        .map(|value| (*value).clone())
}

#[tauri::command(async)]
pub fn get_folder_children(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    parent_id: Option<i64>,
    parent_ids: Option<Vec<i64>>,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Folder>, String> {
    if let Some(parent) = parent_id.filter(|id| *id != -2) {
        let path = listing.0.asset(parent).and_then(|a| a.path).or_else(|| {
            db::folder_ancestors(&state.0, parent)
                .ok()?
                .last()
                .and_then(|f| f.path.clone())
        });
        if let Some(path) = path {
            let filter = AssetFilter {
                kind: "folder".into(),
                id: Some(parent),
                root_id: None,
                include_children: false,
                folder_path: Some(path),
                ids: parent_ids.clone(),
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 0,
                limit: 500,
                rating_mode: None,
                rating_value: None,
                label_filter: None,
                tagged_only: false,
                category_filter: None,
                keyword_filter: None,
                include_categories: false,
                include_keywords: false,
                date_from: None,
                date_to: None,
                date_source: None,
                date_no_date: false,
                file_group: None,
            };
            let page = listing.0.page(&filter)?;
            let mut dirs = Vec::new();
            for asset in page.items.into_iter().filter(|a| a.is_directory) {
                let matched = asset
                    .path
                    .as_deref()
                    .and_then(|p| db::folder_by_path(&state.0, p).ok().flatten());
                let id = matched.as_ref().map(|f| f.id).unwrap_or(asset.id);
                dirs.push(db::Folder {
                    id,
                    parent_id: Some(parent),
                    root_id: matched.as_ref().and_then(|f| f.root_id),
                    name: asset.name,
                    path: asset.path,
                    count: matched.as_ref().map(|f| f.count).unwrap_or(0),
                    folder_ids: matched.map(|f| f.folder_ids).unwrap_or_else(|| vec![id]),
                    virtual_node: None,
                    offline: false,
                    missing: false,
                });
            }
            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return Ok(tree_children_from_listing(dirs, Vec::new())
                .into_iter()
                .skip(offset.max(0) as usize)
                .take(limit.clamp(1, 500) as usize)
                .collect());
        }
    }
    let mut rows = db::folders_page(&state.0, parent_id, parent_ids, 0, 500).map_err(|e| e.to_string())?;
    let mut volumes = std::collections::HashMap::new();
    let mut live = Vec::new();
    for mut folder in rows.drain(..) {
        if folder.virtual_node.as_deref() == Some("devices") {
            let mut ids = Vec::new();
            for id in folder.folder_ids {
                let path = db::folder_ancestors(&state.0, id)
                    .ok()
                    .and_then(|chain| chain.last().and_then(|f| f.path.clone()));
                if path.as_deref().is_some_and(|p| Path::new(p).is_dir()) {
                    ids.push(id)
                }
            }
            if !ids.is_empty() {
                folder.folder_ids = ids;
                live.push(folder)
            }
        } else if let Some(path) = folder.path.as_deref() {
            let (offline, missing) = classify_source(path, &mut volumes);
            folder.offline = offline;
            folder.missing = missing;
            if !offline && !missing {
                live.push(folder)
            }
        }
    }
    Ok(live
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit.clamp(1, 500) as usize)
        .collect())
}

#[tauri::command(async)]
pub fn find_folder_path(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    path: String,
) -> Result<Option<db::Folder>, String> {
    if let Some(folder) = db::folder_by_path(&state.0, &path).map_err(|e| e.to_string())? {
        return Ok(Some(folder));
    }
    listing.0.resolve_folder(&path)
}

#[tauri::command(async)]
pub fn get_folder_ancestors(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    id: i64,
) -> Result<Vec<db::Folder>, String> {
    let _ = state;
    listing.0.folder_ancestors(id)
}

#[tauri::command(async)]
pub fn get_category_roots(
    state: State<'_, BackgroundDbState>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Root>, String> {
    db::roots_page(&state.0, offset, limit, include_children).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_catalog_nodes(
    state: State<'_, BackgroundDbState>,
    kind: String,
    parent_id: Option<i64>,
    root_id: Option<i64>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::nodes_page(&state.0, &kind, parent_id, root_id, offset, limit, include_children).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn search_keywords(
    state: State<'_, DatabaseState>,
    query: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::keyword_search_page(&state.0, &query, offset, limit).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn search_catalog_tree(
    state: State<'_, BackgroundDbState>,
    kind: String,
    query: String,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::catalog_tree_search(&state.0, &kind, &query, offset, limit, include_children).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_catalog_values(
    state: State<'_, BackgroundDbState>,
    formats: State<'_, FormatsState>,
    kind: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::ValueCount>, String> {
    db::values_page(&state.0, &kind, offset, limit, &formats.0).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_folder_rules(writer: State<'_, CatalogWriterState>) -> Result<Vec<crate::folder_rules::FolderRule>, String> {
    writer.0.folder_rules()
}

#[tauri::command(async)]
pub fn get_folder_rule_state(
    writer: State<'_, CatalogWriterState>,
    path: String,
) -> Result<crate::folder_rules::FolderRuleState, String> {
    writer.0.folder_rule_state(&path)
}

#[tauri::command(async)]
pub fn get_folder_catalogue_states(
    writer: State<'_, CatalogWriterState>,
    paths: Vec<String>,
) -> Result<std::collections::HashMap<String, crate::folder_rules::FolderCatalogueState>, String> {
    writer.0.folder_catalogue_states(paths)
}

#[tauri::command(async)]
pub fn get_exclusion_impact(
    writer: State<'_, CatalogWriterState>,
    path: String,
) -> Result<crate::journal::ExclusionImpact, String> {
    writer.0.exclusion_impact(&path)
}

/// mode: "include" | "exclude" | null (remove the explicit rule). Excluding removes the folder's catalogue rows.
#[tauri::command(async)]
pub fn set_folder_rule(
    writer: State<'_, CatalogWriterState>,
    ai: State<'_, AiState>,
    listing: State<'_, ListingState>,
    path: String,
    mode: Option<String>,
) -> Result<usize, String> {
    let removed = writer.0.set_folder_rule(&path, mode.as_deref())?;
    let count = removed.len();
    if count > 0 {
        ai.0.forget(removed);
    }
    let _ = listing.0.invalidate_path(&path);
    Ok(count)
}
