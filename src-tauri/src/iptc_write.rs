use crate::{
    db::Db,
    exiftool::ExifTool,
    file_ops::{FileSystemOps, NativeFileSystem, sidecar_candidates},
    formats::FormatTable,
    journal::{AssetTextField, keyword_components},
    listing::ListingService,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::SystemTime,
};

const DESCRIPTION: [&str; 2] = ["XMP-dc:Description", "IPTC:Caption-Abstract"];
const CREATOR: [&str; 2] = ["XMP-dc:Creator", "IPTC:By-line"];
const KEYWORDS: [&str; 2] = ["XMP-dc:Subject", "IPTC:Keywords"];
const HIERARCHY: &str = "XMP-lr:HierarchicalSubject";
const SHARED: &str = "Spoločný sidecar pre viac súborov";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct CatalogPhoto {
    pub id: i64,
    #[serde(skip)]
    path: Option<String>,
    #[serde(skip)]
    ext: String,
    caption: Option<String>,
    author: Option<String>,
    keywords: Vec<Vec<String>>,
    #[serde(skip)]
    keyword_ids: Vec<i64>,
}

pub fn catalog_photos(
    db: &Db,
    listing: &ListingService,
    formats: &FormatTable,
    ids: &[i64],
) -> Result<Vec<CatalogPhoto>, String> {
    let temporary: HashMap<_, _> = ids
        .iter()
        .filter(|&&id| id < 0)
        .filter_map(|&id| listing.asset(id).map(|asset| (id, asset)))
        .collect();
    let conn = db.lock().map_err(|e| e.to_string())?;
    let base = "SELECT id,path,ext,caption,author FROM assets WHERE is_listed=1 AND ext_lower<>''";
    let mut by_id = conn.prepare(&format!("{base} AND id=?1")).map_err(|e| e.to_string())?;
    let mut by_path = conn
        .prepare(&format!("{base} AND path=?1 COLLATE NOCASE ORDER BY id LIMIT 1"))
        .map_err(|e| e.to_string())?;
    let map = |r: &rusqlite::Row<'_>| {
        Ok(CatalogPhoto {
            id: r.get(0)?,
            path: r.get(1)?,
            ext: r.get(2)?,
            caption: r.get(3)?,
            author: r.get(4)?,
            keywords: Vec::new(),
            keyword_ids: Vec::new(),
        })
    };
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        let photo = if id > 0 {
            by_id.query_row([id], map).optional().map_err(|e| e.to_string())?
        } else if let Some(asset) = temporary.get(&id).filter(|asset| !asset.is_directory) {
            let mut catalogued = None;
            if let Some(path) = &asset.path {
                catalogued = by_path.query_row([path], map).optional().map_err(|e| e.to_string())?;
            }
            catalogued.or_else(|| {
                Some(CatalogPhoto {
                    id,
                    path: asset.path.clone(),
                    ext: asset.ext.clone(),
                    caption: asset.caption.clone(),
                    author: asset.author.clone(),
                    keywords: Vec::new(),
                    keyword_ids: Vec::new(),
                })
            })
        } else {
            None
        };
        let Some(mut photo) = photo.filter(|photo| is_photo(formats, &photo.ext)) else {
            continue;
        };
        if photo.id > 0 {
            (photo.keyword_ids, photo.keywords) = catalog_keywords(&conn, photo.id)?;
        }
        photo.id = id;
        result.push(photo);
    }
    Ok(result)
}

fn catalog_keywords(conn: &Connection, id: i64) -> Result<(Vec<i64>, Vec<Vec<String>>), String> {
    let mut stmt = conn
        .prepare("SELECT keyword_id FROM asset_keywords WHERE asset_id=?1 ORDER BY keyword_id")
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let ids = ids.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    let paths = ids
        .iter()
        .map(|&id| keyword_components(conn, id))
        .collect::<Result<_, _>>()?;
    Ok((ids, paths))
}

fn is_photo(formats: &FormatTable, ext: &str) -> bool {
    matches!(formats.group(ext), Some("image" | "raw"))
}

fn embedded_file_type(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("JPEG"),
        "tif" | "tiff" => Some("TIFF"),
        // DNG is the only RAW format authorized for embedded writes.
        "dng" => Some("DNG"),
        _ => None,
    }
}

fn ext(path: &Path) -> &str {
    path.extension().and_then(|value| value.to_str()).unwrap_or("")
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    canonical: PathBuf,
}

fn stamp(path: &Path) -> Result<Option<Stamp>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Cieľ nie je bežný súbor".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_OFFLINE;
        if metadata.file_attributes() & FILE_ATTRIBUTE_OFFLINE.0 != 0 {
            return Err("Súbor je offline".into());
        }
    }
    Ok(Some(Stamp {
        size: metadata.len(),
        modified: metadata.modified().map_err(|e| e.to_string())?,
        canonical: path.canonicalize().map_err(|e| e.to_string())?,
    }))
}

fn sidecar_target(source: &Path, formats: &FormatTable) -> Result<(PathBuf, Option<Stamp>), String> {
    let candidates = sidecar_candidates(source);
    let [base, qualified] = candidates.as_slice() else {
        return Err("Názov XMP súboru nie je dostupný".into());
    };
    let base_stamp = stamp(base)?;
    let qualified_stamp = stamp(qualified)?;
    if base_stamp.is_none() && qualified_stamp.is_some() {
        return Ok((qualified.clone(), qualified_stamp));
    }
    let parent = source.parent().ok_or("Priečinok súboru nie je dostupný")?;
    let canonical = source.canonicalize().map_err(|e| e.to_string())?;
    // Also look outside the selection. Creating a shared basename would be ambiguous later.
    for entry in fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let other = entry.path();
        let same_stem = other
            .file_stem()
            .zip(source.file_stem())
            .is_some_and(|(a, b)| a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase());
        if same_stem
            && other.file_name() != canonical.file_name()
            && is_photo(formats, ext(&other))
            && embedded_file_type(ext(&other)).is_none()
            && !entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            return Err(SHARED.into());
        }
    }
    if qualified_stamp.is_some() {
        return Err("Pre fotku existujú dva XMP sidecary".into());
    }
    Ok((base.clone(), base_stamp))
}

fn matches_tag(key: &str, tag: &str) -> bool {
    let (group, name) = tag.split_once(':').unwrap();
    let mut parts = key.split(':');
    let Some(actual_name) = parts.next_back() else {
        return false;
    };
    (actual_name == name || (tag == DESCRIPTION[0] && actual_name.starts_with("Description-")))
        && parts.any(|part| part == group)
}

fn values(value: &Value) -> impl Iterator<Item = &str> {
    value
        .as_str()
        .into_iter()
        .chain(value.as_array().into_iter().flatten().filter_map(Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

fn tag_values<'a>(metadata: &'a Value, tag: &'a str) -> impl Iterator<Item = &'a str> {
    metadata
        .as_object()
        .into_iter()
        .flatten()
        .filter(move |(key, _)| matches_tag(key, tag))
        .flat_map(|(_, value)| values(value))
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum EditKind {
    Set,
    Append,
    Remove,
}

#[derive(Debug, PartialEq, Eq)]
struct Edit {
    tag: &'static str,
    value: String,
    kind: EditKind,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Merge {
    edits: Vec<Edit>,
    warnings: Vec<String>,
}

fn safe_iptc(metadata: &Value) -> bool {
    // Relabelling a legacy IPTC block as UTF-8 can corrupt unrelated tags.
    let existing_iptc = metadata
        .as_object()
        .is_some_and(|tags| tags.keys().any(|key| key.split(':').any(|part| part == "IPTC")));
    !existing_iptc || tag_values(metadata, "IPTC:CodedCharacterSet").any(|value| value.eq_ignore_ascii_case("UTF8"))
}

fn merge(photo: &CatalogPhoto, metadata: &Value, embedded: bool) -> Merge {
    let mut result = Merge::default();
    let safe_iptc = safe_iptc(metadata);
    let mut legacy_warning = false;
    let mut add = |tag: &'static str, value: &str, append: bool, limit: usize| {
        if value.contains('\0') || (append && value.contains(['\r', '\n'])) {
            result
                .warnings
                .push(format!("{tag} obsahuje nepovolený znak: {value:?}"));
        } else if tag.starts_with("IPTC:") && !safe_iptc {
            legacy_warning = true;
        } else if value.len() > limit {
            result
                .warnings
                .push(format!("{tag} presahuje {limit} bajtov, hodnota sa zapíše iba do XMP"));
        } else {
            let kind = if append { EditKind::Append } else { EditKind::Set };
            result.edits.push(Edit {
                tag,
                value: value.replace("\r\n", "\n").replace('\r', "\n"),
                kind,
            });
        }
    };
    for (tags, value, limit) in [
        (DESCRIPTION, photo.caption.as_deref(), 2000),
        (CREATOR, photo.author.as_deref(), 32),
    ] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        if tags.iter().any(|tag| tag_values(metadata, tag).next().is_some()) {
            continue;
        }
        add(tags[0], value, false, usize::MAX);
        if embedded {
            add(tags[1], value, false, limit);
        }
    }
    let mut invalid_hierarchy = false;
    for (tag, limit) in [(KEYWORDS[0], usize::MAX), (KEYWORDS[1], 64), (HIERARCHY, usize::MAX)] {
        if tag == KEYWORDS[1] && !embedded {
            continue;
        }
        let mut present: HashSet<_> = tag_values(metadata, tag)
            .map(|value| value.trim().to_lowercase())
            .collect();
        for path in &photo.keywords {
            let Some(leaf) = path.last() else { continue };
            let value = if tag == HIERARCHY {
                if path.iter().any(|part| part.contains('|')) {
                    invalid_hierarchy = true;
                    continue;
                }
                path.join("|")
            } else {
                leaf.to_owned()
            };
            if !value.trim().is_empty() && present.insert(value.trim().to_lowercase()) {
                add(tag, &value, true, limit);
            }
        }
    }
    if legacy_warning {
        result
            .warnings
            .push("Existujúce kódovanie IPTC nie je UTF-8. Doplní sa iba XMP.".into());
    }
    if invalid_hierarchy {
        result
            .warnings
            .push("Cesta s názvom obsahujúcim znak | sa do hierarchie XMP nezapíše.".into());
    }
    result
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub enum ManualEdit {
    Text {
        field: AssetTextField,
        #[serde(deserialize_with = "explicit_text_value")]
        value: Option<String>,
    },
    RemoveKeyword {
        keyword: String,
    },
    AddKeywords,
}

fn explicit_text_value<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(value)
}

fn keyword_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn file_keywords(metadata: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    KEYWORDS
        .iter()
        .flat_map(|tag| tag_values(metadata, tag))
        .filter(|value| seen.insert(keyword_key(value)))
        .map(str::to_owned)
        .collect()
}

fn exact_tag(key: &str, tag: &str) -> bool {
    let name = tag.split_once(':').unwrap().1;
    key.rsplit(':').next() == Some(name) && matches_tag(key, tag)
}

fn text_value(metadata: &Value, tags: [&str; 2], multiple: bool) -> Option<String> {
    let preferred: Vec<_> = metadata
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| exact_tag(key, tags[0]))
        .flat_map(|(_, value)| values(value))
        .collect();
    let values = if !preferred.is_empty() {
        preferred
    } else {
        let alternatives = tag_values(metadata, tags[0]).collect::<Vec<_>>();
        if alternatives.is_empty() {
            tag_values(metadata, tags[1]).collect()
        } else {
            alternatives
        }
    };
    if multiple && !values.is_empty() {
        Some(values.join("; "))
    } else {
        values.first().map(|value| (*value).to_owned())
    }
}

fn manual_merge(photo: &CatalogPhoto, metadata: &Value, embedded: bool, action: &ManualEdit) -> Result<Merge, String> {
    if matches!(action, ManualEdit::AddKeywords) {
        let present: HashSet<_> = file_keywords(metadata).iter().map(|value| keyword_key(value)).collect();
        let mut additions = photo.clone();
        additions.caption = None;
        additions.author = None;
        additions
            .keywords
            .retain(|path| path.last().is_some_and(|leaf| !present.contains(&keyword_key(leaf))));
        return Ok(merge(&additions, metadata, embedded));
    }
    let mut result = Merge::default();
    let safe_iptc = safe_iptc(metadata);
    let mut warn_legacy = false;
    let mut add = |tag: &'static str, value: &str, kind, limit| {
        if tag.starts_with("IPTC:") && !safe_iptc {
            warn_legacy = true;
        } else if value.len() > limit {
            result
                .warnings
                .push(format!("{tag} presahuje {limit} bajtov, hodnota sa zapíše iba do XMP"));
        } else {
            result.edits.push(Edit {
                tag,
                value: value.to_owned(),
                kind,
            });
        }
    };
    match action {
        ManualEdit::Text { field, value } => {
            let (tags, limit) = match field {
                AssetTextField::Caption => (DESCRIPTION, 2000),
                AssetTextField::Author => (CREATOR, 32),
            };
            let value = value.as_deref().unwrap_or_default();
            for (tag, limit) in [(tags[0], usize::MAX), (tags[1], limit)]
                .into_iter()
                .take(if embedded { 2 } else { 1 })
            {
                let current: Vec<_> = tag_values(metadata, tag).collect();
                let already_set = current == [value]
                    && metadata.as_object().is_some_and(|tags| {
                        tags.keys()
                            .filter(|key| matches_tag(key, tag))
                            .all(|key| exact_tag(key, tag))
                    });
                let absent = !metadata
                    .as_object()
                    .is_some_and(|tags| tags.keys().any(|key| matches_tag(key, tag)));
                if (value.is_empty() && !absent) || (!value.is_empty() && !already_set) {
                    add(tag, value, EditKind::Set, limit);
                }
            }
        }
        ManualEdit::RemoveKeyword { keyword } => {
            let key = keyword_key(keyword);
            for tag in KEYWORDS.into_iter().chain([HIERARCHY]) {
                if tag == KEYWORDS[1] && !embedded {
                    continue;
                }
                let mut seen = HashSet::new();
                for value in tag_values(metadata, tag) {
                    let leaf = if tag == HIERARCHY {
                        value.rsplit('|').next().unwrap_or(value)
                    } else {
                        value
                    };
                    if keyword_key(leaf) == key && seen.insert(value) {
                        if value.contains('\0') {
                            return Err("Kľúčové slovo obsahuje nepovolený znak".into());
                        }
                        add(tag, value, EditKind::Remove, usize::MAX);
                    }
                }
            }
        }
        ManualEdit::AddKeywords => unreachable!(),
    }
    if warn_legacy {
        result
            .warnings
            .push("Existujúce kódovanie IPTC nie je UTF-8. Upraviť možno iba XMP.".into());
    }
    Ok(result)
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct Issue {
    path: String,
    message: String,
}

impl Issue {
    fn new(path: &Path, message: String) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            message,
        }
    }

    fn for_photo(photo: &CatalogPhoto, message: String) -> Self {
        Self {
            path: photo.path.clone().unwrap_or_else(|| format!("ID {}", photo.id)),
            message,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct EditorPhoto {
    #[serde(flatten)]
    catalog: CatalogPhoto,
    file: Option<EditorFile>,
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
struct EditorFile {
    caption: Option<String>,
    author: Option<String>,
    keywords: Vec<String>,
    revision: String,
}

pub fn editor_photos(photos: Vec<CatalogPhoto>, exif: &ExifTool, formats: &FormatTable) -> Vec<EditorPhoto> {
    photos
        .into_iter()
        .map(|catalog| {
            let file = (|| {
                let source = catalog.path.as_deref().ok_or("Fotka nemá cestu")?;
                let job = Job::read(catalog.clone(), PathBuf::from(source), exif, formats, true)?;
                Ok::<_, String>(EditorFile {
                    caption: text_value(&job.before, DESCRIPTION, false),
                    author: text_value(&job.before, CREATOR, true),
                    keywords: file_keywords(&job.before),
                    revision: job.revision(),
                })
            })();
            match file {
                Ok(file) => EditorPhoto {
                    catalog,
                    file: Some(file),
                    reason: None,
                },
                Err(reason) => EditorPhoto {
                    catalog,
                    file: None,
                    reason: Some(reason),
                },
            }
        })
        .collect()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct EditorTarget {
    pub id: i64,
    revision: String,
}

#[derive(Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct PlanSummary {
    pub id: u64,
    files: usize,
    embedded: usize,
    sidecar: usize,
    unchanged: usize,
    description: usize,
    creator: usize,
    keywords: usize,
    errors: Vec<Issue>,
    warnings: Vec<Issue>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct WriteSummary {
    written: usize,
    unchanged: usize,
    cancelled: usize,
    errors: Vec<Issue>,
    warnings: Vec<Issue>,
}

impl WriteSummary {
    pub fn catalog_error(&mut self, photo: &CatalogPhoto, message: &str) {
        self.errors.push(Issue::for_photo(photo, format!("Katalóg: {message}")));
    }
}

impl CatalogPhoto {
    pub fn matching_keyword_ids(&self, keyword: &str) -> impl Iterator<Item = i64> + '_ {
        let key = keyword_key(keyword);
        self.keyword_ids
            .iter()
            .copied()
            .zip(&self.keywords)
            .filter_map(move |(id, path)| path.last().is_some_and(|leaf| keyword_key(leaf) == key).then_some(id))
    }
}

pub fn edit_photos(
    photos: Vec<CatalogPhoto>,
    targets: Vec<EditorTarget>,
    action: &ManualEdit,
    exif: &ExifTool,
    formats: &FormatTable,
    mut guard: impl FnMut(i64) -> Result<(), String>,
    progress: impl Fn(usize, usize),
) -> (WriteSummary, Vec<CatalogPhoto>) {
    let mut summary = WriteSummary::default();
    let mut succeeded = Vec::new();
    let mut photos: HashMap<_, _> = photos.into_iter().map(|photo| (photo.id, photo)).collect();
    let total = targets.len();
    progress(0, total);
    for (index, target) in targets.into_iter().enumerate() {
        if let Some(photo) = photos.remove(&target.id) {
            let apply = || {
                let source = photo.path.as_deref().ok_or("Fotka nemá cestu")?;
                let mut job = Job::read(photo.clone(), PathBuf::from(source), exif, formats, true)?;
                if job.revision() != target.revision {
                    return Err("Súbor alebo metadáta sa zmenili od načítania".into());
                }
                job.merge = manual_merge(&photo, &job.before, job.embedded.is_some(), action)?;
                let diagnostic = job.write(exif, formats)?;
                Ok::<_, String>((job, diagnostic))
            };
            match guard(photo.id).and_then(|_| apply()) {
                Ok((job, diagnostic)) => {
                    if job.merge.edits.is_empty() {
                        summary.unchanged += 1;
                    } else {
                        summary.written += 1;
                    }
                    summary.warnings.extend(
                        job.merge
                            .warnings
                            .into_iter()
                            .chain((!diagnostic.is_empty()).then_some(diagnostic))
                            .map(|warning| Issue::new(&job.target, warning)),
                    );
                    succeeded.push(photo);
                }
                Err(message) => summary.errors.push(Issue::for_photo(&photo, message)),
            }
        } else {
            summary.errors.push(Issue {
                path: format!("ID {}", target.id),
                message: "Fotka už nie je dostupná".into(),
            });
        }
        progress(index + 1, total);
    }
    (summary, succeeded)
}

struct Job {
    photo: CatalogPhoto,
    source: PathBuf,
    source_stamp: Stamp,
    target: PathBuf,
    target_stamp: Option<Stamp>,
    embedded: Option<&'static str>,
    before: Value,
    merge: Merge,
    manual: bool,
}

fn read_target(
    exif: &ExifTool,
    target: &Path,
    target_stamp: Option<&Stamp>,
    embedded: Option<&str>,
    manual: bool,
) -> Result<Value, String> {
    if target_stamp.is_none() {
        return Ok(Value::Null);
    }
    let metadata = if manual {
        exif.iptc_editor_metadata(target)
    } else {
        exif.iptc_metadata(target)
    }
    .map_err(|e| e.to_string())?;
    let expected = embedded.unwrap_or("XMP");
    if !tag_values(&metadata, "File:FileType").any(|file_type| file_type == expected) {
        return Err(format!("ExifTool nerozpoznal očakávaný typ {expected}"));
    }
    Ok(metadata)
}

impl Job {
    fn prepare(photo: CatalogPhoto, source: PathBuf, exif: &ExifTool, formats: &FormatTable) -> Result<Self, String> {
        let mut job = Self::read(photo, source, exif, formats, false)?;
        job.merge = merge(&job.photo, &job.before, job.embedded.is_some());
        Ok(job)
    }

    fn read(
        photo: CatalogPhoto,
        source: PathBuf,
        exif: &ExifTool,
        formats: &FormatTable,
        manual: bool,
    ) -> Result<Self, String> {
        if !source.is_absolute() || !ext(&source).eq_ignore_ascii_case(&photo.ext) {
            return Err("Cesta alebo prípona fotografie sa nezhoduje s katalógom".into());
        }
        let source_stamp = stamp(&source)?.ok_or("Fotka chýba alebo je offline")?;
        let embedded = embedded_file_type(ext(&source));
        let (target, target_stamp) = if embedded.is_some() {
            (source.clone(), stamp(&source)?)
        } else {
            sidecar_target(&source, formats)?
        };
        if embedded.is_none()
            && target_stamp
                .as_ref()
                .is_some_and(|target| target.canonical == source_stamp.canonical)
        {
            return Err("XMP sidecar odkazuje na originál".into());
        }
        let before = read_target(exif, &target, target_stamp.as_ref(), embedded, manual)?;
        Ok(Self {
            photo,
            source,
            source_stamp,
            target,
            target_stamp,
            embedded,
            before,
            merge: Merge::default(),
            manual,
        })
    }

    fn revision(&self) -> String {
        let mut hash = DefaultHasher::new();
        self.source.hash(&mut hash);
        self.source_stamp.hash(&mut hash);
        self.target.hash(&mut hash);
        self.target_stamp.hash(&mut hash);
        self.before.to_string().hash(&mut hash);
        format!("{:016x}", hash.finish())
    }

    fn check_target(&self, formats: &FormatTable) -> Result<(), String> {
        if stamp(&self.source)?.as_ref() != Some(&self.source_stamp) {
            return Err("Fotka sa od kontroly zmenila alebo je offline".into());
        }
        let (path, current) = if self.embedded.is_some() {
            (self.target.clone(), stamp(&self.target)?)
        } else {
            sidecar_target(&self.source, formats)?
        };
        if path != self.target || current != self.target_stamp {
            return Err("Cieľ sa od kontroly zmenil".into());
        }
        Ok(())
    }

    fn write(&self, exif: &ExifTool, formats: &FormatTable) -> Result<String, String> {
        self.check_target(formats)?;
        let live = read_target(
            exif,
            &self.target,
            self.target_stamp.as_ref(),
            self.embedded,
            self.manual,
        )?;
        if live != self.before {
            return Err("Metadáta sa od kontroly zmenili".into());
        }
        if self.merge.edits.is_empty() {
            return Ok(String::new());
        }
        let mut args = Vec::new();
        let mut temp_files = Vec::new();
        if self.merge.edits.iter().any(|edit| edit.tag.starts_with("IPTC:")) {
            args.extend([
                "-charset".into(),
                "iptc=UTF8".into(),
                "-IPTC:CodedCharacterSet=UTF8".into(),
            ]);
        }
        for edit in &self.merge.edits {
            if edit.kind == EditKind::Append {
                args.extend([
                    format!("-{}-={}", edit.tag, edit.value),
                    format!("-{}+={}", edit.tag, edit.value),
                ]);
            } else if edit.value.contains(['\r', '\n']) {
                let temp = TempFile::new(&std::env::temp_dir(), edit.value.as_bytes()).map_err(|e| e.to_string())?;
                let operator = if edit.kind == EditKind::Remove { "-<=" } else { "<=" };
                args.push(format!("-{}{operator}{}", edit.tag, temp.0.display()));
                temp_files.push(temp);
            } else {
                let operator = if edit.kind == EditKind::Remove { "-=" } else { "=" };
                args.push(format!("-{}{operator}{}", edit.tag, edit.value));
            }
        }
        self.check_target(formats)?;
        let warning = if self.target_stamp.is_some() {
            exif.write(&self.target, &args).map_err(|e| e.to_string())?
        } else {
            let (bytes, warning) = exif.create_xmp(&args).map_err(|e| e.to_string())?;
            let parent = self.target.parent().ok_or("Priečinok pre XMP nie je dostupný")?;
            let temp = TempFile::new(parent, &bytes).map_err(|e| e.to_string())?;
            self.check_target(formats)?;
            NativeFileSystem
                .move_no_replace(&temp.0, &self.target)
                .map_err(|e| e.to_string())?;
            warning
        };
        let after_stamp = stamp(&self.target)?.ok_or("Po zápise chýba cieľový súbor")?;
        if let Some(before) = &self.target_stamp {
            restore_mtime(&self.target, before.modified, &after_stamp)?;
        }
        let after = read_target(exif, &self.target, Some(&after_stamp), self.embedded, self.manual)?;
        for edit in &self.merge.edits {
            let mut values = tag_values(&after, edit.tag).map(|value| value.replace("\r\n", "\n").replace('\r', "\n"));
            let verified = match edit.kind {
                EditKind::Remove => tag_values(&after, edit.tag).all(|value| value != edit.value),
                EditKind::Set if self.manual && edit.value.is_empty() => !after
                    .as_object()
                    .is_some_and(|tags| tags.keys().any(|key| matches_tag(key, edit.tag))),
                EditKind::Set if self.manual => {
                    values.next().as_deref() == Some(&edit.value) && values.next().is_none()
                }
                _ => values.any(|value| value == edit.value),
            };
            if !verified {
                let error = if self.manual {
                    format!("Kontrola hodnoty {} po zápise zlyhala", edit.tag)
                } else {
                    format!("Kontrola po zápise nenašla {}", edit.tag)
                };
                return Err(if warning.is_empty() {
                    error
                } else {
                    format!("{error} · {warning}")
                });
            }
        }
        for tag in DESCRIPTION
            .into_iter()
            .chain(CREATOR)
            .chain(KEYWORDS)
            .chain([HIERARCHY])
        {
            if tag_values(&self.before, tag).any(|value| {
                let replaced = self.manual
                    && self.merge.edits.iter().any(|edit| {
                        edit.tag == tag
                            && (edit.kind == EditKind::Set || (edit.kind == EditKind::Remove && edit.value == value))
                    });
                !replaced && !tag_values(&after, tag).any(|present| present == value)
            }) {
                return Err(format!("Kontrola po zápise nenašla pôvodnú hodnotu {tag}"));
            }
        }
        Ok(warning)
    }
}

fn restore_mtime(path: &Path, original: SystemTime, after: &Stamp) -> Result<(), String> {
    if original == after.modified {
        return Ok(());
    }
    // ExifTool -P may round file times to the platform's Perl timestamp precision.
    let difference = original
        .duration_since(after.modified)
        .unwrap_or_else(|error| error.duration());
    if difference >= std::time::Duration::from_secs(1) || stamp(path)?.as_ref() != Some(after) {
        return Err("Čas zmeny súboru sa napriek -P nezachoval".into());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let stat = file.metadata().map_err(|e| e.to_string())?;
    if stat.len() != after.size || stat.modified().map_err(|e| e.to_string())? != after.modified {
        return Err("Súbor sa po zápise zmenil".into());
    }
    file.set_modified(original).map_err(|e| e.to_string())?;
    if file
        .metadata()
        .and_then(|stat| stat.modified())
        .map_err(|e| e.to_string())?
        != original
    {
        return Err("Nepodarilo sa obnoviť presný čas zmeny".into());
    }
    Ok(())
}

struct TempFile(PathBuf);
impl TempFile {
    fn new(dir: &Path, bytes: &[u8]) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = dir.join(format!(
                ".ddreams-iptc-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let temp = Self(path);
                    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                        drop(file);
                        return Err(error);
                    }
                    return Ok(temp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct Plan {
    summary: PlanSummary,
    jobs: Vec<Job>,
}

impl Plan {
    pub fn prepare(id: u64, photos: Vec<CatalogPhoto>, exif: &ExifTool, formats: &FormatTable) -> Self {
        let mut summary = PlanSummary {
            id,
            ..Default::default()
        };
        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        for photo in photos {
            let Some(path) = &photo.path else {
                summary.files += 1;
                summary.errors.push(Issue {
                    path: format!("ID {}", photo.id),
                    message: "Fotka nemá cestu".into(),
                });
                continue;
            };
            let source = PathBuf::from(path);
            if !seen.insert(path.to_lowercase().replace('/', "\\")) {
                continue;
            }
            summary.files += 1;
            match Job::prepare(photo, source.clone(), exif, formats) {
                Ok(job) => {
                    summary.embedded += usize::from(job.embedded.is_some());
                    summary.sidecar += usize::from(job.embedded.is_none());
                    summary.warnings.extend(
                        job.merge
                            .warnings
                            .iter()
                            .map(|warning| Issue::new(&job.target, warning.clone())),
                    );
                    if job.merge.edits.is_empty() {
                        summary.unchanged += 1;
                        continue;
                    }
                    let has = |tags: &[&str]| usize::from(job.merge.edits.iter().any(|edit| tags.contains(&edit.tag)));
                    summary.description += has(&DESCRIPTION);
                    summary.creator += has(&CREATOR);
                    summary.keywords += has(&[KEYWORDS[0], KEYWORDS[1], HIERARCHY]);
                    jobs.push(job);
                }
                Err(error) => summary.errors.push(Issue::new(&source, error)),
            }
        }
        Self { summary, jobs }
    }

    pub fn execute(
        self,
        exif: &ExifTool,
        formats: &FormatTable,
        cancel: &AtomicBool,
        progress: impl Fn(usize, usize),
    ) -> WriteSummary {
        let mut result = WriteSummary {
            unchanged: self.summary.unchanged,
            errors: self.summary.errors,
            warnings: self.summary.warnings,
            ..Default::default()
        };
        let total = self.jobs.len();
        progress(0, total);
        for (index, job) in self.jobs.into_iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                result.cancelled = total - index;
                break;
            }
            match job.write(exif, formats) {
                Ok(warning) => {
                    result.written += 1;
                    if !warning.is_empty() {
                        result.warnings.push(Issue::new(&job.target, warning));
                    }
                }
                Err(error) => result.errors.push(Issue::new(&job.target, error)),
            }
            progress(index + 1, total);
        }
        result
    }
}

#[derive(Default)]
pub struct IptcState {
    state: Mutex<BatchState>,
}

#[derive(Default)]
struct BatchState {
    next: u64,
    pending: Option<(Plan, Arc<AtomicBool>)>,
    running: Option<(u64, Arc<AtomicBool>)>,
}

impl IptcState {
    pub fn begin_plan(&self) -> Result<u64, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.running.is_some() {
            return Err("Zápis IPTC už prebieha".into());
        }
        state.pending = None;
        state.next += 1;
        Ok(state.next)
    }

    pub fn store(&self, plan: Plan) -> Result<PlanSummary, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if plan.summary.id != state.next || state.running.is_some() {
            return Err("Plán IPTC už nie je aktuálny".into());
        }
        let summary = plan.summary.clone();
        state.pending = Some((plan, Arc::new(AtomicBool::new(false))));
        Ok(summary)
    }

    pub fn execute(
        &self,
        id: u64,
        exif: &ExifTool,
        formats: &FormatTable,
        progress: impl Fn(usize, usize),
    ) -> Result<WriteSummary, String> {
        let (plan, cancel) = {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            if state.running.is_some() || !state.pending.as_ref().is_some_and(|(plan, _)| plan.summary.id == id) {
                return Err("Plán IPTC už nie je aktuálny".into());
            }
            let (plan, cancel) = state.pending.take().unwrap();
            state.running = Some((id, cancel.clone()));
            (plan, cancel)
        };
        let result = plan.execute(exif, formats, &cancel, progress);
        self.state.lock().map_err(|e| e.to_string())?.running = None;
        Ok(result)
    }

    pub fn cancel(&self, id: u64) -> Result<(), String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        if let Some((running_id, cancel)) = &state.running {
            if *running_id == id {
                cancel.store(true, Ordering::Release);
            }
        }
        if let Some((plan, cancel)) = &state.pending {
            if plan.summary.id == id {
                cancel.store(true, Ordering::Release);
            }
        }
        Ok(())
    }
}

pub(crate) fn metadata_with_sidecar(exif: &ExifTool, path: &Path) -> std::io::Result<Value> {
    let main = exif.metadata(path);
    if let Some(sidecar) = sidecar_candidates(path).into_iter().find(|path| path.is_file()) {
        if let Ok(side) = exif.metadata(&sidecar) {
            if let Some(tags) = side.as_object() {
                let mut main = main
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| Value::Object(Default::default()));
                let has_main = [DESCRIPTION, CREATOR, KEYWORDS]
                    .map(|group| group.iter().any(|tag| tag_values(&main, tag).next().is_some()));
                let main = main.as_object_mut().unwrap();
                for (key, value) in tags {
                    if !key.starts_with("XMP-") && !key.starts_with("IPTC:") {
                        continue;
                    }
                    let equivalent = [DESCRIPTION, CREATOR, KEYWORDS]
                        .iter()
                        .enumerate()
                        .any(|(i, group)| has_main[i] && group.iter().any(|tag| matches_tag(key, tag)));
                    if !equivalent
                        && main.get(key).is_none_or(|existing| {
                            existing.is_null()
                                || existing.as_str().is_some_and(|text| text.trim().is_empty())
                                || existing.as_array().is_some_and(Vec::is_empty)
                        })
                    {
                        main.insert(key.clone(), value.clone());
                    }
                }
                return Ok(Value::Object(std::mem::take(main)));
            }
        }
    }
    main
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ddreams-iptc-test-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn photo(path: &Path) -> CatalogPhoto {
        CatalogPhoto {
            id: 1,
            path: Some(path.to_string_lossy().into_owned()),
            ext: ext(path).into(),
            caption: Some("Prvý riadok\nŽlté kľúče".into()),
            author: Some("Ľuboš".into()),
            keywords: vec![vec!["Vianoce".into(), "Deti".into()], vec!["deti".into()]],
            keyword_ids: vec![1, 2],
        }
    }

    #[test]
    fn identifies_only_verified_embedded_formats_and_shared_sidecars() {
        for ext in ["JPG", "jpeg", "tif", "tiff", "dng"] {
            assert!(embedded_file_type(ext).is_some());
        }
        for ext in ["arw", "nef", "cr2", "cr3", "rw2", "orf", "raf", "heic", "png", "webp"] {
            assert!(embedded_file_type(ext).is_none());
        }
        let dir = TempDir::new();
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let raw = dir.file("A.ARW");
        fs::write(&raw, b"RAW").unwrap();
        assert_eq!(sidecar_target(&raw, &formats).unwrap().0, dir.file("A.xmp"));
        let qualified = dir.file("A.ARW.xmp");
        fs::write(&qualified, b"XMP").unwrap();
        assert_eq!(sidecar_target(&raw, &formats).unwrap().0, qualified);
        fs::remove_file(&qualified).unwrap();
        let sibling = dir.file("a.nef");
        fs::write(&sibling, b"RAW").unwrap();
        for input in [&raw, &sibling] {
            assert_eq!(sidecar_target(input, &formats).unwrap_err(), SHARED);
        }
        fs::write(dir.file("A.xmp"), b"shared XMP").unwrap();
        for input in [&raw, &sibling] {
            assert_eq!(sidecar_target(input, &formats).unwrap_err(), SHARED);
        }
    }

    #[test]
    fn preserves_existing_text_and_deduplicates_keywords_per_tag() {
        let mut photo = photo(Path::new("a.jpg"));
        photo.author = Some("Ž".repeat(20));
        let metadata = json!({"XMP-dc:Copy1:Description-fr":"Existant","XMP-dc:Subject":["DETI","Staré"]});
        let planned = merge(&photo, &metadata, true);
        assert!(!planned.edits.iter().any(|edit| DESCRIPTION.contains(&edit.tag)));
        assert!(!planned.edits.iter().any(|edit| edit.tag == KEYWORDS[0]));
        assert_eq!(planned.edits.iter().filter(|edit| edit.tag == KEYWORDS[1]).count(), 1);
        assert!(
            planned
                .edits
                .iter()
                .any(|edit| edit.tag == HIERARCHY && edit.value == "Vianoce|Deti")
        );
        assert!(planned.edits.iter().any(|edit| edit.tag == CREATOR[0]));
        assert!(!planned.edits.iter().any(|edit| edit.tag == CREATOR[1]));
        assert!(planned.warnings.iter().any(|warning| warning.contains("32")));
        let legacy = merge(&photo, &json!({"IPTC:CopyrightNotice":"pôvodné"}), true);
        assert!(legacy.edits.iter().all(|edit| !edit.tag.starts_with("IPTC:")));
        assert!(!legacy.warnings.is_empty());
        let filled = json!({"IPTC:Caption-Abstract":"Pôvodný", "IPTC:By-line":["Autor"], "IPTC:CodedCharacterSet":"UTF8",
            "IPTC:Keywords":["DETI"], "XMP-dc:Subject":["Deti"], "XMP-lr:HierarchicalSubject":["vianoce|deti","DETI"]});
        assert!(merge(&photo, &filled, true).edits.is_empty());
    }

    #[cfg(windows)]
    fn edit_temporary_photo(
        photo: CatalogPhoto,
        action: ManualEdit,
        exif: &ExifTool,
        formats: &FormatTable,
    ) -> WriteSummary {
        let loaded = editor_photos(vec![photo.clone()], exif, formats).pop().unwrap();
        let targets = vec![EditorTarget {
            id: photo.id,
            revision: loaded.file.unwrap().revision,
        }];
        let (result, applied) = edit_photos(vec![photo], targets, &action, exif, formats, |_| Ok(()), |_, _| {});
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(applied.len(), 1);
        result
    }

    #[test]
    fn manual_edits_replace_delete_and_remove_only_the_requested_tags() {
        assert!(serde_json::from_value::<ManualEdit>(json!({"kind":"text", "field":"caption"})).is_err());
        assert!(serde_json::from_value::<ManualEdit>(json!({"kind":"text", "field":"caption", "value":null})).is_ok());
        let photo = photo(Path::new("a.jpg"));
        let metadata = json!({
            "XMP-dc:Description": "pôvodný",
            "XMP-dc:Description-fr": "traduction",
            "IPTC:Caption-Abstract": "starší",
            "IPTC:CodedCharacterSet": "UTF8",
            "XMP-dc:Subject": ["DETI", "iné"],
            "IPTC:Keywords": ["Deti", "iné"],
            "XMP-lr:HierarchicalSubject": ["Vianoce|deti", "Deti|iné", "Deti"],
        });
        assert_eq!(text_value(&metadata, DESCRIPTION, false).as_deref(), Some("pôvodný"));
        for value in [Some("nový".to_string()), None] {
            let action = ManualEdit::Text {
                field: AssetTextField::Caption,
                value: value.clone(),
            };
            let result = manual_merge(&photo, &metadata, true, &action).unwrap();
            assert_eq!(
                result.edits,
                DESCRIPTION.map(|tag| Edit {
                    tag,
                    value: value.clone().unwrap_or_default(),
                    kind: EditKind::Set,
                })
            );
        }
        let remove = ManualEdit::RemoveKeyword { keyword: "dEti".into() };
        let edit = manual_merge(&photo, &metadata, true, &remove).unwrap();
        assert_eq!(
            edit.edits
                .iter()
                .map(|edit| (edit.tag, edit.value.as_str(), edit.kind))
                .collect::<Vec<_>>(),
            [
                (KEYWORDS[0], "DETI", EditKind::Remove),
                (KEYWORDS[1], "Deti", EditKind::Remove),
                (HIERARCHY, "Vianoce|deti", EditKind::Remove),
                (HIERARCHY, "Deti", EditKind::Remove),
            ]
        );
        assert_eq!(photo.matching_keyword_ids("DETI").collect::<Vec<_>>(), [1, 2]);
        let legacy = manual_merge(
            &photo,
            &json!({"IPTC:Keywords": "Deti", "XMP-dc:Subject": "deti"}),
            true,
            &remove,
        )
        .unwrap();
        assert_eq!(legacy.edits.len(), 1);
        assert_eq!(legacy.edits[0].tag, KEYWORDS[0]);
        assert_eq!(legacy.warnings.len(), 1);
        let long_author = ManualEdit::Text {
            field: AssetTextField::Author,
            value: Some("Ž".repeat(20)),
        };
        let long = manual_merge(&photo, &Value::Null, true, &long_author).unwrap();
        assert_eq!(long.edits.len(), 1);
        assert_eq!(long.edits[0].tag, CREATOR[0]);
        assert_eq!(long.warnings.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn bundled_exiftool_writes_only_missing_metadata_on_temporary_files() {
        let executable = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/exiftool/exiftool.exe");
        if !executable.is_file() {
            eprintln!("Preskočené: pribalený ExifTool chýba");
            return;
        }
        let exif = ExifTool::new(executable);
        let dir = TempDir::new();
        let jpeg = dir.file("snímka.jpg");
        image::RgbImage::new(16, 16).save(&jpeg).unwrap();
        exif.write(
            &jpeg,
            &[
                "-charset".into(),
                "iptc=UTF8".into(),
                "-IPTC:CodedCharacterSet=UTF8".into(),
                "-IPTC:Caption-Abstract=Starší popis".into(),
                "-XMP-dc:Description=Pôvodný popis".into(),
                "-XMP-dc:Subject=Staré".into(),
            ],
        )
        .unwrap();
        let tiff = dir.file("snímka.tiff");
        let dng_seed = dir.file("dng-seed.tiff");
        let dng = dir.file("snímka.dng");
        for path in [&tiff, &dng_seed] {
            image::RgbImage::new(16, 16).save(path).unwrap();
        }
        exif.write(&dng_seed, &["-DNGVersion=1.4.0.0".into()]).unwrap();
        fs::rename(&dng_seed, &dng).unwrap();
        let old_raw = dir.file("starý.ARW");
        let new_raw = dir.file("nový %d.NEF");
        for raw in [&old_raw, &new_raw] {
            fs::write(raw, b"fake RAW").unwrap();
        }
        let old_xmp = dir.file("starý.ARW.xmp");
        let (old_packet, _) = exif
            .create_xmp(&["-XMP-crs:Exposure2012=0.5".into(), "-XMP-dc:Subject=Staré".into()])
            .unwrap();
        fs::write(&old_xmp, old_packet).unwrap();
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(123_456_700);
        for path in [&jpeg, &old_xmp] {
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
        }
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let photos = vec![
            photo(&jpeg),
            photo(&tiff),
            photo(&dng),
            photo(&old_raw),
            photo(&new_raw),
        ];
        let plan = Plan::prepare(1, photos.clone(), &exif, &formats);
        assert!(plan.summary.errors.is_empty(), "{:?}", plan.summary.errors);
        assert_eq!((plan.summary.embedded, plan.summary.sidecar), (3, 2));
        let result = plan.execute(&exif, &formats, &AtomicBool::new(false), |_, _| {});
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.written, 5);
        let jpeg_meta = exif.iptc_metadata(&jpeg).unwrap();
        assert_eq!(tag_values(&jpeg_meta, DESCRIPTION[0]).next(), Some("Pôvodný popis"));
        assert!(tag_values(&jpeg_meta, KEYWORDS[0]).any(|value| value == "Staré"));
        assert!(tag_values(&jpeg_meta, KEYWORDS[1]).any(|value| value == "Deti"));
        assert!(tag_values(&jpeg_meta, "IPTC:CodedCharacterSet").any(|value| value == "UTF8"));
        let side = exif.metadata(&old_xmp).unwrap();
        assert!(side.get("XMP-crs:Exposure2012").is_some());
        assert!(tag_values(&side, KEYWORDS[0]).any(|value| value == "Staré"));
        assert_eq!(
            tag_values(&side, DESCRIPTION[0]).next(),
            Some("Prvý riadok\nŽlté kľúče")
        );
        let displayed = metadata_with_sidecar(&exif, &old_raw).unwrap();
        assert_eq!(
            tag_values(&displayed, DESCRIPTION[0]).next(),
            Some("Prvý riadok\nŽlté kľúče")
        );
        assert!(dir.file("nový %d.xmp").is_file());
        for path in [&jpeg, &old_xmp] {
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), mtime);
        }
        for raw in [&old_raw, &new_raw] {
            assert_eq!(fs::read(raw).unwrap(), b"fake RAW");
        }
        let second = Plan::prepare(2, photos, &exif, &formats);
        assert_eq!(
            (second.summary.unchanged, second.jobs.len()),
            (5, 0),
            "{:?}",
            second.summary.errors
        );
        let before = fs::read(&old_xmp).unwrap();
        assert_eq!(
            second
                .execute(&exif, &formats, &AtomicBool::new(false), |_, _| {})
                .written,
            0
        );
        assert_eq!(fs::read(&old_xmp).unwrap(), before);

        let describe = ManualEdit::Text {
            field: AssetTextField::Caption,
            value: Some("Nový popis\nDruhý riadok".into()),
        };
        assert_eq!(edit_temporary_photo(photo(&jpeg), describe, &exif, &formats).written, 1);
        let after = exif.iptc_editor_metadata(&jpeg).unwrap();
        for tag in DESCRIPTION {
            assert_eq!(tag_values(&after, tag).next(), Some("Nový popis\nDruhý riadok"));
        }
        let creator = ManualEdit::Text {
            field: AssetTextField::Author,
            value: Some("Sára".into()),
        };
        assert_eq!(
            edit_temporary_photo(photo(&old_raw), creator, &exif, &formats).written,
            1
        );
        let side = exif.metadata(&old_xmp).unwrap();
        assert_eq!(tag_values(&side, CREATOR[0]).next(), Some("Sára"));
        assert!(side.get("XMP-crs:Exposure2012").is_some());
        let clear = ManualEdit::Text {
            field: AssetTextField::Caption,
            value: None,
        };
        assert_eq!(edit_temporary_photo(photo(&jpeg), clear, &exif, &formats).written, 1);
        let after = exif.iptc_editor_metadata(&jpeg).unwrap();
        for tag in DESCRIPTION {
            assert!(tag_values(&after, tag).next().is_none());
        }
        let remove = ManualEdit::RemoveKeyword { keyword: "DETI".into() };
        assert_eq!(edit_temporary_photo(photo(&jpeg), remove, &exif, &formats).written, 1);
        let after = exif.iptc_editor_metadata(&jpeg).unwrap();
        assert_eq!(file_keywords(&after), ["Staré"]);
        assert!(tag_values(&after, HIERARCHY).next().is_none());
        let mut keyword_photo = photo(&jpeg);
        keyword_photo.keywords = vec![vec!["007".into()]];
        assert_eq!(
            edit_temporary_photo(keyword_photo, ManualEdit::AddKeywords, &exif, &formats).written,
            1
        );
        let after = exif.iptc_editor_metadata(&jpeg).unwrap();
        assert!(file_keywords(&after).contains(&"007".into()));
        assert!(tag_values(&after, DESCRIPTION[0]).next().is_none());
        for path in [&jpeg, &old_xmp] {
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), mtime);
        }
        for raw in [&old_raw, &new_raw] {
            assert_eq!(fs::read(raw).unwrap(), b"fake RAW");
        }

        let manual_raw = dir.file("ručný %d.ARW");
        let manual_sidecar = manual_raw.with_extension("xmp");
        fs::write(&manual_raw, b"fake RAW").unwrap();
        let manual_stamp = stamp(&manual_raw).unwrap();
        let manual_photo = photo(&manual_raw);
        let old_revision = editor_photos(vec![manual_photo.clone()], &exif, &formats)
            .pop()
            .unwrap()
            .file
            .unwrap()
            .revision;
        let describe = ManualEdit::Text {
            field: AssetTextField::Caption,
            value: Some("Ručne".into()),
        };
        assert_eq!(
            edit_temporary_photo(manual_photo.clone(), describe, &exif, &formats).written,
            1
        );
        let before = fs::read(&manual_sidecar).unwrap();
        let clear = ManualEdit::Text {
            field: AssetTextField::Caption,
            value: None,
        };
        let targets = vec![EditorTarget {
            id: manual_photo.id,
            revision: old_revision,
        }];
        let (stale_edit, applied) = edit_photos(
            vec![manual_photo],
            targets,
            &clear,
            &exif,
            &formats,
            |_| Ok(()),
            |_, _| {},
        );
        assert_eq!((stale_edit.written, stale_edit.errors.len(), applied.len()), (0, 1, 0));
        assert_eq!(fs::read(&manual_sidecar).unwrap(), before);
        assert_eq!(stamp(&manual_raw).unwrap(), manual_stamp);
        assert_eq!(fs::read(&manual_raw).unwrap(), b"fake RAW");

        let fresh = dir.file("pretek.ARW");
        fs::write(&fresh, b"fake RAW").unwrap();
        let batch = IptcState::default();
        let id = batch.begin_plan().unwrap();
        batch
            .store(Plan::prepare(id, vec![photo(&fresh)], &exif, &formats))
            .unwrap();
        batch.cancel(id).unwrap();
        let cancelled = batch.execute(id, &exif, &formats, |_, _| {}).unwrap();
        assert_eq!(cancelled.cancelled, 1);
        assert!(!fresh.with_extension("xmp").exists());
        let stale = Plan::prepare(4, vec![photo(&fresh)], &exif, &formats);
        let (other_packet, _) = exif.create_xmp(&["-XMP-dc:Subject=Iné".into()]).unwrap();
        fs::write(fresh.with_extension("xmp"), other_packet).unwrap();
        let stale_result = stale.execute(&exif, &formats, &AtomicBool::new(false), |_, _| {});
        assert_eq!(stale_result.written, 0);
        assert_eq!(stale_result.errors.len(), 1);
        assert_eq!(
            tag_values(&exif.metadata(&fresh.with_extension("xmp")).unwrap(), KEYWORDS[0]).next(),
            Some("Iné")
        );
    }
}
