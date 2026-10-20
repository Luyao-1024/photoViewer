use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::error::Result;
use gtk4::glib;
use std::cell::Cell;
use std::rc::Rc;

type AlbumRefreshJob = Rc<dyn Fn() -> Result<()>>;

/// A download admits photos faster than an album materialization completes.
/// Space the next rebuild out so the derived refresh cannot occupy the writer
/// back-to-back for the whole transfer; the trailing dirty mark still runs.
const ALBUM_REFRESH_RERUN_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryStats {
    pub live_total: usize,
    pub thumbnails_generated: usize,
}

#[derive(Clone)]
pub struct RefreshCoordinator {
    album_refresh_running: Rc<Cell<bool>>,
    album_refresh_pending: Rc<Cell<bool>>,
    album_refresh_cooldown: Rc<Cell<bool>>,
    album_job: AlbumRefreshJob,
    album_actor: Option<DbActorHandle>,
    on_albums_refreshed: Option<Rc<dyn Fn()>>,
}

impl RefreshCoordinator {
    pub fn new(db_actor: DbActorHandle, on_albums_refreshed: Rc<dyn Fn()>) -> Self {
        let actor_for_sync = db_actor.clone();
        let callback_for_sync = on_albums_refreshed.clone();
        Self {
            album_refresh_running: Rc::new(Cell::new(false)),
            album_refresh_pending: Rc::new(Cell::new(false)),
            album_refresh_cooldown: Rc::new(Cell::new(false)),
            album_job: Rc::new(move || {
                actor_for_sync.execute_blocking(DbCommand::RefreshAlbumsInternal)?;
                callback_for_sync();
                Ok(())
            }),
            album_actor: Some(db_actor),
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
            album_refresh_cooldown: Rc::new(Cell::new(false)),
            album_job: Rc::new(album_job),
            album_actor: None,
            on_albums_refreshed: None,
        }
    }

    pub fn mark_albums_dirty(&self) -> bool {
        if self.album_refresh_running.get() {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_mark_sync action=queue running=true pending_before={}",
                self.album_refresh_pending.get()
            );
            self.album_refresh_pending.set(true);
            return false;
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE album_refresh_mark_sync action=start"
        );
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
        if self.album_refresh_running.get() || self.album_refresh_cooldown.get() {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_mark_async action=queue running={} cooling={} pending_before={}",
                self.album_refresh_running.get(),
                self.album_refresh_cooldown.get(),
                self.album_refresh_pending.get()
            );
            self.album_refresh_pending.set(true);
            return;
        }
        let Some(actor) = self.album_actor.clone() else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_mark_async fallback=sync_no_pool"
            );
            self.mark_albums_dirty();
            return;
        };
        let Some(on_albums_refreshed) = self.on_albums_refreshed.clone() else {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_mark_async fallback=sync_no_callback"
            );
            self.mark_albums_dirty();
            return;
        };
        self.start_album_refresh_async(actor, on_albums_refreshed);
    }

    fn start_album_refresh_async(&self, actor: DbActorHandle, on_albums_refreshed: Rc<dyn Fn()>) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE album_refresh_mark_async action=start"
        );
        self.album_refresh_running.set(true);
        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_worker_start"
            );
            let result = gtk4::gio::spawn_blocking(move || {
                actor.execute_blocking(DbCommand::RefreshAlbumsInternal)
            })
            .await;
            match result {
                Ok(Ok(_)) => {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE album_refresh_worker_ok invoking_callback"
                    );
                    on_albums_refreshed();
                }
                Ok(Err(err)) => tracing::warn!("album refresh failed: {err}"),
                Err(err) => tracing::warn!("album refresh join failed: {err:?}"),
            }
            this.album_refresh_running.set(false);
            if this.album_refresh_pending.replace(false) {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE album_refresh_worker_done pending=true cooling"
                );
                this.schedule_album_refresh_rerun();
            } else {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE album_refresh_worker_done pending=false"
                );
            }
        });
    }

    fn schedule_album_refresh_rerun(&self) {
        self.album_refresh_cooldown.set(true);
        let this = self.clone();
        glib::timeout_add_local_once(ALBUM_REFRESH_RERUN_DELAY, move || {
            this.album_refresh_cooldown.set(false);
            this.album_refresh_pending.set(false);
            this.mark_albums_dirty_async();
        });
    }
}

#[cfg(test)]
mod tests;
