use super::files::FileProgress;
use super::values::listed_asset_candidates;
use super::{CatalogWriterState, DatabaseState, ExifState, FileManagerState, FormatsState, ListingState};
use std::sync::Arc;
use tauri::{Emitter, State};

#[tauri::command]
pub async fn get_asset_iptc(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    asset_ids: Vec<i64>,
    exif: State<'_, ExifState>,
) -> Result<Vec<crate::iptc_write::EditorPhoto>, String> {
    let (db, listing, formats, exif) = (db.0.clone(), listing.0.clone(), formats.0.clone(), exif.0.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &asset_ids)?;
        Ok(crate::iptc_write::editor_photos(photos, &exif, &formats))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct IptcEditResult {
    #[serde(flatten)]
    summary: crate::iptc_write::WriteSummary,
    text: Option<crate::journal::SetAssetTextResult>,
    catalog_keywords_changed: bool,
}

#[tauri::command]
pub async fn edit_asset_iptc(
    app: tauri::AppHandle,
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>,
    writer: State<'_, CatalogWriterState>,
    manager: State<'_, FileManagerState>,
    targets: Vec<crate::iptc_write::EditorTarget>,
    mut action: crate::iptc_write::ManualEdit,
) -> Result<IptcEditResult, String> {
    use crate::iptc_write::ManualEdit;
    if !writer.0.is_writable() {
        return Err("Katalóg je iba na čítanie".into());
    }
    if let ManualEdit::Text { field, value } = &mut action {
        *value = field.normalize(value.take())?;
    }
    if matches!(&action, ManualEdit::RemoveKeyword { keyword } if keyword.trim().is_empty() || keyword.contains('\0')) {
        return Err("Neplatné kľúčové slovo".into());
    }
    let (db, listing, formats, exif, writer, manager) = (
        db.0.clone(),
        listing.0.clone(),
        formats.0.clone(),
        exif.0.clone(),
        writer.0.clone(),
        manager.0.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = manager.lock_operation()?;
        let ids: Vec<_> = targets.iter().map(|target| target.id).collect();
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &ids)?;
        let mut candidates = Vec::new();
        let (mut summary, succeeded) = crate::iptc_write::edit_photos(
            photos,
            targets,
            &action,
            &exif,
            &formats,
            |id| {
                let (_, listed) = listed_asset_candidates(&listing, &[id])?;
                writer.check_iptc_edit(id, &listed)?;
                candidates.extend(listed);
                Ok(())
            },
            |done, total| {
                if total > 5 {
                    let _ = app.emit(
                        "file-operation-progress",
                        FileProgress {
                            operation: "Upravujem IPTC".into(),
                            done,
                            total,
                        },
                    );
                }
            },
        );
        let mut result = IptcEditResult {
            summary: Default::default(),
            text: None,
            catalog_keywords_changed: false,
        };
        let ids: Vec<_> = succeeded.iter().map(|photo| photo.id).collect();
        candidates.retain(|candidate| ids.contains(&candidate.temporary_id));
        match action {
            ManualEdit::Text { field, value } if !ids.is_empty() => {
                match writer.set_asset_text(&ids, &candidates, field, value) {
                    Ok(text) => result.text = Some(text),
                    Err(error) => {
                        for photo in &succeeded {
                            summary.catalog_error(photo, &error);
                        }
                    }
                }
            }
            ManualEdit::RemoveKeyword { keyword } => {
                let mut by_keyword = std::collections::BTreeMap::<_, Vec<_>>::new();
                for photo in &succeeded {
                    for id in photo.matching_keyword_ids(&keyword) {
                        by_keyword.entry(id).or_default().push(photo);
                    }
                }
                for (id, photos) in by_keyword {
                    let assets: Vec<_> = photos.iter().map(|photo| photo.id).collect();
                    let listed: Vec<_> = candidates
                        .iter()
                        .filter(|candidate| assets.contains(&candidate.temporary_id))
                        .cloned()
                        .collect();
                    match writer.set_asset_keywords(&assets, &listed, id, false) {
                        Ok(edit) => result.catalog_keywords_changed |= edit.changed_count > 0,
                        Err(error) => {
                            for photo in photos {
                                summary.catalog_error(photo, &error);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        for id in ids.into_iter().filter(|id| *id >= 0) {
            let _ = listing.invalidate_asset_parent(id);
        }
        for candidate in candidates {
            let _ = listing.invalidate_path(&candidate.folder_path);
        }
        result.summary = summary;
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plan_iptc_write(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>,
    writer: State<'_, CatalogWriterState>,
    batch: State<'_, Arc<crate::iptc_write::IptcState>>,
    asset_ids: Vec<i64>,
) -> Result<crate::iptc_write::PlanSummary, String> {
    if !writer.0.is_writable() {
        return Err("Katalóg je iba na čítanie".into());
    }
    let (db, listing, formats, exif, batch) = (
        db.0.clone(),
        listing.0.clone(),
        formats.0.clone(),
        exif.0.clone(),
        batch.inner().clone(),
    );
    let id = batch.begin_plan()?;
    tauri::async_runtime::spawn_blocking(move || {
        let photos = crate::iptc_write::catalog_photos(&db, &listing, &formats, &asset_ids)?;
        batch.store(crate::iptc_write::Plan::prepare(id, photos, &exif, &formats))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn execute_iptc_write(
    app: tauri::AppHandle,
    formats: State<'_, FormatsState>,
    exif: State<'_, ExifState>,
    writer: State<'_, CatalogWriterState>,
    manager: State<'_, FileManagerState>,
    batch: State<'_, Arc<crate::iptc_write::IptcState>>,
    plan_id: u64,
) -> Result<crate::iptc_write::WriteSummary, String> {
    if !writer.0.is_writable() {
        return Err("Katalóg je iba na čítanie".into());
    }
    let (formats, exif, manager, batch) = (
        formats.0.clone(),
        exif.0.clone(),
        manager.0.clone(),
        batch.inner().clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = manager.lock_operation()?;
        batch.execute(plan_id, &exif, &formats, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Zapisujem IPTC".into(),
                    done,
                    total,
                },
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_iptc_write(batch: State<'_, Arc<crate::iptc_write::IptcState>>, plan_id: u64) -> Result<(), String> {
    batch.cancel(plan_id)
}
