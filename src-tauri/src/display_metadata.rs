use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet, VecDeque}, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TileExifValues {
    pub iso: Option<String>, pub aperture: Option<String>, pub shutter: Option<String>,
    pub focal_length: Option<String>, pub lens: Option<String>, pub camera: Option<String>, pub date_taken: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileExifItem { pub asset_id: i64, #[serde(flatten)] pub values: TileExifValues }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry { pub loaded: Vec<String>, pub values: TileExifValues }

const MAX_MEMORY_ENTRIES: usize = 4096;

#[derive(Default)]
struct MemoryEntries {
    values: HashMap<String, CacheEntry>,
    order: VecDeque<String>,
}

impl MemoryEntries {
    fn insert(&mut self, key: &str, entry: CacheEntry) {
        if self.values.insert(key.to_owned(), entry).is_none() {
            self.order.push_back(key.to_owned());
            if self.order.len() > MAX_MEMORY_ENTRIES {
                if let Some(oldest) = self.order.pop_front() { self.values.remove(&oldest); }
            }
        }
    }
}

pub struct DisplayMetadataCache { root: PathBuf, entries: Mutex<MemoryEntries> }

pub fn cache_key(path: &str, size: u64, modified_ns: u128) -> String {
    Sha256::digest(format!("tile-exif-v1|{path}|{size}|{modified_ns}").as_bytes()).iter().map(|b|format!("{b:02x}")).collect()
}

impl DisplayMetadataCache {
    pub fn new(root: PathBuf) -> Arc<Self> { Arc::new(Self { root: root.join("tile-exif"), entries: Mutex::new(MemoryEntries::default()) }) }
    fn path(&self,key:&str)->PathBuf { self.root.join(format!("{key}.json")) }
    pub fn get(&self,key:&str)->CacheEntry {
        if let Ok(entries)=self.entries.lock(){if let Some(value)=entries.values.get(key){return value.clone();}}
        let value=std::fs::read(self.path(key)).ok().and_then(|bytes|serde_json::from_slice::<CacheEntry>(&bytes).ok()).unwrap_or_default();
        if let Ok(mut entries)=self.entries.lock(){entries.insert(key,value.clone());}
        value
    }
    pub fn put(&self,key:&str,value:CacheEntry) {
        if let Ok(mut entries)=self.entries.lock(){entries.insert(key,value.clone());}
        if std::fs::create_dir_all(&self.root).is_err(){return;}
        let path=self.path(key); let temp=path.with_extension(format!("{}.tmp",std::process::id()));
        if let Ok(bytes)=serde_json::to_vec(&value){if std::fs::write(&temp,bytes).is_ok(){if path.exists(){let _=std::fs::remove_file(&path);}if std::fs::rename(&temp,&path).is_err(){let _=std::fs::remove_file(&temp);}}}
    }
}

pub fn allowed_fields(fields:&[String])->Vec<String>{
    let allowed=["iso","aperture","shutter","focalLength","lens","camera","dateTaken"];
    let mut seen=HashSet::new(); fields.iter().filter(|f|allowed.contains(&f.as_str())&&seen.insert(f.as_str())).cloned().collect()
}

pub fn tags_for(fields:&[String])->Vec<&'static str>{
    let mut tags=Vec::new();
    for field in fields { let tag=match field.as_str(){"iso"=>"ISO","aperture"=>"FNumber","shutter"=>"ExposureTime","focalLength"=>"FocalLength","lens"=>"LensModel","camera"=>"Model","dateTaken"=>"DateTimeOriginal",_=>continue}; if !tags.contains(&tag){tags.push(tag);} }
    tags
}

pub fn file_key(path:&Path)->std::io::Result<(String,u64,u128)> {
    let metadata=std::fs::metadata(path)?;
    let modified=metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH).duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path_text=path.to_string_lossy().to_lowercase();
    Ok((cache_key(&path_text,metadata.len(),modified),metadata.len(),modified))
}

pub fn parse_values(object:&serde_json::Map<String,Value>, fields:&[String])->TileExifValues {
    let get=|tags:&[&str]|->Option<String>{
        for tag in tags {
            let Some(value)=object.iter().find(|(key,_)|key.rsplit_once(':').map(|(_,name)|name.eq_ignore_ascii_case(tag)).unwrap_or_else(||key.eq_ignore_ascii_case(tag))).map(|(_,v)|v) else {continue};
            let text=match value {Value::String(s)=>s.clone(),Value::Null=>String::new(),_=>value.to_string()};
            if !text.trim().is_empty(){return Some(text);}
        }
        None
    };
    let has=|id:&str|fields.iter().any(|field|field==id);
    let iso=has("iso").then(||get(&["ISO","PhotographicSensitivity"])).flatten();
    let aperture=has("aperture").then(||get(&["FNumber"])).flatten().map(|v|if v.starts_with('f') {v} else {format!("f/{v}")});
    let shutter=has("shutter").then(||get(&["ExposureTime"])).flatten().map(|v|if v.ends_with(" s") {v} else {format!("{v} s")});
    let focal_length=has("focalLength").then(||get(&["FocalLength"])).flatten();
    let lens=has("lens").then(||get(&["LensModel"])).flatten();
    let camera=has("camera").then(||get(&["Model"])).flatten();
    let date_taken=has("dateTaken").then(||get(&["DateTimeOriginal"])).flatten().map(|v|crate::metadata_view::format_date(&v));
    TileExifValues{iso,aperture,shutter,focal_length,lens,camera,date_taken}
}

pub fn merge_values(target:&mut TileExifValues,source:TileExifValues){
    macro_rules! merge {($($field:ident),+ $(,)?)=>{$(if source.$field.is_some(){target.$field=source.$field;})+};}
    merge!(iso,aperture,shutter,focal_length,lens,camera,date_taken);
}

pub fn missing_fields(entry:&CacheEntry,requested:&[String],catalog_date:Option<&str>)->Vec<String>{
    requested.iter().filter(|field|field.as_str()!="dateTaken"||catalog_date.is_none()).filter(|field|!entry.loaded.contains(field)).cloned().collect()
}

pub fn mark_loaded(entry:&mut CacheEntry,fields:&[String]){for field in fields{if !entry.loaded.contains(field){entry.loaded.push(field.clone());}}}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn cache_key_tracks_path_size_and_mtime(){let a=cache_key("f:/a.jpg",10,20);assert_ne!(a,cache_key("f:/b.jpg",10,20));assert_ne!(a,cache_key("f:/a.jpg",11,20));assert_ne!(a,cache_key("f:/a.jpg",10,21));}
    #[test]fn changed_file_metadata_invalidates_cached_entry(){let root=std::env::temp_dir().join(format!("ddreams-tile-cache-test-{}",std::process::id()));let cache=DisplayMetadataCache::new(root.clone());let old=cache_key("p",10,1);let current=cache_key("p",10,2);cache.put(&old,CacheEntry{loaded:vec!["iso".into()],values:TileExifValues{iso:Some("100".into()),..Default::default()}});assert_eq!(cache.get(&old).values.iso.as_deref(),Some("100"));assert!(missing_fields(&cache.get(&current),&["iso".into()],None).contains(&"iso".into()));let _=std::fs::remove_dir_all(root);}
    #[test]fn field_filter_and_tag_mapping_are_allowlisted(){let fields=allowed_fields(&["iso".into(),"evil".into(),"shutter".into(),"iso".into()]);assert_eq!(fields,["iso","shutter"]);assert_eq!(tags_for(&fields),["ISO","ExposureTime"]);}
    #[test]fn exif_tile_values_use_the_properties_panel_formats(){let value:Value=serde_json::from_str(r#"{"ExifIFD:ISO":"200","ExifIFD:FNumber":2.8,"ExifIFD:ExposureTime":"1/250","ExifIFD:DateTimeOriginal":"2020:06:13 11:25:25"}"#).unwrap();let fields=vec!["iso","aperture","shutter","dateTaken"].into_iter().map(str::to_owned).collect::<Vec<_>>();let parsed=parse_values(value.as_object().unwrap(),&fields);assert_eq!(parsed.iso.as_deref(),Some("200"));assert_eq!(parsed.aperture.as_deref(),Some("f/2.8"));assert_eq!(parsed.shutter.as_deref(),Some("1/250 s"));assert_eq!(parsed.date_taken.as_deref(),Some("13. 6. 2020 11:25:25"));}

    #[test]
    #[ignore = "requires the real local catalog and ExifTool; prints a cold batch and warm cache timing"]
    fn bench_thumbnail_metadata_batch_30_real() {
        use std::time::Instant;
        let catalog=crate::db::catalog_path();
        let connection=crate::db::open_read_only(&catalog).expect("open local catalog read-only");
        let mut statement=connection.prepare("SELECT path FROM assets WHERE is_listed=1 AND path IS NOT NULL ORDER BY id").unwrap();
        let paths=statement.query_map([],|row|row.get::<_,String>(0)).unwrap().filter_map(Result::ok).filter(|path|Path::new(path).is_file()).take(30).map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(paths.len(),30,"need 30 online files in the local catalog");
        let executable=std::env::var_os("DDREAMS_EXIFTOOL").map(PathBuf::from).unwrap_or_else(||PathBuf::from(r"G:\Programovanie\ddreams-browser\tools\exiftool\exiftool.exe"));
        let exif=crate::exiftool::ExifTool::new(executable);
        let fields=vec!["iso","aperture","shutter","focalLength","lens","camera","dateTaken"].into_iter().map(str::to_owned).collect::<Vec<_>>();
        let tags=tags_for(&fields);
        let bench_cache=std::env::temp_dir().join(format!("ddreams-exif-bench-{}",std::process::id()));
        let cache=DisplayMetadataCache::new(bench_cache.join("bench"));
        let keys=paths.iter().map(|path|file_key(path).expect("file metadata").0).collect::<Vec<_>>();
        let cold=Instant::now();
        let records=exif.metadata_batch(&paths,&tags).expect("single ExifTool batch");
        let cold_elapsed=cold.elapsed();
        for (key,record) in keys.iter().zip(records.iter()) { let mut entry=CacheEntry::default();if let Some(object)=record.as_object(){entry.values=parse_values(object,&fields);}mark_loaded(&mut entry,&fields);cache.put(key,entry); }
        let warm=Instant::now();
        let cached=keys.iter().map(|key|cache.get(key)).collect::<Vec<_>>();
        let warm_elapsed=warm.elapsed();
        drop(cache);
        let disk_cache=DisplayMetadataCache::new(bench_cache.join("bench"));
        let disk=Instant::now();
        let disk_cached=keys.iter().map(|key|disk_cache.get(key)).collect::<Vec<_>>();
        let disk_elapsed=disk.elapsed();
        assert_eq!(cached.len(),30);
        assert_eq!(disk_cached.len(),30);
        println!("EXIF batch 30: cold ExifTool={cold_elapsed:?}; cached memory={warm_elapsed:?}; cached disk reads={disk_elapsed:?}");
        drop(disk_cache);
        let _=std::fs::remove_dir_all(bench_cache);
    }
}
