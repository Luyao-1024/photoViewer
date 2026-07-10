use crate::core::db::{self, DbPool, SearchField};
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::error::{AppError, Result};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem};
use crate::core::refresh::LibraryStats;
use crate::core::section_model::{counts_from_date_groups, GroupBy, SectionKey};
use rusqlite::types::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaQuery {
    LiveAll,
    Search {
        term: String,
        field: SearchField,
    },
    SearchKind {
        term: String,
        media_kind: String,
        field: SearchField,
    },
    AlbumFolder(PathBuf),
    Favorites,
    Images,
    Videos,
    MotionPhotos,
    Attribute(String),
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

#[derive(Debug, Clone)]
pub struct MediaNeighbor {
    pub query: MediaQuery,
    pub index: u32,
    pub total: u32,
    pub item: MediaItem,
}

#[derive(Clone)]
pub struct MediaRepository {
    pool: DbPool,
}

fn search_count_filter(
    term: String,
    media_kind: Option<String>,
    field: SearchField,
) -> (String, Vec<Value>) {
    let pattern = db::search_like_pattern(&term);
    let (field_clause, mut params) = match field {
        SearchField::All => (
            "(lower(path) LIKE lower(?) ESCAPE '\\' \
             OR strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ? ESCAPE '\\')",
            vec![Value::Text(pattern.clone()), Value::Text(pattern)],
        ),
        SearchField::Name => (
            "lower(path) LIKE lower(?) ESCAPE '\\'",
            vec![Value::Text(pattern)],
        ),
        SearchField::Date => (
            "strftime('%Y-%m-%d', datetime(COALESCE(taken_at, file_mtime), 'unixepoch')) LIKE ? ESCAPE '\\'",
            vec![Value::Text(pattern)],
        ),
    };

    if let Some(media_kind) = media_kind {
        let mut with_kind = vec![Value::Text(media_kind)];
        with_kind.append(&mut params);
        (
            format!("trashed_at IS NULL AND media_kind = ? AND {field_clause}"),
            with_kind,
        )
    } else {
        (format!("trashed_at IS NULL AND {field_clause}"), params)
    }
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
            MediaQuery::Search { term, field } => {
                db::count_live_media_search(&self.pool, &term, None, field)?
            }
            MediaQuery::SearchKind {
                term,
                media_kind,
                field,
            } => db::count_live_media_search(&self.pool, &term, Some(&media_kind), field)?,
            MediaQuery::Trash => db::count_trashed_media(&self.pool)?,
            MediaQuery::AlbumFolder(path) => db::count_media_by_folder(&self.pool, &path)?,
            MediaQuery::Favorites => db::count_favorite_media(&self.pool)?,
            MediaQuery::Images => db::count_media_by_kind(&self.pool, "image")?,
            MediaQuery::Videos => db::count_media_by_kind(&self.pool, "video")?,
            MediaQuery::MotionPhotos => db::count_media_by_subkind(
                &self.pool,
                crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
            )?,
            MediaQuery::Attribute(attribute) => {
                db::count_media_by_attribute(&self.pool, &attribute)?
            }
        };
        u32::try_from(count)
            .map_err(|_| AppError::Backend(format!("media count does not fit u32: {count}")))
    }

    #[tracing::instrument(name = "repo:page", skip(self))]
    pub fn page(&self, query: MediaQuery, start: u32, limit: u32) -> Result<MediaPage> {
        let total = self.count(query.clone())?;
        let items = self.items(query.clone(), start, limit)?;
        if !matches!(query, MediaQuery::LiveAll) {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                ?query,
                start,
                limit,
                total,
                item_count = items.len(),
                "media_repository_page: loaded"
            );
        }

        Ok(MediaPage {
            query,
            start,
            total,
            items,
        })
    }

    pub fn items(&self, query: MediaQuery, start: u32, limit: u32) -> Result<Vec<MediaItem>> {
        match query {
            MediaQuery::LiveAll => db::list_media_page(&self.pool, start, limit),
            MediaQuery::Search { term, field } => {
                db::list_media_search_page(&self.pool, &term, None, field, start, limit)
            }
            MediaQuery::SearchKind {
                term,
                media_kind,
                field,
            } => db::list_media_search_page(
                &self.pool,
                &term,
                Some(&media_kind),
                field,
                start,
                limit,
            ),
            MediaQuery::Trash => db::list_trashed_media_page(&self.pool, start, limit),
            MediaQuery::AlbumFolder(path) => {
                db::list_media_by_folder_page(&self.pool, &path, start, limit)
            }
            MediaQuery::Favorites => db::list_favorite_media_page(&self.pool, start, limit),
            MediaQuery::Images => db::list_media_by_kind_page(&self.pool, "image", start, limit),
            MediaQuery::Videos => db::list_media_by_kind_page(&self.pool, "video", start, limit),
            MediaQuery::MotionPhotos => db::list_media_by_subkind_page(
                &self.pool,
                crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
                start,
                limit,
            ),
            MediaQuery::Attribute(attribute) => {
                db::list_media_by_attribute_page(&self.pool, &attribute, start, limit)
            }
        }
    }

    pub fn library_stats(&self) -> Result<LibraryStats> {
        Ok(LibraryStats {
            live_total: db::count_live_media(&self.pool)?,
            thumbnails_generated: db::count_thumbnail_generated(&self.pool)?,
        })
    }

    /// 按 Year/Month/Day 分组返回每个 section 的真实媒体计数。
    ///
    /// 计数来自整个库（DB 聚合），而非当前虚拟分页窗口，因此单个年份/月份超过
    /// `virtual_media_page_size` 时仍准确。供 `MediaGrid` 覆盖 section 头部计数。
    pub fn section_counts(&self, mode: GroupBy) -> Result<HashMap<SectionKey, u32>> {
        self.section_counts_for_query(MediaQuery::LiveAll, mode)
    }

    pub fn section_counts_for_query(
        &self,
        query: MediaQuery,
        mode: GroupBy,
    ) -> Result<HashMap<SectionKey, u32>> {
        let groups = match query {
            MediaQuery::LiveAll => db::count_live_media_by_date(&self.pool)?,
            MediaQuery::Search { term, field } => {
                let (where_clause, params) = search_count_filter(term, None, field);
                db::count_media_by_date_for_filter(&self.pool, &where_clause, params)?
            }
            MediaQuery::SearchKind {
                term,
                media_kind,
                field,
            } => {
                let (where_clause, params) = search_count_filter(term, Some(media_kind), field);
                db::count_media_by_date_for_filter(&self.pool, &where_clause, params)?
            }
            MediaQuery::AlbumFolder(path) => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND folder_path = ?",
                vec![Value::Text(path.to_string_lossy().to_string())],
            )?,
            MediaQuery::Favorites => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND is_favorite = 1",
                Vec::new(),
            )?,
            MediaQuery::Images => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND media_kind = ?",
                vec![Value::Text("image".into())],
            )?,
            MediaQuery::Videos => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND media_kind = ?",
                vec![Value::Text("video".into())],
            )?,
            MediaQuery::MotionPhotos => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND media_subkind = ?",
                vec![Value::Text(
                    crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO.into(),
                )],
            )?,
            MediaQuery::Attribute(attribute) => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NULL AND json_extract(media_attributes, ?) = 1",
                vec![Value::Text(format!("$.{attribute}"))],
            )?,
            MediaQuery::Trash => db::count_media_by_date_for_filter(
                &self.pool,
                "trashed_at IS NOT NULL",
                Vec::new(),
            )?,
        };
        Ok(counts_from_date_groups(&groups, mode))
    }

    pub fn neighbor(
        &self,
        query: MediaQuery,
        current_id: MediaId,
        delta: i32,
    ) -> Result<Option<MediaNeighbor>> {
        if delta == 0 {
            return Ok(None);
        }
        let neighbor = match &query {
            MediaQuery::LiveAll => db::live_media_neighbor(&self.pool, current_id.get(), delta)?,
            MediaQuery::AlbumFolder(path) => {
                db::folder_media_neighbor(&self.pool, path, current_id.get(), delta)?
            }
            MediaQuery::Favorites => {
                db::favorite_media_neighbor(&self.pool, current_id.get(), delta)?
            }
            MediaQuery::Images => {
                db::kind_media_neighbor(&self.pool, "image", current_id.get(), delta)?
            }
            MediaQuery::Videos => {
                db::kind_media_neighbor(&self.pool, "video", current_id.get(), delta)?
            }
            MediaQuery::MotionPhotos => db::subkind_media_neighbor(
                &self.pool,
                crate::core::media::MEDIA_SUBKIND_MOTION_PHOTO,
                current_id.get(),
                delta,
            )?,
            MediaQuery::Attribute(attribute) => {
                db::attribute_media_neighbor(&self.pool, attribute, current_id.get(), delta)?
            }
            MediaQuery::Search { term, field } => {
                db::search_media_neighbor(&self.pool, term, None, *field, current_id.get(), delta)?
            }
            MediaQuery::SearchKind {
                term,
                media_kind,
                field,
            } => db::search_media_neighbor(
                &self.pool,
                term,
                Some(media_kind),
                *field,
                current_id.get(),
                delta,
            )?,
            MediaQuery::Trash => db::trashed_media_neighbor(&self.pool, current_id.get(), delta)?,
        };

        Ok(neighbor.map(|(index, total, item)| MediaNeighbor {
            query,
            index,
            total,
            item,
        }))
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
            mutation
                .changed_items
                .push(db::get_media_item(&self.pool, id.get())?);
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

    pub fn restore_from_trash(&self, ids: &[MediaId]) -> Result<MediaMutation> {
        let mut mutation = MediaMutation::default();
        for id in ids {
            let item = db::get_media_item(&self.pool, id.get())?;
            crate::core::trash::restore_from_trash(&item.uri)?;
            db::unmark_trashed(&self.pool, id.get())?;
            let mut restored = item;
            restored.trashed_at = None;
            mutation.changed_ids.push(*id);
            mutation.changed_items.push(restored);
        }
        crate::core::albums::refresh(&self.pool)?;
        Ok(mutation)
    }

    pub fn delete_permanently(&self, ids: &[MediaId]) -> Result<MediaMutation> {
        let mut mutation = MediaMutation::default();
        for id in ids {
            let item = db::get_media_item(&self.pool, id.get())?;
            if let Err(err) = crate::core::trash::delete_permanently(&item.uri) {
                tracing::warn!("failed to delete trashed file for {}: {err}", item.uri);
            }
            db::delete_media_item(&self.pool, id.get())?;
            mutation.changed_ids.push(*id);
            mutation.removed_uris.push(item.uri);
        }
        crate::core::albums::refresh(&self.pool)?;
        Ok(mutation)
    }

    pub fn empty_trash(&self) -> Result<MediaMutation> {
        let ids = db::list_trashed_media(&self.pool)?
            .into_iter()
            .map(|item| MediaId::from(item.id))
            .collect::<Vec<_>>();
        self.delete_permanently(&ids)
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

    pub fn rename_media_file(&self, id: MediaId, requested_name: &str) -> Result<MediaMutation> {
        self.rename_media_file_inner(id, requested_name, None)
    }

    pub fn rename_media_file_with_actor(
        &self,
        id: MediaId,
        requested_name: &str,
        db_actor: &DbActorHandle,
    ) -> Result<MediaMutation> {
        self.rename_media_file_inner(id, requested_name, Some(db_actor))
    }

    fn rename_media_file_inner(
        &self,
        id: MediaId,
        requested_name: &str,
        db_actor: Option<&DbActorHandle>,
    ) -> Result<MediaMutation> {
        let item = db::get_media_item(&self.pool, id.get())?;
        let target = rename_target_path(&item.path, requested_name)?;
        if target == item.path {
            return Ok(MediaMutation {
                changed_ids: vec![id],
                changed_items: vec![item],
                removed_uris: Vec::new(),
            });
        }
        if target.exists() {
            return Err(AppError::Backend(format!(
                "target file already exists: {}",
                target.display()
            )));
        }

        std::fs::rename(&item.path, &target)?;
        let parent = target
            .parent()
            .ok_or_else(|| AppError::Backend("renamed file has no parent folder".into()))?;
        let changed_result: Result<MediaItem> = match db_actor {
            Some(actor) => match actor.execute_blocking(DbCommand::UpdateMediaLocation {
                id,
                path: target.clone(),
                folder_path: parent.to_path_buf(),
            }) {
                Ok(DbCommandResult::MediaItems(mut items)) => items
                    .pop()
                    .ok_or_else(|| AppError::Backend("renamed media row was not returned".into())),
                Ok(other) => Err(AppError::Backend(format!(
                    "unexpected rename DB result: {other:?}"
                ))),
                Err(err) => Err(err),
            },
            None => db::update_media_location(&self.pool, item.id, &target, parent)
                .and_then(|_| db::get_media_item(&self.pool, item.id)),
        };
        let changed = match changed_result {
            Ok(changed) => changed,
            Err(err) => {
                if let Err(rollback_err) = std::fs::rename(&target, &item.path) {
                    tracing::warn!(
                        "failed to roll back rename from {} to {} after DB update error {err}: {rollback_err}",
                        target.display(),
                        item.path.display()
                    );
                }
                return Err(err);
            }
        };
        if changed.path != target {
            let err = AppError::Backend("renamed media row did not persist target path".into());
            if let Err(rollback_err) = std::fs::rename(&target, &item.path) {
                tracing::warn!(
                    "failed to roll back rename from {} to {} after DB update error {err}: {rollback_err}",
                    target.display(),
                    item.path.display()
                );
            }
            return Err(err);
        }
        Ok(MediaMutation {
            changed_ids: vec![id],
            changed_items: vec![changed],
            removed_uris: Vec::new(),
        })
    }
}

fn rename_target_path(current_path: &Path, requested_name: &str) -> Result<PathBuf> {
    let trimmed = requested_name.trim();
    if trimmed.is_empty() {
        return Err(AppError::Backend("file name cannot be empty".into()));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
        return Err(AppError::Backend(
            "file name cannot contain path separators".into(),
        ));
    }

    let requested_path = Path::new(trimmed);
    let stem = requested_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| AppError::Backend("file name cannot be empty".into()))?;
    let parent = current_path
        .parent()
        .ok_or_else(|| AppError::Backend("current file has no parent folder".into()))?;
    let mut file_name = stem.to_string();
    if let Some(ext) = current_path.extension().and_then(|ext| ext.to_str()) {
        file_name.push('.');
        file_name.push_str(ext);
    }
    Ok(parent.join(file_name))
}
