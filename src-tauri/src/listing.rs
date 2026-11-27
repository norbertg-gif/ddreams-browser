use crate::{db::{self, Asset, AssetFilter, AssetPage, Db}, formats::FormatTable};
use std::{collections::{BTreeMap, HashMap, HashSet, hash_map::DefaultHasher}, hash::{Hash, Hasher}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicI64, AtomicU64, Ordering}, mpsc}, time::{Duration, Instant}};

const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const OFFLINE_RETRY: Duration = Duration::from_secs(8);
const MAX_LISTINGS: usize = 32;
const MAX_LISTING_ROWS: usize = 125_000;
const MAX_TEMPORARY_ENTRIES: usize = 125_000;

#[derive(Default)]
struct TemporaryEntries {
    entries: BTreeMap<i64, Asset>,
    parents: HashMap<i64, i64>,
    paths: HashMap<(String, bool), i64>,
}

impl TemporaryEntries {
    fn path_key(path: &str, is_directory: bool) -> (String, bool) {
        (if is_directory { normalize(path) } else { path.to_owned() }, is_directory)
    }

    fn insert(&mut self, asset: Asset, parent: Option<i64>) {
        let id = asset.id;
        if asset.is_directory {
            if let Some(parent) = parent { self.parents.insert(id, parent); }
        }
        if let Some(path) = asset.path.as_deref() { self.paths.insert(Self::path_key(path, asset.is_directory), id); }
        self.entries.insert(id, asset);
        while self.entries.len() > MAX_TEMPORARY_ENTRIES {
            let Some((id, entry)) = self.entries.pop_last() else { break };
            self.parents.remove(&id);
            if let Some(path) = entry.path.as_deref() {
                let key = Self::path_key(path, entry.is_directory);
                if self.paths.get(&key) == Some(&id) { self.paths.remove(&key); }
            }
        }
    }
}

pub struct ListingService {
    db: Db,
    formats: std::sync::Arc<FormatTable>,
    exif: std::sync::OnceLock<Arc<crate::exiftool::ExifTool>>,
    temporary: std::sync::Mutex<TemporaryEntries>,
    listings: std::sync::Mutex<HashMap<String, CachedListing>>,
    checking: std::sync::Mutex<HashSet<String>>,
    prepared: std::sync::Mutex<Option<PreparedListing>>,
    next_id: AtomicI64,
    revision: AtomicU64,
}

struct CachedListing {
    when: Instant,
    rows: Arc<Vec<Asset>>,
    signature: Option<DirectorySignature>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DirectorySignature {
    count: usize,
    hash: u64,
}

struct FolderCheck {
    service: Arc<ListingService>,
    key: String,
}

impl Drop for FolderCheck {
    fn drop(&mut self) {
        if let Ok(mut checking) = self.service.checking.lock() { checking.remove(&self.key); }
    }
}

struct PreparedListing {
    source: Arc<Vec<Asset>>,
    key: String,
    rows: Arc<Vec<Asset>>,
    total_size: f64,
}

impl ListingService {
    pub fn new(db: Db, formats: std::sync::Arc<FormatTable>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { db, formats, exif: Default::default(), temporary: std::sync::Mutex::new(TemporaryEntries::default()), listings: std::sync::Mutex::new(HashMap::new()), checking: std::sync::Mutex::new(HashSet::new()), prepared: std::sync::Mutex::new(None), next_id: AtomicI64::new(-10_000_000), revision: AtomicU64::new(0) })
    }

    pub(crate) fn set_exif_tool(&self, exif: Arc<crate::exiftool::ExifTool>) {
        self.exif.get_or_init(|| exif);
    }

    pub(crate) fn missing_capture_dates<'a>(&self, rows: impl IntoIterator<Item = &'a Asset>) -> HashMap<i64, String> {
        let Some(exif) = self.exif.get() else { return HashMap::new(); };
        let mut photos: Vec<_> = rows.into_iter().filter_map(|asset| {
            if asset.is_directory || asset.offline || asset.missing
                || !asset.exif_date.as_deref().is_none_or(str::is_empty)
                || !matches!(self.formats.group(&asset.ext), Some("image" | "raw"))
            {
                return None;
            }
            Some(crate::incoming::Photo { id: asset.id, path: asset.path.clone()?, capture: None })
        }).collect();
        crate::incoming::fill_capture_dates(exif, &mut photos, |_| true);
        photos.into_iter().filter_map(|photo| {
            let date = crate::incoming::catalog_capture_date(photo.capture.as_deref()?)?;
            Some((photo.id, date))
        }).collect()
    }

    fn id(&self) -> i64 { self.next_id.fetch_sub(1, Ordering::Relaxed) }

    fn folder_path(&self, folder_id: i64) -> Option<String> {
        self.asset(folder_id).and_then(|a| a.path).or_else(|| {
            // The ancestor query also counts catalogued assets, which a path lookup does not need.
            self.db.lock().ok()?.query_row("SELECT path FROM folders WHERE id=?1", [folder_id], |row| row.get(0))
                .ok().flatten()
        })
    }

    pub fn invalidate(&self, folder_id: i64) -> Result<(), String> {
        self.invalidate_path(&self.folder_path(folder_id).ok_or("folder path unavailable")?)
    }

    pub fn check_folder_changed(self: &Arc<Self>, folder_id: i64) -> bool {
        let Some(path) = self.folder_path(folder_id) else { return false };
        let key = normalize(&path);
        let (signature, source) = {
            let Ok(cache) = self.listings.lock() else { return false };
            let Some(cached) = cache.get(&key) else { return false };
            let Some(signature) = cached.signature else { return false };
            (signature, cached.rows.clone())
        };
        {
            let Ok(mut checking) = self.checking.lock() else { return false };
            if !checking.insert(key.clone()) { return false; }
        }

        let (tx, rx) = mpsc::sync_channel(1);
        let check = Arc::new(FolderCheck { service: self.clone(), key: key.clone() });
        // Both the caller and worker hold the marker through completion, including a timeout.
        let worker = check.clone();
        if std::thread::Builder::new().spawn(move || {
            let result = read_directory(Path::new(&path), &worker.service.formats, false);
            drop(worker);
            let _ = tx.send(result);
        }).is_err() {
            return false;
        }
        let Ok(Ok((current, _))) = rx.recv_timeout(LIST_TIMEOUT) else { return false };
        if current == signature { return false; }

        let Ok(mut cache) = self.listings.lock() else { return false };
        // F5 or another write may have replaced this listing while the check ran.
        if !cache.get(&key).is_some_and(|cached| Arc::ptr_eq(&cached.rows, &source)) { return false; }
        cache.remove(&key);
        drop(cache);
        if let Ok(mut prepared) = self.prepared.lock() {
            if prepared.as_ref().is_some_and(|listing| Arc::ptr_eq(&listing.source, &source)) {
                *prepared = None;
            }
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn revision(&self) -> u64 { self.revision.load(Ordering::Relaxed) }

    pub fn invalidate_path(&self, path: &str) -> Result<(), String> {
        self.listings.lock().map_err(|_| "listing cache poisoned")?.remove(&normalize(path));
        *self.prepared.lock().map_err(|_| "prepared listing cache poisoned")? = None;
        self.revision.fetch_add(1, Ordering::Relaxed);
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

    fn remember_listing(&self, key: String, rows: Arc<Vec<Asset>>, signature: Option<DirectorySignature>) {
        if let Ok(mut cache) = self.listings.lock() {
            cache.remove(&key);
            let mut count = rows.len() + cache.values().map(|cached| cached.rows.len()).sum::<usize>();
            while !cache.is_empty() && (cache.len() >= MAX_LISTINGS || count > MAX_LISTING_ROWS) {
                let oldest = cache.iter().min_by_key(|(_, cached)| cached.when).map(|(key, _)| key.clone()).unwrap();
                if let Some(old) = cache.remove(&oldest) { count -= old.rows.len(); }
            }
            cache.insert(key, CachedListing { when: Instant::now(), rows, signature });
        }
    }

    fn list(&self, path: &str, folder_ids: &[i64]) -> (Arc<Vec<Asset>>, bool) {
        let key = normalize(path);
        if let Ok(cache) = self.listings.lock() {
            if let Some(cached) = cache.get(&key) {
                let offline = cached.signature.is_none();
                if !offline || cached.when.elapsed() < OFFLINE_RETRY { return (cached.rows.clone(), offline); }
            }
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let root = PathBuf::from(path);
        let formats = self.formats.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_directory(&root, &formats, true));
        });
        let (signature, disk) = match rx.recv_timeout(LIST_TIMEOUT) { Ok(Ok(v)) => v, _ => {
            let mut rows = Vec::new();
            if let Ok(context) = db::folder_disk_context(&self.db, folder_ids) {
                for (_, _, assets) in context { rows.extend(assets.into_iter().filter(|a| !a.ext.is_empty() && !self.formats.is_hidden(&a.ext)).map(|mut a| { a.offline = true; a })); }
            }
            let rows = Arc::new(rows);
            self.remember_listing(key, rows.clone(), None);
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
                path: Some(full.to_string_lossy().to_string()), is_directory: is_dir, offline: false, missing: false, has_categories: false, has_keywords: false, display_categories: None, display_keywords: None, quarantined_at: None, edit_pending_since: None,
            });
            asset.name = name;
            asset.ext = ext;
            asset.size = if is_dir { Some(0.0) } else { Some(meta.len() as f64) };
            asset.file_modified = meta.modified().ok().map(system_time_iso);
            asset.path = Some(full.to_string_lossy().to_string());
            asset.is_directory = is_dir;
            asset.offline = false;
            if asset.id <= -10_000_000 {
                if let Ok(mut entries) = self.temporary.lock() {
                    if let Some(path) = &asset.path {
                        if let Some(id) = entries.paths.get(&TemporaryEntries::path_key(path, is_dir)) { asset.id = *id; }
                    }
                    entries.insert(asset.clone(), folder_ids.first().copied());
                }
            }
            rows.push(asset);
        }
        rows.sort_by(|a,b| a.is_directory.cmp(&b.is_directory).reverse().then_with(||a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id)));
        let rows = Arc::new(rows);
        self.remember_listing(key, rows.clone(), Some(signature));
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

    pub(crate) fn listed_folder(&self, filter: &AssetFilter) -> Result<(String, Arc<Vec<Asset>>), String> {
        let (path, ids) = self.folder_of_filter(filter)?;
        let (rows, _) = self.list(&path, &ids);
        Ok((path, rows))
    }

    pub(crate) fn ordered_rows(&self, filter: &AssetFilter) -> Result<(Arc<Vec<Asset>>, f64), String> {
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
        if matches!(filter.sort.as_str(), "date" | "dateTaken" | "captureTime") {
            let mut dates = self.missing_capture_dates(&files);
            for file in &mut files {
                if let Some(date) = dates.remove(&file.id) { file.exif_date = Some(date); }
            }
        }
        let ordering = |a: &Asset,b: &Asset| db::compare_assets(a,b,&filter.sort,filter.descending);
        dirs.sort_by(|a,b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(ordering);
        dirs.extend(files);
        let total_size=dirs.iter().filter(|a|!a.is_directory).map(|a|a.size.unwrap_or(0.0)).sum();
        let rows = Arc::new(dirs);
        *self.prepared.lock().map_err(|_| "prepared listing cache poisoned")? = Some(PreparedListing { source, key, rows: rows.clone(), total_size });
        (rows, total_size)
        };
        Ok((rows, total_size))
    }

    pub(crate) fn locate_asset(&self, filter: &AssetFilter, id: i64) -> Result<Option<usize>, String> {
        let (rows, _) = self.ordered_rows(filter)?;
        Ok(rows.iter().position(|asset| asset.id == id))
    }

    pub fn page(&self, filter: &AssetFilter) -> Result<AssetPage, String> {
        let (rows, total_size) = self.ordered_rows(filter)?;
        let total = rows.len() as i64;
        let offset=filter.offset.max(0) as usize; let limit=filter.limit.clamp(1,500) as usize;
        let start=offset.min(rows.len());
        let end=start.saturating_add(limit).min(rows.len());
        let mut items=rows[start..end].to_vec();
        db::attach_display_values(&self.db,&mut items,filter.include_categories,filter.include_keywords).map_err(|e|e.to_string())?;
        Ok(AssetPage { items, total, total_size })
    }

    pub fn asset(&self, id: i64) -> Option<Asset> { {
        if let Some(asset) = self.temporary.lock().ok()?.entries.get(&id).cloned() { return Some(asset); }
        if id > -10_000_000 { return None; }
        self.listings.lock().ok()?.values().find_map(|cached| cached.rows.iter().find(|a| a.id == id).cloned())
    } }
    #[cfg(test)]
    pub fn insert_test_asset(&self, asset:Asset){if let Ok(mut entries)=self.temporary.lock(){entries.insert(asset,None);}}

    pub fn resolve_folder(&self,path:&str)->Result<Option<db::Folder>,String>{
        if let Some(folder)=db::folder_by_path(&self.db,path).map_err(|e|e.to_string())? { return Ok(Some(folder)); }
        let path_buf=PathBuf::from(path);
        if !path_buf.is_dir(){return Ok(None)}
        let key=normalize(path);
        let cached_id = self.temporary.lock().map_err(|_|"path map poisoned")?.paths.get(&(key.clone(), true)).copied();
        if let Some(id)=cached_id{
            if let Some(asset)=self.asset(id){let parent=self.temporary.lock().ok().and_then(|p|p.parents.get(&id).copied());return Ok(Some(db::Folder{id,parent_id:parent,root_id:parent.and_then(|p|self.asset(p)).and_then(|a|a.folder_id).or_else(||parent.and_then(|p|db::folder_ancestors(&self.db,p).ok()?.last()?.root_id)),name:asset.name,path:asset.path,count:0,folder_ids:vec![id],virtual_node:None,offline:false,missing:false}));}
        }
        let parent_path=path_buf.parent().map(|p|p.to_string_lossy().to_string());
        let parent=if let Some(parent_path)=parent_path.filter(|p|!p.is_empty()&&normalize(p)!=key){self.resolve_folder(&parent_path)?}else{None};
        let name=path_buf.file_name().map(|n|n.to_string_lossy().to_string()).unwrap_or_else(||path.to_string());
        let id=self.id(); let parent_id=parent.as_ref().map(|p|p.id); let root_id=parent.as_ref().and_then(|p|p.root_id);
        let asset=Asset{id,folder_id:None,name:name.clone(),ext:String::new(),size:Some(0.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(path_buf.to_string_lossy().to_string()),is_directory:true,offline:false,missing:false,has_categories:false, has_keywords: false,display_categories:None,display_keywords:None,quarantined_at:None,edit_pending_since:None};
        self.temporary.lock().map_err(|_|"entry map poisoned")?.insert(asset, parent_id);
        Ok(Some(db::Folder{id,parent_id,root_id,name,path:Some(path_buf.to_string_lossy().to_string()),count:0,folder_ids:vec![id],virtual_node:None,offline:false,missing:false}))
    }

    pub fn folder_ancestors(&self, id:i64) -> Result<Vec<db::Folder>,String> {
        let Some(asset)=self.asset(id).filter(|a|a.is_directory) else { return db::folder_ancestors(&self.db,id).map_err(|e|e.to_string()) };
        if let Some(catalog_id)=asset.folder_id { return db::folder_ancestors(&self.db,catalog_id).map_err(|e|e.to_string()); }
        let parent=self.temporary.lock().map_err(|_|"folder map poisoned")?.parents.get(&id).copied().ok_or("folder parent not found")?;
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
        service.remember_listing(normalize(path), source, Some(DirectorySignature::default()));
        let mut filter: AssetFilter = serde_json::from_value(serde_json::json!({"kind":"folder","folderPath":path,"sort":"name","descending":false,"offset":0,"limit":2})).unwrap();
        let first = service.page(&filter).unwrap();
        assert_eq!(first.items.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["folder", "a.jpg"]);
        assert_eq!((first.total, first.total_size), (3, 30.0));
        let prepared = service.prepared.lock().unwrap().as_ref().unwrap().rows.clone();
        filter.offset = 2; filter.limit = 1; filter.include_categories = true;
        let second = service.page(&filter).unwrap();
        assert_eq!(second.items[0].name, "b.jpg");
        assert_eq!(service.locate_asset(&filter, -3).unwrap(), Some(1));
        assert!(Arc::ptr_eq(&prepared, &service.prepared.lock().unwrap().as_ref().unwrap().rows));
        filter.query = Some("b.jpg".into()); filter.offset = 0;
        let filtered = service.page(&filter).unwrap();
        assert_eq!((filtered.total, filtered.total_size), (2, 20.0));
        assert_eq!(service.locate_asset(&filter, -3).unwrap(), None);
        assert_eq!(service.locate_asset(&filter, -2).unwrap(), Some(1));
        assert!(!Arc::ptr_eq(&prepared, &service.prepared.lock().unwrap().as_ref().unwrap().rows));
        service.invalidate_path(path).unwrap();
        assert!(service.prepared.lock().unwrap().is_none());
        service.remember_listing(normalize(path), Arc::new(vec![asset(-4,"new.jpg",false,7.0)]), Some(DirectorySignature::default()));
        filter.query = None;
        let refreshed = service.page(&filter).unwrap();
        assert_eq!((refreshed.total, refreshed.total_size, refreshed.items[0].name.as_str()), (1, 7.0, "new.jpg"));
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("ddreams-listing-{}-{unique}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn directory_signature_tracks_visible_entry_changes() {
        let dir = TempDir::new();
        let formats = FormatTable::load_selected(None);
        let signature = || read_directory(&dir.0, &formats, false).unwrap().0;
        let empty = signature();
        assert_eq!(signature(), empty);
        std::fs::write(dir.0.join(".ignored.jpg"), b"ignored").unwrap();
        assert_eq!(signature(), empty);

        let file = dir.0.join("photo.jpg");
        std::fs::write(&file, b"a").unwrap();
        let added = signature();
        assert_ne!(added, empty);
        assert_eq!(signature(), added);
        assert_eq!(read_directory(&dir.0, &formats, true).unwrap().0, added);
        std::fs::OpenOptions::new().write(true).open(&file).unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000)).unwrap();
        let modified = signature();
        assert_ne!(modified, added);
        std::fs::write(&file, b"larger").unwrap();
        std::fs::OpenOptions::new().write(true).open(&file).unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000)).unwrap();
        let resized = signature();
        assert_ne!(resized, modified);
        let renamed = dir.0.join("renamed.jpg");
        std::fs::rename(file, &renamed).unwrap();
        assert_ne!(signature(), resized);
        std::fs::remove_file(renamed).unwrap();
        assert_eq!(signature(), empty);
    }

    #[test]
    fn folder_check_requires_online_cache_and_keeps_existing_file_ids() {
        let dir = TempDir::new();
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let service = ListingService::new(db, FormatTable::load_selected(None));
        let path = dir.0.to_str().unwrap();
        let mut folder = asset(-11_000_000, "test", true, 0.0);
        folder.path = Some(path.to_owned());
        let id = folder.id;
        service.insert_test_asset(folder);
        assert!(!service.check_folder_changed(id));

        service.remember_listing(normalize(path), Arc::new(Vec::new()), None);
        let file = dir.0.join("photo.jpg");
        std::fs::write(&file, b"a").unwrap();
        assert!(!service.check_folder_changed(id));
        service.invalidate(id).unwrap();
        let (rows, offline) = service.list(path, &[]);
        assert!(!offline);
        let file_id = rows[0].id;
        assert!(!service.check_folder_changed(id));
        std::fs::write(dir.0.join("another.jpg"), b"b").unwrap();
        assert!(service.check_folder_changed(id));
        assert!(!service.check_folder_changed(id));
        let (rows, _) = service.list(path, &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().find(|row| row.name == "photo.jpg").unwrap().id, file_id);
    }
}

type DiskEntry = (PathBuf, String, std::fs::Metadata);

fn read_directory(path: &Path, formats: &FormatTable, collect_rows: bool) -> std::io::Result<(DirectorySignature, Vec<DiskEntry>)> {
    let mut signature = DirectorySignature::default();
    let mut rows = Vec::new();
    for item in std::fs::read_dir(path)? {
        let (entry, metadata) = match item.and_then(|entry| {
            let metadata = entry.metadata()?;
            Ok((entry, metadata))
        }) {
            Ok(entry) => entry,
            // The normal listing can display readable entries. A check must not
            // interpret an incomplete scan as deletions.
            Err(_) if collect_rows => continue,
            Err(error) => return Err(error),
        };
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if hidden_or_ignored(&name, &metadata, formats) { continue; }
        let modified = match metadata.modified() {
            Ok(time) => Some(time),
            Err(_) if collect_rows => None,
            Err(error) => return Err(error),
        };
        let mut hash = DefaultHasher::new();
        (file_name.as_os_str(), metadata.is_dir(), metadata.len(), modified).hash(&mut hash);
        signature.count += 1;
        // Directory iteration order is unspecified. Include case for case-only renames.
        signature.hash = signature.hash.wrapping_add(hash.finish());
        if collect_rows { rows.push((entry.path(), name.into_owned(), metadata)); }
    }
    Ok((signature, rows))
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
