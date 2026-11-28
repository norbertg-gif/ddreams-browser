use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;

use super::CatalogWriter;
use super::categories::set_asset_links;
use super::folders::delete_row;
use super::history::record_change;
use super::values::{slovak_count, update_column};
use super::writer::{AssetIdRemap, ListedAsset, next_id, resolve_write_assets};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAssetKeywordsResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub changed_count: usize,
    pub remapped_ids: Vec<AssetIdRemap>,
    pub keyword_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub ancestor_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordMutationResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub keyword_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub ancestor_ids: Vec<i64>,
    pub deleted_keyword_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordDeleteInfo {
    pub id: i64,
    pub name: String,
    pub assigned_files: usize,
    pub subkeywords: usize,
}

fn validated_keyword_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("Názov kľúčového slova nesmie byť prázdny".into());
    }
    if name.chars().count() > 100 {
        return Err("Názov kľúčového slova môže mať najviac 100 znakov".into());
    }
    Ok(name.to_owned())
}

fn keyword_exists(tx: &Transaction<'_>, id: i64) -> Result<bool, String> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM keywords WHERE id=?1)", [id], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn ensure_unique_keyword_name(
    tx: &Transaction<'_>,
    parent: Option<i64>,
    name: &str,
    exclude: Option<i64>,
) -> Result<(), String> {
    let mut stmt = tx
        .prepare("SELECT id,name FROM keywords WHERE parent_id IS ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([parent], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let normalized = name.trim().to_lowercase();
    if rows
        .iter()
        .any(|(id, value)| Some(*id) != exclude && value.trim().to_lowercase() == normalized)
    {
        return Err("Na tejto úrovni už existuje kľúčové slovo s týmto názvom".into());
    }
    Ok(())
}

fn keyword_subtree(tx: &Transaction<'_>, id: i64) -> Result<Vec<i64>, String> {
    let mut stmt=tx.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM keywords WHERE id=?1 UNION SELECT k.id FROM keywords k JOIN subtree s ON k.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    if ids.is_empty() {
        return Err("Kľúčové slovo neexistuje".into());
    }
    Ok(ids)
}

fn keyword_ancestor_ids(tx: &Transaction<'_>, id: i64) -> Result<Vec<i64>, String> {
    let mut current = tx
        .query_row("SELECT parent_id FROM keywords WHERE id=?1", [id], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Kľúčové slovo neexistuje".to_string())?;
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = current {
        if !seen.insert(parent) {
            break;
        }
        result.push(parent);
        current = tx
            .query_row("SELECT parent_id FROM keywords WHERE id=?1", [parent], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
    }
    Ok(result)
}

fn keyword_path(tx: &Transaction<'_>, id: i64) -> Result<String, String> {
    Ok(keyword_components(tx, id)?.join(" › "))
}

pub(crate) fn keyword_components(tx: &rusqlite::Connection, id: i64) -> Result<Vec<String>, String> {
    let mut current = Some(id);
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    while let Some(key) = current {
        if !seen.insert(key) {
            break;
        }
        let row: Option<(String, Option<i64>)> = tx
            .query_row("SELECT name,parent_id FROM keywords WHERE id=?1", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((name, parent)) = row else {
            if key == id {
                return Err("Kľúčové slovo neexistuje".into());
            }
            break;
        };
        names.push(name);
        current = parent;
    }
    names.reverse();
    Ok(names)
}

fn keyword_delete_info(connection: &rusqlite::Connection, id: i64) -> Result<KeywordDeleteInfo, String> {
    let name: String = connection
        .query_row("SELECT name FROM keywords WHERE id=?1", [id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Kľúčové slovo neexistuje".to_string())?;
    let mut stmt=connection.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM keywords WHERE id=?1 UNION SELECT k.id FROM keywords k JOIN subtree s ON k.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let marks = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!("SELECT count(DISTINCT asset_id) FROM asset_keywords WHERE keyword_id IN ({marks})");
    let assigned = connection
        .query_row(&sql, rusqlite::params_from_iter(ids.iter()), |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())? as usize;
    Ok(KeywordDeleteInfo {
        id,
        name,
        assigned_files: assigned,
        subkeywords: ids.len().saturating_sub(1),
    })
}

impl CatalogWriter {
    pub fn set_asset_keywords(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        keyword_id: i64,
        assign: bool,
    ) -> Result<SetAssetKeywordsResult, String> {
        self.require_write()?;
        if asset_ids.is_empty() {
            return Err("Nie sú vybrané žiadne súbory".into());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let (name, parent_id): (String, Option<i64>) = tx
            .query_row("SELECT name,parent_id FROM keywords WHERE id=?1", [keyword_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("Kľúčové slovo neexistuje")?;
        let (targets, remapped_ids, inserted) =
            resolve_write_assets(&tx, asset_ids, listed_assets, assign, false, |tx, id, path| {
                self.track_incoming(tx, id, path)
            })?;
        let changed = set_asset_links(&tx, "asset_keywords", "keyword_id", &targets, keyword_id, assign)?;
        if changed < inserted {
            return Err("Nový súbor sa nepodarilo priradiť ku kľúčovému slovu".into());
        }
        let summary = if changed == 0 {
            format!("Žiadne zmeny pri kľúčovom slove {name}")
        } else {
            let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
            format!(
                "{} kľúčové slovo {name} · {changed} {file_word}",
                if assign { "Pridané" } else { "Odobrané" }
            )
        };
        let ancestor_ids = keyword_ancestor_ids(&tx, keyword_id)?;
        let change_id = if changed > 0 {
            record_change(&tx, "set_asset_keywords", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        if changed > 0 {
            self.notify_labels_changed(&["keyword"]);
        }
        Ok(SetAssetKeywordsResult {
            change_id,
            summary,
            changed_count: changed,
            remapped_ids,
            keyword_id: Some(keyword_id),
            parent_id,
            ancestor_ids,
        })
    }

    pub fn create_keyword(&self, parent_id: Option<i64>, name: &str) -> Result<KeywordMutationResult, String> {
        self.require_write()?;
        let name = validated_keyword_name(name)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if let Some(parent) = parent_id {
            if !keyword_exists(&tx, parent)? {
                return Err("Nadradené kľúčové slovo neexistuje".into());
            }
        }
        ensure_unique_keyword_name(&tx, parent_id, &name, None)?;
        let id = next_id(&tx, "keywords")?;
        tx.execute(
            "INSERT INTO keywords(id,parent_id,name) VALUES(?1,?2,?3)",
            (id, parent_id, &name),
        )
        .map_err(|e| e.to_string())?;
        let summary = if let Some(parent) = parent_id {
            format!("Nové kľúčové slovo {} › {name}", keyword_path(&tx, parent)?)
        } else {
            format!("Nové kľúčové slovo {name}")
        };
        let ancestors = keyword_ancestor_ids(&tx, id)?;
        let change_id = record_change(&tx, "create_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(KeywordMutationResult {
            change_id,
            summary,
            keyword_id: Some(id),
            parent_id,
            ancestor_ids: ancestors,
            deleted_keyword_ids: Vec::new(),
        })
    }

    pub fn create_and_assign_keyword(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        parent_id: Option<i64>,
        name: &str,
    ) -> Result<SetAssetKeywordsResult, String> {
        self.require_write()?;
        let name = validated_keyword_name(name)?;
        if asset_ids.is_empty() {
            return Err("Nie sú vybrané žiadne súbory".into());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if let Some(parent) = parent_id {
            if !keyword_exists(&tx, parent)? {
                return Err("Nadradené kľúčové slovo neexistuje".into());
            }
        }
        ensure_unique_keyword_name(&tx, parent_id, &name, None)?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let keyword_id = next_id(&tx, "keywords")?;
        tx.execute(
            "INSERT INTO keywords(id,parent_id,name) VALUES(?1,?2,?3)",
            (keyword_id, parent_id, &name),
        )
        .map_err(|e| e.to_string())?;
        let (targets, remapped_ids, _) =
            resolve_write_assets(&tx, asset_ids, listed_assets, true, false, |tx, id, path| {
                self.track_incoming(tx, id, path)
            })?;
        let changed = set_asset_links(&tx, "asset_keywords", "keyword_id", &targets, keyword_id, true)?;
        if changed == 0 {
            return Err("Nie je možné vytvoriť kľúčové slovo bez platných súborov".into());
        }
        let ancestor_ids = keyword_ancestor_ids(&tx, keyword_id)?;
        let path = keyword_path(&tx, keyword_id)?;
        let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
        let summary = format!("Nové kľúčové slovo {path} a priradené · {changed} {file_word}");
        let change_id = record_change(&tx, "create_assign_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(SetAssetKeywordsResult {
            change_id,
            summary,
            changed_count: changed,
            remapped_ids,
            keyword_id: Some(keyword_id),
            parent_id,
            ancestor_ids,
        })
    }

    pub fn rename_keyword(&self, id: i64, name: &str) -> Result<KeywordMutationResult, String> {
        self.require_write()?;
        let name = validated_keyword_name(name)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (old, parent): (String, Option<i64>) = tx
            .query_row("SELECT name,parent_id FROM keywords WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Kľúčové slovo neexistuje".to_string())?;
        ensure_unique_keyword_name(&tx, parent, &name, Some(id))?;
        let ancestors = keyword_ancestor_ids(&tx, id)?;
        if old == name {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(KeywordMutationResult {
                change_id: None,
                summary: format!("Kľúčové slovo už má názov {old}"),
                keyword_id: Some(id),
                parent_id: parent,
                ancestor_ids: ancestors,
                deleted_keyword_ids: Vec::new(),
            });
        }
        update_column(&tx, "keywords", id, "name", SqlValue::Text(name.clone()))?;
        let summary = format!("Premenované {old} → {name}");
        let change_id = record_change(&tx, "rename_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(KeywordMutationResult {
            change_id,
            summary,
            keyword_id: Some(id),
            parent_id: parent,
            ancestor_ids: ancestors,
            deleted_keyword_ids: Vec::new(),
        })
    }

    pub fn move_keyword(&self, id: i64, new_parent_id: Option<i64>) -> Result<KeywordMutationResult, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (name, old_parent): (String, Option<i64>) = tx
            .query_row("SELECT name,parent_id FROM keywords WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Kľúčové slovo neexistuje".to_string())?;
        let subtree = keyword_subtree(&tx, id)?;
        if new_parent_id.is_some_and(|parent| subtree.contains(&parent)) {
            return Err("Kľúčové slovo nemožno presunúť do seba ani do jeho podriadeného slova".into());
        }
        if let Some(parent) = new_parent_id {
            if !keyword_exists(&tx, parent)? {
                return Err("Cieľové kľúčové slovo neexistuje".into());
            }
        }
        ensure_unique_keyword_name(&tx, new_parent_id, &name, Some(id))?;
        if old_parent == new_parent_id {
            let ancestors = keyword_ancestor_ids(&tx, id)?;
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(KeywordMutationResult {
                change_id: None,
                summary: format!("Kľúčové slovo {name} je už na tomto mieste"),
                keyword_id: Some(id),
                parent_id: old_parent,
                ancestor_ids: ancestors,
                deleted_keyword_ids: Vec::new(),
            });
        }
        update_column(
            &tx,
            "keywords",
            id,
            "parent_id",
            new_parent_id.map(SqlValue::Integer).unwrap_or(SqlValue::Null),
        )?;
        let summary = if let Some(parent) = new_parent_id {
            format!("Presunuté {name} do {}", keyword_path(&tx, parent)?)
        } else {
            format!("Presunuté {name} na najvyššiu úroveň")
        };
        let ancestors = keyword_ancestor_ids(&tx, id)?;
        let change_id = record_change(&tx, "move_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(KeywordMutationResult {
            change_id,
            summary,
            keyword_id: Some(id),
            parent_id: new_parent_id,
            ancestor_ids: ancestors,
            deleted_keyword_ids: Vec::new(),
        })
    }

    pub fn keyword_delete_info(&self, id: i64) -> Result<KeywordDeleteInfo, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        keyword_delete_info(&conn, id)
    }

    pub fn delete_keyword(&self, id: i64) -> Result<KeywordMutationResult, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let info = keyword_delete_info(&tx, id)?;
        if info.assigned_files > 0 {
            return Err(format!(
                "Kľúčové slovo {} nie je možné zmazať, obsahuje {} {} (vrátane podriadených slov). Najprv z neho súbory odoberte.",
                info.name,
                info.assigned_files,
                slovak_count(info.assigned_files, "súbor", "súbory", "súborov")
            ));
        }
        let ancestors = keyword_ancestor_ids(&tx, id)?;
        let ids = keyword_subtree(&tx, id)?;
        for keyword_id in ids.iter().rev().copied() {
            delete_row(&tx, "keywords", keyword_id)?;
        }
        let summary = format!("Zmazané kľúčové slovo {}", info.name);
        let change_id = record_change(&tx, "delete_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(KeywordMutationResult {
            change_id,
            summary,
            keyword_id: None,
            parent_id: None,
            ancestor_ids: ancestors,
            deleted_keyword_ids: ids,
        })
    }
}
