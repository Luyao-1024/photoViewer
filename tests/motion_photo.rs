use photo_viewer::core::motion_photo::{detect, MotionPhotoFormat};

fn fake_mp4(len: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; len.max(32)];
    bytes[0..4].copy_from_slice(&(24_u32.to_be_bytes()));
    bytes[4..8].copy_from_slice(b"ftyp");
    bytes[8..12].copy_from_slice(b"mp42");
    bytes
}

#[test]
fn detects_google_micro_video_offset_from_file_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("micro.jpg");
    let video = fake_mp4(128);
    let xmp = br#"
        <x:xmpmeta>
          <rdf:Description
            GCamera:MicroVideo="1"
            GCamera:MicroVideoOffset="128"
            GCamera:MicroVideoPresentationTimestampUs="910546"/>
        </x:xmpmeta>
    "#;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\xff\xd8");
    bytes.extend_from_slice(xmp);
    bytes.extend_from_slice(b"\xff\xd9");
    let video_offset = bytes.len() as u64;
    bytes.extend_from_slice(&video);
    std::fs::write(&path, bytes).unwrap();

    let info = detect(&path).expect("micro video should be detected");
    assert_eq!(info.format, MotionPhotoFormat::GoogleMicroVideo);
    assert_eq!(info.video_offset, video_offset);
    assert_eq!(info.video_length, 128);
    assert_eq!(info.presentation_timestamp_us, Some(910_546));
}

#[test]
fn detects_google_motion_photo_container_after_gain_map() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("container.jpg");
    let gain_map = vec![0x7a; 64];
    let video = fake_mp4(256);
    let xmp = br#"
        <x:xmpmeta>
          <rdf:Description
            GCamera:MotionPhoto="1"
            GCamera:MotionPhotoPresentationTimestampUs="1048025">
            <Container:Directory>
              <rdf:Seq>
                <rdf:li rdf:parseType="Resource">
                  <Container:Item Item:Mime="image/jpeg" Item:Semantic="Primary" Item:Length="0"/>
                </rdf:li>
                <rdf:li rdf:parseType="Resource">
                  <Container:Item Item:Mime="image/jpeg" Item:Semantic="GainMap" Item:Length="64"/>
                </rdf:li>
                <rdf:li rdf:parseType="Resource">
                  <Container:Item Item:Mime="video/mp4" Item:Semantic="MotionPhoto" Item:Length="256"/>
                </rdf:li>
              </rdf:Seq>
            </Container:Directory>
          </rdf:Description>
        </x:xmpmeta>
    "#;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\xff\xd8");
    bytes.extend_from_slice(xmp);
    bytes.extend_from_slice(b"\xff\xd9");
    let gain_map_offset = bytes.len() as u64;
    bytes.extend_from_slice(&gain_map);
    let video_offset = bytes.len() as u64;
    bytes.extend_from_slice(&video);
    std::fs::write(&path, bytes).unwrap();

    let info = detect(&path).expect("container motion photo should be detected");
    assert_eq!(info.format, MotionPhotoFormat::GoogleMotionPhotoContainer);
    assert_eq!(info.video_offset, video_offset);
    assert_eq!(info.video_length, 256);
    assert_eq!(info.gain_map_offset, Some(gain_map_offset));
    assert_eq!(info.gain_map_length, Some(64));
    assert_eq!(info.presentation_timestamp_us, Some(1_048_025));
}

#[test]
fn ignores_plain_jpeg_without_motion_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.jpg");
    std::fs::write(&path, b"\xff\xd8plain\xff\xd9").unwrap();

    assert!(detect(&path).is_none());
}
