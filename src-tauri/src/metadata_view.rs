use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, path::{Path, PathBuf}, sync::Arc};

const DEFAULT_VIEW: &str = include_str!("../metadata_view.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewField { pub label: String, pub keys: Vec<String>, #[serde(default)] pub format: Option<String> }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSubgroup { pub name: String, pub fields: Vec<ViewField> }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSection { pub name: String, pub subgroups: Vec<ViewSubgroup> }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataView { pub version: u32, pub sections: Vec<ViewSection> }

impl MetadataView {
    pub fn load() -> Arc<Self> {
        Self::load_from(std::env::var_os("DDREAMS_METADATA_VIEW").map(PathBuf::from), std::env::current_exe().ok())
    }
    fn load_from(override_path: Option<PathBuf>, executable: Option<PathBuf>) -> Arc<Self> {
        let adjacent = executable.as_deref().and_then(Path::parent).map(|p| p.join("metadata_view.json"));
        let selected = override_path.or_else(|| adjacent.filter(|p| p.is_file()));
        if let Some(path) = selected {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| Self::parse(&s).map_err(|e| e.to_string())) {
                Ok(view) => return Arc::new(view),
                Err(error) => eprintln!("Invalid metadata view file {}: {error}; using embedded defaults", path.display()),
            }
        }
        Arc::new(Self::parse(DEFAULT_VIEW).expect("embedded metadata view must be valid"))
    }
    pub fn parse(source: &str) -> Result<Self, String> {
        let view: Self = serde_json::from_str(source).map_err(|e| e.to_string())?;
        if view.version == 0 { return Err("version must be positive".into()); }
        let mut sections = HashSet::new();
        for section in &view.sections {
            if section.name.trim().is_empty() || !sections.insert(section.name.clone()) { return Err("section names must be non-empty and unique".into()); }
            for subgroup in &section.subgroups {
                if subgroup.name.trim().is_empty() { return Err("subgroup name must be non-empty".into()); }
                for field in &subgroup.fields {
                    if field.label.trim().is_empty() || field.keys.is_empty() { return Err("each field needs a label and at least one key".into()); }
                    if field.format.as_deref().is_some_and(|f| !matches!(f, "date" | "none" | "fnumber" | "seconds" | "dimensions")) { return Err(format!("unsupported format in {}", field.label)); }
                }
            }
        }
        Ok(view)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTag { pub key: String, pub group: String, pub name: String, pub value: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CuratedRow { pub group: String, pub subgroup: String, pub name: String, pub value: String }

pub fn value_text(value: &Value) -> Option<String> {
    let text = match value { Value::String(s) => s.clone(), Value::Null => return None,
        Value::Array(a) => a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string())).collect::<Vec<_>>().join(", "),
        _ => value.to_string() };
    if text.trim().is_empty() { return None; }
    if let Some(size) = text.strip_prefix("(Binary data ").and_then(|v|v.split_whitespace().next()) {
        return Some(format!("Binárne údaje: {size} bajtov"));
    }
    Some(text)
}

pub fn resolve_field(field: &ViewField, tags: &[RawTag]) -> Option<(String, Vec<String>)> {
    for expression in &field.keys {
        let parts = expression.split('+').collect::<Vec<_>>();
        let found = parts.iter().map(|part| {
            tags.iter().find(|tag| tag.key.eq_ignore_ascii_case(part)).or_else(|| tags.iter().find(|tag| tag.name.eq_ignore_ascii_case(part)))
        }).collect::<Option<Vec<_>>>();
        if let Some(found) = found {
            if found.iter().any(|tag| tag.value.trim().is_empty()) { continue; }
            let value = found.iter().map(|tag| tag.value.as_str()).collect::<Vec<_>>().join(" ");
            let value = format_value(field.format.as_deref(), &value);
            if !value.trim().is_empty() { return Some((value, found.iter().map(|tag| tag.key.clone()).collect())); }
        }
    }
    None
}

pub fn format_value(format: Option<&str>, value: &str) -> String {
    match format {
        Some("date") => format_date(value),
        Some("fnumber") => if value.starts_with("f/") { value.to_owned() } else { format!("f/{}", value.strip_suffix(".0").unwrap_or(value)) },
        Some("seconds") => if value.ends_with('s') { value.to_owned() } else { format!("{value} s") },
        Some("dimensions") => value.replacen('x', " × ", 1),
        _ => value.to_owned(),
    }
}

pub fn format_date(value: &str) -> String {
    // ExifTool date: YYYY:MM:DD HH:MM:SS with an optional timezone suffix.
    let sep = value.as_bytes().get(4).copied();
    if value.len() < 19 || !matches!(sep, Some(b':') | Some(b'-')) || value.as_bytes().get(7) != sep.as_ref() { return value.to_owned(); }
    let (date, rest) = value.split_at(10);
    let rest = rest.strip_prefix('T').map(|r| format!(" {r}")).unwrap_or_else(|| rest.to_owned());
    let day = &date[8..10]; let month = &date[5..7]; let year = &date[..4];
    if !day.bytes().all(|c| c.is_ascii_digit()) || !month.bytes().all(|c| c.is_ascii_digit()) || !year.bytes().all(|c| c.is_ascii_digit()) { return value.to_owned(); }
    format!("{}. {}. {}{}", day.parse::<u32>().unwrap_or(0), month.parse::<u32>().unwrap_or(0), year, rest)
}

pub fn is_maker_group(group: &str) -> bool {
    ["makernotes", "pentax", "nikon", "canon", "sony", "olympus", "panasonic", "fujifilm", "samsung", "apple", "casio", "kodak", "leica", "ricoh", "sigma", "minolta", "konica", "kyocera", "sanyo", "epson", "gopro", "hasselblad", "phaseone"]
        .iter().any(|name| group.to_ascii_lowercase().contains(name))
}

pub fn curate(view: &MetadataView, tags: &[RawTag], db_tags: &[RawTag]) -> Vec<CuratedRow> {
    let mut rows = Vec::new();
    let mut consumed = HashSet::new();
    let all = tags.iter().chain(db_tags.iter()).cloned().collect::<Vec<_>>();
    for section in &view.sections {
        for subgroup in &section.subgroups {
            for field in &subgroup.fields {
                if let Some((value, keys)) = resolve_field(field, &all) {
                    consumed.extend(keys);
                    rows.push(CuratedRow { group: section.name.clone(), subgroup: subgroup.name.clone(), name: field.label.clone(), value });
                }
            }
        }
    }
    let maker_groups = tags.iter().filter(|tag| is_maker_group(&tag.group)).map(|tag|tag.group.clone()).collect::<HashSet<_>>();
    let mut makers = maker_groups.into_iter().collect::<Vec<_>>(); makers.sort();
    for maker in makers {
        let mut maker_tags = tags.iter().filter(|tag|tag.group == maker).cloned().collect::<Vec<_>>(); maker_tags.sort_by(|a,b|a.name.cmp(&b.name));
        for tag in maker_tags { if !consumed.contains(&tag.key) { rows.push(CuratedRow { group: format!("Maker notes ({maker})"), subgroup: maker.clone(), name: tag.name, value: tag.value }); consumed.insert(tag.key); } }
    }
    let mut remaining = tags.iter().filter(|tag|!is_maker_group(&tag.group) && !consumed.contains(&tag.key)).cloned().collect::<Vec<_>>();
    remaining.sort_by(|a,b|a.group.cmp(&b.group).then(a.name.cmp(&b.name)));
    for tag in remaining { rows.push(CuratedRow { group: "Všetky metadáta".into(), subgroup: tag.group, name: tag.name, value: format_date(&tag.value) }); }
    let mut db_remaining = db_tags.iter().filter(|tag| !consumed.contains(&tag.key)).cloned().collect::<Vec<_>>();
    db_remaining.sort_by(|a,b|a.name.cmp(&b.name));
    for tag in db_remaining { rows.push(CuratedRow { group: "Všetky metadáta".into(), subgroup: "Katalóg".into(), name: tag.name, value: format_date(&tag.value) }); }
    rows
}

pub fn raw_tags(object: &serde_json::Map<String, Value>) -> Vec<RawTag> {
    let mut tags = Vec::new();
    for (key, value) in object {
        if key == "SourceFile" || key.starts_with("ExifTool:") { continue; }
        let Some(value) = value_text(value) else { continue; };
        let (group,name) = key.split_once(':').unwrap_or(("Metadata",key));
        tags.push(RawTag { key:key.clone(), group:group.into(), name:name.into(), value });
    }
    tags.sort_by(|a,b|a.key.cmp(&b.key));
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn default_and_override_loaders_work_with_invalid_fallback() {
        assert_eq!(MetadataView::parse(DEFAULT_VIEW).unwrap().version, 1);
        let root=std::env::temp_dir().join(format!("ddreams-meta-view-{}",std::process::id())); let _=std::fs::remove_dir_all(&root); std::fs::create_dir_all(&root).unwrap();
        let side=root.join("metadata_view.json"); std::fs::write(&side,r#"{"version":1,"sections":[{"name":"Only","subgroups":[]}]}"#).unwrap();
        assert_eq!(MetadataView::load_from(None,Some(root.join("app.exe"))).sections[0].name,"Only");
        let bad=root.join("bad.json"); std::fs::write(&bad,"broken").unwrap(); assert_eq!(MetadataView::load_from(Some(bad),None).sections[0].name,"IPTC"); let _=std::fs::remove_dir_all(root);
    }
    #[test] fn formats_values() { assert_eq!(format_value(Some("fnumber"),"5.0"),"f/5"); assert_eq!(format_value(Some("fnumber"),"0"),"f/0"); assert_eq!(format_value(Some("fnumber"),"2.8"),"f/2.8"); assert_eq!(format_value(Some("seconds"),"1/250"),"1/250 s"); assert_eq!(format_value(Some("dimensions"),"6048x4024"),"6048 × 4024"); assert_eq!(format_date("2020-06-13T11:25:25"),"13. 6. 2020 11:25:25"); }
    #[test] fn formats_dates_and_preserves_timezone() { assert_eq!(format_date("2019:05:17 13:05:38+02:00"),"17. 5. 2019 13:05:38+02:00"); assert_eq!(format_date("bad"),"bad"); }
    #[test] fn first_non_empty_key_wins_and_combined_keys_work() {
        let f=ViewField{label:"Poloha".into(),keys:vec!["Composite:GPSPosition".into(),"GPSLatitude+GPSLongitude".into()],format:None};
        let tags=vec![RawTag{key:"Composite:GPSPosition".into(),group:"Composite".into(),name:"GPSPosition".into(),value:"  ".into()},RawTag{key:"GPS:GPSLatitude".into(),group:"GPS".into(),name:"GPSLatitude".into(),value:"48 N".into()},RawTag{key:"GPS:GPSLongitude".into(),group:"GPS".into(),name:"GPSLongitude".into(),value:"17 E".into()}];
        assert_eq!(resolve_field(&f,&tags).unwrap().0,"48 N 17 E");
    }
    #[test] fn detects_maker_groups() { assert!(is_maker_group("Pentax")); assert!(is_maker_group("MakerNotes")); assert!(!is_maker_group("ExifIFD")); }
    #[test] fn binary_tag_remains_reachable_as_a_size_summary() { assert_eq!(value_text(&Value::String("(Binary data 123 bytes, use -b option to extract)".into())).as_deref(),Some("Binárne údaje: 123 bajtov")); }
    #[test] fn curated_tags_and_remaining_tags_are_separated_without_loss() {
        let view=MetadataView::parse(DEFAULT_VIEW).unwrap();
        let tags=vec![RawTag{key:"ExifIFD:ExposureTime".into(),group:"ExifIFD".into(),name:"ExposureTime".into(),value:"1/100".into()},RawTag{key:"Pentax:Foo".into(),group:"Pentax".into(),name:"Foo".into(),value:"bar".into()},RawTag{key:"XMP-dc:Creator".into(),group:"XMP-dc".into(),name:"Creator".into(),value:"A".into()}];
        let rows=curate(&view,&tags,&[]); assert!(rows.iter().any(|r|r.name=="Čas uzávierky")); assert!(rows.iter().any(|r|r.group=="Maker notes (Pentax)"&&r.name=="Foo")); assert!(!rows.iter().any(|r|r.group=="Všetky metadáta"&&r.name=="Creator"));
    }
}
