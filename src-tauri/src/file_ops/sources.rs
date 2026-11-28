use rusqlite::OptionalExtension;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::rename::key;
use super::{FileManager, Source, Transfer};

pub(crate) fn sidecar_candidates(source: &Path) -> Vec<PathBuf> {
    if source.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xmp")) {
        return Vec::new();
    }
    let Some(name) = source.file_name() else {
        return Vec::new();
    };
    let mut full_name = name.to_os_string();
    full_name.push(".xmp");
    vec![source.with_extension("xmp"), source.with_file_name(full_name)]
}

pub(super) fn sidecar_pairs(source: &Path, target: &Path) -> Result<Vec<Transfer>, String> {
    let mut pairs = vec![Transfer {
        from: source.to_path_buf(),
        to: target.to_path_buf(),
    }];
    let Some(filename) = source.file_name().map(|x| x.to_string_lossy().to_string()) else {
        return Err("Názov súboru sa nedá prečítať".into());
    };
    let candidates = sidecar_candidates(source);
    let mut seen = HashSet::new();
    for candidate in candidates {
        if candidate.exists() && seen.insert(key(&candidate)) {
            let mapped = if candidate
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&format!("{filename}.xmp")))
            {
                target.with_file_name(format!("{}.xmp", target.file_name().unwrap().to_string_lossy()))
            } else {
                target.with_extension("xmp")
            };
            pairs.push(Transfer {
                from: candidate,
                to: mapped,
            });
        }
    }
    Ok(pairs)
}

impl FileManager {
    pub(super) fn resolve_sources(&self, ids: &[i64]) -> Result<Vec<Source>, String> {
        let mut unique = HashSet::new();
        let mut out = Vec::new();
        for id in ids.iter().copied() {
            if !unique.insert(id) {
                continue;
            }
            let requested_id = id;
            let (id, path, listed_name) = if id < 0 {
                let asset = self
                    .listing
                    .asset(id)
                    .ok_or_else(|| format!("Súbor {id} už nie je v aktuálnom zozname"))?;
                let path = asset.path.ok_or("Zoznam nemá cestu k súboru")?;
                let catalog_id: Option<i64> = {
                    let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
                    conn.query_row(
                        "SELECT id FROM assets WHERE is_listed=1 AND lower(path)=lower(?1) ORDER BY id LIMIT 1",
                        [&path],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                };
                (catalog_id.unwrap_or(id), PathBuf::from(path), asset.name)
            } else {
                let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
                let row: Option<(String, String)> = conn
                    .query_row(
                        "SELECT path,name FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>''",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let (path, name) = row.ok_or_else(|| format!("Súbor katalógu {id} sa nenašiel"))?;
                (id, PathBuf::from(path), name)
            };
            let metadata = self
                .fs
                .metadata(&path)
                .map_err(|_| format!("Súbor nie je dostupný: {}", path.display()))?;
            if !metadata.is_file() {
                return Err(format!("Vybraná položka nie je súbor: {}", path.display()));
            }
            let name = path
                .file_name()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or(listed_name);
            let modified = metadata
                .modified()
                .map_err(|e| format!("Čas zmeny sa nedá prečítať: {} ({e})", path.display()))?;
            out.push(Source {
                id,
                requested_id,
                path,
                name,
                size: metadata.len(),
                modified,
            });
        }
        Ok(out)
    }
}
