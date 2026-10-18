use super::*;
use crate::core::sync::store::{runtime::Command, ManagedPhoto, PhotoAction};

impl SyncService {
    /// The run slot protects publication/reset against this user decision too.
    /// A stale form or an unknown issued outcome is rejected, never discarded.
    pub async fn accept_photo_action(
        &self,
        job: i64,
        captured: ManagedPhoto,
        action: PhotoAction,
    ) -> Result<Option<PathBuf>> {
        let _slot = self.acquire_run_slot(job).await?;
        self.require_selected_account(job)?;
        if matches!(action, PhotoAction::KeepLocal) && captured.conflict.is_some() {
            self.transfer_selected(job).await?;
        }
        let service = self.clone();
        let recovery = tokio::task::spawn_blocking(move || {
            service.recover_publications(job)?;
            let _files = super::super::publication_io::write()?;
            let recovery = if captured.conflict.is_some() {
                let directory = service.staging_root().parent().unwrap_or(service.staging_root()).join("sync-recovery").join(uuid::Uuid::new_v4().to_string());
                std::fs::create_dir_all(&directory)?;
                if matches!(action, PhotoAction::KeepLocal) {
                    let conn=service.pool().get()?;
                    let mut statement=conn.prepare("SELECT r.staging_path,r.role FROM sync_native_resources r JOIN sync_conflicts f ON f.job_id=r.job_id JOIN sync_records cr ON cr.owner_kind='conflict' AND cr.owner_key=CAST(f.id AS TEXT) JOIN sync_record_fields cf ON cf.record_id=cr.id AND cf.asset_id=r.asset_id AND ((cf.field_name='resOriginalRes' AND r.role='original') OR (cf.field_name='resJPEGThumbRes' AND r.role='thumbnail')) WHERE f.entry_id=?1 AND f.state='open' AND r.state='staged'")?;
                    let paths=statement.query_map([captured.entry_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    for (source,role) in paths { let target=directory.join(format!("cloud-{role}"));std::fs::copy(source,&target)?;std::fs::File::open(&target)?.sync_all()?; }
                }
                if let Some(source) = captured.path.as_ref() {

                let canonical_root = service.store.get_job(job)?.ok_or_else(|| AppError::Backend("job removed".into()))?.local_root;
                if !std::fs::canonicalize(source)?.starts_with(std::fs::canonicalize(canonical_root)?) { return Err(AppError::Backend("recovery source escaped the local root".into())); }
                let target = directory.join(source.file_name().ok_or_else(|| AppError::Backend("invalid source path".into()))?);
                let before=std::fs::metadata(source)?;
                std::fs::copy(source, &target)?;
                let after=std::fs::metadata(source)?;
                use std::os::unix::fs::MetadataExt;
                if before.ino()!=after.ino() || before.dev()!=after.dev() || before.len()!=after.len() || before.mtime()!=after.mtime() || before.mtime_nsec()!=after.mtime_nsec() {return Err(AppError::Backend("local photo changed while preserving recovery copy".into()));}
                std::fs::File::open(&target)?.sync_all()?;
                }
                let metadata=service.pool().get()?.query_row("SELECT m.is_favorite,m.taken_at FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[captured.entry_id],|r|Ok((r.get::<_,bool>(0)?,r.get::<_,Option<i64>>(1)?))).ok();
                let note=format!("PhotoViewer recovery copy\nJob: {job}\nEntry: {}\nLocal version: {}\nOriginal relative path: {}\nFavorite: {:?}\nTaken at: {:?}\n",captured.entry_id,captured.local_version,captured.relative_path,metadata.map(|m|m.0),metadata.and_then(|m|m.1));
                std::fs::write(directory.join("README.txt"),note)?;
                std::fs::File::open(directory.join("README.txt"))?.sync_all()?;
                std::fs::File::open(&directory)?.sync_all()?;
                Some(directory)
            } else if matches!(action, PhotoAction::UseCloud) { Some(service.staging_root().to_path_buf()) } else { None };
            service.store.runtime(Command::UserAction {job,entry:captured.entry_id,version:captured.local_version,conflict:captured.conflict,action,recovery:recovery.as_ref().map(|p|p.to_string_lossy().to_string())})?;
            Ok::<_, AppError>(recovery)
        }).await.map_err(|_|AppError::Backend("photo decision worker stopped".into()))??;
        drop(_slot);
        self.request_job_sync(job)?;
        Ok(recovery)
    }
}
