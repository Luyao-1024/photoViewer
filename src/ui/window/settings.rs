use super::{build_about_label, widget_or_ancestor_has_class, MainWindow};

use crate::config;
use crate::core::db::DbPool;
use crate::core::i18n::{locale, tr, trf};
use crate::core::prefs::{self, TrashBackend};
use crate::core::runtime_config;
use crate::ui::{grid_css, theme};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::{Map, Value};
use std::cell::Cell;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

impl MainWindow {
    pub(super) fn build_settings_dialog(&self, host: &gtk::Widget) -> adw::Dialog {
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(false)
            .min_content_height(0)
            .max_content_height(700)
            .child(&self.build_settings_page(host))
            .build();

        let dialog = adw::Dialog::builder()
            .title(tr("setting.page.title"))
            .content_width(540)
            .content_height(700)
            .child(&scroller)
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_css_class("settings-dialog-backdrop");
        dialog.set_can_close(true);
        add_close_on_backdrop_click(&dialog);
        dialog
    }

    pub(super) fn build_settings_page(&self, parent: &gtk::Widget) -> gtk::Box {
        let current = locale().to_string();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        content.add_css_class("settings-dialog-content");

        let title = gtk::Label::new(Some(&tr("setting.section.language")));
        title.set_xalign(0.0);
        content.append(&title);

        let description = gtk::Label::new(Some(&tr("setting.section.language_description")));
        description.set_wrap(true);
        description.set_xalign(0.0);
        content.append(&description);

        let lang_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let btn_zh = gtk::Button::with_label(&tr("setting.lang.zh"));
        let btn_en = gtk::Button::with_label(&tr("setting.lang.en"));

        btn_zh.set_sensitive(current != "zh-CN");
        btn_en.set_sensitive(current != "en");

        let parent_for_zh = parent.clone();
        let parent_for_en = parent.clone();
        let btn_zh_ref = btn_zh.clone();
        let btn_en_ref = btn_en.clone();
        let btn_zh_ref2 = btn_zh.clone();
        let btn_en_ref2 = btn_en.clone();

        btn_zh.connect_clicked(move |_| match persist_locale("zh-CN") {
            Ok(()) => {
                show_restart_required_dialog(&parent_for_zh);
                btn_zh_ref.set_sensitive(false);
                btn_en_ref.set_sensitive(true);
            }
            Err(err) => {
                show_settings_restart_dialog(&parent_for_zh, false, Some(err));
            }
        });

        btn_en.connect_clicked(move |_| match persist_locale("en") {
            Ok(()) => {
                show_restart_required_dialog(&parent_for_en);
                btn_zh_ref2.set_sensitive(true);
                btn_en_ref2.set_sensitive(false);
            }
            Err(err) => {
                show_settings_restart_dialog(&parent_for_en, false, Some(err));
            }
        });

        lang_box.append(&btn_zh);
        lang_box.append(&btn_en);
        content.append(&lang_box);

        let appearance_group = adw::PreferencesGroup::new();
        appearance_group.set_title(&tr("setting.section.appearance"));
        appearance_group.add_css_class("settings-preferences-group");
        content.append(&appearance_group);

        let btn_theme_system = gtk::CheckButton::with_label(&tr("setting.theme.system"));
        let btn_theme_light = gtk::CheckButton::with_label(&tr("setting.theme.light"));
        let btn_theme_dark = gtk::CheckButton::with_label(&tr("setting.theme.dark"));
        btn_theme_light.set_group(Some(&btn_theme_system));
        btn_theme_dark.set_group(Some(&btn_theme_system));

        match prefs::theme_preference() {
            prefs::ThemePreference::System => btn_theme_system.set_active(true),
            prefs::ThemePreference::Light => btn_theme_light.set_active(true),
            prefs::ThemePreference::Dark => btn_theme_dark.set_active(true),
        }

        let theme_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        theme_box.set_valign(gtk::Align::Center);
        theme_box.append(&btn_theme_system);
        theme_box.append(&btn_theme_light);
        theme_box.append(&btn_theme_dark);

        let theme_row = adw::ActionRow::new();
        theme_row.add_css_class("settings-action-row");
        theme_row.set_title(&tr("setting.theme"));
        theme_row.set_activatable(false);
        theme_row.add_suffix(&theme_box);
        appearance_group.add(&theme_row);

        let parent_for_theme = parent.clone();
        let connect_theme_btn =
            move |btn: &gtk::CheckButton, preference: prefs::ThemePreference| {
                let parent = parent_for_theme.clone();
                btn.connect_toggled(move |btn| {
                    if !btn.is_active() {
                        return;
                    }
                    match prefs::set_theme_preference(preference) {
                        Ok(()) => theme::apply(preference),
                        Err(err) => show_settings_error_dialog(
                            &parent,
                            &trf("setting.theme_save_failed", &[("error", &err)]),
                        ),
                    }
                });
            };
        connect_theme_btn(&btn_theme_system, prefs::ThemePreference::System);
        connect_theme_btn(&btn_theme_light, prefs::ThemePreference::Light);
        connect_theme_btn(&btn_theme_dark, prefs::ThemePreference::Dark);

        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::liquid_glass_enabled())
            .build();

        let glass_row = adw::ActionRow::new();
        glass_row.add_css_class("settings-action-row");
        glass_row.set_title(&tr("setting.liquid_glass"));
        glass_row.set_activatable(false);
        glass_row.add_suffix(&switch);
        appearance_group.add(&glass_row);

        let parent_for_glass = parent.clone();
        switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            let active = sw.is_active();
            match prefs::set_liquid_glass(active) {
                Ok(()) => grid_css::reapply(active),
                Err(err) => {
                    show_settings_error_dialog(
                        &parent_for_glass,
                        &trf("setting.liquid_glass_save_failed", &[("error", &err)]),
                    );
                }
            }
        });

        let transparency_scale =
            gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        transparency_scale.set_hexpand(true);
        transparency_scale.set_size_request(300, -1);
        transparency_scale.set_digits(0);
        transparency_scale.set_value(prefs::liquid_glass_transparency() * 100.0);
        for mark in (0..=100).step_by(10) {
            let label = mark.to_string();
            transparency_scale.add_mark(mark as f64, gtk::PositionType::Bottom, Some(&label));
        }

        let transparency_row = adw::ActionRow::new();
        transparency_row.add_css_class("settings-action-row");
        transparency_row.set_title(&tr("setting.liquid_glass_transparency"));
        transparency_row.set_activatable(false);
        transparency_row.add_suffix(&transparency_scale);
        appearance_group.add(&transparency_row);

        let parent_for_transparency = parent.clone();
        transparency_scale.connect_value_changed(move |scale| {
            let transparency = scale.value() / 100.0;
            match prefs::set_liquid_glass_transparency(transparency) {
                Ok(()) => grid_css::reapply(prefs::liquid_glass_enabled()),
                Err(err) => {
                    show_settings_error_dialog(
                        &parent_for_transparency,
                        &trf(
                            "setting.liquid_glass_transparency_save_failed",
                            &[("error", &err)],
                        ),
                    );
                }
            }
        });

        let video_group = adw::PreferencesGroup::new();
        video_group.set_title(&tr("setting.section.video"));
        video_group.add_css_class("settings-preferences-group");
        content.append(&video_group);

        let muted_switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::video_default_muted())
            .build();

        let muted_row = adw::ActionRow::new();
        muted_row.add_css_class("settings-action-row");
        muted_row.set_title(&tr("setting.video_default_muted"));
        muted_row.set_activatable(false);
        muted_row.add_suffix(&muted_switch);
        video_group.add(&muted_row);

        let parent_for_muted = parent.clone();
        muted_switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            if let Err(err) = prefs::set_video_default_muted(sw.is_active()) {
                show_settings_error_dialog(
                    &parent_for_muted,
                    &trf(
                        "setting.video_default_muted_save_failed",
                        &[("error", &err)],
                    ),
                );
            }
        });

        let auto_play_motion_switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(prefs::auto_play_motion_photo())
            .build();

        let auto_play_motion_row = adw::ActionRow::new();
        auto_play_motion_row.add_css_class("settings-action-row");
        auto_play_motion_row.set_title(&tr("setting.auto_play_motion_photo"));
        auto_play_motion_row.set_activatable(false);
        auto_play_motion_row.add_suffix(&auto_play_motion_switch);
        video_group.add(&auto_play_motion_row);

        let parent_for_auto_play = parent.clone();
        auto_play_motion_switch.connect_notify_local(Some("active"), move |sw, _pspec| {
            if let Err(err) = prefs::set_auto_play_motion_photo(sw.is_active()) {
                show_settings_error_dialog(
                    &parent_for_auto_play,
                    &trf(
                        "setting.auto_play_motion_photo_save_failed",
                        &[("error", &err)],
                    ),
                );
            }
        });

        content.append(&build_scan_paths_group(parent));
        content.append(&self.build_trash_settings_group(parent));

        let grid_group = adw::PreferencesGroup::new();
        grid_group.set_title(&tr("setting.section.grid"));
        grid_group.set_description(Some(&tr("setting.section.grid_description")));
        grid_group.add_css_class("settings-preferences-group");
        content.append(&grid_group);

        let initial_columns = runtime_config::photos_grid_columns();
        let columns_value = gtk::Label::new(Some(&initial_columns.to_string()));
        columns_value.add_css_class("settings-stepper-value");
        columns_value.set_width_chars(2);
        columns_value.set_xalign(0.5);

        let columns_minus = gtk::Button::from_icon_name("list-remove-symbolic");
        let columns_plus = gtk::Button::from_icon_name("list-add-symbolic");
        for button in [&columns_minus, &columns_plus] {
            button.add_css_class("settings-stepper-button");
            button.set_valign(gtk::Align::Center);
            button.set_focus_on_click(false);
        }
        columns_minus.set_sensitive(initial_columns > runtime_config::MIN_PHOTOS_GRID_COLUMNS);
        columns_plus.set_sensitive(initial_columns < runtime_config::MAX_PHOTOS_GRID_COLUMNS);

        let columns_stepper = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        columns_stepper.add_css_class("settings-stepper");
        columns_stepper.append(&columns_minus);
        columns_stepper.append(&columns_value);
        columns_stepper.append(&columns_plus);

        let columns_row = adw::ActionRow::new();
        columns_row.add_css_class("settings-action-row");
        columns_row.set_title(&tr("setting.photos_grid_columns"));
        columns_row.set_subtitle(&tr("setting.photos_grid_columns_description"));
        columns_row.set_activatable(false);
        columns_row.add_suffix(&columns_stepper);
        grid_group.add(&columns_row);

        let parent_for_columns = parent.clone();
        let current_columns = Rc::new(Cell::new(initial_columns));
        let current_for_apply = current_columns.clone();
        let value_for_apply = columns_value.clone();
        let minus_for_apply = columns_minus.clone();
        let plus_for_apply = columns_plus.clone();
        let apply_columns =
            Rc::new(
                move |columns: usize| match runtime_config::set_photos_grid_columns(columns) {
                    Ok(()) => {
                        tracing::trace!(
                            target: "ui::grid_settings",
                            columns,
                            "day_grid_columns_apply_saved"
                        );
                        current_for_apply.set(columns);
                        value_for_apply.set_label(&columns.to_string());
                        minus_for_apply
                            .set_sensitive(columns > runtime_config::MIN_PHOTOS_GRID_COLUMNS);
                        plus_for_apply
                            .set_sensitive(columns < runtime_config::MAX_PHOTOS_GRID_COLUMNS);
                        if let Ok(window) = parent_for_columns.clone().downcast::<MainWindow>() {
                            window.set_photos_grid_columns(columns);
                        }
                    }
                    Err(err) => show_settings_error_dialog(
                        &parent_for_columns,
                        &trf(
                            "setting.photos_grid_columns_save_failed",
                            &[("error", &err)],
                        ),
                    ),
                },
            );

        let apply_minus = apply_columns.clone();
        let current_for_minus = current_columns.clone();
        columns_minus.connect_clicked(move |_| {
            tracing::trace!(target: "ui::grid_settings", "day_grid_columns_minus_clicked");
            let columns = current_for_minus
                .get()
                .saturating_sub(1)
                .max(runtime_config::MIN_PHOTOS_GRID_COLUMNS);
            apply_minus(columns);
        });

        let current_for_plus = current_columns;
        columns_plus.connect_clicked(move |_| {
            tracing::trace!(target: "ui::grid_settings", "day_grid_columns_plus_clicked");
            let columns = current_for_plus
                .get()
                .saturating_add(1)
                .min(runtime_config::MAX_PHOTOS_GRID_COLUMNS);
            apply_columns(columns);
        });

        let storage_group = adw::PreferencesGroup::new();
        storage_group.set_title(&tr("setting.section.storage"));
        storage_group.set_description(Some(&tr("setting.section.storage_description")));
        storage_group.add_css_class("settings-preferences-group");
        content.append(&storage_group);

        let slow_label = tr("setting.thumbnail_generation_speed.slow");
        let normal_label = tr("setting.thumbnail_generation_speed.normal");
        let fast_label = tr("setting.thumbnail_generation_speed.fast");
        let fastest_label = tr("setting.thumbnail_generation_speed.fastest");

        let btn_slow = gtk::CheckButton::with_label(&slow_label);
        let btn_normal = gtk::CheckButton::with_label(&normal_label);
        let btn_fast = gtk::CheckButton::with_label(&fast_label);
        let btn_fastest = gtk::CheckButton::with_label(&fastest_label);
        btn_normal.set_group(Some(&btn_slow));
        btn_fast.set_group(Some(&btn_slow));
        btn_fastest.set_group(Some(&btn_slow));

        match runtime_config::thumbnail_generation_speed() {
            runtime_config::ThumbnailGenerationSpeed::Slow => btn_slow.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Normal => btn_normal.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Fast => btn_fast.set_active(true),
            runtime_config::ThumbnailGenerationSpeed::Fastest => btn_fastest.set_active(true),
        }

        let speed_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        speed_box.set_valign(gtk::Align::Center);
        speed_box.append(&btn_slow);
        speed_box.append(&btn_normal);
        speed_box.append(&btn_fast);
        speed_box.append(&btn_fastest);

        let speed_row = adw::ActionRow::new();
        speed_row.add_css_class("settings-action-row");
        speed_row.set_title(&tr("setting.thumbnail_generation_speed"));
        speed_row.set_subtitle(&tr("setting.thumbnail_generation_speed_description"));
        speed_row.set_activatable(false);
        speed_row.add_suffix(&speed_box);
        storage_group.add(&speed_row);

        let parent_for_speed = parent.clone();
        let connect_speed_btn =
            move |btn: &gtk::CheckButton, speed: runtime_config::ThumbnailGenerationSpeed| {
                let parent = parent_for_speed.clone();
                btn.connect_toggled(move |btn| {
                    if !btn.is_active() {
                        return;
                    }
                    if let Err(err) = runtime_config::set_thumbnail_generation_speed(speed) {
                        show_settings_error_dialog(
                            &parent,
                            &trf(
                                "setting.thumbnail_generation_speed_save_failed",
                                &[("error", &err)],
                            ),
                        );
                    } else {
                        show_restart_required_dialog(&parent);
                    }
                });
            };
        connect_speed_btn(&btn_slow, runtime_config::ThumbnailGenerationSpeed::Slow);
        connect_speed_btn(
            &btn_normal,
            runtime_config::ThumbnailGenerationSpeed::Normal,
        );
        connect_speed_btn(&btn_fast, runtime_config::ThumbnailGenerationSpeed::Fast);
        connect_speed_btn(
            &btn_fastest,
            runtime_config::ThumbnailGenerationSpeed::Fastest,
        );

        let cache_dir = config::cache_dir();
        let thumb_dir = cache_dir.join("thumbnails");
        let db_path = crate::config::data_dir().join("photos.db");

        let thumb_row = adw::ActionRow::new();
        thumb_row.add_css_class("settings-action-row");
        thumb_row.set_title(&tr("setting.clear_thumbnails"));
        thumb_row.set_activatable(false);
        update_storage_size_async(&thumb_row, move || crate::core::cache::dir_size(&thumb_dir));

        let btn_clear_thumbs = gtk::Button::new();
        btn_clear_thumbs.set_icon_name("user-trash-symbolic");
        btn_clear_thumbs.set_valign(gtk::Align::Center);
        btn_clear_thumbs.add_css_class("glass-toolbar-button");
        btn_clear_thumbs.add_css_class("glass-toolbar-danger");
        btn_clear_thumbs.set_tooltip_text(Some(&tr("setting.clear_thumbnails")));
        thumb_row.add_suffix(&btn_clear_thumbs);
        storage_group.add(&thumb_row);

        let parent_for_thumbs = parent.clone();
        let loader_for_thumbs = self.imp().loader.borrow().clone();
        let thumb_row_for_thumbs = thumb_row.clone();
        btn_clear_thumbs.connect_clicked(move |_| {
            let loader_clone = loader_for_thumbs.clone();
            let row_clone = thumb_row_for_thumbs.clone();
            show_clear_confirm_dialog(
                &parent_for_thumbs,
                &tr("setting.clear_thumbnails_confirm_title"),
                &tr("setting.clear_thumbnails_confirm_body"),
                move || {
                    let cache_dir = config::cache_dir();
                    let thumb_dir = cache_dir.join("thumbnails");
                    match crate::core::cache::enforce_size_limit(&thumb_dir, 0) {
                        Ok(count) => {
                            if let Some(ref loader) = loader_clone {
                                loader.clear_mem_cache();
                            }
                            row_clone.set_subtitle(&format_size(0));
                            show_clear_success_toast(&trf(
                                "setting.clear_thumbnails_success",
                                &[("count", &count.to_string())],
                            ));
                        }
                        Err(err) => {
                            show_clear_error_toast(&trf(
                                "setting.clear_failed",
                                &[("error", &err.to_string())],
                            ));
                        }
                    }
                },
            );
        });

        let db_row = adw::ActionRow::new();
        db_row.add_css_class("settings-action-row");
        db_row.set_title(&tr("setting.clear_database"));
        db_row.set_activatable(false);
        update_storage_size_async(&db_row, move || {
            std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0)
        });

        let btn_clear_db = gtk::Button::new();
        btn_clear_db.set_icon_name("user-trash-symbolic");
        btn_clear_db.set_valign(gtk::Align::Center);
        btn_clear_db.add_css_class("glass-toolbar-button");
        btn_clear_db.add_css_class("glass-toolbar-danger");
        btn_clear_db.set_tooltip_text(Some(&tr("setting.clear_database")));
        db_row.add_suffix(&btn_clear_db);
        storage_group.add(&db_row);

        let parent_for_db = parent.clone();
        let db_actor_for_db = self.imp().db_actor.borrow().clone();
        let loader_for_db = self.imp().loader.borrow().clone();
        let media_list_for_db = self.imp().media_list.borrow().clone();
        let db_row_for_db = db_row.clone();
        btn_clear_db.connect_clicked(move |_| {
            let db_actor_clone = db_actor_for_db.clone();
            let loader_clone = loader_for_db.clone();
            let media_list_clone = media_list_for_db.clone();
            let row_clone = db_row_for_db.clone();
            show_clear_confirm_dialog(
                &parent_for_db,
                &tr("setting.clear_database_confirm_title"),
                &tr("setting.clear_database_confirm_body"),
                move || {
                    if let Some(db_actor) = db_actor_clone.as_ref() {
                        match db_actor
                            .execute_blocking(crate::core::db_actor::DbCommand::ClearAllMedia)
                        {
                            Ok(crate::core::db_actor::DbCommandResult::Count(count)) => {
                                if let Some(ref loader) = loader_clone {
                                    loader.clear_mem_cache();
                                }
                                if let Some(ref media_list) = media_list_clone {
                                    media_list.remove_all();
                                }
                                row_clone.set_subtitle(&format_size(0));
                                show_clear_success_toast(&trf(
                                    "setting.clear_database_success",
                                    &[("count", &count.to_string())],
                                ));
                            }
                            Ok(other) => {
                                show_clear_error_toast(&trf(
                                    "setting.clear_failed",
                                    &[("error", &format!("unexpected result: {other:?}"))],
                                ));
                            }
                            Err(err) => {
                                show_clear_error_toast(&trf(
                                    "setting.clear_failed",
                                    &[("error", &err.to_string())],
                                ));
                            }
                        }
                    }
                },
            );
        });

        let spacer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        content.append(&spacer);
        content.append(&build_about_label());

        content
    }

    pub(super) fn build_trash_settings_group(&self, parent: &gtk::Widget) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::new();
        group.set_title(&tr("setting.section.trash"));
        group.set_description(Some(&tr("setting.section.trash_description")));
        group.add_css_class("settings-preferences-group");

        let status_row = adw::ActionRow::new();
        status_row.add_css_class("settings-action-row");
        status_row.set_title(&tr("setting.trash.backend"));
        status_row.set_subtitle(&trash_backend_subtitle(prefs::trash_backend(), None));
        status_row.set_activatable(false);

        let switch_button =
            gtk::Button::with_label(&trash_backend_switch_label(prefs::trash_backend(), None));
        switch_button.set_valign(gtk::Align::Center);
        switch_button.add_css_class("glass-toolbar-button");
        status_row.add_suffix(&switch_button);
        group.add(&status_row);

        let suggestion_row = adw::ActionRow::new();
        suggestion_row.add_css_class("settings-action-row");
        suggestion_row.set_title(&tr("setting.trash.system_available_title"));
        suggestion_row.set_subtitle(&tr("setting.trash.system_available_subtitle"));
        suggestion_row.set_activatable(false);
        suggestion_row.set_visible(false);
        let suggestion_button = gtk::Button::with_label(&tr("setting.trash.migrate_to_system"));
        suggestion_button.set_valign(gtk::Align::Center);
        suggestion_button.add_css_class("glass-toolbar-button");
        suggestion_button.add_css_class("suggested-action");
        suggestion_row.add_suffix(&suggestion_button);
        group.add(&suggestion_row);

        let pool = self.imp().pool.borrow().clone();
        let parent_for_switch = parent.clone();
        let status_for_switch = status_row.clone();
        let switch_for_switch = switch_button.clone();
        let suggestion_for_switch = suggestion_row.clone();
        let suggestion_button_for_switch = suggestion_button.clone();
        switch_button.connect_clicked(move |_| {
            let Some(pool) = pool.clone() else {
                show_settings_error_dialog(
                    &parent_for_switch,
                    &tr("setting.trash.switch_unavailable_without_database"),
                );
                return;
            };
            let target = match prefs::trash_backend() {
                TrashBackend::System => TrashBackend::App,
                TrashBackend::App => TrashBackend::System,
            };
            run_trash_backend_switch(
                &parent_for_switch,
                pool,
                target,
                &status_for_switch,
                &switch_for_switch,
                &suggestion_for_switch,
                &suggestion_button_for_switch,
            );
        });

        let pool_for_suggestion = self.imp().pool.borrow().clone();
        let parent_for_suggestion = parent.clone();
        let status_for_suggestion = status_row.clone();
        let switch_for_suggestion = switch_button.clone();
        let suggestion_for_suggestion = suggestion_row.clone();
        let suggestion_button_for_suggestion = suggestion_button.clone();
        suggestion_button.connect_clicked(move |_| {
            let Some(pool) = pool_for_suggestion.clone() else {
                show_settings_error_dialog(
                    &parent_for_suggestion,
                    &tr("setting.trash.switch_unavailable_without_database"),
                );
                return;
            };
            run_trash_backend_switch(
                &parent_for_suggestion,
                pool,
                TrashBackend::System,
                &status_for_suggestion,
                &switch_for_suggestion,
                &suggestion_for_suggestion,
                &suggestion_button_for_suggestion,
            );
        });

        #[cfg(not(test))]
        {
            let status_for_probe = status_row.clone();
            let switch_for_probe = switch_button.clone();
            let suggestion_for_probe = suggestion_row.clone();
            let suggestion_button_for_probe = suggestion_button.clone();
            glib::spawn_future_local(async move {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_for_probe,
                    &switch_for_probe,
                    &suggestion_for_probe,
                    &suggestion_button_for_probe,
                    Some(system_available),
                    false,
                );
            });
        }

        group
    }
}

pub(super) fn add_close_on_backdrop_click(dialog: &adw::Dialog) {
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(glib::clone!(@weak dialog => move |_, _n_press, x, y| {
        let picked = dialog.pick(x, y, gtk::PickFlags::DEFAULT);
        if !picked
            .as_ref()
            .is_some_and(|widget| widget_or_ancestor_has_class(widget, "settings-dialog-content"))
        {
            let _ = dialog.close();
        }
    }));
    dialog.add_controller(gesture);
}

fn trash_backend_label(backend: TrashBackend) -> String {
    match backend {
        TrashBackend::System => tr("setting.trash.backend.system"),
        TrashBackend::App => tr("setting.trash.backend.app"),
    }
}

pub(super) fn trash_backend_subtitle(
    backend: TrashBackend,
    system_available: Option<bool>,
) -> String {
    match (backend, system_available) {
        (TrashBackend::System, Some(false)) => tr("setting.trash.system_unavailable"),
        (TrashBackend::System, _) => tr("setting.trash.using_system"),
        (TrashBackend::App, Some(true)) => tr("setting.trash.using_app_system_available"),
        (TrashBackend::App, Some(false)) => tr("setting.trash.using_app_system_unavailable"),
        (TrashBackend::App, None) => tr("setting.trash.checking_system"),
    }
}

pub(super) fn trash_backend_switch_label(
    backend: TrashBackend,
    system_available: Option<bool>,
) -> String {
    match backend {
        TrashBackend::System => tr("setting.trash.switch_to_app"),
        TrashBackend::App if system_available == Some(false) => {
            tr("setting.trash.system_unavailable_short")
        }
        TrashBackend::App => tr("setting.trash.switch_to_system"),
    }
}

pub(super) fn update_trash_settings_state(
    status_row: &adw::ActionRow,
    switch_button: &gtk::Button,
    suggestion_row: &adw::ActionRow,
    suggestion_button: &gtk::Button,
    system_available: Option<bool>,
    busy: bool,
) {
    let backend = prefs::trash_backend();
    status_row.set_subtitle(&trash_backend_subtitle(backend, system_available));
    switch_button.set_label(&trash_backend_switch_label(backend, system_available));
    let switch_sensitive = !busy
        && match backend {
            TrashBackend::System => true,
            TrashBackend::App => system_available.unwrap_or(false),
        };
    switch_button.set_sensitive(switch_sensitive);
    suggestion_row.set_visible(backend == TrashBackend::App && system_available == Some(true));
    suggestion_button.set_sensitive(!busy && system_available == Some(true));
    if busy {
        status_row.set_subtitle(&tr("setting.trash.migrating"));
    }
}

pub(super) fn run_trash_backend_switch(
    parent: &gtk::Widget,
    pool: DbPool,
    target: TrashBackend,
    status_row: &adw::ActionRow,
    switch_button: &gtk::Button,
    suggestion_row: &adw::ActionRow,
    suggestion_button: &gtk::Button,
) {
    update_trash_settings_state(
        status_row,
        switch_button,
        suggestion_row,
        suggestion_button,
        None,
        true,
    );
    let parent = parent.clone();
    let status_row = status_row.clone();
    let switch_button = switch_button.clone();
    let suggestion_row = suggestion_row.clone();
    let suggestion_button = suggestion_button.clone();
    glib::spawn_future_local(async move {
        let result = gtk::gio::spawn_blocking(move || {
            crate::core::trash::switch_trash_backend(&pool, target)
        })
        .await;
        match result {
            Ok(Ok(stats)) => {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    Some(system_available),
                    false,
                );
                show_settings_info_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_success",
                        &[
                            ("backend", &trash_backend_label(target)),
                            ("count", &stats.moved.to_string()),
                        ],
                    ),
                );
            }
            Ok(Err(err)) => {
                let probe = gtk::gio::spawn_blocking(crate::core::trash::probe_system_trash).await;
                let system_available = matches!(probe, Ok(Ok(())));
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    Some(system_available),
                    false,
                );
                show_settings_error_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_failed",
                        &[("error", &err.to_string())],
                    ),
                );
            }
            Err(err) => {
                update_trash_settings_state(
                    &status_row,
                    &switch_button,
                    &suggestion_row,
                    &suggestion_button,
                    None,
                    false,
                );
                show_settings_error_dialog(
                    &parent,
                    &trf(
                        "setting.trash.switch_failed",
                        &[("error", &format!("{err:?}"))],
                    ),
                );
            }
        }
    });
}

pub(super) fn show_restart_required_dialog(parent: &gtk::Widget) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.restart_required_title"))
        .body(tr("setting.restart_required_body"))
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("later", &tr("button.no"));
    dialog.add_response("restart", &tr("button.yes"));
    dialog.set_default_response(Some("restart"));
    dialog.set_close_response("later");

    let parent_for_error = parent.clone();
    dialog.connect_response(Some("restart"), move |_, _| {
        if let Err(err) = restart_application() {
            show_settings_error_dialog(
                &parent_for_error,
                &trf("setting.restart_now_failed", &[("error", &err)]),
            );
        }
    });

    dialog.present(parent);
}

pub(super) fn show_settings_restart_dialog(
    parent: &gtk::Widget,
    success: bool,
    error: Option<String>,
) {
    let heading = if success {
        tr("setting.locale.saved")
    } else {
        tr("setting.locale.failed")
    };
    let body = if let Some(error) = error {
        trf("setting.restart_failed", &[("error", &error)])
    } else {
        tr("setting.restart_hint")
    };
    let dialog = adw::AlertDialog::builder()
        .heading(&heading)
        .body(&body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

pub(super) fn show_settings_error_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.save_failed"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

pub(super) fn show_settings_info_dialog(parent: &gtk::Widget, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("setting.done"))
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("ok", &tr("button.ok"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(parent);
}

pub(super) fn persist_locale(locale: &str) -> Result<(), String> {
    let path = config::config_dir().join("i18n.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut object = match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str::<Value>(&data)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
        Err(_) => Map::new(),
    };
    object.insert("locale".to_string(), Value::String(locale.to_string()));
    let value = Value::Object(object);
    let json = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ScanPathListKind {
    Custom,
    Excluded,
}

fn build_scan_paths_group(parent: &gtk::Widget) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&tr("setting.section.scan_paths"));
    group.set_description(Some(&tr("setting.section.scan_paths_description")));
    group.add_css_class("settings-preferences-group");

    add_scan_path_section(
        &group,
        parent,
        ScanPathListKind::Custom,
        &tr("setting.scan_paths.custom"),
        &tr("setting.scan_paths.custom_description"),
        prefs::custom_scan_roots(),
    );
    add_scan_path_section(
        &group,
        parent,
        ScanPathListKind::Excluded,
        &tr("setting.scan_paths.excluded"),
        &tr("setting.scan_paths.excluded_description"),
        prefs::excluded_scan_roots(),
    );

    group
}

fn add_scan_path_section(
    group: &adw::PreferencesGroup,
    parent: &gtk::Widget,
    kind: ScanPathListKind,
    title: &str,
    subtitle: &str,
    paths: Vec<PathBuf>,
) {
    let row = adw::ActionRow::new();
    row.add_css_class("settings-action-row");
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_activatable(false);

    let add_button = gtk::Button::new();
    add_button.set_icon_name("list-add-symbolic");
    add_button.set_valign(gtk::Align::Center);
    add_button.add_css_class("glass-toolbar-button");
    add_button.set_tooltip_text(Some(&tr("setting.scan_paths.add")));
    row.add_suffix(&add_button);
    group.add(&row);

    for path in paths {
        add_scan_path_value_row(group, parent, kind, path);
    }

    let parent_for_add = parent.clone();
    let group_for_add = group.clone();
    add_button.connect_clicked(move |_| {
        choose_scan_folder(&parent_for_add, {
            let parent = parent_for_add.clone();
            let group = group_for_add.clone();
            move |path| match append_scan_path(kind, path.clone()) {
                Ok(true) => {
                    add_scan_path_value_row(&group, &parent, kind, path);
                    show_restart_required_dialog(&parent);
                }
                Ok(false) => show_restart_required_dialog(&parent),
                Err(err) => show_settings_error_dialog(
                    &parent,
                    &trf("setting.scan_paths.save_failed", &[("error", &err)]),
                ),
            }
        });
    });
}

fn add_scan_path_value_row(
    group: &adw::PreferencesGroup,
    parent: &gtk::Widget,
    kind: ScanPathListKind,
    path: PathBuf,
) {
    let row = adw::ActionRow::new();
    row.add_css_class("settings-action-row");
    row.set_title(&path.to_string_lossy());
    row.set_activatable(false);

    let remove_button = gtk::Button::new();
    remove_button.set_icon_name("user-trash-symbolic");
    remove_button.set_valign(gtk::Align::Center);
    remove_button.add_css_class("glass-toolbar-button");
    remove_button.add_css_class("glass-toolbar-danger");
    remove_button.set_tooltip_text(Some(&tr("setting.scan_paths.remove")));
    row.add_suffix(&remove_button);
    group.add(&row);

    let parent_for_remove = parent.clone();
    let row_for_remove = row.clone();
    remove_button.connect_clicked(move |_| match remove_scan_path(kind, &path) {
        Ok(()) => {
            row_for_remove.set_visible(false);
            show_restart_required_dialog(&parent_for_remove);
        }
        Err(err) => show_settings_error_dialog(
            &parent_for_remove,
            &trf("setting.scan_paths.save_failed", &[("error", &err)]),
        ),
    });
}

fn choose_scan_folder<F>(parent: &gtk::Widget, on_selected: F)
where
    F: Fn(PathBuf) + 'static,
{
    let native = gtk::FileChooserNative::builder()
        .title(tr("setting.scan_paths.choose_folder"))
        .action(gtk::FileChooserAction::SelectFolder)
        .accept_label(tr("setting.scan_paths.choose"))
        .cancel_label(tr("button.cancel"))
        .build();
    if let Some(window) = parent.root().and_downcast::<gtk::Window>() {
        native.set_transient_for(Some(&window));
    }
    native.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                on_selected(path);
            }
        }
        dialog.destroy();
    });
    native.show();
}

fn append_scan_path(kind: ScanPathListKind, path: PathBuf) -> Result<bool, String> {
    let mut paths = scan_paths(kind);
    if paths.iter().any(|existing| existing == &path) {
        return Ok(false);
    }
    paths.push(path);
    set_scan_paths(kind, &paths)?;
    Ok(true)
}

pub(super) fn add_excluded_scan_path(path: PathBuf) -> Result<bool, String> {
    append_scan_path(ScanPathListKind::Excluded, path)
}

fn remove_scan_path(kind: ScanPathListKind, path: &PathBuf) -> Result<(), String> {
    let mut paths = scan_paths(kind);
    paths.retain(|existing| existing != path);
    set_scan_paths(kind, &paths)
}

fn scan_paths(kind: ScanPathListKind) -> Vec<PathBuf> {
    match kind {
        ScanPathListKind::Custom => prefs::custom_scan_roots(),
        ScanPathListKind::Excluded => prefs::excluded_scan_roots(),
    }
}

fn set_scan_paths(kind: ScanPathListKind, paths: &[PathBuf]) -> Result<(), String> {
    match kind {
        ScanPathListKind::Custom => prefs::set_custom_scan_roots(paths),
        ScanPathListKind::Excluded => prefs::set_excluded_scan_roots(paths),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RestartSpec {
    pub(super) program: PathBuf,
    pub(super) args: Vec<OsString>,
    pub(super) exit_current_process_after_spawn: bool,
}

fn restart_spec_from(program: PathBuf, args: Vec<OsString>) -> RestartSpec {
    RestartSpec {
        program,
        args,
        exit_current_process_after_spawn: true,
    }
}

fn current_restart_spec() -> Result<RestartSpec, String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let args = std::env::args_os().skip(1).collect();
    Ok(restart_spec_from(program, args))
}

fn restart_application() -> Result<(), String> {
    let spec = current_restart_spec()?;
    Command::new("sh")
        .arg("-c")
        .arg("sleep 0.2; exec \"$@\"")
        .arg("photo-viewer-restart")
        .arg(&spec.program)
        .args(&spec.args)
        .spawn()
        .map_err(|e| e.to_string())?;
    if spec.exit_current_process_after_spawn {
        let app = gtk::Application::default();
        for window in app.windows() {
            window.close();
        }
        app.quit();
        std::process::exit(0);
    }
    Ok(())
}

fn update_storage_size_async<F>(row: &adw::ActionRow, compute_size: F)
where
    F: FnOnce() -> u64 + Send + 'static,
{
    row.set_subtitle(&tr("setting.storage_usage_calculating"));

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(compute_size());
    });

    let row = row.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(size) => {
                if let Some(row) = row.upgrade() {
                    row.set_subtitle(&format_size(size));
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => {
                if row.upgrade().is_some() {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// Show a confirmation dialog for clearing cache/database.
fn show_clear_confirm_dialog<F: Fn() + 'static>(
    parent: &gtk::Widget,
    title: &str,
    body: &str,
    on_confirm: F,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(title)
        .body(body)
        .build();
    dialog.add_css_class("glass-alert-dialog");
    dialog.add_response("cancel", &tr("button.cancel"));
    dialog.add_response("confirm", &tr("dialog.confirm"));
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.connect_response(Some("confirm"), move |_, _| {
        on_confirm();
    });

    dialog.present(parent);
}

/// Show a success toast notification.
fn show_clear_success_toast(message: &str) {
    let app = gtk::Application::default();
    if let Some(window) = app.active_window() {
        if let Ok(_win) = window.downcast::<MainWindow>() {
            let notification = gtk::gio::Notification::new(&tr("setting.clear_success"));
            notification.set_body(Some(message));
            app.send_notification(None, &notification);
        }
    }
}

/// Show an error toast notification.
fn show_clear_error_toast(message: &str) {
    let app = gtk::Application::default();
    if let Some(window) = app.active_window() {
        if let Ok(_win) = window.downcast::<MainWindow>() {
            let notification = gtk::gio::Notification::new(&tr("setting.clear_failed"));
            notification.set_body(Some(message));
            app.send_notification(None, &notification);
        }
    }
}

/// Format bytes into human-readable size (KB, MB, GB).
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
pub(super) fn restart_spec_from_for_tests(program: PathBuf, args: Vec<OsString>) -> RestartSpec {
    restart_spec_from(program, args)
}

#[cfg(test)]
mod tests;
