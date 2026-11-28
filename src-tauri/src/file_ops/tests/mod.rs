use super::filesystem::parse_recycle_info;
use super::transfer::copy_then_remove;
use super::*;
use crate::db::Db;
use crate::journal::CatalogWriter;
use crate::listing::ListingService;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs::{self, File, FileTimes};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

// Include the domain suites in this module to retain the original test names.
include!("transfer.rs");
include!("rename.rs");
include!("quarantine.rs");
include!("delete.rs");
include!("folders.rs");
include!("fixtures.rs");
