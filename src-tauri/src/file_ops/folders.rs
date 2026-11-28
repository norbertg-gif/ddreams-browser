use serde::Serialize;
use std::path::{Path, PathBuf};

use super::filesystem::FileSystemOps;
use super::rename::{key, path_eq};
use super::transfer::{combine_error, move_case_safe};
use super::{FileManager, FileOperationResult, validate_windows_name};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFolderInfo {
    pub files: usize,
    pub folders: usize,
    pub catalogued: usize,
    pub bytes: u64,
    pub recycle_available: bool,
}

fn count_folder_files(fs: &dyn FileSystemOps, path: &Path) -> Result<usize, String> {
    let mut count = 0;
    for item in fs.read_dir(path).map_err(|e| e.to_string())? {
        if fs.is_symlink(&item) {
            return Err(format!(
                "Odkaz mimo priečinok nemožno bezpečne spracovať: {}",
                item.display()
            ));
        } else if fs.is_dir(&item) {
            count += count_folder_files(fs, &item)?
        } else {
            count += 1
        }
    }
    Ok(count)
}

fn copy_folder_contents(
    fs: &dyn FileSystemOps,
    source: &Path,
    target: &Path,
    copied: &mut usize,
    progress: &mut dyn FnMut(usize, usize),
    total: usize,
) -> Result<(), String> {
    for item in fs.read_dir(source).map_err(|e| e.to_string())? {
        let dest = target.join(item.file_name().ok_or("Neplatná cesta")?);
        if fs.is_symlink(&item) {
            return Err(format!("Odkaz mimo priečinok nemožno kopírovať: {}", item.display()));
        }
        if fs.is_dir(&item) {
            fs.create_dir(&dest).map_err(|e| e.to_string())?;
            copy_folder_contents(fs, &item, &dest, copied, progress, total)?;
        } else {
            let modified = fs
                .metadata(&item)
                .and_then(|metadata| metadata.modified())
                .map_err(|e| e.to_string())?;
            fs.copy_no_replace(&item, &dest).map_err(|e| e.to_string())?;
            fs.set_modified(&dest, modified).map_err(|e| e.to_string())?;
            *copied += 1;
            progress(*copied, total);
        }
    }
    Ok(())
}

fn remove_tree(fs: &dyn FileSystemOps, path: &Path) -> Result<(), String> {
    for item in fs.read_dir(path).map_err(|e| e.to_string())? {
        if fs.is_dir(&item) {
            remove_tree(fs, &item)?
        } else {
            fs.remove_file(&item).map_err(|e| e.to_string())?
        }
    }
    fs.remove_dir(path).map_err(|e| e.to_string())
}

fn is_volume_root(path: &str) -> bool {
    let value = path.replace("/", "\\");
    (value.len() == 2 && value.as_bytes().get(1) == Some(&b':'))
        || (value.len() == 3 && value.as_bytes().get(1) == Some(&b':') && value.ends_with("\\"))
        || (value.starts_with("\\\\") && value.trim_matches('\\').split('\\').filter(|p| !p.is_empty()).count() == 2)
}

fn validate_folder_name(name: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("Názov nesmie byť prázdny".into());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("Názov sa nesmie končiť bodkou ani medzerou".into());
    }
    if name
        .chars()
        .any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err("Názov obsahuje znak, ktorý Windows nepovoľuje".into());
    }
    let _ = validate_windows_name(name)?;
    Ok(name.to_owned())
}

impl FileManager {
    pub fn rename_folder(&self, path: &str, new_name: &str) -> Result<FileOperationResult, String> {
        let _operation = self.lock_operation()?;
        self.writable()?;
        if is_volume_root(path) {
            return Err("Diskovú koreňovú zložku nemožno premenovať".into());
        }
        let source = self.validate_folder(path)?;
        if source.parent().is_none() || source.parent() == Some(source.as_path()) {
            return Err("Diskovú koreňovú zložku nemožno premenovať".into());
        }
        let name = validate_folder_name(new_name)?;
        if name == "." || name == ".." {
            return Err("Neplatný názov priečinka".into());
        }
        let target = source.parent().unwrap().join(name);
        let case_only = path_eq(&source, &target) && source != target;
        if self.fs.exists(&target) && !case_only {
            return Err(format!("Cieľ už existuje: {}", target.display()));
        }
        if source == target {
            return Ok(self.no_change_result(0, "Premenovanie priečinka"));
        }
        move_case_safe(self.fs.as_ref(), &source, &target).map_err(|e| e.to_string())?;
        let summary = format!("Premenovaný priečinok: {}", target.display());
        match self.writer.rewrite_folder_paths(
            &source.to_string_lossy(),
            &target.to_string_lossy(),
            "rename_folder",
            &summary,
        ) {
            Ok(change_id) => {
                if let Some(parent) = source.parent() {
                    let _ = self.listing.invalidate_path(&parent.to_string_lossy());
                }
                Ok(FileOperationResult {
                    change_id,
                    summary,
                    changed_count: 1,
                    skipped_count: 0,
                    remapped_ids: Vec::new(),
                    affected_ids: Vec::new(),
                    affected_folder_paths: vec![
                        source.to_string_lossy().to_string(),
                        target.to_string_lossy().to_string(),
                    ],
                })
            }
            Err(error) => {
                let rollback = move_case_safe(self.fs.as_ref(), &target, &source).map_err(|e| e.to_string());
                Err(combine_error(error, rollback))
            }
        }
    }

    pub fn delete_folder_info(&self, path: &str) -> Result<DeleteFolderInfo, String> {
        if is_volume_root(path) {
            return Err("Diskovú koreňovú zložku nemožno odstrániť".into());
        }
        let root = self.validate_folder(path)?;
        let mut files = 0usize;
        let mut folders = 0usize;
        let mut bytes = 0u64;
        fn walk(
            fs: &dyn FileSystemOps,
            p: &Path,
            files: &mut usize,
            folders: &mut usize,
            bytes: &mut u64,
        ) -> Result<(), String> {
            *folders += 1;
            for item in fs.read_dir(p).map_err(|e| e.to_string())? {
                if fs.is_symlink(&item) {
                    return Err(format!(
                        "Odkaz mimo priečinok nemožno bezpečne spracovať: {}",
                        item.display()
                    ));
                } else if fs.is_dir(&item) {
                    walk(fs, &item, files, folders, bytes)?
                } else {
                    *files += 1;
                    *bytes = bytes.saturating_add(fs.metadata(&item).map_err(|e| e.to_string())?.len())
                }
            }
            Ok(())
        }
        walk(self.fs.as_ref(), &root, &mut files, &mut folders, &mut bytes)?;
        let catalogued = {
            let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
            let norm = crate::journal::normalize_fs_path(&root.to_string_lossy());
            let prefix = format!("{norm}\\");
            let mut stmt = conn
                .prepare("SELECT path FROM assets WHERE path IS NOT NULL")
                .map_err(|e| e.to_string())?;
            let paths = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            paths
                .iter()
                .filter(|p| {
                    let p = crate::journal::normalize_fs_path(p);
                    p == norm || p.starts_with(&prefix)
                })
                .count()
        };
        Ok(DeleteFolderInfo {
            files,
            folders,
            catalogued,
            bytes,
            recycle_available: self.fs.has_recycle_bin(&root),
        })
    }

    pub fn delete_folder(&self, path: &str) -> Result<FileOperationResult, String> {
        let _operation = self.lock_operation()?;
        self.writable()?;
        if is_volume_root(path) {
            return Err("Diskovú koreňovú zložku nemožno odstrániť".into());
        }
        let root = self.validate_folder(path)?;
        if root.parent().is_none() || root.parent() == Some(root.as_path()) {
            return Err("Diskovú koreňovú zložku nemožno odstrániť".into());
        }
        if !self.fs.has_recycle_bin(&root) {
            return Err("Na tomto disku nie je dostupný Kôš; priečinok nemožno bezpečne odstrániť.".into());
        }
        let info = self.delete_folder_info(path)?;
        self.fs.recycle_delete(&[root.clone()])?;
        let summary = format!(
            "Priečinok presunutý do Koša: {}",
            root.file_name().unwrap_or_default().to_string_lossy()
        );
        let change_id = match self.writer.record_folder_delete(&root.to_string_lossy(), &summary) {
            Ok(id) => id,
            Err(e) => {
                let rollback = self.fs.restore_recycled(&root);
                return Err(combine_error(e, rollback));
            }
        };
        if let Some(parent) = root.parent() {
            let _ = self.listing.invalidate_path(&parent.to_string_lossy());
        }
        Ok(FileOperationResult {
            change_id,
            summary,
            changed_count: info.files,
            skipped_count: 0,
            remapped_ids: Vec::new(),
            affected_ids: Vec::new(),
            affected_folder_paths: vec![root.to_string_lossy().to_string()],
        })
    }

    pub fn move_folder(&self, path: &str, target: &str) -> Result<FileOperationResult, String> {
        self.transfer_folder(path, target, false, &mut |_, _| {})
    }

    pub fn move_folder_with_progress<F>(
        &self,
        path: &str,
        target: &str,
        mut progress: F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        self.transfer_folder(path, target, false, &mut progress)
    }

    pub fn copy_folder(&self, path: &str, target: &str) -> Result<FileOperationResult, String> {
        self.transfer_folder(path, target, true, &mut |_, _| {})
    }

    pub fn copy_folder_with_progress<F>(
        &self,
        path: &str,
        target: &str,
        mut progress: F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        self.transfer_folder(path, target, true, &mut progress)
    }

    pub(super) fn transfer_folder<F>(
        &self,
        path: &str,
        target: &str,
        copy: bool,
        progress: &mut F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        let _operation = self.lock_operation()?;
        self.writable()?;
        if is_volume_root(path) {
            return Err("Diskovú koreňovú zložku nemožno presúvať".into());
        }
        let source = self.validate_folder(path)?;
        let parent = self.validate_folder(target)?;
        if source.parent().is_none() || source.parent() == Some(source.as_path()) {
            return Err("Diskovú koreňovú zložku nemožno presúvať".into());
        }
        let source_key = key(&source);
        let target_key = key(&parent);
        if target_key == source_key || target_key.starts_with(&(source_key + "\\")) {
            return Err("Priečinok nemožno vložiť do seba ani do jeho podpriečinka".into());
        }
        let dest = parent.join(source.file_name().ok_or("Názov priečinka chýba")?);
        if self.fs.exists(&dest) {
            return Err(format!(
                "V cieli už existuje priečinok „{}“",
                dest.file_name().unwrap().to_string_lossy()
            ));
        }
        let cross_volume = !self.fs.same_volume(&source, &dest);
        let count = count_folder_files(self.fs.as_ref(), &source)?;
        let remove_copy = || {
            remove_tree(self.fs.as_ref(), &dest)
                .map_err(|error| format!("Nepodarilo sa odstrániť kópiu {}: {error}", dest.display()))
        };
        let restore_source = || {
            self.fs
                .restore_recycled(&source)
                .map_err(|error| format!("Nepodarilo sa obnoviť priečinok {}: {error}", source.display()))?;
            if !self.fs.is_dir(&source) {
                return Err(format!("Obnovený priečinok nie je dostupný: {}", source.display()));
            }
            Ok(())
        };
        if copy || cross_volume {
            self.fs.create_dir(&dest).map_err(|e| e.to_string())?;
            let mut copied = 0;
            if let Err(error) = copy_folder_contents(self.fs.as_ref(), &source, &dest, &mut copied, progress, count) {
                return Err(combine_error(error, remove_copy()));
            }
        } else {
            progress(1, 1);
            move_case_safe(self.fs.as_ref(), &source, &dest).map_err(|e| e.to_string())?;
        }
        if !copy && cross_volume {
            if let Err(error) = self.fs.recycle_delete(std::slice::from_ref(&source)) {
                let rollback = if self.fs.is_dir(&source) {
                    remove_copy()
                } else {
                    restore_source().and_then(|()| remove_copy())
                };
                return Err(combine_error(error, rollback));
            }
        }
        let summary = if copy {
            format!("Skopírované: {count} súborov (do katalógu sa pridajú pri otvorení priečinka)")
        } else {
            format!("Presunutý priečinok: {}", dest.display())
        };
        let recorded = if copy {
            self.writer
                .record_file_change("copy_folder", &summary, &[], &[], &[])
                .map(|result| result.0)
        } else {
            self.writer.rewrite_folder_paths(
                &source.to_string_lossy(),
                &dest.to_string_lossy(),
                "move_folder",
                &summary,
            )
        };
        let change_id = recorded.map_err(|error| {
            let rollback = if copy {
                remove_copy()
            } else if cross_volume {
                restore_source().and_then(|()| remove_copy())
            } else {
                move_case_safe(self.fs.as_ref(), &dest, &source)
                    .map_err(|error| format!("Nepodarilo sa obnoviť priečinok {}: {error}", source.display()))
            };
            combine_error(error, rollback)
        })?;
        let mut affected = vec![
            source.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
            parent.to_string_lossy().into_owned(),
        ];
        if let Some(old_parent) = source.parent() {
            affected.push(old_parent.to_string_lossy().into_owned());
        }
        for path in &affected {
            let _ = self.listing.invalidate_path(path);
        }
        Ok(FileOperationResult {
            change_id,
            summary,
            changed_count: count,
            skipped_count: 0,
            remapped_ids: Vec::new(),
            affected_ids: Vec::new(),
            affected_folder_paths: affected,
        })
    }

    pub fn create_folder(&self, parent: &str, name: &str) -> Result<FileOperationResult, String> {
        let _operation = self.lock_operation()?;
        self.writable()?;
        let folder = self.validate_folder(parent)?;
        let name = validate_windows_name(name)?;
        let target = folder.join(name);
        if self.fs.exists(&target) {
            return Err(format!("Priečinok už existuje: {}", target.display()));
        }
        self.fs
            .create_dir(&target)
            .map_err(|e| format!("Nepodarilo sa vytvoriť priečinok {}: {e}", target.display()))?;
        let summary = format!(
            "Vytvorený priečinok {}",
            target.file_name().unwrap_or_default().to_string_lossy()
        );
        match self.writer.record_file_change(
            "create_folder",
            &summary,
            &[],
            &[],
            &[target.to_string_lossy().to_string()],
        ) {
            Ok((change_id, _)) => {
                self.listing.invalidate_path(parent).ok();
                Ok(FileOperationResult {
                    change_id,
                    summary,
                    changed_count: 1,
                    skipped_count: 0,
                    remapped_ids: Vec::new(),
                    affected_ids: Vec::new(),
                    affected_folder_paths: vec![parent.to_string()],
                })
            }
            Err(error) => {
                let rollback = self
                    .fs
                    .remove_dir(&target)
                    .map_err(|e| format!("Nepodarilo sa vrátiť priečinok {}: {e}", target.display()));
                Err(combine_error(error, rollback))
            }
        }
    }

    pub(super) fn validate_folder(&self, path: &str) -> Result<PathBuf, String> {
        let candidate = PathBuf::from(path);
        if !candidate.is_absolute() || !self.fs.is_dir(&candidate) {
            return Err(format!("Cieľový priečinok nie je dostupný: {path}"));
        }
        Ok(candidate)
    }
}
