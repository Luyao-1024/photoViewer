#[test]
fn flatpak_manifest_allows_gtk_to_discover_the_at_spi_bus() {
    let manifest = std::fs::read_to_string("io.github.luyao_1024.photoviewer.yml")
        .expect("Flatpak manifest should be readable");

    assert!(
        manifest
            .lines()
            .any(|line| line.trim() == "- --talk-name=org.a11y.Bus"),
        "Flatpak must let GTK discover the AT-SPI bus through org.a11y.Bus"
    );
}

#[test]
fn flatpak_development_runner_allows_gtk_to_discover_the_at_spi_bus() {
    let runner = std::fs::read_to_string("run-flatpak.sh")
        .expect("development Flatpak runner should be readable");

    assert!(
        runner.contains("--talk-name=org.a11y.Bus"),
        "development Flatpak runs must grant GTK access to org.a11y.Bus"
    );
}
