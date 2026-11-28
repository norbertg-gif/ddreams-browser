use super::DatabaseState;
use crate::ai::AiState;
use crate::db::Db;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFacesStatus {
    pub available: bool,
    pub scanned: usize,
    pub pending: usize,
    pub faces: usize,
    pub named: usize,
    pub unknown: usize,
    pub persons: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFacePerson {
    pub id: i64,
    pub name: String,
    pub confirmed: usize,
    pub auto: usize,
    pub assets: usize,
    pub cover: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFacesPersons {
    pub persons: Vec<AiFacePerson>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFaceReview {
    pub face_id: i64,
    pub asset_id: i64,
    pub score: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFacesReview {
    pub faces: Vec<AiFaceReview>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFaceClusterGuess {
    pub person_id: i64,
    pub name: String,
    pub score: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFaceCluster {
    pub face_ids: Vec<i64>,
    pub total: usize,
    pub asset_ids: Vec<i64>,
    pub assets: usize,
    pub guess: Option<AiFaceClusterGuess>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFacesClusters {
    pub available: bool,
    pub groups: Vec<AiFaceCluster>,
    pub unknown: usize,
    pub clustered: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFaceAsset {
    pub face_id: i64,
    pub asset_id: i64,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub person_id: Option<i64>,
    pub name: Option<String>,
    pub state: String,
    pub score: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiFaceThumb {
    pub face_id: i64,
    pub data_url: String,
}

#[tauri::command(async)]
pub fn ai_faces_status(state: State<'_, AiState>) -> Result<AiFacesStatus, String> {
    let v = state.0.faces_status()?;
    Ok(AiFacesStatus {
        available: v["available"].as_bool().unwrap_or(false),
        scanned: v["scanned"].as_u64().unwrap_or(0) as usize,
        pending: v["pending"].as_u64().unwrap_or(0) as usize,
        faces: v["faces"].as_u64().unwrap_or(0) as usize,
        named: v["named"].as_u64().unwrap_or(0) as usize,
        unknown: v["unknown"].as_u64().unwrap_or(0) as usize,
        persons: v["persons"].as_u64().unwrap_or(0) as usize,
    })
}

#[tauri::command(async)]
pub fn ai_faces_persons(state: State<'_, AiState>) -> Result<AiFacesPersons, String> {
    let v = state.0.faces_persons()?;
    let persons = v["persons"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            Some(AiFacePerson {
                id: p["id"].as_i64()?,
                name: p["name"].as_str()?.to_owned(),
                confirmed: p["confirmed"].as_u64().unwrap_or(0) as usize,
                auto: p["auto"].as_u64().unwrap_or(0) as usize,
                assets: p["assets"].as_u64().unwrap_or(0) as usize,
                cover: p["cover"].as_i64(),
            })
        })
        .collect();
    Ok(AiFacesPersons { persons })
}

pub(super) fn filter_listed_ids(db: &Db, ids: &[i64], sort_by_date: bool) -> Result<Vec<i64>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.lock().map_err(|e| e.to_string())?;
    if sort_by_date {
        let mut dated = Vec::<(i64, Option<String>)>::new();
        for chunk in ids.chunks(800) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT id,exif_date FROM assets WHERE is_listed=1 AND id IN ({marks})");
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            dated.extend(
                stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?,
            );
        }
        dated.sort_by(|a, b| match (&a.1, &b.1) {
            (Some(x), Some(y)) => x.cmp(y).then(a.0.cmp(&b.0)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        });
        return Ok(dated.into_iter().map(|v| v.0).collect());
    }
    let mut listed = std::collections::HashSet::new();
    for chunk in ids.chunks(800) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        listed.extend(
            stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?,
        );
    }
    let mut seen = std::collections::HashSet::new();
    Ok(ids
        .iter()
        .copied()
        .filter(|id| listed.contains(id) && seen.insert(*id))
        .collect())
}

#[tauri::command(async)]
pub fn ai_faces_person_assets(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    person_id: i64,
    include_auto: Option<bool>,
) -> Result<Vec<i64>, String> {
    let v = ai.0.faces_person_assets(person_id, include_auto.unwrap_or(true))?;
    let ids = v["asset_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect::<Vec<_>>();
    filter_listed_ids(&db.0, &ids, true)
}

#[tauri::command(async)]
pub fn ai_faces_review(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    person_id: i64,
    limit: Option<u32>,
) -> Result<AiFacesReview, String> {
    let v = ai.0.faces_review(person_id, limit)?;
    let raw = v["faces"].as_array().cloned().unwrap_or_default();
    let ids = raw.iter().filter_map(|f| f["asset_id"].as_i64()).collect::<Vec<_>>();
    let listed = filter_listed_ids(&db.0, &ids, false)?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let faces = raw
        .into_iter()
        .filter_map(|f| {
            let asset_id = f["asset_id"].as_i64()?;
            if !listed.contains(&asset_id) {
                return None;
            }
            Some(AiFaceReview {
                face_id: f["face_id"].as_i64()?,
                asset_id,
                score: f["score"].as_f64(),
            })
        })
        .collect();
    Ok(AiFacesReview { faces })
}

#[tauri::command(async)]
pub fn ai_faces_clusters(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    min_size: Option<u32>,
) -> Result<AiFacesClusters, String> {
    let v = ai.0.faces_clusters(min_size)?;
    let groups = v["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let raw_assets = g["assetIds"].as_array().or_else(|| g["asset_ids"].as_array())?;
            let raw_ids = g["faceIds"].as_array().or_else(|| g["face_ids"].as_array())?;
            let asset_ids = raw_assets.iter().filter_map(Value::as_i64).collect::<Vec<_>>();
            let listed = filter_listed_ids(&db.0, &asset_ids, false).ok()?;
            if listed.is_empty() {
                return None;
            }
            let guess = g.get("guess").filter(|x| !x.is_null()).and_then(|x| {
                Some(AiFaceClusterGuess {
                    person_id: x.get("personId").or_else(|| x.get("person_id"))?.as_i64()?,
                    name: x.get("name")?.as_str()?.to_owned(),
                    score: x.get("score")?.as_f64()?,
                })
            });
            Some(AiFaceCluster {
                face_ids: raw_ids.iter().filter_map(Value::as_i64).collect(),
                total: g["total"].as_u64().unwrap_or(0) as usize,
                assets: g["assets"].as_u64().unwrap_or(listed.len() as u64) as usize,
                asset_ids: listed,
                guess,
            })
        })
        .collect();
    Ok(AiFacesClusters {
        available: v["available"].as_bool().unwrap_or(false),
        groups,
        unknown: v["unknown"].as_u64().unwrap_or(0) as usize,
        clustered: v["clustered"].as_u64().unwrap_or(0) as usize,
    })
}

#[tauri::command(async)]
pub fn ai_faces_for_assets(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    asset_ids: Vec<i64>,
) -> Result<Vec<AiFaceAsset>, String> {
    if asset_ids.len() > 500 {
        return Err("Najviac 500 asset ID naraz".into());
    }
    let ids = asset_ids.into_iter().filter(|id| *id > 0).collect::<Vec<_>>();
    let v = ai.0.faces_for_assets(&ids)?;
    let raw = v["faces"].as_array().cloned().unwrap_or_default();
    let asset_ids = raw.iter().filter_map(|f| f["asset_id"].as_i64()).collect::<Vec<_>>();
    let listed = filter_listed_ids(&db.0, &asset_ids, false)?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    Ok(raw
        .into_iter()
        .filter_map(|f| {
            let asset_id = f["asset_id"].as_i64()?;
            if !listed.contains(&asset_id) {
                return None;
            }
            Some(AiFaceAsset {
                face_id: f["face_id"].as_i64()?,
                asset_id,
                x: f["x"].as_f64(),
                y: f["y"].as_f64(),
                w: f["w"].as_f64(),
                h: f["h"].as_f64(),
                person_id: f["person_id"].as_i64(),
                name: f["name"].as_str().map(str::to_owned),
                state: f["state"].as_str()?.to_owned(),
                score: f["score"].as_f64(),
            })
        })
        .collect())
}

#[tauri::command(async)]
pub fn ai_faces_set(
    ai: State<'_, AiState>,
    face_ids: Vec<i64>,
    action: String,
    person_id: Option<i64>,
) -> Result<usize, String> {
    if face_ids.len() > 500 {
        return Err("Najviac 500 tvárí naraz".into());
    }
    let ids = face_ids.into_iter().filter(|id| *id > 0).collect::<Vec<_>>();
    let v = ai.0.faces_set(&ids, &action, person_id)?;
    Ok(v["changed"].as_u64().unwrap_or(0) as usize)
}

#[tauri::command(async)]
pub fn ai_faces_asset_assign(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    asset_ids: Vec<i64>,
    person_id: i64,
) -> Result<usize, String> {
    if asset_ids.len() > 2000 {
        return Err("Najviac 2000 fotiek naraz".into());
    }
    let ids = filter_listed_ids(&db.0, &asset_ids, false)?;
    let v = ai.0.faces_asset_assign(&ids, person_id)?;
    Ok(v["changed"].as_u64().unwrap_or(0) as usize)
}

#[tauri::command(async)]
pub fn ai_faces_asset_remove(
    ai: State<'_, AiState>,
    db: State<'_, DatabaseState>,
    asset_ids: Vec<i64>,
    person_id: i64,
) -> Result<usize, String> {
    if asset_ids.len() > 2000 {
        return Err("Najviac 2000 fotiek naraz".into());
    }
    let ids = filter_listed_ids(&db.0, &asset_ids, false)?;
    let v = ai.0.faces_asset_remove(&ids, person_id)?;
    Ok(v["changed"].as_u64().unwrap_or(0) as usize)
}

#[tauri::command(async)]
pub fn ai_faces_person_create(ai: State<'_, AiState>, name: String) -> Result<i64, String> {
    let v = ai.0.faces_person_create(&name)?;
    v["id"].as_i64().ok_or_else(|| "AI nevrátilo ID osoby".into())
}

#[tauri::command(async)]
pub fn ai_faces_person_rename(ai: State<'_, AiState>, person_id: i64, name: String) -> Result<bool, String> {
    let v = ai.0.faces_person_rename(person_id, &name)?;
    Ok(v["ok"].as_bool().unwrap_or(false))
}

#[tauri::command(async)]
pub fn ai_faces_recognize_all(ai: State<'_, AiState>) -> Result<usize, String> {
    let v = ai.0.faces_recognize_all()?;
    Ok(v["changed"].as_u64().unwrap_or(0) as usize)
}

#[tauri::command(async)]
pub fn ai_face_thumbs(ai: State<'_, AiState>, face_ids: Vec<i64>) -> Result<Vec<AiFaceThumb>, String> {
    Ok(ai
        .0
        .face_thumbs(&face_ids)?
        .into_iter()
        .filter_map(|v| {
            Some(AiFaceThumb {
                face_id: v["faceId"].as_i64()?,
                data_url: v["dataUrl"].as_str()?.to_owned(),
            })
        })
        .collect())
}

#[tauri::command(async)]
pub fn ai_cluster_ignore(state: State<'_, AiState>, asset_ids: Vec<i64>) -> Result<usize, String> {
    state.0.cluster_ignore(&asset_ids)
}

#[tauri::command(async)]
pub fn ai_cluster_ignore_reset(state: State<'_, AiState>) -> Result<usize, String> {
    state.0.cluster_ignore_reset()
}
