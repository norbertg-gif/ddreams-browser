use super::values::slovak_count;
use super::*;
use crate::db::Db;
use rusqlite::Connection;
use serde_json::Value;
use std::sync::{Arc, Mutex};

// Include the domain suites in this module to retain the original test names.
include!("values.rs");
include!("folders.rs");
include!("writer.rs");
include!("categories.rs");
include!("keywords.rs");
include!("history.rs");
include!("fixtures.rs");
