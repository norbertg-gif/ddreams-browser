use super::open::Db;
use crate::formats::FormatTable;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueCount {
    pub value: String,
    pub count: i64,
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
            let photos = formats.sql_in("a.ext_lower", &["image", "raw"]);
            let hidden = formats.sql_not_in("a.ext_lower", &formats.hidden_extensions());
            let suggested = if crate::incoming::has_deletion_column(&conn)? {
                format!(
                    " UNION ALL SELECT 'delete_suggested' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND a.delete_suggestion=1"
                )
            } else {
                String::new()
            };
            let quarantine = if crate::schema::has_asset_column(&conn, "quarantined_at")? {
                format!(
                    " UNION ALL SELECT 'quarantine' FROM assets a WHERE is_listed=1 AND {hidden} AND a.quarantined_at IS NOT NULL AND a.quarantine_origin IS NOT NULL"
                )
            } else {
                String::new()
            };
            let edit_pending = if crate::schema::has_asset_column(&conn, "edit_pending_since")? {
                format!(
                    " UNION ALL SELECT 'edit_pending' FROM assets a WHERE is_listed=1 AND {hidden} AND a.edit_pending_since IS NOT NULL"
                )
            } else {
                String::new()
            };
            let sql = format!(
                "SELECT v,count(*) FROM (SELECT 'all_photos' v FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} UNION ALL SELECT 'uncategorized' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND NOT EXISTS(SELECT 1 FROM asset_categories ac WHERE ac.asset_id=a.id) UNION ALL SELECT 'unkeyworded' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND NOT EXISTS(SELECT 1 FROM asset_keywords ak WHERE ak.asset_id=a.id) UNION ALL SELECT 'tagged' FROM assets a WHERE is_listed=1 AND {hidden} AND {photos} AND coalesce(tagged,0)<>0{suggested}{quarantine}{edit_pending}) GROUP BY v ORDER BY v LIMIT ? OFFSET ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            return stmt
                .query_map(params![limit.clamp(1, 500), offset.max(0)], |r| {
                    Ok(ValueCount {
                        value: r.get(0)?,
                        count: r.get(1)?,
                    })
                })?
                .collect();
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
