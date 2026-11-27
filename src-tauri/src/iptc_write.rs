use crate::{db::Db, exiftool::ExifTool, file_ops::{sidecar_candidates, FileSystemOps, NativeFileSystem},
    formats::FormatTable, journal::keyword_components, listing::ListingService};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::SystemTime,
};

const DESCRIPTION: [&str; 2] = ["XMP-dc:Description", "IPTC:Caption-Abstract"];
const CREATOR: [&str; 2] = ["XMP-dc:Creator", "IPTC:By-line"];
const KEYWORDS: [&str; 2] = ["XMP-dc:Subject", "IPTC:Keywords"];
const HIERARCHY: &str = "XMP-lr:HierarchicalSubject";
const SHARED: &str = "Spoločný sidecar pre viac súborov";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPhoto {
    pub id: i64,
    #[serde(skip)]
    path: Option<String>,
    #[serde(skip)]
    ext: String,
    caption: Option<String>,
    author: Option<String>,
    keywords: Vec<Vec<String>>,
}

pub fn catalog_photos(db: &Db, listing: &ListingService, formats: &FormatTable, ids: &[i64]) -> Result<Vec<CatalogPhoto>, String> {
    let temporary: HashMap<_, _> = ids.iter().filter(|&&id| id < 0)
        .filter_map(|&id| listing.asset(id).map(|asset| (id, asset))).collect();
    let conn = db.lock().map_err(|e| e.to_string())?;
    let base = "SELECT id,path,ext,caption,author FROM assets WHERE is_listed=1 AND ext_lower<>''";
    let mut by_id = conn.prepare(&format!("{base} AND id=?1")).map_err(|e| e.to_string())?;
    let mut by_path = conn.prepare(&format!("{base} AND path=?1 COLLATE NOCASE ORDER BY id LIMIT 1")).map_err(|e| e.to_string())?;
    let map = |r: &rusqlite::Row<'_>| Ok(CatalogPhoto {
        id: r.get(0)?, path: r.get(1)?, ext: r.get(2)?, caption: r.get(3)?, author: r.get(4)?, keywords: Vec::new(),
    });
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for &id in ids {
        if !seen.insert(id) { continue; }
        let photo = if id > 0 {
            by_id.query_row([id], map).optional().map_err(|e| e.to_string())?
        } else if let Some(asset) = temporary.get(&id).filter(|asset| !asset.is_directory) {
            let mut catalogued = None;
            if let Some(path) = &asset.path {
                catalogued = by_path.query_row([path], map).optional().map_err(|e| e.to_string())?;
            }
            catalogued.or_else(|| Some(CatalogPhoto {
                id, path: asset.path.clone(), ext: asset.ext.clone(),
                caption: asset.caption.clone(), author: asset.author.clone(), keywords: Vec::new(),
            }))
        } else { None };
        let Some(mut photo) = photo.filter(|photo| is_photo(formats, &photo.ext)) else { continue };
        if photo.id > 0 { photo.keywords = catalog_keywords(&conn, photo.id)?; }
        photo.id = id;
        result.push(photo);
    }
    Ok(result)
}

fn catalog_keywords(conn: &Connection, id: i64) -> Result<Vec<Vec<String>>, String> {
    let mut stmt = conn.prepare("SELECT keyword_id FROM asset_keywords WHERE asset_id=?1 ORDER BY keyword_id").map_err(|e| e.to_string())?;
    let ids = stmt.query_map([id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
    ids.map(|id| keyword_components(conn, id.map_err(|e| e.to_string())?)).collect()
}

fn is_photo(formats: &FormatTable, ext: &str) -> bool { matches!(formats.group(ext), Some("image" | "raw")) }

fn embedded_file_type(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("JPEG"),
        "tif" | "tiff" => Some("TIFF"),
        // DNG is the only RAW format authorized for embedded writes.
        "dng" => Some("DNG"),
        _ => None,
    }
}

fn ext(path: &Path) -> &str { path.extension().and_then(|value| value.to_str()).unwrap_or("") }

#[derive(Debug, PartialEq, Eq)]
struct Stamp { size: u64, modified: SystemTime, canonical: PathBuf }

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
    let [base, qualified] = candidates.as_slice() else { return Err("Názov XMP súboru nie je dostupný".into()); };
    let base_stamp = stamp(base)?;
    let qualified_stamp = stamp(qualified)?;
    if base_stamp.is_none() && qualified_stamp.is_some() { return Ok((qualified.clone(), qualified_stamp)); }
    let parent = source.parent().ok_or("Priečinok súboru nie je dostupný")?;
    let canonical = source.canonicalize().map_err(|e| e.to_string())?;
    // Also look outside the selection. Creating a shared basename would be ambiguous later.
    for entry in fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let other = entry.path();
        let same_stem = other.file_stem().zip(source.file_stem())
            .is_some_and(|(a, b)| a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase());
        if same_stem && other.file_name() != canonical.file_name() && is_photo(formats, ext(&other))
            && embedded_file_type(ext(&other)).is_none() && !entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            return Err(SHARED.into());
        }
    }
    if qualified_stamp.is_some() { return Err("Pre fotku existujú dva XMP sidecary".into()); }
    Ok((base.clone(), base_stamp))
}

fn matches_tag(key: &str, tag: &str) -> bool {
    let (group, name) = tag.split_once(':').unwrap();
    let mut parts = key.split(':');
    let Some(actual_name) = parts.next_back() else { return false };
    (actual_name == name || (tag == DESCRIPTION[0] && actual_name.starts_with("Description-")))
        && parts.any(|part| part == group)
}

fn values(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(value.as_array().into_iter().flatten().filter_map(Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

fn tag_values<'a>(metadata: &'a Value, tag: &'a str) -> impl Iterator<Item = &'a str> {
    metadata.as_object().into_iter().flatten()
        .filter(move |(key, _)| matches_tag(key, tag)).flat_map(|(_, value)| values(value))
}

#[derive(Debug, PartialEq, Eq)]
struct Edit { tag: &'static str, value: String, append: bool }

#[derive(Debug, Default, PartialEq, Eq)]
struct Merge { edits: Vec<Edit>, warnings: Vec<String> }

fn merge(photo: &CatalogPhoto, metadata: &Value, embedded: bool) -> Merge {
    let mut result = Merge::default();
    // Relabelling a legacy IPTC block as UTF-8 can corrupt unrelated tags.
    let existing_iptc = metadata.as_object().is_some_and(|tags| tags.keys().any(|key| key.split(':').any(|part| part == "IPTC")));
    let safe_iptc = !existing_iptc || tag_values(metadata, "IPTC:CodedCharacterSet").any(|value| value.eq_ignore_ascii_case("UTF8"));
    let mut legacy_warning = false;
    let mut add = |tag: &'static str, value: &str, append: bool, limit: usize| {
        if value.contains('\0') || (append && value.contains(['\r', '\n'])) {
            result.warnings.push(format!("{tag} obsahuje nepovolený znak: {value:?}"));
        } else if tag.starts_with("IPTC:") && !safe_iptc {
            legacy_warning = true;
        } else if value.len() > limit {
            result.warnings.push(format!("{tag} presahuje {limit} bajtov, hodnota sa zapíše iba do XMP"));
        } else {
            result.edits.push(Edit { tag, value: value.replace("\r\n", "\n").replace('\r', "\n"), append });
        }
    };
    for (tags, value, limit) in [(DESCRIPTION, photo.caption.as_deref(), 2000), (CREATOR, photo.author.as_deref(), 32)] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else { continue };
        if tags.iter().any(|tag| tag_values(metadata, tag).next().is_some()) { continue; }
        add(tags[0], value, false, usize::MAX);
        if embedded { add(tags[1], value, false, limit); }
    }
    let mut invalid_hierarchy = false;
    for (tag, limit) in [(KEYWORDS[0], usize::MAX), (KEYWORDS[1], 64), (HIERARCHY, usize::MAX)] {
        if tag == KEYWORDS[1] && !embedded { continue; }
        let mut present: HashSet<_> = tag_values(metadata, tag).map(|value| value.trim().to_lowercase()).collect();
        for path in &photo.keywords {
            let Some(leaf) = path.last() else { continue };
            let value = if tag == HIERARCHY {
                if path.iter().any(|part| part.contains('|')) { invalid_hierarchy = true; continue; }
                path.join("|")
            } else { leaf.to_owned() };
            if !value.trim().is_empty() && present.insert(value.trim().to_lowercase()) {
                add(tag, &value, true, limit);
            }
        }
    }
    if legacy_warning { result.warnings.push("Existujúce kódovanie IPTC nie je UTF-8. Doplní sa iba XMP.".into()); }
    if invalid_hierarchy { result.warnings.push("Cesta s názvom obsahujúcim znak | sa do hierarchie XMP nezapíše.".into()); }
    result
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Issue { path: String, message: String }

impl Issue {
    fn new(path: &Path, message: String) -> Self { Self { path: path.to_string_lossy().into_owned(), message } }
}

#[derive(Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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
pub struct WriteSummary {
    written: usize,
    unchanged: usize,
    cancelled: usize,
    errors: Vec<Issue>,
    warnings: Vec<Issue>,
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
}

fn read_target(exif: &ExifTool, target: &Path, target_stamp: Option<&Stamp>, embedded: Option<&str>) -> Result<Value, String> {
    if target_stamp.is_none() { return Ok(Value::Null); }
    let metadata = exif.iptc_metadata(target).map_err(|e| e.to_string())?;
    let expected = embedded.unwrap_or("XMP");
    if !tag_values(&metadata, "File:FileType").any(|file_type| file_type == expected) {
        return Err(format!("ExifTool nerozpoznal očakávaný typ {expected}"));
    }
    Ok(metadata)
}

impl Job {
    fn prepare(photo: CatalogPhoto, source: PathBuf, exif: &ExifTool, formats: &FormatTable) -> Result<Self, String> {
        if !source.is_absolute() || !ext(&source).eq_ignore_ascii_case(&photo.ext) {
            return Err("Cesta alebo prípona fotografie sa nezhoduje s katalógom".into());
        }
        let source_stamp = stamp(&source)?.ok_or("Fotka chýba alebo je offline")?;
        let embedded = embedded_file_type(ext(&source));
        let (target, target_stamp) = if embedded.is_some() { (source.clone(), stamp(&source)?) }
            else { sidecar_target(&source, formats)? };
        if embedded.is_none() && target_stamp.as_ref().is_some_and(|target| target.canonical == source_stamp.canonical) {
            return Err("XMP sidecar odkazuje na originál".into());
        }
        let before = read_target(exif, &target, target_stamp.as_ref(), embedded)?;
        let merge = merge(&photo, &before, embedded.is_some());
        Ok(Self { photo, source, source_stamp, target, target_stamp, embedded, before, merge })
    }

    fn check_target(&self, formats: &FormatTable) -> Result<(), String> {
        if stamp(&self.source)?.as_ref() != Some(&self.source_stamp) {
            return Err("Fotka sa od kontroly zmenila alebo je offline".into());
        }
        let (path, current) = if self.embedded.is_some() { (self.target.clone(), stamp(&self.target)?) }
            else { sidecar_target(&self.source, formats)? };
        if path != self.target || current != self.target_stamp {
            return Err("Cieľ sa od kontroly zmenil".into());
        }
        Ok(())
    }

    fn write(&self, exif: &ExifTool, formats: &FormatTable) -> Result<String, String> {
        self.check_target(formats)?;
        let live = read_target(exif, &self.target, self.target_stamp.as_ref(), self.embedded)?;
        if live != self.before || merge(&self.photo, &live, self.embedded.is_some()) != self.merge {
            return Err("Metadáta sa od kontroly zmenili".into());
        }
        let mut args = Vec::new();
        let mut temp_files = Vec::new();
        if self.merge.edits.iter().any(|edit| edit.tag.starts_with("IPTC:")) {
            args.extend(["-charset".into(), "iptc=UTF8".into(), "-IPTC:CodedCharacterSet=UTF8".into()]);
        }
        for edit in &self.merge.edits {
            if edit.append {
                args.extend([format!("-{}-={}", edit.tag, edit.value), format!("-{}+={}", edit.tag, edit.value)]);
            } else if edit.value.contains('\n') {
                let temp = TempFile::new(&std::env::temp_dir(), edit.value.as_bytes()).map_err(|e| e.to_string())?;
                args.push(format!("-{}<={}", edit.tag, temp.0.display()));
                temp_files.push(temp);
            } else {
                args.push(format!("-{}={}", edit.tag, edit.value));
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
            NativeFileSystem.move_no_replace(&temp.0, &self.target).map_err(|e| e.to_string())?;
            warning
        };
        let after_stamp = stamp(&self.target)?.ok_or("Po zápise chýba cieľový súbor")?;
        if let Some(before) = &self.target_stamp {
            restore_mtime(&self.target, before.modified, &after_stamp)?;
        }
        let after = read_target(exif, &self.target, Some(&after_stamp), self.embedded)?;
        for edit in &self.merge.edits {
            if !tag_values(&after, edit.tag).any(|value| value.replace("\r\n", "\n").replace('\r', "\n") == edit.value) {
                return Err(format!("Kontrola po zápise nenašla {}{}", edit.tag, if warning.is_empty() { String::new() } else { format!(" · {warning}") }));
            }
        }
        for tag in DESCRIPTION.into_iter().chain(CREATOR).chain(KEYWORDS).chain([HIERARCHY]) {
            if tag_values(&self.before, tag).any(|value| !tag_values(&after, tag).any(|present| present == value)) {
                return Err(format!("Kontrola po zápise nenašla pôvodnú hodnotu {tag}"));
            }
        }
        Ok(warning)
    }
}

fn restore_mtime(path: &Path, original: SystemTime, after: &Stamp) -> Result<(), String> {
    if original == after.modified { return Ok(()); }
    // ExifTool -P may round file times to the platform's Perl timestamp precision.
    let difference = original.duration_since(after.modified).unwrap_or_else(|error| error.duration());
    if difference >= std::time::Duration::from_secs(1) || stamp(path)?.as_ref() != Some(after) {
        return Err("Čas zmeny súboru sa napriek -P nezachoval".into());
    }
    let file = fs::OpenOptions::new().write(true).open(path).map_err(|e| e.to_string())?;
    let stat = file.metadata().map_err(|e| e.to_string())?;
    if stat.len() != after.size || stat.modified().map_err(|e| e.to_string())? != after.modified {
        return Err("Súbor sa po zápise zmenil".into());
    }
    file.set_modified(original).map_err(|e| e.to_string())?;
    if file.metadata().and_then(|stat| stat.modified()).map_err(|e| e.to_string())? != original {
        return Err("Nepodarilo sa obnoviť presný čas zmeny".into());
    }
    Ok(())
}

struct TempFile(PathBuf);
impl TempFile {
    fn new(dir: &Path, bytes: &[u8]) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = dir.join(format!(".ddreams-iptc-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let temp = Self(path);
                    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) { drop(file); return Err(error); }
                    return Ok(temp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}
impl Drop for TempFile {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

pub struct Plan { summary: PlanSummary, jobs: Vec<Job> }

impl Plan {
    pub fn prepare(id: u64, photos: Vec<CatalogPhoto>, exif: &ExifTool, formats: &FormatTable) -> Self {
        let mut summary = PlanSummary { id, ..Default::default() };
        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        for photo in photos {
            let Some(path) = &photo.path else {
                summary.files += 1;
                summary.errors.push(Issue { path: format!("ID {}", photo.id), message: "Fotka nemá cestu".into() });
                continue;
            };
            let source = PathBuf::from(path);
            if !seen.insert(path.to_lowercase().replace('/', "\\")) { continue; }
            summary.files += 1;
            match Job::prepare(photo, source.clone(), exif, formats) {
                Ok(job) => {
                    summary.embedded += usize::from(job.embedded.is_some());
                    summary.sidecar += usize::from(job.embedded.is_none());
                    summary.warnings.extend(job.merge.warnings.iter().map(|warning| Issue::new(&job.target, warning.clone())));
                    if job.merge.edits.is_empty() { summary.unchanged += 1; continue; }
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

    pub fn execute(self, exif: &ExifTool, formats: &FormatTable, cancel: &AtomicBool, progress: impl Fn(usize, usize)) -> WriteSummary {
        let mut result = WriteSummary { unchanged: self.summary.unchanged, errors: self.summary.errors,
            warnings: self.summary.warnings, ..Default::default() };
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
                    if !warning.is_empty() { result.warnings.push(Issue::new(&job.target, warning)); }
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
struct BatchState { next: u64, pending: Option<(Plan, Arc<AtomicBool>)>, running: Option<(u64, Arc<AtomicBool>)> }

impl IptcState {
    pub fn begin_plan(&self) -> Result<u64, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.running.is_some() { return Err("Zápis IPTC už prebieha".into()); }
        state.pending = None;
        state.next += 1;
        Ok(state.next)
    }

    pub fn store(&self, plan: Plan) -> Result<PlanSummary, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if plan.summary.id != state.next || state.running.is_some() { return Err("Plán IPTC už nie je aktuálny".into()); }
        let summary = plan.summary.clone();
        state.pending = Some((plan, Arc::new(AtomicBool::new(false))));
        Ok(summary)
    }

    pub fn execute(&self, id: u64, exif: &ExifTool, formats: &FormatTable, progress: impl Fn(usize, usize)) -> Result<WriteSummary, String> {
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
            if *running_id == id { cancel.store(true, Ordering::Release); }
        }
        if let Some((plan, cancel)) = &state.pending {
            if plan.summary.id == id { cancel.store(true, Ordering::Release); }
        }
        Ok(())
    }
}

pub(crate) fn metadata_with_sidecar(exif: &ExifTool, path: &Path) -> std::io::Result<Value> {
    let main = exif.metadata(path);
    if let Some(sidecar) = sidecar_candidates(path).into_iter().find(|path| path.is_file()) {
        if let Ok(side) = exif.metadata(&sidecar) {
            if let Some(tags) = side.as_object() {
                let mut main = main.ok().filter(Value::is_object).unwrap_or_else(|| Value::Object(Default::default()));
                let has_main = [DESCRIPTION, CREATOR, KEYWORDS].map(|group| group.iter().any(|tag| tag_values(&main, tag).next().is_some()));
                let main = main.as_object_mut().unwrap();
                for (key, value) in tags {
                    if !key.starts_with("XMP-") && !key.starts_with("IPTC:") { continue; }
                    let equivalent = [DESCRIPTION, CREATOR, KEYWORDS].iter().enumerate()
                        .any(|(i, group)| has_main[i] && group.iter().any(|tag| matches_tag(key, tag)));
                    if !equivalent && main.get(key).is_none_or(|existing| existing.is_null() || existing.as_str().is_some_and(|text| text.trim().is_empty()) || existing.as_array().is_some_and(Vec::is_empty)) {
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
            let path = std::env::temp_dir().join(format!("ddreams-iptc-test-{}-{}-{}", std::process::id(),
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf { self.0.join(name) }
    }
    impl Drop for TempDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn photo(path: &Path) -> CatalogPhoto {
        CatalogPhoto { id: 1, path: Some(path.to_string_lossy().into_owned()), ext: ext(path).into(),
            caption: Some("Prvý riadok\nŽlté kľúče".into()), author: Some("Ľuboš".into()),
            keywords: vec![vec!["Vianoce".into(), "Deti".into()], vec!["deti".into()]] }
    }

    #[test]
    fn identifies_only_verified_embedded_formats_and_shared_sidecars() {
        for ext in ["JPG", "jpeg", "tif", "tiff", "dng"] { assert!(embedded_file_type(ext).is_some()); }
        for ext in ["arw", "nef", "cr2", "cr3", "rw2", "orf", "raf", "heic", "png", "webp"] { assert!(embedded_file_type(ext).is_none()); }
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
        for input in [&raw, &sibling] { assert_eq!(sidecar_target(input, &formats).unwrap_err(), SHARED); }
        fs::write(dir.file("A.xmp"), b"shared XMP").unwrap();
        for input in [&raw, &sibling] { assert_eq!(sidecar_target(input, &formats).unwrap_err(), SHARED); }
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
        assert!(planned.edits.iter().any(|edit| edit.tag == HIERARCHY && edit.value == "Vianoce|Deti"));
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
    #[test]
    fn bundled_exiftool_writes_only_missing_metadata_on_temporary_files() {
        let executable = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/exiftool/exiftool.exe");
        if !executable.is_file() { eprintln!("Preskočené: pribalený ExifTool chýba"); return; }
        let exif = ExifTool::new(executable);
        let dir = TempDir::new();
        let jpeg = dir.file("snímka.jpg");
        image::RgbImage::new(16, 16).save(&jpeg).unwrap();
        exif.write(&jpeg, &["-XMP-dc:Description=Pôvodný popis".into(), "-XMP-dc:Subject=Staré".into()]).unwrap();
        let tiff = dir.file("snímka.tiff");
        let dng_seed = dir.file("dng-seed.tiff");
        let dng = dir.file("snímka.dng");
        for path in [&tiff, &dng_seed] { image::RgbImage::new(16, 16).save(path).unwrap(); }
        exif.write(&dng_seed, &["-DNGVersion=1.4.0.0".into()]).unwrap();
        fs::rename(&dng_seed, &dng).unwrap();
        let old_raw = dir.file("starý.ARW");
        let new_raw = dir.file("nový %d.NEF");
        for raw in [&old_raw, &new_raw] { fs::write(raw, b"fake RAW").unwrap(); }
        let old_xmp = dir.file("starý.ARW.xmp");
        let (old_packet, _) = exif.create_xmp(&["-XMP-crs:Exposure2012=0.5".into(), "-XMP-dc:Subject=Staré".into()]).unwrap();
        fs::write(&old_xmp, old_packet).unwrap();
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_nanos(123_456_700);
        for path in [&jpeg, &old_xmp] { fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(mtime).unwrap(); }
        let formats = FormatTable::parse(include_str!("../formats.json")).unwrap();
        let photos = vec![photo(&jpeg), photo(&tiff), photo(&dng), photo(&old_raw), photo(&new_raw)];
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
        assert_eq!(tag_values(&side, DESCRIPTION[0]).next(), Some("Prvý riadok\nŽlté kľúče"));
        let displayed = metadata_with_sidecar(&exif, &old_raw).unwrap();
        assert_eq!(tag_values(&displayed, DESCRIPTION[0]).next(), Some("Prvý riadok\nŽlté kľúče"));
        assert!(dir.file("nový %d.xmp").is_file());
        for path in [&jpeg, &old_xmp] { assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), mtime); }
        for raw in [&old_raw, &new_raw] { assert_eq!(fs::read(raw).unwrap(), b"fake RAW"); }
        let second = Plan::prepare(2, photos, &exif, &formats);
        assert_eq!((second.summary.unchanged, second.jobs.len()), (5, 0), "{:?}", second.summary.errors);
        let before = fs::read(&old_xmp).unwrap();
        assert_eq!(second.execute(&exif, &formats, &AtomicBool::new(false), |_, _| {}).written, 0);
        assert_eq!(fs::read(&old_xmp).unwrap(), before);

        let fresh = dir.file("pretek.ARW");
        fs::write(&fresh, b"fake RAW").unwrap();
        let batch = IptcState::default();
        let id = batch.begin_plan().unwrap();
        batch.store(Plan::prepare(id, vec![photo(&fresh)], &exif, &formats)).unwrap();
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
        assert_eq!(tag_values(&exif.metadata(&fresh.with_extension("xmp")).unwrap(), KEYWORDS[0]).next(), Some("Iné"));
    }
}
