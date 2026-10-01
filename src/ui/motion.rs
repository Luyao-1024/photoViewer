//! The desktop's "reduce animations" preference, answered in one place.
//!
//! GNOME's Accessibility panel drives `GtkSettings:gtk-enable-animations`.
//! Nothing else in this app read it, so every transition we author played
//! anyway. Two channels carry the preference: the CSS provider (rebuilt with a
//! tail block, see [`REDUCED_MOTION_CSS`]) and the widget-level transitions that
//! Blueprint templates own and CSS cannot reach, see [`apply_to`].

use gtk4 as gtk;
use gtk4::prelude::*;

/// Appended after every other CSS block while motion is reduced.
///
/// GTK's CSS subset has no `@media` feature queries - and `grid_css.rs` keeps
/// them out of the sheets on purpose - so the preference is applied by
/// rebuilding the provider with this tail rather than by a conditional style.
/// Only the duration is touched: the authored `transition-property` lists stay
/// as they are, so nothing changes *which* properties animate, only that they
/// arrive immediately.
pub const REDUCED_MOTION_CSS: &str = "\n/* reduce-motion: GtkSettings:gtk-enable-animations is off */\n* {\n  transition-duration: 0ms;\n}\n";

/// Whether the desktop currently wants animations. GTK's own default is true,
/// so a session that never set the property behaves as before.
pub fn enabled() -> bool {
    // No settings backend (headless, or GTK without an XDG settings portal)
    // means no opinion, and GTK's own default is to animate.
    gtk::Settings::default()
        .map(|settings| settings.is_gtk_enable_animations())
        .unwrap_or(true)
}

/// Drop template-authored motion from `root`'s subtree.
///
/// `GtkRevealer` and `GtkStack` take their transition from the Blueprint
/// template and expose no CSS hook, so the preference has to be written back
/// onto the widgets. It is applied when a page is built and again whenever the
/// desktop turns animations off. Turning them back on does not restore a page
/// that is already on screen: that would mean remembering every widget's
/// authored value, so the preference is picked up when the page is next built
/// instead. CSS transitions and the filmstrip's frame-clock scroll are live in
/// both directions.
pub fn apply_to(root: &impl IsA<gtk::Widget>) {
    if enabled() {
        return;
    }
    strip_motion(root.as_ref());
}

fn strip_motion(widget: &gtk::Widget) {
    if let Some(revealer) = widget.downcast_ref::<gtk::Revealer>() {
        revealer.set_transition_type(gtk::RevealerTransitionType::None);
    }
    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_transition_duration(0);
    }
    let mut child = widget.first_child();
    while let Some(node) = child {
        strip_motion(&node);
        child = node.next_sibling();
    }
}

#[cfg(test)]
mod tests;
