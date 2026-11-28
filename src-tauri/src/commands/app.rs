use super::AppInfoState;
use crate::paths::AppInfo;
use std::path::Path;
use tauri::State;

#[tauri::command(async)]
pub fn get_app_info(state: State<'_, AppInfoState>) -> AppInfo {
    state.0.clone()
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
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(first)
        .append(!first)
        .open(path)
    {
        let _ = writeln!(file, "{ms:>7} ms  {}", label.chars().take(120).collect::<String>());
    }
}
