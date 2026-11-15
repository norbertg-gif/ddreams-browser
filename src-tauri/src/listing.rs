use crate::{db::{self, Asset, AssetFilter, AssetPage, Db}, formats::FormatTable};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicI64, Ordering}, mpsc}, time::Duration};

const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const OFFLINE_RETRY: Duration = Duration::from_secs(8);

pub struct ListingService {
    db: Db,
    formats: std::sync::Arc<FormatTable>,
    entries: std::sync::Mutex<HashMap<i64, Asset>>,
    directory_parents: std::sync::Mutex<HashMap<i64, i64>>,
    path_ids: std::sync::Mutex<HashMap<String,i64>>,
    listings: std::sync::Mutex<HashMap<String, (std::time::Instant, Arc<Vec<Asset>>)>>,
    prepared: std::sync::Mutex<Option<PreparedListing>>,
    next_id: AtomicI64,
}

struct PreparedListing {
    source: Arc<Vec<Asset>>,
    key: String,
    rows: Arc<Vec<Asset>>,
    total_size: f64,
}

impl ListingService {
    pub fn new(db: Db, formats: std::sync::Arc<FormatTable>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { db, formats, entries: std::sync::Mutex::new(HashMap::new()), directory_parents: std::sync::Mutex::new(HashMap::new()), path_ids:std::sync::Mutex::new(HashMap::new()), listings: std::sync::Mutex::new(HashMap::new()), prepared: std::sync::Mutex::new(None), next_id: AtomicI64::new(-10_000_000) })
    }

    fn id(&self) -> i64 { self.next_id.fetch_sub(1, Ordering::Relaxed) }

    pub fn invalidate(&self, folder_id: i64) -> Result<(), String> {
        let path = self.asset(folder_id).and_then(|a| a.path).or_else(|| db::folder_ancestors(&self.db, folder_id).ok()?.last()?.path.clone()).ok_or("folder path unavailable")?;
        self.invalidate_path(&path)?;
        Ok(())
    }

    pub fn invalidate_path(&self, path: &str) -> Result<(), String> {
        self.listings.lock().map_err(|_| "listing cache poisoned")?.remove(&normalize(path));
        *self.prepared.lock().map_err(|_| "prepared listing cache poisoned")? = None;
        Ok(())
    }

    pub fn invalidate_asset_parent(&self, asset_id: i64) -> Result<(), String> {
        let path: String = self.db.lock().map_err(|_| "catalog lock poisoned")?
            .query_row("SELECT coalesce(path,'') FROM assets WHERE id=?1", [asset_id], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if path.is_empty() { return Err("asset path unavailable".into()); }
        let parent = Path::new(&path).parent().ok_or("asset parent unavailable")?.to_string_lossy().to_string();
        self.invalidate_path(&parent)
    }

    fn list(&self, path: &str, folder_ids: &[i64]) -> (Arc<Vec<Asset>>, bool) {
        let key = normalize(path);
        if let Ok(cache) = self.listings.lock() {
            if let Some((when, rows)) = cache.get(&key) {
                if !rows.iter().any(|a| a.offline) || when.elapsed() < OFFLINE_RETRY { return (rows.clone(), rows.iter().any(|a| a.offline)); }
            }
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let root = PathBuf::from(path);
        let formats = self.formats.clone();
        std::thread::spawn(move || {
            let result = std::fs::read_dir(&root).map(|items| {
                items.filter_map(Result::ok).filter_map(|entry| {
                    let p = entry.path(); let name = entry.file_name().to_string_lossy().to_string();
                    let meta = entry.metadata().ok()?;
                    if hidden_or_ignored(&name, &meta, &formats) { return None; }
                    Some((p, name, meta))
                }).collect::<Vec<_>>()
            });
            let _ = tx.send(result);
        });
        let disk = match rx.recv_timeout(LIST_TIMEOUT) { Ok(Ok(v)) => v, _ => {
            let mut rows = Vec::new();
            if let Ok(context) = db::folder_disk_context(&self.db, folder_ids) {
                for (_, _, assets) in context { rows.extend(assets.into_iter().filter(|a| !a.ext.is_empty() && !self.formats.is_hidden(&a.ext)).map(|mut a| { a.offline = true; a })); }
            }
            let rows = Arc::new(rows);
            if let Ok(mut cache) = self.listings.lock() { cache.insert(key, (std::time::Instant::now(), rows.clone())); }
            return (rows, true);
        }};
        let parent_folder_id = db::folder_by_path(&self.db, path).ok().flatten().map(|folder| folder.id)
            .or_else(|| match folder_ids { [id] if *id > 0 => Some(*id), _ => None });
        // Also match against the catalogue folder found by path: a folder catalogued just now (auto-catalogue
        // of an included folder) isn't among the ids the UI sent with the request.
        let mut context_ids = folder_ids.to_vec();
        if let Some(id) = parent_folder_id { if !context_ids.contains(&id) { context_ids.push(id); } }
        let contexts = db::folder_disk_context(&self.db, &context_ids).unwrap_or_default();
        let mut known = HashMap::<String, Asset>::new();
        for (_, catalog_path, assets) in contexts { if normalize(&catalog_path)==key { for asset in assets { known.insert(asset.name.to_lowercase(), asset); } } }
        let folder_paths = disk.iter().filter(|(_, _, meta)| meta.is_dir()).map(|(path, _, _)| path.to_string_lossy().to_string()).collect::<Vec<_>>();
        let matched_folders = db::folders_by_paths(&self.db, &folder_paths).unwrap_or_else(|_| folder_paths.iter().filter_map(|path| db::folder_by_path(&self.db, path).ok().flatten().map(|folder| (path.clone(), folder))).collect());
        let mut rows = Vec::with_capacity(disk.len());
        for (full, name, meta) in disk {
            let is_dir = meta.is_dir();
            let ext = if is_dir { String::new() } else { full.extension().unwrap_or_default().to_string_lossy().trim_start_matches('.').to_ascii_lowercase() };
            let matched_folder = if is_dir { matched_folders.get(full.to_string_lossy().as_ref()).map(|f| f.id) } else { None };
            let mut asset = known.remove(&name.to_lowercase()).unwrap_or_else(|| Asset {
                id: self.id(), folder_id: if is_dir { matched_folder } else { parent_folder_id }, name: name.clone(), ext: ext.clone(), size: None,
                width: None, height: None, bpp: None, caption: None, notes: None, author: None, rating: None, tagged: None,
                label: None, file_modified: None, exif_date: None, acdsee_date: None, md_dirty: None, is_orphan: None,
                path: Some(full.to_string_lossy().to_string()), is_directory: is_dir, offline: false, missing: false, has_categories: false, display_categories: None, display_keywords: None,
            });
            asset.name = name;
            asset.ext = ext;
            asset.size = if is_dir { Some(0.0) } else { Some(meta.len() as f64) };
            asset.file_modified = meta.modified().ok().map(system_time_iso);
            asset.path = Some(full.to_string_lossy().to_string());
            asset.is_directory = is_dir;
            asset.offline = false;
            if asset.id <= -10_000_000 { if let Ok(mut entries) = self.entries.lock() { entries.insert(asset.id, asset.clone()); } }
            if is_dir {
                if let (Some(parent),Ok(mut parents))=(folder_ids.first(),self.directory_parents.lock()) { parents.insert(asset.id,*parent); }
                if let Ok(mut paths)=self.path_ids.lock(){paths.insert(normalize(&full.to_string_lossy()),asset.id);}
            }
            rows.push(asset);
        }
        rows.sort_by(|a,b| a.is_directory.cmp(&b.is_directory).reverse().then_with(||a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id)));
        let rows = Arc::new(rows);
        if let Ok(mut cache) = self.listings.lock() { cache.insert(key, (std::time::Instant::now(), rows.clone())); }
        (rows, false)
    }

    fn folder_of_filter(&self, filter: &AssetFilter) -> Result<(String, Vec<i64>), String> {
        let mut path = filter.folder_path.clone().unwrap_or_default();
        if path.is_empty() {
            let id = filter.id.unwrap_or(-1);
            if id <= -10_000_000 {
                path = self.asset(id).and_then(|a| a.path).unwrap_or_default();
            } else if let Some(f) = db::folder_ancestors(&self.db, id).map_err(|e| e.to_string())?.last() { path = f.path.clone().unwrap_or_default(); }
        }
        let ids = filter.ids.clone().unwrap_or_else(|| filter.id.map(|id| match self.asset(id).filter(|a|a.is_directory) { Some(a)=>vec![a.folder_id.unwrap_or(id)],None=>vec![id] }).unwrap_or_default());
        Ok((path, ids))
    }

    /// The folder's disk path and its files that are not in the catalogue yet (negative listing ids).
    pub fn uncatalogued_files(&self, filter: &AssetFilter) -> Result<(String, Vec<Asset>), String> {
        let (path, ids) = self.folder_of_filter(filter)?;
        let (rows, _) = self.list(&path, &ids);
        Ok((path, rows.iter().filter(|a| !a.is_directory && a.id < 0 && !a.ext.is_empty() && a.path.is_some()).cloned().collect()))
    }

    pub fn page(&self, filter: &AssetFilter) -> Result<AssetPage, String> {
        let (path, ids) = self.folder_of_filter(filter)?;
        let (source, _) = self.list(&path, &ids);
        // Pagination and display-only fields do not change the filtered order.
        let mut key_filter = filter.clone();
        key_filter.offset = 0; key_filter.limit = 0;
        key_filter.include_categories = false; key_filter.include_keywords = false;
        let key = serde_json::to_string(&key_filter).map_err(|e| e.to_string())?;
        let cached = self.prepared.lock().map_err(|_| "prepared listing cache poisoned")?
            .as_ref().filter(|p| p.key == key && Arc::ptr_eq(&p.source, &source))
            .map(|p| (p.rows.clone(), p.total_size));
        let (rows, total_size) = if let Some(cached) = cached { cached } else {
        let mut rows = source.as_ref().clone();
        rows.retain(|a| a.is_directory || (!a.ext.is_empty() && !self.formats.is_hidden(&a.ext)));
        if let Some(q) = filter.query.as_deref().filter(|q| !q.trim().is_empty()) { let q=q.to_lowercase(); rows.retain(|a|a.is_directory||a.name.to_lowercase().contains(&q)||a.caption.as_deref().is_some_and(|s|s.to_lowercase().contains(&q))||a.notes.as_deref().is_some_and(|s|s.to_lowercase().contains(&q))); }
        rows.retain(|a| a.is_directory || db::asset_filter_matches(a, filter, &self.formats));
        let dir_count = rows.iter().filter(|a| a.is_directory).count();
        let (dirs, files) = rows.split_at(dir_count);
        let mut dirs=dirs.to_vec(); let mut files=files.to_vec();
        let ordering = |a: &Asset,b: &Asset| db::compare_assets(a,b,&filter.sort,filter.descending);
        dirs.sort_by(|a,b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(ordering);
        dirs.extend(files);
        let total_size=dirs.iter().filter(|a|!a.is_directory).map(|a|a.size.unwrap_or(0.0)).sum();
        let rows = Arc::new(dirs);
        *self.prepared.lock().map_err(|_| "prepared listing cache poisoned")? = Some(PreparedListing { source, key, rows: rows.clone(), total_size });
        (rows, total_size)
        };
        let total = rows.len() as i64;
        let offset=filter.offset.max(0) as usize; let limit=filter.limit.clamp(1,500) as usize;
        let start=offset.min(rows.len());
        let end=start.saturating_add(limit).min(rows.len());
        let mut items=rows[start..end].to_vec();
        db::attach_display_values(&self.db,&mut items,filter.include_categories,filter.include_keywords).map_err(|e|e.to_string())?;
        Ok(AssetPage { items, total, total_size })
    }

    pub fn asset(&self, id: i64) -> Option<Asset> { self.entries.lock().ok()?.get(&id).cloned() }
    #[cfg(test)]
    pub fn insert_test_asset(&self, asset:Asset){if let Ok(mut entries)=self.entries.lock(){entries.insert(asset.id,asset);}}

    pub fn resolve_folder(&self,path:&str)->Result<Option<db::Folder>,String>{
        if let Some(folder)=db::folder_by_path(&self.db,path).map_err(|e|e.to_string())? { return Ok(Some(folder)); }
        let path_buf=PathBuf::from(path);
        if !path_buf.is_dir(){return Ok(None)}
        let key=normalize(path);
        if let Some(id)=self.path_ids.lock().map_err(|_|"path map poisoned")?.get(&key).copied(){
            if let Some(asset)=self.asset(id){let parent=self.directory_parents.lock().ok().and_then(|p|p.get(&id).copied());return Ok(Some(db::Folder{id,parent_id:parent,root_id:parent.and_then(|p|self.asset(p)).and_then(|a|a.folder_id).or_else(||parent.and_then(|p|db::folder_ancestors(&self.db,p).ok()?.last()?.root_id)),name:asset.name,path:asset.path,count:0,folder_ids:vec![id],virtual_node:None,offline:false,missing:false}));}
        }
        let parent_path=path_buf.parent().map(|p|p.to_string_lossy().to_string());
        let parent=if let Some(parent_path)=parent_path.filter(|p|!p.is_empty()&&normalize(p)!=key){self.resolve_folder(&parent_path)?}else{None};
        let name=path_buf.file_name().map(|n|n.to_string_lossy().to_string()).unwrap_or_else(||path.to_string());
        let id=self.id(); let parent_id=parent.as_ref().map(|p|p.id); let root_id=parent.as_ref().and_then(|p|p.root_id);
        let asset=Asset{id,folder_id:None,name:name.clone(),ext:String::new(),size:Some(0.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(path_buf.to_string_lossy().to_string()),is_directory:true,offline:false,missing:false,has_categories:false,display_categories:None,display_keywords:None};
        self.entries.lock().map_err(|_|"entry map poisoned")?.insert(id,asset);
        if let Some(parent_id)=parent_id{self.directory_parents.lock().map_err(|_|"folder map poisoned")?.insert(id,parent_id);}
        self.path_ids.lock().map_err(|_|"path map poisoned")?.insert(key,id);
        Ok(Some(db::Folder{id,parent_id,root_id,name,path:Some(path_buf.to_string_lossy().to_string()),count:0,folder_ids:vec![id],virtual_node:None,offline:false,missing:false}))
    }

    pub fn folder_ancestors(&self, id:i64) -> Result<Vec<db::Folder>,String> {
        let Some(asset)=self.asset(id).filter(|a|a.is_directory) else { return db::folder_ancestors(&self.db,id).map_err(|e|e.to_string()) };
        if let Some(catalog_id)=asset.folder_id { return db::folder_ancestors(&self.db,catalog_id).map_err(|e|e.to_string()); }
        let parent=self.directory_parents.lock().map_err(|_|"folder map poisoned")?.get(&id).copied().ok_or("folder parent not found")?;
        let mut chain=if parent <= -10_000_000 { self.folder_ancestors(parent)? } else { db::folder_ancestors(&self.db,parent).map_err(|e|e.to_string())? };
        let root_id=chain.last().and_then(|f|f.root_id);
        chain.push(db::Folder{id,parent_id:Some(parent),root_id,name:asset.name,path:asset.path,count:0,folder_ids:vec![id],virtual_node:None,offline:false,missing:false});
        Ok(chain)
    }
}

#[cfg(test)]
mod listing_cache_tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    fn asset(id: i64, name: &str, is_directory: bool, size: f64) -> Asset {
        serde_json::from_value(serde_json::json!({"id":id,"name":name,"ext":if is_directory { "" } else { "jpg" },"isDirectory":is_directory,"size":size})).unwrap()
    }

    #[test]
    fn pages_reuse_order_but_filters_and_invalidation_refresh_it() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let service = ListingService::new(db, FormatTable::load_selected(None));
        let path = "C:\\photos";
        let source = Arc::new(vec![asset(-1,"folder",true,0.0), asset(-2,"b.jpg",false,20.0), asset(-3,"a.jpg",false,10.0)]);
        service.listings.lock().unwrap().insert(normalize(path), (std::time::Instant::now(), source));
        let mut filter: AssetFilter = serde_json::from_value(serde_json::json!({"kind":"folder","folderPath":path,"sort":"name","descending":false,"offset":0,"limit":2})).unwrap();
        let first = service.page(&filter).unwrap();
        assert_eq!(first.items.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["folder", "a.jpg"]);
        assert_eq!((first.total, first.total_size), (3, 30.0));
        let prepared = service.prepared.lock().unwrap().as_ref().unwrap().rows.clone();
        filter.offset = 2; filter.limit = 1; filter.include_categories = true;
        let second = service.page(&filter).unwrap();
        assert_eq!(second.items[0].name, "b.jpg");
        assert!(Arc::ptr_eq(&prepared, &service.prepared.lock().unwrap().as_ref().unwrap().rows));
        filter.query = Some("b.jpg".into()); filter.offset = 0;
        let filtered = service.page(&filter).unwrap();
        assert_eq!((filtered.total, filtered.total_size), (2, 20.0));
        assert!(!Arc::ptr_eq(&prepared, &service.prepared.lock().unwrap().as_ref().unwrap().rows));
        service.invalidate_path(path).unwrap();
        assert!(service.prepared.lock().unwrap().is_none());
        service.listings.lock().unwrap().insert(normalize(path), (std::time::Instant::now(), Arc::new(vec![asset(-4,"new.jpg",false,7.0)])));
        filter.query = None;
        let refreshed = service.page(&filter).unwrap();
        assert_eq!((refreshed.total, refreshed.total_size, refreshed.items[0].name.as_str()), (1, 7.0, "new.jpg"));
    }
}

fn normalize(path: &str) -> String { path.trim_end_matches(['\\','/']).replace('/',"\\").to_lowercase() }
fn hidden_or_ignored(name: &str, metadata: &std::fs::Metadata, formats: &FormatTable) -> bool {
    let lower=name.to_ascii_lowercase();
    let ext=Path::new(name).extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if name.starts_with('.') || ["desktop.ini",".picasa.ini"].contains(&lower.as_str()) || formats.is_hidden(&ext) { return true; }
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if metadata.file_attributes() & (0x2 | 0x4) != 0 { return true; } }
    let _=metadata; false
}
pub(crate) fn system_time_iso(time: std::time::SystemTime) -> String {
    let secs=time.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let days=secs.div_euclid(86_400); let day_secs=secs.rem_euclid(86_400);
    let z=days+719_468; let era=(if z>=0 {z} else {z-146_096})/146_097; let doe=z-era*146_097;
    let yoe=(doe-doe/1_460+doe/36_524-doe/146_096)/365; let mut year=yoe+era*400;
    let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153;
    let day=doy-(153*mp+2)/5+1; let month=mp+if mp<10 {3} else {-9}; if month<=2 {year+=1;}
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",day_secs/3600,(day_secs%3600)/60,day_secs%60)
}
