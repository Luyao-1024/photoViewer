use super::*;

#[test]
fn subprocess_timeout_terminates_the_process_group() {
    let start = Instant::now();
    let error = output(
        Command::new("sh").args(["-c", "sleep 30 & wait"]),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn subprocess_returns_output_and_status() {
    let output = output(
        Command::new("sh").args(["-c", "printf out; printf err >&2; exit 7"]),
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
}
