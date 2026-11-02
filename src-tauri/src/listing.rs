use crate::db::{self, Asset, AssetFilter, AssetPage, Db};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{atomic::{AtomicI64, Ordering}, mpsc}, time::Duration};

const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const OFFLINE_RETRY: Duration = Duration::from_secs(8);

pub struct ListingService {
    db: Db,
    entries: std::sync::Mutex<HashMap<i64, Asset>>,
    directory_parents: std::sync::Mutex<HashMap<i64, i64>>,
    path_ids: std::sync::Mutex<HashMap<String,i64>>,
    listings: std::sync::Mutex<HashMap<String, (std::time::Instant, Vec<Asset>)>>,
    next_id: AtomicI64,
}

impl ListingService {
    pub fn new(db: Db) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { db, entries: std::sync::Mutex::new(HashMap::new()), directory_parents: std::sync::Mutex::new(HashMap::new()), path_ids:std::sync::Mutex::new(HashMap::new()), listings: std::sync::Mutex::new(HashMap::new()), next_id: AtomicI64::new(-10_000_000) })
    }

    fn id(&self) -> i64 { self.next_id.fetch_sub(1, Ordering::Relaxed) }

    pub fn invalidate(&self, folder_id: i64) -> Result<(), String> {
        let path = self.asset(folder_id).and_then(|a| a.path).or_else(|| db::folder_ancestors(&self.db, folder_id).ok()?.last()?.path.clone()).ok_or("folder path unavailable")?;
        self.listings.lock().map_err(|_| "listing cache poisoned")?.remove(&normalize(&path));
        Ok(())
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
        std::thread::spawn(move || {
            let result = std::fs::read_dir(&root).map(|items| {
                items.filter_map(Result::ok).filter_map(|entry| {
                    let p = entry.path(); let name = entry.file_name().to_string_lossy().to_string();
                    if hidden_or_ignored(&name, &entry.metadata().ok()?) { return None; }
                    let meta = entry.metadata().ok()?;
                    Some((p, name, meta))
                }).collect::<Vec<_>>()
            });
            let _ = tx.send(result);
        });
        let disk = match rx.recv_timeout(LIST_TIMEOUT) { Ok(Ok(v)) => v, _ => {
            let mut rows = Vec::new();
            if let Ok(context) = db::folder_disk_context(&self.db, folder_ids) {
                for (_, _, assets) in context { rows.extend(assets.into_iter().filter(|a| a.ext != "" && a.ext.to_ascii_lowercase() != "xmp").map(|mut a| { a.offline = true; a })); }
            }
            if let Ok(mut cache) = self.listings.lock() { cache.insert(key, (std::time::Instant::now(), rows.clone())); }
            return (rows, true);
        }};
        let contexts = db::folder_disk_context(&self.db, folder_ids).unwrap_or_default();
        let mut known = HashMap::<String, Asset>::new();
        for (_, catalog_path, assets) in contexts { if normalize(&catalog_path)==key { for asset in assets { known.insert(asset.name.to_lowercase(), asset); } } }
        let mut rows = Vec::with_capacity(disk.len());
        for (full, name, meta) in disk {
            let is_dir = meta.is_dir();
            let ext = if is_dir { String::new() } else { full.extension().unwrap_or_default().to_string_lossy().trim_start_matches('.').to_ascii_lowercase() };
            let matched_folder = if is_dir { db::folder_by_path(&self.db, &full.to_string_lossy()).ok().flatten().map(|f|f.id) } else { None };
            let mut asset = known.remove(&name.to_lowercase()).unwrap_or_else(|| Asset {
                id: self.id(), folder_id: matched_folder, name: name.clone(), ext: ext.clone(), size: None,
                width: None, height: None, bpp: None, caption: None, notes: None, author: None, rating: None, tagged: None,
                label: None, file_modified: None, exif_date: None, acdsee_date: None, md_dirty: None, is_orphan: None,
                path: Some(full.to_string_lossy().to_string()), is_directory: is_dir, offline: false, has_categories: false,
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

    pub fn page(&self, filter: &AssetFilter) -> Result<AssetPage, String> {
        let mut path = filter.folder_path.clone().unwrap_or_default();
        if path.is_empty() {
            let id = filter.id.unwrap_or(-1);
            if id <= -10_000_000 {
                path = self.asset(id).and_then(|a| a.path).unwrap_or_default();
            } else if let Some(f) = db::folder_ancestors(&self.db, id).map_err(|e| e.to_string())?.last() { path = f.path.clone().unwrap_or_default(); }
        }
        let ids = filter.ids.clone().unwrap_or_else(|| filter.id.map(|id| match self.asset(id).filter(|a|a.is_directory) { Some(a)=>vec![a.folder_id.unwrap_or(id)],None=>vec![id] }).unwrap_or_default());
        let (mut rows, _) = self.list(&path, &ids);
        rows.retain(|a| a.is_directory || (!a.ext.is_empty() && a.ext.to_ascii_lowercase() != "xmp"));
        if let Some(q) = filter.query.as_deref().filter(|q| !q.trim().is_empty()) { let q=q.to_lowercase(); rows.retain(|a|a.is_directory||a.name.to_lowercase().contains(&q)||a.caption.as_deref().is_some_and(|s|s.to_lowercase().contains(&q))||a.notes.as_deref().is_some_and(|s|s.to_lowercase().contains(&q))); }
        rows.retain(|a| a.is_directory || db::asset_filter_matches(a, filter));
        let dir_count = rows.iter().filter(|a| a.is_directory).count();
        let (dirs, files) = rows.split_at(dir_count);
        let mut dirs=dirs.to_vec(); let mut files=files.to_vec();
        let ordering = |a: &Asset,b: &Asset| db::compare_assets(a,b,&filter.sort,filter.descending);
        dirs.sort_by(|a,b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(ordering);
        dirs.extend(files);
        let total = dirs.len() as i64; let total_size=dirs.iter().filter(|a|!a.is_directory).map(|a|a.size.unwrap_or(0.0)).sum();
        let offset=filter.offset.max(0) as usize; let limit=filter.limit.clamp(1,500) as usize;
        Ok(AssetPage { items: dirs.into_iter().skip(offset).take(limit).collect(), total, total_size })
    }

    pub fn asset(&self, id: i64) -> Option<Asset> { self.entries.lock().ok()?.get(&id).cloned() }

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
        let asset=Asset{id,folder_id:None,name:name.clone(),ext:String::new(),size:Some(0.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(path_buf.to_string_lossy().to_string()),is_directory:true,offline:false,has_categories:false};
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
fn hidden_or_ignored(name: &str, metadata: &std::fs::Metadata) -> bool {
    let lower=name.to_ascii_lowercase();
    if name.starts_with('.') || ["desktop.ini",".picasa.ini"].contains(&lower.as_str()) || ["xmp","acr","thm"].contains(&Path::new(name).extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase().as_str()) { return true; }
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if metadata.file_attributes() & (0x2 | 0x4) != 0 { return true; } }
    let _=metadata; false
}
fn system_time_iso(time: std::time::SystemTime) -> String {
    let secs=time.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let days=secs.div_euclid(86_400); let day_secs=secs.rem_euclid(86_400);
    let z=days+719_468; let era=(if z>=0 {z} else {z-146_096})/146_097; let doe=z-era*146_097;
    let yoe=(doe-doe/1_460+doe/36_524-doe/146_096)/365; let mut year=yoe+era*400;
    let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153;
    let day=doy-(153*mp+2)/5+1; let month=mp+if mp<10 {3} else {-9}; if month<=2 {year+=1;}
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",day_secs/3600,(day_secs%3600)/60,day_secs%60)
}
