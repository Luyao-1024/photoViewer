use std::path::{Path, PathBuf};
use std::process::Command;

fn compile_blueprint(src: &str, dst: &Path) {
    let status = Command::new("blueprint-compiler")
        .args(["compile", "--output"])
        .arg(dst)
        .arg(src)
        .status()
        .expect("failed to invoke blueprint-compiler (is it installed?)");

    assert!(status.success(), "blueprint-compiler failed for {}", src);

    // Re-run build.rs if the source changes.
    println!("cargo:rerun-if-changed={src}");
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let generated_ui_dir = out_dir.join("ui");
    std::fs::create_dir_all(&generated_ui_dir).expect("failed to create generated UI directory");
    // 0. Compile C shim for libjpeg IDCT scaling (uses system libjpeg-turbo)
    cc::Build::new()
        .file("src/core/jpeg_scale_shim.c")
        .compile("jpeg_scale_shim");
    println!("cargo:rustc-link-lib=jpeg");

    // 1. Compile Blueprint → .ui
    let blueprint_files = [
        "data/ui/window.blp",
        "data/ui/photos-page.blp",
        "data/ui/album-detail-page.blp",
        "data/ui/media-grid.blp",
        "data/ui/virtual-media-grid.blp",
        "data/ui/mode-selector.blp",
        "data/ui/search-page.blp",
        "data/ui/viewer-page.blp",
        "data/ui/trash-page.blp",
        "data/ui/editor-panel.blp",
    ];
    for blp in blueprint_files {
        let file_name = Path::new(blp)
            .file_stem()
            .expect("blueprint has a file stem");
        let ui_path = generated_ui_dir.join(file_name).with_extension("ui");
        compile_blueprint(blp, &ui_path);
    }

    // 2. Compile GResource (must contain all .ui files + icons)
    glib_build_tools::compile_resources(
        &[out_dir],                         // generated resource base dir
        "data/resources.gresource.xml",     // resource manifest
        "photo_viewer_resources.gresource", // resource name (C identifier)
    );

    // Re-run if the gresource manifest changes
    println!(
        "cargo:rerun-if-changed={}",
        Path::new("data/resources.gresource.xml").display()
    );
}
