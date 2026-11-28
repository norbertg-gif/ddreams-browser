use std::sync::{Arc, Mutex};

mod categories;
mod folders;
mod history;
mod keywords;
mod values;
mod writer;

pub use categories::*;
pub use folders::*;
pub use history::*;
pub use keywords::*;
pub use values::*;
pub use writer::*;

type ChangeHook = Arc<dyn Fn(&[&str]) + Send + Sync>;

pub struct CatalogWriter {
    db: Arc<Mutex<rusqlite::Connection>>,
    writable: bool,
    change_hook: Mutex<Option<ChangeHook>>,
    rules: crate::folder_rules::FolderRules,
    incoming: Mutex<String>,
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

#[cfg(test)]
mod tests;
