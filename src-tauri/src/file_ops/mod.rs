use crate::db::Db;
use crate::journal::{AssetIdRemap, CatalogWriter, FileCatalogMove};
use crate::listing::ListingService;
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

mod delete;
mod filesystem;
mod folders;
mod quarantine_ops;
mod rename;
mod sources;
mod transfer;

pub use delete::*;
pub use filesystem::*;
pub use folders::*;
pub use rename::validate_windows_name;
pub(crate) use sources::sidecar_candidates;
pub use transfer::*;

use filesystem::volume_key;
use rename::key;
use sources::sidecar_pairs;
use transfer::{combine_error, move_transfers, rollback_moves};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "./"))]
pub struct FileOperationResult {
    pub change_id: Option<i64>,
    pub summary: String,
    pub changed_count: usize,
    pub skipped_count: usize,
    pub remapped_ids: Vec<AssetIdRemap>,
    pub affected_ids: Vec<i64>,
    pub affected_folder_paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct Transfer {
    from: PathBuf,
    to: PathBuf,
}

#[derive(Clone, Debug)]
struct Source {
    id: i64,
    requested_id: i64,
    path: PathBuf,
    name: String,
    size: u64,
    modified: SystemTime,
}

#[derive(Clone, Debug)]
struct Plan {
    source: Source,
    target: PathBuf,
    pairs: Vec<Transfer>,
    conflict: bool,
}

pub struct FileManager {
    db: Db,
    listing: Arc<ListingService>,
    writer: Arc<CatalogWriter>,
    readonly: bool,
    fs: Arc<dyn FileSystemOps>,
    operation: Mutex<()>,
    review: Mutex<Option<crate::quarantine::Review>>,
    quarantine_delete: Mutex<Vec<crate::quarantine::Snapshot>>,
}

#[cfg(all(test, windows))]
#[test]
fn explorer_select_quotes_path_with_spaces_and_comma() {
    assert_eq!(
        explorer_select_arg(Path::new(r"F:\Moje fotky, 2025")),
        r#"/select,"F:\Moje fotky, 2025""#
    );
}

impl FileManager {
    pub fn new(db: Db, listing: Arc<ListingService>, writer: Arc<CatalogWriter>, readonly: bool) -> Arc<Self> {
        Arc::new(Self {
            db,
            listing,
            writer,
            readonly,
            fs: Arc::new(NativeFileSystem),
            operation: Mutex::new(()),
            review: Mutex::new(None),
            quarantine_delete: Mutex::default(),
        })
    }

    #[cfg(test)]
    fn with_fs(
        db: Db,
        listing: Arc<ListingService>,
        writer: Arc<CatalogWriter>,
        readonly: bool,
        fs: Arc<dyn FileSystemOps>,
    ) -> Self {
        Self {
            db,
            listing,
            writer,
            readonly,
            fs,
            operation: Mutex::new(()),
            review: Mutex::new(None),
            quarantine_delete: Mutex::default(),
        }
    }

    fn writable(&self) -> Result<(), String> {
        if self.readonly {
            Err("Katalóg je iba na čítanie".into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.operation.lock().map_err(|e| e.to_string())
    }

    pub(crate) fn sync_incoming(&self, path: &str) -> Result<(), String> {
        self.writer.sync_incoming(path)
    }
}

#[cfg(test)]
mod tests;
