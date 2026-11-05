//! Local-first mail replica. UI writes only durable intents, never server state.
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::storage::Store;

const REPLICAS: TableDefinition<&str, &str> = TableDefinition::new("replica_metadata_v1");
const OBJECTS: TableDefinition<&str, &str> = TableDefinition::new("replica_objects_v1");
const OUTBOX: TableDefinition<&str, &str> = TableDefinition::new("replica_outbox_v1");
const DRAFTS: TableDefinition<&str, &str> = TableDefinition::new("replica_local_drafts_v1");
const DELETE_OUTBOX: TableDefinition<&str, &str> = TableDefinition::new("replica_delete_outbox_v1");

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Collection {
    Email,
    Mailbox,
}

impl Collection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Email => "Email",
            Self::Mailbox => "Mailbox",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub email_state: Option<String>,
    pub mailbox_state: Option<String>,
    pub next_operation: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingKeyword {
    pub sequence: u64,
    pub email_id: String,
    pub keyword: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<Value>,
    pub enabled: bool,
    #[serde(default)]
    pub blocked: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDelete {
    pub sequence: u64,
    pub email_id: String,
    #[serde(default)]
    pub blocked: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMail {
    pub emails: Vec<Value>,
    pub mailboxes: Vec<Value>,
    pub pending: Vec<PendingKeyword>,
    pub pending_deletes: Vec<PendingDelete>,
    pub initialized: bool,
}

fn encode<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "Lokale Daten konnten nicht codiert werden.".into())
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| "Lokale Daten sind beschädigt.".into())
}
fn prefix(scope: &str, collection: Collection) -> String {
    format!("{}\0{}\0", scope, collection.name())
}
fn object_key(scope: &str, collection: Collection, id: &str) -> String {
    format!("{}{id}", prefix(scope, collection))
}
fn outbox_prefix(scope: &str) -> String {
    format!("{scope}\0")
}
fn operation_key(scope: &str, sequence: u64) -> String {
    format!("{}{sequence:020}", outbox_prefix(scope))
}
fn database_error(_: impl std::fmt::Display) -> String {
    "Lokale Datenbankoperation fehlgeschlagen.".into()
}

impl Store {
    pub fn initialize_replica(&self) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        tx.open_table(REPLICAS).map_err(database_error)?;
        tx.open_table(OBJECTS).map_err(database_error)?;
        tx.open_table(OUTBOX).map_err(database_error)?;
        tx.open_table(DRAFTS).map_err(database_error)?;
        tx.open_table(DELETE_OUTBOX).map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn metadata(&self, scope: &str) -> Result<Metadata, String> {
        let tx = self.0.begin_read().map_err(database_error)?;
        let table = tx.open_table(REPLICAS).map_err(database_error)?;
        let row = table.get(scope).map_err(database_error)?;
        row.map(|v| decode(v.value()))
            .transpose()
            .map(|v| v.unwrap_or_default())
    }

    /// Commit objects and the corresponding server state in the same transaction.
    /// Full replacement never removes pending local edits; those live in OUTBOX.
    pub fn apply_remote(
        &self,
        scope: &str,
        collection: Collection,
        expected: Option<&str>,
        state: &str,
        objects: &[Value],
        destroyed: &[String],
        replace: bool,
    ) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let mut metadata = tx.open_table(REPLICAS).map_err(database_error)?;
            let mut meta: Metadata = metadata
                .get(scope)
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .unwrap_or_default();
            let current = match collection {
                Collection::Email => &mut meta.email_state,
                Collection::Mailbox => &mut meta.mailbox_state,
            };
            if current.as_deref() != expected {
                return Err("Synchronisierungszustand hat sich geändert. Erneut versuchen.".into());
            }
            let mut table = tx.open_table(OBJECTS).map_err(database_error)?;
            if replace {
                let start = prefix(scope, collection);
                let keys: Vec<String> = table
                    .range(start.as_str()..)
                    .map_err(database_error)?
                    .map(|row| {
                        row.map(|(k, _)| k.value().to_owned())
                            .map_err(database_error)
                    })
                    .take_while(|row| row.as_ref().map(|k| k.starts_with(&start)).unwrap_or(true))
                    .collect::<Result<_, _>>()?;
                for key in keys {
                    table.remove(key.as_str()).map_err(database_error)?;
                }
            }
            for id in destroyed {
                table
                    .remove(object_key(scope, collection, id).as_str())
                    .map_err(database_error)?;
            }
            for object in objects {
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Serverobjekt ohne ID.")?;
                table
                    .insert(
                        object_key(scope, collection, id).as_str(),
                        encode(object)?.as_str(),
                    )
                    .map_err(database_error)?;
            }
            if matches!(collection, Collection::Email) {
                let destroyed_ids: std::collections::BTreeSet<&str> =
                    destroyed.iter().map(String::as_str).collect();
                let server_ids: std::collections::BTreeSet<&str> = objects
                    .iter()
                    .filter_map(|object| object["id"].as_str())
                    .collect();
                let queue_prefix = outbox_prefix(scope);
                let mut deletes = tx.open_table(DELETE_OUTBOX).map_err(database_error)?;
                let obsolete: Vec<String> = deletes
                    .range(queue_prefix.as_str()..)
                    .map_err(database_error)?
                    .filter_map(|row| {
                        let (key, value) = row.ok()?;
                        if !key.value().starts_with(&queue_prefix) {
                            return None;
                        }
                        let operation: PendingDelete = decode(value.value()).ok()?;
                        let confirmed = destroyed_ids.contains(operation.email_id.as_str())
                            || (replace && !server_ids.contains(operation.email_id.as_str()));
                        confirmed.then(|| key.value().to_owned())
                    })
                    .collect();
                let reconciled = obsolete.len();
                for key in obsolete {
                    deletes.remove(key.as_str()).map_err(database_error)?;
                }
                if reconciled > 0 {
                    crate::logging::event(
                        "info",
                        "sync",
                        "draft_delete_reconciled",
                        serde_json::json!({"count":reconciled}),
                    );
                }
            }
            *current = Some(state.to_owned());
            metadata
                .insert(scope, encode(&meta)?.as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    pub fn local_mail(&self, scope: &str) -> Result<LocalMail, String> {
        let tx = self.0.begin_read().map_err(database_error)?;
        let table = tx.open_table(OBJECTS).map_err(database_error)?;
        let read = |collection| -> Result<Vec<Value>, String> {
            let start = prefix(scope, collection);
            let mut objects = Vec::new();
            for row in table.range(start.as_str()..).map_err(database_error)? {
                let (key, value) = row.map_err(database_error)?;
                if !key.value().starts_with(&start) {
                    break;
                }
                objects.push(decode(value.value())?);
            }
            Ok(objects)
        };
        let mut emails = read(Collection::Email)?;
        let mut mailboxes = read(Collection::Mailbox)?;
        {
            let drafts = tx.open_table(DRAFTS).map_err(database_error)?;
            let start = format!("{scope}\0");
            for row in drafts.range(start.as_str()..).map_err(database_error)? {
                let (key, value) = row.map_err(database_error)?;
                if !key.value().starts_with(&start) {
                    break;
                }
                emails.push(decode(value.value())?);
            }
        }
        let queue = tx.open_table(OUTBOX).map_err(database_error)?;
        let start = outbox_prefix(scope);
        let mut pending: Vec<PendingKeyword> = Vec::new();
        for row in queue.range(start.as_str()..).map_err(database_error)? {
            let (key, value) = row.map_err(database_error)?;
            if !key.value().starts_with(&start) {
                break;
            }
            pending.push(decode(value.value())?);
        }
        let delete_queue = tx.open_table(DELETE_OUTBOX).map_err(database_error)?;
        let mut pending_deletes: Vec<PendingDelete> = Vec::new();
        for row in delete_queue
            .range(start.as_str()..)
            .map_err(database_error)?
        {
            let (key, value) = row.map_err(database_error)?;
            if !key.value().starts_with(&start) {
                break;
            }
            pending_deletes.push(decode(value.value())?);
        }
        // UI sees local intent overlaid on the latest server baseline.
        for operation in &pending {
            if let Some(email) = emails
                .iter_mut()
                .find(|email| email["id"].as_str() == Some(&operation.email_id))
            {
                apply_keyword(email, operation)?;
            }
        }
        let deleted_ids: std::collections::BTreeSet<_> = pending_deletes
            .iter()
            .map(|item| item.email_id.as_str())
            .collect();
        emails.retain(|email| {
            !email["id"]
                .as_str()
                .is_some_and(|id| deleted_ids.contains(id))
        });
        emails.sort_by(|a, b| b["receivedAt"].as_str().cmp(&a["receivedAt"].as_str()));
        let metadata = tx.open_table(REPLICAS).map_err(database_error)?;
        let meta: Metadata = metadata
            .get(scope)
            .map_err(database_error)?
            .map(|v| decode(v.value()))
            .transpose()?
            .unwrap_or_default();
        if meta.email_state.is_some() {
            for mailbox in &mut mailboxes {
                let Some(id) = mailbox["id"].as_str().map(str::to_owned) else {
                    continue;
                };
                let in_folder: Vec<_> = emails
                    .iter()
                    .filter(|email| email["mailboxIds"][&id] == true)
                    .collect();
                let unread: Vec<_> = in_folder
                    .iter()
                    .filter(|email| email["keywords"]["$seen"] != true)
                    .collect();
                let threads: std::collections::BTreeSet<_> = in_folder
                    .iter()
                    .filter_map(|email| email["threadId"].as_str().or_else(|| email["id"].as_str()))
                    .collect();
                let unread_threads: std::collections::BTreeSet<_> = unread
                    .iter()
                    .filter_map(|email| email["threadId"].as_str().or_else(|| email["id"].as_str()))
                    .collect();
                mailbox["totalEmails"] = serde_json::json!(in_folder.len());
                mailbox["unreadEmails"] = serde_json::json!(unread.len());
                mailbox["totalThreads"] = serde_json::json!(threads.len());
                mailbox["unreadThreads"] = serde_json::json!(unread_threads.len());
            }
        }
        Ok(LocalMail {
            emails,
            mailboxes,
            pending,
            pending_deletes,
            initialized: meta.email_state.is_some(),
        })
    }

    pub fn queue_draft_delete(&self, scope: &str, email_id: &str) -> Result<PendingDelete, String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        let mut operation;
        {
            let objects = tx.open_table(OBJECTS).map_err(database_error)?;
            let object: Value = objects
                .get(object_key(scope, Collection::Email, email_id).as_str())
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .ok_or("Entwurf ist lokal nicht vorhanden.")?;
            if object["keywords"]["$draft"] != true {
                return Err("Nur Entwürfe können hier gelöscht werden.".into());
            }
            let mut metadata = tx.open_table(REPLICAS).map_err(database_error)?;
            let mut meta: Metadata = metadata
                .get(scope)
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .unwrap_or_default();
            meta.next_operation = meta
                .next_operation
                .checked_add(1)
                .ok_or("Lokale Warteschlange ist voll.")?;
            operation = PendingDelete {
                sequence: meta.next_operation,
                email_id: email_id.into(),
                blocked: None,
            };
            tx.open_table(DELETE_OUTBOX)
                .map_err(database_error)?
                .insert(
                    operation_key(scope, operation.sequence).as_str(),
                    encode(&operation)?.as_str(),
                )
                .map_err(database_error)?;
            metadata
                .insert(scope, encode(&meta)?.as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        operation.blocked = None;
        Ok(operation)
    }

    pub fn acknowledge_delete(&self, scope: &str, operation: &PendingDelete) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            tx.open_table(OBJECTS)
                .map_err(database_error)?
                .remove(object_key(scope, Collection::Email, &operation.email_id).as_str())
                .map_err(database_error)?;
            tx.open_table(DELETE_OUTBOX)
                .map_err(database_error)?
                .remove(operation_key(scope, operation.sequence).as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    pub fn block_delete(
        &self,
        scope: &str,
        operation: &PendingDelete,
        reason: &str,
    ) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let mut queue = tx.open_table(DELETE_OUTBOX).map_err(database_error)?;
            let key = operation_key(scope, operation.sequence);
            let value = queue
                .get(key.as_str())
                .map_err(database_error)?
                .map(|row| row.value().to_owned());
            if let Some(value) = value {
                let mut current: PendingDelete = decode(&value)?;
                current.blocked = Some(reason.into());
                queue
                    .insert(key.as_str(), encode(&current)?.as_str())
                    .map_err(database_error)?;
            }
        }
        tx.commit().map_err(database_error)
    }

    pub fn queue_keyword(
        &self,
        scope: &str,
        email_id: &str,
        keyword: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.queue_keywords(scope, email_id, &[(keyword.to_owned(), enabled)])
    }

    /// Save a composer draft durably in redb. Local draft ids are stable across
    /// restarts and are scoped by account; binary attachments need the separate
    /// upload/blob adapter and are rejected here instead of being lost silently.
    pub fn save_local_draft(&self, scope: &str, mut email: Value) -> Result<String, String> {
        if email["keywords"]["$draft"] != true
            || email.get("attachments").is_some()
            || email.get("bodyStructure").is_some()
        {
            return Err("Lokale Entwürfe unterstützen derzeit keine Dateianhänge.".into());
        }
        let tx = self.0.begin_write().map_err(database_error)?;
        let id;
        {
            let mut metadata_table = tx.open_table(REPLICAS).map_err(database_error)?;
            let mut meta: Metadata = metadata_table
                .get(scope)
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .unwrap_or_default();
            meta.next_operation = meta
                .next_operation
                .checked_add(1)
                .ok_or("Lokale Entwurfsnummer ist ausgeschöpft.")?;
            id = format!("local-draft-{:020}", meta.next_operation);
            email["id"] = Value::String(id.clone());
            let encoded = encode(&email)?;
            if encoded.len() > 5 * 1024 * 1024 {
                return Err("Entwurf überschreitet die lokale Speichergrenze.".into());
            }
            tx.open_table(DRAFTS)
                .map_err(database_error)?
                .insert(
                    object_key(scope, Collection::Email, &id).as_str(),
                    encoded.as_str(),
                )
                .map_err(database_error)?;
            metadata_table
                .insert(scope, encode(&meta)?.as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(id)
    }

    pub fn delete_local_draft(&self, scope: &str, id: &str) -> Result<(), String> {
        if !id.starts_with("local-draft-") {
            return Err("notFound".into());
        }
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let mut table = tx.open_table(DRAFTS).map_err(database_error)?;
            if table
                .remove(object_key(scope, Collection::Email, id).as_str())
                .map_err(database_error)?
                .is_none()
            {
                return Err("notFound".into());
            }
            let mut metadata_table = tx.open_table(REPLICAS).map_err(database_error)?;
            let mut meta: Metadata = metadata_table
                .get(scope)
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .unwrap_or_default();
            meta.next_operation = meta
                .next_operation
                .checked_add(1)
                .ok_or("Lokale Entwurfsnummer ist ausgeschöpft.")?;
            metadata_table
                .insert(scope, encode(&meta)?.as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    pub fn queue_keywords(
        &self,
        scope: &str,
        email_id: &str,
        changes: &[(String, bool)],
    ) -> Result<(), String> {
        self.queue_email_update(scope, email_id, changes, None)
    }

    pub fn queue_email_update(
        &self,
        scope: &str,
        email_id: &str,
        changes: &[(String, bool)],
        mailbox_ids: Option<Value>,
    ) -> Result<(), String> {
        if changes
            .iter()
            .any(|(keyword, _)| !["$seen", "$flagged"].contains(&keyword.as_str()))
        {
            return Err("Nicht unterstützte Offline-Aktion.".into());
        }
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let objects = tx.open_table(OBJECTS).map_err(database_error)?;
            if objects
                .get(object_key(scope, Collection::Email, email_id).as_str())
                .map_err(database_error)?
                .is_none()
            {
                return Err("Nachricht ist lokal nicht vorhanden.".into());
            }
            let mut table = tx.open_table(REPLICAS).map_err(database_error)?;
            let mut meta: Metadata = table
                .get(scope)
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?
                .unwrap_or_default();
            let mut queue = tx.open_table(OUTBOX).map_err(database_error)?;
            let mut entries = changes.to_vec();
            if mailbox_ids.is_some() {
                entries.push((String::new(), false));
            }
            for (keyword, enabled) in &entries {
                meta.next_operation = meta
                    .next_operation
                    .checked_add(1)
                    .ok_or("Lokale Warteschlange ist voll.")?;
                let operation = PendingKeyword {
                    sequence: meta.next_operation,
                    email_id: email_id.into(),
                    keyword: keyword.clone(),
                    mailbox_ids: if keyword.is_empty() {
                        mailbox_ids.clone()
                    } else {
                        None
                    },
                    enabled: *enabled,
                    blocked: None,
                };
                queue
                    .insert(
                        operation_key(scope, operation.sequence).as_str(),
                        encode(&operation)?.as_str(),
                    )
                    .map_err(database_error)?;
            }
            table
                .insert(scope, encode(&meta)?.as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    /// Acknowledge a specific operation and update its baseline atomically.
    pub fn acknowledge_keyword(
        &self,
        scope: &str,
        operation: &PendingKeyword,
    ) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let mut objects = tx.open_table(OBJECTS).map_err(database_error)?;
            let key = object_key(scope, Collection::Email, &operation.email_id);
            let object: Option<Value> = objects
                .get(key.as_str())
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?;
            if let Some(mut email) = object {
                apply_keyword(&mut email, operation)?;
                objects
                    .insert(key.as_str(), encode(&email)?.as_str())
                    .map_err(database_error)?;
            }
            let mut queue = tx.open_table(OUTBOX).map_err(database_error)?;
            queue
                .remove(operation_key(scope, operation.sequence).as_str())
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    pub fn block_keyword(
        &self,
        scope: &str,
        operation: &PendingKeyword,
        reason: &str,
    ) -> Result<(), String> {
        let tx = self.0.begin_write().map_err(database_error)?;
        {
            let mut queue = tx.open_table(OUTBOX).map_err(database_error)?;
            let key = operation_key(scope, operation.sequence);
            let current: Option<PendingKeyword> = queue
                .get(key.as_str())
                .map_err(database_error)?
                .map(|v| decode(v.value()))
                .transpose()?;
            if let Some(mut current) = current {
                current.blocked = Some(reason.into());
                queue
                    .insert(key.as_str(), encode(&current)?.as_str())
                    .map_err(database_error)?;
            }
        }
        tx.commit().map_err(database_error)
    }
}

fn apply_keyword(email: &mut Value, operation: &PendingKeyword) -> Result<(), String> {
    if let Some(ids) = &operation.mailbox_ids {
        email["mailboxIds"] = ids.clone();
        return Ok(());
    }
    let keywords = email
        .get_mut("keywords")
        .and_then(Value::as_object_mut)
        .ok_or("Nachrichten-Keywords sind ungültig.")?;
    if operation.enabled {
        keywords.insert(operation.keyword.clone(), Value::Bool(true));
    } else {
        keywords.remove(&operation.keyword);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed(store: &Store, scope: &str) {
        store
            .apply_remote(
                scope,
                Collection::Email,
                None,
                "s1",
                &[json!({"id":"m1","keywords":{},"subject":"Offline"})],
                &[],
                true,
            )
            .unwrap();
    }

    #[test]
    fn mailbox_move_survives_restart_and_newer_move_survives_ack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("move.redb");
        {
            let store = Store::open(&path).unwrap();
            seed(&store, "a");
            store
                .queue_email_update("a", "m1", &[], Some(json!({"trash":true})))
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let local = store.local_mail("a").unwrap();
        assert_eq!(local.emails[0]["mailboxIds"], json!({"trash":true}));
        assert!(store.local_mail("b").unwrap().pending.is_empty());
        let older = local.pending[0].clone();
        store
            .apply_remote(
                "a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[json!({"id":"m1","keywords":{},"mailboxIds":{"inbox":true}})],
                &[],
                true,
            )
            .unwrap();
        assert_eq!(
            store.local_mail("a").unwrap().emails[0]["mailboxIds"],
            json!({"trash":true})
        );
        store
            .queue_email_update("a", "m1", &[], Some(json!({"inbox":true})))
            .unwrap();
        store.acknowledge_keyword("a", &older).unwrap();
        let local = store.local_mail("a").unwrap();
        assert_eq!(local.emails[0]["mailboxIds"], json!({"inbox":true}));
        assert_eq!(local.pending.len(), 1);
    }

    #[test]
    fn local_intent_survives_restart_full_resync_and_acknowledging_older_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replica.redb");
        {
            let store = Store::open(&path).unwrap();
            seed(&store, "a");
            store.queue_keyword("a", "m1", "$seen", true).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.local_mail("a").unwrap().emails[0]["keywords"]["$seen"],
            true
        );
        assert!(store.local_mail("b").unwrap().emails.is_empty());
        store
            .apply_remote(
                "a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[json!({"id":"m1","keywords":{}})],
                &[],
                true,
            )
            .unwrap();
        let older = store.local_mail("a").unwrap().pending.remove(0);
        store.queue_keyword("a", "m1", "$seen", false).unwrap();
        store.acknowledge_keyword("a", &older).unwrap();
        let local = store.local_mail("a").unwrap();
        assert!(local.emails[0]["keywords"].get("$seen").is_none());
        assert_eq!(local.pending.len(), 1);
    }

    #[test]
    fn failed_state_comparison_or_invalid_object_rolls_back_everything() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("replica.redb")).unwrap();
        seed(&store, "a");
        assert!(store
            .apply_remote(
                "a",
                Collection::Email,
                Some("wrong"),
                "s2",
                &[],
                &["m1".into()],
                false
            )
            .is_err());
        assert!(store
            .apply_remote(
                "a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[json!({})],
                &["m1".into()],
                false
            )
            .is_err());
        assert_eq!(
            store.metadata("a").unwrap().email_state.as_deref(),
            Some("s1")
        );
        assert_eq!(store.local_mail("a").unwrap().emails.len(), 1);
        store
            .apply_remote(
                "a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[],
                &["m1".into()],
                false,
            )
            .unwrap();
        assert!(store.local_mail("a").unwrap().emails.is_empty());
    }

    #[test]
    fn local_drafts_survive_restart_are_account_scoped_and_can_be_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drafts.redb");
        let id = {
            let store = Store::open(&path).unwrap();
            let id = store
                .save_local_draft(
                    "account-a",
                    json!({
                        "from":[{"email":"sender@example.test"}],
                        "to":[{"email":"recipient@example.test"}],
                        "subject":"Local draft",
                        "keywords":{"$seen":true,"$draft":true},
                        "mailboxIds":{"drafts":true},
                        "bodyValues":{"1":{"value":"Saved before sending"}},
                        "textBody":[{"partId":"1"}]
                    }),
                )
                .unwrap();
            assert!(id.starts_with("local-draft-"));
            id
        };
        let store = Store::open(&path).unwrap();
        let mail = store.local_mail("account-a").unwrap();
        assert_eq!(mail.emails.len(), 1);
        assert_eq!(mail.emails[0]["id"], id);
        assert_eq!(
            mail.emails[0]["bodyValues"]["1"]["value"],
            "Saved before sending"
        );
        assert!(store.local_mail("account-b").unwrap().emails.is_empty());
        store.delete_local_draft("account-a", &id).unwrap();
        assert!(store.local_mail("account-a").unwrap().emails.is_empty());
    }

    #[test]
    fn local_draft_rejects_binary_attachments_instead_of_saving_a_broken_reference() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("drafts.redb")).unwrap();
        let result = store.save_local_draft(
            "account-a",
            json!({
                "keywords":{"$draft":true}, "attachments":[{"blobId":"remote-only"}]
            }),
        );
        assert!(result.is_err());
        assert!(store.local_mail("account-a").unwrap().emails.is_empty());
    }

    #[test]
    fn server_draft_delete_is_durable_and_hidden_until_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delete-draft.redb");
        {
            let store = Store::open(&path).unwrap();
            store.apply_remote("account-a", Collection::Email, None, "s1", &[
                json!({"id":"server-draft","keywords":{"$draft":true},"mailboxIds":{"drafts":true}})
            ], &[], true).unwrap();
            store
                .apply_remote("account-a", Collection::Mailbox, None, "m1", &[], &[], true)
                .unwrap();
            let operation = store
                .queue_draft_delete("account-a", "server-draft")
                .unwrap();
            assert!(store.local_mail("account-a").unwrap().emails.is_empty());
            assert_eq!(
                store.local_mail("account-a").unwrap().pending_deletes.len(),
                1
            );
            drop(operation);
        }
        let store = Store::open(&path).unwrap();
        let operation = store
            .local_mail("account-a")
            .unwrap()
            .pending_deletes
            .remove(0);
        store.apply_remote("account-a", Collection::Email, Some("s1"), "s2", &[
            json!({"id":"server-draft","keywords":{"$draft":true},"mailboxIds":{"drafts":true}})
        ], &[], true).unwrap();
        assert!(store.local_mail("account-a").unwrap().emails.is_empty());
        assert!(store
            .local_mail("account-b")
            .unwrap()
            .pending_deletes
            .is_empty());
        store.acknowledge_delete("account-a", &operation).unwrap();
        assert!(store
            .local_mail("account-a")
            .unwrap()
            .pending_deletes
            .is_empty());
    }

    #[test]
    fn cannot_queue_delete_for_a_non_draft_email() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("not-draft.redb")).unwrap();
        seed(&store, "account-a");
        assert!(store.queue_draft_delete("account-a", "m1").is_err());
    }

    #[test]
    fn external_server_cleanup_reconciles_a_blocked_local_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("external-delete.redb")).unwrap();
        store
            .apply_remote(
                "account-a",
                Collection::Email,
                None,
                "s1",
                &[json!({"id":"draft-1","keywords":{"$draft":true}})],
                &[],
                true,
            )
            .unwrap();
        let operation = store.queue_draft_delete("account-a", "draft-1").unwrap();
        store
            .block_delete("account-a", &operation, "forbidden")
            .unwrap();
        store
            .apply_remote(
                "account-a",
                Collection::Email,
                Some("s1"),
                "s2",
                &[],
                &["draft-1".into()],
                false,
            )
            .unwrap();
        let mail = store.local_mail("account-a").unwrap();
        assert!(mail.pending_deletes.is_empty());
        assert!(mail.emails.is_empty());
    }
}
