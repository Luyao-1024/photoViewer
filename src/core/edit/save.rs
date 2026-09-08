//! Render to a temporary sibling, publish atomically, then commit the library row.
//! Overwrites keep a durable `.bak`; a failed database commit restores that backup.
use std::collections::HashSet;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use image::ImageReader;
use tempfile::NamedTempFile;

use crate::core::db::{self, DbPool};
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::edit::{apply_all, EditRegistry, EditState};
use crate::core::error::{AppError, Result};
use crate::core::identity::MediaId;
use crate::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use crate::core::orientation;
use crate::core::telemetry::{OperationTrace, TraceChain};

static SAVING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

struct SaveGuard(PathBuf);

impl SaveGuard {
    fn acquire(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let mut active = SAVING
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| AppError::Backend("save lock unavailable".into()))?;
        if !active.insert(path.clone()) {
            return Err(AppError::Backend(
                "this image is already being saved".into(),
            ));
        }
        Ok(Self(path))
    }
}

impl Drop for SaveGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = SAVING.get_or_init(Mutex::default).lock() {
            active.remove(&self.0);
        }
    }
}

pub fn save_as_copy(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
) -> Result<MediaItem> {
    save_copy(source, state, pool, registry, None)
}

pub fn save_as_copy_with_actor(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
    actor: &DbActorHandle,
) -> Result<MediaItem> {
    save_copy(source, state, pool, registry, Some(actor))
}

fn save_copy(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
    actor: Option<&DbActorHandle>,
) -> Result<MediaItem> {
    let trace = OperationTrace::start(TraceChain::Mutation, "save_as_copy");
    let result = (|| {
        let _guard = SaveGuard::acquire(&source.path)?;
        let rendered = render(source, state, registry, &trace)?;
        let target = generate_edited_path(&source.path);
        let format = image::ImageFormat::from_path(&target)?;
        let staged = encode_sibling(&rendered, &target, format)?;
        // No exists-then-create race: another file must never be overwritten.
        staged
            .persist_noclobber(&target)
            .map_err(|e| AppError::Io(e.error))?;
        sync_parent(&target)?;
        let item = edited_metadata(source, &target, &rendered)?;
        let commit = match actor {
            Some(actor) => actor
                .execute_blocking_in_trace(
                    trace.clone(),
                    DbCommand::InsertEditedMedia { item: item.clone() },
                )
                .and_then(saved_item),
            None => db::upsert_media_items_batch(pool, &[item]).and_then(|mut items| {
                items
                    .pop()
                    .ok_or_else(|| AppError::Backend("saved copy row missing".into()))
            }),
        };
        if commit.is_err() {
            if let Err(error) = std::fs::remove_file(&target) {
                tracing::warn!(
                    "failed to remove unsuccessful edited copy {}: {error}",
                    target.display()
                );
            }
        }
        commit
    })();
    log_edit_result(&trace, result)
}

pub fn save_overwrite(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
) -> Result<()> {
    overwrite(source, state, pool, registry, None)
}

pub fn save_overwrite_with_actor(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
    actor: &DbActorHandle,
) -> Result<()> {
    overwrite(source, state, pool, registry, Some(actor))
}

fn overwrite(
    source: &MediaItem,
    state: &EditState,
    pool: &DbPool,
    registry: &EditRegistry,
    actor: Option<&DbActorHandle>,
) -> Result<()> {
    let trace = OperationTrace::start(TraceChain::Mutation, "save_overwrite");
    let result = (|| {
        let _guard = SaveGuard::acquire(&source.path)?;
        let before = std::fs::metadata(&source.path)?;
        let format = image::ImageFormat::from_path(&source.path)?;
        let rendered = render(source, state, registry, &trace)?;
        let staged = encode_sibling(&rendered, &source.path, format)?;
        staged.as_file().set_permissions(before.permissions())?;
        staged.as_file().sync_all()?;
        let current = std::fs::metadata(&source.path)?;
        if before.len() != current.len() || before.modified()? != current.modified()? {
            return Err(AppError::Backend(
                "source changed while rendering; reopen the image before saving".into(),
            ));
        }
        let backup = backup_path_for(&source.path);
        atomic_copy(&source.path, &backup)?;
        staged
            .persist(&source.path)
            .map_err(|e| AppError::Io(e.error))?;
        sync_parent(&source.path)?;
        let commit = (|| {
            let item = edited_metadata(source, &source.path, &rendered)?;
            match actor {
                Some(actor) => actor
                    .execute_blocking_in_trace(
                        trace.clone(),
                        DbCommand::UpdateEditedMedia {
                            id: MediaId::from(source.id),
                            item,
                        },
                    )
                    .map(|_| ()),
                None => db::update_media_edit_metadata(pool, source.id, &item),
            }
        })();
        if let Err(error) = commit {
            if let Err(rollback) = atomic_copy(&backup, &source.path) {
                return Err(AppError::Backend(format!("save commit failed: {error}; restore failed: {rollback}; original retained at {}", backup.display())));
            }
            return Err(error);
        }
        Ok(())
    })();
    log_edit_result(&trace, result)
}

fn saved_item(result: DbCommandResult) -> Result<MediaItem> {
    match result {
        DbCommandResult::MediaItems(mut items) => items
            .pop()
            .ok_or_else(|| AppError::Backend("saved media row missing".into())),
        other => Err(AppError::Backend(format!(
            "unexpected save response: {other:?}"
        ))),
    }
}

fn render(
    source: &MediaItem,
    state: &EditState,
    registry: &EditRegistry,
    trace: &OperationTrace,
) -> Result<image::DynamicImage> {
    let image = {
        let _stage = trace.stage("load_source");
        load_source_image(&source.path)?
    };
    let _stage = trace.stage("render");
    apply_all(registry, image, state).map_err(AppError::Decode)
}

fn encode_sibling(
    image: &image::DynamicImage,
    target: &Path,
    format: image::ImageFormat,
) -> Result<NamedTempFile> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Backend("save target has no parent".into()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".photo-viewer-save-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        image.write_to(&mut writer, format)?;
        writer.flush()?;
    }
    staged.as_file().sync_all()?;
    let decoded = ImageReader::open(staged.path())?
        .with_guessed_format()?
        .into_dimensions()?;
    if decoded != (image.width(), image.height()) {
        return Err(AppError::Decode(
            "saved image dimensions failed validation".into(),
        ));
    }
    Ok(staged)
}

fn atomic_copy(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Backend("backup target has no parent".into()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".photo-viewer-backup-")
        .tempfile_in(parent)?;
    let mut input = std::fs::File::open(source)?;
    std::io::copy(&mut input, staged.as_file_mut())?;
    staged
        .as_file()
        .set_permissions(input.metadata()?.permissions())?;
    staged.as_file().sync_all()?;
    staged.persist(target).map_err(|e| AppError::Io(e.error))?;
    sync_parent(target)
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn edited_metadata(
    source: &MediaItem,
    path: &Path,
    image: &image::DynamicImage,
) -> Result<NewMediaItem> {
    let metadata = std::fs::metadata(path)?;
    Ok(NewMediaItem {
        uri: format!("file://{}", path.display()),
        path: path.to_owned(),
        folder_path: source.folder_path.clone(),
        mime_type: source.mime_type.clone(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(image.width()),
        height: Some(image.height()),
        video_duration_secs: None,
        taken_at: source.taken_at,
        file_mtime: metadata.modified()?.into(),
        file_size: metadata.len(),
        blake3_hash: stream_file_hash(path)?,
    })
}

fn stream_file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn log_edit_result<T>(trace: &OperationTrace, result: Result<T>) -> Result<T> {
    if let Err(error) = &result {
        crate::core::telemetry::log_error(trace, "save", error);
    }
    result
}

fn load_source_image(path: &Path) -> Result<image::DynamicImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let img = reader.decode()?;
    Ok(apply_orientation_to_image(
        img,
        orientation::read_orientation(path)?,
    ))
}

fn apply_orientation_to_image(img: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

fn generate_edited_path(orig: &Path) -> PathBuf {
    let stem = orig.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let base = match stem.rsplit_once("_edited_") {
        Some((base, timestamp))
            if !base.is_empty()
                && !timestamp.is_empty()
                && timestamp.chars().all(|c| c.is_ascii_digit()) =>
        {
            base
        }
        _ => stem,
    };
    let ext = orig.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    let parent = orig.parent().unwrap_or(Path::new("."));
    let mut timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    loop {
        let candidate = parent.join(format!("{base}_edited_{timestamp}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
        timestamp += 1;
    }
}
