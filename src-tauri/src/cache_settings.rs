//! Local-only download-cache settings; never contacts a server.
use crate::{blobs::CacheUsage, config::TestConfig, storage::Store, sync, SyncGate};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

fn accounts(store: &Store, config: &TestConfig) -> Result<serde_json::Map<String, Value>, String> {
    let raw = store
        .get(&format!("{}:desktop-session", config.cache_key()))?
        .ok_or("Keine lokale Sitzung verfügbar.")?;
    let session: Value = serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
    session["accounts"]
        .as_object()
        .cloned()
        .ok_or("Lokale Konten fehlen.".into())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCache {
    account_id: String,
    name: String,
    #[serde(flatten)]
    usage: CacheUsage,
}
#[tauri::command]
pub async fn download_cache_usage(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
) -> Result<Vec<AccountCache>, String> {
    let _guard = gate.0.lock().await;
    let config = TestConfig::load()?;
    accounts(&store, &config)?
        .into_iter()
        .map(|(id, account)| {
            let usage = store.blob_cache_usage(&sync::scope(&config, &id))?;
            Ok(AccountCache {
                name: account["name"].as_str().unwrap_or(&id).into(),
                account_id: id,
                usage,
            })
        })
        .collect()
}
#[tauri::command]
pub async fn set_download_cache_limit(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    account_id: String,
    limit_mib: u64,
) -> Result<(), String> {
    let _guard = gate.0.lock().await;
    let config = TestConfig::load()?;
    if !accounts(&store, &config)?.contains_key(&account_id) {
        return Err("Konto gehört nicht zur aktuellen lokalen Sitzung.".into());
    }
    store.set_blob_cache_limit(&sync::scope(&config, &account_id), limit_mib)
}
