use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: i64 = 1;

pub fn migrate_catalog(connection: &mut Connection) -> Result<(), String> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    transaction.execute_batch("CREATE TABLE IF NOT EXISTS ddreams_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(|e|e.to_string())?;
    let owner: Option<String> = transaction.query_row("SELECT value FROM ddreams_meta WHERE key='catalog_owner'", [], |row|row.get(0)).optional().map_err(|e|e.to_string())?;
    if owner.as_deref().is_some_and(|value| value != "ddreams") {
        return Err(format!("Refusing to open a catalog owned by {}", owner.unwrap_or_default()));
    }
    let version_text: Option<String> = transaction.query_row("SELECT value FROM ddreams_meta WHERE key='schema_version'", [], |row|row.get(0)).optional().map_err(|e|e.to_string())?;
    let version = version_text.as_deref().unwrap_or("0").parse::<i64>().map_err(|_| "Invalid DDreams schema_version".to_string())?;
    if version > SCHEMA_VERSION { return Err(format!("Catalog schema {version} is newer than this app supports ({SCHEMA_VERSION})")); }
    if version < 1 {
        let has_assets: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='assets')", [], |row|row.get(0)).map_err(|e|e.to_string())?;
        if !has_assets { return Err("Catalog has no imported assets table; import or copy a catalog before starting DDreams".into()); }
        transaction.execute_batch("CREATE INDEX IF NOT EXISTS idx_assets_listed_exif_date_ext ON assets(is_listed,exif_date,ext_lower);
            CREATE INDEX IF NOT EXISTS idx_assets_listed_acdsee_date_ext ON assets(is_listed,acdsee_date,ext_lower);
            CREATE INDEX IF NOT EXISTS idx_assets_listed_file_modified_ext ON assets(is_listed,file_modified,ext_lower);
            CREATE TABLE IF NOT EXISTS ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT NOT NULL,action TEXT NOT NULL,summary TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS ddreams_change_items(change_id INTEGER NOT NULL,table_name TEXT NOT NULL,row_key TEXT NOT NULL,column_name TEXT NOT NULL,old_value TEXT,new_value TEXT,FOREIGN KEY(change_id) REFERENCES ddreams_changes(id) ON DELETE CASCADE);
            CREATE INDEX IF NOT EXISTS idx_ddreams_change_items_change_id ON ddreams_change_items(change_id);").map_err(|e|e.to_string())?;
        set_meta(&transaction, "created_from", "acdsee_import")?;
        let imported = iso_now();
        set_meta(&transaction, "import_date", &imported)?;
        set_meta(&transaction, "catalog_owner", "ddreams")?;
        set_meta(&transaction, "schema_version", "1")?;
    } else {
        set_meta_if_absent(&transaction, "catalog_owner", "ddreams")?;
        set_meta_if_absent(&transaction, "created_from", "acdsee_import")?;
        set_meta_if_absent(&transaction, "import_date", &iso_now())?;
        set_meta_if_absent(&transaction, "schema_version", "1")?;
    }
    transaction.execute_batch("CREATE INDEX IF NOT EXISTS folders_norm_path ON folders(lower(rtrim(replace(path,'/','\\'),'\\'))); CREATE INDEX IF NOT EXISTS idx_assets_folder_listed ON assets(folder_id,is_listed); CREATE TABLE IF NOT EXISTS asset_color_overrides(asset_id INTEGER NOT NULL,color TEXT NOT NULL,state INTEGER NOT NULL CHECK(state IN (0,1)),PRIMARY KEY(asset_id,color)); CREATE TABLE IF NOT EXISTS ai_suggestion_rejections(asset_id INTEGER NOT NULL,kind TEXT NOT NULL,label_id INTEGER NOT NULL,PRIMARY KEY(asset_id,kind,label_id)); CREATE TABLE IF NOT EXISTS ddreams_folder_rules(path TEXT PRIMARY KEY COLLATE NOCASE,mode TEXT NOT NULL CHECK(mode IN ('include','exclude')),created TEXT NOT NULL);").map_err(|e|e.to_string())?;
    if !crate::incoming::has_deletion_column(&transaction).map_err(|e| e.to_string())? {
        transaction.execute("ALTER TABLE assets ADD COLUMN delete_suggestion INTEGER CHECK(delete_suggestion IN (0,1))", [])
            .map_err(|e| e.to_string())?;
    }
    transaction.execute("CREATE INDEX IF NOT EXISTS idx_assets_delete_suggestion ON assets(id) WHERE is_listed=1 AND delete_suggestion=1", [])
        .map_err(|e| e.to_string())?;
    transaction.commit().map_err(|e| e.to_string())
}

fn set_meta(transaction: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<(), String> {
    transaction.execute("INSERT INTO ddreams_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (key,value)).map_err(|e|e.to_string())?;
    Ok(())
}

fn set_meta_if_absent(transaction: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<(), String> {
    transaction.execute("INSERT OR IGNORE INTO ddreams_meta(key,value) VALUES(?1,?2)", (key,value)).map_err(|e|e.to_string())?;
    Ok(())
}

fn iso_now() -> String {
    let seconds=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days=(seconds/86_400) as i64; let sec=seconds%86_400;
    let z=days+719_468; let era=if z>=0{z}else{z-146_096}/146_097; let doe=z-era*146_097;
    let yoe=(doe-doe/1_460+doe/36_524-doe/146_096)/365; let mut year=yoe+era*400;
    let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153; let day=doy-(153*mp+2)/5+1;
    let month=mp+if mp<10{3}else{-9}; if month<=2{year+=1;}
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",sec/3600,sec/60%60,sec%60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fixture() -> Connection {
        let connection=Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE folders(id INTEGER PRIMARY KEY,path TEXT);CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,is_listed INTEGER,ext_lower TEXT,exif_date TEXT,acdsee_date TEXT,file_modified TEXT);").unwrap();
        connection
    }

    #[test]
    fn migration_is_idempotent_and_marks_existing_imported_catalog() {
        let mut connection=fixture();
        migrate_catalog(&mut connection).unwrap();
        migrate_catalog(&mut connection).unwrap();
        for (key,expected) in [("schema_version","1"),("catalog_owner","ddreams"),("created_from","acdsee_import")] {
            assert_eq!(connection.query_row("SELECT value FROM ddreams_meta WHERE key=?1",[key],|row|row.get::<_,String>(0)).unwrap(),expected);
        }
        assert!(connection.query_row("SELECT value FROM ddreams_meta WHERE key='import_date'",[],|row|row.get::<_,String>(0)).unwrap().ends_with('Z'));
        for index in ["idx_assets_listed_exif_date_ext","idx_assets_listed_acdsee_date_ext","idx_assets_listed_file_modified_ext"] {
            assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",[index],|row|row.get::<_,i64>(0)).unwrap(),1);
        }
        assert!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='ddreams_change_items'",[],|row|row.get::<_,i64>(0)).unwrap()==1);
    }

    #[test]
    fn deletion_suggestion_migration_preserves_existing_rows_and_values() {
        let mut conn = fixture();
        conn.execute("INSERT INTO assets(id,is_listed) VALUES(1,1)", []).unwrap();
        migrate_catalog(&mut conn).unwrap();
        assert_eq!(conn.query_row("SELECT delete_suggestion FROM assets WHERE id=1", [], |row| row.get::<_, Option<i64>>(0)).unwrap(), None);
        conn.execute("UPDATE assets SET delete_suggestion=0 WHERE id=1", []).unwrap();
        migrate_catalog(&mut conn).unwrap();
        assert_eq!(conn.query_row("SELECT is_listed,delete_suggestion FROM assets WHERE id=1", [], |row| Ok((row.get::<_, i64>(0)?,row.get::<_, i64>(1)?))).unwrap(), (1,0));
    }

    #[test]
    fn migration_refuses_a_catalog_claimed_by_another_owner() {
        let mut connection=fixture();
        connection.execute_batch("CREATE TABLE ddreams_meta(key TEXT PRIMARY KEY,value TEXT);INSERT INTO ddreams_meta VALUES('catalog_owner','other');").unwrap();
        assert!(migrate_catalog(&mut connection).unwrap_err().contains("owned by other"));
    }
}
