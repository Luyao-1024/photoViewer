use std::path::Path;
use std::process::Command;

fn compile_blueprint(src: &str, dst: &str) {
    let tmp = format!("{dst}.tmp");
    let status = Command::new("blueprint-compiler")
        .args(["compile", "--output"])
        .arg(&tmp)
        .arg(src)
        .status()
        .expect("failed to invoke blueprint-compiler (is it installed?)");

    assert!(status.success(), "blueprint-compiler failed for {}", src);

    let changed = match (std::fs::read(dst), std::fs::read(&tmp)) {
        (Ok(existing), Ok(new)) => existing != new,
        (Err(_), Ok(_)) => true,
        (_, Err(e)) => panic!("failed to read compiled blueprint output {tmp}: {e}"),
    };
    if changed {
        std::fs::rename(&tmp, dst).expect("failed to update compiled blueprint output");
    } else {
        std::fs::remove_file(&tmp).expect("failed to remove unchanged blueprint temp output");
    }

    // Re-run build.rs if the source changes.
    println!("cargo:rerun-if-changed={src}");
}

fn main() {
    // 1. Compile Blueprint → .ui
    let blueprint_files = [
        "data/ui/window.blp",
        "data/ui/photos-page.blp",
        "data/ui/album-detail-page.blp",
        "data/ui/media-grid.blp",
        "data/ui/mode-selector.blp",
        "data/ui/photo-tile.blp",
        "data/ui/section-header.blp",
        "data/ui/viewer-page.blp",
        "data/ui/trash-page.blp",
        "data/ui/editor-panel.blp",
    ];
    for blp in blueprint_files {
        let ui_path = blp.replace(".blp", ".ui");
        compile_blueprint(blp, &ui_path);
    }

    // 2. Compile GResource (must contain all .ui files + icons)
    glib_build_tools::compile_resources(
        &["data"],                          // resource base dir
        "data/resources.gresource.xml",     // resource manifest
        "photo_viewer_resources.gresource", // resource name (C identifier)
    );

    // Re-run if the gresource manifest changes
    println!(
        "cargo:rerun-if-changed={}",
        Path::new("data/resources.gresource.xml").display()
    );
}
