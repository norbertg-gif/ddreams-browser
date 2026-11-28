use super::{
    DatabaseState, DisplayMetadataState, ExifState, FormatsState, ListingState, MetadataViewState, ThumbnailState,
};
use crate::db::{self, AssetMetadata};
use crate::display_metadata::{self, CacheEntry, TileExifItem, TileExifValues};
use crate::formats::FormatTable;
use crate::metadata_view::{self, MetadataView};
use crate::thumbs;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataRow {
    pub group: String,
    pub subgroup: String,
    pub name: String,
    pub value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataResponse {
    pub asset: db::Asset,
    pub offline: bool,
    pub rows: Vec<MetadataRow>,
}

#[tauri::command(async)]
pub fn set_thumbnail_epoch(state: State<'_, ThumbnailState>, epoch: u64) {
    state.0.set_epoch(epoch)
}

#[tauri::command(async)]
pub fn get_thumbnail_state(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> thumbs::ThumbnailState {
    state.0.state(asset_id, epoch)
}

fn db_tags(data: &AssetMetadata) -> Vec<metadata_view::RawTag> {
    let a = &data.asset;
    let mut tags = Vec::new();
    let mut add = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            tags.push(metadata_view::RawTag {
                key: format!("Db:{name}"),
                group: "Db".into(),
                name: name.into(),
                value,
            });
        }
    };
    add("Caption", a.caption.clone());
    add("Notes", a.notes.clone());
    add("Author", a.author.clone());
    add(
        "Rating",
        Some(
            a.rating
                .filter(|v| *v > 0)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
    );
    add("Label", Some(a.label.clone().unwrap_or_else(|| "-".into())));
    add(
        "Tagged",
        Some(if a.tagged.unwrap_or(0) != 0 { "Áno" } else { "Nie" }.into()),
    );
    add(
        "Categories",
        (!data.categories.is_empty()).then(|| data.categories.join(", ")),
    );
    add(
        "Keywords",
        (!data.keywords.is_empty()).then(|| data.keywords.join(", ")),
    );
    add("Date taken", a.exif_date.clone());
    add(
        "Dimensions",
        match (a.width, a.height) {
            (Some(w), Some(h)) => Some(format!("{w} × {h}")),
            _ => None,
        },
    );
    add("Bits per pixel", a.bpp.map(|v| v.to_string()));
    add("File modified", a.file_modified.clone());
    tags
}

fn live_rows(json: Value, view: &MetadataView, data: Option<&AssetMetadata>) -> Vec<MetadataRow> {
    let Some(obj) = json.as_object() else {
        return Vec::new();
    };
    let tags = metadata_view::raw_tags(obj);
    rows_from_curated(metadata_view::curate(
        view,
        &tags,
        &data.map(db_tags).unwrap_or_default(),
    ))
}

fn db_only_rows(view: &MetadataView, data: &AssetMetadata) -> Vec<MetadataRow> {
    rows_from_curated(metadata_view::curate(view, &[], &db_tags(data)))
}

fn rows_from_curated(rows: Vec<metadata_view::CuratedRow>) -> Vec<MetadataRow> {
    rows.into_iter()
        .map(|row| MetadataRow {
            group: row.group,
            subgroup: row.subgroup,
            name: row.name,
            value: row.value,
        })
        .collect()
}

#[tauri::command(async)]
pub fn get_metadata(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    exif: State<'_, ExifState>,
    view: State<'_, MetadataViewState>,
    asset_id: i64,
) -> Result<Option<MetadataResponse>, String> {
    if let Some(asset) = listing.0.asset(asset_id) {
        let path = asset.path.clone().unwrap_or_default();
        let online = Path::new(&path).is_file();
        let rows = if online {
            crate::iptc_write::metadata_with_sidecar(&exif.0, Path::new(&path))
                .map(|json| live_rows(json, &view.0, None))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        return Ok(Some(MetadataResponse {
            asset,
            offline: !online,
            rows,
        }));
    }
    let Some(data) = db::asset_metadata(&db.0, asset_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = data.asset.path.as_deref().unwrap_or("");
    let online = Path::new(path).is_file();
    if online {
        if let Ok(value) = crate::iptc_write::metadata_with_sidecar(&exif.0, Path::new(path)) {
            let rows = live_rows(value, &view.0, Some(&data));
            if !rows.is_empty() {
                return Ok(Some(MetadataResponse {
                    asset: data.asset,
                    offline: false,
                    rows,
                }));
            }
        }
    }
    let rows = db_only_rows(&view.0, &data);
    Ok(Some(MetadataResponse {
        asset: data.asset,
        offline: !online,
        rows,
    }))
}

#[tauri::command(async)]
pub fn get_catalog_asset_metadata(
    state: State<'_, DatabaseState>,
    asset_id: i64,
) -> Result<Option<AssetMetadata>, String> {
    db::asset_metadata(&state.0, asset_id).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_thumbnail_metadata(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    exif: State<'_, ExifState>,
    cache: State<'_, DisplayMetadataState>,
    asset_ids: Vec<i64>,
    fields: Vec<String>,
) -> Result<Vec<TileExifItem>, String> {
    #[derive(Clone)]
    struct Candidate {
        id: i64,
        path: PathBuf,
        key: String,
        entry: CacheEntry,
        missing: Vec<String>,
    }
    let requested = display_metadata::allowed_fields(&fields);
    if requested.is_empty() {
        return Ok(asset_ids
            .into_iter()
            .map(|asset_id| TileExifItem {
                asset_id,
                values: TileExifValues::default(),
            })
            .collect());
    }
    let mut candidates = Vec::new();
    for id in asset_ids {
        let (path, date, is_directory) = if let Some(asset) = listing.0.asset(id) {
            (asset.path, asset.exif_date, asset.is_directory)
        } else {
            let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
            conn.query_row(
                "SELECT path,exif_date FROM assets WHERE id=? AND is_listed=1",
                [id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map(|(p, d)| (p, d, false))
            .unwrap_or((None, None, false))
        };
        let Some(path) = path.filter(|_| !is_directory) else {
            candidates.push(Candidate {
                id,
                path: PathBuf::new(),
                key: String::new(),
                entry: CacheEntry::default(),
                missing: Vec::new(),
            });
            continue;
        };
        let path = PathBuf::from(path);
        if !path.is_file() {
            candidates.push(Candidate {
                id,
                path,
                key: String::new(),
                entry: CacheEntry::default(),
                missing: Vec::new(),
            });
            continue;
        }
        let Ok((key, _, _)) = display_metadata::file_key(&path) else {
            candidates.push(Candidate {
                id,
                path,
                key: String::new(),
                entry: CacheEntry::default(),
                missing: Vec::new(),
            });
            continue;
        };
        let entry = cache.0.get(&key);
        let missing = display_metadata::missing_fields(&entry, &requested, date.as_deref());
        candidates.push(Candidate {
            id,
            path,
            key,
            entry,
            missing,
        });
    }
    let query_fields = requested
        .iter()
        .filter(|field| candidates.iter().any(|c| c.missing.contains(field)))
        .cloned()
        .collect::<Vec<_>>();
    if !query_fields.is_empty() {
        let targets = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.missing.is_empty() && !c.key.is_empty())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let paths = targets.iter().map(|i| candidates[*i].path.clone()).collect::<Vec<_>>();
        let tags = display_metadata::tags_for(&query_fields);
        let records = crate::native_exif::metadata_batch(&exif.0, &paths, &tags);
        for (idx, record) in targets.into_iter().zip(records.into_iter()) {
            let Some(record) = record else {
                continue;
            };
            let candidate = &mut candidates[idx];
            if let Some(object) = record.as_object() {
                let values = display_metadata::parse_values(object, &query_fields);
                display_metadata::merge_values(&mut candidate.entry.values, values);
            }
            display_metadata::mark_loaded(&mut candidate.entry, &query_fields);
            cache.0.put(&candidate.key, candidate.entry.clone());
        }
    }
    Ok(candidates
        .into_iter()
        .map(|candidate| TileExifItem {
            asset_id: candidate.id,
            values: candidate.entry.values,
        })
        .collect())
}

#[tauri::command(async)]
pub fn get_catalog_asset_links(
    state: State<'_, DatabaseState>,
    asset_id: i64,
) -> Result<Option<db::CatalogAssetLinks>, String> {
    db::asset_catalog_links(&state.0, asset_id).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_formats(state: State<'_, FormatsState>) -> FormatTable {
    state.0.as_ref().clone()
}

#[tauri::command(async)]
pub fn get_preview_source(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> Result<String, String> {
    state
        .0
        .render(asset_id, 0, epoch)
        .map(|result| result.source)
        .map_err(|e| format!("{e:?}"))
}

#[tauri::command]
pub async fn prepare_full_image(
    state: State<'_, ThumbnailState>,
    asset_id: i64,
    raw_decode: bool,
) -> Result<Option<thumbs::FullImageInfo>, String> {
    let thumbs = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || thumbs.prepare_full_image(asset_id, raw_decode))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn read_full_image_chunk(
    state: State<'_, ThumbnailState>,
    asset_id: i64,
    raw_decode: bool,
    key: String,
    offset: u64,
) -> Result<tauri::ipc::Response, String> {
    let thumbs = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || thumbs.read_full_image_chunk(asset_id, raw_decode, &key, offset))
        .await
        .map_err(|e| e.to_string())?
        .map(tauri::ipc::Response::new)
}
