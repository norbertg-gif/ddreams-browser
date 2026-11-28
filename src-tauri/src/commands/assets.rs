use super::sources::classify_source;
use super::{
    AutoExifState, BackgroundDbState, CatalogWriterState, DatabaseState, FileManagerState, FormatsState, ListingState,
};
use crate::ai::AiState;
use crate::db::{self, AssetFilter, Db};
use crate::journal::ListedAsset;
use crate::listing::ListingService;
use std::path::Path;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

#[tauri::command(async)]
pub fn auto_categories(
    state: State<'_, AutoExifState>,
) -> Result<(Vec<crate::exif_index::Group>, crate::exif_index::Progress), String> {
    state.0.snapshot()
}

fn classify_page(page: &mut db::AssetPage) {
    let mut cache = std::collections::HashMap::new();
    for asset in &mut page.items {
        if let Some(path) = asset.path.as_deref() {
            let folder = Path::new(path).parent().and_then(Path::to_str).unwrap_or(path);
            (asset.offline, asset.missing) = classify_source(folder, &mut cache)
        }
    }
}

#[tauri::command]
pub async fn get_assets(
    app: tauri::AppHandle,
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    ai: State<'_, AiState>,
    writer: State<'_, CatalogWriterState>,
    file_manager: State<'_, FileManagerState>,
    filter: AssetFilter,
) -> Result<db::AssetPage, String> {
    let (db, listing, formats, ai, writer, file_manager) = (
        state.0.clone(),
        listing.0.clone(),
        formats.0.clone(),
        ai.0.clone(),
        writer.0.clone(),
        file_manager.0.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        // Only the first page checks for new files and missing catalogue dates.
        if filter.kind == "folder" && filter.offset == 0 && writer.is_writable() {
            let _operation = file_manager.lock_operation()?;
            if auto_catalog_folder(&listing, &formats, &writer, &ai, &filter).is_ok_and(|count| count > 0) {
                let _ = app.emit("folder-catalogue-changed", ());
            }
        }
        let mut page = if filter.sort == "similarity"
            && filter.kind != "series"
            && filter.kind != "people"
            && !(filter.kind == "special" && filter.value.as_deref() == Some("edit_pending"))
        {
            similarity_page(&db, &listing, &formats, &filter, &ai.status(), |ids| {
                ai.order_by_similarity(ids)
            })?
        } else if filter.kind == "folder" {
            listing.page(&filter)?
        } else {
            db::assets_page(&db, &filter, &formats).map_err(|e| e.to_string())?
        };
        classify_page(&mut page);
        Ok(page)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn locate_asset(app: tauri::AppHandle, filter: AssetFilter, id: i64) -> Option<usize> {
    if filter.sort == "similarity" || matches!(filter.kind.as_str(), "ai" | "series" | "people") {
        return None;
    }
    let db = app.state::<DatabaseState>().0.clone();
    let listing = app.state::<ListingState>().0.clone();
    let formats = app.state::<FormatsState>().0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if filter.kind == "folder" {
            listing.locate_asset(&filter, id).ok().flatten()
        } else {
            db::locate_asset(&db, &filter, &formats, id).ok().flatten()
        }
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command(async)]
pub fn get_folder_previews(
    state: State<'_, BackgroundDbState>,
    formats: State<'_, FormatsState>,
    paths: Vec<String>,
) -> Result<std::collections::HashMap<String, Vec<db::FolderPreviewAsset>>, String> {
    db::folder_previews(&state.0, &paths, &formats.0).map_err(|e| e.to_string())
}

fn auto_catalog_folder(
    listing: &ListingService,
    formats: &crate::formats::FormatTable,
    writer: &crate::journal::CatalogWriter,
    ai: &crate::ai::AiService,
    filter: &AssetFilter,
) -> Result<usize, String> {
    let (path, files) = listing.listed_folder(filter)?;
    writer.observe_incoming_folder(&path)?;
    let fulfilled = writer.fulfil_edits_from_listing(&files)?;
    if fulfilled > 0 {
        listing.invalidate_path(&path)?;
    }
    let dates = listing.missing_capture_dates(files.iter().filter(|asset| asset.id > 0));
    let backfilled = writer.backfill_capture_dates(&dates)?;
    if backfilled > 0 {
        listing.invalidate_path(&path)?;
    }
    let updated = backfilled + fulfilled;
    if path.is_empty() || !files.iter().any(|asset| !asset.is_directory && asset.id < 0) {
        return Ok(updated);
    }
    let mode = writer.effective_folder_mode(&path);
    let included = mode.as_deref() == Some("include");
    if mode.as_deref() == Some("exclude")
        || (!included
            && !ai
                .incoming_folder()
                .is_ok_and(|folder| crate::incoming::contains_folder(&path, &folder)))
    {
        return Ok(updated);
    }
    let candidates = files
        .iter()
        .filter(|a| !a.is_directory && a.id < 0)
        .filter(|a| match formats.group(&a.ext) {
            Some("image" | "raw") => true,
            Some("video") => included,
            _ => false,
        })
        .filter_map(|a| {
            let file_path = a.path.clone()?;
            let folder_path = Path::new(&file_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            Some(ListedAsset {
                temporary_id: a.id,
                name: a.name.clone(),
                ext: a.ext.clone(),
                size: a.size,
                file_modified: a.file_modified.clone(),
                path: file_path,
                folder_path,
            })
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(updated);
    }
    let dates = listing.missing_capture_dates(files.iter().filter(|asset| asset.id < 0));
    let remaps = if dates.is_empty() {
        writer.catalog_listed_assets(&candidates)?
    } else {
        writer.catalog_listed_assets_with_dates(&candidates, &dates)?
    };
    if !remaps.is_empty() {
        let _ = listing.invalidate_path(&path);
        ai.index_assets(&remaps.iter().map(|r| r.new_id).collect::<Vec<_>>());
    }
    Ok(remaps.len() + updated)
}

#[tauri::command(async)]
pub fn refresh_folder_listing(listing: State<'_, ListingState>, folder_id: i64) -> Result<(), String> {
    listing.0.invalidate(folder_id)
}

#[tauri::command(async)]
pub async fn check_folder_changed(listing: State<'_, ListingState>, folder_id: i64) -> Result<bool, String> {
    let service = listing.0.clone();
    Ok(
        tauri::async_runtime::spawn_blocking(move || service.check_folder_changed(folder_id))
            .await
            .unwrap_or(false),
    )
}

#[tauri::command(async)]
pub fn get_asset_ids(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    filter: AssetFilter,
    offset: i64,
    limit: i64,
) -> Result<Vec<i64>, String> {
    if filter.kind == "folder" {
        let mut f = filter;
        f.offset = offset;
        f.limit = limit;
        return Ok(listing.0.page(&f)?.items.into_iter().map(|a| a.id).collect());
    }
    if filter.kind == "ai" || filter.kind == "series" || filter.kind == "people" {
        let mut f = filter;
        f.offset = offset;
        f.limit = limit;
        return Ok(db::assets_page(&state.0, &f, &formats.0)
            .map_err(|e| e.to_string())?
            .items
            .into_iter()
            .map(|a| a.id)
            .collect());
    }
    db::asset_ids_page(&state.0, &filter, &formats.0, offset, limit).map_err(|e| e.to_string())
}

struct SimilarityOrder {
    ids: Vec<i64>,
    total_size: f64,
    folder: Option<(Arc<Vec<db::Asset>>, std::collections::HashMap<i64, usize>)>,
}

struct SimilarityEntry {
    owner: std::sync::Weak<std::sync::Mutex<rusqlite::Connection>>,
    filter: String,
    revision: (u64, i64, i64, i64, u64, bool, u64, u64),
    at: std::time::Instant,
    order: Arc<SimilarityOrder>,
}

pub(super) fn similarity_page(
    db: &Db,
    listing: &ListingService,
    formats: &crate::formats::FormatTable,
    filter: &AssetFilter,
    status: &crate::ai::AiStatus,
    rank: impl FnOnce(Vec<i64>) -> Result<Vec<i64>, String>,
) -> Result<db::AssetPage, String> {
    static CACHE: std::sync::Mutex<Vec<SimilarityEntry>> = std::sync::Mutex::new(Vec::new());
    let mut base = filter.clone();
    base.sort = "name".into();
    base.descending = false;
    base.offset = 0;
    base.limit = 0;
    base.include_categories = false;
    base.include_keywords = false;
    let key = serde_json::to_string(&base).map_err(|e| e.to_string())?;
    let revision = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let version = |sql| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap_or(-1);
        (
            conn.total_changes(),
            version("PRAGMA main.data_version"),
            version("PRAGMA ai.data_version"),
            version("PRAGMA auto_exif.data_version"),
            if base.kind == "folder" { listing.revision() } else { 0 },
            status.available,
            status.indexed,
            status.done,
        )
    };
    let cached = {
        let mut cache = CACHE.lock().map_err(|e| e.to_string())?;
        cache.retain(|entry| entry.owner.strong_count() > 0 && entry.at.elapsed().as_secs() < 8);
        cache
            .iter()
            .position(|entry| {
                entry.owner.ptr_eq(&Arc::downgrade(db)) && entry.filter == key && entry.revision == revision
            })
            .map(|pos| {
                let entry = cache.remove(pos);
                let order = entry.order.clone();
                cache.push(entry);
                order
            })
    };
    let order = if let Some(order) = cached {
        order
    } else {
        let (mut ids, total_size, folder, directories) = if base.kind == "folder" {
            let (rows, total_size) = listing.ordered_rows(&base)?;
            let positions = rows.iter().enumerate().map(|(i, asset)| (asset.id, i)).collect();
            let ids = rows.iter().map(|asset| asset.id).collect::<Vec<_>>();
            let directories = rows.iter().take_while(|asset| asset.is_directory).count();
            (ids, total_size, Some((rows, positions)), directories)
        } else {
            let conn = db.lock().map_err(|e| e.to_string())?;
            let (ids, total_size) = db::catalog_ordered_ids(db, &conn, &base, formats).map_err(|e| e.to_string())?;
            (ids.as_ref().clone(), total_size, None, 0)
        };
        if status.available && directories < ids.len() {
            let ranked = rank(ids[directories..].to_vec())?;
            rank_similarity_ids(&mut ids[directories..], &ranked);
        }
        let order = Arc::new(SimilarityOrder {
            ids,
            total_size,
            folder,
        });
        let mut cache = CACHE.lock().map_err(|e| e.to_string())?;
        cache.retain(|entry| !(entry.owner.ptr_eq(&Arc::downgrade(db)) && entry.filter == key));
        if cache.len() >= 2 {
            cache.remove(0);
        }
        cache.push(SimilarityEntry {
            owner: Arc::downgrade(db),
            filter: key,
            revision,
            at: std::time::Instant::now(),
            order: order.clone(),
        });
        order
    };
    let ids = &order.ids;
    let start = (filter.offset.max(0) as usize).min(ids.len());
    let end = (start + filter.limit.clamp(1, 500) as usize).min(ids.len());
    let ids_page = &ids[start..end];
    let items = if let Some((rows, positions)) = &order.folder {
        let mut items = ids_page
            .iter()
            .filter_map(|id| positions.get(id).map(|&position| rows[position].clone()))
            .collect::<Vec<_>>();
        db::attach_display_values(db, &mut items, filter.include_categories, filter.include_keywords)
            .map_err(|e| e.to_string())?;
        items
    } else {
        let conn = db.lock().map_err(|e| e.to_string())?;
        db::hydrate_asset_ids(&conn, ids_page, filter).map_err(|e| e.to_string())?
    };
    Ok(db::AssetPage {
        items,
        total: ids.len() as i64,
        total_size: order.total_size,
    })
}

pub(super) fn rank_similarity_ids(ids: &mut [i64], ranked: &[i64]) {
    let positions = ranked
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect::<std::collections::HashMap<_, _>>();
    ids.sort_by_key(|id| positions.get(id).copied().unwrap_or(usize::MAX));
}
