use std::sync::Arc;

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use photo_viewer::core::section_model::GroupBy;
use photo_viewer::ui::SearchPage;

fn css_classes_vec<W: gtk::prelude::WidgetExt>(w: &W) -> Vec<String> {
    w.css_classes().iter().map(|s| s.to_string()).collect()
}

#[test]
fn search_page_has_dedicated_search_surface_and_split_result_areas() {
    gtk::init().expect("GTK init failed");

    let app = adw::Application::builder()
        .application_id("io.github.luyao_1024.photoviewer.SearchPage")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let tmp = tempfile::tempdir().unwrap();
    let pool = photo_viewer::core::db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));

    let page = SearchPage::new(pool, loader);
    let imp = page.imp();

    assert!(
        css_classes_vec(&imp.header_bar.get())
            .iter()
            .any(|class| class == "glass-header"),
        "search page header should use glass-header"
    );
    assert!(
        imp.search_entry.get().placeholder_text().is_some(),
        "search page should expose a filename-search entry"
    );
    assert!(
        imp.image_results_box
            .get()
            .has_css_class("search-results-section"),
        "image results should be a distinct section"
    );
    assert!(
        imp.video_results_box
            .get()
            .has_css_class("search-results-section"),
        "video results should be a distinct section"
    );
    assert!(
        !imp.image_results_box.get().vexpands(),
        "image results should size to their search content instead of claiming half the page"
    );
    assert!(
        !imp.video_results_box.get().vexpands(),
        "video results should size to their search content instead of claiming half the page"
    );

    let image_grid = imp
        .image_grid
        .borrow()
        .as_ref()
        .expect("search page should own an image result grid")
        .clone();
    let video_grid = imp
        .video_grid
        .borrow()
        .as_ref()
        .expect("search page should own a video result grid")
        .clone();
    assert_eq!(
        image_grid.mode(),
        GroupBy::Year,
        "search image thumbnails should use the compact year-view tile size"
    );
    assert_eq!(
        video_grid.mode(),
        GroupBy::Year,
        "search video thumbnails should use the compact year-view tile size"
    );
    assert!(
        !image_grid.vexpands(),
        "image grid should let its scroller size to result content"
    );
    assert!(
        !video_grid.vexpands(),
        "video grid should let its scroller size to result content"
    );
    assert!(
        image_grid.uses_flat_sections(),
        "search image previews should be flat so they can fill two rows across years"
    );
    assert!(
        video_grid.uses_flat_sections(),
        "search video previews should be flat so they can fill two rows across years"
    );
    assert_eq!(
        image_grid.hscrollbar_policy(),
        gtk::PolicyType::Never,
        "search image preview should wrap rows instead of scrolling horizontally"
    );
    assert_eq!(
        video_grid.hscrollbar_policy(),
        gtk::PolicyType::Never,
        "search video preview should wrap rows instead of scrolling horizontally"
    );
    assert_eq!(
        image_grid.vscrollbar_policy(),
        gtk::PolicyType::Never,
        "search image preview should show only fitted results and use more instead of inner scrolling"
    );
    assert_eq!(
        video_grid.vscrollbar_policy(),
        gtk::PolicyType::Never,
        "search video preview should show only fitted results and use more instead of inner scrolling"
    );
    assert!(
        SearchPage::preview_item_limit() > 0,
        "search preview should expose a finite tile capacity before showing more"
    );
    assert_eq!(
        SearchPage::preview_capacity_for_width(980),
        20,
        "search preview should fill two complete year-view rows for the available width"
    );
    assert_eq!(
        SearchPage::preview_capacity_for_area(980, 680, 2),
        30,
        "search preview should use available section height to show more than two rows when they fit"
    );
    assert_eq!(
        SearchPage::preview_capacity_for_width(0),
        SearchPage::preview_item_limit(),
        "search preview should fall back to a stable two-row capacity before layout is allocated"
    );
    assert!(
        !imp.image_more_btn
            .borrow()
            .as_ref()
            .expect("image results should own a more button")
            .is_visible(),
        "image more button should stay hidden until preview results overflow"
    );
    assert!(
        !imp.video_more_btn
            .borrow()
            .as_ref()
            .expect("video results should own a more button")
            .is_visible(),
        "video more button should stay hidden until preview results overflow"
    );
}
