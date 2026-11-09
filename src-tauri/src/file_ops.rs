use crate::{db::Db, journal::{AssetIdRemap, CatalogWriter, FileCatalogCopy, FileCatalogMove}, listing::ListingService};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs::{self, File, FileTimes, OpenOptions}, io, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::{SystemTime, UNIX_EPOCH}};
#[cfg(test)] use std::sync::atomic::AtomicI64;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy { Skip, KeepBoth, Cancel }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult { pub change_id: Option<i64>, pub summary: String, pub changed_count: usize, pub skipped_count: usize, pub remapped_ids: Vec<AssetIdRemap>, pub affected_ids: Vec<i64>, pub affected_folder_paths: Vec<String> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum UndoPayload { Move { transfers: Vec<Transfer> }, Copy { copied: Vec<FileStamp> }, CreateFolder { path: PathBuf }, Delete { files: Vec<PathBuf> } }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Transfer { from: PathBuf, to: PathBuf }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileStamp { path: PathBuf, size: u64, modified_ns: String }
#[derive(Clone, Debug)]
struct Source { id: i64, path: PathBuf, name: String, size: u64, modified: SystemTime }
#[derive(Clone, Debug)]
struct Plan { source: Source, target: PathBuf, pairs: Vec<Transfer>, conflict: bool }

pub trait FileSystemOps: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn metadata(&self, path: &Path) -> io::Result<fs::Metadata>;
    fn same_volume(&self, source: &Path, target: &Path) -> bool;
    fn move_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn copy_no_replace(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    fn recycle_files(&self, paths: &[PathBuf]) -> Result<(), String>;
    /// User-initiated delete: Recycle Bin only, refused where there is no Recycle Bin (USB/network).
    fn recycle_delete(&self, paths: &[PathBuf]) -> Result<(), String>;
    /// The Recycle Bin item that holds `original`, if any (most recently deleted).
    fn recycled_item(&self, original: &Path) -> Option<PathBuf>;
    fn restore_recycled(&self, original: &Path) -> Result<(), String>;
}

pub struct NativeFileSystem;
impl FileSystemOps for NativeFileSystem {
    fn exists(&self, path:&Path)->bool { path.exists() }
    fn is_dir(&self,path:&Path)->bool { path.is_dir() }
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
    fn recycle_files(&self,paths:&[PathBuf])->Result<(),String> { recycle_windows(paths) }
    fn recycle_delete(&self,paths:&[PathBuf])->Result<(),String> {
        for path in paths { if !has_recycle_bin(path) { return Err(format!("Na disku so súborom {} nie je Kôš (USB alebo sieťový disk). DDreams tu súbory nemaže, zmazanie by bolo trvalé.",path.display())); } }
        recycle_windows(paths)?;
        if let Some(left)=paths.iter().find(|p|p.exists()) { return Err(format!("Súbor sa nepodarilo presunúť do Koša: {}",left.display())); }
        Ok(())
    }
    fn recycled_item(&self,original:&Path)->Option<PathBuf> { find_recycled(original).map(|(data,_)|data) }
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

pub struct FileManager { db:Db, listing:Arc<ListingService>, writer:Arc<CatalogWriter>, readonly:bool, fs:Arc<dyn FileSystemOps> }
impl FileManager {
    pub fn new(db:Db,listing:Arc<ListingService>,writer:Arc<CatalogWriter>,readonly:bool)->Arc<Self>{Arc::new(Self{db,listing,writer,readonly,fs:Arc::new(NativeFileSystem)})}
    #[cfg(test)] fn with_fs(db:Db,listing:Arc<ListingService>,writer:Arc<CatalogWriter>,readonly:bool,fs:Arc<dyn FileSystemOps>)->Self{Self{db,listing,writer,readonly,fs}}
    fn writable(&self)->Result<(),String>{if self.readonly{Err("Katalóg je iba na čítanie".into())}else{Ok(())}}

    pub fn preview_conflicts(&self,ids:&[i64],target:&str,rename_base:Option<&str>)->Result<usize,String>{let folder=self.validate_folder(target)?;let sources=self.resolve_sources(ids)?;let plans=self.plan(&sources,&folder,rename_base)?;Ok(plans.iter().filter(|p|!path_eq(p.source.path.parent().unwrap_or(Path::new("")),&folder)&&p.conflict).count())}

    pub fn move_files<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){self.run_copy_move(ids,target,policy,false,&mut progress)}
    pub fn copy_files<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){self.run_copy_move(ids,target,policy,true,&mut progress)}

    fn run_copy_move<F>(&self,ids:&[i64],target:&str,policy:ConflictPolicy,copy:bool,progress:&mut F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){
        self.writable()?;let folder=self.validate_folder(target)?;let sources=self.resolve_sources(ids)?;if sources.is_empty(){return Err("Nie sú vybrané žiadne súbory".into())}let mut plans=self.plan(&sources,&folder,None)?;let same_folder_count=plans.iter().filter(|plan|path_eq(plan.source.path.parent().unwrap_or(Path::new("")),&folder)).count();plans.retain(|plan|!path_eq(plan.source.path.parent().unwrap_or(Path::new("")),&folder));if same_folder_count==sources.len(){return Ok(FileOperationResult{change_id:None,summary:"Súbory už sú v tomto priečinku".into(),changed_count:0,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:Vec::new()});}
        let conflicts=plans.iter().filter(|p|p.conflict).count();if conflicts>0&&policy==ConflictPolicy::Cancel{return Err("Operácia bola zrušená".into())}
        let mut skipped=0usize;let mut reserved=HashSet::new();let mut selected=Vec::new();
        for mut plan in plans.drain(..){if plan.conflict&&policy==ConflictPolicy::Skip{skipped+=1;continue;}if policy==ConflictPolicy::KeepBoth { plan=self.number_conflict(plan,&folder,&reserved)?; }
            if plan.pairs.iter().any(|pair|self.fs.exists(&pair.to)||reserved.contains(&key(&pair.to))){if policy==ConflictPolicy::Skip{skipped+=1;continue;}return Err(format!("V cieli už existuje súbor: {}",plan.target.display()));}
            for pair in &plan.pairs{reserved.insert(key(&pair.to));}selected.push(plan);
        }
        if selected.is_empty(){return Ok(self.no_change_result(skipped,if copy{"Kopírovanie"}else{"Presun"}));}
        let total=selected.len();let mut applied=Vec::<Transfer>::new();let mut copied_transfers=Vec::<Transfer>::new();let mut moved_catalog=Vec::new();let mut copied_catalog=Vec::new();let mut source_ids=Vec::new();let mut copied_stamps=Vec::new();
        for (index,plan) in selected.iter().enumerate(){progress(index+1,total);let result=if copy{copy_transfers(self.fs.as_ref(),&plan.pairs)}else{move_transfers(self.fs.as_ref(),&plan.pairs)};
            match result{Ok(done)=>{if copy{copied_transfers.extend(done.iter().cloned());for pair in &done{match stamp(self.fs.as_ref(),&pair.to){Ok(value)=>copied_stamps.push(value),Err(error)=>{let rollback=remove_created(self.fs.as_ref(),&copied_transfers.iter().map(|x|x.to.clone()).collect::<Vec<_>>());return Err(combine_error(error,rollback));}}}}else{applied.extend(done);}source_ids.push(plan.source.id);
                if plan.source.id>0 {if copy{copied_catalog.push(FileCatalogCopy{source_asset_id:plan.source.id,target_path:plan.target.to_string_lossy().to_string(),target_folder_path:folder.to_string_lossy().to_string(),name:plan.target.file_name().unwrap_or_default().to_string_lossy().to_string(),ext:plan.target.extension().unwrap_or_default().to_string_lossy().to_string(),size:plan.source.size as f64,file_modified:Some(crate::listing::system_time_iso(plan.source.modified))});}else{moved_catalog.push(FileCatalogMove{asset_id:plan.source.id,target_path:plan.target.to_string_lossy().to_string(),target_folder_path:folder.to_string_lossy().to_string(),name:plan.target.file_name().unwrap_or_default().to_string_lossy().to_string(),ext:plan.target.extension().unwrap_or_default().to_string_lossy().to_string(),size:plan.source.size as f64,file_modified:Some(crate::listing::system_time_iso(plan.source.modified))});}}
            }Err(error)=>{let rollback=if copy{remove_created(self.fs.as_ref(),&copied_transfers.iter().map(|x|x.to.clone()).collect::<Vec<_>>())}else{rollback_moves(self.fs.as_ref(),&applied)};return Err(combine_error(error,rollback));}}
        }
        let payload=if copy{UndoPayload::Copy{copied:copied_stamps.clone()}}else{UndoPayload::Move{transfers:applied.clone()}};let payload=serde_json::to_string(&payload).map_err(|e|e.to_string())?;
        let count=source_ids.len();let noun=slovak_files(count);let past=if copy{if count==1{"Skopírovaný"}else if count<=4{"Skopírované"}else{"Skopírovaných"}}else if count==1{"Presunutý"}else if count<=4{"Presunuté"}else{"Presunutých"};let destination=folder.file_name().unwrap_or(folder.as_os_str()).to_string_lossy();let summary=format!("{past} {count} {noun} do {destination}");
        let recorded=self.writer.record_file_change(if copy{"copy_files"}else{"move_files"},&summary,&moved_catalog,&copied_catalog,&[],&payload);
        let (change_id,remapped_ids)=match recorded{Ok(v)=>v,Err(error)=>{let rollback=if copy{remove_created(self.fs.as_ref(),&copied_stamps.iter().map(|x|x.path.clone()).collect::<Vec<_>>())}else{rollback_moves(self.fs.as_ref(),&applied)};return Err(combine_error(error,rollback));}};
        self.invalidate_plans(&selected,&folder);let mut affected_folder_paths=vec![folder.to_string_lossy().to_string()];for plan in &selected{if let Some(parent)=plan.source.path.parent(){let path=parent.to_string_lossy().to_string();if !affected_folder_paths.iter().any(|item|path_eq(Path::new(item),Path::new(&path))){affected_folder_paths.push(path);}}}Ok(FileOperationResult{change_id,summary,changed_count:source_ids.len(),skipped_count:skipped,remapped_ids,affected_ids:source_ids,affected_folder_paths})
    }

    pub fn rename_file<F>(&self,id:i64,new_base_name:&str,mut progress:F)->Result<FileOperationResult,String> where F:FnMut(usize,usize){
        self.writable()?;let base=validate_windows_name(new_base_name)?;let mut sources=self.resolve_sources(&[id])?;let source=sources.pop().ok_or("Súbor sa nenašiel")?;let parent=source.path.parent().ok_or("Priečinok súboru sa nenašiel")?;let ext=source.path.extension().map(|x|x.to_string_lossy().to_string());let file_name=ext.as_ref().map(|e|format!("{base}.{e}")).unwrap_or_else(||base.clone());let target=parent.join(file_name);let mut pairs=sidecar_pairs(&source.path,&target)?;
        if pairs.iter().any(|p|self.fs.exists(&p.to)&&!path_eq(&p.from,&p.to)){return Err(format!("V priečinku už existuje súbor: {}",target.display()));}if pairs.iter().all(|p|p.from==p.to){return Ok(self.no_change_result(0,"Premenovanie"));}
        progress(1,1);let applied=move_transfers(self.fs.as_ref(),&pairs)?;pairs.clear();let payload=serde_json::to_string(&UndoPayload::Move{transfers:applied.clone()}).map_err(|e|e.to_string())?;let name=target.file_name().unwrap().to_string_lossy().to_string();let catalog_move=if source.id>0{vec![FileCatalogMove{asset_id:source.id,target_path:target.to_string_lossy().to_string(),target_folder_path:parent.to_string_lossy().to_string(),name:name.clone(),ext:target.extension().unwrap_or_default().to_string_lossy().to_string(),size:source.size as f64,file_modified:Some(crate::listing::system_time_iso(source.modified))}]}else{Vec::new()};let summary=format!("Premenované {} → {name}",source.path.file_name().unwrap_or_default().to_string_lossy());
        let (change_id,remapped_ids)=match self.writer.record_file_change("rename_file",&summary,&catalog_move,&[],&[],&payload){Ok(v)=>v,Err(error)=>return Err(combine_error(error,rollback_moves(self.fs.as_ref(),&applied)))};
        self.listing.invalidate_path(&parent.to_string_lossy()).ok();Ok(FileOperationResult{change_id,summary,changed_count:1,skipped_count:0,remapped_ids,affected_ids:vec![id],affected_folder_paths:vec![parent.to_string_lossy().to_string()]})
    }

    pub fn create_folder(&self,parent:&str,name:&str)->Result<FileOperationResult,String>{self.writable()?;let folder=self.validate_folder(parent)?;let name=validate_windows_name(name)?;let target=folder.join(name);if self.fs.exists(&target){return Err(format!("Priečinok už existuje: {}",target.display()))}self.fs.create_dir(&target).map_err(|e|format!("Nepodarilo sa vytvoriť priečinok {}: {e}",target.display()))?;let payload=serde_json::to_string(&UndoPayload::CreateFolder{path:target.clone()}).map_err(|e|e.to_string())?;let summary=format!("Vytvorený priečinok {}",target.file_name().unwrap_or_default().to_string_lossy());match self.writer.record_file_change("create_folder",&summary,&[],&[],&[target.to_string_lossy().to_string()],&payload){Ok((change_id,_))=>{self.listing.invalidate_path(parent).ok();Ok(FileOperationResult{change_id,summary,changed_count:1,skipped_count:0,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:vec![parent.to_string()]})},Err(error)=>{let rollback=self.fs.remove_dir(&target).map_err(|e|format!("Nepodarilo sa vrátiť priečinok {}: {e}",target.display()));Err(combine_error(error,rollback))}}
    }

    /// Moves the selected files and their sidecars to the Recycle Bin and removes their catalogue rows,
    /// as one undoable change. Directories are skipped.
    pub fn delete_files(&self,ids:&[i64])->Result<FileOperationResult,String>{
        self.writable()?;
        let files_only=ids.iter().copied().filter(|id|!(*id<0&&self.listing.asset(*id).is_some_and(|a|a.is_directory))).collect::<Vec<_>>();
        if files_only.is_empty(){return Err("Priečinky sa týmto nemažú".into());}
        let sources=self.resolve_sources(&files_only)?;
        if sources.is_empty(){return Err("Nie sú vybrané žiadne súbory".into());}
        let mut files=Vec::new();for source in &sources{for pair in sidecar_pairs(&source.path,&source.path)?{if !files.iter().any(|f:&PathBuf|path_eq(f,&pair.from)){files.push(pair.from);}}}
        self.fs.recycle_delete(&files)?;
        let payload=serde_json::to_string(&UndoPayload::Delete{files:files.clone()}).map_err(|e|e.to_string())?;
        let count=sources.len();let summary=format!("Odstránené do Koša: {count} {}",if count==1{"súbor"}else if count<5{"súbory"}else{"súborov"});
        let catalog_ids=sources.iter().filter(|s|s.id>0).map(|s|s.id).collect::<Vec<_>>();
        let change_id=match self.writer.record_file_delete(&catalog_ids,&summary,&payload){Ok(id)=>id,Err(error)=>{let rollback=files.iter().try_for_each(|f|self.fs.restore_recycled(f));return Err(combine_error(error,rollback));}};
        let mut folders=Vec::<String>::new();for source in &sources{if let Some(parent)=source.path.parent(){let p=parent.to_string_lossy().to_string();self.listing.invalidate_path(&p).ok();if !folders.contains(&p){folders.push(p);}}}
        Ok(FileOperationResult{change_id,summary,changed_count:count,skipped_count:0,remapped_ids:Vec::new(),affected_ids:files_only,affected_folder_paths:folders})
    }

    pub fn undo_last_change(&self)->Result<Option<crate::journal::ChangeSummary>,String>{self.writable()?;self.writer.undo_last_with(|payload|self.undo_payload(payload))}

    fn undo_payload(&self,payload:&str)->Result<(),String>{let action:UndoPayload=serde_json::from_str(payload).map_err(|e|format!("Neplatný záznam operácie so súborom: {e}"))?;match action{
        UndoPayload::Move{transfers}=>{for pair in &transfers{if !self.fs.exists(&pair.to){return Err(format!("Nemožno vrátiť zmenu, súbor už nie je na očakávanej ceste: {}",pair.to.display()))}if self.fs.exists(&pair.from)&&!path_eq(&pair.from,&pair.to){return Err(format!("Nemožno vrátiť zmenu bez prepísania existujúceho súboru: {}",pair.from.display()))}}let reverse=transfers.iter().rev().map(|p|Transfer{from:p.to.clone(),to:p.from.clone()}).collect::<Vec<_>>();move_transfers(self.fs.as_ref(),&reverse).map(|_|())},
        UndoPayload::Copy{copied}=>{for item in &copied{let current=stamp(self.fs.as_ref(),&item.path).map_err(|_|format!("Kópiu nemožno vrátiť, súbor chýba: {}",item.path.display()))?;if current.size!=item.size||current.modified_ns!=item.modified_ns{return Err(format!("Kópiu nemožno vrátiť, súbor sa po skopírovaní zmenil: {}",item.path.display()))}}self.fs.recycle_files(&copied.iter().map(|s|s.path.clone()).collect::<Vec<_>>())},
        UndoPayload::Delete{files}=>{for file in &files{if self.fs.exists(file){return Err(format!("Nemožno vrátiť: na pôvodnom mieste už je súbor {}",file.display()))}if self.fs.recycled_item(file).is_none(){return Err(format!("Nemožno vrátiť: súbor už nie je v Koši (Kôš bol možno vyprázdnený): {}",file.display()))}}for file in &files{self.fs.restore_recycled(file)?;if let Some(parent)=file.parent(){self.listing.invalidate_path(&parent.to_string_lossy()).ok();}}Ok(())},
        UndoPayload::CreateFolder{path}=>self.fs.remove_dir(&path).map_err(|e|format!("Priečinok nemožno vrátiť, nie je prázdny alebo už neexistuje: {} ({e})",path.display()))
    }}

    fn validate_folder(&self,path:&str)->Result<PathBuf,String>{let candidate=PathBuf::from(path);if !candidate.is_absolute()||!self.fs.is_dir(&candidate){return Err(format!("Cieľový priečinok nie je dostupný: {path}"))}Ok(candidate)}
    fn resolve_sources(&self,ids:&[i64])->Result<Vec<Source>,String>{let mut unique=HashSet::new();let mut out=Vec::new();for id in ids.iter().copied(){if !unique.insert(id){continue;}let (id,path,listed_name)=if id<0{let asset=self.listing.asset(id).ok_or_else(||format!("Súbor {id} už nie je v aktuálnom zozname"))?;let path=asset.path.ok_or("Zoznam nemá cestu k súboru")?;let catalog_id:Option<i64>={let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;conn.query_row("SELECT id FROM assets WHERE is_listed=1 AND lower(path)=lower(?1) ORDER BY id LIMIT 1",[&path],|r|r.get(0)).optional().map_err(|e|e.to_string())?};(catalog_id.unwrap_or(id),PathBuf::from(path),asset.name)}else{let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;let row:Option<(String,String)>=conn.query_row("SELECT path,name FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>''",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;let (path,name)=row.ok_or_else(||format!("Súbor katalógu {id} sa nenašiel"))?;(id,PathBuf::from(path),name)};let metadata=self.fs.metadata(&path).map_err(|_|format!("Súbor nie je dostupný: {}",path.display()))?;if !metadata.is_file(){return Err(format!("Vybraná položka nie je súbor: {}",path.display()))}let name=path.file_name().map(|x|x.to_string_lossy().to_string()).unwrap_or(listed_name);let modified=metadata.modified().map_err(|e|format!("Čas zmeny sa nedá prečítať: {} ({e})",path.display()))?;out.push(Source{id,path,name,size:metadata.len(),modified});}Ok(out)}

    fn plan(&self,sources:&[Source],folder:&Path,rename_base:Option<&str>)->Result<Vec<Plan>,String>{let mut plans=Vec::new();let mut reserved=HashSet::new();for source in sources{let main=if let Some(base)=rename_base{let ext=source.path.extension().map(|e|e.to_string_lossy().to_string());folder.join(ext.map(|e|format!("{base}.{e}")).unwrap_or_else(||base.to_string()))}else{folder.join(&source.name)};let pairs=sidecar_pairs(&source.path,&main)?;let conflict=pairs.iter().any(|p|self.fs.exists(&p.to)||reserved.contains(&key(&p.to)));for pair in &pairs{reserved.insert(key(&pair.to));}plans.push(Plan{source:source.clone(),target:main,pairs,conflict});}Ok(plans)}

    fn number_conflict(&self,plan:Plan,folder:&Path,reserved:&HashSet<String>)->Result<Plan,String>{if !plan.conflict{return Ok(plan)}let original=plan.target.file_stem().unwrap_or_default().to_string_lossy().to_string();let ext=plan.target.extension().map(|e|e.to_string_lossy().to_string());for n in 2..100_000u32{let base=format!("{original} ({n})");let target=folder.join(ext.as_ref().map(|e|format!("{base}.{e}")).unwrap_or_else(||base.clone()));let pairs=sidecar_pairs(&plan.source.path,&target)?;if pairs.iter().all(|p|!self.fs.exists(&p.to)&&!reserved.contains(&key(&p.to))){return Ok(Plan{source:plan.source,target, pairs, conflict:false});}}Err("Nenašlo sa voľné číslo pre názov súboru".into())}

    fn invalidate_plans(&self,plans:&[Plan],folder:&Path){let _=self.listing.invalidate_path(&folder.to_string_lossy());for plan in plans{if let Some(parent)=plan.source.path.parent(){let _=self.listing.invalidate_path(&parent.to_string_lossy());}}}
    fn no_change_result(&self,skipped:usize,operation:&str)->FileOperationResult{FileOperationResult{change_id:None,summary:if skipped>0{format!("{operation}: preskočené {skipped} {}",slovak_files(skipped))}else{format!("{operation}: bez zmien")},changed_count:0,skipped_count:skipped,remapped_ids:Vec::new(),affected_ids:Vec::new(),affected_folder_paths:Vec::new()}}
}

fn sidecar_pairs(source:&Path,target:&Path)->Result<Vec<Transfer>,String>{let mut pairs=vec![Transfer{from:source.to_path_buf(),to:target.to_path_buf()}];let Some(filename)=source.file_name().map(|x|x.to_string_lossy().to_string())else{return Err("Názov súboru sa nedá prečítať".into())};let mut candidates=Vec::new();if !source.extension().is_some_and(|x|x.eq_ignore_ascii_case("xmp")){candidates.push(source.with_extension("xmp"));candidates.push(source.with_file_name(format!("{filename}.xmp")));}let mut seen=HashSet::new();for candidate in candidates{if candidate.exists()&&seen.insert(key(&candidate)){let mapped=if candidate.file_name().is_some_and(|n|n.to_string_lossy().eq_ignore_ascii_case(&format!("{filename}.xmp"))){target.with_file_name(format!("{}.xmp",target.file_name().unwrap().to_string_lossy()))}else{target.with_extension("xmp")};pairs.push(Transfer{from:candidate,to:mapped});}}Ok(pairs)}

fn copy_transfers(fs:&dyn FileSystemOps,pairs:&[Transfer])->Result<Vec<Transfer>,String>{let mut done=Vec::<Transfer>::new();for pair in pairs{if let Err(error)=fs.copy_no_replace(&pair.from,&pair.to){let rollback=remove_created(fs,&done.iter().map(|x|x.to.clone()).collect::<Vec<_>>());return Err(combine_error(format!("{}: {error}",pair.from.display()),rollback));}done.push(pair.clone());}Ok(done)}
fn move_transfers(fs:&dyn FileSystemOps,pairs:&[Transfer])->Result<Vec<Transfer>,String>{let mut done=Vec::<Transfer>::new();for pair in pairs{let result=if fs.same_volume(&pair.from,&pair.to){move_case_safe(fs,&pair.from,&pair.to)}else{copy_then_remove(fs,&pair.from,&pair.to)};if let Err(error)=result{let rollback=rollback_moves(fs,&done);return Err(combine_error(format!("{}: {error}",pair.from.display()),rollback));}done.push(pair.clone());}Ok(done)}
fn move_case_safe(fs:&dyn FileSystemOps,source:&Path,target:&Path)->io::Result<()>{if path_eq(source,target)&&source!=target{let temp=temporary_sibling(source);fs.move_no_replace(source,&temp)?;if let Err(error)=fs.move_no_replace(&temp,target){let _=fs.move_no_replace(&temp,source);return Err(error)}Ok(())}else{fs.move_no_replace(source,target)}}
fn temporary_sibling(path:&Path)->PathBuf{static NEXT:AtomicUsize=AtomicUsize::new(0);let n=NEXT.fetch_add(1,Ordering::Relaxed);path.with_file_name(format!(".ddreams-rename-{}-{n}",std::process::id()))}
fn copy_then_remove(fs:&dyn FileSystemOps,source:&Path,target:&Path)->io::Result<()>{let before=fs.metadata(source)?;let before_modified=before.modified()?;fs.copy_no_replace(source,target)?;let copied=fs.metadata(target)?;let copied_modified=copied.modified()?;let source_after=fs.metadata(source)?;let after_modified=source_after.modified()?;let time_difference=if before_modified>=copied_modified{before_modified.duration_since(copied_modified).unwrap_or_else(|error|error.duration())}else{copied_modified.duration_since(before_modified).unwrap_or_else(|error|error.duration())};if before.len()!=copied.len()||before.len()!=source_after.len()||before_modified!=after_modified||time_difference>std::time::Duration::from_secs(2){let _=fs.remove_file(target);return Err(io::Error::other("copy verification failed"));}if let Err(error)=fs.remove_file(source){let _=fs.remove_file(target);return Err(error)}Ok(())}
fn rollback_moves(fs:&dyn FileSystemOps,done:&[Transfer])->Result<(),String>{let reverse=done.iter().rev().map(|x|Transfer{from:x.to.clone(),to:x.from.clone()}).collect::<Vec<_>>();let mut restored=Vec::<Transfer>::new();for pair in &reverse{let result=if fs.same_volume(&pair.from,&pair.to){move_case_safe(fs,&pair.from,&pair.to)}else{copy_then_remove(fs,&pair.from,&pair.to)};if let Err(error)=result{let redo=restored.iter().rev().map(|x|Transfer{from:x.to.clone(),to:x.from.clone()}).collect::<Vec<_>>();let _=move_transfers(fs,&redo);return Err(format!("Rollback failed for {}: {error}",pair.from.display()))}restored.push(pair.clone());}Ok(())}
fn remove_created(fs:&dyn FileSystemOps,paths:&[PathBuf])->Result<(),String>{for path in paths.iter().rev(){if fs.exists(path){fs.remove_file(path).map_err(|e|format!("Nepodarilo sa odstrániť čiastočnú kópiu {}: {e}",path.display()))?;}}Ok(())}
fn stamp(fs:&dyn FileSystemOps,path:&Path)->Result<FileStamp,String>{let meta=fs.metadata(path).map_err(|e|format!("Nepodarilo sa overiť súbor {}: {e}",path.display()))?;let ns=meta.modified().map_err(|e|e.to_string())?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().to_string();Ok(FileStamp{path:path.to_path_buf(),size:meta.len(),modified_ns:ns})}
fn path_eq(a:&Path,b:&Path)->bool{key(a)==key(b)}
fn key(path:&Path)->String{path.to_string_lossy().replace('/',"\\").trim_end_matches('\\').to_lowercase()}
fn combine_error(error:impl ToString,rollback:Result<(),String>)->String{match rollback{Ok(())=>error.to_string(),Err(rollback)=>format!("{}; {}",error.to_string(),rollback)}}
fn slovak_files(n:usize)->&'static str{if n==1{"súbor"}else if (2..=4).contains(&n){"súbory"}else{"súborov"}}

pub fn validate_windows_name(name:&str)->Result<String,String>{let value=name.trim();if value.is_empty(){return Err("Názov nesmie byť prázdny".into())}if value.len()>255{return Err("Názov môže mať najviac 255 znakov".into())}if value.ends_with('.')||value.ends_with(' '){return Err("Názov sa nesmie končiť bodkou ani medzerou".into())}if value.chars().any(|c|matches!(c,'\\'|'/'|':'|'*'|'?'|'"'|'<'|'>'|'|')){return Err("Názov obsahuje znak, ktorý Windows nepovoľuje".into())}let stem=value.split('.').next().unwrap_or("").trim_end_matches([' ','.']).to_ascii_uppercase();if matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL")||(1..=9).any(|i|stem==format!("COM{i}")||stem==format!("LPT{i}")){return Err("Tento názov je vyhradený systémom Windows".into())}Ok(value.to_owned())}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    #[test] fn rename_case_only_and_undo_restore_main_and_sidecar(){let root=temp();let(disk,base_manager)=set_up(&root,None);let db=base_manager.db.clone();seed_asset_path(&db,&root);fs::write(root.join("src/a.xmp"),b"side").unwrap();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),base_manager.writer.clone(),false,disk);let result=manager.rename_file(1,"A",|_,_|{}).unwrap();assert!(root.join("src/A.jpg").exists());assert!(root.join("src/A.xmp").exists());assert!(result.summary.contains("a.jpg"));manager.undo_last_change().unwrap();assert!(root.join("src/a.jpg").exists());assert!(root.join("src/a.xmp").exists());assert_eq!(db.lock().unwrap().query_row("SELECT name FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),"a.jpg");let _=fs::remove_dir_all(root);}
    #[test] fn create_folder_undo_removes_only_empty_folder(){let root=temp();let(disk,base_manager)=set_up(&root,None);let db=base_manager.db.clone();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),base_manager.writer.clone(),false,disk);manager.create_folder(root.join("dst").to_str().unwrap(),"new").unwrap();let created=root.join("dst").join("new");assert!(created.is_dir());manager.undo_last_change().unwrap();assert!(!created.exists());let _=fs::remove_dir_all(root);}
    #[test] fn moving_or_copying_into_same_folder_is_a_noop(){let root=temp();let(disk,base_manager)=set_up(&root,None);let db=base_manager.db.clone();seed_asset_path(&db,&root);let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),base_manager.writer.clone(),false,disk);let source=root.join("src/a.jpg");for copy in [false,true]{let result=manager.run_copy_move(&[1],root.join("src").to_str().unwrap(),ConflictPolicy::Cancel,copy,&mut|_,_|{}).unwrap();assert_eq!(result.summary,"Súbory už sú v tomto priečinku");assert_eq!(result.changed_count,0);assert!(source.exists());}assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|row|row.get::<_,i64>(0)).unwrap(),0);let _=fs::remove_dir_all(root);}
    #[test] fn cross_volume_copy_accepts_two_second_mtime_rounding_but_checks_source_exactly(){let root=temp();let(disk,_manager)=set_up(&root,Some(false));disk.copied_mtime_offset.store(-2,Ordering::SeqCst);let source=root.join("src").join("a.jpg");let target=root.join("dst").join("a.jpg");copy_then_remove(disk.as_ref(),&source,&target).unwrap();assert!(!source.exists());assert_eq!(fs::read(&target).unwrap(),b"abc");fs::remove_file(&target).unwrap();fs::write(&source,b"abc").unwrap();disk.copied_mtime_offset.store(3,Ordering::SeqCst);assert!(copy_then_remove(disk.as_ref(),&source,&target).is_err());assert!(source.exists());assert!(!target.exists());let _=fs::remove_dir_all(root);}

    struct TestFs { force_cross:Option<bool>, fail_move:AtomicUsize, move_calls:AtomicUsize, recycle:PathBuf, copied_mtime_offset:AtomicI64, recycled:std::sync::Mutex<std::collections::HashMap<PathBuf,PathBuf>> }
    impl FileSystemOps for TestFs {
        fn exists(&self,p:&Path)->bool{p.exists()}fn is_dir(&self,p:&Path)->bool{p.is_dir()}fn metadata(&self,p:&Path)->io::Result<fs::Metadata>{fs::metadata(p)}
        fn same_volume(&self,a:&Path,b:&Path)->bool{self.force_cross.unwrap_or_else(||volume_key(a)==volume_key(b))}
        fn move_no_replace(&self,a:&Path,b:&Path)->io::Result<()>{let n=self.move_calls.fetch_add(1,Ordering::SeqCst);if n==self.fail_move.load(Ordering::SeqCst){return Err(io::Error::other("simulated failure"))}NativeFileSystem.move_no_replace(a,b)}
        fn copy_no_replace(&self,a:&Path,b:&Path)->io::Result<()>{NativeFileSystem.copy_no_replace(a,b)?;let offset=self.copied_mtime_offset.load(Ordering::SeqCst);if offset!=i64::MIN{let modified=fs::metadata(a)?.modified()?;let rounded=if offset<0{modified.checked_sub(std::time::Duration::from_secs(offset.unsigned_abs())).unwrap()}else{modified.checked_add(std::time::Duration::from_secs(offset as u64)).unwrap()};File::options().write(true).open(b)?.set_times(FileTimes::new().set_modified(rounded))?;}Ok(())}fn remove_file(&self,p:&Path)->io::Result<()>{NativeFileSystem.remove_file(p)}fn create_dir(&self,p:&Path)->io::Result<()>{NativeFileSystem.create_dir(p)}fn remove_dir(&self,p:&Path)->io::Result<()>{NativeFileSystem.remove_dir(p)}
        fn recycle_files(&self,paths:&[PathBuf])->Result<(),String>{for path in paths{let to=self.recycle.join(path.file_name().unwrap());NativeFileSystem.move_no_replace(path,&to).map_err(|e|e.to_string())?;}Ok(())}
        fn recycle_delete(&self,paths:&[PathBuf])->Result<(),String>{for path in paths{let to=self.recycle.join(format!("$R{}-{}",self.move_calls.fetch_add(1,Ordering::SeqCst),path.file_name().unwrap().to_string_lossy()));NativeFileSystem.move_no_replace(path,&to).map_err(|e|e.to_string())?;self.recycled.lock().unwrap().insert(path.clone(),to);}Ok(())}
        fn recycled_item(&self,original:&Path)->Option<PathBuf>{self.recycled.lock().unwrap().get(original).filter(|p|p.exists()).cloned()}
        fn restore_recycled(&self,original:&Path)->Result<(),String>{let data=self.recycled_item(original).ok_or("not in bin")?;NativeFileSystem.move_no_replace(&data,original).map_err(|e|e.to_string())?;self.recycled.lock().unwrap().remove(original);Ok(())}
    }
    fn temp()->PathBuf{let root=std::env::temp_dir().join(format!("ddreams-fileops-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir_all(&root).unwrap();root}
    fn db()->Db{let c=Connection::open_in_memory().unwrap();c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT,action TEXT,summary TEXT,undone INTEGER DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER,table_name TEXT,row_key TEXT,column_name TEXT,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));INSERT INTO folder_roots VALUES(1,'R:','R:\\');INSERT INTO folders VALUES(1,NULL,1,'R:','R:\\');INSERT INTO folders VALUES(2,1,1,'src','R:\\src');INSERT INTO folders VALUES(3,1,1,'dst','R:\\dst');INSERT INTO assets VALUES(1,2,'a.jpg','jpg','jpg',1,3,10,20,24,'cap','notes','author',4,1,'Red','2020-01-01T00:00:00Z','2020-01-02',NULL,0,0,'R:\\src\\a.jpg');INSERT INTO categories VALUES(1,'Cat',NULL,1);INSERT INTO keywords VALUES(1,NULL,'Key');INSERT INTO asset_categories VALUES(1,1);INSERT INTO asset_keywords VALUES(1,1);").unwrap();Arc::new(Mutex::new(c))}
    fn manager(_root:&Path,readonly:bool,fs:Arc<dyn FileSystemOps>)->FileManager{let db=db();let formats=crate::formats::FormatTable::load_selected(None);let listing=ListingService::new(db.clone(),formats);let writer=Arc::new(CatalogWriter::new(db.clone(),!readonly));FileManager::with_fs(db,listing,writer,readonly,fs)}
    fn set_up(root:&Path,force_cross:Option<bool>)->(Arc<TestFs>,FileManager){let src=root.join("src");let dst=root.join("dst");fs::create_dir_all(&src).unwrap();fs::create_dir_all(&dst).unwrap();fs::write(src.join("a.jpg"),b"abc").unwrap();let recycle=root.join("recycle");fs::create_dir_all(&recycle).unwrap();let disk=Arc::new(TestFs{force_cross,fail_move:AtomicUsize::new(usize::MAX),move_calls:AtomicUsize::new(0),recycle,copied_mtime_offset:AtomicI64::new(i64::MIN),recycled:Default::default()});let manager=manager(root,false,disk.clone());(disk,manager)}
    fn seed_asset_path(db:&Db,root:&Path){let path=root.join("src/a.jpg");db.lock().unwrap().execute("UPDATE assets SET path=?1 WHERE id=1",[path.to_string_lossy().to_string()]).unwrap();}

    #[test]fn same_volume_move_sidecars_catalog_and_undo_restore(){let root=temp();let(disk,manager)=set_up(&root,None);let src=root.join("src/a.jpg");fs::write(root.join("src/a.xmp"),b"side").unwrap();let db=manager.db.clone();seed_asset_path(&db,&root);let listed=ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None));let manager=FileManager::with_fs(db.clone(),listed,manager.writer.clone(),false,disk);let result=manager.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();assert_eq!(result.changed_count,1);assert!(!src.exists());assert!(root.join("dst/a.xmp").exists());assert_eq!(db.lock().unwrap().query_row("SELECT path||':'||rating FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),format!("{}:4",root.join("dst").join("a.jpg").display()));manager.undo_last_change().unwrap();assert!(src.exists());assert!(root.join("src/a.xmp").exists());assert_eq!(db.lock().unwrap().query_row("SELECT path FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),src.to_string_lossy());let _=fs::remove_dir_all(root);}
    #[test]fn copy_duplicates_metadata_relations_and_undo_uses_recycle_seam(){let root=temp();let(disk,mut manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);let listing=ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None));manager=FileManager::with_fs(db.clone(),listing,manager.writer.clone(),false,disk.clone());let result=manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();let id=result.remapped_ids[0].new_id;{let c=db.lock().unwrap();assert_eq!(c.query_row("SELECT rating||':'||caption FROM assets WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),"4:cap");assert_eq!(c.query_row("SELECT count(*) FROM asset_categories WHERE asset_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);assert_eq!(c.query_row("SELECT count(*) FROM asset_keywords WHERE asset_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);}manager.undo_last_change().unwrap();assert!(!root.join("dst/a.jpg").exists());assert_eq!(fs::read(disk.recycle.join("a.jpg")).unwrap(),b"abc");assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),0);let _=fs::remove_dir_all(root);}
    #[test]fn copy_undo_refuses_modified_copy_without_partial_change(){let root=temp();let(disk,mut manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk);let result=manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();let id=result.remapped_ids[0].new_id;fs::write(root.join("dst/a.jpg"),b"changed").unwrap();assert!(manager.undo_last_change().unwrap_err().contains("zmenil"));assert!(root.join("dst/a.jpg").exists());assert!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=?1",[id],|r|r.get::<_,i64>(0)).unwrap()==1);let _=fs::remove_dir_all(root);}
    #[test]fn conflicts_skip_keep_both_and_overwrite_refusal(){let root=temp();let(disk,mut manager)=set_up(&root,None);fs::write(root.join("dst/a.jpg"),b"existing").unwrap();let db=manager.db.clone();seed_asset_path(&db,&root);manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk);assert_eq!(manager.preview_conflicts(&[1],root.join("dst").to_str().unwrap(),None).unwrap(),1);let skip=manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Skip,|_,_|{}).unwrap();assert_eq!(skip.skipped_count,1);assert_eq!(fs::read(root.join("dst/a.jpg")).unwrap(),b"existing");manager.copy_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::KeepBoth,|_,_|{}).unwrap();assert_eq!(fs::read(root.join("dst/a (2).jpg")).unwrap(),b"abc");let _=fs::remove_dir_all(root);}
    #[test]fn cross_volume_switch_and_uncatalogued_move_are_testable(){let root=temp();let(disk,manager)=set_up(&root,Some(false));let listed=ListingService::new(manager.db.clone(),crate::formats::FormatTable::load_selected(None));let _=listed;let mut m=manager;let source=root.join("src/a.jpg");let asset=crate::db::Asset{id:-10_000_000,folder_id:None,name:"a.jpg".into(),ext:"jpg".into(),size:Some(3.0),width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:Some(source.to_string_lossy().to_string()),is_directory:false,offline:false,has_categories:false,display_categories:None,display_keywords:None};m.listing.insert_test_asset(asset);m.move_files(&[-10_000_000],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap();assert!(!source.exists());assert!(root.join("dst/a.jpg").exists());assert_eq!(m.db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE path LIKE '%a.jpg'",[],|r|r.get::<_,i64>(0)).unwrap(),1);let _=fs::remove_dir_all(root);}
    #[test]fn simulated_mid_batch_failure_rolls_back_and_readonly_refuses(){let root=temp();let(disk,manager)=set_up(&root,Some(true));fs::write(root.join("src/a.xmp"),b"side").unwrap();seed_asset_path(&manager.db,&root);disk.fail_move.store(1,Ordering::SeqCst);let error=manager.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap_err();assert!(error.contains("simulated failure"));assert!(root.join("src/a.jpg").exists());assert!(root.join("src/a.xmp").exists());assert!(!root.join("dst/a.jpg").exists());assert_eq!(manager.db.lock().unwrap().query_row("SELECT path FROM assets WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),root.join("src/a.jpg").to_string_lossy());let ro=FileManager::with_fs(manager.db.clone(),manager.listing.clone(),manager.writer.clone(),true,disk);assert!(ro.move_files(&[1],root.join("dst").to_str().unwrap(),ConflictPolicy::Cancel,|_,_|{}).unwrap_err().contains("iba na čítanie"));let _=fs::remove_dir_all(root);}
    #[test]fn delete_recycles_main_and_sidecar_removes_rows_and_undo_restores(){let root=temp();let(disk,manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);fs::write(root.join("src/a.xmp"),b"side").unwrap();let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk.clone());
        let before:String=db.lock().unwrap().query_row("SELECT path||':'||coalesce(rating,'') FROM assets WHERE id=1",[],|r|r.get(0)).unwrap();
        let result=manager.delete_files(&[1]).unwrap();assert_eq!(result.summary,"Odstránené do Koša: 1 súbor");assert!(!root.join("src/a.jpg").exists());assert!(!root.join("src/a.xmp").exists());
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        manager.undo_last_change().unwrap();assert_eq!(fs::read(root.join("src/a.jpg")).unwrap(),b"abc");assert_eq!(fs::read(root.join("src/a.xmp")).unwrap(),b"side");
        let after:String=db.lock().unwrap().query_row("SELECT path||':'||coalesce(rating,'') FROM assets WHERE id=1",[],|r|r.get(0)).unwrap();assert_eq!(after,before);let _=fs::remove_dir_all(root);}
    #[test]fn delete_undo_refuses_when_bin_item_missing_or_target_occupied(){let root=temp();let(disk,manager)=set_up(&root,None);let db=manager.db.clone();seed_asset_path(&db,&root);let manager=FileManager::with_fs(db.clone(),ListingService::new(db.clone(),crate::formats::FormatTable::load_selected(None)),manager.writer.clone(),false,disk.clone());
        manager.delete_files(&[1]).unwrap();fs::write(root.join("src/a.jpg"),b"new").unwrap();assert!(manager.undo_last_change().unwrap_err().contains("pôvodnom mieste"));
        fs::remove_file(root.join("src/a.jpg")).unwrap();let bin=disk.recycled_item(&root.join("src/a.jpg")).unwrap();fs::remove_file(bin).unwrap();assert!(manager.undo_last_change().unwrap_err().contains("nie je v Koši"));
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0,"no partial undo");let _=fs::remove_dir_all(root);}
    #[test]fn recycle_info_parser_reads_v1_and_v2(){let path="C:\\Foto\\a.jpg";let units:Vec<u8>=path.encode_utf16().chain(Some(0)).flat_map(|u|u.to_le_bytes()).collect();
        let mut v2=Vec::new();v2.extend(2u64.to_le_bytes());v2.extend(5u64.to_le_bytes());v2.extend(77u64.to_le_bytes());v2.extend(((units.len()/2) as u32).to_le_bytes());v2.extend(&units);assert_eq!(parse_recycle_info(&v2),Some((77,path.to_string())));
        let mut v1=Vec::new();v1.extend(1u64.to_le_bytes());v1.extend(5u64.to_le_bytes());v1.extend(9u64.to_le_bytes());let mut fixed=units.clone();fixed.resize(520,0);v1.extend(fixed);assert_eq!(parse_recycle_info(&v1),Some((9,path.to_string())));assert_eq!(parse_recycle_info(&[0;10]),None);}
}
