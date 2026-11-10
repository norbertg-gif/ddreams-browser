use crate::{
    ai::{AiSearchResult, AiState, AiStatus},
    calendar::{CalendarAggregate, CalendarService, DateSource},
    db::{self, AssetFilter, AssetMetadata, Db},
    display_metadata::{self, CacheEntry, DisplayMetadataCache, TileExifItem, TileExifValues},
    exiftool::ExifTool,
    file_ops::{ConflictPolicy, FileManager, FileOperationResult},
    formats::FormatTable,
    journal::{CatalogWriter, CategoryDeleteInfo, CategoryMutationResult, ChangeSummary, KeywordDeleteInfo, KeywordMutationResult, ListedAsset, SetAssetCategoriesResult, SetAssetKeywordsResult, SetAssetValuesResult},
    listing::ListingService,
    metadata_view::{self, MetadataView},
    paths::AppInfo,
    thumbs::ThumbService,
};
use crate::thumbs;
use serde::Serialize;
use serde_json::Value;
use std::{path::{Path, PathBuf}, sync::Arc};
use tauri::State;
use tauri::Emitter;
use rusqlite::OptionalExtension;

pub struct DatabaseState(pub Db);
pub struct ExifState(pub Arc<ExifTool>);
pub struct ThumbnailState(pub Arc<ThumbService>);
pub struct ListingState(pub Arc<ListingService>);
pub struct FormatsState(pub Arc<FormatTable>);
pub struct MetadataViewState(pub Arc<MetadataView>);
pub struct DisplayMetadataState(pub Arc<DisplayMetadataCache>);
pub struct CalendarState(pub Arc<CalendarService>);
pub struct CatalogWriterState(pub Arc<CatalogWriter>);
pub struct FileManagerState(pub Arc<FileManager>);
pub struct AppInfoState(pub AppInfo);

#[tauri::command(async)]
pub fn ai_status(state: State<'_, AiState>) -> AiStatus { state.0.status() }
#[tauri::command(async)]
pub fn ai_start(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.start() }
#[tauri::command(async)]
pub fn ai_stop(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.stop() }
#[tauri::command(async)]
pub fn ai_pause(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.pause() }
#[tauri::command(async)]
pub fn ai_resume(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.resume() }
#[tauri::command(async)]
pub fn ai_reindex(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.reindex() }
#[tauri::command(async)]
pub fn ai_search_text(state: State<'_, AiState>, query: String, limit: Option<u32>) -> Result<AiSearchResult, String> { state.0.search_text(query, limit) }
#[tauri::command(async)]
pub fn ai_similar(state: State<'_, AiState>, asset_id: i64, limit: Option<u32>) -> Result<AiSearchResult, String> { state.0.similar(asset_id, limit) }
#[tauri::command(async)]
pub fn ai_duplicate_groups(state:State<'_,AiState>,level:String)->Result<Value,String>{state.0.duplicate_groups(&level)}
#[tauri::command(async)]
pub fn ai_duplicates_of(state:State<'_,AiState>,asset_id:i64,level:String)->Result<Vec<i64>,String>{state.0.duplicates_of(asset_id,&level)}
#[tauri::command(async)]
pub fn ai_suggest(state:State<'_,AiState>,asset_ids:Vec<i64>,kind:String)->Result<Value,String>{state.0.suggest(asset_ids,&kind)}
#[tauri::command(async)]
pub fn ai_reject_suggestion(state:State<'_,AiState>,asset_ids:Vec<i64>,kind:String,label_id:i64)->Result<usize,String>{state.0.reject_suggestion(&asset_ids,&kind,label_id)}
#[tauri::command(async)]
pub fn ai_suggestion_groups(state:State<'_,AiState>,kind:String)->Result<Value,String>{state.0.suggestion_groups(&kind)}
#[tauri::command(async)]
pub fn ai_sync_labels(state:State<'_,AiState>,kind:String)->Result<(),String>{if kind!="category"&&kind!="keyword"{return Err("Neplatný typ štítku".into())}state.0.sync_labels_debounced(&kind);Ok(())}

#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct ColorCount { pub value:String,pub name:String,pub hex:String,pub count:usize }
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct AssetColorState { pub value:String,pub name:String,pub hex:String,pub auto:bool,pub effective:bool,pub manual_state:Option<bool> }
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct AssetColors { pub asset_id:i64,pub computed:bool,pub colors:Vec<AssetColorState> }
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct ColorImportResult { pub done:bool,pub matched:usize,pub ambiguous:usize,pub missing:usize }

#[tauri::command(async)]
pub fn get_photosort_color_import_status(db:State<'_,DatabaseState>)->bool{db.0.lock().ok().and_then(|conn|conn.query_row("SELECT EXISTS(SELECT 1 FROM ddreams_meta WHERE key='photosort_colors_imported')",[],|r|r.get(0)).ok()).unwrap_or(false)}

#[tauri::command(async)]
pub fn set_asset_colors(writer:State<'_,CatalogWriterState>,ai:State<'_,AiState>,asset_ids:Vec<i64>,color:String,state:Option<bool>)->Result<usize,String>{let count=writer.0.set_asset_colors(&asset_ids,&color,state,"Oprava farby")?;if count>0{ai.0.learn_colors_debounced();}Ok(count)}

#[tauri::command(async)]
pub fn get_asset_colors(db:State<'_,DatabaseState>,ai:State<'_,AiState>,asset_ids:Vec<i64>)->Result<Vec<AssetColors>,String>{
    let attached=ai.0.ensure_colors_attached();let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;let mut out=Vec::new();let mut seen=std::collections::HashSet::new();
    for id in asset_ids.into_iter().filter(|id|*id>0).filter(|id|seen.insert(*id)).take(500){let listed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)",[id],|r|r.get(0)).map_err(|e|e.to_string())?;if !listed{continue;}
        let auto_json:Option<String>=if attached{conn.query_row("SELECT auto FROM ai.colors WHERE asset_id=?1",[id],|r|r.get(0)).optional().map_err(|e|e.to_string())?}else{None};let auto=auto_json.as_deref().and_then(|v|serde_json::from_str::<Vec<String>>(v).ok()).unwrap_or_default();
        let overrides=conn.prepare("SELECT color,state FROM asset_color_overrides WHERE asset_id=?1").map_err(|e|e.to_string())?.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        let effective=crate::colors::effective_colors(&auto,&overrides);let colors=crate::colors::PALETTE.iter().map(|c|{let manual=overrides.iter().find(|o|o.0==c.value).map(|o|o.1!=0);AssetColorState{value:c.value.into(),name:c.name.into(),hex:c.hex.into(),auto:auto.iter().any(|v|v==c.value),effective:effective.iter().any(|v|v==c.value),manual_state:manual}}).collect();out.push(AssetColors{asset_id:id,computed:auto_json.is_some(),colors});}
    Ok(out)
}

#[tauri::command(async)]
pub fn get_color_counts(db:State<'_,DatabaseState>,ai:State<'_,AiState>)->Result<Vec<ColorCount>,String>{
    let attached=ai.0.ensure_colors_attached();let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;let mut counts=std::collections::HashMap::<String,usize>::new();
    let sql=if attached{"SELECT a.id,c.auto FROM assets a LEFT JOIN ai.colors c ON c.asset_id=a.id WHERE a.is_listed=1 AND a.id>0"}else{"SELECT a.id,NULL FROM assets a WHERE a.is_listed=1 AND a.id>0"};let rows=conn.prepare(sql).map_err(|e|e.to_string())?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    let overrides=conn.prepare("SELECT asset_id,color,state FROM asset_color_overrides").map_err(|e|e.to_string())?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let mut by_id=std::collections::HashMap::<i64,Vec<(String,i64)>>::new();for(id,color,state)in overrides{by_id.entry(id).or_default().push((color,state));}
    for(id,json)in rows{let auto=json.as_deref().and_then(|v|serde_json::from_str::<Vec<String>>(v).ok()).unwrap_or_default();let effective=crate::colors::effective_colors(&auto,by_id.get(&id).map(Vec::as_slice).unwrap_or(&[]));for color in effective{*counts.entry(color).or_default()+=1;}}
    Ok(crate::colors::PALETTE.iter().map(|c|ColorCount{value:c.value.into(),name:c.name.into(),hex:c.hex.into(),count:*counts.get(c.value).unwrap_or(&0)}).collect())
}

#[tauri::command(async)]
pub fn import_photosort_colors(db:State<'_,DatabaseState>,ai:State<'_,AiState>,writer:State<'_,CatalogWriterState>)->Result<ColorImportResult,String>{
    let Some(path)=ai.0.photosort_corrections_path() else{return Err("Photosort learned priečinok nie je dostupný".into())};let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;let done:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM ddreams_meta WHERE key='photosort_colors_imported')",[],|r|r.get(0)).unwrap_or(false);if done{return Ok(ColorImportResult{done:true,matched:0,ambiguous:0,missing:0});}
    let input=std::fs::read_to_string(&path).map_err(|e|format!("Nepodarilo sa načítať opravy Photosortu: {e}"))?;let corrections:serde_json::Map<String,Value>=serde_json::from_str(&input).map_err(|e|e.to_string())?;let rows=conn.prepare("SELECT id,name FROM assets WHERE is_listed=1 AND id>0").map_err(|e|e.to_string())?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;drop(conn);
    let mut imported=Vec::new();let(mut matched,mut ambiguous,mut missing)=(0,0,0);
    for(name,record)in corrections{match crate::colors::unique_name_match(&name,&rows){Ok(Some(id))=>{matched+=1;let colors=record.get("colors").and_then(Value::as_array).map(|v|v.iter().filter_map(Value::as_str).collect::<std::collections::HashSet<_>>()).unwrap_or_default();for c in crate::colors::PALETTE{imported.push((id,c.value.to_string(),colors.contains(c.value)));}},Ok(None)=>missing+=1,Err(())=>ambiguous+=1}}
    writer.0.import_asset_colors(&imported,"Import farieb z photosortu")?;let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;conn.execute("INSERT OR REPLACE INTO ddreams_meta(key,value) VALUES('photosort_colors_imported','1')",[]).map_err(|e|e.to_string())?;drop(conn);ai.0.learn_colors_debounced();Ok(ColorImportResult{done:true,matched,ambiguous,missing})
}

#[tauri::command(async)]
pub fn get_app_info(state: State<'_, AppInfoState>) -> AppInfo { state.0.clone() }

#[tauri::command(async)]
pub fn get_recent_changes(state: State<'_, CatalogWriterState>, limit: i64) -> Result<Vec<ChangeSummary>, String> {
    state.0.recent_changes(limit)
}

#[tauri::command]
pub async fn undo_last_change(state: State<'_, FileManagerState>,ai:State<'_,AiState>) -> Result<Option<ChangeSummary>, String> {
    let manager=state.0.clone();let change=tauri::async_runtime::spawn_blocking(move||manager.undo_last_change()).await.map_err(|e|e.to_string())??;if change.as_ref().is_some_and(|item|item.action=="set_asset_colors"||item.action=="import_photosort_colors"){ai.0.learn_colors_debounced();}if change.as_ref().is_some_and(|item|item.action.contains("categor")||item.action.contains("keyword")){ai.0.sync_labels_debounced("category");ai.0.sync_labels_debounced("keyword");}Ok(change)
}

#[tauri::command(async)]
pub fn get_file_operation_conflicts(state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String)->Result<usize,String>{state.0.preview_conflicts(&asset_ids,&target_folder_path,None)}
#[tauri::command]
pub async fn move_files(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String,conflict:ConflictPolicy)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.move_files(&asset_ids,&target_folder_path,conflict,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Presúvam".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command]
pub async fn copy_files(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String,conflict:ConflictPolicy)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.copy_files(&asset_ids,&target_folder_path,conflict,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Kopírujem".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command]
pub async fn rename_file(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_id:i64,new_base_name:String)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.rename_file(asset_id,&new_base_name,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Premenúvam".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command]
pub async fn create_folder(state:State<'_,FileManagerState>,parent_path:String,name:String)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.create_folder(&parent_path,&name)).await.map_err(|e|e.to_string())?}

#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct FileProgress{pub operation:String,pub done:usize,pub total:usize}

#[tauri::command(async)]
pub fn set_asset_values(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    asset_ids: Vec<i64>,
    changes: serde_json::Map<String, Value>,
) -> Result<SetAssetValuesResult, String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let asset = listing.0.asset(id).ok_or_else(|| format!("Listed asset {id} is no longer available"))?;
        if asset.is_directory || asset.ext.is_empty() { continue; }
        let path = asset.path.ok_or_else(|| format!("Path for listed asset {id} is unavailable"))?;
        let folder_path = Path::new(&path).parent().map(|parent| parent.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;
        candidates.push(ListedAsset { temporary_id: id, name: asset.name, ext: asset.ext, size: asset.size, file_modified: asset.file_modified, path, folder_path });
        file_ids.push(id);
    }
    let result = state.0.set_asset_values(&file_ids, &candidates, &changes)?;
    for id in asset_ids.iter().copied().filter(|id| *id >= 0) { let _ = listing.0.invalidate_asset_parent(id); }
    for candidate in candidates { let _ = listing.0.invalidate_path(&candidate.folder_path); }
    Ok(result)
}

#[tauri::command(async)]
pub fn set_asset_categories(
    state: State<'_, CatalogWriterState>, listing: State<'_, ListingState>, ai:State<'_,AiState>, asset_ids: Vec<i64>, category_id: i64, assign: bool,
) -> Result<SetAssetCategoriesResult, String> {
    let mut candidates=Vec::new();let mut file_ids=asset_ids.iter().copied().filter(|id|*id>=0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id|*id<0){
        let Some(asset)=listing.0.asset(id) else { continue; };
        if asset.is_directory||asset.ext.is_empty(){continue;}
        let path=asset.path.ok_or_else(||format!("Listed file {id} has no resolved path"))?;
        let folder_path=Path::new(&path).parent().map(|p|p.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;
        candidates.push(ListedAsset{temporary_id:id,name:asset.name,ext:asset.ext,size:asset.size,file_modified:asset.file_modified,path,folder_path});file_ids.push(id);
    }
    let result=state.0.set_asset_categories(&file_ids,&candidates,category_id,assign)?;
    if result.changed_count>0{ai.0.sync_labels_debounced("category");}
    for id in file_ids.iter().copied().filter(|id|*id>=0){let _=listing.0.invalidate_asset_parent(id);}
    for candidate in candidates{let _=listing.0.invalidate_path(&candidate.folder_path);}
    Ok(result)
}

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct CategorySelectionCounts { pub total_files:usize, pub assigned:std::collections::HashMap<i64,usize> }

#[tauri::command(async)]
pub fn get_category_selection_counts(db:State<'_,DatabaseState>,listing:State<'_,ListingState>,asset_ids:Vec<i64>)->Result<CategorySelectionCounts,String>{
    let mut positive=Vec::new();let mut total=0usize;
    for id in asset_ids{if id<0{if listing.0.asset(id).is_some_and(|a|!a.is_directory&&!a.ext.is_empty()){total+=1;}}else{positive.push(id);}}
    let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
    for chunk in positive.chunks(500){let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT count(*) FROM assets WHERE is_listed=1 AND ext_lower<>'' AND id IN ({marks})");total+=conn.query_row(&sql,rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? as usize;}
    let mut assigned=std::collections::HashMap::new();
    for chunk in positive.chunks(500){if chunk.is_empty(){continue;}let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT ac.category_id,count(DISTINCT ac.asset_id) FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1 AND a.ext_lower<>'' AND ac.asset_id IN ({marks}) GROUP BY ac.category_id");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let rows=stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).map_err(|e|e.to_string())?;for row in rows{let(id,count)=row.map_err(|e|e.to_string())?;*assigned.entry(id).or_insert(0)+=count as usize;}}
    Ok(CategorySelectionCounts{total_files:total,assigned})
}

#[tauri::command(async)]
pub fn get_keyword_selection_counts(db:State<'_,DatabaseState>,listing:State<'_,ListingState>,asset_ids:Vec<i64>)->Result<CategorySelectionCounts,String>{
    let mut positive=Vec::new();let mut total=0usize;for id in asset_ids{if id<0{if listing.0.asset(id).is_some_and(|a|!a.is_directory&&!a.ext.is_empty()){total+=1;}}else{positive.push(id);}}
    let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;for chunk in positive.chunks(500){if chunk.is_empty(){continue;}let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT count(*) FROM assets WHERE is_listed=1 AND ext_lower<>'' AND id IN ({marks})");total+=conn.query_row(&sql,rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? as usize;}
    let mut assigned=std::collections::HashMap::new();for chunk in positive.chunks(500){if chunk.is_empty(){continue;}let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT ak.keyword_id,count(DISTINCT ak.asset_id) FROM asset_keywords ak JOIN assets a ON a.id=ak.asset_id WHERE a.is_listed=1 AND a.ext_lower<>'' AND ak.asset_id IN ({marks}) GROUP BY ak.keyword_id");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let rows=stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).map_err(|e|e.to_string())?;for row in rows{let(id,count)=row.map_err(|e|e.to_string())?;*assigned.entry(id).or_insert(0)+=count as usize;}}
    Ok(CategorySelectionCounts{total_files:total,assigned})
}

#[tauri::command(async)]
pub fn create_category(state:State<'_,CatalogWriterState>,parent_id:Option<i64>,name:String)->Result<CategoryMutationResult,String>{state.0.create_category(parent_id,&name)}
#[tauri::command(async)]
pub fn rename_category(state:State<'_,CatalogWriterState>,id:i64,name:String)->Result<CategoryMutationResult,String>{state.0.rename_category(id,&name)}
#[tauri::command(async)]
pub fn move_category(state:State<'_,CatalogWriterState>,id:i64,new_parent_id:Option<i64>)->Result<CategoryMutationResult,String>{state.0.move_category(id,new_parent_id)}
#[tauri::command(async)]
pub fn get_category_delete_info(state:State<'_,CatalogWriterState>,id:i64)->Result<CategoryDeleteInfo,String>{state.0.category_delete_info(id)}
#[tauri::command(async)]
pub fn delete_category(state:State<'_,CatalogWriterState>,id:i64)->Result<CategoryMutationResult,String>{state.0.delete_category(id)}

#[tauri::command(async)]
pub fn set_asset_keywords(state:State<'_,CatalogWriterState>,listing:State<'_,ListingState>,ai:State<'_,AiState>,asset_ids:Vec<i64>,keyword_id:i64,assign:bool)->Result<SetAssetKeywordsResult,String>{
    let mut candidates=Vec::new();let mut file_ids=asset_ids.iter().copied().filter(|id|*id>=0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id|*id<0){let Some(asset)=listing.0.asset(id)else{continue};if asset.is_directory||asset.ext.is_empty(){continue;}let path=asset.path.ok_or_else(||format!("Listed file {id} has no resolved path"))?;let folder_path=Path::new(&path).parent().map(|p|p.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;candidates.push(ListedAsset{temporary_id:id,name:asset.name,ext:asset.ext,size:asset.size,file_modified:asset.file_modified,path,folder_path});file_ids.push(id);}
    let result=state.0.set_asset_keywords(&file_ids,&candidates,keyword_id,assign)?;if result.changed_count>0{ai.0.sync_labels_debounced("keyword");}for id in file_ids.iter().copied().filter(|id|*id>=0){let _=listing.0.invalidate_asset_parent(id);}for candidate in candidates{let _=listing.0.invalidate_path(&candidate.folder_path);}Ok(result)
}
#[tauri::command(async)]
pub fn create_and_assign_keyword(state:State<'_,CatalogWriterState>,listing:State<'_,ListingState>,asset_ids:Vec<i64>,parent_id:Option<i64>,name:String)->Result<SetAssetKeywordsResult,String>{
    let mut candidates=Vec::new();let mut file_ids=asset_ids.iter().copied().filter(|id|*id>=0).collect::<Vec<_>>();for id in asset_ids.iter().copied().filter(|id|*id<0){let Some(asset)=listing.0.asset(id)else{continue};if asset.is_directory||asset.ext.is_empty(){continue;}let path=asset.path.ok_or_else(||format!("Listed file {id} has no resolved path"))?;let folder_path=Path::new(&path).parent().map(|p|p.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;candidates.push(ListedAsset{temporary_id:id,name:asset.name,ext:asset.ext,size:asset.size,file_modified:asset.file_modified,path,folder_path});file_ids.push(id);}
    let result=state.0.create_and_assign_keyword(&file_ids,&candidates,parent_id,&name)?;for id in file_ids.iter().copied().filter(|id|*id>=0){let _=listing.0.invalidate_asset_parent(id);}for candidate in candidates{let _=listing.0.invalidate_path(&candidate.folder_path);}Ok(result)
}
#[tauri::command(async)]
pub fn create_keyword(state:State<'_,CatalogWriterState>,parent_id:Option<i64>,name:String)->Result<KeywordMutationResult,String>{state.0.create_keyword(parent_id,&name)}
#[tauri::command(async)]
pub fn rename_keyword(state:State<'_,CatalogWriterState>,id:i64,name:String)->Result<KeywordMutationResult,String>{state.0.rename_keyword(id,&name)}
#[tauri::command(async)]
pub fn move_keyword(state:State<'_,CatalogWriterState>,id:i64,new_parent_id:Option<i64>)->Result<KeywordMutationResult,String>{state.0.move_keyword(id,new_parent_id)}
#[tauri::command(async)]
pub fn get_keyword_delete_info(state:State<'_,CatalogWriterState>,id:i64)->Result<KeywordDeleteInfo,String>{state.0.keyword_delete_info(id)}
#[tauri::command(async)]
pub fn delete_keyword(state:State<'_,CatalogWriterState>,id:i64)->Result<KeywordMutationResult,String>{state.0.delete_keyword(id)}
#[tauri::command(async)]
pub fn search_keyword_options(state:State<'_,DatabaseState>,query:String)->Result<Vec<db::KeywordOption>,String>{db::keyword_options(&state.0,&query).map_err(|e|e.to_string())}

#[tauri::command(async)]
pub fn get_calendar_aggregate(state: State<'_, CalendarState>, source: String) -> Result<CalendarAggregate, String> {
    state.0.aggregate(DateSource::parse(&source)?).map(|value| (*value).clone())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataRow {
    pub group: String,
    pub subgroup: String,
    pub name: String,
    pub value: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataResponse {
    pub asset: db::Asset,
    pub offline: bool,
    pub rows: Vec<MetadataRow>,
}

#[tauri::command(async)]
pub fn get_assets(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    ai: State<'_, AiState>,
    writer: State<'_, CatalogWriterState>,
    filter: AssetFilter,
) -> Result<db::AssetPage, String> {
    // Opening an included folder catalogues its new photos/videos (the user's workflow: new photos land in
    // one inbox folder). Only on the first page, so scrolling doesn't re-check; errors never block browsing.
    if filter.kind == "folder" && filter.offset == 0 { let _ = auto_catalog_folder(&listing.0, &formats.0, &writer.0, &ai.0, &filter); }
    if filter.sort == "similarity" {
        let mut base=filter.clone();base.sort="name".into();base.descending=false;base.offset=0;base.limit=500;
        let mut all=Vec::new();let mut offset=0;let mut total=0;let mut total_size=0.0;
        loop {base.offset=offset;let page=if filter.kind=="folder"{listing.0.page(&base)?}else{db::assets_page(&state.0,&base,&formats.0).map_err(|e|e.to_string())?};total=page.total;total_size=page.total_size;let count=page.items.len();all.extend(page.items);offset+=count as i64;if count<500||offset>=total{break}}
        let order=if ai.0.status().available{Some(ai.0.order_by_similarity(all.iter().filter(|a|!a.is_directory).map(|a|a.id).collect())?)}else{None};let all=apply_similarity_order(all,order.as_deref());let items=all.into_iter().skip(filter.offset.max(0) as usize).take(filter.limit.clamp(1,500) as usize).collect();return Ok(db::AssetPage{items,total,total_size});
    }
    if filter.kind == "folder" { return listing.0.page(&filter); }
    db::assets_page(&state.0, &filter, &formats.0).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_folder_previews(state: State<'_, DatabaseState>, formats: State<'_, FormatsState>, paths: Vec<String>) -> Result<std::collections::HashMap<String, Vec<db::FolderPreviewAsset>>, String> {
    db::folder_previews(&state.0, &paths, &formats.0).map_err(|e| e.to_string())
}
fn auto_catalog_folder(listing:&ListingService,formats:&crate::formats::FormatTable,writer:&crate::journal::CatalogWriter,ai:&crate::ai::AiService,filter:&AssetFilter)->Result<usize,String>{
    let (path,files)=listing.uncatalogued_files(filter)?;
    if path.is_empty()||files.is_empty()||writer.effective_folder_mode(&path).as_deref()!=Some("include"){return Ok(0);}
    let candidates=files.into_iter().filter(|a|matches!(formats.group(&a.ext),Some("image"|"raw"|"video"))).filter_map(|a|{let file_path=a.path.clone()?;let folder_path=Path::new(&file_path).parent().map(|p|p.to_string_lossy().to_string()).unwrap_or_default();Some(ListedAsset{temporary_id:a.id,name:a.name,ext:a.ext,size:a.size,file_modified:a.file_modified,path:file_path,folder_path})}).collect::<Vec<_>>();
    if candidates.is_empty(){return Ok(0);}
    let remaps=writer.catalog_listed_assets(&candidates)?;
    if !remaps.is_empty(){let _=listing.invalidate_path(&path);ai.index_assets(&remaps.iter().map(|r|r.new_id).collect::<Vec<_>>());}
    Ok(remaps.len())
}
#[tauri::command(async)]
pub fn refresh_folder_listing(listing: State<'_, ListingState>, folder_id: i64) -> Result<(), String> {
    listing.0.invalidate(folder_id)
}
#[tauri::command(async)]
pub fn get_asset_paths(db: State<'_, DatabaseState>, listing: State<'_, ListingState>, ids: Vec<i64>) -> Result<Vec<String>, String> {
    Ok(ids.into_iter().filter_map(|id| listing.0.asset(id).and_then(|a|a.path).or_else(||db::asset_metadata(&db.0,id).ok().flatten().and_then(|m|m.asset.path))).collect())
}
#[tauri::command(async)]
pub fn asset_action(db: State<'_, DatabaseState>, listing: State<'_, ListingState>, asset_id: i64, action: String) -> Result<(), String> {
    let path = listing.0.asset(asset_id).and_then(|a|a.path).or_else(||db::asset_metadata(&db.0,asset_id).ok().flatten().and_then(|m|m.asset.path)).ok_or("Cesta súboru sa nenašla")?;
    if !Path::new(&path).exists() { return Err("Súbor je offline".into()); }
    let mut command=std::process::Command::new("explorer.exe");
    match action.as_str() {
        "open" => { command.arg(&path); }
        "reveal" => {
            use std::os::windows::process::CommandExt;
            let escaped=path.replace('"',"\"\"");command.raw_arg(format!("/select,\"{escaped}\""));
        }
        _ => return Err("Neznáma akcia".into()),
    }
    command.spawn().map(|_|()).map_err(|e|e.to_string())
}
/// Opens the given files in an external editor chosen in Settings. `multi` = the program takes all
/// files in one launch; otherwise it is started once per file (at most 20).
#[tauri::command(async)]
pub fn open_in_editor(db: State<'_, DatabaseState>, listing: State<'_, ListingState>, asset_ids: Vec<i64>, exe: String, multi: bool) -> Result<usize, String> {
    let exe = exe.trim();
    if !Path::new(exe).is_file() { return Err(format!("Program sa nenašiel: {exe}")); }
    let paths: Vec<String> = asset_ids.into_iter()
        .filter_map(|id| listing.0.asset(id).and_then(|a|a.path).or_else(||db::asset_metadata(&db.0,id).ok().flatten().and_then(|m|m.asset.path)))
        .filter(|p| Path::new(p).is_file()).collect();
    if paths.is_empty() { return Err("Žiadny z vybraných súborov nie je dostupný".into()); }
    if multi {
        std::process::Command::new(exe).args(&paths).spawn().map_err(|e| e.to_string())?;
    } else {
        if paths.len() > 20 { return Err("Tento program berie súbory po jednom – vyber najviac 20 súborov".into()); }
        for path in &paths { std::process::Command::new(exe).arg(path).spawn().map_err(|e| e.to_string())?; }
    }
    Ok(paths.len())
}

/// Windows file dialog for choosing an editor program. Returns None when cancelled.
#[tauri::command(async)]
pub fn pick_editor_program() -> Result<Option<String>, String> {
    use std::os::windows::process::CommandExt;
    let script = "Add-Type -AssemblyName System.Windows.Forms; $o=New-Object System.Windows.Forms.Form -Property @{TopMost=$true;ShowInTaskbar=$false;Opacity=0}; $d=New-Object System.Windows.Forms.OpenFileDialog; $d.Filter='Programy (*.exe)|*.exe'; $d.Title='Vyber program'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; if($d.ShowDialog($o) -eq 'OK'){[Console]::Write($d.FileName)}";
    let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", script]).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(if path.is_empty() { None } else { Some(path) })
}

#[tauri::command(async)]
pub fn program_exists(path: String) -> bool { Path::new(path.trim()).is_file() }

#[tauri::command(async)]
pub fn get_asset_ids(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    filter: AssetFilter,
    offset: i64,
    limit: i64,
) -> Result<Vec<i64>, String> {
    if filter.kind == "folder" { let mut f=filter; f.offset=offset; f.limit=limit; return Ok(listing.0.page(&f)?.items.into_iter().map(|a|a.id).collect()); }
    db::asset_ids_page(&state.0, &filter, &formats.0, offset, limit).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn get_folder_children(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    parent_id: Option<i64>,
    parent_ids: Option<Vec<i64>>,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Folder>, String> {
    if let Some(parent)=parent_id.filter(|id| *id != -2) {
        let path=listing.0.asset(parent).and_then(|a|a.path).or_else(||db::folder_ancestors(&state.0,parent).ok()?.last().and_then(|f|f.path.clone()));
        if let Some(path)=path {
            let filter=AssetFilter{kind:"folder".into(),id:Some(parent),root_id:None,include_children:false,folder_path:Some(path),ids:parent_ids.clone(),value:None,query:None,sort:"name".into(),descending:false,offset:0,limit:500,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group:None};
            let page=listing.0.page(&filter)?;
            let mut dirs=Vec::new();
            for asset in page.items.into_iter().filter(|a|a.is_directory) {
                let matched=asset.path.as_deref().and_then(|p|db::folder_by_path(&state.0,p).ok().flatten());
                let id=matched.as_ref().map(|f|f.id).unwrap_or(asset.id);
                dirs.push(db::Folder{id,parent_id:Some(parent),root_id:matched.as_ref().and_then(|f|f.root_id),name:asset.name,path:asset.path,count:matched.as_ref().map(|f|f.count).unwrap_or(0),folder_ids:matched.map(|f|f.folder_ids).unwrap_or_else(||vec![id]),virtual_node:None,offline:false});
            }
            let fallback=db::folders_page(&state.0,Some(parent),parent_ids.clone(),0,limit.clamp(1,500)).map_err(|e|e.to_string())?;
            let mut seen=dirs.iter().filter_map(|f|f.path.as_deref()).map(|p|p.to_lowercase()).collect::<std::collections::HashSet<_>>();
            for mut folder in fallback { if !folder.path.as_deref().is_some_and(|p|seen.contains(&p.to_lowercase())) { folder.offline=true; seen.insert(folder.path.as_deref().unwrap_or("").to_lowercase()); dirs.push(folder); } }
            dirs.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return Ok(dirs.into_iter().skip(offset.max(0) as usize).take(limit.clamp(1,500) as usize).collect());
        }
    }
    db::folders_page(&state.0, parent_id, parent_ids, offset, limit).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn find_folder_path(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    path: String,
) -> Result<Option<db::Folder>, String> {
    if let Some(folder)=db::folder_by_path(&state.0,&path).map_err(|e|e.to_string())? { return Ok(Some(folder)); }
    listing.0.resolve_folder(&path)
}
#[tauri::command(async)]
pub fn get_folder_ancestors(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    id: i64,
) -> Result<Vec<db::Folder>, String> {
    let _=state;
    listing.0.folder_ancestors(id)
}
#[tauri::command(async)]
pub fn get_category_roots(
    state: State<'_, DatabaseState>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Root>, String> {
    db::roots_page(&state.0, offset, limit, include_children).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn get_catalog_nodes(
    state: State<'_, DatabaseState>,
    kind: String,
    parent_id: Option<i64>,
    root_id: Option<i64>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::nodes_page(&state.0, &kind, parent_id, root_id, offset, limit, include_children).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn search_keywords(
    state: State<'_, DatabaseState>,
    query: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::keyword_search_page(&state.0, &query, offset, limit).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn search_catalog_tree(state: State<'_, DatabaseState>, kind:String, query:String, include_children:bool, offset:i64, limit:i64)->Result<Vec<db::CatalogNode>,String>{
    db::catalog_tree_search(&state.0,&kind,&query,offset,limit,include_children).map_err(|e|e.to_string())
}
#[tauri::command(async)]
pub fn get_catalog_values(
    state: State<'_, DatabaseState>,
    formats: State<'_, FormatsState>,
    kind: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::ValueCount>, String> {
    db::values_page(&state.0, &kind, offset, limit, &formats.0).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn set_thumbnail_epoch(state: State<'_, ThumbnailState>, epoch: u64) {
    state.0.set_epoch(epoch)
}
#[tauri::command(async)]
pub fn get_thumbnail_state(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> thumbs::ThumbnailState {
    state.0.state(asset_id, epoch)
}

fn db_tags(data: &AssetMetadata) -> Vec<metadata_view::RawTag> {
    let a = &data.asset;
    let mut tags = Vec::new();
    let mut add = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            tags.push(metadata_view::RawTag { key: format!("Db:{name}"), group: "Db".into(), name: name.into(), value });
        }
    };
    add("Caption", a.caption.clone()); add("Notes", a.notes.clone()); add("Author", a.author.clone());
    add("Rating", Some(a.rating.filter(|v| *v > 0).map(|v| v.to_string()).unwrap_or_else(|| "0".into())));
    add("Label", Some(a.label.clone().unwrap_or_else(|| "-".into())));
    add("Tagged", Some(if a.tagged.unwrap_or(0) != 0 { "Áno" } else { "Nie" }.into()));
    add("Categories", (!data.categories.is_empty()).then(||data.categories.join(", ")));
    add("Keywords", (!data.keywords.is_empty()).then(||data.keywords.join(", ")));
    add("Date taken", a.exif_date.clone());
    add("Dimensions", match (a.width,a.height) {(Some(w),Some(h))=>Some(format!("{w} × {h}")),_=>None});
    add("Bits per pixel", a.bpp.map(|v| v.to_string()));
    add("File modified", a.file_modified.clone());
    tags
}

fn apply_similarity_order(mut assets:Vec<db::Asset>,order:Option<&[i64]>)->Vec<db::Asset>{
    let mut dirs=assets.iter().filter(|a|a.is_directory).cloned().collect::<Vec<_>>();let mut files=assets.drain(..).filter(|a|!a.is_directory).collect::<Vec<_>>();dirs.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    if let Some(order)=order{let positions=order.iter().enumerate().map(|(i,id)|(*id,i)).collect::<std::collections::HashMap<_,_>>();files.sort_by_key(|a|positions.get(&a.id).copied().unwrap_or(usize::MAX));}else{files.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));}
    dirs.extend(files);dirs
}

#[cfg(test)]
mod similarity_order_tests{
    use super::*;
    fn asset(id:i64,name:&str)->db::Asset{db::Asset{id,folder_id:None,name:name.into(),ext:"jpg".into(),size:None,width:None,height:None,bpp:None,caption:None,notes:None,author:None,rating:None,tagged:None,label:None,file_modified:None,exif_date:None,acdsee_date:None,md_dirty:None,is_orphan:None,path:None,is_directory:false,offline:false,has_categories:false,display_categories:None,display_keywords:None}}
    #[test]fn similarity_order_pages_from_cached_ids(){let all=vec![asset(1,"c"),asset(2,"a"),asset(3,"b")];let ordered=apply_similarity_order(all,Some(&[3,1,2]));let ids=ordered.into_iter().map(|a|a.id).collect::<Vec<_>>();assert_eq!(&ids[0..2],&[3,1]);assert_eq!(&ids[2..3],&[2]);}
    #[test]fn unavailable_ai_falls_back_to_name_order(){let all=vec![asset(1,"c"),asset(2,"a"),asset(3,"b")];let ids=apply_similarity_order(all,None).into_iter().map(|a|a.id).collect::<Vec<_>>();assert_eq!(ids,vec![2,3,1]);}
}
fn live_rows(json: Value, view: &MetadataView, data: Option<&AssetMetadata>) -> Vec<MetadataRow> {
    let Some(obj) = json.as_object() else { return Vec::new(); };
    let tags = metadata_view::raw_tags(obj);
    rows_from_curated(metadata_view::curate(view, &tags, &data.map(db_tags).unwrap_or_default()))
}
fn db_only_rows(view: &MetadataView, data: &AssetMetadata) -> Vec<MetadataRow> {
    rows_from_curated(metadata_view::curate(view, &[], &db_tags(data)))
}
fn rows_from_curated(rows: Vec<metadata_view::CuratedRow>) -> Vec<MetadataRow> {
    rows.into_iter().map(|row|MetadataRow{group:row.group,subgroup:row.subgroup,name:row.name,value:row.value}).collect()
}
#[tauri::command(async)]
pub fn get_metadata(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    exif: State<'_, ExifState>,
    view: State<'_, MetadataViewState>,
    asset_id: i64,
) -> Result<Option<MetadataResponse>, String> {
    if let Some(asset)=listing.0.asset(asset_id) {
        let path=asset.path.clone().unwrap_or_default();
        let online=Path::new(&path).is_file();
        let rows=if online { exif.0.metadata(Path::new(&path)).map(|json|live_rows(json,&view.0,None)).unwrap_or_default() } else { Vec::new() };
        return Ok(Some(MetadataResponse{asset,offline:!online,rows}));
    }
    let Some(data) = db::asset_metadata(&db.0, asset_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = data.asset.path.as_deref().unwrap_or("");
    let online = Path::new(path).is_file();
    if online {
        if let Ok(value) = exif.0.metadata(Path::new(path)) {
            let rows = live_rows(value, &view.0, Some(&data));
            if !rows.is_empty() {
                return Ok(Some(MetadataResponse {
                    asset: data.asset,
                    offline: false,
                    rows,
                }));
            }
        }
    }
    let rows = db_only_rows(&view.0, &data);
    Ok(Some(MetadataResponse {
        asset: data.asset,
        offline: !online,
        rows,
    }))
}
#[tauri::command(async)]
pub fn get_catalog_asset_metadata(state:State<'_,DatabaseState>,asset_id:i64)->Result<Option<AssetMetadata>,String>{
    db::asset_metadata(&state.0,asset_id).map_err(|e|e.to_string())
}

#[tauri::command(async)]
pub fn get_thumbnail_metadata(
    db: State<'_, DatabaseState>, listing: State<'_, ListingState>, exif: State<'_, ExifState>,
    cache: State<'_, DisplayMetadataState>, asset_ids: Vec<i64>, fields: Vec<String>,
) -> Result<Vec<TileExifItem>, String> {
    #[derive(Clone)] struct Candidate { id:i64, path:PathBuf, key:String, entry:CacheEntry, missing:Vec<String> }
    let requested=display_metadata::allowed_fields(&fields);
    if requested.is_empty() { return Ok(asset_ids.into_iter().map(|asset_id|TileExifItem{asset_id,values:TileExifValues::default()}).collect()); }
    let mut candidates=Vec::new();
    for id in asset_ids {
        let (path,date,is_directory)=if let Some(asset)=listing.0.asset(id){(asset.path,asset.exif_date,asset.is_directory)}else{
            let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
            conn.query_row("SELECT path,exif_date FROM assets WHERE id=? AND is_listed=1",[id],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?))).map(|(p,d)|(p,d,false)).unwrap_or((None,None,false))
        };
        let Some(path)=path.filter(|_|!is_directory) else { candidates.push(Candidate{id,path:PathBuf::new(),key:String::new(),entry:CacheEntry::default(),missing:Vec::new()});continue; };
        let path=PathBuf::from(path);
        if !path.is_file(){candidates.push(Candidate{id,path,key:String::new(),entry:CacheEntry::default(),missing:Vec::new()});continue;}
        let (key,_,_)=display_metadata::file_key(&path).map_err(|e|e.to_string())?;
        let entry=cache.0.get(&key);
        let missing=display_metadata::missing_fields(&entry,&requested,date.as_deref());
        candidates.push(Candidate{id,path,key,entry,missing});
    }
    let query_fields=requested.iter().filter(|field|candidates.iter().any(|c|c.missing.contains(field))).cloned().collect::<Vec<_>>();
    if !query_fields.is_empty(){
        let targets=candidates.iter().enumerate().filter(|(_,c)|!c.missing.is_empty()&&!c.key.is_empty()).map(|(i,_)|i).collect::<Vec<_>>();
        let paths=targets.iter().map(|i|candidates[*i].path.clone()).collect::<Vec<_>>();
        let tags=display_metadata::tags_for(&query_fields);
        let records=exif.0.metadata_batch(&paths,&tags).map_err(|e|e.to_string())?;
        for (idx,record) in targets.into_iter().zip(records.into_iter()){
            let candidate=&mut candidates[idx];
            if let Some(object)=record.as_object(){let values=display_metadata::parse_values(object,&query_fields);display_metadata::merge_values(&mut candidate.entry.values,values);}
            display_metadata::mark_loaded(&mut candidate.entry,&query_fields);
            cache.0.put(&candidate.key,candidate.entry.clone());
        }
    }
    Ok(candidates.into_iter().map(|candidate|TileExifItem{asset_id:candidate.id,values:candidate.entry.values}).collect())
}
#[tauri::command(async)]
pub fn get_catalog_asset_links(state:State<'_,DatabaseState>,asset_id:i64)->Result<Option<db::CatalogAssetLinks>,String>{
    db::asset_catalog_links(&state.0,asset_id).map_err(|e|e.to_string())
}

#[tauri::command(async)]
pub fn get_formats(state: State<'_, FormatsState>) -> FormatTable { state.0.as_ref().clone() }

#[tauri::command(async)]
pub fn get_preview_source(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> Result<String, String> {
    state.0.render(asset_id, 0, epoch).map(|result| result.source).map_err(|e| format!("{e:?}"))
}

#[tauri::command(async)]
pub fn folder_exists(path:String)->bool{Path::new(path.trim()).is_dir()}

#[tauri::command(async)]
pub fn get_delete_info(state:State<'_,FileManagerState>,asset_ids:Vec<i64>,permanent:bool)->Result<crate::file_ops::DeleteInfo,String>{state.0.delete_info(&asset_ids,permanent)}

/// Deletes the selected files (with sidecars): to the Recycle Bin (undoable) or permanently when
/// `permanent` (Shift+Delete) or the drive has no Recycle Bin.
#[tauri::command]
pub async fn delete_files(state:State<'_,FileManagerState>,ai:State<'_,AiState>,asset_ids:Vec<i64>,permanent:Option<bool>)->Result<FileOperationResult,String>{let manager=state.0.clone();let (result,purged)=tauri::async_runtime::spawn_blocking(move||manager.delete_files(&asset_ids,permanent.unwrap_or(false))).await.map_err(|e|e.to_string())??;ai.0.forget(purged);Ok(result)}

#[tauri::command(async)]
pub fn get_folder_rules(writer:State<'_,CatalogWriterState>)->Result<Vec<crate::folder_rules::FolderRule>,String>{writer.0.folder_rules()}

#[tauri::command(async)]
pub fn get_folder_rule_state(writer:State<'_,CatalogWriterState>,path:String)->Result<crate::folder_rules::FolderRuleState,String>{writer.0.folder_rule_state(&path)}

#[tauri::command(async)]
pub fn get_exclusion_impact(writer:State<'_,CatalogWriterState>,path:String)->Result<crate::journal::ExclusionImpact,String>{writer.0.exclusion_impact(&path)}

/// mode: "include" | "exclude" | null (remove the explicit rule). Excluding removes the folder's catalogue rows (undoable).
#[tauri::command(async)]
pub fn set_folder_rule(writer:State<'_,CatalogWriterState>,ai:State<'_,AiState>,listing:State<'_,ListingState>,path:String,mode:Option<String>)->Result<usize,String>{
    let removed=writer.0.set_folder_rule(&path,mode.as_deref())?;
    let count=removed.len();
    if count>0{ai.0.forget(removed);}
    let _=listing.0.invalidate_path(&path);
    Ok(count)
}
