use crate::core::db::DbPool;
use crate::core::error::Result;
use gtk4::glib;
use std::cell::Cell;
use std::rc::Rc;

type AlbumRefreshJob = Rc<dyn Fn() -> Result<()>>;

#[derive(Clone)]
pub struct RefreshCoordinator {
    album_refresh_running: Rc<Cell<bool>>,
    album_refresh_pending: Rc<Cell<bool>>,
    album_job: AlbumRefreshJob,
    album_pool: Option<DbPool>,
    on_albums_refreshed: Option<Rc<dyn Fn()>>,
}

impl RefreshCoordinator {
    pub fn new(pool: DbPool, on_albums_refreshed: Rc<dyn Fn()>) -> Self {
        let pool_for_sync = pool.clone();
        let callback_for_sync = on_albums_refreshed.clone();
        Self {
            album_refresh_running: Rc::new(Cell::new(false)),
            album_refresh_pending: Rc::new(Cell::new(false)),
            album_job: Rc::new(move || {
                crate::core::albums::refresh(&pool_for_sync)?;
                callback_for_sync();
                Ok(())
            }),
            album_pool: Some(pool),
            on_albums_refreshed: Some(on_albums_refreshed),
        }
    }

    pub fn new_for_tests<F>(album_job: F) -> Self
    where
        F: Fn() -> Result<()> + 'static,
    {
        Self {
            album_refresh_running: Rc::new(Cell::new(false)),
            album_refresh_pending: Rc::new(Cell::new(false)),
            album_job: Rc::new(album_job),
            album_pool: None,
            on_albums_refreshed: None,
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

    pub fn mark_albums_dirty_async(&self) {
        if self.album_refresh_running.get() {
            self.album_refresh_pending.set(true);
            return;
        }
        let Some(pool) = self.album_pool.clone() else {
            self.mark_albums_dirty();
            return;
        };
        let Some(on_albums_refreshed) = self.on_albums_refreshed.clone() else {
            self.mark_albums_dirty();
            return;
        };

        self.album_refresh_running.set(true);
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let result =
                gtk4::gio::spawn_blocking(move || crate::core::albums::refresh(&pool)).await;
            match result {
                Ok(Ok(())) => on_albums_refreshed(),
                Ok(Err(err)) => tracing::warn!("album refresh failed: {err}"),
                Err(err) => tracing::warn!("album refresh join failed: {err:?}"),
            }
            this.album_refresh_running.set(false);
            if this.album_refresh_pending.replace(false) {
                this.mark_albums_dirty_async();
            }
        });
    }
}
