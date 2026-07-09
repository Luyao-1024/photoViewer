use crate::core::db::DbPool;
use crate::core::error::Result;
use gtk4::glib;
use std::cell::Cell;
use std::rc::Rc;

type AlbumRefreshJob = Rc<dyn Fn() -> Result<()>>;

#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryStats {
    pub live_total: usize,
    pub thumbnails_generated: usize,
}

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
        if self.album_refresh_running.get() {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "SIDEBAR_TRACE album_refresh_mark_async action=queue running=true pending_before={}",
                self.album_refresh_pending.get()
            );
            self.album_refresh_pending.set(true);
            return;
        }
        let Some(pool) = self.album_pool.clone() else {
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
            let result =
                gtk4::gio::spawn_blocking(move || crate::core::albums::refresh(&pool)).await;
            match result {
                Ok(Ok(())) => {
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
                    "SIDEBAR_TRACE album_refresh_worker_done pending=true rerun"
                );
                this.mark_albums_dirty_async();
            } else {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE album_refresh_worker_done pending=false"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sidebar_refresh_trace_logs_stay_debug() {
        let source = include_str!("refresh.rs");
        let production_source = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("refresh.rs must contain production code");

        let mut search_from = 0;
        while let Some(relative_index) = production_source[search_from..].find("SIDEBAR_TRACE") {
            let message_index = search_from + relative_index;
            let before = &production_source[..message_index];
            let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
                .iter()
                .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
                .max_by_key(|(index, _)| *index)
                .map(|(_, candidate)| candidate)
                .expect("SIDEBAR_TRACE message should be inside a tracing macro");
            assert_eq!(
                actual_macro, "tracing::debug!(",
                "SIDEBAR_TRACE refresh diagnostics should stay out of default INFO logs"
            );
            search_from = message_index + "SIDEBAR_TRACE".len();
        }
    }
}
