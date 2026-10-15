//! Persistent in-memory credential provider; never contacts the real system keyring.
use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, Mutex},
};
type Values = Arc<Mutex<HashMap<String, Vec<u8>>>>;
struct Builder(Values);
struct Entry {
    values: Values,
    key: String,
}
impl keyring::credential::CredentialApi for Entry {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(self.key.clone(), secret.to_vec());
        Ok(())
    }
    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.values
            .lock()
            .unwrap()
            .get(&self.key)
            .cloned()
            .ok_or(keyring::Error::NoEntry)
    }
    fn delete_credential(&self) -> keyring::Result<()> {
        self.values
            .lock()
            .unwrap()
            .remove(&self.key)
            .map(|_| ())
            .ok_or(keyring::Error::NoEntry)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl keyring::credential::CredentialBuilderApi for Builder {
    fn build(
        &self,
        target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<keyring::credential::Credential>> {
        Ok(Box::new(Entry {
            values: self.0.clone(),
            key: format!("{target:?}/{service}/{user}"),
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
pub fn install() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        keyring::set_default_credential_builder(Box::new(Builder(Arc::new(Mutex::new(
            HashMap::new(),
        )))))
    });
}
