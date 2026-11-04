use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{sync::{Arc,Mutex}, time::{SystemTime, UNIX_EPOCH}};

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
        "assets" => matches!(column, "rating" | "label" | "tagged" | "caption" | "author" | "notes"),
        "categories" => matches!(column, "name" | "parent_id" | "root_id"),
        "keywords" => matches!(column, "name" | "parent_id"),
        "asset_categories" | "asset_keywords" => column == "__exists__",
        _ => false,
    };
    if allowed { Ok(()) } else { Err(format!("Unsupported journal target: {table}.{column}")) }
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
}
