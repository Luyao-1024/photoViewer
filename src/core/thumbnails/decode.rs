use super::cache::{cache_stem_for, load_pixbuf_sync_or_remove, resolve_src};
use super::jpeg_turbo::{decode_jpeg_scaled, file_has_jpeg_signature};
use super::video::extract_video_frame;
use super::ThumbnailSize;
use crate::core::media::{media_kind_from_mime, mime_from_extension, MediaKind};
use crate::core::orientation;
use gdk_pixbuf::Pixbuf;
use image::ImageEncoder;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, warn};

#[tracing::instrument(name = "thumb:generate", skip(cache_dir), level = "debug")]
pub(in crate::core::thumbnails) fn generate(
    cache_dir: &Path,
    uri: &str,
    size: ThumbnailSize,
    mtime: Option<SystemTime>,
) -> anyhow::Result<Pixbuf> {
    let (src_path, mtime) = resolve_src(uri, mtime)?;
    let cache_stem = cache_stem_for(cache_dir, uri, size, Some(mtime))?;
    let jpeg_path = cache_stem.with_extension("jpg");
    let webp_path = cache_stem.with_extension("webp");

    for cache_path in [&webp_path, &jpeg_path] {
        if !cache_path.exists() {
            continue;
        }
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB disk_cache_hit source_uri={} source_path={} size={:?} cache_path={}",
            uri,
            src_path.display(),
            size,
            cache_path.display()
        );
        // 磁盘命中：必须解码一次才能拿到像素做 Texture（不可避免）。
        // 使用同步读取确保文件完全写入后再解码。坏缓存会删除并继续重新生成。
        match load_pixbuf_sync_or_remove(cache_path) {
            Ok(pb) => return Ok(pb),
            Err(e) => {
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB disk_cache_invalid source_uri={} source_path={} size={:?} cache_path={} error={}",
                    uri,
                    src_path.display(),
                    size,
                    cache_path.display(),
                    e
                );
            }
        }
    }

    if let Some(parent) = cache_stem.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if mime_from_extension(&src_path).and_then(media_kind_from_mime) == Some(MediaKind::Video) {
        match extract_video_frame(&src_path, size.max_dim()) {
            Ok(pb) => {
                let scaled = scale_pixbuf_to_fit(&pb, size.max_dim());
                let cache_path = cache_stem.with_extension("jpg");
                let thumb = ensure_opaque(&scaled);
                save_pixbuf_as_jpeg_atomic(&thumb, &cache_path)
                    .map_err(|e| anyhow::anyhow!("视频缩略图保存失败 {:?}: {}", cache_path, e))?;
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB video_generated source_uri={} source_path={} size={:?} cache_path={}",
                    uri,
                    src_path.display(),
                    size,
                    cache_path.display()
                );
                return Ok(thumb);
            }
            Err(e) => {
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB video_extract_failed source_uri={} source_path={} size={:?} error={}",
                    uri,
                    src_path.display(),
                    size,
                    e
                );
                let placeholder =
                    generate_unavailable_placeholder(size.max_dim(), &cache_stem, true)?;
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB video_placeholder_generated source_uri={} source_path={} size={:?}",
                    uri,
                    src_path.display(),
                    size
                );
                return Ok(placeholder);
            }
        }
    }

    // 统一用 gdk-pixbuf 解码 + 缩放：覆盖面广（JPEG/PNG/WebP/TIFF，flatpak
    // GNOME 50 runtime 还自带 libheif，能解 HEIC/AVIF），且其双线性缩放与 image
    // crate 的面积滤波在缩略图尺寸下肉眼无差（已 A/B 对照确认），故走单一路径。
    // 直接把缩放好的 pixbuf 返回给 worker 复用，省掉"写盘后再解码一次"的冗余。
    match generate_via_pixbuf(&src_path, size.max_dim(), &cache_stem) {
        Ok(pixbuf) => {
            debug!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB image_generated source_uri={} source_path={} size={:?} cache_stem={}",
                uri,
                src_path.display(),
                size,
                cache_stem.display()
            );
            Ok(pixbuf)
        }
        Err(e) => {
            warn!(
                target: crate::core::log_targets::THUMBNAILS,
                "THUMB image_decode_failed source_uri={} source_path={} size={:?} error={}",
                uri,
                src_path.display(),
                size,
                e
            );
            generate_unavailable_placeholder(size.max_dim(), &cache_stem, false)
        }
    }
}

pub(in crate::core::thumbnails) fn generate_unavailable_placeholder(
    max_dim: u32,
    cache_stem: &Path,
    is_video: bool,
) -> anyhow::Result<Pixbuf> {
    let width = max_dim as i32;
    let height = if is_video {
        ((max_dim as f64) * 9.0 / 16.0).round().max(1.0) as i32
    } else {
        width
    };
    let pb = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, width, height)
        .ok_or_else(|| anyhow::anyhow!("failed to allocate unavailable thumbnail"))?;
    pb.fill(0x242932ff);

    let rowstride = pb.rowstride() as usize;
    let channels = pb.n_channels() as usize;
    let cx = width / 2;
    let cy = height / 2;
    let icon = (width.min(height) / 3).clamp(28, 140);
    let left = (cx - icon / 2).max(0);
    let right = (cx + icon / 2).min(width - 1);
    let top = (cy - icon / 2).max(0);
    let bottom = (cy + icon / 2).min(height - 1);
    let stroke = (icon / 12).clamp(3, 10);

    unsafe {
        let pixels = pb.pixels();
        for y in 0..height {
            for x in 0..width {
                let i = y as usize * rowstride + x as usize * channels;
                if i + 2 < pixels.len() {
                    let vignette = (((x - cx).abs() + (y - cy).abs()) * 22 / width.max(1)) as u8;
                    pixels[i] = 36_u8.saturating_add(vignette);
                    pixels[i + 1] = 41_u8.saturating_add(vignette);
                    pixels[i + 2] = 50_u8.saturating_add(vignette);
                }
            }
        }

        for y in top..=bottom {
            for x in left..=right {
                let border = x < left + stroke
                    || x > right - stroke
                    || y < top + stroke
                    || y > bottom - stroke;
                let slash = ((x - left) - (y - top)).abs() <= stroke;
                if !border && !slash {
                    continue;
                }
                let i = y as usize * rowstride + x as usize * channels;
                if i + 2 < pixels.len() {
                    if slash {
                        pixels[i] = 239;
                        pixels[i + 1] = 99;
                        pixels[i + 2] = 88;
                    } else {
                        pixels[i] = 154;
                        pixels[i + 1] = 163;
                        pixels[i + 2] = 176;
                    }
                }
            }
        }
    }

    let cache_path = cache_stem.with_extension("jpg");
    save_pixbuf_as_jpeg_atomic(&pb, &cache_path).map_err(|e| {
        anyhow::anyhow!(
            "unavailable thumbnail save failed {:?}: {}",
            cache_stem.with_extension("jpg"),
            e
        )
    })?;
    Ok(pb)
}

/// `image` crate 解不了的格式（HEIC/AVIF 等）走 gdk-pixbuf：解码 → 等比缩放 → 存磁盘缓存。
/// 返回内存里已缩放好的 pixbuf，让调用方直接做成 Texture，省掉读盘重解码。
///
/// JPEG 格式优先走 turbojpeg IDCT 缩放解码（快速路径），失败时回退到 gdk-pixbuf。
pub(in crate::core::thumbnails) fn generate_via_pixbuf(
    src_path: &Path,
    max_dim: u32,
    cache_stem: &Path,
) -> anyhow::Result<Pixbuf> {
    // 只读一次 EXIF 方向：turbojpeg 路径与 orientation 应用都复用这个值。
    let orientation = orientation::read_orientation(src_path).unwrap_or(1);
    let use_jpeg_fast_path =
        mime_from_extension(src_path) == Some("image/jpeg") && file_has_jpeg_signature(src_path);

    // decode 阶段（最耗时）：JPEG 优先走 turbojpeg IDCT 缩放解码，失败/非 JPEG
    // 回退 gdk-pixbuf。阶段耗时由 `thumb:pb_decode` span 承载（父级 `thumb:generate`）。
    let pb = {
        let decode_span = tracing::debug_span!("thumb:pb_decode");
        let _decode = decode_span.enter();
        if use_jpeg_fast_path {
            match decode_jpeg_scaled(src_path, max_dim, orientation) {
                Some(pb) => pb,
                None => {
                    // turbojpeg 失败（CMYK/渐进式/损坏），回退到 gdk-pixbuf。
                    debug!(
                        target: crate::core::log_targets::THUMBNAILS,
                        "THUMB turbojpeg_fallback path={}",
                        src_path.display()
                    );
                    orientation::load_oriented_pixbuf(src_path)
                        .map_err(|e| anyhow::anyhow!("gdk-pixbuf 解码失败: {e}"))?
                }
            }
        } else {
            orientation::load_oriented_pixbuf(src_path)
                .map_err(|e| anyhow::anyhow!("gdk-pixbuf 解码失败: {e}"))?
        }
    };

    // scale 阶段：等比缩放到目标尺寸。
    let scaled = {
        let scale_span = tracing::debug_span!("thumb:pb_scale");
        let _scale = scale_span.enter();
        scale_pixbuf_to_fit(&pb, max_dim)
    };

    // save 阶段：写磁盘缓存（有透明度用 webp，否则 jpeg）。
    if pixbuf_has_transparency(&scaled) {
        let cache_path = cache_stem.with_extension("webp");
        {
            let save_span = tracing::debug_span!("thumb:pb_save");
            let _save = save_span.enter();
            save_pixbuf_as_webp(&scaled, &cache_path)?;
        }
        return Ok(scaled);
    }

    let cache_path = cache_stem.with_extension("jpg");
    let thumb = ensure_opaque(&scaled);
    {
        let save_span = tracing::debug_span!("thumb:pb_save");
        let _save = save_span.enter();
        save_pixbuf_as_jpeg_atomic(&thumb, &cache_path).map_err(|e| {
            anyhow::anyhow!(
                "gdk-pixbuf JPEG 保存失败 {:?}: {}",
                cache_stem.with_extension("jpg"),
                e
            )
        })?;
    }
    Ok(thumb)
}

fn pixbuf_has_transparency(pb: &Pixbuf) -> bool {
    if !pb.has_alpha() {
        return false;
    }
    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    if n_channels < 4 {
        return false;
    }
    for y in 0..pb.height() as usize {
        for x in 0..pb.width() as usize {
            let i = y * rowstride + x * n_channels + 3;
            if i < buf.len() && buf[i] < 255 {
                return true;
            }
        }
    }
    false
}

fn save_pixbuf_as_webp(pb: &Pixbuf, cache_path: &Path) -> anyhow::Result<()> {
    let rgba = pixbuf_to_rgba_bytes(pb)?;
    let tmp_path = temporary_cache_path(cache_path);
    let file = File::create(&tmp_path)?;
    let writer = BufWriter::new(file);
    image::codecs::webp::WebPEncoder::new_lossless(writer)
        .write_image(
            &rgba,
            pb.width() as u32,
            pb.height() as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| anyhow::anyhow!("WebP 缩略图保存失败 {:?}: {}", cache_path, e))?;
    std::fs::rename(&tmp_path, cache_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::anyhow!("WebP 缩略图发布失败 {:?}: {}", cache_path, e)
    })
}

fn save_pixbuf_as_jpeg_atomic(pb: &Pixbuf, cache_path: &Path) -> anyhow::Result<()> {
    let tmp_path = temporary_cache_path(cache_path);
    pb.savev(&tmp_path, "jpeg", &[])
        .map_err(|e| anyhow::anyhow!("JPEG 缩略图保存失败 {:?}: {}", tmp_path, e))?;
    std::fs::rename(&tmp_path, cache_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::anyhow!("JPEG 缩略图发布失败 {:?}: {}", cache_path, e)
    })
}

fn temporary_cache_path(cache_path: &Path) -> PathBuf {
    let ext = cache_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("tmp");
    let nonce = format!(
        "{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    cache_path.with_extension(format!("{ext}.{nonce}.tmp"))
}

fn pixbuf_to_rgba_bytes(pb: &Pixbuf) -> anyhow::Result<Vec<u8>> {
    let width = pb.width() as usize;
    let height = pb.height() as usize;
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    if n_channels != 3 && n_channels != 4 {
        anyhow::bail!("不支持的 pixbuf 通道数: {}", n_channels);
    }

    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let i = y * rowstride + x * n_channels;
            if i + n_channels > buf.len() {
                anyhow::bail!("pixbuf 像素缓冲区越界");
            }
            rgba.extend_from_slice(&buf[i..i + 3]);
            rgba.push(if n_channels == 4 { buf[i + 3] } else { 255 });
        }
    }
    Ok(rgba)
}

/// 返回等尺寸的**不透明**（无 alpha）pixbuf：有 alpha 时合成到不透明白底上，
/// 无 alpha 时原样克隆。供 JPEG 保存前使用（JPEG 无 alpha 通道）。
pub(in crate::core::thumbnails) fn ensure_opaque(pb: &Pixbuf) -> Pixbuf {
    if !pb.has_alpha() {
        return pb.clone();
    }
    let (w, h) = (pb.width(), pb.height());
    let bg =
        Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, w, h).expect("分配不透明背景 pixbuf");
    bg.fill(0xFFFFFFFF); // 不透明白
    pb.composite(
        &bg,
        0,
        0,
        w,
        h,
        0.0,
        0.0,
        1.0,
        1.0,
        gdk_pixbuf::InterpType::Bilinear,
        255,
    );
    bg
}

/// 等比缩放到 `max_dim` 内（不放大），行为对齐 `image::DynamicImage::thumbnail`。
pub(in crate::core::thumbnails) fn scale_pixbuf_to_fit(pb: &Pixbuf, max_dim: u32) -> Pixbuf {
    let (w, h) = (pb.width(), pb.height());
    let longest = (w.max(h).max(1)) as f64;
    let scale = ((max_dim as f64) / longest).min(1.0);
    let nw = ((w as f64) * scale).round().max(1.0) as i32;
    let nh = ((h as f64) * scale).round().max(1.0) as i32;
    pb.scale_simple(nw, nh, gdk_pixbuf::InterpType::Bilinear)
        .unwrap_or_else(|| pb.clone())
}

/// 采样 pixbuf 像素估算平均亮度（>=160 视为"亮"背景），用于 tile 文字配色。
///
/// 在 worker 线程就地读取像素缓冲（零拷贝借用），替代原来在主线程对每张
/// texture 做 `Texture::download` + 大 buffer 分配的做法。RGB(3 通道)/RGBA(4 通道)
/// 均适用：`x * n_channels` 自动按实际通道数定位。
pub(in crate::core::thumbnails) fn pixbuf_is_light(pb: &Pixbuf) -> Option<bool> {
    let width = pb.width();
    let height = pb.height();
    if width <= 0 || height <= 0 {
        return None;
    }
    let bytes = pb.read_pixel_bytes();
    let buf: &[u8] = bytes.as_ref();
    let n_channels = pb.n_channels() as usize;
    let rowstride = pb.rowstride() as usize;
    let step_x = (width / 24).max(1) as usize;
    let step_y = (height / 24).max(1) as usize;
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for y in (0..height as usize).step_by(step_y) {
        for x in (0..width as usize).step_by(step_x) {
            let i = y * rowstride + x * n_channels;
            if i + 2 < buf.len() {
                total += (buf[i] as f64 + buf[i + 1] as f64 + buf[i + 2] as f64) / 3.0;
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        return None;
    }
    Some(total / count >= 160.0)
}
