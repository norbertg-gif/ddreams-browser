use crate::journal::FileCatalogMove;
use std::path::{Path, PathBuf};

use super::filesystem::explorer_select_arg;
use super::sources::sidecar_pairs;
use super::transfer::{combine_error, move_transfers, rollback_moves};
use super::{FileManager, FileOperationResult};

pub(super) fn path_eq(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

pub(super) fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

pub fn validate_windows_name(name: &str) -> Result<String, String> {
    let value = name.trim();
    if value.is_empty() {
        return Err("Názov nesmie byť prázdny".into());
    }
    if value.len() > 255 {
        return Err("Názov môže mať najviac 255 znakov".into());
    }
    if value.ends_with('.') || value.ends_with(' ') {
        return Err("Názov sa nesmie končiť bodkou ani medzerou".into());
    }
    if value
        .chars()
        .any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err("Názov obsahuje znak, ktorý Windows nepovoľuje".into());
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (1..=9).any(|i| stem == format!("COM{i}") || stem == format!("LPT{i}"))
    {
        return Err("Tento názov je vyhradený systémom Windows".into());
    }
    Ok(value.to_owned())
}

impl FileManager {
    pub fn rename_file<F>(&self, id: i64, new_base_name: &str, mut progress: F) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        let _operation = self.lock_operation()?;
        self.writable()?;
        let base = validate_windows_name(new_base_name)?;
        let mut sources = self.resolve_sources(&[id])?;
        let source = sources.pop().ok_or("Súbor sa nenašiel")?;
        let parent = source.path.parent().ok_or("Priečinok súboru sa nenašiel")?;
        let ext = source.path.extension().map(|x| x.to_string_lossy().to_string());
        let file_name = ext
            .as_ref()
            .map(|e| format!("{base}.{e}"))
            .unwrap_or_else(|| base.clone());
        let target = parent.join(file_name);
        let mut pairs = sidecar_pairs(&source.path, &target)?;
        if pairs.iter().any(|p| self.fs.exists(&p.to) && !path_eq(&p.from, &p.to)) {
            return Err(format!("V priečinku už existuje súbor: {}", target.display()));
        }
        if pairs.iter().all(|p| p.from == p.to) {
            return Ok(self.no_change_result(0, "Premenovanie"));
        }
        progress(1, 1);
        let applied = move_transfers(self.fs.as_ref(), &pairs)?;
        pairs.clear();
        let name = target.file_name().unwrap().to_string_lossy().to_string();
        let catalog_move = if source.id > 0 {
            vec![FileCatalogMove {
                asset_id: source.id,
                target_path: target.to_string_lossy().to_string(),
                target_folder_path: parent.to_string_lossy().to_string(),
                name: name.clone(),
                ext: target.extension().unwrap_or_default().to_string_lossy().to_string(),
                size: source.size as f64,
                file_modified: Some(crate::listing::system_time_iso(source.modified)),
            }]
        } else {
            Vec::new()
        };
        let summary = format!(
            "Premenované {} → {name}",
            source.path.file_name().unwrap_or_default().to_string_lossy()
        );
        let (change_id, remapped_ids) =
            match self
                .writer
                .record_file_change("rename_file", &summary, &catalog_move, &[], &[])
            {
                Ok(v) => v,
                Err(error) => return Err(combine_error(error, rollback_moves(self.fs.as_ref(), &applied))),
            };
        self.listing.invalidate_path(&parent.to_string_lossy()).ok();
        Ok(FileOperationResult {
            change_id,
            summary,
            changed_count: 1,
            skipped_count: 0,
            remapped_ids,
            affected_ids: vec![id],
            affected_folder_paths: vec![parent.to_string_lossy().to_string()],
        })
    }

    pub fn reveal_in_explorer(&self, path: &str) -> Result<(), String> {
        let target = PathBuf::from(path);
        if !self.fs.exists(&target) {
            return Err("Priečinok neexistuje".into());
        }
        #[cfg(windows)]
        {
            use std::{os::windows::process::CommandExt, process::Command};
            Command::new("explorer.exe")
                .raw_arg(explorer_select_arg(&target))
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
