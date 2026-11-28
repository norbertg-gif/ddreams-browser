use super::faces::filter_listed_ids;
use super::{BackgroundDbState, DatabaseState, FormatsState, ListingState};
use crate::ai::{AiSearchResult, AiState, AiStatus};
use crate::db::{self, AssetFilter};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

#[tauri::command(async)]
pub fn ai_status(state: State<'_, AiState>) -> AiStatus {
    state.0.status()
}

#[tauri::command(async)]
pub fn ai_start(state: State<'_, AiState>, lazy_model: Option<bool>) -> Result<AiStatus, String> {
    state.0.start(lazy_model.unwrap_or(false))
}

#[tauri::command(async)]
pub fn ai_load_model(state: State<'_, AiState>) -> Result<AiStatus, String> {
    state.0.load_model()
}

#[tauri::command(async)]
pub fn ai_stop(state: State<'_, AiState>) -> Result<AiStatus, String> {
    state.0.stop()
}

#[tauri::command(async)]
pub fn ai_pause(state: State<'_, AiState>) -> Result<AiStatus, String> {
    state.0.pause()
}

#[tauri::command(async)]
pub fn ai_resume(state: State<'_, AiState>) -> Result<AiStatus, String> {
    state.0.resume()
}

#[tauri::command(async)]
pub fn ai_reindex(state: State<'_, AiState>) -> Result<AiStatus, String> {
    state.0.reindex()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMapCluster {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub count: usize,
    pub name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMapData {
    pub ids: Vec<i64>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub cluster: Vec<u32>,
    pub clusters: Vec<AiMapCluster>,
    pub built_at: Option<f64>,
    pub indexed: usize,
}

#[tauri::command(async)]
pub fn ai_map_build(state: State<'_, AiState>) -> Result<Value, String> {
    state.0.map_build()
}

#[tauri::command(async)]
pub fn ai_map_data(db: State<'_, BackgroundDbState>, ai: State<'_, AiState>) -> Result<AiMapData, String> {
    let empty = || AiMapData {
        ids: Vec::new(),
        x: Vec::new(),
        y: Vec::new(),
        cluster: Vec::new(),
        clusters: Vec::new(),
        built_at: None,
        indexed: 0,
    };
    let path = ai.0.ai_db_path();
    if !path.is_file() {
        return Ok(empty());
    }
    let conn = match rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return Ok(empty()),
    };
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='map_points')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !exists {
        return Ok(empty());
    }
    let points = {
        let mut stmt = conn
            .prepare("SELECT asset_id,x,y,cluster FROM map_points ORDER BY asset_id")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f32>(1)?,
                r.get::<_, f32>(2)?,
                r.get::<_, u32>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?
    };
    let mut allowed = std::collections::HashSet::new();
    let catalog = db.0.lock().map_err(|_| "catalog lock poisoned")?;
    for chunk in points.iter().map(|p| p.0).collect::<Vec<_>>().chunks(800) {
        if chunk.is_empty() {
            continue;
        }
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt = catalog
            .prepare(&format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})"))
            .map_err(|e| e.to_string())?;
        for id in stmt
            .query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
        {
            allowed.insert(id.map_err(|e| e.to_string())?);
        }
    }
    drop(catalog);
    let filtered = points
        .into_iter()
        .filter(|p| allowed.contains(&p.0))
        .collect::<Vec<_>>();
    let clusters = {
        let mut stmt = conn
            .prepare("SELECT id,x,y,r,count,name FROM map_clusters ORDER BY id")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| {
            Ok(AiMapCluster {
                id: r.get::<_, u32>(0)?,
                x: r.get(1)?,
                y: r.get(2)?,
                r: r.get(3)?,
                count: r.get::<_, i64>(4)?.max(0) as usize,
                name: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?
    };
    let built_at = conn
        .query_row("SELECT value FROM meta WHERE key='map_built_at'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok());
    let indexed = conn
        .query_row("SELECT value FROM meta WHERE key='map_photo_count'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(filtered.len());
    Ok(AiMapData {
        ids: filtered.iter().map(|p| p.0).collect(),
        x: filtered.iter().map(|p| p.1).collect(),
        y: filtered.iter().map(|p| p.2).collect(),
        cluster: filtered.iter().map(|p| p.3).collect(),
        clusters,
        built_at,
        indexed,
    })
}

#[tauri::command(async)]
pub fn ai_search_text(state: State<'_, AiState>, query: String, limit: Option<u32>) -> Result<AiSearchResult, String> {
    state.0.search_text(query, limit)
}

#[tauri::command(async)]
pub fn ai_similar(state: State<'_, AiState>, asset_id: i64, limit: Option<u32>) -> Result<AiSearchResult, String> {
    state.0.similar(asset_id, limit)
}

#[tauri::command(async)]
pub fn ai_duplicate_groups(state: State<'_, AiState>, level: String) -> Result<Value, String> {
    state.0.duplicate_groups(&level)
}

#[derive(Serialize)]
pub struct AiSeriesGroup {
    pub ids: Vec<i64>,
    pub best: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSeriesResult {
    pub groups: Vec<AiSeriesGroup>,
    pub analyzed: usize,
    pub truncated: bool,
}

pub(crate) fn exif_date_seconds(value: &str) -> Option<f64> {
    if !value.is_ascii() {
        return None;
    }
    let split = value.find(|c| c == 'T' || c == ' ')?;
    let date = &value[..split];
    let time = &value[split + 1..];
    if date.len() != 10 || date.as_bytes()[4] != b'-' || date.as_bytes()[7] != b'-' || time.len() < 8 {
        return None;
    }
    let year = date[0..4].parse::<i64>().ok()?;
    let month = date[5..7].parse::<i64>().ok()?;
    let day = date[8..10].parse::<i64>().ok()?;
    let hour = time[0..2].parse::<i64>().ok()?;
    let minute = time[3..5].parse::<i64>().ok()?;
    let second = time[6..8].parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if day < 1 || day > month_days[(month - 1) as usize] {
        return None;
    }
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let tail = &time[8..];
    let offset = if tail.starts_with('+') || tail.starts_with('-') {
        let sign = if tail.starts_with('+') { 1 } else { -1 };
        let h = tail.get(1..3)?.parse::<i64>().ok()?;
        let m = tail.get(4..6)?.parse::<i64>().ok()?;
        sign * (h * 3600 + m * 60)
    } else {
        0
    };
    Some((days * 86400 + hour * 3600 + minute * 60 + second - offset) as f64)
}

#[tauri::command(async)]
pub fn ai_series(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    ai: State<'_, AiState>,
    mut filter: AssetFilter,
    preset: String,
) -> Result<AiSeriesResult, String> {
    let raw_only = filter.file_group.as_deref() == Some("raw");
    if matches!(filter.file_group.as_deref(), Some("video" | "other")) {
        return Ok(AiSeriesResult {
            groups: Vec::new(),
            analyzed: 0,
            truncated: false,
        });
    }
    filter.file_group = Some(if raw_only { "raw".into() } else { "images".into() });
    filter.sort = "captureTime".into();
    filter.descending = false;
    filter.offset = 0;
    filter.limit = 500;
    let cap = 20_000usize;
    let mut truncated = false;
    let mut candidates: Vec<(i64, Option<f64>)> = Vec::new();
    if filter.kind == "ai" {
        let page = db::assets_page(&state.0, &filter, &formats.0).map_err(|e| e.to_string())?;
        candidates.extend(
            page.items
                .into_iter()
                .filter(|a| !a.is_directory)
                .map(|a| (a.id, a.exif_date.as_deref().and_then(exif_date_seconds))),
        );
    } else {
        let mut offset = 0i64;
        loop {
            filter.offset = offset;
            let (rows, page_count, total) = if filter.kind == "folder" {
                let page = listing.0.page(&filter)?;
                let page_count = page.items.len();
                let total = page.total;
                (
                    page.items
                        .into_iter()
                        .filter(|a| !a.is_directory)
                        .map(|a| (a.id, a.exif_date.as_deref().and_then(exif_date_seconds)))
                        .collect::<Vec<_>>(),
                    page_count,
                    Some(total),
                )
            } else {
                let ids = db::asset_ids_page(&state.0, &filter, &formats.0, offset, 500).map_err(|e| e.to_string())?;
                let page_count = ids.len();
                let conn = state.0.lock().map_err(|_| "catalog lock poisoned")?;
                let mut rows = Vec::new();
                for chunk in ids.chunks(500) {
                    let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
                    let mut stmt = conn
                        .prepare(&format!(
                            "SELECT id,exif_date FROM assets WHERE id IN ({marks}) AND is_listed=1"
                        ))
                        .map_err(|e| e.to_string())?;
                    rows.extend(
                        stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                        })
                        .map_err(|e| e.to_string())?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| e.to_string())?,
                    );
                }
                (
                    rows.into_iter()
                        .map(|(id, date)| (id, date.as_deref().and_then(exif_date_seconds)))
                        .collect::<Vec<_>>(),
                    page_count,
                    None,
                )
            };
            for (id, date) in rows {
                if candidates.len() == cap {
                    truncated = true;
                    break;
                }
                candidates.push((id, date));
            }
            if truncated || page_count < 500 || total.is_some_and(|n| offset + page_count as i64 >= n) {
                break;
            }
            offset += page_count as i64;
        }
    }
    candidates.sort_by(|a, b| {
        a.1.is_none()
            .cmp(&b.1.is_none())
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.0.cmp(&b.0))
    });
    if candidates.len() > cap {
        candidates.truncate(cap);
        truncated = true;
    }
    let items = candidates
        .iter()
        .map(|(id, date)| serde_json::json!({"asset_id":id,"t":date}))
        .collect::<Vec<_>>();
    let analyzed = items.len();
    let result = ai.0.analyze_series(items, preset.as_str())?;
    let groups = result
        .get("groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|group| {
            let ids = group
                .get("ids")?
                .as_array()?
                .iter()
                .filter_map(Value::as_i64)
                .collect::<Vec<_>>();
            let best = group.get("best")?.as_i64()?;
            (ids.len() > 1).then_some(AiSeriesGroup { ids, best })
        })
        .collect();
    Ok(AiSeriesResult {
        groups,
        analyzed,
        truncated,
    })
}

#[tauri::command(async)]
pub fn ai_duplicates_of(state: State<'_, AiState>, asset_id: i64, level: String) -> Result<Vec<i64>, String> {
    state.0.duplicates_of(asset_id, &level)
}

#[tauri::command(async)]
pub fn ai_suggest(state: State<'_, AiState>, asset_ids: Vec<i64>, kind: String) -> Result<Value, String> {
    state.0.suggest(asset_ids, &kind)
}

#[tauri::command(async)]
pub fn ai_content_keywords(state: State<'_, AiState>, asset_id: i64) -> Result<Value, String> {
    state.0.content_keywords(asset_id)
}

#[tauri::command(async)]
pub fn ai_keyword_matches(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    keyword_id: i64,
) -> Result<Value, String> {
    let mut result = ai.0.keyword_matches(keyword_id)?;
    let ids: Vec<i64> = result["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    result["ids"] = serde_json::json!(filter_listed_ids(&db.0, &ids, false)?);
    Ok(result)
}

#[tauri::command(async)]
pub fn ai_incoming_groups(ai: State<'_, AiState>) -> Result<Value, String> {
    ai.0.incoming_groups()
}

#[tauri::command(async)]
pub fn ai_reject_suggestion(
    state: State<'_, AiState>,
    asset_ids: Vec<i64>,
    kind: String,
    label_id: i64,
) -> Result<usize, String> {
    state.0.reject_suggestion(&asset_ids, &kind, label_id)
}

#[tauri::command(async)]
pub fn ai_suggestion_groups(state: State<'_, AiState>, kind: String) -> Result<Value, String> {
    state.0.suggestion_groups(&kind)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUnsortedCluster {
    pub ids: Vec<i64>,
    pub total: usize,
    pub cohesion: f64,
    pub label_id: Option<i64>,
    pub label_path: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUnsortedClusters {
    pub groups: Vec<AiUnsortedCluster>,
    pub unlabeled: usize,
    pub clustered: usize,
}

#[tauri::command(async)]
pub fn ai_unsorted_clusters(state: State<'_, AiState>, min_size: Option<u32>) -> Result<AiUnsortedClusters, String> {
    let value = state.0.unsorted_clusters(min_size)?;
    let groups = value
        .get("groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|g| AiUnsortedCluster {
            ids: g
                .get("ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| v.as_i64())
                .collect(),
            total: g.get("total").and_then(Value::as_u64).unwrap_or(0) as usize,
            cohesion: g.get("cohesion").and_then(Value::as_f64).unwrap_or(0.0),
            label_id: g.get("labelId").and_then(Value::as_i64),
            label_path: g.get("labelPath").and_then(Value::as_str).map(str::to_owned),
            name: g.get("name").and_then(Value::as_str).map(str::to_owned),
        })
        .collect();
    Ok(AiUnsortedClusters {
        groups,
        unlabeled: value.get("unlabeled").and_then(Value::as_u64).unwrap_or(0) as usize,
        clustered: value.get("clustered").and_then(Value::as_u64).unwrap_or(0) as usize,
    })
}

#[tauri::command(async)]
pub fn ai_category_paths(state: State<'_, AiState>) -> Result<Vec<Value>, String> {
    Ok(state
        .0
        .category_paths()?
        .into_iter()
        .map(|(id, path)| serde_json::json!({"id":id,"path":path}))
        .collect())
}
