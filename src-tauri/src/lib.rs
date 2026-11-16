mod ai;
mod commands;
mod backup;
mod calendar;
mod colors;
mod db;
mod display_metadata;
mod exiftool;
mod exif_index;
mod file_ops;
mod formats;
mod folder_rules;
pub mod journal;
mod listing;
mod metadata_view;
mod paths;
mod quality;
mod schema;
mod thumbs;

use commands::{AppInfoState, CalendarState, CatalogWriterState, DatabaseState, DisplayMetadataState, ExifState, FileManagerState, FormatsState, ListingState, MetadataViewState, ThumbnailState};
use db::open_catalog;
use std::sync::{Arc, Mutex, atomic::Ordering};
use tauri::http::{Response, StatusCode, header};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = commands::PROCESS_START.set(std::time::Instant::now());
    let paths = paths::Paths::resolve();
    let info = paths.info();
    eprintln!("DDreams paths: root={}, catalog={}, backups={}, cache={}, exiftool={}, formats={}, metadata_view={}, settings={}, portable={}, readonly={}", info.root, info.catalog, info.backups, info.cache, info.exiftool, info.formats, info.metadata_view, info.settings, info.portable, info.readonly);
    if let Err(error) = paths.ensure_runtime_dirs() { eprintln!("Warning: cannot create runtime directories: {error}"); }
    // No backup at startup (it delayed the window by ~20 s): the app reminds about once a month and
    // Settings has "Zálohovať teraz" (backup.rs, BackupReminder.tsx).
    let mut connection = open_catalog(&paths.catalog, paths.readonly).unwrap_or_else(|e| {
        panic!("Cannot open catalog at {}: {e}", paths.catalog.display())
    });
    if !paths.readonly {
        schema::migrate_catalog(&mut connection).unwrap_or_else(|e| panic!("Catalog migration failed at {}: {e}", paths.catalog.display()));
    }
    let db = Arc::new(Mutex::new(connection));
    // Falls back to the main connection if a second one cannot be opened.
    let background_db = db::open_read_only(&paths.catalog).map(|c| Arc::new(Mutex::new(c))).unwrap_or_else(|e| { eprintln!("Warning: background catalog connection failed: {e}"); db.clone() });
    let exif = exiftool::ExifTool::new(paths.exiftool.clone());
    let formats = formats::FormatTable::load_selected(paths.formats.as_deref());
    let metadata_view = metadata_view::MetadataView::load_selected(paths.metadata_view.as_deref());
    let display_metadata = display_metadata::DisplayMetadataCache::new(paths.cache.clone());
    let listing = listing::ListingService::new(db.clone(), formats.clone());
    let calendar = calendar::CalendarService::new(background_db.clone(), formats.clone());
    // RAW plugin lives next to the AI module (modules/raw or <repo>/raw) and uses the same Python runtime.
    let raw_plugin = paths.ai_python.clone().zip(paths.ai_script.parent().and_then(|p| p.parent()).map(|p| p.join("raw").join("ddreams_raw.py")))
        .filter(|(python, script)| python.is_file() && script.is_file())
        .map(|(python, script)| thumbs::RawPluginConfig { python, script });
    let thumbs = thumbs::ThumbService::new(db.clone(), listing.clone(), exif.clone(), formats.clone(), paths.cache.clone(), raw_plugin);
    let ai_service = ai::AiService::new(paths.clone(), db.clone(), formats.clone());
    let writer = Arc::new(journal::CatalogWriter::new(db.clone(), !paths.readonly));
    let journal_ai=ai_service.clone();
    writer.set_change_hook(Arc::new(move|kinds|for kind in kinds { journal_ai.sync_labels_debounced(kind); }));
    let file_manager=file_ops::FileManager::new(db.clone(),listing.clone(),writer.clone(),paths.readonly);
    let _=ai_service.ensure_colors_attached();
    let auto_exif = exif_index::ExifIndex::new(paths.catalog.clone(),paths.exiftool.clone(),formats.clone(),&db);
    let setup_ai = ai_service.clone();
    let protocol_thumbs = thumbs.clone();
    let shutdown_exif = exif.clone();
    let shutdown_ai = ai_service.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| { setup_ai.set_app(app.handle().clone()); Ok(()) })
        .manage(commands::AutoExifState(auto_exif))
        .manage(DatabaseState(db))
        .manage(commands::BackgroundDbState(background_db))
        .manage(ExifState(exif))
        .manage(ListingState(listing))
        .manage(FormatsState(formats))
        .manage(MetadataViewState(metadata_view))
        .manage(DisplayMetadataState(display_metadata))
        .manage(ThumbnailState(thumbs))
        .manage(CalendarState(calendar))
        .manage(CatalogWriterState(writer))
        .manage(FileManagerState(file_manager))
        .manage(ai::AiState(ai_service))
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
                let worker = service.clone();
                let lookup = tauri::async_runtime::spawn_blocking(move || worker.lookup(id, size, epoch)).await;
                let result = match lookup {
                    Ok(Ok(thumbs::CacheLookup::Hit(data))) => Ok(Ok(data)),
                    Ok(Err(error)) => Ok(Err(error)),
                    Err(error) => Err(error),
                    Ok(Ok(thumbs::CacheLookup::NeedsRender(_))) => {
                        let permit = service.workers.clone().acquire_owned().await;
                        if permit.is_err() {
                            responder.respond(Response::builder().status(StatusCode::SERVICE_UNAVAILABLE).body(Vec::new()).unwrap());
                            return;
                        }
                        if epoch < service.epoch.load(Ordering::Relaxed) {
                            responder.respond(Response::builder().status(StatusCode::CONFLICT).header(header::CACHE_CONTROL,"no-store").body(Vec::new()).unwrap());
                            return;
                        }
                        let worker = service.clone();
                        // Recheck after acquiring the permit in case another worker filled the cache.
                        tauri::async_runtime::spawn_blocking(move || worker.render(id, size, epoch)).await
                    }
                };
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
            commands::auto_categories,
            commands::ai_status,
            commands::ai_start,
            commands::ai_load_model,
            commands::ai_stop,
            commands::ai_pause,
            commands::ai_resume,
            commands::ai_reindex,
            commands::ai_search_text,
            commands::ai_similar,
            commands::ai_duplicate_groups,
            commands::ai_series,
            commands::ai_suggest,
            commands::ai_reject_suggestion,
            commands::ai_suggestion_groups,
            commands::ai_unsorted_clusters,
            commands::ai_map_build,
            commands::ai_map_data,
            commands::ai_cluster_ignore,
            commands::ai_category_paths,
            commands::ai_faces_status,
            commands::ai_faces_persons,
            commands::ai_faces_person_assets,
            commands::ai_faces_review,
            commands::ai_faces_clusters,
            commands::ai_faces_for_assets,
            commands::ai_faces_set,
            commands::ai_faces_asset_assign,
            commands::ai_faces_asset_remove,
            commands::get_catalog_missing_sources,
            commands::remove_folder_from_catalog_info,
            commands::remove_folder_from_catalog,
            commands::ai_faces_person_create,
            commands::ai_faces_person_rename,
            commands::ai_faces_recognize_all,
            commands::ai_face_thumbs,
            commands::ai_cluster_ignore_reset,
            commands::ai_sync_labels,
            commands::set_asset_colors,
            commands::get_asset_colors,
            commands::get_color_counts,
            commands::get_asset_quality,
            commands::get_quality_counts,
            commands::get_recent_changes,
            commands::undo_last_change,
            commands::get_file_operation_conflicts,
            commands::move_files,
            commands::copy_files,
            commands::rename_file,
            commands::reveal_in_explorer,
            commands::rename_folder,
            commands::delete_folder_info,
            commands::delete_folder,
            commands::move_folder,
            commands::copy_folder,
            commands::create_folder,
            commands::set_asset_values,
            commands::get_asset_labels,
            commands::set_asset_categories,
            commands::get_category_selection_counts,
            commands::get_keyword_selection_counts,
            commands::create_category,
            commands::rename_category,
            commands::move_category,
            commands::get_category_delete_info,
            commands::delete_category,
            commands::set_asset_keywords,
            commands::create_and_assign_keyword,
            commands::create_keyword,
            commands::rename_keyword,
            commands::move_keyword,
            commands::get_keyword_delete_info,
            commands::delete_keyword,
            commands::search_keyword_options,
            commands::get_calendar_aggregate,
            commands::get_assets,
            commands::get_folder_previews,
            commands::get_formats,
            commands::get_preview_source,
            commands::folder_exists,
            commands::delete_files,
            commands::get_delete_info,
            backup::get_backup_status,
            backup::backup_catalog_now,
            backup::set_backup_interval_days,
            backup::snooze_backup_reminder,
            commands::ai_duplicates_of,
            commands::get_folder_rules,
            commands::get_folder_rule_state,
            commands::get_exclusion_impact,
            commands::set_folder_rule,
            commands::refresh_folder_listing,
            commands::get_asset_paths,
            commands::asset_action,commands::startup_mark,commands::open_in_editor,commands::get_editors,commands::set_editors,commands::detect_photoshop,commands::pick_editor_program,commands::program_exists,
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
                let _ = shutdown_ai.stop();
            }
        });
}
