//! Real backdrop refraction for the ModeSelector "liquid glass" pill.
//!
//! GTK4 CSS has no `backdrop-filter`, and a widget's GL shader cannot sample its
//! own backdrop — so the lens-refraction that defines `shuding/liquid-glass`
//! can't be done the web way. Instead we:
//!   1. capture the photo grid *behind* the pill to an offscreen texture via
//!      [`gsk::Renderer::render_texture`] of the grid's [`gtk::WidgetPaintable`],
//!      cropped to the pill's rectangle;
//!   2. pull those pixels to the CPU and apply a convex-lens displacement —
//!      each output pixel samples a point pulled toward the centre, the optical
//!      signature of a magnifying lens — plus a brightness/saturation lift
//!      (the reference's `brightness(1.05)/saturate(1.1)`);
//!   3. hand back a [`gdk::MemoryTexture`] the caller paints in its snapshot.
//!
//! All of this runs on the CPU over a small region (the pill is a few hundred
//! px), so it is renderer-agnostic and debuggable. See [`refract_region`].

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, glib, graphene, gsk};

/// Capture the region `rect` (in `source`'s coordinate space) of `source`'s
/// current rendering, apply a convex-lens displacement, and return a texture
/// the same size as the captured region.
///
/// `strength` ∈ `[0, 1)` is the maximum inward pull at the pill edge (≈ how
/// strong the magnification reads). Returns `None` when offscreen rendering is
/// unavailable (headless / unrealized), so callers can fall back to a flat fill.
pub fn refract_region(
    source: &gtk::Widget,
    rect: &graphene::Rect,
    renderer: &gsk::Renderer,
    strength: f32,
) -> Option<gdk::MemoryTexture> {
    let (sw, sh) = (source.width(), source.height());
    if sw <= 0 || sh <= 0 {
        return None;
    }

    // Build a render node describing the grid as it currently appears. The
    // paintable renders the whole widget; the viewport crop below (in
    // `render_texture`) limits the actual painting to the pill region.
    let paintable = gtk::WidgetPaintable::new(Some(source));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, sw as f64, sh as f64);
    let node = snap.to_node()?;

    // Render only the sub-rect behind the pill to a small texture.
    let tex = renderer.render_texture(&node, Some(rect));
    let tw = tex.width();
    let th = tex.height();
    if tw <= 0 || th <= 0 {
        return None;
    }

    let stride = tw as usize * 4;
    let mut src = vec![0u8; stride * th as usize];
    tex.download(&mut src, stride);

    // Convex-lens displacement + brightness/saturation lift.
    let mut out = vec![0u8; stride * th as usize];
    let wf = tw as f32;
    let hf = th as f32;
    let sat = 1.12_f32;
    let brightness = 1.06_f32;
    for y in 0..th {
        for x in 0..tw {
            // Normalised centre-origin coords in [-0.5, 0.5].
            let nx = (x as f32 + 0.5) / wf - 0.5;
            let ny = (y as f32 + 0.5) / hf - 0.5;
            let r = (nx * nx + ny * ny).sqrt();
            // Lens strength: 0 at the centre → 1 toward the edge.
            let s = smoothstep(0.0, 0.5, r);
            // Pull the sample toward the centre by `strength * s`.
            let factor = 1.0 - strength * s;
            let sx = ((nx * factor + 0.5) * wf).round().clamp(0.0, wf - 1.0) as usize;
            let sy = ((ny * factor + 0.5) * hf).round().clamp(0.0, hf - 1.0) as usize;
            let si = sy * stride + sx * 4;
            let oi = y as usize * stride + x as usize * 4;

            let r0 = src[si] as f32;
            let g0 = src[si + 1] as f32;
            let b0 = src[si + 2] as f32;
            let lum = 0.299 * r0 + 0.587 * g0 + 0.114 * b0;
            let rr = (lum + (r0 - lum) * sat) * brightness;
            let gg = (lum + (g0 - lum) * sat) * brightness;
            let bb = (lum + (b0 - lum) * sat) * brightness;
            out[oi] = clamp_u8(rr);
            out[oi + 1] = clamp_u8(gg);
            out[oi + 2] = clamp_u8(bb);
            out[oi + 3] = src[si + 3];
        }
    }

    let bytes = glib::Bytes::from_owned(out);
    Some(gdk::MemoryTexture::new(
        tw,
        th,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    ))
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}
