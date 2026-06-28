use photo_viewer::core::motion_photo::{detect, MotionPhotoFormat};

/// Adobe XMP APP1 载荷签名（真实 Google 动图把 XMP 放在标准 APP1 段里）。
const XMP_APP1_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn fake_mp4(len: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; len.max(32)];
    bytes[0..4].copy_from_slice(&(24_u32.to_be_bytes()));
    bytes[4..8].copy_from_slice(b"ftyp");
    bytes[8..12].copy_from_slice(b"mp42");
    bytes
}

/// 构造一个标准 XMP APP1 段：`FF E1` + 长度(含自身 2 字节) + 签名 + XMP 报文。
fn app1_xmp_segment(xmp: &[u8]) -> Vec<u8> {
    let payload_len = XMP_APP1_SIG.len() + xmp.len();
    let seg_len = u16::try_from(payload_len + 2).unwrap();
    let mut seg = Vec::with_capacity(4 + payload_len);
    seg.extend_from_slice(&[0xFF, 0xE1]);
    seg.extend_from_slice(&seg_len.to_be_bytes());
    seg.extend_from_slice(XMP_APP1_SIG);
    seg.extend_from_slice(xmp);
    seg
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
    bytes.extend_from_slice(&app1_xmp_segment(xmp));
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
    bytes.extend_from_slice(&app1_xmp_segment(xmp));
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
