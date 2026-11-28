use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

use super::CatalogWriter;
use super::history::record_change;
use super::writer::{AssetIdRemap, ListedAsset, resolve_write_assets};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct AssetValueState {
    pub id: i64,
    pub rating: Option<i64>,
    pub label: Option<String>,
    pub tagged: bool,
    pub edit_pending_since: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct SetAssetValuesResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub remapped_ids: Vec<AssetIdRemap>,
    pub assets: Vec<AssetValueState>,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub enum AssetTextField {
    Caption,
    Author,
}

impl AssetTextField {
    fn settings(self) -> (&'static str, &'static str, usize) {
        match self {
            Self::Caption => ("caption", "Popis", 2000),
            Self::Author => ("author", "Autor", 200),
        }
    }

    pub(crate) fn normalize(self, value: Option<String>) -> Result<Option<String>, String> {
        let (column, label, limit) = self.settings();
        let value = value
            .map(|value| value.replace("\r\n", "\n").replace('\r', "\n"))
            .filter(|value| !value.trim().is_empty());
        if let Some(value) = &value {
            if value.chars().count() > limit {
                return Err(format!("{label} môže mať najviac {limit} znakov"));
            }
            if value.contains('\0') || (column == "author" && value.contains('\n')) {
                return Err(format!("{label} obsahuje nepovolený znak"));
            }
        }
        Ok(value)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct SetAssetTextResult {
    #[serde(flatten)]
    pub(super) result: SetAssetValuesResult,
    pub(super) value: Option<String>,
}

#[derive(Default)]
struct AssetUpdates {
    rating: Option<Option<i64>>,
    label: Option<Option<String>>,
    tagged: Option<bool>,
    tagged_toggle: bool,
    edit_pending: Option<bool>,
    edit_pending_toggle: bool,
}

impl AssetUpdates {
    fn is_empty(&self) -> bool {
        self.rating.is_none()
            && self.label.is_none()
            && self.tagged.is_none()
            && !self.tagged_toggle
            && self.edit_pending.is_none()
            && !self.edit_pending_toggle
    }
}

fn parse_asset_updates(values: &serde_json::Map<String, Value>) -> Result<AssetUpdates, String> {
    let mut result = AssetUpdates::default();
    for (key, value) in values {
        match key.as_str() {
            "rating" => {
                result.rating = Some(if value.is_null() {
                    None
                } else {
                    let rating = value.as_i64().ok_or("Rating must be 1 to 5 or null")?;
                    if !(1..=5).contains(&rating) {
                        return Err("Rating must be 1 to 5 or null".into());
                    }
                    Some(rating)
                });
            }
            "label" => {
                result.label = Some(if value.is_null() {
                    None
                } else {
                    let label = value.as_str().ok_or("Invalid colour label")?;
                    if !["Red", "Yellow", "Green", "Blue", "Purple", "Gray"].contains(&label) {
                        return Err("Invalid colour label".into());
                    }
                    Some(label.to_owned())
                });
            }
            "tagged" if value.as_str() == Some("toggle") => result.tagged_toggle = true,
            "tagged" => result.tagged = Some(value.as_bool().ok_or("Tagged state must be true, false, or 'toggle'")?),
            "editPending" if value.as_str() == Some("toggle") => result.edit_pending_toggle = true,
            "editPending" => result.edit_pending = Some(value.as_bool().ok_or("Neplatný stav úpravy")?),
            _ => return Err(format!("Unsupported asset value: {key}")),
        }
    }
    if result.tagged_toggle && result.tagged.is_some() {
        return Err("Tagged toggle cannot be combined with a fixed tagged value".into());
    }
    Ok(result)
}

fn update_summary_parts(updates: &AssetUpdates, tagged: Option<bool>) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(rating) = updates.rating {
        parts.push(
            rating
                .map(|value| format!("Hodnotenie {value} ★"))
                .unwrap_or_else(|| "Bez hodnotenia".into()),
        );
    }
    if let Some(label) = &updates.label {
        parts.push(
            label
                .as_ref()
                .map(|value| format!("Farebný štítok {}", label_name(value)))
                .unwrap_or_else(|| "Bez štítku".into()),
        );
    }
    if let Some(tagged) = tagged {
        parts.push(if tagged { "Označené" } else { "Zrušené označenie" }.into());
    }
    parts
}

fn label_name(value: &str) -> &str {
    match value {
        "Red" => "Červená",
        "Yellow" => "Žltá",
        "Green" => "Zelená",
        "Blue" => "Modrá",
        "Purple" => "Fialová",
        "Gray" => "Na vyradenie",
        _ => value,
    }
}

pub(super) fn slovak_count(count: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else if (2..=4).contains(&count) {
        few
    } else {
        many
    }
}

pub(super) fn read_asset_values(transaction: &Transaction<'_>, id: i64) -> Result<Option<AssetValueState>, String> {
    transaction
        .query_row(
            "SELECT id,rating,label,coalesce(tagged,0),ext_lower,is_listed FROM assets WHERE id=?1",
            [id],
            |row| {
                let id = row.get(0)?;
                let rating = row.get(1)?;
                let label = row.get(2)?;
                let tagged = row.get::<_, i64>(3)? != 0;
                let ext: String = row.get(4)?;
                let listed: i64 = row.get(5)?;
                Ok((
                    ext,
                    listed,
                    AssetValueState {
                        id,
                        rating,
                        label,
                        tagged,
                        edit_pending_since: None,
                    },
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())
        .map(|row| {
            row.filter(|(ext, listed, _)| !ext.is_empty() && *listed != 0)
                .map(|(_, _, state)| state)
        })
}

pub(super) fn update_column(
    tx: &Transaction<'_>,
    table: &str,
    id: i64,
    column: &str,
    value: impl rusqlite::ToSql,
) -> Result<bool, String> {
    let count = tx
        .execute(
            &format!("UPDATE {table} SET {column}=?1 WHERE id=?2 AND {column} IS NOT ?1"),
            (value, id),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

impl CatalogWriter {
    pub fn set_asset_colors(
        &self,
        asset_ids: &[i64],
        color: &str,
        state: Option<bool>,
        summary: &str,
    ) -> Result<usize, String> {
        const PALETTE: [&str; 13] = [
            "červená",
            "oranžová",
            "hnedá",
            "žltá",
            "zelená",
            "tyrkysová",
            "modrá",
            "fialová",
            "ružová",
            "biela",
            "sivá",
            "čierna",
            "čiernobiela",
        ];
        self.require_write()?;
        if !PALETTE.contains(&color) {
            return Err("Neznáma farba".into());
        }
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, &[])?;
        let state = state.map(i64::from);
        let mut count = 0;
        let mut seen = std::collections::HashSet::new();
        for id in asset_ids.iter().copied().filter(|id| *id > 0 && seen.insert(*id)) {
            let listed: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !listed {
                continue;
            }
            let old: Option<i64> = tx
                .query_row(
                    "SELECT state FROM asset_color_overrides WHERE asset_id=?1 AND color=?2",
                    (id, color),
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if old == state {
                continue;
            }
            if let Some(state) = state {
                tx.execute(
                    "INSERT OR REPLACE INTO asset_color_overrides(asset_id,color,state) VALUES(?1,?2,?3)",
                    (id, color, state),
                )
            } else {
                tx.execute(
                    "DELETE FROM asset_color_overrides WHERE asset_id=?1 AND color=?2",
                    (id, color),
                )
            }
            .map_err(|e| e.to_string())?;
            count += 1;
        }
        if count > 0 {
            record_change(&tx, "set_asset_colors", summary)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub fn set_asset_values(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        changes: &serde_json::Map<String, Value>,
    ) -> Result<SetAssetValuesResult, String> {
        self.require_write()?;
        let updates = parse_asset_updates(changes)?;
        if updates.is_empty() {
            return Err("No asset values were supplied".into());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let insert_missing = updates.edit_pending != Some(false)
            || updates.rating.is_some()
            || updates.label.is_some()
            || updates.tagged.is_some()
            || updates.tagged_toggle;
        let (mut targets, remapped_ids, inserted) =
            resolve_write_assets(&tx, asset_ids, listed_assets, insert_missing, true, |tx, id, path| {
                self.track_incoming(tx, id, path)
            })?;
        if targets.is_empty() {
            return Err("No files were selected".into());
        }
        let toggle_tagged_to = if updates.tagged_toggle {
            Some(targets.iter().any(|asset| !asset.tagged))
        } else {
            updates.tagged
        };
        let mut pending_to = updates.edit_pending;
        let mut pending_targets = HashMap::new();
        if updates.edit_pending_toggle || pending_to.is_some() {
            let mut stmt = tx
                .prepare("SELECT edit_pending_since FROM assets WHERE id=?1 AND quarantined_at IS NULL")
                .map_err(|e| e.to_string())?;
            for target in &targets {
                if let Some(since) = stmt
                    .query_row([target.id], |r| r.get::<_, Option<i64>>(0))
                    .optional()
                    .map_err(|e| e.to_string())?
                {
                    pending_targets.insert(target.id, since);
                }
            }
            if pending_targets.is_empty()
                && updates.rating.is_none()
                && updates.label.is_none()
                && toggle_tagged_to.is_none()
            {
                return Err("Vyberte súbory mimo karantény".into());
            }
            if updates.edit_pending_toggle {
                pending_to = Some(pending_targets.values().any(Option::is_none));
            }
        }
        let marked_at = pending_to.filter(|pending| *pending).map(|_| crate::quarantine::now());
        let mut changed = inserted > 0;
        for target in &mut targets {
            if let Some(rating) = updates.rating {
                changed |= update_column(&tx, "assets", target.id, "rating", rating)?;
                target.rating = rating;
            }
            if let Some(label) = &updates.label {
                changed |= update_column(&tx, "assets", target.id, "label", label.as_deref())?;
                target.label = label.clone();
            }
            if let Some(tagged) = toggle_tagged_to {
                changed |= update_column(&tx, "assets", target.id, "tagged", i64::from(tagged))?;
                target.tagged = tagged;
            }
            if let Some(&since) = pending_targets.get(&target.id) {
                let next = marked_at.map(|now| since.unwrap_or(now));
                changed |= update_column(&tx, "assets", target.id, "edit_pending_since", next)?;
                target.edit_pending_since = next;
            }
        }

        let mut action_parts = update_summary_parts(&updates, toggle_tagged_to);
        if let Some(pending) = pending_to {
            action_parts.push(if pending { "Na úpravu" } else { "Hotovo" }.into());
        }
        let file_word = slovak_count(targets.len(), "súbor", "súbory", "súborov");
        let summary = format!("{} · {} {}", action_parts.join(" · "), targets.len(), file_word);
        let change_id = if changed {
            record_change(&tx, "set_asset_values", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(SetAssetValuesResult {
            change_id,
            summary,
            remapped_ids,
            assets: targets,
        })
    }

    pub fn set_asset_text(
        &self,
        asset_ids: &[i64],
        listed: &[ListedAsset],
        field: AssetTextField,
        value: Option<String>,
    ) -> Result<SetAssetTextResult, String> {
        self.require_write()?;
        let (column, label, _) = field.settings();
        let value = field.normalize(value)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, listed)?;
        let (assets, remapped_ids, inserted) =
            resolve_write_assets(&tx, asset_ids, listed, value.is_some(), true, |tx, id, path| {
                self.track_incoming(tx, id, path)
            })?;
        let mut changed = inserted > 0;
        for asset in &assets {
            changed |= update_column(&tx, "assets", asset.id, column, value.as_deref())?;
        }
        let summary = format!(
            "{label} · {} {}",
            assets.len(),
            slovak_count(assets.len(), "súbor", "súbory", "súborov")
        );
        let change_id = if changed {
            record_change(&tx, "set_asset_text", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(SetAssetTextResult {
            result: SetAssetValuesResult {
                change_id,
                summary,
                remapped_ids,
                assets,
            },
            value,
        })
    }

    pub(crate) fn check_iptc_edit(&self, id: i64, listed: &[ListedAsset]) -> Result<(), String> {
        self.require_write()?;
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        self.rules.guard(&conn, &[id], listed)
    }
}
