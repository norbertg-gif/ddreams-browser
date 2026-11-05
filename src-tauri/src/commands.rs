use crate::{
    calendar::{CalendarAggregate, CalendarService, DateSource},
    db::{self, AssetFilter, AssetMetadata, Db},
    display_metadata::{self, CacheEntry, DisplayMetadataCache, TileExifItem, TileExifValues},
    exiftool::ExifTool,
    formats::FormatTable,
    journal::{CatalogWriter, ChangeSummary, ListedAsset, SetAssetCategoriesResult, SetAssetValuesResult},
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

pub struct DatabaseState(pub Db);
pub struct ExifState(pub Arc<ExifTool>);
pub struct ThumbnailState(pub Arc<ThumbService>);
pub struct ListingState(pub Arc<ListingService>);
pub struct FormatsState(pub Arc<FormatTable>);
pub struct MetadataViewState(pub Arc<MetadataView>);
pub struct DisplayMetadataState(pub Arc<DisplayMetadataCache>);
pub struct CalendarState(pub Arc<CalendarService>);
pub struct CatalogWriterState(pub Arc<CatalogWriter>);
pub struct AppInfoState(pub AppInfo);

#[tauri::command]
pub fn get_app_info(state: State<'_, AppInfoState>) -> AppInfo { state.0.clone() }

#[tauri::command]
pub fn get_recent_changes(state: State<'_, CatalogWriterState>, limit: i64) -> Result<Vec<ChangeSummary>, String> {
    state.0.recent_changes(limit)
}

#[tauri::command]
pub fn undo_last_change(state: State<'_, CatalogWriterState>) -> Result<Option<ChangeSummary>, String> {
    state.0.undo_last()
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub fn get_category_selection_counts(db:State<'_,DatabaseState>,listing:State<'_,ListingState>,asset_ids:Vec<i64>)->Result<CategorySelectionCounts,String>{
    let mut positive=Vec::new();let mut total=0usize;
    for id in asset_ids{if id<0{if listing.0.asset(id).is_some_and(|a|!a.is_directory&&!a.ext.is_empty()){total+=1;}}else{positive.push(id);}}
    let conn=db.0.lock().map_err(|_|"catalog lock poisoned")?;
    for chunk in positive.chunks(500){let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT count(*) FROM assets WHERE is_listed=1 AND ext_lower<>'' AND id IN ({marks})");total+=conn.query_row(&sql,rusqlite::params_from_iter(chunk.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? as usize;}
    let mut assigned=std::collections::HashMap::new();
    for chunk in positive.chunks(500){if chunk.is_empty(){continue;}let marks=std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT ac.category_id,count(DISTINCT ac.asset_id) FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id WHERE a.is_listed=1 AND a.ext_lower<>'' AND ac.asset_id IN ({marks}) GROUP BY ac.category_id");let mut stmt=conn.prepare(&sql).map_err(|e|e.to_string())?;let rows=stmt.query_map(rusqlite::params_from_iter(chunk.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).map_err(|e|e.to_string())?;for row in rows{let(id,count)=row.map_err(|e|e.to_string())?;*assigned.entry(id).or_insert(0)+=count as usize;}}
    Ok(CategorySelectionCounts{total_files:total,assigned})
}

#[tauri::command]
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
pub fn get_assets(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    filter: AssetFilter,
) -> Result<db::AssetPage, String> {
    if filter.kind == "folder" { return listing.0.page(&filter); }
    db::assets_page(&state.0, &filter, &formats.0).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn refresh_folder_listing(listing: State<'_, ListingState>, folder_id: i64) -> Result<(), String> {
    listing.0.invalidate(folder_id)
}
#[tauri::command]
pub fn get_asset_paths(db: State<'_, DatabaseState>, listing: State<'_, ListingState>, ids: Vec<i64>) -> Result<Vec<String>, String> {
    Ok(ids.into_iter().filter_map(|id| listing.0.asset(id).and_then(|a|a.path).or_else(||db::asset_metadata(&db.0,id).ok().flatten().and_then(|m|m.asset.path))).collect())
}
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
pub fn find_folder_path(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    path: String,
) -> Result<Option<db::Folder>, String> {
    if let Some(folder)=db::folder_by_path(&state.0,&path).map_err(|e|e.to_string())? { return Ok(Some(folder)); }
    listing.0.resolve_folder(&path)
}
#[tauri::command]
pub fn get_folder_ancestors(
    state: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    id: i64,
) -> Result<Vec<db::Folder>, String> {
    let _=state;
    listing.0.folder_ancestors(id)
}
#[tauri::command]
pub fn get_category_roots(
    state: State<'_, DatabaseState>,
    include_children: bool,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::Root>, String> {
    db::roots_page(&state.0, offset, limit, include_children).map_err(|e| e.to_string())
}
#[tauri::command]
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
#[tauri::command]
pub fn search_keywords(
    state: State<'_, DatabaseState>,
    query: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::CatalogNode>, String> {
    db::keyword_search_page(&state.0, &query, offset, limit).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn search_catalog_tree(state: State<'_, DatabaseState>, kind:String, query:String, include_children:bool, offset:i64, limit:i64)->Result<Vec<db::CatalogNode>,String>{
    db::catalog_tree_search(&state.0,&kind,&query,offset,limit,include_children).map_err(|e|e.to_string())
}
#[tauri::command]
pub fn get_catalog_values(
    state: State<'_, DatabaseState>,
    formats: State<'_, FormatsState>,
    kind: String,
    offset: i64,
    limit: i64,
) -> Result<Vec<db::ValueCount>, String> {
    db::values_page(&state.0, &kind, offset, limit, &formats.0).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn set_thumbnail_epoch(state: State<'_, ThumbnailState>, epoch: u64) {
    state.0.set_epoch(epoch)
}
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
pub fn get_catalog_asset_metadata(state:State<'_,DatabaseState>,asset_id:i64)->Result<Option<AssetMetadata>,String>{
    db::asset_metadata(&state.0,asset_id).map_err(|e|e.to_string())
}

#[tauri::command]
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
#[tauri::command]
pub fn get_catalog_asset_links(state:State<'_,DatabaseState>,asset_id:i64)->Result<Option<db::CatalogAssetLinks>,String>{
    db::asset_catalog_links(&state.0,asset_id).map_err(|e|e.to_string())
}

#[tauri::command]
pub fn get_formats(state: State<'_, FormatsState>) -> FormatTable { state.0.as_ref().clone() }

#[tauri::command]
pub fn get_preview_source(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> Result<String, String> {
    state.0.render(asset_id, 0, epoch).map(|result| result.source).map_err(|e| format!("{e:?}"))
}
