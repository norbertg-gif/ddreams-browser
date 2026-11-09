use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FolderRule { pub path: String, pub mode: String, pub created: String }

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct FolderRuleState { pub effective: Option<String>, pub rule_path: Option<String>, pub explicit_mode: Option<String> }

#[derive(Default)]
pub struct FolderRules { cache: Mutex<Option<Vec<FolderRule>>> }

pub fn normalize(path: &str) -> String {
    let path = path.replace('/', "\\");
    let trimmed = path.trim_end_matches('\\');
    if trimmed.is_empty() && !path.is_empty() { "\\".into() } else { trimmed.to_lowercase() }
}

pub fn contains_path(parent: &str, path: &str) -> bool {
    let parent=normalize(parent); let path=normalize(path);
    !parent.is_empty() && (path == parent || (parent == "\\" && path.starts_with('\\')) || path.strip_prefix(&parent).is_some_and(|rest|rest.starts_with('\\')))
}

impl FolderRules {
    pub fn list(&self, conn: &Connection) -> Result<Vec<FolderRule>, String> {
        let mut cache=self.cache.lock().map_err(|e|e.to_string())?;
        if let Some(rules)=cache.as_ref() { return Ok(rules.clone()); }
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ddreams_folder_rules')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        let rules=if exists { let mut stmt=conn.prepare("SELECT path,mode,created FROM ddreams_folder_rules ORDER BY path COLLATE NOCASE").map_err(|e|e.to_string())?; let rows=stmt.query_map([],|r|Ok(FolderRule{path:r.get(0)?,mode:r.get(1)?,created:r.get(2)?})).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?; rows } else { Vec::new() };
        *cache=Some(rules.clone()); Ok(rules)
    }
    pub fn invalidate(&self) { if let Ok(mut cache)=self.cache.lock(){*cache=None;} }
    pub fn state(&self, conn:&Connection, path:&str)->Result<FolderRuleState,String>{
        let rules=self.list(conn)?; let normalized=normalize(path);
        let explicit=rules.iter().find(|rule|normalize(&rule.path)==normalized);
        let effective=rules.iter().filter(|rule|contains_path(&rule.path,path)).max_by_key(|rule|normalize(&rule.path).len());
        Ok(FolderRuleState{effective:effective.map(|r|r.mode.clone()),rule_path:effective.map(|r|r.path.clone()),explicit_mode:explicit.map(|r|r.mode.clone())})
    }
    pub fn guard(&self,conn:&Connection,ids:&[i64],listed:&[crate::journal::ListedAsset])->Result<(),String>{
        if self.list(conn)?.is_empty() { return Ok(()); }
        for id in ids {
            let path=if *id<0 { listed.iter().find(|a|a.temporary_id==*id).map(|a|a.folder_path.clone()) } else { conn.query_row("SELECT COALESCE(f.path,a.path) FROM assets a LEFT JOIN folders f ON f.id=a.folder_id WHERE a.id=?1",[id],|r|r.get::<_,Option<String>>(0)).optional().map_err(|e|e.to_string())?.flatten() };
            if let Some(path)=path { if self.state(conn,&path)?.effective.as_deref()==Some("exclude") {return Err("Priečinok je vylúčený z katalógu.".into());} }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn longest_prefix_and_component_boundary(){
        let conn=Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT); INSERT INTO ddreams_folder_rules VALUES('F:\\a','exclude','x'),('F:\\a\\b','include','y');").unwrap();
        let rules=FolderRules::default();
        assert_eq!(normalize("F:/A/B/"),"f:\\a\\b");
        assert_eq!(rules.state(&conn,"f:/A/B/c").unwrap().effective.as_deref(),Some("include"));
        assert_eq!(rules.state(&conn,"F:\\a\\bc").unwrap().effective.as_deref(),Some("exclude"));
        assert_eq!(rules.state(&conn,"F:\\abc").unwrap().effective,None);
    }
    #[test] fn guard_checks_catalogued_paths(){
        let conn=Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT); INSERT INTO ddreams_folder_rules VALUES('F:\\a','exclude','x'),('F:\\a\\b','include','y'); CREATE TABLE folders(id INTEGER PRIMARY KEY,path TEXT); INSERT INTO folders VALUES(1,'F:\\a'),(2,'F:\\a\\b'); CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,path TEXT); INSERT INTO assets VALUES(1,1,'F:\\a\\x.jpg'),(2,2,'F:\\a\\b\\x.jpg');").unwrap();
        let rules=FolderRules::default();
        assert_eq!(rules.guard(&conn,&[1],&[]).unwrap_err(),"Priečinok je vylúčený z katalógu.");
        assert!(rules.guard(&conn,&[2],&[]).is_ok());
    }
}
