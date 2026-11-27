use crate::{db::Db, journal::{AssetIdRemap, CatalogWriter, FileCatalogCopy, FileCatalogMove}, listing::ListingService};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};
use std::{collections::HashSet, fs::{self, File, FileTimes, OpenOptions}, io, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::SystemTime};
#[cfg(test)] use std::sync::atomic::AtomicI64;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy { Skip, KeepBoth, Cancel }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteInfo { pub count: usize, pub names: Vec<String>, pub sidecars: usize, pub permanent_count: usize, pub no_bin_drives: Vec<String> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct DeleteFolderInfo{pub files:usize,pub folders:usize,pub catalogued:usize,pub bytes:u64,pub recycle_available:bool}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult { pub change_id: Option<i64>, pub summary: String, pub changed_count: usize, pub skipped_count: usize, pub remapped_ids: Vec<AssetIdRemap>, pub affected_ids: Vec<i64>, pub affected_folder_paths: Vec<String> }

#[derive(Clone, Debug)]
struct Transfer { from: PathBuf, to: PathBuf }
#[derive(Clone, Debug)]
struct Source { id: i64, requested_id: i64, path: PathBuf, name: String, size: u64, modified: SystemTime }
#[derive(Clone, Debug)]
struct Plan { source: Source, target: PathBuf, pairs: Vec<Transfer>, conflict: bool }

pub trait FileSystemOps: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn is_symlink(&self,path:&Path)->bool;
    fn metadata(&self, path: &Path) -> io::Result<fs::Metadata>;
    fn same_volume(&self, source: &Path, target: &Path) -> bool;
    fn move_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn copy_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn set_modified(&self, path: &Path, time: SystemTime) -> io::Result<()>;
    /// User-initiated delete: Recycle Bin only, refused where there is no Recycle Bin (USB/network).
    fn recycle_delete(&self, paths: &[PathBuf]) -> Result<(), String>;
    /// Whether deleting from this path can go to the Recycle Bin (fixed local disk).
    fn has_recycle_bin(&self, path: &Path) -> bool;
    /// Rolls back a recycle operation when the catalogue write fails.
    fn restore_recycled(&self, original: &Path) -> Result<(), String>;
}

pub struct NativeFileSystem;
impl FileSystemOps for NativeFileSystem {
    fn exists(&self, path:&Path)->bool { path.exists() }
    fn is_dir(&self,path:&Path)->bool { path.is_dir() }
    fn is_symlink(&self,path:&Path)->bool { fs::symlink_metadata(path).is_ok_and(|m|m.file_type().is_symlink()) }
    fn metadata(&self,path:&Path)->io::Result<fs::Metadata> { fs::metadata(path) }
    fn same_volume(&self,a:&Path,b:&Path)->bool { volume_key(a)==volume_key(b) }
    fn move_no_replace(&self,source:&Path,target:&Path)->io::Result<()> {
        #[cfg(windows)] { use std::os::windows::ffi::OsStrExt; use windows::{core::PCWSTR,Win32::Storage::FileSystem::MoveFileW}; let from:Vec<u16>=source.as_os_str().encode_wide().chain(Some(0)).collect();let to:Vec<u16>=target.as_os_str().encode_wide().chain(Some(0)).collect();unsafe { MoveFileW(PCWSTR(from.as_ptr()),PCWSTR(to.as_ptr())).map_err(|e|io::Error::other(e.to_string()))?; }Ok(()) }
        #[cfg(not(windows))] { if target.exists(){return Err(io::Error::new(io::ErrorKind::AlreadyExists,"target exists"));}fs::rename(source,target) }
    }
    fn copy_no_replace(&self,source:&Path,target:&Path)->io::Result<()> {
        let mut input=File::open(source)?;let mut output=OpenOptions::new().write(true).create_new(true).open(target)?;
        let result=(||{io::copy(&mut input,&mut output)?;output.set_times(FileTimes::new().set_modified(input.metadata()?.modified()?))?;output.sync_all()?;Ok(())})();
        if result.is_err(){let _=fs::remove_file(target);}result
    }
    fn remove_file(&self,path:&Path)->io::Result<()> { fs::remove_file(path) }
    fn create_dir(&self,path:&Path)->io::Result<()> { fs::create_dir(path) }
    fn remove_dir(&self,path:&Path)->io::Result<()> { fs::remove_dir(path) }
    fn read_dir(&self,path:&Path)->io::Result<Vec<PathBuf>> { fs::read_dir(path)?.map(|entry|entry.map(|e|e.path())).collect() }
    fn set_modified(&self,path:&Path,time:SystemTime)->io::Result<()> { File::options().write(true).open(path)?.set_times(FileTimes::new().set_modified(time)) }
    fn recycle_delete(&self,paths:&[PathBuf])->Result<(),String> {
        for path in paths { if !has_recycle_bin(path) { return Err(format!("Na disku so súborom {} nie je Kôš (USB alebo sieťový disk). DDreams tu súbory nemaže, zmazanie by bolo trvalé.",path.display())); } }
        recycle_windows(paths)?;
        if let Some(left)=paths.iter().find(|p|p.exists()) { return Err(format!("Súbor sa nepodarilo presunúť do Koša: {}",left.display())); }
        Ok(())
    }
    fn has_recycle_bin(&self,path:&Path)->bool { has_recycle_bin(path) }
    fn restore_recycled(&self,original:&Path)->Result<(),String> {
        let (data,info)=find_recycled(original).ok_or_else(||format!("Súbor nie je v Koši (Kôš bol možno vyprázdnený): {}",original.display()))?;
        if original.exists() { return Err(format!("Na pôvodnom mieste už je súbor: {}",original.display())); }
        if let Some(parent)=original.parent() { fs::create_dir_all(parent).map_err(|e|format!("{}: {e}",parent.display()))?; }
        self.move_no_replace(&data,original).map_err(|e|format!("Súbor sa nepodarilo vrátiť z Koša: {} ({e})",original.display()))?;
        let _=fs::remove_file(info);
        Ok(())
    }
}

/// Fixed local disks have a Recycle Bin; removable and network drives delete permanently.
fn has_recycle_bin(path:&Path)->bool{
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt; use windows::{core::PCWSTR,Win32::Storage::FileSystem::GetDriveTypeW};
        let Some(root)=path.ancestors().last().map(|r|r.to_path_buf()) else { return false };
        let root_text=root.to_string_lossy().to_string(); if root_text.len()<2||!root_text.as_bytes()[1].eq(&b':') { return false; }
        let wide:Vec<u16>=std::ffi::OsStr::new(&format!("{}\\",&root_text[..2])).encode_wide().chain(Some(0)).collect();
        const DRIVE_FIXED:u32=3; unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) == DRIVE_FIXED }
    }
    #[cfg(not(windows))] { let _=path; false }
}

/// Windows keeps each recycled file as `$R<id>.<ext>` (content) next to `$I<id>.<ext>` (metadata:
/// version, size, deletion time, original path) in `<drive>\$Recycle.Bin\<user SID>\`.
/// Returns (`$R` data file, `$I` info file) of the most recently deleted match.
fn find_recycled(original:&Path)->Option<(PathBuf,PathBuf)>{
    let text=original.to_string_lossy().to_string(); if text.len()<2 { return None; }
    let bin=PathBuf::from(format!("{}\\$Recycle.Bin",&text[..2]));
    let wanted=text.replace('/',"\\").to_lowercase();
    let mut best:Option<(u64,PathBuf,PathBuf)>=None;
    for sid in fs::read_dir(&bin).ok()?.flatten() {
        let Ok(entries)=fs::read_dir(sid.path()) else { continue };
        for entry in entries.flatten() {
            let name=entry.file_name().to_string_lossy().to_string(); if !name.starts_with("$I") { continue; }
            let Ok(bytes)=fs::read(entry.path()) else { continue };
            let Some((deleted,path))=parse_recycle_info(&bytes) else { continue };
            if path.to_lowercase()!=wanted { continue; }
            let data=entry.path().with_file_name(format!("$R{}",&name[2..])); if !data.exists() { continue; }
            if best.as_ref().map(|b|deleted>b.0).unwrap_or(true) { best=Some((deleted,data,entry.path())); }
        }
    }
    best.map(|(_,data,info)|(data,info))
}

/// Parses a `$I` file: v1 = 8 version, 8 size, 8 FILETIME, 520 bytes path; v2 = same + 4-byte length + path.
fn parse_recycle_info(bytes:&[u8])->Option<(u64,String)>{
    if bytes.len()<28 { return None; }
    let version=u64::from_le_bytes(bytes[0..8].try_into().ok()?); let deleted=u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let utf16=|raw:&[u8]|->String{let units:Vec<u16>=raw.chunks_exact(2).map(|c|u16::from_le_bytes([c[0],c[1]])).take_while(|u|*u!=0).collect();String::from_utf16_lossy(&units)};
    match version {
        1 => Some((deleted,utf16(bytes.get(24..(24+520).min(bytes.len()))?))),
        2 => { let len=u32::from_le_bytes(bytes[24..28].try_into().ok()?) as usize; Some((deleted,utf16(bytes.get(28..(28+len*2).min(bytes.len()))?))) }
        _ => None,
    }
}

fn volume_key(path:&Path)->String {
    #[cfg(windows)] { use std::path::Component; for part in path.components(){if let Component::Prefix(prefix)=part{return prefix.as_os_str().to_string_lossy().to_lowercase();}} }
    path.components().next().map(|part|part.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default()
}

#[cfg(windows)]
fn recycle_windows(paths:&[PathBuf])->Result<(),String>{
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR,Win32::{Foundation::HWND,UI::Shell::{SHFileOperationW,SHFILEOPSTRUCTW,FO_DELETE,FOF_ALLOWUNDO,FOF_NOCONFIRMATION,FOF_NOERRORUI,FOF_SILENT}}};
    if paths.is_empty(){return Ok(())}
    let mut from=Vec::<u16>::new();for path in paths{from.extend(path.as_os_str().encode_wide());from.push(0);}from.push(0);
    let mut operation=SHFILEOPSTRUCTW{hwnd:HWND(std::ptr::null_mut()),wFunc:FO_DELETE,pFrom:PCWSTR(from.as_ptr()),pTo:PCWSTR::null(),fFlags:(FOF_ALLOWUNDO|FOF_NOCONFIRMATION|FOF_NOERRORUI|FOF_SILENT).0 as u16,..Default::default()};
    let code=unsafe{SHFileOperationW(&mut operation)};if code!=0||operation.fAnyOperationsAborted.as_bool(){Err(format!("Nepodarilo sa presunúť kópiu do Koša (kód {code})"))}else{Ok(())}
}
#[cfg(not(windows))]
fn recycle_windows(_paths:&[PathBuf])->Result<(),String>{Err("Kôš je dostupný iba vo Windows".into())}

mod quarantine_ops;

pub struct FileManager { db:Db, listing:Arc<ListingService>, writer:Arc<CatalogWriter>, readonly:bool, fs:Arc<dyn FileSystemOps>, operation: Mutex<()>, review: Mutex<Option<crate::quarantine::Review>>, quarantine_delete: Mutex<Vec<crate::quarantine::Snapshot>> }
impl FileManager {
    pub fn new(db:Db,listing:Arc<ListingService>,writer:Arc<CatalogWriter>,readonly:bool)->Arc<Self>{Arc::new(Self{db,listing,writer,readonly,fs:Arc::new(NativeFileSystem),operation:Mutex::new(()),review:Mutex::new(None),quarantine_delete:Mutex::default()})}
    #[cfg(test)] fn with_fs(db:Db,listing:Arc<ListingService>,writer:Arc<CatalogWriter>,readonly:bool,fs:Arc<dyn FileSystemOps>)->Self{Self{db,listing,writer,readonly,fs,operation:Mutex::new(()),review:Mutex::new(None),quarantine_delete:Mutex::default()}}
    fn writable(&self)->Result<(),String>{if self.readonly{Err("Katalóg je iba na čítanie".into())}else{Ok(())}}

    pub(crate) fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.operation.lock().map_err(|e| e.to_string())
    }

    pub(crate) fn sync_incoming(&self, path: &str) -> Result<(), String> {
        self.writer.sync_incoming(path)
    }

    pub fn preview_conflicts(&self,ids:&[i64],target:&str,rename_base:Option<&str>)->Result<usize,String>{let folder=self.validate_folder(target)?;let sources=self.resolve_sources(ids)?;let plans=self.plan(&sources,&folder,rename_base)?;Ok(plans.iter().filter(|p|!path_eq(p.source.path.parent().unwrap_or(Path::new("")),&folder)&&p.conflict).count())}

    pub fn uncategorized_photos_for_move(
        &self, ids: &[i64], target: &str, policy: ConflictPolicy, formats: &crate::formats::FormatTable,
    ) -> Result<usize, String> {
        let folder = self.validate_folder(target)?;
        let sources = self.resolve_sources(ids)?;
        let mut candidates = Vec::new();
        for plan in self.plan(&sources, &folder, None)? {
            if path_eq(plan.source.path.parent().unwrap_or(Path::new("")), &folder)
                || (policy == ConflictPolicy::Skip && plan.conflict)
            {
                continue;
            }
            let ext = plan.source.path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            if matches!(formats.group(&ext), Some("image" | "raw")) {
                candidates.push(plan.source.id);
            }
        }
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let mut assigned = conn.prepare("SELECT EXISTS(SELECT 1 FROM asset_categories WHERE asset_id=?1)")
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        for id in candidates {
            if id < 0 || !assigned.query_row([id], |row| row.get::<_, bool>(0)).map_err(|e| e.to_string())? {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn move_files<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){self.run_copy_move(ids,target,policy,false,&mut progress)}
    pub fn copy_files<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){self.run_copy_move(ids,target,policy,true,&mut progress)}

    fn run_copy_move<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,copy:bool,progress:&mut F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){
        let _operation = self.lock_operation()?;
        self.writable()?;let folder=self.validate_folder(target)?;let sources=self.resolve_sources(ids)?;if sources.is_empty(){return Err("Nie sú vybrané žiadne súbory".into())}let mut plans=self.plan(&sources,&folder,None)?;let same_folder_count=plans.iter().filter(|plan|path_eq(plan.source.path.parent().unwrap_or(Path::new("")),&folder)).count();plans.retain(|plan|!path_eq(plan.source.path.parent().unwrap_or(Path::new("")),&folder));if same_folder_count==sources.len(){return Ok(FileOperationResult{change_id:None,summary:"Súbory už sú v tomto priečinku".into(),changed_count:0,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:Vec::new()});}
        let conflicts=plans.iter().filter(|p|p.conflict).count();if conflicts>0&&policy==ConflictPolicy::Cancel{return Err("Operácia bola zrušená".into())}
        let mut skipped=0usize;let mut reserved=HashSet::new();let mut selected=Vec::new();
        for mut plan in plans.drain(..){if plan.conflict&&policy==ConflictPolicy::Skip{skipped+=1;continue;}if policy==ConflictPolicy::KeepBoth { plan=self.number_conflict(plan,&folder,&reserved)?; }
            if plan.pairs.iter().any(|pair|self.fs.exists(&pair.to)||reserved.contains(&key(&pair.to))){if policy==ConflictPolicy::Skip{skipped+=1;continue;}return Err(format!("V cieli už existuje súbor: {}",plan.target.display()));}
            for pair in &plan.pairs{reserved.insert(key(&pair.to));}selected.push(plan);
        }
        if selected.is_empty(){return Ok(self.no_change_result(skipped,if copy{"Kopírovanie"}else{"Presun"}));}
        let total=selected.len();let mut applied=Vec::<Transfer>::new();let mut copied_transfers=Vec::<Transfer>::new();let mut moved_catalog=Vec::new();let mut copied_catalog=Vec::new();let mut source_ids=Vec::new();
        for (index,plan) in selected.iter().enumerate(){progress(index+1,total);let result=if copy{copy_transfers(self.fs.as_ref(),&plan.pairs)}else{move_transfers(self.fs.as_ref(),&plan.pairs)};
            match result{Ok(done)=>{if copy{copied_transfers.extend(done);}else{applied.extend(done);}source_ids.push(plan.source.id);
                if plan.source.id>0 {if copy{copied_catalog.push(FileCatalogCopy{source_asset_id:plan.source.id,target_path:plan.target.to_string_lossy().to_string(),target_folder_path:folder.to_string_lossy().to_string(),name:plan.target.file_name().unwrap_or_default().to_string_lossy().to_string(),ext:plan.target.extension().unwrap_or_default().to_string_lossy().to_string(),size:plan.source.size as f64,file_modified:Some(crate::listing::system_time_iso(plan.source.modified))});}else{moved_catalog.push(FileCatalogMove{asset_id:plan.source.id,target_path:plan.target.to_string_lossy().to_string(),target_folder_path:folder.to_string_lossy().to_string(),name:plan.target.file_name().unwrap_or_default().to_string_lossy().to_string(),ext:plan.target.extension().unwrap_or_default().to_string_lossy().to_string(),size:plan.source.size as f64,file_modified:Some(crate::listing::system_time_iso(plan.source.modified))});}}
            }Err(error)=>{let rollback=if copy{remove_created(self.fs.as_ref(),&copied_transfers.iter().map(|x|x.to.clone()).collect::<Vec<_>>())}else{rollback_moves(self.fs.as_ref(),&applied)};return Err(combine_error(error,rollback));}}
        }

        let count=source_ids.len();let noun=slovak_files(count);let past=if copy{if count==1{"Skopírovaný"}else if count<=4{"Skopírované"}else{"Skopírovaných"}}else if count==1{"Presunutý"}else if count<=4{"Presunuté"}else{"Presunutých"};let destination=folder.file_name().unwrap_or(folder.as_os_str()).to_string_lossy();let summary=format!("{past} {count} {noun} do {destination}");
        let recorded=self.writer.record_file_change(if copy{"copy_files"}else{"move_files"},&summary,&moved_catalog,&copied_catalog,&[]);
        let (change_id,remapped_ids)=match recorded{Ok(v)=>v,Err(error)=>{let rollback=if copy{remove_created(self.fs.as_ref(),&copied_transfers.iter().map(|x|x.to.clone()).collect::<Vec<_>>())}else{rollback_moves(self.fs.as_ref(),&applied)};return Err(combine_error(error,rollback));}};
        self.invalidate_plans(&selected,&folder);let mut affected_folder_paths=vec![folder.to_string_lossy().to_string()];for plan in &selected{if let Some(parent)=plan.source.path.parent(){let path=parent.to_string_lossy().to_string();if !affected_folder_paths.iter().any(|item|path_eq(Path::new(item),Path::new(&path))){affected_folder_paths.push(path);}}}Ok(FileOperationResult{change_id,summary,changed_count:source_ids.len(),skipped_count:skipped,remapped_ids,affected_ids:source_ids,affected_folder_paths})
    }

    pub fn rename_file<F>(&self,id:i64,new_base_name:&str,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){
        let _operation = self.lock_operation()?;
        self.writable()?;let base=validate_windows_name(new_base_name)?;let mut sources=self.resolve_sources(&[id])?;let source=sources.pop().ok_or("Súbor sa nenašiel")?;let parent=source.path.parent().ok_or("Priečinok súboru sa nenašiel")?;let ext=source.path.extension().map(|x|x.to_string_lossy().to_string());let file_name=ext.as_ref().map(|e|format!("{base}.{e}")).unwrap_or_else(||base.clone());let target=parent.join(file_name);let mut pairs=sidecar_pairs(&source.path,&target)?;
        if pairs.iter().any(|p|self.fs.exists(&p.to)&&!path_eq(&p.from,&p.to)){return Err(format!("V priečinku už existuje súbor: {}",target.display()));}if pairs.iter().all(|p|p.from==p.to){return Ok(self.no_change_result(0,"Premenovanie"));}
        progress(1,1);let applied=move_transfers(self.fs.as_ref(),&pairs)?;pairs.clear();let name=target.file_name().unwrap().to_string_lossy().to_string();let catalog_move=if source.id>0{vec![FileCatalogMove{asset_id:source.id,target_path:target.to_string_lossy().to_string(),target_folder_path:parent.to_string_lossy().to_string(),name:name.clone(),ext:target.extension().unwrap_or_default().to_string_lossy().to_string(),size:source.size as f64,file_modified:Some(crate::listing::system_time_iso(source.modified))}]}else{Vec::new()};let summary=format!("Premenované {} → {name}",source.path.file_name().unwrap_or_default().to_string_lossy());
        let (change_id,remapped_ids)=match self.writer.record_file_change("rename_file",&summary,&catalog_move,&[],&[]){Ok(v)=>v,Err(error)=>return Err(combine_error(error,rollback_moves(self.fs.as_ref(),&applied)))};
        self.listing.invalidate_path(&parent.to_string_lossy()).ok();Ok(FileOperationResult{change_id,summary,changed_count:1,skipped_count:0,remapped_ids,affected_ids:vec![id],affected_folder_paths:vec![parent.to_string_lossy().to_string()]})
    }

    pub fn reveal_in_explorer(&self,path:&str)->Result<(),String>{let target=PathBuf::from(path);if !self.fs.exists(&target){return Err("Priečinok neexistuje".into())}#[cfg(windows)]{use std::{os::windows::process::CommandExt,process::Command};Command::new("explorer.exe").raw_arg(explorer_select_arg(&target)).spawn().map_err(|e|e.to_string())?;}Ok(())}
    pub fn rename_folder(&self,path:&str,new_name:&str)->Result<FileOperationResult,String>{let _operation=self.lock_operation()?;self.writable()?;if is_volume_root(path){return Err("Diskovú koreňovú zložku nemožno premenovať".into())}let source=self.validate_folder(path)?;if source.parent().is_none()||source.parent()==Some(source.as_path()){return Err("Diskovú koreňovú zložku nemožno premenovať".into())}let name=validate_folder_name(new_name)?;if name=="."||name==".."{return Err("Neplatný názov priečinka".into())}let target=source.parent().unwrap().join(name);let case_only=path_eq(&source,&target)&&source!=target;if self.fs.exists(&target)&&!case_only{return Err(format!("Cieľ už existuje: {}",target.display()))}if source==target{return Ok(self.no_change_result(0,"Premenovanie priečinka"))}move_case_safe(self.fs.as_ref(),&source,&target).map_err(|e|e.to_string())?;let summary=format!("Premenovaný priečinok: {}",target.display());match self.writer.rewrite_folder_paths(&source.to_string_lossy(),&target.to_string_lossy(),"rename_folder",&summary){Ok(change_id)=>{if let Some(parent)=source.parent(){let _=self.listing.invalidate_path(&parent.to_string_lossy());}Ok(FileOperationResult{change_id,summary,changed_count:1,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:vec![source.to_string_lossy().to_string(),target.to_string_lossy().to_string()]})},Err(error)=>{let rollback=move_case_safe(self.fs.as_ref(),&target,&source).map_err(|e|e.to_string());Err(combine_error(error,rollback))}}
    }
    pub fn delete_folder_info(&self,path:&str)->Result<DeleteFolderInfo,String>{if is_volume_root(path){return Err("Diskovú koreňovú zložku nemožno odstrániť".into())}let root=self.validate_folder(path)?;let mut files=0usize;let mut folders=0usize;let mut bytes=0u64;fn walk(fs:&dyn FileSystemOps,p:&Path,files:&mut usize,folders:&mut usize,bytes:&mut u64)->Result<(),String>{*folders+=1;for item in fs.read_dir(p).map_err(|e|e.to_string())?{if fs.is_symlink(&item){return Err(format!("Odkaz mimo priečinok nemožno bezpečne spracovať: {}",item.display()))}else if fs.is_dir(&item){walk(fs,&item,files,folders,bytes)?}else{*files+=1;*bytes=bytes.saturating_add(fs.metadata(&item).map_err(|e|e.to_string())?.len())}}Ok(())}walk(self.fs.as_ref(),&root,&mut files,&mut folders,&mut bytes)?;let catalogued={let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;let norm=crate::journal::normalize_fs_path(&root.to_string_lossy());let prefix=format!("{norm}\\");let mut stmt=conn.prepare("SELECT path FROM assets WHERE path IS NOT NULL").map_err(|e|e.to_string())?;let paths=stmt.query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;paths.iter().filter(|p|{let p=crate::journal::normalize_fs_path(p);p==norm||p.starts_with(&prefix)}).count()};Ok(DeleteFolderInfo{files,folders,catalogued,bytes,recycle_available:self.fs.has_recycle_bin(&root)})}
    pub fn delete_folder(&self,path:&str)->Result<FileOperationResult,String>{let _operation=self.lock_operation()?;self.writable()?;if is_volume_root(path){return Err("Diskovú koreňovú zložku nemožno odstrániť".into())}let root=self.validate_folder(path)?;if root.parent().is_none()||root.parent()==Some(root.as_path()){return Err("Diskovú koreňovú zložku nemožno odstrániť".into())}if !self.fs.has_recycle_bin(&root){return Err("Na tomto disku nie je dostupný Kôš; priečinok nemožno bezpečne odstrániť.".into())}let info=self.delete_folder_info(path)?;self.fs.recycle_delete(&[root.clone()])?;let summary=format!("Priečinok presunutý do Koša: {}",root.file_name().unwrap_or_default().to_string_lossy());let change_id=match self.writer.record_folder_delete(&root.to_string_lossy(),&summary){Ok(id)=>id,Err(e)=>{let rollback=self.fs.restore_recycled(&root);return Err(combine_error(e,rollback))}};if let Some(parent)=root.parent(){let _=self.listing.invalidate_path(&parent.to_string_lossy());}Ok(FileOperationResult{change_id,summary,changed_count:info.files,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:vec![root.to_string_lossy().to_string()]})}
    pub fn move_folder(&self,path:&str,target:&str)->Result<FileOperationResult,String>{self.transfer_folder(path,target,false,&mut|_,_|{})}
    pub fn move_folder_with_progress<F>(&self,path:&str,target:&str,mut progress:F)->Result<FileOperationResult,String>where F:FnMut(usize,usize){self.transfer_folder(path,target,false,&mut progress)}
    pub fn copy_folder(&self,path:&str,target:&str)->Result<FileOperationResult,String>{self.transfer_folder(path,target,true,&mut|_,_|{})}
    pub fn copy_folder_with_progress<F>(&self,path:&str,target:&str,mut progress:F)->Result<FileOperationResult,String>where F:FnMut(usize,usize){self.transfer_folder(path,target,true,&mut progress)}
    fn transfer_folder<F>(&self, path: &str, target: &str, copy: bool, progress: &mut F) -> Result<FileOperationResult, String>
    where F: FnMut(usize, usize) {
        let _operation = self.lock_operation()?;
        self.writable()?;
        if is_volume_root(path) { return Err("Diskovú koreňovú zložku nemožno presúvať".into()); }
        let source = self.validate_folder(path)?;
        let parent = self.validate_folder(target)?;
        if source.parent().is_none() || source.parent() == Some(source.as_path()) {
            return Err("Diskovú koreňovú zložku nemožno presúvať".into());
        }
        let source_key = key(&source);
        let target_key = key(&parent);
        if target_key == source_key || target_key.starts_with(&(source_key + "\\")) {
            return Err("Priečinok nemožno vložiť do seba ani do jeho podpriečinka".into());
        }
        let dest = parent.join(source.file_name().ok_or("Názov priečinka chýba")?);
        if self.fs.exists(&dest) {
            return Err(format!("V cieli už existuje priečinok „{}“", dest.file_name().unwrap().to_string_lossy()));
        }
        let cross_volume = !self.fs.same_volume(&source, &dest);
        let count = count_folder_files(self.fs.as_ref(), &source)?;
        let remove_copy = || remove_tree(self.fs.as_ref(), &dest)
            .map_err(|error| format!("Nepodarilo sa odstrániť kópiu {}: {error}", dest.display()));
        let restore_source = || {
            self.fs.restore_recycled(&source)
                .map_err(|error| format!("Nepodarilo sa obnoviť priečinok {}: {error}", source.display()))?;
            if !self.fs.is_dir(&source) {
                return Err(format!("Obnovený priečinok nie je dostupný: {}", source.display()));
            }
            Ok(())
        };
        if copy || cross_volume {
            self.fs.create_dir(&dest).map_err(|e| e.to_string())?;
            let mut copied = 0;
            if let Err(error) = copy_folder_contents(self.fs.as_ref(), &source, &dest, &mut copied, progress, count) {
                return Err(combine_error(error, remove_copy()));
            }
        } else {
            progress(1, 1);
            move_case_safe(self.fs.as_ref(), &source, &dest).map_err(|e| e.to_string())?;
        }
        if !copy && cross_volume {
            if let Err(error) = self.fs.recycle_delete(std::slice::from_ref(&source)) {
                let rollback = if self.fs.is_dir(&source) { remove_copy() } else { restore_source().and_then(|()| remove_copy()) };
                return Err(combine_error(error, rollback));
            }
        }
        let summary = if copy {
            format!("Skopírované: {count} súborov (do katalógu sa pridajú pri otvorení priečinka)")
        } else {
            format!("Presunutý priečinok: {}", dest.display())
        };
        let recorded = if copy {
            self.writer.record_file_change("copy_folder", &summary, &[], &[], &[]).map(|result| result.0)
        } else {
            self.writer.rewrite_folder_paths(&source.to_string_lossy(), &dest.to_string_lossy(), "move_folder", &summary)
        };
        let change_id = recorded.map_err(|error| {
            let rollback = if copy {
                remove_copy()
            } else if cross_volume {
                restore_source().and_then(|()| remove_copy())
            } else {
                move_case_safe(self.fs.as_ref(), &dest, &source)
                    .map_err(|error| format!("Nepodarilo sa obnoviť priečinok {}: {error}", source.display()))
            };
            combine_error(error, rollback)
        })?;
        let mut affected = vec![source.to_string_lossy().into_owned(), dest.to_string_lossy().into_owned(), parent.to_string_lossy().into_owned()];
        if let Some(old_parent) = source.parent() { affected.push(old_parent.to_string_lossy().into_owned()); }
        for path in &affected { let _ = self.listing.invalidate_path(path); }
        Ok(FileOperationResult { change_id, summary, changed_count: count, skipped_count: 0, remapped_ids: Vec::new(), affected_ids: Vec::new(), affected_folder_paths: affected })
    }
    pub fn create_folder(&self,parent:&str,name:&str)->Result<FileOperationResult,String>{let _operation=self.lock_operation()?;self.writable()?;let folder=self.validate_folder(parent)?;let name=validate_windows_name(name)?;let target=folder.join(name);if self.fs.exists(&target){return Err(format!("Priečinok už existuje: {}",target.display()))}self.fs.create_dir(&target).map_err(|e|format!("Nepodarilo sa vytvoriť priečinok {}: {e}",target.display()))?;let summary=format!("Vytvorený priečinok {}",target.file_name().unwrap_or_default().to_string_lossy());match self.writer.record_file_change("create_folder",&summary,&[],&[],&[target.to_string_lossy().to_string()]){Ok((change_id,_))=>{self.listing.invalidate_path(parent).ok();Ok(FileOperationResult{change_id,summary,changed_count:1,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:vec![parent.to_string()]})},Err(error)=>{let rollback=self.fs.remove_dir(&target).map_err(|e|format!("Nepodarilo sa vrátiť priečinok {}: {e}",target.display()));Err(combine_error(error,rollback))}}
    }

    /// Moves the selected files and their sidecars to the Recycle Bin and removes their catalogue rows,
    /// in a single batch. Directories are skipped.
    fn delete_sources(&self,ids:&[i64])->Result<Vec<Source>,String>{
        let files_only=ids.iter().copied().filter(|id|!(*id<0&&self.listing.asset(*id).is_some_and(|a|a.is_directory))).collect::<Vec<_>>();
        if files_only.is_empty(){return Err("Priečinky sa týmto nemažú".into());}
        let sources=self.resolve_sources(&files_only)?;
        if sources.is_empty(){return Err("Nie sú vybrané žiadne súbory".into());}
        Ok(sources)
    }
    fn files_of(&self,source:&Source)->Result<Vec<PathBuf>,String>{Ok(sidecar_pairs(&source.path,&source.path)?.into_iter().map(|p|p.from).collect())}

    /// What a delete would do, for the confirmation dialog: how many files go permanently (no Recycle
    /// Bin on their drive, or `permanent` requested with Shift+Delete).
    pub fn delete_info(&self,ids:&[i64],permanent:bool)->Result<DeleteInfo,String>{
        let sources=self.delete_sources(ids)?;let mut sidecars=0usize;let mut permanent_count=0usize;let mut drives=Vec::<String>::new();
        for source in &sources{sidecars+=self.files_of(source)?.len().saturating_sub(1);let bin=self.fs.has_recycle_bin(&source.path);if permanent||!bin{permanent_count+=1;}if !bin{let drive=volume_key(&source.path).to_uppercase();if !drives.contains(&drive){drives.push(drive);}}}
        Ok(DeleteInfo{count:sources.len(),names:sources.iter().take(5).map(|s|s.name.clone()).collect(),sidecars,permanent_count,no_bin_drives:drives})
    }

    /// Adds physically purged catalogue ids to the output even when the catalogue write fails.
    pub fn delete_files(&self, ids: &[i64], permanent: bool, confirmed_no_bin_drives: &[String], purged_ids: &mut Vec<i64>) -> Result<FileOperationResult, String> {
        let _operation = self.lock_operation()?;
        self.writable()?;
        let sources = self.delete_sources(ids)?;
        let (to_bin, to_purge): (Vec<_>, Vec<_>) = sources.iter().partition(|s| !permanent && self.fs.has_recycle_bin(&s.path));
        if !permanent && to_purge.iter().any(|source| {
            let drive = volume_key(&source.path).to_uppercase();
            !confirmed_no_bin_drives.contains(&drive)
        }) {
            return Err("Na disku nie je dostupný Kôš. Trvalé zmazanie vyžaduje potvrdenie.".into());
        }
        let to_purge = to_purge.into_iter().map(|source| Ok((source, self.files_of(source)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut summary_parts = Vec::new();
        let mut change_id = None;
        let mut affected_ids = Vec::new();
        let mut folders = Vec::new();
        let mut invalidate_parent = |path: &Path| {
            if let Some(parent) = path.parent() {
                let parent = parent.to_string_lossy().into_owned();
                if !folders.contains(&parent) {
                    let _ = self.listing.invalidate_path(&parent);
                    folders.push(parent);
                }
            }
        };
        if !to_bin.is_empty() {
            let mut files = Vec::new();
            for source in &to_bin {
                for file in self.files_of(source)? {
                    if !files.iter().any(|f: &PathBuf| path_eq(f, &file)) { files.push(file); }
                }
            }
            self.fs.recycle_delete(&files)?;
            for source in &to_bin { invalidate_parent(&source.path); }

            let count = to_bin.len();
            let summary = format!("Odstránené do Koša: {count} {}", slovak_files(count));
            let catalog_ids = to_bin.iter().filter(|s| s.id > 0).map(|s| s.id).collect::<Vec<_>>();
            change_id = self.writer.record_file_delete(&catalog_ids, &summary)
                .map_err(|error| combine_error(error, self.restore_recycled_files(&files, false)))?;
            affected_ids.extend(to_bin.iter().map(|s| s.requested_id));
            summary_parts.push(summary);
        }
        if !to_purge.is_empty() {
            let mut failed = Vec::new();
            let mut deleted_files = Vec::new();
            let mut deleted_paths = HashSet::new();
            let mut done = 0;
            let first_purged = purged_ids.len();
            for (source, files) in to_purge {
                for file in files {
                    if deleted_paths.contains(&key(&file)) { continue; }
                    match self.fs.remove_file(&file) {
                        Ok(()) => {
                            invalidate_parent(&file);
                            deleted_paths.insert(key(&file));
                            deleted_files.push(file.display().to_string());
                        }
                        Err(error) => failed.push(format!("{}: {error}", file.display())),
                    }
                }
                if deleted_paths.contains(&key(&source.path)) {
                    done += 1;
                    affected_ids.push(source.requested_id);
                    if source.id > 0 { purged_ids.push(source.id); }
                }
            }
            let summary = format!("Zmazané natrvalo: {done} {}", slovak_files(done));
            if !deleted_files.is_empty() {
                change_id = self.writer.record_named_file_delete(&purged_ids[first_purged..], "purge_files", &summary)
                    .map_err(|error| format!("Súbory už boli natrvalo zmazané z disku, zápis do katalógu zlyhal: {error} · {}", deleted_files.join(", ")))?
                    .or(change_id);
            }
            summary_parts.push(summary);
            if !failed.is_empty() { summary_parts.push(format!("Nepodarilo sa zmazať: {}", failed.join("; "))); }
        }
        Ok(FileOperationResult { change_id, summary: summary_parts.join(" · "), changed_count: affected_ids.len(),
            skipped_count: sources.len() - affected_ids.len(), remapped_ids: Vec::new(), affected_ids, affected_folder_paths: folders })
    }

    fn validate_folder(&self,path:&str)->Result<PathBuf,String>{let candidate=PathBuf::from(path);if !candidate.is_absolute()||!self.fs.is_dir(&candidate){return Err(format!("Cieľový priečinok nie je dostupný: {path}"))}Ok(candidate)}
    fn resolve_sources(&self,ids:&[i64])->Result<Vec<Source>,String>{let mut unique=HashSet::new();let mut out=Vec::new();for id in ids.iter().copied(){if !unique.insert(id){continue;}let requested_id=id;let (id,path,listed_name)=if id<0{let asset=self.listing.asset(id).ok_or_else(||format!("Súbor {id} už nie je v aktuálnom zozname"))?;let path=asset.path.ok_or("Zoznam nemá cestu k súboru")?;let catalog_id:Option<i64>={let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;conn.query_row("SELECT id FROM assets WHERE is_listed=1 AND lower(path)=lower(?1) ORDER BY id LIMIT 1",[&path],|r|r.get(0)).optional().map_err(|e|e.to_string())?};(catalog_id.unwrap_or(id),PathBuf::from(path),asset.name)}else{let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;let row:Option<(String,String)>=conn.query_row("SELECT path,name FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>''",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;let (path,name)=row.ok_or_else(||format!("Súbor katalógu {id} sa nenašiel"))?;(id,PathBuf::from(path),name)};let metadata=self.fs.metadata(&path).map_err(|_|format!("Súbor nie je dostupný: {}",path.display()))?;if !metadata.is_file(){return Err(format!("Vybraná položka nie je súbor: {}",path.display()))}let name=path.file_name().map(|x|x.to_string_lossy().to_string()).unwrap_or(listed_name);let modified=metadata.modified().map_err(|e|format!("Čas zmeny sa nedá prečítať: {} ({e})",path.display()))?;out.push(Source{id,requested_id,path,name,size:metadata.len(),modified});}Ok(out)}

    fn plan(&self,sources:&[Source],folder:&Path,rename_base:Option<&str>)->Result<Vec<Plan>,String>{let mut plans=Vec::new();let mut reserved=HashSet::new();for source in sources{let main=if let Some(base)=rename_base{let ext=source.path.extension().map(|e|e.to_string_lossy().to_string());folder.join(ext.map(|e|format!("{base}.{e}")).unwrap_or_else(||base.to_string()))}else{folder.join(&source.name)};let pairs=sidecar_pairs(&source.path,&main)?;let conflict=pairs.iter().any(|p|self.fs.exists(&p.to)||reserved.contains(&key(&p.to)));for pair in &pairs{reserved.insert(key(&pair.to));}plans.push(Plan{source:source.clone(),target:main,pairs,conflict});}Ok(plans)}

    fn number_conflict(&self, plan: Plan, folder: &Path, reserved: &HashSet<String>) -> Result<Plan, String> {
        self.number_conflict_with(plan, folder, |path| reserved.contains(&key(path)))
    }

    fn number_conflict_with(&self, plan: Plan, folder: &Path, reserved: impl Fn(&Path) -> bool) -> Result<Plan, String> {
        if !plan.conflict { return Ok(plan); }
        let original = plan.target.file_stem().unwrap_or_default().to_string_lossy();
        let ext = plan.target.extension().map(|e| e.to_string_lossy());
        for n in 2..100_000u32 {
            let base = format!("{original} ({n})");
            let target = folder.join(ext.as_ref().map(|e| format!("{base}.{e}")).unwrap_or(base));
            let pairs = sidecar_pairs(&plan.source.path, &target)?;
            if pairs.iter().all(|pair| !self.fs.exists(&pair.to) && !reserved(&pair.to)) {
                return Ok(Plan { source: plan.source, target, pairs, conflict: false });
            }
        }
        Err("Nenašlo sa voľné číslo pre názov súboru".into())
    }

    fn invalidate_plans(&self,plans:&[Plan],folder:&Path){let _=self.listing.invalidate_path(&folder.to_string_lossy());for plan in plans{if let Some(parent)=plan.source.path.parent(){let _=self.listing.invalidate_path(&parent.to_string_lossy());}}}
    fn no_change_result(&self,skipped:usize,operation:&str)->FileOperationResult{FileOperationResult{change_id:None,summary:if skipped>0{format!("{operation}: preskočené {skipped} {}",slovak_files(skipped))}else{format!("{operation}: bez zmien")},changed_count:0,skipped_count:skipped,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:Vec::new()}}
}

fn count_folder_files(fs:&dyn FileSystemOps,path:&Path)->Result<usize,String>{let mut count=0;for item in fs.read_dir(path).map_err(|e|e.to_string())?{if fs.is_symlink(&item){return Err(format!("Odkaz mimo priečinok nemožno bezpečne spracovať: {}",item.display()))}else if fs.is_dir(&item){count+=count_folder_files(fs,&item)?}else{count+=1}}Ok(count)}
fn copy_folder_contents(fs: &dyn FileSystemOps, source: &Path, target: &Path, copied: &mut usize, progress: &mut dyn FnMut(usize, usize), total: usize) -> Result<(), String> {
    for item in fs.read_dir(source).map_err(|e| e.to_string())? {
        let dest = target.join(item.file_name().ok_or("Neplatná cesta")?);
        if fs.is_symlink(&item) {
            return Err(format!("Odkaz mimo priečinok nemožno kopírovať: {}", item.display()));
        }
        if fs.is_dir(&item) {
            fs.create_dir(&dest).map_err(|e| e.to_string())?;
            copy_folder_contents(fs, &item, &dest, copied, progress, total)?;
        } else {
            let modified = fs.metadata(&item).and_then(|metadata| metadata.modified()).map_err(|e| e.to_string())?;
            fs.copy_no_replace(&item, &dest).map_err(|e| e.to_string())?;
            fs.set_modified(&dest, modified).map_err(|e| e.to_string())?;
            *copied += 1;
            progress(*copied, total);
        }
    }
    Ok(())
}
fn remove_tree(fs:&dyn FileSystemOps,path:&Path)->Result<(),String>{for item in fs.read_dir(path).map_err(|e|e.to_string())?{if fs.is_dir(&item){remove_tree(fs,&item)?}else{fs.remove_file(&item).map_err(|e|e.to_string())?}}fs.remove_dir(path).map_err(|e|e.to_string())}
pub(crate) fn sidecar_candidates(source: &Path) -> Vec<PathBuf> {
    if source.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xmp")) { return Vec::new(); }
    let Some(name) = source.file_name() else { return Vec::new(); };
    let mut full_name = name.to_os_string();
    full_name.push(".xmp");
    vec![source.with_extension("xmp"), source.with_file_name(full_name)]
}

fn sidecar_pairs(source:&Path,target:&Path)->Result<Vec<Transfer>,String>{let mut pairs=vec![Transfer{from:source.to_path_buf(),to:target.to_path_buf()}];let Some(filename)=source.file_name().map(|x|x.to_string_lossy().to_string())else{return Err("Názov súboru sa nedá prečítať".into())};let candidates=sidecar_candidates(source);let mut seen=HashSet::new();for candidate in candidates{if candidate.exists()&&seen.insert(key(&candidate)){let mapped=if candidate.file_name().is_some_and(|n|n.to_string_lossy().eq_ignore_ascii_case(&format!("{filename}.xmp"))){target.with_file_name(format!("{}.xmp",target.file_name().unwrap().to_string_lossy()))}else{target.with_extension("xmp")};pairs.push(Transfer{from:candidate,to:mapped});}}Ok(pairs)}

fn copy_transfers(fs:&dyn FileSystemOps,pairs:&[Transfer])->Result<Vec<Transfer>,String>{let mut done=Vec::<Transfer>::new();for pair in pairs{if let Err(error)=fs.copy_no_replace(&pair.from,&pair.to){let rollback=remove_created(fs,&done.iter().map(|x|x.to.clone()).collect::<Vec<_>>());return Err(combine_error(format!("{}: {error}",pair.from.display()),rollback));}done.push(pair.clone());}Ok(done)}
fn move_transfers(fs:&dyn FileSystemOps,pairs:&[Transfer])->Result<Vec<Transfer>,String>{let mut done=Vec::<Transfer>::new();for pair in pairs{let result=if fs.same_volume(&pair.from,&pair.to){move_case_safe(fs,&pair.from,&pair.to)}else{copy_then_remove(fs,&pair.from,&pair.to)};if let Err(error)=result{let rollback=rollback_moves(fs,&done);return Err(combine_error(format!("{}: {error}",pair.from.display()),rollback));}done.push(pair.clone());}Ok(done)}
fn move_case_safe(fs:&dyn FileSystemOps,source:&Path,target:&Path)->io::Result<()>{if path_eq(source,target)&&source!=target{let temp=temporary_sibling(source);fs.move_no_replace(source,&temp)?;if let Err(error)=fs.move_no_replace(&temp,target){let _=fs.move_no_replace(&temp,source);return Err(error)}Ok(())}else{fs.move_no_replace(source,target)}}
fn temporary_sibling(path:&Path)->PathBuf{static NEXT:AtomicUsize=AtomicUsize::new(0);let n=NEXT.fetch_add(1,Ordering::Relaxed);path.with_file_name(format!(".ddreams-rename-{}-{n}",std::process::id()))}
fn copy_then_remove(fs:&dyn FileSystemOps,source:&Path,target:&Path)->io::Result<()>{let before=fs.metadata(source)?;let before_modified=before.modified()?;fs.copy_no_replace(source,target)?;let copied=fs.metadata(target)?;let copied_modified=copied.modified()?;let source_after=fs.metadata(source)?;let after_modified=source_after.modified()?;let time_difference=if before_modified>=copied_modified{before_modified.duration_since(copied_modified).unwrap_or_else(|error|error.duration())}else{copied_modified.duration_since(before_modified).unwrap_or_else(|error|error.duration())};if before.len()!=copied.len()||before.len()!=source_after.len()||before_modified!=after_modified||time_difference>std::time::Duration::from_secs(2){let _=fs.remove_file(target);return Err(io::Error::other("copy verification failed"));}if let Err(error)=fs.remove_file(source){let _=fs.remove_file(target);return Err(error)}Ok(())}
fn rollback_moves(fs:&dyn FileSystemOps,done:&[Transfer])->Result<(),String>{let reverse=done.iter().rev().map(|x|Transfer{from:x.to.clone(),to:x.from.clone()}).collect::<Vec<_>>();let mut restored=Vec::<Transfer>::new();for pair in &reverse{let result=if fs.same_volume(&pair.from,&pair.to){move_case_safe(fs,&pair.from,&pair.to)}else{copy_then_remove(fs,&pair.from,&pair.to)};if let Err(error)=result{let redo=restored.iter().rev().map(|x|Transfer{from:x.to.clone(),to:x.from.clone()}).collect::<Vec<_>>();let _=move_transfers(fs,&redo);return Err(format!("Rollback failed for {}: {error}",pair.from.display()))}restored.push(pair.clone());}Ok(())}
fn remove_created(fs:&dyn FileSystemOps,paths:&[PathBuf])->Result<(),String>{for path in paths.iter().rev(){if fs.exists(path){fs.remove_file(path).map_err(|e|format!("Nepodarilo sa odstrániť čiastočnú kópiu {}: {e}",path.display()))?;}}Ok(())}
#[cfg(windows)]
pub(crate) fn explorer_select_arg(path: &Path) -> String {
    let escaped = path.to_string_lossy().replace('"', "\"\"");
    format!("/select,\"{escaped}\"")
}

#[cfg(all(test, windows))]
#[test]
fn explorer_select_quotes_path_with_spaces_and_comma() {
    assert_eq!(explorer_select_arg(Path::new(r"F:\Moje fotky, 2025")), r#"/select,"F:\Moje fotky, 2025""#);
}

fn is_volume_root(path:&str)->bool{let value=path.replace("/","\\");(value.len()==2&&value.as_bytes().get(1)==Some(&b':'))||(value.len()==3&&value.as_bytes().get(1)==Some(&b':')&&value.ends_with("\\"))||(value.starts_with("\\\\")&&value.trim_matches('\\').split('\\').filter(|p|!p.is_empty()).count()==2)}
fn path_eq(a:&Path,b:&Path)->bool{key(a)==key(b)}
fn key(path:&Path)->String{path.to_string_lossy().replace('/',"\\").trim_end_matches('\\').to_lowercase()}
fn combine_error(error:impl ToString,rollback:Result<(),String>)->String{match rollback{Ok(())=>error.to_string(),Err(rollback)=>format!("{}; {}",error.to_string(),rollback)}}
fn slovak_files(n:usize)->&'static str{if n==1{"súbor"}else if (2..=4).contains(&n){"súbory"}else{"súborov"}}

fn validate_folder_name(name:&str)->Result<String,String>{if name.trim().is_empty(){return Err("Názov nesmie byť prázdny".into())}if name.ends_with('.')||name.ends_with(' '){return Err("Názov sa nesmie končiť bodkou ani medzerou".into())}if name.chars().any(|c|matches!(c,'\\'|'/'|':'|'*'|'?'|'"'|'<'|'>'|'|')){return Err("Názov obsahuje znak, ktorý Windows nepovoľuje".into())}let _=validate_windows_name(name)?;Ok(name.to_owned())}

pub fn validate_windows_name(name:&str)->Result<String,String>{let value=name.trim();if value.is_empty(){return Err("Názov nesmie byť prázdny".into())}if value.len()>255{return Err("Názov môže mať najviac 255 znakov".into())}if value.ends_with('.')||value.ends_with(' '){return Err("Názov sa nesmie končiť bodkou ani medzerou".into())}if value.chars().any(|c|matches!(c,'\\'|'/'|':'|'*'|'?'|'"'|'<'|'>'|'|')){return Err("Názov obsahuje znak, ktorý Windows nepovoľuje".into())}let stem=value.split('.').next().unwrap_or("").trim_end_matches([' ','.']).to_ascii_uppercase();if matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL")||(1..=9).any(|i|stem==format!("COM{i}")||stem==format!("LPT{i}")){return Err("Tento názov je vyhradený systémom Windows".into())}Ok(value.to_owned())}

#[cfg(test)]
mod tests {
    #[test]
    fn move_warning_counts_only_uncategorized_photos_that_would_move() {
        let root = temp();
        let (_disk, manager) = set_up(&root, None);
        seed_asset_path(&manager.db, &root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let source = root.join("src");
        let destination = root.join("dst");
        let count = |target: &Path, policy| {
            manager.uncategorized_photos_for_move(&[1, 1], target.to_str().unwrap(), policy, &formats).unwrap()
        };

        assert_eq!(count(&destination, ConflictPolicy::Cancel), 0);
        manager.db.lock().unwrap().execute("DELETE FROM asset_categories WHERE asset_id=1", []).unwrap();
        assert_eq!(count(&destination, ConflictPolicy::Cancel), 1);
        assert_eq!(count(&source, ConflictPolicy::Cancel), 0);
        fs::write(destination.join("a.jpg"), b"conflict").unwrap();
        assert_eq!(count(&destination, ConflictPolicy::Skip), 0);
        assert_eq!(count(&destination, ConflictPolicy::KeepBoth), 1);
        fs::remove_dir_all(root).unwrap();
    }

    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    #[test] fn rename_case_only_keeps_main_and_sidecar(){let root=temp();let(disk,base_manager)=set_up(&root,None);let db=base_manager.db.clone();seed_asset_path(&db,&root);fs::write(root.join("src/a.xmp"),b"side").unwrap();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),base_manager.writer.clone(),false,disk);let result=manager.rename_file(1,"A",|_,_|{}).unwrap();assert!(root.join("src/A.jpg").exists());assert!(root.join("src/A.xmp").exists());assert!(result.summary.contains("a.jpg"));let _=fs::remove_dir_all(root);}
    #[test] fn moving_or_copying_into_same_folder_is_a_noop(){let root=temp();let(disk,base_manager)=set_up(&root,None);let db=base_manager.db.clone();seed_asset_path(&db,&root);let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),base_manager.writer.clone(),false,disk);let source=root.join("src/a.jpg");for copy in [false,true]{let result=manager.run_copy_move(&[1],root.join("src").to_str().unwrap(),ConflictPolicy::Cancel,copy,&mut|_,_|{}).unwrap();assert_eq!(result.summary,"Súbory už sú v tomto priečinku");assert_eq!(result.changed_count,0);assert!(source.exists());}assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|row|row.get::<_,i64>(0)).unwrap(),0);let _=fs::remove_dir_all(root);}
    #[test] fn cross_volume_copy_accepts_two_second_mtime_rounding_but_checks_source_exactly(){let root=temp();let(disk,_manager)=set_up(&root,Some(false));disk.copied_mtime_offset.store(-2,Ordering::SeqCst);let source=root.join("src").join("a.jpg");let target=root.join("dst").join("a.jpg");copy_then_remove(disk.as_ref(),&source,&target).unwrap();assert!(!source.exists());assert_eq!(fs::read(&target).unwrap(),b"abc");fs::remove_file(&target).unwrap();fs::write(&source,b"abc").unwrap();disk.copied_mtime_offset.store(3,Ordering::SeqCst);assert!(copy_then_remove(disk.as_ref(),&source,&target).is_err());assert!(source.exists());assert!(!target.exists());let _=fs::remove_dir_all(root);}

    struct TestFs { failed_restores: std::sync::Mutex<HashSet<PathBuf>>, force_cross:Option<bool>, fail_move:AtomicUsize, move_calls:AtomicUsize, recycle:PathBuf, copied_mtime_offset:AtomicI64, recycled:std::sync::Mutex<std::collections::HashMap<PathBuf,PathBuf>> }
    impl TestFs {
        fn recycled_item(&self,original:&Path)->Option<PathBuf>{self.recycled.lock().unwrap().get(original).filter(|p|p.exists()).cloned()}
    }
    impl FileSystemOps for TestFs {
        fn exists(&self,p:&Path)->bool{p.exists()}fn is_dir(&self,p:&Path)->bool{p.is_dir()}fn is_symlink(&self,p:&Path)->bool{NativeFileSystem.is_symlink(p)}fn metadata(&self,p:&Path)->io::Result<fs::Metadata>{fs::metadata(p)}
        fn same_volume(&self,a:&Path,b:&Path)->bool{self.force_cross.unwrap_or_else(||volume_key(a)==volume_key(b))}
        fn move_no_replace(&self,a:&Path,b:&Path)->io::Result<()>{let n=self.move_calls.fetch_add(1,Ordering::SeqCst);if n==self.fail_move.load(Ordering::SeqCst){return Err(io::Error::other("simulated failure"))}NativeFileSystem.move_no_replace(a,b)}
        fn copy_no_replace(&self,a:&Path,b:&Path)->io::Result<()>{NativeFileSystem.copy_no_replace(a,b)?;let offset=self.copied_mtime_offset.load(Ordering::SeqCst);if offset!=i64::MIN{let modified=fs::metadata(a)?.modified()?;let rounded=if offset<0{modified.checked_sub(std::time::Duration::from_secs(offset.unsigned_abs())).unwrap()}else{modified.checked_add(std::time::Duration::from_secs(offset as u64)).unwrap()};File::options().write(true).open(b)?.set_times(FileTimes::new().set_modified(rounded))?;}Ok(())}fn remove_file(&self,p:&Path)->io::Result<()>{NativeFileSystem.remove_file(p)}fn create_dir(&self,p:&Path)->io::Result<()>{NativeFileSystem.create_dir(p)}fn remove_dir(&self,p:&Path)->io::Result<()>{NativeFileSystem.remove_dir(p)}fn read_dir(&self,p:&Path)->io::Result<Vec<PathBuf>>{NativeFileSystem.read_dir(p)}fn set_modified(&self,p:&Path,t:SystemTime)->io::Result<()>{NativeFileSystem.set_modified(p,t)}
        fn recycle_delete(&self,paths:&[PathBuf])->Result<(),String>{for path in paths{let to=self.recycle.join(format!("$R{}-{}",self.move_calls.fetch_add(1,Ordering::SeqCst),path.file_name().unwrap().to_string_lossy()));NativeFileSystem.move_no_replace(path,&to).map_err(|e|e.to_string())?;self.recycled.lock().unwrap().insert(path.clone(),to);}Ok(())}
        fn has_recycle_bin(&self,path:&Path)->bool{!path.to_string_lossy().contains("nobin")}
        fn restore_recycled(&self,original:&Path)->Result<(),String>{
            if self.failed_restores.lock().unwrap().contains(original) { return Err("simulated restore failure".into()); }
            let data=self.recycled_item(original).ok_or("not in bin")?;NativeFileSystem.move_no_replace(&data,original).map_err(|e|e.to_string())?;self.recycled.lock().unwrap().remove(original);Ok(())}
    }
    fn temp()->PathBuf{let root=std::env::temp_dir().join(format!("ddreams-fileops-{}-{}",std::process::id(),SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir_all(&root).unwrap();root}
    fn db()->Db{let c=Connection::open_in_memory().unwrap();c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT,action TEXT,summary TEXT,undone INTEGER DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER,table_name TEXT,row_key TEXT,column_name TEXT,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));INSERT INTO folder_roots VALUES(1,'R:','R:\\');INSERT INTO folders VALUES(1,NULL,1,'R:','R:\\');INSERT INTO folders VALUES(2,1,1,'src','R:\\src');INSERT INTO folders VALUES(3,1,1,'dst','R:\\dst');INSERT INTO assets VALUES(1,2,'a.jpg','jpg','jpg',1,3,10,20,24,'cap','notes','author',4,1,'Red','2020-01-01T00:00:00Z','2020-01-02',NULL,0,0,'R:\\src\\a.jpg');INSERT INTO categories VALUES(1,'Cat',NULL,1);INSERT INTO keywords VALUES(1,NULL,'Key');INSERT INTO asset_categories VALUES(1,1);INSERT INTO asset_keywords VALUES(1,1);").unwrap();Arc::new(Mutex::new(c))}
    fn manager(_root:&Path,readonly:bool,fs:Arc<dyn FileSystemOps>)->FileManager{let db=db();let formats=crate::formats::FormatTable::load_selected(None);let listing=ListingService::new(db.clone(),formats);let writer=Arc::new(CatalogWriter::new(db.clone(),!readonly));FileManager::with_fs(db,listing,writer,readonly,fs)}
    fn set_up(root:&Path,force_cross:Option<bool>)->(Arc<TestFs>,FileManager){let src=root.join("src");let dst=root.join("dst");fs::create_dir_all(&src).unwrap();fs::create_dir_all(&dst).unwrap();fs::write(src.join("a.jpg"),b"abc").unwrap();let recycle=root.join("recycle");fs::create_dir_all(&recycle).unwrap();let disk=Arc::new(TestFs{failed_restores:Default::default(),force_cross,fail_move:AtomicUsize::new(usize::MAX),move_calls:AtomicUsize::new(0),recycle,copied_mtime_offset:AtomicI64::new(i64::MIN),recycled:Default::default()});let manager=manager(root,false,disk.clone());(disk,manager)}
    fn seed_asset_path(db:&Db,root:&Path){let path=root.join("src/a.jpg");db.lock().unwrap().execute("UPDATE assets SET path=?1 WHERE id=1",[path.to_string_lossy().to_string()]).unwrap();}

    fn quarantine_fixture(root: &Path) -> (Arc<TestFs>, FileManager, crate::quarantine::Settings) {
        let (disk, manager) = set_up(root, None);
        seed_asset_path(&manager.db, root);
        crate::schema::migrate_catalog(&mut manager.db.lock().unwrap()).unwrap();
        let settings = crate::quarantine::Settings::fixture(&root.join("src"), &root.join("dst"));
        manager.sync_incoming(settings.incoming().to_str().unwrap()).unwrap();
        (disk, manager, settings)
    }

    #[test]
    fn quarantine_review_excludes_protected_old_undated_outside_and_nonphotos() {
        let root = temp();
        let (_, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let now = crate::quarantine::now();
        let aged = now - 61 * crate::quarantine::DAY;
        {
            let conn = manager.db.lock().unwrap();
            conn.execute_batch("DELETE FROM assets; DELETE FROM asset_categories;").unwrap();
            for (id, ext, date, rating, label, tagged, base, seen) in [
                (1, "jpg", Some("2024-06-01T12:30:00"), None, None, None, "src", aged),
                (2, "rw2", Some("2024-06-02"), Some(0), Some(""), Some(0), "src", aged),
                (3, "jpg", Some("2024-07-01"), Some(4), None, None, "src", aged),
                (4, "jpg", Some("2024-07-01"), None, Some("Red"), None, "src", aged),
                (5, "jpg", Some("2024-07-01"), None, None, Some(1), "src", aged),
                (6, "jpg", Some("2024-07-01"), None, None, None, "src", aged),
                (7, "jpg", Some("2024-05-31"), None, None, None, "src", aged),
                (8, "jpg", None, None, None, None, "src", aged),
                (9, "jpg", Some("2024-07-01"), None, None, None, "src-other", aged),
                (10, "mp4", Some("2024-07-01"), None, None, None, "src", aged),
                (11, "jpg", Some("2024-07-01"), None, None, None, "src", now),
                (12, "jpg", Some("2024-02-31"), None, None, None, "src", aged),
            ] {
                let name = format!("{id}.{ext}");
                let folder = root.join(base);
                fs::create_dir_all(&folder).unwrap();
                let path = folder.join(&name);
                fs::write(&path, b"test").unwrap();
                conn.execute("INSERT INTO assets(id,name,ext,ext_lower,is_listed,path,exif_date,rating,label,tagged,incoming_seen_at)
                    VALUES(?1,?2,?3,?3,1,?4,?5,?6,?7,?8,?9)",
                    (id, name, ext, path.to_str().unwrap(), date, rating, label, tagged, seen)).unwrap();
            }
            conn.execute("INSERT INTO asset_categories VALUES(6,1)", []).unwrap();
        }
        let preview = manager.prepare_quarantine_review(settings.clone(), &formats, None).unwrap();
        assert_eq!(preview.moves.ids, vec![1, 2]);
        assert_eq!(preview.moves.bytes, 8);
        assert!(preview.deletes.ids.is_empty());
        assert!(root.join("src/1.jpg").exists());
        manager.db.lock().unwrap().execute("UPDATE assets SET rating=5 WHERE id=1", []).unwrap();
        let result = manager.execute_quarantine_review(&settings, &formats, None, crate::quarantine::ReviewSelection { moves: true, deletes: true, stale_edits: true }, &[], |_, _| {}).unwrap();
        assert_eq!((result.moved, result.failures.len()), (1, 1));
        assert!(root.join("src/1.jpg").exists());
        assert!(root.join("dst/2.rw2").exists());
        assert!(manager.execute_quarantine_review(&settings, &formats, None, crate::quarantine::ReviewSelection { moves: true, deletes: true, stale_edits: true }, &[], |_, _| {}).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_edit_batch_preserves_first_mark_and_lists_oldest_first() {
        let root = temp();
        let (_, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let path = settings.incoming().join("b.jpg");
        {
            let conn = manager.db.lock().unwrap();
            conn.execute("UPDATE assets SET edit_pending_since=123 WHERE id=1", []).unwrap();
            conn.execute("INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,path)
                VALUES(2,2,'b.jpg','jpg','jpg',1,?1)", [path.to_str().unwrap()]).unwrap();
        }
        let change = |value| manager.writer.set_asset_values(&[1, 2], &[],
            &serde_json::Map::from_iter([("editPending".into(), value)])).unwrap();
        let marked = change(serde_json::json!("toggle"));
        assert_eq!(marked.assets[0].edit_pending_since, Some(123));
        assert!(marked.assets[1].edit_pending_since.unwrap() > 123);
        assert!(change(serde_json::json!(true)).change_id.is_none());
        let filter = serde_json::from_value(serde_json::json!({
            "kind":"special","value":"edit_pending","sort":"name","descending":true,"offset":0,"limit":50
        })).unwrap();
        assert_eq!(crate::db::asset_ids_page(&manager.db, &filter, &formats, 0, 50).unwrap(), vec![1, 2]);
        let page = crate::db::assets_page(&manager.db, &filter, &formats).unwrap();
        assert_eq!(page.items.iter().map(|asset| asset.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(page.items[0].edit_pending_since, Some(123));
        let counts = crate::db::values_page(&manager.db, "special", 0, 50, &formats).unwrap();
        assert_eq!(counts.iter().find(|item| item.value == "edit_pending").unwrap().count, 2);
        assert!(change(serde_json::json!("toggle")).assets.iter().all(|asset| asset.edit_pending_since.is_none()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_edit_clears_only_when_listing_mtime_passes_the_grace_period() {
        let root = temp();
        let (disk, manager, settings) = quarantine_fixture(&root);
        let src = settings.incoming();
        let file = src.join("a.jpg");
        let since = 1_700_000_000_i64;
        {
            let conn = manager.db.lock().unwrap();
            conn.execute("UPDATE folders SET path=?1 WHERE id=2", [src.to_str().unwrap()]).unwrap();
            conn.execute("UPDATE assets SET edit_pending_since=?1 WHERE id=1", [since]).unwrap();
        }
        let filter = serde_json::from_value(serde_json::json!({
            "kind":"folder","id":2,"folderPath":src,"sort":"name","descending":false,"offset":0,"limit":50
        })).unwrap();
        for (seconds, expected) in [(60, 0), (61, 1)] {
            let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs((since + seconds) as u64);
            disk.set_modified(&file, modified).unwrap();
            manager.listing.invalidate_path(src.to_str().unwrap()).unwrap();
            let (_, rows) = manager.listing.listed_folder(&filter).unwrap();
            assert_eq!(manager.writer.fulfil_edits_from_listing(&rows).unwrap(), expected);
        }
        let pending: Option<i64> = manager.db.lock().unwrap().query_row("SELECT edit_pending_since FROM assets WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(pending, None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_edit_clears_for_a_newer_listed_sibling_in_the_same_folder() {
        let root = temp();
        let (_, manager, settings) = quarantine_fixture(&root);
        let src = settings.incoming();
        let dst = settings.quarantine();
        {
            let conn = manager.db.lock().unwrap();
            conn.execute("UPDATE folders SET path=?1 WHERE id=2", [src.to_str().unwrap()]).unwrap();
            conn.execute("UPDATE folders SET path=?1 WHERE id=3", [dst.to_str().unwrap()]).unwrap();
            conn.execute("UPDATE assets SET edit_pending_since=unixepoch('2024-01-02') WHERE id=1", []).unwrap();
        }
        let listed = |id, folder: &Path, name: &str, ext: &str, modified: &str| crate::journal::ListedAsset {
            temporary_id: id, name: name.into(), ext: ext.into(), size: Some(4.0),
            file_modified: Some(modified.into()), path: folder.join(name).to_string_lossy().into_owned(),
            folder_path: folder.to_string_lossy().into_owned(),
        };
        let pending = || manager.db.lock().unwrap().query_row("SELECT edit_pending_since IS NOT NULL FROM assets WHERE id=1", [], |r| r.get::<_, bool>(0)).unwrap();
        let old = listed(-2, src, "A.tif", "tif", "2024-01-01T00:00:00Z");
        for sibling in [old.clone(), listed(-3, dst, "A.psd", "psd", "2024-01-03T00:00:00Z")] {
            manager.writer.catalog_listed_assets(&[sibling]).unwrap();
            assert!(pending());
        }
        let touched = crate::journal::ListedAsset { file_modified: Some("2024-01-03T00:00:00Z".into()), ..old };
        assert!(manager.writer.catalog_listed_assets(&[touched]).unwrap().is_empty());
        assert!(pending());
        manager.writer.catalog_listed_assets(&[listed(-4, src, "A.PSD", "psd", "2024-01-03T00:00:00Z")]).unwrap();
        assert!(!pending());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_edit_review_can_be_skipped_and_rechecks_scope_before_moving() {
        use crate::quarantine::{DAY, ReviewSelection};
        let root = temp();
        let (disk, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let source = settings.incoming().join("a.jpg");
        let since = crate::quarantine::now() - 91 * DAY;
        disk.set_modified(&source, std::time::UNIX_EPOCH + std::time::Duration::from_secs(since as u64)).unwrap();
        let mark = |timestamp| manager.db.lock().unwrap().execute("UPDATE assets SET edit_pending_since=?1 WHERE id=1", [timestamp]).unwrap();
        let preview = || manager.prepare_quarantine_review(settings.clone(), &formats, None).unwrap();
        let execute = |stale_edits| manager.execute_quarantine_review(&settings, &formats, None,
            ReviewSelection { moves: true, deletes: true, stale_edits }, &[], |_, _| {}).unwrap();
        mark(Some(since));
        assert_eq!(preview().stale_edits.ids, vec![1]);
        assert_eq!(execute(false).moved, 0);
        assert!(source.exists());
        assert_eq!(preview().stale_edits.ids, vec![1]);
        mark(None);
        let outcome = execute(true);
        assert_eq!((outcome.moved, outcome.failures.len()), (0, 1));
        mark(Some(since));
        assert_eq!(preview().stale_edits.ids, vec![1]);
        assert_eq!(execute(true).moved, 1);
        let (pending, quarantined): (Option<i64>, Option<i64>) = manager.db.lock().unwrap().query_row(
            "SELECT edit_pending_since,quarantined_at FROM assets WHERE id=1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(pending.is_none() && quarantined.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_round_trip_keeps_sidecars_and_both_conflicting_photos() {
        let root = temp();
        let (_, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let source = root.join("src/a.jpg");
        let sidecar = root.join("src/a.xmp");
        fs::write(&sidecar, b"sidecar").unwrap();
        fs::write(root.join("dst/a.rw2"), b"existing quarantine file with the same XMP stem").unwrap();
        let skipped = manager.move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::Skip, false, |_, _| {}).unwrap();
        assert_eq!((skipped.moved, skipped.failures.len()), (0, 1));
        let moved = manager.move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::KeepBoth, false, |_, _| {}).unwrap();
        assert_eq!((moved.moved, moved.failures.len()), (1, 0));
        assert!(!source.exists() && !sidecar.exists());
        assert_eq!(fs::read(root.join("dst/a (2).xmp")).unwrap(), b"sidecar");
        assert!(root.join("dst/a.rw2").exists());
        {
            let conn = manager.db.lock().unwrap();
            let (path, origin, seen, stamp): (String, String, Option<i64>, i64) =
                conn.query_row("SELECT path,quarantine_origin,incoming_seen_at,quarantined_at FROM assets WHERE id=1", [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
            assert_eq!(Path::new(&path), root.join("dst/a (2).jpg"));
            assert_eq!(origin, source.to_string_lossy());
            assert_eq!(seen, None);
            assert!(stamp > 0);
        }
        fs::write(&source, b"new original-name file").unwrap();
        let restored = manager.restore_from_quarantine(&[1], &settings, |_, _| {}).unwrap();
        assert_eq!((restored.restored, restored.failures.len()), (1, 0));
        assert_eq!(fs::read(&source).unwrap(), b"new original-name file");
        assert_eq!(fs::read(root.join("src/a (2).jpg")).unwrap(), b"abc");
        assert_eq!(fs::read(root.join("src/a (2).xmp")).unwrap(), b"sidecar");
        assert!(manager.db.lock().unwrap().query_row("SELECT incoming_seen_at IS NOT NULL AND
            quarantined_at IS NULL AND quarantine_origin IS NULL FROM assets WHERE id=1", [], |r| r.get::<_, bool>(0)).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_delete_uses_bin_and_requires_drive_confirmation_for_purge() {
        for bin in [true, false] {
            let temp_dir = temp();
            let root = temp_dir.join(if bin { "fixed" } else { "nobin" });
            let (disk, manager, settings) = quarantine_fixture(&root);
            let formats = crate::formats::FormatTable::load_selected(None);
            fs::write(root.join("src/a.jpg.xmp"), b"sidecar").unwrap();
            let moved = manager.move_to_quarantine(&[1], &settings, &formats, ConflictPolicy::KeepBoth, false, |_, _| {}).unwrap();
            assert_eq!(moved.moved, 1);
            let photo = root.join("dst/a.jpg");
            let sidecar = root.join("dst/a.jpg.xmp");
            let info = manager.quarantine_delete_info(&[1], &settings).unwrap();
            assert_eq!((info.count, info.sidecars, info.permanent_count), (1, 1, usize::from(!bin)));
            if !bin {
                let refused = manager.delete_from_quarantine(&[1], &settings, &[], |_, _| {}).unwrap();
                assert!(refused.failures[0].contains("potvrdenie"));
                assert!(photo.exists() && sidecar.exists());
                manager.quarantine_delete_info(&[1], &settings).unwrap();
            }
            let deleted = manager.delete_from_quarantine(&[1], &settings, &info.no_bin_drives, |_, _| {}).unwrap();
            assert_eq!((deleted.deleted, deleted.failures.len()), (1, 0));
            assert!(!photo.exists() && !sidecar.exists());
            assert_eq!(disk.recycled_item(&photo).is_some(), bin);
            assert_eq!(disk.recycled_item(&sidecar).is_some(), bin);
            let conn = manager.db.lock().unwrap();
            assert_eq!(conn.query_row("SELECT count(*) FROM assets WHERE id=1", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(conn.query_row("SELECT action FROM ddreams_changes ORDER BY id DESC LIMIT 1", [],
                |r| r.get::<_, String>(0)).unwrap(), if bin { "delete_from_quarantine" } else { "purge_from_quarantine" });
            fs::remove_dir_all(temp_dir).unwrap();
        }
    }

    #[test]
    fn incoming_seen_is_initialized_for_existing_and_new_assets_and_cleared_on_move() {
        let root = temp();
        let (_, manager, settings) = quarantine_fixture(&root);
        let src = settings.incoming();
        let seen = |id| manager.db.lock().unwrap().query_row("SELECT incoming_seen_at FROM assets WHERE id=?1",
            [id], |r| r.get::<_, Option<i64>>(0)).unwrap();
        assert!(seen(1).is_some());
        manager.db.lock().unwrap().execute("UPDATE assets SET incoming_seen_at=1 WHERE id=1", []).unwrap();
        manager.sync_incoming(src.to_str().unwrap()).unwrap();
        assert_eq!(seen(1), Some(1));
        let path = src.join("b.jpg");
        fs::write(&path, b"test").unwrap();
        let new = manager.writer.catalog_listed_assets(&[crate::journal::ListedAsset {
            temporary_id: -2, name: "b.jpg".into(), ext: "jpg".into(), size: Some(4.0),
            file_modified: None, path: path.to_string_lossy().into_owned(), folder_path: src.to_string_lossy().into_owned(),
        }]).unwrap()[0].new_id;
        assert!(seen(new).unwrap() > 1);
        manager.db.lock().unwrap().execute("UPDATE assets SET delete_suggestion=1 WHERE id IN (1,?1)", [new]).unwrap();
        manager.move_files(&[1], settings.quarantine().to_str().unwrap(), ConflictPolicy::Cancel, |_, _| {}).unwrap();
        assert_eq!(seen(1), None);
        let suggestion = |id| manager.db.lock().unwrap().query_row("SELECT delete_suggestion FROM assets WHERE id=?1",
            [id], |r| r.get::<_, Option<i64>>(0)).unwrap();
        assert_eq!(suggestion(1), None);
        assert_eq!(suggestion(new), Some(1));
        fs::remove_file(path).unwrap();
        manager.sync_incoming(src.to_str().unwrap()).unwrap();
        assert_eq!(seen(new), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_rejects_import_folder_and_its_descendants() {
        let root = temp();
        let source = root.join("Panasonic");
        fs::create_dir(&source).unwrap();
        for target in [&source, &source.join("nested"), &root, &root.join("other/../Panasonic/nested")] {
            assert!(crate::quarantine::validate_roots(&source, target).is_err());
        }
        assert!(crate::quarantine::validate_roots(&source, &root.join("_Karanténa")).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_quarantine_review_deletes_only_previously_reviewed_unchanged_expired_files() {
        let root = temp();
        let (disk, manager, settings) = quarantine_fixture(&root);
        let formats = crate::formats::FormatTable::load_selected(None);
        let old = crate::quarantine::now() - 31 * crate::quarantine::DAY;
        let fresh = crate::quarantine::now();
        {
            let conn = manager.db.lock().unwrap();
            for (id, folder, age) in [(2, "dst", old), (3, "dst", fresh), (4, "src", old), (5, "dst", old)] {
                let name = format!("{id}.jpg");
                let path = root.join(folder).join(&name);
                fs::write(&path, b"before").unwrap();
                conn.execute("INSERT INTO assets(id,path,name,ext,ext_lower,is_listed,quarantined_at,quarantine_origin)
                    VALUES(?1,?2,?3,'jpg','jpg',1,?4,?5)", (id, path.to_str().unwrap(), &name, age, root.join("src").join(&name).to_str().unwrap())).unwrap();
            }
        }
        let preview = manager.prepare_quarantine_review(settings.clone(), &formats, None).unwrap();
        assert_eq!(preview.deletes.ids, vec![2, 5]);
        assert_eq!(preview.deletes.bytes, 12);
        assert!(root.join("dst/2.jpg").exists());
        fs::write(root.join("dst/5.jpg"), b"replacement").unwrap();
        let result = manager.execute_quarantine_review(&settings, &formats, None, crate::quarantine::ReviewSelection { moves: true, deletes: true, stale_edits: true }, &[], |_, _| {}).unwrap();
        assert_eq!((result.deleted, result.failures.len()), (1, 1));
        assert!(disk.recycled_item(&root.join("dst").join("2.jpg")).is_some());
        assert!(root.join("dst/3.jpg").exists());
        assert!(root.join("src/4.jpg").exists());
        assert_eq!(fs::read(root.join("dst/5.jpg")).unwrap(), b"replacement");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]fn same_volume_move_updates_catalogue_and_moves_sidecar(){let root=temp();let(disk,manager)=set_up(&root,None);let src=root.join("src/a.jpg");fs::write(root.join("src/a.xmp"),b"side").unwrap();let db=manager.db.clone();seed_asset_path(&db,&root);let listed=ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None));let manager=FileManager::with_fs(db.clone(),listed,manager.writer.clone(),false,disk);let result=manager.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();assert_eq!(result.changed_count,1);assert!(!src.exists());assert!(root.join("dst/a.xmp").exists());assert_eq!(db.lock().unwrap().query_row("SELECT path||':'||rating FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),format!("{}:4",root.join("dst").join("a.jpg").display()));let _=fs::remove_dir_all(root);}
    #[test]fn copy_duplicates_metadata_and_relations(){let root=temp();let(disk,mut manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);let listing=ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None));manager=FileManager::with_fs(db.clone(),listing,manager.writer.clone(),false,disk.clone());let result=manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();let id=result.remapped_ids[0].new_id;{let c=db.lock().unwrap();assert_eq!(c.query_row("SELECT rating||':'||caption FROM assets WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),"4:cap");assert_eq!(c.query_row("SELECT count(*) FROM asset_categories WHERE asset_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);assert_eq!(c.query_row("SELECT count(*) FROM asset_keywords WHERE asset_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);}let _=fs::remove_dir_all(root);}
    #[test]fn conflicts_skip_keep_both_and_overwrite_refusal(){let root=temp();let(disk,mut manager)=set_up(&root,None);fs::write(root.join("dst/a.jpg"),b"existing").unwrap();let db=manager.db.clone();seed_asset_path(&db,&root);manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk);assert_eq!(manager.preview_conflicts(&[1],root.join("dst").to_str().unwrap(),None).unwrap(),1);let skip=manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Skip,|_,_|{}).unwrap();assert_eq!(skip.skipped_count,1);assert_eq!(fs::read(root.join("dst/a.jpg")).unwrap(),b"existing");manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::KeepBoth,|_,_|{}).unwrap();assert_eq!(fs::read(root.join("dst/a (2).jpg")).unwrap(),b"abc");let _=fs::remove_dir_all(root);}
    #[test]fn cross_volume_switch_and_uncatalogued_move_are_testable(){let root=temp();let(disk,manager)=set_up(&root,Some(false));let listed=ListingService::new(manager.db.clone(),crate::formats::FormatTable::load_selected(None));let _=listed;let mut m=manager;let source=root.join("src/a.jpg");let asset=crate::db::Asset{id:-10_000_000,folder_id:None,name:"a.jpg".into(),ext:"jpg".into(),size:Some(3.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(source.to_string_lossy().to_string()),is_directory:false,offline:false,missing:false,has_categories:false, has_keywords: false,display_categories:None,display_keywords:None,quarantined_at:None,edit_pending_since:None};m.listing.insert_test_asset(asset);m.move_files(&[-10_000_000],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();assert!(!source.exists());assert!(root.join("dst/a.jpg").exists());assert_eq!(m.db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE path LIKE '%a.jpg'",[],|r|r.get::<_,i64>(0)).unwrap(),1);let _=fs::remove_dir_all(root);}
    #[test]fn simulated_mid_batch_failure_rolls_back_and_readonly_refuses(){let root=temp();let(disk,manager)=set_up(&root,Some(true));fs::write(root.join("src/a.xmp"),b"side").unwrap();seed_asset_path(&manager.db,&root);disk.fail_move.store(1,Ordering::SeqCst);let error=manager.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap_err();assert!(error.contains("simulated failure"));assert!(root.join("src/a.jpg").exists());assert!(root.join("src/a.xmp").exists());assert!(!root.join("dst/a.jpg").exists());assert_eq!(manager.db.lock().unwrap().query_row("SELECT path FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),root.join("src/a.jpg").to_string_lossy());let ro=FileManager::with_fs(manager.db.clone(),manager.listing.clone(),manager.writer.clone(),true,disk);assert!(ro.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap_err().contains("iba na čítanie"));let _=fs::remove_dir_all(root);}
    #[test]fn delete_recycles_main_and_sidecar_and_removes_rows(){let root=temp();let(disk,manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);fs::write(root.join("src/a.xmp"),b"side").unwrap();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk.clone());
        let result=manager.delete_files(&[1],false,&[],&mut Vec::new()).unwrap();assert_eq!(result.summary,"Odstránené do Koša: 1 súbor");assert!(!root.join("src/a.jpg").exists());assert!(!root.join("src/a.xmp").exists());
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        let _=fs::remove_dir_all(root);}
    #[test]
    fn failed_catalogue_write_rolls_back_file_copy_and_recycle() {
        let root = temp();
        let (disk, base) = set_up(&root, None);
        let db = base.db.clone();
        seed_asset_path(&db, &root);
        let source = root.join("src/a.jpg");
        let sidecar = root.join("src/a.xmp");
        fs::write(&sidecar, b"synthetic sidecar").unwrap();
        db.lock().unwrap().execute_batch(
            "CREATE TRIGGER fail_history BEFORE INSERT ON ddreams_changes
             BEGIN SELECT RAISE(ABORT,'simulated catalogue failure'); END;"
        ).unwrap();
        let manager = FileManager::with_fs(db.clone(), base.listing.clone(), base.writer.clone(), false, disk.clone());

        let target = root.join("dst");
        assert!(manager.copy_files(&[1], target.to_str().unwrap(), ConflictPolicy::Cancel, |_, _| {})
            .unwrap_err().contains("simulated catalogue failure"));
        assert!(!target.join("a.jpg").exists());
        assert!(!target.join("a.xmp").exists());
        assert!(manager.delete_files(&[1], false, &[], &mut Vec::new()).unwrap_err().contains("simulated catalogue failure"));
        assert_eq!(fs::read(&source).unwrap(), b"abc");
        assert_eq!(fs::read(&sidecar).unwrap(), b"synthetic sidecar");
        assert!(disk.recycled_item(&source).is_none());
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let _ = fs::remove_dir_all(root);
    }

    fn fail_history(db: &Db) {
        db.lock().unwrap().execute_batch(
            "CREATE TRIGGER fail_history BEFORE INSERT ON ddreams_changes
             BEGIN SELECT RAISE(ABORT,'simulated catalogue failure'); END;"
        ).unwrap();
    }

    #[test]
    fn failed_delete_record_reports_all_restore_errors_and_restores_later_files() {
        let root = temp();
        let (disk, manager) = set_up(&root, None);
        seed_asset_path(&manager.db, &root);
        let main = root.join("src/a.jpg");
        let xmp = root.join("src/a.xmp");
        let last = root.join("src/a.jpg.xmp");
        fs::write(&xmp, b"side").unwrap();
        fs::write(&last, b"last").unwrap();
        disk.failed_restores.lock().unwrap().extend([main.clone(), xmp.clone()]);
        fail_history(&manager.db);

        let error = manager.delete_files(&[1], false, &[], &mut Vec::new()).unwrap_err();
        assert!(error.contains("simulated catalogue failure"));
        assert!(error.contains(&main.display().to_string()));
        assert!(error.contains(&xmp.display().to_string()));
        assert_eq!(fs::read(&last).unwrap(), b"last");
        assert!(disk.recycled_item(&main).is_some() && disk.recycled_item(&xmp).is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_folder_record_removes_copy_only_when_source_is_safe() {
        for (copy, same_volume, fail_restore) in [(true, true, false), (false, false, false), (false, false, true), (false, true, false)] {
            let root = temp();
            let (disk, manager) = set_up(&root, Some(same_volume));
            let source = root.join("src");
            let parent = root.join("nobin");
            let dest = parent.join("src");
            fs::create_dir(&parent).unwrap();
            fail_history(&manager.db);
            if fail_restore { disk.failed_restores.lock().unwrap().insert(source.clone()); }

            let error = manager.transfer_folder(source.to_str().unwrap(), parent.to_str().unwrap(), copy, &mut |_, _| {})
                .unwrap_err();
            assert!(error.contains("simulated catalogue failure"), "{error}");
            assert_eq!(error.contains("simulated restore failure"), fail_restore);
            assert_eq!(source.join("a.jpg").is_file(), !fail_restore);
            assert_eq!(dest.join("a.jpg").is_file(), fail_restore);
            assert_eq!(fs::read_dir(&disk.recycle).unwrap().count(), usize::from(fail_restore));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn failed_permanent_delete_record_reports_disk_deletions_and_keeps_ids_for_ai() {
        let root = temp();
        let (_, manager) = set_up(&root, None);
        seed_asset_path(&manager.db, &root);
        let main = root.join("src/a.jpg");
        let xmp = root.join("src/a.xmp");
        fs::write(&xmp, b"side").unwrap();
        fail_history(&manager.db);
        let mut purged = Vec::new();

        let error = manager.delete_files(&[1], true, &[], &mut purged).unwrap_err();
        assert!(error.contains("už boli natrvalo zmazané z disku"));
        assert!(error.contains(&main.display().to_string()) && error.contains(&xmp.display().to_string()));
        assert!(!main.exists() && !xmp.exists());
        assert_eq!(purged, vec![1]);
        assert_eq!(manager.db.lock().unwrap().query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]fn recycle_info_parser_reads_v1_and_v2(){let path="C:\\Foto\\a.jpg";let units:Vec<u8>=path.encode_utf16().chain(Some(0)).flat_map(|u|u.to_le_bytes()).collect();
        let mut v2=Vec::new();v2.extend(2u64.to_le_bytes());v2.extend(5u64.to_le_bytes());v2.extend(77u64.to_le_bytes());v2.extend(((units.len()/2) as u32).to_le_bytes());v2.extend(&units);assert_eq!(parse_recycle_info(&v2),Some((77,path.to_string())));
        let mut v1=Vec::new();v1.extend(1u64.to_le_bytes());v1.extend(5u64.to_le_bytes());v1.extend(9u64.to_le_bytes());let mut fixed=units.clone();fixed.resize(520,0);v1.extend(fixed);assert_eq!(parse_recycle_info(&v1),Some((9,path.to_string())));assert_eq!(parse_recycle_info(&[0;10]),None);}
    #[test]fn drive_without_bin_requires_confirmation_before_purging(){let root=temp().join("nobin");fs::create_dir_all(&root).unwrap();let(disk,manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);fs::write(root.join("src/a.xmp"),b"side").unwrap();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk.clone());
        let info=manager.delete_info(&[1],false).unwrap();assert_eq!((info.count,info.sidecars,info.permanent_count),(1,1,1));
        assert!(manager.delete_files(&[1],false,&[],&mut Vec::new()).unwrap_err().contains("potvrdenie"));
        assert!(root.join("src/a.jpg").exists());
        let mut purged=Vec::new();let result=manager.delete_files(&[1],false,&info.no_bin_drives,&mut purged).unwrap();assert_eq!(result.summary,"Zmazané natrvalo: 1 súbor");assert_eq!(purged,vec![1]);
        assert_eq!(db.lock().unwrap().query_row("SELECT action FROM ddreams_changes WHERE id=?1",
            [result.change_id.unwrap()], |r| r.get::<_, String>(0)).unwrap(), "purge_files");
        assert!(!root.join("src/a.jpg").exists());assert!(!root.join("src/a.xmp").exists());assert!(disk.recycled_item(&root.join("src/a.jpg")).is_none(),"not in the bin");
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0);let _=fs::remove_dir_all(root.parent().unwrap());}
    #[test]fn shift_delete_is_permanent_even_with_a_bin(){let root=temp();let(disk,manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk.clone());
        assert_eq!(manager.delete_info(&[1],true).unwrap().permanent_count,1);assert_eq!(manager.delete_info(&[1],false).unwrap().permanent_count,0);
        let result=manager.delete_files(&[1],true,&[],&mut Vec::new()).unwrap();assert!(result.summary.starts_with("Zmazané natrvalo"));assert!(!root.join("src/a.jpg").exists());assert!(disk.recycled_item(&root.join("src/a.jpg")).is_none());let _=fs::remove_dir_all(root);}
    #[test]fn folder_rename_rewrites_catalogue_paths_and_rules(){let root=temp();let(disk,base)=set_up(&root,None);let db=base.db.clone();let source=root.join("src");let db_path=source.to_string_lossy().to_string();{let c=db.lock().unwrap();c.execute("UPDATE assets SET path=?1",[source.join("a.jpg").to_string_lossy().to_string()]).unwrap();c.execute("UPDATE folders SET path=?1 WHERE id=2",[&db_path]).unwrap();c.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT);").unwrap();c.execute("INSERT INTO ddreams_folder_rules VALUES(?1,'include','now')",[&db_path]).unwrap();}let manager=FileManager::with_fs(db.clone(),base.listing.clone(),base.writer.clone(),false,disk);let target=root.join("renamed");manager.rename_folder(&db_path,"renamed").unwrap();assert_eq!(db.lock().unwrap().query_row("SELECT path FROM ddreams_folder_rules",[],|r|r.get::<_,String>(0)).unwrap(),target.to_string_lossy());assert!(target.join("a.jpg").exists());assert_eq!(db.lock().unwrap().query_row("SELECT path FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap().to_lowercase(),crate::journal::normalize_fs_path(&target.join("a.jpg").to_string_lossy()));let _=fs::remove_dir_all(root);}
    #[test]fn folder_rename_refuses_invalid_names_and_existing_targets(){let root=temp();let(disk,base)=set_up(&root,None);fs::create_dir(root.join("dst/src")).unwrap();let manager=FileManager::with_fs(base.db.clone(),base.listing.clone(),base.writer.clone(),false,disk);assert!(manager.rename_folder(root.join("src").to_str().unwrap(),"bad/name").is_err());assert!(manager.rename_folder(root.join("src").to_str().unwrap(),"bad.").is_err());assert!(manager.rename_folder(root.join("src").to_str().unwrap(),"bad ").is_err());assert!(manager.rename_folder("C:\\","x").is_err());assert!(manager.rename_folder(root.join("src").to_str().unwrap(),"dst").is_err());let _=fs::remove_dir_all(root);}
    #[test]fn folder_delete_recycles_and_removes_rows(){let root=temp();let(disk,base)=set_up(&root,None);let db=base.db.clone();let source=root.join("src");{let c=db.lock().unwrap();c.execute("UPDATE assets SET path=?1",[source.join("a.jpg").to_string_lossy().to_string()]).unwrap();c.execute("UPDATE folders SET path=?1 WHERE id=2",[source.to_string_lossy().to_string()]).unwrap();}let manager=FileManager::with_fs(db.clone(),base.listing.clone(),base.writer.clone(),false,disk);manager.delete_folder(source.to_str().unwrap()).unwrap();assert!(!source.exists());assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets",[],|r|r.get::<_,i64>(0)).unwrap(),0);let _=fs::remove_dir_all(root);}
    #[test]fn folder_copy_has_no_catalogue_rows_and_refuses_conflicts_and_descendants(){let root=temp();let(disk,base)=set_up(&root,None);let manager=FileManager::with_fs(base.db.clone(),base.listing.clone(),base.writer.clone(),false,disk);let source=root.join("src");let target=root.join("dst");manager.copy_folder(source.to_str().unwrap(),target.to_str().unwrap()).unwrap();assert!(target.join("src/a.jpg").exists());assert_eq!(base.db.lock().unwrap().query_row("SELECT count(*) FROM assets",[],|r|r.get::<_,i64>(0)).unwrap(),1);assert!(manager.copy_folder(source.to_str().unwrap(),target.to_str().unwrap()).is_err());assert!(manager.move_folder(source.to_str().unwrap(),source.join("child").to_str().unwrap()).is_err());let _=fs::remove_dir_all(root);}

    #[test]fn same_volume_folder_move_rewrites_paths(){let root=temp();let(disk,base)=set_up(&root,None);let db=base.db.clone();let source=root.join("src");let dest=root.join("dst/src");{let c=db.lock().unwrap();c.execute("UPDATE assets SET path=?1",[source.join("a.jpg").to_string_lossy().to_string()]).unwrap();c.execute("UPDATE folders SET path=?1 WHERE id=2",[source.to_string_lossy().to_string()]).unwrap();}let manager=FileManager::with_fs(db.clone(),base.listing.clone(),base.writer.clone(),false,disk);manager.move_folder(source.to_str().unwrap(),root.join("dst").to_str().unwrap()).unwrap();assert!(dest.join("a.jpg").exists());assert_eq!(db.lock().unwrap().query_row("SELECT path FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap().to_lowercase(),crate::journal::normalize_fs_path(&dest.join("a.jpg").to_string_lossy()));let _=fs::remove_dir_all(root);}
    #[test]fn folder_delete_refuses_when_recycle_bin_is_unavailable(){let root=temp().join("nobin");fs::create_dir_all(&root).unwrap();let(disk,base)=set_up(&root,None);let manager=FileManager::with_fs(base.db.clone(),base.listing.clone(),base.writer.clone(),false,disk);assert!(manager.delete_folder(root.join("src").to_str().unwrap()).is_err());assert!(root.join("src/a.jpg").exists());let _=fs::remove_dir_all(root.parent().unwrap());}

}
