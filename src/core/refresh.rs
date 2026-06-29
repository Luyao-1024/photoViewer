use crate::core::error::Result;
use std::cell::Cell;
use std::rc::Rc;

type AlbumRefreshJob = Rc<dyn Fn() -> Result<()>>;

#[derive(Clone)]
pub struct RefreshCoordinator {
    album_refresh_running: Rc<Cell<bool>>,
    album_refresh_pending: Rc<Cell<bool>>,
    album_job: AlbumRefreshJob,
}

impl RefreshCoordinator {
    pub fn new_for_tests<F>(album_job: F) -> Self
    where
        F: Fn() -> Result<()> + 'static,
    {
        Self {
            album_refresh_running: Rc::new(Cell::new(false)),
            album_refresh_pending: Rc::new(Cell::new(false)),
            album_job: Rc::new(album_job),
        }
    }

    pub fn mark_albums_dirty(&self) -> bool {
        if self.album_refresh_running.get() {
            self.album_refresh_pending.set(true);
            return false;
        }
        self.album_refresh_running.set(true);
        if let Err(err) = (self.album_job)() {
            tracing::warn!("album refresh failed: {err}");
        }
        true
    }

    pub fn finish_album_refresh_for_tests(&self) -> Result<()> {
        self.album_refresh_running.set(false);
        if self.album_refresh_pending.replace(false) {
            self.mark_albums_dirty();
        }
        Ok(())
    }
}
