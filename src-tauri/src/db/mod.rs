mod assets;
mod catalog;
mod counts;
mod filters;
mod folders;
mod open;

pub use assets::*;
pub use catalog::*;
pub use counts::*;
pub use filters::*;
pub use folders::*;
pub use open::*;

// Register test suites here to preserve their original harness paths.
#[cfg(test)]
#[path = "tests/open.rs"]
mod catalog_open_tests;

#[cfg(test)]
#[path = "tests/folders.rs"]
mod folder_path_tests;

#[cfg(test)]
include!("tests/assets.rs");

#[cfg(test)]
#[path = "tests/benches.rs"]
mod benches;
