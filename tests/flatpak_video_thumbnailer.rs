#[test]
fn flatpak_manifest_installs_ffmpegthumbnailer_library_in_loader_path() {
    let manifest = std::fs::read_to_string("io.github.luyao_1024.photoviewer.yml")
        .expect("flatpak manifest should be readable");

    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "- -DCMAKE_INSTALL_LIBDIR=lib"),
        "ffmpegthumbnailer must install libffmpegthumbnailer.so into /app/lib; /app/lib64 is not in the Flatpak loader path"
    );
}
