use crate::calendar::CalendarService;
use crate::db::Db;
use crate::display_metadata::DisplayMetadataCache;
use crate::exiftool::ExifTool;
use crate::file_ops::FileManager;
use crate::formats::FormatTable;
use crate::journal::CatalogWriter;
use crate::listing::ListingService;
use crate::metadata_view::MetadataView;
use crate::paths::AppInfo;
use crate::thumbs::ThumbService;
use std::sync::Arc;

mod ai;
mod app;
mod assets;
mod catalog;
mod faces;
mod files;
mod iptc;
mod metadata;
mod quality;
mod sources;
mod values;

pub use ai::*;
pub use app::*;
pub use assets::*;
pub use catalog::*;
pub use faces::*;
pub use files::*;
pub use iptc::*;
pub use metadata::*;
pub use quality::*;
pub use sources::*;
pub use values::*;

pub struct DatabaseState(pub Db);

pub struct AutoExifState(pub Arc<crate::exif_index::ExifIndex>);

/// Second, read-only connection for side-panel data (counts, trees, folder previews, calendar).
/// WAL lets it read while the main connection serves the folder listing, so background loading
/// never makes the grid wait for the catalogue lock.
pub struct BackgroundDbState(pub Db);

pub struct ExifState(pub Arc<ExifTool>);

pub struct ThumbnailState(pub Arc<ThumbService>);

pub struct ListingState(pub Arc<ListingService>);

pub struct FormatsState(pub Arc<FormatTable>);

pub struct MetadataViewState(pub Arc<MetadataView>);

pub struct DisplayMetadataState(pub Arc<DisplayMetadataCache>);

pub struct CalendarState(pub Arc<CalendarService>);

pub struct CatalogWriterState(pub Arc<CatalogWriter>);

pub struct FileManagerState(pub Arc<FileManager>);

pub struct AppInfoState(pub AppInfo);

// Register test suites here to preserve their original harness paths.
#[cfg(test)]
#[path = "tests/editors.rs"]
mod editor_tests;

#[cfg(test)]
#[path = "tests/sources.rs"]
mod source_tests;

#[cfg(test)]
include!("tests/similarity.rs");
