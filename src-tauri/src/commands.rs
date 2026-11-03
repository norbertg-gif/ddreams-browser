use crate::{
    db::{self, AssetFilter, AssetMetadata, Db},
    exiftool::ExifTool,
    formats::FormatTable,
    listing::ListingService,
    metadata_view::{self, MetadataView},
    thumbs::ThumbService,
};
use crate::thumbs;
use serde::Serialize;
use serde_json::Value;
use std::{path::Path, sync::Arc};
use tauri::State;

pub struct DatabaseState(pub Db);
pub struct ExifState(pub Arc<ExifTool>);
pub struct ThumbnailState(pub Arc<ThumbService>);
pub struct ListingState(pub Arc<ListingService>);
pub struct FormatsState(pub Arc<FormatTable>);
pub struct MetadataViewState(pub Arc<MetadataView>);

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
            let filter=AssetFilter{kind:"folder".into(),id:Some(parent),root_id:None,include_children:false,folder_path:Some(path),ids:parent_ids.clone(),value:None,query:None,sort:"name".into(),descending:false,offset:0,limit:500,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false,file_group:None};
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
    add("Rating", a.rating.filter(|v| *v > 0).map(|v| v.to_string())); add("Label", a.label.clone());
    add("Tagged", a.tagged.map(|v| if v != 0 { "Áno" } else { "Nie" }.into()));
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
pub fn get_catalog_asset_links(state:State<'_,DatabaseState>,asset_id:i64)->Result<Option<db::CatalogAssetLinks>,String>{
    db::asset_catalog_links(&state.0,asset_id).map_err(|e|e.to_string())
}

#[tauri::command]
pub fn get_formats(state: State<'_, FormatsState>) -> FormatTable { state.0.as_ref().clone() }

#[tauri::command]
pub fn get_preview_source(state: State<'_, ThumbnailState>, asset_id: i64, epoch: u64) -> Result<String, String> {
    state.0.render(asset_id, 0, epoch).map(|result| result.source).map_err(|e| format!("{e:?}"))
}
