use std::process::Command;

#[test]
fn runner_documents_focused_trace_chain_switches() {
    let output = Command::new("bash")
        .arg("run-flatpak.sh")
        .arg("--help")
        .output()
        .expect("run Flatpak runner help");

    assert!(
        output.status.success(),
        "runner help should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for required in [
        "--trace-chain, -T",
        "startup, database, scan, filesystem",
        "thumbnail, mutation, all",
    ] {
        assert!(help.contains(required), "runner help missing {required}");
    }
}
