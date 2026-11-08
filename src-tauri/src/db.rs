use crate::formats::FormatTable;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, path::Path, sync::{Arc, Mutex}};
#[cfg(test)]
use std::path::PathBuf;

pub type Db = Arc<Mutex<Connection>>;

#[cfg(test)]
pub fn catalog_path() -> PathBuf {
    crate::paths::Paths::resolve().catalog
}

pub fn open_catalog(path: &Path, readonly: bool) -> rusqlite::Result<Connection> {
    if readonly { return open_read_only(path); }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

pub fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(4))?;
    Ok(conn)
}

#[cfg(test)]
mod catalog_open_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn writable_catalog_uses_wal_busy_timeout_and_foreign_keys() {
        let root = std::env::temp_dir().join(format!(
            "ddreams-open-catalog-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("catalog.sqlite");
        Connection::open(&path).unwrap();
        let connection = open_catalog(&path, false).unwrap();
        let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
        let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0)).unwrap();
        let busy_timeout: i64 = connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0)).unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, 5_000);
        drop(connection);
        let _ = std::fs::remove_dir_all(root);
    }
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
    #[serde(default)]
    pub display_categories: Option<String>,
    #[serde(default)]
    pub display_keywords: Option<String>,
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
    #[serde(default)]
    pub has_children: bool,
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
    pub count: i64,
    #[serde(default)]
    pub merged_category_id: Option<i64>,
    #[serde(default)]
    pub has_children: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAssetLinks {
    pub category_ids: Vec<i64>,
    pub category_paths: Vec<CategoryPath>,
    pub category_ancestor_ids: Vec<i64>,
    pub keyword_ids: Vec<i64>,
    pub keyword_paths: Vec<CategoryPath>,
    pub keyword_ancestor_ids: Vec<i64>,
    pub root_ids: Vec<i64>,
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
        has_categories: row.get::<_, Option<i64>>(20)?.unwrap_or(0) != 0,
        display_categories: if include_categories { row.get(21)? } else { None },
        display_keywords: if include_keywords { row.get(if include_categories { 22 } else { 21 })? } else { None },
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CategoryPath { pub id: i64, pub path: String }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeywordOption { pub id: i64, pub name: String, pub path: String }

const ASSET_COLS: &str = "a.id,a.folder_id,a.name,a.ext,a.size,a.width,a.height,a.bpp,a.caption,a.notes,a.author,a.rating,a.tagged,a.label,a.file_modified,a.exif_date,a.acdsee_date,a.md_dirty,a.is_orphan,a.path,EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id)";
fn asset_cols(include_categories: bool, include_keywords: bool) -> String {
    let mut columns = ASSET_COLS.to_owned();
    if include_categories { columns.push_str(", (SELECT group_concat(name, ', ') FROM (SELECT DISTINCT c.name AS name FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id=a.id ORDER BY c.name COLLATE NOCASE))"); }
    if include_keywords { columns.push_str(", (SELECT group_concat(name, ', ') FROM (SELECT DISTINCT k.name AS name FROM asset_keywords ak JOIN keywords k ON k.id=ak.keyword_id WHERE ak.asset_id=a.id ORDER BY k.name COLLATE NOCASE))"); }
    columns
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetFilter {
    pub kind: String,
    pub id: Option<i64>,
    #[serde(default)]
    pub root_id: Option<i64>,
    #[serde(default)]
    pub include_children: bool,
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
    #[serde(default)] pub include_categories: bool,
    #[serde(default)] pub include_keywords: bool,
    #[serde(default)] pub date_from: Option<String>,
    #[serde(default)] pub date_to: Option<String>,
    #[serde(default)] pub date_source: Option<String>,
    #[serde(default)] pub date_no_date: bool,
}

pub fn date_column(source: Option<&str>) -> Option<&'static str> {
    match source.unwrap_or("taken") { "taken" => Some("a.exif_date"), "database" => Some("a.acdsee_date"), "modified" => Some("a.file_modified"), _ => None }
}

pub fn valid_date_sql(column: &str) -> String {
    format!("{column} IS NOT NULL AND length({column})>=10 AND substr({column},1,4) GLOB '[0-9][0-9][0-9][0-9]' AND substr({column},1,4)>='1900' AND substr({column},5,1)='-' AND CAST(substr({column},6,2) AS INTEGER) BETWEEN 1 AND 12 AND substr({column},8,1)='-' AND CAST(substr({column},9,2) AS INTEGER) BETWEEN 1 AND 31")
}

pub fn next_date(date: &str) -> Option<String> {
    let bytes=date.as_bytes();
    if bytes.len()!=10 || bytes[4]!=b'-' || bytes[7]!=b'-' { return None; }
    let year=date[0..4].parse::<i32>().ok()?; let month=date[5..7].parse::<u32>().ok()?; let day=date[8..10].parse::<u32>().ok()?;
    if !(1900..=9998).contains(&year) || !(1..=12).contains(&month) { return None; }
    let leap=year%4==0&&(year%100!=0||year%400==0);
    let days=[31,if leap{29}else{28},31,30,31,30,31,31,30,31,30,31];
    if day==0||day>days[(month-1) as usize] { return None; }
    let (ny,nm,nd)=if day<days[(month-1) as usize]{(year,month,day+1)}else if month<12{(year,month+1,1)}else{(year+1,1,1)};
    Some(format!("{ny:04}-{nm:02}-{nd:02}"))
}

fn add_date_filter(where_sql:&mut String,args:&mut Vec<rusqlite::types::Value>,filter:&AssetFilter,formats:&FormatTable){
    let Some(column)=date_column(filter.date_source.as_deref()) else { where_sql.push_str(" AND 0");return; };
    where_sql.push_str(&format!(" AND {}",formats.sql_in("a.ext_lower", &["image","raw","video"])));
    let valid=valid_date_sql(column);
    if filter.date_no_date { where_sql.push_str(&format!(" AND CASE WHEN {valid} THEN 0 ELSE 1 END=1"));return; }
    let (Some(from),Some(to))=(filter.date_from.as_deref(),filter.date_to.as_deref()) else { where_sql.push_str(" AND 0");return; };
    let Some(exclusive)=next_date(to) else { where_sql.push_str(" AND 0");return; };
    if next_date(from).is_none() || from>to { where_sql.push_str(" AND 0");return; }
    where_sql.push_str(&format!(" AND ({valid}) AND {column}>=? AND {column}<?"));
    args.push(from.to_owned().into());args.push(exclusive.into());
}

fn descendant_assets_cte(filter: &AssetFilter) -> Option<(String, rusqlite::types::Value)> {
    let (tree_sql, id) = match (filter.kind.as_str(), filter.root_id, filter.id, filter.include_children) {
        ("category", Some(root_id), _, include_children) => {
            let seed = "SELECT id FROM categories WHERE root_id=? AND (parent_id IS NULL OR parent_id=0)";
            let tree = if include_children {
                format!("{seed} UNION SELECT c.id FROM categories c JOIN category_tree t ON c.parent_id=t.id")
            } else { seed.to_string() };
            (format!("WITH RECURSIVE category_tree(id) AS ({tree}), matching_assets(asset_id) AS MATERIALIZED (SELECT DISTINCT ac.asset_id FROM category_tree t JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset ON ac.category_id=t.id)"), root_id)
        }
        ("category", None, Some(id), true) => (
            "WITH RECURSIVE category_tree(id) AS (SELECT id FROM categories WHERE id=? UNION SELECT c.id FROM categories c JOIN category_tree t ON c.parent_id=t.id), matching_assets(asset_id) AS MATERIALIZED (SELECT DISTINCT ac.asset_id FROM category_tree t JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset ON ac.category_id=t.id)".into(), id),
        ("keyword", None, Some(id), true) => (
            "WITH RECURSIVE keyword_tree(id) AS (SELECT id FROM keywords WHERE id=? UNION SELECT k.id FROM keywords k JOIN keyword_tree t ON k.parent_id=t.id), matching_assets(asset_id) AS MATERIALIZED (SELECT DISTINCT ak.asset_id FROM keyword_tree t JOIN asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset ON ak.keyword_id=t.id)".into(), id),
        _ => return None,
    };
    Some((tree_sql, id.into()))
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
        let assets = stmt.query_map([id], |row| map_asset(row, false, false))?.collect::<rusqlite::Result<Vec<_>>>()?;
        result.push((*id, path, assets));
    }
    Ok(result)
}

pub fn assets_page(db: &Db, filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<AssetPage> {
    if filter.kind == "ai" { return ai_assets_page(db, filter, formats); }
    let conn = db.lock().unwrap();
    let ai_attached=conn.query_row("SELECT EXISTS(SELECT 1 FROM ai.sqlite_master WHERE type='table' AND name='colors')",[],|r|r.get::<_,bool>(0)).unwrap_or(false);
    let selected_cols = asset_cols(filter.include_categories, filter.include_keywords);
    let extra_cols = filter.include_categories as i64 + filter.include_keywords as i64;
    let mut from_sql = String::from("assets a");
    let mut with_sql = String::new();
    let mut where_sql = format!("a.is_listed=1 AND {}", formats.sql_not_in("a.ext_lower", &formats.hidden_extensions()));
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some((cte, root_or_node_id)) = descendant_assets_cte(filter) {
        with_sql = cte;
        args.push(root_or_node_id);
        from_sql = "matching_assets m CROSS JOIN assets a".into();
        where_sql.push_str(" AND a.id=m.asset_id");
    }
    match filter.kind.as_str() {
        "folder" => {
            if let Some(ids) = filter.ids.as_ref().filter(|ids| !ids.is_empty()) {
                where_sql.push_str(&format!(" AND a.folder_id IN ({})", vec!["?"; ids.len()].join(",")));
                args.extend(ids.iter().copied().map(Into::into));
            } else { where_sql.push_str(" AND a.folder_id=?"); args.push(filter.id.unwrap_or(-1).into()); }
        }
        "category" => {
            if with_sql.is_empty() {
                from_sql = String::from("asset_categories ac INDEXED BY idx_asset_categories_category_asset JOIN assets a ON a.id=ac.asset_id");
                where_sql.push_str(" AND ac.category_id=?");
                args.push(filter.id.unwrap_or(-1).into());
            }
        }
        "keyword" => {
            if with_sql.is_empty() {
                from_sql = String::from("asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset JOIN assets a ON a.id=ak.asset_id");
                where_sql.push_str(" AND ak.keyword_id=?");
                args.push(filter.id.unwrap_or(-1).into());
            }
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
        "color" => {
            let color=filter.value.as_deref().unwrap_or("");
            if !crate::colors::PALETTE.iter().any(|entry|entry.value==color) { where_sql.push_str(" AND 0"); }
            else if !ai_attached { where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=? AND o.state=1)");args.push(color.to_owned().into()); }
            else {
                where_sql.push_str(" AND (EXISTS(SELECT 1 FROM ai.colors c,json_each(c.auto) j WHERE c.asset_id=a.id AND j.value=? AND coalesce((SELECT o.state FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=?),1)=1) OR EXISTS(SELECT 1 FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=? AND o.state=1))");
                args.extend([color.to_owned().into(),color.to_owned().into(),color.to_owned().into()]);
            }
        }
        "special" => match filter.value.as_deref().unwrap_or("") {
            "all_photos" => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "uncategorized" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "unkeyworded" => where_sql.push_str(&format!(" AND {} AND NOT EXISTS(SELECT 1 FROM asset_keywords akx WHERE akx.asset_id=a.id)", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "tagged" => where_sql.push_str(&format!(" AND {} AND coalesce(a.tagged,0)<>0", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            _ => where_sql.push_str(" AND 0"),
        },
        "date" => add_date_filter(&mut where_sql,&mut args,filter,formats),
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
    if !with_sql.is_empty() {
        let order=order_clause(&filter.sort,filter.descending);
        let sql = format!("{with_sql}, agg AS (SELECT count(*) AS total_count,coalesce(sum(a.size),0) AS total_size FROM {from_sql} WHERE {where_sql}), page AS (SELECT {selected_cols} FROM {from_sql} WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?) SELECT page.*,agg.total_count,agg.total_size FROM agg LEFT JOIN page ON 1");
        let mut query_args=args.clone();
        query_args.extend(args.iter().skip(1).cloned());
        query_args.push(filter.limit.clamp(1,500).into());
        query_args.push(filter.offset.max(0).into());
        let mut stmt=conn.prepare(&sql)?;
        let rows=stmt.query_map(rusqlite::params_from_iter(query_args.iter()),|r|{
            let id:Option<i64>=r.get(0)?;
            let total=r.get((21+extra_cols) as usize)?;
            let total_size=r.get((22+extra_cols) as usize)?;
            let asset=if id.is_some(){Some(map_asset(r,filter.include_categories,filter.include_keywords)?)}else{None};
            Ok((asset,total,total_size))
        })?;
        let mut items=Vec::new();let mut total=0;let mut total_size=0.0;
        for row in rows { let (asset,count,size)=row?;total=count;total_size=size;if let Some(asset)=asset{items.push(asset);} }
        return Ok(AssetPage{items,total,total_size});
    }
    let order = order_clause(&filter.sort, filter.descending);
    let count_sql =
        format!("{with_sql} SELECT count(*),coalesce(sum(a.size),0) FROM {from_sql} WHERE {where_sql}");
    let (total, total_size): (i64, f64) =
        conn.query_row(&count_sql, rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let sql = format!(
        "{with_sql} SELECT {selected_cols} FROM {from_sql} WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    args.push(filter.limit.clamp(1, 500).into());
    args.push(filter.offset.max(0).into());
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| map_asset(r,filter.include_categories,filter.include_keywords))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AssetPage {
        items,
        total,
        total_size,
    })
}

fn ai_assets_page(db: &Db, filter: &AssetFilter, formats: &FormatTable) -> rusqlite::Result<AssetPage> {
    let mut seen = HashSet::new();
    let ids = filter.ids.as_deref().unwrap_or_default().iter().copied().filter(|id| *id > 0 && seen.insert(*id)).take(500).collect::<Vec<_>>();
    if ids.is_empty() { return Ok(AssetPage { items: Vec::new(), total: 0, total_size: 0.0 }); }
    let requested = ids.iter().enumerate().map(|(index, _)| format!("(?{}, {})", index + 1, index)).collect::<Vec<_>>().join(",");
    let with_sql = format!("WITH requested(id,ord) AS (VALUES {requested})");
    let from_sql = "requested r JOIN assets a ON a.id=r.id";
    let mut where_sql = format!("a.id>0 AND a.is_listed=1 AND {}", formats.sql_not_in("a.ext_lower", &formats.hidden_extensions()));
    let mut args = ids.iter().copied().map(Into::into).collect::<Vec<rusqlite::types::Value>>();
    if let Some(q) = filter.query.as_ref().filter(|q| !q.trim().is_empty()) {
        where_sql.push_str(" AND (a.name LIKE ? ESCAPE '\\' OR coalesce(a.caption,'') LIKE ? ESCAPE '\\' OR coalesce(a.notes,'') LIKE ? ESCAPE '\\')");
        let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        args.extend([format!("%{escaped}%").into(), format!("%{escaped}%").into(), format!("%{escaped}%").into()]);
    }
    if filter.date_no_date || filter.date_from.is_some() || filter.date_to.is_some() {
        add_date_filter(&mut where_sql, &mut args, filter, formats);
    }
    add_view_filters(&mut where_sql, &mut args, filter, formats);
    let conn = db.lock().unwrap();
    let count_sql = format!("{with_sql} SELECT count(*),coalesce(sum(a.size),0) FROM {from_sql} WHERE {where_sql}");
    let (total,total_size):(i64,f64)=conn.query_row(&count_sql,rusqlite::params_from_iter(args.iter()),|row|Ok((row.get(0)?,row.get(1)?)))?;
    let selected_cols = asset_cols(filter.include_categories, filter.include_keywords);
    let sql = format!("{with_sql} SELECT {selected_cols} FROM {from_sql} WHERE {where_sql} ORDER BY r.ord LIMIT ? OFFSET ?");
    args.push(filter.limit.clamp(1,500).into());
    args.push(filter.offset.max(0).into());
    let mut statement=conn.prepare(&sql)?;
    let items=statement.query_map(rusqlite::params_from_iter(args.iter()),|row|map_asset(row,filter.include_categories,filter.include_keywords))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AssetPage { items, total, total_size })
}

pub fn attach_display_values(db: &Db, assets: &mut [Asset], include_categories: bool, include_keywords: bool) -> rusqlite::Result<()> {
    if assets.is_empty() || (!include_categories && !include_keywords) { return Ok(()); }
    let conn = db.lock().unwrap();
    let ids = assets.iter().filter(|a| !a.is_directory && a.id > 0).map(|a|a.id).collect::<Vec<_>>();
    if ids.is_empty() { return Ok(()); }
    let placeholders=vec!["?";ids.len()].join(",");
    let args=rusqlite::params_from_iter(ids.iter());
    if include_categories {
        let sql=format!("SELECT asset_id,group_concat(name, ', ') FROM (SELECT ac.asset_id,c.name AS name FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id IN ({placeholders}) GROUP BY ac.asset_id,c.name ORDER BY ac.asset_id,c.name COLLATE NOCASE) GROUP BY asset_id");
        let mut stmt=conn.prepare(&sql)?;
        for row in stmt.query_map(args,|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? { let (id,value)=row?; if let Some(asset)=assets.iter_mut().find(|a|a.id==id){asset.display_categories=Some(value);} }
    }
    if include_keywords {
        let sql=format!("SELECT asset_id,group_concat(name, ', ') FROM (SELECT ak.asset_id,k.name AS name FROM asset_keywords ak JOIN keywords k ON k.id=ak.keyword_id WHERE ak.asset_id IN ({placeholders}) GROUP BY ak.asset_id,k.name ORDER BY ak.asset_id,k.name COLLATE NOCASE) GROUP BY asset_id");
        let mut stmt=conn.prepare(&sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(ids.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? { let (id,value)=row?; if let Some(asset)=assets.iter_mut().find(|a|a.id==id){asset.display_keywords=Some(value);} }
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
    let conn = db.lock().unwrap();
    let mut with_sql = String::new();
    let mut from_sql = String::from("assets a");
    let mut where_sql = format!("a.is_listed=1 AND {}", formats.sql_not_in("a.ext_lower", &formats.hidden_extensions()));
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some((cte, root_or_node_id)) = descendant_assets_cte(filter) {
        with_sql = cte;
        args.push(root_or_node_id);
        from_sql = "matching_assets m CROSS JOIN assets a".into();
        where_sql.push_str(" AND a.id=m.asset_id");
    }
    match filter.kind.as_str() {
        "folder" => {
            if let Some(ids) = filter.ids.as_ref().filter(|ids| !ids.is_empty()) {
                where_sql.push_str(&format!(" AND a.folder_id IN ({})", vec!["?"; ids.len()].join(",")));
                args.extend(ids.iter().copied().map(Into::into));
            } else { where_sql.push_str(" AND a.folder_id=?"); args.push(filter.id.unwrap_or(-1).into()); }
        }
        "category" => {
            if with_sql.is_empty() {
                where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_categories ac WHERE ac.asset_id=a.id AND ac.category_id=?)");
                args.push(filter.id.unwrap_or(-1).into())
            }
        }
        "keyword" => {
            if with_sql.is_empty() {
                where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_keywords ak WHERE ak.asset_id=a.id AND ak.keyword_id=?)");
                args.push(filter.id.unwrap_or(-1).into())
            }
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
        "date" => add_date_filter(&mut where_sql,&mut args,filter,formats),
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
    let sql = format!("{with_sql} SELECT a.id FROM {from_sql} WHERE {where_sql} ORDER BY {} LIMIT ? OFFSET ?", order_clause(&filter.sort, filter.descending));
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

fn category_root_matches(conn: &Connection, root_id: i64) -> rusqlite::Result<Option<i64>> {
    let root_name: String = conn.query_row("SELECT name FROM category_roots WHERE id=?", [root_id], |r| r.get(0))?;
    let mut stmt = conn.prepare("SELECT id,name FROM categories WHERE root_id=? AND (parent_id IS NULL OR parent_id=0)")?;
    let candidates = stmt.query_map([root_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let wanted = root_name.trim().to_lowercase();
    Ok(candidates.into_iter().find(|(_, name)| name.trim().to_lowercase() == wanted).map(|(id, _)| id))
}

pub fn roots_page(db: &Db, offset: i64, limit: i64, include_children: bool) -> rusqlite::Result<Vec<Root>> {
    let conn = db.lock().unwrap();
    let mut roots = conn.prepare("SELECT id,name FROM category_roots ORDER BY name COLLATE NOCASE,id LIMIT ? OFFSET ?")?;
    let rows = roots.query_map(params![limit.clamp(1, 500), offset.max(0)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut top_stmt=conn.prepare("SELECT c.id,c.root_id,c.name,EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=c.id) FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0")?;
    let top=top_stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut seeds=Vec::<(i64,i64)>::new();
    let mut root_data=Vec::with_capacity(rows.len());
    for (id,name) in rows {
        let candidates=top.iter().filter(|item|item.1==id).collect::<Vec<_>>();
        let wanted=name.trim().to_lowercase();
        let merged=candidates.iter().find(|item|item.2.trim().to_lowercase()==wanted).map(|item|item.0);
        if let Some(category_id)=merged { seeds.push((id,category_id)); }
        else { seeds.extend(candidates.iter().map(|item|(id,item.0))); }
        let has_children=if let Some(category_id)=merged {
            candidates.iter().any(|item|item.0!=category_id)||candidates.iter().any(|item|item.0==category_id&&item.3)
        } else { !candidates.is_empty() };
        root_data.push((id,name,merged,has_children));
    }
    let mut counts=std::collections::HashMap::<i64,i64>::new();
    if !seeds.is_empty() {
        let values=vec!["(?,?)";seeds.len()].join(",");
        let recursion=if include_children {"WITH RECURSIVE root_seeds(root_id,seed_id) AS (VALUES __VALUES__), d(root_id,seed_id,id) AS (SELECT root_id,seed_id,seed_id FROM root_seeds UNION SELECT d.root_id,d.seed_id,c.id FROM d JOIN categories c ON c.parent_id=d.id AND c.root_id=d.root_id)"} else {"WITH root_seeds(root_id,seed_id) AS (VALUES __VALUES__), d(root_id,seed_id,id) AS (SELECT root_id,seed_id,seed_id FROM root_seeds)"};
        let sql=format!("{} SELECT d.root_id,count(DISTINCT ac.asset_id) FROM d CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=d.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY d.root_id",recursion.replace("__VALUES__",&values));
        let mut args:Vec<rusqlite::types::Value>=Vec::with_capacity(seeds.len()*2); for (root_id,seed_id) in seeds {args.push(root_id.into());args.push(seed_id.into());}
        let mut stmt=conn.prepare(&sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(args.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))? {let (id,count)=row?;counts.insert(id,count);}
    }
    let mut result = Vec::with_capacity(root_data.len());
    for (id,name,merged_category_id,has_children) in root_data {
        result.push(Root { id, name, count:*counts.get(&id).unwrap_or(&0), merged_category_id, has_children });
    }
    Ok(result)
}

pub fn nodes_page(
    db: &Db,
    table: &str,
    parent_id: Option<i64>,
    root_id: Option<i64>,
    offset: i64,
    limit: i64,
    include_children: bool,
) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    if table == "category" {
        let root_value = root_id.map(rusqlite::types::Value::Integer).unwrap_or(rusqlite::types::Value::Null);
        if parent_id.is_none() {
            if let Some(root) = root_id {
                if let Some(merged_id) = category_root_matches(&conn, root)? {
                    let (sql, args): (&str, Vec<rusqlite::types::Value>) = if include_children {
                        ("WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM categories WHERE root_id=? UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id WHERE c.root_id=?), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,n.root_id,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id) FROM categories n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE (n.parent_id=? OR ((n.parent_id IS NULL OR n.parent_id=0) AND n.id<>?)) AND n.root_id=? ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                            vec![root.into(),root.into(),merged_id.into(),merged_id.into(),root.into(),limit.clamp(1,500).into(),offset.max(0).into()])
                    } else {
                        ("SELECT n.id,n.parent_id,n.root_id,n.name,count(DISTINCT ac.asset_id),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id) FROM categories n LEFT JOIN asset_categories ac ON ac.category_id=n.id LEFT JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 WHERE (n.parent_id=? OR ((n.parent_id IS NULL OR n.parent_id=0) AND n.id<>?)) AND n.root_id=? GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                            vec![merged_id.into(),merged_id.into(),root.into(),limit.clamp(1,500).into(),offset.max(0).into()])
                    };
                    let mut stmt=conn.prepare(sql)?;
                    return stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| Ok(CatalogNode { id:r.get(0)?, parent_id:r.get(1)?, root_id:r.get(2)?, name:r.get(3)?, count:r.get(4)?, has_children:r.get(5)? }))?.collect();
                }
            }
        }
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = if include_children {
            ("WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM categories WHERE (? IS NULL OR root_id=?) UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id WHERE (? IS NULL OR c.root_id=?)), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,n.root_id,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id) FROM categories n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE coalesce(n.parent_id,0)=? AND (? IS NULL OR n.root_id=?) ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![root_value.clone(),root_value.clone(),root_value.clone(),root_value.clone(),parent_id.unwrap_or(0).into(),root_value.clone(),root_value.clone(),limit.clamp(1,500).into(),offset.max(0).into()])
        } else {
            ("SELECT n.id,n.parent_id,n.root_id,n.name,count(DISTINCT ac.asset_id),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id) FROM categories n LEFT JOIN asset_categories ac ON ac.category_id=n.id LEFT JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 JOIN (SELECT ? AS parent_filter, ? AS root_filter) p WHERE coalesce(n.parent_id,0)=p.parent_filter AND (p.root_filter IS NULL OR n.root_id=p.root_filter) GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![parent_id.unwrap_or(0).into(),root_value,limit.clamp(1,500).into(),offset.max(0).into()])
        };
        let mut stmt = conn.prepare(sql)?;
        stmt.query_map(
            rusqlite::params_from_iter(args.iter()),
            |r| {
                Ok(CatalogNode {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    root_id: r.get(2)?,
                    name: r.get(3)?,
                    count: r.get(4)?,
                    has_children: r.get(5)?,
                })
            },
        )?
        .collect()
    } else {
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = if include_children {
            ("WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM keywords UNION SELECT closure.ancestor_id,k.id FROM closure JOIN keywords k ON k.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ak.asset_id) n FROM closure CROSS JOIN asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset CROSS JOIN assets a WHERE ak.keyword_id=closure.id AND a.id=ak.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,NULL,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM keywords child WHERE child.parent_id=n.id) FROM keywords n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE coalesce(n.parent_id,0)=? ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![parent_id.unwrap_or(0).into(),limit.clamp(1,500).into(),offset.max(0).into()])
        } else {
            ("SELECT n.id,n.parent_id,NULL,n.name,count(DISTINCT ak.asset_id),EXISTS(SELECT 1 FROM keywords child WHERE child.parent_id=n.id) FROM keywords n LEFT JOIN asset_keywords ak ON ak.keyword_id=n.id WHERE coalesce(n.parent_id,0)=? GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![parent_id.unwrap_or(0).into(),limit.clamp(1,500).into(),offset.max(0).into()])
        };
        let mut stmt = conn.prepare(sql)?;
        stmt.query_map(
            rusqlite::params_from_iter(args.iter()),
            |r| {
                Ok(CatalogNode {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    root_id: None,
                    name: r.get(3)?,
                    count: r.get(4)?,
                    has_children: r.get(5)?,
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
                has_children: false,
            })
        },
    )?
    .collect()
}

pub fn catalog_tree_search(db:&Db, kind:&str, query:&str, offset:i64, limit:i64, include_children:bool)->rusqlite::Result<Vec<CatalogNode>>{
    let conn=db.lock().unwrap();
    let sql=if kind=="category" {
        if include_children {
            "WITH RECURSIVE t(id,parent_id,root_id,name,path,ancestry) AS (SELECT c.id,c.parent_id,c.root_id,c.name,coalesce((SELECT r.name FROM category_roots r WHERE r.id=c.root_id)||' > ','')||c.name,','||c.id||',' FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0 UNION SELECT n.id,n.parent_id,n.root_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM categories n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0), closure(ancestor_id,id) AS (SELECT id,id FROM categories UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT t.id,t.parent_id,t.root_id,t.path,coalesce(counts.n,0) FROM t LEFT JOIN counts ON counts.ancestor_id=t.id WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
        } else {
            "WITH RECURSIVE t(id,parent_id,root_id,name,path,ancestry) AS (SELECT c.id,c.parent_id,c.root_id,c.name,coalesce((SELECT r.name FROM category_roots r WHERE r.id=c.root_id)||' > ','')||c.name,','||c.id||',' FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0 UNION SELECT n.id,n.parent_id,n.root_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM categories n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0) SELECT t.id,t.parent_id,t.root_id,t.path,(SELECT count(DISTINCT ac.asset_id) FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 WHERE ac.category_id=t.id) FROM t WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
        }
    } else if include_children {
        "WITH RECURSIVE t(id,parent_id,name,path,ancestry) AS (SELECT id,parent_id,name,name,','||id||',' FROM keywords WHERE parent_id IS NULL OR parent_id=0 UNION SELECT n.id,n.parent_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM keywords n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0), closure(ancestor_id,id) AS (SELECT id,id FROM keywords UNION SELECT closure.ancestor_id,k.id FROM closure JOIN keywords k ON k.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ak.asset_id) n FROM closure CROSS JOIN asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset CROSS JOIN assets a WHERE ak.keyword_id=closure.id AND a.id=ak.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT t.id,t.parent_id,NULL,t.path,coalesce(counts.n,0) FROM t LEFT JOIN counts ON counts.ancestor_id=t.id WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
    } else {
        "WITH RECURSIVE t(id,parent_id,name,path,ancestry) AS (SELECT id,parent_id,name,name,','||id||',' FROM keywords WHERE parent_id IS NULL OR parent_id=0 UNION SELECT n.id,n.parent_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM keywords n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0) SELECT t.id,t.parent_id,NULL,t.path,(SELECT count(DISTINCT ak.asset_id) FROM asset_keywords ak JOIN assets a ON a.id=ak.asset_id AND a.is_listed=1 WHERE ak.keyword_id=t.id) FROM t WHERE t.name LIKE ? COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
    };
    let mut stmt=conn.prepare(sql)?;
    let mut nodes=stmt.query_map(params![format!("%{}%",query.replace('%',"\\%").replace('_',"\\_")),limit.clamp(1,500),offset.max(0)],|r|Ok(CatalogNode{id:r.get(0)?,parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,count:r.get(4)?,has_children:false}))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if kind=="category" {
        for node in &mut nodes {
            let mut parts=node.name.split(" > ").collect::<Vec<_>>();
            if parts.len()>1 && parts[0].trim().to_lowercase()==parts[1].trim().to_lowercase() { parts.remove(1); node.name=parts.join(" > "); }
        }
    }
    Ok(nodes)
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
            |row| map_asset(row, false, false),
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


pub fn asset_catalog_links(db: &Db, id: i64) -> rusqlite::Result<Option<CatalogAssetLinks>> {
    let conn = db.lock().unwrap();
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?)", [id], |r| r.get(0))?;
    if !exists { return Ok(None); }
    let category_ids = conn.prepare("SELECT category_id FROM asset_categories WHERE asset_id=? ORDER BY category_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut category_paths = Vec::with_capacity(category_ids.len());
    for category_id in &category_ids {
        let (root, root_name): (Option<i64>, Option<String>) = conn.query_row("SELECT c.root_id,r.name FROM categories c LEFT JOIN category_roots r ON r.id=c.root_id WHERE c.id=?", [category_id], |r| Ok((r.get(0)?,r.get(1)?)))?;
        let mut names=Vec::new();let mut current=Some(*category_id);let mut seen=HashSet::new();
        while let Some(cid)=current { if !seen.insert(cid){break;} let row:Option<(String,Option<i64>)>=conn.query_row("SELECT name,parent_id FROM categories WHERE id=?",[cid],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;let Some((name,parent))=row else{break};names.push(name);current=parent; }
        names.reverse();if let Some(root_name)=root_name{if !names.first().is_some_and(|n|n.trim().eq_ignore_ascii_case(root_name.trim())){names.insert(0,root_name);}}else if root.is_some()&&names.is_empty(){continue;}
        category_paths.push(CategoryPath{id:*category_id,path:names.join(" › ")});
    }
    let keyword_ids = conn.prepare("SELECT keyword_id FROM asset_keywords WHERE asset_id=? ORDER BY keyword_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut keyword_paths=Vec::with_capacity(keyword_ids.len());for keyword_id in &keyword_ids{let mut names=Vec::new();let mut current=Some(*keyword_id);let mut seen=HashSet::new();while let Some(key)=current{if !seen.insert(key){break;}let row:Option<(String,Option<i64>)>=conn.query_row("SELECT name,parent_id FROM keywords WHERE id=?",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;let Some((name,parent))=row else{break};names.push(name);current=parent;}names.reverse();keyword_paths.push(CategoryPath{id:*keyword_id,path:names.join(" › ")});}
    let category_ancestor_ids = conn.prepare("WITH RECURSIVE ancestors(id) AS (SELECT parent_id FROM categories WHERE id IN (SELECT category_id FROM asset_categories WHERE asset_id=?) AND parent_id IS NOT NULL UNION SELECT c.parent_id FROM categories c JOIN ancestors a ON c.id=a.id WHERE c.parent_id IS NOT NULL) SELECT id FROM ancestors ORDER BY id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let keyword_ancestor_ids = conn.prepare("WITH RECURSIVE ancestors(id) AS (SELECT parent_id FROM keywords WHERE id IN (SELECT keyword_id FROM asset_keywords WHERE asset_id=?) AND parent_id IS NOT NULL UNION SELECT k.parent_id FROM keywords k JOIN ancestors a ON k.id=a.id WHERE k.parent_id IS NOT NULL) SELECT id FROM ancestors ORDER BY id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let root_ids = conn.prepare("SELECT DISTINCT c.root_id FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id=? AND c.root_id IS NOT NULL ORDER BY c.root_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(CatalogAssetLinks { category_ids, category_paths, category_ancestor_ids, keyword_ids, keyword_paths, keyword_ancestor_ids, root_ids }))
}

pub fn keyword_options(db:&Db,query:&str)->rusqlite::Result<Vec<KeywordOption>>{
    let conn=db.lock().unwrap();let rows=conn.prepare("SELECT id,parent_id,name FROM keywords ORDER BY id")?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let by_id=rows.iter().map(|(id,parent,name)|(*id,(*parent,name.clone()))).collect::<HashMap<_,_>>();let needle=query.trim().to_lowercase();let mut found=Vec::new();
    for(id,_,name)in rows{if !needle.is_empty()&&!name.to_lowercase().contains(&needle){continue;}let mut current=Some(id);let mut seen=HashSet::new();let mut names=Vec::new();while let Some(key)=current{if !seen.insert(key){break;}let Some((parent,label))=by_id.get(&key)else{break};names.push(label.clone());current=*parent;}names.reverse();found.push((!name.to_lowercase().starts_with(&needle),name.to_lowercase(),KeywordOption{id,name,path:names.join(" › ")}));}
    found.sort_by(|a,b|a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.path.cmp(&b.2.path)));Ok(found.into_iter().take(30).map(|(_,_,item)|item).collect())
}

#[cfg(test)]
mod feature_tests {
    use super::*;
    use std::cmp::Ordering;
    fn asset(id:i64,name:&str,size:Option<f64>,rating:Option<i64>)->Asset { Asset{id,folder_id:Some(1),name:name.into(),ext:"jpg".into(),size,width:Some(10),height:Some(10),bpp:None,caption:None,notes:None,author:None,rating,tagged:Some(0),label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:None,is_directory:false,offline:false,has_categories:false,display_categories:None,display_keywords:None} }
    #[test]
    fn filter_logic_supports_modes_and_groups() {
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let unrated = asset(1, "a.jpg", None, None);
        let rated = asset(2, "b.jpg", Some(10.0), Some(4));
        let mut f = AssetFilter { kind:"folder".into(), id:None, root_id:None, include_children:false, folder_path:None, ids:None, value:None, query:None, sort:"name".into(), descending:false, offset:0, limit:100, rating_mode:Some("atLeast".into()), rating_value:Some(3), label_filter:None, tagged_only:false, include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group:Some("images".into()) };
        assert!(asset_filter_matches(&rated, &f, &formats));
        assert!(!asset_filter_matches(&unrated, &f, &formats));
        f.rating_mode = Some("unrated".into());
        assert!(asset_filter_matches(&unrated, &f, &formats));
        assert!(!asset_filter_matches(&rated, &f, &formats));
    }
    #[test] fn sort_keeps_empty_values_last_in_both_directions(){let empty=asset(1,"empty",None,None);let full=asset(2,"full",Some(5.0),Some(4));for desc in [false,true]{assert_eq!(compare_assets(&full,&empty,"size",desc),Ordering::Less);}}
    fn test_db() -> Db {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);CREATE TABLE categories(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT);CREATE TABLE category_roots(id INTEGER PRIMARY KEY,name TEXT);CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));CREATE INDEX idx_asset_categories_category_asset ON asset_categories(category_id,asset_id);CREATE INDEX idx_asset_keywords_keyword_asset ON asset_keywords(keyword_id,asset_id);CREATE INDEX idx_categories_parent_id ON categories(parent_id);CREATE INDEX idx_keywords_parent_id ON keywords(parent_id);").unwrap();
        Arc::new(Mutex::new(c))
    }
    fn test_filter(kind:&str,value:&str)->AssetFilter { AssetFilter{kind:kind.into(),id:None,root_id:None,include_children:false,folder_path:None,ids:None,value:Some(value.into()),query:None,sort:"name".into(),descending:false,offset:0,limit:10,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group:None} }
    #[test] fn special_items_use_listed_photos_and_relationships(){
        let db=test_db();db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,tagged,rating) VALUES(1,'a.jpg','jpg','jpg',1,10,1,4),(2,'b.doc','doc','doc',1,20,0,5),(3,'gone.jpg','jpg','jpg',0,30,1,3),(4,'d.png','png','png',1,40,0,NULL);INSERT INTO asset_categories VALUES(1,9),(2,9);INSERT INTO asset_keywords VALUES(1,8);").unwrap();
        for (kind,count) in [("all_photos",2),("uncategorized",1),("unkeyworded",1),("tagged",1)] { assert_eq!(assets_page(&db,&test_filter("special",kind),&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().total,count,"{kind}"); }
        assert_eq!(values_page(&db,"special",0,10,&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().iter().find(|v|v.value=="unkeyworded").unwrap().count,1);
    }
    #[test] fn keyword_subquery_is_only_selected_when_requested() {
        let db=test_db(); db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'a.jpg','jpg','jpg',1,10);INSERT INTO keywords(id,name) VALUES(1,'Rodina'),(2,'Príroda');INSERT INTO asset_keywords VALUES(1,1),(1,2);").unwrap();
        let formats=FormatTable::parse(include_str!("../formats.json")).unwrap(); let mut f=test_filter("special","all_photos");
        let without=assets_page(&db,&f,&formats).unwrap(); assert_eq!(without.items[0].display_keywords,None);
        f.include_keywords=true; let with=assets_page(&db,&f,&formats).unwrap(); assert_eq!(with.items[0].display_keywords.as_deref(),Some("Príroda, Rodina"));
    }
    #[test] fn rating_query_uses_selected_value_and_excludes_unrated(){
        let db=test_db();db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating) VALUES(1,'one.jpg','jpg','jpg',1,1,1),(2,'three.jpg','jpg','jpg',1,1,3),(3,'five.jpg','jpg','jpg',1,1,5),(4,'none.jpg','jpg','jpg',1,1,NULL);").unwrap();
        for (mode,value,expected) in [("equal",3,1),("atLeast",3,2),("atMost",3,2)] { let mut f=test_filter("special","all_photos");f.rating_mode=Some(mode.into());f.rating_value=Some(value);assert_eq!(assets_page(&db,&f,&FormatTable::parse(include_str!("../formats.json")).unwrap()).unwrap().total,expected,"{mode}"); }
    }
    #[test]
    fn descendant_views_are_distinct_optional_and_include_roots() {
        let db=test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(10,'Root'),(20,'Cycle');
            INSERT INTO categories VALUES(100,NULL,10,'Top'),(101,100,10,'Child'),(102,101,10,'Grandchild'),(200,201,20,'Cycle A'),(201,200,20,'Cycle B');
            INSERT INTO keywords VALUES(500,NULL,'Top'),(501,500,'Child'),(502,501,'Grandchild');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'a.jpg','jpg','jpg',1,10),(2,'b.jpg','jpg','jpg',1,20),(3,'c.jpg','jpg','jpg',1,30);
            INSERT INTO asset_categories VALUES(1,100),(2,101),(2,102),(3,102),(1,200),(2,201);
            INSERT INTO asset_keywords VALUES(1,500),(2,501),(2,502),(3,502);").unwrap();
        let formats=FormatTable::parse(include_str!("../formats.json")).unwrap();
        let mut category=test_filter("category","");category.id=Some(101);
        let direct=assets_page(&db,&category,&formats).unwrap();assert_eq!(direct.total,1);assert_eq!(direct.total_size,20.0);
        category.include_children=true;
        let descendants=assets_page(&db,&category,&formats).unwrap();assert_eq!(descendants.total,2);assert_eq!(descendants.total_size,50.0);
        assert_eq!(asset_ids_page(&db,&category,&formats,0,20).unwrap(),vec![2,3]);
        let mut root=test_filter("category","");root.root_id=Some(10);root.include_children=true;
        let root_page=assets_page(&db,&root,&formats).unwrap();assert_eq!(root_page.total,3);assert_eq!(root_page.total_size,60.0);
        root.include_children=false;assert_eq!(assets_page(&db,&root,&formats).unwrap().total,1);
        let mut keyword=test_filter("keyword","");keyword.id=Some(501);
        assert_eq!(assets_page(&db,&keyword,&formats).unwrap().total,1);
        keyword.include_children=true;assert_eq!(assets_page(&db,&keyword,&formats).unwrap().total,2);
        assert_eq!(asset_ids_page(&db,&keyword,&formats,0,20).unwrap(),vec![2,3]);
        let mut cycle=test_filter("category","");cycle.id=Some(200);cycle.include_children=true;
        assert_eq!(assets_page(&db,&cycle,&formats).unwrap().total,2);
        let roots_on=roots_page(&db,0,20,true).unwrap();assert_eq!(roots_on.iter().find(|r|r.id==10).unwrap().count,3);
        let roots_off=roots_page(&db,0,20,false).unwrap();assert_eq!(roots_off.iter().find(|r|r.id==10).unwrap().count,1);
        let top_on=nodes_page(&db,"category",None,Some(10),0,20,true).unwrap();assert_eq!(top_on[0].count,3);
        let top_off=nodes_page(&db,"category",None,Some(10),0,20,false).unwrap();assert_eq!(top_off[0].count,1);
        let child_on=nodes_page(&db,"category",Some(100),Some(10),0,20,true).unwrap();assert_eq!(child_on[0].count,2);
        let keywords_on=nodes_page(&db,"keyword",None,None,0,20,true).unwrap();assert_eq!(keywords_on[0].count,3);
    }
    #[test]
    fn merged_category_roots_expand_count_and_search_without_duplicate_level() {
        let db=test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(1,'Miesta'),(2,'Príroda'),(3,'Bez kategórií');
            INSERT INTO categories VALUES(10,NULL,1,' miesta '),(11,10,1,'Dieťa'),(12,NULL,1,'Extra'),(20,NULL,2,'Krajina');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) VALUES(1,'root.jpg','jpg','jpg',1,1),(2,'child.jpg','jpg','jpg',1,1),(3,'extra.jpg','jpg','jpg',1,1),(4,'nature.jpg','jpg','jpg',1,1);
            INSERT INTO asset_categories VALUES(1,10),(2,11),(3,12),(4,20);").unwrap();
        let roots_on=roots_page(&db,0,10,true).unwrap();
        let places=roots_on.iter().find(|r|r.id==1).unwrap();
        assert_eq!(places.merged_category_id,Some(10));assert_eq!(places.count,2);assert!(places.has_children);
        let places_off=roots_page(&db,0,10,false).unwrap().into_iter().find(|r|r.id==1).unwrap();
        assert_eq!(places_off.count,1);
        let children=nodes_page(&db,"category",None,Some(1),0,20,true).unwrap();
        assert_eq!(children.iter().map(|n|n.id).collect::<Vec<_>>(),vec![11,12]);
        assert!(children.iter().any(|n|n.id==11&&n.count==1));
        let path=catalog_tree_search(&db,"category","dieťa",0,20,true).unwrap();
        assert_eq!(path[0].name,"Miesta > Dieťa");
        let nature=roots_page(&db,0,10,true).unwrap().into_iter().find(|r|r.id==2).unwrap();
        assert_eq!(nature.merged_category_id,None);assert_eq!(nature.count,1);
        assert_eq!(nodes_page(&db,"category",None,Some(2),0,20,true).unwrap()[0].id,20);
        let empty=roots_page(&db,0,10,true).unwrap().into_iter().find(|r|r.id==3).unwrap();
        assert_eq!(empty.merged_category_id,None);assert_eq!(empty.count,0);assert!(!empty.has_children);
    }
    #[test]
    fn asset_catalog_links_include_direct_links_ancestors_and_root_ids() {
        let db=test_db();
        db.lock().unwrap().execute_batch("INSERT INTO category_roots VALUES(1,'Miesta');
            INSERT INTO categories VALUES(10,NULL,1,'Miesta'),(11,10,1,'Dieťa'),(12,11,1,'Vnuk');
            INSERT INTO keywords VALUES(20,NULL,'Téma'),(21,20,'Podtéma'),(22,21,'List');
            INSERT INTO assets(id,name,ext,ext_lower,is_listed) VALUES(1,'linked.jpg','jpg','jpg',1),(2,'uncatalogued.jpg','jpg','jpg',1);
            INSERT INTO asset_categories VALUES(1,12);INSERT INTO asset_keywords VALUES(1,22);").unwrap();
        let links=asset_catalog_links(&db,1).unwrap().unwrap();
        assert_eq!(links.category_ids,vec![12]);assert_eq!(links.category_paths,vec![CategoryPath{id:12,path:"Miesta › Dieťa › Vnuk".into()}]);assert_eq!(links.category_ancestor_ids,vec![10,11]);
        assert_eq!(links.keyword_ids,vec![22]);assert_eq!(links.keyword_ancestor_ids,vec![20,21]);
        assert_eq!(links.root_ids,vec![1]);
        assert!(asset_catalog_links(&db,2).unwrap().unwrap().category_ids.is_empty());
        assert!(asset_catalog_links(&db,99).unwrap().is_none());
    }

    #[test]
    fn ai_view_preserves_order_applies_filters_and_skips_unknown_ids() {
        let db=test_db();
        db.lock().unwrap().execute_batch("INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating,tagged) VALUES
            (1,'plain.jpg','jpg','jpg',1,10,4,0),(2,'z-last.jpg','jpg','jpg',1,20,4,1),
            (3,'offline-row.jpg','jpg','jpg',0,30,5,1),(4,'a-first.png','png','png',1,40,3,1);").unwrap();
        let formats=FormatTable::parse(include_str!("../formats.json")).unwrap();
        let filter=AssetFilter { kind:"ai".into(),id:None,root_id:None,include_children:false,folder_path:None,ids:Some(vec![4,2,404,3,1,2]),value:None,query:None,sort:"name".into(),descending:true,offset:0,limit:500,rating_mode:Some("atLeast".into()),rating_value:Some(3),label_filter:None,tagged_only:true,file_group:Some("images".into()),include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false };
        let page=assets_page(&db,&filter,&formats).unwrap();
        assert_eq!(page.items.iter().map(|asset|asset.id).collect::<Vec<_>>(),vec![4,2]);
        assert_eq!(page.total,2);assert_eq!(page.total_size,60.0);
        let mut second=filter.clone();second.offset=1;second.limit=1;
        let page=assets_page(&db,&second,&formats).unwrap();
        assert_eq!(page.items.iter().map(|asset|asset.id).collect::<Vec<_>>(),vec![2]);
        assert_eq!(page.total,2);assert_eq!(page.total_size,60.0);
    }

    #[test]
    fn color_view_uses_effective_colors_and_normal_paging_filters() {
        let db=test_db();let conn=db.lock().unwrap();conn.execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER,color TEXT,state INTEGER,PRIMARY KEY(asset_id,color));ATTACH DATABASE ':memory:' AS ai;CREATE TABLE ai.colors(asset_id INTEGER PRIMARY KEY,auto TEXT);INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,rating) VALUES(1,'a.jpg','jpg','jpg',1,10,4),(2,'b.jpg','jpg','jpg',1,20,2),(3,'c.jpg','jpg','jpg',1,30,5),(4,'hidden.jpg','jpg','jpg',0,40,5);INSERT INTO ai.colors VALUES(1,'[\"červená\"]'),(2,'[]'),(3,'[\"červená\"]');INSERT INTO asset_color_overrides VALUES(2,'červená',1),(3,'červená',0);").unwrap();drop(conn);
        let formats=FormatTable::parse(include_str!("../formats.json")).unwrap();let mut filter=test_filter("color","červená");filter.limit=1;filter.rating_mode=Some("atLeast".into());filter.rating_value=Some(3);
        let first=assets_page(&db,&filter,&formats).unwrap();assert_eq!(first.total,1);assert_eq!(first.total_size,10.0);assert_eq!(first.items[0].id,1);
        filter.rating_mode=None;filter.offset=1;let second=assets_page(&db,&filter,&formats).unwrap();assert_eq!(second.total,2);assert_eq!(second.items[0].id,2);
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
                root_id: None,
                include_children: false,
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 0,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group: None,
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
                root_id: None,
                include_children: false,
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 3000,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group: None,
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
        let _ = roots_page(&db, 0, 100, true);
        let _ = nodes_page(&db, "category", None, Some(1), 0, 500, true).unwrap();
        println!("category tree first level: {:?}", t.elapsed());
    }
    #[test]
    #[ignore]
    fn bench_keyword_tree() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let t = Instant::now();
        let _ = nodes_page(&db, "keyword", None, None, 0, 500, true).unwrap();
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
                root_id: None,
                include_children: false,
                folder_path: None,
                ids: None,
                value: None,
                query: None,
                sort: "name".into(),
                descending: false,
                offset: 0,
                limit: 100,
                rating_mode: None, rating_value: None, label_filter: None, tagged_only: false, include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group: None,
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
    fn bench_largest_category_root() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db = Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let root = roots_page(&db, 0, 500, true).unwrap().into_iter().max_by_key(|r| r.count).unwrap();
        let formats = FormatTable::load();
        let started = Instant::now();
        let page = assets_page(&db, &AssetFilter {
            kind: "category".into(), id: None, root_id: Some(root.id), include_children: true,
            folder_path: None, ids: None, value: None, query: None, sort: "name".into(),
            descending: false, offset: 0, limit: 100, rating_mode: None, rating_value: None,
            label_filter: None, tagged_only: false, include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group: None,
        }, &formats).unwrap();
        println!("largest category root (id {}, tree count {}, view total {}, first page {}): {:?}", root.id, root.count, page.total, page.items.len(), started.elapsed());
    }
    #[test]
    #[ignore]
    fn bench_special_items() {
        let _guard = BENCH_LOCK.lock().unwrap();
        let db=Arc::new(Mutex::new(open_read_only(&db_path()).unwrap()));
        let formats=FormatTable::load();
        for value in ["all_photos","uncategorized","unkeyworded","tagged"] {
            let t=Instant::now();let p=assets_page(&db,&AssetFilter{kind:"special".into(),id:None,root_id:None,include_children:false,folder_path:None,ids:None,value:Some(value.into()),query:None,sort:"name".into(),descending:false,offset:0,limit:100,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group:None},&formats).unwrap();
            println!("special {value}: total {}, first page {}, {:?}",p.total,p.items.len(),t.elapsed());
        }
    }
}
