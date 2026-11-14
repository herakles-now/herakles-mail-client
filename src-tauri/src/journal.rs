//! Durable, account-scoped write intentions. In-flight sends are never replayed.
use crate::storage::Store;
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub type Calls = Vec<(String, Value, String)>;
const TABLE: TableDefinition<&str, &str> = TableDefinition::new("desktop_write_journal_v1");

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
