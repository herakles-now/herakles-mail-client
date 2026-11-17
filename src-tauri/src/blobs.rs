//! Account-scoped immutable binary cache; only the synchronization worker fetches bytes.
use crate::{replica::LocalMail, storage::Store};
use base64::{engine::general_purpose::STANDARD, Engine};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::Serialize;
use serde_json::Value;

const BLOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("mail_blobs_v1");
// Adapted from Bulwark lib/offline/cache.ts: bounded cache, oldest first,
// with durable outgoing data kept outside the evictable tables.
const ORDER: TableDefinition<&str, u64> = TableDefinition::new("mail_blob_order_v1");
pub const MAX_BLOB_BYTES: usize = 25 * 1024 * 1024;
const LIMITS: TableDefinition<&str, u64> = TableDefinition::new("mail_blob_limits_v1");
const MAX_ACCOUNT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheUsage {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub count: u64,
}

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
        tx.open_table(ORDER).map_err(db_error)?;
        tx.open_table(LIMITS).map_err(db_error)?;
        tx.commit().map_err(db_error)
    }
    pub fn blob_cache_usage(&self, scope: &str) -> Result<CacheUsage, String> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = tx.open_table(BLOBS).map_err(db_error)?;
        let mut used_bytes = 0_u64;
        let mut count = 0;
        for row in table.iter().map_err(db_error)? {
            let (k, v) = row.map_err(db_error)?;
            let (owner, _): (String, String) = serde_json::from_str(k.value()).map_err(db_error)?;
            if owner == scope {
                used_bytes += v.value().len() as u64;
                count += 1;
            }
        }
        let limit_bytes = tx
            .open_table(LIMITS)
            .map_err(db_error)?
            .get(scope)
            .map_err(db_error)?
            .map_or(MAX_ACCOUNT_BYTES as u64, |v| v.value());
        Ok(CacheUsage {
            used_bytes,
            limit_bytes,
            count,
        })
    }
    pub fn set_blob_cache_limit(&self, scope: &str, limit_mib: u64) -> Result<(), String> {
        if !(25..=4096).contains(&limit_mib) {
            return Err("Das Cachelimit muss zwischen 25 und 4096 MiB liegen.".into());
        }
        let limit = limit_mib * 1024 * 1024;
        let tx = self.0.begin_write().map_err(db_error)?;
        {
            let mut table = tx.open_table(BLOBS).map_err(db_error)?;
            let mut order = tx.open_table(ORDER).map_err(db_error)?;
            let mut entries = Vec::new();
            let mut total = 0_u64;
            for row in table.iter().map_err(db_error)? {
                let (k, v) = row.map_err(db_error)?;
                let (owner, _): (String, String) =
                    serde_json::from_str(k.value()).map_err(db_error)?;
                if owner == scope {
                    let size = v.value().len() as u64;
                    total += size;
                    let age = order
                        .get(k.value())
                        .map_err(db_error)?
                        .map_or(0, |v| v.value());
                    entries.push((age, k.value().to_owned(), size));
                }
            }
            entries.sort();
            for (_, key, size) in entries {
                if total <= limit {
                    break;
                }
                table.remove(key.as_str()).map_err(db_error)?;
                order.remove(key.as_str()).map_err(db_error)?;
                total -= size;
            }
            tx.open_table(LIMITS)
                .map_err(db_error)?
                .insert(scope, limit)
                .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }
    pub fn blob_bytes(&self, scope: &str, id: &str) -> Result<Option<Vec<u8>>, String> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = tx.open_table(BLOBS).map_err(db_error)?;
        let value = table.get(key(scope, id).as_str()).map_err(db_error)?;
        Ok(value.map(|row| row.value().to_vec()))
    }
    pub fn cache_blob(&self, scope: &str, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.cache_blob_bounded(scope, id, bytes, 0, true)
    }
    /// Background prefetch must not evict blobs just to download them again next sync.
    pub fn prefetch_blob(&self, scope: &str, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.cache_blob_bounded(scope, id, bytes, 0, false)
    }
    fn cache_blob_bounded(
        &self,
        scope: &str,
        id: &str,
        bytes: &[u8],
        limit: usize,
        evict: bool,
    ) -> Result<(), String> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err("Anhang ist groesser als das Downloadlimit von 25 MiB.".into());
        }
        let tx = self.0.begin_write().map_err(db_error)?;
        {
            let limit = if limit == 0 {
                tx.open_table(LIMITS)
                    .map_err(db_error)?
                    .get(scope)
                    .map_err(db_error)?
                    .map_or(MAX_ACCOUNT_BYTES, |v| v.value() as usize)
            } else {
                limit
            };
            let mut table = tx.open_table(BLOBS).map_err(db_error)?;
            let mut order = tx.open_table(ORDER).map_err(db_error)?;
            let cache_key = key(scope, id);
            if table.get(cache_key.as_str()).map_err(db_error)?.is_some() {
                return Ok(());
            }
            let mut total = bytes.len();
            let mut candidates = Vec::new();
            let mut sequence = 0_u64;
            for entry in table.iter().map_err(db_error)? {
                let (k, v) = entry.map_err(db_error)?;
                let (owner, _): (String, String) =
                    serde_json::from_str(k.value()).map_err(db_error)?;
                if owner == scope {
                    total = total.saturating_add(v.value().len());
                    // Pre-ledger blobs migrate lazily and are evicted first.
                    let age = order
                        .get(k.value())
                        .map_err(db_error)?
                        .map_or(0, |v| v.value());
                    sequence = sequence.max(age);
                    candidates.push((age, k.value().to_owned(), v.value().len()));
                }
            }
            if bytes.len() > limit || (!evict && total > limit) {
                return Err("Das eingestellte Downloadcache-Limit reicht nicht aus.".into());
            }
            candidates.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            for (_, old_key, size) in candidates {
                if total <= limit {
                    break;
                }
                table.remove(old_key.as_str()).map_err(db_error)?;
                order.remove(old_key.as_str()).map_err(db_error)?;
                total -= size;
            }
            table.insert(cache_key.as_str(), bytes).map_err(db_error)?;
            order
                .insert(cache_key.as_str(), sequence.saturating_add(1))
                .map_err(db_error)?;
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
    fn configured_limit_is_atomic_persistent_and_used_by_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("limits.redb");
        let payload = vec![5; 14 * 1024 * 1024];
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store.blob_cache_usage("a").unwrap().limit_bytes,
                256 * 1024 * 1024
            );
            store.cache_blob("a", "old", &payload).unwrap();
            store.cache_blob("a", "new", &payload).unwrap();
            store.cache_blob("b", "old", &[9]).unwrap();
            store.put("draft-sentinel", "preserved").unwrap();
            assert!(store.set_blob_cache_limit("a", 24).is_err());
            assert!(store.set_blob_cache_limit("a", 4097).is_err());
            assert_eq!(store.blob_cache_usage("a").unwrap().count, 2);
            store.set_blob_cache_limit("a", 25).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let usage = store.blob_cache_usage("a").unwrap();
        assert_eq!(usage.limit_bytes, 25 * 1024 * 1024);
        assert_eq!(usage.used_bytes, payload.len() as u64);
        assert_eq!(usage.count, 1);
        assert!(store.blob_bytes("a", "old").unwrap().is_none());
        assert_eq!(
            store.blob_cache_usage("b").unwrap().limit_bytes,
            256 * 1024 * 1024
        );
        assert_eq!(store.blob_bytes("b", "old").unwrap(), Some(vec![9]));
        assert_eq!(
            store.get("draft-sentinel").unwrap().as_deref(),
            Some("preserved")
        );
        assert!(store.prefetch_blob("a", "next", &payload).is_err());
        assert!(store.blob_bytes("a", "new").unwrap().is_some());
        store.cache_blob("a", "next", &payload).unwrap();
        assert!(store.blob_bytes("a", "new").unwrap().is_none());
        assert_eq!(store.blob_cache_usage("a").unwrap().count, 1);
        store.set_blob_cache_limit("a", 512).unwrap();
        store.prefetch_blob("a", "extra", &payload).unwrap();
        assert_eq!(store.blob_cache_usage("a").unwrap().count, 2);
    }
    #[test]
    fn eviction_is_durable_isolated_and_prefetch_does_not_evict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bounded.redb");
        {
            let store = Store::open(&path).unwrap();
            store
                .cache_blob_bounded("a", "z-old", &[1; 3], 6, true)
                .unwrap();
            store
                .cache_blob_bounded("a", "a-new", &[2; 3], 6, true)
                .unwrap();
            store.cache_blob("b", "z-old", &[9; 3]).unwrap();
        }
        let store = Store::open(&path).unwrap();
        store
            .cache_blob_bounded("a", "z-old", &[7], 6, true)
            .unwrap();
        assert!(store
            .cache_blob_bounded("a", "next", &[3; 3], 6, false)
            .is_err());
        assert_eq!(store.blob_bytes("a", "z-old").unwrap(), Some(vec![1; 3]));
        store
            .cache_blob_bounded("a", "next", &[3; 3], 6, true)
            .unwrap();
        assert!(store.blob_bytes("a", "z-old").unwrap().is_none());
        assert_eq!(store.blob_bytes("a", "a-new").unwrap(), Some(vec![2; 3]));
        assert_eq!(store.blob_bytes("b", "z-old").unwrap(), Some(vec![9; 3]));
        assert!(store
            .cache_blob_bounded("a", "big", &[4; 7], 6, true)
            .is_err());
        assert_eq!(store.blob_bytes("a", "next").unwrap(), Some(vec![3; 3]));
    }

    #[test]
    fn legacy_blobs_migrate_without_data_loss_and_are_evicted_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.redb");
        {
            let db = redb::Database::create(&path).unwrap();
            let tx = db.begin_write().unwrap();
            tx.open_table(BLOBS)
                .unwrap()
                .insert(key("a", "legacy").as_str(), &[1_u8; 3][..])
                .unwrap();
            tx.commit().unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.blob_bytes("a", "legacy").unwrap(), Some(vec![1; 3]));
        store
            .cache_blob_bounded("a", "new", &[2; 3], 6, true)
            .unwrap();
        store
            .cache_blob_bounded("a", "next", &[3; 3], 6, true)
            .unwrap();
        assert!(store.blob_bytes("a", "legacy").unwrap().is_none());
        assert_eq!(store.blob_bytes("a", "new").unwrap(), Some(vec![2; 3]));
    }
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
