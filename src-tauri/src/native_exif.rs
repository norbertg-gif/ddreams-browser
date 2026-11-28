use crate::exiftool::ExifTool;
use exif::{Exif, In, Reader, Tag, Value as ExifValue};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const IFD0_TAGS: &[u16] = &[0x010f, 0x0110, 0x8769];
const EXIF_TAGS: &[u16] = &[0x829a, 0x829d, 0x8827, 0x9003, 0x9202, 0x920a, 0xa434];
const EXIF_IFD: u16 = 0x8769;

#[derive(Clone, Copy)]
struct ByteOrder {
    little: bool,
}

impl ByteOrder {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        if self.little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    }
    fn u32(self, bytes: [u8; 4]) -> u32 {
        if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    }
    fn encode16(self, value: u16) -> [u8; 2] {
        if self.little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    }
    fn encode32(self, value: u32) -> [u8; 4] {
        if self.little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    }
}

type IfdEntry = [u8; 12];

fn selected_entries(
    file: &mut BufReader<File>,
    offset: u32,
    order: ByteOrder,
    tags: &[u16],
) -> Option<Vec<IfdEntry>> {
    if offset < 8 {
        return None;
    }
    file.seek(SeekFrom::Start(offset.into())).ok()?;
    let mut count = [0; 2];
    file.read_exact(&mut count).ok()?;
    let count = order.u16(count);
    if count > 4096 {
        return None;
    }
    let mut entries = Vec::new();
    for _ in 0..count {
        let mut entry = [0; 12];
        file.read_exact(&mut entry).ok()?;
        if tags.contains(&order.u16(entry[..2].try_into().ok()?)) {
            entries.push(entry);
        }
    }
    file.read_exact(&mut [0; 4]).ok()?;
    Some(entries)
}

// The crate's container helper reads the entire TIFF. Rebase just the selected
// IFD entries and data into a small TIFF block for its normal EXIF decoder.
fn compact_tiff(
    file: &mut BufReader<File>,
    mut header: [u8; 8],
    order: ByteOrder,
) -> Option<Vec<u8>> {
    let offset = order.u32(header[4..].try_into().ok()?);
    let mut primary = selected_entries(file, offset, order, IFD0_TAGS)?;
    let exif_offset = 8 + 2 + 12 * primary.len() as u32 + 4;
    let pointer = primary
        .iter_mut()
        .find(|entry| order.u16([entry[0], entry[1]]) == EXIF_IFD);
    let sub = if let Some(pointer) = pointer {
        if order.u16(pointer[2..4].try_into().ok()?) != 4
            || order.u32(pointer[4..8].try_into().ok()?) != 1
        {
            return None;
        }
        let offset = order.u32(pointer[8..].try_into().ok()?);
        pointer[8..].copy_from_slice(&order.encode32(exif_offset));
        Some(selected_entries(file, offset, order, EXIF_TAGS)?)
    } else {
        None
    };
    header[4..].copy_from_slice(&order.encode32(8));
    let mut block = header.to_vec();
    let mut data = Vec::new();
    let data_offset = exif_offset
        + sub
            .as_ref()
            .map_or(0, |entries| 2 + 12 * entries.len() as u32 + 4);
    for entries in std::iter::once(primary).chain(sub) {
        block.extend(order.encode16(entries.len() as u16));
        for mut entry in entries {
            let unit = match order.u16(entry[2..4].try_into().ok()?) {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return None,
            };
            let bytes = order.u32(entry[4..8].try_into().ok()?).checked_mul(unit)? as usize;
            if bytes > 4096 || data.len() + bytes > 65536 {
                return None;
            }
            if bytes > 4 {
                let original = order.u32(entry[8..].try_into().ok()?);
                file.seek(SeekFrom::Start(original.into())).ok()?;
                let start = data.len();
                data.resize(start + bytes, 0);
                file.read_exact(&mut data[start..]).ok()?;
                entry[8..].copy_from_slice(&order.encode32(data_offset + start as u32));
            }
            block.extend(entry);
        }
        block.extend([0; 4]);
    }
    block.extend(data);
    Some(block)
}

fn read_exif(path: &Path) -> Option<Exif> {
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut header = [0; 8];
    file.read_exact(&mut header).ok()?;
    let reader = Reader::new();
    match &header[..4] {
        b"II\x2a\0" | b"MM\0\x2a" => {
            let order = ByteOrder {
                little: header[0] == b'I',
            };
            reader
                .read_raw(compact_tiff(&mut file, header, order)?)
                .ok()
        }
        _ => {
            file.rewind().ok()?;
            reader.read_from_container(&mut file).ok()
        }
    }
}

fn ascii(exif: &Exif, tag: Tag) -> Option<Value> {
    let ExifValue::Ascii(values) = &exif.get_field(tag, In::PRIMARY)?.value else {
        return None;
    };
    let text = std::str::from_utf8(values.first()?).ok()?.trim();
    (!text.is_empty() && text.is_ascii()).then(|| Value::from(text))
}

fn rational(exif: &Exif, tag: Tag) -> Option<f64> {
    let value = match &exif.get_field(tag, In::PRIMARY)?.value {
        ExifValue::Rational(values) => values.first()?.to_f64(),
        ExifValue::SRational(values) => values.first()?.to_f64(),
        _ => return None,
    };
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    // ExifTool rounds 64-bit RATIONAL/SRATIONAL to ten significant digits.
    format!("{value:.9e}").parse().ok()
}

fn iso(exif: &Exif) -> Option<u32> {
    // Saturated or missing legacy ISO may require ExifTool's sensitivity or maker-note rules.
    let iso = &exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)?
        .value;
    if iso.get_uint(1).is_some() {
        return None;
    }
    let value = iso.get_uint(0)?;
    (value > 0 && value < 65535).then_some(value)
}

fn one_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

fn exposure(value: f64) -> String {
    if value < 0.25001 {
        format!("1/{:.0}", (1.0 / value).round())
    } else {
        one_decimal(value)
    }
}

pub fn read(path: &Path, tags: &[&str], numeric: bool) -> Option<Value> {
    let exif = read_exif(path)?;
    let mut result = Map::new();
    for &name in tags {
        let value = match name {
            "Make" => ascii(&exif, Tag::Make),
            "Model" => ascii(&exif, Tag::Model),
            "LensModel" => ascii(&exif, Tag::LensModel),
            "DateTimeOriginal" => ascii(&exif, Tag::DateTimeOriginal),
            "ISO" => iso(&exif).map(Value::from),
            "FNumber" => rational(&exif, Tag::FNumber).map(|n| {
                if numeric {
                    Value::from(n)
                } else {
                    Value::from(format!(
                        "{n:.precision$}",
                        precision = if n < 1.0 { 2 } else { 1 }
                    ))
                }
            }),
            "Aperture" if numeric => rational(&exif, Tag::FNumber)
                .or_else(|| rational(&exif, Tag::ApertureValue).map(|apex| 2f64.powf(apex / 2.0)))
                .map(Value::from),
            "ExposureTime" => rational(&exif, Tag::ExposureTime).map(|n| {
                if numeric {
                    Value::from(n)
                } else {
                    Value::from(exposure(n))
                }
            }),
            "FocalLength" => rational(&exif, Tag::FocalLength).map(|n| {
                if numeric {
                    Value::from(n)
                } else {
                    Value::from(format!("{n:.1} mm"))
                }
            }),
            _ => None,
        };
        if let Some(value) = value {
            let group = if matches!(name, "Make" | "Model") {
                "IFD0"
            } else {
                "ExifIFD"
            };
            result.insert(
                if numeric {
                    name.into()
                } else {
                    format!("{group}:{name}")
                },
                value,
            );
        }
    }
    if result.is_empty() {
        return None;
    }
    result.insert(
        "SourceFile".into(),
        Value::from(path.to_string_lossy().as_ref()),
    );
    Some(Value::Object(result))
}

pub fn path_key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

pub fn with_fallback(
    paths: &[PathBuf],
    tags: &[&str],
    numeric: bool,
    complete: impl Fn(&Value) -> bool,
    fallback: impl FnOnce(&[PathBuf]) -> Result<Vec<Value>, String>,
) -> (Vec<Option<Value>>, Option<String>) {
    let mut records = Vec::with_capacity(paths.len());
    let mut pending = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let native = read(path, tags, numeric).filter(&complete);
        if native.is_none() && !path.to_string_lossy().contains(['\r', '\n']) {
            pending.push((i, path.clone()));
        }
        records.push(native);
    }
    if pending.is_empty() {
        return (records, None);
    }
    let paths = pending
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    let rows = match fallback(&paths) {
        Ok(rows) => rows,
        Err(error) => return (records, Some(error)),
    };
    let by_path: HashMap<_, _> = rows
        .into_iter()
        .filter_map(|row| {
            let path = path_key(row["SourceFile"].as_str()?);
            (!row
                .as_object()?
                .keys()
                .any(|key| key == "Error" || key.ends_with(":Error")))
            .then_some((path, row))
        })
        .collect();
    for (i, path) in pending {
        if let Some(record) = by_path.get(&path_key(&path.to_string_lossy())) {
            records[i] = Some(record.clone());
        }
    }
    (records, None)
}

pub fn metadata_batch(exif: &ExifTool, paths: &[PathBuf], tags: &[&str]) -> Vec<Option<Value>> {
    with_fallback(
        paths,
        tags,
        false,
        |row| formatted_complete(row, tags),
        |paths| exif.metadata_batch(paths, tags).map_err(|e| e.to_string()),
    )
    .0
}

fn formatted_complete(row: &Value, tags: &[&str]) -> bool {
    tags.iter().all(|tag| {
        let group = if matches!(*tag, "Make" | "Model") {
            "IFD0"
        } else {
            "ExifIFD"
        };
        row.get(format!("{group}:{tag}")).is_some_and(|value| {
            *tag != "DateTimeOriginal"
                || value
                    .as_str()
                    .and_then(crate::incoming::catalog_capture_date)
                    .is_some()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{display_metadata, exif_index, incoming::catalog_capture_date};
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{Instant, SystemTime},
    };

    const CAPTURE_TAGS: &[&str] = &["DateTimeOriginal"];
    const TILE_FIELDS: &[&str] = &[
        "iso",
        "aperture",
        "shutter",
        "focalLength",
        "lens",
        "camera",
        "dateTaken",
    ];

    fn tool_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/exiftool/exiftool.exe")
    }

    fn tile_fields() -> Vec<String> {
        TILE_FIELDS.iter().map(|field| (*field).into()).collect()
    }

    fn capture(row: &Value) -> Value {
        let date = row
            .as_object()
            .and_then(|tags| {
                tags.iter().find_map(|(tag, value)| {
                    tag.ends_with(":DateTimeOriginal")
                        .then(|| value.as_str())
                        .flatten()
                })
            })
            .and_then(catalog_capture_date);
        json!({ "dateTaken": date })
    }

    fn tile(row: &Value, fields: &[String]) -> Value {
        let values = row
            .as_object()
            .map(|object| display_metadata::parse_values(object, fields))
            .unwrap_or_default();
        serde_json::to_value(values).unwrap()
    }

    fn index(row: &Value) -> Value {
        serde_json::to_value(exif_index::index_values(row)).unwrap()
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "ddreams-native-exif-{}-{stamp}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fallback_matches_paths_and_isolates_bad_rows() {
        let dir = TempDir::new();
        let paths = ["bad.jpg", "good.jpg"].map(|name| dir.0.join(name));
        fs::write(&paths[0], b"not a photo").unwrap();
        assert!(read(&paths[0], CAPTURE_TAGS, false).is_none());
        let (rows, error) = with_fallback(
            &paths,
            CAPTURE_TAGS,
            false,
            |_| true,
            |pending| {
                assert_eq!(pending, &paths);
                Ok(vec![
                    json!({"SourceFile": paths[1].to_string_lossy().replace('\\', "/"), "ExifIFD:DateTimeOriginal": "2024:05:01 12:34:56"}),
                    json!({"SourceFile": paths[0].to_string_lossy(), "ExifTool:Error": "bad file"}),
                ])
            },
        );
        assert!(error.is_none() && rows[0].is_none());
        assert_eq!(
            capture(rows[1].as_ref().unwrap())["dateTaken"],
            "2024-05-01T12:34:56"
        );
    }

    #[cfg(windows)]
    #[test]
    fn generated_jpeg_and_tiff_match_exiftool_for_all_three_paths() {
        let tool = tool_path();
        if !tool.is_file() {
            eprintln!("Bundled ExifTool missing, skipping generated EXIF fixtures");
            return;
        }
        let exif = ExifTool::new(tool.clone());
        let dir = TempDir::new();
        let fields = tile_fields();
        let tags = display_metadata::tags_for(&fields);
        for ext in ["jpg", "tiff"] {
            let path = dir.0.join(format!("fixture.{ext}"));
            image::RgbImage::new(4, 4).save(&path).unwrap();
            let args = [
                "-EXIF:Make=Test",
                "-EXIF:Model=Test Camera",
                "-EXIF:LensModel=Test Lens",
                "-EXIF:ISO=200",
                "-EXIF:FNumber=4",
                "-EXIF:FocalLength=35",
                if ext == "jpg" {
                    "-EXIF:ExposureTime=1/250"
                } else {
                    "-EXIF:ExposureTime=4/3"
                },
                "-EXIF:DateTimeOriginal=2024:05:01 12:34:56",
            ]
            .map(str::to_owned);
            exif.write(&path, &args).unwrap();
            let formatted = exif
                .metadata_batch(std::slice::from_ref(&path), &tags)
                .unwrap();
            let numeric = exif_index::read_exiftool(&tool, std::slice::from_ref(&path)).unwrap();
            let native = read(&path, &tags, false).unwrap();
            assert_eq!(capture(&native)["dateTaken"], "2024-05-01T12:34:56");
            assert_eq!(capture(&native), capture(&formatted[0]));
            assert_eq!(tile(&native, &fields), tile(&formatted[0], &fields));
            let indexed = read(&path, exif_index::INDEX_TAGS, true).unwrap();
            assert!(exif_index::index_complete(&indexed));
            assert_eq!(index(&indexed), index(&numeric[0]));
            let (_, error) = with_fallback(
                &[path.clone()],
                &tags,
                false,
                |row| formatted_complete(row, &tags),
                |_| panic!("native fixture needs no fallback"),
            );
            assert!(error.is_none());
            if ext == "tiff" {
                let mut file = BufReader::new(File::open(&path).unwrap());
                let mut header = [0; 8];
                file.read_exact(&mut header).unwrap();
                let order = ByteOrder {
                    little: header[0] == b'I',
                };
                assert!(compact_tiff(&mut file, header, order).unwrap().len() < 1024);
            }
        }
    }

    fn compare_paths(root: &Path, limit: usize) -> Vec<PathBuf> {
        let formats = crate::formats::FormatTable::parse(include_str!("../formats.json")).unwrap();
        let mut dirs = vec![root.to_path_buf()];
        let mut paths = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir)
                .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
                .flatten()
            {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    dirs.push(path);
                } else if kind.is_file()
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            matches!(formats.group(ext), Some("image" | "raw" | "video"))
                        })
                {
                    paths.push(path);
                    if paths.len() == limit {
                        return paths;
                    }
                }
            }
        }
        paths
    }

    #[test]
    #[ignore = "read-only comparison against files explicitly selected with DDREAMS_EXIF_COMPARE_DIR"]
    fn compare_native_exif() {
        let root = PathBuf::from(
            std::env::var_os("DDREAMS_EXIF_COMPARE_DIR").expect("set DDREAMS_EXIF_COMPARE_DIR"),
        );
        let root = root.canonicalize().unwrap();
        // Avoid passing Windows verbatim paths to the bundled ExifTool.
        let root = exiftool_path(&root);
        let limit = std::env::var("DDREAMS_EXIF_COMPARE_LIMIT")
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .expect("invalid DDREAMS_EXIF_COMPARE_LIMIT")
            })
            .unwrap_or(300);
        assert!(limit > 0);
        let paths = compare_paths(&root, limit);
        assert!(!paths.is_empty(), "no supported photo or video files");
        let tool = tool_path();
        assert!(
            tool.is_file(),
            "bundled ExifTool missing: {}",
            tool.display()
        );
        let exif = ExifTool::new(tool.clone());
        let fields = tile_fields();
        let tile_tags = display_metadata::tags_for(&fields);
        for (mode, tags, numeric) in [
            ("index", exif_index::INDEX_TAGS, true),
            ("capture", CAPTURE_TAGS, false),
            ("tile", tile_tags.as_slice(), false),
        ] {
            let start = Instant::now();
            let native = paths
                .iter()
                .map(|path| read(path, tags, numeric))
                .collect::<Vec<_>>();
            let native_time = start.elapsed();
            let start = Instant::now();
            let mut baseline = HashMap::new();
            for chunk in paths.chunks(if numeric { 200 } else { 64 }) {
                let rows = if numeric {
                    exif_index::read_exiftool(&tool, chunk)
                } else {
                    exif.metadata_batch(chunk, tags)
                        .map_err(|error| error.to_string())
                }
                .expect("ExifTool batch failed");
                for row in rows {
                    if let Some(path) = row["SourceFile"].as_str() {
                        baseline.insert(path_key(path), row);
                    }
                }
            }
            let tool_time = start.elapsed();
            let final_value = |row: &Value| match mode {
                "index" => index(row),
                "capture" => capture(row),
                _ => tile(row, &fields),
            };
            let mut counts = BTreeMap::<String, (usize, usize)>::new();
            let mut fallback = 0;
            for (path, row) in paths.iter().zip(&native) {
                let native_row = row.as_ref().unwrap_or(&Value::Null);
                let complete = if numeric {
                    exif_index::index_complete(native_row)
                } else {
                    formatted_complete(native_row, tags)
                };
                fallback += usize::from(!complete);
                let baseline_row = baseline
                    .get(&path_key(&path.to_string_lossy()))
                    .unwrap_or(&Value::Null);
                let actual = final_value(native_row);
                let expected = final_value(baseline_row);
                for (field, value) in expected.as_object().unwrap() {
                    let count = counts.entry(field.clone()).or_default();
                    if actual[field] != *value {
                        count.0 += 1;
                        count.1 += usize::from(complete);
                        if count.0 <= 3 {
                            println!(
                                "{mode}.{field} {} native={} ExifTool={value} fallback={}",
                                path.display(),
                                actual[field],
                                !complete
                            );
                        }
                    }
                }
            }
            println!(
                "{mode}: {} files, native {native_time:?}, ExifTool {tool_time:?}, fallback {fallback}",
                paths.len()
            );
            for (field, (raw, hybrid)) in counts {
                println!("  {field}: {raw} native mismatches, {hybrid} after fallback");
            }
        }
    }

    fn exiftool_path(path: &Path) -> PathBuf {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
    }
}
