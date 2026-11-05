//! Account-scoped immutable binary cache; only the synchronization worker fetches bytes.
use crate::{replica::LocalMail, storage::Store};
use base64::{engine::general_purpose::STANDARD, Engine};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::Serialize;
use serde_json::Value;

const BLOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("mail_blobs_v1");
pub const MAX_BLOB_BYTES: usize = 25 * 1024 * 1024;
const MAX_ACCOUNT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct BlobRef {
    pub id: String,
    pub media_type: String,
    pub size: Option<u64>,
    pub attachment: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedBlob {
    pub base64: String,
    pub media_type: String,
}
fn db_error(_: impl std::fmt::Display) -> String {
    "Anhangspeicher ist nicht verfuegbar.".into()
}
fn key(scope: &str, id: &str) -> String {
    serde_json::to_string(&(scope, id)).expect("string tuple")
}

pub fn references(mail: &LocalMail) -> Vec<BlobRef> {
    fn walk(value: &Value, root: bool, result: &mut Vec<BlobRef>) {
        match value {
            Value::Object(object) => {
                if let Some(id) = object.get("blobId").and_then(Value::as_str) {
                    if !result.iter().any(|item| item.id == id) {
                        result.push(BlobRef {
                            id: id.into(),
                            media_type: if root {
                                "message/rfc822".into()
                            } else {
                                object
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .unwrap_or("application/octet-stream")
                                    .into()
                            },
                            size: object.get("size").and_then(Value::as_u64),
                            attachment: !root,
                        });
                    }
                }
                for (field, child) in object {
                    if field != "bodyValues" {
                        walk(child, false, result);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, false, result);
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for email in &mail.emails {
        walk(email, true, &mut result);
    }
    result
}
impl Store {
    pub fn initialize_blobs(&self) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(db_error)?;
        tx.open_table(BLOBS).map_err(db_error)?;
        tx.commit().map_err(db_error)
    }
    pub fn blob_bytes(&self, scope: &str, id: &str) -> Result<Option<Vec<u8>>, String> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = tx.open_table(BLOBS).map_err(db_error)?;
        let value = table.get(key(scope, id).as_str()).map_err(db_error)?;
        Ok(value.map(|row| row.value().to_vec()))
    }
    pub fn cache_blob(&self, scope: &str, id: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err("Anhang ist groesser als das Downloadlimit von 25 MiB.".into());
        }
        let tx = self.0.begin_write().map_err(db_error)?;
        {
            let mut table = tx.open_table(BLOBS).map_err(db_error)?;
            let cache_key = key(scope, id);
            if table.get(cache_key.as_str()).map_err(db_error)?.is_some() {
                return Ok(());
            }
            let mut total = bytes.len();
            for entry in table.iter().map_err(db_error)? {
                let (k, v) = entry.map_err(db_error)?;
                let (owner, _): (String, String) =
                    serde_json::from_str(k.value()).map_err(db_error)?;
                if owner == scope {
                    total = total.saturating_add(v.value().len());
                }
            }
            if total > MAX_ACCOUNT_BYTES {
                return Err(
                    "Der lokale Anhangspeicher fuer dieses Konto ist voll (256 MiB).".into(),
                );
            }
            table.insert(cache_key.as_str(), bytes).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }
    pub fn cached_blob(&self, scope: &str, id: &str) -> Result<Option<CachedBlob>, String> {
        let reference = references(&self.local_mail(scope)?)
            .into_iter()
            .find(|item| item.id == id)
            .ok_or("Anhang gehoert zu keiner lokal verfuegbaren Nachricht.")?;
        Ok(self.blob_bytes(scope, id)?.map(|bytes| CachedBlob {
            base64: STANDARD.encode(bytes),
            media_type: reference.media_type,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replica::Collection;
    use serde_json::json;
    #[test]
    fn binary_cache_survives_restart_is_isolated_and_requires_a_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blobs.redb");
        {
            let store = Store::open(&path).unwrap();
            store.apply_remote("a", Collection::Email, None, "s1", &[json!({"id":"e","blobId":"message","keywords":{},"attachments":[{"blobId":"part","type":"application/pdf","size":3}]})], &[], true).unwrap();
            store.cache_blob("a", "part", &[0, 128, 255]).unwrap();
            store.cache_blob("a", "part", &[9]).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.blob_bytes("a", "part").unwrap(),
            Some(vec![0, 128, 255])
        );
        assert!(store.blob_bytes("b", "part").unwrap().is_none());
        assert!(store.cached_blob("b", "part").is_err());
        let cached = store.cached_blob("a", "part").unwrap().unwrap();
        assert_eq!(cached.media_type, "application/pdf");
        assert_eq!(STANDARD.decode(cached.base64).unwrap(), vec![0, 128, 255]);
        store
            .apply_remote(
                "a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[],
                &["e".into()],
                false,
            )
            .unwrap();
        assert!(store.cached_blob("a", "part").is_err());
    }
    #[test]
    fn oversize_blob_is_never_published() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("blobs.redb")).unwrap();
        assert!(store
            .cache_blob("a", "big", &vec![0; MAX_BLOB_BYTES + 1])
            .is_err());
        assert!(store.blob_bytes("a", "big").unwrap().is_none());
    }
}
