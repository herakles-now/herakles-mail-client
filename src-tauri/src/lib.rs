mod accounts;
mod blobs;
mod calendar;
mod changes;
mod config;
mod desktop;
mod journal;
mod logging;
mod push;
mod replica;
mod session;
mod storage;
mod sync;
mod uploads;
mod writes;

use session::SessionSummary;
use tauri::{Manager, State};

#[derive(Default)]
struct SyncGate(tokio::sync::Mutex<()>);

#[derive(Clone, Copy)]
pub(crate) struct NetworkState {
    pub enabled: bool,
    pub generation: u64,
}

struct NetworkMode(tokio::sync::watch::Sender<NetworkState>);

impl NetworkMode {
    fn new() -> (Self, tokio::sync::watch::Receiver<NetworkState>) {
        let (sender, receiver) = tokio::sync::watch::channel(NetworkState {
            enabled: false,
            generation: 0,
        });
        (Self(sender), receiver)
    }
    fn enabled(&self) -> bool {
        self.0.borrow().enabled
    }
    fn set_enabled(&self, enabled: bool) {
        let mut state = *self.0.borrow();
        if state.enabled != enabled {
            state.enabled = enabled;
            self.0.send_replace(state);
        }
    }
    fn reconnect(&self) {
        let mut state = *self.0.borrow();
        state.generation = state.generation.wrapping_add(1);
        self.0.send_replace(state);
    }
}

fn require_network(mode: &NetworkMode) -> Result<(), String> {
    if mode.enabled() {
        Ok(())
    } else {
        Err("Offline-Modus aktiv: Änderungen bleiben lokal vorgemerkt.".into())
    }
}

#[tauri::command]
fn push_status() -> bool {
    push::connected()
}

#[tauri::command]
fn set_network_enabled(
    enabled: bool,
    mode: State<'_, NetworkMode>,
    store: State<'_, storage::Store>,
) -> Result<(), String> {
    let config = config::TestConfig::load()?;
    for account in all_change_accounts(&store, &config)? {
        store.set_change_review(
            &sync::scope(&config, &account),
            !enabled,
            !enabled || !mode.enabled(),
        )?;
        if !enabled || !mode.enabled() {
            journal::hold(&store, &sync::scope(&config, &account))?;
        }
    }
    mode.set_enabled(enabled);
    logging::event(
        "info",
        "sync",
        "network_mode_changed",
        serde_json::json!({"enabled":enabled}),
    );
    Ok(())
}

#[tauri::command]
fn has_account_configuration() -> Result<bool, String> {
    match config::TestConfig::load() {
        Ok(_) => Ok(true),
        Err(message) if message.starts_with("Kein Konto eingerichtet.") => Ok(false),
        Err(message) => Err(message),
    }
}

#[tauri::command]
async fn setup_account(
    store: State<'_, storage::Store>,
    network: State<'_, NetworkMode>,
    profile_name: String,
    server_url: String,
    username: String,
    password: String,
    read_only: bool,
) -> Result<accounts::Registry, String> {
    let registry = accounts::add_account(
        &store,
        None,
        Some(profile_name),
        None,
        server_url,
        username,
        password,
        read_only,
    )
    .await?;
    network.reconnect();
    logging::event(
        "info",
        "app",
        "account_setup_succeeded",
        serde_json::json!({}),
    );
    Ok(registry)
}

#[tauri::command]
fn profile_inventory(store: State<'_, storage::Store>) -> Result<accounts::Registry, String> {
    accounts::inventory(&store)
}

#[tauri::command]
fn create_profile(
    store: State<'_, storage::Store>,
    name: String,
) -> Result<accounts::Registry, String> {
    accounts::create_profile(&store, name)
}

#[tauri::command]
async fn add_profile_account(
    store: State<'_, storage::Store>,
    network: State<'_, NetworkMode>,
    profile_id: String,
    label: Option<String>,
    server_url: String,
    username: String,
    password: String,
    read_only: bool,
) -> Result<accounts::Registry, String> {
    let registry = accounts::add_account(
        &store,
        Some(profile_id),
        None,
        label,
        server_url,
        username,
        password,
        read_only,
    )
    .await?;
    network.reconnect();
    Ok(registry)
}

#[tauri::command]
fn switch_profile(
    store: State<'_, storage::Store>,
    network: State<'_, NetworkMode>,
    profile_id: String,
) -> Result<accounts::Registry, String> {
    let registry = accounts::activate_profile(&store, &profile_id)?;
    network.reconnect();
    Ok(registry)
}

#[tauri::command]
fn switch_profile_account(
    store: State<'_, storage::Store>,
    network: State<'_, NetworkMode>,
    profile_id: String,
    account_id: String,
) -> Result<accounts::Registry, String> {
    let registry = accounts::activate_account(&store, &profile_id, &account_id)?;
    network.reconnect();
    Ok(registry)
}

#[tauri::command]
fn write_status(store: State<'_, storage::Store>) -> Result<serde_json::Value, String> {
    let config = config::TestConfig::load()?;
    let mut pending = 0;
    let mut blocked = 0;
    let mut pending_deletes = 0;
    let mut blocked_deletes = 0;
    let mut uncertain = 0;
    if let Some(raw) = store.get(&format!("{}:desktop-session", config.cache_key()))? {
        let session: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
        if let Some(accounts) = session["accounts"].as_object() {
            for id in accounts.keys() {
                if sync::account_supports(&session, id, "urn:ietf:params:jmap:mail") {
                    let mail = store.local_mail(&sync::scope(&config, id))?;
                    pending += mail.pending.len();
                    pending += mail.pending_deletes.len();
                    pending_deletes += mail.pending_deletes.len();
                    blocked += mail
                        .pending
                        .iter()
                        .filter(|operation| operation.blocked.is_some())
                        .count();
                    blocked += mail
                        .pending_deletes
                        .iter()
                        .filter(|operation| operation.blocked.is_some())
                        .count();
                    blocked_deletes += mail
                        .pending_deletes
                        .iter()
                        .filter(|operation| operation.blocked.is_some())
                        .count();
                }
            }
        }
    }
    for account in all_change_accounts(&store, &config)? {
        let entries = journal::entries(&store, &sync::scope(&config, &account))?;
        pending += entries.iter().filter(|e| e.status != "applied").count();
        blocked += entries.iter().filter(|e| e.status == "blocked").count();
        uncertain += entries.iter().filter(|e| e.status == "uncertain").count();
    }
    Ok(
        serde_json::json!({"pending":pending,"blocked":blocked,"uncertain":uncertain,"pendingDeletes":pending_deletes,"blockedDeletes":blocked_deletes}),
    )
}

fn change_accounts(
    store: &storage::Store,
    config: &config::TestConfig,
) -> Result<Vec<String>, String> {
    let Some(raw) = store.get(&format!("{}:desktop-session", config.cache_key()))? else {
        return Ok(vec![]);
    };
    let session: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
    Ok(session["accounts"]
        .as_object()
        .into_iter()
        .flat_map(|accounts| accounts.keys())
        .filter(|id| sync::account_supports(&session, id, "urn:ietf:params:jmap:mail"))
        .cloned()
        .collect())
}

fn all_change_accounts(
    store: &storage::Store,
    config: &config::TestConfig,
) -> Result<Vec<String>, String> {
    let Some(raw) = store.get(&format!("{}:desktop-session", config.cache_key()))? else {
        return Ok(vec![]);
    };
    let session: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
    Ok(session["accounts"]
        .as_object()
        .into_iter()
        .flat_map(|a| a.keys())
        .filter(|id| {
            ["urn:ietf:params:jmap:mail", sync::CALENDARS, sync::FILES]
                .iter()
                .any(|cap| sync::account_supports(&session, id, cap))
        })
        .cloned()
        .collect())
}

pub(crate) fn hold_offline_changes(store: &storage::Store) -> Result<(), String> {
    let config = config::TestConfig::load()?;
    for account in all_change_accounts(store, &config)? {
        store.set_change_review(&sync::scope(&config, &account), true, true)?;
        journal::hold(store, &sync::scope(&config, &account))?;
    }
    Ok(())
}

#[tauri::command]
fn pending_changes(store: State<'_, storage::Store>) -> Result<Vec<serde_json::Value>, String> {
    let config = config::TestConfig::load()?;
    let mut rows = Vec::new();
    for account in change_accounts(&store, &config)? {
        let mail = store.local_mail(&sync::scope(&config, &account))?;
        let title = |id: &str| {
            mail.emails
                .iter()
                .find(|e| e["id"] == id)
                .and_then(|e| e["subject"].as_str())
                .unwrap_or("Nachricht nicht mehr lokal verfügbar")
                .to_owned()
        };
        for op in &mail.pending {
            let action = if op.mailbox_ids.is_some() {
                "Ordner ändern"
            } else if op.keyword == "$seen" {
                if op.enabled {
                    "Als gelesen markieren"
                } else {
                    "Als ungelesen markieren"
                }
            } else if op.enabled {
                "Stern hinzufügen"
            } else {
                "Stern entfernen"
            };
            rows.push(serde_json::json!({"accountId":account,"sequence":op.sequence,"objectId":op.email_id,
                "kind":"mail","title":title(&op.email_id),"action":action,"requiresReview":op.requires_review,"blocked":op.blocked,"transferable":true}));
        }
        for op in &mail.pending_deletes {
            rows.push(serde_json::json!({"accountId":account,"sequence":op.sequence,"objectId":op.email_id,
                "kind":"mail","title":"Gelöschter Entwurf","action":"Entwurf löschen","requiresReview":op.requires_review,"blocked":op.blocked,"transferable":true}));
        }
        for email in &mail.emails {
            if email["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("local-draft-"))
            {
                rows.push(serde_json::json!({"accountId":account,"objectId":email["id"],"kind":"draft",
                    "title":email["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("Ohne Betreff"),
                    "action":"Lokaler Entwurf · noch nicht zum Versand vorgemerkt","requiresReview":false,"transferable":false}));
            }
        }
    }
    for account in all_change_accounts(&store, &config)? {
        for entry in journal::entries(&store, &sync::scope(&config, &account))? {
            rows.push(serde_json::json!({"accountId":account,"journalId":entry.id,"objectId":entry.object_id,
                "kind":entry.kind,"title":entry.title,"action":entry.action,"status":entry.status,
                "requiresReview":entry.status=="held","transferable":true,
                "blocked":if ["blocked","uncertain"].contains(&entry.status.as_str()){Some(entry.error.unwrap_or("Serverstatus prüfen".into()))}else{None}}));
        }
    }
    Ok(rows)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeSelection {
    account_id: String,
    sequence: Option<u64>,
    journal_id: Option<u64>,
}

#[tauri::command]
fn approve_pending_changes(
    store: State<'_, storage::Store>,
    changes: Vec<ChangeSelection>,
) -> Result<(), String> {
    let config = config::TestConfig::load()?;
    if config.read_only {
        return Err("Nur-Lese-Modus: Übertragung ist gesperrt.".into());
    }
    let accounts = all_change_accounts(&store, &config)?;
    if changes.iter().any(|c| !accounts.contains(&c.account_id)) {
        return Err("Konto ist nicht verfügbar.".into());
    }
    for account in accounts {
        let sequences: Vec<_> = changes
            .iter()
            .filter(|c| c.account_id == account)
            .filter_map(|c| c.sequence)
            .collect();
        store.approve_changes(&sync::scope(&config, &account), &sequences)?;
        let ids: Vec<_> = changes
            .iter()
            .filter(|c| c.account_id == account)
            .filter_map(|c| c.journal_id)
            .collect();
        journal::approve(&store, &sync::scope(&config, &account), &ids)?;
    }
    Ok(())
}

fn report_sync_error(area: &str, operation: &str, error: sync::SyncError) -> String {
    let reason = error.message();
    logging::event(
        "error",
        area,
        "operation_failed",
        serde_json::json!({"operation":operation,"reason":reason}),
    );
    reason
}

#[tauri::command]
fn log_frontend_event(
    level: String,
    area: String,
    event: String,
    code: Option<String>,
) -> Result<(), String> {
    if !["info", "warn", "error"].contains(&level.as_str())
        || ![
            "composer", "mail", "calendar", "storage", "plugins", "sync", "ui",
        ]
        .contains(&area.as_str())
        || event.len() > 64
        || !event
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || code.as_ref().is_some_and(|value| {
            value.len() > 48
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err("Ungültiges Diagnoseereignis.".into());
    }
    logging::event(&level, &area, &event, serde_json::json!({"code":code}));
    Ok(())
}

#[tauri::command]
fn local_calendar_snapshot(
    store: State<'_, storage::Store>,
    account_id: String,
) -> Result<calendar::Snapshot, String> {
    let config = config::TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, sync::CALENDARS)?;
    calendar::read(&store, &sync::scope(&config, &account_id))
}

#[tauri::command]
async fn sync_calendar(
    store: State<'_, storage::Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    require_network(&network)?;
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Ein Abgleich läuft bereits.")?;
    let config = config::TestConfig::load()?;
    let connection = sync::Connection::discover_for(&config, &account_id, sync::CALENDARS)
        .await
        .map_err(|error| error.message())?;
    calendar::synchronize(
        &store,
        &connection,
        &sync::scope(&config, &account_id),
        &account_id,
    )
    .await
    .map_err(|error| error.message())?;
    if network.enabled() {
        store.set_change_review(&sync::scope(&config, &account_id), false, false)?;
    }
    Ok(())
}

#[tauri::command]
fn local_mail(
    store: State<'_, storage::Store>,
    account_id: String,
) -> Result<replica::LocalMail, String> {
    let config = config::TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    store.local_mail(&sync::scope(&config, &account_id))
}

#[tauri::command]
fn queue_keyword(
    store: State<'_, storage::Store>,
    account_id: String,
    email_id: String,
    keyword: String,
    enabled: bool,
) -> Result<(), String> {
    let config = config::TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    if config.read_only {
        return Err("Nur-Lese-Modus: Änderungen sind gesperrt.".into());
    }
    store.queue_keyword(
        &sync::scope(&config, &account_id),
        &email_id,
        &keyword,
        enabled,
    )
}

fn blob_reference(
    store: &storage::Store,
    config: &config::TestConfig,
    account: &str,
    blob: &str,
) -> Result<(blobs::BlobRef, &'static str), String> {
    let scope = sync::scope(config, account);
    if desktop::require_capability(store, config, account, "urn:ietf:params:jmap:mail").is_ok() {
        if let Some(reference) = blobs::references(&store.local_mail(&scope)?)
            .into_iter()
            .find(|r| r.id == blob)
        {
            return Ok((reference, "urn:ietf:params:jmap:mail"));
        }
    }
    if desktop::require_capability(store, config, account, sync::FILES).is_ok() {
        if let Some(file) = writes::files(store, &scope)?["list"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|file| file["blobId"] == blob && file["myRights"]["mayRead"] != false)
        {
            return Ok((
                blobs::BlobRef {
                    id: blob.into(),
                    media_type: file["type"]
                        .as_str()
                        .unwrap_or("application/octet-stream")
                        .into(),
                    size: file["size"].as_u64(),
                    attachment: true,
                },
                sync::FILES,
            ));
        }
    }
    Err("Datei gehört zu keinem lokal verfügbaren Objekt mit Leserecht.".into())
}
#[tauri::command]
fn local_blob(
    store: State<'_, storage::Store>,
    account_id: String,
    blob_id: String,
) -> Result<Option<blobs::CachedBlob>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let config = config::TestConfig::load()?;
    let scope = sync::scope(&config, &account_id);
    if blob_id.starts_with(uploads::PREFIX) {
        desktop::require_capability(&store, &config, &account_id, "urn:ietf:params:jmap:mail")
            .or_else(|_| desktop::require_capability(&store, &config, &account_id, sync::FILES))?;
        return uploads::cached(&store, &scope, &blob_id).map(Some);
    }
    let (reference, capability) = blob_reference(&store, &config, &account_id, &blob_id)?;
    if capability == "urn:ietf:params:jmap:mail" {
        if let Some(cached) = store.cached_blob(&scope, &blob_id)? {
            return Ok(Some(cached));
        }
    }
    if let Some(bytes) = store.blob_bytes(&scope, &blob_id)? {
        return Ok(Some(blobs::CachedBlob {
            base64: STANDARD.encode(bytes),
            media_type: reference.media_type,
        }));
    }
    uploads::cached_remote(&store, &scope, &blob_id)
}
#[tauri::command]
async fn sync_blob(
    store: State<'_, storage::Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
    account_id: String,
    blob_id: String,
) -> Result<(), String> {
    require_network(&network)?;
    let _guard = gate.0.lock().await;
    require_network(&network)?;
    let config = config::TestConfig::load()?;
    let scope = sync::scope(&config, &account_id);
    let (reference, capability) = blob_reference(&store, &config, &account_id, &blob_id)?;
    if store.blob_bytes(&scope, &blob_id)?.is_some() {
        return Ok(());
    }
    let connection = sync::Connection::discover_for(&config, &account_id, capability)
        .await
        .map_err(|e| e.message())?;
    let bytes = connection
        .download_blob(&account_id, &reference)
        .await
        .map_err(|e| e.message())?;
    require_network(&network)?;
    store.cache_blob(&scope, &blob_id, &bytes)
}

#[tauri::command]
async fn sync_mail(
    store: State<'_, storage::Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    require_network(&network)?;
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Ein Abgleich läuft bereits.")?;
    let config = config::TestConfig::load()?;
    let connection = sync::Connection::discover(&config, &account_id)
        .await
        .map_err(|error| {
            let _ = hold_offline_changes(&store);
            report_sync_error("sync", "mail_discovery", error)
        })?;
    sync::synchronize_with_policy(
        &store,
        &connection,
        &sync::scope(&config, &account_id),
        &account_id,
        config.read_only,
    )
    .await
    .map_err(|error| {
        let _ = hold_offline_changes(&store);
        report_sync_error("sync", "mail", error)
    })?;
    if network.enabled() {
        store.set_change_review(&sync::scope(&config, &account_id), false, false)?;
    }
    let synchronized = store.server_emails(&sync::scope(&config, &account_id))?;
    journal::clear_applied_sends_after_readback(
        &store,
        &sync::scope(&config, &account_id),
        &synchronized,
    )?;
    // A bounded, best-effort pass must not turn an attachment failure into a mail-sync failure.
    let scope = sync::scope(&config, &account_id);
    let prefetch = async {
        let mut remaining = 4 * 1024 * 1024;
        let mut attempted = 0;
        for reference in blobs::references(&store.local_mail(&scope)?) {
            if !network.enabled() {
                break;
            }
            if !reference.attachment
                || !reference
                    .size
                    .is_some_and(|size| size <= 1024 * 1024 && size <= remaining as u64)
                || store.blob_bytes(&scope, &reference.id)?.is_some()
            {
                continue;
            }
            attempted += 1;
            if let Ok(bytes) = connection.download_blob(&account_id, &reference).await {
                if !network.enabled() {
                    break;
                }
                remaining -= bytes.len();
                if store.cache_blob(&scope, &reference.id, &bytes).is_err() {
                    break;
                }
            }
            if attempted >= 4 {
                break;
            }
        }
        Ok::<(), String>(())
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), prefetch).await;
    Ok(())
}

#[tauri::command]
async fn sync_mailboxes(
    store: State<'_, storage::Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    require_network(&network)?;
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Ein Abgleich läuft bereits.")?;
    let config = config::TestConfig::load()?;
    let connection = sync::Connection::discover(&config, &account_id)
        .await
        .map_err(|error| report_sync_error("sync", "mailbox_discovery", error))?;
    sync::pull_mailboxes(
        &store,
        &connection,
        &sync::scope(&config, &account_id),
        &account_id,
    )
    .await
    .map_err(|error| report_sync_error("sync", "mailboxes", error))
}

#[tauri::command]
fn read_only_mode() -> Result<bool, String> {
    Ok(config::TestConfig::load()?.read_only)
}

#[tauri::command]
fn cached_session(store: State<'_, storage::Store>) -> Result<Option<SessionSummary>, String> {
    let config = config::TestConfig::load()?;
    store
        .get(&config.cache_key())?
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|_| "Gespeicherte Kontoinformationen sind ungültig.".into())
        })
        .transpose()
}

#[tauri::command]
async fn probe_session(store: State<'_, storage::Store>) -> Result<SessionSummary, String> {
    let config = config::TestConfig::load()?;
    let summary = session::probe(&config).await?;
    let json = serde_json::to_string(&summary)
        .map_err(|_| "Kontoinformationen konnten nicht verarbeitet werden.")?;
    store.put(&config.cache_key(), &json)?;
    Ok(summary)
}

pub fn run() {
    let (network_mode, network_events) = NetworkMode::new();
    tauri::Builder::default()
        .register_uri_scheme_protocol("bulwark-plugin", |context, request| {
            let path = request.uri().path().trim_start_matches('/');
            // Serve raw HTML here: Tauri injects the host CSP into embedded HTML on Linux.
            // The isolated plugin realm deliberately has its own CSP, including eval for bundles.
            if path == "index.html" || path.is_empty() {
                return tauri::http::Response::builder()
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Content-Security-Policy", "default-src 'none'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src data: blob:; font-src 'self'; connect-src 'none'; object-src 'none'; base-uri 'none'; frame-src 'none'")
                    .body(br#"<!doctype html><html><head><meta charset="utf-8"><title>Bulwark plugin sandbox</title><link rel="stylesheet" href="/plugin-sandbox-assets/kit.css"><script type="module" src="/plugin-sandbox-assets/runtime.js"></script></head><body style="margin:0;background:transparent"><div id="plugin-sandbox-root"></div></body></html>"#.to_vec()).unwrap();
            }
            let asset_path = if path == "index.html" || path.is_empty() {
                Some("plugin-sandbox/index.html".to_string())
            } else if path.starts_with("plugin-sandbox-assets/")
                && path.strip_prefix("plugin-sandbox-assets/").is_some_and(|name| !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))) {
                Some(path.to_string())
            } else { None };
            let asset = asset_path.and_then(|path| context.app_handle().asset_resolver().get(path));
            match asset {
                Some(asset) => tauri::http::Response::builder()
                    .header("Content-Type", asset.mime_type)
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Content-Security-Policy", "default-src 'none'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src data: blob:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-src 'none'")
                    .body(asset.bytes).unwrap(),
                None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap(),
            }
        })
        .manage(SyncGate::default())
        .manage(network_mode)
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            std::fs::create_dir_all(&directory)?;
            logging::init(&directory)?;
            let store = storage::Store::open(&directory.join("jmap.redb"))
                .map_err(std::io::Error::other)?;
            accounts::initialize(&store).map_err(std::io::Error::other)?;
            app.manage(store);
            push::start(app.handle().clone(), network_events);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cached_session,
            probe_session,
            has_account_configuration,
            setup_account,
            profile_inventory,
            create_profile,
            add_profile_account,
            switch_profile,
            switch_profile_account,
            set_network_enabled,
            push_status,
            local_mail,
            queue_keyword,
            sync_mail,
            local_blob,
            sync_blob,
            sync_mailboxes,
            read_only_mode,
            write_status,
            pending_changes,
            changes::change_details,
            changes::resolve_change,
            approve_pending_changes,
            writes::stage_upload,
            writes::local_identities,
            writes::sync_identities,
            writes::queue_submission,
            writes::edit_queued_send,
            writes::local_object_set,
            writes::local_files,
            writes::sync_files,
            writes::sync_pending_changes,
            log_frontend_event,
            local_calendar_snapshot,
            sync_calendar,
            desktop::desktop_bootstrap,
            desktop::refresh_desktop_session,
            desktop::local_jmap,
            desktop::desktop_kv_get,
            desktop::desktop_kv_put,
            desktop::desktop_kv_delete,
            desktop::desktop_kv_keys
        ])
        .run(tauri::generate_context!())
        .expect("Herakles Mail konnte nicht gestartet werden");
}
