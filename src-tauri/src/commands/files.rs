use super::{AppInfoState, DatabaseState, FileManagerState, FormatsState, ListingState};
use crate::ai::AiState;
use crate::db;
use crate::file_ops::{ConflictPolicy, FileOperationResult};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{Emitter, State};

#[tauri::command(async)]
pub fn get_file_operation_conflicts(
    state: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    target_folder_path: String,
) -> Result<usize, String> {
    state.0.preview_conflicts(&asset_ids, &target_folder_path, None)
}

#[tauri::command(async)]
pub fn get_uncategorized_photos_for_move(
    state: State<'_, FileManagerState>,
    formats: State<'_, FormatsState>,
    asset_ids: Vec<i64>,
    target_folder_path: String,
    conflict: ConflictPolicy,
) -> Result<usize, String> {
    state
        .0
        .uncategorized_photos_for_move(&asset_ids, &target_folder_path, conflict, &formats.0)
}

#[tauri::command]
pub async fn move_files(
    app: tauri::AppHandle,
    state: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    target_folder_path: String,
    conflict: ConflictPolicy,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.move_files(&asset_ids, &target_folder_path, conflict, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Presúvam".into(),
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
pub async fn copy_files(
    app: tauri::AppHandle,
    state: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    target_folder_path: String,
    conflict: ConflictPolicy,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.copy_files(&asset_ids, &target_folder_path, conflict, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Kopírujem".into(),
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
pub async fn rename_file(
    app: tauri::AppHandle,
    state: State<'_, FileManagerState>,
    asset_id: i64,
    new_base_name: String,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.rename_file(asset_id, &new_base_name, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Premenúvam".into(),
                    done,
                    total,
                },
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct FolderRenameResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub new_path: String,
}

#[tauri::command(async)]
pub fn reveal_in_explorer(state: State<'_, FileManagerState>, path: String) -> Result<(), String> {
    state.0.reveal_in_explorer(&path)
}

#[tauri::command(async)]
pub async fn rename_folder(
    state: State<'_, FileManagerState>,
    path: String,
    new_name: String,
) -> Result<FolderRenameResult, String> {
    let manager = state.0.clone();
    let original_path = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || manager.rename_folder(&path, &new_name))
        .await
        .map_err(|e| e.to_string())??;
    let new_path = result.affected_folder_paths.get(1).cloned().unwrap_or(original_path);
    Ok(FolderRenameResult {
        change_id: result.change_id,
        summary: result.summary,
        new_path,
    })
}

#[tauri::command(async)]
pub fn delete_folder_info(
    state: State<'_, FileManagerState>,
    path: String,
) -> Result<crate::file_ops::DeleteFolderInfo, String> {
    state.0.delete_folder_info(&path)
}

#[tauri::command(async)]
pub async fn delete_folder(
    state: State<'_, FileManagerState>,
    path: String,
) -> Result<crate::file_ops::FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.delete_folder(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub async fn move_folder(
    app: tauri::AppHandle,
    state: State<'_, FileManagerState>,
    path: String,
    target: String,
) -> Result<crate::file_ops::FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.move_folder_with_progress(&path, &target, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Presúvam priečinok".into(),
                    done,
                    total,
                },
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub async fn copy_folder(
    app: tauri::AppHandle,
    state: State<'_, FileManagerState>,
    path: String,
    target: String,
) -> Result<crate::file_ops::FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.copy_folder_with_progress(&path, &target, |done, total| {
            let _ = app.emit(
                "file-operation-progress",
                FileProgress {
                    operation: "Kopírujem priečinok".into(),
                    done,
                    total,
                },
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub async fn create_folder(
    state: State<'_, FileManagerState>,
    parent_path: String,
    name: String,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.create_folder(&parent_path, &name))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct FileProgress {
    pub operation: String,
    pub done: usize,
    pub total: usize,
}

#[tauri::command(async)]
pub fn get_asset_paths(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    ids: Vec<i64>,
) -> Result<Vec<String>, String> {
    Ok(ids
        .into_iter()
        .filter_map(|id| {
            listing
                .0
                .asset(id)
                .and_then(|a| a.path)
                .or_else(|| db::asset_metadata(&db.0, id).ok().flatten().and_then(|m| m.asset.path))
        })
        .collect())
}

#[tauri::command(async)]
pub fn asset_action(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    asset_id: i64,
    action: String,
) -> Result<(), String> {
    let path = listing
        .0
        .asset(asset_id)
        .and_then(|a| a.path)
        .or_else(|| {
            db::asset_metadata(&db.0, asset_id)
                .ok()
                .flatten()
                .and_then(|m| m.asset.path)
        })
        .ok_or("Cesta súboru sa nenašla")?;
    if !Path::new(&path).exists() {
        return Err("Súbor je offline".into());
    }
    let mut command = std::process::Command::new("explorer.exe");
    match action.as_str() {
        "open" => {
            command.arg(&path);
        }
        "reveal" => {
            use std::os::windows::process::CommandExt;
            command.raw_arg(crate::file_ops::explorer_select_arg(Path::new(&path)));
        }
        _ => return Err("Neznáma akcia".into()),
    }
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct Editor {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) multi: bool,
}

pub(super) fn read_editor_settings(path: &Path) -> Result<Vec<Editor>, String> {
    let settings = crate::incoming::read_settings(path)?;
    match settings.get("editors") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
        None => Ok(vec![]),
    }
}

pub(super) fn write_editor_settings(path: &Path, editors: &[Editor]) -> Result<(), String> {
    crate::incoming::write_setting(
        path,
        "editors",
        serde_json::to_value(editors).map_err(|e| e.to_string())?,
    )
}

#[tauri::command(async)]
pub fn get_editors(info: State<'_, AppInfoState>) -> Result<Vec<Editor>, String> {
    read_editor_settings(Path::new(&info.0.settings))
}

#[tauri::command(async)]
pub fn set_editors(info: State<'_, AppInfoState>, editors: Vec<Editor>) -> Result<(), String> {
    write_editor_settings(Path::new(&info.0.settings), &editors)
}

pub(super) fn newest_photoshop(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .filter(|p| p.is_file())
        .max_by(|a, b| a.parent().cmp(&b.parent()))
}

#[tauri::command(async)]
pub fn detect_photoshop() -> Option<Editor> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Photoshop.exe",
            "/ve",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .ok();
    let registry = output
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout).lines().find_map(|line| {
                line.split_once("REG_SZ")
                    .map(|(_, v)| PathBuf::from(v.trim().trim_matches('"')))
            })
        })
        .filter(|p| p.is_file());
    let fallback = || {
        let root = Path::new(r"C:\Program Files\Adobe");
        newest_photoshop(
            std::fs::read_dir(root)
                .ok()?
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with("adobe photoshop")
                })
                .map(|e| e.path().join("Photoshop.exe")),
        )
    };
    registry.or_else(fallback).map(|p| Editor {
        name: "Adobe Photoshop".into(),
        path: p.to_string_lossy().into_owned(),
        multi: true,
    })
}

pub(super) fn allowed_editor(
    editors: &[Editor],
    requested: &str,
    detect: impl FnOnce() -> Option<Editor>,
) -> Result<Editor, String> {
    let requested = requested.trim().to_lowercase();
    let matches = |editor: &Editor| editor.path.trim().to_lowercase() == requested;
    if !requested.is_empty() {
        if let Some(editor) = editors
            .iter()
            .find(|editor| matches(editor))
            .cloned()
            .or_else(|| detect().filter(matches))
        {
            return Ok(editor);
        }
    }
    Err("Editor nie je uložený v nastaveniach ani automaticky zistený".into())
}

/// Opens the given files in an external editor saved in Settings or detected by DDreams. `multi` = the program takes all
/// files in one launch; otherwise it is started once per file (at most 20).
#[tauri::command(async)]
pub fn open_in_editor(
    db: State<'_, DatabaseState>,
    listing: State<'_, ListingState>,
    info: State<'_, AppInfoState>,
    asset_ids: Vec<i64>,
    exe: String,
) -> Result<usize, String> {
    let editor = allowed_editor(
        &read_editor_settings(Path::new(&info.0.settings))?,
        &exe,
        detect_photoshop,
    )?;
    let exe = std::path::absolute(editor.path.trim()).map_err(|e| e.to_string())?;
    if !exe.is_file() {
        return Err(format!("Program sa nenašiel: {}", exe.display()));
    }
    let paths: Vec<String> = asset_ids
        .into_iter()
        .filter_map(|id| {
            listing
                .0
                .asset(id)
                .and_then(|a| a.path)
                .or_else(|| db::asset_metadata(&db.0, id).ok().flatten().and_then(|m| m.asset.path))
        })
        .filter(|p| Path::new(p).is_file())
        .collect();
    if paths.is_empty() {
        return Err("Žiadny z vybraných súborov nie je dostupný".into());
    }
    if editor.multi {
        std::process::Command::new(&exe)
            .args(&paths)
            .spawn()
            .map_err(|e| e.to_string())?;
    } else {
        if paths.len() > 20 {
            return Err("Tento program berie súbory po jednom – vyber najviac 20 súborov".into());
        }
        for path in &paths {
            std::process::Command::new(&exe)
                .arg(path)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(paths.len())
}

/// Windows file dialog for choosing an editor program. Returns None when cancelled.
#[tauri::command(async)]
pub fn pick_editor_program() -> Result<Option<String>, String> {
    use std::os::windows::process::CommandExt;
    let script = "Add-Type -AssemblyName System.Windows.Forms; $o=New-Object System.Windows.Forms.Form -Property @{TopMost=$true;ShowInTaskbar=$false;Opacity=0}; $d=New-Object System.Windows.Forms.OpenFileDialog; $d.Filter='Programy (*.exe)|*.exe'; $d.Title='Vyber program'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; if($d.ShowDialog($o) -eq 'OK'){[Console]::Write($d.FileName)}";
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| e.to_string())?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(if path.is_empty() { None } else { Some(path) })
}

#[tauri::command(async)]
pub fn program_exists(path: String) -> bool {
    Path::new(path.trim()).is_file()
}

#[tauri::command(async)]
pub fn folder_exists(path: String) -> bool {
    Path::new(path.trim()).is_dir()
}

#[tauri::command(async)]
pub fn get_delete_info(
    state: State<'_, FileManagerState>,
    asset_ids: Vec<i64>,
    permanent: bool,
) -> Result<crate::file_ops::DeleteInfo, String> {
    state.0.delete_info(&asset_ids, permanent)
}

/// Deletes the selected files (with sidecars): to the Recycle Bin or permanently when
/// Shift+Delete is requested or permanent deletion without a Recycle Bin is confirmed.
#[tauri::command]
pub async fn delete_files(
    state: State<'_, FileManagerState>,
    ai: State<'_, AiState>,
    asset_ids: Vec<i64>,
    permanent: Option<bool>,
    confirmed_no_bin_drives: Option<Vec<String>>,
) -> Result<FileOperationResult, String> {
    let manager = state.0.clone();
    let (result, purged) = tauri::async_runtime::spawn_blocking(move || {
        let mut purged = Vec::new();
        let result = manager.delete_files(
            &asset_ids,
            permanent.unwrap_or(false),
            &confirmed_no_bin_drives.unwrap_or_default(),
            &mut purged,
        );
        (result, purged)
    })
    .await
    .map_err(|e| e.to_string())?;
    ai.0.forget(purged);
    result
}
