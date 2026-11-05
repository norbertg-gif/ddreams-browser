use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::{Arc,Mutex}, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug)]
pub struct PendingItem { table_name: String, row_key: String, column_name: String, old_value: Value, new_value: Value }

#[derive(Clone, Debug)]
pub struct ChangeBuilder { action: String, summary: String, items: Vec<PendingItem> }

impl ChangeBuilder {
    pub fn record(&mut self, table_name: &str, row_key: &str, column_name: &str, old_value: Value, new_value: Value) {
        self.items.push(PendingItem { table_name: table_name.into(), row_key: row_key.into(), column_name: column_name.into(), old_value, new_value });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary { pub id: i64, pub ts: String, pub action: String, pub summary: String, pub undone: bool }

#[derive(Clone, Debug)]
pub struct ListedAsset {
    pub temporary_id: i64,
    pub name: String,
    pub ext: String,
    pub size: Option<f64>,
    pub file_modified: Option<String>,
    pub path: String,
    pub folder_path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIdRemap { pub old_id: i64, pub new_id: i64 }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetValueState { pub id: i64, pub rating: Option<i64>, pub label: Option<String>, pub tagged: bool }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAssetValuesResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub remapped_ids: Vec<AssetIdRemap>,
    pub assets: Vec<AssetValueState>,
}

#[derive(Clone, Debug)]
struct StoredItem { table_name: String, row_key: String, column_name: String, old_value: SqlValue, new_value: SqlValue }

pub struct CatalogWriter { db: Arc<Mutex<rusqlite::Connection>>, writable: bool }

impl CatalogWriter {
    pub fn new(db: Arc<Mutex<rusqlite::Connection>>, writable: bool) -> Self { Self { db, writable } }
    pub fn begin_change(&self, action: impl Into<String>, summary: impl Into<String>) -> ChangeBuilder {
        ChangeBuilder { action: action.into(), summary: summary.into(), items: Vec::new() }
    }

    pub fn commit_change(&self, change: ChangeBuilder) -> Result<i64, String> {
        self.require_write()?;
        if change.items.is_empty() { return Err("A change must contain at least one item".into()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut stored = Vec::with_capacity(change.items.len());
        for item in change.items {
            validate_item(&item.table_name, &item.column_name)?;
            let old_value = json_to_sql(&item.old_value)?;
            let new_value = json_to_sql(&item.new_value)?;
            let current = read_item(&transaction, &item.table_name, &item.row_key, &item.column_name)?;
            if current != old_value { return Err(format!("Catalog row changed before commit: {} {}", item.table_name, item.row_key)); }
            apply_item(&transaction, &item.table_name, &item.row_key, &item.column_name, &new_value)?;
            stored.push(StoredItem { table_name: item.table_name, row_key: item.row_key, column_name: item.column_name, old_value: current, new_value });
        }
        let ts = iso_now();
        transaction.execute("INSERT INTO ddreams_changes(ts,action,summary,undone) VALUES(?1,?2,?3,0)", (&ts, &change.action, &change.summary)).map_err(|e| e.to_string())?;
        let change_id = transaction.last_insert_rowid();
        for item in stored {
            transaction.execute(
                "INSERT INTO ddreams_change_items(change_id,table_name,row_key,column_name,old_value,new_value) VALUES(?1,?2,?3,?4,?5,?6)",
                (change_id, item.table_name, item.row_key, item.column_name, encode_sql(&item.old_value)?, encode_sql(&item.new_value)?),
            ).map_err(|e| e.to_string())?;
        }
        mark_catalog_modified(&transaction)?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(change_id)
    }

    pub fn set_asset_values(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        changes: &serde_json::Map<String, Value>,
    ) -> Result<SetAssetValuesResult, String> {
        self.require_write()?;
        let updates = parse_asset_updates(changes)?;
        if updates.is_empty() { return Err("No asset values were supplied".into()); }
        let candidates = listed_assets.iter().map(|asset| (asset.temporary_id, asset)).collect::<HashMap<_, _>>();
        let mut ids = asset_ids.iter().copied().collect::<Vec<_>>();
        ids.sort_unstable(); ids.dedup();
        if ids.is_empty() { return Err("No files were selected".into()); }

        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut folder_paths = load_folder_paths(&transaction)?;
        let mut folder_roots = load_folder_roots(&transaction)?;
        let mut events = Vec::<StoredItem>::new();
        let mut targets = Vec::<AssetValueState>::new();
        let mut insertions = HashMap::<i64, (ListedAsset, i64)>::new();
        let mut remapped_ids = Vec::new();
        let mut any_untagged = false;

        for requested_id in ids {
            let id = if requested_id < 0 {
                let candidate = candidates.get(&requested_id).ok_or_else(|| format!("Listed file {requested_id} is no longer available"))?;
                if candidate.ext.trim().is_empty() { continue; }
                if let Some(existing) = find_asset_by_path(&transaction, &candidate.path)? {
                    if existing != requested_id { remapped_ids.push(AssetIdRemap { old_id: requested_id, new_id: existing }); }
                    existing
                } else {
                    let folder_id = ensure_folder(&transaction, &candidate.folder_path, &mut folder_paths, &mut folder_roots, &mut events)?;
                    let new_id = next_id(&transaction, "assets")?;
                    insertions.insert(new_id, ((*candidate).clone(), folder_id));
                    remapped_ids.push(AssetIdRemap { old_id: requested_id, new_id });
                    new_id
                }
            } else { requested_id };

            if let Some((candidate, _)) = insertions.get(&id) {
                targets.push(AssetValueState { id, rating: None, label: None, tagged: false });
                let _ = candidate;
            } else {
                let Some(state) = read_asset_values(&transaction, id)? else { continue; };
                if !state.tagged { any_untagged = true; }
                targets.push(state);
            }
        }
        if targets.is_empty() { return Err("No files were selected".into()); }
        if targets.iter().any(|asset| !asset.tagged) { any_untagged = true; }
        let toggle_tagged_to = if updates.tagged_toggle { Some(any_untagged) } else { updates.tagged };

        for target in &mut targets {
            let rating = updates.rating.unwrap_or(target.rating);
            let label = updates.label.as_ref().map(|value| value.clone()).unwrap_or_else(|| target.label.clone());
            let tagged = toggle_tagged_to.unwrap_or(target.tagged);
            if let Some((candidate, folder_id)) = insertions.get(&target.id) {
                let row = NewCatalogAsset { id: target.id, folder_id: *folder_id, name: candidate.name.clone(), ext: candidate.ext.clone(), size: candidate.size, file_modified: candidate.file_modified.clone(), path: candidate.path.clone(), rating, label: label.clone(), tagged };
                insert_asset(&transaction, &row)?;
                push_insert_snapshot(&transaction, &mut events, "assets", target.id)?;
            } else {
                if updates.rating.is_some() { record_asset_update(&transaction, &mut events, target.id, "rating", rating.map(SqlValue::Integer).unwrap_or(SqlValue::Null))?; }
                if updates.label.is_some() { record_asset_update(&transaction, &mut events, target.id, "label", label.clone().map(SqlValue::Text).unwrap_or(SqlValue::Null))?; }
                if updates.tagged.is_some() || updates.tagged_toggle { record_asset_update(&transaction, &mut events, target.id, "tagged", SqlValue::Integer(i64::from(tagged)))?; }
            }
            target.rating = rating; target.label = label; target.tagged = tagged;
        }

        let action_parts = update_summary_parts(&updates, toggle_tagged_to);
        let file_word = slovak_count(targets.len(), "súbor", "súbory", "súborov");
        let summary = format!("{} · {} {}", action_parts.join(" · "), targets.len(), file_word);
        let change_id = if events.is_empty() {
            None
        } else {
            let ts = iso_now();
            transaction.execute("INSERT INTO ddreams_changes(ts,action,summary,undone) VALUES(?1,'set_asset_values',?2,0)", (&ts, &summary)).map_err(|e| e.to_string())?;
            let change_id = transaction.last_insert_rowid();
            for item in events {
                transaction.execute(
                    "INSERT INTO ddreams_change_items(change_id,table_name,row_key,column_name,old_value,new_value) VALUES(?1,?2,?3,?4,?5,?6)",
                    (change_id, item.table_name, item.row_key, item.column_name, encode_sql(&item.old_value)?, encode_sql(&item.new_value)?),
                ).map_err(|e| e.to_string())?;
            }
            mark_catalog_modified(&transaction)?;
            Some(change_id)
        };
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(SetAssetValuesResult { change_id, summary, remapped_ids, assets: targets })
    }

    pub fn recent_changes(&self, limit: i64) -> Result<Vec<ChangeSummary>, String> {
        let connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let mut statement = connection.prepare("SELECT id,ts,action,summary,undone FROM ddreams_changes ORDER BY id DESC LIMIT ?1").map_err(|e| e.to_string())?;
        statement.query_map([limit.clamp(1, 200)], |row| Ok(ChangeSummary { id: row.get(0)?, ts: row.get(1)?, action: row.get(2)?, summary: row.get(3)?, undone: row.get::<_, i64>(4)? != 0 }))
            .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
    }

    pub fn undo_last(&self) -> Result<Option<ChangeSummary>, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let latest = transaction.query_row(
            "SELECT id,ts,action,summary,undone FROM ddreams_changes WHERE undone=0 ORDER BY id DESC LIMIT 1", [],
            |row| Ok(ChangeSummary { id: row.get(0)?, ts: row.get(1)?, action: row.get(2)?, summary: row.get(3)?, undone: row.get::<_, i64>(4)? != 0 }),
        ).optional().map_err(|e| e.to_string())?;
        let Some(change) = latest else { return Ok(None); };
        let items = {
            let mut statement = transaction.prepare("SELECT table_name,row_key,column_name,old_value,new_value FROM ddreams_change_items WHERE change_id=?1 ORDER BY rowid DESC").map_err(|e| e.to_string())?;
            statement.query_map([change.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)))
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
        };
        for (table, key, column, old, new) in items {
            validate_item(&table, &column)?;
            let old_value = decode_sql(&old)?;
            let new_value = decode_sql(&new)?;
            if read_item(&transaction, &table, &key, &column)? != new_value { return Err(format!("Cannot undo because {} {} changed since the journal entry", table, key)); }
            apply_item(&transaction, &table, &key, &column, &old_value)?;
        }
        transaction.execute("UPDATE ddreams_changes SET undone=1 WHERE id=?1", [change.id]).map_err(|e| e.to_string())?;
        mark_catalog_modified(&transaction)?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(Some(change))
    }

    fn require_write(&self) -> Result<(), String> {
        if self.writable { Ok(()) } else { Err("Catalog is in strict read-only mode".into()) }
    }
}

fn validate_item(table: &str, column: &str) -> Result<(), String> {
    let allowed = match table {
        "assets" => matches!(column, "rating" | "label" | "tagged" | "caption" | "author" | "notes" | "__snapshot__"),
        "folders" | "folder_roots" => column == "__snapshot__",
        "categories" => matches!(column, "name" | "parent_id" | "root_id"),
        "keywords" => matches!(column, "name" | "parent_id"),
        "asset_categories" | "asset_keywords" => column == "__exists__",
        _ => false,
    };
    if allowed { Ok(()) } else { Err(format!("Unsupported journal target: {table}.{column}")) }
}

#[derive(Default)]
struct AssetUpdates {
    rating: Option<Option<i64>>,
    label: Option<Option<String>>,
    tagged: Option<bool>,
    tagged_toggle: bool,
}

impl AssetUpdates {
    fn is_empty(&self) -> bool { self.rating.is_none() && self.label.is_none() && self.tagged.is_none() && !self.tagged_toggle }
}

struct NewCatalogAsset {
    id: i64,
    folder_id: i64,
    name: String,
    ext: String,
    size: Option<f64>,
    file_modified: Option<String>,
    path: String,
    rating: Option<i64>,
    label: Option<String>,
    tagged: bool,
}

fn parse_asset_updates(values: &serde_json::Map<String, Value>) -> Result<AssetUpdates, String> {
    let mut result = AssetUpdates::default();
    for (key, value) in values {
        match key.as_str() {
            "rating" => {
                result.rating = Some(if value.is_null() { None } else {
                    let rating = value.as_i64().ok_or("Rating must be 1 to 5 or null")?;
                    if !(1..=5).contains(&rating) { return Err("Rating must be 1 to 5 or null".into()); }
                    Some(rating)
                });
            }
            "label" => {
                result.label = Some(if value.is_null() { None } else {
                    let label = value.as_str().ok_or("Invalid colour label")?;
                    if !["Red", "Yellow", "Green", "Blue", "Purple"].contains(&label) { return Err("Invalid colour label".into()); }
                    Some(label.to_owned())
                });
            }
            "tagged" if value.as_str() == Some("toggle") => result.tagged_toggle = true,
            "tagged" => result.tagged = Some(value.as_bool().ok_or("Tagged state must be true, false, or 'toggle'")?),
            _ => return Err(format!("Unsupported asset value: {key}")),
        }
    }
    if result.tagged_toggle && result.tagged.is_some() { return Err("Tagged toggle cannot be combined with a fixed tagged value".into()); }
    Ok(result)
}

fn update_summary_parts(updates: &AssetUpdates, tagged: Option<bool>) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(rating) = updates.rating { parts.push(rating.map(|value| format!("Hodnotenie {value} ★")).unwrap_or_else(|| "Bez hodnotenia".into())); }
    if let Some(label) = &updates.label { parts.push(label.as_ref().map(|value| format!("Farebný štítok {}", label_name(value))).unwrap_or_else(|| "Bez štítku".into())); }
    if let Some(tagged) = tagged { parts.push(if tagged { "Označené" } else { "Zrušené označenie" }.into()); }
    parts
}

fn label_name(value: &str) -> &str {
    match value { "Red" => "Červená", "Yellow" => "Žltá", "Green" => "Zelená", "Blue" => "Modrá", "Purple" => "Fialová", _ => value }
}

fn slovak_count(count: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else if (2..=4).contains(&count) { few } else { many }
}

fn read_asset_values(transaction: &Transaction<'_>, id: i64) -> Result<Option<AssetValueState>, String> {
    transaction.query_row("SELECT id,rating,label,coalesce(tagged,0),ext_lower,is_listed FROM assets WHERE id=?1", [id], |row| {
        let id = row.get(0)?; let rating = row.get(1)?; let label = row.get(2)?; let tagged = row.get::<_, i64>(3)? != 0;
        let ext: String = row.get(4)?; let listed: i64 = row.get(5)?;
        Ok((ext, listed, AssetValueState { id, rating, label, tagged }))
    }).optional().map_err(|e| e.to_string()).map(|row| row.filter(|(ext,listed,_)| !ext.is_empty() && *listed != 0).map(|(_,_,state)| state))
}

fn find_asset_by_path(transaction: &Transaction<'_>, path: &str) -> Result<Option<i64>, String> {
    transaction.query_row("SELECT id FROM assets WHERE lower(rtrim(path,'/\\'))=lower(rtrim(?1,'/\\')) ORDER BY id LIMIT 1", [path], |row| row.get(0)).optional().map_err(|e| e.to_string())
}

fn insert_asset(transaction: &Transaction<'_>, asset: &NewCatalogAsset) -> Result<(), String> {
    transaction.execute(
        "INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,size,width,height,bpp,caption,notes,author,rating,tagged,label,file_modified,exif_date,acdsee_date,md_dirty,is_orphan,path) VALUES(?1,?2,?3,?4,?5,1,?6,NULL,NULL,NULL,NULL,NULL,NULL,?7,?8,?9,?10,NULL,NULL,NULL,NULL,?11)",
        (asset.id, asset.folder_id, &asset.name, &asset.ext, asset.ext.to_lowercase(), asset.size, asset.rating, i64::from(asset.tagged), &asset.label, &asset.file_modified, &asset.path),
    ).map_err(|e| e.to_string())?;
    Ok(())
}

fn record_asset_update(transaction: &Transaction<'_>, events: &mut Vec<StoredItem>, id: i64, column: &str, new_value: SqlValue) -> Result<(), String> {
    let old_value = read_item(transaction, "assets", &id.to_string(), column)?;
    if old_value == new_value { return Ok(()); }
    apply_item(transaction, "assets", &id.to_string(), column, &new_value)?;
    events.push(StoredItem { table_name: "assets".into(), row_key: id.to_string(), column_name: column.into(), old_value, new_value });
    Ok(())
}

fn load_folder_paths(transaction: &Transaction<'_>) -> Result<HashMap<String, i64>, String> {
    let mut statement = transaction.prepare("SELECT id,path FROM folders WHERE path IS NOT NULL").map_err(|e| e.to_string())?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    for row in rows { let (id,path) = row.map_err(|e| e.to_string())?; result.insert(normalize_fs_path(&path), id); }
    Ok(result)
}

fn load_folder_roots(transaction: &Transaction<'_>) -> Result<Vec<(i64, String, String)>, String> {
    let mut statement = transaction.prepare("SELECT id,name,path FROM folder_roots WHERE path IS NOT NULL").map_err(|e| e.to_string())?;
    statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
        .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

fn ensure_folder(transaction: &Transaction<'_>, target: &str, folders: &mut HashMap<String, i64>, roots: &mut Vec<(i64, String, String)>, events: &mut Vec<StoredItem>) -> Result<i64, String> {
    let target_key = normalize_fs_path(target);
    if let Some(id) = folders.get(&target_key) { return Ok(*id); }
    if target.trim().is_empty() { return Err("Folder path is empty".into()); }
    let mut root = roots.iter().filter(|(_,_,path)| is_path_prefix(path,target)).max_by_key(|(_,_,path)| path.len()).cloned();
    if root.is_none() {
        let (name,path) = derive_path_root(target);
        let id = next_id(transaction,"folder_roots")?;
        transaction.execute("INSERT INTO folder_roots(id,name,path) VALUES(?1,?2,?3)",(id,&name,&path)).map_err(|e|e.to_string())?;
        push_insert_snapshot(transaction,events,"folder_roots",id)?;
        root = Some((id,name,path));
    }
    let (root_id,root_name,root_path) = root.unwrap();
    let root_key = normalize_fs_path(&root_path);
    let mut parent_id = if let Some(id)=folders.get(&root_key) { *id } else {
        let id=next_id(transaction,"folders")?;
        transaction.execute("INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,NULL,?2,'\\',?3)",(id,root_id,&root_path)).map_err(|e|e.to_string())?;
        push_insert_snapshot(transaction,events,"folders",id)?;
        folders.insert(root_key.clone(),id);
        let _=root_name;
        id
    };
    let target_parts = path_parts(target);
    let root_parts = path_parts(&root_path);
    let skip = root_parts.len();
    let mut current = PathBuf::from(&root_path);
    for part in target_parts.into_iter().skip(skip) {
        current.push(&part);
        let path=current.to_string_lossy().to_string(); let key=normalize_fs_path(&path);
        if let Some(id)=folders.get(&key) { parent_id=*id; continue; }
        let id=next_id(transaction,"folders")?;
        transaction.execute("INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,?2,?3,?4,?5)",(id,parent_id,root_id,&part,&path)).map_err(|e|e.to_string())?;
        push_insert_snapshot(transaction,events,"folders",id)?;
        folders.insert(key,id); parent_id=id;
    }
    Ok(parent_id)
}

fn derive_path_root(path: &str) -> (String,String) {
    let normalized=path.replace('/',"\\");
    if normalized.len()>=3 && normalized.as_bytes()[1]==b':' && normalized.as_bytes()[2]==b'\\' {
        return (normalized[..2].to_owned(),normalized[..3].to_owned());
    }
    let parts=path_parts(&normalized);
    if normalized.starts_with("\\\\") && parts.len()>=2 {
        let root=format!("\\\\{}\\{}\\",parts[0],parts[1]);
        return (format!("{}\\{}",parts[0],parts[1]),root);
    }
    let first=parts.first().cloned().unwrap_or_else(||normalized.clone());
    (first.clone(),first)
}

fn path_parts(path:&str)->Vec<String>{path.split(['\\','/']).filter(|part|!part.is_empty()).map(str::to_owned).collect()}
fn normalize_fs_path(path:&str)->String{path.trim_end_matches(['\\','/']).replace('/',"\\").to_lowercase()}
fn is_path_prefix(root:&str,target:&str)->bool{let root=normalize_fs_path(root);let target=normalize_fs_path(target);target==root||target.strip_prefix(&root).is_some_and(|suffix|suffix.starts_with('\\'))}

fn next_id(transaction:&Transaction<'_>,table:&str)->Result<i64,String>{
    let (max_id,journal_id):(i64,i64)=transaction.query_row(&format!("SELECT coalesce((SELECT max(id) FROM {table}),0),coalesce((SELECT max(CAST(row_key AS INTEGER)) FROM ddreams_change_items WHERE table_name=?1 AND column_name='__snapshot__'),0)"),[table],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|e|e.to_string())?;
    Ok(max_id.max(journal_id)+1)
}

fn push_insert_snapshot(transaction:&Transaction<'_>,events:&mut Vec<StoredItem>,table:&str,id:i64)->Result<(),String>{
    let value=read_item(transaction,table,&id.to_string(),"__snapshot__")?;
    events.push(StoredItem{table_name:table.into(),row_key:id.to_string(),column_name:"__snapshot__".into(),old_value:SqlValue::Null,new_value:value});
    Ok(())
}

fn json_to_sql(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(value) => Ok(SqlValue::Integer(i64::from(*value))),
        Value::Number(value) => value.as_i64().map(SqlValue::Integer).or_else(|| value.as_f64().map(SqlValue::Real)).ok_or_else(|| "Unsupported JSON number".into()),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        _ => Err("Journal values must be SQLite scalar values".into()),
    }
}

fn encode_sql(value: &SqlValue) -> Result<String, String> {
    let tagged = match value {
        SqlValue::Null => serde_json::json!({"type":"null"}),
        SqlValue::Integer(value) => serde_json::json!({"type":"integer","value":value}),
        SqlValue::Real(value) => serde_json::json!({"type":"real","value":value}),
        SqlValue::Text(value) => serde_json::json!({"type":"text","value":value}),
        SqlValue::Blob(value) => serde_json::json!({"type":"blob","value":value.iter().map(|byte|format!("{byte:02x}")).collect::<String>()}),
    };
    serde_json::to_string(&tagged).map_err(|e| e.to_string())
}

fn decode_sql(value: &str) -> Result<SqlValue, String> {
    let tagged: Value = serde_json::from_str(value).map_err(|e| e.to_string())?;
    match tagged.get("type").and_then(Value::as_str) {
        Some("null") => Ok(SqlValue::Null),
        Some("integer") => tagged.get("value").and_then(Value::as_i64).map(SqlValue::Integer).ok_or_else(|| "Invalid journal integer".into()),
        Some("real") => tagged.get("value").and_then(Value::as_f64).map(SqlValue::Real).ok_or_else(|| "Invalid journal real".into()),
        Some("text") => tagged.get("value").and_then(Value::as_str).map(|value| SqlValue::Text(value.into())).ok_or_else(|| "Invalid journal text".into()),
        Some("blob") => {
            let text = tagged.get("value").and_then(Value::as_str).ok_or("Invalid journal blob")?;
            if text.len() % 2 != 0 { return Err("Invalid journal blob encoding".into()); }
            let bytes = (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i+2],16).map_err(|e|e.to_string())).collect::<Result<Vec<_>,_>>()?;
            Ok(SqlValue::Blob(bytes))
        }
        _ => Err("Invalid journal value type".into()),
    }
}

fn key_id(row_key: &str) -> Result<i64, String> { row_key.parse::<i64>().map_err(|e| e.to_string()) }

fn link_ids(row_key: &str) -> Result<(i64,i64), String> {
    let (left,right) = row_key.split_once(':').ok_or("Relationship row key must be asset_id:target_id")?;
    Ok((key_id(left)?, key_id(right)?))
}

fn read_item(transaction: &Transaction<'_>, table: &str, row_key: &str, column: &str) -> Result<SqlValue, String> {
    if column == "__snapshot__" { return read_snapshot(transaction, table, row_key); }
    if table == "asset_categories" || table == "asset_keywords" {
        let (asset_id, target_id) = link_ids(row_key)?;
        let target_column = if table == "asset_categories" { "category_id" } else { "keyword_id" };
        let exists: bool = transaction.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE asset_id=?1 AND {target_column}=?2)"), (asset_id,target_id), |row| row.get(0)).map_err(|e|e.to_string())?;
        return Ok(SqlValue::Integer(i64::from(exists)));
    }
    let id = key_id(row_key)?;
    transaction.query_row(&format!("SELECT {column} FROM {table} WHERE id=?1"), [id], |row| row.get(0)).map_err(|e|e.to_string())
}

fn apply_item(transaction: &Transaction<'_>, table: &str, row_key: &str, column: &str, value: &SqlValue) -> Result<(), String> {
    if column == "__snapshot__" { return restore_snapshot(transaction, table, row_key, value); }
    if table == "asset_categories" || table == "asset_keywords" {
        let (asset_id,target_id) = link_ids(row_key)?;
        let target_column = if table == "asset_categories" { "category_id" } else { "keyword_id" };
        if value_truth(value)? {
            transaction.execute(&format!("INSERT OR IGNORE INTO {table}(asset_id,{target_column}) VALUES(?1,?2)"), (asset_id,target_id)).map_err(|e|e.to_string())?;
        } else {
            transaction.execute(&format!("DELETE FROM {table} WHERE asset_id=?1 AND {target_column}=?2"), (asset_id,target_id)).map_err(|e|e.to_string())?;
        }
        return Ok(());
    }
    let id = key_id(row_key)?;
    let changed = transaction.execute(&format!("UPDATE {table} SET {column}=?1 WHERE id=?2"), (value,id)).map_err(|e|e.to_string())?;
    if changed != 1 { return Err(format!("Journal target row not found: {table} {row_key}")); }
    Ok(())
}

fn snapshot_columns(table:&str)->Result<&'static [&'static str],String>{
    match table {
        "assets"=>Ok(&["id","folder_id","name","ext","ext_lower","is_listed","size","width","height","bpp","caption","notes","author","rating","tagged","label","file_modified","exif_date","acdsee_date","md_dirty","is_orphan","path"]),
        "folders"=>Ok(&["id","parent_id","root_id","name","path"]),
        "folder_roots"=>Ok(&["id","name","path"]),
        _=>Err(format!("Unsupported snapshot table: {table}")),
    }
}

fn read_snapshot(transaction:&Transaction<'_>,table:&str,row_key:&str)->Result<SqlValue,String>{
    let columns=snapshot_columns(table)?;let id=key_id(row_key)?;
    let sql=format!("SELECT {} FROM {table} WHERE id=?1",columns.join(","));
    let snapshot=transaction.query_row(&sql,[id],|row|{
        let mut values=Vec::with_capacity(columns.len());
        for index in 0..columns.len(){values.push(encode_sql(&row.get::<_,SqlValue>(index)?).map_err(|_|rusqlite::Error::InvalidQuery)?);}
        Ok(values)
    }).optional().map_err(|e|e.to_string())?;
    match snapshot {None=>Ok(SqlValue::Null),Some(values)=>serde_json::to_string(&values).map(SqlValue::Text).map_err(|e|e.to_string())}
}

fn restore_snapshot(transaction:&Transaction<'_>,table:&str,row_key:&str,value:&SqlValue)->Result<(),String>{
    let columns=snapshot_columns(table)?;let id=key_id(row_key)?;
    if matches!(value,SqlValue::Null){transaction.execute(&format!("DELETE FROM {table} WHERE id=?1"),[id]).map_err(|e|e.to_string())?;return Ok(());}
    let SqlValue::Text(snapshot)=value else{return Err("Invalid row snapshot value".into())};
    let encoded:Vec<String>=serde_json::from_str(snapshot).map_err(|e|e.to_string())?;
    if encoded.len()!=columns.len(){return Err("Invalid row snapshot width".into());}
    let values=encoded.iter().map(|item|decode_sql(item)).collect::<Result<Vec<_>,_>>()?;
    let sql=format!("INSERT OR REPLACE INTO {table}({}) VALUES({})",columns.join(","),vec!["?";columns.len()].join(","));
    transaction.execute(&sql,rusqlite::params_from_iter(values.iter())).map_err(|e|e.to_string())?;
    Ok(())
}

fn value_truth(value: &SqlValue) -> Result<bool, String> {
    match value { SqlValue::Integer(value) => Ok(*value != 0), SqlValue::Null => Ok(false), _ => Err("Relationship journal values must be boolean or integer".into()) }
}

fn mark_catalog_modified(transaction: &Transaction<'_>) -> Result<(), String> {
    transaction.execute("UPDATE meta SET value='1' WHERE key='catalog_modified'", []).map_err(|e|e.to_string())?;
    Ok(())
}

fn iso_now() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = (duration.as_secs() / 86_400) as i64;
    let second_of_day = duration.as_secs() % 86_400;
    let z=days+719_468; let era=if z>=0{z}else{z-146_096}/146_097; let doe=z-era*146_097;
    let yoe=(doe-doe/1_460+doe/36_524-doe/146_096)/365; let mut year=yoe+era*400;
    let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153; let day=doy-(153*mp+2)/5+1;
    let month=mp+if mp<10{3}else{-9}; if month<=2{year+=1;}
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",second_of_day/3600,second_of_day/60%60,second_of_day%60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use crate::db::Db;
    use std::sync::{Arc,Mutex};

    fn fixture() -> Db {
        let connection=Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE assets(id INTEGER PRIMARY KEY,rating INTEGER,label TEXT,tagged INTEGER);INSERT INTO assets VALUES(1,2,'Red',0);CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);CREATE TABLE keywords(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT NOT NULL,action TEXT NOT NULL,summary TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER NOT NULL,table_name TEXT NOT NULL,row_key TEXT NOT NULL,column_name TEXT NOT NULL,old_value TEXT,new_value TEXT);CREATE INDEX idx_change_items_change ON ddreams_change_items(change_id);").unwrap();
        Arc::new(Mutex::new(connection))
    }

    fn write_fixture() -> Db {
        let connection=Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT NOT NULL,action TEXT NOT NULL,summary TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER NOT NULL,table_name TEXT NOT NULL,row_key TEXT NOT NULL,column_name TEXT NOT NULL,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE INDEX idx_assets_path ON assets(path);INSERT INTO folder_roots VALUES(1,'F:','F:\\');INSERT INTO folders VALUES(1,NULL,1,'F:','F:\\');INSERT INTO folders VALUES(2,1,1,'Photos','F:\\Photos');INSERT INTO assets VALUES(1,2,'one.jpg','jpg','jpg',1,123.0,10,10,24,NULL,NULL,NULL,2,0,'Red','2020-01-01T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\one.jpg');INSERT INTO assets VALUES(2,2,'two.jpg','jpg','jpg',1,456.0,10,10,24,NULL,NULL,NULL,4,1,NULL,'2020-01-02T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\two.jpg');").unwrap();
        Arc::new(Mutex::new(connection))
    }

    #[test]
    fn journal_records_change_and_undo_restores_exact_value() {
        let db=fixture(); let writer=CatalogWriter::new(db.clone(),true);
        let mut change=writer.begin_change("rating","Set rating");
        change.record("assets","1","rating",Value::from(2),Value::from(5));
        let id=writer.commit_change(change).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),5);
        let recent=writer.recent_changes(5).unwrap(); assert_eq!(recent[0].id,id); assert!(!recent[0].undone);
        let undone=writer.undo_last().unwrap().unwrap(); assert_eq!(undone.id,id);
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        assert!(writer.recent_changes(5).unwrap()[0].undone);
    }

    #[test]
    fn relationship_journal_restores_link_existence() {
        let db=fixture(); db.lock().unwrap().execute("INSERT INTO categories(id,name) VALUES(7,'Test')",[]).unwrap();
        let writer=CatalogWriter::new(db.clone(),true); let mut change=writer.begin_change("category","Assign category");
        change.record("asset_categories","1:7","__exists__",Value::Bool(false),Value::Bool(true)); writer.commit_change(change).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_categories",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        writer.undo_last().unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_categories",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn readonly_writer_refuses_commit_and_undo() {
        let root=std::env::temp_dir().join(format!("ddreams-readonly-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap(); let path=root.join("catalog.sqlite");
        let conn=Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE assets(id INTEGER PRIMARY KEY,rating INTEGER);INSERT INTO assets VALUES(1,2);").unwrap(); drop(conn);
        let db: Db=Arc::new(Mutex::new(crate::db::open_catalog(&path,true).unwrap()));
        let writer=CatalogWriter::new(db.clone(),false); let mut change=writer.begin_change("rating","Set rating");
        change.record("assets","1","rating",Value::from(2),Value::from(5));
        assert!(writer.commit_change(change).unwrap_err().contains("read-only"));
        assert!(writer.undo_last().unwrap_err().contains("read-only"));
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        assert!(db.lock().unwrap().execute("UPDATE assets SET rating=5 WHERE id=1",[]).is_err());
        drop(db); let _=std::fs::remove_dir_all(root);
    }

    #[test]
    fn set_values_updates_one_and_many_assets_with_one_journal_change() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(5)),("label".into(),Value::from("Blue"))]);
        let result=writer.set_asset_values(&[1,2,1],&[],&changes).unwrap();
        assert!(result.summary.contains("Hodnotenie 5"));assert!(result.summary.contains("2 súbory"));
        let connection=db.lock().unwrap();
        assert_eq!(connection.query_row("SELECT count(*) FROM ddreams_changes",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(connection.query_row("SELECT count(*) FROM assets WHERE rating=5 AND label='Blue'",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        drop(connection);writer.undo_last().unwrap();
        let connection=db.lock().unwrap();
        assert_eq!(connection.query_row("SELECT rating||':'||coalesce(label,'NULL') FROM assets WHERE id=1",[],|row|row.get::<_,String>(0)).unwrap(),"2:Red");
        assert_eq!(connection.query_row("SELECT rating||':'||coalesce(label,'NULL') FROM assets WHERE id=2",[],|row|row.get::<_,String>(0)).unwrap(),"4:NULL");
        drop(connection);
        let single_changes=serde_json::Map::from_iter([("rating".into(),Value::from(1))]);
        let single=writer.set_asset_values(&[1],&[],&single_changes).unwrap();
        assert!(single.summary.contains("1 súbor"));
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        writer.undo_last().unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn set_values_toggle_tags_together_for_a_mixed_selection() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let changes=serde_json::Map::from_iter([("tagged".into(),Value::from("toggle"))]);
        writer.set_asset_values(&[1,2],&[],&changes).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE tagged=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        writer.set_asset_values(&[1,2],&[],&changes).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE tagged=0",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn uncatalogued_listed_asset_is_inserted_and_undo_removes_asset_and_created_folders() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let candidate=ListedAsset{temporary_id:-10,name:"new.jpg".into(),ext:"jpg".into(),size:Some(789.0),file_modified:Some("2024-03-04T00:00:00Z".into()),path:"G:\\Camera\\new.jpg".into(),folder_path:"G:\\Camera".into()};
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(3))]);
        let result=writer.set_asset_values(&[-10],&[candidate],&changes).unwrap();let inserted=result.remapped_ids[0].new_id;
        assert_eq!(db.lock().unwrap().query_row("SELECT name||':'||rating FROM assets WHERE id=?1",[inserted],|row|row.get::<_,String>(0)).unwrap(),"new.jpg:3");
        writer.undo_last().unwrap();
        let connection=db.lock().unwrap();
        assert_eq!(connection.query_row("SELECT count(*) FROM assets WHERE id=?1",[inserted],|row|row.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(connection.query_row("SELECT count(*) FROM folders WHERE path='G:\\Camera'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(connection.query_row("SELECT count(*) FROM folder_roots WHERE name='G:'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn readonly_writer_refuses_set_asset_values() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),false);
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(4))]);
        assert!(writer.set_asset_values(&[1],&[],&changes).unwrap_err().contains("read-only"));
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    }
}
