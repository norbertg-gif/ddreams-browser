use super::{folder_by_path, folders_by_paths};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

#[test]
fn indexed_path_and_batch_match_single_lookup() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT); CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,is_listed INTEGER); CREATE INDEX folders_norm_path ON folders(lower(rtrim(replace(path,'/','\\'),'\\'))); CREATE INDEX idx_assets_folder_listed ON assets(folder_id,is_listed); INSERT INTO folders VALUES(1,NULL,1,'A','C:\\Photos\\A'),(2,NULL,2,'B','C:/Photos/B/'); INSERT INTO assets VALUES(1,1,1),(2,1,0);").unwrap();
    let plan: String = conn.query_row("EXPLAIN QUERY PLAN SELECT id FROM folders WHERE lower(rtrim(replace(path,'/','\\'),'\\'))=lower(rtrim(replace(?,'/','\\'),'\\')) LIMIT 1", ["c:/photos/a/"], |r| r.get(3)).unwrap();
    assert!(plan.contains("folders_norm_path"), "{plan}");
    let db = Arc::new(Mutex::new(conn));
    let paths = vec![
        "c:/photos/a/".to_string(),
        "C:\\PHOTOS\\B\\".to_string(),
        "C:/missing".to_string(),
    ];
    let batch = folders_by_paths(&db, &paths).unwrap();
    for path in paths {
        let single = folder_by_path(&db, &path).unwrap();
        assert_eq!(
            batch.get(&path).map(|f| (f.id, f.count)),
            single.map(|f| (f.id, f.count))
        );
    }
    assert_eq!(batch.get("c:/photos/a/").unwrap().count, 1);
}
