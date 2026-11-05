use crate::{config::TestConfig, replica::LocalMail, storage::Store, sync};
use redb::{ReadableTable, TableDefinition};
use serde_json::{json, Value};
use tauri::State;

const KV: TableDefinition<&str, &str> = TableDefinition::new("desktop_values_v1");
fn db_error(_: impl std::fmt::Display) -> String {
    "Desktop-Speicher ist nicht verfügbar.".into()
}
fn validate_key(key: &str) -> Result<(), String> {
    if key.len() > 4096
        || !["plugins/", "ui/"]
            .iter()
            .any(|prefix| key.starts_with(prefix))
    {
        return Err("Unzulässiger Desktop-Speicherschlüssel.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn desktop_kv_get(store: State<'_, Store>, key: String) -> Result<Option<String>, String> {
    kv_get(&store, &key)
}
pub fn kv_get(store: &Store, key: &str) -> Result<Option<String>, String> {
    validate_key(key)?;
    let tx = store.0.begin_write().map_err(db_error)?;
    let result = {
        let table = tx.open_table(KV).map_err(db_error)?;
        let result = table
            .get(key)
            .map_err(db_error)?
            .map(|value| value.value().to_owned());
        result
    };
    tx.commit().map_err(db_error)?;
    Ok(result)
}
#[tauri::command]
pub fn desktop_kv_put(store: State<'_, Store>, key: String, value: String) -> Result<(), String> {
    validate_key(&key)?;
    if value.len() > 64 * 1024 * 1024 {
        return Err("Desktop-Datensatz ist zu groß.".into());
    }
    let tx = store.0.begin_write().map_err(db_error)?;
    {
        tx.open_table(KV)
            .map_err(db_error)?
            .insert(key.as_str(), value.as_str())
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)
}
#[tauri::command]
pub fn desktop_kv_delete(store: State<'_, Store>, key: String) -> Result<(), String> {
    validate_key(&key)?;
    let tx = store.0.begin_write().map_err(db_error)?;
    {
        tx.open_table(KV)
            .map_err(db_error)?
            .remove(key.as_str())
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)
}
#[tauri::command]
pub fn desktop_kv_keys(store: State<'_, Store>, prefix: String) -> Result<Vec<String>, String> {
    validate_key(&prefix)?;
    let tx = store.0.begin_write().map_err(db_error)?;
    let mut keys = Vec::new();
    {
        let table = tx.open_table(KV).map_err(db_error)?;
        for row in table.range(prefix.as_str()..).map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if !key.value().starts_with(&prefix) {
                break;
            }
            keys.push(key.value().to_owned());
        }
    }
    tx.commit().map_err(db_error)?;
    Ok(keys)
}

#[tauri::command]
pub async fn desktop_bootstrap(store: State<'_, Store>) -> Result<Value, String> {
    bootstrap_payload(&store, false).await
}

#[tauri::command]
pub async fn refresh_desktop_session(store: State<'_, Store>) -> Result<Value, String> {
    bootstrap_payload(&store, true).await
}

async fn bootstrap_payload(store: &Store, refresh: bool) -> Result<Value, String> {
    let config = TestConfig::load()?;
    let cache_key = format!("{}:desktop-session", config.cache_key());
    let cached = if refresh {
        None
    } else {
        store.get(&cache_key)?
    };
    let mut session: Value = if let Some(cached) = cached {
        serde_json::from_str(&cached).map_err(|_| "Lokale Sitzung ist ungültig.")?
    } else {
        let raw: Value = serde_json::from_slice(&crate::session::session_bytes(&config).await?)
            .map_err(|_| "Server-Sitzung ist ungültig.")?;
        // Never forward unknown top-level server fields (tokens, diagnostic extensions, etc.).
        let mut session = serde_json::Map::new();
        for key in [
            "capabilities",
            "accounts",
            "primaryAccounts",
            "username",
            "apiUrl",
            "downloadUrl",
            "uploadUrl",
            "eventSourceUrl",
            "state",
        ] {
            if let Some(value) = raw.get(key) {
                session.insert(key.into(), value.clone());
            }
        }
        let session = Value::Object(session);
        store.put(
            &cache_key,
            &serde_json::to_string(&session)
                .map_err(|_| "Sitzung konnte nicht gespeichert werden.")?,
        )?;
        session
    };
    if config.read_only {
        if let Some(accounts) = session["accounts"].as_object_mut() {
            for account in accounts.values_mut() {
                account["isReadOnly"] = json!(true);
            }
        }
    }
    Ok(
        json!({"session":session,"serverUrl":config.url.origin().ascii_serialization(),
        "username":config.username,"readOnly":config.read_only}),
    )
}

fn available_account(store: &Store, config: &TestConfig, account: &str) -> Result<bool, String> {
    let Some(raw) = store.get(&format!("{}:desktop-session", config.cache_key()))? else {
        return Ok(false);
    };
    let session: Value = serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
    Ok(session["accounts"].get(account).is_some())
}

pub fn require_capability(
    store: &Store,
    config: &TestConfig,
    account: &str,
    capability: &str,
) -> Result<(), String> {
    let Some(raw) = store.get(&format!("{}:desktop-session", config.cache_key()))? else {
        return Err("accountNotFound".into());
    };
    let session: Value = serde_json::from_str(&raw).map_err(|_| "Lokale Sitzung ist ungültig.")?;
    if !sync::account_supports(&session, account, capability) {
        return Err("accountNotSupportedByMethod".into());
    }
    Ok(())
}

#[tauri::command]
pub fn local_jmap(
    store: State<'_, Store>,
    account_id: String,
    method_calls: Vec<(String, Value, String)>,
) -> Result<Value, String> {
    let config = TestConfig::load()?;
    answer_local_batch(&store, &config, &account_id, method_calls)
}

fn answer_local_batch(
    store: &Store,
    config: &TestConfig,
    account_id: &str,
    method_calls: Vec<(String, Value, String)>,
) -> Result<Value, String> {
    let mut snapshots = std::collections::HashMap::new();
    let mut responses = Vec::new();
    let mut prior = std::collections::HashMap::new();
    for (method, args, id) in method_calls {
        let resolved = resolve_references(&args, &prior);
        let target = resolved
            .as_ref()
            .ok()
            .and_then(|value| value.get("accountId"))
            .and_then(Value::as_str)
            .unwrap_or(account_id);
        if !snapshots.contains_key(target) && available_account(&store, &config, target)? {
            let scope = sync::scope(&config, target);
            snapshots.insert(
                target.to_string(),
                (store.local_mail(&scope)?, store.metadata(&scope)?),
            );
        }
        let answer = if let Err(kind) = &resolved {
            Err(*kind)
        } else if let Some((mail, metadata)) = snapshots.get(target) {
            let state = format!(
                "local:{}:{}:{}",
                metadata.email_state.as_deref().unwrap_or("empty"),
                metadata.mailbox_state.as_deref().unwrap_or("empty"),
                metadata.next_operation
            );
            if require_capability(store, config, target, "urn:ietf:params:jmap:mail").is_err() {
                Err("accountNotSupportedByMethod")
            } else if (method.starts_with("Mailbox/") && metadata.mailbox_state.is_none())
                || ((method.starts_with("Email/") || method.starts_with("Thread/"))
                    && metadata.email_state.is_none())
            {
                Err("serverUnavailable")
            } else {
                if method == "Email/set" {
                    answer_email_set(
                        store,
                        config,
                        target,
                        mail,
                        &state,
                        resolved.as_ref().unwrap(),
                    )
                } else {
                    answer_read(mail, target, &state, &method, resolved.as_ref().unwrap())
                }
            }
        } else {
            Err("accountNotFound")
        };
        match answer {
            Ok(value) => {
                if method == "Email/set" {
                    snapshots.remove(target);
                }
                prior.insert((id.clone(), method.clone()), value.clone());
                responses.push(json!([method, value, id]));
            }
            Err(kind) => {
                crate::logging::event(
                    "warn",
                    "mail",
                    "local_jmap_method_failed",
                    json!({"method":method,"code":kind}),
                );
                responses.push(json!(["error",{"type":kind},id]));
            }
        }
    }
    Ok(json!({"methodResponses":responses,"sessionState":"desktop"}))
}

fn answer_email_set(
    store: &Store,
    config: &TestConfig,
    account: &str,
    mail: &LocalMail,
    old_state: &str,
    args: &Value,
) -> Result<Value, &'static str> {
    if args
        .get("ifInState")
        .is_some_and(|value| value.as_str() != Some(old_state))
    {
        return Err("stateMismatch");
    }
    let scope = sync::scope(config, account);
    if let Some(updates) = args.get("update") {
        if args.get("create").is_some() || args.get("destroy").is_some() || config.read_only {
            return Err("forbidden");
        }
        let raw = store
            .get(&format!("{}:desktop-session", config.cache_key()))
            .map_err(|_| "serverFail")?
            .ok_or("accountNotFound")?;
        let session: Value = serde_json::from_str(&raw).map_err(|_| "serverFail")?;
        if session["accounts"][account]["isReadOnly"] == true {
            return Err("forbidden");
        }
        let updates = updates.as_object().ok_or("invalidArguments")?;
        let mut updated = serde_json::Map::new();
        let mut not_updated = serde_json::Map::new();
        for (id, patch) in updates {
            let Some(email) = mail.emails.iter().find(|email| email["id"] == *id) else {
                not_updated.insert(id.clone(), json!({"type":"notFound"}));
                continue;
            };
            match email_update(mail, email, patch) {
                Ok((changes, mailbox_ids)) => {
                    match store.queue_email_update(&scope, id, &changes, mailbox_ids) {
                        Ok(()) => {
                            updated.insert(id.clone(), Value::Null);
                        }
                        Err(_) => {
                            not_updated.insert(id.clone(), json!({"type":"serverFail"}));
                        }
                    }
                }
                Err(kind) => {
                    not_updated.insert(id.clone(), json!({"type":kind}));
                }
            }
        }
        let metadata = store.metadata(&scope).map_err(|_| "serverFail")?;
        let new_state = format!(
            "local:{}:{}:{}",
            metadata.email_state.as_deref().unwrap_or("empty"),
            metadata.mailbox_state.as_deref().unwrap_or("empty"),
            metadata.next_operation
        );
        return Ok(
            json!({"accountId":account,"oldState":old_state,"newState":new_state,"updated":updated,"notUpdated":not_updated}),
        );
    }
    let mut created = serde_json::Map::new();
    let mut not_created = serde_json::Map::new();
    let mut destroyed = Vec::new();
    let mut not_destroyed = serde_json::Map::new();
    if let Some(creates) = args.get("create") {
        let creates = creates.as_object().ok_or("invalidArguments")?;
        for (creation_id, email) in creates {
            match store.save_local_draft(&scope, email.clone()) {
                Ok(local_id) => {
                    crate::logging::event("info", "composer", "draft_saved_locally", json!({}));
                    created.insert(creation_id.clone(), json!({"id":local_id}));
                }
                Err(reason) => {
                    crate::logging::event(
                        "warn",
                        "composer",
                        "draft_save_failed",
                        json!({"reason":reason}),
                    );
                    not_created.insert(
                        creation_id.clone(),
                        json!({"type":"invalidProperties","description":reason}),
                    );
                }
            }
        }
    }
    if let Some(ids) = args.get("destroy") {
        for id in ids
            .as_array()
            .ok_or("invalidArguments")?
            .iter()
            .filter_map(Value::as_str)
        {
            if store.delete_local_draft(&scope, id).is_ok() {
                crate::logging::event("info", "composer", "local_draft_deleted", json!({}));
                destroyed.push(id.to_owned());
                continue;
            }
            if config.read_only {
                not_destroyed.insert(id.to_owned(), json!({"type":"forbidden"}));
                continue;
            }
            let raw = store
                .get(&format!("{}:desktop-session", config.cache_key()))
                .map_err(|_| "serverFail")?
                .ok_or("accountNotFound")?;
            let session: Value = serde_json::from_str(&raw).map_err(|_| "serverFail")?;
            if session["accounts"][account]["isReadOnly"] == true {
                not_destroyed.insert(id.to_owned(), json!({"type":"forbidden"}));
                continue;
            }
            match store.queue_draft_delete(&scope, id) {
                Ok(_) => {
                    crate::logging::event(
                        "info",
                        "composer",
                        "server_draft_delete_queued",
                        json!({}),
                    );
                    destroyed.push(id.to_owned());
                }
                Err(_) => {
                    not_destroyed.insert(id.to_owned(), json!({"type":"notFound"}));
                }
            }
        }
    }
    if created.is_empty()
        && destroyed.is_empty()
        && not_created.is_empty()
        && not_destroyed.is_empty()
    {
        return Err("invalidArguments");
    }
    let metadata = store.metadata(&scope).map_err(|_| "serverFail")?;
    let new_state = format!(
        "local:{}:{}:{}",
        metadata.email_state.as_deref().unwrap_or("empty"),
        metadata.mailbox_state.as_deref().unwrap_or("empty"),
        metadata.next_operation
    );
    Ok(
        json!({"accountId":account,"oldState":old_state,"newState":new_state,"created":created,"notCreated":not_created,"destroyed":destroyed,"notDestroyed":not_destroyed}),
    )
}

type EmailUpdate = (Vec<(String, bool)>, Option<Value>);

fn email_update(
    mail: &LocalMail,
    email: &Value,
    patch: &Value,
) -> Result<EmailUpdate, &'static str> {
    let patch = patch.as_object().ok_or("invalidProperties")?;
    let mut flags = serde_json::Map::new();
    let mut desired = email["mailboxIds"].as_object().cloned().unwrap_or_default();
    let before = desired.clone();
    let mut moving = false;
    for (property, value) in patch {
        if property == "mailboxIds" {
            if patch.keys().any(|key| key.starts_with("mailboxIds/")) {
                return Err("invalidProperties");
            }
            desired = value.as_object().cloned().ok_or("invalidProperties")?;
            moving = true;
        } else if let Some(token) = property.strip_prefix("mailboxIds/") {
            // Decode one JSON Pointer token, rejecting malformed escapes.
            let mut id = String::new();
            let mut chars = token.chars();
            while let Some(ch) = chars.next() {
                if ch == '~' {
                    id.push(match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => return Err("invalidProperties"),
                    });
                } else if ch == '/' {
                    return Err("invalidProperties");
                } else {
                    id.push(ch);
                }
            }
            if value.is_null() {
                desired.remove(&id);
            } else if value == &Value::Bool(true) {
                desired.insert(id, value.clone());
            } else {
                return Err("invalidProperties");
            }
            moving = true;
        } else {
            flags.insert(property.clone(), value.clone());
        }
    }
    let changes = keyword_patch(email, &Value::Object(flags))?;
    if moving {
        if desired.is_empty() || desired.values().any(|v| v != &Value::Bool(true)) {
            return Err("invalidProperties");
        }
        for id in before.keys().chain(desired.keys()) {
            if before.contains_key(id) == desired.contains_key(id) {
                continue;
            }
            let mailbox = mail
                .mailboxes
                .iter()
                .find(|m| m["id"] == *id)
                .ok_or("invalidProperties")?;
            let right = if desired.contains_key(id) {
                "mayAddItems"
            } else {
                "mayRemoveItems"
            };
            if mailbox["myRights"][right] != true {
                return Err("forbidden");
            }
        }
    }
    Ok((changes, moving.then_some(Value::Object(desired))))
}

#[test]
fn mailbox_patches_validate_rights_tokens_and_mixed_updates() {
    let mut mail = LocalMail {
        initialized: true,
        pending: vec![],
        pending_deletes: vec![],
        emails: vec![],
        mailboxes: vec![
            json!({"id":"inbox","myRights":{"mayRemoveItems":true,"mayAddItems":true}}),
            json!({"id":"trash/~","myRights":{"mayAddItems":true}}),
            json!({"id":"locked","myRights":{"mayAddItems":false}}),
        ],
    };
    let email = json!({"keywords":{},"mailboxIds":{"inbox":true}});
    let (flags, ids) = email_update(
        &mail,
        &email,
        &json!({"mailboxIds/inbox":null,"mailboxIds/trash~1~0":true,"keywords/$seen":true}),
    )
    .unwrap();
    assert_eq!(ids.unwrap(), json!({"trash/~":true}));
    assert_eq!(flags, vec![("$seen".into(), true)]);
    for patch in [
        json!({"mailboxIds":{}}),
        json!({"mailboxIds":{"unknown":true}}),
        json!({"mailboxIds":{"locked":true}}),
        json!({"mailboxIds":{"inbox":true},"subject":"bad"}),
        json!({"mailboxIds/bad~2":true}),
    ] {
        assert!(email_update(&mail, &email, &patch).is_err());
    }
    mail.mailboxes[0]["myRights"]["mayRemoveItems"] = json!(false);
    assert!(email_update(&mail, &email, &json!({"mailboxIds":{"trash/~":true}})).is_err());
}

fn keyword_patch(email: &Value, patch: &Value) -> Result<Vec<(String, bool)>, &'static str> {
    let patch = patch.as_object().ok_or("invalidProperties")?;
    let before = email["keywords"].as_object().ok_or("invalidProperties")?;
    let mut desired = before.clone();
    for (property, value) in patch {
        if property == "keywords" {
            if patch.len() != 1 {
                return Err("invalidProperties");
            }
            let values = value.as_object().ok_or("invalidProperties")?;
            if values.values().any(|v| v != &Value::Bool(true)) {
                return Err("invalidProperties");
            }
            desired = values.clone();
        } else if let Some(keyword) = property.strip_prefix("keywords/") {
            if !["$seen", "$flagged"].contains(&keyword)
                || (!value.is_null() && value != &Value::Bool(true))
            {
                return Err("invalidProperties");
            }
            if value.is_null() {
                desired.remove(keyword);
            } else {
                desired.insert(keyword.into(), Value::Bool(true));
            }
        } else {
            return Err("invalidProperties");
        }
    }
    let keys: std::collections::BTreeSet<_> = before.keys().chain(desired.keys()).collect();
    let mut changes = Vec::new();
    for key in keys {
        if before.get(key) != desired.get(key) {
            if !["$seen", "$flagged"].contains(&key.as_str()) {
                return Err("invalidProperties");
            }
            changes.push((key.clone(), desired.get(key) == Some(&Value::Bool(true))));
        }
    }
    Ok(changes)
}

fn resolve_references(
    args: &Value,
    prior: &std::collections::HashMap<(String, String), Value>,
) -> Result<Value, &'static str> {
    let mut result = args.as_object().ok_or("invalidArguments")?.clone();
    for (name, reference) in args.as_object().ok_or("invalidArguments")? {
        let Some(property) = name.strip_prefix('#') else {
            continue;
        };
        let id = reference["resultOf"]
            .as_str()
            .ok_or("invalidResultReference")?;
        let method = reference["name"].as_str().ok_or("invalidResultReference")?;
        let path = reference["path"].as_str().ok_or("invalidResultReference")?;
        let source = prior
            .get(&(id.into(), method.into()))
            .ok_or("invalidResultReference")?;
        let value = if path == "/list/*/threadId" {
            Value::Array(
                source["list"]
                    .as_array()
                    .ok_or("invalidResultReference")?
                    .iter()
                    .map(|row| row.get("threadId").cloned().ok_or("invalidResultReference"))
                    .collect::<Result<_, _>>()?,
            )
        } else {
            source
                .pointer(path)
                .cloned()
                .ok_or("invalidResultReference")?
        };
        if result.contains_key(property) {
            return Err("invalidArguments");
        }
        result.remove(name);
        result.insert(property.into(), value);
    }
    Ok(Value::Object(result))
}

fn object_get(
    objects: &[Value],
    account: &str,
    state: &str,
    args: &Value,
) -> Result<Value, &'static str> {
    let selected: Vec<&Value> = match args.get("ids").filter(|ids| !ids.is_null()) {
        None => objects.iter().collect(),
        Some(ids) => {
            let ids = ids.as_array().ok_or("invalidArguments")?;
            objects
                .iter()
                .filter(|object| ids.contains(&object["id"]))
                .collect()
        }
    };
    let not_found: Vec<&Value> = args["ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter(|id| !selected.iter().any(|row| &row["id"] == *id))
                .collect()
        })
        .unwrap_or_default();
    let list: Vec<Value> = selected
        .into_iter()
        .map(|object| {
            if let Some(properties) = args["properties"].as_array() {
                let mut row = serde_json::Map::new();
                row.insert("id".into(), object["id"].clone());
                for property in properties.iter().filter_map(Value::as_str) {
                    if let Some(value) = object.get(property) {
                        row.insert(property.into(), value.clone());
                    }
                }
                Value::Object(row)
            } else {
                object.clone()
            }
        })
        .collect();
    Ok(json!({"accountId":account,"state":state,"list":list,"notFound":not_found}))
}

fn matches_filter(email: &Value, filter: &Value) -> Result<bool, &'static str> {
    if filter.is_null() {
        return Ok(true);
    }
    let object = filter.as_object().ok_or("invalidArguments")?;
    if let Some(operator) = object.get("operator").and_then(Value::as_str) {
        let results: Vec<bool> = filter["conditions"]
            .as_array()
            .ok_or("invalidArguments")?
            .iter()
            .map(|condition| matches_filter(email, condition))
            .collect::<Result<_, _>>()?;
        return match operator {
            "AND" => Ok(results.iter().all(|v| *v)),
            "OR" => Ok(results.iter().any(|v| *v)),
            "NOT" => Ok(!results.iter().any(|v| *v)),
            _ => Err("unsupportedFilter"),
        };
    }
    for (key, value) in object {
        let keyword = |keyword: &Value| email["keywords"][keyword.as_str().unwrap_or("")] == true;
        let contains = |field: &str| {
            email[field]
                .to_string()
                .to_lowercase()
                .contains(&value.as_str().unwrap_or("").to_lowercase())
        };
        let accepted = match key.as_str() {
            "inMailbox" => email["mailboxIds"][value.as_str().ok_or("invalidArguments")?] == true,
            "inMailboxOtherThan" => email["mailboxIds"].as_object().is_some_and(|ids| {
                ids.keys()
                    .any(|id| !value.as_array().unwrap_or(&Vec::new()).contains(&json!(id)))
            }),
            "hasKeyword" => keyword(value),
            "notKeyword" => !keyword(value),
            "hasAttachment" => email["hasAttachment"] == *value,
            "before" => email["receivedAt"].as_str() < value.as_str(),
            "after" => email["receivedAt"].as_str() >= value.as_str(),
            "minSize" => email["size"].as_u64() >= value.as_u64(),
            "maxSize" => email["size"].as_u64() < value.as_u64(),
            "subject" | "from" | "to" | "cc" | "bcc" => contains(key),
            "body" => contains("bodyValues"),
            "text" => ["subject", "from", "to", "preview", "bodyValues"]
                .iter()
                .any(|field| contains(field)),
            _ => return Err("unsupportedFilter"),
        };
        if !accepted {
            return Ok(false);
        }
    }
    Ok(true)
}

fn answer_read(
    mail: &LocalMail,
    account: &str,
    state: &str,
    method: &str,
    args: &Value,
) -> Result<Value, &'static str> {
    match method {
        "Mailbox/query" => {
            if args
                .get("filter")
                .is_some_and(|v| !v.is_null() && v != &json!({}))
            {
                return Err("unsupportedFilter");
            }
            let position = args["position"].as_u64().unwrap_or(0) as usize;
            let ids: Vec<Value> = mail
                .mailboxes
                .iter()
                .skip(position)
                .take(args["limit"].as_u64().unwrap_or(100) as usize)
                .map(|row| row["id"].clone())
                .collect();
            Ok(
                json!({"accountId":account,"queryState":state,"canCalculateChanges":false,"position":position,"ids":ids,"total":mail.mailboxes.len()}),
            )
        }
        "Mailbox/get" => object_get(&mail.mailboxes, account, state, args),
        "Email/get" => object_get(&mail.emails, account, state, args),
        "Identity/get" => object_get(&[], account, state, args),
        "Thread/get" => {
            let mut threads = std::collections::BTreeMap::<String, Vec<Value>>::new();
            for email in mail.emails.iter().rev() {
                if let Some(thread) = email["threadId"].as_str() {
                    threads
                        .entry(thread.into())
                        .or_default()
                        .push(email["id"].clone());
                }
            }
            let rows: Vec<Value> = threads
                .into_iter()
                .map(|(id, email_ids)| json!({"id":id,"emailIds":email_ids}))
                .collect();
            object_get(&rows, account, state, args)
        }
        "Email/query" => {
            let mut emails = Vec::new();
            for email in &mail.emails {
                if matches_filter(email, &args["filter"])? {
                    emails.push(email);
                }
            }
            let sorts = args["sort"].as_array();
            if let Some(sorts) = sorts {
                for sort in sorts {
                    if ![
                        "receivedAt",
                        "sentAt",
                        "size",
                        "subject",
                        "id",
                        "from",
                        "to",
                        "hasKeyword",
                    ]
                    .contains(&sort["property"].as_str().unwrap_or(""))
                    {
                        return Err("unsupportedSort");
                    }
                    if sort["property"] == "hasKeyword" && sort["keyword"].as_str().is_none() {
                        return Err("unsupportedSort");
                    }
                }
                emails.sort_by(|a, b| {
                    for sort in sorts {
                        let property = sort["property"].as_str().unwrap_or("receivedAt");
                        let order = if property == "hasKeyword" {
                            let keyword = sort["keyword"].as_str().unwrap_or("");
                            (a["keywords"][keyword] == true).cmp(&(b["keywords"][keyword] == true))
                        } else if property == "from" || property == "to" {
                            let address = |email: &Value| {
                                let first = &email[property][0];
                                first["name"]
                                    .as_str()
                                    .filter(|name| !name.is_empty())
                                    .or_else(|| first["email"].as_str())
                                    .unwrap_or("")
                                    .to_lowercase()
                            };
                            address(a).cmp(&address(b))
                        } else if property == "size" {
                            a[property].as_u64().cmp(&b[property].as_u64())
                        } else {
                            a[property].as_str().cmp(&b[property].as_str())
                        };
                        if !order.is_eq() {
                            return if sort["isAscending"] == true {
                                order
                            } else {
                                order.reverse()
                            };
                        }
                    }
                    a["id"].as_str().cmp(&b["id"].as_str())
                });
            }
            if args["collapseThreads"] == true {
                let mut threads = std::collections::HashSet::new();
                emails.retain(|email| {
                    threads.insert(
                        email["threadId"]
                            .as_str()
                            .unwrap_or(email["id"].as_str().unwrap_or("")),
                    )
                });
            }
            let total = emails.len();
            let position = if let Some(anchor) = args["anchor"].as_str() {
                let position = emails
                    .iter()
                    .position(|email| email["id"] == anchor)
                    .ok_or("anchorNotFound")?;
                (position as i64 + args["anchorOffset"].as_i64().unwrap_or(0)).max(0) as usize
            } else {
                args["position"].as_u64().unwrap_or(0) as usize
            };
            let ids: Vec<Value> = emails
                .into_iter()
                .skip(position)
                .take(args["limit"].as_u64().unwrap_or(100) as usize)
                .map(|email| email["id"].clone())
                .collect();
            Ok(
                json!({"accountId":account,"queryState":state,"canCalculateChanges":false,"position":position,"ids":ids,"total":total,"collapseThreads":args["collapseThreads"] == true}),
            )
        }
        "Email/changes" | "Email/queryChanges" | "Mailbox/changes" => Err("cannotCalculateChanges"),
        method if method.ends_with("/set") || method.ends_with("/import") => Err("forbidden"),
        _ => Err("unknownMethod"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_sort_supports_sender_and_keyword_order() {
        let mail = LocalMail {
            initialized: true,
            pending: vec![],
            pending_deletes: vec![],
            mailboxes: vec![],
            emails: vec![
                json!({"id":"a","from":[{"name":"Zoe"}],"keywords":{}}),
                json!({"id":"b","from":[{"email":"alice@example.test"}],"keywords":{"$flagged":true}}),
            ],
        };
        let sorted = answer_read(
            &mail,
            "account",
            "s",
            "Email/query",
            &json!({"sort":[{"property":"from","isAscending":true}]}),
        )
        .unwrap();
        assert_eq!(sorted["ids"], json!(["b", "a"]));
        let sorted = answer_read(
            &mail,
            "account",
            "s",
            "Email/query",
            &json!({"sort":[{"property":"hasKeyword","keyword":"$flagged","isAscending":false}]}),
        )
        .unwrap();
        assert_eq!(sorted["ids"], json!(["b", "a"]));
        assert!(answer_read(
            &mail,
            "account",
            "s",
            "Email/query",
            &json!({"sort":[{"property":"hasKeyword"}]})
        )
        .is_err());
    }

    #[test]
    fn writable_jmap_queues_flags_atomically_and_read_only_remains_enforced() {
        use crate::replica::Collection;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writes.redb");
        let store = Store::open(&path).unwrap();
        let mut config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "test".into(),
            password: String::new(),
            read_only: false,
        };
        store.put(&format!("{}:desktop-session",config.cache_key()),&json!({"accounts":{"a":{"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}}}}).to_string()).unwrap();
        let scope = sync::scope(&config, "a");
        store
            .apply_remote(
                &scope,
                Collection::Mailbox,
                None,
                "m1",
                &[json!({"id":"inbox"})],
                &[],
                true,
            )
            .unwrap();
        store
            .apply_remote(
                &scope,
                Collection::Email,
                None,
                "s1",
                &[json!({"id":"e","threadId":"t","mailboxIds":{"inbox":true},"keywords":{}})],
                &[],
                true,
            )
            .unwrap();
        let result=answer_local_batch(&store,&config,"a",vec![
            ("Email/set".into(),json!({"accountId":"a","update":{"e":{"keywords/$seen":true,"keywords/$flagged":true}}}),"write".into()),
            ("Email/get".into(),json!({"accountId":"a","ids":["e"]}),"read".into()),
        ]).unwrap();
        assert!(result["methodResponses"][0][1]["updated"]
            .get("e")
            .is_some());
        assert_eq!(
            result["methodResponses"][1][1]["list"][0]["keywords"]["$seen"],
            true
        );
        assert_eq!(
            result["methodResponses"][1][1]["list"][0]["keywords"]["$flagged"],
            true
        );
        assert_eq!(store.local_mail(&scope).unwrap().pending.len(), 2);
        assert_eq!(
            store.local_mail(&scope).unwrap().mailboxes[0]["unreadEmails"],
            0
        );
        let invalid = answer_local_batch(
            &store,
            &config,
            "a",
            vec![(
                "Email/set".into(),
                json!({"update":{"e":{"keywords/$seen":null,"subject":"forbidden change"}}}),
                "invalid".into(),
            )],
        )
        .unwrap();
        assert_eq!(
            invalid["methodResponses"][0][1]["notUpdated"]["e"]["type"],
            "invalidProperties"
        );
        assert_eq!(store.local_mail(&scope).unwrap().pending.len(), 2);
        config.read_only = true;
        let blocked = answer_local_batch(
            &store,
            &config,
            "a",
            vec![(
                "Email/set".into(),
                json!({"update":{"e":{"keywords/$seen":null}}}),
                "blocked".into(),
            )],
        )
        .unwrap();
        assert_eq!(blocked["methodResponses"][0][1]["type"], "forbidden");
        config.read_only = false;
        store.put(&format!("{}:desktop-session",config.cache_key()),&json!({"accounts":{"a":{"isReadOnly":true,"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}}}}).to_string()).unwrap();
        let server_readonly = answer_local_batch(
            &store,
            &config,
            "a",
            vec![(
                "Email/set".into(),
                json!({"update":{"e":{"keywords/$seen":null}}}),
                "server-readonly".into(),
            )],
        )
        .unwrap();
        assert_eq!(
            server_readonly["methodResponses"][0][1]["type"],
            "forbidden"
        );
        drop(store);
        assert_eq!(
            Store::open(&path)
                .unwrap()
                .local_mail(&scope)
                .unwrap()
                .pending
                .len(),
            2
        );
    }
    #[test]
    fn shared_batches_route_by_account_and_revoked_accounts_cannot_read_cached_data() {
        use crate::replica::Collection;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("shared.redb")).unwrap();
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "test".into(),
            password: String::new(),
            read_only: true,
        };
        let session_key = format!("{}:desktop-session", config.cache_key());
        store
            .put(
                &session_key,
            &json!({"accounts":{"personal":{"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}},"shared":{"accountCapabilities":{"urn:ietf:params:jmap:mail":{}}}}}).to_string(),
            )
            .unwrap();
        for account in ["personal", "shared"] {
            let scope = sync::scope(&config, account);
            store
                .apply_remote(
                    &scope,
                    Collection::Mailbox,
                    None,
                    "m1",
                    &[json!({"id":"folder","name":account})],
                    &[],
                    true,
                )
                .unwrap();
            store
                .apply_remote(
                    &scope,
                    Collection::Email,
                    None,
                    "e1",
                    &[json!({"id":"same-id","subject":account,"keywords":{}})],
                    &[],
                    true,
                )
                .unwrap();
        }
        let answer = answer_local_batch(&store,&config,"personal",vec![
            ("Mailbox/get".into(),json!({"accountId":"personal"}),"p".into()),
            ("Mailbox/get".into(),json!({"accountId":"shared"}),"s".into()),
            ("Email/query".into(),json!({"accountId":"shared"}),"q".into()),
            ("Email/get".into(),json!({"accountId":"shared","#ids":{"resultOf":"q","name":"Email/query","path":"/ids"}}),"g".into()),
        ]).unwrap();
        assert_eq!(
            answer["methodResponses"][0][1]["list"][0]["name"],
            "personal"
        );
        assert_eq!(answer["methodResponses"][1][1]["list"][0]["name"], "shared");
        assert_eq!(answer["methodResponses"][3][1]["accountId"], "shared");
        assert_eq!(
            answer["methodResponses"][3][1]["list"][0]["subject"],
            "shared"
        );
        store
            .put(
                &session_key,
                &json!({"accounts":{"personal":{}}}).to_string(),
            )
            .unwrap();
        let revoked = answer_local_batch(
            &store,
            &config,
            "personal",
            vec![(
                "Email/get".into(),
                json!({"accountId":"shared","ids":["same-id"]}),
                "r".into(),
            )],
        )
        .unwrap();
        assert_eq!(revoked["methodResponses"][0][1]["type"], "accountNotFound");
        assert!(
            require_capability(&store, &config, "shared", "urn:ietf:params:jmap:mail").is_err()
        );
    }

    #[test]
    #[ignore = "Requires local Stalwart; read-only shared-account discovery and downloads"]
    fn live_shared_read_only() {
        use crate::sync::{account_supports, Connection, Remote, CALENDARS};
        const MAIL: &str = "urn:ietf:params:jmap:mail";
        tauri::async_runtime::block_on(async {
            let mut config = TestConfig::load().unwrap();
            config.read_only = true;
            let session: Value =
                serde_json::from_slice(&crate::session::session_bytes(&config).await.unwrap())
                    .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("shared-live.redb")).unwrap();
            let mut shared_count = 0;
            let mut mail_count = 0;
            let mut calendar_count = 0;
            for (id, account) in session["accounts"].as_object().unwrap() {
                if account["isPersonal"] != false {
                    continue;
                }
                shared_count += 1;
                let scope = sync::scope(&config, id);
                if account_supports(&session, id, MAIL) {
                    let connection = Connection::discover_for(&config, id, MAIL)
                        .await
                        .unwrap_or_else(|e| panic!("{}", e.message()));
                    sync::pull_mailboxes(&store, &connection, &scope, id)
                        .await
                        .unwrap_or_else(|e| panic!("{}", e.message()));
                    let query = connection
                        .call("Email/query", json!({"accountId":id,"limit":1}))
                        .await
                        .unwrap_or_else(|e| panic!("{}", e.message()));
                    assert!(query["ids"].is_array());
                    let sample_ids = query["ids"].as_array().unwrap();
                    assert!(sample_ids.len() <= 1);
                    if !sample_ids.is_empty() {
                        let sample = connection.call("Email/get", json!({"accountId":id,"ids":sample_ids,"properties":["id","keywords"]}))
                            .await.unwrap_or_else(|error| panic!("{}", error.message()));
                        assert!(sample["list"].is_array());
                    }
                    mail_count += 1;
                }
                if account_supports(&session, id, CALENDARS) {
                    let connection = Connection::discover_for(&config, id, CALENDARS)
                        .await
                        .unwrap_or_else(|e| panic!("{}", e.message()));
                    crate::calendar::synchronize(&store, &connection, &scope, id)
                        .await
                        .unwrap_or_else(|e| panic!("{}", e.message()));
                    let calendar = crate::calendar::read(&store, &scope).unwrap();
                    if !calendar.access_denied {
                        assert!(calendar.event_state.is_some());
                        calendar_count += 1;
                    }
                }
            }
            println!("Freigegebene Konten: {shared_count}; Mailzugriff geprüft: {mail_count}; Kalenderzugriff geprüft: {calendar_count}");
        });
    }
    #[test]
    fn result_references_are_scoped_to_successful_named_calls() {
        let prior = std::collections::HashMap::from([
            (("query".into(), "Email/query".into()), json!({"ids":["a"]})),
            (
                ("get".into(), "Email/get".into()),
                json!({"list":[{"threadId":"t1"},{"threadId":"t2"}]}),
            ),
        ]);
        let args = resolve_references(
            &json!({"#ids":{"resultOf":"query","name":"Email/query","path":"/ids"}}),
            &prior,
        )
        .unwrap();
        assert_eq!(args["ids"], json!(["a"]));
        let threads = resolve_references(
            &json!({"#ids":{"resultOf":"get","name":"Email/get","path":"/list/*/threadId"}}),
            &prior,
        )
        .unwrap();
        assert_eq!(threads["ids"], json!(["t1", "t2"]));
        assert!(resolve_references(
            &json!({"#ids":{"resultOf":"query","name":"Mailbox/query","path":"/ids"}}),
            &prior
        )
        .is_err());
        assert!(resolve_references(
            &json!({"ids":[],"#ids":{"resultOf":"query","name":"Email/query","path":"/ids"}}),
            &prior
        )
        .is_err());
    }
    #[test]
    fn local_jmap_filters_threads_and_rejects_unimplemented_writes() {
        let mail = LocalMail {
            initialized: true,
            pending: vec![],
            pending_deletes: vec![],
            mailboxes: vec![],
            emails: vec![
                json!({"id":"a","threadId":"t","receivedAt":"2026-11-03","mailboxIds":{"inbox":true},"keywords":{},"subject":"Hello"}),
                json!({"id":"b","threadId":"t","receivedAt":"2026-11-02","mailboxIds":{"inbox":true},"keywords":{"$seen":true}}),
            ],
        };
        let result = answer_read(
            &mail,
            "account",
            "local:1",
            "Email/query",
            &json!({"filter":{"inMailbox":"inbox","notKeyword":"$seen"},"collapseThreads":true}),
        )
        .unwrap();
        assert_eq!(result["ids"], json!(["a"]));
        assert_eq!(
            answer_read(
                &mail,
                "account",
                "local:1",
                "Thread/get",
                &json!({"ids":["t"]})
            )
            .unwrap()["list"][0]["emailIds"],
            json!(["b", "a"])
        );
        assert!(answer_read(&mail, "account", "local:1", "Email/set", &json!({})).is_err());
        assert!(answer_read(
            &mail,
            "account",
            "local:1",
            "Email/query",
            &json!({"filter":{"unknown":true}})
        )
        .is_err());
    }

    #[test]
    fn local_jmap_saves_and_discards_redb_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("desktop-draft.redb")).unwrap();
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/.well-known/jmap").unwrap(),
            username: "user@example.test".into(),
            password: "unused-test-secret".into(),
            read_only: true,
        };
        store
            .put(
                &format!("{}:desktop-session", config.cache_key()),
                &json!({
                    "accounts":{"primary":{"isReadOnly":true}}
                })
                .to_string(),
            )
            .unwrap();
        let mail = store.local_mail(&sync::scope(&config, "primary")).unwrap();
        let saved = answer_email_set(
            &store,
            &config,
            "primary",
            &mail,
            "local:empty:empty:0",
            &json!({
                "accountId":"primary",
                "create":{"draft-1":{
                    "from":[{"email":"sender@example.test"}],
                    "to":[{"email":"recipient@example.test"}],
                    "subject":"Offline draft",
                    "keywords":{"$seen":true,"$draft":true},
                    "mailboxIds":{"drafts":true},
                    "bodyValues":{"1":{"value":"Persist me"}},
                    "textBody":[{"partId":"1"}]
                }}
            }),
        )
        .unwrap();
        let id = saved["created"]["draft-1"]["id"].as_str().unwrap();
        let mail = store.local_mail(&sync::scope(&config, "primary")).unwrap();
        assert_eq!(mail.emails[0]["id"], id);
        assert_eq!(mail.emails[0]["subject"], "Offline draft");
        let removed = answer_email_set(
            &store,
            &config,
            "primary",
            &mail,
            "local:state",
            &json!({
                "accountId":"primary","destroy":[id]
            }),
        )
        .unwrap();
        assert_eq!(removed["destroyed"], json!([id]));
        assert!(store
            .local_mail(&sync::scope(&config, "primary"))
            .unwrap()
            .emails
            .is_empty());
    }
}
