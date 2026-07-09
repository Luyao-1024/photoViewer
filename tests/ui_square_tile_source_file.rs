use std::fs;
use std::path::Path;

#[test]
fn square_tile_widget_lives_in_own_source_file() {
    let tile_path = Path::new("src/ui/square_tile.rs");
    assert!(
        tile_path.exists(),
        "SquareTile should live in its own source file"
    );

    let tile_source = fs::read_to_string(tile_path).expect("square_tile.rs should be readable");
    assert!(tile_source.contains("pub struct SquareTile"));
    assert!(tile_source.contains("const NAME: &'static str = \"PvSquareTile\""));

    let grid_source =
        fs::read_to_string("src/ui/media_grid.rs").expect("media_grid.rs should be readable");
    assert!(
        !grid_source.contains("pub mod square_tile"),
        "media_grid.rs should not inline the SquareTile widget module"
    );
}
