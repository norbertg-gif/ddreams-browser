use super::open::{Db, has_category_icon_column};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub root_id: Option<i64>,
    pub name: String,
    pub count: i64,
    #[serde(default)]
    pub has_children: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub id: i64,
    pub name: String,
    pub count: i64,
    #[serde(default)]
    pub merged_category_id: Option<i64>,
    #[serde(default)]
    pub has_children: bool,
    #[serde(default)]
    pub icon_key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAssetLinks {
    pub category_ids: Vec<i64>,
    pub category_paths: Vec<CategoryPath>,
    pub category_ancestor_ids: Vec<i64>,
    pub keyword_ids: Vec<i64>,
    pub keyword_paths: Vec<CategoryPath>,
    pub keyword_ancestor_ids: Vec<i64>,
    pub root_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CategoryPath {
    pub id: i64,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeywordOption {
    pub id: i64,
    pub name: String,
    pub path: String,
}

pub(crate) fn category_root_matches(conn: &Connection, root_id: i64) -> rusqlite::Result<Option<i64>> {
    let root_name: String = conn.query_row("SELECT name FROM category_roots WHERE id=?", [root_id], |r| r.get(0))?;
    let mut stmt =
        conn.prepare("SELECT id,name FROM categories WHERE root_id=? AND (parent_id IS NULL OR parent_id=0)")?;
    let candidates = stmt
        .query_map([root_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let wanted = root_name.trim().to_lowercase();
    Ok(candidates
        .into_iter()
        .find(|(_, name)| name.trim().to_lowercase() == wanted)
        .map(|(id, _)| id))
}

pub fn roots_page(db: &Db, offset: i64, limit: i64, include_children: bool) -> rusqlite::Result<Vec<Root>> {
    let conn = db.lock().unwrap();
    // Read-only startup also supports catalogues that have never been migrated.
    let root_icon = if has_category_icon_column(&conn, "category_roots")? {
        "icon_key"
    } else {
        "NULL"
    };
    let category_icon = if has_category_icon_column(&conn, "categories")? {
        "c.icon_key"
    } else {
        "NULL"
    };
    let mut roots = conn.prepare(&format!(
        "SELECT id,name,{root_icon} FROM category_roots ORDER BY name COLLATE NOCASE,id LIMIT ? OFFSET ?"
    ))?;
    let rows = roots
        .query_map(params![limit.clamp(1, 500), offset.max(0)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut top_stmt=conn.prepare(&format!("SELECT c.id,c.root_id,c.name,EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=c.id),{category_icon} FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0"))?;
    let top = top_stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut seeds = Vec::<(i64, i64)>::new();
    let mut root_data = Vec::with_capacity(rows.len());
    for (id, name, root_icon) in rows {
        let candidates = top.iter().filter(|item| item.1 == id).collect::<Vec<_>>();
        let wanted = name.trim().to_lowercase();
        let paired = candidates.iter().find(|item| item.2.trim().to_lowercase() == wanted);
        let merged = paired.map(|item| item.0);
        let icon_key = root_icon.or_else(|| paired.and_then(|item| item.4.clone()));
        if let Some(category_id) = merged {
            seeds.push((id, category_id));
        } else {
            seeds.extend(candidates.iter().map(|item| (id, item.0)));
        }
        let has_children = if let Some(category_id) = merged {
            candidates.iter().any(|item| item.0 != category_id)
                || candidates.iter().any(|item| item.0 == category_id && item.3)
        } else {
            !candidates.is_empty()
        };
        root_data.push((id, name, merged, has_children, icon_key));
    }
    let mut counts = std::collections::HashMap::<i64, i64>::new();
    if !seeds.is_empty() {
        let values = vec!["(?,?)"; seeds.len()].join(",");
        let recursion = if include_children {
            "WITH RECURSIVE root_seeds(root_id,seed_id) AS (VALUES __VALUES__), d(root_id,seed_id,id) AS (SELECT root_id,seed_id,seed_id FROM root_seeds UNION SELECT d.root_id,d.seed_id,c.id FROM d JOIN categories c ON c.parent_id=d.id AND c.root_id=d.root_id)"
        } else {
            "WITH root_seeds(root_id,seed_id) AS (VALUES __VALUES__), d(root_id,seed_id,id) AS (SELECT root_id,seed_id,seed_id FROM root_seeds)"
        };
        let sql = format!(
            "{} SELECT d.root_id,count(DISTINCT ac.asset_id) FROM d CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=d.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY d.root_id",
            recursion.replace("__VALUES__", &values)
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::with_capacity(seeds.len() * 2);
        for (root_id, seed_id) in seeds {
            args.push(root_id.into());
            args.push(seed_id.into());
        }
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (id, count) = row?;
            counts.insert(id, count);
        }
    }
    let mut result = Vec::with_capacity(root_data.len());
    for (id, name, merged_category_id, has_children, icon_key) in root_data {
        result.push(Root {
            id,
            name,
            count: *counts.get(&id).unwrap_or(&0),
            merged_category_id,
            has_children,
            icon_key,
        });
    }
    Ok(result)
}

pub fn nodes_page(
    db: &Db,
    table: &str,
    parent_id: Option<i64>,
    root_id: Option<i64>,
    offset: i64,
    limit: i64,
    include_children: bool,
) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    if table == "category" {
        let icon = if has_category_icon_column(&conn, "categories")? {
            "n.icon_key"
        } else {
            "NULL"
        };
        let root_value = root_id
            .map(rusqlite::types::Value::Integer)
            .unwrap_or(rusqlite::types::Value::Null);
        if parent_id.is_none() {
            if let Some(root) = root_id {
                if let Some(merged_id) = category_root_matches(&conn, root)? {
                    let (sql, args): (String, Vec<rusqlite::types::Value>) = if include_children {
                        (
                            format!(
                                "WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM categories WHERE root_id=? UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id WHERE c.root_id=?), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,n.root_id,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id),{icon} FROM categories n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE (n.parent_id=? OR ((n.parent_id IS NULL OR n.parent_id=0) AND n.id<>?)) AND n.root_id=? ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?"
                            ),
                            vec![
                                root.into(),
                                root.into(),
                                merged_id.into(),
                                merged_id.into(),
                                root.into(),
                                limit.clamp(1, 500).into(),
                                offset.max(0).into(),
                            ],
                        )
                    } else {
                        (
                            format!(
                                "SELECT n.id,n.parent_id,n.root_id,n.name,count(DISTINCT ac.asset_id),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id),{icon} FROM categories n LEFT JOIN asset_categories ac ON ac.category_id=n.id LEFT JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 WHERE (n.parent_id=? OR ((n.parent_id IS NULL OR n.parent_id=0) AND n.id<>?)) AND n.root_id=? GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?"
                            ),
                            vec![
                                merged_id.into(),
                                merged_id.into(),
                                root.into(),
                                limit.clamp(1, 500).into(),
                                offset.max(0).into(),
                            ],
                        )
                    };
                    let mut stmt = conn.prepare(&sql)?;
                    return stmt
                        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                            Ok(CatalogNode {
                                id: r.get(0)?,
                                parent_id: r.get(1)?,
                                root_id: r.get(2)?,
                                name: r.get(3)?,
                                count: r.get(4)?,
                                has_children: r.get(5)?,
                                icon_key: r.get(6)?,
                            })
                        })?
                        .collect();
                }
            }
        }
        let (sql, args): (String, Vec<rusqlite::types::Value>) = if include_children {
            (
                format!(
                    "WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM categories WHERE (? IS NULL OR root_id=?) UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id WHERE (? IS NULL OR c.root_id=?)), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,n.root_id,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id),{icon} FROM categories n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE coalesce(n.parent_id,0)=? AND (? IS NULL OR n.root_id=?) ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?"
                ),
                vec![
                    root_value.clone(),
                    root_value.clone(),
                    root_value.clone(),
                    root_value.clone(),
                    parent_id.unwrap_or(0).into(),
                    root_value.clone(),
                    root_value.clone(),
                    limit.clamp(1, 500).into(),
                    offset.max(0).into(),
                ],
            )
        } else {
            (
                format!(
                    "SELECT n.id,n.parent_id,n.root_id,n.name,count(DISTINCT ac.asset_id),EXISTS(SELECT 1 FROM categories child WHERE child.parent_id=n.id),{icon} FROM categories n LEFT JOIN asset_categories ac ON ac.category_id=n.id LEFT JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 JOIN (SELECT ? AS parent_filter, ? AS root_filter) p WHERE coalesce(n.parent_id,0)=p.parent_filter AND (p.root_filter IS NULL OR n.root_id=p.root_filter) GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?"
                ),
                vec![
                    parent_id.unwrap_or(0).into(),
                    root_value,
                    limit.clamp(1, 500).into(),
                    offset.max(0).into(),
                ],
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(CatalogNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                root_id: r.get(2)?,
                name: r.get(3)?,
                count: r.get(4)?,
                has_children: r.get(5)?,
                icon_key: r.get(6)?,
            })
        })?
        .collect()
    } else {
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = if include_children {
            (
                "WITH RECURSIVE closure(ancestor_id,id) AS (SELECT id,id FROM keywords UNION SELECT closure.ancestor_id,k.id FROM closure JOIN keywords k ON k.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ak.asset_id) n FROM closure CROSS JOIN asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset CROSS JOIN assets a WHERE ak.keyword_id=closure.id AND a.id=ak.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT n.id,n.parent_id,NULL,n.name,coalesce(counts.n,0),EXISTS(SELECT 1 FROM keywords child WHERE child.parent_id=n.id) FROM keywords n LEFT JOIN counts ON counts.ancestor_id=n.id WHERE coalesce(n.parent_id,0)=? ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![
                    parent_id.unwrap_or(0).into(),
                    limit.clamp(1, 500).into(),
                    offset.max(0).into(),
                ],
            )
        } else {
            (
                "SELECT n.id,n.parent_id,NULL,n.name,count(DISTINCT ak.asset_id),EXISTS(SELECT 1 FROM keywords child WHERE child.parent_id=n.id) FROM keywords n LEFT JOIN asset_keywords ak ON ak.keyword_id=n.id WHERE coalesce(n.parent_id,0)=? GROUP BY n.id ORDER BY n.name COLLATE NOCASE,n.id LIMIT ? OFFSET ?",
                vec![
                    parent_id.unwrap_or(0).into(),
                    limit.clamp(1, 500).into(),
                    offset.max(0).into(),
                ],
            )
        };
        let mut stmt = conn.prepare(sql)?;
        stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(CatalogNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                root_id: None,
                name: r.get(3)?,
                count: r.get(4)?,
                has_children: r.get(5)?,
                icon_key: None,
            })
        })?
        .collect()
    }
}

fn escape_like(query: &str) -> String {
    query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn keyword_search_page(db: &Db, query: &str, offset: i64, limit: i64) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    let mut stmt=conn.prepare("SELECT k.id,k.parent_id,NULL,k.name,(SELECT count(*) FROM asset_keywords ak WHERE ak.keyword_id=k.id) FROM keywords k WHERE k.name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY k.name COLLATE NOCASE,k.id LIMIT ? OFFSET ?")?;
    stmt.query_map(
        params![format!("%{}%", escape_like(query)), limit.clamp(1, 500), offset.max(0)],
        |r| {
            Ok(CatalogNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                root_id: None,
                name: r.get(3)?,
                count: r.get(4)?,
                has_children: false,
                icon_key: None,
            })
        },
    )?
    .collect()
}

pub fn catalog_tree_search(
    db: &Db,
    kind: &str,
    query: &str,
    offset: i64,
    limit: i64,
    include_children: bool,
) -> rusqlite::Result<Vec<CatalogNode>> {
    let conn = db.lock().unwrap();
    let sql = if kind == "category" {
        if include_children {
            "WITH RECURSIVE t(id,parent_id,root_id,name,path,ancestry) AS (SELECT c.id,c.parent_id,c.root_id,c.name,coalesce((SELECT r.name FROM category_roots r WHERE r.id=c.root_id)||' > ','')||c.name,','||c.id||',' FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0 UNION SELECT n.id,n.parent_id,n.root_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM categories n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0), closure(ancestor_id,id) AS (SELECT id,id FROM categories UNION SELECT closure.ancestor_id,c.id FROM closure JOIN categories c ON c.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ac.asset_id) n FROM closure CROSS JOIN asset_categories ac INDEXED BY idx_asset_categories_category_asset CROSS JOIN assets a WHERE ac.category_id=closure.id AND a.id=ac.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT t.id,t.parent_id,t.root_id,t.path,coalesce(counts.n,0) FROM t LEFT JOIN counts ON counts.ancestor_id=t.id WHERE t.name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
        } else {
            "WITH RECURSIVE t(id,parent_id,root_id,name,path,ancestry) AS (SELECT c.id,c.parent_id,c.root_id,c.name,coalesce((SELECT r.name FROM category_roots r WHERE r.id=c.root_id)||' > ','')||c.name,','||c.id||',' FROM categories c WHERE c.parent_id IS NULL OR c.parent_id=0 UNION SELECT n.id,n.parent_id,n.root_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM categories n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0) SELECT t.id,t.parent_id,t.root_id,t.path,(SELECT count(DISTINCT ac.asset_id) FROM asset_categories ac JOIN assets a ON a.id=ac.asset_id AND a.is_listed=1 WHERE ac.category_id=t.id) FROM t WHERE t.name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
        }
    } else if include_children {
        "WITH RECURSIVE t(id,parent_id,name,path,ancestry) AS (SELECT id,parent_id,name,name,','||id||',' FROM keywords WHERE parent_id IS NULL OR parent_id=0 UNION SELECT n.id,n.parent_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM keywords n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0), closure(ancestor_id,id) AS (SELECT id,id FROM keywords UNION SELECT closure.ancestor_id,k.id FROM closure JOIN keywords k ON k.parent_id=closure.id), counts AS (SELECT closure.ancestor_id,count(DISTINCT ak.asset_id) n FROM closure CROSS JOIN asset_keywords ak INDEXED BY idx_asset_keywords_keyword_asset CROSS JOIN assets a WHERE ak.keyword_id=closure.id AND a.id=ak.asset_id AND a.is_listed=1 GROUP BY closure.ancestor_id) SELECT t.id,t.parent_id,NULL,t.path,coalesce(counts.n,0) FROM t LEFT JOIN counts ON counts.ancestor_id=t.id WHERE t.name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
    } else {
        "WITH RECURSIVE t(id,parent_id,name,path,ancestry) AS (SELECT id,parent_id,name,name,','||id||',' FROM keywords WHERE parent_id IS NULL OR parent_id=0 UNION SELECT n.id,n.parent_id,n.name,t.path||' > '||n.name,t.ancestry||n.id||',' FROM keywords n JOIN t ON n.parent_id=t.id WHERE instr(t.ancestry,','||n.id||',')=0) SELECT t.id,t.parent_id,NULL,t.path,(SELECT count(DISTINCT ak.asset_id) FROM asset_keywords ak JOIN assets a ON a.id=ak.asset_id AND a.is_listed=1 WHERE ak.keyword_id=t.id) FROM t WHERE t.name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY t.path COLLATE NOCASE LIMIT ? OFFSET ?"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut nodes = stmt
        .query_map(
            params![format!("%{}%", escape_like(query)), limit.clamp(1, 500), offset.max(0)],
            |r| {
                Ok(CatalogNode {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    root_id: r.get(2)?,
                    name: r.get(3)?,
                    count: r.get(4)?,
                    has_children: false,
                    icon_key: None,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if kind == "category" {
        for node in &mut nodes {
            let mut parts = node.name.split(" > ").collect::<Vec<_>>();
            if parts.len() > 1 && parts[0].trim().to_lowercase() == parts[1].trim().to_lowercase() {
                parts.remove(1);
                node.name = parts.join(" > ");
            }
        }
    }
    Ok(nodes)
}

pub fn asset_catalog_links(db: &Db, id: i64) -> rusqlite::Result<Option<CatalogAssetLinks>> {
    let conn = db.lock().unwrap();
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?)", [id], |r| r.get(0))?;
    if !exists {
        return Ok(None);
    }
    let category_ids = conn
        .prepare("SELECT category_id FROM asset_categories WHERE asset_id=? ORDER BY category_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut category_paths = Vec::with_capacity(category_ids.len());
    for category_id in &category_ids {
        let (root, root_name): (Option<i64>, Option<String>) = conn.query_row(
            "SELECT c.root_id,r.name FROM categories c LEFT JOIN category_roots r ON r.id=c.root_id WHERE c.id=?",
            [category_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut names = Vec::new();
        let mut current = Some(*category_id);
        let mut seen = HashSet::new();
        while let Some(cid) = current {
            if !seen.insert(cid) {
                break;
            }
            let row: Option<(String, Option<i64>)> = conn
                .query_row("SELECT name,parent_id FROM categories WHERE id=?", [cid], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let Some((name, parent)) = row else { break };
            names.push(name);
            current = parent;
        }
        names.reverse();
        if let Some(root_name) = root_name {
            if !names
                .first()
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(root_name.trim()))
            {
                names.insert(0, root_name);
            }
        } else if root.is_some() && names.is_empty() {
            continue;
        }
        category_paths.push(CategoryPath {
            id: *category_id,
            path: names.join(" › "),
        });
    }
    let keyword_ids = conn
        .prepare("SELECT keyword_id FROM asset_keywords WHERE asset_id=? ORDER BY keyword_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut keyword_paths = Vec::with_capacity(keyword_ids.len());
    for keyword_id in &keyword_ids {
        let mut names = Vec::new();
        let mut current = Some(*keyword_id);
        let mut seen = HashSet::new();
        while let Some(key) = current {
            if !seen.insert(key) {
                break;
            }
            let row: Option<(String, Option<i64>)> = conn
                .query_row("SELECT name,parent_id FROM keywords WHERE id=?", [key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let Some((name, parent)) = row else { break };
            names.push(name);
            current = parent;
        }
        names.reverse();
        keyword_paths.push(CategoryPath {
            id: *keyword_id,
            path: names.join(" › "),
        });
    }
    let category_ancestor_ids = conn.prepare("WITH RECURSIVE ancestors(id) AS (SELECT parent_id FROM categories WHERE id IN (SELECT category_id FROM asset_categories WHERE asset_id=?) AND parent_id IS NOT NULL UNION SELECT c.parent_id FROM categories c JOIN ancestors a ON c.id=a.id WHERE c.parent_id IS NOT NULL) SELECT id FROM ancestors ORDER BY id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let keyword_ancestor_ids = conn.prepare("WITH RECURSIVE ancestors(id) AS (SELECT parent_id FROM keywords WHERE id IN (SELECT keyword_id FROM asset_keywords WHERE asset_id=?) AND parent_id IS NOT NULL UNION SELECT k.parent_id FROM keywords k JOIN ancestors a ON k.id=a.id WHERE k.parent_id IS NOT NULL) SELECT id FROM ancestors ORDER BY id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let root_ids = conn.prepare("SELECT DISTINCT c.root_id FROM asset_categories ac JOIN categories c ON c.id=ac.category_id WHERE ac.asset_id=? AND c.root_id IS NOT NULL ORDER BY c.root_id")?
        .query_map([id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(CatalogAssetLinks {
        category_ids,
        category_paths,
        category_ancestor_ids,
        keyword_ids,
        keyword_paths,
        keyword_ancestor_ids,
        root_ids,
    }))
}

pub fn keyword_options(db: &Db, query: &str) -> rusqlite::Result<Vec<KeywordOption>> {
    let conn = db.lock().unwrap();
    let rows = conn
        .prepare("SELECT id,parent_id,name FROM keywords ORDER BY id")?
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let by_id = rows
        .iter()
        .map(|(id, parent, name)| (*id, (*parent, name.clone())))
        .collect::<HashMap<_, _>>();
    let needle = query.trim().to_lowercase();
    let mut found = Vec::new();
    for (id, _, name) in rows {
        if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
            continue;
        }
        let mut current = Some(id);
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        while let Some(key) = current {
            if !seen.insert(key) {
                break;
            }
            let Some((parent, label)) = by_id.get(&key) else { break };
            names.push(label.clone());
            current = *parent;
        }
        names.reverse();
        found.push((
            !name.to_lowercase().starts_with(&needle),
            name.to_lowercase(),
            KeywordOption {
                id,
                name,
                path: names.join(" › "),
            },
        ));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.path.cmp(&b.2.path)));
    Ok(found.into_iter().take(30).map(|(_, _, item)| item).collect())
}
