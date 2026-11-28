use super::open::Db;
use crate::formats::FormatTable;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct FolderPreviewAsset {
    pub id: i64,
    pub path: String,
    pub file_modified: Option<String>,
}

fn normalized_catalog_path(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).replace('\\', "/").to_lowercase()
}

pub fn folder_previews(
    db: &Db,
    paths: &[String],
    formats: &FormatTable,
) -> rusqlite::Result<HashMap<String, Vec<FolderPreviewAsset>>> {
    let mut result: HashMap<String, Vec<FolderPreviewAsset>> = paths.iter().cloned().map(|p| (p, Vec::new())).collect();
    if result.is_empty() {
        return Ok(result);
    }
    // Cache the first four direct and descendant previews in the original name order.
    #[derive(Default)]
    struct FolderPreviews {
        direct: Vec<Arc<FolderPreviewAsset>>,
        descendants: Vec<Arc<FolderPreviewAsset>>,
    }
    type PreviewIndex = Arc<HashMap<String, FolderPreviews>>;
    static CACHE: Mutex<Option<(std::sync::Weak<Mutex<Connection>>, std::time::Instant, PreviewIndex)>> =
        Mutex::new(None);
    let key = Arc::downgrade(db);
    let cached = CACHE
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(cached_db, at, _)| cached_db.ptr_eq(&key) && at.elapsed().as_secs() < 60)
        .map(|(_, _, index)| index.clone());
    let index = match cached {
        Some(index) => index,
        None => {
            let rows = {
                let conn = db.lock().unwrap();
                let visual_extensions = formats.sql_in("ext_lower", &["image", "raw"]);
                let sql = format!(
                    "SELECT id,path,file_modified,ext FROM assets WHERE is_listed=1 AND path IS NOT NULL AND {visual_extensions} ORDER BY name COLLATE NOCASE"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        FolderPreviewAsset {
                            id: r.get(0)?,
                            path: r.get(1)?,
                            file_modified: r.get(2)?,
                        },
                        r.get::<_, String>(3)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut index = HashMap::<String, FolderPreviews>::new();
            for (preview, ext) in rows {
                if !matches!(formats.group(&ext), Some("image" | "raw")) {
                    continue;
                }
                let path = normalized_catalog_path(&preview.path);
                let Some((parent, _)) = path.rsplit_once('/') else {
                    continue;
                };
                let preview = Arc::new(preview);
                let direct = &mut index.entry(parent.to_owned()).or_default().direct;
                if direct.len() < 4 {
                    direct.push(preview.clone());
                }
                let mut ancestor = parent;
                while let Some((next, _)) = ancestor.rsplit_once('/') {
                    let descendants = &mut index.entry(next.to_owned()).or_default().descendants;
                    if descendants.len() < 4 {
                        descendants.push(preview.clone());
                    }
                    ancestor = next;
                }
            }
            let index = Arc::new(index);
            *CACHE.lock().unwrap() = Some((key, std::time::Instant::now(), index.clone()));
            index
        }
    };
    for (requested, previews) in &mut result {
        if let Some(folder) = index.get(&normalized_catalog_path(requested)) {
            let selected = if folder.direct.is_empty() {
                &folder.descendants
            } else {
                &folder.direct
            };
            previews.extend(selected.iter().map(|preview| preview.as_ref().clone()));
        }
    }
    Ok(result)
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub root_id: Option<i64>,
    pub name: String,
    pub path: Option<String>,
    pub count: i64,
    #[serde(default)]
    pub folder_ids: Vec<i64>,
    #[serde(default)]
    pub virtual_node: Option<String>,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub missing: bool,
}

pub fn folders_page(
    db: &Db,
    parent_id: Option<i64>,
    parent_ids: Option<Vec<i64>>,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<Vec<Folder>> {
    let conn = db.lock().unwrap();
    if parent_id.is_none() && parent_ids.is_none() {
        let mut stmt=conn.prepare("WITH RECURSIVE d(root_folder_id,id) AS (SELECT id,id FROM folders WHERE parent_id IS NULL UNION ALL SELECT d.root_folder_id,f.id FROM folders f JOIN d ON f.parent_id=d.id), counts AS (SELECT d.root_folder_id,count(a.id) n FROM d JOIN assets a ON a.folder_id=d.id AND a.is_listed=1 GROUP BY d.root_folder_id) SELECT f.id,f.root_id,coalesce(nullif(r.name,''),r.path),r.path,coalesce(counts.n,0) FROM folders f JOIN folder_roots r ON r.id=f.root_id LEFT JOIN counts ON counts.root_folder_id=f.id WHERE f.parent_id IS NULL ORDER BY r.name COLLATE NOCASE,f.id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut grouped: std::collections::BTreeMap<String, Vec<(i64, i64, Option<String>, i64)>> =
            std::collections::BTreeMap::new();
        for (id, root, name, path, count) in rows {
            if count > 0 {
                let label = if name == "\\" || name == "/" {
                    path.as_deref()
                        .unwrap_or(&name)
                        .trim_end_matches(['\\', '/'])
                        .to_string()
                } else {
                    name
                };
                let key = label.to_lowercase();
                grouped.entry(key).or_default().push((id, root, Some(label), count));
            }
        }
        let mut result = Vec::new();
        let mut devices = Vec::new();
        for (key, items) in grouped {
            let name = items.first().and_then(|i| i.2.clone()).unwrap_or(key);
            let drive = name.len() == 2 && name.ends_with(':');
            let ids = items.iter().map(|i| i.0).collect::<Vec<_>>();
            let roots = items.iter().map(|i| i.1).collect::<Vec<_>>();
            let count = items.iter().map(|i| i.3).sum();
            let path = conn
                .query_row("SELECT path FROM folders WHERE id=?", [ids[0]], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten();
            let node = Folder {
                id: ids[0],
                parent_id: None,
                root_id: roots.first().copied(),
                name,
                path,
                count,
                folder_ids: ids,
                virtual_node: None,
                offline: false,
                missing: false,
            };
            if drive { result.push(node) } else { devices.push(node) }
        }
        result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        if !devices.is_empty() {
            result.push(Folder {
                id: -2,
                parent_id: None,
                root_id: None,
                name: "Zariadenia".into(),
                path: None,
                count: devices.iter().map(|d| d.count).sum(),
                folder_ids: devices.iter().flat_map(|d| d.folder_ids.clone()).collect(),
                virtual_node: Some("devices".into()),
                offline: false,
                missing: false,
            });
        }
        return Ok(result
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.clamp(1, 500) as usize)
            .collect());
    }
    let parents = parent_ids
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![parent_id.unwrap_or(0)]);
    let placeholders = vec!["?"; parents.len()].join(",");
    let sql = format!(
        "WITH RECURSIVE candidate AS (SELECT f.id,f.parent_id,f.root_id,f.name,f.path FROM folders f WHERE f.parent_id IN ({placeholders})), d(folder_id,id) AS (SELECT c.id,c.id FROM candidate c UNION ALL SELECT d.folder_id,f.id FROM folders f JOIN d ON f.parent_id=d.id), counts AS (SELECT d.folder_id,count(a.id) n FROM d JOIN assets a ON a.folder_id=d.id AND a.is_listed=1 GROUP BY d.folder_id) SELECT c.id,c.parent_id,c.root_id,c.name,c.path,coalesce(n,0) FROM candidate c LEFT JOIN counts ON counts.folder_id=c.id WHERE coalesce(n,0)>0 ORDER BY c.name COLLATE NOCASE,c.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(parents), |r| {
            let id = r.get(0)?;
            Ok(Folder {
                id,
                parent_id: r.get(1)?,
                root_id: r.get(2)?,
                name: r.get(3)?,
                path: r.get(4)?,
                count: r.get(5)?,
                folder_ids: vec![id],
                virtual_node: None,
                offline: false,
                missing: false,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut merged: std::collections::BTreeMap<String, Folder> = std::collections::BTreeMap::new();
    for folder in rows {
        let key = folder.name.to_lowercase();
        if let Some(existing) = merged.get_mut(&key) {
            existing.count += folder.count;
            existing.folder_ids.push(folder.id);
        } else {
            merged.insert(key, folder);
        }
    }
    Ok(merged
        .into_values()
        .skip(offset.max(0) as usize)
        .take(limit.clamp(1, 500) as usize)
        .collect())
}

pub fn folder_by_path(db: &Db, path: &str) -> rusqlite::Result<Option<Folder>> {
    let conn = db.lock().unwrap();
    conn.query_row("SELECT id,parent_id,root_id,name,path,(SELECT count(*) FROM assets WHERE folder_id=folders.id AND is_listed=1) FROM folders WHERE lower(rtrim(replace(path,'/','\\'),'\\'))=lower(rtrim(replace(?,'/','\\'),'\\')) LIMIT 1", [path], |r| Ok(Folder { id:r.get(0)?,parent_id:r.get(1)?,root_id:r.get(2)?,name:r.get(3)?,path:r.get(4)?,count:r.get(5)?,folder_ids:vec![r.get(0)?],virtual_node:None,offline:false,missing:false })).optional()
}

pub fn folders_by_paths(db: &Db, paths: &[String]) -> rusqlite::Result<HashMap<String, Folder>> {
    let conn = db.lock().unwrap();
    let mut result = HashMap::new();
    for chunk in paths.chunks(500) {
        let values = vec!["(?)"; chunk.len()].join(",");
        let sql = format!(
            "WITH requested(path) AS (VALUES {values}) SELECT requested.path,folders.id,folders.parent_id,folders.root_id,folders.name,folders.path,(SELECT count(*) FROM assets WHERE folder_id=folders.id AND is_listed=1) FROM requested JOIN folders ON lower(rtrim(replace(folders.path,'/','\\'),'\\'))=lower(rtrim(replace(requested.path,'/','\\'),'\\')) ORDER BY folders.rowid"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |r| {
            let id = r.get(1)?;
            Ok((
                r.get::<_, String>(0)?,
                Folder {
                    id,
                    parent_id: r.get(2)?,
                    root_id: r.get(3)?,
                    name: r.get(4)?,
                    path: r.get(5)?,
                    count: r.get(6)?,
                    folder_ids: vec![id],
                    virtual_node: None,
                    offline: false,
                    missing: false,
                },
            ))
        })?;
        for row in rows {
            let (path, folder) = row?;
            result.entry(path).or_insert(folder);
        }
    }
    Ok(result)
}

pub fn folder_ancestors(db: &Db, id: i64) -> rusqlite::Result<Vec<Folder>> {
    let conn = db.lock().unwrap();
    let mut stmt=conn.prepare("WITH RECURSIVE chain(id,parent_id,root_id,name,path,depth) AS (SELECT id,parent_id,root_id,name,path,0 FROM folders WHERE id=? UNION ALL SELECT f.id,f.parent_id,f.root_id,f.name,f.path,c.depth+1 FROM folders f JOIN chain c ON f.id=c.parent_id) SELECT c.id,c.parent_id,c.root_id,c.name,c.path,(SELECT count(*) FROM assets a WHERE a.folder_id=c.id AND a.is_listed=1) FROM chain c ORDER BY c.depth DESC")?;
    let mut rows = stmt.query([id])?;
    let mut result = Vec::new();
    while let Some(r) = rows.next()? {
        let folder_id = r.get(0)?;
        result.push(Folder {
            id: folder_id,
            parent_id: r.get(1)?,
            root_id: r.get(2)?,
            name: r.get(3)?,
            path: r.get(4)?,
            count: r.get(5)?,
            folder_ids: vec![folder_id],
            virtual_node: None,
            offline: false,
            missing: false,
        });
    }
    Ok(result)
}
