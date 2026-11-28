use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::filesystem::volume_key;
use super::rename::{key, path_eq};
use super::sources::sidecar_pairs;
use super::transfer::{combine_error, slovak_files};
use super::{FileManager, FileOperationResult, Source};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct DeleteInfo {
    pub count: usize,
    pub names: Vec<String>,
    pub sidecars: usize,
    pub permanent_count: usize,
    pub no_bin_drives: Vec<String>,
}

impl FileManager {
    /// Moves the selected files and their sidecars to the Recycle Bin and removes their catalogue rows,
    /// in a single batch. Directories are skipped.
    fn delete_sources(&self, ids: &[i64]) -> Result<Vec<Source>, String> {
        let files_only = ids
            .iter()
            .copied()
            .filter(|id| !(*id < 0 && self.listing.asset(*id).is_some_and(|a| a.is_directory)))
            .collect::<Vec<_>>();
        if files_only.is_empty() {
            return Err("Priečinky sa týmto nemažú".into());
        }
        let sources = self.resolve_sources(&files_only)?;
        if sources.is_empty() {
            return Err("Nie sú vybrané žiadne súbory".into());
        }
        Ok(sources)
    }

    fn files_of(&self, source: &Source) -> Result<Vec<PathBuf>, String> {
        Ok(sidecar_pairs(&source.path, &source.path)?
            .into_iter()
            .map(|p| p.from)
            .collect())
    }

    /// What a delete would do, for the confirmation dialog: how many files go permanently (no Recycle
    /// Bin on their drive, or `permanent` requested with Shift+Delete).
    pub fn delete_info(&self, ids: &[i64], permanent: bool) -> Result<DeleteInfo, String> {
        let sources = self.delete_sources(ids)?;
        let mut sidecars = 0usize;
        let mut permanent_count = 0usize;
        let mut drives = Vec::<String>::new();
        for source in &sources {
            sidecars += self.files_of(source)?.len().saturating_sub(1);
            let bin = self.fs.has_recycle_bin(&source.path);
            if permanent || !bin {
                permanent_count += 1;
            }
            if !bin {
                let drive = volume_key(&source.path).to_uppercase();
                if !drives.contains(&drive) {
                    drives.push(drive);
                }
            }
        }
        Ok(DeleteInfo {
            count: sources.len(),
            names: sources.iter().take(5).map(|s| s.name.clone()).collect(),
            sidecars,
            permanent_count,
            no_bin_drives: drives,
        })
    }

    /// Adds physically purged catalogue ids to the output even when the catalogue write fails.
    pub fn delete_files(
        &self,
        ids: &[i64],
        permanent: bool,
        confirmed_no_bin_drives: &[String],
        purged_ids: &mut Vec<i64>,
    ) -> Result<FileOperationResult, String> {
        let _operation = self.lock_operation()?;
        self.writable()?;
        let sources = self.delete_sources(ids)?;
        let (to_bin, to_purge): (Vec<_>, Vec<_>) = sources
            .iter()
            .partition(|s| !permanent && self.fs.has_recycle_bin(&s.path));
        if !permanent
            && to_purge.iter().any(|source| {
                let drive = volume_key(&source.path).to_uppercase();
                !confirmed_no_bin_drives.contains(&drive)
            })
        {
            return Err("Na disku nie je dostupný Kôš. Trvalé zmazanie vyžaduje potvrdenie.".into());
        }
        let to_purge = to_purge
            .into_iter()
            .map(|source| Ok((source, self.files_of(source)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut summary_parts = Vec::new();
        let mut change_id = None;
        let mut affected_ids = Vec::new();
        let mut folders = Vec::new();
        let mut invalidate_parent = |path: &Path| {
            if let Some(parent) = path.parent() {
                let parent = parent.to_string_lossy().into_owned();
                if !folders.contains(&parent) {
                    let _ = self.listing.invalidate_path(&parent);
                    folders.push(parent);
                }
            }
        };
        if !to_bin.is_empty() {
            let mut files = Vec::new();
            for source in &to_bin {
                for file in self.files_of(source)? {
                    if !files.iter().any(|f: &PathBuf| path_eq(f, &file)) {
                        files.push(file);
                    }
                }
            }
            self.fs.recycle_delete(&files)?;
            for source in &to_bin {
                invalidate_parent(&source.path);
            }

            let count = to_bin.len();
            let summary = format!("Odstránené do Koša: {count} {}", slovak_files(count));
            let catalog_ids = to_bin.iter().filter(|s| s.id > 0).map(|s| s.id).collect::<Vec<_>>();
            change_id = self
                .writer
                .record_file_delete(&catalog_ids, &summary)
                .map_err(|error| combine_error(error, self.restore_recycled_files(&files, false)))?;
            affected_ids.extend(to_bin.iter().map(|s| s.requested_id));
            summary_parts.push(summary);
        }
        if !to_purge.is_empty() {
            let mut failed = Vec::new();
            let mut deleted_files = Vec::new();
            let mut deleted_paths = HashSet::new();
            let mut done = 0;
            let first_purged = purged_ids.len();
            for (source, files) in to_purge {
                for file in files {
                    if deleted_paths.contains(&key(&file)) {
                        continue;
                    }
                    match self.fs.remove_file(&file) {
                        Ok(()) => {
                            invalidate_parent(&file);
                            deleted_paths.insert(key(&file));
                            deleted_files.push(file.display().to_string());
                        }
                        Err(error) => failed.push(format!("{}: {error}", file.display())),
                    }
                }
                if deleted_paths.contains(&key(&source.path)) {
                    done += 1;
                    affected_ids.push(source.requested_id);
                    if source.id > 0 {
                        purged_ids.push(source.id);
                    }
                }
            }
            let summary = format!("Zmazané natrvalo: {done} {}", slovak_files(done));
            if !deleted_files.is_empty() {
                change_id = self
                    .writer
                    .record_named_file_delete(&purged_ids[first_purged..], "purge_files", &summary)
                    .map_err(|error| {
                        format!(
                            "Súbory už boli natrvalo zmazané z disku, zápis do katalógu zlyhal: {error} · {}",
                            deleted_files.join(", ")
                        )
                    })?
                    .or(change_id);
            }
            summary_parts.push(summary);
            if !failed.is_empty() {
                summary_parts.push(format!("Nepodarilo sa zmazať: {}", failed.join("; ")));
            }
        }
        Ok(FileOperationResult {
            change_id,
            summary: summary_parts.join(" · "),
            changed_count: affected_ids.len(),
            skipped_count: sources.len() - affected_ids.len(),
            remapped_ids: Vec::new(),
            affected_ids,
            affected_folder_paths: folders,
        })
    }
}
