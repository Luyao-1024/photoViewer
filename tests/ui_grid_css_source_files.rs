use std::fs;
use std::path::Path;

#[test]
fn grid_css_is_split_into_source_files() {
    let expected = [
        ("data/css/base.css", ".glass-segmented"),
        ("data/css/liquid.css", "backdrop-filter"),
        ("data/css/plain.css", ".glass-toolbar-button"),
        ("data/css/a11y.css", "accessibility fallbacks"),
    ];

    for (path, marker) in expected {
        let path = Path::new(path);
        assert!(path.exists(), "missing CSS source file {}", path.display());
        let contents = fs::read_to_string(path).expect("CSS source should be readable");
        assert!(
            contents.contains(marker),
            "{} should contain marker {marker:?}",
            path.display()
        );
    }
}
