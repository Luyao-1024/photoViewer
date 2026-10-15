//! Explicitly invoked live acceptance/setup tool; never runs as part of tests.
//! Credentials remain in the system keyring. Reports contain no authentication data.
use photo_viewer::core::{
    backend::local::LocalBackend,
    db,
    db_actor::{start_db_actor, DbCommand},
    events::{ChangeSource, DomainEventSender},
    sync::{NativeZone, NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn argument(args: &[String], name: &str) -> anyhow::Result<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|p| PathBuf::from(&p[1]))
        .ok_or_else(|| anyhow::anyhow!("missing {name}"))
}
fn digest(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.iter().any(|a|a=="--configure-library"),"this tool writes your local sync configuration and uploads the chosen album; pass --configure-library explicitly");
    let connection: serde_json::Value =
        serde_json::from_slice(&std::fs::read(argument(&args, "--connection")?)?)?;
    let album = std::fs::canonicalize(argument(&args, "--album")?)?;
    let root = album
        .parent()
        .ok_or_else(|| anyhow::anyhow!("album has no root"))?
        .to_path_buf();
    let relative = album.strip_prefix(&root)?.to_string_lossy().to_string();
    let evidence = argument(&args, "--evidence")?;
    std::fs::create_dir_all(&evidence)?;
    let endpoint = connection["baseURL"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("connection has no endpoint"))?;
    let key = std::fs::read_to_string(
        connection["apiKeyFile"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("connection has no key file"))?,
    )?
    .trim()
    .to_string();
    let data = photo_viewer::config::data_dir();
    std::fs::create_dir_all(&data)?;
    let pool = db::init_pool(&data.join("photos.db"))?;
    let (sender, mut events) = DomainEventSender::new();
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    let actor = start_db_actor(pool.clone(), sender);
    let service = SyncService::with_actor(pool.clone(), actor.clone());
    service.reconcile_interrupted_runs()?;
    let pinned = service.probe_native_connection(endpoint, &key).await?;
    let zone = connection["zones"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("connection has no zone"))?;
    let job = if let Some(job) = service.store().list_jobs()?.into_iter().find(|j| {
        j.endpoint == pinned.public_base_url
            && j.local_root == root
            && j.native_zone.as_ref().is_some_and(|z| z.zone_name == zone)
    }) {
        job
    } else {
        service
            .create_native_job_scoped(
                endpoint,
                &format!("native-{}", uuid::Uuid::new_v4()),
                &key,
                &root,
                zone,
                SyncDirection::Bidirectional,
                UploadScope::SelectedAlbums,
                vec![relative.clone()],
            )
            .await?
    };
    anyhow::ensure!(
        service.store().upload_albums(job.id)? == vec![relative.clone()],
        "existing job has a different upload scope; edit it in Settings before verifying"
    );
    let scan_pool = pool.clone();
    let scan_album = album.clone();
    let items =
        tokio::task::spawn_blocking(move || LocalBackend::new(scan_pool).scan_dir(&scan_album))
            .await??;
    anyhow::ensure!(!items.is_empty(), "album contains no supported media");
    actor.execute_blocking(DbCommand::UpsertMediaBatch {
        source: ChangeSource::StartupScan,
        items,
    })?;
    let started = std::time::Instant::now();
    println!(
        "Connected to {}. Synchronizing only album {}",
        pinned.public_base_url, relative
    );
    let primary = service.run_cycle(job.id).await?;
    anyhow::ensure!(
        primary.failures.is_empty() && primary.outstanding == 0,
        "upload incomplete: {primary:?}"
    );
    println!(
        "Primary: confirmed {} photo mutations",
        primary.mutations_confirmed
    );
    // The second client has its own database and an empty destination outside the library.
    let destination = evidence.join("receiver");
    std::fs::create_dir_all(&destination)?;
    let receiver_pool = db::init_pool(&evidence.join("receiver.sqlite"))?;
    let (sender, mut events) = DomainEventSender::new();
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    let receiver_actor = start_db_actor(receiver_pool.clone(), sender);
    let receiver_key = key.clone();
    let receiver = SyncService::with_actor(receiver_pool.clone(), receiver_actor)
        .with_credential_reader(move |_| Ok(receiver_key.clone()));
    let receive_job = if let Some(job) = receiver.store().list_jobs()?.into_iter().next() {
        job
    } else {
        receiver.store().create_job(&NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: pinned.public_base_url.clone(),
            credential_ref: "live-acceptance-in-memory".into(),
            local_root: destination.clone(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::SelectedAlbums,
            upload_albums: vec![],
            native_zone: Some(NativeZone::new(zone)),
            native_pinned: Some(pinned.clone()),
        })?
    };
    let received = receiver.run_cycle(receive_job.id).await?;
    anyhow::ensure!(
        received.failures.is_empty() && received.outstanding == 0,
        "download incomplete: {received:?}"
    );
    let mut verified = vec![];
    for entry in walkdir::WalkDir::new(&album).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative_path = path.strip_prefix(&root)?;
        let target = destination.join(relative_path);
        let original_hash = digest(path)?;
        anyhow::ensure!(
            target.is_file() && digest(&target)? == original_hash,
            "download differs: {}",
            relative_path.display()
        );
        verified.push(
            json!({"path":relative_path,"bytes":entry.metadata()?.len(),"sha256":original_hash}),
        );
    }
    // Real supported metadata edit through the actor, with unconditional restoration.
    let sample = db::list_all_media(&pool)?
        .into_iter()
        .find(|item| item.path.starts_with(&album))
        .ok_or_else(|| anyhow::anyhow!("no indexed sample"))?;
    let original_favorite = sample.is_favorite;
    let metadata_result = async {
        actor.execute_blocking(DbCommand::SetFavorite {
            ids: vec![photo_viewer::core::identity::MediaId::from(sample.id)],
            is_favorite: !original_favorite,
        })?;
        let changed = service.run_cycle(job.id).await?;
        anyhow::ensure!(
            changed.failures.is_empty()
                && changed.outstanding == 0
                && changed.mutations_confirmed == 1,
            "metadata upload failed: {changed:?}"
        );
        let pulled = receiver.run_cycle(receive_job.id).await?;
        anyhow::ensure!(
            pulled.failures.is_empty() && pulled.outstanding == 0,
            "metadata receive failed: {pulled:?}"
        );
        let path = destination.join(sample.path.strip_prefix(&root)?);
        let item = db::get_media_item_by_uri(
            &receiver_pool,
            &photo_viewer::core::file_uri::from_path(&path),
        )?
        .ok_or_else(|| anyhow::anyhow!("received sample missing"))?;
        anyhow::ensure!(
            item.is_favorite != original_favorite,
            "remote favorite was not applied"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    actor.execute_blocking(DbCommand::SetFavorite {
        ids: vec![photo_viewer::core::identity::MediaId::from(sample.id)],
        is_favorite: original_favorite,
    })?;
    let restored = service.run_cycle(job.id).await?;
    anyhow::ensure!(
        restored.failures.is_empty() && restored.outstanding == 0,
        "metadata restoration failed: {restored:?}"
    );
    let restored_receive = receiver.run_cycle(receive_job.id).await?;
    anyhow::ensure!(
        restored_receive.failures.is_empty() && restored_receive.outstanding == 0,
        "metadata restoration receive failed: {restored_receive:?}"
    );
    metadata_result?;
    let idle = service.run_cycle(job.id).await?;
    let receiver_idle = receiver.run_cycle(receive_job.id).await?;
    anyhow::ensure!(
        idle.failures.is_empty()
            && idle.outstanding == 0
            && idle.mutations_sent == 0
            && receiver_idle.failures.is_empty()
            && receiver_idle.outstanding == 0
            && receiver_idle.mutations_sent == 0,
        "idle cycles were not settled"
    );
    // Enabling is the last step: after the complete real pair acceptance.
    photo_viewer::core::prefs::set_sync_enabled(true).map_err(anyhow::Error::msg)?;
    let report = json!({"endpoint":pinned.public_base_url,"zone":zone,"album":album,"job":job.id,"verifiedPhotos":verified.len(),"verified":verified,"primaryConfirmed":primary.mutations_confirmed,"receiverMedia":db::list_all_media(&receiver_pool)?.len(),"idleMutations":idle.mutations_sent,"receiverIdleMutations":receiver_idle.mutations_sent,"seconds":started.elapsed().as_secs(),"schema":11,"metadataRoundTrip":true,"favoriteRestored":true});
    std::fs::write(
        evidence.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("PASS: {} actual photos uploaded, downloaded and SHA-256 checked; idle cycles issue no mutations. Evidence: {}",verified.len(),evidence.display());
    Ok(())
}
