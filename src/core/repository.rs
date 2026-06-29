use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaQuery {
    LiveAll,
    AlbumFolder(PathBuf),
    Favorites,
    Images,
    Videos,
    Trash,
}

#[derive(Debug, Clone)]
pub struct MediaPage {
    pub query: MediaQuery,
    pub start: u32,
    pub total: u32,
    pub items: Vec<MediaItem>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FavoriteSummary {
    pub has_favorite: bool,
    pub has_unfavorite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MediaMutation {
    pub changed_ids: Vec<MediaId>,
    pub changed_items: Vec<MediaItem>,
    pub removed_uris: Vec<String>,
}

#[derive(Clone)]
pub struct MediaRepository {
    pool: DbPool,
}

impl MediaRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    pub fn count(&self, query: MediaQuery) -> Result<u32> {
        let count = match query {
            MediaQuery::LiveAll => db::count_live_media(&self.pool)?,
            MediaQuery::Trash => db::list_trashed_media(&self.pool)?.len(),
            MediaQuery::AlbumFolder(path) => db::list_media_by_folder(&self.pool, &path)?.len(),
            MediaQuery::Favorites => db::list_favorite_media_ids(&self.pool)?.len(),
            MediaQuery::Images => media_kind_items(&self.pool, "image")?.len(),
            MediaQuery::Videos => media_kind_items(&self.pool, "video")?.len(),
        };
        u32::try_from(count)
            .map_err(|_| AppError::Backend(format!("media count does not fit u32: {count}")))
    }

    pub fn page(&self, query: MediaQuery, start: u32, limit: u32) -> Result<MediaPage> {
        let total = self.count(query.clone())?;
        let items = match &query {
            MediaQuery::LiveAll => db::list_media_page(&self.pool, start, limit)?,
            MediaQuery::Trash => page_vec(db::list_trashed_media(&self.pool)?, start, limit),
            MediaQuery::AlbumFolder(path) => {
                page_vec(db::list_media_by_folder(&self.pool, path)?, start, limit)
            }
            MediaQuery::Favorites => {
                page_by_ids(&self.pool, db::list_favorite_media_ids(&self.pool)?, start, limit)?
            }
            MediaQuery::Images => page_vec(media_kind_items(&self.pool, "image")?, start, limit),
            MediaQuery::Videos => page_vec(media_kind_items(&self.pool, "video")?, start, limit),
        };

        Ok(MediaPage {
            query,
            start,
            total,
            items,
        })
    }

    pub fn favorite_state(&self, ids: &[MediaId]) -> Result<FavoriteSummary> {
        let mut summary = FavoriteSummary::default();
        for id in ids {
            match db::is_media_favorite(&self.pool, id.get()) {
                Ok(true) => summary.has_favorite = true,
                Ok(false) => summary.has_unfavorite = true,
                Err(_) => summary.has_unfavorite = true,
            }
            if summary.has_favorite && summary.has_unfavorite {
                break;
            }
        }
        Ok(summary)
    }

    pub fn set_favorite(&self, ids: &[MediaId], is_favorite: bool) -> Result<MediaMutation> {
        let mut mutation = MediaMutation::default();
        for id in ids {
            db::set_media_favorite(&self.pool, id.get(), is_favorite)?;
            mutation.changed_ids.push(*id);
            mutation.changed_items.push(db::get_media_item(&self.pool, id.get())?);
        }
        Ok(mutation)
    }

    pub fn move_to_trash(&self, ids: &[MediaId]) -> Result<MediaMutation> {
        let mut mutation = MediaMutation::default();
        for id in ids {
            let item = db::get_media_item(&self.pool, id.get())?;
            crate::core::trash::move_to_trash_marked(&self.pool, item.id, &item.uri)?;
            mutation.changed_ids.push(*id);
            mutation.removed_uris.push(item.uri);
        }
        Ok(mutation)
    }

    pub fn upsert_batch(&self, items: &[NewMediaItem]) -> Result<MediaMutation> {
        let changed_items = db::upsert_media_items_batch(&self.pool, items)?;
        let changed_ids = changed_items
            .iter()
            .map(|item| MediaId::from(item.id))
            .collect();
        Ok(MediaMutation {
            changed_ids,
            changed_items,
            removed_uris: Vec::new(),
        })
    }
}

fn page_vec(items: Vec<MediaItem>, start: u32, limit: u32) -> Vec<MediaItem> {
    items
        .into_iter()
        .skip(start as usize)
        .take(limit as usize)
        .collect()
}

fn page_by_ids(pool: &DbPool, ids: Vec<i64>, start: u32, limit: u32) -> Result<Vec<MediaItem>> {
    let mut out = Vec::new();
    for id in ids.into_iter().skip(start as usize).take(limit as usize) {
        out.push(db::get_media_item(pool, id)?);
    }
    Ok(out)
}

fn media_kind_items(pool: &DbPool, media_kind: &str) -> Result<Vec<MediaItem>> {
    Ok(db::list_media_page(pool, 0, u32::MAX)?
        .into_iter()
        .filter(|item| {
            (media_kind == "image" && item.is_image())
                || (media_kind == "video" && item.is_video())
        })
        .collect())
}
