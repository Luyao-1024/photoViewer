use super::super::test_support::*;
use super::super::*;
use super::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[gtk::test]
fn viewer_keyboard_action_navigates_and_closes() {
    init_viewer_test();
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_media_item()));
    let viewer = ViewerPage::new(media_list, 0);

    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_cb = events.clone();
    viewer.connect_navigation(move |delta| {
        events_for_cb.borrow_mut().push(delta);
    });

    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerNext),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::ViewerPrevious),
        crate::ui::keyboard::KeyboardResult::Handled
    );
    assert_eq!(
        viewer.handle_keyboard_action(crate::ui::keyboard::KeyboardAction::CancelOrClose),
        crate::ui::keyboard::KeyboardResult::Handled
    );

    assert_eq!(events.borrow().as_slice(), &[1, -1, NAV_POP]);
}

#[test]
fn next_index_after_deleted_item_stays_in_bounds() {
    assert_eq!(next_index_after_deleted_item(0, 2), Some(0));
    assert_eq!(next_index_after_deleted_item(1, 2), Some(1));
    assert_eq!(next_index_after_deleted_item(2, 2), Some(1));
    assert_eq!(next_index_after_deleted_item(0, 0), None);
}

#[gtk::test]
fn find_media_index_by_id_uses_item_identity() {
    let _ = gtk::init();
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut first = sample_media_item();
    first.id = 10;
    let mut second = sample_media_item();
    second.id = 20;
    list.append(&glib::BoxedAnyObject::new(first));
    list.append(&glib::BoxedAnyObject::new(second));

    assert_eq!(find_media_index_by_id(&list, 20), Some(1));
    assert_eq!(find_media_index_by_id(&list, 30), None);
}

#[gtk::test]
fn current_media_item_stays_anchored_when_startup_scan_inserts_before_it() {
    init_viewer_test();
    let list = gio::ListStore::new::<glib::BoxedAnyObject>();
    let mut opened = sample_media_item();
    opened.id = 20;
    opened.uri = "file:///tmp/opened.jpg".into();
    opened.path = PathBuf::from("/tmp/opened.jpg");
    list.append(&glib::BoxedAnyObject::new(opened));

    let viewer = ViewerPage::new_for_query(MediaQuery::LiveAll, MediaId::from(20), list.clone());

    let mut inserted = sample_media_item();
    inserted.id = 10;
    inserted.uri = "file:///tmp/inserted.jpg".into();
    inserted.path = PathBuf::from("/tmp/inserted.jpg");
    list.insert(0, &glib::BoxedAnyObject::new(inserted));

    let current = viewer
        .current_media_item()
        .expect("viewer should still resolve the opened item");
    assert_eq!(current.id, 20);
    assert_eq!(viewer.current_index(), 1);
}
