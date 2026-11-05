mod commands;
mod backup;
mod calendar;
mod db;
mod display_metadata;
mod exiftool;
mod formats;
pub mod journal;
mod listing;
mod metadata_view;
mod paths;
mod schema;
mod thumbs;

use commands::{AppInfoState, CalendarState, CatalogWriterState, DatabaseState, DisplayMetadataState, ExifState, FormatsState, ListingState, MetadataViewState, ThumbnailState};
use db::open_catalog;
use std::sync::{Arc, Mutex, atomic::Ordering};
use tauri::http::{Response, StatusCode, header};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = paths::Paths::resolve();
    let info = paths.info();
    eprintln!("DDreams paths: root={}, catalog={}, backups={}, cache={}, exiftool={}, formats={}, metadata_view={}, settings={}, portable={}, readonly={}", info.root, info.catalog, info.backups, info.cache, info.exiftool, info.formats, info.metadata_view, info.settings, info.portable, info.readonly);
    if let Err(error) = paths.ensure_runtime_dirs() { eprintln!("Warning: cannot create runtime directories: {error}"); }
    match backup::create_startup_backup(&paths.catalog, &paths.backups) {
        Ok(Some(path)) => eprintln!("Catalog backup created: {}", path.display()),
        Ok(None) => eprintln!("Catalog backup skipped: unchanged within the last hour"),
        Err(error) => eprintln!("Warning: catalog backup failed: {error}"),
    }
    let mut connection = open_catalog(&paths.catalog, paths.readonly).unwrap_or_else(|e| {
        panic!("Cannot open catalog at {}: {e}", paths.catalog.display())
    });
    if !paths.readonly {
        schema::migrate_catalog(&mut connection).unwrap_or_else(|e| panic!("Catalog migration failed at {}: {e}", paths.catalog.display()));
    }
    let db = Arc::new(Mutex::new(connection));
    let exif = exiftool::ExifTool::new(paths.exiftool.clone());
    let formats = formats::FormatTable::load_selected(paths.formats.as_deref());
    let metadata_view = metadata_view::MetadataView::load_selected(paths.metadata_view.as_deref());
    let display_metadata = display_metadata::DisplayMetadataCache::new(paths.cache.clone());
    let listing = listing::ListingService::new(db.clone(), formats.clone());
    let calendar = calendar::CalendarService::new(db.clone(), formats.clone());
    let thumbs = thumbs::ThumbService::new(db.clone(), listing.clone(), exif.clone(), formats.clone(), paths.cache.clone());
    let writer = Arc::new(journal::CatalogWriter::new(db.clone(), !paths.readonly));
    let protocol_thumbs = thumbs.clone();
    let shutdown_exif = exif.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(DatabaseState(db))
        .manage(ExifState(exif))
        .manage(ListingState(listing))
        .manage(FormatsState(formats))
        .manage(MetadataViewState(metadata_view))
        .manage(DisplayMetadataState(display_metadata))
        .manage(ThumbnailState(thumbs))
        .manage(CalendarState(calendar))
        .manage(CatalogWriterState(writer))
        .manage(AppInfoState(info))
        .register_asynchronous_uri_scheme_protocol("thumb", move |_ctx, request, responder| {
            let service = protocol_thumbs.clone();
            let query = request.uri().query().unwrap_or_default();
            let mut size = 180u32;
            let mut epoch = 0u64;
            let mut id = None;
            for pair in query.split('&') {
                let mut it = pair.splitn(2, '=');
                match (it.next().unwrap_or(""), it.next().unwrap_or("")) {
                    ("s", v) => size = v.parse().unwrap_or(180),
                    ("v", v) => epoch = v.parse().unwrap_or(0),
                    ("id", v) => id = v.parse::<i64>().ok(),
                    _ => {}
                }
            }
            tauri::async_runtime::spawn(async move {
                let Some(id) = id else {
                    responder.respond(
                        Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Vec::new())
                            .unwrap(),
                    );
                    return;
                };
                service.set_epoch(epoch);
                let permit = service.workers.clone().acquire_owned().await;
                if permit.is_err() {
                    responder.respond(
                        Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Vec::new())
                            .unwrap(),
                    );
                    return;
                }
                if epoch < service.epoch.load(Ordering::Relaxed) {
                    responder.respond(Response::builder().status(StatusCode::CONFLICT).header(header::CACHE_CONTROL,"no-store").body(Vec::new()).unwrap());
                    return;
                }
                let worker = service.clone();
                let result = tauri::async_runtime::spawn_blocking(move || worker.render(id, size, epoch)).await;
                match result {
                    Ok(Ok(data)) => responder.respond(
                        Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE,"image/jpeg").header(header::CACHE_CONTROL,"no-store").body(data.bytes).unwrap()),
                    Ok(Err(error)) => {
                        let status=match error { thumbs::ThumbnailError::Offline=>StatusCode::NOT_FOUND, thumbs::ThumbnailError::Stale=>StatusCode::CONFLICT, thumbs::ThumbnailError::Unsupported=>StatusCode::UNSUPPORTED_MEDIA_TYPE, thumbs::ThumbnailError::Decode=>StatusCode::UNPROCESSABLE_ENTITY, thumbs::ThumbnailError::Unavailable=>StatusCode::INTERNAL_SERVER_ERROR };
                        responder.respond(Response::builder().status(status).header(header::CACHE_CONTROL,"no-store").body(Vec::new()).unwrap());
                    }
                    Err(_) => responder.respond(Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Vec::new()).unwrap()),
                }
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_recent_changes,
            commands::undo_last_change,
            commands::set_asset_values,
            commands::set_asset_categories,
            commands::get_category_selection_counts,
            commands::get_calendar_aggregate,
            commands::get_assets,
            commands::get_formats,
            commands::get_preview_source,
            commands::refresh_folder_listing,
            commands::get_asset_paths,
            commands::asset_action,
            commands::get_asset_ids,
            commands::get_folder_children,
            commands::find_folder_path,
            commands::get_folder_ancestors,
            commands::get_category_roots,
            commands::get_catalog_nodes,
            commands::search_keywords,
            commands::search_catalog_tree,
            commands::get_catalog_values,
            commands::get_metadata,
            commands::get_thumbnail_metadata,
            commands::get_catalog_asset_metadata,
            commands::get_catalog_asset_links,
            commands::set_thumbnail_epoch,
            commands::get_thumbnail_state
        ])
        .build(tauri::generate_context!())
        .expect("error while building DDreams browser")
        .run(move |_app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                shutdown_exif.shutdown();
            }
        });
}
