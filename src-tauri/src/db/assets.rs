use super::filters::{
    AssetFilter, CatalogAssetFilter, add_date_filter, add_view_filters, asset_cols, catalog_asset_filter, order_clause,
};
use super::open::Db;
use crate::formats::FormatTable;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct Asset {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub name: String,
    pub ext: String,
    pub size: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub bpp: Option<i64>,
    pub caption: Option<String>,
    pub notes: Option<String>,
    pub author: Option<String>,
    pub rating: Option<i64>,
    pub tagged: Option<i64>,
    pub label: Option<String>,
    pub file_modified: Option<String>,
    pub exif_date: Option<String>,
    pub acdsee_date: Option<String>,
    pub md_dirty: Option<f64>,
    pub is_orphan: Option<i64>,
    pub path: Option<String>,
    #[serde(default)]
    pub is_directory: bool,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub missing: bool,
    #[serde(default)]
    pub has_categories: bool,
    #[serde(default)]
    pub has_keywords: bool,
    #[serde(default)]
    pub display_categories: Option<String>,
    #[serde(default)]
    pub display_keywords: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub quarantined_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub edit_pending_since: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct AssetPage {
    pub items: Vec<Asset>,
    pub total: i64,
    pub total_size: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct AssetMetadata {
    pub asset: Asset,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

fn map_asset(row: &rusqlite::Row<'_>, include_categories: bool, include_keywords: bool) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        name: row.get(2)?,
        ext: row.get(3)?,
        size: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        bpp: row.get(7)?,
        caption: row.get(8)?,
        notes: row.get(9)?,
        author: row.get(10)?,
        rating: row.get(11)?,
        tagged: row.get(12)?,
        label: row.get(13)?,
        file_modified: row.get(14)?,
        exif_date: row.get(15)?,
        acdsee_date: row.get(16)?,
        md_dirty: row.get(17)?,
        is_orphan: row.get(18)?,
        path: row.get(19)?,
        is_directory: false,
        offline: false,
        missing: false,
        has_categories: row.get::<_, Option<i64>>(20)?.unwrap_or(0) != 0,
        has_keywords: row.get::<_, i64>(21)? != 0,
        display_categories: if include_categories { row.get(22)? } else { None },
        display_keywords: if include_keywords {
            row.get(if include_categories { 23 } else { 22 })?
        } else {
            None
        },
        quarantined_at: None,
        edit_pending_since: row.get("edit_pending_since")?,
    })
}

pub fn folder_disk_context(db: &Db, ids: &[i64]) -> rusqlite::Result<Vec<(i64, String, Vec<Asset>)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.lock().unwrap();
    let cols = asset_cols(&conn, false, false)?;
    let mut result = Vec::new();
    for id in ids {
        let path: Option<String> = conn
            .query_row("SELECT path FROM folders WHERE id=?", [id], |r| r.get(0))
            .optional()?
            .flatten();
        let Some(path) = path else { continue };
        let sql = format!("SELECT {cols} FROM assets a WHERE a.folder_id=? AND a.is_listed=1");
        let mut stmt = conn.prepare(&sql)?;
        let assets = stmt
            .query_map([id], |row| map_asset(row, false, false))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.push((*id, path, assets));
    }
    Ok(result)
}

pub fn assets_page(db: &Db, filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<AssetPage> {
    if filter.kind == "ai" || filter.kind == "series" || filter.kind == "people" {
        return ai_assets_page(db, filter, formats);
    }
    let conn = db.lock().unwrap();
    let (ids, total_size) = catalog_ordered_ids(db, &conn, filter, formats)?;
    let total = ids.len() as i64;
    let start = (filter.offset.max(0) as u64).min(ids.len() as u64) as usize;
    let end = start.saturating_add(filter.limit.clamp(1, 500) as usize).min(ids.len());
    let items = hydrate_asset_ids(&conn, &ids[start..end], filter)?;
    Ok(AssetPage {
        items,
        total,
        total_size,
    })
}

pub(crate) fn catalog_ordered_ids(
    db: &Db,
    conn: &Connection,
    filter: &AssetFilter,
    formats: &FormatTable,
) -> rusqlite::Result<(Arc<Vec<i64>>, f64)> {
    if filter.kind == "ai" {
        return collect_ordered_ids(conn, ai_asset_filter(filter, formats)?, "r.ord");
    }
    let CatalogAssetFilter {
        with_sql,
        from_sql,
        where_sql,
        args,
    } = catalog_asset_filter(&conn, filter, formats)?;
    let order = catalog_order(conn, filter)?;
    let cacheable = conn.is_autocommit()
        && !matches!(filter.kind.as_str(), "auto" | "color" | "quality")
        && !where_sql.contains("ai.")
        && !where_sql.contains("auto_exif.");
    let key = (
        Arc::as_ptr(db) as usize,
        with_sql.clone(),
        from_sql.clone(),
        where_sql.clone(),
        order.to_string(),
        args.clone(),
    );
    type CacheKey = (usize, String, String, String, String, Vec<rusqlite::types::Value>);
    struct Entry {
        db: std::sync::Weak<Mutex<Connection>>,
        key: CacheKey,
        at: std::time::Instant,
        changes: u64,
        version: i64,
        ids: Arc<Vec<i64>>,
        total_size: f64,
    }
    static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    // total_changes catches writes on this connection; data_version catches commits by others.
    // Transactions and attached-database filters are not cached; the weak pointer prevents address reuse.
    let changes = conn.total_changes();
    let version: i64 = conn.query_row("PRAGMA main.data_version", [], |r| r.get(0))?;
    let cached = if cacheable {
        let mut cache = CACHE.lock().unwrap();
        let pos = cache.iter().position(|entry| {
            entry.key == key
                && entry.db.upgrade().is_some_and(|owner| Arc::ptr_eq(&owner, db))
                && entry.at.elapsed().as_secs() < 60
                && entry.changes == changes
                && entry.version == version
        });
        pos.map(|pos| {
            let entry = cache.remove(pos);
            let result = (entry.ids.clone(), entry.total_size);
            cache.push(entry);
            result
        })
    } else {
        None
    };
    let (ids, total_size) = match cached {
        Some(result) => result,
        None => {
            let (ids, total_size) = collect_ordered_ids(
                conn,
                CatalogAssetFilter {
                    with_sql,
                    from_sql,
                    where_sql,
                    args,
                },
                &order,
            )?;
            if cacheable {
                let mut cache = CACHE.lock().unwrap();
                cache.retain(|entry| {
                    entry.key != key && entry.db.strong_count() > 0 && entry.at.elapsed().as_secs() < 60
                });
                if cache.len() >= 2 {
                    cache.remove(0);
                }
                cache.push(Entry {
                    db: Arc::downgrade(db),
                    key,
                    at: std::time::Instant::now(),
                    changes,
                    version,
                    ids: ids.clone(),
                    total_size,
                });
            }
            (ids, total_size)
        }
    };
    Ok((ids, total_size))
}

fn catalog_order(conn: &Connection, filter: &AssetFilter) -> rusqlite::Result<String> {
    if filter.kind == "special"
        && filter.value.as_deref() == Some("edit_pending")
        && crate::schema::has_asset_column(conn, "edit_pending_since")?
    {
        return Ok("a.edit_pending_since ASC,a.id ASC".into());
    }
    Ok(order_clause(&filter.sort, filter.descending))
}

pub(crate) fn locate_asset(
    db: &Db,
    filter: &AssetFilter,
    formats: &FormatTable,
    id: i64,
) -> rusqlite::Result<Option<usize>> {
    let conn = db.lock().unwrap();
    let (ids, _) = catalog_ordered_ids(db, &conn, filter, formats)?;
    Ok(ids.iter().position(|&asset_id| asset_id == id))
}

fn collect_ordered_ids(
    conn: &Connection,
    filter: CatalogAssetFilter,
    order: &str,
) -> rusqlite::Result<(Arc<Vec<i64>>, f64)> {
    let CatalogAssetFilter {
        with_sql,
        from_sql,
        where_sql,
        args,
    } = filter;
    let sql = format!("{with_sql} SELECT a.id,a.size FROM {from_sql} WHERE {where_sql} ORDER BY {order}");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut ids = Vec::new();
    let mut total_size = 0.0;
    while let Some(row) = rows.next()? {
        ids.push(row.get(0)?);
        total_size += row.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
    }
    Ok((Arc::new(ids), total_size))
}

pub(crate) fn hydrate_asset_ids(
    conn: &Connection,
    page_ids: &[i64],
    filter: &AssetFilter,
) -> rusqlite::Result<Vec<Asset>> {
    let mut selected_cols = asset_cols(conn, filter.include_categories, filter.include_keywords)?;
    let quarantine = filter.kind == "quarantine_review"
        || (filter.kind == "special" && filter.value.as_deref() == Some("quarantine"));
    let quarantine = quarantine && crate::schema::has_asset_column(conn, "quarantined_at")?;
    if quarantine {
        selected_cols.push_str(",a.quarantined_at");
    }
    let mut items = Vec::with_capacity(page_ids.len());
    if !page_ids.is_empty() {
        let sql = format!(
            "SELECT {selected_cols} FROM assets a WHERE a.id IN ({})",
            vec!["?"; page_ids.len()].join(",")
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(page_ids.iter()), |r| {
            let mut asset = map_asset(r, filter.include_categories, filter.include_keywords)?;
            if quarantine {
                asset.quarantined_at = r.get("quarantined_at")?;
            }
            Ok(asset)
        })?;
        let mut by_id = rows
            .map(|row| row.map(|asset| (asset.id, asset)))
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        for id in page_ids {
            if let Some(asset) = by_id.remove(id) {
                items.push(asset);
            }
        }
    }
    Ok(items)
}

fn ai_asset_filter(filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<CatalogAssetFilter> {
    let mut seen = HashSet::new();
    let cap = if filter.kind == "series" { 20_000 } else { 50_000 };
    let ids = filter
        .ids
        .as_deref()
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|id| *id > 0 && seen.insert(*id))
        .take(cap)
        .collect::<Vec<_>>();
    let with_sql =
        "WITH requested(id,ord) AS MATERIALIZED (SELECT CAST(value AS INTEGER),CAST(key AS INTEGER) FROM json_each(?))";
    let from_sql = "requested r CROSS JOIN assets a ON a.id=r.id";
    let mut where_sql = format!(
        "a.id>0 AND a.is_listed=1 AND {}",
        formats.sql_not_in("a.ext_lower", &formats.hidden_extensions())
    );
    let mut args = vec![
        serde_json::to_string(&ids)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
            .into(),
    ];
    if let Some(q) = filter.query.as_ref().filter(|q| !q.trim().is_empty()) {
        where_sql.push_str(" AND (a.name LIKE ? ESCAPE '\\' OR coalesce(a.caption,'') LIKE ? ESCAPE '\\' OR coalesce(a.notes,'') LIKE ? ESCAPE '\\')");
        let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        args.extend([
            format!("%{escaped}%").into(),
            format!("%{escaped}%").into(),
            format!("%{escaped}%").into(),
        ]);
    }
    if filter.date_no_date || filter.date_from.is_some() || filter.date_to.is_some() {
        add_date_filter(&mut where_sql, &mut args, filter, formats);
    }
    add_view_filters(&mut where_sql, &mut args, filter, formats);
    Ok(CatalogAssetFilter {
        with_sql: with_sql.into(),
        from_sql: from_sql.into(),
        where_sql,
        args,
    })
}

fn ai_assets_page(db: &Db, filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<AssetPage> {
    let CatalogAssetFilter {
        with_sql,
        from_sql,
        where_sql,
        mut args,
    } = ai_asset_filter(filter, formats)?;
    let conn = db.lock().unwrap();
    let count_sql = format!("{with_sql} SELECT count(*),coalesce(sum(a.size),0) FROM {from_sql} WHERE {where_sql}");
    let (total, total_size): (i64, f64) =
        conn.query_row(&count_sql, rusqlite::params_from_iter(args.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let selected_cols = asset_cols(&conn, filter.include_categories, filter.include_keywords)?;
    let sql =
        format!("{with_sql} SELECT {selected_cols} FROM {from_sql} WHERE {where_sql} ORDER BY r.ord LIMIT ? OFFSET ?");
    args.push(filter.limit.clamp(1, 500).into());
    args.push(filter.offset.max(0).into());
    let mut statement = conn.prepare(&sql)?;
    let items = statement
        .query_map(rusqlite::params_from_iter(args.iter()), |row| {
            map_asset(row, filter.include_categories, filter.include_keywords)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AssetPage {
        items,
        total,
        total_size,
    })
}

pub fn attach_display_values(
    db: &Db,
    assets: &mut [Asset],
    include_categories: bool,
    include_keywords: bool,
) -> rusqlite::Result<()> {
    if assets.is_empty() || (!include_categories && !include_keywords) {
        return Ok(());
    }
    let conn = db.lock().unwrap();
    let ids = assets
        .iter()
        .filter(|a| !a.is_directory && a.id > 0)
        .map(|a| a.id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let args = rusqlite::params_from_iter(ids.iter());
    if include_categories {
        let sql = format!(
            "SELECT asset_id,group_concat(name, ', ') FROM (SELECT ac.asset_id,c.name AS name FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id IN ({placeholders}) GROUP BY ac.asset_id,c.name ORDER BY ac.asset_id,c.name COLLATE NOCASE) GROUP BY asset_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(args, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, value) = row?;
            if let Some(asset) = assets.iter_mut().find(|a| a.id == id) {
                asset.display_categories = Some(value);
            }
        }
    }
    if include_keywords {
        let sql = format!(
            "SELECT asset_id,group_concat(name, ', ') FROM (SELECT ak.asset_id,k.name AS name FROM asset_keywords ak JOIN keywords k ON k.id=ak.keyword_id WHERE ak.asset_id IN ({placeholders}) GROUP BY ak.asset_id,k.name ORDER BY ak.asset_id,k.name COLLATE NOCASE) GROUP BY asset_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, value) = row?;
            if let Some(asset) = assets.iter_mut().find(|a| a.id == id) {
                asset.display_keywords = Some(value);
            }
        }
    }
    Ok(())
}

pub fn asset_ids_page(
    db: &Db,
    filter: &AssetFilter,
    formats: &FormatTable,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<i64>> {
    if filter.kind == "ai" || filter.kind == "series" || filter.kind == "people" {
        let mut page_filter = filter.clone();
        page_filter.offset = offset;
        page_filter.limit = limit.clamp(1, 500);
        return Ok(ai_assets_page(db, &page_filter, formats)?
            .items
            .into_iter()
            .map(|asset| asset.id)
            .collect());
    }
    let conn = db.lock().unwrap();
    let CatalogAssetFilter {
        with_sql,
        from_sql,
        where_sql,
        mut args,
    } = catalog_asset_filter(&conn, filter, formats)?;
    let order = catalog_order(&conn, filter)?;
    let sql = format!("{with_sql} SELECT a.id FROM {from_sql} WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?");
    args.push(limit.clamp(1, 500).into());
    args.push(offset.max(0).into());
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| r.get(0))?
        .collect()
}

pub fn asset_metadata(db: &Db, id: i64) -> rusqlite::Result<Option<AssetMetadata>> {
    let conn = db.lock().unwrap();
    let cols = asset_cols(&conn, false, false)?;
    let asset = conn
        .query_row(&format!("SELECT {cols} FROM assets a WHERE a.id=?"), [id], |row| {
            map_asset(row, false, false)
        })
        .optional()?;
    let Some(asset) = asset else { return Ok(None) };
    let mut cs=conn.prepare("SELECT c.name FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id=? ORDER BY c.name COLLATE NOCASE,c.id")?;
    let categories = cs
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let mut ks=conn.prepare("SELECT k.name FROM asset_keywords ak JOIN keywords k ON k.id=ak.keyword_id WHERE ak.asset_id=? ORDER BY k.name COLLATE NOCASE,k.id")?;
    let keywords = ks
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(Some(AssetMetadata {
        asset,
        categories,
        keywords,
    }))
}
