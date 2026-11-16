use serde::Serialize;
use std::{collections::HashMap, env, ffi::OsString, path::{Path, PathBuf}};

const DEV_EXIFTOOL: &str = r"G:\Programovanie\ddreams-browser\tools\exiftool\exiftool.exe";
const DEV_CACHE: &str = r"G:\ddreams-work\thumb-cache";
const DEV_AI_PYTHON: &str = r"G:\DDreams\modules\python\Scripts\python.exe";
const DEV_AI_MODEL: &str = r"G:\DDreams\modules\ai\model";

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub portable: bool,
    pub catalog: PathBuf,
    pub backups: PathBuf,
    pub cache: PathBuf,
    pub exiftool: PathBuf,
    pub formats: Option<PathBuf>,
    pub metadata_view: Option<PathBuf>,
    pub settings: PathBuf,
    pub ai_script: PathBuf,
    pub ai_model: PathBuf,
    pub ai_python: Option<PathBuf>,
    pub ai_db: PathBuf,
    pub ai_log: PathBuf,
    pub readonly: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub root: String,
    pub catalog: String,
    pub backups: String,
    pub cache: String,
    pub exiftool: String,
    pub formats: String,
    pub metadata_view: String,
    pub settings: String,
    pub portable: bool,
    pub readonly: bool,
}

impl Paths {
    pub fn resolve() -> Self {
        let executable = env::current_exe().unwrap_or_default();
        let cwd = env::current_dir().unwrap_or_default();
        let vars = env::vars_os().collect::<HashMap<_, _>>();
        Self::resolve_with(&executable, &cwd, &vars)
    }

    pub fn resolve_with(executable: &Path, cwd: &Path, vars: &HashMap<OsString, OsString>) -> Self {
        let root = executable.parent().unwrap_or(executable).to_path_buf();
        let portable_catalog = root.join("catalog").join("catalog.sqlite");
        let portable = portable_catalog.is_file() || root.join("ddreams.portable").exists();
        let catalog = env_path(vars, "DDREAMS_CATALOG").unwrap_or_else(|| {
            if portable { portable_catalog.clone() } else { dev_catalog_path(executable, cwd) }
        });
        let backups = if portable { root.join("backups") } else { catalog.parent().unwrap_or(cwd).join("backups") };
        let cache = env_path(vars, "DDREAMS_CACHE").unwrap_or_else(|| {
            if portable { root.join("cache") } else { PathBuf::from(DEV_CACHE) }
        });
        let exiftool = env_path(vars, "DDREAMS_EXIFTOOL").unwrap_or_else(|| {
            if portable { root.join("modules").join("exiftool").join("exiftool.exe") } else { PathBuf::from(DEV_EXIFTOOL) }
        });
        let formats = env_path(vars, "DDREAMS_FORMATS").or_else(|| {
            let candidate = if portable { root.join("modules").join("formats.json") } else { root.join("formats.json") };
            candidate.is_file().then_some(candidate)
        });
        let metadata_view = env_path(vars, "DDREAMS_METADATA_VIEW").or_else(|| {
            let candidate = if portable { root.join("modules").join("metadata_view.json") } else { root.join("metadata_view.json") };
            candidate.is_file().then_some(candidate)
        });
        let settings = root.join("settings.json");
        let ai_dir = if portable { root.join("modules").join("ai") } else { cwd_repo(&catalog, cwd).join("ai") };
        let ai_script = ai_dir.join("ddreams_ai.py");
        let ai_model = if portable { ai_dir.join("model") } else { PathBuf::from(DEV_AI_MODEL) };
        let ai_python = if portable { read_ai_python(&ai_dir.join("ai.json")) } else { Some(PathBuf::from(DEV_AI_PYTHON)) };
        let ai_db = catalog.parent().unwrap_or(cwd).join("ai.sqlite");
        let ai_log = catalog.parent().unwrap_or(cwd).join("ai.log");
        let readonly = vars.get(&OsString::from("DDREAMS_READONLY")).is_some_and(|value| value == "1");
        Self { root, portable, catalog, backups, cache, exiftool, formats, metadata_view, settings, ai_script, ai_model, ai_python, ai_db, ai_log, readonly }
    }

    pub fn ensure_runtime_dirs(&self) -> std::io::Result<()> {
        if let Some(parent) = self.catalog.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::create_dir_all(&self.backups)?;
        std::fs::create_dir_all(&self.cache)?;
        Ok(())
    }

    pub fn info(&self) -> AppInfo {
        AppInfo {
            root: self.root.display().to_string(), catalog: self.catalog.display().to_string(),
            backups: self.backups.display().to_string(), cache: self.cache.display().to_string(),
            exiftool: self.exiftool.display().to_string(), formats: display_optional(&self.formats),
            metadata_view: display_optional(&self.metadata_view), settings: self.settings.display().to_string(),
            portable: self.portable, readonly: self.readonly,
        }
    }
}

fn cwd_repo(catalog: &Path, cwd: &Path) -> PathBuf {
    catalog.parent().and_then(Path::parent).unwrap_or(cwd).to_path_buf()
}

fn read_ai_python(path: &Path) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    value.get("python")?.as_str().filter(|value| !value.trim().is_empty()).map(PathBuf::from)
}

fn display_optional(path: &Option<PathBuf>) -> String {
    path.as_ref().map(|value| value.display().to_string()).unwrap_or_else(|| "Vstavané predvolené hodnoty".into())
}

fn env_path(vars: &HashMap<OsString, OsString>, name: &str) -> Option<PathBuf> {
    vars.get(&OsString::from(name)).filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn dev_catalog_path(executable: &Path, cwd: &Path) -> PathBuf {
    executable.parent().into_iter().flat_map(Path::ancestors)
        .chain(cwd.ancestors())
        .map(|directory| directory.join(".local").join("catalog.sqlite"))
        .find(|path| path.exists())
        .unwrap_or_else(|| cwd.join(".local").join("catalog.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let path = env::temp_dir().join(format!("ddreams-paths-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn detects_portable_catalog_and_marker_layout() {
        let root = temp_root();
        let exe = root.join("ddreams-browser.exe");
        let catalog = root.join("catalog/catalog.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(&catalog, []).unwrap();
        let paths = Paths::resolve_with(&exe, &root, &HashMap::new());
        assert!(paths.portable);
        assert_eq!(paths.catalog, catalog);
        assert_eq!(paths.backups, root.join("backups"));
        std::fs::remove_file(&catalog).unwrap();
        std::fs::write(root.join("ddreams.portable"), []).unwrap();
        assert!(Paths::resolve_with(&exe, &root, &HashMap::new()).portable);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn development_paths_keep_repo_catalog_defaults() {
        let root = temp_root();
        let exe = root.join("src-tauri/target/release/ddreams-browser.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join(".local")).unwrap();
        std::fs::write(root.join(".local/catalog.sqlite"), []).unwrap();
        let paths = Paths::resolve_with(&exe, &root, &HashMap::new());
        assert!(!paths.portable);
        assert_eq!(paths.catalog, root.join(".local/catalog.sqlite"));
        assert_eq!(paths.cache, PathBuf::from(DEV_CACHE));
        assert_eq!(paths.exiftool, PathBuf::from(DEV_EXIFTOOL));
        assert_eq!(paths.ai_script, root.join("ai/ddreams_ai.py"));
        assert_eq!(paths.ai_model, PathBuf::from(DEV_AI_MODEL));
        assert_eq!(paths.ai_python, Some(PathBuf::from(DEV_AI_PYTHON)));
        assert_eq!(paths.ai_db, root.join(".local/ai.sqlite"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn portable_ai_paths_read_python_from_module_config() {
        let root = temp_root();
        let catalog = root.join("catalog/catalog.sqlite");
        let ai_dir = root.join("modules/ai");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&ai_dir).unwrap();
        std::fs::write(&catalog, []).unwrap();
        std::fs::write(ai_dir.join("ai.json"), br#"{"python":"C:\\python.exe"}"#).unwrap();
        let paths = Paths::resolve_with(&root.join("ddreams.exe"), &root, &HashMap::new());
        assert_eq!(paths.ai_script, ai_dir.join("ddreams_ai.py"));
        assert_eq!(paths.ai_model, ai_dir.join("model"));
        assert_eq!(paths.ai_python, Some(PathBuf::from(r"C:\python.exe")));
        assert_eq!(paths.ai_db, root.join("catalog/ai.sqlite"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn environment_overrides_each_path_and_readonly_mode() {
        let root = temp_root();
        let exe = root.join("ddreams-browser.exe");
        let mut vars = HashMap::new();
        for (key, value) in [
            ("DDREAMS_CATALOG", "X:/catalog.sqlite"), ("DDREAMS_CACHE", "X:/cache"),
            ("DDREAMS_EXIFTOOL", "X:/exiftool.exe"), ("DDREAMS_FORMATS", "X:/formats.json"),
            ("DDREAMS_METADATA_VIEW", "X:/metadata.json"), ("DDREAMS_READONLY", "1"),
        ] { vars.insert(OsString::from(key), OsString::from(value)); }
        let paths = Paths::resolve_with(&exe, &root, &vars);
        assert_eq!(paths.catalog, PathBuf::from("X:/catalog.sqlite"));
        assert_eq!(paths.cache, PathBuf::from("X:/cache"));
        assert_eq!(paths.exiftool, PathBuf::from("X:/exiftool.exe"));
        assert_eq!(paths.formats, Some(PathBuf::from("X:/formats.json")));
        assert_eq!(paths.metadata_view, Some(PathBuf::from("X:/metadata.json")));
        assert!(paths.readonly);
        let _ = std::fs::remove_dir_all(root);
    }
}
