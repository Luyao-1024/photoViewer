#[test]
fn flatpak_manifest_grants_audio_socket_for_video_playback() {
    let manifest = std::fs::read_to_string("io.github.luyao_1024.photoviewer.yml")
        .expect("flatpak manifest should be readable");

    assert!(
        manifest.lines().any(|line| line.trim() == "- --socket=pulseaudio"),
        "Flatpak manifest must grant the PulseAudio/PipeWire audio socket so GtkVideo can output sound"
    );
}

#[test]
fn flatpak_development_runner_grants_audio_socket_for_video_playback() {
    let runner = std::fs::read_to_string("run-flatpak.sh")
        .expect("development Flatpak runner should be readable");

    assert!(
        runner.contains("--socket=pulseaudio"),
        "run-flatpak.sh must grant the same audio socket as the installed Flatpak"
    );
}
