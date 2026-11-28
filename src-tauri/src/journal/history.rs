use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

use super::CatalogWriter;
use super::values::update_column;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub id: i64,
    pub ts: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionSuggestionChange {
    pub change_id: Option<i64>,
    pub summary: String,
}

pub(super) fn record_change(tx: &Transaction<'_>, action: &str, summary: &str) -> Result<Option<i64>, String> {
    tx.execute(
        "INSERT INTO ddreams_changes(ts,action,summary) VALUES(?1,?2,?3)",
        (iso_now(), action, summary),
    )
    .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    mark_catalog_modified(tx)?;
    Ok(Some(id))
}

pub(super) fn mark_catalog_modified(transaction: &Transaction<'_>) -> Result<(), String> {
    transaction
        .execute("UPDATE meta SET value='1' WHERE key='catalog_modified'", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn iso_now() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = (duration.as_secs() / 86_400) as i64;
    let second_of_day = duration.as_secs() % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        second_of_day / 60 % 60,
        second_of_day % 60
    )
}

impl CatalogWriter {
    pub fn suggest_deletions(
        &self,
        incoming: &str,
        suggestions: &[(i64, bool)],
        formats: &crate::formats::FormatTable,
    ) -> Result<DeletionSuggestionChange, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut changed = 0;
        {
            let mut select = tx
                .prepare("SELECT path,ext_lower,delete_suggestion FROM assets WHERE id=?1 AND is_listed=1")
                .map_err(|e| e.to_string())?;
            for &(id, weak) in suggestions {
                let row = select
                    .query_row([id], |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                        ))
                    })
                    .optional()
                    .map_err(|e| e.to_string())?;
                let Some((Some(path), ext, current)) = row else {
                    continue;
                };
                if !crate::incoming::is_inside(&path, incoming) || !matches!(formats.group(&ext), Some("image" | "raw"))
                {
                    continue;
                }
                // NULL: eligible, 0: explicitly kept, 1: suggested. A new best is eligible again.
                let new_value = match (weak, current) {
                    (true, None) => SqlValue::Integer(1),
                    (false, Some(1)) => SqlValue::Null,
                    _ => continue,
                };
                changed += usize::from(update_column(&tx, "assets", id, "delete_suggestion", new_value)?);
            }
        }
        let summary = if changed == 0 {
            "Žiadne nové návrhy na vymazanie".into()
        } else {
            format!("Zmenené návrhy na vymazanie ({changed})")
        };
        let change_id = if changed > 0 {
            record_change(&tx, "suggest_deletions", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(DeletionSuggestionChange { change_id, summary })
    }

    pub fn keep_suggested_assets(&self, selection: Option<&[i64]>) -> Result<DeletionSuggestionChange, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let selection = selection.map(|ids| ids.iter().copied().collect::<std::collections::HashSet<_>>());
        let active = {
            let mut select = tx
                .prepare("SELECT id FROM assets WHERE delete_suggestion=1 AND is_listed=1 ORDER BY id")
                .map_err(|e| e.to_string())?;
            select
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?
        };
        let mut changed = 0;
        for id in active
            .into_iter()
            .filter(|id| selection.as_ref().is_none_or(|ids| ids.contains(id)))
        {
            changed += usize::from(update_column(&tx, "assets", id, "delete_suggestion", 0)?);
        }
        let summary = if changed == 0 {
            "Žiadne návrhy na vymazanie".into()
        } else {
            format!("Zrušené návrhy na vymazanie ({changed})")
        };
        let change_id = if changed > 0 {
            record_change(&tx, "keep_suggested_assets", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(DeletionSuggestionChange { change_id, summary })
    }

    pub fn recent_changes(&self, limit: i64) -> Result<Vec<ChangeSummary>, String> {
        let connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let mut statement = connection
            .prepare("SELECT id,ts,summary FROM ddreams_changes WHERE undone=0 ORDER BY id DESC LIMIT ?1")
            .map_err(|e| e.to_string())?;
        statement
            .query_map([limit.clamp(1, 200)], |row| {
                Ok(ChangeSummary {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    summary: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }
}
