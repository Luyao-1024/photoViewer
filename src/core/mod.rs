pub mod backend;
pub mod db;
pub mod error;
pub mod media;
pub mod metadata;
pub mod section_model;

pub use backend::local::LocalBackend;
pub use db::{init_pool, run_migrations, DbPool};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use metadata::{extract as extract_metadata, RawMetadata};
pub use section_model::{group_items, GroupBy, MediaSection, SectionKey};