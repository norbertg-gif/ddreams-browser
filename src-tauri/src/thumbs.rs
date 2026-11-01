use crate::{db::Db, exiftool::ExifTool, listing::ListingService};
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

pub struct ThumbService { pub db: Db, pub listing: Arc<ListingService>, pub exif: Arc<ExifTool>, pub cache: PathBuf, pub epoch: AtomicU64, pub workers: Arc<Semaphore> }
impl ThumbService {
    pub fn new(db: Db, listing: Arc<ListingService>, exif: Arc<ExifTool>) -> Arc<Self> {
        let cache=std::env::var_os("DDREAMS_CACHE").map(PathBuf::from).unwrap_or_else(||PathBuf::from(r"G:\ddreams-work\thumb-cache"));
        Arc::new(Self{db,listing,exif,cache,epoch:AtomicU64::new(0),workers:Arc::new(Semaphore::new(4))})
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
    pub fn render(&self,id:i64,size:u32,epoch:u64)->Result<Vec<u8>,ThumbnailError>{
        self.set_epoch(epoch); if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        let (path,ext)=self.asset_path(id)?; if path.is_empty(){return Err(ThumbnailError::Unavailable)}
        let file=Path::new(&path);
        let meta=match std::fs::metadata(file){Ok(m)=>m,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Err(ThumbnailError::Offline),Err(_)=>return Err(ThumbnailError::Unavailable)};
        let modified=meta.modified().ok().and_then(|v|v.duration_since(std::time::UNIX_EPOCH).ok()).map(|v|v.as_nanos()).unwrap_or(0);
        let hash=Sha256::digest(format!("thumb-v2-orientation|{}|{}|{}|{}",path,size,modified,meta.len()).as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>();
        let cache_file=self.cache.join(format!("{hash}.jpg")); if let Ok(bytes)=std::fs::read(&cache_file){if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}return Ok(bytes)}
        let ext=ext.to_ascii_lowercase();
        let raw=["arw","dng","cr2","nef","raf","orf","rw2","pef","acr","raw"].contains(&ext.as_str());
        let tiff=["tif","tiff"].contains(&ext.as_str());
        let decoded=if raw {let (preview,orientation)=self.exif.largest_preview_with_orientation(file).map_err(|_|ThumbnailError::Decode)?; decode_memory(&preview,orientation).map_err(|_|ThumbnailError::Decode)?}
            else if ["jpg","jpeg","png","webp","tif","tiff"].contains(&ext.as_str()) {
                match decode_file(file){Ok(img)=>img,Err(_) if !file.try_exists().unwrap_or(true)=>return Err(ThumbnailError::Offline),Err(_) if tiff=>{let preview=self.exif.largest_preview(file).map_err(|_|if !file.try_exists().unwrap_or(true){ThumbnailError::Offline}else{ThumbnailError::Decode})?;decode_memory(&preview,None).map_err(|_|ThumbnailError::Decode)?},Err(_)=>return Err(ThumbnailError::Decode)}
            } else {return Err(ThumbnailError::Unsupported)};
        let target=if size==0{4096}else{size.clamp(48,1200)};
        let bytes=encode_jpeg(decoded.thumbnail(target,target)).map_err(|_|ThumbnailError::Decode)?;
        if epoch<self.epoch.load(Ordering::Relaxed){return Err(ThumbnailError::Stale)}
        std::fs::create_dir_all(&self.cache).map_err(|_|ThumbnailError::Unavailable)?;
        static TEMP_ID:AtomicUsize=AtomicUsize::new(0);
        let temp=cache_file.with_extension(format!("{}.{}.tmp",std::process::id(),TEMP_ID.fetch_add(1,Ordering::Relaxed)));
        std::fs::write(&temp,&bytes).map_err(|_|ThumbnailError::Unavailable)?;
        if let Err(e)=std::fs::rename(&temp,&cache_file){let _=std::fs::remove_file(&temp);if !cache_file.exists(){return Err(if e.kind()==std::io::ErrorKind::PermissionDenied{ThumbnailError::Unavailable}else{ThumbnailError::Unavailable})}}
        Ok(bytes)
    }
}
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
