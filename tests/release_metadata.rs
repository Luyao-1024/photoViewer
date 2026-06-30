const APP_ID: &str = "io.github.luyao_1024.photoviewer";
const VERSION: &str = "0.9.0";

#[test]
fn release_metadata_uses_git_derived_app_id_and_version() {
    let cargo = std::fs::read_to_string("Cargo.toml").expect("read Cargo.toml");
    assert!(
        cargo.contains(&format!("version = \"{VERSION}\"")),
        "Cargo package version must match the 0.9 release"
    );

    let manifest_path = format!("{APP_ID}.yml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read Flatpak manifest");
    assert!(
        manifest.contains(&format!("app-id: {APP_ID}")),
        "Flatpak manifest app-id must match the GitHub-derived app-id"
    );
    assert!(
        manifest.contains(&format!("data/{APP_ID}.desktop")),
        "Flatpak manifest should install the renamed desktop file"
    );
    assert!(
        manifest.contains(&format!("data/{APP_ID}.metainfo.xml")),
        "Flatpak manifest should install the renamed metainfo file"
    );
    assert!(
        manifest.contains("cargo-sources.json"),
        "Flatpak manifest should include generated cargo sources for offline builds"
    );
    assert!(
        !manifest.contains("--share=network"),
        "release Flatpak manifest must not require build-time network access"
    );
    assert!(
        manifest.contains("CARGO_NET_OFFLINE: \"true\""),
        "release Flatpak manifest should force cargo offline mode"
    );

    let desktop_path = format!("data/{APP_ID}.desktop");
    let desktop = std::fs::read_to_string(&desktop_path).expect("read desktop file");
    assert!(
        desktop.contains(&format!("Icon={APP_ID}")),
        "desktop icon name must match the app-id"
    );

    let metainfo_path = format!("data/{APP_ID}.metainfo.xml");
    let metainfo = std::fs::read_to_string(&metainfo_path).expect("read metainfo");
    assert!(
        metainfo.contains(&format!("<id>{APP_ID}</id>")),
        "metainfo id must match the app-id"
    );
    assert!(
        metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )),
        "metainfo launchable must point at the renamed desktop file"
    );
    assert!(
        metainfo.contains(&format!("<release version=\"{VERSION}\"")),
        "metainfo release list must include the 0.9 release"
    );
}

#[test]
fn active_release_files_do_not_reference_old_app_id() {
    let active_files = [
        "Cargo.toml",
        "meson.build",
        "README.md",
        "docs/development.md",
        "run-flatpak.sh",
        "tools/visual-check-x11.sh",
        "src/app.rs",
        "src/core/trash.rs",
        "data/resources.gresource.xml",
    ];

    for path in active_files {
        let text = std::fs::read_to_string(path).expect("read active release file");
        assert!(
            !text.contains("org.gnome.PhotoViewer"),
            "{path} still references the old GNOME-style app-id"
        );
    }
}
