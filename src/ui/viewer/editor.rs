use super::ViewerPage;
use crate::core::i18n::tr;
use crate::ui::editor_panel::{CropOverlayUpdate, SaveResultKind, ToastKind};
use crate::ui::toasts;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt, NavigationPageExt};

impl ViewerPage {
    /// Wire the Edit button: configure the embedded `EditorPanel` for the
    /// current item and reveal it as a right-side overlay (same pattern as
    /// the details panel), instead of pushing a separate `NavigationPage`.
    pub(super) fn setup_edit_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.edit_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let pool = match this.imp().pool.borrow().as_ref() {
                Some(p) => p.clone(),
                None => {
                    tracing::warn!("ViewerPage: Edit pressed but pool not set");
                    return;
                }
            };
            let item = match this.current_media_item() {
                Some(i) => i,
                None => return,
            };
            if item.is_video() {
                return;
            }

            // Close details panel if open — only one side panel at a time.
            if this.imp().details_split_view.get().shows_sidebar() {
                this.set_details_revealed(false, "edit_start");
            }

            // Save the original texture so we can restore on cancel.
            *this.imp().original_texture.borrow_mut() = this
                .imp()
                .picture
                .get()
                .paintable()
                .and_then(|p| p.downcast::<gdk::Texture>().ok());

            // Configure and reveal the editor panel.
            this.imp().editor_panel.get().configure(item, pool);
            this.start_editing();
        });
    }

    /// Reveal the editor side-panel and lock navigation gestures.
    pub(super) fn start_editing(&self) {
        self.reset_viewer_transform();
        self.imp().is_editing.set(true);
        self.set_overlay_navigation_visible(false);
        self.set_zoom_controls_visible(false);
        self.imp().motion_play_btn.get().set_visible(false);
        self.set_editor_sidebar_child_visible(true);
        self.imp().editor_split_view.get().set_show_sidebar(true);
        self.set_can_pop(false);
    }

    /// Hide the editor side-panel, restore the original image, and
    /// re-enable navigation gestures.
    pub(super) fn stop_editing(&self) {
        let imp = self.imp();
        imp.editor_panel.get().cancel_preview();
        imp.is_editing.set(false);
        self.set_overlay_navigation_visible(true);
        self.set_zoom_controls_visible(imp.picture.get().is_visible());
        if let Some(item) = self.current_media_item() {
            self.set_motion_play_button_for_item(&item);
        }
        imp.editor_split_view.get().set_show_sidebar(false);
        self.set_crop_overlay(CropOverlayUpdate {
            active: false,
            rect: None,
            image_dimensions: (0, 0),
        });

        // Restore the original texture (cancel case).
        if let Some(tex) = imp.original_texture.borrow().clone() {
            imp.picture.get().set_paintable(Some(&tex));
        }
        *imp.original_texture.borrow_mut() = None;

        // Re-enable pop after the slide-out animation.
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            if let Some(this) = weak.upgrade() {
                if !this.imp().is_editing.get()
                    && !this.imp().editor_split_view.get().shows_sidebar()
                {
                    this.set_editor_sidebar_child_visible(false);
                    this.set_can_pop(true);
                }
            }
        });
    }

    /// Connect EditorPanel callbacks to ViewerPage state (picture, spinner,
    /// toast overlay). Called once during construction.
    pub(super) fn setup_editor_callbacks(&self) {
        let panel = self.imp().editor_panel.get();

        // Preview texture → update the viewer's picture.
        let weak = self.downgrade();
        panel.connect_texture_ready(move |texture| {
            if let Some(this) = weak.upgrade() {
                this.imp().picture.get().set_paintable(Some(&texture));
                this.imp().crop_overlay.get().queue_draw();
            }
        });

        // Spinner visibility.
        let weak = self.downgrade();
        panel.connect_spinner(move |visible| {
            if let Some(this) = weak.upgrade() {
                this.set_spinner_visible(visible);
            }
        });

        // Close (cancel or save-complete) → hide panel.
        let weak = self.downgrade();
        panel.connect_close(move || {
            if let Some(this) = weak.upgrade() {
                this.stop_editing();
            }
        });

        let weak = self.downgrade();
        panel.connect_save_result(move |kind, heading: String, body: String| {
            if let Some(this) = weak.upgrade() {
                if save_result_closes_editor(kind) {
                    this.stop_editing();
                }
                this.present_save_result_dialog(&heading, &body);
            }
        });

        // Toast messages.
        let weak = self.downgrade();
        panel.connect_toast(move |msg, kind| {
            if let Some(this) = weak.upgrade() {
                match kind {
                    ToastKind::Success => toasts::success(&this.imp().toast_overlay.get(), msg),
                    ToastKind::Error => toasts::error(&this.imp().toast_overlay.get(), msg),
                }
            }
        });

        let weak = self.downgrade();
        panel.connect_crop_overlay(move |update| {
            if let Some(this) = weak.upgrade() {
                this.set_crop_overlay(update);
            }
        });
    }

    fn present_save_result_dialog(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("ok", &tr("button.ok"));
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        dialog.present(self);
    }
}

fn save_result_closes_editor(kind: SaveResultKind) -> bool {
    kind == SaveResultKind::Success
}

#[cfg(test)]
#[path = "editor/tests.rs"]
mod tests;
