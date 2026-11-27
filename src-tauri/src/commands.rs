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
use tauri::{Emitter, Manager};
use rusqlite::OptionalExtension;

pub struct DatabaseState(pub Db);
pub struct AutoExifState(pub Arc<crate::exif_index::ExifIndex>);
#[tauri::command(async)]
pub fn auto_categories(state: State<'_, AutoExifState>) -> Result<(Vec<crate::exif_index::Group>, crate::exif_index::Progress), String> { state.0.snapshot() }
/// Second, read-only connection for side-panel data (counts, trees, folder previews, calendar).
/// WAL lets it read while the main connection serves the folder listing, so background loading
/// never makes the grid wait for the catalogue lock.
pub struct BackgroundDbState(pub Db);
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

fn path_volume(path:&str)->String{let p=path.replace('/',"\\");if p.starts_with("\\\\"){let mut parts=p.trim_start_matches('\\').split('\\');let server=parts.next().unwrap_or("");let share=parts.next().unwrap_or("");return format!("\\\\{server}\\{share}")}if p.as_bytes().get(1)==Some(&b':'){return p[..3.min(p.len())].to_string()}"\\".into()}
fn classify_source_with(path:&str,cache:&mut std::collections::HashMap<String,bool>,mut exists:impl FnMut(&str)->bool)->(bool,bool){let volume=path_volume(path);let reachable=*cache.entry(volume.clone()).or_insert_with(||exists(&volume));if !reachable{(true,false)}else{(false,!exists(path))}}
fn classify_source(path:&str,cache:&mut std::collections::HashMap<String,bool>)->(bool,bool){classify_source_with(path,cache,|p|Path::new(p).exists())}
fn require_missing_source(offline:bool,missing:bool)->Result<(),String>{if offline||missing{Ok(())}else{Err("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie".into())}}
fn tree_children_from_listing(disk:Vec<db::Folder>,_catalogue:Vec<db::Folder>)->Vec<db::Folder>{disk}
fn classify_page(page:&mut db::AssetPage){let mut cache=std::collections::HashMap::new();for asset in &mut page.items{if let Some(path)=asset.path.as_deref(){let folder=Path::new(path).parent().and_then(Path::to_str).unwrap_or(path);(asset.offline,asset.missing)=classify_source(folder,&mut cache)}}}

#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct CatalogSourceStatus{pub path:String,pub assets:usize,pub offline:bool,pub missing:bool}
#[derive(Clone)] struct CatalogFolderNode{id:i64,parent:Option<i64>,path:String,count:usize}
fn collect_catalog_missing_sources(mut nodes:Vec<CatalogFolderNode>,mut exists:impl FnMut(&str)->bool)->Vec<CatalogSourceStatus>{
    nodes.sort_by_key(|n|std::cmp::Reverse(n.path.len()));
    let mut counts=nodes.iter().map(|n|(n.id,n.count)).collect::<std::collections::HashMap<_,_>>();
    for node in &nodes { if let Some(parent)=node.parent {let count=counts.get(&node.id).copied().unwrap_or(0);*counts.entry(parent).or_default()+=count;} }
    nodes.sort_by_key(|n|n.path.len());
    let mut reachable=std::collections::HashMap::<String,bool>::new();let mut status=std::collections::HashMap::<i64,(bool,bool)>::new();
    for node in &nodes {
        let volume=path_volume(&node.path);let online=*reachable.entry(volume.clone()).or_insert_with(||exists(&volume));
        let value=if !online {(true,false)} else {let parent_status=node.parent.and_then(|id|status.get(&id).copied()).unwrap_or((false,false));if parent_status.0||parent_status.1{parent_status}else{(false,!exists(&node.path))}};
        status.insert(node.id,value);
    }
    let mut result=Vec::new();
    for node in nodes {
        let Some(&(offline,missing))=status.get(&node.id) else{continue};if !offline&&!missing{continue}
        let parent_bad=node.parent.and_then(|id|status.get(&id).copied()).is_some_and(|(o,m)|o||m);
        if !parent_bad {result.push(CatalogSourceStatus{path:node.path,assets:counts.get(&node.id).copied().unwrap_or(0),offline,missing});}
    }
    result
}
#[tauri::command(async)]
pub fn get_catalog_missing_sources(state:State<'_,BackgroundDbState>)->Result<Vec<CatalogSourceStatus>,String>{
    let conn=state.0.lock().map_err(|_|"catalog lock poisoned")?;
    let mut stmt=conn.prepare("SELECT f.id,f.parent_id,f.path,count(a.id) FROM folders f LEFT JOIN assets a ON a.folder_id=f.id WHERE f.path IS NOT NULL GROUP BY f.id,f.parent_id,f.path").map_err(|e|e.to_string())?;
    let nodes=stmt.query_map([],|r|Ok(CatalogFolderNode{id:r.get(0)?,parent:r.get(1)?,path:r.get(2)?,count:r.get::<_,i64>(3)?.max(0) as usize})).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    drop(stmt);drop(conn);Ok(collect_catalog_missing_sources(nodes,|p|Path::new(p).is_dir()))
}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct RemoveCatalogInfo{pub assets:usize,pub folders:usize,pub offline:bool,pub missing:bool}
#[tauri::command(async)]
pub fn remove_folder_from_catalog_info(db:State<'_,BackgroundDbState>,folder_path:String)->Result<RemoveCatalogInfo,String>{let path=folder_path.trim().trim_end_matches(['\\','/']);let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;let normal=crate::journal::normalize_fs_path(path);let prefix=format!("{}\\",normal);let asset_paths={let mut stmt=conn.prepare("SELECT path FROM assets WHERE path IS NOT NULL").map_err(|e|e.to_string())?;stmt.query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?};let assets=asset_paths.iter().filter(|p|{let p=crate::journal::normalize_fs_path(p);p==normal||p.starts_with(&prefix)}).count();let all={let mut stmt=conn.prepare("SELECT path FROM folders WHERE path IS NOT NULL").map_err(|e|e.to_string())?;stmt.query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?};let prefix=format!("{}\\",path.replace('/',"\\").trim_end_matches('\\'));let folders=all.iter().filter(|p|p.eq_ignore_ascii_case(path)||p.to_lowercase().starts_with(&prefix.to_lowercase())).count();drop(conn);let mut cache=std::collections::HashMap::new();let(offline,missing)=classify_source(path,&mut cache);Ok(RemoveCatalogInfo{assets,folders,offline,missing})}
#[tauri::command(async)]
pub fn remove_folder_from_catalog(writer:State<'_,CatalogWriterState>,ai:State<'_,AiState>,app:State<'_,AppInfoState>,listing:State<'_,ListingState>,folder_path:String)->Result<crate::journal::RemoveCatalogResult,String>{let mut cache=std::collections::HashMap::new();let(offline,missing)=classify_source(&folder_path,&mut cache);if !offline&&!missing{return Err("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie".into())}if app.0.readonly{return Err("Katalóg je iba na čítanie".into())}crate::backup::backup_now(Path::new(&app.0.catalog),Path::new(&app.0.backups))?;let result=writer.0.remove_folder_from_catalog(&folder_path)?;ai.0.forget(result.asset_ids.clone());let _=listing.0.invalidate_path(&folder_path);Ok(result)}

#[tauri::command(async)]
pub fn ai_status(state: State<'_, AiState>) -> AiStatus { state.0.status() }
#[tauri::command(async)]
pub fn ai_start(state: State<'_, AiState>, lazy_model: Option<bool>) -> Result<AiStatus, String> { state.0.start(lazy_model.unwrap_or(false)) }
#[tauri::command(async)]
pub fn ai_load_model(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.load_model() }
#[tauri::command(async)]
pub fn ai_stop(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.stop() }
#[tauri::command(async)]
pub fn ai_pause(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.pause() }
#[tauri::command(async)]
pub fn ai_resume(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.resume() }
#[tauri::command(async)]
pub fn ai_reindex(state: State<'_, AiState>) -> Result<AiStatus, String> { state.0.reindex() }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiMapCluster { pub id:u32,pub x:f32,pub y:f32,pub r:f32,pub count:usize,pub name:Option<String> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiMapData { pub ids:Vec<i64>,pub x:Vec<f32>,pub y:Vec<f32>,pub cluster:Vec<u32>,pub clusters:Vec<AiMapCluster>,pub built_at:Option<f64>,pub indexed:usize }
#[tauri::command(async)]
pub fn ai_map_build(state:State<'_,AiState>)->Result<Value,String>{state.0.map_build()}
#[tauri::command(async)]
pub fn ai_map_data(db:State<'_,BackgroundDbState>,ai:State<'_,AiState>)->Result<AiMapData,String>{
    let empty=||AiMapData{ids:Vec::new(),x:Vec::new(),y:Vec::new(),cluster:Vec::new(),clusters:Vec::new(),built_at:None,indexed:0};
    let path=ai.0.ai_db_path();if !path.is_file(){return Ok(empty())}
    let conn=match rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX){Ok(c)=>c,Err(_)=>return Ok(empty())};
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='map_points')",[],|r|r.get(0)).unwrap_or(false);if !exists{return Ok(empty())}
    let points={let mut stmt=conn.prepare("SELECT asset_id,x,y,cluster FROM map_points ORDER BY asset_id").map_err(|e|e.to_string())?;stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f32>(1)?,r.get::<_,f32>(2)?,r.get::<_,u32>(3)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?};
    let mut allowed=std::collections::HashSet::new();let catalog=db.0.lock().map_err(|_|"catalog lock poisoned")?;
    for chunk in points.iter().map(|p|p.0).collect::<Vec<_>>().chunks(800){if chunk.is_empty(){continue}let marks=vec!["?";chunk.len()].join(",");let mut stmt=catalog.prepare(&format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})")).map_err(|e|e.to_string())?;for id in stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?{allowed.insert(id.map_err(|e|e.to_string())?);}}
    drop(catalog);let filtered=points.into_iter().filter(|p|allowed.contains(&p.0)).collect::<Vec<_>>();
    let clusters={let mut stmt=conn.prepare("SELECT id,x,y,r,count,name FROM map_clusters ORDER BY id").map_err(|e|e.to_string())?;stmt.query_map([],|r|Ok(AiMapCluster{id:r.get::<_,u32>(0)?,x:r.get(1)?,y:r.get(2)?,r:r.get(3)?,count:r.get::<_,i64>(4)?.max(0) as usize,name:r.get(5)?})).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?};
    let built_at=conn.query_row("SELECT value FROM meta WHERE key='map_built_at'",[],|r|r.get::<_,String>(0)).ok().and_then(|v|v.parse().ok());
    let indexed=conn.query_row("SELECT value FROM meta WHERE key='map_photo_count'",[],|r|r.get::<_,String>(0)).ok().and_then(|v|v.parse().ok()).unwrap_or(filtered.len());
    Ok(AiMapData{ids:filtered.iter().map(|p|p.0).collect(),x:filtered.iter().map(|p|p.1).collect(),y:filtered.iter().map(|p|p.2).collect(),cluster:filtered.iter().map(|p|p.3).collect(),clusters,built_at,indexed})
}
#[tauri::command(async)]
pub fn ai_search_text(state: State<'_, AiState>, query: String, limit: Option<u32>) -> Result<AiSearchResult, String> { state.0.search_text(query, limit) }
#[tauri::command(async)]
pub fn ai_similar(state: State<'_, AiState>, asset_id: i64, limit: Option<u32>) -> Result<AiSearchResult, String> { state.0.similar(asset_id, limit) }
#[tauri::command(async)]
pub fn ai_duplicate_groups(state:State<'_,AiState>,level:String)->Result<Value,String>{state.0.duplicate_groups(&level)}
#[derive(Serialize)]
pub struct AiSeriesGroup { pub ids:Vec<i64>, pub best:i64 }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiSeriesResult { pub groups:Vec<AiSeriesGroup>, pub analyzed:usize, pub truncated:bool }

pub(crate) fn exif_date_seconds(value:&str)->Option<f64>{
    if !value.is_ascii(){return None;}
    let split=value.find(|c|c=='T'||c==' ')?;let date=&value[..split];let time=&value[split+1..];
    if date.len()!=10||date.as_bytes()[4]!=b'-'||date.as_bytes()[7]!=b'-'||time.len()<8{return None;}
    let year=date[0..4].parse::<i64>().ok()?;let month=date[5..7].parse::<i64>().ok()?;let day=date[8..10].parse::<i64>().ok()?;
    let hour=time[0..2].parse::<i64>().ok()?;let minute=time[3..5].parse::<i64>().ok()?;let second=time[6..8].parse::<i64>().ok()?;
    if !(1..=12).contains(&month)||!(0..=23).contains(&hour)||!(0..=59).contains(&minute)||!(0..=60).contains(&second){return None;}
    let leap=year%4==0&&(year%100!=0||year%400==0);let month_days=[31,if leap{29}else{28},31,30,31,30,31,31,30,31,30,31];if day<1||day>month_days[(month-1) as usize]{return None;}
    let y=year-if month<=2{1}else{0};let era=y/400;let yoe=y-era*400;let mp=month+if month>2{-3}else{9};let doy=(153*mp+2)/5+day-1;let doe=yoe*365+yoe/4-yoe/100+doy;let days=era*146097+doe-719468;
    let tail=&time[8..];let offset=if tail.starts_with('+')||tail.starts_with('-'){let sign=if tail.starts_with('+'){1}else{-1};let h=tail.get(1..3)?.parse::<i64>().ok()?;let m=tail.get(4..6)?.parse::<i64>().ok()?;sign*(h*3600+m*60)}else{0};
    Some((days*86400+hour*3600+minute*60+second-offset) as f64)
}

#[tauri::command(async)]
pub fn ai_series(state:State<'_,DatabaseState>,listing:State<'_,ListingState>,formats:State<'_,FormatsState>,ai:State<'_,AiState>,mut filter:AssetFilter,preset:String)->Result<AiSeriesResult,String>{
    let raw_only=filter.file_group.as_deref()==Some("raw");
    if matches!(filter.file_group.as_deref(),Some("video"|"other")){return Ok(AiSeriesResult{groups:Vec::new(),analyzed:0,truncated:false});}
    filter.file_group=Some(if raw_only{"raw".into()}else{"images".into()});
    filter.sort="captureTime".into();filter.descending=false;filter.offset=0;filter.limit=500;
    let cap=20_000usize;let mut truncated=false;let mut candidates:Vec<(i64,Option<f64>)>=Vec::new();
    if filter.kind=="ai"{let page=db::assets_page(&state.0,&filter,&formats.0).map_err(|e|e.to_string())?;candidates.extend(page.items.into_iter().filter(|a|!a.is_directory).map(|a|(a.id,a.exif_date.as_deref().and_then(exif_date_seconds))));}
    else {
        let mut offset=0i64;
        loop {
            filter.offset=offset;
            let (rows,page_count,total)=if filter.kind=="folder" {
                let page=listing.0.page(&filter)?;let page_count=page.items.len();let total=page.total;
                (page.items.into_iter().filter(|a|!a.is_directory).map(|a|(a.id,a.exif_date.as_deref().and_then(exif_date_seconds))).collect::<Vec<_>>(),page_count,Some(total))
            } else {
                let ids=db::asset_ids_page(&state.0,&filter,&formats.0,offset,500).map_err(|e|e.to_string())?;let page_count=ids.len();
                let conn=state.0.lock().map_err(|_|"catalog lock poisoned")?;let mut rows=Vec::new();
                for chunk in ids.chunks(500){let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let mut stmt=conn.prepare(&format!("SELECT id,exif_date FROM assets WHERE id IN ({marks}) AND is_listed=1")).map_err(|e|e.to_string())?;rows.extend(stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?);}
                (rows.into_iter().map(|(id,date)|(id,date.as_deref().and_then(exif_date_seconds))).collect::<Vec<_>>(),page_count,None)
            };
            for (id,date) in rows{if candidates.len()==cap{truncated=true;break;}candidates.push((id,date));}
            if truncated||page_count<500||total.is_some_and(|n|offset+page_count as i64>=n){break;}
            offset+=page_count as i64;
        }
    }
    candidates.sort_by(|a,b|a.1.is_none().cmp(&b.1.is_none()).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).then(a.0.cmp(&b.0)));
    if candidates.len()>cap{candidates.truncate(cap);truncated=true;}
    let items=candidates.iter().map(|(id,date)|serde_json::json!({"asset_id":id,"t":date})).collect::<Vec<_>>();
    let analyzed=items.len();
    let result=ai.0.analyze_series(items,preset.as_str())?;
    let groups=result.get("groups").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|group|{
        let ids=group.get("ids")?.as_array()?.iter().filter_map(Value::as_i64).collect::<Vec<_>>();let best=group.get("best")?.as_i64()?;(ids.len()>1).then_some(AiSeriesGroup{ids,best})
    }).collect();
    Ok(AiSeriesResult{groups,analyzed,truncated})
}
#[tauri::command(async)]
pub fn ai_duplicates_of(state:State<'_,AiState>,asset_id:i64,level:String)->Result<Vec<i64>,String>{state.0.duplicates_of(asset_id,&level)}
#[tauri::command(async)]
pub fn ai_suggest(state:State<'_,AiState>,asset_ids:Vec<i64>,kind:String)->Result<Value,String>{state.0.suggest(asset_ids,&kind)}

#[tauri::command(async)]
pub fn ai_content_keywords(state: State<'_, AiState>, asset_id: i64) -> Result<Value, String> {
    state.0.content_keywords(asset_id)
}

#[tauri::command(async)]
pub fn ai_keyword_matches(ai: State<'_, AiState>, db: State<'_, DatabaseState>, keyword_id: i64) -> Result<Value, String> {
    let mut result = ai.0.keyword_matches(keyword_id)?;
    let ids: Vec<i64> = result["ids"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    result["ids"] = serde_json::json!(filter_listed_ids(&db.0, &ids, false)?);
    Ok(result)
}

#[tauri::command(async)]
pub fn ai_incoming_groups(ai: State<'_, AiState>) -> Result<Value, String> {
    ai.0.incoming_groups()
}
#[tauri::command(async)]
pub fn ai_reject_suggestion(state:State<'_,AiState>,asset_ids:Vec<i64>,kind:String,label_id:i64)->Result<usize,String>{state.0.reject_suggestion(&asset_ids,&kind,label_id)}
#[tauri::command(async)]
pub fn ai_suggestion_groups(state:State<'_,AiState>,kind:String)->Result<Value,String>{state.0.suggestion_groups(&kind)}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiUnsortedCluster { pub ids:Vec<i64>,pub total:usize,pub cohesion:f64,pub label_id:Option<i64>,pub label_path:Option<String>,pub name:Option<String> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiUnsortedClusters { pub groups:Vec<AiUnsortedCluster>,pub unlabeled:usize,pub clustered:usize }
#[tauri::command(async)]
pub fn ai_unsorted_clusters(state:State<'_,AiState>,min_size:Option<u32>)->Result<AiUnsortedClusters,String>{let value=state.0.unsorted_clusters(min_size)?;let groups=value.get("groups").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|g|AiUnsortedCluster{ids:g.get("ids").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|v|v.as_i64()).collect(),total:g.get("total").and_then(Value::as_u64).unwrap_or(0) as usize,cohesion:g.get("cohesion").and_then(Value::as_f64).unwrap_or(0.0),label_id:g.get("labelId").and_then(Value::as_i64),label_path:g.get("labelPath").and_then(Value::as_str).map(str::to_owned),name:g.get("name").and_then(Value::as_str).map(str::to_owned)}).collect();Ok(AiUnsortedClusters{groups,unlabeled:value.get("unlabeled").and_then(Value::as_u64).unwrap_or(0) as usize,clustered:value.get("clustered").and_then(Value::as_u64).unwrap_or(0) as usize})}
#[tauri::command(async)]
pub fn ai_category_paths(state:State<'_,AiState>)->Result<Vec<Value>,String>{Ok(state.0.category_paths()?.into_iter().map(|(id,path)|serde_json::json!({"id":id,"path":path})).collect())}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFacesStatus { pub available:bool,pub scanned:usize,pub pending:usize,pub faces:usize,pub named:usize,pub unknown:usize,pub persons:usize }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFacePerson { pub id:i64,pub name:String,pub confirmed:usize,pub auto:usize,pub assets:usize,pub cover:Option<i64> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFacesPersons { pub persons:Vec<AiFacePerson> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFaceReview { pub face_id:i64,pub asset_id:i64,pub score:Option<f64> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFacesReview { pub faces:Vec<AiFaceReview> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFaceClusterGuess { pub person_id:i64,pub name:String,pub score:f64 }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFaceCluster { pub face_ids:Vec<i64>,pub total:usize,pub asset_ids:Vec<i64>,pub assets:usize,pub guess:Option<AiFaceClusterGuess> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFacesClusters { pub available:bool,pub groups:Vec<AiFaceCluster>,pub unknown:usize,pub clustered:usize }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFaceAsset { pub face_id:i64,pub asset_id:i64,pub x:Option<f64>,pub y:Option<f64>,pub w:Option<f64>,pub h:Option<f64>,pub person_id:Option<i64>,pub name:Option<String>,pub state:String,pub score:Option<f64> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct AiFaceThumb { pub face_id:i64,pub data_url:String }

#[tauri::command(async)]
pub fn ai_faces_status(state:State<'_,AiState>)->Result<AiFacesStatus,String>{let v=state.0.faces_status()?;Ok(AiFacesStatus{available:v["available"].as_bool().unwrap_or(false),scanned:v["scanned"].as_u64().unwrap_or(0) as usize,pending:v["pending"].as_u64().unwrap_or(0) as usize,faces:v["faces"].as_u64().unwrap_or(0) as usize,named:v["named"].as_u64().unwrap_or(0) as usize,unknown:v["unknown"].as_u64().unwrap_or(0) as usize,persons:v["persons"].as_u64().unwrap_or(0) as usize})}
#[tauri::command(async)]
pub fn ai_faces_persons(state:State<'_,AiState>)->Result<AiFacesPersons,String>{let v=state.0.faces_persons()?;let persons=v["persons"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|p|Some(AiFacePerson{id:p["id"].as_i64()?,name:p["name"].as_str()?.to_owned(),confirmed:p["confirmed"].as_u64().unwrap_or(0) as usize,auto:p["auto"].as_u64().unwrap_or(0) as usize,assets:p["assets"].as_u64().unwrap_or(0) as usize,cover:p["cover"].as_i64()})).collect();Ok(AiFacesPersons{persons})}
fn filter_listed_ids(db:&Db,ids:&[i64],sort_by_date:bool)->Result<Vec<i64>,String>{
    if ids.is_empty(){return Ok(Vec::new())}
    let conn=db.lock().map_err(|e|e.to_string())?;
    if sort_by_date {
        let mut dated=Vec::<(i64,Option<String>)>::new();
        for chunk in ids.chunks(800){let marks=vec!["?";chunk.len()].join(",");let sql=format!("SELECT id,exif_date FROM assets WHERE is_listed=1 AND id IN ({marks})");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;dated.extend(stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?);}
        dated.sort_by(|a,b|match (&a.1,&b.1){(Some(x),Some(y))=>x.cmp(y).then(a.0.cmp(&b.0)),(Some(_),None)=>std::cmp::Ordering::Less,(None,Some(_))=>std::cmp::Ordering::Greater,(None,None)=>a.0.cmp(&b.0)});
        return Ok(dated.into_iter().map(|v|v.0).collect());
    }
    let mut listed=std::collections::HashSet::new();
    for chunk in ids.chunks(800){let marks=vec!["?";chunk.len()].join(",");let sql=format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;listed.extend(stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?);}
    let mut seen=std::collections::HashSet::new();Ok(ids.iter().copied().filter(|id|listed.contains(id)&&seen.insert(*id)).collect())
}
#[tauri::command(async)]
pub fn ai_faces_person_assets(ai:State<'_,AiState>,db:State<'_,DatabaseState>,person_id:i64,include_auto:Option<bool>)->Result<Vec<i64>,String>{let v=ai.0.faces_person_assets(person_id,include_auto.unwrap_or(true))?;let ids=v["asset_ids"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect::<Vec<_>>();filter_listed_ids(&db.0,&ids,true)}
#[tauri::command(async)]
pub fn ai_faces_review(ai:State<'_,AiState>,db:State<'_,DatabaseState>,person_id:i64,limit:Option<u32>)->Result<AiFacesReview,String>{let v=ai.0.faces_review(person_id,limit)?;let raw=v["faces"].as_array().cloned().unwrap_or_default();let ids=raw.iter().filter_map(|f|f["asset_id"].as_i64()).collect::<Vec<_>>();let listed=filter_listed_ids(&db.0,&ids,false)?.into_iter().collect::<std::collections::HashSet<_>>();let faces=raw.into_iter().filter_map(|f|{let asset_id=f["asset_id"].as_i64()?;if !listed.contains(&asset_id){return None}Some(AiFaceReview{face_id:f["face_id"].as_i64()?,asset_id,score:f["score"].as_f64()})}).collect();Ok(AiFacesReview{faces})}
#[tauri::command(async)]
pub fn ai_faces_clusters(ai:State<'_,AiState>,db:State<'_,DatabaseState>,min_size:Option<u32>)->Result<AiFacesClusters,String>{let v=ai.0.faces_clusters(min_size)?;let groups=v["groups"].as_array().into_iter().flatten().filter_map(|g|{let raw_assets=g["assetIds"].as_array().or_else(||g["asset_ids"].as_array())?;let raw_ids=g["faceIds"].as_array().or_else(||g["face_ids"].as_array())?;let asset_ids=raw_assets.iter().filter_map(Value::as_i64).collect::<Vec<_>>();let listed=filter_listed_ids(&db.0,&asset_ids,false).ok()?;if listed.is_empty(){return None}let guess=g.get("guess").filter(|x|!x.is_null()).and_then(|x|Some(AiFaceClusterGuess{person_id:x.get("personId").or_else(||x.get("person_id"))?.as_i64()?,name:x.get("name")?.as_str()?.to_owned(),score:x.get("score")?.as_f64()?}));Some(AiFaceCluster{face_ids:raw_ids.iter().filter_map(Value::as_i64).collect(),total:g["total"].as_u64().unwrap_or(0) as usize,assets:g["assets"].as_u64().unwrap_or(listed.len() as u64) as usize,asset_ids:listed,guess})}).collect();Ok(AiFacesClusters{available:v["available"].as_bool().unwrap_or(false),groups,unknown:v["unknown"].as_u64().unwrap_or(0) as usize,clustered:v["clustered"].as_u64().unwrap_or(0) as usize})}
#[tauri::command(async)]
pub fn ai_faces_for_assets(ai:State<'_,AiState>,db:State<'_,DatabaseState>,asset_ids:Vec<i64>)->Result<Vec<AiFaceAsset>,String>{if asset_ids.len()>500{return Err("Najviac 500 asset ID naraz".into())}let ids=asset_ids.into_iter().filter(|id|*id>0).collect::<Vec<_>>();let v=ai.0.faces_for_assets(&ids)?;let raw=v["faces"].as_array().cloned().unwrap_or_default();let asset_ids=raw.iter().filter_map(|f|f["asset_id"].as_i64()).collect::<Vec<_>>();let listed=filter_listed_ids(&db.0,&asset_ids,false)?.into_iter().collect::<std::collections::HashSet<_>>();Ok(raw.into_iter().filter_map(|f|{let asset_id=f["asset_id"].as_i64()?;if !listed.contains(&asset_id){return None}Some(AiFaceAsset{face_id:f["face_id"].as_i64()?,asset_id,x:f["x"].as_f64(),y:f["y"].as_f64(),w:f["w"].as_f64(),h:f["h"].as_f64(),person_id:f["person_id"].as_i64(),name:f["name"].as_str().map(str::to_owned),state:f["state"].as_str()?.to_owned(),score:f["score"].as_f64()})}).collect())}
#[tauri::command(async)]
pub fn ai_faces_set(ai:State<'_,AiState>,face_ids:Vec<i64>,action:String,person_id:Option<i64>)->Result<usize,String>{if face_ids.len()>500{return Err("Najviac 500 tvárí naraz".into())}let ids=face_ids.into_iter().filter(|id|*id>0).collect::<Vec<_>>();let v=ai.0.faces_set(&ids,&action,person_id)?;Ok(v["changed"].as_u64().unwrap_or(0) as usize)}
#[tauri::command(async)]
pub fn ai_faces_asset_assign(ai:State<'_,AiState>,db:State<'_,DatabaseState>,asset_ids:Vec<i64>,person_id:i64)->Result<usize,String>{if asset_ids.len()>2000{return Err("Najviac 2000 fotiek naraz".into())}let ids=filter_listed_ids(&db.0,&asset_ids,false)?;let v=ai.0.faces_asset_assign(&ids,person_id)?;Ok(v["changed"].as_u64().unwrap_or(0) as usize)}
#[tauri::command(async)]
pub fn ai_faces_asset_remove(ai:State<'_,AiState>,db:State<'_,DatabaseState>,asset_ids:Vec<i64>,person_id:i64)->Result<usize,String>{if asset_ids.len()>2000{return Err("Najviac 2000 fotiek naraz".into())}let ids=filter_listed_ids(&db.0,&asset_ids,false)?;let v=ai.0.faces_asset_remove(&ids,person_id)?;Ok(v["changed"].as_u64().unwrap_or(0) as usize)}
#[tauri::command(async)]
pub fn ai_faces_person_create(ai:State<'_,AiState>,name:String)->Result<i64,String>{let v=ai.0.faces_person_create(&name)?;v["id"].as_i64().ok_or_else(||"AI nevrátilo ID osoby".into())}
#[tauri::command(async)]
pub fn ai_faces_person_rename(ai:State<'_,AiState>,person_id:i64,name:String)->Result<bool,String>{let v=ai.0.faces_person_rename(person_id,&name)?;Ok(v["ok"].as_bool().unwrap_or(false))}
#[tauri::command(async)]
pub fn ai_faces_recognize_all(ai:State<'_,AiState>)->Result<usize,String>{let v=ai.0.faces_recognize_all()?;Ok(v["changed"].as_u64().unwrap_or(0) as usize)}
#[tauri::command(async)]
pub fn ai_face_thumbs(ai:State<'_,AiState>,face_ids:Vec<i64>)->Result<Vec<AiFaceThumb>,String>{Ok(ai.0.face_thumbs(&face_ids)?.into_iter().filter_map(|v|Some(AiFaceThumb{face_id:v["faceId"].as_i64()?,data_url:v["dataUrl"].as_str()?.to_owned()})).collect())}
#[tauri::command(async)]
pub fn ai_cluster_ignore(state:State<'_,AiState>,asset_ids:Vec<i64>)->Result<usize,String>{state.0.cluster_ignore(&asset_ids)}
#[tauri::command(async)]
pub fn ai_cluster_ignore_reset(state:State<'_,AiState>)->Result<usize,String>{state.0.cluster_ignore_reset()}
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct ColorCount { pub value:String,pub name:String,pub hex:String,pub count:usize }
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct AssetColorState { pub value:String,pub name:String,pub hex:String,pub auto:bool,pub effective:bool,pub manual_state:Option<bool> }
#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct AssetColors { pub asset_id:i64,pub computed:bool,pub colors:Vec<AssetColorState> }
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
pub fn get_color_counts(db:State<'_, BackgroundDbState>,ai:State<'_,AiState>)->Result<Vec<ColorCount>,String>{
    // The catalogue lock is held only for the two quick catalogue reads. ai.sqlite (hundreds of MB,
    // on the same disk the AI model loads from) is read through its own read-only connection:
    // with the lock held, a slow cold read here used to freeze every other query for many seconds.
    let mut counts=std::collections::HashMap::<String,usize>::new();
    let (ids,overrides)={
        let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
        let ids=conn.prepare("SELECT id FROM assets WHERE is_listed=1 AND id>0").map_err(|e|e.to_string())?.query_map([],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        let overrides=conn.prepare("SELECT asset_id,color,state FROM asset_color_overrides").map_err(|e|e.to_string())?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        (ids,overrides)
    };
    let mut auto_by_id=std::collections::HashMap::<i64,String>::new();
    let ai_db=ai.0.ai_db_path();
    if ai_db.is_file() {
        if let Ok(conn)=rusqlite::Connection::open_with_flags(&ai_db,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            let _=conn.busy_timeout(std::time::Duration::from_secs(2));
            if let Ok(mut stmt)=conn.prepare("SELECT asset_id,auto FROM colors") {
                if let Ok(found)=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?))) { for (id,auto) in found.flatten() { if let Some(auto)=auto { auto_by_id.insert(id,auto); } } }
            }
        }
    }
    let rows:Vec<(i64,Option<String>)>=ids.into_iter().map(|id|{let auto=auto_by_id.remove(&id);(id,auto)}).collect();
    let mut by_id=std::collections::HashMap::<i64,Vec<(String,i64)>>::new();for(id,color,state)in overrides{by_id.entry(id).or_default().push((color,state));}
    for(id,json)in rows{let auto=json.as_deref().and_then(|v|serde_json::from_str::<Vec<String>>(v).ok()).unwrap_or_default();let effective=crate::colors::effective_colors(&auto,by_id.get(&id).map(Vec::as_slice).unwrap_or(&[]));for color in effective{*counts.entry(color).or_default()+=1;}}
    Ok(crate::colors::PALETTE.iter().map(|c|ColorCount{value:c.value.into(),name:c.name.into(),hex:c.hex.into(),count:*counts.get(c.value).unwrap_or(&0)}).collect())
}

#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct AssetQuality { pub asset_id:i64,pub sharp:f64,pub hi:f64,pub lo:f64,pub mean:f64,pub flags:Vec<String> }

fn quality_connection(ai:&AiState)->Option<rusqlite::Connection>{
    let path=ai.0.ai_db_path();if !path.is_file(){return None;}
    let conn=rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _=conn.busy_timeout(std::time::Duration::from_secs(2));Some(conn)
}

#[tauri::command(async)]
pub fn get_asset_quality(db:State<'_,BackgroundDbState>,ai:State<'_,AiState>,asset_ids:Vec<i64>)->Result<Vec<AssetQuality>,String>{
    let ids={
        let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
        let wanted=asset_ids.into_iter().filter(|id|*id>0).collect::<std::collections::HashSet<_>>();
        let mut out=std::collections::HashSet::new();
        for chunk in wanted.iter().copied().collect::<Vec<_>>().chunks(500){
            let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
            let mut stmt=conn.prepare(&format!("SELECT id FROM assets WHERE is_listed=1 AND id IN ({marks})")).map_err(|e|e.to_string())?;
            for row in stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? {out.insert(row.map_err(|e|e.to_string())?);}
        }out
    };
    let Some(conn)=quality_connection(&ai) else{return Ok(Vec::new())};
    let mut out=Vec::new();
    for chunk in ids.iter().copied().collect::<Vec<_>>().chunks(500){
        if chunk.is_empty(){continue}
        let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql=format!("SELECT asset_id,sharp,hi,lo,mean FROM quality WHERE version=? AND asset_id IN ({marks})");
        let mut stmt=match conn.prepare(&sql){Ok(v)=>v,Err(_)=>return Ok(Vec::new())};
        let mut args=vec![rusqlite::types::Value::Integer(crate::quality::QUALITY_VERSION)];args.extend(chunk.iter().copied().map(Into::into));
        let rows=stmt.query_map(rusqlite::params_from_iter(args.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?,r.get::<_,f64>(2)?,r.get::<_,f64>(3)?,r.get::<_,f64>(4)?))).map_err(|e|e.to_string())?;
        for row in rows{let(id,sharp,hi,lo,mean)=row.map_err(|e|e.to_string())?;let flags=crate::quality::flags(sharp,hi,lo,mean).into_iter().map(str::to_owned).collect();out.push(AssetQuality{asset_id:id,sharp,hi,lo,mean,flags});}
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn get_quality_counts(db:State<'_,BackgroundDbState>,ai:State<'_,AiState>)->Result<Vec<db::ValueCount>,String>{
    let ids={
        let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
        conn.prepare("SELECT id FROM assets WHERE is_listed=1 AND id>0").map_err(|e|e.to_string())?.query_map([],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<std::collections::HashSet<i64>>>().map_err(|e|e.to_string())?
    };
    let mut counts=std::collections::HashMap::<&'static str,i64>::new();
    if let Some(conn)=quality_connection(&ai){
        if let Ok(mut stmt)=conn.prepare("SELECT asset_id,sharp,hi,mean FROM quality WHERE version=?"){
            if let Ok(rows)=stmt.query_map([crate::quality::QUALITY_VERSION],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?,r.get::<_,f64>(2)?,r.get::<_,f64>(3)?))){
                for row in rows.flatten(){let(id,sharp,hi,mean)=row;if !ids.contains(&id){continue}for flag in crate::quality::flags(sharp,hi,0.0,mean){*counts.entry(flag).or_default()+=1;}}
            }
        }
    }
    Ok(["blur","over","under"].into_iter().map(|value|db::ValueCount{value:value.into(),count:*counts.get(value).unwrap_or(&0)}).collect())
}

#[tauri::command(async)]
pub fn get_app_info(state: State<'_, AppInfoState>) -> AppInfo { state.0.clone() }

#[tauri::command(async)]
pub fn get_recent_changes(state: State<'_, CatalogWriterState>, limit: i64) -> Result<Vec<ChangeSummary>, String> {
    state.0.recent_changes(limit)
}

#[tauri::command(async)]
pub fn get_file_operation_conflicts(state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String)->Result<usize,String>{state.0.preview_conflicts(&asset_ids,&target_folder_path,None)}
#[tauri::command(async)]
pub fn get_uncategorized_photos_for_move(
    state: State<'_, FileManagerState>, formats: State<'_, FormatsState>,
    asset_ids: Vec<i64>, target_folder_path: String, conflict: ConflictPolicy,
) -> Result<usize, String> {
    state.0.uncategorized_photos_for_move(&asset_ids, &target_folder_path, conflict, &formats.0)
}
#[tauri::command]
pub async fn move_files(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String,conflict:ConflictPolicy)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.move_files(&asset_ids,&target_folder_path,conflict,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Presúvam".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command]
pub async fn copy_files(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_ids:Vec<i64>,target_folder_path:String,conflict:ConflictPolicy)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.copy_files(&asset_ids,&target_folder_path,conflict,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Kopírujem".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command]
pub async fn rename_file(app:tauri::AppHandle,state:State<'_,FileManagerState>,asset_id:i64,new_base_name:String)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.rename_file(asset_id,&new_base_name,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Premenúvam".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct FolderRenameResult{pub change_id:Option<i64>,pub summary:String,pub new_path:String}
#[tauri::command(async)]
pub fn reveal_in_explorer(state:State<'_,FileManagerState>,path:String)->Result<(),String>{state.0.reveal_in_explorer(&path)}
#[tauri::command(async)]
pub async fn rename_folder(state:State<'_,FileManagerState>,path:String,new_name:String)->Result<FolderRenameResult,String>{let manager=state.0.clone();let original_path=path.clone();let result=tauri::async_runtime::spawn_blocking(move||manager.rename_folder(&path,&new_name)).await.map_err(|e|e.to_string())??;let new_path=result.affected_folder_paths.get(1).cloned().unwrap_or(original_path);Ok(FolderRenameResult{change_id:result.change_id,summary:result.summary,new_path})}
#[tauri::command(async)]
pub fn delete_folder_info(state:State<'_,FileManagerState>,path:String)->Result<crate::file_ops::DeleteFolderInfo,String>{state.0.delete_folder_info(&path)}
#[tauri::command(async)]
pub async fn delete_folder(state:State<'_,FileManagerState>,path:String)->Result<crate::file_ops::FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.delete_folder(&path)).await.map_err(|e|e.to_string())?}
#[tauri::command(async)]
pub async fn move_folder(app:tauri::AppHandle,state:State<'_,FileManagerState>,path:String,target:String)->Result<crate::file_ops::FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.move_folder_with_progress(&path,&target,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Presúvam priečinok".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command(async)]
pub async fn copy_folder(app:tauri::AppHandle,state:State<'_,FileManagerState>,path:String,target:String)->Result<crate::file_ops::FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.copy_folder_with_progress(&path,&target,|done,total|{let _=app.emit("file-operation-progress",FileProgress{operation:"Kopírujem priečinok".into(),done,total});})).await.map_err(|e|e.to_string())?}
#[tauri::command(async)]
pub async fn create_folder(state:State<'_,FileManagerState>,parent_path:String,name:String)->Result<FileOperationResult,String>{let manager=state.0.clone();tauri::async_runtime::spawn_blocking(move||manager.create_folder(&parent_path,&name)).await.map_err(|e|e.to_string())?}

#[derive(Serialize,Clone)]
#[serde(rename_all="camelCase")]
pub struct FileProgress{pub operation:String,pub done:usize,pub total:usize}

#[tauri::command(async)]
pub fn set_asset_values(
    state: State<'_, CatalogWriterState>,
    listing: State<'_, ListingState>,
    file_manager: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    changes: serde_json::Map<String, Value>,
) -> Result<SetAssetValuesResult, String> {
    let _operation = changes.contains_key("editPending").then(|| file_manager.0.lock_operation()).transpose()?;
    let (file_ids, candidates) = listed_asset_candidates(&listing.0, &asset_ids)?;
    let result = state.0.set_asset_values(&file_ids, &candidates, &changes)?;
    for id in asset_ids.iter().copied().filter(|id| *id >= 0) { let _ = listing.0.invalidate_asset_parent(id); }
    for candidate in candidates { let _ = listing.0.invalidate_path(&candidate.folder_path); }
    Ok(result)
}

fn listed_asset_candidates(listing: &ListingService, asset_ids: &[i64]) -> Result<(Vec<i64>, Vec<ListedAsset>), String> {
    let mut candidates = Vec::new();
    let mut file_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        let asset = listing.asset(id).ok_or_else(|| format!("Listed asset {id} is no longer available"))?;
        if asset.is_directory || asset.ext.is_empty() { continue; }
        let path = asset.path.ok_or_else(|| format!("Path for listed asset {id} is unavailable"))?;
        let folder_path = Path::new(&path).parent().map(|parent| parent.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;
        candidates.push(ListedAsset { temporary_id: id, name: asset.name, ext: asset.ext, size: asset.size, file_modified: asset.file_modified, path, folder_path });
        file_ids.push(id);
    }
    Ok((file_ids, candidates))
}

#[tauri::command(async)]
pub fn set_asset_text(
    state: State<'_, CatalogWriterState>, listing: State<'_, ListingState>,
    asset_ids: Vec<i64>, field: crate::journal::AssetTextField, value: Option<String>,
) -> Result<crate::journal::SetAssetTextResult, String> {
    let (file_ids, candidates) = listed_asset_candidates(&listing.0, &asset_ids)?;
    let result = state.0.set_asset_text(&file_ids, &candidates, field, value)?;
    for id in file_ids.into_iter().filter(|id| *id >= 0) { let _ = listing.0.invalidate_asset_parent(id); }
    for candidate in candidates { let _ = listing.0.invalidate_path(&candidate.folder_path); }
    Ok(result)
}

#[tauri::command]
pub async fn get_asset_iptc(
    db: State<'_, DatabaseState>, listing: State<'_, ListingState>, formats: State<'_, FormatsState>, asset_ids: Vec<i64>,
    exif: State<'_, ExifState>,
) -> Result<Vec<crate::iptc_write::EditorPhoto>, String> {
    let (db, listing, formats, exif) = (db.0.clone(), listing.0.clone(), formats.0.clone(), exif.0.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &asset_ids)?;
        Ok(crate::iptc_write::editor_photos(photos, &exif, &formats))
    }).await.map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IptcEditResult {
    #[serde(flatten)]
    summary: crate::iptc_write::WriteSummary,
    text: Option<crate::journal::SetAssetTextResult>,
    catalog_keywords_changed: bool,
}

#[tauri::command]
pub async fn edit_asset_iptc(
    app: tauri::AppHandle, db: State<'_, DatabaseState>, listing: State<'_, ListingState>, formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>, writer: State<'_, CatalogWriterState>, manager: State<'_, FileManagerState>,
    targets: Vec<crate::iptc_write::EditorTarget>, mut action: crate::iptc_write::ManualEdit,
) -> Result<IptcEditResult, String> {
    use crate::iptc_write::ManualEdit;
    if !writer.0.is_writable() { return Err("Katalóg je iba na čítanie".into()); }
    if let ManualEdit::Text { field, value } = &mut action { *value = field.normalize(value.take())?; }
    if matches!(&action, ManualEdit::RemoveKeyword { keyword } if keyword.trim().is_empty() || keyword.contains('\0')) {
        return Err("Neplatné kľúčové slovo".into());
    }
    let (db, listing, formats, exif, writer, manager) =
        (db.0.clone(), listing.0.clone(), formats.0.clone(), exif.0.clone(), writer.0.clone(), manager.0.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = manager.lock_operation()?;
        let ids: Vec<_> = targets.iter().map(|target| target.id).collect();
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &ids)?;
        let mut candidates = Vec::new();
        let (mut summary, succeeded) = crate::iptc_write::edit_photos(photos, targets, &action, &exif, &formats, |id| {
            let (_, listed) = listed_asset_candidates(&listing, &[id])?;
            writer.check_iptc_edit(id, &listed)?;
            candidates.extend(listed);
            Ok(())
        }, |done, total| {
            if total > 5 {
                let _ = app.emit("file-operation-progress", FileProgress { operation: "Upravujem IPTC".into(), done, total });
            }
        });
        let mut result = IptcEditResult { summary: Default::default(), text: None, catalog_keywords_changed: false };
        let ids: Vec<_> = succeeded.iter().map(|photo| photo.id).collect();
        candidates.retain(|candidate| ids.contains(&candidate.temporary_id));
        match action {
            ManualEdit::Text { field, value } if !ids.is_empty() => {
                match writer.set_asset_text(&ids, &candidates, field, value) {
                    Ok(text) => result.text = Some(text),
                    Err(error) => for photo in &succeeded { summary.catalog_error(photo, &error); },
                }
            }
            ManualEdit::RemoveKeyword { keyword } => {
                let mut by_keyword = std::collections::BTreeMap::<_, Vec<_>>::new();
                for photo in &succeeded {
                    for id in photo.matching_keyword_ids(&keyword) { by_keyword.entry(id).or_default().push(photo); }
                }
                for (id, photos) in by_keyword {
                    let assets: Vec<_> = photos.iter().map(|photo| photo.id).collect();
                    let listed: Vec<_> = candidates.iter().filter(|candidate| assets.contains(&candidate.temporary_id)).cloned().collect();
                    match writer.set_asset_keywords(&assets, &listed, id, false) {
                        Ok(edit) => result.catalog_keywords_changed |= edit.changed_count > 0,
                        Err(error) => for photo in photos { summary.catalog_error(photo, &error); },
                    }
                }
            }
            _ => {}
        }
        for id in ids.into_iter().filter(|id| *id >= 0) { let _ = listing.invalidate_asset_parent(id); }
        for candidate in candidates { let _ = listing.invalidate_path(&candidate.folder_path); }
        result.summary = summary;
        Ok(result)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plan_iptc_write(
    db: State<'_, DatabaseState>, listing: State<'_, ListingState>, formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>, writer: State<'_, CatalogWriterState>, batch: State<'_, Arc<crate::iptc_write::IptcState>>, asset_ids: Vec<i64>,
) -> Result<crate::iptc_write::PlanSummary, String> {
    if !writer.0.is_writable() { return Err("Katalóg je iba na čítanie".into()); }
    let (db, listing, formats, exif, batch) = (db.0.clone(), listing.0.clone(), formats.0.clone(), exif.0.clone(), batch.inner().clone());
    let id = batch.begin_plan()?;
    tauri::async_runtime::spawn_blocking(move || {
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &asset_ids)?;
        batch.store(crate::iptc_write::Plan::prepare(id, photos, &exif, &formats))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn execute_iptc_write(
    app: tauri::AppHandle, formats: State<'_, FormatsState>, exif: State<'_, ExifState>,
    writer: State<'_, CatalogWriterState>, manager: State<'_, FileManagerState>, batch: State<'_, Arc<crate::iptc_write::IptcState>>, plan_id: u64,
) -> Result<crate::iptc_write::WriteSummary, String> {
    if !writer.0.is_writable() { return Err("Katalóg je iba na čítanie".into()); }
    let (formats, exif, manager, batch) = (formats.0.clone(), exif.0.clone(), manager.0.clone(), batch.inner().clone());
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = manager.lock_operation()?;
        batch.execute(plan_id, &exif, &formats, |done, total| {
            let _ = app.emit("file-operation-progress", FileProgress { operation: "Zapisujem IPTC".into(), done, total });
        })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_iptc_write(batch: State<'_, Arc<crate::iptc_write::IptcState>>, plan_id: u64) -> Result<(), String> {
    batch.cancel(plan_id)
}

#[tauri::command(async)]
pub fn get_asset_labels(state: State<'_, DatabaseState>, listing: State<'_, ListingState>, asset_ids: Vec<i64>) -> Result<Vec<(i64, Option<String>)>, String> {
    let mut labels = Vec::new();
    for id in asset_ids.iter().copied().filter(|id| *id < 0) {
        if let Some(asset) = listing.0.asset(id) { labels.push((id, asset.label)); }
    }
    let conn = state.0.lock().map_err(|_| "catalog lock poisoned")?;
    let listed_ids = asset_ids.iter().copied().filter(|id| *id >= 0).collect::<Vec<_>>();
    for chunk in listed_ids.chunks(500) {
        if chunk.is_empty() { continue; }
        let marks = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id,label FROM assets WHERE is_listed=1 AND id IN ({marks})");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        labels.extend(stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| Ok((row.get(0)?, row.get(1)?))).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?);
    }
    Ok(labels)
}

#[tauri::command(async)]
pub fn set_asset_categories(
    state: State<'_, CatalogWriterState>, listing: State<'_, ListingState>, asset_ids: Vec<i64>, category_id: i64, assign: bool,
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
pub fn create_category(state:State<'_,CatalogWriterState>,parent_id:Option<i64>,name:String,icon_key:Option<String>)->Result<CategoryMutationResult,String>{state.0.create_category(parent_id,&name,icon_key.as_deref())}

#[tauri::command(async)]
pub fn set_category_icon(state: State<'_, CatalogWriterState>, id: i64, is_root: bool, icon_key: Option<String>) -> Result<CategoryMutationResult, String> {
    state.0.set_category_icon(id, is_root, icon_key.as_deref())
}
#[tauri::command(async)]
pub fn rename_category(state:State<'_,CatalogWriterState>,id:i64,name:String)->Result<CategoryMutationResult,String>{state.0.rename_category(id,&name)}
#[tauri::command(async)]
pub fn move_category(state:State<'_,CatalogWriterState>,id:i64,new_parent_id:Option<i64>)->Result<CategoryMutationResult,String>{state.0.move_category(id,new_parent_id)}
#[tauri::command(async)]
pub fn get_category_delete_info(state:State<'_,CatalogWriterState>,id:i64)->Result<CategoryDeleteInfo,String>{state.0.category_delete_info(id)}
#[tauri::command(async)]
pub fn delete_category(state:State<'_,CatalogWriterState>,id:i64)->Result<CategoryMutationResult,String>{state.0.delete_category(id)}

#[tauri::command(async)]
pub fn set_asset_keywords(state:State<'_,CatalogWriterState>,listing:State<'_,ListingState>,asset_ids:Vec<i64>,keyword_id:i64,assign:bool)->Result<SetAssetKeywordsResult,String>{
    let mut candidates=Vec::new();let mut file_ids=asset_ids.iter().copied().filter(|id|*id>=0).collect::<Vec<_>>();
    for id in asset_ids.iter().copied().filter(|id|*id<0){let Some(asset)=listing.0.asset(id)else{continue};if asset.is_directory||asset.ext.is_empty(){continue;}let path=asset.path.ok_or_else(||format!("Listed file {id} has no resolved path"))?;let folder_path=Path::new(&path).parent().map(|p|p.to_string_lossy().to_string()).ok_or("Parent folder is unavailable")?;candidates.push(ListedAsset{temporary_id:id,name:asset.name,ext:asset.ext,size:asset.size,file_modified:asset.file_modified,path,folder_path});file_ids.push(id);}
    let result=state.0.set_asset_keywords(&file_ids,&candidates,keyword_id,assign)?;for id in file_ids.iter().copied().filter(|id|*id>=0){let _=listing.0.invalidate_asset_parent(id);}for candidate in candidates{let _=listing.0.invalidate_path(&candidate.folder_path);}Ok(result)
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
pub fn search_keyword_options(state:State<'_, BackgroundDbState>,query:String)->Result<Vec<db::KeywordOption>,String>{db::keyword_options(&state.0,&query).map_err(|e|e.to_string())}

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

#[tauri::command]
pub async fn get_assets(
    app: tauri::AppHandle,
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    ai: State<'_, AiState>,
    writer: State<'_, CatalogWriterState>,
    file_manager: State<'_, FileManagerState>,
    filter: AssetFilter,
) -> Result<db::AssetPage, String> {
    let (db, listing, formats, ai, writer, file_manager) = (
        state.0.clone(), listing.0.clone(), formats.0.clone(), ai.0.clone(), writer.0.clone(), file_manager.0.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        // Only the first page checks for new files and missing catalogue dates.
        if filter.kind == "folder" && filter.offset == 0 && writer.is_writable() {
            let _operation = file_manager.lock_operation()?;
            if auto_catalog_folder(&listing, &formats, &writer, &ai, &filter).is_ok_and(|count| count > 0) {
                let _ = app.emit("folder-catalogue-changed", ());
            }
        }
        let mut page = if filter.sort == "similarity" && filter.kind != "series" && filter.kind != "people"
            && !(filter.kind == "special" && filter.value.as_deref() == Some("edit_pending")) {
            similarity_page(&db, &listing, &formats, &filter, &ai.status(), |ids| ai.order_by_similarity(ids))?
        } else if filter.kind == "folder" {
            listing.page(&filter)?
        } else {
            db::assets_page(&db, &filter, &formats).map_err(|e| e.to_string())?
        };
        classify_page(&mut page);
        Ok(page)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn locate_asset(
    app: tauri::AppHandle,
    filter: AssetFilter,
    id: i64,
) -> Option<usize> {
    if filter.sort == "similarity" || matches!(filter.kind.as_str(), "ai" | "series" | "people") {
        return None;
    }
    let db = app.state::<DatabaseState>().0.clone();
    let listing = app.state::<ListingState>().0.clone();
    let formats = app.state::<FormatsState>().0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if filter.kind == "folder" {
            listing.locate_asset(&filter, id).ok().flatten()
        } else {
            db::locate_asset(&db, &filter, &formats, id).ok().flatten()
        }
    }).await.ok().flatten()
}

#[tauri::command(async)]
pub fn get_folder_previews(state: State<'_, BackgroundDbState>, formats: State<'_, FormatsState>, paths: Vec<String>) -> Result<std::collections::HashMap<String, Vec<db::FolderPreviewAsset>>, String> {
    db::folder_previews(&state.0, &paths, &formats.0).map_err(|e| e.to_string())
}
fn auto_catalog_folder(listing:&ListingService,formats:&crate::formats::FormatTable,writer:&crate::journal::CatalogWriter,ai:&crate::ai::AiService,filter:&AssetFilter)->Result<usize,String>{
    let (path,files)=listing.listed_folder(filter)?;
    writer.observe_incoming_folder(&path)?;
    let fulfilled = writer.fulfil_edits_from_listing(&files)?;
    if fulfilled > 0 { listing.invalidate_path(&path)?; }
    let dates = listing.missing_capture_dates(files.iter().filter(|asset| asset.id > 0));
    let backfilled = writer.backfill_capture_dates(&dates)?;
    if backfilled > 0 { listing.invalidate_path(&path)?; }
    let updated = backfilled + fulfilled;
    if path.is_empty() || !files.iter().any(|asset| !asset.is_directory && asset.id < 0) { return Ok(updated); }
    let mode = writer.effective_folder_mode(&path);
    let included = mode.as_deref() == Some("include");
    if mode.as_deref() == Some("exclude") || (!included &&
        !ai.incoming_folder().is_ok_and(|folder| crate::incoming::contains_folder(&path, &folder))) {
        return Ok(updated);
    }
    let candidates=files.iter().filter(|a| !a.is_directory && a.id < 0).filter(|a|match formats.group(&a.ext) {
        Some("image" | "raw") => true,
        Some("video") => included,
        _ => false,
    }).filter_map(|a|{let file_path=a.path.clone()?;let folder_path=Path::new(&file_path).parent().map(|p|p.to_string_lossy().to_string()).unwrap_or_default();Some(ListedAsset{temporary_id:a.id,name:a.name.clone(),ext:a.ext.clone(),size:a.size,file_modified:a.file_modified.clone(),path:file_path,folder_path})}).collect::<Vec<_>>();
    if candidates.is_empty(){return Ok(updated);}
    let dates = listing.missing_capture_dates(files.iter().filter(|asset| asset.id < 0));
    let remaps = if dates.is_empty() {
        writer.catalog_listed_assets(&candidates)?
    } else {
        writer.catalog_listed_assets_with_dates(&candidates, &dates)?
    };
    if !remaps.is_empty(){let _=listing.invalidate_path(&path);ai.index_assets(&remaps.iter().map(|r|r.new_id).collect::<Vec<_>>());}
    Ok(remaps.len() + updated)
}
#[tauri::command(async)]
pub fn refresh_folder_listing(listing: State<'_, ListingState>, folder_id: i64) -> Result<(), String> {
    listing.0.invalidate(folder_id)
}
#[tauri::command(async)]
pub async fn check_folder_changed(listing: State<'_, ListingState>, folder_id: i64) -> Result<bool, String> {
    let service = listing.0.clone();
    Ok(tauri::async_runtime::spawn_blocking(move || service.check_folder_changed(folder_id))
        .await.unwrap_or(false))
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
            command.raw_arg(crate::file_ops::explorer_select_arg(Path::new(&path)));
        }
        _ => return Err("Neznáma akcia".into()),
    }
    command.spawn().map(|_|()).map_err(|e|e.to_string())
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Editor { name: String, path: String, multi: bool }

fn read_editor_settings(path: &Path) -> Result<Vec<Editor>, String> {
    let settings = crate::incoming::read_settings(path)?;
    match settings.get("editors") { Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()), None => Ok(vec![]) }
}
fn write_editor_settings(path: &Path, editors: &[Editor]) -> Result<(), String> {
    crate::incoming::write_setting(path, "editors", serde_json::to_value(editors).map_err(|e| e.to_string())?)
}
#[tauri::command(async)]
pub fn get_editors(info: State<'_, AppInfoState>) -> Result<Vec<Editor>, String> { read_editor_settings(Path::new(&info.0.settings)) }
#[tauri::command(async)]
pub fn set_editors(info: State<'_, AppInfoState>, editors: Vec<Editor>) -> Result<(), String> { write_editor_settings(Path::new(&info.0.settings), &editors) }

fn newest_photoshop(paths: impl IntoIterator<Item=PathBuf>) -> Option<PathBuf> {
    paths.into_iter().filter(|p| p.is_file()).max_by(|a,b| a.parent().cmp(&b.parent()))
}
#[tauri::command(async)]
pub fn detect_photoshop() -> Option<Editor> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("reg.exe").args(["query", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Photoshop.exe", "/ve"]).creation_flags(0x0800_0000).output().ok();
    let registry = output.filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find_map(|line| line.split_once("REG_SZ").map(|(_,v)| PathBuf::from(v.trim().trim_matches('"'))))).filter(|p| p.is_file());
    let fallback = || { let root = Path::new(r"C:\Program Files\Adobe"); newest_photoshop(std::fs::read_dir(root).ok()?.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with("adobe photoshop")).map(|e| e.path().join("Photoshop.exe"))) };
    registry.or_else(fallback).map(|p| Editor { name: "Adobe Photoshop".into(), path: p.to_string_lossy().into_owned(), multi: true })
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test] fn editor_settings_round_trip() {
        let dir = std::env::temp_dir().join(format!("ddreams-editors-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap(); let path=dir.join("settings.json");
        std::fs::write(&path, r#"{"other":42}"#).unwrap(); let editors=vec![Editor{name:"Test".into(),path:"x.exe".into(),multi:true}];
        write_editor_settings(&path,&editors).unwrap(); assert_eq!(read_editor_settings(&path).unwrap(),editors);
        let json: Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(); assert_eq!(json["other"],42);
        write_editor_settings(&path,&[]).unwrap(); assert!(read_editor_settings(&path).unwrap().is_empty()); std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn editor_lookup_accepts_saved_and_detected_paths_and_rejects_others() {
        let saved = Editor { name: "Test".into(), path: r"C:\Editory\Úpravy.exe".into(), multi: false };
        let photoshop = Editor { name: "Adobe Photoshop".into(), path: r"C:\Adobe\Photoshop.exe".into(), multi: true };
        let detect = || Some(photoshop.clone());
        assert_eq!(allowed_editor(std::slice::from_ref(&saved), r"c:\editory\úpravy.EXE", || panic!("saved editor should skip detection")).unwrap(), saved);
        assert_eq!(allowed_editor(&[], r"c:\adobe\photoshop.EXE", detect).unwrap(), photoshop);
        assert_eq!(allowed_editor(std::slice::from_ref(&saved), r"c:\adobe\photoshop.EXE", detect).unwrap(), photoshop);
        assert!(allowed_editor(&[saved], r"C:\other.exe", detect).is_err());
    }

    #[test] fn photoshop_fallback_selects_newest_installed() {
        let dir=std::env::temp_dir().join(format!("ddreams-photoshop-{}-{:?}",std::process::id(),std::thread::current().id()));
        let old=dir.join("Adobe Photoshop 2024/Photoshop.exe");let new=dir.join("Adobe Photoshop 2026/Photoshop.exe");let missing=dir.join("Adobe Photoshop 2027/Photoshop.exe");
        for p in [&old,&new] { std::fs::create_dir_all(p.parent().unwrap()).unwrap();std::fs::write(p,"").unwrap(); }
        assert_eq!(newest_photoshop([old,new.clone(),missing]),Some(new));std::fs::remove_dir_all(dir).unwrap();
    }
}

fn allowed_editor(editors: &[Editor], requested: &str, detect: impl FnOnce() -> Option<Editor>) -> Result<Editor, String> {
    let requested = requested.trim().to_lowercase();
    let matches = |editor: &Editor| editor.path.trim().to_lowercase() == requested;
    if !requested.is_empty() {
        if let Some(editor) = editors.iter().find(|editor| matches(editor)).cloned().or_else(|| detect().filter(matches)) {
            return Ok(editor);
        }
    }
    Err("Editor nie je uložený v nastaveniach ani automaticky zistený".into())
}

/// Opens the given files in an external editor saved in Settings or detected by DDreams. `multi` = the program takes all
/// files in one launch; otherwise it is started once per file (at most 20).
#[tauri::command(async)]
pub fn open_in_editor(db: State<'_, DatabaseState>, listing: State<'_, ListingState>, info: State<'_, AppInfoState>, asset_ids: Vec<i64>, exe: String) -> Result<usize, String> {
    let editor = allowed_editor(&read_editor_settings(Path::new(&info.0.settings))?, &exe, detect_photoshop)?;
    let exe = std::path::absolute(editor.path.trim()).map_err(|e| e.to_string())?;
    if !exe.is_file() { return Err(format!("Program sa nenašiel: {}", exe.display())); }
    let paths: Vec<String> = asset_ids.into_iter()
        .filter_map(|id| listing.0.asset(id).and_then(|a|a.path).or_else(||db::asset_metadata(&db.0,id).ok().flatten().and_then(|m|m.asset.path)))
        .filter(|p| Path::new(p).is_file()).collect();
    if paths.is_empty() { return Err("Žiadny z vybraných súborov nie je dostupný".into()); }
    if editor.multi {
        std::process::Command::new(&exe).args(&paths).spawn().map_err(|e| e.to_string())?;
    } else {
        if paths.len() > 20 { return Err("Tento program berie súbory po jednom – vyber najviac 20 súborov".into()); }
        for path in &paths { std::process::Command::new(&exe).arg(path).spawn().map_err(|e| e.to_string())?; }
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

pub static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Appends "<ms since process start> <label>" to cache/startup.log (the file restarts with the
/// first mark of a run). Used to see where a slow cold start spends its time.
#[tauri::command(async)]
pub fn startup_mark(info: State<'_, AppInfoState>, label: String) {
    use std::io::Write;
    static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    let ms = PROCESS_START.get().map(|t| t.elapsed().as_millis()).unwrap_or(0);
    let path = Path::new(&info.0.cache).join("startup.log");
    let first = FIRST.swap(false, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).write(true).truncate(first).append(!first).open(path) {
        let _ = writeln!(file, "{ms:>7} ms  {}", label.chars().take(120).collect::<String>());
    }
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
    if filter.kind == "ai" || filter.kind == "series" || filter.kind == "people" { let mut f=filter;f.offset=offset;f.limit=limit;return Ok(db::assets_page(&state.0,&f,&formats.0).map_err(|e|e.to_string())?.items.into_iter().map(|a|a.id).collect()); }
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
            let filter=AssetFilter{kind:"folder".into(),id:Some(parent),root_id:None,include_children:false,folder_path:Some(path),ids:parent_ids.clone(),value:None,query:None,sort:"name".into(),descending:false,offset:0,limit:500,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false, category_filter: None, keyword_filter: None,include_categories:false,include_keywords:false,date_from:None,date_to:None,date_source:None,date_no_date:false,file_group:None};
            let page=listing.0.page(&filter)?;
            let mut dirs=Vec::new();
            for asset in page.items.into_iter().filter(|a|a.is_directory) {
                let matched=asset.path.as_deref().and_then(|p|db::folder_by_path(&state.0,p).ok().flatten());
                let id=matched.as_ref().map(|f|f.id).unwrap_or(asset.id);
                dirs.push(db::Folder{id,parent_id:Some(parent),root_id:matched.as_ref().and_then(|f|f.root_id),name:asset.name,path:asset.path,count:matched.as_ref().map(|f|f.count).unwrap_or(0),folder_ids:matched.map(|f|f.folder_ids).unwrap_or_else(||vec![id]),virtual_node:None,offline:false,missing:false});
            }
            dirs.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return Ok(tree_children_from_listing(dirs,Vec::new()).into_iter().skip(offset.max(0) as usize).take(limit.clamp(1,500) as usize).collect());
        }
    }
    let mut rows=db::folders_page(&state.0,parent_id,parent_ids,0,500).map_err(|e|e.to_string())?;
    let mut volumes=std::collections::HashMap::new();let mut live=Vec::new();
    for mut folder in rows.drain(..){
        if folder.virtual_node.as_deref()==Some("devices"){
            let mut ids=Vec::new();for id in folder.folder_ids{let path=db::folder_ancestors(&state.0,id).ok().and_then(|chain|chain.last().and_then(|f|f.path.clone()));if path.as_deref().is_some_and(|p|Path::new(p).is_dir()){ids.push(id)}}
            if !ids.is_empty(){folder.folder_ids=ids;live.push(folder)}
        }else if let Some(path)=folder.path.as_deref(){let (offline,missing)=classify_source(path,&mut volumes);folder.offline=offline;folder.missing=missing;if !offline&&!missing{live.push(folder)}}
    }
    Ok(live.into_iter().skip(offset.max(0) as usize).take(limit.clamp(1,500) as usize).collect())
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
    state: State<'_, BackgroundDbState>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Root>, String> {
    db::roots_page(&state.0, offset, limit, include_children).map_err(|e| e.to_string())
}
#[tauri::command(async)]
pub fn get_catalog_nodes(
    state: State<'_, BackgroundDbState>,
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
pub fn search_catalog_tree(state: State<'_, BackgroundDbState>, kind:String, query:String, include_children:bool, offset:i64, limit:i64)->Result<Vec<db::CatalogNode>,String>{
    db::catalog_tree_search(&state.0,&kind,&query,offset,limit,include_children).map_err(|e|e.to_string())
}
#[tauri::command(async)]
pub fn get_catalog_values(
    state: State<'_, BackgroundDbState>,
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

struct SimilarityOrder {
    ids: Vec<i64>,
    total_size: f64,
    folder: Option<(Arc<Vec<db::Asset>>, std::collections::HashMap<i64, usize>)>,
}

struct SimilarityEntry {
    owner: std::sync::Weak<std::sync::Mutex<rusqlite::Connection>>,
    filter: String,
    revision: (u64, i64, i64, i64, u64, bool, u64, u64),
    at: std::time::Instant,
    order: Arc<SimilarityOrder>,
}

fn similarity_page(db: &Db, listing: &ListingService, formats: &crate::formats::FormatTable, filter: &AssetFilter, status: &crate::ai::AiStatus, rank: impl FnOnce(Vec<i64>) -> Result<Vec<i64>, String>) -> Result<db::AssetPage, String> {
    static CACHE: std::sync::Mutex<Vec<SimilarityEntry>> = std::sync::Mutex::new(Vec::new());
    let mut base = filter.clone();
    base.sort = "name".into();
    base.descending = false;
    base.offset = 0;
    base.limit = 0;
    base.include_categories = false;
    base.include_keywords = false;
    let key = serde_json::to_string(&base).map_err(|e| e.to_string())?;
    let revision = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let version = |sql| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap_or(-1);
        (conn.total_changes(), version("PRAGMA main.data_version"),
         version("PRAGMA ai.data_version"), version("PRAGMA auto_exif.data_version"),
         if base.kind == "folder" { listing.revision() } else { 0 }, status.available, status.indexed, status.done)
    };
    let cached = {
        let mut cache = CACHE.lock().map_err(|e| e.to_string())?;
        cache.retain(|entry| entry.owner.strong_count() > 0 && entry.at.elapsed().as_secs() < 8);
        cache.iter().position(|entry| entry.owner.ptr_eq(&Arc::downgrade(db)) && entry.filter == key && entry.revision == revision)
            .map(|pos| { let entry = cache.remove(pos); let order = entry.order.clone(); cache.push(entry); order })
    };
    let order = if let Some(order) = cached { order } else {
        let (mut ids, total_size, folder, directories) = if base.kind == "folder" {
            let (rows, total_size) = listing.ordered_rows(&base)?;
            let positions = rows.iter().enumerate().map(|(i, asset)| (asset.id, i)).collect();
            let ids = rows.iter().map(|asset| asset.id).collect::<Vec<_>>();
            let directories = rows.iter().take_while(|asset| asset.is_directory).count();
            (ids, total_size, Some((rows, positions)), directories)
        } else {
            let conn = db.lock().map_err(|e| e.to_string())?;
            let (ids, total_size) = db::catalog_ordered_ids(db, &conn, &base, formats).map_err(|e| e.to_string())?;
            (ids.as_ref().clone(), total_size, None, 0)
        };
        if status.available && directories < ids.len() {
            let ranked = rank(ids[directories..].to_vec())?;
            rank_similarity_ids(&mut ids[directories..], &ranked);
        }
        let order = Arc::new(SimilarityOrder { ids, total_size, folder });
        let mut cache = CACHE.lock().map_err(|e| e.to_string())?;
        cache.retain(|entry| !(entry.owner.ptr_eq(&Arc::downgrade(db)) && entry.filter == key));
        if cache.len() >= 2 { cache.remove(0); }
        cache.push(SimilarityEntry { owner: Arc::downgrade(db), filter: key, revision, at: std::time::Instant::now(), order: order.clone() });
        order
    };
    let ids = &order.ids;
    let start = (filter.offset.max(0) as usize).min(ids.len());
    let end = (start + filter.limit.clamp(1,500) as usize).min(ids.len());
    let ids_page = &ids[start..end];
    let items = if let Some((rows, positions)) = &order.folder {
        let mut items = ids_page.iter().filter_map(|id| positions.get(id).map(|&position| rows[position].clone())).collect::<Vec<_>>();
        db::attach_display_values(db, &mut items, filter.include_categories, filter.include_keywords).map_err(|e| e.to_string())?;
        items
    } else {
        let conn = db.lock().map_err(|e| e.to_string())?;
        db::hydrate_asset_ids(&conn, ids_page, filter).map_err(|e| e.to_string())?
    };
    Ok(db::AssetPage { items, total: ids.len() as i64, total_size: order.total_size })
}

fn rank_similarity_ids(ids: &mut [i64], ranked: &[i64]) {
    let positions = ranked.iter().enumerate().map(|(i, id)| (*id, i)).collect::<std::collections::HashMap<_,_>>();
    ids.sort_by_key(|id| positions.get(id).copied().unwrap_or(usize::MAX));
}

#[cfg(test)]
#[test]
fn similarity_order_preserves_unranked_ids_and_directory_prefix() {
    let mut ids = [-1, 2, 3, 1, 4];
    rank_similarity_ids(&mut ids[1..], &[3, 4]);
    assert_eq!(ids, [-1, 3, 4, 2, 1]);
}

#[cfg(test)]
#[test]
fn similarity_cache_reuses_order_and_invalidates_when_rows_change() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);
                              CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER);
                              CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER);
                              CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT);
                              CREATE TABLE keywords(id INTEGER PRIMARY KEY,name TEXT);").unwrap();
    let measure = std::env::var_os("DDREAMS_BENCH_SIMILARITY_SYNTHETIC").is_some();
    let count = if measure { 71_000 } else { 1_200 };
    connection.execute("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?)
                        INSERT INTO assets(id,name,ext,ext_lower,is_listed,size) SELECT id,printf('%06d.jpg',id),'jpg','jpg',1,100 FROM n", [count]).unwrap();
    let db = Arc::new(std::sync::Mutex::new(connection));
    let formats = crate::formats::FormatTable::load_selected(None);
    let listing = ListingService::new(db.clone(), formats.clone());
    let filter: AssetFilter = serde_json::from_value(serde_json::json!({"kind":"special","value":"all_photos","sort":"similarity","descending":false,"offset":0,"limit":120})).unwrap();
    let status: crate::ai::AiStatus = crate::ai::AiStatus {
        available:true, reason:None, state:"ready".into(), model_state:"ready".into(), waiting:0, indexed:count as u64,
        queued:0, done:0, failed:0, offline:0, total:count as u64, rate_per_s:0.0, eta_s:None, last_error:None,
    };
    let ranks = std::cell::Cell::new(0);
    let rank = |mut ids: Vec<i64>| { ranks.set(ranks.get()+1); ids.reverse(); Ok(ids) };
    let first = similarity_page(&db, &listing, &formats, &filter, &status, rank).unwrap();
    assert_eq!(first.items[0].id, count);
    let mut next = filter.clone();
    next.offset = 120;
    let after_start = std::time::Instant::now();
    let second = similarity_page(&db, &listing, &formats, &next, &status, rank).unwrap();
    let after = after_start.elapsed();
    assert_eq!(second.items[0].id, count-120);
    assert_eq!(ranks.get(), 1);
    if measure {
        // Old warm-page path: read every Asset row, then apply the cached AI order to all rows.
        let mut base = filter.clone();
        base.sort = "name".into();
        base.limit = 500;
        let before_start = std::time::Instant::now();
        let mut all = Vec::new();
        for offset in (0..count).step_by(500) {
            base.offset = offset;
            all.extend(db::assets_page(&db, &base, &formats).unwrap().items);
        }
        let positions: std::collections::HashMap<_,_> = (1..=count).rev().enumerate().map(|(position, id)| (id, position)).collect();
        all.sort_by_key(|asset| positions[&asset.id]);
        assert_eq!(all[120].id, second.items[0].id);
        eprintln!("similarity synthetic warm page: old {:.2?}, new {:.2?}, assets {count}", before_start.elapsed(), after);
    }
    db.lock().unwrap().execute("UPDATE assets SET is_listed=0 WHERE id=?", [count]).unwrap();
    let refreshed = similarity_page(&db, &listing, &formats, &filter, &status, rank).unwrap();
    assert_eq!(refreshed.items[0].id, count-1);
    assert_eq!(ranks.get(), 2);
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
        let rows=if online { crate::iptc_write::metadata_with_sidecar(&exif.0, Path::new(&path)).map(|json|live_rows(json,&view.0,None)).unwrap_or_default() } else { Vec::new() };
        return Ok(Some(MetadataResponse{asset,offline:!online,rows}));
    }
    let Some(data) = db::asset_metadata(&db.0, asset_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = data.asset.path.as_deref().unwrap_or("");
    let online = Path::new(path).is_file();
    if online {
        if let Ok(value) = crate::iptc_write::metadata_with_sidecar(&exif.0, Path::new(path)) {
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

#[tauri::command]
pub async fn prepare_full_image(
    state: State<'_, ThumbnailState>, asset_id: i64, raw_decode: bool,
) -> Result<Option<thumbs::FullImageInfo>, String> {
    let thumbs = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || thumbs.prepare_full_image(asset_id, raw_decode))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn read_full_image_chunk(
    state: State<'_, ThumbnailState>, asset_id: i64, raw_decode: bool, key: String, offset: u64,
) -> Result<tauri::ipc::Response, String> {
    let thumbs = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || thumbs.read_full_image_chunk(asset_id, raw_decode, &key, offset))
        .await.map_err(|e| e.to_string())?.map(tauri::ipc::Response::new)
}

#[tauri::command(async)]
pub fn folder_exists(path:String)->bool{Path::new(path.trim()).is_dir()}

#[tauri::command(async)]
pub fn get_delete_info(state:State<'_,FileManagerState>,asset_ids:Vec<i64>,permanent:bool)->Result<crate::file_ops::DeleteInfo,String>{state.0.delete_info(&asset_ids,permanent)}

/// Deletes the selected files (with sidecars): to the Recycle Bin or permanently when
/// Shift+Delete is requested or permanent deletion without a Recycle Bin is confirmed.
#[tauri::command]
pub async fn delete_files(
    state: State<'_, FileManagerState>, ai: State<'_, AiState>, asset_ids: Vec<i64>,
    permanent: Option<bool>, confirmed_no_bin_drives: Option<Vec<String>>,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    let (result, purged) = tauri::async_runtime::spawn_blocking(move || {
        let mut purged = Vec::new();
        let result = manager.delete_files(&asset_ids, permanent.unwrap_or(false), &confirmed_no_bin_drives.unwrap_or_default(), &mut purged);
        (result, purged)
    }).await.map_err(|e| e.to_string())?;
    ai.0.forget(purged);
    result
}

#[tauri::command(async)]
pub fn get_folder_rules(writer:State<'_,CatalogWriterState>)->Result<Vec<crate::folder_rules::FolderRule>,String>{writer.0.folder_rules()}

#[tauri::command(async)]
pub fn get_folder_rule_state(writer:State<'_,CatalogWriterState>,path:String)->Result<crate::folder_rules::FolderRuleState,String>{writer.0.folder_rule_state(&path)}

#[tauri::command(async)]
pub fn get_folder_catalogue_states(
    writer: State<'_, CatalogWriterState>,
    paths: Vec<String>,
) -> Result<std::collections::HashMap<String, crate::folder_rules::FolderCatalogueState>, String> {
    writer.0.folder_catalogue_states(paths)
}

#[tauri::command(async)]
pub fn get_exclusion_impact(writer:State<'_,CatalogWriterState>,path:String)->Result<crate::journal::ExclusionImpact,String>{writer.0.exclusion_impact(&path)}

/// mode: "include" | "exclude" | null (remove the explicit rule). Excluding removes the folder's catalogue rows.
#[tauri::command(async)]
pub fn set_folder_rule(writer:State<'_,CatalogWriterState>,ai:State<'_,AiState>,listing:State<'_,ListingState>,path:String,mode:Option<String>)->Result<usize,String>{
    let removed=writer.0.set_folder_rule(&path,mode.as_deref())?;
    let count=removed.len();
    if count>0{ai.0.forget(removed);}
    let _=listing.0.invalidate_path(&path);
    Ok(count)
}

#[cfg(test)]
mod source_tests{
    use super::*;
    #[test]
    fn missing_and_offline_sources_are_mutually_exclusive_and_volume_is_cached(){
        let mut cache=std::collections::HashMap::new();let mut probes=0;
        let offline=classify_source_with("X:\\photos",&mut cache,|p|{probes+=1;p!="X:\\"});
        assert_eq!(offline,(true,false));assert_eq!(probes,1);let second=classify_source_with("X:\\other",&mut cache,|p|{probes+=1;p!="X:\\"});assert_eq!(second,(true,false));assert_eq!(probes,1,"volume probe is cached and unreachable paths are not probed");
        let mut cache=std::collections::HashMap::new();let missing=classify_source_with("F:\\missing",&mut cache,|p|p=="F:\\");
        assert_eq!(missing,(false,true));
        assert_eq!(require_missing_source(false,false).unwrap_err(),"Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie");
    }

    #[test]
    fn settings_sources_are_topmost_and_include_descendant_asset_counts(){
        let nodes=vec![
            CatalogFolderNode{id:1,parent:None,path:"X:\\archive".into(),count:2},
            CatalogFolderNode{id:2,parent:Some(1),path:"X:\\archive\\child".into(),count:3},
            CatalogFolderNode{id:3,parent:None,path:"F:\\missing".into(),count:4},
            CatalogFolderNode{id:4,parent:Some(3),path:"F:\\missing\\child".into(),count:5},
            CatalogFolderNode{id:5,parent:None,path:"F:\\live".into(),count:1},
        ];
        let sources=collect_catalog_missing_sources(nodes,|p|p=="F:\\"||p=="F:\\live");
        assert_eq!(sources.len(),2);
        assert_eq!((sources[0].path.as_str(),sources[0].assets,sources[0].offline,sources[0].missing),("X:\\archive",5,true,false));
        assert_eq!((sources[1].path.as_str(),sources[1].assets,sources[1].offline,sources[1].missing),("F:\\missing",9,false,true));
    }

    #[test]
    fn tree_child_rows_ignore_catalogue_only_folders(){
        let live=db::Folder{id:1,parent_id:Some(10),root_id:Some(10),name:"exists".into(),path:Some("F:\\root\\exists".into()),count:4,folder_ids:vec![1],virtual_node:None,offline:false,missing:false};
        let catalogue_only=db::Folder{id:2,parent_id:Some(10),root_id:Some(10),name:"removed".into(),path:Some("F:\\root\\removed".into()),count:2,folder_ids:vec![2],virtual_node:None,offline:false,missing:false};
        let rows=tree_children_from_listing(vec![live],vec![catalogue_only]);
        assert_eq!(rows.len(),1);assert_eq!(rows[0].name,"exists");
    }

}
