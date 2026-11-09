use crate::{db::{self, Asset, AssetFilter, AssetPage, Db}, formats::FormatTable};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{atomic::{AtomicI64, Ordering}, mpsc}, time::Duration};

const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const OFFLINE_RETRY: Duration = Duration::from_secs(8);

pub struct ListingService {
    db: Db,
    formats: std::sync::Arc<FormatTable>,
    entries: std::sync::Mutex<HashMap<i64, Asset>>,
    directory_parents: std::sync::Mutex<HashMap<i64, i64>>,
    path_ids: std::sync::Mutex<HashMap<String,i64>>,
    listings: std::sync::Mutex<HashMap<String, (std::time::Instant, Vec<Asset>)>>,
    next_id: AtomicI64,
}

impl ListingService {
    pub fn new(db: Db, formats: std::sync::Arc<FormatTable>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { db, formats, entries: std::sync::Mutex::new(HashMap::new()), directory_parents: std::sync::Mutex::new(HashMap::new()), path_ids:std::sync::Mutex::new(HashMap::new()), listings: std::sync::Mutex::new(HashMap::new()), next_id: AtomicI64::new(-10_000_000) })
    }

    fn id(&self) -> i64 { self.next_id.fetch_sub(1, Ordering::Relaxed) }

    pub fn invalidate(&self, folder_id: i64) -> Result<(), String> {
        let path = self.asset(folder_id).and_then(|a| a.path).or_else(|| db::folder_ancestors(&self.db, folder_id).ok()?.last()?.path.clone()).ok_or("folder path unavailable")?;
        self.invalidate_path(&path)?;
        Ok(())
    }

    pub fn invalidate_path(&self, path: &str) -> Result<(), String> {
        self.listings.lock().map_err(|_| "listing cache poisoned")?.remove(&normalize(path));
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

    fn list(&self, path: &str, folder_ids: &[i64]) -> (Vec<Asset>, bool) {
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
                    if hidden_or_ignored(&name, &entry.metadata().ok()?, &formats) { return None; }
                    let meta = entry.metadata().ok()?;
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
        let mut rows = Vec::with_capacity(disk.len());
        for (full, name, meta) in disk {
            let is_dir = meta.is_dir();
            let ext = if is_dir { String::new() } else { full.extension().unwrap_or_default().to_string_lossy().trim_start_matches('.').to_ascii_lowercase() };
            let matched_folder = if is_dir { db::folder_by_path(&self.db, &full.to_string_lossy()).ok().flatten().map(|f|f.id) } else { None };
            let mut asset = known.remove(&name.to_lowercase()).unwrap_or_else(|| Asset {
                id: self.id(), folder_id: if is_dir { matched_folder } else { parent_folder_id }, name: name.clone(), ext: ext.clone(), size: None,
                width: None, height: None, bpp: None, caption: None, notes: None, author: None, rating: None, tagged: None,
                label: None, file_modified: None, exif_date: None, acdsee_date: None, md_dirty: None, is_orphan: None,
                path: Some(full.to_string_lossy().to_string()), is_directory: is_dir, offline: false, has_categories: false, display_categories: None, display_keywords: None,
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
        Ok((path, rows.into_iter().filter(|a| !a.is_directory && a.id < 0 && !a.ext.is_empty() && a.path.is_some()).collect()))
    }

    pub fn page(&self, filter: &AssetFilter) -> Result<AssetPage, String> {
        let (path, ids) = self.folder_of_filter(filter)?;
        let (mut rows, _) = self.list(&path, &ids);
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
        let total = dirs.len() as i64; let total_size=dirs.iter().filter(|a|!a.is_directory).map(|a|a.size.unwrap_or(0.0)).sum();
        let offset=filter.offset.max(0) as usize; let limit=filter.limit.clamp(1,500) as usize;
        let mut items=dirs.into_iter().skip(offset).take(limit).collect::<Vec<_>>();
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
            if let Some(asset)=self.asset(id){let parent=self.directory_parents.lock().ok().and_then(|p|p.get(&id).copied());return Ok(Some(db::Folder{id,parent_id:parent,root_id:parent.and_then(|p|self.asset(p)).and_then(|a|a.folder_id).or_else(||parent.and_then(|p|db::folder_ancestors(&self.db,p).ok()?.last()?.root_id)),name:asset.name,path:asset.path,count:0,folder_ids:vec![id],virtual_node:None,offline:false}));}
        }
        let parent_path=path_buf.parent().map(|p|p.to_string_lossy().to_string());
        let parent=if let Some(parent_path)=parent_path.filter(|p|!p.is_empty()&&normalize(p)!=key){self.resolve_folder(&parent_path)?}else{None};
        let name=path_buf.file_name().map(|n|n.to_string_lossy().to_string()).unwrap_or_else(||path.to_string());
        let id=self.id(); let parent_id=parent.as_ref().map(|p|p.id); let root_id=parent.as_ref().and_then(|p|p.root_id);
        let asset=Asset{id,folder_id:None,name:name.clone(),ext:String::new(),size:Some(0.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(path_buf.to_string_lossy().to_string()),is_directory:true,offline:false,has_categories:false,display_categories:None,display_keywords:None};
        self.entries.lock().map_err(|_|"entry map poisoned")?.insert(id,asset);
        if let Some(parent_id)=parent_id{self.directory_parents.lock().map_err(|_|"folder map poisoned")?.insert(id,parent_id);}
        self.path_ids.lock().map_err(|_|"path map poisoned")?.insert(key,id);
        Ok(Some(db::Folder{id,parent_id,root_id,name,path:Some(path_buf.to_string_lossy().to_string()),count:0,folder_ids:vec![id],virtual_node:None,offline:false}))
    }

    pub fn folder_ancestors(&self, id:i64) -> Result<Vec<db::Folder>,String> {
        let Some(asset)=self.asset(id).filter(|a|a.is_directory) else { return db::folder_ancestors(&self.db,id).map_err(|e|e.to_string()) };
        if let Some(catalog_id)=asset.folder_id { return db::folder_ancestors(&self.db,catalog_id).map_err(|e|e.to_string()); }
        let parent=self.directory_parents.lock().map_err(|_|"folder map poisoned")?.get(&id).copied().ok_or("folder parent not found")?;
        let mut chain=if parent <= -10_000_000 { self.folder_ancestors(parent)? } else { db::folder_ancestors(&self.db,parent).map_err(|e|e.to_string())? };
        let root_id=chain.last().and_then(|f|f.root_id);
        chain.push(db::Folder{id,parent_id:Some(parent),root_id,name:asset.name,path:asset.path,count:0,folder_ids:vec![id],virtual_node:None,offline:false});
        Ok(chain)
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
