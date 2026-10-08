use async_trait::async_trait;
use photo_viewer::core::sync::provider::{ProviderError, ProviderResult};
use photo_viewer::core::sync::{
    ProviderCapabilities, RemoteEntry, Revision, SyncProvider, WriteCondition,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

#[derive(Default)]
pub struct ControlledProvider {
    pub objects: Mutex<BTreeMap<String, Vec<u8>>>,
    pub probes: AtomicU64,
    pub entered: AtomicBool,
    hold_next: AtomicBool,
    release: tokio::sync::Notify,
}
impl ControlledProvider {
    pub fn hold_next_download(&self) {
        self.entered.store(false, Ordering::Release);
        self.hold_next.store(true, Ordering::Release);
    }
    pub fn release_download(&self) {
        self.release.notify_one();
    }
    pub fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.objects.lock().unwrap().insert(key.into(), bytes);
    }
    fn entry(key: &str, bytes: &[u8]) -> RemoteEntry {
        RemoteEntry {
            key: key.into(),
            is_collection: false,
            size: bytes.len() as u64,
            modified_unix: None,
            revision: Some(Revision::etag(format!(
                "\"{}\"",
                blake3::hash(bytes).to_hex()
            ))),
        }
    }
}
#[async_trait]
impl SyncProvider for ControlledProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            read: true,
            write_new: true,
            conditional_replace: true,
            conditional_delete: true,
            ..Default::default()
        }
    }
    async fn probe(&self) -> ProviderResult<ProviderCapabilities> {
        self.probes.fetch_add(1, Ordering::AcqRel);
        Ok(self.capabilities())
    }
    async fn ensure_collection(&self, _: &str) -> ProviderResult<()> {
        Ok(())
    }
    async fn list_children(&self, collection: &str) -> ProviderResult<Vec<RemoteEntry>> {
        let prefix = if collection.is_empty() {
            String::new()
        } else {
            format!("{}/", collection.trim_matches('/'))
        };
        let objects = self.objects.lock().unwrap();
        let mut entries = BTreeMap::new();
        for (key, bytes) in objects.iter().filter(|(key, _)| key.starts_with(&prefix)) {
            if let Some((name, _)) = key[prefix.len()..].split_once('/') {
                let child = format!("{prefix}{name}");
                entries.entry(child.clone()).or_insert(RemoteEntry {
                    key: child,
                    is_collection: true,
                    size: 0,
                    modified_unix: None,
                    revision: None,
                });
            } else {
                entries.insert(key.clone(), Self::entry(key, bytes));
            }
        }
        Ok(entries.into_values().collect())
    }
    async fn stat(&self, key: &str) -> ProviderResult<Option<RemoteEntry>> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|bytes| Self::entry(key, bytes)))
    }
    async fn download(
        &self,
        key: &str,
        expected: Option<&Revision>,
        destination: &Path,
    ) -> ProviderResult<RemoteEntry> {
        let bytes = self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(key.into()))?;
        let entry = Self::entry(key, &bytes);
        if expected.is_some_and(|rev| Some(rev) != entry.revision.as_ref()) {
            return Err(ProviderError::PreconditionFailed(key.into()));
        }
        if self.hold_next.swap(false, Ordering::AcqRel) {
            self.entered.store(true, Ordering::Release);
            self.release.notified().await;
        }
        tokio::fs::write(destination, &bytes).await?;
        Ok(entry)
    }
    async fn upload(
        &self,
        key: &str,
        source: &Path,
        condition: WriteCondition,
    ) -> ProviderResult<RemoteEntry> {
        let bytes = tokio::fs::read(source).await?;
        let mut objects = self.objects.lock().unwrap();
        match condition {
            WriteCondition::CreateOnly if objects.contains_key(key) => {
                return Err(ProviderError::PreconditionFailed(key.into()))
            }
            WriteCondition::ReplaceIf(rev)
                if objects.get(key).is_none_or(|value| {
                    Self::entry(key, value).revision.as_ref() != Some(&rev)
                }) =>
            {
                return Err(ProviderError::PreconditionFailed(key.into()))
            }
            _ => {}
        }
        objects.insert(key.into(), bytes.clone());
        Ok(Self::entry(key, &bytes))
    }
    async fn delete(&self, key: &str, expected: &Revision) -> ProviderResult<()> {
        let mut objects = self.objects.lock().unwrap();
        if objects
            .get(key)
            .is_none_or(|bytes| Self::entry(key, bytes).revision.as_ref() != Some(expected))
        {
            return Err(ProviderError::PreconditionFailed(key.into()));
        }
        objects.remove(key);
        Ok(())
    }
}
