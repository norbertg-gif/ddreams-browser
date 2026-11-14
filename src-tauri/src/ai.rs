use crate::{db::Db, formats::FormatTable, paths::Paths};
use serde::Serialize;
use serde_json::{json, Value};
use rusqlite::OptionalExtension;
use std::{collections::HashMap, fs::{self, File, OpenOptions}, io::{BufRead, BufReader, BufWriter, Write}, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}, sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc, Arc, Mutex}, thread, time::{Duration, UNIX_EPOCH}};
use tauri::Emitter;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const SERIES_PRESETS: &[(&str, f64, f64)] = &[("strict", 3.0, 0.93), ("normal", 10.0, 0.90), ("loose", 60.0, 0.86)];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    pub available: bool,
    pub reason: Option<String>,
    pub state: String,
    pub indexed: u64,
    pub queued: u64,
    pub done: u64,
    pub failed: u64,
    pub offline: u64,
    pub total: u64,
    pub rate_per_s: f64,
    pub eta_s: Option<f64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchResult { pub ids: Vec<i64>, pub searched: u64 }

impl AiStatus {
    fn unavailable(reason: String) -> Self {
        Self { available: false, reason: Some(reason), state: "off".into(), indexed: 0, queued: 0, done: 0, failed: 0, offline: 0, total: 0, rate_per_s: 0.0, eta_s: None, last_error: None }
    }
}

struct ProcessState {
    child: Option<Child>,
    writer: Option<Arc<Mutex<ChildStdin>>>,
}

struct AiInner {
    paths: Paths,
    db: Db,
    formats: Arc<FormatTable>,
    app: Mutex<Option<tauri::AppHandle>>,
    process: Mutex<ProcessState>,
    status: Mutex<AiStatus>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>,
    next_id: AtomicU64,
    stopping: AtomicBool,
    auto_index_started: AtomicBool,
    colors_attached: AtomicBool,
    color_learn_generation: AtomicU64,
    label_generation: AtomicU64,
    last_stderr: Mutex<String>,
    stderr_reader: Mutex<Option<thread::JoinHandle<()>>>,
    similarity_order_cache: Mutex<HashMap<u64, Vec<i64>>>,
}

#[derive(Clone)]
pub struct AiService(Arc<AiInner>);

pub struct AiState(pub AiService);

impl AiService {
    pub fn new(paths: Paths, db: Db, formats: Arc<FormatTable>) -> Self {
        let reason = availability_reason(&paths);
        let status = reason.map(AiStatus::unavailable).unwrap_or(AiStatus {
            available: true, reason: None, state: "off".into(), indexed: 0, queued: 0, done: 0, failed: 0, offline: 0, total: 0, rate_per_s: 0.0, eta_s: None, last_error: None,
        });
        Self(Arc::new(AiInner {
            paths, db, formats, app: Mutex::new(None), process: Mutex::new(ProcessState { child: None, writer: None }),
            status: Mutex::new(status), pending: Mutex::new(HashMap::new()), next_id: AtomicU64::new(1), stopping: AtomicBool::new(false),
            auto_index_started: AtomicBool::new(false), last_stderr: Mutex::new(String::new()), stderr_reader: Mutex::new(None),
            colors_attached: AtomicBool::new(false),
            color_learn_generation: AtomicU64::new(0),
            label_generation: AtomicU64::new(0),
            similarity_order_cache: Mutex::new(HashMap::new()),
        }))
    }

    pub fn status(&self) -> AiStatus { self.0.status.lock().unwrap().clone() }

    pub fn ai_db_path(&self) -> PathBuf { self.0.paths.ai_db.clone() }

    pub fn ensure_colors_attached(&self) -> bool {
        if self.0.colors_attached.load(Ordering::Relaxed) { return self.has_colors_table(); }
        if !self.0.paths.ai_db.is_file() { return false; }
        let path=self.0.paths.ai_db.to_string_lossy().replace('\\',"/").replace(' ',"%20");
        let uri=format!("file:{path}?mode=ro");
        let Ok(conn)=self.0.db.lock() else{return false};
        if conn.execute("ATTACH DATABASE ?1 AS ai",[uri]).is_ok() { self.0.colors_attached.store(true,Ordering::Relaxed);drop(conn);self.has_colors_table() } else { false }
    }

    fn has_colors_table(&self)->bool{self.0.db.lock().ok().and_then(|conn|conn.query_row("SELECT EXISTS(SELECT 1 FROM ai.sqlite_master WHERE type='table' AND name='colors')",[],|r|r.get::<_,bool>(0)).ok()).unwrap_or(false)}

    pub fn photosort_corrections_path(&self) -> Option<PathBuf> { self.0.paths.ai_seed_colors.as_ref().map(|path|path.join("color_corrections.json")) }

    pub fn learn_colors_debounced(&self) {
        let generation=self.0.color_learn_generation.fetch_add(1,Ordering::Relaxed)+1;
        let service=self.clone();
        let _=thread::Builder::new().name("ai-color-learn-debounce".into()).spawn(move||{
            thread::sleep(Duration::from_secs(2));
            if service.0.color_learn_generation.load(Ordering::Relaxed)!=generation || !service.status().available { return; }
            let rows={let Ok(conn)=service.0.db.lock()else{return};let Ok(mut stmt)=conn.prepare("SELECT asset_id,color,state FROM asset_color_overrides ORDER BY asset_id,color")else{return};let Ok(rows)=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))else{return};rows.filter_map(Result::ok).map(|(asset_id,color,state)|json!({"asset_id":asset_id,"color":color,"state":state!=0})).collect::<Vec<_>>()};
            if service.0.process.lock().ok().and_then(|p|p.writer.clone()).is_some(){let _=service.request("learn_colors",json!({"corrections":rows}),Duration::from_secs(60));}
        });
    }

    pub fn sync_labels_debounced(&self,kind:&str){let Ok(kind)=valid_label_kind(kind)else{return};let generation=self.0.label_generation.fetch_add(1,Ordering::Relaxed)+1;let service=self.clone();let kind=kind.to_owned();let _=thread::Builder::new().name("ai-label-sync-debounce".into()).spawn(move||{thread::sleep(Duration::from_secs(3));if service.0.label_generation.load(Ordering::Relaxed)!=generation||!service.status().available{return;}let _=service.sync_labels(&kind);});}

    fn sync_labels(&self,kind:&str)->Result<(),String>{
        let (pairs,parents,listed)={let conn=self.0.db.lock().map_err(|e|e.to_string())?;let (relation,label,table)=if kind=="category" {("asset_categories","category_id","categories")}else{("asset_keywords","keyword_id","keywords")};let sql=format!("SELECT rel.asset_id,rel.{label} FROM {relation} rel JOIN assets a ON a.id=rel.asset_id WHERE a.is_listed=1 ORDER BY rel.asset_id,rel.{label}");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let pairs=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let sql=format!("SELECT id,parent_id FROM {table}");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let mut stmt=conn.prepare("SELECT id FROM assets WHERE is_listed=1 AND id>0").map_err(|e|e.to_string())?;let listed=stmt.query_map([],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<std::collections::HashSet<_>>>().map_err(|e|e.to_string())?;(pairs,rows,listed)};
        self.request("set_labels",build_label_payload(kind,&pairs,&parents,&listed),Duration::from_secs(60))?;Ok(())
    }

    pub fn set_app(&self, app: tauri::AppHandle) { *self.0.app.lock().unwrap() = Some(app); }

    pub fn start(&self) -> Result<AiStatus, String> {
        if !self.status().available { return Ok(self.status()); }
        let mut process = self.0.process.lock().map_err(|e| e.to_string())?;
        if let Some(child) = process.child.as_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() { return Ok(self.status()); }
            process.child = None;
            process.writer = None;
        }
        let python = self.0.paths.ai_python.as_ref().ok_or_else(|| "Python AI nie je nastavený".to_string())?;
        let log_path = self.0.paths.ai_log.clone();
        if let Some(parent) = log_path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        if fs::metadata(&log_path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            File::create(&log_path).map_err(|e| e.to_string())?;
        }
        let mut command = Command::new(python);
        command.arg("-I").arg(&self.0.paths.ai_script).arg("--model").arg(&self.0.paths.ai_model).arg("--db").arg(&self.0.paths.ai_db);
        if let Some(seed) = &self.0.paths.ai_seed_colors { command.arg("--seed-colors").arg(seed); }
        command
            .env("PYTHONIOENCODING", "utf-8").env("HF_HUB_OFFLINE", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|error| {
            let message = format!("AI server sa nepodarilo spustiť: {error}");
            self.set_error(message.clone());
            message
        })?;
        let stdin = child.stdin.take().ok_or_else(|| "AI server nemá vstupný kanál".to_string())?;
        let stdout = child.stdout.take().ok_or_else(|| "AI server nemá výstupný kanál".to_string())?;
        let stderr = child.stderr.take().ok_or_else(|| "AI server nemá chybový kanál".to_string())?;
        process.writer = Some(Arc::new(Mutex::new(stdin)));
        process.child = Some(child);
        self.0.stopping.store(false, Ordering::Relaxed);
        self.0.auto_index_started.store(false, Ordering::Relaxed);
        self.0.last_stderr.lock().unwrap().clear();
        {
            let mut status = self.0.status.lock().unwrap();
            status.state = "loading".into();
            status.last_error = None;
            status.total = 0;
            status.done = 0;
            status.failed = 0;
            status.offline = 0;
            self.emit(&status);
        }
        let service = self.clone();
        let stderr_thread = thread::Builder::new().name("ai-stderr".into()).spawn(move || service.read_stderr(stderr, log_path)).map_err(|e| e.to_string())?;
        *self.0.stderr_reader.lock().unwrap() = Some(stderr_thread);
        let service = self.clone();
        thread::Builder::new().name("ai-stdout".into()).spawn(move || service.read_stdout(stdout)).map_err(|e| e.to_string())?;
        Ok(self.status())
    }

    pub fn stop(&self) -> Result<AiStatus, String> {
        let running = self.0.process.lock().map_err(|e| e.to_string())?.child.is_some();
        if !running { return Ok(self.status()); }
        self.0.stopping.store(true, Ordering::Relaxed);
        let _ = self.request("shutdown", json!({}), Duration::from_secs(2));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut process = self.0.process.lock().map_err(|e| e.to_string())?;
        if let Some(child) = process.child.as_mut() {
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if std::time::Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                    _ => { let _ = child.kill(); let _ = child.wait(); break; }
                }
            }
        }
        process.writer = None;
        process.child = None;
        drop(process);
        if let Some(reader) = self.0.stderr_reader.lock().unwrap().take() { let _ = reader.join(); }
        let mut status = self.0.status.lock().unwrap();
        if status.state != "error" { status.state = "off".into(); }
        self.emit(&status);
        Ok(status.clone())
    }

    pub fn pause(&self) -> Result<AiStatus, String> { self.command_status("pause") }
    pub fn resume(&self) -> Result<AiStatus, String> { self.command_status("resume") }

    fn command_status(&self, cmd: &str) -> Result<AiStatus, String> {
        self.request(cmd, json!({}), Duration::from_secs(60))?;
        Ok(self.status())
    }

    pub fn reindex(&self) -> Result<AiStatus, String> {
        if !self.status().available { return Ok(self.status()); }
        if self.0.process.lock().map_err(|e| e.to_string())?.child.is_none() { return Err("AI server nie je spustený".into()); }
        let service = self.clone();
        thread::Builder::new().name("ai-reindex".into()).spawn(move || {
            if let Err(error) = service.index_catalog() { service.set_error(error); }
        }).map_err(|e| e.to_string())?;
        Ok(self.status())
    }

    pub fn search_text(&self, query: String, limit: Option<u32>) -> Result<AiSearchResult, String> {
        let result = self.request("search_text", json!({ "query": query, "limit": limit.unwrap_or(200).clamp(1, 1000) }), Duration::from_secs(30))?;
        result_page(result)
    }

    pub fn similar(&self, asset_id: i64, limit: Option<u32>) -> Result<AiSearchResult, String> {
        if asset_id <= 0 { return Err("Fotka ešte nie je zaindexovaná".into()); }
        let result = self.request("similar", json!({ "asset_id": asset_id, "limit": limit.unwrap_or(200).clamp(1, 1000) }), Duration::from_secs(60))?;
        result_page(result)
    }

    /// Duplicates of ONE photo: "similar" = cosine ≥ 0.93 (bursts, same scene); "same" = cosine ≥ 0.95 and
    /// the same base name with non-conflicting capture time (copies, RAW+JPG, edited exports) — the same
    /// rules as the catalogue-wide scan. Returns the photo itself first, then the matches by similarity.
    pub fn duplicates_of(&self, asset_id:i64, level:&str)->Result<Vec<i64>,String>{
        if asset_id <= 0 { return Err("Fotka ešte nie je v katalógu".into()); }
        let threshold=match level{"same"=>0.95,"similar"=>0.93,_=>return Err("Neplatná úroveň podobnosti".into())};
        let result=self.request("similar",json!({"asset_id":asset_id,"limit":1000}),Duration::from_secs(60))?;
        let rows=result.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
        let candidates=rows.iter().filter(|r|r.get("score").and_then(Value::as_f64).unwrap_or(0.0)>=threshold).filter_map(|r|r.get("asset_id").and_then(Value::as_i64)).filter(|id|*id!=asset_id).collect::<Vec<_>>();
        let conn=self.0.db.lock().map_err(|e|e.to_string())?;
        let mut meta=std::collections::HashMap::new();
        for id in std::iter::once(asset_id).chain(candidates.iter().copied()) {
            if let Some(row)=conn.query_row("SELECT name,exif_date,size FROM assets WHERE id=?1 AND is_listed=1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<f64>>(2)?))).optional().map_err(|e|e.to_string())? { meta.insert(id,row); }
        }
        let listed=candidates.into_iter().filter(|id|meta.contains_key(id)).collect::<Vec<_>>();
        let matches=if level=="same" {
            let ids=std::iter::once(asset_id).chain(listed.iter().copied()).collect::<Vec<_>>();
            let group=split_same_groups(&ids,&meta).into_iter().find(|g|g.contains(&asset_id)).unwrap_or_default();
            listed.into_iter().filter(|id|group.contains(id)).collect::<Vec<_>>()
        } else { listed };
        if matches.is_empty() { return Ok(Vec::new()); }
        Ok(std::iter::once(asset_id).chain(matches).collect())
    }

    pub fn duplicate_groups(&self, level:&str)->Result<Value,String>{
        // Burst neighbours reach cosine 0.999, so pixels alone can't tell a copy from the next frame.
        // "same" = visually close (≥ 0.95) AND the same base name AND capture times not contradicting.
        let threshold=match level{"same"=>0.95,"similar"=>0.93,_=>return Err("Neplatná úroveň podobnosti".into())};
        let result=self.request("duplicate_groups",json!({"threshold":threshold}),Duration::from_secs(300))?;
        let conn=self.0.db.lock().map_err(|e|e.to_string())?;let rows=result.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();let mut stmt=conn.prepare("SELECT id,name,exif_date,size FROM assets WHERE is_listed=1").map_err(|e|e.to_string())?;let meta=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,(r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<f64>>(3)?)))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<std::collections::HashMap<_,_>>>().map_err(|e|e.to_string())?;let listed=meta.keys().copied().collect::<std::collections::HashSet<_>>();let accepted=filter_duplicate_groups(&rows,&listed);let mut groups=Vec::new();
        for (index,ids) in accepted{let row=&rows[index];if level=="same"{for part in split_same_groups(&ids,&meta){groups.push(json!({"assetIds":part,"minSim":row.get("min_sim"),"maxSim":row.get("max_sim")}));}}else{groups.push(json!({"assetIds":ids,"minSim":row.get("min_sim"),"maxSim":row.get("max_sim")}));}}
        groups.sort_by_key(|g|std::cmp::Reverse(g["assetIds"].as_array().map(|a|a.len()).unwrap_or(0)));
        Ok(json!({"groups":groups}))
    }

    pub fn analyze_series(&self, items: Vec<Value>, preset: &str) -> Result<Value, String> {
        let (_, max_gap, min_sim) = SERIES_PRESETS.iter().copied().find(|(name, _, _)| *name == preset).unwrap_or(SERIES_PRESETS[1]);
        self.request("series", json!({"items":items,"max_gap":max_gap,"min_sim":min_sim}), Duration::from_secs(300))
    }

    pub fn order_by_similarity(&self,asset_ids:Vec<i64>)->Result<Vec<i64>,String>{
        use std::hash::{Hash,Hasher};let mut hasher=std::collections::hash_map::DefaultHasher::new();asset_ids.hash(&mut hasher);let key=hasher.finish();
        if let Some(ids)=self.0.similarity_order_cache.lock().map_err(|e|e.to_string())?.get(&key).cloned(){return Ok(ids)}
        let result=self.request("order_by_similarity",json!({"asset_ids":asset_ids}),Duration::from_secs(300))?;let ids=result.get("asset_ids").and_then(Value::as_array).ok_or("AI vrátilo neplatné poradie")?.iter().filter_map(Value::as_i64).collect::<Vec<_>>();
        let mut cache=self.0.similarity_order_cache.lock().map_err(|e|e.to_string())?;if cache.len()>128{cache.clear()}cache.insert(key,ids.clone());Ok(ids)
    }

    pub fn suggest(&self, asset_ids: Vec<i64>, kind: &str) -> Result<Value, String> {
        let kind = valid_label_kind(kind)?;
        if asset_ids.is_empty() { return Ok(json!([])); }
        let combined = asset_ids.len() > 1;
        let response = self.request("suggest", json!({"asset_ids":asset_ids,"kind":kind,"limit":5}), Duration::from_secs(60))?;
        let rows = if combined { response.get("combined").and_then(Value::as_array).cloned().unwrap_or_default() } else { response.get("results").and_then(|v|v.get(asset_ids[0].to_string())).and_then(Value::as_array).cloned().unwrap_or_default() };
        let conn=self.0.db.lock().map_err(|e|e.to_string())?;
        let mut out=Vec::new();
        for row in rows {
            let Some(id)=row.get("label_id").and_then(Value::as_i64) else {continue};
            let path=label_path(&conn,kind,id)?;
            let candidates=if combined { asset_ids.iter().copied().filter(|aid|response.get("results").and_then(|v|v.get(aid.to_string())).and_then(Value::as_array).is_some_and(|items|items.iter().any(|item|item.get("label_id").and_then(Value::as_i64)==Some(id)))).collect::<Vec<_>>() } else { asset_ids.clone() };
            let rejected=candidates.iter().filter_map(|aid|conn.query_row("SELECT EXISTS(SELECT 1 FROM ai_suggestion_rejections WHERE asset_id=?1 AND kind=?2 AND label_id=?3)",rusqlite::params![aid,kind,id],|r|r.get::<_,bool>(0)).ok().filter(|rejected|*rejected).map(|_|*aid)).collect::<std::collections::HashSet<_>>();
            let accepted=filter_suggest_assets(&candidates,&rejected);
            if accepted.is_empty() {continue;}
            let count=if combined {accepted.len() as u64} else {row.get("support").and_then(Value::as_u64).unwrap_or(0)};
            out.push(json!({"labelId":id,"path":path,"score":row.get("score").and_then(Value::as_f64).unwrap_or(0.0),"count":count,"assetIds":accepted}));
        }
        Ok(Value::Array(out))
    }

    pub fn reject_suggestion(&self, asset_ids:&[i64], kind:&str, label_id:i64)->Result<usize,String>{
        let kind=valid_label_kind(kind)?; let conn=self.0.db.lock().map_err(|e|e.to_string())?; let mut n=0;
        for id in asset_ids { n+=conn.execute("INSERT OR IGNORE INTO ai_suggestion_rejections(asset_id,kind,label_id) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)",rusqlite::params![id,kind,label_id]).map_err(|e|e.to_string())?; } Ok(n)
    }

    pub fn suggestion_groups(&self,kind:&str)->Result<Value,String>{let kind=valid_label_kind(kind)?;let result=self.request("suggest_batch",json!({"kind":kind,"asset_ids":"unlabeled"}),Duration::from_secs(300))?;let groups=result.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();let conn=self.0.db.lock().map_err(|e|e.to_string())?;let mut out=Vec::new();for group in groups{let Some(label_id)=group.get("label_id").and_then(Value::as_i64)else{continue};let ids=group.get("asset_ids").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|v|v.as_i64()).collect::<Vec<_>>();let mut listed=std::collections::HashSet::new();let mut rejected=std::collections::HashSet::new();for id in &ids{if conn.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)",[id],|r|r.get::<_,bool>(0)).unwrap_or(false){listed.insert(*id);}if conn.query_row("SELECT EXISTS(SELECT 1 FROM ai_suggestion_rejections WHERE asset_id=?1 AND kind=?2 AND label_id=?3)",rusqlite::params![id,kind,label_id],|r|r.get::<_,bool>(0)).unwrap_or(false){rejected.insert(*id);}}let accepted=filter_group_assets(&ids,&listed,&rejected);if !accepted.is_empty(){out.push(json!({"labelId":label_id,"path":label_path(&conn,kind,label_id)?,"count":accepted.len(),"assetIds":accepted}));}}out.sort_by(|a,b|b["count"].as_u64().cmp(&a["count"].as_u64()));Ok(Value::Array(out))}

    pub fn unsorted_clusters(&self,min_size:Option<u32>)->Result<Value,String>{
        let mut args=json!({"limit":30});if let Some(value)=min_size{args["min_size"]=json!(value);}
        let result=self.request("clusters",args,Duration::from_secs(300))?;
        let groups=result.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();
        let conn=self.0.db.lock().map_err(|e|e.to_string())?;let mut out=Vec::new();
        for group in groups{
            let ids=group.get("ids").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|v|v.as_i64()).collect::<Vec<_>>();
            let mut listed=std::collections::HashSet::new();for id in &ids{if conn.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>'')",[id],|r|r.get::<_,bool>(0)).unwrap_or(false){listed.insert(*id);}}
            let accepted=ids.into_iter().filter(|id|listed.contains(id)).collect::<Vec<_>>();if accepted.is_empty(){continue;}
            let label_id=group.get("label_id").and_then(Value::as_i64);let label_path=label_id.map(|id|label_path(&conn,"category",id)).transpose()?;
            out.push(json!({"ids":accepted,"total":group.get("total").and_then(Value::as_u64).unwrap_or(0),"cohesion":group.get("cohesion").and_then(Value::as_f64).unwrap_or(0.0),"labelId":label_id,"labelPath":label_path,"name":group.get("name").and_then(Value::as_str),"labelScore":group.get("label_score").and_then(Value::as_f64)}));
        }
        Ok(json!({"groups":out,"unlabeled":result.get("unlabeled").and_then(Value::as_u64).unwrap_or(0),"clustered":result.get("clustered").and_then(Value::as_u64).unwrap_or(0)}))
    }

    pub fn map_build(&self)->Result<Value,String>{self.request("map_build",json!({}),Duration::from_secs(600))}

    pub fn category_paths(&self)->Result<Vec<(i64,String)>,String>{
        let conn=self.0.db.lock().map_err(|e|e.to_string())?;
        let ids={let mut stmt=conn.prepare("SELECT id FROM categories").map_err(|e|e.to_string())?;let rows=stmt.query_map([],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;rows};
        let mut out=Vec::with_capacity(ids.len());for id in ids{if let Ok(path)=label_path(&conn,"category",id){out.push((id,path));}}
        out.sort_by(|a,b|a.1.to_lowercase().cmp(&b.1.to_lowercase()));Ok(out)
    }

    pub fn faces_status(&self)->Result<Value,String>{self.request("faces_status",json!({}),Duration::from_secs(30))}
    pub fn faces_persons(&self)->Result<Value,String>{self.request("faces_persons",json!({}),Duration::from_secs(30))}
    pub fn faces_person_assets(&self,person_id:i64,include_auto:bool)->Result<Value,String>{self.request("faces_person_assets",json!({"person_id":person_id,"include_auto":include_auto}),Duration::from_secs(30))}
    pub fn faces_review(&self,person_id:i64,limit:Option<u32>)->Result<Value,String>{self.request("faces_review",json!({"person_id":person_id,"limit":limit.unwrap_or(200).clamp(1,1000)}),Duration::from_secs(30))}
    pub fn faces_clusters(&self,min_size:Option<u32>)->Result<Value,String>{let args=min_size.map(|v|json!({"min_size":v})).unwrap_or_else(||json!({}));self.request("faces_clusters",args,Duration::from_secs(300))}
    pub fn faces_for_assets(&self,asset_ids:&[i64])->Result<Value,String>{self.request("faces_for_assets",json!({"asset_ids":asset_ids}),Duration::from_secs(30))}
    pub fn faces_set(&self,face_ids:&[i64],action:&str,person_id:Option<i64>)->Result<Value,String>{self.request("faces_set",json!({"face_ids":face_ids,"action":action,"person_id":person_id}),Duration::from_secs(30))}
    pub fn faces_asset_assign(&self,asset_ids:&[i64],person_id:i64)->Result<Value,String>{self.request("faces_asset_assign",json!({"asset_ids":asset_ids,"person_id":person_id}),Duration::from_secs(30))}
    pub fn faces_asset_remove(&self,asset_ids:&[i64],person_id:i64)->Result<Value,String>{self.request("faces_asset_remove",json!({"asset_ids":asset_ids,"person_id":person_id}),Duration::from_secs(30))}
    pub fn faces_person_create(&self,name:&str)->Result<Value,String>{self.request("faces_person_create",json!({"name":name}),Duration::from_secs(30))}
    pub fn faces_person_rename(&self,person_id:i64,name:&str)->Result<Value,String>{self.request("faces_person_rename",json!({"person_id":person_id,"name":name}),Duration::from_secs(30))}
    pub fn faces_recognize_all(&self)->Result<Value,String>{self.request("faces_recognize_all",json!({}),Duration::from_secs(300))}

    pub fn face_thumbs(&self,face_ids:&[i64])->Result<Vec<Value>,String>{
        if face_ids.len()>300{return Err("Najviac 300 náhľadov tvárí naraz".into());}
        let path=self.ai_db_path().parent().unwrap_or_else(||std::path::Path::new(".")).join("faces.sqlite");
        if !path.is_file(){return Ok(Vec::new());}
        let conn=rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e|e.to_string())?;
        let mut out=Vec::new();
        for chunk in face_ids.chunks(300){if chunk.is_empty(){continue;}let marks=vec!["?";chunk.len()].join(",");let sql=format!("SELECT face_id,jpeg FROM face_thumbs WHERE face_id IN ({marks})");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;for row in stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?))).map_err(|e|e.to_string())?{let(id,jpeg)=row.map_err(|e|e.to_string())?;use base64::Engine;let encoded=base64::engine::general_purpose::STANDARD.encode(jpeg);out.push(json!({"faceId":id,"dataUrl":format!("data:image/jpeg;base64,{encoded}")}));}}
        Ok(out)
    }

    pub fn cluster_ignore(&self,asset_ids:&[i64])->Result<usize,String>{
        let result=self.request("cluster_ignore",json!({"asset_ids":asset_ids}),Duration::from_secs(30))?;
        Ok(result.get("ignored").and_then(Value::as_u64).unwrap_or(0) as usize)
    }

    pub fn cluster_ignore_reset(&self)->Result<usize,String>{
        let result=self.request("cluster_ignore_reset",json!({}),Duration::from_secs(30))?;
        Ok(result.get("ignored").and_then(Value::as_u64).unwrap_or(0) as usize)
    }

    fn request(&self, cmd: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        let writer = self.0.process.lock().map_err(|e| e.to_string())?.writer.clone().ok_or_else(|| "AI server nie je spustený".to_string())?;
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.0.pending.lock().map_err(|e| e.to_string())?.insert(id, tx);
        let mut request = args.as_object().cloned().unwrap_or_default();
        request.insert("id".into(), json!(id));
        request.insert("cmd".into(), json!(cmd));
        let write_result = (|| {
            let mut stdin = writer.lock().map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut *stdin, &Value::Object(request)).map_err(|e| e.to_string())?;
            stdin.write_all(b"\n").map_err(|e| e.to_string())?;
            stdin.flush().map_err(|e| e.to_string())
        })();
        if let Err(error) = write_result {
            self.0.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.0.pending.lock().unwrap().remove(&id);
                Err(format!("AI server neodpovedal na {cmd} včas"))
            }
            Err(error) => { self.0.pending.lock().unwrap().remove(&id); Err(error.to_string()) }
        }
    }

    fn read_stdout(&self, stdout: impl std::io::Read) {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break; };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue; };
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                if let Some(sender) = self.0.pending.lock().unwrap().remove(&id) {
                    let result = if message.get("ok").and_then(Value::as_bool) == Some(true) {
                        message.get("result").cloned().ok_or_else(|| "AI server vrátil prázdny výsledok".into())
                    } else { Err(message.get("error").and_then(Value::as_str).unwrap_or("AI požiadavka zlyhala").to_string()) };
                    let _ = sender.send(result);
                }
            } else if matches!(message.get("event").and_then(Value::as_str), Some("status" | "progress")) {
                self.update_status(&message);
                if message.get("event").and_then(Value::as_str) == Some("status") && message.get("state").and_then(Value::as_str) == Some("ready")
                    && !self.0.auto_index_started.swap(true, Ordering::Relaxed) {
                    let _=self.ensure_colors_attached();
                    let service = self.clone();
                    let _ = thread::Builder::new().name("ai-auto-index".into()).spawn(move || {
                        if let Err(error) = service.index_catalog() { service.set_error(error); }
                    });
                }
            }
        }
        if let Some(reader) = self.0.stderr_reader.lock().unwrap().take() { let _ = reader.join(); }
        if !self.0.stopping.load(Ordering::Relaxed) {
            let detail = self.0.last_stderr.lock().unwrap().clone();
            let message = if detail.is_empty() { "AI server sa neočakávane ukončil".into() } else { format!("AI server sa neočakávane ukončil: {detail}") };
            self.set_error(message);
        }
        for (_, sender) in self.0.pending.lock().unwrap().drain() { let _ = sender.send(Err("AI server sa ukončil".into())); }
    }

    fn read_stderr(&self, stderr: impl std::io::Read, path: PathBuf) {
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        let mut writer = file.map(BufWriter::new);
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break; };
            *self.0.last_stderr.lock().unwrap() = line.clone();
            if let Some(writer) = writer.as_mut() { let _ = writeln!(writer, "{line}"); let _ = writer.flush(); }
        }
    }

    fn update_status(&self, message: &Value) {
        let mut status = self.0.status.lock().unwrap();
        if let Some(state) = message.get("state").and_then(Value::as_str) { status.state = state.into(); }
        if let Some(value) = message.get("indexed").and_then(Value::as_u64) { status.indexed = value; }
        if let Some(value) = message.get("queued").and_then(Value::as_u64) { status.queued = value; }
        if let Some(value) = message.get("done").and_then(Value::as_u64) { status.done = value; }
        if let Some(value) = message.get("failed").and_then(Value::as_u64) { status.failed = value; }
        if let Some(value) = message.get("offline").and_then(Value::as_u64) { status.offline = value; }
        if let Some(value) = message.get("rate_per_s").and_then(Value::as_f64) { status.rate_per_s = value; }
        if message.get("eta_s").is_some() { status.eta_s = message.get("eta_s").and_then(Value::as_f64); }
        if message.get("last_error").is_some() { status.last_error = message.get("last_error").and_then(Value::as_str).map(str::to_string); }
        self.emit(&status);
    }

    fn emit(&self, status: &AiStatus) {
        if let Some(app) = self.0.app.lock().unwrap().as_ref() { let _ = app.emit("ai-status", status.clone()); }
    }

    fn set_error(&self, message: String) {
        let mut status = self.0.status.lock().unwrap();
        status.state = "error".into();
        status.last_error = Some(message);
        self.emit(&status);
    }

    fn index_catalog(&self) -> Result<(), String> {
        // Labels first: they don't depend on the index, and stat-ing ~90k files below can take minutes.
        self.sync_labels("category")?;self.sync_labels("keyword")?;
        let extensions = self.0.formats.extensions(&["image", "raw"]);
        if extensions.is_empty() { return Err("Nie sú nakonfigurované formáty fotografií".into()); }
        let sql = format!("SELECT id,path,size FROM assets WHERE is_listed=1 AND id>0 AND path IS NOT NULL AND trim(path)<>'' AND ext_lower IN ({})", extensions.iter().map(|ext| format!("'{ext}'")).collect::<Vec<_>>().join(","));
        let rows = {
            let conn = self.0.db.lock().map_err(|e| e.to_string())?;
            let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
            statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<f64>>(2)?)))
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
        };
        let items = rows.into_iter().map(|(asset_id, path, catalog_size)| {
            match fs::metadata(&path).and_then(|meta| Ok((meta.len(), meta.modified()?))) {
                Ok((size, modified)) => {
                    let mtime_ns = modified.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().min(i64::MAX as u128) as i64;
                    json!({"asset_id":asset_id,"path":path,"size":size.min(i64::MAX as u64),"mtime_ns":mtime_ns})
                }
                Err(_) => json!({"asset_id":asset_id,"path":path,"size":catalog_size.unwrap_or(0.0).max(0.0).min(i64::MAX as f64) as u64,"mtime_ns":0}),
            }
        }).collect::<Vec<_>>();
        let result = self.request("index", json!({ "items": items, "prune": true }), Duration::from_secs(60))?;
        let mut status = self.0.status.lock().unwrap();
        status.total = result.get("queued").and_then(Value::as_u64).unwrap_or(0)
            + result.get("up_to_date").and_then(Value::as_u64).unwrap_or(0)
            + result.get("failed_before").and_then(Value::as_u64).unwrap_or(0);
        self.emit(&status);
        drop(status);
        Ok(())
    }

    /// Queues newly catalogued photos without touching the running queue (best effort: AI may be off).
    pub fn index_assets(&self, asset_ids:&[i64]) {
        if asset_ids.is_empty() { return; }
        let extensions=self.0.formats.extensions(&["image","raw"]);
        let rows={
            let Ok(conn)=self.0.db.lock() else { return };
            let mut out=Vec::new();
            for id in asset_ids { if let Ok(Some((path,ext))) = conn.query_row("SELECT path,ext_lower FROM assets WHERE id=?1 AND is_listed=1 AND path IS NOT NULL",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional() { if extensions.iter().any(|e|e==&ext) { out.push((*id,path)); } } }
            out
        };
        let items=rows.into_iter().filter_map(|(asset_id,path)|{let meta=fs::metadata(&path).ok()?;let mtime_ns=meta.modified().ok()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().min(i64::MAX as u128) as i64;Some(json!({"asset_id":asset_id,"path":path,"size":meta.len().min(i64::MAX as u64),"mtime_ns":mtime_ns}))}).collect::<Vec<_>>();
        if items.is_empty() { return; }
        let service=self.clone();
        let _=thread::Builder::new().name("ai-index-new".into()).spawn(move||{let _=service.request("index",json!({"items":items,"append":true}),Duration::from_secs(60));});
    }

    /// Removes assets that left the catalogue from the AI index (best effort: AI may be off).
    pub fn forget(&self, asset_ids:Vec<i64>) {
        if asset_ids.is_empty() { return; }
        let service=self.clone();
        let _=thread::Builder::new().name("ai-forget".into()).spawn(move||{let _=service.request("forget",json!({"asset_ids":asset_ids}),Duration::from_secs(60));});
    }
}

fn result_page(result: Value) -> Result<AiSearchResult, String> {
    let rows = result.get("results").and_then(Value::as_array).ok_or_else(|| "AI server vrátil neplatný zoznam".to_string())?;
    let ids = rows.iter().map(|row| row.get("asset_id").and_then(Value::as_i64).ok_or_else(|| "AI server vrátil neplatné asset_id".to_string())).collect::<Result<Vec<_>,_>>()?;
    let searched = result.get("searched").and_then(Value::as_u64).ok_or_else(|| "AI server nevrátil počet prehľadaných fotiek".to_string())?;
    Ok(AiSearchResult { ids, searched })
}

fn valid_label_kind(kind:&str)->Result<&str,String>{match kind{"category"=>Ok("category"),"keyword"=>Ok("keyword"),_=>Err("Neplatný typ štítku".into())}}
fn filter_unrejected_assets(candidates:&[i64],rejected:&std::collections::HashSet<i64>)->Vec<i64>{candidates.iter().copied().filter(|id|!rejected.contains(id)).collect()}
fn filter_suggest_assets(candidates:&[i64],rejected:&std::collections::HashSet<i64>)->Vec<i64>{filter_unrejected_assets(candidates,rejected)}
fn filter_group_assets(candidates:&[i64],listed:&std::collections::HashSet<i64>,rejected:&std::collections::HashSet<i64>)->Vec<i64>{filter_unrejected_assets(candidates,rejected).into_iter().filter(|id|listed.contains(id)).collect()}
/// Splits a visually similar group into "same photo" parts. Two photos are the same when they share
/// the base name (copies, RAW+JPG pair, `tag`/`-1`/` (2)` variants) and their EXIF capture times don't
/// differ. File size is no evidence: uncompressed Sony RAW files of a burst have identical sizes.
/// A reused camera counter has a different time.
type SameMeta=(String,Option<String>,Option<f64>);
fn split_same_groups(ids:&[i64],meta:&std::collections::HashMap<i64,SameMeta>)->Vec<Vec<i64>>{
    let items=ids.iter().filter_map(|id|meta.get(id).map(|m|(*id,base_name(&m.0),m.1.as_deref().map(str::trim).filter(|d|!d.is_empty()).map(str::to_string),m.2))).collect::<Vec<_>>();
    let mut parent=(0..items.len()).collect::<Vec<_>>();
    fn root(p:&mut Vec<usize>,mut i:usize)->usize{while p[i]!=i{p[i]=p[p[i]];i=p[i]}i}
    for a in 0..items.len(){for b in a+1..items.len(){let (x,y)=(&items[a],&items[b]);
        let times_ok=match (&x.2,&y.2){(Some(t),Some(u))=>t==u,_=>true};
        if times_ok&&x.1==y.1{let (ra,rb)=(root(&mut parent,a),root(&mut parent,b));if ra!=rb{parent[rb]=ra}}}}
    let mut parts:Vec<(usize,Vec<i64>)>=Vec::new();
    for i in 0..items.len(){let r=root(&mut parent,i);match parts.iter_mut().find(|(k,_)|*k==r){Some((_,v))=>v.push(items[i].0),None=>parts.push((r,vec![items[i].0]))}}
    parts.into_iter().map(|(_,v)|v).filter(|v|v.len()>1).collect()
}
fn base_name(name:&str)->String{
    let mut stem=name.rsplit_once('.').map(|(s,_)|s).unwrap_or(name).trim().to_lowercase();
    if let Some(open)=stem.rfind(" (") {if stem.ends_with(')')&&stem[open+2..stem.len()-1].chars().all(|c|c.is_ascii_digit()){stem.truncate(open)}}
    if let Some(dash)=stem.rfind('-') {if dash+1<stem.len()&&stem[dash+1..].chars().all(|c|c.is_ascii_digit()){stem.truncate(dash)}}
    for suffix in ["tag","_edit","-edit"]{if let Some(s)=stem.strip_suffix(suffix){stem=s.to_string();}}
    stem
}

fn filter_duplicate_groups(rows:&[Value],listed:&std::collections::HashSet<i64>)->Vec<(usize,Vec<i64>)>{rows.iter().enumerate().filter_map(|(index,row)|{let ids=row.get("asset_ids").and_then(Value::as_array)?.iter().filter_map(Value::as_i64).filter(|id|listed.contains(id)).collect::<Vec<_>>();(ids.len()>1).then_some((index,ids))}).collect()}
fn build_label_payload(kind:&str,pairs:&[(i64,i64)],parents:&[(i64,Option<i64>)],listed:&std::collections::HashSet<i64>)->Value{json!({"kind":kind,"pairs":pairs.iter().filter(|(asset_id,_)|listed.contains(asset_id)).collect::<Vec<_>>(),"parents":parents.iter().map(|(id,parent)|(id.to_string(),parent)).collect::<HashMap<_,_>>()})}
fn label_path(conn:&rusqlite::Connection,kind:&str,id:i64)->Result<String,String>{
    let (table,parent_col)=if kind=="category" {("categories","parent_id")} else {("keywords","parent_id")};
    let mut parts=Vec::new();let mut current=Some(id);let mut seen=std::collections::HashSet::new();
    while let Some(value)=current {if !seen.insert(value){break;}let sql=format!("SELECT name,{parent_col} FROM {table} WHERE id=?1");let row:Option<(String,Option<i64>)>=conn.query_row(&sql,[value],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;let Some((name,parent))=row else {return Err("AI návrh odkazuje na neexistujúci štítok".into())};parts.push(name);current=parent;}
    parts.reverse();Ok(parts.join("/"))
}

fn availability_reason(paths: &Paths) -> Option<String> {
    if !paths.ai_python.as_ref().is_some_and(|path| path.is_file()) { return Some("AI modul nie je nainštalovaný".into()); }
    if !paths.ai_script.is_file() { return Some("AI modul nie je nainštalovaný".into()); }
    if !paths.ai_model.join("open_clip_model.fp16.safetensors").is_file() { return Some("AI model nie je nainštalovaný".into()); }
    None
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;

    #[test]
    fn suggest_rejections_filter_only_rejected_assets(){
        let candidates=[1,2,3];let rejected=std::collections::HashSet::from([2]);
        assert_eq!(filter_suggest_assets(&candidates,&rejected),vec![1,3]);
        assert!(filter_suggest_assets(&[2],&rejected).is_empty());
    }

    #[test]
    fn group_rejections_and_unlisted_assets_are_filtered(){
        let candidates=[1,2,3];let listed=std::collections::HashSet::from([1,2]);let rejected=std::collections::HashSet::from([2]);
        assert_eq!(filter_group_assets(&candidates,&listed,&rejected),vec![1]);
    }

    #[test]
    fn same_groups_split_by_capture_time_or_base_name(){
        let m=|n:&str,t:Option<&str>,s:f64|(n.to_string(),t.map(str::to_string),Some(s));
        let meta=std::collections::HashMap::from([
            (1,m("NG_1.ARW",Some("2022-07-01T10:00:00"),24.0)),(2,m("NG_1.JPG",Some("2022-07-01T10:00:00"),8.0)),
            (3,m("NG_2.ARW",Some("2022-07-01T10:00:00"),25.0)),
            (4,m("scan.tif",None,5.0)),(5,m("scan-1.tif",None,6.0)),(6,m("scan (2).jpg",None,7.0)),
            (7,m("NG_09734tag.ARW",None,9.0)),(8,m("NG_09734.jpg",None,3.0)),(9,m("other.jpg",None,11.0)),
            (10,m("NG_7.ARW",Some("2022-07-01T10:00:05"),24.0)),(11,m("NG_8.ARW",Some("2022-07-01T10:00:05"),24.0)),
            (12,m("NG_5.ARW",Some("2019-01-01T00:00:00"),30.0)),(13,m("NG_5.ARW",Some("2023-01-01T00:00:00"),30.0))]);
        assert_eq!(split_same_groups(&[1,2,3,4,5,6,7,8,9,10,11,12,13],&meta),vec![vec![1,2],vec![4,5,6],vec![7,8]]);
    }

    #[test]
    fn duplicate_groups_drop_unlisted_members_and_singletons(){
        let rows=vec![json!({"asset_ids":[1,2,3],"min_sim":0.95}),json!({"asset_ids":[4,5]})];let listed=std::collections::HashSet::from([1,3,4]);
        let groups=filter_duplicate_groups(&rows,&listed);
        assert_eq!(groups,vec![(0,vec![1,3])]);
    }

    #[test]
    fn label_sync_payload_excludes_unlisted_asset_pairs_and_keeps_tree_parents(){
        let pairs=[(1,11),(2,12),(3,13)];let parents=[(11,None),(12,Some(11)),(13,None)];let listed=std::collections::HashSet::from([1,2]);
        let payload=build_label_payload("category",&pairs,&parents,&listed);
        assert_eq!(payload["pairs"],json!([[1,11],[2,12]]));
        assert_eq!(payload["parents"]["12"],json!(11));
        assert!(payload["parents"].get("13").is_some());
    }
}
