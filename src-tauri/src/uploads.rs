//! Durable local upload bytes. Uploads are dependencies of approved writes,
//! never independent background jobs. Retrying an upload cannot submit mail.
use crate::{blobs, journal::Calls, storage::Store, sync::SyncError};
use base64::{engine::general_purpose::STANDARD, Engine};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DATA: TableDefinition<&str, &[u8]> = TableDefinition::new("desktop_upload_bytes_v1");
const META: TableDefinition<&str, &str> = TableDefinition::new("desktop_upload_meta_v1");
pub const PREFIX: &str = "local-upload-";
const MAX_ACCOUNT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub blob_id: String,
    pub size: usize,
    #[serde(rename = "type")]
    pub media_type: String,
}
#[derive(Serialize, Deserialize)]
struct Metadata {
    upload: Upload,
    remote: Option<String>,
    mapped_at: u64,
}
fn error(_: impl std::fmt::Display) -> String {
    "Lokaler Uploadspeicher ist nicht verfügbar.".into()
}
fn key(scope: &str, id: &str) -> String {
    serde_json::to_string(&(scope, id)).expect("string tuple")
}
fn now() -> Result<std::time::Duration, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)
}
pub fn initialize(store: &Store) -> Result<(), String> {
    let tx = store.0.begin_write().map_err(error)?;
    tx.open_table(DATA).map_err(error)?;
    tx.open_table(META).map_err(error)?;
    tx.commit().map_err(error)
}
pub fn stage(store: &Store, scope: &str, bytes: &[u8], media_type: &str) -> Result<Upload, String> {
    if bytes.len() > blobs::MAX_BLOB_BYTES {
        return Err("Datei überschreitet das Uploadlimit von 25 MiB.".into());
    }
    if media_type.len() > 255 || media_type.bytes().any(|b| b.is_ascii_control()) {
        return Err("Ungültiger Dateityp.".into());
    }
    let media_type = if media_type.is_empty() {
        "application/octet-stream"
    } else {
        media_type
    };
    let tx = store.0.begin_write().map_err(error)?;
    let upload;
    {
        let mut data = tx.open_table(DATA).map_err(error)?;
        let mut meta = tx.open_table(META).map_err(error)?;
        let mut total = bytes.len();
        for row in data.iter().map_err(error)? {
            let (k, value) = row.map_err(error)?;
            let (owner, _): (String, String) = serde_json::from_str(k.value()).map_err(error)?;
            if owner == scope {
                total = total.saturating_add(value.value().len());
            }
        }
        if total > MAX_ACCOUNT_BYTES {
            return Err("Lokaler Uploadspeicher ist voll (256 MiB pro Konto).".into());
        }
        let mut serial = now()?.as_nanos();
        let id = loop {
            let id = format!("{PREFIX}{serial}");
            if meta.get(key(scope, &id).as_str()).map_err(error)?.is_none() {
                break id;
            }
            serial = serial.checked_add(1).ok_or("Uploadnummer ausgeschöpft.")?;
        };
        upload = Upload {
            blob_id: id,
            size: bytes.len(),
            media_type: media_type.into(),
        };
        let k = key(scope, &upload.blob_id);
        data.insert(k.as_str(), bytes).map_err(error)?;
        let encoded = serde_json::to_string(&Metadata {
            upload: upload.clone(),
            remote: None,
            mapped_at: 0,
        })
        .map_err(error)?;
        meta.insert(k.as_str(), encoded.as_str()).map_err(error)?;
    }
    tx.commit().map_err(error)?;
    Ok(upload)
}
fn read(store: &Store, scope: &str, id: &str) -> Result<Metadata, String> {
    let tx = store.0.begin_read().map_err(error)?;
    let table = tx.open_table(META).map_err(error)?;
    let row = table
        .get(key(scope, id).as_str())
        .map_err(error)?
        .ok_or("Lokaler Anhang fehlt in diesem Konto.")?;
    serde_json::from_str(row.value()).map_err(error)
}
pub fn describe(store: &Store, scope: &str, id: &str) -> Result<Upload, String> {
    Ok(read(store, scope, id)?.upload)
}
pub fn cached(store: &Store, scope: &str, id: &str) -> Result<blobs::CachedBlob, String> {
    let meta = read(store, scope, id)?;
    Ok(blobs::CachedBlob {
        base64: STANDARD.encode(bytes(store, scope, id)?),
        media_type: meta.upload.media_type,
    })
}
fn bytes(store: &Store, scope: &str, id: &str) -> Result<Vec<u8>, String> {
    let tx = store.0.begin_read().map_err(error)?;
    let table = tx.open_table(DATA).map_err(error)?;
    let row = table
        .get(key(scope, id).as_str())
        .map_err(error)?
        .ok_or("Lokale Dateidaten fehlen.")?;
    Ok(row.value().to_vec())
}
// Caller must first authorize a current mail/file reference to the server id.
pub fn cached_remote(
    store: &Store,
    scope: &str,
    id: &str,
) -> Result<Option<blobs::CachedBlob>, String> {
    let tx = store.0.begin_read().map_err(error)?;
    let table = tx.open_table(META).map_err(error)?;
    for row in table.iter().map_err(error)? {
        let (k, value) = row.map_err(error)?;
        let (owner, _): (String, String) = serde_json::from_str(k.value()).map_err(error)?;
        if owner == scope {
            let meta: Metadata = serde_json::from_str(value.value()).map_err(error)?;
            if meta.remote.as_deref() == Some(id) {
                return cached(store, scope, &meta.upload.blob_id).map(Some);
            }
        }
    }
    Ok(None)
}
pub fn local_ids(value: &Value) -> Vec<String> {
    fn walk(value: &Value, ids: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (field, value) in map {
                    if field == "blobId" {
                        if let Some(id) = value.as_str().filter(|id| id.starts_with(PREFIX)) {
                            if !ids.iter().any(|v| v == id) {
                                ids.push(id.into());
                            }
                        }
                    } else if field != "bodyValues" {
                        walk(value, ids);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, ids);
                }
            }
            _ => {}
        }
    }
    let mut ids = vec![];
    walk(value, &mut ids);
    ids
}
pub fn validate(store: &Store, scope: &str, value: &Value) -> Result<(), String> {
    for id in local_ids(value) {
        read(store, scope, &id)?;
    }
    Ok(())
}
pub trait UploadRemote {
    fn upload(
        &self,
        account: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> impl std::future::Future<Output = Result<String, SyncError>> + Send;
}
pub async fn prepare(
    store: &Store,
    scope: &str,
    account: &str,
    calls: &Calls,
    remote: &impl UploadRemote,
    enabled: impl Fn() -> bool,
) -> Result<Calls, String> {
    let mut result = calls.clone();
    for (_, args, _) in &mut result {
        // A staged blob belongs only to the operation's account.
        for id in local_ids(args) {
            if args["accountId"] != account {
                return Err("Anhang gehört zu einem anderen Konto.".into());
            }
            let mut meta = read(store, scope, &id)?;
            // Unreferenced server blobs can expire. Only reuse a recent mapping;
            // original local bytes remain available for a later upload.
            if meta.remote.is_none() || now()?.as_secs().saturating_sub(meta.mapped_at) >= 300 {
                if !enabled() {
                    return Err("Offline: Upload bleibt lokal gespeichert.".into());
                }
                let bytes = bytes(store, scope, &id)?;
                let remote_id = remote
                    .upload(account, bytes, &meta.upload.media_type)
                    .await
                    .map_err(|e| e.message())?;
                if remote_id.is_empty() || remote_id.starts_with(PREFIX) {
                    return Err("Ungültige Uploadantwort.".into());
                }
                meta.remote = Some(remote_id);
                meta.mapped_at = now()?.as_secs();
                let tx = store.0.begin_write().map_err(error)?;
                {
                    let mut table = tx.open_table(META).map_err(error)?;
                    let encoded = serde_json::to_string(&meta).map_err(error)?;
                    table
                        .insert(key(scope, &id).as_str(), encoded.as_str())
                        .map_err(error)?;
                }
                tx.commit().map_err(error)?;
            }
            replace(
                args,
                &id,
                meta.remote.as_deref().ok_or("Uploadzuordnung fehlt.")?,
            );
        }
    }
    if !enabled() {
        return Err("Offline: Auftrag bleibt lokal gespeichert.".into());
    }
    Ok(result)
}
fn replace(value: &mut Value, id: &str, remote: &str) {
    match value {
        Value::Object(map) => {
            for (field, child) in map {
                if field == "blobId" && child.as_str() == Some(id) {
                    *child = Value::String(remote.into());
                } else if field != "bodyValues" {
                    replace(child, id, remote);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                replace(item, id, remote);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fake {
        count: AtomicUsize,
        fail: bool,
    }
    impl UploadRemote for Fake {
        async fn upload(
            &self,
            account: &str,
            bytes: Vec<u8>,
            media_type: &str,
        ) -> Result<String, SyncError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(account, "a");
            assert_eq!(bytes, [0, 128, 255]);
            assert_eq!(media_type, "application/octet-stream");
            if self.fail {
                Err(SyncError::Network)
            } else {
                Ok("server-blob".into())
            }
        }
    }
    fn calls(id: &str) -> Calls {
        vec![(
            "Email/set".into(),
            json!({"accountId":"a","create":{"e":{"attachments":[{"blobId":id}],"bodyStructure":{"subParts":[{"blobId":id}]},"bodyValues":{"text":{"value":id}}}}}),
            "mail".into(),
        )]
    }
    #[tokio::test]
    async fn attachments_and_mappings_survive_restart_without_changing_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uploads.redb");
        let upload;
        let draft;
        {
            let store = Store::open(&path).unwrap();
            upload = stage(&store, "a", &[0, 128, 255], "").unwrap();
            draft = store.save_local_draft("a", json!({"keywords":{"$draft":true},"attachments":[{"blobId":upload.blob_id,"name":"test.bin"}]})).unwrap();
            assert!(store
                .save_local_draft(
                    "b",
                    json!({"keywords":{"$draft":true},"attachments":[{"blobId":upload.blob_id}]})
                )
                .is_err());
        }
        let fake = Fake {
            count: AtomicUsize::new(0),
            fail: false,
        };
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                STANDARD
                    .decode(cached(&store, "a", &upload.blob_id).unwrap().base64)
                    .unwrap(),
                [0, 128, 255]
            );
            assert!(cached(&store, "b", &upload.blob_id).is_err());
            let prepared = prepare(&store, "a", "a", &calls(&upload.blob_id), &fake, || true)
                .await
                .unwrap();
            assert_eq!(fake.count.load(Ordering::SeqCst), 1);
            assert_eq!(
                prepared[0].1["create"]["e"]["attachments"][0]["blobId"],
                "server-blob"
            );
            assert_eq!(
                prepared[0].1["create"]["e"]["bodyStructure"]["subParts"][0]["blobId"],
                "server-blob"
            );
            assert_eq!(
                prepared[0].1["create"]["e"]["bodyValues"]["text"]["value"],
                upload.blob_id
            );
            let mail = store.local_mail("a").unwrap();
            assert_eq!(
                mail.emails
                    .iter()
                    .find(|email| email["id"] == draft)
                    .unwrap()["attachments"][0]["blobId"],
                upload.blob_id
            );
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            STANDARD
                .decode(
                    cached_remote(&store, "a", "server-blob")
                        .unwrap()
                        .unwrap()
                        .base64
                )
                .unwrap(),
            [0, 128, 255]
        );
        assert!(cached_remote(&store, "b", "server-blob").unwrap().is_none());
        prepare(&store, "a", "a", &calls(&upload.blob_id), &fake, || true)
            .await
            .unwrap();
        assert_eq!(fake.count.load(Ordering::SeqCst), 1);
        assert!(
            prepare(&store, "b", "a", &calls(&upload.blob_id), &fake, || true)
                .await
                .is_err()
        );
        assert!(
            prepare(&store, "a", "b", &calls(&upload.blob_id), &fake, || true)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn failed_or_offline_upload_keeps_bytes_and_never_changes_the_send() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("uploads.redb")).unwrap();
        let upload = stage(&store, "a", &[0, 128, 255], "").unwrap();
        let fake = Fake {
            count: AtomicUsize::new(0),
            fail: true,
        };
        let calls = calls(&upload.blob_id);
        let id =
            crate::journal::enqueue_send(&store, "a", calls.clone(), true, None, None).unwrap();
        assert!(prepare(&store, "a", "a", &calls, &fake, || false)
            .await
            .is_err());
        assert_eq!(fake.count.load(Ordering::SeqCst), 0);
        crate::journal::approve(&store, "a", &[id]).unwrap();
        assert!(prepare(&store, "a", "a", &calls, &fake, || true)
            .await
            .is_err());
        let entry = crate::journal::entries(&store, "a").unwrap().remove(0);
        assert_eq!(entry.status, "ready");
        assert_eq!(entry.calls, calls);
        assert!(read(&store, "a", &upload.blob_id).unwrap().remote.is_none());
        assert!(cached(&store, "a", &upload.blob_id).is_ok());
        assert!(stage(&store, "a", &[1], "text/plain\r\ninjected: true").is_err());
        assert!(stage(&store, "a", &vec![0; blobs::MAX_BLOB_BYTES + 1], "").is_err());
    }
}
