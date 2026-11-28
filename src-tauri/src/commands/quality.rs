use super::{BackgroundDbState, CatalogWriterState, DatabaseState};
use crate::ai::AiState;
use crate::db;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ColorCount {
    pub value: String,
    pub name: String,
    pub hex: String,
    pub count: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssetColorState {
    pub value: String,
    pub name: String,
    pub hex: String,
    pub auto: bool,
    pub effective: bool,
    pub manual_state: Option<bool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssetColors {
    pub asset_id: i64,
    pub computed: bool,
    pub colors: Vec<AssetColorState>,
}

#[tauri::command(async)]
pub fn set_asset_colors(
    writer: State<'_, CatalogWriterState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
    color: String,
    state: Option<bool>,
) -> Result<usize, String> {
    let count = writer.0.set_asset_colors(&asset_ids, &color, state, "Oprava farby")?;
    if count > 0 {
        ai.0.learn_colors_debounced();
    }
    Ok(count)
}

#[tauri::command(async)]
pub fn get_asset_colors(
    db: State<'_, DatabaseState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
) -> Result<Vec<AssetColors>, String> {
    let attached = ai.0.ensure_colors_attached();
    let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in asset_ids
        .into_iter()
        .filter(|id| *id > 0)
        .filter(|id| seen.insert(*id))
        .take(500)
    {
        let listed: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !listed {
            continue;
        }
        let auto_json: Option<String> = if attached {
            conn.query_row("SELECT auto FROM ai.colors WHERE asset_id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?
        } else {
            None
        };
        let auto = auto_json
            .as_deref()
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
            .unwrap_or_default();
        let overrides = conn
            .prepare("SELECT color,state FROM asset_color_overrides WHERE asset_id=?1")
            .map_err(|e| e.to_string())?
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let effective = crate::colors::effective_colors(&auto, &overrides);
        let colors = crate::colors::PALETTE
            .iter()
            .map(|c| {
                let manual = overrides.iter().find(|o| o.0 == c.value).map(|o| o.1 != 0);
                AssetColorState {
                    value: c.value.into(),
                    name: c.name.into(),
                    hex: c.hex.into(),
                    auto: auto.iter().any(|v| v == c.value),
                    effective: effective.iter().any(|v| v == c.value),
                    manual_state: manual,
                }
            })
            .collect();
        out.push(AssetColors {
            asset_id: id,
            computed: auto_json.is_some(),
            colors,
        });
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn get_color_counts(db: State<'_, BackgroundDbState>, ai: State<'_, AiState>) -> Result<Vec<ColorCount>, String> {
    // The catalogue lock is held only for the two quick catalogue reads. ai.sqlite (hundreds of MB,
    // on the same disk the AI model loads from) is read through its own read-only connection:
    // with the lock held, a slow cold read here used to freeze every other query for many seconds.
    let mut counts = std::collections::HashMap::<String, usize>::new();
    let (ids, overrides) = {
        let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
        let ids = conn
            .prepare("SELECT id FROM assets WHERE is_listed=1 AND id>0")
            .map_err(|e| e.to_string())?
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let overrides = conn
            .prepare("SELECT asset_id,color,state FROM asset_color_overrides")
            .map_err(|e| e.to_string())?
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        (ids, overrides)
    };
    let mut auto_by_id = std::collections::HashMap::<i64, String>::new();
    let ai_db = ai.0.ai_db_path();
    if ai_db.is_file() {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(&ai_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
            if let Ok(mut stmt) = conn.prepare("SELECT asset_id,auto FROM colors") {
                if let Ok(found) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))) {
                    for (id, auto) in found.flatten() {
                        if let Some(auto) = auto {
                            auto_by_id.insert(id, auto);
                        }
                    }
                }
            }
        }
    }
    let rows: Vec<(i64, Option<String>)> = ids
        .into_iter()
        .map(|id| {
            let auto = auto_by_id.remove(&id);
            (id, auto)
        })
        .collect();
    let mut by_id = std::collections::HashMap::<i64, Vec<(String, i64)>>::new();
    for (id, color, state) in overrides {
        by_id.entry(id).or_default().push((color, state));
    }
    for (id, json) in rows {
        let auto = json
            .as_deref()
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
            .unwrap_or_default();
        let effective = crate::colors::effective_colors(&auto, by_id.get(&id).map(Vec::as_slice).unwrap_or(&[]));
        for color in effective {
            *counts.entry(color).or_default() += 1;
        }
    }
    Ok(crate::colors::PALETTE
        .iter()
        .map(|c| ColorCount {
            value: c.value.into(),
            name: c.name.into(),
            hex: c.hex.into(),
            count: *counts.get(c.value).unwrap_or(&0),
        })
        .collect())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssetQuality {
    pub asset_id: i64,
    pub sharp: f64,
    pub hi: f64,
    pub lo: f64,
    pub mean: f64,
    pub flags: Vec<String>,
}

fn quality_connection(ai: &AiState) -> Option<rusqlite::Connection> {
    let path = ai.0.ai_db_path();
    if !path.is_file() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
    Some(conn)
}

#[tauri::command(async)]
pub fn get_asset_quality(
    db: State<'_, BackgroundDbState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
) -> Result<Vec<AssetQuality>, String> {
    let ids = {
        let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
        let wanted = asset_ids
            .into_iter()
            .filter(|id| *id > 0)
            .collect::<std::collections::HashSet<_>>();
        let mut out = std::collections::HashSet::new();
        for chunk in wanted.iter().copied().collect::<Vec<_>>().chunks(500) {
            let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
            let mut stmt = conn
                .prepare(&format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})"))
                .map_err(|e| e.to_string())?;
            for row in stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
            {
                out.insert(row.map_err(|e| e.to_string())?);
            }
        }
        out
    };
    let Some(conn) = quality_connection(&ai) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for chunk in ids.iter().copied().collect::<Vec<_>>().chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT asset_id,sharp,hi,lo,mean FROM quality WHERE version=? AND asset_id IN ({marks})");
        let mut stmt = match conn.prepare(&sql) {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };
        let mut args = vec![rusqlite::types::Value::Integer(crate::quality::QUALITY_VERSION)];
        args.extend(chunk.iter().copied().map(Into::into));
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, f64>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, sharp, hi, lo, mean) = row.map_err(|e| e.to_string())?;
            let flags = crate::quality::flags(sharp, hi, lo, mean)
                .into_iter()
                .map(str::to_owned)
                .collect();
            out.push(AssetQuality {
                asset_id: id,
                sharp,
                hi,
                lo,
                mean,
                flags,
            });
        }
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn get_quality_counts(
    db: State<'_, BackgroundDbState>,
    ai: State<'_, AiState>,
) -> Result<Vec<db::ValueCount>, String> {
    let ids = {
        let conn = db.0.lock().map_err(|_| "catalog lock poisoned")?;
        conn.prepare("SELECT id FROM assets WHERE is_listed=1 AND id>0")
            .map_err(|e| e.to_string())?
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<std::collections::HashSet<i64>>>()
            .map_err(|e| e.to_string())?
    };
    let mut counts = std::collections::HashMap::<&'static str, i64>::new();
    if let Some(conn) = quality_connection(&ai) {
        if let Ok(mut stmt) = conn.prepare("SELECT asset_id,sharp,hi,mean FROM quality WHERE version=?") {
            if let Ok(rows) = stmt.query_map([crate::quality::QUALITY_VERSION], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (id, sharp, hi, mean) = row;
                    if !ids.contains(&id) {
                        continue;
                    }
                    for flag in crate::quality::flags(sharp, hi, 0.0, mean) {
                        *counts.entry(flag).or_default() += 1;
                    }
                }
            }
        }
    }
    Ok(["blur", "over", "under"]
        .into_iter()
        .map(|value| db::ValueCount {
            value: value.into(),
            count: *counts.get(value).unwrap_or(&0),
        })
        .collect())
}
