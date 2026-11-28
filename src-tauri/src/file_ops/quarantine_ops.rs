use super::*;
use crate::{
    exiftool::ExifTool,
    formats::FormatTable,
    incoming::{Photo, fill_capture_dates},
    quarantine::{
        Outcome, Review, ReviewList, ReviewPreview, ReviewSelection, Settings, Snapshot, Stage, checked_path,
        due_candidates, inside, validate_roots,
    },
};

impl FileManager {
    pub(crate) fn check_quarantine_relocation(&self, old: &str, new: &str) -> Result<(), String> {
        if key(Path::new(old)) == key(Path::new(new)) {
            return Ok(());
        }
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("SELECT coalesce(path,'') FROM assets WHERE quarantined_at IS NOT NULL AND quarantine_origin IS NOT NULL")
            .map_err(|e| e.to_string())?;
        for row in stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
        {
            let path = row.map_err(|e| e.to_string())?;
            if !inside(Path::new(&path), Path::new(new)) {
                return Err("V karanténe sú fotky. Pred zmenou priečinka ich vráťte alebo odstráňte.".into());
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_quarantine_review(
        &self,
        settings: Settings,
        formats: &FormatTable,
        exif: Option<&ExifTool>,
    ) -> Result<ReviewPreview, String> {
        self.writable()?;
        let mut pending = self.review.lock().map_err(|e| e.to_string())?;
        *pending = None;
        if settings.quarantine().as_os_str().is_empty() {
            return Ok(ReviewPreview::default());
        }
        validate_roots(settings.incoming(), settings.quarantine())?;
        let (moves, deletes, stale_edits) = {
            let conn = self.db.lock().map_err(|e| e.to_string())?;
            let at = crate::quarantine::now();
            (
                due_candidates(&conn, &settings, formats, Stage::Move, at, None)?,
                due_candidates(&conn, &settings, formats, Stage::Delete, at, None)?,
                due_candidates(&conn, &settings, formats, Stage::StaleEdit, at, None)?,
            )
        };
        let (mut move_candidates, move_snapshots): (Vec<_>, Vec<_>) = moves
            .into_iter()
            .filter_map(|photo| {
                let path = Path::new(&photo.path);
                if !photo_format(formats, path) {
                    return None;
                }
                let snapshot = self.snapshot(photo.id, path, settings.incoming()).ok()?;
                Some((photo, snapshot))
            })
            .unzip();
        fill_missing_dates(exif, &mut move_candidates);
        let moves = move_candidates
            .iter()
            .zip(move_snapshots)
            .filter_map(|(photo, snapshot)| settings.manages(photo).then_some(snapshot))
            .collect::<Vec<_>>();
        let deletes = deletes
            .into_iter()
            .filter_map(|photo| {
                self.snapshot(photo.id, Path::new(&photo.path), settings.quarantine())
                    .ok()
            })
            .collect::<Vec<_>>();
        let stale_edits = stale_edits
            .into_iter()
            .filter_map(|photo| {
                let path = Path::new(&photo.path);
                if !photo_format(formats, path) {
                    return None;
                }
                self.review_snapshot(Stage::StaleEdit, photo.id, path, &settings).ok()
            })
            .collect::<Vec<_>>();
        let list = |files: &[Snapshot]| ReviewList {
            ids: files.iter().map(|file| file.id).collect(),
            bytes: files
                .iter()
                .flat_map(|file| &file.files)
                .map(|(_, size, _, _)| size)
                .sum(),
        };
        let mut no_bin_drives = deletes
            .iter()
            .filter(|file| !self.fs.has_recycle_bin(&file.path))
            .map(|file| volume_key(&file.path).to_uppercase())
            .collect::<Vec<_>>();
        no_bin_drives.sort();
        no_bin_drives.dedup();
        let preview = ReviewPreview {
            moves: list(&moves),
            deletes: list(&deletes),
            stale_edits: list(&stale_edits),
            edit_after_days: settings.edit_after_days(),
            no_bin_drives,
        };
        *pending = Some(Review {
            settings,
            moves,
            deletes,
            stale_edits,
        });
        Ok(preview)
    }

    pub(crate) fn execute_quarantine_review(
        &self,
        settings: &Settings,
        formats: &FormatTable,
        exif: Option<&ExifTool>,
        selection: ReviewSelection,
        drives: &[String],
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Outcome, String> {
        self.writable()?;
        let mut review = self
            .review
            .lock()
            .map_err(|e| e.to_string())?
            .take()
            .ok_or("Chýba zoznam fotiek na schválenie")?;
        if review.settings != *settings {
            return Err("Nastavenia importu alebo karantény sa zmenili. Skontrolujte zoznam po ďalšom štarte.".into());
        }
        validate_roots(settings.incoming(), settings.quarantine())?;
        if !selection.moves {
            review.moves.clear();
        }
        if !selection.deletes {
            review.deletes.clear();
        }
        if !selection.stale_edits {
            review.stale_edits.clear();
        }
        let total = review.moves.len() + review.deletes.len() + review.stale_edits.len();
        let mut reserved = if review.moves.is_empty() && review.stale_edits.is_empty() {
            Ok(HashSet::new())
        } else {
            self.quarantine_stems(settings.quarantine())
        };
        let mut outcome = Outcome::default();
        for (index, (stage, snapshot)) in review
            .moves
            .into_iter()
            .map(|p| (Stage::Move, p))
            .chain(review.stale_edits.into_iter().map(|p| (Stage::StaleEdit, p)))
            .chain(review.deletes.into_iter().map(|p| (Stage::Delete, p)))
            .enumerate()
        {
            progress(index, total);
            let result = (|| {
                let mut eligible = {
                    let conn = self.db.lock().map_err(|e| e.to_string())?;
                    due_candidates(
                        &conn,
                        settings,
                        formats,
                        stage,
                        crate::quarantine::now(),
                        Some(snapshot.id),
                    )?
                };
                let path = eligible
                    .first()
                    .map(|photo| Path::new(&photo.path))
                    .ok_or("Podmienky sa zmenili")?;
                if self.review_snapshot(stage, snapshot.id, path, settings)? != snapshot {
                    return Err("Súbor alebo XMP sa od kontroly zmenil".into());
                }
                if stage == Stage::Move {
                    fill_missing_dates(exif, &mut eligible);
                    if !settings.manages(&eligible[0]) {
                        return Err("Dátum snímania nespĺňa podmienky".into());
                    }
                }
                let source = self.quarantine_source(snapshot.id)?;
                if source.path != snapshot.path {
                    return Err("Cesta súboru sa od kontroly zmenila".into());
                }
                match stage {
                    Stage::Move | Stage::StaleEdit => self.move_quarantine_file(
                        source,
                        settings.quarantine(),
                        ConflictPolicy::KeepBoth,
                        Some(reserved.as_mut().map_err(|error| error.clone())?),
                        &mut outcome,
                    ),
                    Stage::Delete => self.delete_quarantine_file(&source, drives, &mut outcome),
                }
            })();
            if let Err(error) = result {
                outcome.failures.push(format!("{}: {error}", snapshot.path.display()));
            }
        }
        progress(total, total);
        Ok(outcome)
    }

    pub(crate) fn move_to_quarantine(
        &self,
        ids: &[i64],
        settings: &Settings,
        formats: &FormatTable,
        policy: ConflictPolicy,
        suggestions_only: bool,
        progress: impl FnMut(usize, usize),
    ) -> Result<Outcome, String> {
        self.writable()?;
        validate_roots(settings.incoming(), settings.quarantine())?;
        let target = settings.quarantine();
        if target.as_os_str().is_empty() {
            return Err("Najprv vyberte priečinok karantény v nastaveniach".into());
        }
        let mut reserved = self.quarantine_stems(target)?;
        Ok(self.quarantine_batch(ids, progress, |id, outcome| {
            let source = self.quarantine_source(id)?;
            if !photo_format(formats, &source.path) {
                return Err("Do karantény možno presunúť len fotky".into());
            }
            if source.id > 0 && self.quarantine_origin(source.id)?.is_some() {
                return Err("Fotka už je v karanténe".into());
            }
            if suggestions_only {
                let conn = self.db.lock().map_err(|e| e.to_string())?;
                let suggested: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND delete_suggestion=1)",
                        [source.id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if !suggested || !inside(&checked_path(&source.path)?, &checked_path(settings.incoming())?) {
                    return Err("Fotka už nie je návrhom v importovacom priečinku".into());
                }
            }
            if inside(&checked_path(&source.path)?, &checked_path(target)?) {
                return Err("Fotka už je v priečinku karantény".into());
            }
            self.move_quarantine_file(source, target, policy, Some(&mut reserved), outcome)
        }))
    }

    pub(crate) fn restore_from_quarantine(
        &self,
        ids: &[i64],
        settings: &Settings,
        progress: impl FnMut(usize, usize),
    ) -> Result<Outcome, String> {
        self.writable()?;
        validate_roots(settings.incoming(), settings.quarantine())?;
        Ok(self.quarantine_batch(ids, progress, |id, outcome| {
            let (mut source, origin) = self.quarantined_source(id)?;
            self.snapshot(id, &source.path, settings.quarantine())?;
            let target = origin.parent().ok_or("Chýba pôvodný priečinok")?;
            if inside(&checked_path(&origin)?, &checked_path(settings.quarantine())?) {
                return Err("Pôvodná cesta patrí do karantény".into());
            }
            source.name = origin
                .file_name()
                .ok_or("Chýba pôvodný názov")?
                .to_string_lossy()
                .into_owned();
            self.move_quarantine_file(source, target, ConflictPolicy::KeepBoth, None, outcome)
        }))
    }

    pub(crate) fn quarantine_delete_info(&self, ids: &[i64], settings: &Settings) -> Result<DeleteInfo, String> {
        self.writable()?;
        validate_roots(settings.incoming(), settings.quarantine())?;
        let mut pending = self.quarantine_delete.lock().map_err(|e| e.to_string())?;
        pending.clear();
        let snapshots = ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .map(|id| {
                let (source, _) = self.quarantined_source(id)?;
                self.snapshot(id, &source.path, settings.quarantine())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let info = self.delete_info(ids, false)?;
        *pending = snapshots;
        Ok(info)
    }

    pub(crate) fn delete_from_quarantine(
        &self,
        ids: &[i64],
        settings: &Settings,
        drives: &[String],
        progress: impl FnMut(usize, usize),
    ) -> Result<Outcome, String> {
        self.writable()?;
        validate_roots(settings.incoming(), settings.quarantine())?;
        let pending = std::mem::take(&mut *self.quarantine_delete.lock().map_err(|e| e.to_string())?);
        let snapshots: std::collections::HashMap<_, _> = pending.into_iter().map(|file| (file.id, file)).collect();
        if snapshots.is_empty()
            || snapshots.len() != ids.iter().collect::<HashSet<_>>().len()
            || ids.iter().any(|id| !snapshots.contains_key(id))
        {
            return Err("Výber sa zmenil. Znova otvorte potvrdenie zmazania.".into());
        }
        Ok(self.quarantine_batch(ids, progress, |id, outcome| {
            let (source, _) = self.quarantined_source(id)?;
            if self.snapshot(id, &source.path, settings.quarantine())? != snapshots[&id] {
                return Err("Súbor alebo XMP sa od potvrdenia zmenil".into());
            }
            self.delete_quarantine_file(&source, drives, outcome)
        }))
    }

    fn quarantine_batch(
        &self,
        ids: &[i64],
        mut progress: impl FnMut(usize, usize),
        mut action: impl FnMut(i64, &mut Outcome) -> Result<(), String>,
    ) -> Outcome {
        let mut result = Outcome::default();
        let mut seen = HashSet::new();
        let ids = ids.iter().copied().filter(|id| seen.insert(*id)).collect::<Vec<_>>();
        for (index, &id) in ids.iter().enumerate() {
            progress(index, ids.len());
            if let Err(error) = action(id, &mut result) {
                result.failures.push(format!("Položka {id}: {error}"));
            }
        }
        progress(ids.len(), ids.len());
        result
    }

    fn quarantine_source(&self, id: i64) -> Result<Source, String> {
        self.resolve_sources(&[id])?
            .pop()
            .ok_or_else(|| "Súbor sa nenašiel".into())
    }

    fn quarantine_origin(&self, id: i64) -> Result<Option<PathBuf>, String> {
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT quarantine_origin FROM assets WHERE id=?1 AND quarantined_at IS NOT NULL",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|row| row.flatten().map(PathBuf::from))
        .map_err(|e| e.to_string())
    }

    fn quarantined_source(&self, id: i64) -> Result<(Source, PathBuf), String> {
        let source = self.quarantine_source(id)?;
        let origin = self.quarantine_origin(source.id)?.ok_or("Položka nie je v karanténe")?;
        Ok((source, origin))
    }

    fn review_snapshot(&self, stage: Stage, id: i64, path: &Path, settings: &Settings) -> Result<Snapshot, String> {
        let snapshot = self.snapshot(id, path, stage.root(settings))?;
        if stage == Stage::StaleEdit {
            let since: Option<i64> = self
                .db
                .lock()
                .map_err(|e| e.to_string())?
                .query_row("SELECT edit_pending_since FROM assets WHERE id=?1", [id], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if since.is_none_or(|since| {
                snapshot.files.iter().any(|(file, _, modified, _)| {
                    file == path && crate::quarantine::modified_after_mark(*modified, since)
                })
            }) {
                return Err("Fotka už bola upravená".into());
            }
        }
        Ok(snapshot)
    }

    fn snapshot(&self, id: i64, path: &Path, root: &Path) -> Result<Snapshot, String> {
        let scope = checked_path(root)?;
        let files = sidecar_pairs(path, path)?
            .into_iter()
            .map(|pair| {
                let resolved = checked_path(&pair.from)?;
                if resolved == scope || !resolved.starts_with(&scope) {
                    return Err(format!("Súbor je mimo povoleného priečinka: {}", pair.from.display()));
                }
                let info = self
                    .fs
                    .metadata(&pair.from)
                    .map_err(|e| format!("{}: {e}", pair.from.display()))?;
                if !info.is_file() {
                    return Err(format!("Položka nie je súbor: {}", pair.from.display()));
                }
                Ok((
                    pair.from,
                    info.len(),
                    info.modified().map_err(|e| e.to_string())?,
                    info.created().ok(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Snapshot {
            id,
            path: path.to_owned(),
            files,
        })
    }

    fn quarantine_stems(&self, folder: &Path) -> Result<HashSet<String>, String> {
        if !self.fs.exists(folder) {
            return Ok(HashSet::new());
        }
        let files = self
            .fs
            .read_dir(folder)
            .map_err(|e| format!("{}: {e}", folder.display()))?;
        Ok(files.iter().map(|path| file_stem_key(path)).collect())
    }

    fn move_quarantine_file(
        &self,
        source: Source,
        folder: &Path,
        policy: ConflictPolicy,
        reserved: Option<&mut HashSet<String>>,
        outcome: &mut Outcome,
    ) -> Result<(), String> {
        let source_folder = source.path.parent().ok_or("Chýba priečinok súboru")?.to_owned();
        self.snapshot(source.id, &source.path, &source_folder)?;
        checked_path(folder)?;
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        checked_path(folder)?;
        let mut plan = self.plan(&[source], folder, None)?.pop().ok_or("Chýba plán presunu")?;
        let entering = reserved.is_some();
        // Two photos with different extensions can share a stem-based XMP.
        // Assign distinct stems in quarantine so their expiry can be independent.
        let overlaps_sidecar = |path: &Path| {
            entering
                && !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"))
                && (reserved
                    .as_ref()
                    .is_some_and(|stems| stems.contains(&file_stem_key(path)))
                    || self.fs.exists(&path.with_extension("xmp"))
                    || self.fs.exists(&path.with_file_name(format!(
                        "{}.xmp",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ))))
        };
        plan.conflict |= overlaps_sidecar(&plan.target);
        if plan.conflict && policy != ConflictPolicy::KeepBoth {
            return Err(format!("Kolízia názvu, preskočené: {}", plan.target.display()));
        }
        let plan = self.number_conflict_with(plan, folder, overlaps_sidecar)?;
        let transfer = FileCatalogMove {
            asset_id: plan.source.id,
            target_path: plan.target.to_string_lossy().into_owned(),
            target_folder_path: folder.to_string_lossy().into_owned(),
            name: plan
                .target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            ext: plan
                .target
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            size: plan.source.size as f64,
            file_modified: Some(crate::listing::system_time_iso(plan.source.modified)),
        };
        let origin = plan.source.path.to_string_lossy();
        let summary = format!(
            "{}: {} → {}",
            if entering {
                "Presun do karantény"
            } else {
                "Návrat z karantény"
            },
            origin,
            transfer.target_path
        );
        self.writer
            .record_file_intent("confirm_quarantine_move", &format!("Potvrdené · {summary}"))?;
        let applied = move_transfers(self.fs.as_ref(), &plan.pairs)?;
        if let Err(error) = self
            .writer
            .record_quarantine_move(&summary, &transfer, &origin, entering)
        {
            return Err(combine_error(error, rollback_moves(self.fs.as_ref(), &applied)));
        }
        if let Some(stems) = reserved {
            stems.insert(file_stem_key(&plan.target));
        }
        self.quarantine_invalidate(&source_folder, outcome);
        self.quarantine_invalidate(folder, outcome);
        if entering {
            outcome.moved += 1;
        } else {
            outcome.restored += 1;
        }
        Ok(())
    }

    fn delete_quarantine_file(&self, source: &Source, drives: &[String], outcome: &mut Outcome) -> Result<(), String> {
        let recycle = self.fs.has_recycle_bin(&source.path);
        if !recycle && !drives.contains(&volume_key(&source.path).to_uppercase()) {
            return Err("Na disku nie je dostupný Kôš. Trvalé zmazanie vyžaduje potvrdenie.".into());
        }
        let folder = source.path.parent().ok_or("Chýba priečinok súboru")?;
        let snapshot = self.snapshot(source.id, &source.path, folder)?;
        let files = snapshot
            .files
            .into_iter()
            .map(|(path, _, _, _)| path)
            .collect::<Vec<_>>();
        let names = files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" · ");
        let action = if recycle {
            "delete_from_quarantine"
        } else {
            "purge_from_quarantine"
        };
        let confirmation = if recycle {
            "Potvrdený presun z karantény do Koša"
        } else {
            "Potvrdené trvalé zmazanie z karantény"
        };
        self.writer
            .record_file_intent("confirm_quarantine_delete", &format!("{confirmation}: {names}"))?;
        let mut failures = Vec::new();
        if recycle {
            if let Err(error) = self.fs.recycle_delete(&files) {
                return Err(combine_error(error, self.restore_recycled_files(&files, true)));
            }
        } else {
            self.fs
                .remove_file(&source.path)
                .map_err(|e| format!("{}: {e}", source.path.display()))?;
            for sidecar in files.iter().skip(1) {
                if let Err(e) = self.fs.remove_file(sidecar) {
                    failures.push(format!("{}: {e}", sidecar.display()));
                }
            }
            outcome.purged.push(source.id);
        }
        let summary = format!(
            "{}: {names}",
            if recycle {
                "Z karantény do Koša"
            } else {
                "Z karantény zmazané natrvalo"
            }
        );
        if let Err(error) = self.writer.record_named_file_delete(&[source.id], action, &summary) {
            if recycle {
                return Err(combine_error(error, self.restore_recycled_files(&files, false)));
            }
            outcome.deleted += 1;
            self.quarantine_invalidate(folder, outcome);
            let cleanup = self.writer.purge_assets(&[source.id]);
            return Err(match cleanup {
                Ok(()) => {
                    format!("Súbor aj záznam boli odstránené, zápis o výsledku zlyhal: {error}")
                }
                Err(cleanup) => {
                    format!("Súbor bol zmazaný, záznam ostal v katalógu: {error} · {cleanup}")
                }
            });
        }
        self.quarantine_invalidate(folder, outcome);
        outcome.deleted += 1;
        if !failures.is_empty() {
            return Err(format!("Nepodarilo sa zmazať XMP: {}", failures.join(" · ")));
        }
        Ok(())
    }

    pub(super) fn restore_recycled_files(&self, files: &[PathBuf], skip_existing: bool) -> Result<(), String> {
        let errors = files
            .iter()
            .filter(|path| !skip_existing || !self.fs.exists(path))
            .filter_map(|path| {
                self.fs
                    .restore_recycled(path)
                    .err()
                    .map(|error| format!("{}: {error}", path.display()))
            })
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" · "))
        }
    }

    fn quarantine_invalidate(&self, folder: &Path, outcome: &mut Outcome) {
        let path = folder.to_string_lossy().into_owned();
        let _ = self.listing.invalidate_path(&path);
        if !outcome.affected_folder_paths.contains(&path) {
            outcome.affected_folder_paths.push(path);
        }
    }
}

fn fill_missing_dates(exif: Option<&ExifTool>, photos: &mut [Photo]) {
    if let Some(exif) = exif {
        let missing: HashSet<_> = photos
            .iter()
            .filter(|photo| photo.capture.is_none())
            .map(|photo| photo.id)
            .collect();
        fill_capture_dates(exif, photos, |id| missing.contains(&id));
    }
}

fn photo_format(formats: &FormatTable, path: &Path) -> bool {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(formats.group(&ext), Some("image" | "raw"))
}

fn file_stem_key(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase()
}
