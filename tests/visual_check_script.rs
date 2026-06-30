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
