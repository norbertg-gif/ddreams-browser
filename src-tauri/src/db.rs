use crate::formats::FormatTable;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub type Db = Arc<Mutex<Connection>>;

pub fn catalog_path() -> PathBuf {
    if let Some(path) = std::env::var_os("DDREAMS_CATALOG") {
        return PathBuf::from(path);
    }
    let here = std::env::current_dir().unwrap_or_default();
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .chain(here.ancestors())
        .map(|dir| dir.join(".local").join("catalog.sqlite"))
        .find(|path| path.exists())
        .unwrap_or_else(|| here.join(".local").join("catalog.sqlite"))
}

pub fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(4))?;
    Ok(conn)
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
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
    pub has_categories: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPage {
    pub items: Vec<Asset>,
    pub total: i64,
    pub total_size: f64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub root_id: Option<i64>,
    pub name: String,
    pub path: Option<String>,
    pub count: i64,
    #[serde(default)]
    pub folder_ids: Vec<i64>,
    #[serde(default)]
    pub virtual_node: Option<String>,
    #[serde(default)]
    pub offline: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub root_id: Option<i64>,
    pub name: String,
    pub count: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueCount {
    pub value: String,
    pub count: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetMetadata {
    pub asset: Asset,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub id: i64,
    pub name: String,
}

fn map_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
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
        has_categories: row.get::<_, Option<i64>>(20)?.unwrap_or(0) != 0,
    })
}

const ASSET_COLS: &str = "a.id,a.folder_id,a.name,a.ext,a.size,a.width,a.height,a.bpp,a.caption,a.notes,a.author,a.rating,a.tagged,a.label,a.file_modified,a.exif_date,a.acdsee_date,a.md_dirty,a.is_orphan,a.path,EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id)";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetFilter {
    pub kind: String,
    pub id: Option<i64>,
    #[serde(default)]
    pub folder_path: Option<String>,
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    pub value: Option<String>,
    pub query: Option<String>,
    pub sort: String,
    pub descending: bool,
    pub offset: i64,
    pub limit: i64,
    #[serde(default)] pub rating_mode: Option<String>,
    #[serde(default)] pub rating_value: Option<i64>,
    #[serde(default)] pub label_filter: Option<String>,
    #[serde(default)] pub tagged_only: bool,
    #[serde(default)] pub file_group: Option<String>,
}

fn add_view_filters(where_sql: &mut String, args: &mut Vec<rusqlite::types::Value>, filter: &AssetFilter, formats: &FormatTable) {
    if filter.rating_mode.as_deref()==Some("unrated") { where_sql.push_str(" AND coalesce(a.rating,0)=0"); }
    else if let (Some(mode), Some(value)) = (filter.rating_mode.as_deref(), filter.rating_value) {
        match mode {
            "equal" => { where_sql.push_str(" AND coalesce(a.rating,0)=?"); args.push(value.into()); }
            "atLeast" => { where_sql.push_str(" AND a.rating>=?"); args.push(value.into()); }
            "atMost" => { where_sql.push_str(" AND a.rating<=?"); args.push(value.into()); }
            "unrated" => where_sql.push_str(" AND coalesce(a.rating,0)=0"),
            _ => {}
        }
    }
    if let Some(label) = filter.label_filter.as_ref() { where_sql.push_str(" AND coalesce(a.label,'')=?"); args.push(label.clone().into()); }
    if filter.tagged_only { where_sql.push_str(" AND coalesce(a.tagged,0)<>0"); }
    match filter.file_group.as_deref() {
        Some("images") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
        Some("raw") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["raw"]))),
        Some("video") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["video"]))),
        Some("other") => where_sql.push_str(&format!(" AND {}", formats.sql_not_in("a.ext_lower", &["image", "raw", "video"]))),
        _ => {}
    }
}

fn order_clause(sort: &str, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let (expr, empty) = match sort {
        "date" | "dateTaken" => ("coalesce(a.exif_date,a.file_modified)", "coalesce(a.exif_date,a.file_modified) IS NULL OR coalesce(a.exif_date,a.file_modified)=''"),
        "modified" => ("a.file_modified", "a.file_modified IS NULL OR a.file_modified=''"),
        "size" => ("a.size", "a.size IS NULL"), "type" => ("a.ext_lower", "a.ext_lower IS NULL OR a.ext_lower=''"),
        "dimensions" => ("coalesce(a.width,0)*coalesce(a.height,0)", "a.width IS NULL OR a.height IS NULL"),
        "rating" => ("a.rating", "a.rating IS NULL OR a.rating=0"), "label" => ("a.label", "a.label IS NULL OR a.label=''"),
        "tagged" => ("a.tagged", "a.tagged IS NULL"), "caption" => ("a.caption", "a.caption IS NULL OR a.caption=''"),
        _ => ("a.name COLLATE NOCASE", "a.name IS NULL OR a.name=''"),
    };
    format!("({empty}) ASC, {expr} {direction}, a.name COLLATE NOCASE ASC, a.id ASC")
}

pub fn asset_filter_matches(a: &Asset, f: &AssetFilter, formats: &FormatTable) -> bool {
    if f.rating_mode.as_deref()==Some("unrated") && a.rating.unwrap_or(0)!=0 { return false; }
    if let (Some(mode), Some(value)) = (f.rating_mode.as_deref(), f.rating_value) {
        let rating = a.rating.unwrap_or(0);
        let ok = match mode { "equal" => rating == value, "atLeast" => a.rating.is_some_and(|r| r >= value), "atMost" => a.rating.is_some_and(|r| r <= value), "unrated" => rating == 0, _ => true };
        if !ok { return false; }
    }
    if f.label_filter.as_ref().is_some_and(|v| a.label.as_deref().unwrap_or("") != v) || (f.tagged_only && a.tagged.unwrap_or(0) == 0) { return false; }
    let group=formats.group(&a.ext);
    match f.file_group.as_deref() {
        Some("images") if !matches!(group,Some("image"|"raw")) => false,
        Some("raw") if group!=Some("raw") => false,
        Some("video") if group!=Some("video") => false,
        Some("other") if matches!(group,Some("image"|"raw"|"video")) => false,
        _ => true
    }
}

pub fn compare_assets(a: &Asset, b: &Asset, sort: &str, descending: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let empty_a; let empty_b;
    let (cmp, ea, eb) = match sort {
        "date" | "dateTaken" => { let av=a.exif_date.as_ref().or(a.file_modified.as_ref()); let bv=b.exif_date.as_ref().or(b.file_modified.as_ref()); (av.cmp(&bv), av.is_none(), bv.is_none()) },
        "modified" => (a.file_modified.cmp(&b.file_modified), a.file_modified.is_none(), b.file_modified.is_none()),
        "size" => (a.size.partial_cmp(&b.size).unwrap_or(Ordering::Equal), a.size.is_none(), b.size.is_none()),
        "type" => (a.ext.to_lowercase().cmp(&b.ext.to_lowercase()), a.ext.is_empty(), b.ext.is_empty()),
        "dimensions" => { let av=a.width.zip(a.height).map(|(w,h)|w.saturating_mul(h));let bv=b.width.zip(b.height).map(|(w,h)|w.saturating_mul(h));(av.cmp(&bv),av.is_none(),bv.is_none()) },
        "rating" => (a.rating.cmp(&b.rating), a.rating.is_none_or(|v|v==0), b.rating.is_none_or(|v|v==0)),
        "label" => (a.label.as_ref().map(|s|s.to_lowercase()).cmp(&b.label.as_ref().map(|s|s.to_lowercase())),a.label.as_deref().is_none_or(str::is_empty),b.label.as_deref().is_none_or(str::is_empty)),
        "tagged" => (a.tagged.cmp(&b.tagged),a.tagged.is_none(),b.tagged.is_none()),
        "caption" => (a.caption.as_ref().map(|s|s.to_lowercase()).cmp(&b.caption.as_ref().map(|s|s.to_lowercase())),a.caption.as_deref().is_none_or(str::is_empty),b.caption.as_deref().is_none_or(str::is_empty)),
        _ => (a.name.to_lowercase().cmp(&b.name.to_lowercase()),a.name.is_empty(),b.name.is_empty()),
    };
    empty_a=ea; empty_b=eb;
    if empty_a != empty_b { return empty_a.cmp(&empty_b); }
    let directed=if descending {cmp.reverse()} else {cmp};
    directed.then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id))
}

pub fn folder_disk_context(db: &Db, ids: &[i64]) -> rusqlite::Result<Vec<(i64, String, Vec<Asset>)>> {
    let conn = db.lock().unwrap();
    let mut result = Vec::new();
    for id in ids {
        let path: Option<String> = conn.query_row("SELECT path FROM folders WHERE id=?", [id], |r| r.get(0)).optional()?.flatten();
        let Some(path) = path else { continue };
        let sql = format!("SELECT {ASSET_COLS} FROM assets a WHERE a.folder_id=? AND a.is_listed=1");
        let mut stmt = conn.prepare(&sql)?;
        let assets = stmt.query_map([id], map_asset)?.collect::<rusqlite::Result<Vec<_>>>()?;
        result.push((*id, path, assets));
    }
    Ok(result)
}

pub fn assets_page(db: &Db, filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<AssetPage> {
    let conn = db.lock().unwrap();
    let mut from_sql = String::from("assets a");
    let mut where_sql = format!("a.is_listed=1 AND {}", formats.sql_not_in("a.ext_lower", &formats.hidden_extensions()));
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    match filter.kind.as_str() {
        "folder" => {
            if let Some(ids) = filter.ids.as_ref().filter(|ids| !ids.is_empty()) {
                where_sql.push_str(&format!(" AND a.folder_id IN ({})", vec!["?"; ids.len()].join(",")));
                args.extend(ids.iter().copied().map(Into::into));
            } else { where_sql.push_str(" AND a.folder_id=?"); args.push(filter.id.unwrap_or(-1).into()); }
        }
        "category" => {
            from_sql = String::from("asset_categories ac INDEXED BY idx_asset_categories_category_asset JOIN assets a ON a.id=ac.asset_id");
            where_sql.push_str(" AND ac.category_id=?");
            args.push(filter.id.unwrap_or(-1).into());
        }
        "keyword" => {
            from_sql = String::from("asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset JOIN assets a ON a.id=ak.asset_id");
            where_sql.push_str(" AND ak.keyword_id=?");
            args.push(filter.id.unwrap_or(-1).into());
        }
        "rating" => {
            if filter.value.as_deref() == Some("unrated") {
                where_sql.push_str(" AND coalesce(a.rating,0)=0");
            } else {
                where_sql.push_str(" AND a.rating=?");
                args.push(
                    filter
                        .value
                        .as_deref()
                        .unwrap_or("0")
                        .parse::<i64>()
                        .unwrap_or(0)
                        .into(),
                );
            }
        }
        "label" => {
            where_sql.push_str(" AND coalesce(a.label,'')=?");
            args.push(filter.value.clone().unwrap_or_default().into());
        }
        "special" => match filter.value.as_deref().unwrap_or("") {
            "all_photos" => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "uncategorized" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "unkeyworded" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_keywords akx WHERE akx.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "tagged" => where_sql.push_str(&format!(" AND {} AND coalesce(a.tagged,0)<>0", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            _ => where_sql.push_str(" AND 0"),
        },
        _ => {
            where_sql.push_str(" AND 0");
        }
    }
    if let Some(q) = filter.query.as_ref().filter(|q| !q.trim().is_empty()) {
        where_sql.push_str(
            " AND (a.name LIKE ? ESCAPE '\\' OR coalesce(a.caption,'') LIKE ? ESCAPE '\\' OR coalesce(a.notes,'') LIKE ? ESCAPE '\\')",
        );
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        args.push(format!("%{escaped}%").into());
        args.push(format!("%{escaped}%").into());
        args.push(format!("%{escaped}%").into());
    }
    add_view_filters(&mut where_sql, &mut args, filter, formats);
    let order = order_clause(&filter.sort, filter.descending);
    let count_sql =
        format!("SELECT count(*),coalesce(sum(a.size),0) FROM {from_sql} WHERE {where_sql}");
    let (total, total_size): (i64, f64) =
        conn.query_row(&count_sql, rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let sql = format!(
        "SELECT {ASSET_COLS} FROM {from_sql} WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    args.push(filter.limit.clamp(1, 500).into());
    args.push(filter.offset.max(0).into());
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), map_asset)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AssetPage {
        items,
        total,
        total_size,
    })
}

pub fn asset_ids_page(
    db: &Db,
    filter: &AssetFilter,
    formats: &FormatTable,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<i64>> {
    let conn = db.lock().unwrap();
    let mut where_sql = format!("a.is_listed=1 AND {}", formats.sql_not_in("a.ext_lower", &formats.hidden_extensions()));
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    match filter.kind.as_str() {
        "folder" => {
            if let Some(ids) = filter.ids.as_ref().filter(|ids| !ids.is_empty()) {
                where_sql.push_str(&format!(" AND a.folder_id IN ({})", vec!["?"; ids.len()].join(",")));
                args.extend(ids.iter().copied().map(Into::into));
            } else { where_sql.push_str(" AND a.folder_id=?"); args.push(filter.id.unwrap_or(-1).into()); }
        }
        "category" => {
            where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_categories ac WHERE ac.asset_id=a.id AND ac.category_id=?)");
            args.push(filter.id.unwrap_or(-1).into())
        }
        "keyword" => {
            where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_keywords ak WHERE ak.asset_id=a.id AND ak.keyword_id=?)");
            args.push(filter.id.unwrap_or(-1).into())
        }
        "rating" => {
            if filter.value.as_deref() == Some("unrated") {
                where_sql.push_str(" AND coalesce(a.rating,0)=0")
            } else {
                where_sql.push_str(" AND a.rating=?");
                args.push(
                    filter
                        .value
                        .as_deref()
                        .unwrap_or("0")
                        .parse::<i64>()
                        .unwrap_or(0)
                        .into(),
                )
            }
        }
        "label" => {
            where_sql.push_str(" AND coalesce(a.label,'')=?");
            args.push(filter.value.clone().unwrap_or_default().into())
        }
        "special" => match filter.value.as_deref().unwrap_or("") {
            "all_photos" => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "uncategorized" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_categories ac WHERE ac.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "unkeyworded" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_keywords ak WHERE ak.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "tagged" => where_sql.push_str(&format!(" AND {} AND coalesce(a.tagged,0)<>0", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            _ => where_sql.push_str(" AND 0"),
        },
        _ => where_sql.push_str(" AND 0"),
    }
    add_view_filters(&mut where_sql, &mut args, filter, formats);
    if let Some(q) = filter.query.as_ref().filter(|q| !q.trim().is_empty()) {
        where_sql.push_str(
            " AND (a.name LIKE ? ESCAPE '\\' OR coalesce(a.caption,'') LIKE ? ESCAPE '\\' OR coalesce(a.notes,'') LIKE ? ESCAPE '\\')",
        );
        let e = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        args.push(format!("%{e}%").into());
        args.push(format!("%{e}%").into());
        args.push(format!("%{e}%").into());
    }
    let sql = format!("SELECT a.id FROM assets a WHERE {where_sql} ORDER BY {} LIMIT ? OFFSET ?", order_clause(&filter.sort, filter.descending));
    args.push(limit.clamp(1, 500).into());
    args.push(offset.max(0).into());
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| r.get(0))?
        .collect()
}

pub fn folders_page(
    db: &Db,
    parent_id: Option<i64>,
    parent_ids: Option<Vec<i64>>,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<Folder>> {
    let conn = db.lock().unwrap();
    if parent_id.is_none() && parent_ids.is_none() {
        let mut stmt=conn.prepare("WITH RECURSIVE d(root_folder_id,id) AS (SELECT id,id FROM folders WHERE parent_id IS NULL UNION ALL SELECT d.root_folder_id,f.id FROM folders f JOIN d ON f.parent_id=d.id), counts AS (SELECT d.root_folder_id,count(a.id) n FROM d JOIN assets a ON a.folder_id=d.id AND a.is_listed=1 GROUP BY d.root_folder_id) SELECT f.id,f.root_id,coalesce(nullif(r.name,''),r.path),r.path,coalesce(counts.n,0) FROM folders f JOIN folder_roots r ON r.id=f.root_id LEFT JOIN counts ON counts.root_folder_id=f.id WHERE f.parent_id IS NULL ORDER BY r.name COLLATE NOCASE,f.id")?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,i64>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut grouped: std::collections::BTreeMap<String,Vec<(i64,i64,Option<String>,i64)>>=std::collections::BTreeMap::new();
        for (id,root,name,path,count) in rows { if count>0 { let label=if name=="\\" || name=="/" { path.as_deref().unwrap_or(&name).trim_end_matches(['\\','/']).to_string() } else { name }; let key=label.to_lowercase(); grouped.entry(key).or_default().push((id,root,Some(label),count)); } }
        let mut result=Vec::new(); let mut devices=Vec::new();
        for (key,items) in grouped { let name=items.first().and_then(|i|i.2.clone()).unwrap_or(key); let drive=name.len()==2 && name.ends_with(':'); let ids=items.iter().map(|i|i.0).collect::<Vec<_>>(); let roots=items.iter().map(|i|i.1).collect::<Vec<_>>(); let count=items.iter().map(|i|i.3).sum(); let path=conn.query_row("SELECT path FROM folders WHERE id=?",[ids[0]],|r|r.get::<_,Option<String>>(0)).ok().flatten(); let node=Folder{id:ids[0],parent_id:None,root_id:roots.first().copied(),name,path,count,folder_ids:ids,virtual_node:None,offline:false}; if drive {result.push(node)} else {devices.push(node)} }
        result.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        if !devices.is_empty() { result.push(Folder{id:-2,parent_id:None,root_id:None,name:"Zariadenia".into(),path:None,count:devices.iter().map(|d|d.count).sum(),folder_ids:devices.iter().flat_map(|d|d.folder_ids.clone()).collect(),virtual_node:Some("devices".into()),offline:false}); }
        return Ok(result.into_iter().skip(offset.max(0) as usize).take(limit.clamp(1,500) as usize).collect());
    }
    let parents=parent_ids.filter(|v|!v.is_empty()).unwrap_or_else(||vec![parent_id.unwrap_or(0)]);
    let placeholders=vec!["?";parents.len()].join(",");
    let sql=format!("WITH RECURSIVE candidate AS (SELECT f.id,f.parent_id,f.root_id,f.name,f.path FROM folders f WHERE f.parent_id IN ({placeholders})), d(folder_id,id) AS (SELECT c.id,c.id FROM candidate c UNION ALL SELECT d.folder_id,f.id FROM folders f JOIN d ON f.parent_id=d.id), counts AS (SELECT d.folder_id,count(a.id) n FROM d JOIN assets a ON a.folder_id=d.id AND a.is_listed=1 GROUP BY d.folder_id) SELECT c.id,c.parent_id,c.root_id,c.name,c.path,coalesce(n,0) FROM candidate c LEFT JOIN counts ON counts.folder_id=c.id WHERE coalesce(n,0)>0 ORDER BY c.name COLLATE NOCASE,c.id LIMIT ? OFFSET ?");
    let mut args:Vec<rusqlite::types::Value>=parents.into_iter().map(Into::into).collect(); args.push(limit.clamp(1,500).into()); args.push(offset.max(0).into());
    let mut stmt=conn.prepare(&sql)?;
    let rows=stmt.query_map(rusqlite::params_from_iter(args),|r|{let id=r.get(0)?; Ok(Folder{id, parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,path:r.get(4)?,count:r.get(5)?,folder_ids:vec![id],virtual_node:None,offline:false})})?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut merged:std::collections::BTreeMap<String,Folder>=std::collections::BTreeMap::new();
    for folder in rows { let key=folder.name.to_lowercase(); if let Some(existing)=merged.get_mut(&key) {existing.count+=folder.count;existing.folder_ids.push(folder.id);} else {merged.insert(key,folder);} }
    Ok(merged.into_values().skip(offset.max(0) as usize).take(limit.clamp(1,500) as usize).collect())
}

pub fn folder_by_path(db: &Db, path: &str) -> rusqlite::Result<Option<Folder>> {
    let conn = db.lock().unwrap();
    conn.query_row("SELECT id,parent_id,root_id,name,path,(SELECT count(*) FROM assets WHERE folder_id=folders.id AND is_listed=1) FROM folders WHERE lower(rtrim(replace(path,'/','\\'),'\\'))=lower(rtrim(replace(?,'/','\\'),'\\')) LIMIT 1", [path], |r| Ok(Folder { id:r.get(0)?,parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,path:r.get(4)?,count:r.get(5)?,folder_ids:vec![r.get(0)?],virtual_node:None,offline:false })).optional()
}

pub fn folder_ancestors(db: &Db, id: i64) -> rusqlite::Result<Vec<Folder>> {
    let conn = db.lock().unwrap();
    let mut stmt=conn.prepare("WITH RECURSIVE chain(id,parent_id,root_id,name,path,depth) AS (SELECT id,parent_id,root_id,name,path,0 FROM folders WHERE id=? UNION ALL SELECT f.id,f.parent_id,f.root_id,f.name,f.path,c.depth+1 FROM folders f JOIN chain c ON f.id=c.parent_id) SELECT c.id,c.parent_id,c.root_id,c.name,c.path,(SELECT count(*) FROM assets a WHERE a.folder_id=c.id AND a.is_listed=1) FROM chain c ORDER BY c.depth DESC")?;
    let mut rows=stmt.query([id])?; let mut result=Vec::new();
    while let Some(r)=rows.next()? { let folder_id=r.get(0)?; result.push(Folder{id:folder_id,parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,path:r.get(4)?,count:r.get(5)?,folder_ids:vec![folder_id],virtual_node:None,offline:false}); }
    Ok(result)
}

pub fn roots_page(db: &Db, offset: i64, limit: i64) -> rusqlite::Result<Vec<Root>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id,name FROM category_roots ORDER BY name COLLATE NOCASE,id LIMIT ? OFFSET ?",
    )?;
    stmt.query_map(params![limit.clamp(1, 500), offset.max(0)], |r| {
        Ok(Root {
            id: r.get(0)?,
            name: r.get(1)?,
        })
    })?
    .collect()
}

pub fn nodes_page(
    db: &Db,
    table: &str,
    parent_id: Option<i64>,
    root_id: Option<i64>,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    let (sql, id_col) = if table == "category" {
        (
            "SELECT n.id,n.parent_id,n.root_id,n.name,count(ac.asset_id) FROM categories n LEFT JOIN asset_categories ac ON ac.category_id=n.id WHERE coalesce(n.parent_id,0)=? AND (? IS NULL OR n.root_id=?) GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
            "category_id",
        )
    } else {
        (
            "SELECT n.id,n.parent_id,NULL,n.name,count(ak.asset_id) FROM keywords n LEFT JOIN asset_keywords ak ON ak.keyword_id=n.id WHERE coalesce(n.parent_id,0)=? GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
            "keyword_id",
        )
    };
    let _ = id_col;
    let mut stmt = conn.prepare(sql)?;
    if table == "category" {
        stmt.query_map(
            params![
                parent_id.unwrap_or(0),
                root_id,
                root_id,
                limit.clamp(1, 500),
                offset.max(0)
            ],
            |r| {
                Ok(CatalogNode {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    root_id: r.get(2)?,
                    name: r.get(3)?,
                    count: r.get(4)?,
                })
            },
        )?
        .collect()
    } else {
        stmt.query_map(
            params![parent_id.unwrap_or(0), limit.clamp(1, 500), offset.max(0)],
            |r| {
                Ok(CatalogNode {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    root_id: None,
                    name: r.get(3)?,
                    count: r.get(4)?,
                })
            },
        )?
        .collect()
    }
}

pub fn keyword_search_page(
    db: &Db,
    query: &str,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    let mut stmt=conn.prepare("SELECT k.id,k.parent_id,NULL,k.name,(SELECT count(*) FROM asset_keywords ak WHERE ak.keyword_id=k.id) FROM keywords k WHERE k.name LIKE ? COLLATE NOCASE ORDER BY k.name COLLATE NOCASE,k.id LIMIT ? OFFSET ?")?;
    stmt.query_map(
        params![
            format!("%{}%", query.replace('%', "\\%").replace('_', "\\_")),
            limit.clamp(1, 500),
            offset.max(0)
        ],
        |r| {
            Ok(CatalogNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                root_id: None,
                name: r.get(3)?,
                count: r.get(4)?,
            })
        },
    )?
    .collect()
}

pub fn catalog_tree_search(db:&Db, kind:&str, query:&str, offset:i64, limit:i64)->rusqlite::Result<Vec<CatalogNode>>{
    let conn=db.lock().unwrap();
    let sql=if kind=="category" {"WITH RECURSIVE t(id,parent_id,root_id,name,path) AS (SELECT c.id,c.parent_id,c.root_id,c.name,coalesce((SELECT r.name FROM category_roots r WHERE r.id=c.root_id)||' > ','')||c.name FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0 UNION ALL SELECT n.id,n.parent_id,n.root_id,n.name,t.path||' > '||n.name FROM categories n JOIN t ON n.parent_id=t.id) SELECT t.id,t.parent_id,t.root_id,t.path,(SELECT count(*) FROM asset_categories ac WHERE ac.category_id=t.id) FROM t WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"}else{"WITH RECURSIVE t(id,parent_id,name,path) AS (SELECT id,parent_id,name,name FROM keywords WHERE parent_id IS NULL OR parent_id=0 UNION ALL SELECT n.id,n.parent_id,n.name,t.path||' > '||n.name FROM keywords n JOIN t ON n.parent_id=t.id) SELECT t.id,t.parent_id,NULL,t.path,(SELECT count(*) FROM asset_keywords ak WHERE ak.keyword_id=t.id) FROM t WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"};
    let mut stmt=conn.prepare(sql)?;stmt.query_map(params![format!("%{}%",query.replace('%',"\\%").replace('_',"\\_")),limit.clamp(1,500),offset.max(0)],|r|Ok(CatalogNode{id:r.get(0)?,parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,count:r.get(4)?}))?.collect()
}

pub fn values_page(
    db: &Db,
    kind: &str,
    offset: i64,
    limit: i64,
    formats: &FormatTable,
) -> rusqlite::Result<Vec<ValueCount>> {
    let conn = db.lock().unwrap();
    let sql = match kind {
        "rating" => {
            "SELECT CASE WHEN coalesce(rating,0)=0 THEN 'unrated' ELSE cast(rating as text) END,count(*) FROM assets WHERE is_listed=1 GROUP BY coalesce(rating,0) ORDER BY coalesce(rating,0) LIMIT ? OFFSET ?"
        }
        "label" => {
            "SELECT coalesce(label,''),count(*) FROM assets WHERE is_listed=1 GROUP BY coalesce(label,'') ORDER BY coalesce(label,'') COLLATE NOCASE LIMIT ? OFFSET ?"
        }
        "special" => {
            let photos=formats.sql_in("a.ext_lower", &["image", "raw"]);
            let hidden=formats.sql_not_in("a.ext_lower", &formats.hidden_extensions());
            let sql=format!("SELECT v,count(*) FROM (SELECT 'all_photos' v FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} UNION ALL SELECT 'uncategorized' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND NOT EXISTS(SELECT 1 FROM asset_categories ac WHERE ac.asset_id=a.id) UNION ALL SELECT 'unkeyworded' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND NOT EXISTS(SELECT 1 FROM asset_keywords ak WHERE ak.asset_id=a.id) UNION ALL SELECT 'tagged' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND coalesce(tagged,0)<>0) GROUP BY v ORDER BY v LIMIT ? OFFSET ?");
            let mut stmt=conn.prepare(&sql)?;
            return stmt.query_map(params![limit.clamp(1,500),offset.max(0)],|r|Ok(ValueCount{value:r.get(0)?,count:r.get(1)?}))?.collect();
        }
        _ => return Ok(Vec::new()),
    };
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map(params![limit.clamp(1, 500), offset.max(0)], |r| {
        Ok(ValueCount {
            value: r.get(0)?,
            count: r.get(1)?,
        })
    })?
    .collect()
}

pub fn asset_metadata(db: &Db, id: i64) -> rusqlite::Result<Option<AssetMetadata>> {
    let conn = db.lock().unwrap();
    let asset = conn
        .query_row(
            &format!("SELECT {ASSET_COLS} FROM assets a WHERE a.id=?"),
            [id],
            map_asset,
        )
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

#[cfg(test)]
mod feature_tests {
    use super::*;
    use std::cmp::Ordering;
    fn asset(id:i64,name:&str,size:Option<f64>,rating:Option<i64>)->Asset { Asset{id,folder_id:Some(1),name:name.into(),ext:"jpg".into(),size,width:Some(10),height:Some(10),bpp:None,caption:None,notes:None,author:None,rating,tagged:Some(0),label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:None,is_directory:false,offline:false,has_categories:false} }
    #[test]
    fn filter_logic_supports_modes_and_groups() {
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let unrated = asset(1, "a.jpg", None, None);
        let rated = asset(2, "b.jpg", Some(10.0), Some(4));
        let mut f = AssetFilter { kind:"folder".into(), id:None, folder_path:None, ids:None, value:None, query:None, sort:"name".into(), descending:false, offset:0, limit:100, rating_mode:Some("atLeast".into()), rating_value:Some(3), label_filter:None, tagged_only:false, file_group:Some("images".into()) };
        assert!(asset_filter_matches(&rated, &f, &formats));
        assert!(!asset_filter_matches(&unrated, &f, &formats));
        f.rating_mode = Some("unrated".into());
        assert!(asset_filter_matches(&unrated, &f, &formats));
        assert!(!asset_filter_matches(&rated, &f, &formats));
    }
    #[test] fn sort_keeps_empty_values_last_in_both_directions(){let empty=asset(1,"empty",None,None);let full=asset(2,"full",Some(5.0),Some(4));for desc in [false,true]{assert_eq!(compare_assets(&full,&empty,"size",desc),Ordering::Less);}}
    fn test_db() -> Db {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));").unwrap();
        Arc::new(Mutex::new(c))
    }
    fn test_filter(kind:&str,value:&str)->AssetFilter { AssetFilter{kind:kind.into(),id:None,folder_path:None,ids:None,value:Some(value.into()),query:None,sort:"name".into(),descending:false,offset:0,limit:10,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,file_group:None} }
    #[test] fn special_items_use_listed_photos_and_relationships(){
        let db=test_db();db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,tagged,rating) VALUES(1,'a.jpg','jpg','jpg',1,10,1,4),(2,'b.doc','doc','doc',1,20,0,5),(3,'gone.jpg','jpg','jpg',0,30,1,3),(4,'d.png','png','png',1,40,0,NULL);INSERT INTO asset_categories VALUES(1,9),(2,9);INSERT INTO asset_keywords VALUES(1,8);").unwrap();
        for (kind,count) in [("all_photos",2),("uncategorized",1),("unkeyworded",1),("tagged",1)] { assert_eq!(assets_page(&db,&test_filter("special",kind),&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().total,count,"{kind}"); }
        assert_eq!(values_page(&db,"special",0,10,&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().iter().find(|v|v.value=="unkeyworded").unwrap().count,1);
    }
    #[test] fn rating_query_uses_selected_value_and_excludes_unrated(){
        let db=test_db();db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating) VALUES(1,'one.jpg','jpg','jpg',1,1,1),(2,'three.jpg','jpg','jpg',1,1,3),(3,'five.jpg','jpg','jpg',1,1,5),(4,'none.jpg','jpg','jpg',1,1,NULL);").unwrap();
        for (mode,value,expected) in [("equal",3,1),("atLeast",3,2),("atMost",3,2)] { let mut f=test_filter("special","all_photos");f.rating_mode=Some(mode.into());f.rating_value=Some(value);assert_eq!(assets_page(&db,&f,&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().total,expected,"{mode}"); }
    }
}

#[cfg(test)]
mod benches {
    use super::*;
    use std::time::Instant;
    static BENCH_LOCK: Mutex<()> = Mutex::new(());
    fn db_path() -> PathBuf {
        let p = catalog_path();
        if p.exists() {
            p
        } else {
            PathBuf::from("../.local/catalog.sqlite")
        }
    }
    #[test]
    #[ignore]
    fn bench_open_catalog() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let p = db_path();
        let t = Instant::now();
        let c = open_read_only(&p).unwrap();
        println!("open catalog: {:?}", t.elapsed());
        drop(c);
    }
    #[test]
    #[ignore]
    fn bench_largest_folder_first_page() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let (f, folder_count) = {
            let c = db.lock().unwrap();
            c.query_row("SELECT folder_id,count(*) n FROM assets WHERE is_listed=1 GROUP BY folder_id ORDER BY n DESC LIMIT 1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap()
        };
        let t = Instant::now();
        let formats=FormatTable::load();
        let p = assets_page(
            &db,
            &AssetFilter {
                kind: "folder".into(),
                id: Some(f),
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 0,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, file_group: None,
            }, &formats,
        )
        .unwrap();
        println!(
            "largest folder first page (folder {f}, {} assets, {} rows): {:?}",
            folder_count,
            p.items.len(),
            t.elapsed()
        );
    }
    #[test]
    #[ignore]
    fn bench_deep_offset_page() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let f = {
            let c = db.lock().unwrap();
            c.query_row("SELECT folder_id FROM assets WHERE is_listed=1 GROUP BY folder_id ORDER BY count(*) DESC LIMIT 1",[],|r|r.get::<_,i64>(0)).unwrap()
        };
        let folder_count = {
            let c=db.lock().unwrap();
            c.query_row("SELECT count(*) FROM assets WHERE folder_id=? AND is_listed=1",[f],|r|r.get::<_,i64>(0)).unwrap()
        };
        let t = Instant::now();
        let formats=FormatTable::load();
        let _ = assets_page(
            &db,
            &AssetFilter {
                kind: "folder".into(),
                id: Some(f),
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 3000,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, file_group: None,
            }, &formats,
        )
        .unwrap();
        println!(
            "deep offset page (folder {f}, row count {}, offset 3000): {:?}",
            folder_count,
            t.elapsed()
        );
    }
    #[test]
    #[ignore]
    fn bench_category_tree() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let t = Instant::now();
        let _ = roots_page(&db, 0, 100);
        let _ = nodes_page(&db, "category", None, Some(1), 0, 500).unwrap();
        println!("category tree first level: {:?}", t.elapsed());
    }
    #[test]
    #[ignore]
    fn bench_keyword_tree() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let t = Instant::now();
        let _ = nodes_page(&db, "keyword", None, None, 0, 500).unwrap();
        println!("keyword tree first level: {:?}", t.elapsed());
    }
    #[test]
    #[ignore]
    fn bench_largest_category() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let c = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT ac.category_id,count(*) n FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1 GROUP BY ac.category_id ORDER BY n DESC LIMIT 1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap()
        };
        let t = Instant::now();
        let formats=FormatTable::load();
        let p = assets_page(
            &db,
            &AssetFilter {
                kind: "category".into(),
                id: Some(c.0),
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 0,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, file_group: None,
            }, &formats,
        )
        .unwrap();
        println!(
            "largest category (id {}, {} assets, page {}): {:?}",
            c.0,
            c.1,
            p.items.len(),
            t.elapsed()
        );
    }
    #[test]
    #[ignore]
    fn bench_special_items() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db=Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let formats=FormatTable::load();
        for value in ["all_photos","uncategorized","unkeyworded","tagged"] {
            let t=Instant::now();let p=assets_page(&db,&AssetFilter{kind:"special".into(),id:None,folder_path:None,ids:None,value:Some(value.into()),query:None,sort:"name".into(),descending:false,offset:0,limit:100,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,file_group:None},&formats).unwrap();
            println!("special {value}: total {}, first page {}, {:?}",p.total,p.items.len(),t.elapsed());
        }
    }
}
