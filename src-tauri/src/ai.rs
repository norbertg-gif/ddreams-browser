use crate::{db::Db, formats::FormatTable, paths::Paths};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::HashMap, fs::{self, File, OpenOptions}, io::{BufRead, BufReader, BufWriter, Write}, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}, sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc, Arc, Mutex}, thread, time::{Duration, UNIX_EPOCH}};
use tauri::Emitter;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

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
    last_stderr: Mutex<String>,
    stderr_reader: Mutex<Option<thread::JoinHandle<()>>>,
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
        }))
    }

    pub fn status(&self) -> AiStatus { self.0.status.lock().unwrap().clone() }

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
        Ok(())
    }
}

fn result_page(result: Value) -> Result<AiSearchResult, String> {
    let rows = result.get("results").and_then(Value::as_array).ok_or_else(|| "AI server vrátil neplatný zoznam".to_string())?;
    let ids = rows.iter().map(|row| row.get("asset_id").and_then(Value::as_i64).ok_or_else(|| "AI server vrátil neplatné asset_id".to_string())).collect::<Result<Vec<_>,_>>()?;
    let searched = result.get("searched").and_then(Value::as_u64).ok_or_else(|| "AI server nevrátil počet prehľadaných fotiek".to_string())?;
    Ok(AiSearchResult { ids, searched })
}

fn availability_reason(paths: &Paths) -> Option<String> {
    if !paths.ai_python.as_ref().is_some_and(|path| path.is_file()) { return Some("AI modul nie je nainštalovaný".into()); }
    if !paths.ai_script.is_file() { return Some("AI modul nie je nainštalovaný".into()); }
    if !paths.ai_model.join("open_clip_model.fp16.safetensors").is_file() { return Some("AI model nie je nainštalovaný".into()); }
    None
}
