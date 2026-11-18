use crate::{
    calendar,
    config::TestConfig,
    desktop,
    journal::{self, Calls},
    storage::Store,
    sync::{self, Remote},
    NetworkMode, SyncGate,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tauri::State;

pub(crate) fn writable(
    store: &Store,
    config: &TestConfig,
    account: &str,
    capability: &str,
) -> Result<(), String> {
    if config.read_only {
        return Err("Nur-Lese-Modus: Änderungen sind gesperrt.".into());
    }
    desktop::require_capability(store, config, account, capability)?;
    let session: Value = serde_json::from_str(
        &store
            .get(&format!("{}:desktop-session", config.cache_key()))?
            .ok_or("accountNotFound")?,
    )
    .map_err(|_| "Lokale Sitzung ist ungültig.")?;
    if session["accounts"][account]["isReadOnly"] == true {
        return Err("forbidden".into());
    }
    Ok(())
}
#[tauri::command]
pub fn stage_upload(
    store: State<'_, Store>,
    account_id: String,
    base64: String,
    media_type: String,
) -> Result<crate::uploads::Upload, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let config = TestConfig::load()?;
    writable(&store, &config, &account_id, "urn:ietf:params:jmap:mail")
        .or_else(|_| writable(&store, &config, &account_id, sync::FILES))?;
    if base64.len() > crate::blobs::MAX_BLOB_BYTES.div_ceil(3) * 4 {
        return Err("Datei überschreitet das Uploadlimit von 25 MiB.".into());
    }
    let bytes = STANDARD
        .decode(base64)
        .map_err(|_| "Ungültige Dateidaten.")?;
    crate::uploads::stage(
        &store,
        &sync::scope(&config, &account_id),
        &bytes,
        &media_type,
    )
}
#[tauri::command]
pub fn edit_queued_send(
    store: State<'_, Store>,
    account_id: String,
    journal_id: u64,
) -> Result<String, String> {
    let config = TestConfig::load()?;
    writable(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    journal::edit_send(&store, &sync::scope(&config, &account_id), journal_id)
}
#[tauri::command]
pub fn local_identities(store: State<'_, Store>, account_id: String) -> Result<Value, String> {
    let config = TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, sync::SUBMISSION)?;
    let list: Value = store
        .get(&format!("identities:{}", sync::scope(&config, &account_id)))?
        .map(|s| {
            serde_json::from_str(&s).map_err(|_| "Lokale Identitäten sind ungültig.".to_owned())
        })
        .transpose()?
        .unwrap_or(json!([]));
    Ok(json!({"accountId":account_id,"state":"local","list":list,"notFound":[]}))
}
#[tauri::command]
pub async fn sync_identities(
    store: State<'_, Store>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    crate::require_network(&network)?;
    let config = TestConfig::load()?;
    let connection = sync::Connection::discover_for(&config, &account_id, sync::SUBMISSION)
        .await
        .map_err(|e| e.message())?;
    let response = match connection
        .call("Identity/get", json!({"accountId":account_id}))
        .await
    {
        Ok(value) => value,
        Err(sync::SyncError::Method(kind))
            if ["accountNotSupportedByMethod", "forbidden", "unknownMethod"]
                .contains(&kind.as_str()) =>
        {
            json!({"list":[]})
        }
        Err(error) => return Err(error.message()),
    };
    if !response["list"].is_array() {
        return Err("Ungültige Identitätsliste.".into());
    }
    store.put(
        &format!("identities:{}", sync::scope(&config, &account_id)),
        &response["list"].to_string(),
    )
}
#[tauri::command]
pub async fn queue_draft_sync(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    account_id: String,
    draft_id: String,
) -> Result<(), String> {
    let _guard = gate.0.lock().await;
    let config = TestConfig::load()?;
    writable(&store, &config, &account_id, "urn:ietf:params:jmap:mail")?;
    journal::queue_draft(
        &store,
        &sync::scope(&config, &account_id),
        &account_id,
        &draft_id,
    )
}

#[tauri::command]
pub async fn queue_submission(
    store: State<'_, Store>,
    network: State<'_, NetworkMode>,
    gate: State<'_, SyncGate>,
    method_calls: Calls,
    draft_id: Option<String>,
    draft_account_id: Option<String>,
) -> Result<Value, String> {
    let config = TestConfig::load()?;
    if method_calls.len() != 3
        || method_calls[0].0 != "Email/set"
        || method_calls[1].0 != "EmailSubmission/set"
        || method_calls[2].0 != "EmailSubmission/get"
    {
        return Err("Ungültiger Versandauftrag.".into());
    }
    let email_create = method_calls[0].1["create"]
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or("Ungültige Nachricht.")?;
    let email_key = email_create.keys().next().ok_or("Ungültige Nachricht.")?;
    if method_calls[0].1.get("update").is_some()
        || method_calls[0].1.get("destroy").is_some()
        || method_calls[1].1["create"]
            .as_object()
            .is_none_or(|m| m.len() != 1 || !m.contains_key("1"))
        || method_calls[1].1["create"]["1"]["emailId"] != format!("#{email_key}")
        || method_calls[1].1.get("update").is_some()
        || method_calls[1].1.get("destroy").is_some()
        || method_calls[1].1["onSuccessDestroyEmail"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id != "#1"))
        || method_calls[1].1["onSuccessUpdateEmail"]
            .as_object()
            .is_some_and(|m| m.keys().any(|id| id != "#1"))
    {
        return Err("Ungültiger Versandauftrag.".into());
    }
    let account = method_calls[0].1["accountId"]
        .as_str()
        .ok_or("accountNotFound")?
        .to_owned();
    let submission_account = method_calls[1].1["accountId"]
        .as_str()
        .ok_or("accountNotFound")?
        .to_owned();
    writable(&store, &config, &account, "urn:ietf:params:jmap:mail")?;
    for call in &method_calls[1..] {
        writable(
            &store,
            &config,
            call.1["accountId"].as_str().ok_or("accountNotFound")?,
            sync::SUBMISSION,
        )?;
    }
    let scope = sync::scope(&config, &account);
    for (_, args, _) in &method_calls {
        crate::uploads::validate(&store, &scope, args)?;
    }
    let source_account = draft_account_id.as_deref().unwrap_or(&account);
    if draft_id.is_some() {
        desktop::require_capability(&store, &config, source_account, "urn:ietf:params:jmap:mail")?;
    }
    // Online sends use the native transport immediately so the upstream
    // composer receives the real JMAP response. Offline/review-gated sends
    // stay durably queued for the user's approval.
    let _send_guard = gate.0.lock().await;
    if let Some(id) = draft_id
        .as_deref()
        .filter(|id| id.starts_with("local-draft-"))
    {
        let draft_scope = sync::scope(&config, source_account);
        if journal::entries(&store, &draft_scope)?
            .iter()
            .any(|entry| entry.kind == "draft" && entry.object_id == id)
        {
            return Err(
                "Zuerst den vorgemerkten Entwurfsabgleich abschließen oder verwerfen.".into(),
            );
        }
    }
    let hold = !network.enabled() || store.metadata(&scope)?.review_new_changes;
    let id = journal::enqueue_send(
        &store,
        &scope,
        method_calls,
        hold,
        draft_id.as_deref(),
        Some(source_account),
    )?;
    // An ordinary local draft is retained until the queued send is durably stored.
    if let Some(id) = draft_id.filter(|id| id.starts_with("local-draft-")) {
        let _ = store.delete_local_draft(&sync::scope(&config, source_account), &id);
    }
    if !network.enabled() {
        journal::hold(&store, &scope)?;
        return Ok(json!({"scheduled":false,"queued":true,"outboxId":id}));
    }
    if !hold {
        if !network.enabled() {
            journal::hold(&store, &scope)?;
            return Ok(json!({"scheduled":false,"queued":true,"outboxId":id}));
        }
        // Upload dependencies before claiming the send. Failed uploads cannot
        // leave a possibly submitted mail; retain the durable held operation.
        let queued_calls = journal::entries(&store, &scope)?
            .into_iter()
            .find(|e| e.id == id)
            .ok_or("Versandauftrag fehlt.")?
            .calls;
        let prepared = async {
            let remote =
                sync::Connection::discover_for(&config, &account, "urn:ietf:params:jmap:mail")
                    .await
                    .map_err(|e| e.message())?;
            crate::uploads::prepare(&store, &scope, &account, &queued_calls, &remote, || {
                network.enabled()
            })
            .await
        }
        .await;
        let calls = match prepared {
            Ok(calls) => calls,
            Err(_) => {
                journal::finish(
                    &store,
                    &scope,
                    id,
                    Some("held"),
                    Some("Vorbereitung fehlgeschlagen; Anhänge und Verbindung prüfen."),
                )?;
                return Ok(json!({"scheduled":false,"queued":true,"outboxId":id}));
            }
        };
        if !journal::claim(&store, &scope, id)? {
            return Err("Der Versandauftrag konnte nicht sicher übernommen werden.".into());
        }
        match sync::composer_request(&store, &config, &calls).await {
            Ok(answer) => {
                let rows = answer["methodResponses"].as_array();
                let accepted = rows.is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row[0] == "EmailSubmission/set"
                            && row[1]["created"].as_object().is_some_and(|created| {
                                !created.is_empty()
                                    && created.values().all(|item| item["id"].is_string())
                            })
                    })
                });
                let rejected_recipient = rows.is_some_and(|rows| {
                    rows.iter()
                        .filter(|row| row[0] == "EmailSubmission/get")
                        .flat_map(|row| row[1]["list"].as_array().into_iter().flatten())
                        .flat_map(|item| {
                            item["deliveryStatus"]
                                .as_object()
                                .into_iter()
                                .flat_map(|map| map.values())
                        })
                        .any(|status| status["delivered"] == "no")
                });
                let definitive_error = rows.is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row[0] == "error"
                            || row[1]["notCreated"]
                                .as_object()
                                .is_some_and(|map| !map.is_empty())
                    })
                });
                if accepted && !rejected_recipient {
                    journal::applied(&store, &scope, id, None)?;
                    let submission_ids: Vec<_> = rows
                        .into_iter()
                        .flatten()
                        .filter(|row| row[0] == "EmailSubmission/set")
                        .flat_map(|row| {
                            row[1]["created"]
                                .as_object()
                                .into_iter()
                                .flat_map(|map| map.values())
                        })
                        .filter_map(|item| item["id"].as_str())
                        .collect();
                    if !submission_ids.is_empty() {
                        let cleanup = vec![(
                            "EmailSubmission/set".into(),
                            json!({"accountId":submission_account,"destroy":submission_ids}),
                            "cleanup".into(),
                        )];
                        let _ = sync::composer_request(&store, &config, &cleanup).await;
                    }
                } else if definitive_error && !accepted {
                    journal::finish(
                        &store,
                        &scope,
                        id,
                        Some("blocked"),
                        Some("Der Server hat den Versand abgelehnt."),
                    )?;
                } else {
                    journal::finish(
                        &store,
                        &scope,
                        id,
                        Some("uncertain"),
                        Some("Versandstatus unklar; vor erneutem Senden prüfen."),
                    )?;
                }
                Ok(answer)
            }
            Err(_) => {
                journal::finish(
                    &store,
                    &scope,
                    id,
                    Some("uncertain"),
                    Some("Versandstatus unklar; vor erneutem Senden prüfen."),
                )?;
                Err("Versandstatus unklar. Bitte vor einem erneuten Senden die ausstehenden Aktionen prüfen.".into())
            }
        }
    } else {
        Ok(json!({"scheduled":false,"queued":true,"outboxId":id}))
    }
}

#[tauri::command]
pub fn local_object_set(
    store: State<'_, Store>,
    network: State<'_, NetworkMode>,
    method: String,
    args: Value,
) -> Result<Value, String> {
    let config = TestConfig::load()?;
    local_object_set_inner(&store, &config, network.enabled(), method, args)
}
pub(crate) fn local_object_set_inner(
    store: &Store,
    config: &TestConfig,
    network_enabled: bool,
    method: String,
    args: Value,
) -> Result<Value, String> {
    let account = args["accountId"].as_str().ok_or("accountNotFound")?;
    let scope = sync::scope(config, account);
    let hold = !network_enabled || store.metadata(&scope)?.review_new_changes;
    match method.as_str() {
        "CalendarEvent/set" => {
            writable(&store, &config, account, sync::CALENDARS)?;
            let snapshot = calendar::read(&store, &scope)?;
            if snapshot.access_denied {
                return Err("forbidden".into());
            }
            if let Some(state) = args.get("ifInState").filter(|v| !v.is_null()) {
                let expected = format!(
                    "local-calendar:{}:{}:{}",
                    snapshot.event_state.as_deref().unwrap_or("empty"),
                    snapshot.calendar_state.as_deref().unwrap_or(""),
                    snapshot.local_revision
                );
                if state.as_str() != Some(expected.as_str()) {
                    return Err("stateMismatch".into());
                }
            }
            // Validate source and destination rights before publishing any optimistic result.
            let check = |event: &Value| -> Result<(), String> {
                let ids = event["calendarIds"]
                    .as_object()
                    .ok_or("invalidProperties")?;
                if ids.is_empty() || ids.values().any(|v| v != true) {
                    return Err("invalidProperties".into());
                }
                for id in ids.keys() {
                    if !snapshot
                        .calendars
                        .iter()
                        .any(|c| c["id"] == *id && c["myRights"]["mayWriteAll"] == true)
                    {
                        return Err("forbidden".into());
                    }
                }
                Ok(())
            };
            for event in args["create"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
            {
                check(event)?;
            }
            for (id, patch) in args["update"].as_object().into_iter().flatten() {
                let mut event = snapshot.events.get(id).cloned().ok_or("notFound")?;
                check(&event)?;
                let previous_sequence = event["sequence"].as_u64().unwrap_or(0);
                journal::patch(&mut event, patch)?;
                // Explicit incoming versions must never roll a meeting back.
                // Ordinary edits without sequence retain the stored version.
                if patch.get("sequence").is_some()
                    && event["sequence"]
                        .as_u64()
                        .is_none_or(|s| s < previous_sequence)
                {
                    return Err("invalidProperties".into());
                }
                check(&event)?;
            }
            for id in args["destroy"].as_array().into_iter().flatten() {
                check(
                    snapshot
                        .events
                        .get(id.as_str().ok_or("invalidArguments")?)
                        .ok_or("notFound")?,
                )?;
            }
            journal::queue_object(
                &store,
                &scope,
                "calendar",
                &method,
                args,
                &snapshot.events,
                snapshot.event_state,
                hold,
            )
        }
        "FileNode/set" => {
            writable(&store, &config, account, sync::FILES)?;
            let snapshot = files(&store, &scope)?;
            let objects: BTreeMap<String, Value> = snapshot["list"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((v["id"].as_str()?.to_owned(), v.clone())))
                .collect();
            let check_parent = |parent: &Value| -> Result<(), String> {
                if parent.is_null() {
                    return Ok(());
                }
                let parent = objects
                    .get(parent.as_str().ok_or("invalidProperties")?)
                    .ok_or("notFound")?;
                if parent["myRights"]["mayAddChildren"] != true
                    && parent["myRights"]["mayWrite"] != true
                {
                    return Err("forbidden".into());
                }
                Ok(())
            };
            for (id, patch) in args["update"].as_object().into_iter().flatten() {
                let file = objects.get(id).ok_or("notFound")?;
                let keys = patch.as_object().ok_or("invalidPatch")?;
                if keys
                    .keys()
                    .any(|k| !["name", "parentId"].contains(&k.as_str()))
                {
                    return Err("Dateiänderung wird noch nicht unterstützt.".into());
                }
                if file["myRights"]["mayWrite"] != true && file["myRights"]["mayRename"] != true {
                    return Err("forbidden".into());
                }
                if let Some(parent) = keys.get("parentId") {
                    if file["myRights"]["mayDelete"] != true && file["myRights"]["mayWrite"] != true
                    {
                        return Err("forbidden".into());
                    }
                    check_parent(parent)?;
                    let mut ancestor = parent.as_str();
                    let mut visited = std::collections::BTreeSet::new();
                    while let Some(current) = ancestor {
                        if current == id || !visited.insert(current.to_owned()) {
                            return Err("invalidProperties".into());
                        }
                        ancestor = objects.get(current).and_then(|o| o["parentId"].as_str());
                    }
                }
            }
            for file in args["create"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
            {
                if let Some(blob) = file.get("blobId").filter(|v| !v.is_null()) {
                    blob.as_str()
                        .filter(|id| id.starts_with(crate::uploads::PREFIX))
                        .ok_or("Nur lokal gespeicherte Datei-Uploads werden unterstützt.")?;
                    crate::uploads::validate(&store, &scope, file)?;
                } else if file["@type"].as_str().is_some_and(|s| s != "Directory")
                    || file["nodeType"].as_str().is_some_and(|s| s != "directory")
                {
                    return Err("Datei-Inhalt fehlt.".into());
                }
                check_parent(&file["parentId"])?;
            }
            for id in args["destroy"].as_array().into_iter().flatten() {
                let file = objects
                    .get(id.as_str().ok_or("invalidArguments")?)
                    .ok_or("notFound")?;
                if file["myRights"]["mayDelete"] != true && file["myRights"]["mayWrite"] != true {
                    return Err("forbidden".into());
                }
            }
            journal::queue_object(
                &store,
                &scope,
                "file",
                &method,
                args,
                &objects,
                snapshot["state"].as_str().map(str::to_owned),
                hold,
            )
        }
        _ => Err("unknownMethod".into()),
    }
}
pub fn files(store: &Store, scope: &str) -> Result<Value, String> {
    let mut snapshot: Value = store
        .get(&format!("files:{scope}"))?
        .map(|s| serde_json::from_str(&s).map_err(|_| "Lokale Dateien sind ungültig.".to_owned()))
        .transpose()?
        .unwrap_or(json!({"list":[],"state":null}));
    let mut objects: BTreeMap<String, Value> = snapshot["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["id"].as_str()?.to_owned(), v.clone())))
        .collect();
    journal::overlay(store, scope, "file", &mut objects)?;
    snapshot["localRevision"] = json!(journal::revision(store, scope)?);
    snapshot["list"] = json!(objects.values().collect::<Vec<_>>());
    Ok(snapshot)
}
#[tauri::command]
pub fn local_files(store: State<'_, Store>, account_id: String) -> Result<Value, String> {
    let config = TestConfig::load()?;
    desktop::require_capability(&store, &config, &account_id, sync::FILES)?;
    files(&store, &sync::scope(&config, &account_id))
}
#[tauri::command]
pub async fn sync_files(
    store: State<'_, Store>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    crate::require_network(&network)?;
    let config = TestConfig::load()?;
    let remote = sync::Connection::discover_for(&config, &account_id, sync::FILES)
        .await
        .map_err(|e| e.message())?;
    let response = match remote
        .call("FileNode/get", json!({"accountId":account_id,"ids":null}))
        .await
    {
        Ok(value) => value,
        Err(sync::SyncError::Method(kind))
            if ["accountNotSupportedByMethod", "forbidden", "unknownMethod"]
                .contains(&kind.as_str()) =>
        {
            json!({"list":[],"state":"denied"})
        }
        Err(error) => return Err(error.message()),
    };
    if !response["list"].is_array() || !response["notFound"].as_array().is_none_or(|v| v.is_empty())
    {
        return Err("Dateiabgleich ist unvollständig.".into());
    }
    store.put(
        &format!("files:{}", sync::scope(&config, &account_id)),
        &response.to_string(),
    )?;
    if network.enabled() {
        store.set_change_review(&sync::scope(&config, &account_id), false, false)?;
    }
    journal::clear_applied(&store, &sync::scope(&config, &account_id), "file")
}

#[tauri::command]
pub async fn sync_pending_changes(
    store: State<'_, Store>,
    network: State<'_, NetworkMode>,
    gate: State<'_, SyncGate>,
) -> Result<bool, String> {
    crate::require_network(&network)?;
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Ein Abgleich läuft bereits.")?;
    let config = TestConfig::load()?;
    sync_pending_changes_inner(&store, &config, &network).await
}
pub(crate) async fn sync_pending_changes_inner(
    store: &Store,
    config: &TestConfig,
    network: &NetworkMode,
) -> Result<bool, String> {
    crate::require_network(network)?;
    if config.read_only {
        return Ok(false);
    }
    let mut changed = false;
    for account in crate::all_change_accounts(&store, &config)? {
        let scope = sync::scope(&config, &account);
        for queued in journal::entries(&store, &scope)? {
            let Some(entry) = journal::entries(&store, &scope)?
                .into_iter()
                .find(|e| e.id == queued.id)
            else {
                continue;
            };
            if !network.enabled() {
                return Ok(changed);
            }
            if entry.status != "ready" || entry.kind == "contact" {
                continue;
            }
            if entry.kind == "send" {
                let submission_account = entry.calls[1].1["accountId"]
                    .as_str()
                    .ok_or("accountNotFound")?;
                let identity_remote = match sync::Connection::discover_for(
                    &config,
                    submission_account,
                    sync::SUBMISSION,
                )
                .await
                {
                    Ok(remote) => remote,
                    Err(_) => {
                        journal::hold(&store, &scope)?;
                        continue;
                    }
                };
                let identities = match identity_remote
                    .call("Identity/get", json!({"accountId":submission_account}))
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        journal::hold(&store, &scope)?;
                        continue;
                    }
                };
                let mut calls = entry.calls.clone();
                let sender = entry
                    .local
                    .as_ref()
                    .and_then(|v| v["from"][0]["email"].as_str())
                    .unwrap_or("");
                let wanted = calls[1].1["create"]["1"]["identityId"]
                    .as_str()
                    .unwrap_or("");
                let identity = identities["list"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| v["id"] == wanted)
                    .or_else(|| {
                        identities["list"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|v| {
                                v["email"]
                                    .as_str()
                                    .is_some_and(|s| s.eq_ignore_ascii_case(sender))
                            })
                    });
                let Some(identity) = identity else {
                    journal::finish(
                        &store,
                        &scope,
                        entry.id,
                        Some("blocked"),
                        Some("Absenderidentität ist nicht mehr verfügbar"),
                    )?;
                    continue;
                };
                calls[1].1["create"]["1"]["identityId"] = identity["id"].clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "Systemzeit ist ungültig.")?
                    .as_secs();
                let elapsed = now.saturating_sub(
                    entry
                        .local
                        .as_ref()
                        .and_then(|v| v["desktopQueuedAt"].as_u64())
                        .unwrap_or(now),
                );
                if let Some(parameters) =
                    calls[1].1["create"]["1"]["envelope"]["mailFrom"]["parameters"].as_object_mut()
                {
                    if let Some(seconds) = parameters
                        .get("HOLDFOR")
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        let remaining = seconds.saturating_sub(elapsed);
                        if remaining == 0 {
                            parameters.remove("HOLDFOR");
                        } else {
                            parameters.insert("HOLDFOR".into(), json!(remaining.to_string()));
                        }
                    }
                }
                let prepared = async {
                    let remote = sync::Connection::discover_for(
                        &config,
                        &account,
                        "urn:ietf:params:jmap:mail",
                    )
                    .await
                    .map_err(|e| e.message())?;
                    crate::uploads::prepare(&store, &scope, &account, &calls, &remote, || {
                        network.enabled()
                    })
                    .await
                }
                .await;
                calls = match prepared {
                    Ok(calls) => calls,
                    Err(_) => {
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            Some("held"),
                            Some("Upload fehlgeschlagen; Auftrag bleibt lokal."),
                        )?;
                        continue;
                    }
                };
                // Claim is durable BEFORE any potentially side-effecting network request.
                if !journal::claim(&store, &scope, entry.id)? {
                    continue;
                }
                let result = sync::composer_request(&store, &config, &calls).await;
                match result {
                    Ok(answer) => {
                        let rows = answer["methodResponses"].as_array();
                        let accepted = rows.is_some_and(|rows| {
                            rows.iter().any(|r| {
                                r[0] == "EmailSubmission/set"
                                    && r[1]["created"].as_object().is_some_and(|m| {
                                        !m.is_empty() && m.values().all(|v| v["id"].is_string())
                                    })
                            })
                        });
                        let refused = rows.is_some_and(|rows| {
                            rows.iter().any(|r| {
                                r[0] == "EmailSubmission/set"
                                    && r[1]["notCreated"]
                                        .as_object()
                                        .is_some_and(|m| !m.is_empty())
                            })
                        });
                        let rejected_recipient = rows.is_some_and(|rows| {
                            rows.iter()
                                .filter(|r| r[0] == "EmailSubmission/get")
                                .flat_map(|r| r[1]["list"].as_array().into_iter().flatten())
                                .flat_map(|v| {
                                    v["deliveryStatus"]
                                        .as_object()
                                        .into_iter()
                                        .flat_map(|m| m.values())
                                })
                                .any(|status| status["delivered"] == "no")
                        });
                        if accepted && rejected_recipient {
                            journal::finish(&store,&scope,entry.id,Some("uncertain"),Some("Empfänger teilweise oder vollständig abgelehnt; Zustellung vor erneutem Senden prüfen"))?;
                            continue;
                        }
                        if accepted {
                            journal::applied(&store, &scope, entry.id, None)?;
                            changed = true;
                            // Submission metadata cleanup runs only in the worker.
                            if let Some(rows) = rows {
                                let ids: Vec<_> = rows
                                    .iter()
                                    .filter(|r| r[0] == "EmailSubmission/set")
                                    .flat_map(|r| {
                                        r[1]["created"]
                                            .as_object()
                                            .into_iter()
                                            .flat_map(|m| m.values())
                                    })
                                    .filter_map(|v| v["id"].as_str())
                                    .collect();
                                if !ids.is_empty() {
                                    let cleanup = vec![(
                                        "EmailSubmission/set".into(),
                                        json!({"accountId":submission_account,"destroy":ids}),
                                        "cleanup".into(),
                                    )];
                                    let _ = sync::composer_request(&store, &config, &cleanup).await;
                                }
                            }
                            if let Some(local) = &entry.local {
                                if let Some(draft) = local["desktopSourceDraftId"]
                                    .as_str()
                                    .filter(|id| !id.starts_with("local-"))
                                {
                                    let source = local["desktopSourceDraftAccountId"]
                                        .as_str()
                                        .unwrap_or(&account);
                                    if writable(
                                        &store,
                                        &config,
                                        source,
                                        "urn:ietf:params:jmap:mail",
                                    )
                                    .is_ok()
                                    {
                                        let _ = store.queue_draft_delete(
                                            &sync::scope(&config, source),
                                            draft,
                                        );
                                    }
                                }
                            }
                            continue;
                        }
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            if accepted {
                                None
                            } else if refused {
                                Some("blocked")
                            } else {
                                Some("uncertain")
                            },
                            if accepted {
                                None
                            } else {
                                Some("Versandantwort prüfen")
                            },
                        )?;
                    }
                    Err(_) => journal::finish(
                        &store,
                        &scope,
                        entry.id,
                        Some("uncertain"),
                        Some("Versandstatus unklar; vor erneutem Senden prüfen"),
                    )?,
                }
            } else {
                let capability = if entry.kind == "draft" {
                    "urn:ietf:params:jmap:mail"
                } else if entry.kind == "calendar" {
                    sync::CALENDARS
                } else {
                    sync::FILES
                };
                let remote =
                    match sync::Connection::discover_for(&config, &account, capability).await {
                        Ok(r) => r,
                        Err(_) => {
                            journal::hold(&store, &scope)?;
                            continue;
                        }
                    };
                writable(&store, &config, &account, capability)?;
                if entry.kind == "draft" {
                    let mail = store.local_mail(&scope)?;
                    let mailbox = entry.calls[0].1["create"]["draft"]["mailboxIds"].as_object();
                    let valid = mailbox.is_some_and(|ids| {
                        ids.len() == 1
                            && mail.mailboxes.iter().any(|book| {
                                book["role"] == "drafts"
                                    && book["myRights"]["mayAddItems"] != false
                                    && book["id"]
                                        .as_str()
                                        .is_some_and(|id| ids.get(id) == Some(&Value::Bool(true)))
                            })
                    });
                    if !valid {
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            Some("blocked"),
                            Some("Der Entwurfsordner ist nicht mehr beschreibbar."),
                        )?;
                        continue;
                    }
                }
                let resolved = match journal::resolve_calls(&store, &scope, &entry.calls) {
                    Ok(calls) => calls,
                    Err(_) => {
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            Some("held"),
                            Some("Übergeordnete Änderung zuerst freigeben"),
                        )?;
                        continue;
                    }
                };
                let resolved = match crate::uploads::prepare(
                    &store,
                    &scope,
                    &account,
                    &resolved,
                    &remote,
                    || network.enabled(),
                )
                .await
                {
                    Ok(calls) => calls,
                    Err(_) => {
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            Some("held"),
                            Some("Upload fehlgeschlagen; Auftrag bleibt lokal."),
                        )?;
                        continue;
                    }
                };
                let (method, mut args, _) = resolved[0].clone();
                if !network.enabled() {
                    journal::hold(&store, &scope)?;
                    return Ok(changed);
                }
                if let Some(state) = &entry.base_state {
                    args["ifInState"] = json!(state);
                }
                changed |=
                    transmit_object_change(&store, &remote, &scope, &entry, &method, args).await?;
            }
        }
    }
    Ok(changed)
}

// One durable object operation per entry. An unrelated acknowledgement is
// never evidence that our calendar reply/update reached its target.
pub(crate) async fn transmit_object_change(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    entry: &journal::Entry,
    method: &str,
    args: Value,
) -> Result<bool, String> {
    if !journal::claim(store, scope, entry.id)? {
        return Ok(false);
    }
    match remote.call(method, args.clone()).await {
        Ok(response) => {
            let (action, key) = ["create", "update", "destroy"]
                .into_iter()
                .find_map(|action| {
                    let key = if action == "destroy" {
                        args[action].as_array()?.first()?.as_str()?.to_owned()
                    } else {
                        args[action].as_object()?.keys().next()?.clone()
                    };
                    Some((action, key))
                })
                .ok_or("invalidArguments")?;
            let confirmed = match action {
                "create" => response["created"][&key]["id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "update" => response["updated"]
                    .as_object()
                    .is_some_and(|rows| rows.contains_key(&key)),
                _ => response["destroyed"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|v| v == &key)),
            };
            let error_field = match action {
                "create" => "notCreated",
                "update" => "notUpdated",
                _ => "notDestroyed",
            };
            let rejected = response[error_field][&key].is_object();
            if confirmed
                && !rejected
                && response["newState"].is_string()
                && (!["contact", "draft"].contains(&entry.kind.as_str())
                    || response["accountId"] == args["accountId"])
            {
                if entry.kind == "draft" && action == "create" {
                    journal::confirm_draft(
                        store,
                        scope,
                        entry.id,
                        response["created"][&key]["id"].as_str().unwrap(),
                    )?;
                    return Ok(true);
                }
                if action == "create" {
                    journal::remember_created(
                        store,
                        scope,
                        &entry.object_id,
                        response["created"][&key]["id"].as_str().unwrap(),
                    )?;
                }
                journal::applied(store, scope, entry.id, response["newState"].as_str())?;
                return Ok(true);
            }
            let reason = if rejected && !confirmed {
                match response[error_field][&key]["type"].as_str() {
                    Some("forbidden") => {
                        "Kalender oder Objekt ist nicht mehr beschreibbar. Rechte prüfen."
                    }
                    Some("notFound") => {
                        "Der Termin oder das Objekt ist auf dem Server nicht mehr vorhanden."
                    }
                    Some("invalidProperties") => {
                        "Der Server hat die Termin- oder Objektdaten abgelehnt. Änderung prüfen."
                    }
                    _ => "Server hat die Änderung abgelehnt. Änderung prüfen.",
                }
            } else {
                "Serverantwort unvollständig oder widersprüchlich. Vor erneutem Übertragen den Serverstand prüfen."
            };
            journal::finish(
                store,
                scope,
                entry.id,
                Some(if rejected && !confirmed {
                    "blocked"
                } else {
                    "uncertain"
                }),
                Some(reason),
            )?;
        }
        Err(sync::SyncError::Method(kind)) => {
            let reason = match kind.as_str() {
                "stateMismatch" => "Der Kalender oder das Objekt wurde inzwischen auf dem Server geändert. Aktuellen Stand prüfen.",
                "forbidden" => "Kalender oder Objekt ist nicht mehr beschreibbar. Rechte prüfen.",
                _ => "Server hat die Änderung abgelehnt oder einen Konflikt erkannt.",
            };
            journal::finish(store, scope, entry.id, Some("blocked"), Some(reason))?;
        }
        Err(_) => journal::finish(
            store,
            scope,
            entry.id,
            Some("uncertain"),
            Some("Serverantwort unklar. Vor erneutem Übertragen den Serverstand prüfen."),
        )?,
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn draft_sync_requires_approval_and_exact_readback() {
        struct DraftRemote;
        impl Remote for DraftRemote {
            async fn call(&self, method: &str, args: Value) -> Result<Value, sync::SyncError> {
                assert_eq!(method, "Email/set");
                assert_eq!(args["create"]["draft"]["keywords"]["$draft"], true);
                assert!(args["create"]["draft"].get("id").is_none());
                Ok(
                    json!({"accountId":"a","newState":"e2","created":{"draft":{"id":"server-draft"}}}),
                )
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft-sync.redb");
        let store = Store::open(&path).unwrap();
        let draft =
            json!({"keywords":{"$draft":true},"mailboxIds":{"drafts":true},"subject":"Offline"});
        let id = store.save_local_draft("scope", draft.clone()).unwrap();
        journal::queue_draft(&store, "scope", "a", &id).unwrap();
        assert!(journal::queue_draft(&store, "scope", "a", &id).is_err());
        assert!(store.save_draft_version("scope", draft, Some(&id)).is_err());
        assert!(store.delete_local_draft("scope", &id).is_err());
        drop(store);
        let store = Store::open(&path).unwrap();
        let entry = journal::entries(&store, "scope").unwrap().remove(0);
        let (method, args, _) = entry.calls[0].clone();
        assert!(!transmit_object_change(
            &store,
            &DraftRemote,
            "scope",
            &entry,
            &method,
            args.clone()
        )
        .await
        .unwrap());
        journal::approve(&store, "scope", &[entry.id]).unwrap();
        assert!(
            transmit_object_change(&store, &DraftRemote, "scope", &entry, &method, args)
                .await
                .unwrap()
        );
        journal::clear_confirmed_drafts(
            &store,
            "scope",
            &[json!({"id":"other","keywords":{"$draft":true}})],
        )
        .unwrap();
        assert!(store.local_draft("scope", &id).is_ok());
        journal::clear_confirmed_drafts(
            &store,
            "scope",
            &[json!({"id":"server-draft","keywords":{"$draft":true}})],
        )
        .unwrap();
        assert!(store.local_draft("scope", &id).is_err());
        assert!(journal::entries(&store, "scope").unwrap().is_empty());
    }
    #[test]
    fn file_upload_create_is_durable_held_and_rejects_missing_or_foreign_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("files.redb");
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "synthetic".into(),
            password: String::new(),
            read_only: false,
        };
        let scope = sync::scope(&config, "files");
        let object;
        {
            let store = Store::open(&path).unwrap();
            store.put(&format!("{}:desktop-session",config.cache_key()), &json!({"accounts":{"files":{"accountCapabilities":{sync::FILES:{}},"isReadOnly":false}}}).to_string()).unwrap();
            store
                .put(
                    &format!("files:{scope}"),
                    &json!({"state":"f1","list":[]}).to_string(),
                )
                .unwrap();
            let foreign = crate::uploads::stage(&store, "other", &[1], "text/plain").unwrap();
            let args = |id: &str| json!({"accountId":"files","create":{"one":{"name":"test.txt","type":"text/plain","blobId":id}}});
            assert!(local_object_set_inner(
                &store,
                &config,
                false,
                "FileNode/set".into(),
                args(&foreign.blob_id)
            )
            .is_err());
            assert!(local_object_set_inner(
                &store,
                &config,
                false,
                "FileNode/set".into(),
                args("local-upload-missing")
            )
            .is_err());
            assert!(journal::entries(&store, &scope).unwrap().is_empty());
            let upload =
                crate::uploads::stage(&store, &scope, &[0, 128, 255], "text/plain").unwrap();
            let result = local_object_set_inner(
                &store,
                &config,
                false,
                "FileNode/set".into(),
                args(&upload.blob_id),
            )
            .unwrap();
            object = result["created"]["one"]["id"].as_str().unwrap().to_owned();
            assert_eq!(
                files(&store, &scope).unwrap()["list"][0]["parentId"],
                Value::Null
            );
            assert_eq!(files(&store, &scope).unwrap()["list"][0]["size"], 3);
            local_object_set_inner(
                &store,
                &config,
                false,
                "FileNode/set".into(),
                json!({"accountId":"files","update":{object.clone():{"name":"renamed.txt"}}}),
            )
            .unwrap();
            let entry = journal::entries(&store, &scope).unwrap().remove(0);
            assert_eq!(entry.status, "held");
            assert_eq!(entry.calls[0].1["create"]["one"]["blobId"], upload.blob_id);
            assert_eq!(entry.calls[0].1["create"]["one"]["name"], "renamed.txt");
            assert!(entry.calls[0].1["create"]["one"].get("myRights").is_none());
            assert!(entry.calls[0].1["create"]["one"].get("size").is_none());
            assert_eq!(
                files(&store, &scope).unwrap()["list"][0]["blobId"],
                upload.blob_id
            );
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(files(&store, &scope).unwrap()["list"][0]["id"], object);
        assert_eq!(journal::entries(&store, &scope).unwrap()[0].status, "held");
        let mut read_only = config;
        read_only.read_only = true;
        assert!(local_object_set_inner(
            &store,
            &read_only,
            false,
            "FileNode/set".into(),
            json!({"accountId":"files","destroy":[object]})
        )
        .is_err());
    }
    #[test]
    fn calendar_answers_require_exact_acknowledgement_and_never_replay_uncertain_results() {
        use std::sync::Mutex;
        struct ReplyRemote {
            response: Mutex<Option<Result<Value, sync::SyncError>>>,
            calls: Mutex<Vec<Value>>,
        }
        impl Remote for ReplyRemote {
            async fn call(&self, method: &str, args: Value) -> Result<Value, sync::SyncError> {
                assert_eq!(method, "CalendarEvent/set");
                self.calls.lock().unwrap().push(args);
                self.response
                    .lock()
                    .unwrap()
                    .take()
                    .expect("Unexpected replay")
            }
        }
        tauri::async_runtime::block_on(async {
            for (response, status) in [
                (Ok(json!({"updated":{"e":null},"newState":"s2"})), "applied"),
                (
                    Ok(json!({"updated":{"other":null},"newState":"s2"})),
                    "uncertain",
                ),
                (Ok(json!({"updated":{"e":null}})), "uncertain"),
                (
                    Ok(
                        json!({"updated":{"e":null},"notUpdated":{"e":{"type":"forbidden"}},"newState":"s2"}),
                    ),
                    "uncertain",
                ),
                (
                    Ok(json!({"notUpdated":{"e":{"type":"forbidden"}}})),
                    "blocked",
                ),
                (
                    Err(sync::SyncError::Method("stateMismatch".into())),
                    "blocked",
                ),
                (Err(sync::SyncError::Network), "uncertain"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("reply.redb");
                let store = Store::open(&path).unwrap();
                let objects = BTreeMap::from([(
                    "e".into(),
                    json!({"id":"e","uid":"meeting","participants":{"me":{"participationStatus":"needs-action"}}}),
                )]);
                let args = json!({"accountId":"account","sendSchedulingMessages":true,"update":{"e":{"participants/me/participationStatus":"accepted"}}});
                journal::queue_object(
                    &store,
                    "scope",
                    "calendar",
                    "CalendarEvent/set",
                    args,
                    &objects,
                    Some("s1".into()),
                    true,
                )
                .unwrap();
                let entry = journal::entries(&store, "scope").unwrap().remove(0);
                assert_eq!(entry.status, "held");
                journal::approve(&store, "scope", &[entry.id]).unwrap();
                let remote = ReplyRemote {
                    response: Mutex::new(Some(response)),
                    calls: Mutex::new(vec![]),
                };
                let entry = journal::entries(&store, "scope").unwrap().remove(0);
                let mut args = entry.calls[0].1.clone();
                args["ifInState"] = json!("s1");
                assert_eq!(
                    transmit_object_change(
                        &store,
                        &remote,
                        "scope",
                        &entry,
                        "CalendarEvent/set",
                        args.clone()
                    )
                    .await
                    .unwrap(),
                    status == "applied"
                );
                assert_eq!(
                    remote.calls.lock().unwrap()[0]["sendSchedulingMessages"],
                    true
                );
                assert_eq!(remote.calls.lock().unwrap()[0]["ifInState"], "s1");
                assert_eq!(journal::entries(&store, "scope").unwrap()[0].status, status);
                drop(store);
                let store = Store::open(&path).unwrap();
                journal::initialize(&store).unwrap();
                let entry = journal::entries(&store, "scope").unwrap().remove(0);
                assert_eq!(entry.status, status);
                assert!(!transmit_object_change(
                    &store,
                    &remote,
                    "scope",
                    &entry,
                    "CalendarEvent/set",
                    args
                )
                .await
                .unwrap());
                assert_eq!(remote.calls.lock().unwrap().len(), 1);
                assert!(journal::entries(&store, "different-account")
                    .unwrap()
                    .is_empty());
            }
        });
    }

    #[test]
    fn calendar_versions_reject_stale_state_and_keep_changes_durable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("versions.redb");
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "synthetic".into(),
            password: String::new(),
            read_only: false,
        };
        let scope = sync::scope(&config, "a");
        {
            let store = Store::open(&path).unwrap();
            store.put(&format!("{}:desktop-session",config.cache_key()), &json!({"capabilities":{sync::CALENDARS:{}},"accounts":{"a":{"accountCapabilities":{sync::CALENDARS:{}},"isReadOnly":false}}}).to_string()).unwrap();
            let snapshot = calendar::Snapshot {
                calendar_state: Some("c1".into()),
                event_state: Some("e1".into()),
                calendars: vec![json!({"id":"c","myRights":{"mayWriteAll":true}})],
                events: std::collections::BTreeMap::from([(
                    "e".into(),
                    json!({"id":"e","uid":"meeting","sequence":4,"start":"2026-11-18T10:00:00","calendarIds":{"c":true},"recurrenceOverrides":{"2026-11-25T10:00:00":{"start":"2026-11-25T11:00:00"}}}),
                )]),
                ..Default::default()
            };
            store
                .put(
                    &format!("calendar:{scope}"),
                    &serde_json::to_string(&snapshot).unwrap(),
                )
                .unwrap();
            let current = "local-calendar:e1:c1:0";
            for state in [
                json!("local-calendar:old:c1:0"),
                json!("local-calendar:e1:c1:12"),
                json!(42),
            ] {
                assert_eq!(
                    local_object_set_inner(
                        &store,
                        &config,
                        false,
                        "CalendarEvent/set".into(),
                        json!({"accountId":"a","ifInState":state,"destroy":["e"]})
                    )
                    .unwrap_err(),
                    "stateMismatch"
                );
            }
            for sequence in [json!(3), json!(-1), Value::Null, json!("5")] {
                assert_eq!(local_object_set_inner(&store,&config,false,"CalendarEvent/set".into(),json!({"accountId":"a","ifInState":current,"update":{"e":{"sequence":sequence,"start":"2026-11-18T11:00:00"}}})).unwrap_err(),"invalidProperties");
            }
            assert!(journal::entries(&store, &scope).unwrap().is_empty());
            local_object_set_inner(&store,&config,false,"CalendarEvent/set".into(),json!({"accountId":"a","ifInState":current,"sendSchedulingMessages":false,"update":{"e":{"sequence":5,"start":"2026-11-18T11:00:00"}}})).unwrap();
            // Even the same server state is stale after a local edit.
            assert_eq!(
                local_object_set_inner(
                    &store,
                    &config,
                    false,
                    "CalendarEvent/set".into(),
                    json!({"accountId":"a","ifInState":current,"destroy":["e"]})
                )
                .unwrap_err(),
                "stateMismatch"
            );
        }
        let store = Store::open(&path).unwrap();
        let snapshot = calendar::read(&store, &scope).unwrap();
        assert_eq!(snapshot.events["e"]["sequence"], 5);
        assert_eq!(snapshot.events["e"]["start"], "2026-11-18T11:00:00");
        assert_eq!(journal::entries(&store, &scope).unwrap()[0].status, "held");
        // An individual cancellation preserves the series and prior overrides.
        local_object_set_inner(&store,&config,false,"CalendarEvent/set".into(),json!({"accountId":"a","sendSchedulingMessages":false,"update":{"e":{"recurrenceOverrides/2026-11-18T11:00:00":{"excluded":true}}}})).unwrap();
        let event = calendar::read(&store, &scope)
            .unwrap()
            .events
            .remove("e")
            .unwrap();
        assert_eq!(event["sequence"], 5);
        assert_eq!(
            event["recurrenceOverrides"]["2026-11-18T11:00:00"]["excluded"],
            true
        );
        assert_eq!(
            event["recurrenceOverrides"]["2026-11-25T10:00:00"]["start"],
            "2026-11-25T11:00:00"
        );
        let entry = journal::entries(&store, &scope).unwrap().remove(0);
        assert_eq!(entry.calls[0].1["sendSchedulingMessages"], false);
        assert_eq!(entry.calls[0].1["update"]["e"]["sequence"], 5);
        local_object_set_inner(
            &store,
            &config,
            false,
            "CalendarEvent/set".into(),
            json!({"accountId":"a","sendSchedulingMessages":false,"destroy":["e"]}),
        )
        .unwrap();
        assert!(calendar::read(&store, &scope).unwrap().events.is_empty());
        assert!(calendar::read(&store, "different-account")
            .unwrap()
            .events
            .is_empty());
    }

    #[test]
    fn every_write_kind_enforces_read_only_current_membership_and_account_rights() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rights.redb")).unwrap();
        let mut config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "synthetic".into(),
            password: String::new(),
            read_only: false,
        };
        let capabilities = json!({"urn:ietf:params:jmap:mail":{},sync::SUBMISSION:{},sync::CALENDARS:{},sync::FILES:{}});
        let key = format!("{}:desktop-session", config.cache_key());
        store.put(&key,&json!({"capabilities":capabilities,"accounts":{"allowed":{"accountCapabilities":capabilities,"isReadOnly":false},"locked":{"accountCapabilities":capabilities,"isReadOnly":true}}}).to_string()).unwrap();
        for capability in [
            "urn:ietf:params:jmap:mail",
            sync::SUBMISSION,
            sync::CALENDARS,
            sync::FILES,
        ] {
            assert!(writable(&store, &config, "allowed", capability).is_ok());
            assert!(writable(&store, &config, "locked", capability).is_err());
            assert!(writable(&store, &config, "removed", capability).is_err());
            config.read_only = true;
            assert!(writable(&store, &config, "allowed", capability).is_err());
            config.read_only = false;
        }
        config.username = "another-login".into();
        assert!(writable(&store, &config, "allowed", sync::CALENDARS).is_err());
    }
}
