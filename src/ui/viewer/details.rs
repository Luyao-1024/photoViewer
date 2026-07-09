#![allow(dead_code)]

use crate::core::i18n::tr;
use crate::core::media::MediaItem;
use crate::core::metadata::{self, ExifSummary, VideoSummary};
use chrono::{Local, Utc};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::PathBuf;

use super::ViewerPage;

pub(super) fn action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(false)
        .build()
}

pub(super) fn format_exposure(num: u32, den: u32) -> String {
    if den == 0 {
        return format!("{}/{}s", num, den);
    }
    let v = num as f64 / den as f64;
    if v < 1.0 {
        let n = (1.0 / v).round() as u32;
        if n >= 10000 {
            format!("{:.4}s", v)
        } else {
            format!("1/{}s", n)
        }
    } else {
        format!("{:.1}s", v)
    }
}

pub(super) fn format_duration(secs: f64) -> Option<String> {
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    Some(if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    })
}

pub(super) fn format_bitrate(bps: u64) -> Option<String> {
    if bps == 0 {
        return None;
    }
    let mbps = bps as f64 / 1_000_000.0;
    Some(if mbps >= 1.0 {
        format!("{:.1} Mbps", mbps)
    } else {
        format!("{} kbps", bps / 1000)
    })
}

pub(super) fn format_dimensions(width: Option<u32>, height: Option<u32>) -> String {
    match (width, height) {
        (Some(width), Some(height)) => format!("{width} x {height}"),
        _ => tr("viewer.not_available"),
    }
}

pub(super) fn format_file_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{size} B")
    }
}

pub(super) fn format_datetime(value: Option<chrono::DateTime<Utc>>) -> String {
    value
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| tr("viewer.not_available"))
}

impl ViewerPage {
    pub(super) fn setup_details_panel(&self) {
        let imp = self.imp();

        let weak = self.downgrade();
        imp.details_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let split_view = this.imp().details_split_view.get();
            let before = split_view.shows_sidebar();
            let next = !split_view.shows_sidebar();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_btn clicked index={} before_revealed={} next_revealed={}",
                this.imp().current_index.get(),
                before,
                next
            );
            this.set_details_revealed(next, "details_btn");
            if next {
                if let Some(item) = this.current_media_item() {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG details_btn loading_details index={} name={}",
                        this.imp().current_index.get(),
                        item.display_name()
                    );
                    this.update_details(&item);
                }
            }
        });

        let weak = self.downgrade();
        imp.details_close_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let split_view = this.imp().details_split_view.get();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_close_btn clicked index={} before_revealed={}",
                this.imp().current_index.get(),
                split_view.shows_sidebar()
            );
            this.set_details_revealed(false, "details_close_btn");
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG details_close_btn after set_reveal_child(false) revealed={}",
                split_view.shows_sidebar()
            );
            this.log_nav_state("details_close_btn immediate");
            let weak_after = this.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak_after.upgrade() {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG details_close_btn idle_after revealed={} mapped={} visible={} root_is_some={}",
                        this.imp().details_split_view.get().shows_sidebar(),
                        this.is_mapped(),
                        this.is_visible(),
                        this.root().is_some()
                    );
                    this.log_nav_state("details_close_btn idle_after");
                } else {
                    tracing::debug!(target: crate::core::log_targets::VIEWER, "VIEWER_DEBUG details_close_btn idle_after viewer_dropped");
                }
            });
        });

        let weak = self.downgrade();
        imp.name_row.get().connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                this.start_inline_rename();
            }
        });

        let weak = self.downgrade();
        imp.name_entry.get().connect_activate(move |_| {
            if let Some(this) = weak.upgrade() {
                this.finish_inline_rename(true);
            }
        });

        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.finish_inline_rename(true);
            }
        });
        imp.name_entry.get().add_controller(focus);

        let key = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(this) = weak.upgrade() {
                    this.finish_inline_rename(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        imp.name_entry.get().add_controller(key);
    }

    pub(super) fn set_details_revealed(&self, revealed: bool, reason: &str) {
        let split_view = self.imp().details_split_view.get();
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_details_revealed reason={} index={} from={} to={} can_pop_before={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            revealed,
            self.can_pop()
        );

        if revealed {
            self.set_details_sidebar_child_visible(true);
        }
        split_view.set_show_sidebar(revealed);

        if revealed {
            // While the side panel is open, the viewer page must not be popped
            // by NavigationView's built-in back gesture/action.
            self.set_can_pop(false);
        } else {
            // Keep pop disabled until the slide transition finishes. The log
            // evidence showed NavigationView can emit a delayed built-in pop
            // shortly after the details revealer starts closing.
            self.set_can_pop(false);
            let weak = self.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
                let Some(this) = weak.upgrade() else {
                    tracing::debug!(target: crate::core::log_targets::VIEWER, "VIEWER_DEBUG restore_can_pop viewer_dropped");
                    return;
                };
                if !this.imp().details_split_view.get().shows_sidebar() {
                    this.set_details_sidebar_child_visible(false);
                    this.set_can_pop(true);
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG restore_can_pop restored index={} can_pop={} visible={:?}",
                        this.imp().current_index.get(),
                        this.can_pop(),
                        this.imp()
                            .nav_view
                            .borrow()
                            .as_ref()
                            .and_then(|nav| nav.visible_page())
                            .map(|page| page.title())
                    );
                } else {
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_DEBUG restore_can_pop skipped_details_open index={} can_pop={}",
                        this.imp().current_index.get(),
                        this.can_pop()
                    );
                }
            });
        }

        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG set_details_revealed done reason={} index={} revealed={} can_pop_after={}",
            reason,
            self.imp().current_index.get(),
            split_view.shows_sidebar(),
            self.can_pop()
        );
    }

    pub(super) fn update_details(&self, item: &MediaItem) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG update_details index={} name={} path={}",
            self.imp().current_index.get(),
            item.display_name(),
            item.path.display()
        );
        let imp = self.imp();
        imp.name_row.get().set_title(&tr("viewer.details.name"));
        imp.folder_row.get().set_title(&tr("viewer.details.folder"));
        imp.mime_row.get().set_title(&tr("viewer.details.type"));
        imp.dimensions_row
            .get()
            .set_title(&tr("viewer.details.dimensions"));
        imp.size_row.get().set_title(&tr("viewer.details.size"));
        imp.taken_row
            .get()
            .set_title(&tr("viewer.details.captured"));

        imp.name_entry.get().set_visible(false);
        imp.name_row.get().set_subtitle(item.display_name());
        imp.folder_row
            .get()
            .set_subtitle(&item.folder_path.to_string_lossy());
        imp.mime_row.get().set_subtitle(&item.mime_type);

        // Hide rows whose value is absent instead of showing "Not available".
        let dim = format_dimensions(item.width, item.height);
        imp.dimensions_row
            .get()
            .set_visible(item.width.is_some() && item.height.is_some());
        imp.dimensions_row.get().set_subtitle(&dim);

        if item.file_size > 0 {
            imp.size_row.get().set_visible(true);
            imp.size_row
                .get()
                .set_subtitle(&format_file_size(item.file_size));
        } else {
            imp.size_row.get().set_visible(false);
        }

        if let Some(dt) = item.taken_at {
            imp.taken_row.get().set_visible(true);
            imp.taken_row.get().set_subtitle(&format_datetime(Some(dt)));
        } else {
            // No value in DB — hide for now. If the fresh EXIF parse below
            // finds one, the callback will make it visible again.
            imp.taken_row.get().set_visible(false);
        }

        self.clear_camera_rows();
        self.clear_video_rows();
        if item.is_video() {
            self.load_video_details(item.path.clone(), self.imp().current_token.get());
        } else {
            self.load_camera_details(item.path.clone(), self.imp().current_token.get());
        }
    }

    /// Walk up from an ActionRow to its owning PreferencesGroup.
    fn file_group(&self) -> Option<adw::PreferencesGroup> {
        self.imp()
            .name_row
            .get()
            .ancestor(adw::PreferencesGroup::static_type())
            .and_downcast::<adw::PreferencesGroup>()
    }

    /// Remove all dynamically-created camera-parameter rows from the file group.
    fn clear_camera_rows(&self) {
        if let Some(g) = &self.file_group() {
            for row in self.imp().camera_rows.borrow_mut().drain(..) {
                g.remove(&row);
            }
        }
    }

    fn load_camera_details(&self, path: PathBuf, token: u64) {
        let path_dbg = path.display().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let meta = metadata::extract(&path).ok();
            let summary = meta.as_ref().and_then(|m| m.camera.clone());
            let taken_at = meta.as_ref().and_then(|m| m.taken_at);
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "load_camera_details spawn_blocking path={} summary_some={} taken_at_some={}",
                path_dbg,
                summary.is_some(),
                taken_at.is_some(),
            );
            let _ = tx.send((summary, taken_at));
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok((summary, taken_at)) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            // If the stored MediaItem has no taken_at (e.g. HEIC scanned
            // before the ISOBMFF parser was fixed), fill it from the fresh
            // EXIF parse.
            if let Some(dt) = taken_at {
                let imp = this.imp();
                let row = imp.taken_row.get();
                if !row.is_visible() {
                    row.set_visible(true);
                }
                row.set_subtitle(&format_datetime(Some(dt)));
            }
            this.populate_camera_rows(summary);
        });
    }

    /// Build `ActionRow`s from `ExifSummary` and append them to the file
    /// group (same group that holds name / folder / dimensions etc.).
    ///
    /// Related parameters are merged into fewer rows with compact notation
    /// so the details panel stays scannable.
    fn populate_camera_rows(&self, summary: Option<ExifSummary>) {
        let Some(group) = self.file_group() else {
            tracing::warn!("populate_camera_rows: no PreferencesGroup for name_row");
            return;
        };
        let imp = self.imp();

        let Some(s) = summary else {
            return;
        };

        let mut rows = imp.camera_rows.borrow_mut();

        // Device: Make + Model (phone lens name duplicates body/focal/aperture,
        // so we skip the lens row and show only the merged body here).
        let body = match (&s.make, &s.model) {
            (Some(mk), Some(md)) if md.contains(mk.as_str()) => md.clone(),
            (Some(mk), Some(md)) => format!("{} {}", mk, md),
            (_, Some(md)) => md.clone(),
            (Some(mk), _) => mk.clone(),
            _ => String::new(),
        };
        if !body.is_empty() {
            let row = action_row(&tr("camera.body"), &body);
            group.add(&row);
            rows.push(row);
        }

        // Exposure triangle: aperture, shutter, ISO
        let mut exp: Vec<String> = Vec::new();
        if let Some(v) = s.aperture {
            exp.push(format!("f/{:.1}", v));
        }
        if let Some((num, den)) = s.exposure_time {
            exp.push(if den == 0 {
                format!("{}/{}s", num, den)
            } else {
                format_exposure(num, den)
            });
        }
        if let Some(v) = s.iso {
            exp.push(format!("ISO {}", v));
        }
        if !exp.is_empty() {
            let row = action_row(&tr("camera.exposure"), &exp.join("  "));
            group.add(&row);
            rows.push(row);
        }

        // Focal length + 35mm eq
        match (s.focal_length_mm, s.focal_length_35mm_mm) {
            (Some(fl), Some(fl35)) => {
                let row = action_row(
                    &tr("camera.focal_length"),
                    &format!("{:.1} mm  (35mm: {} mm)", fl, fl35),
                );
                group.add(&row);
                rows.push(row);
            }
            (Some(fl), None) => {
                let row = action_row(&tr("camera.focal_length"), &format!("{:.1} mm", fl));
                group.add(&row);
                rows.push(row);
            }
            (None, Some(fl35)) => {
                let row = action_row(&tr("camera.focal_length"), &format!("35mm: {} mm", fl35));
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Exposure mode + bias
        let mode_str = s.exposure_mode.map(|m| {
            use crate::core::metadata::ExposureMode;
            tr(match m {
                ExposureMode::Auto => "camera.exposure_mode.auto",
                ExposureMode::Manual => "camera.exposure_mode.manual",
                ExposureMode::AutoBracket => "camera.exposure_mode.auto_bracket",
                ExposureMode::AperturePriority => "camera.exposure_mode.aperture_priority",
                ExposureMode::ShutterPriority => "camera.exposure_mode.shutter_priority",
                ExposureMode::Program => "camera.exposure_mode.program",
            })
        });
        let bias_str = s.exposure_bias_ev.map(|v| {
            let sign = if v >= 0.0 { "+" } else { "" };
            format!("{}{:.1} EV", sign, v)
        });
        match (mode_str, bias_str) {
            (Some(m), Some(b)) => {
                let row = action_row(&tr("camera.exposure_mode"), &format!("{}, {}", m, b));
                group.add(&row);
                rows.push(row);
            }
            (Some(m), None) => {
                let row = action_row(&tr("camera.exposure_mode"), &m);
                group.add(&row);
                rows.push(row);
            }
            (None, Some(b)) => {
                let row = action_row(&tr("camera.exposure_bias"), &b);
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Location: GPS + altitude
        let gps_str = s.gps.as_ref().map(|gps| {
            format!(
                "{}°{}′{:.1}″{}  {}°{}′{:.1}″{}",
                gps.lat.deg,
                gps.lat.min,
                gps.lat.sec,
                if gps.lat.north_or_east { "N" } else { "S" },
                gps.lon.deg,
                gps.lon.min,
                gps.lon.sec,
                if gps.lon.north_or_east { "E" } else { "W" },
            )
        });
        let alt_str = s.altitude_m.map(|a| format!("{:.1} m", a));
        match (gps_str, alt_str) {
            (Some(g), Some(a)) => {
                let row = action_row(&tr("camera.location"), &format!("{}  .  {}", g, a));
                group.add(&row);
                rows.push(row);
            }
            (Some(g), None) => {
                let row = action_row(&tr("camera.location"), &g);
                group.add(&row);
                rows.push(row);
            }
            (None, Some(a)) => {
                let row = action_row(&tr("camera.location"), &a);
                group.add(&row);
                rows.push(row);
            }
            _ => {}
        }

        // Secondary: metering, flash, WB
        let metering_str = s.metering_mode.map(|m| {
            use crate::core::metadata::MeteringMode;
            tr(match m {
                MeteringMode::Average => "camera.metering.average",
                MeteringMode::CenterWeighted => "camera.metering.center_weighted",
                MeteringMode::Spot => "camera.metering.spot",
                MeteringMode::Other => "camera.metering.other",
            })
        });
        let flash_str = s.flash.and_then(|f| {
            use crate::core::metadata::FlashState;
            match f {
                FlashState::Fired => Some(tr("camera.flash.fired")),
                FlashState::NotFired => None,
            }
        });
        let wb_str = s.white_balance.and_then(|w| {
            use crate::core::metadata::WhiteBalance;
            match w {
                WhiteBalance::Auto => None,
                WhiteBalance::Manual => Some(tr("camera.white_balance.manual")),
            }
        });
        let secondary: Vec<String> = [metering_str, flash_str, wb_str]
            .into_iter()
            .flatten()
            .collect();
        if !secondary.is_empty() {
            let row = action_row(&tr("camera.secondary"), &secondary.join("  .  "));
            group.add(&row);
            rows.push(row);
        }
    }

    /// Remove all dynamically-created video-info rows from the file group.
    fn clear_video_rows(&self) {
        if let Some(g) = &self.file_group() {
            for row in self.imp().video_rows.borrow_mut().drain(..) {
                g.remove(&row);
            }
        }
    }

    /// 异步加载视频元数据（ffprobe），完成后填充视频属性行；带 token 过期保护。
    /// 镜像 [`load_camera_details`]。
    fn load_video_details(&self, path: PathBuf, token: u64) {
        let path_dbg = path.display().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        gio::spawn_blocking(move || {
            let meta = metadata::extract(&path).ok();
            let summary = meta.as_ref().and_then(|m| m.video.clone());
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "load_video_details spawn_blocking path={} summary_some={}",
                path_dbg,
                summary.is_some(),
            );
            let _ = tx.send(summary);
        });

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(summary) = rx.await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().current_token.get() != token {
                return;
            }
            this.populate_video_rows(summary.as_ref());
        });
    }

    /// Build video `ActionRow`s (duration / codec / fps / bitrate / container /
    /// device) and append them to the file group. Mirrors `populate_camera_rows`.
    fn populate_video_rows(&self, summary: Option<&VideoSummary>) {
        let Some(group) = self.file_group() else {
            tracing::warn!("populate_video_rows: no PreferencesGroup for name_row");
            return;
        };
        let Some(s) = summary else {
            return;
        };

        let mut rows = self.imp().video_rows.borrow_mut();

        if let Some(d) = s.duration_secs {
            if let Some(formatted) = format_duration(d) {
                let row = action_row(&tr("video.duration"), &formatted);
                group.add(&row);
                rows.push(row);
            }
        }
        if let Some(codec) = &s.codec {
            let row = action_row(&tr("video.codec"), codec);
            group.add(&row);
            rows.push(row);
        }
        if let Some(fps) = s.fps {
            let row = action_row(&tr("video.fps"), &format!("{:.0} fps", fps.round()));
            group.add(&row);
            rows.push(row);
        }
        if let Some(br) = s.bitrate {
            if let Some(formatted) = format_bitrate(br) {
                let row = action_row(&tr("video.bitrate"), &formatted);
                group.add(&row);
                rows.push(row);
            }
        }
        if let Some(container) = &s.container {
            let row = action_row(&tr("video.container"), container);
            group.add(&row);
            rows.push(row);
        }
        // Device: make + model 合并（与相机行一致）。
        let body = match (&s.make, &s.model) {
            (Some(mk), Some(md)) if md.contains(mk.as_str()) => md.clone(),
            (Some(mk), Some(md)) => format!("{} {}", mk, md),
            (_, Some(md)) => md.clone(),
            (Some(mk), _) => mk.clone(),
            _ => String::new(),
        };
        if !body.is_empty() {
            let row = action_row(&tr("video.device"), &body);
            group.add(&row);
            rows.push(row);
        }
    }
}
