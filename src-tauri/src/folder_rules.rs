use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FolderRule { pub path: String, pub mode: String, pub created: String }

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct FolderRuleState { pub effective: Option<String>, pub rule_path: Option<String>, pub explicit_mode: Option<String> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCatalogueState {
    #[serde(flatten)]
    rule: FolderRuleState,
    listed_count: i64,
    has_catalogued_descendant: bool,
    partially_excluded: bool,
}

#[derive(Default)]
pub struct FolderRules {
    cache: Mutex<Option<Vec<FolderRule>>>,
    counts: Mutex<Option<CatalogueCounts>>,
}

struct CatalogueCounts {
    revision: (u64, i64),
    by_path: BTreeMap<String, i64>,
}

// Keep the normalization expression identical to folders_norm_path. Driving the
// join from folders also lets SQLite use idx_assets_folder_listed for the count.
const LISTED_COUNTS_SQL: &str = r"
    SELECT lower(rtrim(replace(f.path,'/','\'),'\')), count(*)
    FROM folders f CROSS JOIN assets a ON a.folder_id = f.id AND a.is_listed = 1
    WHERE f.path IS NOT NULL AND f.path <> ''
    GROUP BY lower(rtrim(replace(f.path,'/','\'),'\'))
";

impl CatalogueCounts {
    fn read(conn: &Connection, revision: (u64, i64)) -> rusqlite::Result<Self> {
        let mut stmt = conn.prepare(LISTED_COUNTS_SQL)?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        let mut by_path = BTreeMap::new();
        for row in rows {
            let (path, count) = row?;
            // SQLite lower() only folds ASCII. Merge any remaining aliases in Rust.
            let key = if path.is_empty() { "\\".into() } else { normalize(&path) };
            *by_path.entry(key).or_insert(0) += count;
        }
        Ok(Self { revision, by_path })
    }

    fn has_descendant(&self, path: &str) -> bool {
        if path.is_empty() { return false; }
        let prefix = if path == "\\" { path.to_owned() } else { format!("{path}\\") };
        self.by_path.range((std::ops::Bound::Excluded(prefix.clone()), std::ops::Bound::Unbounded))
            .next().is_some_and(|(candidate, _)| candidate.starts_with(&prefix))
    }
}

pub fn normalize(path: &str) -> String {
    let path = path.replace('/', "\\");
    let trimmed = path.trim_end_matches('\\');
    if trimmed.is_empty() && !path.is_empty() { "\\".into() } else { trimmed.to_lowercase() }
}

fn contains_normalized(parent: &str, path: &str) -> bool {
    !parent.is_empty()
        && (path == parent || (parent == "\\" && path.starts_with('\\'))
            || path.strip_prefix(parent).is_some_and(|rest| rest.starts_with('\\')))
}

fn resolve(rules: &[(String, &FolderRule)], path: &str) -> FolderRuleState {
    let explicit = rules.iter().find(|(root, _)| root == path);
    let effective = rules.iter()
        .filter(|(root, _)| contains_normalized(root, path))
        .max_by_key(|(root, _)| root.len());
    FolderRuleState {
        effective: effective.map(|(_, rule)| rule.mode.clone()),
        rule_path: effective.map(|(_, rule)| rule.path.clone()),
        explicit_mode: explicit.map(|(_, rule)| rule.mode.clone()),
    }
}

impl FolderRules {
    pub fn list(&self, conn: &Connection) -> Result<Vec<FolderRule>, String> {
        let mut cache=self.cache.lock().map_err(|e|e.to_string())?;
        if let Some(rules)=cache.as_ref() { return Ok(rules.clone()); }
        let rules = Self::read_rules(conn)?;
        *cache=Some(rules.clone()); Ok(rules)
    }

    fn read_rules(conn: &Connection) -> Result<Vec<FolderRule>, String> {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ddreams_folder_rules')",
            [], |row| row.get(0),
        ).map_err(|error| error.to_string())?;
        if !exists { return Ok(Vec::new()); }
        let mut stmt = conn.prepare("SELECT path,mode,created FROM ddreams_folder_rules ORDER BY path COLLATE NOCASE")
            .map_err(|error| error.to_string())?;
        let rows = stmt.query_map([], |row| Ok(FolderRule { path: row.get(0)?, mode: row.get(1)?, created: row.get(2)? }))
            .map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    pub fn reload(&self, conn: &Connection) {
        let rules = Self::read_rules(conn).ok();
        if let Ok(mut cache) = self.cache.lock() {
            *cache = rules;
        }
    }

    pub fn state(&self, conn:&Connection, path:&str)->Result<FolderRuleState,String>{
        let rules = self.list(conn)?;
        let rules: Vec<_> = rules.iter().map(|rule| (normalize(&rule.path), rule)).collect();
        Ok(resolve(&rules, &normalize(path)))
    }

    pub fn catalogue_states(&self, conn: &Connection, paths: Vec<String>) -> Result<HashMap<String, FolderCatalogueState>, String> {
        if paths.is_empty() { return Ok(HashMap::new()); }
        let version = conn.query_row("PRAGMA main.data_version", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let revision = (conn.total_changes(), version);
        let mut cache = self.counts.lock().map_err(|error| error.to_string())?;
        if cache.as_ref().is_none_or(|counts| counts.revision != revision) {
            *cache = Some(CatalogueCounts::read(conn, revision).map_err(|error| error.to_string())?);
        }
        let counts = cache.as_ref().expect("folder counts were loaded");
        let rules = self.list(conn)?;
        let rules: Vec<_> = rules.iter().map(|rule| (normalize(&rule.path), rule)).collect();
        let exclusions: Vec<_> = rules.iter().filter(|(_, rule)| rule.mode == "exclude")
            .map(|(root, _)| root.as_str()).collect();
        Ok(paths.into_iter().map(|path| {
            let normalized = normalize(&path);
            let rule = resolve(&rules, &normalized);
            let listed_count = counts.by_path.get(&normalized).copied().unwrap_or(0);
            let has_catalogued_descendant = counts.has_descendant(&normalized);
            let partially_excluded = rule.effective.as_deref() != Some("exclude")
                && (listed_count > 0 || rule.effective.as_deref() == Some("include"))
                && exclusions.iter().any(|root| *root != normalized && contains_normalized(&normalized, root));
            (path, FolderCatalogueState { rule, listed_count, has_catalogued_descendant, partially_excluded })
        }).collect())
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
    #[test]
    fn catalogue_states_count_listed_assets_and_apply_folder_rules() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(r"
            CREATE TABLE folders(id INTEGER PRIMARY KEY, path TEXT);
            CREATE TABLE assets(id INTEGER PRIMARY KEY, folder_id INTEGER, is_listed INTEGER);
            CREATE INDEX folders_norm_path ON folders(lower(rtrim(replace(path,'/','\'),'\')));
            CREATE INDEX idx_assets_folder_listed ON assets(folder_id,is_listed);
            INSERT INTO folders VALUES
                (1,'F:\Photos'), (2,'f:/photos/'), (3,'F:\Photos\Trip\Day'),
                (4,'F:\Photos\Private'), (5,'F:\PhotosExtra\Day'),
                (6,'F:\Ľudia\Časť'), (7,'f:/ľudia/časť/'), (8,'F:\Unlisted');
            INSERT INTO assets VALUES
                (1,1,1), (2,1,0), (3,2,1), (4,3,1), (5,4,1),
                (6,5,1), (7,6,1), (8,7,1), (9,8,0);
            CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY, mode TEXT, created TEXT);
            INSERT INTO ddreams_folder_rules VALUES
                ('F:\Photos\Private', 'exclude', 'x'),
                ('F:\Inbox', 'include', 'x'),
                ('F:\Inbox\Work\Private', 'exclude', 'x');
        ").unwrap();
        let mut plan = conn.prepare(&format!("EXPLAIN QUERY PLAN {LISTED_COUNTS_SQL}")).unwrap();
        let plan = plan.query_map([], |row| row.get::<_, String>(3)).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
        assert!(plan.contains("folders_norm_path") && plan.contains("idx_assets_folder_listed"), "{plan}");
        let rules = FolderRules::default();
        rules.reload(&conn);
        let paths = [
            "F:/PHOTOS/", r"F:\Photos\Trip", r"F:\Photos\Trip\Day", r"F:\Photos\Private",
            r"F:\Photos\Private\Sub", r"F:\PhotosExtr", r"F:\ĽUDIA\ČASŤ", r"F:\Unlisted",
            r"F:\Inbox", r"F:\Inbox\Work", r"F:\Inbox\Empty",
        ];
        let load = || rules.catalogue_states(&conn, paths.iter().map(|path| path.to_string()).collect()).unwrap();
        let states = load();
        let expected = [
            (2, true, None, None, true),
            (0, true, None, None, false),
            (1, false, None, None, false),
            (1, false, Some("exclude"), Some("exclude"), false),
            (0, false, Some("exclude"), None, false),
            (0, false, None, None, false),
            (2, false, None, None, false),
            (0, false, None, None, false),
            (0, false, Some("include"), Some("include"), true),
            (0, false, Some("include"), None, true),
            (0, false, Some("include"), None, false),
        ];
        for (path, expected) in paths.into_iter().zip(expected) {
            let state = &states[path];
            assert_eq!((state.listed_count, state.has_catalogued_descendant, state.rule.effective.as_deref(),
                state.rule.explicit_mode.as_deref(), state.partially_excluded), expected, "{path}");
        }
        conn.execute("UPDATE assets SET is_listed=0 WHERE id=4", []).unwrap();
        let states = load();
        assert!(!states[paths[1]].has_catalogued_descendant);
        assert_eq!(states[paths[2]].listed_count, 0);
        conn.execute("DELETE FROM ddreams_folder_rules WHERE mode='exclude'", []).unwrap();
        rules.reload(&conn);
        let states = load();
        assert!(!states[paths[0]].partially_excluded);
    }
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
