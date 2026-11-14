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

fn writable(
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
    let online_at_start = network.enabled();
    let source_account = draft_account_id.as_deref().unwrap_or(&account);
    if draft_id.is_some() {
        desktop::require_capability(&store, &config, source_account, "urn:ietf:params:jmap:mail")?;
    }
    // Online sends use the native transport immediately so the upstream
    // composer receives the real JMAP response. Offline/review-gated sends
    // stay durably queued for the user's approval.
    let _send_guard = if online_at_start {
        Some(gate.0.lock().await)
    } else {
        None
    };
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
        if !journal::claim(&store, &scope, id)? {
            return Err("Der Versandauftrag konnte nicht sicher übernommen werden.".into());
        }
        let calls = journal::entries(&store, &scope)?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or("Versandauftrag nicht gefunden.")?
            .calls;
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
                } else if definitive_error {
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
    let account = args["accountId"].as_str().ok_or("accountNotFound")?;
    let scope = sync::scope(&config, account);
    let hold = !network.enabled() || store.metadata(&scope)?.review_new_changes;
    match method.as_str() {
        "CalendarEvent/set" => {
            writable(&store, &config, account, sync::CALENDARS)?;
            let snapshot = calendar::read(&store, &scope)?;
            if snapshot.access_denied {
                return Err("forbidden".into());
            }
            if args["ifInState"]
                .as_str()
                .is_some_and(|s| !s.starts_with("local-calendar:"))
            {
                return Err("stateMismatch".into());
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
                journal::patch(&mut event, patch)?;
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
                if file.get("blobId").is_some_and(|v| !v.is_null())
                    || file["@type"].as_str().is_some_and(|s| s != "Directory")
                    || file["nodeType"].as_str().is_some_and(|s| s != "directory")
                {
                    return Err("Neue Datei-Inhalte benötigen noch den Uploadadapter.".into());
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
            if entry.status != "ready" {
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
                let capability = if entry.kind == "calendar" {
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
                let (method, mut args, _) = resolved[0].clone();
                if let Some(state) = &entry.base_state {
                    args["ifInState"] = json!(state);
                }
                if !journal::claim(&store, &scope, entry.id)? {
                    continue;
                }
                match remote.call(&method, args).await {
                    Ok(response) => {
                        let success = ["created", "updated"]
                            .iter()
                            .any(|k| response[k].as_object().is_some_and(|m| !m.is_empty()))
                            || response["destroyed"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty());
                        if success && !response["newState"].is_string() {
                            journal::finish(
                                &store,
                                &scope,
                                entry.id,
                                Some("uncertain"),
                                Some("Unvollständige Serverantwort; Änderung prüfen"),
                            )?;
                            continue;
                        }
                        if success {
                            if let Some(server_id) = response["created"]
                                .as_object()
                                .and_then(|m| m.values().next())
                                .and_then(|v| v["id"].as_str())
                            {
                                journal::remember_created(
                                    &store,
                                    &scope,
                                    &entry.object_id,
                                    server_id,
                                )?;
                            }
                            journal::applied(
                                &store,
                                &scope,
                                entry.id,
                                response["newState"].as_str(),
                            )?;
                            changed = true;
                            continue;
                        }
                        let status = Some("blocked");
                        journal::finish(
                            &store,
                            &scope,
                            entry.id,
                            status,
                            if success {
                                None
                            } else {
                                Some(
                                    "Server hat die Änderung abgelehnt oder einen Konflikt erkannt",
                                )
                            },
                        )?;
                    }
                    Err(sync::SyncError::Method(_)) => journal::finish(
                        &store,
                        &scope,
                        entry.id,
                        Some("blocked"),
                        Some("Server hat die Änderung abgelehnt oder einen Konflikt erkannt"),
                    )?,
                    Err(_) => journal::finish(
                        &store,
                        &scope,
                        entry.id,
                        Some("uncertain"),
                        Some("Serverantwort unklar; Änderung prüfen"),
                    )?,
                }
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
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
