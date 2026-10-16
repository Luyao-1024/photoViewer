//! Focused settings journeys through the real shell, pointer path and rendered text.
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
        .find(|widget| widget.as_ref().widget_name() == name)
        .unwrap_or_else(|| panic!("missing widget {name}"))
}
fn category(shell: &Shell, name: &str) {
    let row = named::<gtk::ListBoxRow>(&shell.window, &format!("settings-category-{name}"));
    Ui::for_widget(&row).click(&row, &format!("Settings category {name}"));
    let stack = named::<gtk::Stack>(&shell.window, "settings-category-stack");
    assert!(shell.ui.wait_until(Duration::from_secs(3), || stack
        .visible_child_name()
        .as_deref()
        == Some(name)));
    assert!(
        shell
            .window
            .imp()
            .settings_dialog
            .borrow()
            .as_ref()
            .is_some_and(|dialog| dialog.is_visible()),
        "a category press dismissed Settings"
    );
}
fn rendered_title_hierarchy(dialog: &adw::Dialog, category_key: &str) {
    let stack = named::<gtk::Stack>(dialog, "settings-category-stack");
    let content = stack.visible_child().unwrap();
    let title = descendants::<gtk::Label>(dialog)
        .into_iter()
        .find(|label| label.is_mapped() && label.label().as_str() == tr("setting.page.title"))
        .expect("the visible Settings window title");
    let heading = descendants::<gtk::Label>(&content)
        .into_iter()
        .find(|label| label.is_mapped() && label.label().as_str() == tr(category_key))
        .expect("the visible category heading");
    let title_height = title.layout().pixel_size().1;
    let heading_height = heading.layout().pixel_size().1;
    assert!(title_height > heading_height,
        "Settings title must render larger than the category: Settings={title_height}px, category={heading_height}px");
}

#[test]
fn settings_titles_inline_management_and_shortcut_return_use_real_controls() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _runtime = runtime.enter();
    common::credentials::install();
    let shell = Shell::new();
    shell
        .ui
        .click(&shell.window.imp().settings_button.get(), "Open Settings");
    let dialog = shell
        .window
        .imp()
        .settings_dialog
        .borrow()
        .as_ref()
        .unwrap()
        .clone();
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(3), || dialog.is_mapped()));

    for pass in 0..2 {
        if pass == 1 {
            // Switch material through the actual preference, not fixture state.
            category(&shell, "appearance");
            let row = descendants::<adw::ActionRow>(&dialog)
                .into_iter()
                .find(|row| row.title().as_str() == tr("setting.liquid_glass"))
                .unwrap();
            let toggle = descendants::<gtk::Switch>(&row).into_iter().next().unwrap();
            let expected = !toggle.is_active();
            Ui::for_widget(&toggle).click_native(&toggle, "Switch Settings material");
            assert!(shell
                .ui
                .wait_until(Duration::from_secs(3), || prefs::liquid_glass_enabled()
                    == expected));
        }
        for (name, key) in [
            ("general", "setting.category.general"),
            ("appearance", "setting.section.appearance"),
            ("media", "setting.category.media"),
            ("sync", "native.settings"),
            ("storage", "setting.category.storage"),
            ("about", "setting.category.about"),
        ] {
            category(&shell, name);
            rendered_title_hierarchy(&dialog, key);
            let navigation = named::<adw::NavigationView>(&dialog, "settings-navigation");
            assert_eq!(
                navigation.visible_page(),
                navigation
                    .navigation_stack()
                    .item(0)
                    .and_downcast::<adw::NavigationPage>(),
                "a category added another navigation page"
            );
            if name == "sync" {
                let content = named::<gtk::Box>(&dialog, "native-sync-content");
                assert!(content.is_mapped());
                let add = named::<gtk::Button>(&content, "native-sync-add");
                Ui::for_widget(&add).assert_reachable(&add, "Inline Add Server button");
            } else if name == "storage" {
                assert!(named::<gtk::Box>(&dialog, "settings-cleanup").is_mapped());
                let limit = named::<adw::SpinRow>(&dialog, "settings-log-limit");
                assert!(limit.is_mapped());
                for name in ["library", "logs", "accounts", "all"] {
                    assert!(
                        named::<adw::ActionRow>(&dialog, &format!("settings-cleanup-{name}"))
                            .is_mapped()
                    );
                }
            }
        }
        category(&shell, "general");
        let stack = named::<gtk::Stack>(&dialog, "settings-category-stack");
        let scroll = stack
            .visible_child()
            .unwrap()
            .downcast::<gtk::ScrolledWindow>()
            .unwrap();
        let before_scroll = scroll.vadjustment().value();
        let row = descendants::<adw::ActionRow>(&dialog)
            .into_iter()
            .find(|row| row.title().as_str() == tr("setting.keyboard.reference"))
            .unwrap();
        Ui::for_widget(&row).click(&row, "Open shortcut reference from Settings");
        let mut reference = None;
        assert!(shell.ui.wait_until(Duration::from_secs(3), || {
            reference = gtk::Window::list_toplevels()
                .into_iter()
                .find_map(|window| {
                    window
                        .downcast::<gtk::ShortcutsWindow>()
                        .ok()
                        .filter(|window| window.is_visible())
                });
            reference.is_some()
        }));
        let reference = reference.unwrap();
        assert!(
            dialog.is_visible(),
            "opening shortcuts closed the original Settings dialog"
        );
        let close = descendants::<gtk::Button>(&reference)
            .into_iter()
            .find(|button| button.is_mapped() && button.has_css_class("close"))
            .expect("the shortcut window's real close button");
        Ui::for_widget(&close).click(&close, "Close shortcut reference");
        assert!(shell
            .ui
            .wait_until(Duration::from_secs(3), || !reference.is_visible()));
        assert_eq!(
            shell.window.imp().settings_dialog.borrow().as_ref(),
            Some(&dialog)
        );
        assert!(dialog.is_mapped());
        assert_eq!(stack.visible_child_name().as_deref(), Some("general"));
        assert!((scroll.vadjustment().value() - before_scroll).abs() < 1.0);
        assert!(shell
            .window
            .nav_view()
            .has_css_class("settings-background-blur"));
    }
}
