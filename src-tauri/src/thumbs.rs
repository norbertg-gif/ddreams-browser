use crate::{db::Db, exiftool::ExifTool, formats::FormatTable, listing::ListingService};
use image::{DynamicImage, ImageDecoder, ImageReader, codecs::jpeg::JpegEncoder, metadata::Orientation};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{io::{Cursor, BufReader}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}}};
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
    pub fn new(exif: Arc<ExifTool>) -> Self {
        Self { sources: vec![Arc::new(NativeSource), Arc::new(ExifToolSource { exif }), Arc::new(WindowsShellSource::new())] }
    }
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

pub struct ThumbService { pub db: Db, pub listing: Arc<ListingService>, pub formats: Arc<FormatTable>, chain: PreviewChain, pub cache: PathBuf, pub epoch: AtomicU64, pub workers: Arc<Semaphore> }
#[derive(Clone, Debug)]
pub struct RenderedPreview { pub bytes: Vec<u8>, pub source: String }
impl ThumbService {
    pub fn new(db: Db, listing: Arc<ListingService>, exif: Arc<ExifTool>, formats: Arc<FormatTable>) -> Arc<Self> {
        let cache=std::env::var_os("DDREAMS_CACHE").map(PathBuf::from).unwrap_or_else(||PathBuf::from(r"G:\ddreams-work\thumb-cache"));
        let chain=PreviewChain::new(exif.clone());
        Arc::new(Self{db,listing,formats,chain,cache,epoch:AtomicU64::new(0),workers:Arc::new(Semaphore::new(4))})
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
    pub fn render(&self,id:i64,size:u32,epoch:u64)->Result<RenderedPreview,ThumbnailError>{
        self.set_epoch(epoch); if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        let (path,ext)=self.asset_path(id)?; if path.is_empty(){return Err(ThumbnailError::Unavailable)}
        let file=Path::new(&path);
        let meta=match std::fs::metadata(file){Ok(m)=>m,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Err(ThumbnailError::Offline),Err(_)=>return Err(ThumbnailError::Unavailable)};
        let target=if size==0{2048}else{size.clamp(48,1200)};
        let modified=meta.modified().ok().and_then(|v|v.duration_since(std::time::UNIX_EPOCH).ok()).map(|v|v.as_nanos()).unwrap_or(0);
        let key=cache_key(&path,target,modified,meta.len());
        let cache_file=self.cache.join(format!("{key}.jpg"));let miss_file=self.cache.join(format!("{key}.miss"));let source_file=self.cache.join(format!("{key}.source"));
        if let Ok(bytes)=std::fs::read(&cache_file){if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}let source=std::fs::read_to_string(source_file).unwrap_or_else(|_|"Cache".into());return Ok(RenderedPreview{bytes,source})}
        if miss_file.is_file(){return Err(ThumbnailError::Unsupported)}
        let request=PreviewRequest{path:file.to_path_buf(),size:target};
        let preview=match self.chain.preview(&request,self.formats.group(&ext)){Ok(v)=>v,Err(ThumbnailError::Unsupported)=>{std::fs::create_dir_all(&self.cache).map_err(|_|ThumbnailError::Unavailable)?;let _=std::fs::write(&miss_file,b"");return Err(ThumbnailError::Unsupported)},Err(e)=>return Err(e)};
        let image=if preview.image.width()<=target&&preview.image.height()<=target{preview.image}else{preview.image.thumbnail(target,target)};
        let bytes=encode_jpeg(image).map_err(|_|ThumbnailError::Decode)?;
        if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        std::fs::create_dir_all(&self.cache).map_err(|_|ThumbnailError::Unavailable)?;
        static TEMP_ID:AtomicUsize=AtomicUsize::new(0);
        let temp=cache_file.with_extension(format!("{}.{}.tmp",std::process::id(),TEMP_ID.fetch_add(1,Ordering::Relaxed)));
        let _=std::fs::write(&source_file,preview.source);
        std::fs::write(&temp,&bytes).map_err(|_|ThumbnailError::Unavailable)?;
        if let Err(e)=std::fs::rename(&temp,&cache_file){let _=std::fs::remove_file(&temp);if !cache_file.exists(){return Err(if e.kind()==std::io::ErrorKind::PermissionDenied{ThumbnailError::Unavailable}else{ThumbnailError::Unavailable})}}
        let _=std::fs::remove_file(&miss_file);
        Ok(RenderedPreview{bytes,source:preview.source.into()})
    }
}

pub fn cache_key(path:&str,size:u32,modified:u128,length:u64)->String {
    Sha256::digest(format!("thumb-v4-real-previews|{path}|{size}|{modified}|{length}").as_bytes()).iter().map(|b|format!("{b:02x}")).collect()
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
    #[test] fn cache_miss_marker_round_trip(){let path=std::env::temp_dir().join(format!("ddreams-{}.miss",std::process::id()));let _=std::fs::remove_file(&path);assert!(!miss_marker_exists(&path));std::fs::write(&path,b"").unwrap();assert!(miss_marker_exists(&path));let _=std::fs::remove_file(path);}
    #[test] fn cache_key_is_versioned_and_size_specific(){assert_ne!(cache_key("a.jpg",180,1,2),cache_key("a.jpg",2048,1,2));}
}
