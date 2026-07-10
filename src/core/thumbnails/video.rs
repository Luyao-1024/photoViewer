use super::cache::load_pixbuf_sync;
use gdk_pixbuf::Pixbuf;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// 用 GStreamer 从视频文件中提取一帧作为缩略图。
///
/// 提取视频封面帧：优先调用 [`extract_video_frame_ffmpeg`]（基于 libav，正确处理
/// limited->full 色彩范围、HDR->SDR 色调映射与旋转），失败时回退到内置 GStreamer
/// 管线 [`extract_video_frame_gst`]。返回的视频帧保持原始画面，不添加 UI 标记。
pub(in crate::core::thumbnails) fn extract_video_frame(
    path: &Path,
    max_dim: u32,
) -> anyhow::Result<Pixbuf> {
    match extract_video_frame_ffmpeg(path, max_dim) {
        Ok(pb) => Ok(pb),
        Err(e) => {
            debug!(
                "VIDEO_THUMB ffmpegthumbnailer 失败，回退 GStreamer {}: {}",
                path.display(),
                e
            );
            extract_video_frame_gst(path, max_dim)
        }
    }
}

/// 用外部 `ffmpegthumbnailer` 生成封面帧。它内部走 libav，会正确扩展 limited
/// range（YUV 16-235 -> RGB 0-255）并做 HDR->SDR 与旋转，避免手写管线把窄范围
/// 原样塞进 RGB 导致缩略图发灰、低饱和。输出 PNG（无损，避免二次 JPEG 压缩），
/// 解码后直接返回视频画面。
pub(in crate::core::thumbnails) fn extract_video_frame_ffmpeg(
    path: &Path,
    max_dim: u32,
) -> anyhow::Result<Pixbuf> {
    let tmp = ffmpeg_thumbnail_temp_path(path, max_dim);

    let out = Command::new("ffmpegthumbnailer")
        .args([
            "-i",
            &path.to_string_lossy(),
            "-o",
            &tmp.to_string_lossy(),
            "-s",
            &max_dim.to_string(),
            "-t",
            "10%",
            "-c",
            "png",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("启动 ffmpegthumbnailer 失败: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "ffmpegthumbnailer 退出码 {:?}: {}",
            out.status.code(),
            stderr.trim()
        );
    }

    let pb = load_pixbuf_sync(&tmp);
    let _ = std::fs::remove_file(&tmp);
    let pb = pb?;
    debug!(
        "VIDEO_THUMB ffmpegthumbnailer 提取成功 {}x{}",
        pb.width(),
        pb.height()
    );
    Ok(pb)
}

pub(in crate::core::thumbnails) fn ffmpeg_thumbnail_temp_path(
    path: &Path,
    max_dim: u32,
) -> PathBuf {
    let key = format!("{}:{max_dim}", path.to_string_lossy());
    std::env::temp_dir().join(format!(
        "pvthumb-{}-{}.png",
        std::process::id(),
        blake3::hash(key.as_bytes()).to_hex()
    ))
}

/// GStreamer fallback：`uridecodebin -> videoflip(auto) -> videoconvert -> appsink`，
/// seek 到约 1 秒（或总时长 10%）处拉取一帧。输出 caps 显式指定 `colorimetry=sRGB`
/// 以强制 videoconvert 做 limited->full 色彩范围扩展，修复 TV-range 视频发灰。
fn extract_video_frame_gst(path: &Path, _max_dim: u32) -> anyhow::Result<Pixbuf> {
    debug!("VIDEO_THUMB 提取视频帧(GStreamer): {}", path.display());
    gst::init().map_err(|e| anyhow::anyhow!("GStreamer 初始化失败: {e}"))?;

    let uri =
        glib::filename_to_uri(path, None).map_err(|e| anyhow::anyhow!("路径转 URI 失败: {e}"))?;

    // 用 uridecodebin 构建管线：自动处理 decodebin 动态 pad 链接。
    // videoflip video-direction=auto 从所有来源（容器 tkhd、编码 SEI、tags）自动检测并应用旋转。
    // videoconvert 负责 YUV->RGB；显式 colorimetry=sRGB 强制输出 full-range sRGB，
    // 避免 limited-range(TV) 视频黑/白点被压在 16/235 导致缩略图发灰低饱和。
    let desc = format!(
        "uridecodebin uri={} ! videoflip video-direction=auto ! videoconvert ! video/x-raw,format=RGB,colorimetry=sRGB ! appsink name=sink",
        uri
    );
    let pipeline =
        gst::parse::launch(&desc).map_err(|e| anyhow::anyhow!("创建 pipeline 失败: {e}"))?;
    let pipeline = pipeline
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("pipeline 类型转换失败"))?;

    // 获取 appsink 元素。
    let appsink_el = pipeline
        .by_name("sink")
        .ok_or_else(|| anyhow::anyhow!("找不到 appsink 元素"))?;
    let appsink = appsink_el
        .downcast_ref::<gst_app::AppSink>()
        .ok_or_else(|| anyhow::anyhow!("appsink 类型转换失败"))?;

    appsink.set_max_buffers(1);
    appsink.set_drop(true);

    // 启动 pipeline。
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| anyhow::anyhow!("设置 Playing 失败: {e}"))?;

    // 等待 pipeline 进入 Playing（带超时）。
    let bus = pipeline
        .bus()
        .ok_or_else(|| anyhow::anyhow!("pipeline 无 bus"))?;
    let start = std::time::Instant::now();
    loop {
        let (_, state, _) = pipeline.state(gst::ClockTime::from_mseconds(100));
        if state == gst::State::Playing {
            break;
        }
        if start.elapsed().as_secs() >= 5 {
            let (_, cur_state, _) = pipeline.state(gst::ClockTime::ZERO);
            pipeline.set_state(gst::State::Null).ok();
            anyhow::bail!("等待 Playing 超时，当前状态: {:?}", cur_state);
        }
        while let Some(msg) = bus.pop() {
            if let gst::MessageView::Error(e) = msg.view() {
                pipeline.set_state(gst::State::Null).ok();
                anyhow::bail!("GStreamer 错误: {}", e.error().message());
            }
        }
    }

    // 查询时长并 seek 到合适位置。
    let seek_pos = if let Some(duration) = pipeline.query_duration::<gst::ClockTime>() {
        let one_sec = gst::ClockTime::from_seconds(1);
        let ten_pct = duration
            .nseconds()
            .checked_mul(10)
            .and_then(|n| n.checked_div(100))
            .map(gst::ClockTime::from_nseconds)
            .unwrap_or(gst::ClockTime::ZERO);
        let target = std::cmp::max(one_sec, ten_pct);
        if target >= duration {
            gst::ClockTime::ZERO
        } else {
            target
        }
    } else {
        gst::ClockTime::from_seconds(1)
    };

    if !seek_pos.is_none() {
        let _ = pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, seek_pos);
        // 等待 seek 完成。
        let _ = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(3),
            &[gst::MessageType::AsyncDone, gst::MessageType::Error],
        );
    }

    // 拉取一帧。
    let sample = match appsink.try_pull_sample(gst::ClockTime::from_seconds(3)) {
        Some(s) => s,
        None => {
            pipeline.set_state(gst::State::Null).ok();
            anyhow::bail!("拉取视频帧超时或无数据");
        }
    };

    let buffer = sample
        .buffer()
        .ok_or_else(|| anyhow::anyhow!("sample 无 buffer"))?;
    let caps = sample
        .caps()
        .ok_or_else(|| anyhow::anyhow!("sample 无 caps"))?;
    let vinfo = gst_video::VideoInfo::from_caps(caps)
        .map_err(|e| anyhow::anyhow!("解析视频 caps 失败: {e}"))?;

    let width = vinfo.width() as i32;
    let height = vinfo.height() as i32;
    let stride = vinfo.stride()[0] as i32;

    let map = buffer
        .map_readable()
        .map_err(|e| anyhow::anyhow!("buffer 映射失败: {e}"))?;
    let data = map.as_slice();

    // 构造 Pixbuf（RGB，无 alpha，3 通道）。
    let pb = Pixbuf::from_mut_slice(
        data.to_vec().into_boxed_slice(),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        width,
        height,
        stride,
    );

    pipeline.set_state(gst::State::Null).ok();

    // 旋转已由 GStreamer pipeline 中的 videoflip video-direction=auto 自动处理，
    // 无需手动读取容器元数据并应用方向校正。

    debug!("VIDEO_THUMB 提取成功 {}x{}", pb.width(), pb.height());

    Ok(pb)
}

/// 从 MP4/MOV 容器的 tkhd atom 中读取视频旋转角度（0/90/180/270）。
///
/// MP4 容器在 track header (tkhd) 中存储一个 3x3 仿射矩阵。
/// 旋转信息编码在矩阵的 a,b,c,d 分量中（16.16 定点数）：
///   - 0°:   a=1, b=0, c=0, d=1
///   - 90°:  a=0, b=1, c=-1, d=0
///   - 180°: a=-1, b=0, c=0, d=-1
///   - 270°: a=0, b=-1, c=1, d=0
///
/// 递归搜索 tkhd atom 以处理嵌套的 box 结构（moov -> trak -> tkhd）。
#[allow(dead_code)] // used in tests
pub(in crate::core::thumbnails) fn read_video_rotation(path: &Path) -> i32 {
    let Ok(data) = std::fs::read(path) else {
        return 0;
    };

    // 递归搜索 tkhd atom。
    fn find_tkhd_rotation(data: &[u8], start: usize, end: usize) -> i32 {
        let mut pos = start;
        while pos + 8 <= end && pos + 8 <= data.len() {
            let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
            if size < 8 {
                break;
            }
            let typ = &data[pos + 4..pos + 8];

            // 递归进入容器 atom（moov, trak 等）。
            if typ == b"moov" || typ == b"trak" {
                let child_start = pos + 8;
                let child_end = pos + size;
                if child_end <= data.len() {
                    let result = find_tkhd_rotation(data, child_start, child_end);
                    if result != 0 {
                        return result;
                    }
                }
            }

            // 找到 tkhd atom，解析旋转矩阵。
            if typ == b"tkhd" && size >= 84 {
                let version = data[pos + 8];
                // 矩阵偏移量：version + flags + creation_time + modification_time +
                // track_ID + reserved + duration + reserved + layer + alternate_group +
                // volume + reserved = 40 bytes for version 0, 52 for version 1
                let matrix_offset = if version == 0 {
                    pos + 8 + 40 // 4 + 4 + 4 + 4 + 4 + 4 + 8 + 2 + 2 + 2 + 2
                } else {
                    pos + 8 + 52 // 4 + 8 + 8 + 4 + 4 + 8 + 8 + 2 + 2 + 2 + 2
                };

                if matrix_offset + 36 <= data.len() {
                    let a = i32::from_be_bytes(
                        data[matrix_offset..matrix_offset + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                    let b = i32::from_be_bytes(
                        data[matrix_offset + 4..matrix_offset + 8]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );

                    let a_f = a as f64 / 65536.0;
                    let b_f = b as f64 / 65536.0;

                    if a_f.abs() < 0.01 && (b_f - 1.0).abs() < 0.01 {
                        return 90;
                    }
                    if (a_f - (-1.0)).abs() < 0.01 && b_f.abs() < 0.01 {
                        return 180;
                    }
                    if a_f.abs() < 0.01 && (b_f - (-1.0)).abs() < 0.01 {
                        return 270;
                    }
                }
            }

            pos += size;
        }
        0
    }

    find_tkhd_rotation(&data, 0, data.len())
}
