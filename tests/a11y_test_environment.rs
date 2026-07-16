use std::process::Command;

#[test]
fn at_spi_test_environment_starts_bus_and_registry() {
    let output = Command::new("bash")
        .arg("tools/with-at-spi.sh")
        .arg("--check")
        .output()
        .expect("verify the AT-SPI test environment");

    assert!(
        output.status.success(),
        "AT-SPI test environment failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("AT-SPI bus and registry ready."),
        "AT-SPI test environment did not confirm both services\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn at_spi_test_environment_documents_its_check_mode() {
    let output = Command::new("bash")
        .arg("tools/with-at-spi.sh")
        .arg("--help")
        .output()
        .expect("show AT-SPI helper help");

    assert!(output.status.success(), "AT-SPI helper help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--check") && stdout.contains("org.a11y.Bus"),
        "AT-SPI helper help should describe the verified services\nstdout:\n{stdout}"
    );
}

#[test]
fn at_spi_test_environment_reuses_a_verified_parent_session() {
    let helper =
        std::fs::read_to_string("tools/with-at-spi.sh").expect("read AT-SPI helper source");

    assert!(
        helper.contains("PHOTO_VIEWER_AT_SPI_SESSION_READY")
            && helper.contains("export PHOTO_VIEWER_AT_SPI_SESSION_READY=1"),
        "nested test commands should reuse the verified parent AT-SPI session"
    );
}
