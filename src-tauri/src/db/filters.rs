use super::assets::Asset;
use crate::formats::FormatTable;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

const ASSET_COLS: &str = "a.id,a.folder_id,a.name,a.ext,a.size,a.width,a.height,a.bpp,a.caption,a.notes,a.author,a.rating,a.tagged,a.label,a.file_modified,a.exif_date,a.acdsee_date,a.md_dirty,a.is_orphan,a.path,EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id),EXISTS(SELECT 1 FROM asset_keywords akx WHERE akx.asset_id=a.id)";

pub(super) fn asset_cols(
    conn: &Connection,
    include_categories: bool,
    include_keywords: bool,
) -> rusqlite::Result<String> {
    let mut columns = ASSET_COLS.to_owned();
    if include_categories {
        columns.push_str(", (SELECT group_concat(name, ', ') FROM (SELECT DISTINCT c.name AS name FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id=a.id ORDER BY c.name COLLATE NOCASE))");
    }
    if include_keywords {
        columns.push_str(", (SELECT group_concat(name, ', ') FROM (SELECT DISTINCT k.name AS name FROM asset_keywords ak JOIN keywords k ON k.id=ak.keyword_id WHERE ak.asset_id=a.id ORDER BY k.name COLLATE NOCASE))");
    }
    columns.push_str(if crate::schema::has_asset_column(conn, "edit_pending_since")? {
        ",a.edit_pending_since"
    } else {
        ",NULL AS edit_pending_since"
    });
    Ok(columns)
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
    #[serde(default)]
    pub rating_mode: Option<String>,
    #[serde(default)]
    pub rating_value: Option<i64>,
    #[serde(default)]
    pub label_filter: Option<String>,
    #[serde(default)]
    pub tagged_only: bool,
    #[serde(default)]
    pub category_filter: Option<AssignmentFilter>,
    #[serde(default)]
    pub keyword_filter: Option<AssignmentFilter>,
    #[serde(default)]
    pub file_group: Option<String>,
    #[serde(default)]
    pub include_categories: bool,
    #[serde(default)]
    pub include_keywords: bool,
    #[serde(default)]
    pub date_from: Option<String>,
    #[serde(default)]
    pub date_to: Option<String>,
    #[serde(default)]
    pub date_source: Option<String>,
    #[serde(default)]
    pub date_no_date: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentFilter {
    With,
    Without,
}

pub fn date_column(source: Option<&str>) -> Option<&'static str> {
    match source.unwrap_or("taken") {
        "taken" => Some("a.exif_date"),
        "database" => Some("a.acdsee_date"),
        "modified" => Some("a.file_modified"),
        _ => None,
    }
}

pub fn valid_date_sql(column: &str) -> String {
    format!(
        "{column} IS NOT NULL AND length({column})>=10 AND substr({column},1,4) GLOB '[0-9][0-9][0-9][0-9]' AND substr({column},1,4)>='1900' AND substr({column},5,1)='-' AND CAST(substr({column},6,2) AS INTEGER) BETWEEN 1 AND 12 AND substr({column},8,1)='-' AND CAST(substr({column},9,2) AS INTEGER) BETWEEN 1 AND 31 AND coalesce(date(substr({column},1,10),'+0 days')=substr({column},1,10),0)"
    )
}

pub fn next_date(date: &str) -> Option<String> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = date[0..4].parse::<i32>().ok()?;
    let month = date[5..7].parse::<u32>().ok()?;
    let day = date[8..10].parse::<u32>().ok()?;
    if !(1900..=9998).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if day == 0 || day > days[(month - 1) as usize] {
        return None;
    }
    let (ny, nm, nd) = if day < days[(month - 1) as usize] {
        (year, month, day + 1)
    } else if month < 12 {
        (year, month + 1, 1)
    } else {
        (year + 1, 1, 1)
    };
    Some(format!("{ny:04}-{nm:02}-{nd:02}"))
}

pub(super) fn add_date_filter(
    where_sql: &mut String,
    args: &mut Vec<rusqlite::types::Value>,
    filter: &AssetFilter,
    formats: &FormatTable,
) {
    let Some(column) = date_column(filter.date_source.as_deref()) else {
        where_sql.push_str(" AND 0");
        return;
    };
    where_sql.push_str(&format!(
        " AND {}",
        formats.sql_in("a.ext_lower", &["image", "raw", "video"])
    ));
    let valid = valid_date_sql(column);
    if filter.date_no_date {
        where_sql.push_str(&format!(" AND CASE WHEN {valid} THEN 0 ELSE 1 END=1"));
        return;
    }
    let (Some(from), Some(to)) = (filter.date_from.as_deref(), filter.date_to.as_deref()) else {
        where_sql.push_str(" AND 0");
        return;
    };
    let Some(exclusive) = next_date(to) else {
        where_sql.push_str(" AND 0");
        return;
    };
    if next_date(from).is_none() || from > to {
        where_sql.push_str(" AND 0");
        return;
    }
    where_sql.push_str(&format!(" AND ({valid}) AND {column}>=? AND {column}<?"));
    args.push(from.to_owned().into());
    args.push(exclusive.into());
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

pub(super) fn add_view_filters(
    where_sql: &mut String,
    args: &mut Vec<rusqlite::types::Value>,
    filter: &AssetFilter,
    formats: &FormatTable,
) {
    if filter.rating_mode.as_deref() == Some("unrated") {
        where_sql.push_str(" AND coalesce(a.rating,0)=0");
    } else if let (Some(mode), Some(value)) = (filter.rating_mode.as_deref(), filter.rating_value) {
        match mode {
            "equal" => {
                where_sql.push_str(" AND coalesce(a.rating,0)=?");
                args.push(value.into());
            }
            "atLeast" => {
                where_sql.push_str(" AND a.rating>=?");
                args.push(value.into());
            }
            "atMost" => {
                where_sql.push_str(" AND a.rating<=?");
                args.push(value.into());
            }
            "unrated" => where_sql.push_str(" AND coalesce(a.rating,0)=0"),
            _ => {}
        }
    }
    if let Some(label) = filter.label_filter.as_ref() {
        where_sql.push_str(" AND coalesce(a.label,'')=?");
        args.push(label.clone().into());
    }
    if filter.tagged_only {
        where_sql.push_str(" AND coalesce(a.tagged,0)<>0");
    }
    for (mode, table) in [
        (filter.category_filter, "asset_categories"),
        (filter.keyword_filter, "asset_keywords"),
    ] {
        if let Some(mode) = mode {
            let operator = match mode {
                AssignmentFilter::With => "EXISTS",
                AssignmentFilter::Without => "NOT EXISTS",
            };
            where_sql.push_str(&format!(" AND {operator}(SELECT 1 FROM {table} WHERE asset_id=a.id)"));
        }
    }
    match filter.file_group.as_deref() {
        Some("images") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
        Some("raw") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["raw"]))),
        Some("video") => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["video"]))),
        Some("other") => where_sql.push_str(&format!(
            " AND {}",
            formats.sql_not_in("a.ext_lower", &["image", "raw", "video"])
        )),
        // "media" = what the grid shows by default: photos, RAW and video; other files stay hidden.
        Some("media") => where_sql.push_str(&format!(
            " AND {}",
            formats.sql_in("a.ext_lower", &["image", "raw", "video"])
        )),
        _ => {}
    }
}

pub(super) fn order_clause(sort: &str, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let (expr, empty) = match sort {
        "date" | "dateTaken" => (
            "coalesce(nullif(a.exif_date,''),nullif(a.file_modified,''))",
            "coalesce(nullif(a.exif_date,''),nullif(a.file_modified,'')) IS NULL",
        ),
        "captureTime" => ("a.exif_date", "a.exif_date IS NULL OR a.exif_date=''"),
        "modified" => ("a.file_modified", "a.file_modified IS NULL OR a.file_modified=''"),
        "size" => ("a.size", "a.size IS NULL"),
        "type" => ("a.ext_lower", "a.ext_lower IS NULL OR a.ext_lower=''"),
        "dimensions" => (
            "coalesce(a.width,0)*coalesce(a.height,0)",
            "a.width IS NULL OR a.height IS NULL",
        ),
        "rating" => ("a.rating", "a.rating IS NULL OR a.rating=0"),
        "label" => ("a.label", "a.label IS NULL OR a.label=''"),
        "tagged" => ("a.tagged", "a.tagged IS NULL"),
        "caption" => ("a.caption", "a.caption IS NULL OR a.caption=''"),
        _ => ("a.name COLLATE NOCASE", "a.name IS NULL OR a.name=''"),
    };
    format!("({empty}) ASC, {expr} {direction}, a.name COLLATE NOCASE ASC, a.id ASC")
}

pub fn asset_filter_matches(a: &Asset, f: &AssetFilter, formats: &FormatTable) -> bool {
    if f.rating_mode.as_deref() == Some("unrated") && a.rating.unwrap_or(0) != 0 {
        return false;
    }
    if let (Some(mode), Some(value)) = (f.rating_mode.as_deref(), f.rating_value) {
        let rating = a.rating.unwrap_or(0);
        let ok = match mode {
            "equal" => rating == value,
            "atLeast" => a.rating.is_some_and(|r| r >= value),
            "atMost" => a.rating.is_some_and(|r| r <= value),
            "unrated" => rating == 0,
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if f.label_filter
        .as_ref()
        .is_some_and(|v| a.label.as_deref().unwrap_or("") != v)
        || (f.tagged_only && a.tagged.unwrap_or(0) == 0)
    {
        return false;
    }
    if f.category_filter
        .is_some_and(|mode| a.has_categories != matches!(mode, AssignmentFilter::With))
        || f.keyword_filter
            .is_some_and(|mode| a.has_keywords != matches!(mode, AssignmentFilter::With))
    {
        return false;
    }
    let group = formats.group(&a.ext);
    match f.file_group.as_deref() {
        Some("images") if !matches!(group, Some("image" | "raw")) => false,
        Some("raw") if group != Some("raw") => false,
        Some("video") if group != Some("video") => false,
        Some("other") if matches!(group, Some("image" | "raw" | "video")) => false,
        Some("media") if !matches!(group, Some("image" | "raw" | "video")) => false,
        _ => true,
    }
}

pub fn compare_assets(a: &Asset, b: &Asset, sort: &str, descending: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let empty_a;
    let empty_b;
    let (cmp, ea, eb) = match sort {
        "date" | "dateTaken" => {
            let av = capture_date(a).or(a.file_modified.as_deref().filter(|date| !date.is_empty()));
            let bv = capture_date(b).or(b.file_modified.as_deref().filter(|date| !date.is_empty()));
            (av.cmp(&bv), av.is_none(), bv.is_none())
        }
        "captureTime" => {
            let (av, bv) = (capture_date(a), capture_date(b));
            (av.cmp(&bv), av.is_none(), bv.is_none())
        }
        "modified" => (
            a.file_modified.cmp(&b.file_modified),
            a.file_modified.is_none(),
            b.file_modified.is_none(),
        ),
        "size" => (
            a.size.partial_cmp(&b.size).unwrap_or(Ordering::Equal),
            a.size.is_none(),
            b.size.is_none(),
        ),
        "type" => (
            a.ext.to_lowercase().cmp(&b.ext.to_lowercase()),
            a.ext.is_empty(),
            b.ext.is_empty(),
        ),
        "dimensions" => {
            let av = a.width.zip(a.height).map(|(w, h)| w.saturating_mul(h));
            let bv = b.width.zip(b.height).map(|(w, h)| w.saturating_mul(h));
            (av.cmp(&bv), av.is_none(), bv.is_none())
        }
        "rating" => (
            a.rating.cmp(&b.rating),
            a.rating.is_none_or(|v| v == 0),
            b.rating.is_none_or(|v| v == 0),
        ),
        "label" => (
            a.label
                .as_ref()
                .map(|s| s.to_lowercase())
                .cmp(&b.label.as_ref().map(|s| s.to_lowercase())),
            a.label.as_deref().is_none_or(str::is_empty),
            b.label.as_deref().is_none_or(str::is_empty),
        ),
        "tagged" => (a.tagged.cmp(&b.tagged), a.tagged.is_none(), b.tagged.is_none()),
        "caption" => (
            a.caption
                .as_ref()
                .map(|s| s.to_lowercase())
                .cmp(&b.caption.as_ref().map(|s| s.to_lowercase())),
            a.caption.as_deref().is_none_or(str::is_empty),
            b.caption.as_deref().is_none_or(str::is_empty),
        ),
        _ => (
            a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            a.name.is_empty(),
            b.name.is_empty(),
        ),
    };
    empty_a = ea;
    empty_b = eb;
    if empty_a != empty_b {
        return empty_a.cmp(&empty_b);
    }
    let directed = if descending { cmp.reverse() } else { cmp };
    directed
        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then(a.id.cmp(&b.id))
}

fn capture_date(asset: &Asset) -> Option<&str> {
    asset.exif_date.as_deref().filter(|date| !date.is_empty())
}

pub(super) struct CatalogAssetFilter {
    pub(super) with_sql: String,
    pub(super) from_sql: String,
    pub(super) where_sql: String,
    pub(super) args: Vec<rusqlite::types::Value>,
}

pub(super) fn catalog_asset_filter(
    conn: &Connection,
    filter: &AssetFilter,
    formats: &FormatTable,
) -> rusqlite::Result<CatalogAssetFilter> {
    let ai_attached = filter.kind == "color"
        && conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM ai.sqlite_master WHERE type='table' AND name='colors')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false);
    let quality_attached = filter.kind == "quality"
        && conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM ai.sqlite_master WHERE type='table' AND name='quality')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false);
    let mut from_sql = String::from("assets a");
    let mut with_sql = String::new();
    let mut where_sql = format!(
        "a.is_listed=1 AND {}",
        formats.sql_not_in("a.ext_lower", &formats.hidden_extensions())
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some((cte, root_or_node_id)) = descendant_assets_cte(filter) {
        with_sql = cte;
        args.push(root_or_node_id);
        from_sql = "matching_assets m CROSS JOIN assets a".into();
        where_sql.push_str(" AND a.id=m.asset_id");
    }
    match filter.kind.as_str() {
        "auto" => {
            let pair = filter.value.as_deref().and_then(|v| v.split_once('\t'));
            if let Some((group, field, value)) =
                pair.and_then(|(group, value)| crate::exif_index::field(group).map(|field| (group, field, value)))
            {
                where_sql.push_str(&format!(
                    " AND a.id IN (SELECT asset_id FROM auto_exif.exif_index WHERE {field}=?)"
                ));
                match group {
                    "iso" | "focal" | "exposure" | "bucket" => args.push(value.parse::<i64>().unwrap_or(-1).into()),
                    "aperture" => args.push(value.parse::<f64>().unwrap_or(-1.0).into()),
                    _ => args.push(value.to_string().into()),
                }
            } else {
                where_sql.push_str(" AND 0");
            }
        }
        "folder" => {
            if let Some(ids) = filter.ids.as_ref().filter(|ids| !ids.is_empty()) {
                where_sql.push_str(&format!(" AND a.folder_id IN ({})", vec!["?"; ids.len()].join(",")));
                args.extend(ids.iter().copied().map(Into::into));
            } else {
                where_sql.push_str(" AND a.folder_id=?");
                args.push(filter.id.unwrap_or(-1).into());
            }
        }
        "category" => {
            if with_sql.is_empty() {
                from_sql = String::from(
                    "asset_categories ac INDEXED BY idx_asset_categories_category_asset JOIN assets a ON a.id=ac.asset_id",
                );
                where_sql.push_str(" AND ac.category_id=?");
                args.push(filter.id.unwrap_or(-1).into());
            }
        }
        "keyword" => {
            if with_sql.is_empty() {
                from_sql = String::from(
                    "asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset JOIN assets a ON a.id=ak.asset_id",
                );
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
            // One colour or several separated by commas; a photo must contain all of them.
            let colors: Vec<&str> = filter
                .value
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect();
            if colors.is_empty() {
                where_sql.push_str(" AND 0");
            }
            for color in colors {
                if !crate::colors::PALETTE.iter().any(|entry| entry.value == color) {
                    where_sql.push_str(" AND 0");
                } else if !ai_attached {
                    where_sql.push_str(" AND EXISTS(SELECT 1 FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=? AND o.state=1)");
                    args.push(color.to_owned().into());
                } else {
                    where_sql.push_str(" AND (EXISTS(SELECT 1 FROM ai.colors c,json_each(c.auto) j WHERE c.asset_id=a.id AND j.value=? AND coalesce((SELECT o.state FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=?),1)=1) OR EXISTS(SELECT 1 FROM asset_color_overrides o WHERE o.asset_id=a.id AND o.color=? AND o.state=1))");
                    args.extend([
                        color.to_owned().into(),
                        color.to_owned().into(),
                        color.to_owned().into(),
                    ]);
                }
            }
        }
        "quality" => match filter.value.as_deref().and_then(crate::quality::flag_sql) {
            Some(fragment) if quality_attached => where_sql.push_str(&format!(
                " AND EXISTS(SELECT 1 FROM ai.quality q WHERE q.asset_id=a.id AND q.version={} AND ({fragment}))",
                crate::quality::QUALITY_VERSION
            )),
            _ => where_sql.push_str(" AND 0"),
        },
        "special" => match filter.value.as_deref().unwrap_or("") {
            "all_photos" => where_sql.push_str(&format!(" AND {}", formats.sql_in("a.ext_lower", &["image", "raw"]))),
            "uncategorized" => where_sql.push_str(&format!(
                " AND {} AND NOT EXISTS(SELECT 1 FROM asset_categories acx WHERE acx.asset_id=a.id)",
                formats.sql_in("a.ext_lower", &["image", "raw"])
            )),
            "unkeyworded" => where_sql.push_str(&format!(
                " AND {} AND NOT EXISTS(SELECT 1 FROM asset_keywords akx WHERE akx.asset_id=a.id)",
                formats.sql_in("a.ext_lower", &["image", "raw"])
            )),
            "tagged" => where_sql.push_str(&format!(
                " AND {} AND coalesce(a.tagged,0)<>0",
                formats.sql_in("a.ext_lower", &["image", "raw"])
            )),
            "delete_suggested" if crate::incoming::has_deletion_column(conn)? => {
                where_sql.push_str(" AND a.delete_suggestion=1")
            }
            "quarantine" if crate::schema::has_asset_column(conn, "quarantined_at")? => {
                where_sql.push_str(" AND a.quarantined_at IS NOT NULL AND a.quarantine_origin IS NOT NULL")
            }
            "edit_pending" if crate::schema::has_asset_column(conn, "edit_pending_since")? => {
                where_sql.push_str(" AND a.edit_pending_since IS NOT NULL")
            }
            _ => where_sql.push_str(" AND 0"),
        },
        "quarantine_review" => {
            where_sql.push_str(" AND a.id IN (SELECT value FROM json_each(?))");
            args.push(
                serde_json::to_string(&filter.ids.as_deref().unwrap_or_default())
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .into(),
            );
        }
        "date" => add_date_filter(&mut where_sql, &mut args, filter, formats),
        _ => {
            where_sql.push_str(" AND 0");
        }
    }
    if let Some(q) = filter.query.as_ref().filter(|q| !q.trim().is_empty()) {
        where_sql.push_str(
            " AND (a.name LIKE ? ESCAPE '\\' OR coalesce(a.caption,'') LIKE ? ESCAPE '\\' OR coalesce(a.notes,'') LIKE ? ESCAPE '\\')",
        );
        let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        args.push(format!("%{escaped}%").into());
        args.push(format!("%{escaped}%").into());
        args.push(format!("%{escaped}%").into());
    }
    add_view_filters(&mut where_sql, &mut args, filter, formats);
    Ok(CatalogAssetFilter {
        with_sql,
        from_sql,
        where_sql,
        args,
    })
}
