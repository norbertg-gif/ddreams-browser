use crate::journal::{FileCatalogCopy, FileCatalogMove};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::filesystem::FileSystemOps;
use super::rename::{key, path_eq};
use super::sources::sidecar_pairs;
use super::{FileManager, FileOperationResult, Plan, Source, Transfer};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Skip,
    KeepBoth,
    Cancel,
}

fn copy_transfers(fs: &dyn FileSystemOps, pairs: &[Transfer]) -> Result<Vec<Transfer>, String> {
    let mut done = Vec::<Transfer>::new();
    for pair in pairs {
        if let Err(error) = fs.copy_no_replace(&pair.from, &pair.to) {
            let rollback = remove_created(fs, &done.iter().map(|x| x.to.clone()).collect::<Vec<_>>());
            return Err(combine_error(format!("{}: {error}", pair.from.display()), rollback));
        }
        done.push(pair.clone());
    }
    Ok(done)
}

pub(super) fn move_transfers(fs: &dyn FileSystemOps, pairs: &[Transfer]) -> Result<Vec<Transfer>, String> {
    let mut done = Vec::<Transfer>::new();
    for pair in pairs {
        let result = if fs.same_volume(&pair.from, &pair.to) {
            move_case_safe(fs, &pair.from, &pair.to)
        } else {
            copy_then_remove(fs, &pair.from, &pair.to)
        };
        if let Err(error) = result {
            let rollback = rollback_moves(fs, &done);
            return Err(combine_error(format!("{}: {error}", pair.from.display()), rollback));
        }
        done.push(pair.clone());
    }
    Ok(done)
}

pub(super) fn move_case_safe(fs: &dyn FileSystemOps, source: &Path, target: &Path) -> io::Result<()> {
    if path_eq(source, target) && source != target {
        let temp = temporary_sibling(source);
        fs.move_no_replace(source, &temp)?;
        if let Err(error) = fs.move_no_replace(&temp, target) {
            let _ = fs.move_no_replace(&temp, source);
            return Err(error);
        }
        Ok(())
    } else {
        fs.move_no_replace(source, target)
    }
}

fn temporary_sibling(path: &Path) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".ddreams-rename-{}-{n}", std::process::id()))
}

pub(super) fn copy_then_remove(fs: &dyn FileSystemOps, source: &Path, target: &Path) -> io::Result<()> {
    let before = fs.metadata(source)?;
    let before_modified = before.modified()?;
    fs.copy_no_replace(source, target)?;
    let copied = fs.metadata(target)?;
    let copied_modified = copied.modified()?;
    let source_after = fs.metadata(source)?;
    let after_modified = source_after.modified()?;
    let time_difference = if before_modified >= copied_modified {
        before_modified
            .duration_since(copied_modified)
            .unwrap_or_else(|error| error.duration())
    } else {
        copied_modified
            .duration_since(before_modified)
            .unwrap_or_else(|error| error.duration())
    };
    if before.len() != copied.len()
        || before.len() != source_after.len()
        || before_modified != after_modified
        || time_difference > std::time::Duration::from_secs(2)
    {
        let _ = fs.remove_file(target);
        return Err(io::Error::other("copy verification failed"));
    }
    if let Err(error) = fs.remove_file(source) {
        let _ = fs.remove_file(target);
        return Err(error);
    }
    Ok(())
}

pub(super) fn rollback_moves(fs: &dyn FileSystemOps, done: &[Transfer]) -> Result<(), String> {
    let reverse = done
        .iter()
        .rev()
        .map(|x| Transfer {
            from: x.to.clone(),
            to: x.from.clone(),
        })
        .collect::<Vec<_>>();
    let mut restored = Vec::<Transfer>::new();
    for pair in &reverse {
        let result = if fs.same_volume(&pair.from, &pair.to) {
            move_case_safe(fs, &pair.from, &pair.to)
        } else {
            copy_then_remove(fs, &pair.from, &pair.to)
        };
        if let Err(error) = result {
            let redo = restored
                .iter()
                .rev()
                .map(|x| Transfer {
                    from: x.to.clone(),
                    to: x.from.clone(),
                })
                .collect::<Vec<_>>();
            let _ = move_transfers(fs, &redo);
            return Err(format!("Rollback failed for {}: {error}", pair.from.display()));
        }
        restored.push(pair.clone());
    }
    Ok(())
}

fn remove_created(fs: &dyn FileSystemOps, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths.iter().rev() {
        if fs.exists(path) {
            fs.remove_file(path)
                .map_err(|e| format!("Nepodarilo sa odstrániť čiastočnú kópiu {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

pub(super) fn combine_error(error: impl ToString, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => error.to_string(),
        Err(rollback) => format!("{}; {}", error.to_string(), rollback),
    }
}

pub(super) fn slovak_files(n: usize) -> &'static str {
    if n == 1 {
        "súbor"
    } else if (2..=4).contains(&n) {
        "súbory"
    } else {
        "súborov"
    }
}

impl FileManager {
    pub fn preview_conflicts(&self, ids: &[i64], target: &str, rename_base: Option<&str>) -> Result<usize, String> {
        let folder = self.validate_folder(target)?;
        let sources = self.resolve_sources(ids)?;
        let plans = self.plan(&sources, &folder, rename_base)?;
        Ok(plans
            .iter()
            .filter(|p| !path_eq(p.source.path.parent().unwrap_or(Path::new("")), &folder) && p.conflict)
            .count())
    }

    pub fn uncategorized_photos_for_move(
        &self,
        ids: &[i64],
        target: &str,
        policy: ConflictPolicy,
        formats: &crate::formats::FormatTable,
    ) -> Result<usize, String> {
        let folder = self.validate_folder(target)?;
        let sources = self.resolve_sources(ids)?;
        let mut candidates = Vec::new();
        for plan in self.plan(&sources, &folder, None)? {
            if path_eq(plan.source.path.parent().unwrap_or(Path::new("")), &folder)
                || (policy == ConflictPolicy::Skip && plan.conflict)
            {
                continue;
            }
            let ext = plan
                .source
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if matches!(formats.group(&ext), Some("image" | "raw")) {
                candidates.push(plan.source.id);
            }
        }
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let mut assigned = conn
            .prepare("SELECT EXISTS(SELECT 1 FROM asset_categories WHERE asset_id=?1)")
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        for id in candidates {
            if id < 0
                || !assigned
                    .query_row([id], |row| row.get::<_, bool>(0))
                    .map_err(|e| e.to_string())?
            {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn move_files<F>(
        &self,
        ids: &[i64],
        target: &str,
        policy: ConflictPolicy,
        mut progress: F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        self.run_copy_move(ids, target, policy, false, &mut progress)
    }

    pub fn copy_files<F>(
        &self,
        ids: &[i64],
        target: &str,
        policy: ConflictPolicy,
        mut progress: F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        self.run_copy_move(ids, target, policy, true, &mut progress)
    }

    pub(super) fn run_copy_move<F>(
        &self,
        ids: &[i64],
        target: &str,
        policy: ConflictPolicy,
        copy: bool,
        progress: &mut F,
    ) -> Result<FileOperationResult, String>
    where
        F: FnMut(usize, usize),
    {
        let _operation = self.lock_operation()?;
        self.writable()?;
        let folder = self.validate_folder(target)?;
        let sources = self.resolve_sources(ids)?;
        if sources.is_empty() {
            return Err("Nie sú vybrané žiadne súbory".into());
        }
        let mut plans = self.plan(&sources, &folder, None)?;
        let same_folder_count = plans
            .iter()
            .filter(|plan| path_eq(plan.source.path.parent().unwrap_or(Path::new("")), &folder))
            .count();
        plans.retain(|plan| !path_eq(plan.source.path.parent().unwrap_or(Path::new("")), &folder));
        if same_folder_count == sources.len() {
            return Ok(FileOperationResult {
                change_id: None,
                summary: "Súbory už sú v tomto priečinku".into(),
                changed_count: 0,
                skipped_count: 0,
                remapped_ids: Vec::new(),
                affected_ids: Vec::new(),
                affected_folder_paths: Vec::new(),
            });
        }
        let conflicts = plans.iter().filter(|p| p.conflict).count();
        if conflicts > 0 && policy == ConflictPolicy::Cancel {
            return Err("Operácia bola zrušená".into());
        }
        let mut skipped = 0usize;
        let mut reserved = HashSet::new();
        let mut selected = Vec::new();
        for mut plan in plans.drain(..) {
            if plan.conflict && policy == ConflictPolicy::Skip {
                skipped += 1;
                continue;
            }
            if policy == ConflictPolicy::KeepBoth {
                plan = self.number_conflict(plan, &folder, &reserved)?;
            }
            if plan
                .pairs
                .iter()
                .any(|pair| self.fs.exists(&pair.to) || reserved.contains(&key(&pair.to)))
            {
                if policy == ConflictPolicy::Skip {
                    skipped += 1;
                    continue;
                }
                return Err(format!("V cieli už existuje súbor: {}", plan.target.display()));
            }
            for pair in &plan.pairs {
                reserved.insert(key(&pair.to));
            }
            selected.push(plan);
        }
        if selected.is_empty() {
            return Ok(self.no_change_result(skipped, if copy { "Kopírovanie" } else { "Presun" }));
        }
        let total = selected.len();
        let mut applied = Vec::<Transfer>::new();
        let mut copied_transfers = Vec::<Transfer>::new();
        let mut moved_catalog = Vec::new();
        let mut copied_catalog = Vec::new();
        let mut source_ids = Vec::new();
        for (index, plan) in selected.iter().enumerate() {
            progress(index + 1, total);
            let result = if copy {
                copy_transfers(self.fs.as_ref(), &plan.pairs)
            } else {
                move_transfers(self.fs.as_ref(), &plan.pairs)
            };
            match result {
                Ok(done) => {
                    if copy {
                        copied_transfers.extend(done);
                    } else {
                        applied.extend(done);
                    }
                    source_ids.push(plan.source.id);
                    if plan.source.id > 0 {
                        if copy {
                            copied_catalog.push(FileCatalogCopy {
                                source_asset_id: plan.source.id,
                                target_path: plan.target.to_string_lossy().to_string(),
                                target_folder_path: folder.to_string_lossy().to_string(),
                                name: plan
                                    .target
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                ext: plan
                                    .target
                                    .extension()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                size: plan.source.size as f64,
                                file_modified: Some(crate::listing::system_time_iso(plan.source.modified)),
                            });
                        } else {
                            moved_catalog.push(FileCatalogMove {
                                asset_id: plan.source.id,
                                target_path: plan.target.to_string_lossy().to_string(),
                                target_folder_path: folder.to_string_lossy().to_string(),
                                name: plan
                                    .target
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                ext: plan
                                    .target
                                    .extension()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                size: plan.source.size as f64,
                                file_modified: Some(crate::listing::system_time_iso(plan.source.modified)),
                            });
                        }
                    }
                }
                Err(error) => {
                    let rollback = if copy {
                        remove_created(
                            self.fs.as_ref(),
                            &copied_transfers.iter().map(|x| x.to.clone()).collect::<Vec<_>>(),
                        )
                    } else {
                        rollback_moves(self.fs.as_ref(), &applied)
                    };
                    return Err(combine_error(error, rollback));
                }
            }
        }

        let count = source_ids.len();
        let noun = slovak_files(count);
        let past = if copy {
            if count == 1 {
                "Skopírovaný"
            } else if count <= 4 {
                "Skopírované"
            } else {
                "Skopírovaných"
            }
        } else if count == 1 {
            "Presunutý"
        } else if count <= 4 {
            "Presunuté"
        } else {
            "Presunutých"
        };
        let destination = folder.file_name().unwrap_or(folder.as_os_str()).to_string_lossy();
        let summary = format!("{past} {count} {noun} do {destination}");
        let recorded = self.writer.record_file_change(
            if copy { "copy_files" } else { "move_files" },
            &summary,
            &moved_catalog,
            &copied_catalog,
            &[],
        );
        let (change_id, remapped_ids) = match recorded {
            Ok(v) => v,
            Err(error) => {
                let rollback = if copy {
                    remove_created(
                        self.fs.as_ref(),
                        &copied_transfers.iter().map(|x| x.to.clone()).collect::<Vec<_>>(),
                    )
                } else {
                    rollback_moves(self.fs.as_ref(), &applied)
                };
                return Err(combine_error(error, rollback));
            }
        };
        self.invalidate_plans(&selected, &folder);
        let mut affected_folder_paths = vec![folder.to_string_lossy().to_string()];
        for plan in &selected {
            if let Some(parent) = plan.source.path.parent() {
                let path = parent.to_string_lossy().to_string();
                if !affected_folder_paths
                    .iter()
                    .any(|item| path_eq(Path::new(item), Path::new(&path)))
                {
                    affected_folder_paths.push(path);
                }
            }
        }
        Ok(FileOperationResult {
            change_id,
            summary,
            changed_count: source_ids.len(),
            skipped_count: skipped,
            remapped_ids,
            affected_ids: source_ids,
            affected_folder_paths,
        })
    }

    pub(super) fn plan(
        &self,
        sources: &[Source],
        folder: &Path,
        rename_base: Option<&str>,
    ) -> Result<Vec<Plan>, String> {
        let mut plans = Vec::new();
        let mut reserved = HashSet::new();
        for source in sources {
            let main = if let Some(base) = rename_base {
                let ext = source.path.extension().map(|e| e.to_string_lossy().to_string());
                folder.join(ext.map(|e| format!("{base}.{e}")).unwrap_or_else(|| base.to_string()))
            } else {
                folder.join(&source.name)
            };
            let pairs = sidecar_pairs(&source.path, &main)?;
            let conflict = pairs
                .iter()
                .any(|p| self.fs.exists(&p.to) || reserved.contains(&key(&p.to)));
            for pair in &pairs {
                reserved.insert(key(&pair.to));
            }
            plans.push(Plan {
                source: source.clone(),
                target: main,
                pairs,
                conflict,
            });
        }
        Ok(plans)
    }

    fn number_conflict(&self, plan: Plan, folder: &Path, reserved: &HashSet<String>) -> Result<Plan, String> {
        self.number_conflict_with(plan, folder, |path| reserved.contains(&key(path)))
    }

    pub(super) fn number_conflict_with(
        &self,
        plan: Plan,
        folder: &Path,
        reserved: impl Fn(&Path) -> bool,
    ) -> Result<Plan, String> {
        if !plan.conflict {
            return Ok(plan);
        }
        let original = plan.target.file_stem().unwrap_or_default().to_string_lossy();
        let ext = plan.target.extension().map(|e| e.to_string_lossy());
        for n in 2..100_000u32 {
            let base = format!("{original} ({n})");
            let target = folder.join(ext.as_ref().map(|e| format!("{base}.{e}")).unwrap_or(base));
            let pairs = sidecar_pairs(&plan.source.path, &target)?;
            if pairs
                .iter()
                .all(|pair| !self.fs.exists(&pair.to) && !reserved(&pair.to))
            {
                return Ok(Plan {
                    source: plan.source,
                    target,
                    pairs,
                    conflict: false,
                });
            }
        }
        Err("Nenašlo sa voľné číslo pre názov súboru".into())
    }

    fn invalidate_plans(&self, plans: &[Plan], folder: &Path) {
        let _ = self.listing.invalidate_path(&folder.to_string_lossy());
        for plan in plans {
            if let Some(parent) = plan.source.path.parent() {
                let _ = self.listing.invalidate_path(&parent.to_string_lossy());
            }
        }
    }

    pub(super) fn no_change_result(&self, skipped: usize, operation: &str) -> FileOperationResult {
        FileOperationResult {
            change_id: None,
            summary: if skipped > 0 {
                format!("{operation}: preskočené {skipped} {}", slovak_files(skipped))
            } else {
                format!("{operation}: bez zmien")
            },
            changed_count: 0,
            skipped_count: skipped,
            remapped_ids: Vec::new(),
            affected_ids: Vec::new(),
            affected_folder_paths: Vec::new(),
        }
    }
}
