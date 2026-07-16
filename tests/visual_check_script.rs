use std::process::Command;

#[test]
fn visual_check_script_self_test_passes() {
    let output = Command::new("bash")
        .arg("tools/visual-check-x11.sh")
        .arg("--self-test")
        .output()
        .expect("run visual check script self-test");

    assert!(
        output.status.success(),
        "visual check script self-test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn visual_check_script_skips_wayland() {
    let output = Command::new("bash")
        .arg("tools/visual-check-x11.sh")
        .env("XDG_SESSION_TYPE", "wayland")
        .output()
        .expect("run visual check script in wayland mode");

    assert!(
        output.status.success(),
        "wayland skip should exit successfully\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Skipping visual check: Wayland"),
        "wayland skip message missing\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn visual_check_script_keyboard_smoke_injects_ctrl_f_and_rejects_no_op() {
    let output = Command::new("bash")
        .arg("tools/visual-check-x11.sh")
        .arg("--help")
        .output()
        .expect("show visual check script help");

    assert!(output.status.success(), "visual check help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--keyboard-smoke"),
        "visual check help should expose the non-destructive keyboard smoke option"
    );

    let script = std::fs::read_to_string("tools/visual-check-x11.sh")
        .expect("read visual check script source");
    assert!(
        script.contains("xdotool windowfocus \"$WINDOW_ID\"")
            && script.contains("xdotool key ctrl+f")
            && !script.contains("xdotool key --window \"$WINDOW_ID\" ctrl+f"),
        "keyboard smoke should focus the Flatpak window and inject Ctrl+F through XTEST"
    );
    assert!(
        script.contains("wait_for_keyboard_router")
            && script.contains("photos grid renderer initialized"),
        "keyboard smoke should wait for the production router instead of racing async startup"
    );
    assert!(
        script.contains("cmp -s \"$SCREENSHOT\" \"$SEARCH_SCREENSHOT\""),
        "keyboard smoke should fail when Ctrl+F leaves the captured window unchanged"
    );
    assert!(
        script.contains("A11Y_SMOKE == 0")
            && script.contains("AT-SPI assertion is the authoritative signal"),
        "semantic smoke should rely on the accessibility tree when Xvfb raster output is identical"
    );
}

#[test]
fn visual_check_script_can_assert_at_spi_search_semantics() {
    let output = Command::new("bash")
        .arg("tools/visual-check-x11.sh")
        .arg("--help")
        .output()
        .expect("show visual check script help");

    assert!(output.status.success(), "visual check help should succeed");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("--a11y-smoke"),
        "visual check help should expose the AT-SPI semantic smoke option"
    );

    let script = std::fs::read_to_string("tools/visual-check-x11.sh")
        .expect("read visual check script source");
    assert!(
        script.contains("A11Y_SMOKE=1")
            && script.contains("KEYBOARD_SMOKE=1")
            && script.contains("assert-at-spi.py")
            && script.contains("timeout \"$((TIMEOUT_SECONDS + 5))s\""),
        "AT-SPI semantic checks should open Search first, then inspect its tree"
    );
}

#[test]
fn visual_check_script_terminates_its_own_flatpak_process_group() {
    let script = std::fs::read_to_string("tools/visual-check-x11.sh")
        .expect("read visual check script source");

    assert!(
        script.contains("setsid ./run-flatpak.sh") && script.contains("kill -- \"-$APP_PID\""),
        "visual checks must not leave a Flatpak window for later checks to target"
    );
}

#[test]
fn at_spi_semantics_probe_documents_the_required_search_contract() {
    let output = Command::new("python3")
        .arg("tools/assert-at-spi.py")
        .arg("--help")
        .output()
        .expect("show AT-SPI semantics probe help");

    assert!(
        output.status.success(),
        "AT-SPI semantics probe help should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("roles, names, and focus") && stdout.contains("--timeout"),
        "AT-SPI semantics probe should document its contract\nstdout:\n{stdout}"
    );

    let probe = std::fs::read_to_string("tools/assert-at-spi.py")
        .expect("read AT-SPI semantics probe source");
    for required in [
        "Photo Viewer",
        "照片查看器",
        "全部",
        "文件名",
        "日期",
        "ROLE_TOGGLE_BUTTON",
        "ROLE_ENTRY",
        "STATE_FOCUSED",
    ] {
        assert!(
            probe.contains(required),
            "AT-SPI semantics probe should require {required:?}"
        );
    }
}

#[test]
fn visual_check_script_bootstraps_at_spi_before_launching_the_app() {
    let script = std::fs::read_to_string("tools/visual-check-x11.sh")
        .expect("read visual check script source");

    assert!(
        script.contains("with-at-spi.sh") && script.contains("PHOTO_VIEWER_AT_SPI_READY"),
        "visual checks should establish AT-SPI before launching the Flatpak app"
    );
}
