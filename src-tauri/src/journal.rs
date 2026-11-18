//! Durable, account-scoped write intentions. In-flight sends are never replayed.
use crate::storage::Store;
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub type Calls = Vec<(String, Value, String)>;
const TABLE: TableDefinition<&str, &str> = TableDefinition::new("desktop_write_journal_v1");
const HISTORY: TableDefinition<&str, &str> =
    TableDefinition::new("desktop_write_resolution_history_v1");

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    pub kind: String,
    pub object_id: String,
    pub title: String,
    pub action: String,
    pub status: String,
    pub calls: Calls,
    pub local: Option<Value>,
    pub base_state: Option<String>,
    pub error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    imports: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    import_previous: std::collections::BTreeMap<String, Option<Value>>,
    next: u64,
    entries: Vec<Entry>,
    #[serde(default)]
    mappings: std::collections::BTreeMap<String, String>,
}
fn err(_: impl std::fmt::Display) -> String {
    "Lokale Änderungsablage fehlgeschlagen.".into()
}
fn mutate<T>(
    store: &Store,
    scope: &str,
    update: impl FnOnce(&mut Journal) -> Result<T, String>,
) -> Result<T, String> {
    let tx = store.0.begin_write().map_err(err)?;
    let result;
    {
        let mut table = tx.open_table(TABLE).map_err(err)?;
        let raw = table.get(scope).map_err(err)?.map(|r| r.value().to_owned());
        let mut journal: Journal = raw
            .map(|s| serde_json::from_str(&s).map_err(err))
            .transpose()?
            .unwrap_or_default();
        result = update(&mut journal)?;
        let raw = serde_json::to_string(&journal).map_err(err)?;
        if raw.len() > 32 * 1024 * 1024 {
            return Err("Lokale Änderungsablage ist voll.".into());
        }
        table.insert(scope, raw.as_str()).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(result)
}
pub fn initialize(store: &Store) -> Result<(), String> {
    let tx = store.0.begin_write().map_err(err)?;
    tx.open_table(HISTORY).map_err(err)?;
    {
        let mut table = tx.open_table(TABLE).map_err(err)?;
        let rows = table
            .iter()
            .map_err(err)?
            .map(|row| {
                row.map(|(key, value)| (key.value().to_owned(), value.value().to_owned()))
                    .map_err(err)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (scope, raw) in rows {
            let mut journal: Journal = serde_json::from_str(&raw).map_err(err)?;
            for entry in &mut journal.entries {
                if entry.status == "sending" {
                    entry.status = "uncertain".into();
                    entry.error =
                        Some("App während der Übertragung beendet; Serverstatus prüfen".into());
                } else if entry.kind == "contact"
                    && ["held", "ready", "blocked"].contains(&entry.status.as_str())
                {
                    entry.status = "blocked".into();
                    entry.error =
                        Some("Kontaktimport wurde in eine eigene App ausgelagert.".into());
                }
            }
            table
                .insert(
                    scope.as_str(),
                    serde_json::to_string(&journal).map_err(err)?.as_str(),
                )
                .map_err(err)?;
        }
    }
    tx.commit().map_err(err)
}
pub fn entries(store: &Store, scope: &str) -> Result<Vec<Entry>, String> {
    let tx = store.0.begin_read().map_err(err)?;
    let table = tx.open_table(TABLE).map_err(err)?;
    let raw = table.get(scope).map_err(err)?;
    let journal: Journal = raw
        .map(|r| serde_json::from_str(r.value()).map_err(err))
        .transpose()?
        .unwrap_or_default();
    Ok(journal.entries)
}

pub(crate) fn ensure_draft_editable(
    tx: &redb::WriteTransaction,
    scope: &str,
    id: &str,
) -> Result<(), String> {
    let table = tx.open_table(TABLE).map_err(err)?;
    if let Some(row) = table.get(scope).map_err(err)? {
        let journal: Journal = serde_json::from_str(row.value()).map_err(err)?;
        if journal
            .entries
            .iter()
            .any(|entry| entry.kind == "draft" && entry.object_id == id)
        {
            return Err(
                "Zuerst den vorgemerkten Entwurfsabgleich abschließen oder verwerfen.".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn queue_draft(
    store: &Store,
    scope: &str,
    account: &str,
    id: &str,
) -> Result<(), String> {
    mutate(store, scope, |journal| {
        if journal
            .entries
            .iter()
            .any(|entry| entry.kind == "draft" && entry.object_id == id)
        {
            return Err("Dieser Entwurf ist bereits vorgemerkt.".into());
        }
        let draft = store.local_draft(scope, id)?;
        let title = draft["subject"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Ohne Betreff")
            .to_owned();
        let mut payload = draft.as_object().cloned().ok_or("Ungültiger Entwurf.")?;
        for key in ["id", "threadId", "receivedAt"] {
            payload.remove(key);
        }
        journal.next = journal
            .next
            .checked_add(1)
            .ok_or("Änderungsnummer ausgeschöpft.")?;
        journal.entries.push(Entry {
            id: journal.next,
            kind: "draft".into(),
            object_id: id.into(),
            title,
            action: "Als Entwurf auf Stalwart speichern".into(),
            status: "held".into(),
            calls: vec![(
                "Email/set".into(),
                json!({"accountId":account,"create":{"draft":payload}}),
                "draft".into(),
            )],
            local: Some(draft),
            base_state: None,
            error: None,
        });
        Ok(())
    })
}
pub fn revision(store: &Store, scope: &str) -> Result<u64, String> {
    let tx = store.0.begin_read().map_err(err)?;
    let table = tx.open_table(TABLE).map_err(err)?;
    let journal: Journal = table
        .get(scope)
        .map_err(err)?
        .map(|r| serde_json::from_str(r.value()).map_err(err))
        .transpose()?
        .unwrap_or_default();
    Ok(journal.next)
}
pub fn hold(store: &Store, scope: &str) -> Result<(), String> {
    mutate(store, scope, |j| {
        for e in &mut j.entries {
            if e.status == "ready" {
                e.status = "held".into();
            }
        }
        Ok(())
    })
}
pub fn approve(store: &Store, scope: &str, ids: &[u64]) -> Result<(), String> {
    mutate(store, scope, |j| {
        for e in &mut j.entries {
            if ids.contains(&e.id) && e.status == "held" {
                e.status = "ready".into();
            }
        }
        Ok(())
    })
}

/// Replace a definitively refused operation, invalidating every old approval.
/// Rebase runs under the redb writer lock, against the latest committed baseline.
pub fn retry(
    store: &Store,
    scope: &str,
    id: u64,
    rebase: impl FnOnce(&Entry) -> Result<(Option<String>, Option<Value>), String>,
) -> Result<u64, String> {
    mutate(store, scope, |j| {
        let index = j
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or("Auftrag wurde inzwischen geändert.")?;
        let entry = &j.entries[index];
        if entry.status != "blocked" || !["calendar", "file"].contains(&entry.kind.as_str()) {
            return Err("Dieser Auftrag darf nicht erneut vorgemerkt werden.".into());
        }
        if j.entries
            .iter()
            .any(|e| e.id != id && e.kind == entry.kind && e.object_id == entry.object_id)
        {
            return Err("Für dieses Objekt liegt eine weitere lokale Änderung vor.".into());
        }
        let (state, local) = rebase(entry)?;
        j.next = j
            .next
            .checked_add(1)
            .ok_or("Änderungsnummer ausgeschöpft.")?;
        let entry = &mut j.entries[index];
        entry.id = j.next;
        entry.status = "held".into();
        entry.error = None;
        entry.base_state = state;
        entry.local = local;
        Ok(entry.id)
    })
}

fn depends_on(value: &Value, id: &str, field: &str) -> bool {
    match value {
        Value::String(value) => {
            ["parentId", "id", "ids", "destroy"].contains(&field) && value == id
        }
        Value::Array(items) => items.iter().any(|v| depends_on(v, id, field)),
        Value::Object(map) => map.iter().any(|(key, value)| {
            (field == "calendarIds" && key == id) || depends_on(value, id, key)
        }),
        _ => false,
    }
}

/// Archive the exact local intent without changing server objects. Uncertain
/// results retain their original receipt and payload in a separate redb table.
pub fn discard(
    store: &Store,
    scope: &str,
    id: u64,
    acknowledge_uncertain: bool,
) -> Result<(), String> {
    let tx = store.0.begin_write().map_err(err)?;
    {
        let mut table = tx.open_table(TABLE).map_err(err)?;
        let raw = table
            .get(scope)
            .map_err(err)?
            .map(|v| v.value().to_owned())
            .ok_or("Auftrag fehlt.")?;
        let mut journal: Journal = serde_json::from_str(&raw).map_err(err)?;
        let index = journal
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or("Auftrag wurde inzwischen geändert.")?;
        let entry = &journal.entries[index];
        if !["held", "ready", "blocked", "uncertain"].contains(&entry.status.as_str())
            || (entry.status == "uncertain" && !acknowledge_uncertain)
        {
            return Err(
                "Serverstatus zuerst prüfen; laufende oder bestätigte Aufträge bleiben erhalten."
                    .into(),
            );
        }
        if entry.object_id.starts_with("local-change-")
            && !journal.mappings.contains_key(&entry.object_id)
            && journal.entries.iter().any(|e| {
                e.id != id
                    && e.calls
                        .iter()
                        .any(|(_, args, _)| depends_on(args, &entry.object_id, ""))
            })
        {
            return Err("Untergeordnete lokale Aufträge zuerst verwerfen.".into());
        }
        let archived = json!({"entry":entry,"resolution":if entry.status=="uncertain"{"checked_locally"}else{"discarded_locally"}});
        tx.open_table(HISTORY)
            .map_err(err)?
            .insert(
                serde_json::to_string(&(scope, id)).map_err(err)?.as_str(),
                archived.to_string().as_str(),
            )
            .map_err(err)?;
        if entry.kind == "contact" && entry.status != "uncertain" {
            if let Some(uid) = entry.local.as_ref().and_then(|v| v["uid"].as_str()) {
                if let Some(previous) = journal.import_previous.remove(uid) {
                    if let Some(value) = previous {
                        journal.imports.insert(uid.into(), value);
                    } else {
                        journal.imports.remove(uid);
                    }
                }
            }
        }
        journal.entries.remove(index);
        journal.next = journal
            .next
            .checked_add(1)
            .ok_or("Änderungsnummer ausgeschöpft.")?;
        table
            .insert(
                scope,
                serde_json::to_string(&journal).map_err(err)?.as_str(),
            )
            .map_err(err)?;
    }
    tx.commit().map_err(err)
}
pub fn enqueue_send(
    store: &Store,
    scope: &str,
    calls: Calls,
    hold: bool,
    draft_id: Option<&str>,
    draft_account: Option<&str>,
) -> Result<u64, String> {
    let email = calls
        .iter()
        .find(|c| c.0 == "Email/set")
        .and_then(|c| c.1["create"].as_object())
        .and_then(|m| m.values().next())
        .cloned()
        .ok_or("Ungültige Nachricht.")?;
    mutate(store, scope, |j| {
        j.next = j
            .next
            .checked_add(1)
            .ok_or("Änderungsnummer ist ausgeschöpft.")?;
        let id = j.next;
        let mut local = email;
        if let Some(previous) = draft_id.filter(|id| id.starts_with("local-send-")) {
            if !j
                .entries
                .iter()
                .any(|e| e.object_id == previous && ["held", "ready"].contains(&e.status.as_str()))
            {
                return Err(
                    "Versandstatus zuerst prüfen; Auftrag kann nicht erneut vorgemerkt werden."
                        .into(),
                );
            }
            j.entries.retain(|e| e.object_id != previous);
        }
        local["id"] = json!(format!("local-send-{id}"));
        local["desktopQueuedAt"] = json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_secs());
        local["desktopSourceDraftId"] = json!(draft_id);
        local["desktopSourceDraftAccountId"] = json!(draft_account);
        j.entries.push(Entry {
            id,
            kind: "send".into(),
            object_id: format!("local-send-{id}"),
            title: local["subject"]
                .as_str()
                .unwrap_or("Ohne Betreff")
                .to_owned(),
            action: "Mail versenden".into(),
            status: if hold { "held" } else { "ready" }.into(),
            calls,
            local: Some(local),
            base_state: None,
            error: None,
        });
        Ok(id)
    })
}
pub fn patch(object: &mut Value, changes: &Value) -> Result<(), String> {
    let changes = changes.as_object().ok_or("invalidPatch")?;
    let keys: Vec<_> = changes.keys().collect();
    if keys.iter().any(|a| {
        keys.iter()
            .any(|b| a != b && b.starts_with(&format!("{a}/")))
    }) {
        return Err("invalidPatch".into());
    }
    for (path, value) in changes {
        let parts: Vec<_> = path
            .split('/')
            .map(|p| p.replace("~1", "/").replace("~0", "~"))
            .collect();
        if parts.is_empty() || parts[0] == "id" {
            return Err("invalidPatch".into());
        }
        let mut node = &mut *object;
        for part in &parts[..parts.len() - 1] {
            node = node.get_mut(part).ok_or("invalidPatch")?;
        }
        let map = node.as_object_mut().ok_or("invalidPatch")?;
        let name = parts.last().ok_or("invalidPatch")?;
        if value.is_null() {
            map.remove(name);
        } else {
            map.insert(name.clone(), value.clone());
        }
    }
    Ok(())
}
pub fn overlay(
    store: &Store,
    scope: &str,
    kind: &str,
    objects: &mut std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    for entry in entries(store, scope)?.iter().filter(|e| e.kind == kind) {
        if let Some(local) = &entry.local {
            objects.insert(entry.object_id.clone(), local.clone());
        } else {
            objects.remove(&entry.object_id);
        }
    }
    Ok(())
}
pub fn queue_object(
    store: &Store,
    scope: &str,
    kind: &str,
    method: &str,
    args: Value,
    objects: &std::collections::BTreeMap<String, Value>,
    base_state: Option<String>,
    hold: bool,
) -> Result<Value, String> {
    if !args.is_object()
        || ["create", "update"]
            .iter()
            .any(|k| args.get(k).is_some_and(|v| !v.is_object()))
        || args.get("destroy").is_some_and(|v| !v.is_array())
    {
        return Err("invalidArguments".into());
    }
    mutate(store, scope, |j| {
        let mut created = json!({});
        let mut updated = json!({});
        let mut destroyed = vec![];
        let mut view = objects.clone();
        for (action, items) in [
            ("create", args["create"].as_object()),
            ("update", args["update"].as_object()),
        ] {
            for (key, value) in items.into_iter().flatten() {
                let mut local = if action == "create" {
                    value.clone()
                } else {
                    view.get(key).cloned().ok_or("notFound")?
                };
                if action == "create" && kind == "file" {
                    local["myRights"] = json!({"mayRead":true,"mayAddChildren":true,"mayRename":true,"mayDelete":true,"mayModifyContent":true,"mayShare":false});
                    if local.get("parentId").is_none() {
                        local["parentId"] = Value::Null;
                    }
                    if let Some(blob) = local["blobId"]
                        .as_str()
                        .filter(|id| id.starts_with(crate::uploads::PREFIX))
                    {
                        local["size"] = json!(crate::uploads::describe(store, scope, blob)?.size);
                    }
                }
                if action == "update" {
                    patch(&mut local, value)?;
                    if j.entries.iter().any(|e| {
                        e.kind == kind
                            && e.object_id == *key
                            && ["sending", "uncertain", "applied"].contains(&e.status.as_str())
                    }) {
                        return Err("Serverstatus dieser Änderung zuerst prüfen.".into());
                    }
                }
                j.next = j
                    .next
                    .checked_add(1)
                    .ok_or("Änderungsnummer ist ausgeschöpft.")?;
                if let Some(existing) = j.entries.iter_mut().find(|e| {
                    e.kind == kind
                        && e.object_id == *key
                        && ["held", "ready", "blocked"].contains(&e.status.as_str())
                }) {
                    // A later RSVP must survive folding into an earlier local
                    // no-notification edit/create. Further quiet edits must not
                    // silently withdraw the already requested scheduling reply.
                    if kind == "calendar" && args["sendSchedulingMessages"] == true {
                        existing.calls[0].1["sendSchedulingMessages"] = json!(true);
                    }
                    let was_blocked = existing.status == "blocked";
                    if was_blocked {
                        existing.base_state = base_state.clone();
                        existing.error = None;
                        existing.status = if hold { "held" } else { "ready" }.into();
                    }
                    // Fold edits to a not-yet-created local object into its durable create.
                    if let Some(create) = existing.calls[0].1["create"].as_object_mut() {
                        let name = create.keys().next().cloned().ok_or("invalidArguments")?;
                        let mut payload = local.clone();
                        payload
                            .as_object_mut()
                            .ok_or("invalidArguments")?
                            .remove("id");
                        if kind == "file" {
                            let object = payload.as_object_mut().ok_or("invalidArguments")?;
                            object.remove("myRights");
                            object.remove("size");
                        }
                        create.insert(name, payload);
                        existing.local = Some(local.clone());
                        existing.id = j.next;
                        existing.title = local["title"]
                            .as_str()
                            .or(local["name"].as_str())
                            .or(local["name"]["full"].as_str())
                            .unwrap_or("Ohne Titel")
                            .into();
                        existing.status = if hold {
                            "held"
                        } else {
                            existing.status.as_str()
                        }
                        .to_owned();
                        updated[key] = Value::Null;
                        view.insert(key.clone(), local);
                        continue;
                    }
                    if let Some(update) = existing.calls[0].1["update"][key].as_object_mut() {
                        for path in value.as_object().ok_or("invalidPatch")?.keys() {
                            let root = path.split('/').next().ok_or("invalidPatch")?;
                            update.retain(|k, _| k.split('/').next() != Some(root));
                            let property = root.replace("~1", "/").replace("~0", "~");
                            update.insert(
                                root.into(),
                                local.get(&property).cloned().unwrap_or(Value::Null),
                            );
                        }
                        existing.local = Some(local.clone());
                        existing.id = j.next;
                        existing.title = local["title"]
                            .as_str()
                            .or(local["name"].as_str())
                            .or(local["name"]["full"].as_str())
                            .unwrap_or("Ohne Titel")
                            .into();
                        if hold {
                            existing.status = "held".into();
                        }
                        updated[key] = Value::Null;
                        view.insert(key.clone(), local);
                        continue;
                    }
                }
                let object_id = if action == "create" {
                    format!("local-change-{}", j.next)
                } else {
                    key.clone()
                };
                local["id"] = json!(object_id);
                let mut call_args = args.clone();
                call_args
                    .as_object_mut()
                    .ok_or("invalidArguments")?
                    .retain(|k, _| {
                        !["create", "update", "destroy", "ifInState"].contains(&k.as_str())
                    });
                call_args[action] = json!({ key: value });
                let title = local["title"]
                    .as_str()
                    .or(local["name"].as_str())
                    .or(local["name"]["full"].as_str())
                    .unwrap_or("Ohne Titel")
                    .to_owned();
                j.entries.push(Entry {
                    id: j.next,
                    kind: kind.into(),
                    object_id: object_id.clone(),
                    title,
                    action: if action == "create" {
                        "Anlegen"
                    } else {
                        "Ändern"
                    }
                    .into(),
                    status: if hold { "held" } else { "ready" }.into(),
                    calls: vec![(method.into(), call_args, "write".into())],
                    local: Some(local.clone()),
                    base_state: base_state.clone(),
                    error: None,
                });
                if action == "create" {
                    created[key] = json!({"id":object_id});
                } else {
                    updated[key] = Value::Null;
                }
                view.insert(object_id, local);
            }
        }
        for id in args["destroy"].as_array().into_iter().flatten() {
            let id = id.as_str().ok_or("invalidArguments")?;
            let local = view.get(id).ok_or("notFound")?;
            if id.starts_with("local-change-") {
                if j.entries.iter().any(|e| {
                    e.kind == kind
                        && e.object_id == id
                        && !["held", "ready", "blocked"].contains(&e.status.as_str())
                }) {
                    return Err("Serverstatus dieser Änderung zuerst prüfen.".into());
                }
                j.entries.retain(|e| !(e.kind == kind && e.object_id == id));
            } else {
                if j.entries.iter().any(|e| {
                    e.kind == kind
                        && e.object_id == id
                        && ["sending", "uncertain", "applied"].contains(&e.status.as_str())
                }) {
                    return Err("Serverstatus dieser Änderung zuerst prüfen.".into());
                }
                j.entries.retain(|e| !(e.kind == kind && e.object_id == id));
                j.next = j
                    .next
                    .checked_add(1)
                    .ok_or("Änderungsnummer ist ausgeschöpft.")?;
                let mut delete_args = json!({"accountId":args["accountId"],"destroy":[id]});
                if let Some(flag) = args.get("sendSchedulingMessages") {
                    delete_args["sendSchedulingMessages"] = flag.clone();
                }
                j.entries.push(Entry {
                    id: j.next,
                    kind: kind.into(),
                    object_id: id.into(),
                    title: local["title"]
                        .as_str()
                        .or(local["name"].as_str())
                        .or(local["name"]["full"].as_str())
                        .unwrap_or("Ohne Titel")
                        .into(),
                    action: "Löschen".into(),
                    status: if hold { "held" } else { "ready" }.into(),
                    calls: vec![(method.into(), delete_args, "write".into())],
                    local: None,
                    base_state: base_state.clone(),
                    error: None,
                });
            }
            destroyed.push(id.to_owned());
        }
        Ok(
            json!({"accountId":args["accountId"],"oldState":"local","newState":format!("local:{}",j.next),"created":created,"updated":updated,"destroyed":destroyed}),
        )
    })
}
/// Persist before transport. A crash or lost response remains visibly uncertain.
pub fn claim(store: &Store, scope: &str, id: u64) -> Result<bool, String> {
    mutate(store, scope, |j| {
        let Some(e) = j
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.status == "ready")
        else {
            return Ok(false);
        };
        e.status = "sending".into();
        Ok(true)
    })
}
pub fn finish(
    store: &Store,
    scope: &str,
    id: u64,
    status: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    mutate(store, scope, |j| {
        if let Some(status) = status {
            if let Some(e) = j.entries.iter_mut().find(|e| e.id == id) {
                e.status = status.into();
                e.error = error.map(str::to_owned);
            }
        } else {
            if let Some(uid) = j
                .entries
                .iter()
                .find(|e| e.id == id && e.kind == "contact")
                .and_then(|e| e.local.as_ref())
                .and_then(|v| v["uid"].as_str())
                .map(str::to_owned)
            {
                j.import_previous.remove(&uid);
            }
            j.entries.retain(|e| e.id != id);
        }
        Ok(())
    })
}

pub fn applied(store: &Store, scope: &str, id: u64, new_state: Option<&str>) -> Result<(), String> {
    mutate(store, scope, |j| {
        let Some(index) = j.entries.iter().position(|e| e.id == id) else {
            return Ok(());
        };
        let old_state = j.entries[index].base_state.clone();
        let kind = j.entries[index].kind.clone();
        j.entries[index].status = "applied".into();
        if let Some(next) = new_state {
            for e in &mut j.entries {
                if e.kind == kind && e.base_state == old_state {
                    e.base_state = Some(next.into());
                }
            }
        }
        Ok(())
    })
}
pub fn clear_applied(store: &Store, scope: &str, kind: &str) -> Result<(), String> {
    mutate(store, scope, |j| {
        j.entries
            .retain(|e| e.kind != kind || e.status != "applied");
        Ok(())
    })
}

pub(crate) fn clear_confirmed_drafts(
    store: &Store,
    scope: &str,
    server_emails: &[Value],
) -> Result<(), String> {
    let confirmed: std::collections::BTreeSet<_> = server_emails
        .iter()
        .filter(|email| email["keywords"]["$draft"] == true)
        .filter_map(|email| email["id"].as_str())
        .collect();
    let applied = mutate(store, scope, |journal| {
        Ok(journal
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == "draft"
                    && entry.status == "applied"
                    && journal
                        .mappings
                        .get(&entry.object_id)
                        .is_some_and(|id| confirmed.contains(id.as_str()))
            })
            .map(|entry| (entry.id, entry.object_id.clone()))
            .collect::<Vec<_>>())
    })?;
    for (entry_id, draft_id) in applied {
        store.remove_local_draft(scope, &draft_id, true)?;
        mutate(store, scope, |journal| {
            journal.entries.retain(|entry| entry.id != entry_id);
            journal.mappings.remove(&draft_id);
            Ok(())
        })?;
    }
    Ok(())
}

pub(crate) fn confirm_draft(
    store: &Store,
    scope: &str,
    entry_id: u64,
    remote_id: &str,
) -> Result<(), String> {
    mutate(store, scope, |journal| {
        let entry = journal
            .entries
            .iter_mut()
            .find(|entry| {
                entry.id == entry_id && entry.kind == "draft" && entry.status == "sending"
            })
            .ok_or("Vorgemerkter Entwurf fehlt.")?;
        entry.status = "applied".into();
        journal
            .mappings
            .insert(entry.object_id.clone(), remote_id.into());
        Ok(())
    })
}

/// A successful submission can be acknowledged before the Sent mailbox is
/// visible in the replica. Keep its local copy until a later mail sync reads
/// back the same RFC Message-ID.
pub fn clear_applied_sends_after_readback(
    store: &Store,
    scope: &str,
    server_emails: &[Value],
) -> Result<(), String> {
    let confirmed: std::collections::BTreeSet<String> = server_emails
        .iter()
        .flat_map(|email| email["messageId"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    mutate(store, scope, |journal| {
        journal.entries.retain(|entry| {
            if entry.kind != "send" || entry.status != "applied" {
                return true;
            }
            let ids = entry
                .local
                .as_ref()
                .and_then(|email| email["messageId"].as_array())
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            !ids.into_iter().any(|id| confirmed.contains(id))
        });
        Ok(())
    })
}

pub fn remember_created(
    store: &Store,
    scope: &str,
    local_id: &str,
    server_id: &str,
) -> Result<(), String> {
    mutate(store, scope, |j| {
        j.mappings.insert(local_id.into(), server_id.into());
        Ok(())
    })
}
pub fn resolve_calls(store: &Store, scope: &str, calls: &Calls) -> Result<Calls, String> {
    let tx = store.0.begin_read().map_err(err)?;
    let table = tx.open_table(TABLE).map_err(err)?;
    let journal: Journal = table
        .get(scope)
        .map_err(err)?
        .map(|r| serde_json::from_str(r.value()).map_err(err))
        .transpose()?
        .unwrap_or_default();
    fn resolve(
        value: &Value,
        mapping: &std::collections::BTreeMap<String, String>,
        context: &str,
    ) -> Result<Value, String> {
        Ok(match value {
            Value::String(s)
                if ["parentId", "id", "ids", "destroy"].contains(&context)
                    && s.starts_with("local-change-") =>
            {
                json!(mapping
                    .get(s)
                    .ok_or("Übergeordnete lokale Änderung zuerst freigeben.")?)
            }
            Value::Array(a) => Value::Array(
                a.iter()
                    .map(|v| resolve(v, mapping, context))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| {
                        let key = if ["update", "calendarIds"].contains(&context)
                            && k.starts_with("local-change-")
                        {
                            mapping
                                .get(k)
                                .ok_or("Lokale Änderung zuerst freigeben.")?
                                .clone()
                        } else {
                            k.clone()
                        };
                        Ok((key, resolve(v, mapping, k)?))
                    })
                    .collect::<Result<_, String>>()?,
            ),
            _ => value.clone(),
        })
    }
    calls
        .iter()
        .map(|(method, args, id)| {
            Ok((
                method.clone(),
                resolve(args, &journal.mappings, "")?,
                id.clone(),
            ))
        })
        .collect()
}

pub fn edit_send(store: &Store, scope: &str, id: u64) -> Result<String, String> {
    let tx = store.0.begin_write().map_err(err)?;
    let draft_id;
    {
        let mut table = tx.open_table(TABLE).map_err(err)?;
        let raw = table
            .get(scope)
            .map_err(err)?
            .map(|r| r.value().to_owned())
            .ok_or("Versandauftrag fehlt.")?;
        let mut journal: Journal = serde_json::from_str(&raw).map_err(err)?;
        let index = journal
            .entries
            .iter()
            .position(|e| {
                e.id == id
                    && e.kind == "send"
                    && ["held", "ready", "blocked"].contains(&e.status.as_str())
            })
            .ok_or("Versandstatus zuerst prüfen.")?;
        let mut email = journal.entries[index]
            .local
            .clone()
            .ok_or("Nachricht fehlt.")?;
        let mut metadata = tx.open_table(crate::replica::REPLICAS).map_err(err)?;
        let mut meta: crate::replica::Metadata = metadata
            .get(scope)
            .map_err(err)?
            .map(|r| serde_json::from_str(r.value()).map_err(err))
            .transpose()?
            .unwrap_or_default();
        meta.next_operation = meta
            .next_operation
            .checked_add(1)
            .ok_or("Entwurfsnummer ist ausgeschöpft.")?;
        draft_id = format!("local-draft-{:020}", meta.next_operation);
        email["id"] = json!(draft_id);
        email["threadId"] = json!(draft_id);
        email
            .as_object_mut()
            .ok_or("Nachricht ist ungültig.")?
            .retain(|key, _| !key.starts_with("desktop"));
        let key = format!("{scope}\0Email\0{draft_id}");
        tx.open_table(crate::replica::DRAFTS)
            .map_err(err)?
            .insert(key.as_str(), email.to_string().as_str())
            .map_err(err)?;
        metadata
            .insert(scope, serde_json::to_string(&meta).map_err(err)?.as_str())
            .map_err(err)?;
        journal.entries.remove(index);
        table
            .insert(
                scope,
                serde_json::to_string(&journal).map_err(err)?.as_str(),
            )
            .map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(draft_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_contact_imports_cannot_be_approved_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contact-import.redb");
        let store = Store::open(&path).unwrap();
        for (kind, method) in [("contact", "ContactCard/set"), ("file", "FileNode/set")] {
            queue_object(
                &store,
                "a",
                kind,
                method,
                json!({"accountId":"a","create":{"sample":{"name":"Sample"}}}),
                &BTreeMap::new(),
                None,
                false,
            )
            .unwrap();
        }
        drop(store);

        let store = Store::open(&path).unwrap();
        let queued = entries(&store, "a").unwrap();
        assert_eq!(queued[0].status, "blocked");
        assert_eq!(queued[1].status, "ready");
        approve(&store, "a", &[queued[0].id]).unwrap();
        assert!(!claim(&store, "a", queued[0].id).unwrap());
        discard(&store, "a", queued[0].id, false).unwrap();
        assert_eq!(entries(&store, "a").unwrap().len(), 1);
    }

    #[test]
    fn discard_protects_local_dependencies_and_archives_uncertain_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resolution.redb");
        let store = Store::open(&path).unwrap();
        let objects = std::collections::BTreeMap::new();
        let parent = queue_object(
            &store,
            "a",
            "file",
            "FileNode/set",
            json!({"accountId":"a","create":{"parent":{"name":"Parent"}}}),
            &objects,
            Some("s1".into()),
            true,
        )
        .unwrap();
        let parent_object = parent["created"]["parent"]["id"].as_str().unwrap();
        let parent_id = entries(&store, "a").unwrap()[0].id;
        queue_object(
            &store,
            "a",
            "file",
            "FileNode/set",
            json!({"accountId":"a","create":{"child":{"name":"Child","parentId":parent_object}}}),
            &objects,
            Some("s1".into()),
            true,
        )
        .unwrap();
        let child_id = entries(&store, "a").unwrap()[1].id;
        assert!(discard(&store, "a", parent_id, false).is_err());
        assert_eq!(entries(&store, "a").unwrap().len(), 2);
        discard(&store, "a", child_id, false).unwrap();
        discard(&store, "a", parent_id, false).unwrap();
        let calls = vec![(
            "Email/set".into(),
            json!({"accountId":"a","create":{"e":{"subject":"Keep receipt","messageId":["stable"]}}}),
            "create".into(),
        )];
        let send = enqueue_send(&store, "a", calls.clone(), false, None, None).unwrap();
        assert!(claim(&store, "a", send).unwrap());
        assert!(discard(&store, "a", send, true).is_err());
        finish(&store, "a", send, Some("uncertain"), Some("timeout")).unwrap();
        assert!(discard(&store, "a", send, false).is_err());
        discard(&store, "a", send, true).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let tx = store.0.begin_read().unwrap();
        let table = tx.open_table(HISTORY).unwrap();
        let key = serde_json::to_string(&("a", send)).unwrap();
        let row = table.get(key.as_str()).unwrap().unwrap();
        let receipt: Value = serde_json::from_str(row.value()).unwrap();
        assert_eq!(receipt["resolution"], "checked_locally");
        assert_eq!(receipt["entry"]["status"], "uncertain");
        assert_eq!(receipt["entry"]["calls"], json!(calls));
        assert_eq!(receipt["entry"]["error"], "timeout");
        assert!(entries(&store, "a").unwrap().is_empty());
    }
    use std::collections::BTreeMap;
    fn send(subject: &str) -> Calls {
        vec![
            (
                "Email/set".into(),
                json!({"accountId":"a","create":{"mail":{"subject":subject,"keywords":{"$draft":true},"mailboxIds":{"drafts":true},"bodyValues":{"text":{"value":"durable body"}},"textBody":[{"partId":"text","type":"text/plain"}]}}}),
                "0".into(),
            ),
            (
                "EmailSubmission/set".into(),
                json!({"accountId":"a","create":{"1":{"emailId":"#mail","identityId":"identity"}}}),
                "1".into(),
            ),
            (
                "EmailSubmission/get".into(),
                json!({"accountId":"a"}),
                "2".into(),
            ),
        ]
    }
    #[test]
    fn offline_send_survives_restart_requires_approval_and_uncertain_sends_never_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.redb");
        let id;
        {
            let store = Store::open(&path).unwrap();
            id = enqueue_send(&store, "a", send("Offline"), true, None, None).unwrap();
            enqueue_send(&store, "b", send("Other account"), true, None, None).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(entries(&store, "a").unwrap()[0].status, "held");
        assert_eq!(
            store.local_mail("a").unwrap().emails[0]["bodyValues"]["text"]["value"],
            "durable body"
        );
        assert!(!claim(&store, "a", id).unwrap());
        approve(&store, "a", &[id]).unwrap();
        assert!(claim(&store, "a", id).unwrap());
        assert_eq!(entries(&store, "a").unwrap()[0].status, "sending");
        approve(&store, "a", &[id]).unwrap();
        assert!(!claim(&store, "a", id).unwrap());
        assert_eq!(entries(&store, "b").unwrap()[0].status, "held");
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(entries(&store, "a").unwrap()[0].status, "uncertain");
        assert!(!claim(&store, "a", id).unwrap());
    }
    #[test]
    fn editing_withdraws_send_and_preserves_body_as_local_draft_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edit.redb");
        let store = Store::open(&path).unwrap();
        let id = enqueue_send(&store, "a", send("Editable"), true, None, None).unwrap();
        assert!(edit_send(&store, "b", id).is_err());
        let draft = edit_send(&store, "a", id).unwrap();
        assert!(entries(&store, "a").unwrap().is_empty());
        assert!(!claim(&store, "a", id).unwrap());
        drop(store);
        let store = Store::open(&path).unwrap();
        let emails = store.local_mail("a").unwrap().emails;
        assert_eq!(emails[0]["id"], draft);
        assert_eq!(emails[0]["bodyValues"]["text"]["value"], "durable body");
        let id = enqueue_send(&store, "a", send("Uncertain"), false, None, None).unwrap();
        assert!(claim(&store, "a", id).unwrap());
        assert!(edit_send(&store, "a", id).is_err());
    }
    #[test]
    fn calendar_create_and_nested_edits_stay_local_and_invalidate_old_approval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calendar.redb");
        let store = Store::open(&path).unwrap();
        let objects = BTreeMap::new();
        let result=queue_object(&store,"a","calendar","CalendarEvent/set",json!({"accountId":"a","create":{"new":{"title":"First","calendarIds":{"cal":true},"start":"2026-11-05T10:00:00","duration":"PT1H"}}}),&objects,Some("state1".into()),true).unwrap();
        let id = result["created"]["new"]["id"].as_str().unwrap().to_owned();
        let old = entries(&store, "a").unwrap()[0].id;
        let mut objects = objects;
        overlay(&store, "a", "calendar", &mut objects).unwrap();
        queue_object(
            &store,
            "a",
            "calendar",
            "CalendarEvent/set",
            json!({"accountId":"a","update":{&id:{"title":"Changed"}}}),
            &objects,
            Some("state1".into()),
            true,
        )
        .unwrap();
        approve(&store, "a", &[old]).unwrap();
        let entry = entries(&store, "a").unwrap().remove(0);
        assert_ne!(entry.id, old);
        assert_eq!(entry.status, "held");
        assert_eq!(entry.calls[0].1["create"]["new"]["title"], "Changed");
        assert_eq!(entry.base_state.as_deref(), Some("state1"));
        drop(store);
        let store = Store::open(&path).unwrap();
        let mut objects = BTreeMap::new();
        overlay(&store, "a", "calendar", &mut objects).unwrap();
        assert_eq!(objects[&id]["title"], "Changed");
        assert!(calendar_scope_is_empty(&store, "b"));
    }
    fn calendar_scope_is_empty(store: &Store, scope: &str) -> bool {
        let mut objects = BTreeMap::new();
        overlay(store, scope, "calendar", &mut objects).unwrap();
        objects.is_empty()
    }
    #[test]
    fn folded_calendar_reply_keeps_scheduling_request_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rsvp.redb");
        for create in [false, true] {
            let scope = if create { "create" } else { "update" };
            let store = Store::open(&path).unwrap();
            let objects = std::collections::BTreeMap::from([(
                "e".into(),
                json!({"id":"e","title":"Meeting","participants":{"me":{"participationStatus":"needs-action"}}}),
            )]);
            let first = if create {
                json!({"accountId":"a","sendSchedulingMessages":false,"create":{"new":{"title":"Meeting","participants":{"me":{"participationStatus":"needs-action"}}}}})
            } else {
                json!({"accountId":"a","sendSchedulingMessages":false,"update":{"e":{"title":"Moved meeting"}}})
            };
            let response = queue_object(
                &store,
                scope,
                "calendar",
                "CalendarEvent/set",
                first,
                &objects,
                Some("s1".into()),
                true,
            )
            .unwrap();
            let id = if create {
                response["created"]["new"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            } else {
                "e".into()
            };
            let mut view = objects.clone();
            overlay(&store, scope, "calendar", &mut view).unwrap();
            queue_object(&store,scope,"calendar","CalendarEvent/set",json!({"accountId":"a","sendSchedulingMessages":true,"update":{id.clone():{"participants/me/participationStatus":"accepted"}}}),&view,Some("s1".into()),true).unwrap();
            overlay(&store, scope, "calendar", &mut view).unwrap();
            queue_object(&store,scope,"calendar","CalendarEvent/set",json!({"accountId":"a","sendSchedulingMessages":false,"update":{id.clone():{"title":"Quiet follow-up"}}}),&view,Some("s1".into()),true).unwrap();
            drop(store);
            let store = Store::open(&path).unwrap();
            let entry = entries(&store, scope).unwrap().remove(0);
            assert_eq!(entries(&store, scope).unwrap().len(), 1);
            assert_eq!(entry.calls[0].1["sendSchedulingMessages"], true);
            assert_eq!(entry.status, "held");
            assert_eq!(
                entry.local.unwrap()["participants"]["me"]["participationStatus"],
                "accepted"
            );
        }
    }

    #[test]
    fn calendar_overlay_never_changes_remote_snapshot_and_file_dependencies_preserve_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("isolation.redb")).unwrap();
        let snapshot = json!({"calendars":[{"id":"cal"}],"events":{"event":{"id":"event","title":"Server"}},"identities":[],"eventState":"s1","calendarState":"c1"});
        store.put("calendar:a", &snapshot.to_string()).unwrap();
        let events = crate::calendar::read_remote(&store, "a").unwrap().events;
        queue_object(
            &store,
            "a",
            "calendar",
            "CalendarEvent/set",
            json!({"accountId":"a","update":{"event":{"title":"Offline"}}}),
            &events,
            Some("s1".into()),
            true,
        )
        .unwrap();
        assert_eq!(
            crate::calendar::read(&store, "a").unwrap().events["event"]["title"],
            "Offline"
        );
        assert_eq!(
            crate::calendar::read_remote(&store, "a").unwrap().events["event"]["title"],
            "Server"
        );
        let calls = vec![(
            "FileNode/set".into(),
            json!({"accountId":"a","create":{"child":{"parentId":"local-change-parent","name":"local-change-title"}}}),
            "create".into(),
        )];
        assert!(resolve_calls(&store, "a", &calls).is_err());
        remember_created(&store, "a", "local-change-parent", "server-parent").unwrap();
        let resolved = resolve_calls(&store, "a", &calls).unwrap();
        assert_eq!(
            resolved[0].1["create"]["child"]["parentId"],
            "server-parent"
        );
        assert_eq!(
            resolved[0].1["create"]["child"]["name"],
            "local-change-title"
        );
        assert!(resolve_calls(&store, "b", &calls).is_err());
    }
    #[test]
    fn delete_supersedes_unsent_edits_and_never_adds_calendar_arguments_to_file_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("delete.redb")).unwrap();
        let objects = BTreeMap::from([("file".into(), json!({"id":"file","name":"Before"}))]);
        queue_object(
            &store,
            "a",
            "file",
            "FileNode/set",
            json!({"accountId":"a","update":{"file":{"name":"After"}}}),
            &objects,
            Some("s1".into()),
            true,
        )
        .unwrap();
        queue_object(
            &store,
            "a",
            "file",
            "FileNode/set",
            json!({"accountId":"a","destroy":["file"]}),
            &objects,
            Some("s1".into()),
            true,
        )
        .unwrap();
        let entries = entries(&store, "a").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].calls[0].1["destroy"], json!(["file"]));
        assert!(entries[0].calls[0]
            .1
            .get("sendSchedulingMessages")
            .is_none());
    }
    #[test]
    fn invalid_mixed_patch_rolls_back_and_own_server_states_advance_without_replaying() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("objects.redb")).unwrap();
        let objects = BTreeMap::from([("event".into(), json!({"id":"event","title":"Before"}))]);
        assert!(queue_object(&store,"a","calendar","CalendarEvent/set",json!({"accountId":"a","create":{"new":{"title":"Good"}},"update":{"event":{"missing/nested":"Bad"}}}),&objects,Some("s1".into()),true).is_err());
        assert!(entries(&store, "a").unwrap().is_empty());
        for id in ["a", "b"] {
            queue_object(
                &store,
                "a",
                "calendar",
                "CalendarEvent/set",
                json!({"accountId":"a","create":{id:{"title":id}}}),
                &objects,
                Some("s1".into()),
                true,
            )
            .unwrap();
        }
        let first = entries(&store, "a").unwrap()[0].id;
        approve(&store, "a", &[first]).unwrap();
        assert!(claim(&store, "a", first).unwrap());
        applied(&store, "a", first, Some("s2")).unwrap();
        assert!(!claim(&store, "a", first).unwrap());
        assert_eq!(
            entries(&store, "a").unwrap()[1].base_state.as_deref(),
            Some("s2")
        );
        clear_applied(&store, "a", "calendar").unwrap();
        assert_eq!(entries(&store, "a").unwrap().len(), 1);
    }
}
