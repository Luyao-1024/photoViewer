//! UI module: top-level window and (later) pages/widgets.

pub mod album_detail_page;
pub mod albums_page;
pub mod edit_panel;
pub mod editor_page;
pub mod empty_states;
pub mod media_grid;
pub mod photo_tile;
pub mod photos_page;
pub mod section_header;
pub mod trash_page;
pub mod viewer_page;
pub mod window;

pub use album_detail_page::AlbumDetailPage;
pub use albums_page::AlbumsPage;
pub use editor_page::EditorPage;
pub use media_grid::MediaGrid;
pub use photo_tile::PhotoTile;
pub use photos_page::PhotosPage;
pub use section_header::SectionHeader;
pub use trash_page::TrashPage;
pub use viewer_page::{NavDelta, ViewerPage, NAV_POP};
pub use window::MainWindow;
