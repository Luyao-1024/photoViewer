use std::{fs, path::Path};

#[test]
fn thumbnail_cache_helpers_live_in_cache_module() {
    let cache_path = Path::new("src/core/thumbnails/cache.rs");
    assert!(
        cache_path.exists(),
        "thumbnail cache helpers should live in src/core/thumbnails/cache.rs"
    );

    let cache = fs::read_to_string(cache_path).expect("cache module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn cache_key_str",
        "pub(in crate::core::thumbnails) fn cache_stem_for",
        "pub(in crate::core::thumbnails) fn existing_cache_path",
        "pub(in crate::core::thumbnails) fn load_pixbuf_sync_or_remove",
    ] {
        assert!(cache.contains(marker), "cache.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(
        root.contains("mod cache;"),
        "thumbnail root should declare the cache module"
    );
    assert!(
        !root.contains("fn cache_key_str("),
        "cache_key_str should move out of thumbnails.rs"
    );
}

#[test]
fn thumbnail_queue_helpers_live_in_queue_module() {
    let queue_path = Path::new("src/core/thumbnails/queue.rs");
    assert!(
        queue_path.exists(),
        "queue helpers should live in src/core/thumbnails/queue.rs"
    );

    let queue = fs::read_to_string(queue_path).expect("queue module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn worker_loop",
        "pub(in crate::core::thumbnails) fn next_request_or_pull",
        "pub(in crate::core::thumbnails) fn pull_batch_and_enqueue",
        "pub(in crate::core::thumbnails) fn drop_in_flight",
    ] {
        assert!(queue.contains(marker), "queue.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(
        root.contains("mod queue;"),
        "thumbnail root should declare the queue module"
    );
    for marker in [
        "fn worker_loop(",
        "fn next_request_or_pull(",
        "fn pull_batch_and_enqueue(",
        "fn drop_in_flight(",
    ] {
        assert!(
            !root.contains(marker),
            "queue helper `{marker}` should move out of thumbnails.rs"
        );
    }
}

#[test]
fn thumbnail_decode_helpers_live_in_decode_module() {
    let decode_path = Path::new("src/core/thumbnails/decode.rs");
    assert!(
        decode_path.exists(),
        "decode helpers should live in src/core/thumbnails/decode.rs"
    );

    let decode = fs::read_to_string(decode_path).expect("decode module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn generate",
        "fn generate_unavailable_placeholder",
        "fn generate_via_pixbuf",
        "fn save_pixbuf_as_webp",
        "fn save_pixbuf_as_jpeg_atomic",
        "fn ensure_opaque",
        "fn scale_pixbuf_to_fit",
        "fn pixbuf_is_light",
    ] {
        assert!(decode.contains(marker), "decode.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(
        root.contains("mod decode;"),
        "thumbnail root should declare the decode module"
    );
    for marker in [
        "fn generate(",
        "fn generate_unavailable_placeholder(",
        "fn generate_via_pixbuf(",
        "fn save_pixbuf_as_webp(",
        "fn save_pixbuf_as_jpeg_atomic(",
        "fn ensure_opaque(",
        "fn scale_pixbuf_to_fit(",
        "fn pixbuf_is_light(",
    ] {
        assert!(
            !root.contains(marker),
            "decode helper `{marker}` should move out of thumbnails.rs"
        );
    }
}

#[test]
fn thumbnail_video_helpers_live_in_video_module() {
    let video_path = Path::new("src/core/thumbnails/video.rs");
    assert!(
        video_path.exists(),
        "video thumbnail helpers should live in src/core/thumbnails/video.rs"
    );

    let video = fs::read_to_string(video_path).expect("video module should be readable");
    for marker in [
        "pub(in crate::core::thumbnails) fn extract_video_frame",
        "pub(in crate::core::thumbnails) fn extract_video_frame_ffmpeg",
        "pub(in crate::core::thumbnails) fn ffmpeg_thumbnail_temp_path",
        "pub(in crate::core::thumbnails) fn overlay_play_icon",
    ] {
        assert!(video.contains(marker), "video.rs missing `{marker}`");
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(
        root.contains("mod video;"),
        "thumbnail root should declare the video module"
    );
    assert!(
        !root.contains("fn extract_video_frame("),
        "extract_video_frame should move out of thumbnails.rs"
    );
}

#[test]
fn thumbnail_jpeg_turbo_helpers_live_in_jpeg_turbo_module() {
    let jpeg_turbo_path = Path::new("src/core/thumbnails/jpeg_turbo.rs");
    assert!(
        jpeg_turbo_path.exists(),
        "JPEG turbo helpers should live in src/core/thumbnails/jpeg_turbo.rs"
    );

    let jpeg_turbo =
        fs::read_to_string(jpeg_turbo_path).expect("jpeg_turbo module should be readable");
    for marker in [
        "fn jpeg_shim_create",
        "pub(in crate::core::thumbnails) fn decode_jpeg_scaled",
        "pub(in crate::core::thumbnails) fn file_has_jpeg_signature",
    ] {
        assert!(
            jpeg_turbo.contains(marker),
            "jpeg_turbo.rs missing `{marker}`"
        );
    }

    let root = fs::read_to_string("src/core/thumbnails.rs").expect("thumbnail root readable");
    assert!(
        root.contains("mod jpeg_turbo;"),
        "thumbnail root should declare the jpeg_turbo module"
    );
    assert!(
        !root.contains("fn decode_jpeg_scaled("),
        "decode_jpeg_scaled should move out of thumbnails.rs"
    );
}
