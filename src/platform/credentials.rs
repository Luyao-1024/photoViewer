use crate::core::error::{AppError, Result};

/// The platform secret store holds one kind of secret: a native SyncServer
/// Bearer API key. The reference — never the key itself — is what SQLite keeps.
const SERVICE: &str = "io.github.luyao_1024.photoviewer.native_syncserver";

pub fn store_api_key(reference: &str, api_key: &str) -> Result<()> {
    if reference.is_empty() || api_key.is_empty() {
        return Err(AppError::Backend(
            "credential reference and API key must not be empty".into(),
        ));
    }
    entry(reference)?
        .set_password(api_key)
        .map_err(|error| AppError::Backend(format!("cannot store sync API key: {error}")))
}

pub fn load_api_key(reference: &str) -> Result<String> {
    entry(reference)?
        .get_password()
        .map_err(|error| AppError::Backend(format!("cannot load sync API key: {error}")))
}

pub fn delete_api_key(reference: &str) -> Result<()> {
    match entry(reference)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(AppError::Backend(format!(
            "cannot delete sync API key: {error}"
        ))),
    }
}

fn entry(reference: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, reference).map_err(|error| {
        AppError::Backend(format!("cannot access system credential store: {error}"))
    })
}
