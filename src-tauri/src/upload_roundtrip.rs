//! Separately gated one-blob integration probe. Never calls JMAP mutations,
//! submission, or the general write worker. Probe databases are temporary;
//! attempt receipts persist to prevent accidental repeated uploads.
use crate::{
    blobs::BlobRef,
    config::TestConfig,
    storage::Store,
    sync::{Connection, SyncError},
    uploads::{self, UploadRemote},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const BYTES: usize = 1024;
const MEDIA_TYPE: &str = "application/octet-stream";
const GATE: &str = "one-blob-1024";
fn payload() -> Vec<u8> {
    (0..BYTES).map(|index| (index % 256) as u8).collect()
}
fn enabled(value: Option<&str>) -> bool {
    value == Some(GATE)
}
fn receipt_name(run: Option<&str>) -> Result<&'static str, String> {
    match run {
        None | Some("initial") => Ok("live-upload-roundtrip.json"),
        Some("http200-fix") => Ok("live-upload-roundtrip-http200-fix.json"),
        _ => Err(failure("unbekannte Prüfausführung")),
    }
}
fn active_profile(registry: &crate::accounts::Registry) -> Result<(&str, &str), String> {
    let profile = registry
        .profiles
        .iter()
        .find(|p| Some(&p.id) == registry.active_profile_id.as_ref())
        .ok_or_else(|| failure("kein aktives Profil"))?;
    let account = profile
        .accounts
        .iter()
        .find(|a| Some(&a.id) == profile.active_account_id.as_ref())
        .ok_or_else(|| failure("kein aktiver Profilzugang"))?;
    Ok((&profile.id, &account.id))
}
pub(crate) fn test_config() -> Result<TestConfig, String> {
    if let Some(path) = std::env::var_os("JMAP_TEST_REGISTRY") {
        let registry = registry_snapshot(&PathBuf::from(path))?;
        let (profile, account) = active_profile(&registry)?;
        TestConfig::existing_for_test(profile, account)
    } else {
        TestConfig::load()
    }
}
fn registry_snapshot(path: &Path) -> Result<crate::accounts::Registry, String> {
    use redb::{ReadableDatabase, TableDefinition};
    // A whole-file shared lock excludes the normal redb writer during copying.
    // Open/recover only the temporary snapshot: a dirty app DB remains untouched.
    let mut source =
        std::fs::File::open(path).map_err(|_| failure("Profilregistrierung nicht lesbar"))?;
    source
        .try_lock_shared()
        .map_err(|_| failure("App-Datenbank ist geöffnet; kein konsistenter Snapshot möglich"))?;
    if source
        .metadata()
        .map_err(|_| failure("Snapshot-Größe nicht lesbar"))?
        .len()
        > 512 * 1024 * 1024
    {
        return Err(failure(
            "Profil-Snapshot überschreitet das Testlimit von 512 MiB",
        ));
    }
    let temporary = tempfile::tempdir().map_err(|_| failure("temporärer Profil-Snapshot"))?;
    let copy_path = temporary.path().join("registry.redb");
    {
        let mut copy =
            std::fs::File::create(&copy_path).map_err(|_| failure("temporärer Profil-Snapshot"))?;
        std::io::copy(&mut source, &mut copy)
            .and_then(|_| copy.sync_all())
            .map_err(|_| failure("Profil-Snapshot nicht vollständig"))?;
    }
    drop(source);
    let database = redb::Database::open(&copy_path)
        .map_err(|_| failure("temporärer Profil-Snapshot nicht lesbar"))?;
    let tx = database
        .begin_read()
        .map_err(|_| failure("Profil-Snapshot nicht lesbar"))?;
    let table = tx
        .open_table(TableDefinition::<&str, &str>::new("session_summaries_v1"))
        .map_err(|_| failure("Profiltabelle fehlt"))?;
    let row = table
        .get("profile-registry-v1")
        .map_err(|_| failure("Profil-Snapshot nicht lesbar"))?
        .ok_or_else(|| failure("Profilregistrierung fehlt"))?;
    serde_json::from_str(row.value()).map_err(|_| failure("Profilregistrierung ungültig"))
}
fn failure(stage: &str) -> String {
    format!("Upload-Prüfung fehlgeschlagen: {stage}. Keine Antwortdaten protokolliert.")
}

trait DownloadProbe {
    fn download(
        &self,
        account: &str,
        blob: &BlobRef,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, SyncError>> + Send;
}
impl DownloadProbe for Connection {
    async fn download(&self, account: &str, blob: &BlobRef) -> Result<Vec<u8>, SyncError> {
        self.download_blob(account, blob).await
    }
}
struct OneBlob<R> {
    inner: R,
    account: String,
    attempted: AtomicBool,
    report: PathBuf,
}
impl<R: UploadRemote + Sync> UploadRemote for OneBlob<R> {
    async fn upload(
        &self,
        account: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<String, SyncError> {
        if account != self.account || bytes != payload() || media_type != MEDIA_TYPE {
            return Err(SyncError::Local(failure("Testumfang überschritten")));
        }
        if self.attempted.swap(true, Ordering::SeqCst) {
            return Err(SyncError::Local(failure("zweiter Upload gesperrt")));
        }
        // Persist before transport, including when the eventual response is
        // lost. A second invocation cannot silently create another test blob.
        let mut report = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.report)
            .map_err(|_| {
                SyncError::Local(failure(
                    "Prüfbeleg existiert bereits oder ist nicht schreibbar",
                ))
            })?;
        use std::io::Write;
        report
            .write_all(
                json!({"status":"upload_attempted","bytes":BYTES,"maxUploads":1})
                    .to_string()
                    .as_bytes(),
            )
            .and_then(|_| report.sync_all())
            .map_err(|_| SyncError::Local(failure("Prüfbeleg nicht gespeichert")))?;
        // No retry, even on a transport timeout: the blob may already exist.
        let id = match self.inner.upload(account, bytes, media_type).await {
            Ok(id) => id,
            Err(error) => {
                let status = match error {
                    SyncError::UploadHttpStatus(_) => "upload_http_error",
                    SyncError::Network => "upload_network_error",
                    SyncError::Protocol => "upload_response_invalid",
                    _ => "upload_local_error",
                };
                write_report(&self.report, status).map_err(SyncError::Local)?;
                if let SyncError::UploadHttpStatus(code) = &error {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&self.report)
                        .map_err(|_| SyncError::Local(failure("Prüfbeleg nicht erreichbar")))?;
                    file.write_all(json!({"status":status,"httpStatus":code,"bytes":BYTES,"maxUploads":1,"mailSent":false,"fileNodeCreated":false}).to_string().as_bytes()).and_then(|_| file.sync_all()).map_err(|_| SyncError::Local(failure("Prüfbeleg nicht gespeichert")))?;
                }
                return Err(error);
            }
        };
        write_report(&self.report, "uploaded").map_err(SyncError::Local)?;
        Ok(id)
    }
}
fn write_report(path: &Path, status: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|_| failure("Prüfbeleg nicht erreichbar"))?;
    file.write_all(json!({"status":status,"bytes":BYTES,"maxUploads":1,"mailSent":false,"fileNodeCreated":false}).to_string().as_bytes())
        .and_then(|_| file.sync_all()).map_err(|_| failure("Prüfbeleg nicht aktualisiert"))
}

// This function only receives blob-specific interfaces. There is no transport
// here capable of sending Email/set, FileNode/set, or EmailSubmission/set.
async fn roundtrip(
    path: &Path,
    scope: &str,
    account: &str,
    uploader: &impl UploadRemote,
    reader: &impl DownloadProbe,
) -> Result<(), String> {
    let local_id;
    {
        let store = Store::open(path)?;
        local_id = uploads::stage(&store, scope, &payload(), MEDIA_TYPE)?.blob_id;
        store.save_local_draft(scope,json!({"subject":"Herakles Upload-Test (nur lokal)","keywords":{"$draft":true},"attachments":[{"blobId":local_id,"name":"test.bin","type":MEDIA_TYPE,"size":BYTES}]}))?;
    }
    let remote_id;
    {
        let store = Store::open(path)?;
        verify_local(&store, scope, &local_id)?;
        // A method call is data for prepare(), never passed to a JMAP caller.
        let calls = vec![(
            "Email/set".into(),
            json!({"accountId":account,"create":{"test":{"attachments":[{"blobId":local_id}]}}}),
            "local-only".into(),
        )];
        if uploads::prepare(&store, scope, account, &calls, uploader, || false)
            .await
            .is_ok()
        {
            return Err(failure("Offline-Sperre"));
        }
        let resolved = uploads::prepare(&store, scope, account, &calls, uploader, || true)
            .await
            .map_err(|_| failure("Upload oder persistente Zuordnung"))?;
        remote_id = resolved[0].1["create"]["test"]["attachments"][0]["blobId"]
            .as_str()
            .filter(|id| !id.starts_with(uploads::PREFIX))
            .ok_or_else(|| failure("Blob-ID-Zuordnung"))?
            .to_owned();
        let reference = BlobRef {
            id: remote_id.clone(),
            media_type: MEDIA_TYPE.into(),
            size: Some(BYTES as u64),
            attachment: true,
        };
        let received = reader
            .download(account, &reference)
            .await
            .map_err(|_| failure("Zurücklesen"))?;
        if received != payload() {
            return Err(failure("binärer Vergleich"));
        }
        store.cache_blob(scope, &remote_id, &received)?;
    }
    // No network interface is available to the reopen/offline verification.
    verify_offline(path, scope, &local_id, &remote_id)
}
fn verify_local(store: &Store, scope: &str, id: &str) -> Result<(), String> {
    let local = uploads::cached(store, scope, id)?;
    if STANDARD
        .decode(local.base64)
        .map_err(|_| failure("lokale Codierung"))?
        != payload()
        || local.media_type != MEDIA_TYPE
    {
        return Err(failure("lokale Datei nach Neustart"));
    }
    if uploads::cached(store, &format!("{scope}:other-account"), id).is_ok() {
        return Err(failure("Kontotrennung"));
    }
    Ok(())
}
fn verify_offline(path: &Path, scope: &str, local: &str, remote: &str) -> Result<(), String> {
    let store = Store::open(path)?;
    verify_local(&store, scope, local)?;
    let mapped = uploads::cached_remote(&store, scope, remote)?
        .ok_or_else(|| failure("persistente Zuordnung nach Neustart"))?;
    if STANDARD
        .decode(mapped.base64)
        .map_err(|_| failure("Zuordnungscodierung"))?
        != payload()
        || store.blob_bytes(scope, remote)?.as_deref() != Some(payload().as_slice())
    {
        return Err(failure("Offline-Cache nach Neustart"));
    }
    if store
        .blob_bytes(&format!("{scope}:other-account"), remote)?
        .is_some()
    {
        return Err(failure("Download-Kontotrennung"));
    }
    Ok(())
}

#[test]
#[ignore = "Explicit one-blob authorization required: JMAP_UPLOAD_TEST=one-blob-1024; uploads exactly 1024 synthetic bytes, no mail or FileNode mutations"]
fn live_upload_blob_roundtrip() {
    assert!(
        enabled(std::env::var("JMAP_UPLOAD_TEST").ok().as_deref()),
        "Live-Upload ist nicht freigegeben. Erforderlich: JMAP_UPLOAD_TEST=one-blob-1024."
    );
    tauri::async_runtime::block_on(async {
        let report = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.cache")
            .join(
                receipt_name(std::env::var("JMAP_UPLOAD_RUN").ok().as_deref())
                    .expect("Prüfausführung nicht freigegeben."),
            );
        assert!(
            !report.exists(),
            "Prüfbeleg vorhanden. Dieselbe Prüfausführung darf nicht wiederholt werden."
        );
        std::fs::create_dir_all(report.parent().unwrap())
            .expect("Prüfverzeichnis nicht verfügbar.");
        let mut config = test_config()
            .expect("Nativer Zugang nicht verfügbar; keine Zugangsdaten protokolliert.");
        config.read_only = true;
        let session: Value = serde_json::from_slice(
            &crate::session::session_bytes(&config)
                .await
                .expect("Nur-Lese-Sitzungsprüfung fehlgeschlagen."),
        )
        .expect("Sitzungsformat ungültig.");
        let account = session["primaryAccounts"]["urn:ietf:params:jmap:mail"]
            .as_str()
            .expect("Kein primäres Mailkonto.");
        assert!(
            session["accounts"][account]["isPersonal"] == true,
            "Test ist auf das persönliche primäre Mailkonto begrenzt."
        );
        assert!(
            session["accounts"][account]["isReadOnly"] != true,
            "Testkonto ist schreibgeschützt."
        );
        let reader = Connection::discover(&config, account)
            .await
            .expect("Nur-Lese-Verbindung fehlgeschlagen.");
        // A separate ephemeral configuration; never save or alter the account's
        // configured read-only setting. The wrapper exposes only one fixed upload.
        let upload_config = TestConfig {
            url: config.url.clone(),
            username: config.username.clone(),
            password: config.password.clone(),
            read_only: false,
        };
        let uploader = OneBlob {
            inner: Connection::discover(&upload_config, account)
                .await
                .expect("Upload-Verbindung fehlgeschlagen."),
            account: account.into(),
            attempted: AtomicBool::new(false),
            report: report.clone(),
        };
        let dir = tempfile::tempdir().expect("Temporäre Datenbank nicht verfügbar.");
        roundtrip(
            &dir.path().join("upload.redb"),
            &crate::sync::scope(&config, account),
            account,
            &uploader,
            &reader,
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(uploader.attempted.load(Ordering::SeqCst));
        write_report(&report, "verified_offline_after_reopen")
            .expect("Prüfbeleg konnte nicht abgeschlossen werden.");
        println!("Ein 1024-Byte-Testblob hochgeladen, bytegenau zurückgelesen und offline nach redb-Neustart geprüft. Kein Versand und kein FileNode angelegt; keine Inhalte protokolliert.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn profile_snapshot_preserves_source_and_refuses_an_active_writer_or_missing_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.redb");
        {
            let store = Store::open(&path).unwrap();
            let registry = json!({"activeProfileId":"selected","profiles":[{"id":"selected","name":"Synthetic","activeAccountId":"login","accounts":[{"id":"login","label":"Synthetic","username":"synthetic","serverUrl":"https://mail.example.test/jmap","readOnly":true}]}]});
            store
                .put("profile-registry-v1", &registry.to_string())
                .unwrap();
            assert!(registry_snapshot(&path).is_err());
        }
        let before = std::fs::read(&path).unwrap();
        let registry = registry_snapshot(&path).unwrap();
        assert_eq!(active_profile(&registry).unwrap(), ("selected", "login"));
        assert!(before == std::fs::read(&path).unwrap());
        let mut missing = registry;
        missing.active_profile_id = Some("missing".into());
        assert!(active_profile(&missing).is_err());
    }
    struct Fake {
        uploads: AtomicUsize,
        downloads: AtomicUsize,
        corrupt: bool,
    }
    impl UploadRemote for Fake {
        async fn upload(
            &self,
            _account: &str,
            bytes: Vec<u8>,
            _media_type: &str,
        ) -> Result<String, SyncError> {
            self.uploads.fetch_add(1, Ordering::SeqCst);
            if bytes != payload() {
                return Err(SyncError::Protocol);
            }
            Ok("synthetic-server-blob".into())
        }
    }
    impl DownloadProbe for Fake {
        async fn download(
            &self,
            _account: &str,
            reference: &BlobRef,
        ) -> Result<Vec<u8>, SyncError> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            if reference.size != Some(BYTES as u64) {
                return Err(SyncError::Protocol);
            }
            let mut bytes = payload();
            if self.corrupt {
                bytes[0] ^= 1;
            }
            Ok(bytes)
        }
    }
    #[tokio::test]
    async fn roundtrip_checks_restart_offline_binary_integrity_and_one_upload_only() {
        let dir = tempfile::tempdir().unwrap();
        let reader = Fake {
            uploads: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
            corrupt: false,
        };
        let uploader = OneBlob {
            inner: Fake {
                uploads: AtomicUsize::new(0),
                downloads: AtomicUsize::new(0),
                corrupt: false,
            },
            account: "a".into(),
            attempted: AtomicBool::new(false),
            report: dir.path().join("report.json"),
        };
        roundtrip(
            &dir.path().join("probe.redb"),
            "scope",
            "a",
            &uploader,
            &reader,
        )
        .await
        .unwrap();
        assert_eq!(uploader.inner.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(reader.downloads.load(Ordering::SeqCst), 1);
        assert!(uploader.upload("a", payload(), MEDIA_TYPE).await.is_err());
        assert_eq!(uploader.inner.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(reader.uploads.load(Ordering::SeqCst), 0);
        assert!(!enabled(None));
        assert!(!enabled(Some("true")));
        assert!(enabled(Some(GATE)));
        assert_ne!(
            receipt_name(None).unwrap(),
            receipt_name(Some("http200-fix")).unwrap()
        );
        assert!(receipt_name(Some("../other")).is_err());
    }
    #[tokio::test]
    async fn existing_receipt_wrong_scope_and_corrupt_download_stop_the_probe() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.json");
        std::fs::write(&report, b"already attempted").unwrap();
        let uploader = OneBlob {
            inner: Fake {
                uploads: AtomicUsize::new(0),
                downloads: AtomicUsize::new(0),
                corrupt: false,
            },
            account: "a".into(),
            attempted: AtomicBool::new(false),
            report: report.clone(),
        };
        assert!(uploader
            .upload("other", payload(), MEDIA_TYPE)
            .await
            .is_err());
        assert!(uploader
            .upload("a", vec![0; BYTES], MEDIA_TYPE)
            .await
            .is_err());
        assert!(uploader.upload("a", payload(), MEDIA_TYPE).await.is_err());
        assert_eq!(uploader.inner.uploads.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(report).unwrap(), b"already attempted");
        let remote = Fake {
            uploads: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
            corrupt: true,
        };
        let result = roundtrip(
            &dir.path().join("corrupt.redb"),
            "scope",
            "a",
            &remote,
            &remote,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(remote.uploads.load(Ordering::SeqCst), 1);
    }
}
