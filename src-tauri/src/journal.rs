use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc,Mutex}, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary { pub id: i64, pub ts: String, pub summary: String }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionSuggestionChange {
    pub change_id: Option<i64>,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct ListedAsset {
    pub temporary_id: i64,
    pub name: String,
    pub ext: String,
    pub size: Option<f64>,
    pub file_modified: Option<String>,
    pub path: String,
    pub folder_path: String,
}

#[derive(Clone, Debug)]
pub struct FileCatalogMove { pub asset_id: i64, pub target_path: String, pub target_folder_path: String, pub name: String, pub ext: String, pub size: f64, pub file_modified: Option<String> }
#[derive(Clone, Debug)]
pub struct FileCatalogCopy { pub source_asset_id: i64, pub target_path: String, pub target_folder_path: String, pub name: String, pub ext: String, pub size: f64, pub file_modified: Option<String> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIdRemap { pub old_id: i64, pub new_id: i64 }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetValueState { pub id: i64, pub rating: Option<i64>, pub label: Option<String>, pub tagged: bool, pub edit_pending_since: Option<i64> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAssetValuesResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub remapped_ids: Vec<AssetIdRemap>,
    pub assets: Vec<AssetValueState>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAssetCategoriesResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub changed_count: usize,
    pub remapped_ids: Vec<AssetIdRemap>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryMutationResult { pub change_id: Option<i64>, pub summary: String, pub category_id: Option<i64>, pub parent_id: Option<i64>, pub root_id: Option<i64>, pub ancestor_ids: Vec<i64>, pub deleted_category_ids: Vec<i64>, pub deleted_root_ids: Vec<i64> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDeleteInfo { pub id: i64, pub name: String, pub assigned_files: usize, pub subcategories: usize }
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExclusionImpact { pub files: usize, pub with_categories: usize, pub with_keywords: usize, pub with_rating_label_tagged: usize, pub with_color_overrides: usize }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct RemoveCatalogResult { pub change_id:Option<i64>,pub summary:String,pub removed_assets:usize,pub removed_folders:usize,pub asset_ids:Vec<i64> }

fn remove_asset_rows(tx: &Transaction<'_>, ids: &[i64], permanent: bool) -> Result<(), String> {
    if let Some(&max_id) = ids.iter().max() {
        remember_id(tx, "assets", max_id)?;
    }
    let tables = ["asset_categories", "asset_keywords", "asset_color_overrides", "ai_suggestion_rejections"];
    for table in tables.into_iter().take(if permanent { tables.len() } else { tables.len() - 1 }) {
        if table_exists(tx, table)? {
            for chunk in ids.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                tx.execute(&format!("DELETE FROM {table} WHERE asset_id IN ({marks})"), rusqlite::params_from_iter(chunk))
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        tx.execute(&format!("DELETE FROM assets WHERE id IN ({marks})"), rusqlite::params_from_iter(chunk))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Catalogue asset ids whose file lies in `folder` or below (whole path components, case-insensitive).
/// Matched in Rust: SQLite's lower() only folds ASCII (Ľudia ≠ ľudia) and LIKE would treat `_` as a wildcard.
fn assets_under(conn:&rusqlite::Connection,folder:&str)->Result<Vec<i64>,String>{
    let prefix=normalize_fs_path(folder)+"\\";
    let mut stmt=conn.prepare("SELECT id,path FROM assets WHERE path IS NOT NULL ORDER BY id").map_err(|e|e.to_string())?;
    let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    Ok(rows.into_iter().filter(|(_,path)|normalize_fs_path(path).starts_with(&prefix)).map(|(id,_)|id).collect())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAssetKeywordsResult { pub change_id: Option<i64>, pub summary: String, pub changed_count: usize, pub remapped_ids: Vec<AssetIdRemap>, pub keyword_id: Option<i64>, pub parent_id: Option<i64>, pub ancestor_ids: Vec<i64> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordMutationResult { pub change_id: Option<i64>, pub summary: String, pub keyword_id: Option<i64>, pub parent_id: Option<i64>, pub ancestor_ids: Vec<i64>, pub deleted_keyword_ids: Vec<i64> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordDeleteInfo { pub id: i64, pub name: String, pub assigned_files: usize, pub subkeywords: usize }

type ChangeHook = Arc<dyn Fn(&[&str]) + Send + Sync>;

pub struct CatalogWriter { db: Arc<Mutex<rusqlite::Connection>>, writable: bool, change_hook: Mutex<Option<ChangeHook>>, rules: crate::folder_rules::FolderRules, incoming: Mutex<String> }

impl CatalogWriter {
    pub fn new(db: Arc<Mutex<rusqlite::Connection>>, writable: bool) -> Self { Self { db, writable, change_hook: Mutex::new(None), rules: crate::folder_rules::FolderRules::default(), incoming: Mutex::new(String::new()) } }

    pub(crate) fn sync_incoming(&self, folder: &str) -> Result<(), String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let mut incoming = self.incoming.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        reconcile_incoming(&tx, folder, None)?;
        tx.commit().map_err(|e| e.to_string())?;
        *incoming = folder.to_owned();
        Ok(())
    }

    pub(crate) fn observe_incoming_folder(&self, folder: &str) -> Result<(), String> {
        if !self.writable { return Ok(()); }
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let incoming = self.incoming.lock().map_err(|e| e.to_string())?;
        if !crate::incoming::contains_folder(folder, &incoming) || !Path::new(folder).is_dir() {
            return Ok(());
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        reconcile_incoming(&tx, &incoming, Some(folder))?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn track_incoming(&self, tx: &Transaction<'_>, id: i64, path: &str) -> Result<(), String> {
        let folder = self.incoming.lock().map_err(|e| e.to_string())?;
        track_incoming(tx, id, path, &folder)
    }
    fn track_incoming_move(&self, tx: &Transaction<'_>, id: i64, from: Option<&str>, to: &str) -> Result<(), String> {
        let folder = self.incoming.lock().map_err(|e| e.to_string())?;
        track_incoming(tx, id, to, &folder)?;
        if !folder.is_empty() && from.is_some_and(|path| crate::incoming::is_inside(path, &folder))
            && !crate::incoming::is_inside(to, &folder)
        {
            tx.execute("UPDATE assets SET delete_suggestion=NULL WHERE id=?1 AND delete_suggestion IS NOT NULL", [id])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn set_change_hook(&self, hook: ChangeHook) {
        *self.change_hook.lock().unwrap() = Some(hook);
    }

    fn notify_labels_changed(&self, kinds: &[&str]) {
        let hook = self.change_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(kinds);
        }
    }

    pub fn folder_rules(&self) -> Result<Vec<crate::folder_rules::FolderRule>, String> { let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?; self.rules.list(&conn) }
    pub fn folder_rule_state(&self, path:&str) -> Result<crate::folder_rules::FolderRuleState, String> { let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?; self.rules.state(&conn,path) }

    pub fn folder_catalogue_states(&self, paths: Vec<String>) -> Result<std::collections::HashMap<String, crate::folder_rules::FolderCatalogueState>, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        self.rules.catalogue_states(&conn, paths)
    }
    /// Effective rule for a folder path: Some("include") / Some("exclude") / None (legacy, no rule).
    pub fn effective_folder_mode(&self, path:&str) -> Option<String> { self.folder_rule_state(path).ok().and_then(|s|s.effective) }

    /// Adds files found in an included folder in a single catalogue transaction.
    /// Files already catalogued (by path) are skipped. Returns listing id → new catalogue id.
    pub fn catalog_listed_assets(&self, listed:&[ListedAsset]) -> Result<Vec<AssetIdRemap>, String> {
        self.catalog_listed_assets_with_dates(listed, &Default::default())
    }

    pub(crate) fn is_writable(&self) -> bool { self.writable }

    pub(crate) fn catalog_listed_assets_with_dates(
        &self, listed: &[ListedAsset], dates: &std::collections::HashMap<i64, String>,
    ) -> Result<Vec<AssetIdRemap>, String> {
        self.require_write()?;
        if listed.is_empty() { return Ok(Vec::new()); }
        let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;
        let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let mut folder_paths=load_folder_paths(&tx)?;let mut folder_roots=load_folder_roots(&tx)?;
        let mut remaps=Vec::new();
        let has_edit_pending = crate::schema::has_asset_column(&tx, "edit_pending_since").map_err(|e| e.to_string())?;
        for candidate in listed {
            if candidate.ext.trim().is_empty() || find_asset_by_path(&tx,&candidate.path)?.is_some() { continue; }
            let folder_id=ensure_folder(&tx,&candidate.folder_path,&mut folder_paths,&mut folder_roots)?;
            let id=next_id(&tx,"assets")?;
            insert_asset(&tx,&NewCatalogAsset{id,folder_id,name:candidate.name.clone(),ext:candidate.ext.clone(),size:candidate.size,file_modified:candidate.file_modified.clone(),path:candidate.path.clone(),rating:None,label:None,tagged:false,exif_date:dates.get(&candidate.temporary_id).cloned()})?;
            self.track_incoming(&tx, id, &candidate.path)?;
            if has_edit_pending { fulfil_edit_with_sibling(&tx, folder_id, candidate)?; }
            remaps.push(AssetIdRemap{old_id:candidate.temporary_id,new_id:id});
        }
        if remaps.is_empty() { return Ok(remaps); }
        let summary=format!("Pridané do katalógu: {} {}",remaps.len(),slovak_count(remaps.len(),"súbor","súbory","súborov"));
        record_change(&tx, "auto_catalog", &summary)?;
        tx.commit().map_err(|e|e.to_string())?;
        Ok(remaps)
    }

    pub(crate) fn backfill_capture_dates(&self, dates: &std::collections::HashMap<i64, String>) -> Result<usize, String> {
        self.require_write()?;
        if dates.is_empty() { return Ok(0); }
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut count = 0;
        {
            let mut update = tx.prepare("UPDATE assets SET exif_date=?2
                WHERE id=?1 AND is_listed=1 AND (exif_date IS NULL OR exif_date='')").map_err(|e| e.to_string())?;
            for (id, date) in dates {
                count += update.execute((id, date)).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub(crate) fn fulfil_edits_from_listing(&self, assets: &[crate::db::Asset]) -> Result<usize, String> {
        if !self.writable { return Ok(0); }
        let mut modified = assets.iter().filter_map(|asset| {
            (asset.id > 0 && !asset.is_directory && !asset.offline && asset.edit_pending_since.is_some())
                .then_some(asset.file_modified.as_deref()).flatten().map(|mtime| (asset.id, mtime))
        }).peekable();
        if modified.peek().is_none() { return Ok(0); }
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut count = 0;
        {
            let mut update = tx.prepare("UPDATE assets SET edit_pending_since=NULL
                WHERE id=?1 AND is_listed=1 AND unixepoch(?2)>edit_pending_since+?3").map_err(|e| e.to_string())?;
            for (id, mtime) in modified {
                count += update.execute((id, mtime, crate::quarantine::EDIT_MTIME_GRACE)).map_err(|e| e.to_string())?;
            }
        }
        if count > 0 { record_change(&tx, "fulfil_pending_edits", &format!("Úpravy zistené v súboroch · {count}"))?; }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// How much catalogue data lives under `path` (shown before excluding the folder).
    pub fn exclusion_impact(&self, path:&str) -> Result<ExclusionImpact, String> {
        let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;
        let ids=assets_under(&conn,path)?;
        let mut impact=ExclusionImpact{files:ids.len(),..Default::default()};
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT coalesce(sum(EXISTS(SELECT 1 FROM asset_categories c WHERE c.asset_id=a.id)),0),
                        coalesce(sum(EXISTS(SELECT 1 FROM asset_keywords k WHERE k.asset_id=a.id)),0),
                        coalesce(sum(coalesce(a.rating,0)<>0 OR coalesce(a.label,'')<>'' OR coalesce(a.tagged,0)<>0),0),
                        coalesce(sum(EXISTS(SELECT 1 FROM asset_color_overrides c WHERE c.asset_id=a.id)),0)
                 FROM assets a WHERE a.id IN ({marks})"
            )).map_err(|e| e.to_string())?;
            let (categories, keywords, values, colors) = stmt.query_row(rusqlite::params_from_iter(chunk),
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))
                .map_err(|e| e.to_string())?;
            impact.with_categories += categories as usize;
            impact.with_keywords += keywords as usize;
            impact.with_rating_label_tagged += values as usize;
            impact.with_color_overrides += colors as usize;
        }
        Ok(impact)
    }

    pub fn remove_folder_from_catalog(&self,path:&str)->Result<RemoveCatalogResult,String>{
        self.require_write()?;let path=path.trim().trim_end_matches(['\\','/']).replace('/',"\\");if path.is_empty(){return Err("Cesta priečinka je prázdna".into())}
        let mut conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let normal=normalize_fs_path(&path);let prefix=format!("{normal}\\");let ids={let mut stmt=tx.prepare("SELECT id,path FROM assets WHERE path IS NOT NULL ORDER BY id").map_err(|e|e.to_string())?;stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?.into_iter().filter(|(_,p)|{let p=normalize_fs_path(p);p==normal||p.starts_with(&prefix)}).map(|(id,_)|id).collect::<Vec<_>>()};
        let folder_rows={let mut stmt=tx.prepare("SELECT id,path,root_id FROM folders WHERE path IS NOT NULL").map_err(|e|e.to_string())?;stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?};
        let folders=folder_rows.into_iter().filter(|(_,p,_)|normalize_fs_path(p)==normalize_fs_path(&path)||normalize_fs_path(p).starts_with(&prefix)).collect::<Vec<_>>();
        remove_asset_rows(&tx, &ids, false)?;
        remove_folder_rows(&tx, &folders)?;
        tx.commit().map_err(|e|e.to_string())?;if !ids.is_empty() { self.notify_labels_changed(&["category", "keyword"]); } Ok(RemoveCatalogResult{change_id:None,summary:format!("Odstránené z katalógu: {} súborov a {} priečinkov",ids.len(),folders.len()),removed_assets:ids.len(),removed_folders:folders.len(),asset_ids:ids})
    }

    /// Saves (or clears, `mode=None`) the rule for `path`. Excluding removes every catalogue row under the
    /// folder, including category, keyword and colour links. Returns the removed ids for the AI index.
    pub fn set_folder_rule(&self, path:&str, mode:Option<&str>) -> Result<Vec<i64>, String> {
        self.require_write()?;
        let path=path.trim().trim_end_matches(['\\','/']).replace('/',"\\");
        if path.is_empty() { return Err("Cesta priečinka je prázdna".into()); }
        if !matches!(mode,None|Some("include")|Some("exclude")) { return Err("Neplatné pravidlo priečinka".into()); }
        let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;
        let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS ddreams_folder_rules(path TEXT PRIMARY KEY COLLATE NOCASE,mode TEXT NOT NULL CHECK(mode IN ('include','exclude')),created TEXT NOT NULL);").map_err(|e|e.to_string())?;
        let mut removed=Vec::new();
        if mode==Some("exclude") {
            removed=assets_under(&tx,&path)?;
            remove_asset_rows(&tx, &removed, false)?;
            let summary=format!("Vylúčené z katalógu: {path} ({} {})",removed.len(),slovak_count(removed.len(),"súbor","súbory","súborov"));
            if !removed.is_empty() { record_change(&tx, "exclude_folder", &summary)?; }
        }
        match mode {
            Some(mode)=>{tx.execute("INSERT INTO ddreams_folder_rules(path,mode,created) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET mode=excluded.mode",(&path,mode,iso_now())).map_err(|e|e.to_string())?;}
            None=>{tx.execute("DELETE FROM ddreams_folder_rules WHERE path=?1",[&path]).map_err(|e|e.to_string())?;}
        }
        tx.commit().map_err(|e|e.to_string())?;
        self.rules.reload(&connection);
        if !removed.is_empty() { self.notify_labels_changed(&["category", "keyword"]); }
        Ok(removed)
    }

    pub fn set_asset_colors(&self, asset_ids: &[i64], color: &str, state: Option<bool>, summary: &str) -> Result<usize, String> {
        const PALETTE: [&str;13] = ["červená","oranžová","hnedá","žltá","zelená","tyrkysová","modrá","fialová","ružová","biela","sivá","čierna","čiernobiela"];
        self.require_write()?;
        if !PALETTE.contains(&color) { return Err("Neznáma farba".into()); }
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, &[])?;
        let state = state.map(i64::from);
        let mut count = 0;
        let mut seen = std::collections::HashSet::new();
        for id in asset_ids.iter().copied().filter(|id| *id > 0 && seen.insert(*id)) {
            let listed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1)", [id], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if !listed { continue; }
            let old: Option<i64> = tx.query_row("SELECT state FROM asset_color_overrides WHERE asset_id=?1 AND color=?2", (id, color), |r| r.get(0))
                .optional().map_err(|e| e.to_string())?;
            if old == state { continue; }
            if let Some(state) = state {
                tx.execute("INSERT OR REPLACE INTO asset_color_overrides(asset_id,color,state) VALUES(?1,?2,?3)", (id, color, state))
            } else {
                tx.execute("DELETE FROM asset_color_overrides WHERE asset_id=?1 AND color=?2", (id, color))
            }.map_err(|e| e.to_string())?;
            count += 1;
        }
        if count > 0 { record_change(&tx, "set_asset_colors", summary)?; }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub fn set_asset_values(
        &self,
        asset_ids: &[i64],
        listed_assets: &[ListedAsset],
        changes: &serde_json::Map<String, Value>,
    ) -> Result<SetAssetValuesResult, String> {
        self.require_write()?;
        let updates = parse_asset_updates(changes)?;
        if updates.is_empty() { return Err("No asset values were supplied".into()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let insert_missing = updates.edit_pending != Some(false) || updates.rating.is_some()
            || updates.label.is_some() || updates.tagged.is_some() || updates.tagged_toggle;
        let (mut targets, remapped_ids, inserted) = resolve_write_assets(&tx, asset_ids, listed_assets, insert_missing, true, |tx, id, path| self.track_incoming(tx, id, path))?;
        if targets.is_empty() { return Err("No files were selected".into()); }
        let toggle_tagged_to = if updates.tagged_toggle { Some(targets.iter().any(|asset| !asset.tagged)) } else { updates.tagged };
        let mut pending_to = updates.edit_pending;
        let mut pending_targets = HashMap::new();
        if updates.edit_pending_toggle || pending_to.is_some() {
            let mut stmt = tx.prepare("SELECT edit_pending_since FROM assets WHERE id=?1 AND quarantined_at IS NULL")
                .map_err(|e| e.to_string())?;
            for target in &targets {
                if let Some(since) = stmt.query_row([target.id], |r| r.get::<_, Option<i64>>(0))
                    .optional().map_err(|e| e.to_string())? {
                    pending_targets.insert(target.id, since);
                }
            }
            if pending_targets.is_empty() && updates.rating.is_none() && updates.label.is_none() && toggle_tagged_to.is_none() {
                return Err("Vyberte súbory mimo karantény".into());
            }
            if updates.edit_pending_toggle { pending_to = Some(pending_targets.values().any(Option::is_none)); }
        }
        let marked_at = pending_to.filter(|pending| *pending).map(|_| crate::quarantine::now());
        let mut changed = inserted > 0;
        for target in &mut targets {
            if let Some(rating) = updates.rating {
                changed |= update_column(&tx, "assets", target.id, "rating", rating)?;
                target.rating = rating;
            }
            if let Some(label) = &updates.label {
                changed |= update_column(&tx, "assets", target.id, "label", label.as_deref())?;
                target.label = label.clone();
            }
            if let Some(tagged) = toggle_tagged_to {
                changed |= update_column(&tx, "assets", target.id, "tagged", i64::from(tagged))?;
                target.tagged = tagged;
            }
            if let Some(&since) = pending_targets.get(&target.id) {
                let next = marked_at.map(|now| since.unwrap_or(now));
                changed |= update_column(&tx, "assets", target.id, "edit_pending_since", next)?;
                target.edit_pending_since = next;
            }
        }

        let mut action_parts = update_summary_parts(&updates, toggle_tagged_to);
        if let Some(pending) = pending_to { action_parts.push(if pending { "Na úpravu" } else { "Hotovo" }.into()); }
        let file_word = slovak_count(targets.len(), "súbor", "súbory", "súborov");
        let summary = format!("{} · {} {}", action_parts.join(" · "), targets.len(), file_word);
        let change_id = if changed { record_change(&tx, "set_asset_values", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(SetAssetValuesResult { change_id, summary, remapped_ids, assets: targets })
    }

    pub fn set_asset_categories(&self, asset_ids: &[i64], listed_assets: &[ListedAsset], category_id: i64, assign: bool) -> Result<SetAssetCategoriesResult, String> {
        self.require_write()?;
        if asset_ids.is_empty() { return Err("No files were selected".into()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let category_path = category_path(&tx, category_id)?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let (targets, remapped_ids, inserted) = resolve_write_assets(&tx, asset_ids, listed_assets, assign, false, |tx, id, path| self.track_incoming(tx, id, path))?;
        let changed = set_asset_links(&tx, "asset_categories", "category_id", &targets, category_id, assign)?;
        if changed < inserted { return Err("New catalog asset did not receive its category".into()); }
        let summary = if changed == 0 {
            format!("Žiadne zmeny v kategórii {category_path}")
        } else {
            let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
            format!("{} {category_path} · {changed} {file_word}", if assign { "Zaradené do" } else { "Vyradené z" })
        };
        let change_id = if changed > 0 { record_change(&tx, "set_asset_categories", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        if changed > 0 { self.notify_labels_changed(&["category"]); }
        Ok(SetAssetCategoriesResult { change_id, summary, changed_count: changed, remapped_ids })
    }

    pub fn create_category(&self, parent_id: Option<i64>, name: &str, icon_key: Option<&str>) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        let name=validated_category_name(name)?;
        validate_category_icon(icon_key)?;
        let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;
        let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let (category_id,summary,root_id)=if let Some(parent)=parent_id {
            let root_id:Option<i64>=tx.query_row("SELECT root_id FROM categories WHERE id=?1",[parent],|r|r.get(0)).optional().map_err(|e|e.to_string())?.ok_or_else(||"Nadradená kategória neexistuje".to_string())?;
            ensure_unique_category_name(&tx,Some(parent),root_id,&name,None,None)?;
            let id=next_id(&tx,"categories")?;
            tx.execute("INSERT INTO categories(id,parent_id,root_id,name,icon_key) VALUES(?1,?2,?3,?4,?5)",(id,parent,root_id,&name,icon_key)).map_err(|e|e.to_string())?;

            let parent_path=category_path(&tx,parent)?;
            (id,format!("Nová kategória {parent_path} › {name}"),root_id)
        } else {
            ensure_unique_category_name(&tx,None,None,&name,None,None)?;
            let root_id=next_id(&tx,"category_roots")?;let id=next_id(&tx,"categories")?;
            tx.execute("INSERT INTO category_roots(id,name,icon_key) VALUES(?1,?2,?3)",(root_id,&name,icon_key)).map_err(|e|e.to_string())?;
            tx.execute("INSERT INTO categories(id,parent_id,root_id,name,icon_key) VALUES(?1,NULL,?2,?3,?4)",(id,root_id,&name,icon_key)).map_err(|e|e.to_string())?;

            (id,format!("Nová kategória {name}"),Some(root_id))
        };
        let ancestor_ids=category_ancestor_ids(&tx,category_id)?;
        let change_id=record_change(&tx, "create_category", &summary)?;
        tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult{change_id,summary,category_id:Some(category_id),parent_id,root_id,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()})
    }

    pub fn set_category_icon(&self, id: i64, is_root: bool, icon_key: Option<&str>) -> Result<CategoryMutationResult, String> {
        self.require_write()?;
        validate_category_icon(icon_key)?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let table = if is_root { "category_roots" } else { "categories" };
        let name: String = tx.query_row(&format!("SELECT name FROM {table} WHERE id=?1"), [id], |row| row.get(0))
            .optional().map_err(|e| e.to_string())?.ok_or("Kategória neexistuje")?;
        let mut changed = update_column(&tx, table, id, "icon_key", icon_key)?;
        // Keep the merged category's icon when the category moves.
        if is_root {
            if let Some(paired) = crate::db::category_root_matches(&tx, id).map_err(|e| e.to_string())? {
                changed |= update_column(&tx, "categories", paired, "icon_key", icon_key)?;
            }
        }
        let summary = if !changed { format!("Ikona kategórie {name} je bez zmeny") }
            else { format!("Zmenená ikona kategórie {name}") };
        let change_id = if changed { record_change(&tx, "set_category_icon", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(CategoryMutationResult {
            change_id, summary, category_id: None, parent_id: None, root_id: None, ancestor_ids: Vec::new(),
            deleted_category_ids: Vec::new(), deleted_root_ids: Vec::new(),
        })
    }

    pub fn rename_category(&self,id:i64,name:&str)->Result<CategoryMutationResult,String>{
        self.require_write()?;let name=validated_category_name(name)?;
        let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let (old,parent,root_id):(String,Option<i64>,Option<i64>)=tx.query_row("SELECT name,parent_id,root_id FROM categories WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kategória neexistuje".to_string())?;
        let paired_root=if parent.is_none(){if let Some(root)=root_id{let root_name:Option<String>=tx.query_row("SELECT name FROM category_roots WHERE id=?1",[root],|r|r.get(0)).optional().map_err(|e|e.to_string())?;if root_name.is_some_and(|value|value.trim().to_lowercase()==old.trim().to_lowercase()){Some(root)}else{None}}else{None}}else{None};
        ensure_unique_category_name(&tx,parent,root_id,&name,Some(id),paired_root)?;
        if old==name {let path=category_path(&tx,id)?;let ancestor_ids=category_ancestor_ids(&tx,id)?;tx.commit().map_err(|e|e.to_string())?;let summary=format!("Kategória už má názov {path}");return Ok(CategoryMutationResult{change_id:None,summary,category_id:Some(id),parent_id:parent,root_id,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()});}
        update_column(&tx,"categories",id,"name",SqlValue::Text(name.clone()))?;
        if let Some(root)=paired_root{update_column(&tx,"category_roots",root,"name",SqlValue::Text(name.clone()))?;}
        let summary=format!("Premenované {old} → {name}");let ancestor_ids=category_ancestor_ids(&tx,id)?;let change_id=record_change(&tx, "rename_category", &summary)?;
        tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult{change_id,summary,category_id:Some(id),parent_id:parent,root_id,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()})
    }

    pub fn move_category(&self,id:i64,new_parent_id:Option<i64>)->Result<CategoryMutationResult,String>{
        self.require_write()?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let (name,old_parent,old_root):(String,Option<i64>,Option<i64>)=tx.query_row("SELECT name,parent_id,root_id FROM categories WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kategória neexistuje".to_string())?;
        let subtree=category_subtree(&tx,id)?;
        let new_root;
        if let Some(parent)=new_parent_id {
            if subtree.contains(&parent){return Err("Kategóriu nemožno presunúť do seba ani do jej podkategórie".into());}
            let target_root:Option<i64>=tx.query_row("SELECT root_id FROM categories WHERE id=?1",[parent],|r|r.get(0)).optional().map_err(|e|e.to_string())?.ok_or_else(||"Cieľová kategória neexistuje".to_string())?;
            ensure_unique_category_name(&tx,Some(parent),target_root,&name,Some(id),None)?;
            if old_parent==Some(parent){let ancestor_ids=category_ancestor_ids(&tx,id)?;tx.commit().map_err(|e|e.to_string())?;return Ok(CategoryMutationResult{change_id:None,summary:format!("Kategória {name} je už na tomto mieste"),category_id:Some(id),parent_id:old_parent,root_id:old_root,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()});}
            new_root=target_root;
            update_column(&tx,"categories",id,"parent_id",SqlValue::Integer(parent))?;
        }else{
            if old_parent.is_none(){let ancestor_ids=category_ancestor_ids(&tx,id)?;tx.commit().map_err(|e|e.to_string())?;return Ok(CategoryMutationResult{change_id:None,summary:format!("Kategória {name} je už na najvyššej úrovni"),category_id:Some(id),parent_id:None,root_id:old_root,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()});}
            ensure_unique_category_name(&tx,None,None,&name,Some(id),None)?;
            let root=next_id(&tx,"category_roots")?;tx.execute("INSERT INTO category_roots(id,name) VALUES(?1,?2)",(root,&name)).map_err(|e|e.to_string())?;new_root=Some(root);
            update_column(&tx,"categories",id,"parent_id",SqlValue::Null)?;
        }
        if old_root!=new_root{for descendant in subtree.iter().copied(){if descendant!=id{update_column(&tx,"categories",descendant,"root_id",new_root.map(SqlValue::Integer).unwrap_or(SqlValue::Null))?;}}update_column(&tx,"categories",id,"root_id",new_root.map(SqlValue::Integer).unwrap_or(SqlValue::Null))?;}
        if let Some(old_root)=old_root.filter(|root|Some(*root)!=new_root){let remaining:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM categories WHERE root_id=?1)",[old_root],|r|r.get(0)).map_err(|e|e.to_string())?;if !remaining{delete_row(&tx,"category_roots",old_root)?;}}
        let summary=if let Some(parent)=new_parent_id{format!("Presunuté {name} do {}",category_path(&tx,parent)?)}else{format!("Presunuté {name} na najvyššiu úroveň")};let ancestor_ids=category_ancestor_ids(&tx,id)?;let change_id=record_change(&tx, "move_category", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult{change_id,summary,category_id:Some(id),parent_id:new_parent_id,root_id:new_root,ancestor_ids,deleted_category_ids:Vec::new(),deleted_root_ids:Vec::new()})
    }

    pub fn category_delete_info(&self,id:i64)->Result<CategoryDeleteInfo,String>{let connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;category_delete_info(&connection,id)}

    pub fn delete_category(&self,id:i64)->Result<CategoryMutationResult,String>{
        self.require_write()?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let info=category_delete_info(&tx,id)?;if info.assigned_files>0{return Err(format!("Kategóriu {} nie je možné zmazať, obsahuje {} {} (vrátane podkategórií). Najprv z nej súbory odoberte.",info.name,info.assigned_files,slovak_count(info.assigned_files as usize,"súbor","súbory","súborov")));}
        let root_id:Option<i64>=tx.query_row("SELECT root_id FROM categories WHERE id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        let ids=category_subtree(&tx,id)?;
        for category_id in ids.iter().rev().copied(){delete_row(&tx,"categories",category_id)?;}
        let mut deleted_root_ids=Vec::new();
        if let Some(root_id)=root_id{
            let any_left:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM categories WHERE root_id=?1)",[root_id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if !any_left{delete_row(&tx,"category_roots",root_id)?;deleted_root_ids.push(root_id);}
        }
        let summary=format!("Zmazaná kategória {}",info.name);let change_id=record_change(&tx, "delete_category", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["category"]);
        Ok(CategoryMutationResult{change_id,summary,category_id:None,parent_id:None,root_id,ancestor_ids:Vec::new(),deleted_category_ids:ids,deleted_root_ids})
    }

    pub fn set_asset_keywords(&self,asset_ids:&[i64],listed_assets:&[ListedAsset],keyword_id:i64,assign:bool)->Result<SetAssetKeywordsResult,String>{
        self.require_write()?;
        if asset_ids.is_empty() { return Err("Nie sú vybrané žiadne súbory".into()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let (name, parent_id): (String, Option<i64>) = tx.query_row("SELECT name,parent_id FROM keywords WHERE id=?1", [keyword_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional().map_err(|e| e.to_string())?.ok_or("Kľúčové slovo neexistuje")?;
        let (targets, remapped_ids, inserted) = resolve_write_assets(&tx, asset_ids, listed_assets, assign, false, |tx, id, path| self.track_incoming(tx, id, path))?;
        let changed = set_asset_links(&tx, "asset_keywords", "keyword_id", &targets, keyword_id, assign)?;
        if changed < inserted { return Err("Nový súbor sa nepodarilo priradiť ku kľúčovému slovu".into()); }
        let summary = if changed == 0 {
            format!("Žiadne zmeny pri kľúčovom slove {name}")
        } else {
            let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
            format!("{} kľúčové slovo {name} · {changed} {file_word}", if assign { "Pridané" } else { "Odobrané" })
        };
        let ancestor_ids = keyword_ancestor_ids(&tx, keyword_id)?;
        let change_id = if changed > 0 { record_change(&tx, "set_asset_keywords", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        if changed > 0 { self.notify_labels_changed(&["keyword"]); }
        Ok(SetAssetKeywordsResult { change_id, summary, changed_count: changed, remapped_ids, keyword_id: Some(keyword_id), parent_id, ancestor_ids })
    }

    pub fn create_keyword(&self,parent_id:Option<i64>,name:&str)->Result<KeywordMutationResult,String>{
        self.require_write()?;let name=validated_keyword_name(name)?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        if let Some(parent)=parent_id{if !keyword_exists(&tx,parent)?{return Err("Nadradené kľúčové slovo neexistuje".into());}}
        ensure_unique_keyword_name(&tx,parent_id,&name,None)?;let id=next_id(&tx,"keywords")?;tx.execute("INSERT INTO keywords(id,parent_id,name) VALUES(?1,?2,?3)",(id,parent_id,&name)).map_err(|e|e.to_string())?;
        let summary=if let Some(parent)=parent_id{format!("Nové kľúčové slovo {} › {name}",keyword_path(&tx,parent)?)}else{format!("Nové kľúčové slovo {name}")};let ancestors=keyword_ancestor_ids(&tx,id)?;let change_id=record_change(&tx, "create_keyword", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["keyword"]);Ok(KeywordMutationResult{change_id,summary,keyword_id:Some(id),parent_id,ancestor_ids:ancestors,deleted_keyword_ids:Vec::new()})
    }

    pub fn create_and_assign_keyword(&self,asset_ids:&[i64],listed_assets:&[ListedAsset],parent_id:Option<i64>,name:&str)->Result<SetAssetKeywordsResult,String>{
        self.require_write()?;
        let name = validated_keyword_name(name)?;
        if asset_ids.is_empty() { return Err("Nie sú vybrané žiadne súbory".into()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        if let Some(parent) = parent_id {
            if !keyword_exists(&tx, parent)? { return Err("Nadradené kľúčové slovo neexistuje".into()); }
        }
        ensure_unique_keyword_name(&tx, parent_id, &name, None)?;
        self.rules.guard(&tx, asset_ids, listed_assets)?;
        let keyword_id = next_id(&tx, "keywords")?;
        tx.execute("INSERT INTO keywords(id,parent_id,name) VALUES(?1,?2,?3)", (keyword_id, parent_id, &name)).map_err(|e| e.to_string())?;
        let (targets, remapped_ids, _) = resolve_write_assets(&tx, asset_ids, listed_assets, true, false, |tx, id, path| self.track_incoming(tx, id, path))?;
        let changed = set_asset_links(&tx, "asset_keywords", "keyword_id", &targets, keyword_id, true)?;
        if changed == 0 { return Err("Nie je možné vytvoriť kľúčové slovo bez platných súborov".into()); }
        let ancestor_ids = keyword_ancestor_ids(&tx, keyword_id)?;
        let path = keyword_path(&tx, keyword_id)?;
        let file_word = slovak_count(changed, "súbor", "súbory", "súborov");
        let summary = format!("Nové kľúčové slovo {path} a priradené · {changed} {file_word}");
        let change_id = record_change(&tx, "create_assign_keyword", &summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["keyword"]);
        Ok(SetAssetKeywordsResult { change_id, summary, changed_count: changed, remapped_ids, keyword_id: Some(keyword_id), parent_id, ancestor_ids })
    }

    pub fn rename_keyword(&self,id:i64,name:&str)->Result<KeywordMutationResult,String>{self.require_write()?;let name=validated_keyword_name(name)?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;let(old,parent):(String,Option<i64>)=tx.query_row("SELECT name,parent_id FROM keywords WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kľúčové slovo neexistuje".to_string())?;ensure_unique_keyword_name(&tx,parent,&name,Some(id))?;let ancestors=keyword_ancestor_ids(&tx,id)?;if old==name{tx.commit().map_err(|e|e.to_string())?;return Ok(KeywordMutationResult{change_id:None,summary:format!("Kľúčové slovo už má názov {old}"),keyword_id:Some(id),parent_id:parent,ancestor_ids:ancestors,deleted_keyword_ids:Vec::new()});}update_column(&tx,"keywords",id,"name",SqlValue::Text(name.clone()))?;let summary=format!("Premenované {old} → {name}");let change_id=record_change(&tx, "rename_keyword", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["keyword"]);Ok(KeywordMutationResult{change_id,summary,keyword_id:Some(id),parent_id:parent,ancestor_ids:ancestors,deleted_keyword_ids:Vec::new()})}

    pub fn move_keyword(&self,id:i64,new_parent_id:Option<i64>)->Result<KeywordMutationResult,String>{self.require_write()?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;let(name,old_parent):(String,Option<i64>)=tx.query_row("SELECT name,parent_id FROM keywords WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kľúčové slovo neexistuje".to_string())?;let subtree=keyword_subtree(&tx,id)?;if new_parent_id.is_some_and(|parent|subtree.contains(&parent)){return Err("Kľúčové slovo nemožno presunúť do seba ani do jeho podriadeného slova".into());}if let Some(parent)=new_parent_id{if !keyword_exists(&tx,parent)?{return Err("Cieľové kľúčové slovo neexistuje".into());}}ensure_unique_keyword_name(&tx,new_parent_id,&name,Some(id))?;if old_parent==new_parent_id{let ancestors=keyword_ancestor_ids(&tx,id)?;tx.commit().map_err(|e|e.to_string())?;return Ok(KeywordMutationResult{change_id:None,summary:format!("Kľúčové slovo {name} je už na tomto mieste"),keyword_id:Some(id),parent_id:old_parent,ancestor_ids:ancestors,deleted_keyword_ids:Vec::new()});}update_column(&tx,"keywords",id,"parent_id",new_parent_id.map(SqlValue::Integer).unwrap_or(SqlValue::Null))?;let summary=if let Some(parent)=new_parent_id{format!("Presunuté {name} do {}",keyword_path(&tx,parent)?)}else{format!("Presunuté {name} na najvyššiu úroveň")};let ancestors=keyword_ancestor_ids(&tx,id)?;let change_id=record_change(&tx, "move_keyword", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["keyword"]);Ok(KeywordMutationResult{change_id,summary,keyword_id:Some(id),parent_id:new_parent_id,ancestor_ids:ancestors,deleted_keyword_ids:Vec::new()})}

    pub fn keyword_delete_info(&self,id:i64)->Result<KeywordDeleteInfo,String>{let conn=self.db.lock().map_err(|_|"catalog lock poisoned")?;keyword_delete_info(&conn,id)}

    pub fn delete_keyword(&self,id:i64)->Result<KeywordMutationResult,String>{self.require_write()?;let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;let info=keyword_delete_info(&tx,id)?;if info.assigned_files>0{return Err(format!("Kľúčové slovo {} nie je možné zmazať, obsahuje {} {} (vrátane podriadených slov). Najprv z neho súbory odoberte.",info.name,info.assigned_files,slovak_count(info.assigned_files,"súbor","súbory","súborov")));}let ancestors=keyword_ancestor_ids(&tx,id)?;let ids=keyword_subtree(&tx,id)?;for keyword_id in ids.iter().rev().copied(){delete_row(&tx,"keywords",keyword_id)?;}let summary=format!("Zmazané kľúčové slovo {}",info.name);let change_id=record_change(&tx, "delete_keyword", &summary)?;tx.commit().map_err(|e|e.to_string())?; self.notify_labels_changed(&["keyword"]);Ok(KeywordMutationResult{change_id,summary,keyword_id:None,parent_id:None,ancestor_ids:ancestors,deleted_keyword_ids:ids})}

    pub fn record_file_change(&self, action:&str, summary:&str, moved:&[FileCatalogMove], copied:&[FileCatalogCopy], created_folders:&[String])->Result<(Option<i64>,Vec<AssetIdRemap>),String>{
        self.record_file_transfer(action, summary, moved, copied, created_folders, None)
    }

    pub(crate) fn record_quarantine_move(&self, summary: &str, moved: &FileCatalogMove, source: &str, entering: bool) -> Result<Option<i64>, String> {
        let action = if entering { "move_to_quarantine" } else { "restore_from_quarantine" };
        self.record_file_transfer(action, summary, std::slice::from_ref(moved), &[], &[], Some((source, entering))).map(|r| r.0)
    }

    fn record_file_transfer(&self, action: &str, summary: &str, moved: &[FileCatalogMove], copied: &[FileCatalogCopy], created_folders: &[String], quarantine: Option<(&str, bool)>) -> Result<(Option<i64>, Vec<AssetIdRemap>), String> {
        self.require_write()?;
        let mut connection=self.db.lock().map_err(|_|"catalog lock poisoned")?;
        let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let mut paths=load_folder_paths(&tx)?;let mut roots=load_folder_roots(&tx)?;let mut remapped=Vec::new();
        let regular_move = action == "move_files"
            && crate::schema::has_asset_column(&tx, "quarantined_at").map_err(|e| e.to_string())?;
        for item in moved {
            if let Some((source, _)) = quarantine {
                let expected = (item.asset_id > 0).then_some(item.asset_id);
                if find_asset_by_path(&tx, source)? != expected {
                    return Err("Zdrojová cesta v katalógu sa zmenila".into());
                }
                if find_asset_by_path(&tx, &item.target_path)?.is_some() {
                    return Err(format!("Cieľová cesta už existuje v katalógu: {}", item.target_path));
                }
            }
            let folder=ensure_folder(&tx,&item.target_folder_path,&mut paths,&mut roots)?;
            let mut previous_path = None;
            let id = if item.asset_id < 0 && matches!(quarantine, Some((_, true))) {
                let id = next_id(&tx, "assets")?;
                insert_asset(&tx, &NewCatalogAsset {
                    id, folder_id: folder, name: item.name.clone(), ext: item.ext.clone(), size: Some(item.size),
                    file_modified: item.file_modified.clone(), path: item.target_path.clone(),
                    rating: None, label: None, tagged: false, exif_date: None,
                })?;
                id
            } else {
                previous_path = tx.query_row("SELECT path FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>''", [item.asset_id], |r| r.get::<_, Option<String>>(0))
                    .optional().map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Súbor katalógu {} sa nenašiel", item.asset_id))?;
                if regular_move {
                    tx.execute("UPDATE assets SET quarantined_at=NULL,quarantine_origin=NULL WHERE id=?1 AND quarantined_at IS NOT NULL",
                        [item.asset_id]).map_err(|e| e.to_string())?;
                }
                for (column,value) in [("folder_id",SqlValue::Integer(folder)),("path",SqlValue::Text(item.target_path.clone())),("name",SqlValue::Text(item.name.clone())),("ext",SqlValue::Text(item.ext.clone())),("ext_lower",SqlValue::Text(item.ext.to_ascii_lowercase())),("size",SqlValue::Real(item.size)),("file_modified",item.file_modified.clone().map(SqlValue::Text).unwrap_or(SqlValue::Null))] {
                    update_column(&tx, "assets", item.asset_id,column,value)?;
                }
                item.asset_id
            };
            if let Some((source, entering)) = quarantine {
                self.track_incoming(&tx, id, &item.target_path)?;
                tx.execute("UPDATE assets SET quarantined_at=?2,quarantine_origin=?3,delete_suggestion=0,edit_pending_since=NULL WHERE id=?1",
                    (id, entering.then(crate::quarantine::now), entering.then_some(source))).map_err(|e| e.to_string())?;
            } else {
                self.track_incoming_move(&tx, id, previous_path.as_deref(), &item.target_path)?;
            }
        }
        for item in copied {
            let folder=ensure_folder(&tx,&item.target_folder_path,&mut paths,&mut roots)?;
            let source_exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND is_listed=1 AND ext_lower<>'')",[item.source_asset_id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if !source_exists { continue; }
            let id=next_id(&tx,"assets")?;
            tx.execute("INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,size,width,height,bpp,caption,notes,author,rating,tagged,label,file_modified,exif_date,acdsee_date,md_dirty,is_orphan,path) SELECT ?1,?2,?3,?4,lower(?4),is_listed,?5,width,height,bpp,caption,notes,author,rating,tagged,label,?6,exif_date,acdsee_date,md_dirty,is_orphan,?7 FROM assets WHERE id=?8",(id,folder,&item.name,&item.ext,item.size,&item.file_modified,&item.target_path,item.source_asset_id)).map_err(|e|e.to_string())?;
            self.track_incoming(&tx, id, &item.target_path)?;
            remapped.push(AssetIdRemap{old_id:item.source_asset_id,new_id:id});
            for (table, column) in [("asset_categories", "category_id"), ("asset_keywords", "keyword_id")] {
                tx.execute(&format!("INSERT OR IGNORE INTO {table}(asset_id,{column}) SELECT ?1,{column} FROM {table} WHERE asset_id=?2"),
                    (id, item.source_asset_id)).map_err(|e| e.to_string())?;
            }
        }
        for folder_path in created_folders { ensure_folder(&tx, folder_path, &mut paths, &mut roots)?; }
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !remapped.is_empty() { self.notify_labels_changed(&["category", "keyword"]); }
        Ok((change_id, remapped))
    }

    pub fn rewrite_folder_paths(&self, old: &str, new: &str, action: &str, summary: &str) -> Result<Option<i64>, String> {
        self.require_write()?;
        let old = normalize_fs_path(old);
        let new = new.trim_end_matches(['\\', '/']).replace('/', "\\");
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        for table in ["assets", "folders", "folder_roots"] {
            let mut stmt = tx.prepare(&format!("SELECT id,path FROM {table} WHERE path IS NOT NULL")).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
            for (id, path) in rows {
                if let Some(suffix) = matched_path_suffix(&path, &old) {
                    let target = format!("{new}{}", suffix.replace('/', "\\"));
                    update_column(&tx, table, id, "path", &target)?;
                    if table == "assets" { self.track_incoming_move(&tx, id, Some(&path), &target)?; }
                    if table == "folders" && suffix.is_empty() {
                        update_column(&tx, table, id, "name", new.rsplit('\\').next().unwrap_or(&new))?;
                    }
                }
            }
        }
        if table_exists(&tx, "ddreams_folder_rules")? {
            let mut stmt = tx.prepare("SELECT path FROM ddreams_folder_rules").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
            for path in rows {
                if let Some(suffix) = matched_path_suffix(&path, &old) {
                    let target = format!("{new}{}", suffix.replace('/', "\\"));
                    tx.execute("UPDATE ddreams_folder_rules SET path=?1 WHERE path=?2", (&target, &path)).map_err(|e| e.to_string())?;
                }
            }
        }
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.rules.reload(&conn);
        Ok(change_id)
    }

    pub fn record_folder_delete(&self, path: &str, summary: &str) -> Result<Option<i64>, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let ids = assets_under(&tx, path)?;
        let old = normalize_fs_path(path);
        let folders = {
            let mut stmt = tx.prepare("SELECT id,path,root_id FROM folders WHERE path IS NOT NULL").map_err(|e| e.to_string())?;
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))
                .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
                .into_iter().filter(|(_, path, _)| matched_path_suffix(path, &old).is_some()).collect::<Vec<_>>()
        };
        remove_asset_rows(&tx, &ids, true)?;
        remove_folder_rows(&tx, &folders)?;
        let change_id = record_change(&tx, "delete_folder", summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !ids.is_empty() { self.notify_labels_changed(&["category", "keyword"]); }
        Ok(change_id)
    }

    pub fn purge_assets(&self, asset_ids: &[i64]) -> Result<(), String> {
        self.require_write()?;
        if asset_ids.is_empty() { return Ok(()); }
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        remove_asset_rows(&tx, asset_ids, true)?;
        mark_catalog_modified(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.notify_labels_changed(&["category", "keyword"]);
        Ok(())
    }

    pub fn record_file_delete(&self, asset_ids: &[i64], summary: &str) -> Result<Option<i64>, String> {
        self.record_named_file_delete(asset_ids, "delete_files", summary)
    }

    pub(crate) fn record_named_file_delete(&self, asset_ids: &[i64], action: &str, summary: &str) -> Result<Option<i64>, String> {
        self.require_write()?;
        let mut connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        remove_asset_rows(&tx, asset_ids, true)?;
        let change_id = record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())?;
        if !asset_ids.is_empty() { self.notify_labels_changed(&["category", "keyword"]); }
        Ok(change_id)
    }

    pub(crate) fn record_file_intent(&self, action: &str, summary: &str) -> Result<(), String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        record_change(&tx, action, summary)?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn suggest_deletions(&self, incoming: &str, suggestions: &[(i64, bool)], formats: &crate::formats::FormatTable) -> Result<DeletionSuggestionChange, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut changed = 0;
        {
            let mut select = tx.prepare("SELECT path,ext_lower,delete_suggestion FROM assets WHERE id=?1 AND is_listed=1")
                .map_err(|e| e.to_string())?;
            for &(id, weak) in suggestions {
                let row = select.query_row([id], |row| Ok((row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?, row.get::<_, Option<i64>>(2)?)))
                    .optional().map_err(|e| e.to_string())?;
                let Some((Some(path), ext, current)) = row else { continue; };
                if !crate::incoming::is_inside(&path, incoming) || !matches!(formats.group(&ext), Some("image" | "raw")) {
                    continue;
                }
                // NULL: eligible, 0: explicitly kept, 1: suggested. A new best is eligible again.
                let new_value = match (weak, current) {
                    (true, None) => SqlValue::Integer(1),
                    (false, Some(1)) => SqlValue::Null,
                    _ => continue,
                };
                changed += usize::from(update_column(&tx, "assets", id, "delete_suggestion", new_value)?);
            }
        }
        let summary = if changed == 0 { "Žiadne nové návrhy na vymazanie".into() }
            else { format!("Zmenené návrhy na vymazanie ({changed})") };
        let change_id = if changed > 0 { record_change(&tx, "suggest_deletions", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(DeletionSuggestionChange { change_id, summary })
    }

    pub fn keep_suggested_assets(&self, selection: Option<&[i64]>) -> Result<DeletionSuggestionChange, String> {
        self.require_write()?;
        let mut conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let selection = selection.map(|ids| ids.iter().copied().collect::<std::collections::HashSet<_>>());
        let active = {
            let mut select = tx.prepare("SELECT id FROM assets WHERE delete_suggestion=1 AND is_listed=1 ORDER BY id")
                .map_err(|e| e.to_string())?;
            select.query_map([], |row| row.get::<_, i64>(0)).map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
        };
        let mut changed = 0;
        for id in active.into_iter().filter(|id| selection.as_ref().is_none_or(|ids| ids.contains(id))) {
            changed += usize::from(update_column(&tx, "assets", id, "delete_suggestion", 0)?);
        }
        let summary = if changed == 0 { "Žiadne návrhy na vymazanie".into() }
            else { format!("Zrušené návrhy na vymazanie ({changed})") };
        let change_id = if changed > 0 { record_change(&tx, "keep_suggested_assets", &summary)? } else { None };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(DeletionSuggestionChange { change_id, summary })
    }

    pub fn recent_changes(&self, limit: i64) -> Result<Vec<ChangeSummary>, String> {
        let connection = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let mut statement = connection.prepare("SELECT id,ts,summary FROM ddreams_changes WHERE undone=0 ORDER BY id DESC LIMIT ?1").map_err(|e| e.to_string())?;
        statement.query_map([limit.clamp(1, 200)], |row| Ok(ChangeSummary { id: row.get(0)?, ts: row.get(1)?, summary: row.get(2)? }))
            .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
    }

    fn require_write(&self) -> Result<(), String> {
        if self.writable { Ok(()) } else { Err("Catalog is in strict read-only mode".into()) }
    }
}

#[derive(Default)]
struct AssetUpdates {
    rating: Option<Option<i64>>,
    label: Option<Option<String>>,
    tagged: Option<bool>,
    tagged_toggle: bool,
    edit_pending: Option<bool>,
    edit_pending_toggle: bool,
}

impl AssetUpdates {
    fn is_empty(&self) -> bool { self.rating.is_none() && self.label.is_none() && self.tagged.is_none() && !self.tagged_toggle && self.edit_pending.is_none() && !self.edit_pending_toggle }
}

struct NewCatalogAsset {
    id: i64,
    folder_id: i64,
    name: String,
    ext: String,
    size: Option<f64>,
    file_modified: Option<String>,
    exif_date: Option<String>,
    path: String,
    rating: Option<i64>,
    label: Option<String>,
    tagged: bool,
}

fn parse_asset_updates(values: &serde_json::Map<String, Value>) -> Result<AssetUpdates, String> {
    let mut result = AssetUpdates::default();
    for (key, value) in values {
        match key.as_str() {
            "rating" => {
                result.rating = Some(if value.is_null() { None } else {
                    let rating = value.as_i64().ok_or("Rating must be 1 to 5 or null")?;
                    if !(1..=5).contains(&rating) { return Err("Rating must be 1 to 5 or null".into()); }
                    Some(rating)
                });
            }
            "label" => {
                result.label = Some(if value.is_null() { None } else {
                    let label = value.as_str().ok_or("Invalid colour label")?;
                    if !["Red", "Yellow", "Green", "Blue", "Purple", "Gray"].contains(&label) { return Err("Invalid colour label".into()); }
                    Some(label.to_owned())
                });
            }
            "tagged" if value.as_str() == Some("toggle") => result.tagged_toggle = true,
            "tagged" => result.tagged = Some(value.as_bool().ok_or("Tagged state must be true, false, or 'toggle'")?),
            "editPending" if value.as_str() == Some("toggle") => result.edit_pending_toggle = true,
            "editPending" => result.edit_pending = Some(value.as_bool().ok_or("Neplatný stav úpravy")?),
            _ => return Err(format!("Unsupported asset value: {key}")),
        }
    }
    if result.tagged_toggle && result.tagged.is_some() { return Err("Tagged toggle cannot be combined with a fixed tagged value".into()); }
    Ok(result)
}

fn update_summary_parts(updates: &AssetUpdates, tagged: Option<bool>) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(rating) = updates.rating { parts.push(rating.map(|value| format!("Hodnotenie {value} ★")).unwrap_or_else(|| "Bez hodnotenia".into())); }
    if let Some(label) = &updates.label { parts.push(label.as_ref().map(|value| format!("Farebný štítok {}", label_name(value))).unwrap_or_else(|| "Bez štítku".into())); }
    if let Some(tagged) = tagged { parts.push(if tagged { "Označené" } else { "Zrušené označenie" }.into()); }
    parts
}

fn label_name(value: &str) -> &str {
    match value { "Red" => "Červená", "Yellow" => "Žltá", "Green" => "Zelená", "Blue" => "Modrá", "Purple" => "Fialová", "Gray" => "Na vyradenie", _ => value }
}

fn slovak_count(count: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else if (2..=4).contains(&count) { few } else { many }
}

fn read_asset_values(transaction: &Transaction<'_>, id: i64) -> Result<Option<AssetValueState>, String> {
    transaction.query_row("SELECT id,rating,label,coalesce(tagged,0),ext_lower,is_listed FROM assets WHERE id=?1", [id], |row| {
        let id = row.get(0)?; let rating = row.get(1)?; let label = row.get(2)?; let tagged = row.get::<_, i64>(3)? != 0;
        let ext: String = row.get(4)?; let listed: i64 = row.get(5)?;
        Ok((ext, listed, AssetValueState { id, rating, label, tagged, edit_pending_since: None }))
    }).optional().map_err(|e| e.to_string()).map(|row| row.filter(|(ext,listed,_)| !ext.is_empty() && *listed != 0).map(|(_,_,state)| state))
}

fn find_asset_by_path(transaction: &Transaction<'_>, path: &str) -> Result<Option<i64>, String> {
    transaction.query_row("SELECT id FROM assets WHERE lower(rtrim(path,'/\\'))=lower(rtrim(?1,'/\\')) ORDER BY id LIMIT 1", [path], |row| row.get(0)).optional().map_err(|e| e.to_string())
}

fn category_path(transaction: &Transaction<'_>, category_id: i64) -> Result<String, String> {
    let (root_id, root_name): (Option<i64>, Option<String>) = transaction.query_row(
        "SELECT c.root_id, r.name FROM categories c LEFT JOIN category_roots r ON r.id=c.root_id WHERE c.id=?1",
        [category_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|e| e.to_string())?.ok_or_else(|| format!("Category {category_id} not found"))?;
    let mut names = Vec::new();
    let mut current = Some(category_id);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) { break; }
        let row: Option<(String, Option<i64>)> = transaction.query_row("SELECT name,parent_id FROM categories WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|e| e.to_string())?;
        let Some((name, parent)) = row else { break; };
        names.push(name);
        current = parent;
    }
    names.reverse();
    if let Some(root) = root_name {
        if !names.first().is_some_and(|name| name.trim().eq_ignore_ascii_case(root.trim())) { names.insert(0, root); }
    } else if root_id.is_some() && names.is_empty() { return Err("Category path is unavailable".into()); }
    Ok(names.join(" › "))
}

fn validate_category_icon(icon_key: Option<&str>) -> Result<(), String> {
    match icon_key {
        None | Some("album" | "people" | "places" | "various" | "event" | "travel" | "nature"
            | "animal" | "home" | "building" | "car" | "food" | "sport" | "music" | "work"
            | "school" | "star" | "heart" | "camera" | "folder") => Ok(()),
        Some(_) => Err("Neznáma ikona kategórie".into()),
    }
}

fn validated_category_name(value:&str)->Result<String,String>{let name=value.trim();if name.is_empty(){return Err("Názov kategórie nesmie byť prázdny".into());}if name.chars().count()>100{return Err("Názov kategórie môže mať najviac 100 znakov".into());}Ok(name.to_owned())}

fn ensure_unique_category_name(tx:&Transaction<'_>,parent:Option<i64>,_root:Option<i64>,name:&str,exclude_category:Option<i64>,exclude_root:Option<i64>)->Result<(),String>{
    let mut existing=Vec::<(i64,String)>::new();
    if let Some(parent)=parent{let mut stmt=tx.prepare("SELECT id,name FROM categories WHERE parent_id=?1").map_err(|e|e.to_string())?;existing.extend(stmt.query_map([parent],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?);}
    else{let mut stmt=tx.prepare("SELECT id,name FROM categories WHERE parent_id IS NULL OR parent_id=0").map_err(|e|e.to_string())?;existing.extend(stmt.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?);}
    if existing.iter().any(|(id,other)|Some(*id)!=exclude_category&&other.trim().to_lowercase()==name.trim().to_lowercase()){return Err("V tejto úrovni už kategória s týmto názvom existuje".into());}
    if parent.is_none(){let mut stmt=tx.prepare("SELECT id,name FROM category_roots").map_err(|e|e.to_string())?;let roots=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;if roots.iter().any(|(id,other)|Some(*id)!=exclude_root&&other.trim().to_lowercase()==name.trim().to_lowercase()){return Err("Koreň kategórie s týmto názvom už existuje".into());}}
    Ok(())
}

fn category_subtree(tx:&Transaction<'_>,id:i64)->Result<Vec<i64>,String>{let mut stmt=tx.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM categories WHERE id=?1 UNION SELECT c.id FROM categories c JOIN subtree s ON c.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;let ids=stmt.query_map([id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;if ids.is_empty(){return Err("Kategória neexistuje".into());}Ok(ids)}

fn category_ancestor_ids(tx:&Transaction<'_>,id:i64)->Result<Vec<i64>,String>{let mut ids=Vec::new();let mut seen=std::collections::HashSet::new();let mut current=tx.query_row("SELECT parent_id FROM categories WHERE id=?1",[id],|r|r.get::<_,Option<i64>>(0)).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kategória neexistuje".to_string())?;while let Some(parent)=current{if !seen.insert(parent){break;}ids.push(parent);current=tx.query_row("SELECT parent_id FROM categories WHERE id=?1",[parent],|r|r.get::<_,Option<i64>>(0)).optional().map_err(|e|e.to_string())?.flatten();}Ok(ids)}

fn category_delete_info(connection:&rusqlite::Connection,id:i64)->Result<CategoryDeleteInfo,String>{let(name,):(String,)=connection.query_row("SELECT name FROM categories WHERE id=?1",[id],|r|Ok((r.get(0)?,))).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kategória neexistuje".to_string())?;let mut stmt=connection.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM categories WHERE id=?1 UNION SELECT c.id FROM categories c JOIN subtree s ON c.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;let ids=stmt.query_map([id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let marks=std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT count(DISTINCT asset_id) FROM asset_categories WHERE category_id IN ({marks})");let assigned_files=connection.query_row(&sql,rusqlite::params_from_iter(ids.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? as usize;Ok(CategoryDeleteInfo{id,name,assigned_files,subcategories:ids.len().saturating_sub(1)})}

fn validated_keyword_name(value:&str)->Result<String,String>{let name=value.trim();if name.is_empty(){return Err("Názov kľúčového slova nesmie byť prázdny".into());}if name.chars().count()>100{return Err("Názov kľúčového slova môže mať najviac 100 znakov".into());}Ok(name.to_owned())}
fn keyword_exists(tx:&Transaction<'_>,id:i64)->Result<bool,String>{tx.query_row("SELECT EXISTS(SELECT 1 FROM keywords WHERE id=?1)",[id],|r|r.get(0)).map_err(|e|e.to_string())}
fn ensure_unique_keyword_name(tx:&Transaction<'_>,parent:Option<i64>,name:&str,exclude:Option<i64>)->Result<(),String>{let mut stmt=tx.prepare("SELECT id,name FROM keywords WHERE parent_id IS ?1").map_err(|e|e.to_string())?;let rows=stmt.query_map([parent],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let normalized=name.trim().to_lowercase();if rows.iter().any(|(id,value)|Some(*id)!=exclude&&value.trim().to_lowercase()==normalized){return Err("Na tejto úrovni už existuje kľúčové slovo s týmto názvom".into());}Ok(())}
fn keyword_subtree(tx:&Transaction<'_>,id:i64)->Result<Vec<i64>,String>{let mut stmt=tx.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM keywords WHERE id=?1 UNION SELECT k.id FROM keywords k JOIN subtree s ON k.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;let ids=stmt.query_map([id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;if ids.is_empty(){return Err("Kľúčové slovo neexistuje".into());}Ok(ids)}
fn keyword_ancestor_ids(tx:&Transaction<'_>,id:i64)->Result<Vec<i64>,String>{let mut current=tx.query_row("SELECT parent_id FROM keywords WHERE id=?1",[id],|r|r.get::<_,Option<i64>>(0)).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kľúčové slovo neexistuje".to_string())?;let mut result=Vec::new();let mut seen=std::collections::HashSet::new();while let Some(parent)=current{if !seen.insert(parent){break;}result.push(parent);current=tx.query_row("SELECT parent_id FROM keywords WHERE id=?1",[parent],|r|r.get::<_,Option<i64>>(0)).optional().map_err(|e|e.to_string())?.flatten();}Ok(result)}
fn keyword_path(tx:&Transaction<'_>,id:i64)->Result<String,String>{let mut current=Some(id);let mut seen=std::collections::HashSet::new();let mut names=Vec::new();while let Some(key)=current{if !seen.insert(key){break;}let row:Option<(String,Option<i64>)>=tx.query_row("SELECT name,parent_id FROM keywords WHERE id=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;let Some((name,parent))=row else{if key==id{return Err("Kľúčové slovo neexistuje".into());}break;};names.push(name);current=parent;}names.reverse();Ok(names.join(" › "))}
fn keyword_delete_info(connection:&rusqlite::Connection,id:i64)->Result<KeywordDeleteInfo,String>{let name:String=connection.query_row("SELECT name FROM keywords WHERE id=?1",[id],|r|r.get(0)).optional().map_err(|e|e.to_string())?.ok_or_else(||"Kľúčové slovo neexistuje".to_string())?;let mut stmt=connection.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM keywords WHERE id=?1 UNION SELECT k.id FROM keywords k JOIN subtree s ON k.parent_id=s.id) SELECT id FROM subtree").map_err(|e|e.to_string())?;let ids=stmt.query_map([id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;let marks=std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");let sql=format!("SELECT count(DISTINCT asset_id) FROM asset_keywords WHERE keyword_id IN ({marks})");let assigned=connection.query_row(&sql,rusqlite::params_from_iter(ids.iter()),|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? as usize;Ok(KeywordDeleteInfo{id,name,assigned_files:assigned,subkeywords:ids.len().saturating_sub(1)})}

fn insert_asset(transaction: &Transaction<'_>, asset: &NewCatalogAsset) -> Result<(), String> {
    transaction.execute(
        "INSERT INTO assets(id,folder_id,name,ext,ext_lower,is_listed,size,width,height,bpp,caption,notes,author,rating,tagged,label,file_modified,exif_date,acdsee_date,md_dirty,is_orphan,path) VALUES(?1,?2,?3,?4,?5,1,?6,NULL,NULL,NULL,NULL,NULL,NULL,?7,?8,?9,?10,?11,NULL,NULL,NULL,?12)",
        (asset.id, asset.folder_id, &asset.name, &asset.ext, asset.ext.to_lowercase(), asset.size, asset.rating, i64::from(asset.tagged), &asset.label, &asset.file_modified, &asset.exif_date, &asset.path),
    ).map_err(|e| e.to_string())?;
    Ok(())
}

fn track_incoming(tx: &Transaction<'_>, id: i64, path: &str, folder: &str) -> Result<(), String> {
    if folder.is_empty() { return Ok(()); }
    if crate::incoming::is_inside(path, folder) {
        tx.execute("UPDATE assets SET incoming_seen_at=?2 WHERE id=?1 AND incoming_seen_at IS NULL",
            (id, crate::quarantine::now())).map_err(|e| e.to_string())?;
    } else {
        tx.execute("UPDATE assets SET incoming_seen_at=NULL WHERE id=?1 AND incoming_seen_at IS NOT NULL",
            [id]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fulfil_edit_with_sibling(tx: &Transaction<'_>, folder_id: i64, candidate: &ListedAsset) -> Result<(), String> {
    let Some(modified) = candidate.file_modified.as_deref() else { return Ok(()); };
    let Some(stem) = Path::new(&candidate.name).file_stem() else { return Ok(()); };
    let stem = stem.to_string_lossy().to_lowercase();
    let mut select = tx.prepare_cached("SELECT id,name,ext_lower FROM assets
        WHERE folder_id=?1 AND is_listed=1 AND edit_pending_since IS NOT NULL AND edit_pending_since<unixepoch(?2)")
        .map_err(|e| e.to_string())?;
    let matches = select.query_map((folder_id, modified), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    }).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    let mut clear = tx.prepare_cached("UPDATE assets SET edit_pending_since=NULL WHERE id=?1").map_err(|e| e.to_string())?;
    for (id, name, ext) in matches {
        if !ext.is_empty() && !ext.eq_ignore_ascii_case(&candidate.ext)
            && Path::new(&name).file_stem().is_some_and(|name| name.to_string_lossy().to_lowercase() == stem) {
            clear.execute([id]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn reconcile_incoming(tx: &Transaction<'_>, root: &str, scanned_folder: Option<&str>) -> Result<(), String> {
    let available = Path::new(root).is_dir();
    let now = crate::quarantine::now();
    let filter = if scanned_folder.is_some() {
        "folder_id IN (SELECT id FROM folders WHERE lower(rtrim(replace(path,'/','\\'),'\\'))=?1)"
    } else {
        "(incoming_seen_at IS NOT NULL OR (is_listed=1 AND path IS NOT NULL))"
    };
    let mut select = tx.prepare(&format!(
        "SELECT id,coalesce(path,''),incoming_seen_at,is_listed=1,edit_pending_since FROM assets WHERE {filter} ORDER BY id"
    )).map_err(|e| e.to_string())?;
    let mut update = tx.prepare("UPDATE assets SET incoming_seen_at=?2 WHERE id=?1").map_err(|e| e.to_string())?;
    let mut clear = tx.prepare("UPDATE assets SET edit_pending_since=NULL WHERE id=?1").map_err(|e| e.to_string())?;
    let mut fulfilled = 0;
    if let Some(folder) = scanned_folder {
        select.raw_bind_parameter(1, normalize_fs_path(folder)).map_err(|e| e.to_string())?;
    }
    let mut rows = select.raw_query();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let id: i64 = row.get(0).map_err(|e| e.to_string())?;
        let path: String = row.get(1).map_err(|e| e.to_string())?;
        let seen: Option<i64> = row.get(2).map_err(|e| e.to_string())?;
        let listed: Option<bool> = row.get(3).map_err(|e| e.to_string())?;
        let pending: Option<i64> = row.get(4).map_err(|e| e.to_string())?;
        let next = if listed != Some(true) || !crate::incoming::is_inside(&path, root) {
            None
        } else if !available {
            seen
        } else {
            match std::fs::metadata(&path) {
                Ok(info) if info.is_file() => {
                    if scanned_folder.is_none() && pending.is_some_and(|since| info.modified().ok()
                        .is_some_and(|mtime| crate::quarantine::modified_after_mark(mtime, since))) {
                        fulfilled += clear.execute([id]).map_err(|e| e.to_string())?;
                    }
                    seen.or(Some(now))
                }
                Ok(_) => None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => seen,
            }
        };
        if next != seen { update.execute((id, next)).map_err(|e| e.to_string())?; }
    }
    if fulfilled > 0 { record_change(tx, "fulfil_pending_edits", &format!("Úpravy zistené v súboroch · {fulfilled}"))?; }
    Ok(())
}

fn load_folder_paths(transaction: &Transaction<'_>) -> Result<HashMap<String, i64>, String> {
    let mut statement = transaction.prepare("SELECT id,path FROM folders WHERE path IS NOT NULL").map_err(|e| e.to_string())?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    for row in rows { let (id,path) = row.map_err(|e| e.to_string())?; result.insert(normalize_fs_path(&path), id); }
    Ok(result)
}

fn load_folder_roots(transaction: &Transaction<'_>) -> Result<Vec<(i64, String)>, String> {
    let mut statement = transaction.prepare("SELECT id,path FROM folder_roots WHERE path IS NOT NULL").map_err(|e| e.to_string())?;
    statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

fn ensure_folder(transaction: &Transaction<'_>, target: &str, folders: &mut HashMap<String, i64>, roots: &mut Vec<(i64, String)>) -> Result<i64, String> {
    let target_key = normalize_fs_path(target);
    if let Some(id) = folders.get(&target_key) { return Ok(*id); }
    if target.trim().is_empty() { return Err("Folder path is empty".into()); }
    let root = roots.iter().filter(|(_,path)| is_path_prefix(path,target)).max_by_key(|(_,path)| path.len()).cloned();
    let (root_id, root_path) = if let Some(root) = root {
        root
    } else {
        let (name,path) = derive_path_root(target);
        let id = next_id(transaction,"folder_roots")?;
        transaction.execute("INSERT INTO folder_roots(id,name,path) VALUES(?1,?2,?3)",(id,&name,&path)).map_err(|e|e.to_string())?;

        roots.push((id, path.clone()));
        (id, path)
    };
    let root_key = normalize_fs_path(&root_path);
    let mut parent_id = if let Some(id)=folders.get(&root_key) { *id } else {
        let id=next_id(transaction,"folders")?;
        transaction.execute("INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,NULL,?2,'\\',?3)",(id,root_id,&root_path)).map_err(|e|e.to_string())?;

        folders.insert(root_key.clone(),id);
        id
    };
    let target_parts = path_parts(target);
    let root_parts = path_parts(&root_path);
    let skip = root_parts.len();
    let mut current = PathBuf::from(&root_path);
    for part in target_parts.into_iter().skip(skip) {
        current.push(&part);
        let path=current.to_string_lossy().to_string(); let key=normalize_fs_path(&path);
        if let Some(id)=folders.get(&key) { parent_id=*id; continue; }
        let id=next_id(transaction,"folders")?;
        transaction.execute("INSERT INTO folders(id,parent_id,root_id,name,path) VALUES(?1,?2,?3,?4,?5)",(id,parent_id,root_id,&part,&path)).map_err(|e|e.to_string())?;

        folders.insert(key,id); parent_id=id;
    }
    Ok(parent_id)
}

fn derive_path_root(path: &str) -> (String,String) {
    let normalized=path.replace('/',"\\");
    if normalized.len()>=3 && normalized.as_bytes()[1]==b':' && normalized.as_bytes()[2]==b'\\' {
        return (normalized[..2].to_owned(),normalized[..3].to_owned());
    }
    let parts=path_parts(&normalized);
    if normalized.starts_with("\\\\") && parts.len()>=2 {
        let root=format!("\\\\{}\\{}\\",parts[0],parts[1]);
        return (format!("{}\\{}",parts[0],parts[1]),root);
    }
    let first=parts.first().cloned().unwrap_or_else(||normalized.clone());
    (first.clone(),first)
}

fn path_parts(path:&str)->Vec<String>{path.split(['\\','/']).filter(|part|!part.is_empty()).map(str::to_owned).collect()}
pub(crate) fn normalize_fs_path(path:&str)->String{path.trim_end_matches(['\\','/']).replace('/',"\\").to_lowercase()}
fn is_path_prefix(root:&str,target:&str)->bool{let root=normalize_fs_path(root);let target=normalize_fs_path(target);target==root||target.strip_prefix(&root).is_some_and(|suffix|suffix.starts_with('\\'))}

// Map the normalized prefix to the separator in the original path, whose UTF-8 width can differ.
fn matched_path_suffix<'a>(path: &'a str, old_normalized: &str) -> Option<&'a str> {
    let normalized = normalize_fs_path(path);
    if normalized == old_normalized {
        return Some("");
    }
    if !normalized.strip_prefix(old_normalized)?.starts_with('\\') {
        return None;
    }
    let separators = old_normalized.matches('\\').count();
    let (boundary, _) = path.match_indices(['\\', '/']).nth(separators)?;
    Some(&path[boundary..])
}

fn resolve_write_assets(
    tx: &Transaction<'_>, asset_ids: &[i64], listed: &[ListedAsset], insert_missing: bool, require_temporary_id: bool,
    track: impl Fn(&Transaction<'_>, i64, &str) -> Result<(), String>,
) -> Result<(Vec<AssetValueState>, Vec<AssetIdRemap>, usize), String> {
    let mut ids = asset_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let candidates: HashMap<_, _> = listed.iter().map(|asset| (asset.temporary_id, asset)).collect();
    let mut folders = None;
    let mut next_asset_id = None;
    let mut states = Vec::new();
    let mut remapped = Vec::new();
    let mut resolved = std::collections::HashSet::new();
    let mut inserted = 0;
    for requested in ids {
        let id = if requested >= 0 {
            requested
        } else {
            let Some(candidate) = candidates.get(&requested) else {
                if require_temporary_id { return Err(format!("Listed file {requested} is no longer available")); }
                continue;
            };
            if candidate.ext.trim().is_empty() { continue; }
            let id = match find_asset_by_path(tx, &candidate.path)? {
                Some(id) => id,
                None if insert_missing => {
                    if folders.is_none() { folders = Some((load_folder_paths(tx)?, load_folder_roots(tx)?)); }
                    let (paths, roots) = folders.as_mut().unwrap();
                    let folder_id = ensure_folder(tx, &candidate.folder_path, paths, roots)?;
                    let id = match next_asset_id {
                        Some(id) => id,
                        None => next_id(tx, "assets")?,
                    };
                    next_asset_id = Some(id.checked_add(1).ok_or("Asset ID limit reached")?);
                    insert_asset(tx, &NewCatalogAsset {
                        id, folder_id, name: candidate.name.clone(), ext: candidate.ext.clone(), size: candidate.size,
                        file_modified: candidate.file_modified.clone(), path: candidate.path.clone(),
                        rating: None, label: None, tagged: false, exif_date: None,
                    })?;
                    track(tx, id, &candidate.path)?;
                    inserted += 1;
                    id
                }
                None => continue,
            };
            remapped.push(AssetIdRemap { old_id: requested, new_id: id });
            id
        };
        if resolved.insert(id) {
            if let Some(state) = read_asset_values(tx, id)? { states.push(state); }
        }
    }
    Ok((states, remapped, inserted))
}

fn set_asset_links(
    tx: &Transaction<'_>, table: &str, column: &str, targets: &[AssetValueState], label_id: i64, assign: bool,
) -> Result<usize, String> {
    let sql = if assign {
        format!("INSERT OR IGNORE INTO {table}(asset_id,{column}) VALUES(?1,?2)")
    } else {
        format!("DELETE FROM {table} WHERE asset_id=?1 AND {column}=?2")
    };
    let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
    let mut changed = 0;
    for target in targets {
        changed += stmt.execute((target.id, label_id)).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

fn update_column(tx: &Transaction<'_>, table: &str, id: i64, column: &str, value: impl rusqlite::ToSql) -> Result<bool, String> {
    let count = tx.execute(&format!("UPDATE {table} SET {column}=?1 WHERE id=?2 AND {column} IS NOT ?1"), (value, id))
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

fn table_exists(tx: &Transaction<'_>, table: &str) -> Result<bool, String> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn delete_row(tx: &Transaction<'_>, table: &str, id: i64) -> Result<(), String> {
    remember_id(tx, table, id)?;
    tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id]).map_err(|e| e.to_string())?;
    Ok(())
}

fn remove_folder_rows(tx: &Transaction<'_>, folders: &[(i64, String, Option<i64>)]) -> Result<(), String> {
    let ids = folders.iter().map(|row| row.0).collect::<Vec<_>>();
    if let Some(&max_id) = ids.iter().max() { remember_id(tx, "folders", max_id)?; }
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        tx.execute(&format!("DELETE FROM folders WHERE id IN ({marks})"), rusqlite::params_from_iter(chunk))
            .map_err(|e| e.to_string())?;
    }
    let roots = folders.iter().filter_map(|row| row.2).collect::<std::collections::HashSet<_>>();
    for root in roots {
        remember_id(tx, "folder_roots", root)?;
        tx.execute("DELETE FROM folder_roots WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM folders WHERE root_id=?1)", [root])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// Deleted IDs can still be present in the AI index and in listing caches.
pub(crate) fn remember_id(tx: &Transaction<'_>, table: &str, id: i64) -> Result<(), String> {
    let key = format!("ddreams_last_deleted_{table}_id");
    tx.execute("INSERT INTO meta(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE CAST(meta.value AS INTEGER) < ?2",
        (key, id)).map_err(|e| e.to_string())?;
    Ok(())
}

fn next_id(tx: &Transaction<'_>, table: &str) -> Result<i64, String> {
    let key = format!("ddreams_last_deleted_{table}_id");
    let (live, deleted): (i64, i64) = tx.query_row(
        &format!("SELECT coalesce((SELECT max(id) FROM {table}),0), coalesce((SELECT CAST(value AS INTEGER) FROM meta WHERE key=?1),0)"),
        [key], |row| Ok((row.get(0)?, row.get(1)?)),
    ).map_err(|e| e.to_string())?;
    live.max(deleted).checked_add(1).ok_or_else(|| format!("ID limit reached for {table}"))
}

fn record_change(tx: &Transaction<'_>, action: &str, summary: &str) -> Result<Option<i64>, String> {
    tx.execute("INSERT INTO ddreams_changes(ts,action,summary) VALUES(?1,?2,?3)", (iso_now(), action, summary))
        .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    mark_catalog_modified(tx)?;
    Ok(Some(id))
}

fn mark_catalog_modified(transaction: &Transaction<'_>) -> Result<(), String> {
    transaction.execute("UPDATE meta SET value='1' WHERE key='catalog_modified'", []).map_err(|e|e.to_string())?;
    Ok(())
}

fn iso_now() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = (duration.as_secs() / 86_400) as i64;
    let second_of_day = duration.as_secs() % 86_400;
    let z=days+719_468; let era=if z>=0{z}else{z-146_096}/146_097; let doe=z-era*146_097;
    let yoe=(doe-doe/1_460+doe/36_524-doe/146_096)/365; let mut year=yoe+era*400;
    let doy=doe-(365*yoe+yoe/4-yoe/100); let mp=(5*doy+2)/153; let day=doy-(153*mp+2)/5+1;
    let month=mp+if mp<10{3}else{-9}; if month<=2{year+=1;}
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",second_of_day/3600,second_of_day/60%60,second_of_day%60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    use crate::db::Db;
    use std::sync::{Arc,Mutex};

    fn write_fixture() -> Db {
        let mut connection=Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT NOT NULL,action TEXT NOT NULL,summary TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER NOT NULL,table_name TEXT NOT NULL,row_key TEXT NOT NULL,column_name TEXT NOT NULL,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE category_roots(id INTEGER PRIMARY KEY,name TEXT);INSERT INTO category_roots VALUES(1,'Rodina');CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);INSERT INTO categories VALUES(10,'Rodina',NULL,1),(11,'Deti',10,1);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE INDEX idx_assets_path ON assets(path);INSERT INTO folder_roots VALUES(1,'F:','F:\\');INSERT INTO folders VALUES(1,NULL,1,'F:','F:\\');INSERT INTO folders VALUES(2,1,1,'Photos','F:\\Photos');INSERT INTO assets VALUES(1,2,'one.jpg','jpg','jpg',1,123.0,10,10,24,NULL,NULL,NULL,2,0,'Red','2020-01-01T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\one.jpg');INSERT INTO assets VALUES(2,2,'two.jpg','jpg','jpg',1,456.0,10,10,24,NULL,NULL,NULL,4,1,NULL,'2020-01-02T00:00:00Z',NULL,NULL,NULL,0,'F:\\Photos\\two.jpg');").unwrap();
        crate::db::migrate_category_icons(&mut connection).unwrap();
        Arc::new(Mutex::new(connection))
    }

    fn keyword_fixture()->Db{let db=write_fixture();db.lock().unwrap().execute_batch("CREATE TABLE keywords(id INTEGER PRIMARY KEY,name TEXT NOT NULL,parent_id INTEGER);INSERT INTO keywords VALUES(10,'Vianoce',NULL),(11,'Deti',10),(12,'Vnúčatá',11),(20,'Rodina',NULL);CREATE TABLE asset_keywords(asset_id INTEGER NOT NULL,keyword_id INTEGER NOT NULL,PRIMARY KEY(asset_id,keyword_id));INSERT INTO asset_keywords VALUES(1,10),(2,12);").unwrap();db}

    fn scope_fixture()->Db{
        let db=keyword_fixture();
        db.lock().unwrap().execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER NOT NULL,color TEXT NOT NULL,state INTEGER NOT NULL,PRIMARY KEY(asset_id,color));
            INSERT INTO folders VALUES(3,1,1,'Ľudia','F:\\Ľudia');
            INSERT INTO assets VALUES(3,3,'lily.jpg','jpg','jpg',1,9.0,10,10,24,NULL,NULL,NULL,5,0,NULL,NULL,NULL,NULL,NULL,0,'F:\\Ľudia\\lily.jpg');
            INSERT INTO assets VALUES(4,2,'p_1%.jpg','jpg','jpg',1,9.0,10,10,24,NULL,NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,NULL,0,'F:\\Photos_old\\p_1%.jpg');
            INSERT INTO asset_categories VALUES(1,11),(3,11);INSERT INTO asset_color_overrides VALUES(1,'modrá',1),(3,'zelená',0);").unwrap();
        db
    }
    fn dump(db:&Db)->Vec<String>{let conn=db.lock().unwrap();let mut out=Vec::new();for sql in ["SELECT id||':'||coalesce(path,'')||':'||coalesce(rating,'') FROM assets ORDER BY id","SELECT asset_id||':'||category_id FROM asset_categories ORDER BY 1","SELECT asset_id||':'||keyword_id FROM asset_keywords ORDER BY 1","SELECT asset_id||':'||color||':'||state FROM asset_color_overrides ORDER BY 1"]{let mut s=conn.prepare(sql).unwrap();out.extend(s.query_map([],|r|r.get::<_,String>(0)).unwrap().map(|r|r.unwrap()));}out}

    #[test]
    fn multi_file_writes_allocate_distinct_ids_and_keep_remaps() {
        for operation in ["values", "category", "keyword", "create_keyword"] {
            let db = keyword_fixture();
            let writer = CatalogWriter::new(db.clone(), true);
            let listed = [(-10, "Camera", "a.jpg"), (-11, "Mobile", "b.jpg")].map(|(temporary_id, folder, name)| ListedAsset {
                temporary_id, name: name.into(), ext: "jpg".into(), size: Some(5.0), file_modified: None,
                path: format!(r"G:\{folder}\{name}"), folder_path: format!(r"G:\{folder}"),
            });
            let ids = [-10, -11];
            let remaps = match operation {
                "values" => writer.set_asset_values(&ids, &listed, &serde_json::Map::from_iter([("rating".into(), Value::from(3))])).unwrap().remapped_ids,
                "category" => writer.set_asset_categories(&ids, &listed, 11, true).unwrap().remapped_ids,
                "keyword" => writer.set_asset_keywords(&ids, &listed, 10, true).unwrap().remapped_ids,
                _ => writer.create_and_assign_keyword(&ids, &listed, None, "Nové").unwrap().remapped_ids,
            };
            assert_eq!(remaps.iter().map(|r| (r.old_id, r.new_id)).collect::<Vec<_>>(), [(-11, 3), (-10, 4)], "{operation}");
            let conn = db.lock().unwrap();
            let sql = match operation {
                "values" => "SELECT count(*) FROM assets WHERE id IN (3,4) AND rating=3",
                "category" => "SELECT count(*) FROM asset_categories WHERE asset_id IN (3,4) AND category_id=11",
                "keyword" => "SELECT count(*) FROM asset_keywords WHERE asset_id IN (3,4) AND keyword_id=10",
                _ => "SELECT count(*) FROM asset_keywords WHERE asset_id IN (3,4) AND keyword_id=(SELECT id FROM keywords WHERE name='Nové')",
            };
            assert_eq!(conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap(), 2, "{operation}");
            assert_eq!(conn.query_row("SELECT count(*) FROM folder_roots WHERE name='G:'", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "{operation}");
            assert_eq!(conn.query_row("SELECT count(DISTINCT folder_id) FROM assets WHERE id IN (3,4)", [], |r| r.get::<_, i64>(0)).unwrap(), 2, "{operation}");
        }
    }

    #[test]
    fn folder_rewrite_handles_trailing_slashes_and_unicode_prefix_widths() {
        let db = write_fixture();
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE folders SET path=?1 WHERE id=2", [r"F:\Kút\"]).unwrap();
            conn.execute("UPDATE assets SET path=?1 WHERE id=1", [r"f:/Kút/Sub/Čaj.JPG"]).unwrap();
            conn.execute("UPDATE assets SET path=?1 WHERE id=2", [r"F:\Kút_old\Two.jpg"]).unwrap();
            conn.execute_batch("CREATE TABLE ddreams_folder_rules(path TEXT PRIMARY KEY,mode TEXT,created TEXT);").unwrap();
            conn.execute("INSERT INTO ddreams_folder_rules VALUES(?1,'include','now')", [r"F:\Kút\Ľudia"]).unwrap();
        }
        let writer = CatalogWriter::new(db.clone(), true);
        writer.rewrite_folder_paths(r"F:\kút///", r"G:/Nový/", "rename_folder", "Premenovaný priečinok").unwrap();
        let conn = db.lock().unwrap();
        for (sql, expected) in [
            ("SELECT path FROM folders WHERE id=2", r"G:\Nový"),
            ("SELECT name FROM folders WHERE id=2", "Nový"),
            ("SELECT path FROM assets WHERE id=1", r"G:\Nový\Sub\Čaj.JPG"),
            ("SELECT path FROM assets WHERE id=2", r"F:\Kút_old\Two.jpg"),
            ("SELECT path FROM ddreams_folder_rules", r"G:\Nový\Ľudia"),
        ] {
            assert_eq!(conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap(), expected);
        }
    }

    #[test]
    fn label_writes_notify_both_kinds_and_keep_legacy_history_details() {
        let db = keyword_fixture();
        db.lock().unwrap().execute_batch(
            "INSERT INTO ddreams_changes VALUES(1,'2000-01-01','old','Starý záznam',1);
             INSERT INTO ddreams_change_items VALUES(1,'assets','1','rating','old','new');"
        ).unwrap();
        let writer = CatalogWriter::new(db.clone(), true);
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = calls.clone();
        writer.set_change_hook(Arc::new(move |kinds| observed.lock().unwrap().extend(kinds.iter().map(|kind| kind.to_string()))));
        writer.set_asset_categories(&[1], &[], 11, true).unwrap();
        writer.set_asset_keywords(&[2], &[], 10, true).unwrap();
        writer.set_asset_keywords(&[2], &[], 10, true).unwrap();
        assert_eq!(*calls.lock().unwrap(), ["category", "keyword"]);
        assert_eq!(writer.recent_changes(5).unwrap().len(), 2);
        let conn = db.lock().unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM ddreams_changes", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        assert_eq!(conn.query_row("SELECT count(*) FROM ddreams_change_items", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn exclusion_removes_only_rows_under_the_folder(){
        let db=scope_fixture();let writer=CatalogWriter::new(db.clone(),true);
        assert_eq!(writer.exclusion_impact("f:/photos/").unwrap(),ExclusionImpact{files:2,with_categories:1,with_keywords:2,with_rating_label_tagged:2,with_color_overrides:1});
        let removed=writer.set_folder_rule("F:\\Photos",Some("exclude")).unwrap();
        assert_eq!(removed,vec![1,2]);
        let after=dump(&db);assert!(after.iter().all(|row|!row.starts_with("1:")&&!row.starts_with("2:")),"{after:?}");
        assert!(after.contains(&"4:F:\\Photos_old\\p_1%.jpg:".to_string()),"component boundary: Photos_old must stay");
        assert_eq!(writer.folder_rule_state("F:\\Photos\\sub").unwrap().effective.as_deref(),Some("exclude"));
        assert!(writer.set_asset_values(&[3],&[],&serde_json::Map::from_iter([("rating".to_string(),serde_json::json!(3))])).is_ok());
    }

    #[test]
    fn exclusion_matches_non_ascii_paths(){
        let db=scope_fixture();let writer=CatalogWriter::new(db.clone(),true);
        assert_eq!(writer.set_folder_rule("f:\\ľudia",Some("exclude")).unwrap(),vec![3]);
    }

    #[test]
    fn auto_catalog_adds_listed_files_once(){
        let db=scope_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let listed=|id:i64,name:&str|ListedAsset{temporary_id:id,name:name.into(),ext:"jpg".into(),size:Some(5.0),file_modified:None,path:format!("F:\\Inbox\\{name}"),folder_path:"F:\\Inbox".into()};
        let date = "2026-08-31T16:16:00";
        let remaps=writer.catalog_listed_assets_with_dates(
            &[listed(-10,"a.jpg"),listed(-11,"b.jpg")], &std::collections::HashMap::from([(-10, date.to_owned())]),
        ).unwrap();
        assert_eq!(remaps.len(),2);
        let stored: String = db.lock().unwrap().query_row("SELECT exif_date FROM assets WHERE id=?1",
            [remaps[0].new_id], |row| row.get(0)).unwrap();
        assert_eq!(stored, date);
        assert!(writer.catalog_listed_assets(&[listed(-12,"a.jpg")]).unwrap().is_empty(),"already catalogued by path");
        let summary:String=db.lock().unwrap().query_row("SELECT summary FROM ddreams_changes ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
        assert_eq!(summary,"Pridané do katalógu: 2 súbory");
    }

    #[test]
    fn backfill_capture_dates_updates_only_missing_values() {
        let db = scope_fixture();
        let writer = CatalogWriter::new(db.clone(), true);
        db.lock().unwrap().execute_batch("UPDATE assets SET exif_date='' WHERE id=2;
            UPDATE assets SET exif_date='2025-01-01T00:00:00' WHERE id=3").unwrap();
        let new_date = "2026-08-31T16:16:00";
        let dates = [1, 2, 3].into_iter().map(|id| (id, new_date.to_owned())).collect();
        assert!(CatalogWriter::new(db.clone(), false).backfill_capture_dates(&dates).is_err());
        assert_eq!(writer.backfill_capture_dates(&dates).unwrap(), 2);
        assert_eq!(writer.backfill_capture_dates(&dates).unwrap(), 0);
        let conn = db.lock().unwrap();
        for (id, expected) in [(1, new_date), (2, new_date), (3, "2025-01-01T00:00:00")] {
            let stored: String = conn.query_row("SELECT exif_date FROM assets WHERE id=?1",
                [id], |row| row.get(0)).unwrap();
            assert_eq!(stored, expected);
        }
    }

    #[test]
    fn excluded_folder_refuses_catalogue_edits(){
        let db=scope_fixture();let writer=CatalogWriter::new(db.clone(),true);
        writer.set_folder_rule("F:\\Ľudia",Some("include")).unwrap();
        writer.set_folder_rule("F:\\Photos",Some("exclude")).unwrap();
        let listed=ListedAsset{temporary_id:-5,name:"new.jpg".into(),ext:"jpg".into(),size:Some(1.0),file_modified:None,path:"F:\\Photos\\new.jpg".into(),folder_path:"F:\\Photos".into()};
        assert_eq!(writer.set_asset_categories(&[-5],&[listed],11,true).unwrap_err(),"Priečinok je vylúčený z katalógu.");
    }

    fn keyword_rows(db:&Db)->Vec<(i64,i64)>{let conn=db.lock().unwrap();let mut stmt=conn.prepare("SELECT asset_id,keyword_id FROM asset_keywords ORDER BY asset_id,keyword_id").unwrap();stmt.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()}

    #[test]
    fn color_override_updates_only_listed_assets() {
        let db=write_fixture();db.lock().unwrap().execute_batch("CREATE TABLE asset_color_overrides(asset_id INTEGER NOT NULL,color TEXT NOT NULL,state INTEGER NOT NULL,PRIMARY KEY(asset_id,color));").unwrap();
        let writer=CatalogWriter::new(db.clone(),true);
        assert_eq!(writer.set_asset_colors(&[1,999],"zelená",Some(true),"Farba: zelená").unwrap(),1);
        assert_eq!(db.lock().unwrap().query_row("SELECT state FROM asset_color_overrides WHERE asset_id=1 AND color='zelená'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn set_values_updates_one_and_many_assets_with_one_journal_change() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(5)),("label".into(),Value::from("Blue"))]);
        let result=writer.set_asset_values(&[1,2,1],&[],&changes).unwrap();
        assert!(result.summary.contains("Hodnotenie 5"));assert!(result.summary.contains("2 súbory"));
        let connection=db.lock().unwrap();
        assert_eq!(connection.query_row("SELECT count(*) FROM ddreams_changes",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(connection.query_row("SELECT count(*) FROM assets WHERE rating=5 AND label='Blue'",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        drop(connection);
        let single_changes=serde_json::Map::from_iter([("rating".into(),Value::from(1))]);
        let single=writer.set_asset_values(&[1],&[],&single_changes).unwrap();
        assert!(single.summary.contains("1 súbor"));
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn set_values_toggle_tags_together_for_a_mixed_selection() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let changes=serde_json::Map::from_iter([("tagged".into(),Value::from("toggle"))]);
        writer.set_asset_values(&[1,2],&[],&changes).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE tagged=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        writer.set_asset_values(&[1,2],&[],&changes).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM assets WHERE tagged=0",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn uncatalogued_listed_asset_is_inserted_with_its_folder() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let candidate=ListedAsset{temporary_id:-10,name:"new.jpg".into(),ext:"jpg".into(),size:Some(789.0),file_modified:Some("2024-03-04T00:00:00Z".into()),path:"G:\\Camera\\new.jpg".into(),folder_path:"G:\\Camera".into()};
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(3))]);
        let result=writer.set_asset_values(&[-10],&[candidate],&changes).unwrap();let inserted=result.remapped_ids[0].new_id;
        assert_eq!(db.lock().unwrap().query_row("SELECT name||':'||rating FROM assets WHERE id=?1",[inserted],|row|row.get::<_,String>(0)).unwrap(),"new.jpg:3");
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM folders WHERE path=?1", [r"G:\Camera"], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn readonly_writer_refuses_set_asset_values() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),false);
        let changes=serde_json::Map::from_iter([("rating".into(),Value::from(4))]);
        assert!(writer.set_asset_values(&[1],&[],&changes).unwrap_err().contains("read-only"));
        assert_eq!(db.lock().unwrap().query_row("SELECT rating FROM assets WHERE id=1",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn category_assignment_is_partial_idempotent_and_removable() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        db.lock().unwrap().execute("INSERT INTO asset_categories VALUES(1,11)",[]).unwrap();
        let assigned=writer.set_asset_categories(&[1,2],&[],11,true).unwrap();
        assert_eq!(assigned.changed_count,1);assert_eq!(assigned.summary,"Zaradené do Rodina › Deti · 1 súbor");
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_categories WHERE category_id=11",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        let journal_count=db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap();
        let no_op=writer.set_asset_categories(&[1,2],&[],11,true).unwrap();assert_eq!(no_op.change_id,None);assert_eq!(no_op.changed_count,0);
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap(),journal_count);
        let removed=writer.set_asset_categories(&[1,2],&[],11,false).unwrap();assert_eq!(removed.changed_count,2);assert!(removed.summary.contains("Vyradené z Rodina › Deti · 2 súbory"));
    }

    #[test]
    fn category_assignment_multi_summary_has_correct_slovak_plural_and_one_journal_row() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let result=writer.set_asset_categories(&[1,2],&[],10,true).unwrap();
        assert_eq!(result.summary,"Zaradené do Rodina · 2 súbory");
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn category_assignment_of_uncatalogued_file_creates_link() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let candidate=ListedAsset{temporary_id:-10,name:"new.jpg".into(),ext:"jpg".into(),size:Some(789.0),file_modified:None,path:"G:\\Camera\\new.jpg".into(),folder_path:"G:\\Camera".into()};
        let result=writer.set_asset_categories(&[-10],&[candidate],11,true).unwrap();let id=result.remapped_ids[0].new_id;
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_categories WHERE asset_id=?1 AND category_id=11",[id],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn readonly_writer_refuses_category_changes() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),false);
        assert!(writer.set_asset_categories(&[1],&[],11,true).unwrap_err().contains("read-only"));
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_categories",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn category_summary_uses_slovenian_plural_forms() {
        assert_eq!(slovak_count(1,"súbor","súbory","súborov"),"súbor");
        assert_eq!(slovak_count(2,"súbor","súbory","súborov"),"súbory");
        assert_eq!(slovak_count(4,"súbor","súbory","súborov"),"súbory");
        assert_eq!(slovak_count(5,"súbor","súbory","súborov"),"súborov");
    }

    #[test]
    fn create_top_level_and_subcategory_stores_hierarchy() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let top=writer.create_category(None,"Nové",None).unwrap();let top_id=top.category_id.unwrap();let root_id=top.root_id.unwrap();
        assert_eq!(top.summary,"Nová kategória Nové");assert_eq!(db.lock().unwrap().query_row("SELECT name FROM category_roots WHERE id=?1",[root_id],|r|r.get::<_,String>(0)).unwrap(),"Nové");
        let sub=writer.create_category(Some(10)," Vnúčatá ",None).unwrap();let sub_id=sub.category_id.unwrap();
        assert_eq!(sub.summary,"Nová kategória Rodina › Vnúčatá");assert_eq!(sub.root_id,Some(1));assert_eq!(db.lock().unwrap().query_row("SELECT parent_id FROM categories WHERE id=?1",[sub_id],|r|r.get::<_,Option<i64>>(0)).unwrap(),Some(10));
        assert!(top_id>0);
    }

    #[test]
    fn category_names_are_trimmed_validated_and_unique_case_insensitively() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        assert!(writer.create_category(Some(10)," dEtI ",None).unwrap_err().contains("už kategória"));
        assert!(writer.create_category(None,"rodina",None).is_err());
        assert!(writer.create_category(Some(10),"  ",None).unwrap_err().contains("nesmie byť prázdny"));
        assert!(writer.create_category(None,&"x".repeat(101),None).unwrap_err().contains("100 znakov"));
        let created=writer.create_category(Some(10),"  Nové meno  ",None).unwrap();
        assert_eq!(db.lock().unwrap().query_row("SELECT name FROM categories WHERE id=?1",[created.category_id.unwrap()],|r|r.get::<_,String>(0)).unwrap(),"Nové meno");
    }

    #[test]
    fn renaming_top_level_keeps_root_pair_consistent() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let result=writer.rename_category(10,"Domov").unwrap();assert_eq!(result.summary,"Premenované Rodina → Domov");
        {let conn=db.lock().unwrap();assert_eq!(conn.query_row("SELECT name FROM categories WHERE id=10",[],|r|r.get::<_,String>(0)).unwrap(),"Domov");assert_eq!(conn.query_row("SELECT name FROM category_roots WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),"Domov");}
    }

    #[test]
    fn moving_subcategory_to_top_creates_root_and_updates_entire_subtree() {
        let db=write_fixture();db.lock().unwrap().execute("INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",[]).unwrap();let writer=CatalogWriter::new(db.clone(),true);
        let result=writer.move_category(11,None).unwrap();let new_root=result.root_id.unwrap();
        let conn=db.lock().unwrap();assert_eq!(conn.query_row("SELECT parent_id FROM categories WHERE id=11",[],|r|r.get::<_,Option<i64>>(0)).unwrap(),None);assert_eq!(conn.query_row("SELECT root_id FROM categories WHERE id=11",[],|r|r.get::<_,i64>(0)).unwrap(),new_root);assert_eq!(conn.query_row("SELECT root_id FROM categories WHERE id=12",[],|r|r.get::<_,i64>(0)).unwrap(),new_root);assert_eq!(conn.query_row("SELECT name FROM category_roots WHERE id=?1",[new_root],|r|r.get::<_,String>(0)).unwrap(),"Deti");drop(conn);
    }

    #[test]
    fn deletion_suggestions_are_batched_and_preserve_keep_decisions() {
        let db = write_fixture();
        db.lock().unwrap().execute("ALTER TABLE assets ADD COLUMN delete_suggestion INTEGER", []).unwrap();
        let writer = CatalogWriter::new(db.clone(), true);
        let formats = crate::formats::FormatTable::load_selected(None);
        let state = |id| db.lock().unwrap().query_row("SELECT delete_suggestion FROM assets WHERE id=?1",
            [id], |row| row.get::<_, Option<i64>>(0)).unwrap();
        let folder = r"F:\Photos";
        let batch = [(1, true), (2, true)];
        assert!(writer.suggest_deletions(r"F:\Elsewhere", &batch, &formats).unwrap().change_id.is_none());
        assert!(writer.suggest_deletions(folder, &batch, &formats).unwrap().change_id.is_some());
        assert_eq!((state(1), state(2)), (Some(1), Some(1)));
        writer.keep_suggested_assets(Some(&[1])).unwrap();
        assert_eq!((state(1), state(2)), (Some(0), Some(1)));
        assert!(writer.suggest_deletions(folder, &batch, &formats).unwrap().change_id.is_none());
        writer.keep_suggested_assets(None).unwrap();
        assert_eq!((state(1), state(2)), (Some(0), Some(0)));
        assert!(CatalogWriter::new(db.clone(), false).keep_suggested_assets(None).is_err());
    }

    #[test]
    fn category_icons_set_clear_and_keep_root_pair() {
        let db = write_fixture();
        let writer = CatalogWriter::new(db.clone(), true);
        let icon = |table: &str, id: i64| {
            db.lock().unwrap().query_row(&format!("SELECT icon_key FROM {table} WHERE id=?1"), [id], |row| {
                row.get::<_, Option<String>>(0)
            }).unwrap()
        };
        assert!(writer.set_category_icon(11, false, Some("camera")).unwrap().change_id.is_some());
        assert_eq!(icon("categories", 11).as_deref(), Some("camera"));
        assert!(writer.set_category_icon(11, false, Some("camera")).unwrap().change_id.is_none());
        writer.set_category_icon(11, false, None).unwrap();
        assert_eq!(icon("categories", 11), None);

        writer.set_category_icon(1, true, Some("people")).unwrap();
        assert_eq!(icon("category_roots", 1).as_deref(), Some("people"));
        assert_eq!(icon("categories", 10).as_deref(), Some("people"));

        db.lock().unwrap().execute("INSERT INTO category_roots(id,name) VALUES(2,'Miesta')", []).unwrap();
        writer.set_category_icon(2, true, Some("places")).unwrap();
        assert_eq!(icon("category_roots", 2).as_deref(), Some("places"));

        let readonly = CatalogWriter::new(db.clone(), false);
        assert!(readonly.set_category_icon(11, false, Some("camera")).unwrap_err().contains("read-only"));
        assert!(writer.set_category_icon(11, false, Some("unknown")).is_err());
    }

    #[test]
    fn create_category_stores_icon_and_delete_removes_empty_root() {
        let db = write_fixture();
        let writer = CatalogWriter::new(db.clone(), true);
        let created = writer.create_category(None, "Výlety", Some("travel")).unwrap();
        let category_id = created.category_id.unwrap();
        let root_id = created.root_id.unwrap();
        let root_icon = || db.lock().unwrap().query_row(
            "SELECT icon_key FROM category_roots WHERE id=?1", [root_id], |row| row.get::<_, String>(0),
        );
        assert_eq!(root_icon().unwrap(), "travel");
        writer.delete_category(category_id).unwrap();
        assert!(matches!(root_icon(), Err(rusqlite::Error::QueryReturnedNoRows)));
    }

    #[test]
    fn moving_top_level_under_another_category_removes_empty_root() {
        let db=write_fixture();db.lock().unwrap().execute_batch("INSERT INTO category_roots(id,name) VALUES(2,'Priatelia');INSERT INTO categories(id,parent_id,root_id,name) VALUES(20,NULL,2,'Priatelia'),(21,20,2,'Blízki'),(12,11,1,'Mladí');").unwrap();let writer=CatalogWriter::new(db.clone(),true);
        writer.move_category(10,Some(21)).unwrap();{let conn=db.lock().unwrap();assert_eq!(conn.query_row("SELECT parent_id||':'||root_id FROM categories WHERE id=10",[],|r|r.get::<_,String>(0)).unwrap(),"21:2");assert_eq!(conn.query_row("SELECT root_id FROM categories WHERE id=12",[],|r|r.get::<_,i64>(0)).unwrap(),2);assert_eq!(conn.query_row("SELECT count(*) FROM category_roots WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0);}
    }

    #[test]
    fn category_move_rejects_self_and_descendant_cycles() {
        let db=write_fixture();db.lock().unwrap().execute("INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",[]).unwrap();let writer=CatalogWriter::new(db,true);
        assert!(writer.move_category(11,Some(11)).unwrap_err().contains("seba"));assert!(writer.move_category(10,Some(12)).unwrap_err().contains("podkategórie"));
    }

    #[test]
    fn delete_refuses_assigned_files_in_descendants_and_reports_count() {
        let db=write_fixture();db.lock().unwrap().execute("INSERT INTO asset_categories VALUES(1,11)",[]).unwrap();let writer=CatalogWriter::new(db.clone(),true);
        let info=writer.category_delete_info(10).unwrap();assert_eq!(info.assigned_files,1);assert_eq!(info.subcategories,1);
        let error=writer.delete_category(10).unwrap_err();assert!(error.contains("obsahuje 1 súbor ("));assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM categories WHERE id IN (10,11)",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn delete_subtree_keeps_parent_category() {
        let db=write_fixture();db.lock().unwrap().execute("INSERT INTO categories(id,parent_id,root_id,name) VALUES(12,11,1,'Mladí')",[]).unwrap();let writer=CatalogWriter::new(db.clone(),true);
        let deleted=writer.delete_category(11).unwrap();assert_eq!(deleted.deleted_category_ids.len(),2);assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM categories WHERE id IN (11,12)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn readonly_writer_refuses_category_structure_changes() {
        let db=write_fixture();let writer=CatalogWriter::new(db.clone(),false);
        assert!(writer.create_category(None,"Test",None).unwrap_err().contains("read-only"));assert!(writer.rename_category(10,"Test").unwrap_err().contains("read-only"));assert!(writer.move_category(11,None).unwrap_err().contains("read-only"));assert!(writer.delete_category(11).unwrap_err().contains("read-only"));
        assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM categories",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn keyword_assignment_partial_unassign_and_noop(){
        let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let added=writer.set_asset_keywords(&[1,2],&[],10,true).unwrap();assert_eq!(added.changed_count,1);assert_eq!(added.summary,"Pridané kľúčové slovo Vianoce · 1 súbor");
        let after=keyword_rows(&db);assert_eq!(after,vec![(1,10),(2,10),(2,12)]);
        let changes=db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap();let noop=writer.set_asset_keywords(&[1,2],&[],10,true).unwrap();assert_eq!(noop.change_id,None);assert_eq!(noop.changed_count,0);assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap(),changes);
        let removed=writer.set_asset_keywords(&[1],&[],10,false).unwrap();assert_eq!(removed.summary,"Odobrané kľúčové slovo Vianoce · 1 súbor");assert_eq!(keyword_rows(&db),vec![(2,10),(2,12)]);
        assert_eq!(writer.set_asset_keywords(&[1,2],&[],20,true).unwrap().summary,"Pridané kľúčové slovo Rodina · 2 súbory");
    }

    #[test]
    fn keyword_create_rename_move_collisions_and_cycles(){
        let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),true);
        assert_eq!(writer.create_keyword(None," Výlety ").unwrap().summary,"Nové kľúčové slovo Výlety");
        let nested=writer.create_keyword(Some(10),"Deti");assert!(nested.is_err());let nested=writer.create_keyword(Some(10),"Oslava").unwrap();let nested_id=nested.keyword_id.unwrap();assert_eq!(db.lock().unwrap().query_row("SELECT parent_id FROM keywords WHERE id=?1",[nested_id],|r|r.get::<_,Option<i64>>(0)).unwrap(),Some(10));
        let renamed=writer.rename_keyword(10,"Sviatky").unwrap();assert_eq!(renamed.summary,"Premenované Vianoce → Sviatky");
        assert!(writer.create_keyword(None," rodina ").unwrap_err().contains("už existuje kľúčové slovo"));assert!(writer.create_keyword(Some(10)," dEtI ").unwrap_err().contains("už existuje kľúčové slovo"));
        assert!(writer.move_keyword(10,Some(12)).unwrap_err().contains("podriadeného"));
        writer.move_keyword(11,None).unwrap();assert_eq!(db.lock().unwrap().query_row("SELECT parent_id FROM keywords WHERE id=11",[],|r|r.get::<_,Option<i64>>(0)).unwrap(),None);
        writer.move_keyword(20,Some(10)).unwrap();assert_eq!(db.lock().unwrap().query_row("SELECT parent_id FROM keywords WHERE id=20",[],|r|r.get::<_,Option<i64>>(0)).unwrap(),Some(10));
    }

    #[test]
    fn keyword_delete_refuses_descendant_links_then_removes_subtree(){
        let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),true);let info=writer.keyword_delete_info(10).unwrap();assert_eq!(info.assigned_files,2);assert_eq!(info.subkeywords,2);assert!(writer.delete_keyword(10).unwrap_err().contains("obsahuje 2 súbory"));
        db.lock().unwrap().execute("DELETE FROM asset_keywords",[]).unwrap();
        let deleted=writer.delete_keyword(10).unwrap();assert_eq!(deleted.deleted_keyword_ids.len(),3);assert_eq!(db.lock().unwrap().query_row("SELECT id FROM keywords", [], |r| r.get::<_, i64>(0)).unwrap(), 20);
    }

    #[test]
    fn create_and_assign_keyword_is_one_change(){
        let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),true);let count_before=db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap();let result=writer.create_and_assign_keyword(&[1,2],&[],None,"Rodinné").unwrap();assert!(result.summary.starts_with("Nové kľúčové slovo Rodinné a priradené · 2 súbory"));assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM ddreams_changes",[],|r|r.get::<_,i64>(0)).unwrap(),count_before+1);assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_keywords WHERE keyword_id=?1",[result.keyword_id.unwrap()],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn uncatalogued_keyword_assignment_creates_asset_and_link(){
        let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),true);let candidate=ListedAsset{temporary_id:-10,name:"new.jpg".into(),ext:"jpg".into(),size:Some(789.0),file_modified:None,path:"G:\\Camera\\new.jpg".into(),folder_path:"G:\\Camera".into()};let result=writer.set_asset_keywords(&[-10],&[candidate],10,true).unwrap();let id=result.remapped_ids[0].new_id;assert_eq!(db.lock().unwrap().query_row("SELECT count(*) FROM asset_keywords WHERE asset_id=?1 AND keyword_id=10",[id],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn readonly_writer_refuses_keyword_mutations(){let db=keyword_fixture();let writer=CatalogWriter::new(db.clone(),false);assert!(writer.set_asset_keywords(&[1],&[],10,true).unwrap_err().contains("read-only"));assert!(writer.create_keyword(None,"Test").unwrap_err().contains("read-only"));assert!(writer.rename_keyword(10,"Test").unwrap_err().contains("read-only"));assert!(writer.move_keyword(11,None).unwrap_err().contains("read-only"));assert!(writer.delete_keyword(10).unwrap_err().contains("read-only"));assert_eq!(keyword_rows(&db),vec![(1,10),(2,12)]);}
    #[test]
    fn remove_catalog_source_deletes_subtree_rows_and_never_creates_journal_entry(){
        let db=scope_fixture();let writer=CatalogWriter::new(db.clone(),true);
        let result=writer.remove_folder_from_catalog("F:\\Photos").unwrap();
        assert_eq!(result.removed_assets,2);assert_eq!(result.removed_folders,1);assert_eq!(result.change_id,None);
        let conn=db.lock().unwrap();assert_eq!(conn.query_row("SELECT count(*) FROM assets WHERE id IN (1,2)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(conn.query_row("SELECT count(*) FROM asset_categories WHERE asset_id IN (1,2)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(conn.query_row("SELECT count(*) FROM folders WHERE lower(path) LIKE lower(?1)",["F:\\Photos%"],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(conn.query_row("SELECT count(*) FROM assets WHERE id=4",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

}
