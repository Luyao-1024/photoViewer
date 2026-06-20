//! UI module: top-level window and (later) pages/widgets.

pub mod albums_page;
pub mod media_grid;
pub mod photo_tile;
pub mod photos_page;
pub mod section_header;
pub mod viewer_page;
pub mod window;

pub use albums_page::AlbumsPage;
pub use media_grid::MediaGrid;
pub use photo_tile::PhotoTile;
pub use photos_page::PhotosPage;
pub use section_header::SectionHeader;
pub use viewer_page::{NavDelta, ViewerPage, NAV_POP};
pub use window::MainWindow;
