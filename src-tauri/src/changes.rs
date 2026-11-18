//! Local-only resolution of exact queued intentions. No server transport here.
use crate::{
    calendar, config::TestConfig, desktop, journal, storage::Store, sync, writes, SyncGate,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tauri::State;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    account_id: String,
    sequence: Option<u64>,
    journal_id: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    title: String,
    revision: String,
    fields: Vec<Field>,
    warning: String,
    can_retry: bool,
    can_discard: bool,
    uncertain: bool,
}
#[derive(Serialize)]
struct Field {
    label: String,
    current: Value,
    desired: Value,
}
fn capability(kind: &str) -> Result<&'static str, String> {
    match kind {
        "calendar" => Ok(sync::CALENDARS),
        "contact" => Ok(sync::CONTACTS),
        "file" => Ok(sync::FILES),
        "send" | "mail" => Ok("urn:ietf:params:jmap:mail"),
        _ => Err("Unbekannte Änderungsart.".into()),
    }
}
fn validate(selection: &Selection) -> Result<(), String> {
    if selection.sequence.is_some() == selection.journal_id.is_some() {
        return Err("Genau eine Aktion auswählen.".into());
    }
    Ok(())
}
fn label(field: &str) -> String {
    match field.split('/').next().unwrap_or(field) {
        "title" | "subject" => "Betreff / Titel",
        "name" => "Name",
        "start" => "Beginn",
        "duration" => "Dauer",
        "timeZone" => "Zeitzone",
        "calendarIds" => "Kalender",
        "parentId" => "Übergeordneter Ordner",
        "description" => "Beschreibung",
        "participants" => "Teilnehmer",
        "keywords" => "Markierungen",
        "mailboxIds" => "Ordner",
        "from" => "Absender",
        "to" => "Empfänger",
        "attachments" => "Anhänge",
        "recurrenceRule" | "recurrenceOverrides" => "Wiederholungen",
        "status" => "Terminstatus",
        "blobId" => "Dateiinhalt",
        "type" => "Dateityp",
        "size" => "Dateigröße",
        _ => field,
    }
    .into()
}
fn baseline(
    store: &Store,
    scope: &str,
    kind: &str,
) -> Result<(Option<String>, BTreeMap<String, Value>), String> {
    if kind == "contact" {
        let snapshot = crate::contacts::read(store, scope)?;
        if snapshot.access_denied {
            return Ok((None, BTreeMap::new()));
        }
        Ok((snapshot.contact_state, snapshot.contacts))
    } else if kind == "calendar" {
        let snapshot = calendar::read_remote(store, scope)?;
        if snapshot.access_denied {
            return Ok((None, BTreeMap::new()));
        }
        Ok((snapshot.event_state, snapshot.events))
    } else {
        let snapshot: Value = store
            .get(&format!("files:{scope}"))?
            .map(|s| serde_json::from_str(&s).map_err(|_| "Dateistand ungültig.".to_owned()))
            .transpose()?
            .unwrap_or(json!({}));
        let state = snapshot["state"]
            .as_str()
            .filter(|s| *s != "denied")
            .map(str::to_owned);
        let objects = snapshot["list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["id"].as_str()?.into(), v.clone())))
            .collect();
        Ok((state, objects))
    }
}
fn projected(
    entry: &journal::Entry,
    objects: &BTreeMap<String, Value>,
) -> Result<Option<Value>, String> {
    let args = &entry.calls.first().ok_or("Auftrag ist leer.")?.1;
    if args["create"].is_object() {
        return Ok(entry.local.clone());
    }
    let object = objects
        .get(&entry.object_id)
        .ok_or("Das Objekt ist im gespeicherten Serverstand nicht mehr vorhanden.")?;
    if let Some(patch) = args["update"].get(&entry.object_id) {
        let mut desired = object.clone();
        journal::patch(&mut desired, patch)?;
        Ok(Some(desired))
    } else if args["destroy"].is_array() {
        Ok(None)
    } else {
        Err("Auftrag kann nicht erneut vorgemerkt werden.".into())
    }
}
fn entry_details(
    store: &Store,
    config: &TestConfig,
    selection: &Selection,
    entry: &journal::Entry,
) -> Result<Details, String> {
    desktop::require_capability(
        store,
        config,
        &selection.account_id,
        capability(&entry.kind)?,
    )?;
    let scope = sync::scope(config, &selection.account_id);
    let (state, objects) = if entry.kind == "send" {
        (None, BTreeMap::new())
    } else {
        baseline(store, &scope, &entry.kind)?
    };
    let current = objects
        .get(&entry.object_id)
        .cloned()
        .unwrap_or(Value::Null);
    let desired = if entry.kind == "send" {
        entry.local.clone()
    } else {
        projected(entry, &objects).unwrap_or_else(|_| entry.local.clone())
    };
    let desired_value = desired.unwrap_or(Value::Null);
    let fields = if entry.kind == "send" {
        vec![
            "subject".into(),
            "from".into(),
            "to".into(),
            "attachments".into(),
        ]
    } else if let Some(patch) = entry.calls[0].1["update"]
        .get(&entry.object_id)
        .and_then(Value::as_object)
    {
        patch.keys().cloned().collect::<Vec<String>>()
    } else {
        current
            .as_object()
            .into_iter()
            .chain(desired_value.as_object())
            .flat_map(|m| m.keys().cloned())
            .filter(|k| {
                !["id", "myRights", "@type", "uid"].contains(&k.as_str())
                    && !k.starts_with("desktop")
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let fields = fields
        .into_iter()
        .map(|field| Field {
            label: label(&field),
            current: if entry.kind == "send" {
                json!("Versandstatus nicht automatisch geprüft")
            } else if entry.status == "uncertain" && current.is_null() {
                json!("Keine bestätigte Zuordnung")
            } else {
                current
                    .pointer(&format!("/{field}"))
                    .cloned()
                    .unwrap_or(Value::Null)
            },
            desired: desired_value
                .pointer(&format!("/{field}"))
                .cloned()
                .unwrap_or(Value::Null),
        })
        .collect();
    let uncertain = entry.status == "uncertain";
    let can_retry = entry.status == "blocked"
        && entry.kind != "send"
        && state.is_some()
        && projected(entry, &objects).is_ok()
        && writes::writable(
            store,
            config,
            &selection.account_id,
            capability(&entry.kind)?,
        )
        .is_ok();
    Ok(Details {
        title: entry.title.clone(), revision: json!([scope,entry.id,entry.status,state]).to_string(), fields,
        warning: if uncertain { "Der Server kann diese Aktion bereits ausgeführt haben. Prüfe den Serverstand und bei Mails auch Gesendet und die Zustellung. Lokales Abschließen nimmt keine Serveränderung zurück und sendet nichts erneut." }
            else if entry.kind == "send" { "Abgelehnte Mails zum Korrigieren als Entwurf öffnen. Es gibt hier keinen erneuten Versand." }
            else { "Verglichen wird der zuletzt erfolgreich gespeicherte Serverstand. Erneutes Vormerken verwendet die lokale Änderung auf dieser Basis und wartet auf eine neue Freigabe. Verwerfen nimmt keine Serveränderung zurück." }.into(),
        can_retry, can_discard: ["held","ready","blocked","uncertain"].contains(&entry.status.as_str()), uncertain,
    })
}
fn details(store: &Store, config: &TestConfig, selection: &Selection) -> Result<Details, String> {
    validate(selection)?;
    let scope = sync::scope(config, &selection.account_id);
    if let Some(id) = selection.journal_id {
        let entry = journal::entries(store, &scope)?
            .into_iter()
            .find(|e| e.id == id)
            .ok_or("Auftrag wurde inzwischen geändert oder abgeschlossen.")?;
        return entry_details(store, config, selection, &entry);
    }
    desktop::require_capability(store, config, &selection.account_id, capability("mail")?)?;
    let sequence = selection.sequence.ok_or("Aktion fehlt.")?;
    let mail = store.local_mail(&scope)?;
    let (id, desired, field, blocked, deleting) =
        if let Some(op) = mail.pending.iter().find(|op| op.sequence == sequence) {
            (
                op.email_id.as_str(),
                op.mailbox_ids.clone().unwrap_or(Value::Bool(op.enabled)),
                if op.mailbox_ids.is_some() {
                    "mailboxIds"
                } else {
                    &op.keyword
                },
                op.blocked.is_some(),
                false,
            )
        } else if let Some(op) = mail
            .pending_deletes
            .iter()
            .find(|op| op.sequence == sequence)
        {
            (
                op.email_id.as_str(),
                json!("Löschen"),
                "Nachricht",
                op.blocked.is_some(),
                true,
            )
        } else {
            return Err("Änderung wurde inzwischen geändert oder abgeschlossen.".into());
        };
    let raw = store
        .server_emails(&scope)?
        .into_iter()
        .find(|e| e["id"] == id);
    let current = raw
        .as_ref()
        .map(|e| {
            if deleting {
                e["subject"].clone()
            } else if field == "mailboxIds" {
                e[field].clone()
            } else {
                json!(e["keywords"][field] == true)
            }
        })
        .unwrap_or(Value::Null);
    let newer = mail
        .pending
        .iter()
        .any(|e| e.email_id == id && e.sequence > sequence)
        || mail
            .pending_deletes
            .iter()
            .any(|e| e.email_id == id && e.sequence > sequence);
    Ok(Details { title: raw.as_ref().and_then(|e| e["subject"].as_str()).unwrap_or("Nachricht").into(), revision: json!([scope,store.pending_revision(&scope)?]).to_string(), fields: vec![Field { label: match field { "$seen"=>"Gelesen".into(), "$flagged"=>"Stern".into(), _=>label(field) }, current, desired }], warning: if newer { "Eine neuere lokale Änderung liegt vor. Die ältere Aktion kann verworfen werden." } else { "Die lokale Absicht wird mit dem zuletzt gespeicherten Serverstand verglichen. Erneutes Vormerken wartet auf eine neue Freigabe. Verwerfen entfernt nur diese lokale Absicht." }.into(), can_retry: blocked && !newer && raw.is_some() && (!deleting || raw.as_ref().is_some_and(|e| e["keywords"]["$draft"] == true)) && writes::writable(store, config, &selection.account_id, capability("mail")?).is_ok(), can_discard: true, uncertain: false })
}
#[tauri::command]
pub async fn change_details(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    selection: Selection,
) -> Result<Details, String> {
    let _guard = gate.0.lock().await;
    details(&store, &TestConfig::load()?, &selection)
}
fn resolve(
    store: &Store,
    config: &TestConfig,
    selection: &Selection,
    action: &str,
    expected_revision: &str,
    acknowledge_uncertain: bool,
) -> Result<(), String> {
    let preview = details(store, config, selection)?;
    if preview.revision != expected_revision {
        return Err("Stand wurde inzwischen geändert. Bitte erneut prüfen.".into());
    }
    let scope = sync::scope(config, &selection.account_id);
    match action {
        "discard" if preview.can_discard => {
            if preview.uncertain && !acknowledge_uncertain {
                return Err("Serverstatus zuerst ausdrücklich prüfen.".into());
            }
            if let Some(id) = selection.journal_id {
                journal::discard(store, &scope, id, acknowledge_uncertain)
            } else {
                store.resolve_pending(
                    &scope,
                    selection.sequence.ok_or("Aktion fehlt.")?,
                    false,
                    "",
                )
            }
        }
        "retry" if preview.can_retry => {
            if let Some(id) = selection.journal_id {
                journal::retry(store, &scope, id, |entry| {
                    writes::writable(
                        store,
                        config,
                        &selection.account_id,
                        capability(&entry.kind)?,
                    )?;
                    let latest = entry_details(store, config, selection, entry)?;
                    if latest.revision != expected_revision || !latest.can_retry {
                        return Err("Stand wurde inzwischen geändert. Bitte erneut prüfen.".into());
                    }
                    let (state, objects) = baseline(store, &scope, &entry.kind)?;
                    Ok((state, projected(entry, &objects)?))
                })?;
                Ok(())
            } else {
                writes::writable(store, config, &selection.account_id, capability("mail")?)?;
                let expected: Value =
                    serde_json::from_str(expected_revision).map_err(|_| "Ungültiger Prüfstand.")?;
                store.resolve_pending(
                    &scope,
                    selection.sequence.ok_or("Aktion fehlt.")?,
                    true,
                    expected[1].as_str().ok_or("Ungültiger Prüfstand.")?,
                )
            }
        }
        _ => Err("Diese Auflösung ist für den Auftrag nicht verfügbar.".into()),
    }
}
#[tauri::command]
pub async fn resolve_change(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    selection: Selection,
    action: String,
    expected_revision: String,
    acknowledge_uncertain: bool,
) -> Result<(), String> {
    let _guard = gate.0.lock().await;
    resolve(
        &store,
        &TestConfig::load()?,
        &selection,
        &action,
        &expected_revision,
        acknowledge_uncertain,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replica::Collection;
    fn config(store: &Store) -> TestConfig {
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "synthetic".into(),
            password: String::new(),
            read_only: false,
        };
        let caps = json!({"urn:ietf:params:jmap:mail":{},sync::FILES:{},sync::CALENDARS:{}});
        store.put(&format!("{}:desktop-session",config.cache_key()), &json!({"accounts":{"a":{"isReadOnly":false,"accountCapabilities":caps},"b":{"isReadOnly":false,"accountCapabilities":caps},"locked":{"isReadOnly":true,"accountCapabilities":caps}}}).to_string()).unwrap();
        config
    }
    fn select(id: u64) -> Selection {
        Selection {
            account_id: "a".into(),
            sequence: None,
            journal_id: Some(id),
        }
    }
    #[test]
    fn conflict_retry_rebases_only_the_patch_requires_fresh_review_and_rejects_stale_previews() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conflicts.redb");
        let store = Store::open(&path).unwrap();
        let config = config(&store);
        let scope = sync::scope(&config, "a");
        let original = json!({"id":"file","name":"Before","description":"Old"});
        let baseline_key = format!("files:{scope}");
        store
            .put(
                &baseline_key,
                &json!({"state":"s1","list":[original]}).to_string(),
            )
            .unwrap();
        journal::queue_object(
            &store,
            &scope,
            "file",
            "FileNode/set",
            json!({"accountId":"a","update":{"file":{"name":"Local"}}}),
            &BTreeMap::from([("file".into(), original)]),
            Some("s1".into()),
            true,
        )
        .unwrap();
        let old = journal::entries(&store, &scope).unwrap()[0].id;
        journal::finish(&store, &scope, old, Some("blocked"), Some("stateMismatch")).unwrap();
        let selection = select(old);
        let stale = details(&store, &config, &selection).unwrap();
        store.put(&baseline_key,&json!({"state":"s2","list":[{"id":"file","name":"External","description":"Keep external"}]}).to_string()).unwrap();
        assert!(resolve(&store, &config, &selection, "retry", &stale.revision, false).is_err());
        let preview = details(&store, &config, &selection).unwrap();
        assert_eq!(preview.fields[0].current, "External");
        assert_eq!(preview.fields[0].desired, "Local");
        assert!(preview.can_retry);
        let read_only = TestConfig {
            read_only: true,
            url: config.url.clone(),
            username: config.username.clone(),
            password: String::new(),
        };
        assert!(resolve(
            &store,
            &read_only,
            &selection,
            "retry",
            &preview.revision,
            false
        )
        .is_err());
        let revision_before = journal::revision(&store, &scope).unwrap();
        resolve(
            &store,
            &config,
            &selection,
            "retry",
            &preview.revision,
            false,
        )
        .unwrap();
        assert!(journal::revision(&store, &scope).unwrap() > revision_before);
        let replacement = journal::entries(&store, &scope).unwrap().remove(0);
        assert_ne!(replacement.id, old);
        assert_eq!(replacement.status, "held");
        assert_eq!(replacement.base_state.as_deref(), Some("s2"));
        assert_eq!(replacement.local.unwrap()["description"], "Keep external");
        journal::approve(&store, &scope, &[old]).unwrap();
        assert_eq!(journal::entries(&store, &scope).unwrap()[0].status, "held");
        assert!(resolve(
            &store,
            &config,
            &selection,
            "discard",
            &preview.revision,
            false
        )
        .is_err());
        let selection = select(replacement.id);
        let preview = details(&store, &read_only, &selection).unwrap();
        resolve(
            &store,
            &read_only,
            &selection,
            "discard",
            &preview.revision,
            false,
        )
        .unwrap();
        assert!(journal::entries(&store, &scope).unwrap().is_empty());
        assert_eq!(
            writes::files(&store, &scope).unwrap()["list"][0]["name"],
            "External"
        );
        drop(store);
        assert!(journal::entries(&Store::open(&path).unwrap(), &scope)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn mail_retry_is_exact_held_and_cannot_overwrite_newer_intentions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("mail.redb")).unwrap();
        let config = config(&store);
        let scope = sync::scope(&config, "a");
        store
            .apply_remote(
                &scope,
                Collection::Email,
                None,
                "e1",
                &[json!({"id":"e","subject":"Test","keywords":{"$draft":true}})],
                &[],
                true,
            )
            .unwrap();
        store.queue_keyword(&scope, "e", "$flagged", true).unwrap();
        let old = store.local_mail(&scope).unwrap().pending.remove(0);
        store.block_keyword(&scope, &old, "forbidden").unwrap();
        let selection = Selection {
            account_id: "a".into(),
            sequence: Some(old.sequence),
            journal_id: None,
        };
        let stale = details(&store, &config, &selection).unwrap();
        store
            .apply_remote(&scope, Collection::Email, Some("e1"), "e2", &[], &[], false)
            .unwrap();
        assert!(resolve(&store, &config, &selection, "retry", &stale.revision, false).is_err());
        let preview = details(&store, &config, &selection).unwrap();
        resolve(
            &store,
            &config,
            &selection,
            "retry",
            &preview.revision,
            false,
        )
        .unwrap();
        let retry = store.local_mail(&scope).unwrap().pending.remove(0);
        assert_ne!(retry.sequence, old.sequence);
        assert!(retry.requires_review);
        assert!(retry.blocked.is_none());
        store.approve_changes(&scope, &[old.sequence]).unwrap();
        assert!(store.local_mail(&scope).unwrap().pending[0].requires_review);
        store.block_keyword(&scope, &retry, "forbidden").unwrap();
        let selection = Selection {
            sequence: Some(retry.sequence),
            ..selection
        };
        store.queue_keyword(&scope, "e", "$flagged", false).unwrap();
        let preview = details(&store, &config, &selection).unwrap();
        assert!(!preview.can_retry);
        assert!(resolve(
            &store,
            &config,
            &selection,
            "retry",
            &preview.revision,
            false
        )
        .is_err());
        resolve(
            &store,
            &config,
            &selection,
            "discard",
            &preview.revision,
            false,
        )
        .unwrap();
        let mail = store.local_mail(&scope).unwrap();
        assert_eq!(mail.pending.len(), 1);
        assert_ne!(mail.emails[0]["keywords"]["$flagged"], true);
        let deletion = store.queue_draft_delete(&scope, "e").unwrap();
        let revision_before = store.pending_revision(&scope).unwrap();
        assert!(store.local_mail(&scope).unwrap().emails.is_empty());
        store
            .resolve_pending(&scope, deletion.sequence, false, "")
            .unwrap();
        assert_ne!(store.pending_revision(&scope).unwrap(), revision_before);
        assert_eq!(store.local_mail(&scope).unwrap().emails.len(), 1);
    }
    #[test]
    fn uncertain_sends_are_never_retried_and_require_acknowledgement_to_close_locally() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sends.redb");
        let store = Store::open(&path).unwrap();
        let config = config(&store);
        let scope = sync::scope(&config, "a");
        let calls = vec![(
            "Email/set".into(),
            json!({"accountId":"a","create":{"e":{"subject":"Uncertain","messageId":["stable-message-id"]}}}),
            "create".into(),
        )];
        let id = journal::enqueue_send(&store, &scope, calls, true, None, None).unwrap();
        journal::approve(&store, &scope, &[id]).unwrap();
        journal::claim(&store, &scope, id).unwrap();
        let selection = select(id);
        let sending = details(&store, &config, &selection).unwrap();
        assert!(!sending.can_discard);
        assert!(resolve(
            &store,
            &config,
            &selection,
            "discard",
            &sending.revision,
            true
        )
        .is_err());
        journal::finish(&store, &scope, id, Some("uncertain"), Some("network")).unwrap();
        let preview = details(&store, &config, &selection).unwrap();
        assert!(!preview.can_retry);
        assert!(resolve(
            &store,
            &config,
            &selection,
            "retry",
            &preview.revision,
            true
        )
        .is_err());
        assert!(resolve(
            &store,
            &config,
            &selection,
            "discard",
            &preview.revision,
            false
        )
        .is_err());
        let foreign = Selection {
            account_id: "b".into(),
            ..selection.clone()
        };
        assert!(details(&store, &config, &foreign).is_err());
        assert!(details(
            &store,
            &config,
            &Selection {
                sequence: Some(1),
                ..selection.clone()
            }
        )
        .is_err());
        resolve(
            &store,
            &config,
            &selection,
            "discard",
            &preview.revision,
            true,
        )
        .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        journal::approve(&store, &scope, &[id]).unwrap();
        assert!(!journal::claim(&store, &scope, id).unwrap());
        assert!(journal::entries(&store, &scope).unwrap().is_empty());
    }
}
