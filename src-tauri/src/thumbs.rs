use crate::{db::Db, exiftool::ExifTool, formats::FormatTable, listing::ListingService};
use image::{DynamicImage, ImageDecoder, ImageReader, codecs::jpeg::JpegEncoder, metadata::Orientation};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{io::{Cursor, BufReader}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailState { Ready, Offline, Stale, Unavailable }

#[derive(Clone, Copy, Debug)]
pub enum ThumbnailError { Offline, Stale, Unsupported, Decode, Unavailable }

#[derive(Clone, Debug)]
pub struct PreviewRequest { pub path: PathBuf, pub size: u32 }
#[derive(Debug)]
pub struct Preview { pub image: DynamicImage, pub source: &'static str }
#[derive(Clone, Copy, Debug)]
pub enum Miss { Failed }

pub trait PreviewSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, group: Option<&str>) -> bool;
    fn preview(&self, request: &PreviewRequest) -> Result<DynamicImage, Miss>;
}

pub struct PreviewChain { sources: Vec<Arc<dyn PreviewSource>> }
impl PreviewChain {
    pub fn new(exif: Arc<ExifTool>, raw_plugin: Option<RawPluginConfig>) -> Self {
        // LibRaw before the Windows shell: for RAW files without an embedded preview the shell often
        // times out (8 s) or fails, while LibRaw renders a phone DNG in ~0.5-1.5 s.
        let mut sources: Vec<Arc<dyn PreviewSource>> = vec![Arc::new(NativeSource), Arc::new(ExifToolSource { exif })];
        if let Some(config) = raw_plugin { sources.push(Arc::new(LibRawSource::new(config))); }
        sources.push(Arc::new(WindowsShellSource::new()));
        Self { sources }
    }
    /// Identifies which sources can produce previews; when it changes, cached "no preview" markers are stale.
    fn signature(&self) -> String { self.sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(",") }
    #[cfg(test)]
    pub fn with_sources(sources: Vec<Arc<dyn PreviewSource>>) -> Self { Self { sources } }
    pub fn preview(&self, request: &PreviewRequest, group: Option<&str>) -> Result<Preview, ThumbnailError> {
        for source in &self.sources {
            if !source.supports(group) { continue; }
            if let Ok(image) = source.preview(request) { return Ok(Preview { image, source: source.name() }); }
        }
        Err(ThumbnailError::Unsupported)
    }
}

struct NativeSource;
impl PreviewSource for NativeSource {
    fn name(&self) -> &'static str { "Native" }
    fn supports(&self, group: Option<&str>) -> bool { group == Some("image") }
    fn preview(&self, request: &PreviewRequest) -> Result<DynamicImage, Miss> { decode_file(&request.path).map_err(|_| Miss::Failed) }
}
struct ExifToolSource { exif: Arc<ExifTool> }
impl PreviewSource for ExifToolSource {
    fn name(&self) -> &'static str { "ExifTool" }
    fn supports(&self, group: Option<&str>) -> bool { matches!(group, Some("image" | "raw")) }
    fn preview(&self, request: &PreviewRequest) -> Result<DynamicImage, Miss> {
        let (bytes, orientation) = self.exif.largest_preview_with_orientation(&request.path).map_err(|_| Miss::Failed)?;
        decode_memory(&bytes, orientation).map_err(|_| Miss::Failed)
    }
}

/// External RAW plugin (raw/ddreams_raw.py, LibRaw via rawpy) for RAW files without a usable embedded
/// preview, e.g. phone DNGs or DxO/Topaz linear DNGs. One long-lived process, started on first use,
/// JSON lines over stdin/stdout, requests answered concurrently and matched by id.
#[derive(Clone, Debug)]
pub struct RawPluginConfig { pub python: PathBuf, pub script: PathBuf }
type RawReply = std::sync::mpsc::SyncSender<Result<Vec<u8>, String>>;
struct RawProcess { child: std::process::Child, stdin: std::process::ChildStdin, pending: Arc<std::sync::Mutex<std::collections::HashMap<u64, RawReply>>> }
pub struct LibRawSource { config: RawPluginConfig, process: std::sync::Mutex<Option<RawProcess>>, next_id: AtomicU64 }
impl LibRawSource {
    pub fn new(config: RawPluginConfig) -> Self { Self { config, process: std::sync::Mutex::new(None), next_id: AtomicU64::new(1) } }
    fn spawn(&self) -> Result<RawProcess, String> {
        use std::io::BufRead;
        let mut command = std::process::Command::new(&self.config.python);
        command.arg("-I").arg(&self.config.script).env("PYTHONIOENCODING", "utf-8")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("RAW plugin has no stdin")?;
        let stdout = child.stdout.take().ok_or("RAW plugin has no stdout")?;
        let pending: Arc<std::sync::Mutex<std::collections::HashMap<u64, RawReply>>> = Default::default();
        let readers = pending.clone();
        std::thread::Builder::new().name("ddreams-raw-plugin".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                let Some(id) = message.get("id").and_then(|v| v.as_u64()) else { continue };
                let Some(reply) = readers.lock().ok().and_then(|mut p| p.remove(&id)) else { continue };
                let result = if message.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    use base64::Engine;
                    message.get("jpeg").and_then(|v| v.as_str()).ok_or_else(|| "RAW plugin returned no image".to_string())
                        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string()))
                } else { Err(message.get("error").and_then(|v| v.as_str()).unwrap_or("RAW plugin failed").to_string()) };
                let _ = reply.send(result);
            }
            // Process gone: fail everything still waiting so callers don't hang until the timeout.
            if let Ok(mut waiting) = readers.lock() { for (_, reply) in waiting.drain() { let _ = reply.send(Err("RAW plugin exited".into())); } }
        }).map_err(|e| e.to_string())?;
        Ok(RawProcess { child, stdin, pending })
    }
    fn render(&self, path: &Path, size: u32) -> Result<Vec<u8>, String> {
        use std::io::Write;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        let pending = {
            let mut process = self.process.lock().map_err(|_| "RAW plugin lock poisoned")?;
            if process.as_mut().map(|p| !matches!(p.child.try_wait(), Ok(None))).unwrap_or(true) { *process = Some(self.spawn()?); }
            let running = process.as_mut().ok_or("RAW plugin unavailable")?;
            running.pending.lock().map_err(|_| "RAW plugin lock poisoned")?.insert(id, reply);
            let line = serde_json::json!({ "id": id, "path": path.to_string_lossy(), "size": size }).to_string() + "\n";
            if let Err(error) = running.stdin.write_all(line.as_bytes()).and_then(|_| running.stdin.flush()) {
                let _ = running.child.kill(); *process = None; return Err(error.to_string());
            }
            running.pending.clone()
        };
        // Huge files (e.g. a 365 MB upscaled DNG) take ~20 s; the result is cached afterwards.
        match response.recv_timeout(std::time::Duration::from_secs(90)) {
            Ok(result) => result,
            Err(_) => { if let Ok(mut p) = pending.lock() { p.remove(&id); } Err("RAW plugin timed out".into()) }
        }
    }
}
impl Drop for LibRawSource {
    fn drop(&mut self) { if let Ok(mut process) = self.process.lock() { if let Some(mut p) = process.take() { let _ = p.child.kill(); } } }
}
impl PreviewSource for LibRawSource {
    fn name(&self) -> &'static str { "LibRaw" }
    fn supports(&self, group: Option<&str>) -> bool { group == Some("raw") }
    fn preview(&self, request: &PreviewRequest) -> Result<DynamicImage, Miss> {
        let bytes = self.render(&request.path, request.size).map_err(|_| Miss::Failed)?;
        decode_memory(&bytes, None).map_err(|_| Miss::Failed)
    }
}

#[derive(Clone)]
struct ShellCommand { path: PathBuf, size: u32, reply: std::sync::mpsc::SyncSender<Result<DynamicImage, String>> }
#[derive(Clone)]
struct ShellWorker { sender: std::sync::mpsc::Sender<ShellCommand>, generation: u64 }
pub struct WindowsShellSource { worker: std::sync::Mutex<ShellWorker>, next_generation: std::sync::atomic::AtomicU64 }
impl WindowsShellSource {
    pub fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_shell_worker(receiver);
        Self { worker: std::sync::Mutex::new(ShellWorker { sender, generation: 1 }), next_generation: std::sync::atomic::AtomicU64::new(2) }
    }
    fn replace_if_current(&self, generation: u64) {
        let Ok(mut worker) = self.worker.lock() else { return; };
        if worker.generation != generation { return; }
        let (sender, receiver) = std::sync::mpsc::channel();
        let next = self.next_generation.fetch_add(1, Ordering::Relaxed);
        spawn_shell_worker(receiver);
        *worker = ShellWorker { sender, generation: next };
    }
}
impl Default for WindowsShellSource { fn default() -> Self { Self::new() } }
impl PreviewSource for WindowsShellSource {
    fn name(&self) -> &'static str { "WindowsShell" }
    fn supports(&self, _group: Option<&str>) -> bool { true }
    fn preview(&self, request: &PreviewRequest) -> Result<DynamicImage, Miss> {
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        let generation = {
            let worker = self.worker.lock().map_err(|_| Miss::Failed)?;
            worker.sender.send(ShellCommand { path: request.path.clone(), size: request.size, reply }).map_err(|_| Miss::Failed)?;
            worker.generation
        };
        match response.recv_timeout(std::time::Duration::from_secs(8)) {
            Ok(Ok(image)) => Ok(image),
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Miss::Failed),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => { self.replace_if_current(generation); Err(Miss::Failed) }
        }
    }
}

#[cfg(windows)]
fn spawn_shell_worker(receiver: std::sync::mpsc::Receiver<ShellCommand>) {
    std::thread::Builder::new().name("ddreams-windows-shell-preview".into()).spawn(move || {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        while let Ok(command) = receiver.recv() {
            let result = if initialized.0 < 0 { Err(format!("COM initialization failed: {:#x}", initialized.0)) }
                else { shell_image(&command.path, command.size) };
            let _ = command.reply.send(result);
        }
        if initialized.0 >= 0 { unsafe { CoUninitialize(); } }
    }).expect("failed to start Windows shell preview worker");
}
#[cfg(not(windows))]
fn spawn_shell_worker(receiver: std::sync::mpsc::Receiver<ShellCommand>) {
    std::thread::spawn(move || while let Ok(command) = receiver.recv() { let _=command.reply.send(Err("Windows shell preview is unavailable".into())); });
}

#[cfg(windows)]
fn shell_image(path: &Path, requested: u32) -> Result<DynamicImage, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::{Foundation::SIZE, Graphics::Gdi::{DeleteDC, DeleteObject, HGDIOBJ, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS, GetDIBits, GetObjectW}, System::Com::IBindCtx, UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY}}};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>) }.map_err(|e| e.to_string())?;
    let side = if requested == 0 { 2048 } else { requested.clamp(48, 2048) } as i32;
    let bitmap = unsafe { factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY) }.map_err(|e| e.to_string())?;
    let converted = unsafe {
        let mut object = BITMAP::default();
        let got = GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some((&mut object as *mut BITMAP).cast()));
        if got == 0 || object.bmWidth <= 0 || object.bmHeight == 0 { Err("invalid shell HBITMAP dimensions".into()) }
        else {
            let width = object.bmWidth as usize; let height = object.bmHeight.unsigned_abs() as usize;
            let mut info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: width as i32, biHeight: -(height as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, biSizeImage: (width*height*4) as u32, ..Default::default() }, ..Default::default() };
            let mut bgra=vec![0u8;width*height*4];
            let dc=CreateCompatibleDC(None);
            if dc.0.is_null() { Err("CreateCompatibleDC failed".into()) }
            else {
                let copied=GetDIBits(dc,bitmap,0,height as u32,Some(bgra.as_mut_ptr().cast()),&mut info,DIB_RGB_COLORS);
                let _ = DeleteDC(dc);
                if copied==0 { Err("GetDIBits failed".into()) }
                else {
                    // Requesting a negative DIB height returns top-down rows. HBITMAP bytes are BGRA; emit RGB.
                    let mut rgb=vec![0u8;width*height*3];
                    for (src,dst) in bgra.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) { dst.copy_from_slice(&[src[2],src[1],src[0]]); }
                    image::RgbImage::from_raw(width as u32,height as u32,rgb).map(DynamicImage::ImageRgb8).ok_or_else(||"invalid DIB pixel buffer".into())
                }
            }
        }
    };
    unsafe { let _ = DeleteObject(HGDIOBJ(bitmap.0)); }
    converted
}

pub struct ThumbService { pub db: Db, pub listing: Arc<ListingService>, pub formats: Arc<FormatTable>, chain: PreviewChain, pub cache: PathBuf, pub epoch: AtomicU64, pub workers: Arc<Semaphore>, writes: [AtomicUsize; 2], evicting: [Arc<AtomicBool>; 2] }
#[derive(Clone, Debug)]
pub struct RenderedPreview { pub bytes: Vec<u8>, pub source: String }
pub enum CacheLookup { Hit(RenderedPreview), NeedsRender(RenderJob) }
pub struct RenderJob { path: PathBuf, ext: String, target: u32, store: usize, directory: PathBuf, cache_file: PathBuf, miss_file: PathBuf, source_file: PathBuf }
impl ThumbService {
    pub fn new(db: Db, listing: Arc<ListingService>, exif: Arc<ExifTool>, formats: Arc<FormatTable>, cache: PathBuf, raw_plugin: Option<RawPluginConfig>) -> Arc<Self> {
        let chain=PreviewChain::new(exif.clone(), raw_plugin);
        let signature=chain.signature();
        let service=Arc::new(Self{db,listing,formats,chain,cache:cache.clone(),epoch:AtomicU64::new(0),workers:Arc::new(Semaphore::new(4)),writes:[AtomicUsize::new(0),AtomicUsize::new(0)],evicting:[Arc::new(AtomicBool::new(false)),Arc::new(AtomicBool::new(false))]});
        std::thread::spawn(move || { let _=std::fs::create_dir_all(cache.join("thumbs")); let _=std::fs::create_dir_all(cache.join("viewer")); let _=reset_misses_if_chain_changed(&cache,&signature); let _=cleanup_legacy_cache(&cache); let _=evict_cache(&cache.join("thumbs"),THUMBS_LIMIT); let _=evict_cache(&cache.join("viewer"),VIEWER_LIMIT); });
        service
    }
    pub fn set_epoch(&self, epoch:u64){self.epoch.fetch_max(epoch,Ordering::Relaxed);}
    fn asset_path(&self,id:i64)->Result<(String,String),ThumbnailError>{
        if let Some(asset)=self.listing.asset(id) { return Ok((asset.path.unwrap_or_default(), asset.ext)); }
        let conn=self.db.lock().map_err(|_|ThumbnailError::Unavailable)?;
        conn.query_row("SELECT path,ext FROM assets WHERE id=?",[id],|r|Ok((r.get::<_,Option<String>>(0)?.unwrap_or_default(),r.get::<_,String>(1)?))).map_err(|_|ThumbnailError::Unavailable)
    }
    pub fn state(&self,id:i64,epoch:u64)->ThumbnailState{
        if epoch < self.epoch.load(Ordering::Relaxed){return ThumbnailState::Stale}
        match self.asset_path(id){Err(_)=>ThumbnailState::Unavailable,Ok((p,_)) if p.is_empty()=>ThumbnailState::Unavailable,Ok((p,_))=>match Path::new(&p).try_exists(){Ok(true)=>ThumbnailState::Ready,Ok(false)=>ThumbnailState::Offline,Err(_)=>ThumbnailState::Unavailable}}
    }
    pub fn lookup(&self,id:i64,size:u32,epoch:u64)->Result<CacheLookup,ThumbnailError>{
        self.set_epoch(epoch); if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        let (path,ext)=self.asset_path(id)?; if path.is_empty(){return Err(ThumbnailError::Unavailable)}
        let file=Path::new(&path);
        let meta=match std::fs::metadata(file){Ok(m)=>m,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Err(ThumbnailError::Offline),Err(_)=>return Err(ThumbnailError::Unavailable)};
        let target=cache_bucket(size);
        let modified=meta.modified().ok().and_then(|v|v.duration_since(std::time::UNIX_EPOCH).ok()).map(|v|v.as_nanos()).unwrap_or(0);
        let key=cache_key(&path,target,modified,meta.len());let store=if size==0 {1}else{0};let directory=self.cache.join(if store==1{"viewer"}else{"thumbs"});
        let cache_file=directory.join(format!("{key}.jpg"));let miss_file=directory.join(format!("{key}.miss"));let source_file=directory.join(format!("{key}.source"));
        if let Ok(bytes)=std::fs::read(&cache_file){if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}touch_if_stale(&cache_file);let source=std::fs::read_to_string(source_file).unwrap_or_else(|_|"Cache".into());return Ok(CacheLookup::Hit(RenderedPreview{bytes,source}))}
        if miss_file.is_file(){return Err(ThumbnailError::Unsupported)}
        Ok(CacheLookup::NeedsRender(RenderJob{path:file.to_path_buf(),ext,target,store,directory,cache_file,miss_file,source_file}))
    }
    pub fn render(&self,id:i64,size:u32,epoch:u64)->Result<RenderedPreview,ThumbnailError>{
        match self.lookup(id,size,epoch)? { CacheLookup::Hit(data)=>Ok(data), CacheLookup::NeedsRender(job)=>self.generate(job,epoch) }
    }
    fn generate(&self,job:RenderJob,epoch:u64)->Result<RenderedPreview,ThumbnailError>{
        let RenderJob{path,ext,target,store,directory,cache_file,miss_file,source_file}=job;
        let request=PreviewRequest{path,size:target};
        let preview=match self.chain.preview(&request,self.formats.group(&ext)){Ok(v)=>v,Err(ThumbnailError::Unsupported)=>{std::fs::create_dir_all(&directory).map_err(|_|ThumbnailError::Unavailable)?;let _=std::fs::write(&miss_file,b"");return Err(ThumbnailError::Unsupported)},Err(e)=>return Err(e)};
        let image=if preview.image.width()<=target&&preview.image.height()<=target{preview.image}else{preview.image.thumbnail(target,target)};
        let bytes=encode_jpeg(image).map_err(|_|ThumbnailError::Decode)?;
        if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        std::fs::create_dir_all(&directory).map_err(|_|ThumbnailError::Unavailable)?;
        static TEMP_ID:AtomicUsize=AtomicUsize::new(0);
        let temp=cache_file.with_extension(format!("{}.{}.tmp",std::process::id(),TEMP_ID.fetch_add(1,Ordering::Relaxed)));
        let _=std::fs::write(&source_file,preview.source);
        std::fs::write(&temp,&bytes).map_err(|_|ThumbnailError::Unavailable)?;
        if let Err(e)=std::fs::rename(&temp,&cache_file){let _=std::fs::remove_file(&temp);if !cache_file.exists(){return Err(if e.kind()==std::io::ErrorKind::PermissionDenied{ThumbnailError::Unavailable}else{ThumbnailError::Unavailable})}}
        let _=std::fs::remove_file(&miss_file);
        if self.writes[store].fetch_add(1,Ordering::Relaxed)%200==199 { self.schedule_eviction(store,directory); }
        Ok(RenderedPreview{bytes,source:preview.source.into()})
    }
    fn schedule_eviction(&self,store:usize,directory:PathBuf){let limit=if store==1{VIEWER_LIMIT}else{THUMBS_LIMIT};let flag=self.evicting[store].clone();if flag.compare_exchange(false,true,Ordering::AcqRel,Ordering::Relaxed).is_ok(){std::thread::spawn(move||{let _=evict_cache(&directory,limit);flag.store(false,Ordering::Release);});}}
}

const THUMBS_LIMIT:u64=1024*1024*1024; const VIEWER_LIMIT:u64=300*1024*1024;
pub fn cache_bucket(size:u32)->u32{if size==0{2048}else if size<=160{160}else if size<=320{320}else{480}}
fn touch_if_stale(path:&Path){if let Ok(meta)=std::fs::metadata(path){if meta.modified().ok().and_then(|v|v.elapsed().ok()).map(|age|age>std::time::Duration::from_secs(86400)).unwrap_or(false){let _=std::fs::OpenOptions::new().write(true).open(path).and_then(|f|f.set_modified(std::time::SystemTime::now()));}}}
pub fn evict_cache(dir:&Path,limit:u64)->std::io::Result<()> {
    let mut groups=std::collections::HashMap::<String,(u64,std::time::SystemTime)>::new();let now=std::time::SystemTime::now();
    for entry in std::fs::read_dir(dir)?{let entry=entry?;let path=entry.path();let name=entry.file_name().to_string_lossy().into_owned();let Some((key,ext))=name.rsplit_once('.') else{continue};if ext=="tmp"{if entry.metadata()?.modified().ok().and_then(|t|now.duration_since(t).ok()).map(|age|age>std::time::Duration::from_secs(3600)).unwrap_or(false){let _=std::fs::remove_file(path);}continue}if !matches!(ext,"jpg"|"source"|"miss")||!is_hash(key){continue}let meta=entry.metadata()?;let group=groups.entry(key.into()).or_insert((0,meta.modified().unwrap_or(std::time::UNIX_EPOCH)));if ext=="jpg"{group.0=meta.len();group.1=meta.modified().unwrap_or(std::time::UNIX_EPOCH);}}
    let mut total:u64=groups.values().map(|v|v.0).sum();if total<=limit{return Ok(())}let target=limit*80/100;let mut ordered:Vec<_>=groups.into_iter().collect();ordered.sort_by_key(|(_,v)|v.1);for (key,(bytes,_)) in ordered{for ext in ["jpg","source","miss"]{let _=std::fs::remove_file(dir.join(format!("{key}.{ext}")));}total=total.saturating_sub(bytes);if total<=target{break}}Ok(())
}
/// `.miss` markers say "no source could preview this file". When the set of sources changes (e.g. the
/// RAW plugin got installed), those verdicts are stale, so drop them once.
pub fn reset_misses_if_chain_changed(cache:&Path,signature:&str)->std::io::Result<bool> {
    let marker=cache.join("preview-chain.txt");
    if std::fs::read_to_string(&marker).map(|s|s.trim()==signature).unwrap_or(false){return Ok(false)}
    for dir in ["thumbs","viewer"]{let Ok(entries)=std::fs::read_dir(cache.join(dir)) else{continue};for entry in entries.flatten(){if entry.path().extension().is_some_and(|e|e=="miss"){let _=std::fs::remove_file(entry.path());}}}
    std::fs::create_dir_all(cache)?;std::fs::write(marker,signature)?;Ok(true)
}
pub fn cleanup_legacy_cache(root:&Path)->std::io::Result<()> {for entry in std::fs::read_dir(root)?{let entry=entry?;if !entry.file_type()?.is_file(){continue}let name=entry.file_name().to_string_lossy().into_owned();if legacy_name(&name){let _=std::fs::remove_file(entry.path());}}Ok(())}
fn is_hash(s:&str)->bool{s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
fn legacy_name(s:&str)->bool{let Some((stem,tail))=s.split_once('.')else{return false};if !is_hash(stem){return false}if matches!(tail,"jpg"|"source"|"miss"){return true}let parts:Vec<_>=tail.split('.').collect();parts.len()==3&&parts[2]=="tmp"&&!parts[0].is_empty()&&!parts[1].is_empty()&&parts[0].bytes().all(|b|b.is_ascii_digit())&&parts[1].bytes().all(|b|b.is_ascii_digit())}

pub fn cache_key(path:&str,size:u32,modified:u128,length:u64)->String {
    Sha256::digest(format!("thumb-v5-buckets|{path}|{size}|{modified}|{length}").as_bytes()).iter().map(|b|format!("{b:02x}")).collect()
}
#[cfg(test)]
pub fn miss_marker_exists(path:&Path)->bool { path.is_file() }

fn decode_file(path:&Path)->image::ImageResult<DynamicImage>{
    let file=std::fs::File::open(path)?; let reader=ImageReader::new(BufReader::new(file)).with_guessed_format()?;
    let mut decoder=reader.into_decoder()?; let orientation=decoder.orientation()?; let mut image=DynamicImage::from_decoder(decoder)?; image.apply_orientation(orientation); Ok(image)
}
fn decode_memory(bytes:&[u8],fallback_exif_orientation:Option<u8>)->image::ImageResult<DynamicImage>{
    let reader=ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder=reader.into_decoder()?;
    let has_own_orientation=decoder.exif_metadata()?.as_deref().and_then(Orientation::from_exif_chunk).is_some();
    let orientation=decoder.orientation()?;
    let mut image=DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    if !has_own_orientation { if let Some(value)=fallback_exif_orientation.and_then(Orientation::from_exif) { image.apply_orientation(value); } }
    Ok(image)
}
fn encode_jpeg(image:DynamicImage)->Result<Vec<u8>,String>{
    let rgb=image.to_rgb8(); let mut out=Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out,86).encode(&rgb,rgb.width(),rgb.height(),image::ExtendedColorType::Rgb8).map_err(|e|e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Fake { name: &'static str, group: &'static str, succeed: bool, calls: AtomicUsize }
    impl Fake { fn new(name:&'static str,group:&'static str,succeed:bool)->Self{Self{name,group,succeed,calls:AtomicUsize::new(0)}} }
    impl PreviewSource for Fake {
        fn name(&self)->&'static str{self.name}
        fn supports(&self,group:Option<&str>)->bool{group==Some(self.group)}
        fn preview(&self,_:&PreviewRequest)->Result<DynamicImage,Miss>{self.calls.fetch_add(1,Ordering::Relaxed);if self.succeed{Ok(DynamicImage::ImageRgb8(image::RgbImage::new(2,2)))}else{Err(Miss::Failed)}}
    }
    #[test] fn chain_preserves_order_and_falls_through(){let first=Arc::new(Fake::new("first","image",false));let second=Arc::new(Fake::new("second","image",true));let chain=PreviewChain::with_sources(vec![first.clone(),second.clone()]);let p=chain.preview(&PreviewRequest{path:PathBuf::new(),size:90},Some("image")).unwrap();assert_eq!(p.source,"second");assert_eq!(first.calls.load(Ordering::Relaxed),1);assert_eq!(second.calls.load(Ordering::Relaxed),1);}
    #[test] fn chain_skips_sources_for_other_groups(){let native=Arc::new(Fake::new("native","image",true));let shell=Arc::new(Fake::new("shell","other",true));let chain=PreviewChain::with_sources(vec![native.clone(),shell.clone()]);assert_eq!(chain.preview(&PreviewRequest{path:PathBuf::new(),size:90},Some("other")).unwrap().source,"shell");assert_eq!(native.calls.load(Ordering::Relaxed),0);}
    #[test] fn lookup_returns_cached_entry_without_preview() {
        let dir=temp_dir("lookup"); std::fs::create_dir_all(dir.join("thumbs")).unwrap();
        let file=dir.join("photo.jpg"); std::fs::write(&file,b"original").unwrap();
        let path=file.to_string_lossy().into_owned(); let meta=std::fs::metadata(&file).unwrap();
        let modified=meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let key=cache_key(&path,cache_bucket(180),modified,meta.len());
        std::fs::write(dir.join("thumbs").join(format!("{key}.jpg")),b"cached").unwrap();
        let conn=rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE assets(id INTEGER PRIMARY KEY,path TEXT,ext TEXT)",[]).unwrap();
        conn.execute("INSERT INTO assets VALUES(1,?1,'jpg')",[&path]).unwrap();
        let db=Arc::new(std::sync::Mutex::new(conn)); let formats=FormatTable::load_selected(None);
        let listing=ListingService::new(db.clone(),formats.clone()); let fake=Arc::new(Fake::new("fake","image",true));
        let service=ThumbService{db,listing,formats,chain:PreviewChain::with_sources(vec![fake.clone()]),cache:dir.clone(),epoch:AtomicU64::new(0),workers:Arc::new(Semaphore::new(4)),writes:[AtomicUsize::new(0),AtomicUsize::new(0)],evicting:[Arc::new(AtomicBool::new(false)),Arc::new(AtomicBool::new(false))]};
        match service.lookup(1,180,1).unwrap(){CacheLookup::Hit(data)=>assert_eq!(data.bytes,b"cached"),CacheLookup::NeedsRender(_)=>panic!("cache miss")};
        assert_eq!(fake.calls.load(Ordering::Relaxed),0); let _=std::fs::remove_dir_all(dir);
    }
    #[test] fn cache_miss_marker_round_trip(){let path=std::env::temp_dir().join(format!("ddreams-{}.miss",std::process::id()));let _=std::fs::remove_file(&path);assert!(!miss_marker_exists(&path));std::fs::write(&path,b"").unwrap();assert!(miss_marker_exists(&path));let _=std::fs::remove_file(path);}
    #[test] fn cache_key_is_versioned_and_size_specific(){assert_ne!(cache_key("a.jpg",cache_bucket(180),1,2),cache_key("a.jpg",cache_bucket(2048),1,2));}
    #[test] fn bucket_mapping_is_fixed(){assert_eq!([cache_bucket(0),cache_bucket(48),cache_bucket(160),cache_bucket(161),cache_bucket(320),cache_bucket(321),cache_bucket(1200)],[2048,160,160,320,320,480,480]);}
    #[test] fn eviction_removes_oldest_groups_to_eighty_percent(){let dir=temp_dir("evict");std::fs::create_dir_all(&dir).unwrap();let keys=["a".repeat(64),"b".repeat(64),"c".repeat(64)];for (key,age) in keys.iter().zip([300,200,100]){std::fs::write(dir.join(format!("{key}.jpg")),vec![0;10]).unwrap();std::fs::write(dir.join(format!("{key}.source")),b"x").unwrap();let time=std::time::SystemTime::now()-std::time::Duration::from_secs(age);std::fs::OpenOptions::new().write(true).open(dir.join(format!("{key}.jpg"))).unwrap().set_modified(time).unwrap();}evict_cache(&dir,25).unwrap();assert!(!dir.join(format!("{}.jpg",keys[0])).exists());assert!(!dir.join(format!("{}.source",keys[0])).exists());assert!(dir.join(format!("{}.jpg",keys[1])).exists());assert!(dir.join(format!("{}.jpg",keys[2])).exists());let _=std::fs::remove_dir_all(dir);}
    #[test] fn legacy_cleanup_only_removes_known_flat_cache_names(){let dir=temp_dir("legacy");std::fs::create_dir_all(dir.join("tile-exif")).unwrap();std::fs::write(dir.join(format!("{}.jpg","a".repeat(64))),b"").unwrap();std::fs::write(dir.join(format!("{}.12.3.tmp","b".repeat(64))),b"").unwrap();std::fs::write(dir.join("notes.txt"),b"").unwrap();cleanup_legacy_cache(&dir).unwrap();assert!(!dir.join(format!("{}.jpg","a".repeat(64))).exists());assert!(!dir.join(format!("{}.12.3.tmp","b".repeat(64))).exists());assert!(dir.join("notes.txt").exists());assert!(dir.join("tile-exif").is_dir());let _=std::fs::remove_dir_all(dir);}
    #[test] fn changed_preview_chain_drops_stale_miss_markers_once(){
        let dir=temp_dir("misses");std::fs::create_dir_all(dir.join("thumbs")).unwrap();std::fs::create_dir_all(dir.join("viewer")).unwrap();
        let miss=dir.join("thumbs").join(format!("{}.miss","a".repeat(64)));let jpg=dir.join("thumbs").join(format!("{}.jpg","b".repeat(64)));
        std::fs::write(&miss,b"").unwrap();std::fs::write(&jpg,b"x").unwrap();
        assert!(reset_misses_if_chain_changed(&dir,"Native,ExifTool,LibRaw,WindowsShell").unwrap());
        assert!(!miss.exists());assert!(jpg.exists());
        std::fs::write(&miss,b"").unwrap();
        assert!(!reset_misses_if_chain_changed(&dir,"Native,ExifTool,LibRaw,WindowsShell").unwrap(),"same chain: markers stay");
        assert!(miss.exists());let _=std::fs::remove_dir_all(dir);
    }
    fn temp_dir(label:&str)->PathBuf{std::env::temp_dir().join(format!("ddreams-{label}-{}-{}",std::process::id(),TEMP_TEST_ID.fetch_add(1,Ordering::Relaxed)))}
    static TEMP_TEST_ID:AtomicUsize=AtomicUsize::new(0);
}
