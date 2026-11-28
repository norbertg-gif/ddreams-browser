fn write_fixture() -> Db {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT NOT NULL,action TEXT NOT NULL,summary TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER NOT NULL,table_name TEXT NOT NULL,row_key TEXT NOT NULL,column_name TEXT NOT NULL,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE category_roots(id INTEGER PRIMARY KEY,name TEXT);INSERT INTO category_roots VALUES(1,'Rodina');CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);INSERT INTO categories VALUES(10,'Rodina',NULL,1),(11,'Deti',10,1);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE INDEX idx_assets_path ON assets(path);INSERT INTO folder_roots VALUES(1,'F:','F:\\');INSERT INTO folders VALUES(1,NULL,1,'F:','F:\\');INSERT INTO folders VALUES(2,1,1,'Photos','F:\\Photos');INSERT INTO assets VALUES(1,2,'one.jpg','jpg','jpg',1,123.0,10,10,24,NULL,NULL,NULL,2,0,'Red','2020-01-01T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\one.jpg');INSERT INTO assets VALUES(2,2,'two.jpg','jpg','jpg',1,456.0,10,10,24,NULL,NULL,NULL,4,1,NULL,'2020-01-02T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\two.jpg');").unwrap();
    crate::db::migrate_category_icons(&mut connection).unwrap();
    Arc::new(Mutex::new(connection))
}

fn keyword_fixture() -> Db {
    let db = write_fixture();
    db.lock().unwrap().execute_batch("CREATE TABLE keywords(id INTEGER PRIMARY KEY,name TEXT NOT NULL,parent_id INTEGER);INSERT INTO keywords VALUES(10,'Vianoce',NULL),(11,'Deti',10),(12,'Vnúčatá',11),(20,'Rodina',NULL);CREATE TABLE asset_keywords(asset_id INTEGER NOT NULL,keyword_id INTEGER NOT NULL,PRIMARY KEY(asset_id,keyword_id));INSERT INTO asset_keywords VALUES(1,10),(2,12);").unwrap();
    db
}

fn scope_fixture() -> Db {
    let db = keyword_fixture();
    db.lock().unwrap().execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER NOT NULL,color TEXT NOT NULL,state INTEGER NOT NULL,PRIMARY KEY(asset_id,color));
            INSERT INTO folders VALUES(3,1,1,'Ľudia','F:\\Ľudia');
            INSERT INTO assets VALUES(3,3,'lily.jpg','jpg','jpg',1,9.0,10,10,24,NULL,NULL,NULL,5,0,NULL,NULL,NULL,NULL,NULL,0,'F:\\Ľudia\\lily.jpg');
            INSERT INTO assets VALUES(4,2,'p_1%.jpg','jpg','jpg',1,9.0,10,10,24,NULL,NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,NULL,0,'F:\\Photos_old\\p_1%.jpg');
            INSERT INTO asset_categories VALUES(1,11),(3,11);INSERT INTO asset_color_overrides VALUES(1,'modrá',1),(3,'zelená',0);").unwrap();
    db
}

fn dump(db: &Db) -> Vec<String> {
    let conn = db.lock().unwrap();
    let mut out = Vec::new();
    for sql in [
        "SELECT id||':'||coalesce(path,'')||':'||coalesce(rating,'') FROM assets ORDER BY id",
        "SELECT asset_id||':'||category_id FROM asset_categories ORDER BY 1",
        "SELECT asset_id||':'||keyword_id FROM asset_keywords ORDER BY 1",
        "SELECT asset_id||':'||color||':'||state FROM asset_color_overrides ORDER BY 1",
    ] {
        let mut s = conn.prepare(sql).unwrap();
        out.extend(s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()));
    }
    out
}

fn keyword_rows(db: &Db) -> Vec<(i64, i64)> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT asset_id,keyword_id FROM asset_keywords ORDER BY asset_id,keyword_id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}
