mod commands;
mod calendar;
mod db;
mod display_metadata;
mod exiftool;
mod formats;
mod listing;
mod metadata_view;
mod thumbs;

use commands::{CalendarState, DatabaseState, DisplayMetadataState, ExifState, FormatsState, ListingState, MetadataViewState, ThumbnailState};
use db::{catalog_path, open_read_only};
use std::sync::{Arc, Mutex, atomic::Ordering};
use tauri::http::{Response, StatusCode, header};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let catalog = catalog_path();
    let connection = open_read_only(&catalog).unwrap_or_else(|e| {
        panic!(
            "Cannot open catalog read-only at {}: {e}",
            catalog.display()
        )
    });
    let db = Arc::new(Mutex::new(connection));
    let exif = exiftool::ExifTool::new(
        std::env::var_os("DDREAMS_EXIFTOOL")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(r"G:\Programovanie\photosort\tools\exiftool\exiftool.exe")
            }),
    );
    let formats = formats::FormatTable::load();
    let metadata_view = metadata_view::MetadataView::load();
    let display_metadata = display_metadata::DisplayMetadataCache::new(display_metadata::cache_root());
    let listing = listing::ListingService::new(db.clone(), formats.clone());
    let calendar = calendar::CalendarService::new(db.clone(), formats.clone());
    let thumbs = thumbs::ThumbService::new(db.clone(), listing.clone(), exif.clone(), formats.clone());
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
