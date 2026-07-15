use photo_viewer::core::db::{self, DbPool};
use photo_viewer::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use photo_viewer::core::events::DomainEventSender;
use photo_viewer::core::identity::MediaId;
use photo_viewer::core::media::NewMediaItem;

fn actor(pool: &DbPool) -> DbActorHandle {
    let (events, _receiver) = DomainEventSender::new();
    photo_viewer::core::db_actor::start_db_actor(pool.clone(), events)
}

pub fn insert_media_item(pool: &DbPool, item: &NewMediaItem) -> photo_viewer::core::Result<i64> {
    let result = actor(pool).execute_blocking(DbCommand::InsertMediaItem { item: item.clone() })?;
    let DbCommandResult::MediaItems(items) = result else {
        return Err(photo_viewer::AppError::Backend(
            "expected inserted media item".into(),
        ));
    };
    items
        .first()
        .map(|item| item.id)
        .ok_or_else(|| photo_viewer::AppError::Backend("insert returned no item".into()))
}

pub fn set_media_favorite(
    pool: &DbPool,
    id: i64,
    is_favorite: bool,
) -> photo_viewer::core::Result<()> {
    actor(pool)
        .execute_blocking(DbCommand::SetFavorite {
            ids: vec![MediaId::from(id)],
            is_favorite,
        })
        .map(|_| ())
}

pub fn mark_trashed(pool: &DbPool, id: i64) -> photo_viewer::core::Result<()> {
    actor(pool)
        .execute_blocking(DbCommand::MarkTrashed {
            ids: vec![MediaId::from(id)],
            trace_id: None,
        })
        .map(|_| ())
}

pub fn delete_media_item(pool: &DbPool, id: i64) -> photo_viewer::core::Result<()> {
    actor(pool)
        .execute_blocking(DbCommand::DeleteMediaRows {
            ids: vec![MediaId::from(id)],
        })
        .map(|_| ())
}

pub fn mark_thumbnails_generated(pool: &DbPool, ids: &[i64]) -> photo_viewer::core::Result<()> {
    actor(pool)
        .execute_blocking(DbCommand::MarkThumbnailsGenerated {
            ids: ids.iter().copied().map(MediaId::from).collect(),
        })
        .map(|_| ())
}

pub fn init_pool(path: &std::path::Path) -> DbPool {
    db::init_pool(path).unwrap()
}
