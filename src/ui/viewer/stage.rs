use crate::core::media::{is_gif_head, MediaItem};
use crate::core::motion_photo::{self, MediaAttributes};
use crate::core::orientation;
use crate::core::prefs;
use crate::core::thumbnails::{ThumbnailSize, TIER_BOOST};
use crate::ui::editor_panel::CropOverlayUpdate;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use image::AnimationDecoder;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use super::ViewerPage;

pub(super) const ANIMATED_IMAGE_LOOP_PAUSE_MS: u64 = 500;

pub(super) const VIDEO_CONTROLS_CLICK_EXCLUSION_PX: f64 = 48.0;

#[derive(Clone)]
pub(super) struct AnimatedImageFrame {
    pub(super) texture: gdk::Texture,
    pub(super) delay: Duration,
}

/// Convert a `file://` URI stored on `MediaItem::uri` to a `PathBuf` for
/// the gdk-pixbuf loader. Anything without the `file://` prefix is treated
/// as a raw path (defensive — the scanner only emits `file://` URIs).
pub(super) fn strip_file_uri(uri: &str) -> PathBuf {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(stripped)
}

pub(super) fn viewer_preview_thumbnail_size() -> ThumbnailSize {
    ThumbnailSize::Medium
}

pub(super) fn should_reveal_prepared_video_stage(
    expected_token: u64,
    current_token: u64,
    stream_prepared: bool,
) -> bool {
    expected_token == current_token && stream_prepared
}

/// True once the authoritative original-resolution texture has already been
/// painted for `token`. A preview thumbnail landing after this point must be
/// suppressed so it cannot clobber the original.
///
/// Non-JPEG thumbnails (e.g. PNG screenshots) decode the full-resolution
/// source before downscaling, so the thumbnail path can finish *after* the
/// lighter original decode. Without this guard the late thumbnail overwrites
/// the already-painted original and the viewer stays stuck on the thumbnail.
/// The `token` comparison also ensures a *previous* item's original never
/// suppresses the *current* item's thumbnail after navigation.
pub(super) fn original_has_landed(original_painted_token: u64, token: u64) -> bool {
    original_painted_token == token
}

fn animated_image_frame_delay(delay: Option<Duration>) -> Duration {
    match delay {
        Some(delay) if delay >= Duration::from_millis(20) => delay,
        _ => Duration::from_millis(100),
    }
}

pub(super) fn animated_image_next_delay(
    frames: &[AnimatedImageFrame],
    current_index: usize,
) -> Duration {
    let frame_delay = frames
        .get(current_index)
        .map(|frame| frame.delay)
        .unwrap_or_else(|| Duration::from_millis(100));
    let next_index = (current_index + 1) % frames.len();
    if next_index == 0 {
        frame_delay + Duration::from_millis(ANIMATED_IMAGE_LOOP_PAUSE_MS)
    } else {
        frame_delay
    }
}

fn image_delay_to_duration(delay: image::Delay) -> Duration {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 || numerator == 0 {
        return animated_image_frame_delay(None);
    }
    animated_image_frame_delay(Some(Duration::from_millis(
        (numerator as u64).div_ceil(denominator as u64),
    )))
}

fn load_animated_image_frames(path: &Path) -> anyhow::Result<Vec<AnimatedImageFrame>> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let decoder = image::codecs::gif::GifDecoder::new(reader)?;
    let frames = decoder.into_frames().collect_frames()?;
    frames
        .into_iter()
        .map(|frame| {
            let delay = image_delay_to_duration(frame.delay());
            let image = frame.into_buffer();
            let width = image.width();
            let height = image.height();
            let rowstride = (width * 4) as usize;
            let bytes = glib::Bytes::from_owned(image.into_raw());
            let texture = gdk::MemoryTexture::new(
                width as i32,
                height as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                rowstride,
            )
            .upcast();
            Ok(AnimatedImageFrame { texture, delay })
        })
        .collect()
}

fn motion_video_cache_path(item: &MediaItem) -> PathBuf {
    // Key on the db id only: it's already unique per motion photo, and
    // blake3_hash is no longer computed at scan time.
    std::env::temp_dir().join(format!("photo-viewer-motion-{}.mp4", item.id))
}

pub(super) fn apply_video_audio_preferences_to_stream(
    stream: &impl IsA<gtk::MediaStream>,
    muted: bool,
    volume: f64,
) {
    stream.set_muted(muted);
    stream.set_volume(volume.clamp(0.0, 1.0));
}

pub(super) fn persist_video_volume_from_stream(stream: &impl IsA<gtk::MediaStream>) {
    if stream.is_muted() {
        return;
    }
    if let Err(err) = prefs::set_video_volume(stream.volume()) {
        tracing::warn!("ViewerPage: failed to persist video volume: {err}");
    }
}

pub(super) fn should_toggle_video_from_stage_click(y: f64, height: f64) -> bool {
    height > VIDEO_CONTROLS_CLICK_EXCLUSION_PX
        && y >= 0.0
        && y < height - VIDEO_CONTROLS_CLICK_EXCLUSION_PX
}

pub(super) fn should_play_animated_image(item: &MediaItem) -> bool {
    if !item.is_image() || item.is_motion_photo() {
        return false;
    }
    item.is_animated() || item.mime_type == "image/gif" || file_starts_with_gif_header(&item.path)
}

pub(super) fn file_starts_with_gif_header(path: &Path) -> bool {
    let mut head = [0_u8; 6];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && is_gif_head(&head)
}

impl ViewerPage {
    pub(super) fn setup_motion_play_button(&self) {
        let weak = self.downgrade();
        self.imp().motion_play_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.play_current_motion_photo();
            }
        });
    }

    pub(super) fn set_motion_play_button_for_item(&self, item: &MediaItem) {
        self.imp().motion_play_btn.get().set_visible(
            item.is_motion_photo() && !item.is_video() && !self.imp().is_editing.get(),
        );
    }

    fn restore_image_after_motion_video(&self, token: u64) {
        if self.imp().current_token.get() != token {
            return;
        }
        self.stop_video_playback();
        self.set_video_error_visible(false);
        self.imp().video.get().set_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().edit_btn.get().set_sensitive(true);
        self.set_zoom_controls_visible(!self.imp().is_editing.get());
        if let Some(item) = self.current_media_item() {
            self.set_motion_play_button_for_item(&item);
        }
    }

    pub(super) fn play_current_motion_photo(&self) {
        let Some(item) = self.current_media_item() else {
            return;
        };
        let attrs = MediaAttributes::from_json(&item.media_attributes);
        let Some(info) = attrs.motion_photo else {
            self.imp().motion_play_btn.get().set_visible(false);
            return;
        };

        let token = self.imp().current_token.get();
        let source = item.path.clone();
        let dest = motion_video_cache_path(&item);
        self.set_spinner_visible(true);
        self.imp().motion_play_btn.get().set_visible(false);

        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let result = motion_photo::extract_video_to(&source, &info, &dest)
                .map(|()| dest)
                .map_err(|err| err.to_string());
            let _ = tx.send(result);
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let video_path = match rx.await {
                Ok(Ok(path)) => path,
                Ok(Err(err)) => {
                    tracing::warn!("ViewerPage: failed to extract motion photo video: {err}");
                    if let Some(this) = weak.upgrade() {
                        this.set_spinner_visible(false);
                        if let Some(item) = this.current_media_item() {
                            this.set_motion_play_button_for_item(&item);
                        }
                    }
                    return;
                }
                Err(_) => return,
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            this.show_motion_video_stage(video_path, token);
        });
    }

    pub(super) fn stop_video_playback(&self) {
        // The GtkVideo keeps its own built-in play/pause + progress controls,
        // so there is no separate slider to reset. We pause and detach the
        // in-flight stream so audio/playback does not continue behind an image.
        //
        // Detach happens first (the picture switches away and audio stops
        // immediately), but we keep our own strong reference a little longer:
        // releasing the *only* reference synchronously finalized the
        // GtkMediaFile while its GstPlay thread was still emitting
        // state-changed signals, crashing that thread with a use-after-free
        // inside libgobject's signal dispatch. The retire slot holds the
        // stream across one idle cycle so GstPlay's terminal signal lands on a
        // live object; the previous retiree (if any) has now had its grace and
        // is released here.
        let stream = self.imp().video.get().media_stream();
        self.imp().attached_video_media_id.set(0);
        self.imp()
            .video
            .get()
            .set_media_stream(gtk::MediaStream::NONE);
        let Some(stream) = stream else {
            return;
        };
        stream.pause();
        *self.imp().retired_video_stream.borrow_mut() = Some(stream);
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(this) = weak.upgrade() else {
                return;
            };
            *this.imp().retired_video_stream.borrow_mut() = None;
        });
    }

    pub(super) fn stop_animated_image_playback(&self) {
        if let Some(source) = self.imp().animated_image_source.borrow_mut().take() {
            source.remove();
        }
    }

    pub(super) fn start_animated_image_playback(&self, path: &Path, token: u64) -> bool {
        self.stop_animated_image_playback();
        let path = path.to_owned();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Decode all frames off the main thread, matching the
        // `request_current_original_image` pattern. GIF frame decoding is
        // CPU-bound (image crate) and previously blocked the GTK main loop
        // for 0.5-2.3 s on large files.
        gio::spawn_blocking(move || {
            let _ = tx.send(load_animated_image_frames(&path));
        });
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let frames = match rx.await {
                Ok(Ok(f)) => f,
                Ok(Err(err)) => {
                    tracing::warn!("ViewerPage: failed to load animated image: {err}");
                    if let Some(this) = weak.upgrade() {
                        this.set_spinner_visible(false);
                    }
                    return;
                }
                Err(_) => return,
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            if frames.len() < 2 {
                // Single frame or decode issue: fall back to original image.
                if let Some(item) = this.current_media_item() {
                    let path = strip_file_uri(&item.uri);
                    this.request_current_original_image(
                        path,
                        token,
                        item.display_name().to_string(),
                    );
                }
                return;
            }
            let frames = Rc::new(frames);
            this.imp()
                .picture
                .get()
                .set_paintable(Some(&frames[0].texture));
            this.set_spinner_visible(false);
            this.imp().edit_btn.get().set_sensitive(true);
            this.schedule_animated_image_frame(frames, 0, token);
        });
        true
    }

    fn schedule_animated_image_frame(
        &self,
        frames: Rc<Vec<AnimatedImageFrame>>,
        current_index: usize,
        token: u64,
    ) {
        let delay = animated_image_next_delay(&frames, current_index);
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(delay, move || {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }

            let next_index = (current_index + 1) % frames.len();
            this.imp()
                .picture
                .get()
                .set_paintable(Some(&frames[next_index].texture));
            this.schedule_animated_image_frame(frames, next_index, token);
        });
        *self.imp().animated_image_source.borrow_mut() = Some(source);
    }

    pub(super) fn toggle_video_playback(&self) -> bool {
        let imp = self.imp();
        let video = imp.video.get();
        if !video.is_visible() || imp.is_editing.get() {
            return false;
        }
        let Some(stream) = video.media_stream() else {
            return false;
        };
        stream.set_playing(!stream.is_playing());
        true
    }

    pub(super) fn show_image_stage(&self) {
        self.stop_video_playback();
        self.set_video_error_visible(false);
        self.imp().video.get().set_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().edit_btn.get().set_sensitive(true);
        self.set_zoom_controls_visible(!self.imp().is_editing.get());
    }

    pub(super) fn show_video_stage(&self, item: &MediaItem, token: u64) {
        self.stop_animated_image_playback();
        self.stop_video_playback();
        self.reset_viewer_transform();
        self.set_video_error_visible(false);
        self.imp().motion_play_btn.get().set_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().video.get().set_visible(false);
        self.set_zoom_controls_visible(false);
        self.set_spinner_visible(true);
        self.imp().edit_btn.get().set_sensitive(false);
        self.set_crop_overlay(CropOverlayUpdate {
            active: false,
            rect: None,
            image_dimensions: (0, 0),
        });

        let stream = gtk::MediaFile::for_filename(&item.path);
        stream.set_loop(false);
        let default_muted = prefs::video_default_muted();
        let persisted_volume = prefs::video_volume();
        self.connect_video_preview_reveal(&stream, token, false);
        self.imp().video.get().set_media_stream(Some(&stream));
        self.imp().attached_video_media_id.set(item.id);
        apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
        stream.connect_volume_notify(persist_video_volume_from_stream);
        stream.set_playing(true);
        self.reveal_prepared_video_stage(&stream, token, false);
        let stream_weak = stream.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(stream) = stream_weak.upgrade() {
                apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
            }
        });
    }

    fn show_motion_video_stage(&self, video_path: PathBuf, token: u64) {
        self.stop_animated_image_playback();
        self.stop_video_playback();
        self.reset_viewer_transform();
        self.set_video_error_visible(false);
        self.imp().picture.get().set_visible(true);
        self.imp().video.get().set_visible(false);
        self.imp().motion_play_btn.get().set_visible(false);
        self.set_zoom_controls_visible(false);
        self.set_spinner_visible(true);
        self.imp().edit_btn.get().set_sensitive(false);

        let stream = gtk::MediaFile::for_filename(&video_path);
        stream.set_loop(false);
        let default_muted = prefs::video_default_muted();
        let persisted_volume = prefs::video_volume();
        self.connect_video_preview_reveal(&stream, token, true);
        self.imp().video.get().set_media_stream(Some(&stream));
        self.imp().attached_video_media_id.set(0);
        apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
        stream.connect_volume_notify(persist_video_volume_from_stream);

        let weak = self.downgrade();
        stream.connect_ended_notify(move |stream| {
            if !stream.is_ended() {
                return;
            }
            if let Some(this) = weak.upgrade() {
                this.restore_image_after_motion_video(token);
            }
        });

        stream.set_playing(true);
        self.reveal_prepared_video_stage(&stream, token, true);
        let stream_weak = stream.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(stream) = stream_weak.upgrade() {
                apply_video_audio_preferences_to_stream(&stream, default_muted, persisted_volume);
            }
        });
    }

    fn connect_video_preview_reveal(
        &self,
        stream: &gtk::MediaFile,
        token: u64,
        restore_motion_on_error: bool,
    ) {
        let weak = self.downgrade();
        stream.connect_prepared_notify(move |stream| {
            if let Some(this) = weak.upgrade() {
                this.reveal_prepared_video_stage(stream, token, restore_motion_on_error);
            }
        });

        let weak = self.downgrade();
        stream.connect_error_notify(move |stream| {
            if stream.error().is_none() {
                return;
            }
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            this.set_spinner_visible(false);
            if restore_motion_on_error {
                this.restore_image_after_motion_video(token);
            } else {
                this.show_video_error_background();
            }
        });
    }

    pub(super) fn set_video_error_visible(&self, visible: bool) {
        self.imp().video_error_box.get().set_visible(visible);
    }

    /// Show/hide the viewer loading spinner with a CSS opacity fade. The spinner
    /// stays `visible: true` (it sits in an overlay slot of the media stage, so
    /// visibility never affects the Picture's allocation); the
    /// `.viewer-spinner-hidden` class drives opacity, and `set_spinning` is
    /// toggled so a hidden spinner stops its rotation work.
    pub(super) fn set_spinner_visible(&self, visible: bool) {
        let spinner = self.imp().spinner.get();
        if visible {
            spinner.remove_css_class("viewer-spinner-hidden");
            spinner.set_spinning(true);
        } else {
            spinner.add_css_class("viewer-spinner-hidden");
            spinner.set_spinning(false);
        }
    }

    pub(super) fn show_video_error_background(&self) {
        self.imp().video.get().set_visible(false);
        self.imp().picture.get().set_visible(false);
        self.set_spinner_visible(false);
        self.set_video_error_visible(true);
    }

    fn reveal_prepared_video_stage(
        &self,
        stream: &impl IsA<gtk::MediaStream>,
        token: u64,
        restore_motion_on_error: bool,
    ) {
        if stream.error().is_some() {
            self.set_spinner_visible(false);
            if restore_motion_on_error {
                self.restore_image_after_motion_video(token);
            } else {
                self.show_video_error_background();
            }
            return;
        }
        if !should_reveal_prepared_video_stage(
            token,
            self.imp().current_token.get(),
            stream.is_prepared(),
        ) {
            return;
        }
        self.imp().picture.get().set_visible(false);
        self.set_video_error_visible(false);
        self.imp().video.get().set_visible(true);
        self.set_spinner_visible(false);
        self.imp().video.get().grab_focus();
    }

    pub(super) fn request_current_preview_thumbnail(&self, item: &MediaItem, token: u64) {
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        let fs_mtime = std::fs::metadata(&item.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let item_mtime = fs_mtime.unwrap_or_else(|| std::time::SystemTime::from(item.file_mtime));
        let item_id = item.id;
        let item_uri = item.uri.clone();
        let item_name = item.display_name().to_string();
        let size = viewer_preview_thumbnail_size();
        // `viewer:thumb_preview` spans the async wait from request to paint —
        // the trace replacement for the old `switch_to_thumb_ms` timing log.
        let thumb_span = tracing::info_span!(
            "viewer:thumb_preview",
            token,
            item_id,
            item_name = %item_name,
            size = ?size,
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request_for_media(item_id, item_uri, size, Some(item_mtime), tx, TIER_BOOST);

        let viewer_weak = self.downgrade();
        glib::spawn_future_local(async move {
            let _thumb_span = thumb_span.enter();
            let Ok(loaded) = rx.await else {
                return;
            };
            let Some(this) = viewer_weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            let texture = loaded.texture;
            if this.imp().animated_image_source.borrow().is_some() {
                return;
            }
            if original_has_landed(this.imp().original_painted_token.get(), token) {
                return;
            }
            this.imp().picture.get().set_paintable(Some(&texture));
            this.set_spinner_visible(false);
        });
    }

    pub(super) fn request_current_original_image(
        &self,
        path: PathBuf,
        token: u64,
        item_name: String,
    ) {
        // Decode the current image off the main thread. `Pixbuf::from_file`
        // dispatches via gdk-pixbuf loaders (JPEG/PNG/HEIC/AVIF/...) and is
        // CPU-bound for big images — `spawn_blocking` keeps the UI responsive.
        // We use `gio::spawn_blocking` (matches `editor_panel.rs`) rather than
        // `tokio::task::spawn_blocking`. Pixbuf itself is `!Send`, so the
        // worker converts it to a `gdk::Texture` (which IS Send) before
        // returning — that way we can hand the texture across the oneshot.
        //
        // `viewer:orig_decode` spans the async wait from dispatch to paint, so
        // the trace carries the original-decode latency that `viewer:show_at`
        // (which returns at dispatch) cannot cover on its own.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let decode_span = tracing::info_span!(
            "viewer:orig_decode",
            token,
            item_name = %item_name,
        );
        gio::spawn_blocking(move || {
            let result = orientation::load_oriented_pixbuf(&path)
                .map(|pb| gdk::Texture::for_pixbuf(&pb))
                .map_err(|e| format!("load_oriented_pixbuf({path:?}) failed: {e}"));
            let _ = tx.send(result);
        });

        let viewer_weak = self.downgrade();
        glib::spawn_future_local(async move {
            let _decode_span = decode_span.enter();
            let texture = match rx.await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => {
                    tracing::warn!("ViewerPage: {e}");
                    if let Some(this) = viewer_weak.upgrade() {
                        if this.imp().current_token.get() == token {
                            this.set_spinner_visible(false);
                        }
                    }
                    return;
                }
                Err(_) => return, // sender dropped — cancelled
            };
            // Stale response: another show_at() ran in the meantime.
            let Some(this) = viewer_weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            // Record that the authoritative original has landed for this token
            // BEFORE painting, so a late preview-thumbnail callback (which can
            // finish after the original for non-JPEG sources) won't clobber it
            // (see `original_has_landed`).
            this.imp().original_painted_token.set(token);
            this.imp().picture.get().set_paintable(Some(&texture));
            this.set_spinner_visible(false);
            this.imp().edit_btn.get().set_sensitive(true);
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_TRACE original_painted token={} item_name={} texture={}x{}",
                token,
                item_name,
                texture.width(),
                texture.height()
            );
        });
    }

    pub(super) fn setup_video_playback_interactions(&self) {
        let video = self.imp().video.get();
        video.set_focusable(true);

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = self.downgrade();
        click.connect_released(move |gesture, _, _, y| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let widget = gesture.widget();
            if !should_toggle_video_from_stage_click(y, widget.allocated_height() as f64) {
                return;
            }
            if this.toggle_video_playback() {
                this.imp().video.get().grab_focus();
            }
        });
        video.add_controller(click);
    }
}

#[cfg(test)]
#[path = "stage/tests.rs"]
mod tests;
