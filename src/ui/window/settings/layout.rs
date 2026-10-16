//! Fedora-style category navigation: a stable sidebar and independently scrolling pages.
use super::*;
pub(super) struct Layout {
    pub root: gtk::Box,
    pub general: gtk::Box,
    pub appearance: gtk::Box,
    pub media: gtk::Box,
    pub sync: gtk::Box,
    pub storage: gtk::Box,
    pub about: gtk::Box,
}
impl Layout {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_widget_name("settings-category-layout");
        let sidebar = gtk::ListBox::new();
        sidebar.set_widget_name("settings-category-sidebar");
        sidebar.add_css_class("navigation-sidebar");
        sidebar.add_css_class("settings-category-sidebar");
        sidebar.set_width_request(180);
        sidebar.set_selection_mode(gtk::SelectionMode::Single);
        sidebar.set_activate_on_single_click(true);
        root.append(&sidebar);
        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        root.append(&separator);
        let stack = gtk::Stack::new();
        stack.set_widget_name("settings-category-stack");
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        root.append(&stack);
        let mut pages = Vec::new();
        for (name, key, icon) in [
            (
                "general",
                "setting.category.general",
                "preferences-system-symbolic",
            ),
            (
                "appearance",
                "setting.section.appearance",
                "applications-graphics-symbolic",
            ),
            (
                "media",
                "setting.category.media",
                "folder-pictures-symbolic",
            ),
            ("sync", "native.settings", "folder-remote-symbolic"),
            (
                "storage",
                "setting.category.storage",
                "drive-harddisk-symbolic",
            ),
            ("about", "setting.category.about", "help-about-symbolic"),
        ] {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&format!("settings-category-{name}"));
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let image = gtk::Image::from_icon_name(icon);
            image.set_accessible_role(gtk::AccessibleRole::Presentation);
            line.append(&image);
            let label = gtk::Label::new(Some(&tr(key)));
            label.set_xalign(0.0);
            label.set_wrap(true);
            line.append(&label);
            row.set_child(Some(&line));
            sidebar.append(&row);
            let page = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(18)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(24)
                .margin_end(24)
                .build();
            page.add_css_class("settings-dialog-content");
            let heading = gtk::Label::new(Some(&tr(key)));
            heading.add_css_class("heading");
            heading.set_margin_bottom(6);
            page.append(&heading);
            let clamp = adw::Clamp::builder()
                .maximum_size(760)
                .tightening_threshold(560)
                .child(&page)
                .build();
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .child(&clamp)
                .build();
            stack.add_named(&scroll, Some(name));
            pages.push(page);
        }
        let weak_stack = stack.downgrade();
        sidebar.connect_row_selected(move |_, row| {
            if let (Some(stack), Some(row)) = (weak_stack.upgrade(), row) {
                let name = row.widget_name();
                if let Some(name) = name.strip_prefix("settings-category-") {
                    stack.set_visible_child_name(name);
                }
            }
        });
        // Initial category is an application default, not a test shortcut.
        sidebar.select_row(sidebar.row_at_index(0).as_ref());
        let mut pages = pages.into_iter();
        Self {
            root,
            general: pages.next().unwrap(),
            appearance: pages.next().unwrap(),
            media: pages.next().unwrap(),
            sync: pages.next().unwrap(),
            storage: pages.next().unwrap(),
            about: pages.next().unwrap(),
        }
    }
}
