use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::PathBuf;
use std::{collections::HashMap, path::Path, sync::Arc};

const DEFAULT_FORMATS: &str = include_str!("../formats.json");

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct Format {
    pub ext: String,
    pub group: String,
    pub name: String,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct FormatTable {
    pub version: u32,
    pub formats: Vec<Format>,
    #[serde(skip)]
    by_ext: HashMap<String, usize>,
}

impl FormatTable {
    #[cfg(test)]
    pub fn load() -> Arc<Self> {
        let paths = crate::paths::Paths::resolve();
        Self::load_selected(paths.formats.as_deref())
    }

    pub fn load_selected(path: Option<&Path>) -> Arc<Self> {
        let Some(path) = path else {
            return Arc::new(Self::parse(DEFAULT_FORMATS).expect("embedded format table must be valid"));
        };
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|source| Self::parse(&source).map_err(|e| e.to_string()))
        {
            Ok(table) => Arc::new(table),
            Err(error) => {
                eprintln!(
                    "Invalid formats file {}: {error}; using embedded defaults",
                    path.display()
                );
                Arc::new(Self::parse(DEFAULT_FORMATS).expect("embedded format table must be valid"))
            }
        }
    }

    #[cfg(test)]
    fn load_from(override_path: Option<PathBuf>, executable: Option<PathBuf>) -> Arc<Self> {
        let adjacent = executable
            .as_deref()
            .and_then(Path::parent)
            .map(|p| p.join("formats.json"));
        let selected = override_path.or_else(|| adjacent.filter(|p| p.is_file()));
        if let Some(path) = selected {
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| Self::parse(&s).map_err(|e| e.to_string()))
            {
                Ok(table) => return Arc::new(table),
                Err(error) => eprintln!(
                    "Invalid formats file {}: {error}; using embedded defaults",
                    path.display()
                ),
            }
        }
        Arc::new(Self::parse(DEFAULT_FORMATS).expect("embedded format table must be valid"))
    }

    pub fn parse(source: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Raw {
            version: u32,
            formats: Vec<Format>,
        }
        let raw: Raw = serde_json::from_str(source).map_err(|e| e.to_string())?;
        if raw.version == 0 {
            return Err("version must be positive".into());
        }
        let mut by_ext = HashMap::new();
        for (index, format) in raw.formats.iter().enumerate() {
            if format.ext.is_empty() || !format.ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
                return Err(format!(
                    "invalid extension {:?}; expected lower-case [a-z0-9]+",
                    format.ext
                ));
            }
            if !["image", "raw", "video", "document", "other"].contains(&format.group.as_str()) {
                return Err(format!("invalid group {:?} for .{}", format.group, format.ext));
            }
            if format.name.trim().is_empty() {
                return Err(format!("empty display name for .{}", format.ext));
            }
            if by_ext.insert(format.ext.clone(), index).is_some() {
                return Err(format!("duplicate extension .{}", format.ext));
            }
        }
        Ok(Self {
            version: raw.version,
            formats: raw.formats,
            by_ext,
        })
    }

    pub fn get(&self, ext: &str) -> Option<&Format> {
        self.by_ext.get(&ext.to_ascii_lowercase()).map(|i| &self.formats[*i])
    }
    pub fn group(&self, ext: &str) -> Option<&str> {
        self.get(ext).map(|f| f.group.as_str())
    }
    pub fn is_hidden(&self, ext: &str) -> bool {
        self.get(ext).is_some_and(|f| f.hidden)
    }
    pub fn extensions(&self, groups: &[&str]) -> Vec<&str> {
        self.formats
            .iter()
            .filter(|f| groups.contains(&f.group.as_str()) && !f.hidden)
            .map(|f| f.ext.as_str())
            .collect()
    }
    pub fn hidden_extensions(&self) -> Vec<&str> {
        self.formats
            .iter()
            .filter(|f| f.hidden)
            .map(|f| f.ext.as_str())
            .collect()
    }
    pub fn sql_in(&self, column: &str, groups: &[&str]) -> String {
        sql_in(column, &self.extensions(groups))
    }
    pub fn sql_not_in(&self, column: &str, groups: &[&str]) -> String {
        sql_not_in(column, &self.extensions(groups))
    }
}

pub fn sql_in(column: &str, extensions: &[&str]) -> String {
    if extensions.is_empty() {
        return "0".into();
    }
    format!(
        "{column} IN ({})",
        extensions
            .iter()
            .map(|e| format!("'{e}'"))
            .collect::<Vec<_>>()
            .join(",")
    )
}
pub fn sql_not_in(column: &str, extensions: &[&str]) -> String {
    if extensions.is_empty() {
        return "1".into();
    }
    format!(
        "{column} NOT IN ({})",
        extensions
            .iter()
            .map(|e| format!("'{e}'"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const TABLE: &str = r#"{"version":1,"formats":[{"ext":"jpg","group":"image","name":"JPEG"},{"ext":"acr","group":"other","name":"ACR sidecar","hidden":true}]}"#;
    #[test]
    fn parses_and_indexes_formats() {
        let f = FormatTable::parse(TABLE).unwrap();
        assert_eq!(f.group("JPG"), Some("image"));
        assert!(f.is_hidden("acr"));
    }
    #[test]
    fn rejects_unsafe_or_uppercase_extensions() {
        for ext in ["../x", "JPG", "a-b", ""] {
            let json = format!(r#"{{"version":1,"formats":[{{"ext":"{ext}","group":"image","name":"test"}}]}}"#);
            assert!(FormatTable::parse(&json).is_err(), "{ext}");
        }
    }
    #[test]
    fn sql_lists_only_validated_extensions() {
        let f = FormatTable::parse(TABLE).unwrap();
        assert_eq!(f.sql_in("a.ext_lower", &["image"]), "a.ext_lower IN ('jpg')");
        assert_eq!(
            sql_not_in("a.ext_lower", &["jpg", "png"]),
            "a.ext_lower NOT IN ('jpg','png')"
        );
    }
    #[test]
    fn invalid_override_falls_back_to_embedded_default() {
        let root = std::env::temp_dir().join(format!("ddreams-invalid-formats-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let bad = root.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let f = FormatTable::load_from(Some(bad), None);
        assert_eq!(f.group("cr3"), Some("raw"));
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn environment_override_and_executable_sidecar_are_selected() {
        let root = std::env::temp_dir().join(format!("ddreams-formats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let adjacent = root.join("formats.json");
        std::fs::write(
            &adjacent,
            r#"{"version":1,"formats":[{"ext":"side","group":"document","name":"Sidecar"}]}"#,
        )
        .unwrap();
        let exe = root.join("app.exe");
        assert_eq!(
            FormatTable::load_from(None, Some(exe.clone())).group("side"),
            Some("document")
        );
        let custom = root.join("custom.json");
        std::fs::write(
            &custom,
            r#"{"version":1,"formats":[{"ext":"xyz","group":"image","name":"Override"}]}"#,
        )
        .unwrap();
        assert_eq!(
            FormatTable::load_from(Some(custom), Some(exe)).group("xyz"),
            Some("image")
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn embedded_extensions_are_unique() {
        let f = FormatTable::parse(DEFAULT_FORMATS).unwrap();
        let mut unique = std::collections::HashSet::new();
        for format in &f.formats {
            assert!(unique.insert(format.ext.as_str()));
        }
    }
    #[test]
    fn hidden_extension_sql_is_generated_from_table() {
        let f = FormatTable::parse(DEFAULT_FORMATS).unwrap();
        assert!(sql_not_in("a.ext_lower", &f.hidden_extensions()).contains("'acr'"));
    }
    #[test]
    fn invalid_groups_are_rejected() {
        let json = r#"{"version":1,"formats":[{"ext":"zzz","group":"binary","name":"Z"}]}"#;
        assert!(FormatTable::parse(json).is_err());
    }
}
