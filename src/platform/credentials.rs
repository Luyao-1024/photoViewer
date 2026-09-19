use crate::core::error::{AppError, Result};

const SERVICE: &str = "io.github.luyao_1024.photoviewer.webdav";

pub fn store(reference: &str, password: &str) -> Result<()> {
    if reference.is_empty() || password.is_empty() {
        return Err(AppError::Backend(
            "credential reference and password must not be empty".into(),
        ));
    }
    entry(reference)?
        .set_password(password)
        .map_err(|error| AppError::Backend(format!("cannot store WebDAV credential: {error}")))
}

pub fn load(reference: &str) -> Result<String> {
    entry(reference)?
        .get_password()
        .map_err(|error| AppError::Backend(format!("cannot load WebDAV credential: {error}")))
}

pub fn delete(reference: &str) -> Result<()> {
    entry(reference)?
        .delete_credential()
        .map_err(|error| AppError::Backend(format!("cannot delete WebDAV credential: {error}")))
}

fn entry(reference: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, reference).map_err(|error| {
        AppError::Backend(format!("cannot access system credential store: {error}"))
    })
}
