pub mod album_ops;
pub mod albums;
pub mod backend;
pub mod bootstrap;
pub mod cache;
pub mod db;
pub mod edit;
pub mod error;
pub mod i18n;
pub mod media;
pub mod media_change_notifier;
pub mod metadata;
pub mod notify_watcher;
pub mod section_model;
pub mod thumbnails;
pub mod trash;

pub use album_ops::{add_to_album, AlbumOpMode};
pub use albums::{refresh as refresh_albums, Album};
pub use backend::local::LocalBackend;
pub use db::{init_pool, run_migrations, DbPool};
pub use edit::{
    CropRect, EditCategory, EditOperation, EditRegistry, EditState, ParamValue, Rotation,
};
pub use error::{AppError, Result};
pub use media::{MediaItem, NewMediaItem};
pub use media_change_notifier::{MediaChangeEvent, MediaChangeNotifier};
pub use metadata::{extract as extract_metadata, RawMetadata};
pub use section_model::{group_items, GroupBy, MediaSection, SectionKey};
pub use thumbnails::{ThumbnailLoader, ThumbnailRequest, ThumbnailSize};
pub use trash::{delete_permanently, move_to_trash, restore_from_trash};
