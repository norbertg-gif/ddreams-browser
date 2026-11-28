use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;

use super::CatalogWriter;
use super::folders::delete_row;
use super::history::record_change;
use super::values::{AssetValueState, slovak_count, update_column};
use super::writer::{AssetIdRemap, ListedAsset, next_id, resolve_write_assets};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct SetAssetCategoriesResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub changed_count: usize,
    pub remapped_ids: Vec<AssetIdRemap>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct CategoryMutationResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub category_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub root_id: Option<i64>,
    pub ancestor_ids: Vec<i64>,
    pub deleted_category_ids: Vec<i64>,
    pub deleted_root_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct CategoryDeleteInfo {
    pub id: i64,
    pub name: String,
    pub assigned_files: usize,
    pub subcategories: usize,
}

fn category_path(transaction: &Transaction<'_>, category_id: i64) -> Result<String, String> {
    let (root_id, root_name): (Option<i64>, Option<String>) = transaction
        .query_row(
            "SELECT c.root_id, r.name FROM categories c LEFT JOIN category_roots r ON r.id=c.root_id WHERE c.id=?1",
            [category_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Category {category_id} not found"))?;
    let mut names = Vec::new();
    let mut current = Some(category_id);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        let row: Option<(String, Option<i64>)> = transaction
            .query_row("SELECT name,parent_id FROM categories WHERE id=?1", [id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((name, parent)) = row else {
            break;
        };
        names.push(name);
        current = parent;
    }
    names.reverse();
    if let Some(root) = root_name {
        if !names
            .first()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case(root.trim()))
        {
            names.insert(0, root);
        }
    } else if root_id.is_some() && names.is_empty() {
        return Err("Category path is unavailable".into());
    }
    Ok(names.join(" › "))
}

fn validate_category_icon(icon_key: Option<&str>) -> Result<(), String> {
    match icon_key {
        None
        | Some(
            "album" | "people" | "places" | "various" | "event" | "travel" | "nature" | "animal" | "home" | "building"
            | "car" | "food" | "sport" | "music" | "work" | "school" | "star" | "heart" | "camera" | "folder",
        ) => Ok(()),
        Some(_) => Err("Neznáma ikona kategórie".into()),
    }
}

fn validated_category_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("Názov kategórie nesmie byť prázdny".into());
    }
    if name.chars().count() > 100 {
        return Err("Názov kategórie môže mať najviac 100 znakov".into());
    }
    Ok(name.to_owned())
}

fn ensure_unique_category_name(
    tx: &Transaction<'_>,
    parent: Option<i64>,
    _root: Option<i64>,
    name: &str,
    exclude_category: Option<i64>,
    exclude_root: Option<i64>,
) -> Result<(), String> {
    let mut existing = Vec::<(i64, String)>::new();
    if let Some(parent) = parent {
        let mut stmt = tx
            .prepare("SELECT id,name FROM categories WHERE parent_id=?1")
            .map_err(|e| e.to_string())?;
        existing.extend(
            stmt.query_map([parent], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?,
        );
    } else {
        let mut stmt = tx
            .prepare("SELECT id,name FROM categories WHERE parent_id IS NULL OR parent_id=0")
            .map_err(|e| e.to_string())?;
        existing.extend(
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?,
        );
    }
    if existing
        .iter()
        .any(|(id, other)| Some(*id) != exclude_category && other.trim().to_lowercase() == name.trim().to_lowercase())
    {
        return Err("V tejto úrovni už kategória s týmto názvom existuje".into());
    }
    if parent.is_none() {
        let mut stmt = tx
            .prepare("SELECT id,name FROM category_roots")
            .map_err(|e| e.to_string())?;
        let roots = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        if roots
            .iter()
            .any(|(id, other)| Some(*id) != exclude_root && other.trim().to_lowercase() == name.trim().to_lowercase())
        {
            return Err("Koreň kategórie s týmto názvom už existuje".into());
        }
    }
    Ok(())
}

fn category_subtree(tx: &Transaction<'_>, id: i64) -> Result<Vec<i64>, String> {
    let mut stmt=tx.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM categories WHERE id=?1 UNION SELECT c.id FROM categories c JOIN subtree s ON c.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    if ids.is_empty() {
        return Err("Kategória neexistuje".into());
    }
    Ok(ids)
}

fn category_ancestor_ids(tx: &Transaction<'_>, id: i64) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut current = tx
        .query_row("SELECT parent_id FROM categories WHERE id=?1", [id], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Kategória neexistuje".to_string())?;
    while let Some(parent) = current {
        if !seen.insert(parent) {
            break;
        }
        ids.push(parent);
        current = tx
            .query_row("SELECT parent_id FROM categories WHERE id=?1", [parent], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
    }
    Ok(ids)
}

fn category_delete_info(connection: &rusqlite::Connection, id: i64) -> Result<CategoryDeleteInfo, String> {
    let (name,): (String,) = connection
        .query_row("SELECT name FROM categories WHERE id=?1", [id], |r| Ok((r.get(0)?,)))
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Kategória neexistuje".to_string())?;
    let mut stmt=connection.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM categories WHERE id=?1 UNION SELECT c.id FROM categories c JOIN subtree s ON c.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let marks = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!("SELECT count(DISTINCT asset_id) FROM asset_categories WHERE category_id IN ({marks})");
    let assigned_files = connection
        .query_row(&sql, rusqlite::params_from_iter(ids.iter()), |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())? as usize;
    Ok(CategoryDeleteInfo {
        id,
        name,
        assigned_files,
        subcategories: ids.len().saturating_sub(1),
    })
}

pub(super) fn set_asset_links(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    targets: &[AssetValueState],
    label_id: i64,
    assign: bool,
) -> Result<usize, String> {
    let sql = if assign {
        format!("INSERT OR IGNORE INTO {table}(asset_id,{column}) VALUES(?1,?2)")
    } else {
        format!("DELETE FROM {table} WHERE asset_id=?1 AND {column}=?2")
    };
    let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
    let mut changed = 0;
    for target in targets {
        changed += stmt.execute((target.id, label_id)).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

impl CatalogWriter {
    pub fn set_asset_categories(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        category_id: i64,
        assign: bool,
    ) -> Result<SetAssetCategoriesResult, String> {
        self.require_write()?;
        if asset_ids.is_empty() {
            return Err("No files were selected".into());
        }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let category_path = category_path(&tx, category_id)?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let (targets, remapped_ids, inserted) =
            resolve_write_assets(&tx, asset_ids, listed_assets, assign, false, |tx, id, path| {
                self.track_incoming(tx, id, path)
            })?;
        let changed = set_asset_links(&tx, "asset_categories", "category_id", &targets, category_id, assign)?;
        if changed < inserted {
            return Err("New catalog asset did not receive its category".into());
        }
        let summary = if changed == 0 {
            format!("Žiadne zmeny v kategórii {category_path}")
        } else {
            let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
            format!(
                "{} {category_path} · {changed} {file_word}",
                if assign { "Zaradené do" } else { "Vyradené z" }
            )
        };
        let change_id = if changed > 0 {
            record_change(&tx, "set_asset_categories", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        if changed > 0 {
            self.notify_labels_changed(&["category"]);
        }
        Ok(SetAssetCategoriesResult {
            change_id,
            summary,
            changed_count: changed,
            remapped_ids,
        })
    }

    pub fn create_category(
        &self,
        parent_id: Option<i64>,
        name: &str,
        icon_key: Option<&str>,
    ) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        let name = validated_category_name(name)?;
        validate_category_icon(icon_key)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (category_id, summary, root_id) = if let Some(parent) = parent_id {
            let root_id: Option<i64> = tx
                .query_row("SELECT root_id FROM categories WHERE id=?1", [parent], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Nadradená kategória neexistuje".to_string())?;
            ensure_unique_category_name(&tx, Some(parent), root_id, &name, None, None)?;
            let id = next_id(&tx, "categories")?;
            tx.execute(
                "INSERT INTO categories(id,parent_id,root_id,name,icon_key) VALUES(?1,?2,?3,?4,?5)",
                (id, parent, root_id, &name, icon_key),
            )
            .map_err(|e| e.to_string())?;

            let parent_path = category_path(&tx, parent)?;
            (id, format!("Nová kategória {parent_path} › {name}"), root_id)
        } else {
            ensure_unique_category_name(&tx, None, None, &name, None, None)?;
            let root_id = next_id(&tx, "category_roots")?;
            let id = next_id(&tx, "categories")?;
            tx.execute(
                "INSERT INTO category_roots(id,name,icon_key) VALUES(?1,?2,?3)",
                (root_id, &name, icon_key),
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO categories(id,parent_id,root_id,name,icon_key) VALUES(?1,NULL,?2,?3,?4)",
                (id, root_id, &name, icon_key),
            )
            .map_err(|e| e.to_string())?;

            (id, format!("Nová kategória {name}"), Some(root_id))
        };
        let ancestor_ids = category_ancestor_ids(&tx, category_id)?;
        let change_id = record_change(&tx, "create_category", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult {
            change_id,
            summary,
            category_id: Some(category_id),
            parent_id,
            root_id,
            ancestor_ids,
            deleted_category_ids: Vec::new(),
            deleted_root_ids: Vec::new(),
        })
    }

    pub fn set_category_icon(
        &self,
        id: i64,
        is_root: bool,
        icon_key: Option<&str>,
    ) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        validate_category_icon(icon_key)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let table = if is_root { "category_roots" } else { "categories" };
        let name: String = tx
            .query_row(&format!("SELECT name FROM {table} WHERE id=?1"), [id], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("Kategória neexistuje")?;
        let mut changed = update_column(&tx, table, id, "icon_key", icon_key)?;
        // Keep the merged category's icon when the category moves.
        if is_root {
            if let Some(paired) = crate::db::category_root_matches(&tx, id).map_err(|e| e.to_string())? {
                changed |= update_column(&tx, "categories", paired, "icon_key", icon_key)?;
            }
        }
        let summary = if !changed {
            format!("Ikona kategórie {name} je bez zmeny")
        } else {
            format!("Zmenená ikona kategórie {name}")
        };
        let change_id = if changed {
            record_change(&tx, "set_category_icon", &summary)?
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(CategoryMutationResult {
            change_id,
            summary,
            category_id: None,
            parent_id: None,
            root_id: None,
            ancestor_ids: Vec::new(),
            deleted_category_ids: Vec::new(),
            deleted_root_ids: Vec::new(),
        })
    }

    pub fn rename_category(&self, id: i64, name: &str) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        let name = validated_category_name(name)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (old, parent, root_id): (String, Option<i64>, Option<i64>) = tx
            .query_row("SELECT name,parent_id,root_id FROM categories WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Kategória neexistuje".to_string())?;
        let paired_root = if parent.is_none() {
            if let Some(root) = root_id {
                let root_name: Option<String> = tx
                    .query_row("SELECT name FROM category_roots WHERE id=?1", [root], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                if root_name.is_some_and(|value| value.trim().to_lowercase() == old.trim().to_lowercase()) {
                    Some(root)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        ensure_unique_category_name(&tx, parent, root_id, &name, Some(id), paired_root)?;
        if old == name {
            let path = category_path(&tx, id)?;
            let ancestor_ids = category_ancestor_ids(&tx, id)?;
            tx.commit().map_err(|e| e.to_string())?;
            let summary = format!("Kategória už má názov {path}");
            return Ok(CategoryMutationResult {
                change_id: None,
                summary,
                category_id: Some(id),
                parent_id: parent,
                root_id,
                ancestor_ids,
                deleted_category_ids: Vec::new(),
                deleted_root_ids: Vec::new(),
            });
        }
        update_column(&tx, "categories", id, "name", SqlValue::Text(name.clone()))?;
        if let Some(root) = paired_root {
            update_column(&tx, "category_roots", root, "name", SqlValue::Text(name.clone()))?;
        }
        let summary = format!("Premenované {old} → {name}");
        let ancestor_ids = category_ancestor_ids(&tx, id)?;
        let change_id = record_change(&tx, "rename_category", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult {
            change_id,
            summary,
            category_id: Some(id),
            parent_id: parent,
            root_id,
            ancestor_ids,
            deleted_category_ids: Vec::new(),
            deleted_root_ids: Vec::new(),
        })
    }

    pub fn move_category(&self, id: i64, new_parent_id: Option<i64>) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (name, old_parent, old_root): (String, Option<i64>, Option<i64>) = tx
            .query_row("SELECT name,parent_id,root_id FROM categories WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Kategória neexistuje".to_string())?;
        let subtree = category_subtree(&tx, id)?;
        let new_root;
        if let Some(parent) = new_parent_id {
            if subtree.contains(&parent) {
                return Err("Kategóriu nemožno presunúť do seba ani do jej podkategórie".into());
            }
            let target_root: Option<i64> = tx
                .query_row("SELECT root_id FROM categories WHERE id=?1", [parent], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Cieľová kategória neexistuje".to_string())?;
            ensure_unique_category_name(&tx, Some(parent), target_root, &name, Some(id), None)?;
            if old_parent == Some(parent) {
                let ancestor_ids = category_ancestor_ids(&tx, id)?;
                tx.commit().map_err(|e| e.to_string())?;
                return Ok(CategoryMutationResult {
                    change_id: None,
                    summary: format!("Kategória {name} je už na tomto mieste"),
                    category_id: Some(id),
                    parent_id: old_parent,
                    root_id: old_root,
                    ancestor_ids,
                    deleted_category_ids: Vec::new(),
                    deleted_root_ids: Vec::new(),
                });
            }
            new_root = target_root;
            update_column(&tx, "categories", id, "parent_id", SqlValue::Integer(parent))?;
        } else {
            if old_parent.is_none() {
                let ancestor_ids = category_ancestor_ids(&tx, id)?;
                tx.commit().map_err(|e| e.to_string())?;
                return Ok(CategoryMutationResult {
                    change_id: None,
                    summary: format!("Kategória {name} je už na najvyššej úrovni"),
                    category_id: Some(id),
                    parent_id: None,
                    root_id: old_root,
                    ancestor_ids,
                    deleted_category_ids: Vec::new(),
                    deleted_root_ids: Vec::new(),
                });
            }
            ensure_unique_category_name(&tx, None, None, &name, Some(id), None)?;
            let root = next_id(&tx, "category_roots")?;
            tx.execute("INSERT INTO category_roots(id,name) VALUES(?1,?2)", (root, &name))
                .map_err(|e| e.to_string())?;
            new_root = Some(root);
            update_column(&tx, "categories", id, "parent_id", SqlValue::Null)?;
        }
        if old_root != new_root {
            for descendant in subtree.iter().copied() {
                if descendant != id {
                    update_column(
                        &tx,
                        "categories",
                        descendant,
                        "root_id",
                        new_root.map(SqlValue::Integer).unwrap_or(SqlValue::Null),
                    )?;
                }
            }
            update_column(
                &tx,
                "categories",
                id,
                "root_id",
                new_root.map(SqlValue::Integer).unwrap_or(SqlValue::Null),
            )?;
        }
        if let Some(old_root) = old_root.filter(|root| Some(*root) != new_root) {
            let remaining: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM categories WHERE root_id=?1)",
                    [old_root],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !remaining {
                delete_row(&tx, "category_roots", old_root)?;
            }
        }
        let summary = if let Some(parent) = new_parent_id {
            format!("Presunuté {name} do {}", category_path(&tx, parent)?)
        } else {
            format!("Presunuté {name} na najvyššiu úroveň")
        };
        let ancestor_ids = category_ancestor_ids(&tx, id)?;
        let change_id = record_change(&tx, "move_category", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult {
            change_id,
            summary,
            category_id: Some(id),
            parent_id: new_parent_id,
            root_id: new_root,
            ancestor_ids,
            deleted_category_ids: Vec::new(),
            deleted_root_ids: Vec::new(),
        })
    }

    pub fn category_delete_info(&self, id: i64) -> Result<CategoryDeleteInfo, String> {
        let connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        category_delete_info(&connection, id)
    }

    pub fn delete_category(&self, id: i64) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let info = category_delete_info(&tx, id)?;
        if info.assigned_files > 0 {
            return Err(format!(
                "Kategóriu {} nie je možné zmazať, obsahuje {} {} (vrátane podkategórií). Najprv z nej súbory odoberte.",
                info.name,
                info.assigned_files,
                slovak_count(info.assigned_files as usize, "súbor", "súbory", "súborov")
            ));
        }
        let root_id: Option<i64> = tx
            .query_row("SELECT root_id FROM categories WHERE id=?1", [id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let ids = category_subtree(&tx, id)?;
        for category_id in ids.iter().rev().copied() {
            delete_row(&tx, "categories", category_id)?;
        }
        let mut deleted_root_ids = Vec::new();
        if let Some(root_id) = root_id {
            let any_left: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM categories WHERE root_id=?1)",
                    [root_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !any_left {
                delete_row(&tx, "category_roots", root_id)?;
                deleted_root_ids.push(root_id);
            }
        }
        let summary = format!("Zmazaná kategória {}", info.name);
        let change_id = record_change(&tx, "delete_category", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult {
            change_id,
            summary,
            category_id: None,
            parent_id: None,
            root_id,
            ancestor_ids: Vec::new(),
            deleted_category_ids: ids,
            deleted_root_ids,
        })
    }
}
