//! Closing cloud sync must leave the real settings surface responsive.
mod common;
use adw::prelude::*;
use common::{
    interaction::{descendants, Ui},
    shell::Shell,
};
use gtk::{glib, subclass::prelude::ObjectSubclassIsExt};
use gtk4 as gtk;
use libadwaita as adw;
use photo_viewer::core::{i18n::tr, prefs};
use std::time::Duration;

fn named<T: IsA<gtk::Widget> + glib::object::IsClass + Clone + glib::object::ObjectType>(
    root: &impl IsA<gtk::Widget>,
    name: &str,
) -> T {
    descendants::<T>(root)
        .into_iter()
        .find(|w| w.as_ref().widget_name() == name)
        .unwrap_or_else(|| panic!("missing widget {name}"))
}
fn respond(shell: &Shell, key: &str) {
    let text = tr(key);
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(3), || descendants::<gtk::Button>(
            &shell.window
        )
        .iter()
        .any(|b| b.is_mapped() && b.label().as_deref() == Some(text.as_str()))));
    let button = descendants::<gtk::Button>(&shell.window)
        .into_iter()
        .find(|b| b.is_mapped() && b.label().as_deref() == Some(text.as_str()))
        .unwrap();
    Ui::for_widget(&button).click_native(&button, &text);
}
#[test]
fn disable_can_be_cancelled_and_confirmed_without_blocking_settings() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _runtime = runtime.enter();
    common::credentials::install();
    let shell = Shell::new();
    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Open Settings");
    let category = named::<gtk::ListBoxRow>(&shell.window, "settings-category-sync");
    Ui::for_widget(&category).click_native(&category, "Cloud sync category");
    let enabled = named::<gtk::Switch>(&shell.window, "native-sync-enabled");
    for pass in 0..2 {
        if pass > 0 {
            Ui::for_widget(&category).click_native(&category, "Return to cloud sync category");
        }
        Ui::for_widget(&enabled).scroll_to_reveal(&enabled, "Cloud sync switch");
        Ui::for_widget(&enabled).click_native(&enabled, "Enable cloud sync");
        assert!(shell
            .ui
            .wait_until(Duration::from_secs(3), prefs::sync_enabled));
        let settings = photo_viewer::config::config_dir().join("settings.json");
        let before = std::fs::metadata(&settings).unwrap().modified().unwrap();
        // Move away so xdotool --sync observes real motion on the next press.
        shell
            .ui
            .pointer_motion(40.0, 40.0, "Leave cloud sync switch");
        // Exhaust the pool while the Settings refresh is due. A synchronous
        // snapshot on GTK blocks the close press; a background snapshot must
        // leave both the confirmation and its cancel button usable.
        let release = if pass == 0 {
            let connections = (0..shell.pool.max_size())
                .map(|_| shell.pool.get().unwrap())
                .collect::<Vec<_>>();
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(5));
                drop(connections);
            }))
        } else {
            None
        };
        let refresh_started = std::time::Instant::now();
        shell.ui.pump(Duration::from_millis(2250));
        assert!(
            refresh_started.elapsed() < Duration::from_secs(3),
            "Settings blocked GTK while waiting for its database snapshot: {:?}",
            refresh_started.elapsed()
        );
        if pass == 0 {
            assert!(
                !shell.photos.imp().overview_refresh_in_flight.get(),
                "idle Photos must not poll sync counts while the pool is busy"
            );
            // Deliver the same invalidation the production SyncStateDirty
            // consumer uses, then drive the close controls with real input.
            shell.window.refresh_sync_state();
            shell.ui.pump(Duration::from_millis(50));
            assert!(
                shell.photos.imp().overview_refresh_in_flight.get(),
                "a sync event should request its overview in the background"
            );
        }
        let pressed_at = std::time::Instant::now();
        Ui::for_widget(&enabled).click_native(&enabled, "Request disabling cloud sync");
        assert!(
            pressed_at.elapsed() < Duration::from_secs(2),
            "opening disable confirmation blocked the GTK input path for {:?}",
            pressed_at.elapsed()
        );
        assert_eq!(
            std::fs::metadata(&settings).unwrap().modified().unwrap(),
            before,
            "opening disable confirmation must not save preferences or restart synchronization"
        );
        respond(&shell, "button.cancel");
        assert!(prefs::sync_enabled());
        assert!(enabled.is_active());
        assert_eq!(
            descendants::<adw::AlertDialog>(&shell.window)
                .iter()
                .filter(|dialog| dialog.is_mapped())
                .count(),
            0
        );
        assert_eq!(
            std::fs::metadata(&settings).unwrap().modified().unwrap(),
            before,
            "cancelling disable must not save preferences or restart synchronization"
        );
        if let Some(release) = release {
            release.join().unwrap();
        }
        Ui::for_widget(&enabled).click_native(&enabled, "Request disabling again");
        respond(&shell, "native.disable");
        assert!(shell
            .ui
            .wait_until(Duration::from_secs(3), || !prefs::sync_enabled()));
        assert!(!enabled.is_active());
        let store = photo_viewer::core::sync::SyncStore::new(shell.pool.clone());
        assert!(shell
            .ui
            .wait_until(Duration::from_secs(3), || !store.reset_pending().unwrap()));
        let general = named::<gtk::ListBoxRow>(&shell.window, "settings-category-general");
        Ui::for_widget(&general).click_native(&general, "Navigate after disabling");
        let stack = named::<gtk::Stack>(&shell.window, "settings-category-stack");
        assert!(shell.ui.wait_until(Duration::from_secs(3), || stack
            .visible_child_name()
            .as_deref()
            == Some("general")));
    }
}
