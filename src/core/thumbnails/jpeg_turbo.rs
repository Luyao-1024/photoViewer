use crate::core::orientation;
use gdk_pixbuf::Pixbuf;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tracing::debug;

// ── libjpeg IDCT 缩放解码（JPEG 快速路径）───────────────────────────────────────
//
// 系统 libjpeg.so 就是 libjpeg-turbo，支持在 jpeg_read_header 与
// jpeg_start_decompress 之间设置 scale_num/scale_denom，让解码器只
// 输出低频 DCT 系数对应的缩小图像（1/2, 1/4, 1/8），避免全分辨率解码后再缩放。
// 例：48MP JPEG → 1/8 输出 1000×750 像素 → 再缩到 512px，约快 8×。

extern "C" {
    fn jpeg_shim_create() -> *mut std::ffi::c_void;
    fn jpeg_shim_destroy(shim: *mut std::ffi::c_void);
    fn jpeg_shim_decode_scaled(
        shim: *mut std::ffi::c_void,
        filename: *const std::ffi::c_char,
        max_dim: std::ffi::c_int,
        out_w: *mut std::ffi::c_int,
        out_h: *mut std::ffi::c_int,
        errmsg: *mut std::ffi::c_char,
        errmsg_size: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn jpeg_shim_take_buffer(shim: *mut std::ffi::c_void, out_len: *mut usize) -> *mut u8;
    fn jpeg_shim_free_buffer(ptr: *mut std::ffi::c_void);
}

/// 对 JPEG 文件利用 libjpeg-turbo 的 IDCT 缩放能力，解码时直接缩放到接近目标尺寸。
///
/// `orientation` 由调用方预算好（避免每次调用都重读 EXIF），在这里经
/// [`orientation::apply_orientation_to_pixbuf`] 应用——与 gdk-pixbuf 路径走同一条
/// EXIF 方向处理，单一实现、不会漂移。
///
/// 失败返回 `None`，调用方回退到 gdk-pixbuf。
pub(in crate::core::thumbnails) fn decode_jpeg_scaled(
    src_path: &Path,
    max_dim: u32,
    orientation: u16,
) -> Option<Pixbuf> {
    let cpath = std::ffi::CString::new(src_path.to_string_lossy().as_bytes()).ok()?;

    let shim = unsafe { jpeg_shim_create() };
    if shim.is_null() {
        return None;
    }

    let mut out_w: std::ffi::c_int = 0;
    let mut out_h: std::ffi::c_int = 0;
    let mut errbuf = vec![0u8; 256];
    let rc = unsafe {
        jpeg_shim_decode_scaled(
            shim,
            cpath.as_ptr(),
            max_dim.max(1) as std::ffi::c_int,
            &mut out_w,
            &mut out_h,
            errbuf.as_mut_ptr() as *mut std::ffi::c_char,
            errbuf.len() as std::ffi::c_int,
        )
    };
    if rc != 0 {
        let msg = String::from_utf8_lossy(&errbuf);
        debug!(
            target: crate::core::log_targets::THUMBNAILS,
            "THUMB jpeg_shim_decode_failed path={} error={}",
            src_path.display(),
            msg.trim_end_matches('\0').trim()
        );
        unsafe {
            jpeg_shim_destroy(shim);
        }
        return None;
    }

    // 取走 C 端 malloc 的解码缓冲区，拷贝进 Rust 拥有的 Vec，再用 C 的 free 释放。
    // 这样 Rust 永远不通过自己的分配器去释放 C 的指针——不依赖两个分配器相同。
    let mut buf_len: usize = 0;
    let raw = unsafe { jpeg_shim_take_buffer(shim, &mut buf_len) };
    unsafe {
        jpeg_shim_destroy(shim);
    }

    if raw.is_null() || buf_len == 0 || out_w <= 0 || out_h <= 0 {
        if !raw.is_null() {
            unsafe {
                jpeg_shim_free_buffer(raw as *mut std::ffi::c_void);
            }
        }
        return None;
    }

    // SAFETY: `raw` 由 shim malloc，长度正好是 buf_len 字节（= w*h*3）。
    // 立刻拷贝进 Rust Vec，随后用 jpeg_shim_free_buffer 释放 C 端内存。
    let pixels: Vec<u8> = unsafe {
        let slice = std::slice::from_raw_parts(raw, buf_len);
        slice.to_vec()
    };
    unsafe {
        jpeg_shim_free_buffer(raw as *mut std::ffi::c_void);
    }

    let (w, h) = (out_w as i32, out_h as i32);
    let rowstride = w as usize * 3;
    // 先用缩放后的 RGB 构造未旋转 pixbuf，再复用 gdk-pixbuf 的方向处理（单一实现）。
    let unrotated = Pixbuf::from_mut_slice(
        pixels.into_boxed_slice(),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        w,
        h,
        rowstride as i32,
    );
    Some(orientation::apply_orientation_to_pixbuf(
        &unrotated,
        orientation,
    ))
}

pub(in crate::core::thumbnails) fn file_has_jpeg_signature(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut head = [0u8; 3];
    match file.read(&mut head) {
        Ok(n) if n == head.len() => head == [0xff, 0xd8, 0xff],
        _ => false,
    }
}
