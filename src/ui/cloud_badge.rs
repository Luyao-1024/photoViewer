use crate::core::sync::CloudState;

pub(crate) fn resource(state: CloudState, dark_background: bool) -> &'static str {
    match (state, dark_background) {
        (CloudState::Synced, true) => {
            "/io/github/luyao_1024/photoviewer/icons/gnome-cloud-white.png"
        }
        (CloudState::Off, true) => {
            "/io/github/luyao_1024/photoviewer/icons/gnome-cloud-off-white.png"
        }
        (CloudState::Synced, false) => {
            "/io/github/luyao_1024/photoviewer/icons/gnome-cloud-dark.png"
        }
        (CloudState::Off, false) => {
            "/io/github/luyao_1024/photoviewer/icons/gnome-cloud-off-dark.png"
        }
    }
}

#[cfg(test)]
mod tests;
