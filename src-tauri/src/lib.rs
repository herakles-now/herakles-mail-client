mod accounts;
mod blobs;
mod calendar;
mod config;
mod desktop;
mod logging;
mod push;
mod replica;
mod session;
mod storage;
mod sync;

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
fn set_network_enabled(enabled: bool, mode: State<'_, NetworkMode>) -> Result<(), String> {
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
    Ok(
        serde_json::json!({"pending":pending,"blocked":blocked,"pendingDeletes":pending_deletes,"blockedDeletes":blocked_deletes}),
    )
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
    .map_err(|error| error.message())
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

#[tauri::command]
fn local_blob(
    store: State<'_, storage::Store>,
    account_id: String,
    blob_id: String,
) -> Result<Option<blobs::CachedBlob>, String> {
    let config = config::TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    store.cached_blob(&sync::scope(&config, &account_id), &blob_id)
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
    desktop::require_capability(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    let scope = sync::scope(&config, &account_id);
    let reference = blobs::references(&store.local_mail(&scope)?)
        .into_iter()
        .find(|item| item.id == blob_id)
        .ok_or("Anhang gehoert zu keiner lokal verfuegbaren Nachricht.")?;
    if store.blob_bytes(&scope, &blob_id)?.is_some() {
        return Ok(());
    }
    let connection = sync::Connection::discover(&config, &account_id)
        .await
        .map_err(|error| error.message())?;
    let bytes = connection
        .download_blob(&account_id, &reference)
        .await
        .map_err(|error| error.message())?;
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
        .map_err(|error| report_sync_error("sync", "mail_discovery", error))?;
    sync::synchronize_with_policy(
        &store,
        &connection,
        &sync::scope(&config, &account_id),
        &account_id,
        config.read_only,
    )
    .await
    .map_err(|error| report_sync_error("sync", "mail", error))?;
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
async fn composer_request(
    store: State<'_, storage::Store>,
    network: State<'_, NetworkMode>,
    method_calls: Vec<(String, serde_json::Value, String)>,
) -> Result<serde_json::Value, String> {
    require_network(&network)?;
    let config = config::TestConfig::load()?;
    let response = match sync::composer_request(&store, &config, &method_calls).await {
        Ok(response) => response,
        Err(error) => {
            logging::event(
                "error",
                "composer",
                "request_failed",
                serde_json::json!({"reason":error.message()}),
            );
            return Err(error.message());
        }
    };
    if let Some(rows) = response["methodResponses"].as_array() {
        for row in rows {
            let method = row[0].as_str().unwrap_or("unknown");
            let method_error = row[1]["type"].as_str().or_else(|| {
                row[1]["notCreated"]
                    .as_object()
                    .and_then(|errors| errors.values().next())
                    .and_then(|item| item["type"].as_str())
            });
            if method_error.is_some() {
                logging::event(
                    "error",
                    "composer",
                    "jmap_method_failed",
                    serde_json::json!({"method":method,"code":method_error}),
                );
            } else if method == "EmailSubmission/set"
                && row[1]["created"]
                    .as_object()
                    .is_some_and(|created| !created.is_empty())
            {
                logging::event(
                    "info",
                    "composer",
                    "submission_accepted",
                    serde_json::json!({}),
                );
            }
        }
    }
    Ok(response)
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
            composer_request,
            read_only_mode,
            write_status,
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
