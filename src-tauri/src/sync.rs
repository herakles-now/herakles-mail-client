//! Server access belongs here, never in the local UI repository.
use crate::{
    config::TestConfig,
    replica::{Collection, PendingKeyword},
    storage::Store,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashSet},
    time::Duration,
};

const CORE: &str = "urn:ietf:params:jmap:core";
const MAIL: &str = "urn:ietf:params:jmap:mail";
const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";
const EMAIL_PROPERTIES: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "from",
    "to",
    "cc",
    "bcc",
    "replyTo",
    "subject",
    "preview",
    "hasAttachment",
    "sentAt",
    "messageId",
    "inReplyTo",
    "references",
    "textBody",
    "htmlBody",
    "attachments",
    "bodyStructure",
    "bodyValues",
];

#[derive(Debug)]
pub enum SyncError {
    Local(String),
    Network,
    Protocol,
    ChangedDuringSnapshot,
    Method(String),
}

impl From<String> for SyncError {
    fn from(value: String) -> Self {
        Self::Local(value)
    }
}
impl SyncError {
    pub fn message(&self) -> String {
        match self {
            Self::Local(message) => message.clone(),
            Self::Network => "Server nicht erreichbar. Lokale Daten und ausstehende Änderungen bleiben erhalten.".into(),
            Self::Protocol => "Unerwartete JMAP-Antwort erhalten.".into(),
            Self::ChangedDuringSnapshot => "Serverdaten wurden während des Erstabgleichs geändert. Bitte erneut synchronisieren.".into(),
            Self::Method(kind) if kind == "stateMismatch" => "Serverzustand wurde geändert. Lokale Änderung bleibt für den nächsten Abgleich vorgemerkt.".into(),
            Self::Method(_) => "Server hat einen Abgleich oder eine Änderung abgelehnt. Lokale Änderungen bleiben vorgemerkt.".into(),
        }
    }
}

pub trait Remote {
    fn call(
        &self,
        method: &str,
        arguments: Value,
    ) -> impl std::future::Future<Output = Result<Value, SyncError>> + Send;
}

pub struct Connection {
    client: reqwest::Client,
    url: url::Url,
    username: String,
    password: String,
    read_only: bool,
    download_template: Option<String>,
}

fn authorize_method(read_only: bool, method: &str) -> Result<(), SyncError> {
    let allowed_read = [
        "Mailbox/get",
        "Email/get",
        "Email/query",
        "Email/changes",
        "Calendar/get",
        "CalendarEvent/get",
        "CalendarEvent/query",
        "CalendarEvent/changes",
        "ParticipantIdentity/get",
    ]
    .contains(&method);
    if !allowed_read && (read_only || method != "Email/set") {
        return Err(SyncError::Local(
            if read_only {
                "Nur-Lese-Modus: Serveränderungen sind gesperrt."
            } else {
                "Diese Schreibfunktion ist noch nicht freigegeben."
            }
            .into(),
        ));
    }
    Ok(())
}

async fn read_json(mut response: reqwest::Response) -> Result<Value, SyncError> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(SyncError::Protocol);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| SyncError::Network)? {
        if body.len() + chunk.len() > 64 * 1024 * 1024 {
            return Err(SyncError::Protocol);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| SyncError::Protocol)
}

fn blob_download_url(
    origin: &url::Url,
    template: &str,
    account: &str,
    blob: &str,
) -> Result<url::Url, SyncError> {
    if [account, blob].iter().any(|id| {
        id.is_empty()
            || *id == "."
            || *id == ".."
            || id.len() > 4096
            || id.chars().any(char::is_control)
    }) {
        return Err(SyncError::Protocol);
    }
    fn encode(value: &str) -> String {
        let mut output = String::new();
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                output.push(byte as char);
            } else {
                output.push_str(&format!("%{byte:02X}"));
            }
        }
        output
    }
    let mut expanded = template.to_owned();
    for (variable, value) in [
        ("accountId", account),
        ("blobId", blob),
        ("name", "download"),
        ("type", "application/octet-stream"),
    ] {
        let token = format!("{{{variable}}}");
        if !expanded.contains(&token) {
            return Err(SyncError::Protocol);
        }
        expanded = expanded.replace(&token, &encode(value));
    }
    if expanded.contains(['{', '}']) {
        return Err(SyncError::Protocol);
    }
    let url = url::Url::parse(&expanded).map_err(|_| SyncError::Protocol)?;
    if url.scheme() != "https"
        || url.origin() != origin.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(SyncError::Protocol);
    }
    Ok(url)
}

impl Connection {
    pub async fn download_blob(
        &self,
        account: &str,
        reference: &crate::blobs::BlobRef,
    ) -> Result<Vec<u8>, SyncError> {
        if reference
            .size
            .is_some_and(|size| size > crate::blobs::MAX_BLOB_BYTES as u64)
        {
            return Err(SyncError::Local(
                "Anhang ist groesser als das Downloadlimit von 25 MiB.".into(),
            ));
        }
        let url = blob_download_url(
            &self.url,
            self.download_template
                .as_deref()
                .ok_or(SyncError::Protocol)?,
            account,
            &reference.id,
        )?;
        let mut response = self
            .client
            .get(url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|_| SyncError::Network)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(SyncError::Protocol);
        }
        if response
            .content_length()
            .is_some_and(|size| size > crate::blobs::MAX_BLOB_BYTES as u64)
        {
            return Err(SyncError::Local(
                "Anhang ist groesser als das Downloadlimit von 25 MiB.".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| SyncError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > crate::blobs::MAX_BLOB_BYTES {
                return Err(SyncError::Local(
                    "Anhang ist groesser als das Downloadlimit von 25 MiB.".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if reference
            .size
            .is_some_and(|size| size != bytes.len() as u64)
        {
            return Err(SyncError::Protocol);
        }
        Ok(bytes)
    }

    pub async fn discover(config: &TestConfig, account: &str) -> Result<Self, SyncError> {
        Self::discover_for(config, account, MAIL).await
    }

    pub async fn discover_for(
        config: &TestConfig,
        account: &str,
        capability: &str,
    ) -> Result<Self, SyncError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| SyncError::Network)?;
        let session: Value = serde_json::from_slice(
            &crate::session::session_bytes(config)
                .await
                .map_err(SyncError::Local)?,
        )
        .map_err(|_| SyncError::Protocol)?;
        if session["capabilities"].get(CORE).is_none()
            || !account_supports(&session, account, capability)
        {
            return Err(SyncError::Protocol);
        }
        let url = url::Url::parse(session["apiUrl"].as_str().ok_or(SyncError::Protocol)?)
            .map_err(|_| SyncError::Protocol)?;
        // Never forward Basic credentials to a different origin supplied by the session.
        if url.origin() != config.url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(SyncError::Protocol);
        }
        Ok(Self {
            client,
            url,
            username: config.username.clone(),
            password: config.password.clone(),
            read_only: config.read_only,
            download_template: session["downloadUrl"].as_str().map(str::to_owned),
        })
    }
}

pub fn account_supports(session: &Value, account: &str, capability: &str) -> bool {
    let Some(account) = session["accounts"].get(account) else {
        return false;
    };
    let caps = account
        .get("accountCapabilities")
        .and_then(Value::as_object);
    if caps.is_some_and(|caps| caps.contains_key(capability)) {
        return true;
    }
    account["isPersonal"] == false
        && caps.is_none_or(|caps| caps.is_empty())
        && session["capabilities"].get(capability).is_some()
}

fn composer_method_capability(method: &str) -> &'static str {
    if method.starts_with("Identity/") || method.starts_with("EmailSubmission/") {
        SUBMISSION
    } else {
        MAIL
    }
}

fn composer_using(method_calls: &[(String, Value, String)]) -> Vec<&'static str> {
    if method_calls
        .iter()
        .any(|(method, _, _)| composer_method_capability(method) == SUBMISSION)
    {
        vec![CORE, MAIL, SUBMISSION]
    } else {
        vec![CORE, MAIL]
    }
}

fn failed_composer_copy_targets(
    answer: &Value,
    method_calls: &[(String, Value, String)],
) -> Vec<(String, Vec<String>)> {
    let rows = answer["methodResponses"].as_array();
    let submitted = rows.is_some_and(|rows| {
        rows.iter().any(|row| {
            row[0] == "EmailSubmission/set"
                && row[1]["created"]
                    .as_object()
                    .is_some_and(|created| !created.is_empty())
        })
    });
    let refused_all = rows.is_some_and(|rows| {
        rows.iter().any(|row| {
            row[0] == "EmailSubmission/get"
                && row[2] == "deliveryStatus"
                && row[1]["list"]
                    .as_array()
                    .and_then(|list| list.first())
                    .and_then(|submission| submission["deliveryStatus"].as_object())
                    .is_some_and(|statuses| {
                        !statuses.is_empty()
                            && statuses.values().all(|status| status["delivered"] == "no")
                    })
        })
    });
    if submitted && !refused_all {
        return Vec::new();
    }
    let Some(rows) = rows else {
        return Vec::new();
    };
    method_calls
        .iter()
        .filter_map(|(method, args, call_id)| {
            if method != "Email/set" {
                return None;
            }
            let account = args["accountId"].as_str()?.to_owned();
            let ids = rows
                .iter()
                .find(|row| row[0] == "Email/set" && row[2].as_str() == Some(call_id.as_str()))?[1]
                ["created"]
                .as_object()?
                .values()
                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            (!ids.is_empty()).then_some((account, ids))
        })
        .collect()
}

impl Remote for Connection {
    async fn call(&self, method: &str, arguments: Value) -> Result<Value, SyncError> {
        // Enforce before constructing or sending any HTTP request. New methods are denied by default.
        authorize_method(self.read_only, method)?;
        let capability =
            if method.starts_with("Calendar") || method.starts_with("ParticipantIdentity/") {
                CALENDARS
            } else {
                MAIL
            };
        let response = self
            .client
            .post(self.url.clone())
            .basic_auth(&self.username, Some(&self.password))
            .json(&json!({"using":[CORE,capability],"methodCalls":[[method,arguments,"call"]]}))
            .send()
            .await
            .map_err(|_| SyncError::Network)?;
        let answer = read_json(response).await?;
        let rows = answer["methodResponses"]
            .as_array()
            .ok_or(SyncError::Protocol)?;
        let row = rows
            .iter()
            .find(|row| row[2] == "call")
            .ok_or(SyncError::Protocol)?;
        if row[0] == "error" {
            #[cfg(test)]
            {
                let kind = row[1]["type"].as_str().unwrap_or("other");
                let safe_kind = if [
                    "forbidden",
                    "accountNotFound",
                    "accountNotSupportedByMethod",
                    "unknownMethod",
                ]
                .contains(&kind)
                {
                    kind
                } else {
                    "other"
                };
                eprintln!("{method}: request rejected ({safe_kind})");
            }
            return Err(SyncError::Method(
                row[1]["type"].as_str().unwrap_or("unknown").into(),
            ));
        }
        if row[0] != method {
            return Err(SyncError::Protocol);
        }
        if arguments.get("accountId").is_some()
            && row[1].get("accountId") != arguments.get("accountId")
        {
            return Err(SyncError::Protocol);
        }
        Ok(row[1].clone())
    }
}

fn submission_receipt_key(config: &TestConfig, account: &str, id: &str) -> String {
    format!(
        "submission-receipt:{}",
        serde_json::to_string(&(scope(config, account), id)).expect("string tuple")
    )
}
fn record_submission_receipts(
    store: &Store,
    config: &TestConfig,
    calls: &[(String, Value, String)],
    response: &Value,
) -> Result<(), SyncError> {
    for (method, args, call_id) in calls {
        if method != "EmailSubmission/set" {
            continue;
        }
        let account = args["accountId"].as_str().ok_or(SyncError::Protocol)?;
        if let Some(created) = response["methodResponses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row[0] == "EmailSubmission/set" && row[2] == *call_id)
            .and_then(|row| row[1]["created"].as_object())
        {
            for value in created.values() {
                if let Some(id) = value["id"].as_str() {
                    store.put(&submission_receipt_key(config, account, id), "accepted")?;
                }
            }
        }
    }
    Ok(())
}
fn validate_submission_cleanup(
    store: &Store,
    config: &TestConfig,
    calls: &[(String, Value, String)],
) -> Result<(), SyncError> {
    for (method, args, _) in calls {
        let Some(object) = args.as_object() else {
            return Err(SyncError::Protocol);
        };
        if method != "EmailSubmission/set"
            || object
                .keys()
                .any(|key| !["accountId", "destroy"].contains(&key.as_str()))
        {
            return Err(SyncError::Protocol);
        }
        let account = args["accountId"].as_str().ok_or(SyncError::Protocol)?;
        let ids = args["destroy"]
            .as_array()
            .filter(|ids| !ids.is_empty() && ids.len() <= 50)
            .ok_or(SyncError::Protocol)?;
        for id in ids {
            let id = id.as_str().ok_or(SyncError::Protocol)?;
            if store
                .get(&submission_receipt_key(config, account, id))?
                .as_deref()
                != Some("accepted")
            {
                return Err(SyncError::Method("forbidden".into()));
            }
        }
    }
    Ok(())
}

/// Narrow native bridge for Bulwark's unchanged composer. All credentials and
/// authenticated HTTP stay in Rust; only identity reads and explicit send
/// batches are accepted here. Submission requests are deliberately never
/// retried because a lost response could otherwise send a duplicate message.
pub async fn composer_request(
    store: &Store,
    config: &TestConfig,
    method_calls: &[(String, Value, String)],
) -> Result<Value, SyncError> {
    if method_calls.is_empty() || method_calls.len() > 4 {
        return Err(SyncError::Protocol);
    }
    if method_calls.iter().any(|(method, _, _)| {
        ![
            "Identity/get",
            "Mailbox/get",
            "Email/set",
            "EmailSubmission/set",
            "EmailSubmission/get",
        ]
        .contains(&method.as_str())
    }) {
        return Err(SyncError::Local(
            "Nicht unterstützter Composer-Aufruf.".into(),
        ));
    }
    let sending = method_calls
        .iter()
        .any(|(method, args, _)| method == "EmailSubmission/set" && args.get("create").is_some());
    let cleaning = !sending
        && method_calls
            .iter()
            .any(|(method, _, _)| method == "EmailSubmission/set");
    if cleaning {
        validate_submission_cleanup(store, config, method_calls)?;
    }
    if method_calls
        .iter()
        .any(|(method, _, _)| method == "Email/set")
        && !sending
    {
        return Err(SyncError::Local(
            "Email/set ist im Composer nur zusammen mit einer Sendung erlaubt.".into(),
        ));
    }
    if (sending || cleaning) && config.read_only {
        return Err(SyncError::Local(
            "Nur-Lese-Modus: Versand ist gesperrt.".into(),
        ));
    }
    if sending
        && (!method_calls
            .iter()
            .any(|(method, _, _)| method == "Email/set")
            || method_calls
                .iter()
                .filter(|(method, _, _)| method == "EmailSubmission/set")
                .count()
                != 1)
    {
        return Err(SyncError::Protocol);
    }
    let session: Value = serde_json::from_slice(
        &crate::session::session_bytes(config)
            .await
            .map_err(SyncError::Local)?,
    )
    .map_err(|_| SyncError::Protocol)?;
    for (method, args, _) in method_calls {
        let account = args
            .get("accountId")
            .and_then(Value::as_str)
            .ok_or(SyncError::Protocol)?;
        let capability = composer_method_capability(method);
        if !account_supports(&session, account, capability) {
            return Err(SyncError::Method("accountNotSupportedByMethod".into()));
        }
        if (sending || cleaning) && session["accounts"][account]["isReadOnly"] == true {
            return Err(SyncError::Method("forbidden".into()));
        }
        if sending
            && matches!(method.as_str(), "Email/set" | "EmailSubmission/set")
            && session["accounts"][account]["isReadOnly"] == true
        {
            return Err(SyncError::Method("forbidden".into()));
        }
        if method == "Email/set"
            && (args.get("update").is_some()
                || args.get("destroy").is_some()
                || args
                    .get("create")
                    .and_then(Value::as_object)
                    .is_none_or(|create| create.is_empty()))
        {
            return Err(SyncError::Local(
                "Der Composer darf nur neue Nachrichten anlegen.".into(),
            ));
        }
        if sending
            && method == "EmailSubmission/set"
            && (args.get("destroy").is_some() || args.get("update").is_some())
        {
            return Err(SyncError::Protocol);
        }
        if sending
            && method == "EmailSubmission/set"
            && (!account_supports(&session, account, SUBMISSION)
                || args
                    .get("create")
                    .and_then(Value::as_object)
                    .is_none_or(|create| create.is_empty()))
        {
            return Err(SyncError::Method("accountNotSupportedByMethod".into()));
        }
    }
    let api = url::Url::parse(session["apiUrl"].as_str().ok_or(SyncError::Protocol)?)
        .map_err(|_| SyncError::Protocol)?;
    if api.origin() != config.url.origin() || !api.username().is_empty() || api.password().is_some()
    {
        return Err(SyncError::Protocol);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| SyncError::Network)?;
    let using = composer_using(method_calls);
    let response = client
        .post(api)
        .basic_auth(&config.username, Some(&config.password))
        .json(&json!({"using":using,"methodCalls":method_calls}))
        .send()
        .await
        .map_err(|_| SyncError::Network)?;
    let answer = read_json(response).await?;
    if sending {
        if record_submission_receipts(store, config, method_calls, &answer).is_err() {
            // An accepted send must not become a failed send because local housekeeping failed.
            crate::logging::event("error", "composer", "receipt_persist_failed", json!({}));
        }
        // The response is definitive: clean only the exact new copy if the
        // submission was refused or every recipient was rejected. We never
        // retry an ambiguous transport result.
        for (account, created) in failed_composer_copy_targets(&answer, method_calls) {
            if let Ok(connection) = Connection::discover(config, &account).await {
                let _ = connection
                    .call("Email/set", json!({"accountId":account,"destroy":created}))
                    .await;
            }
        }
    }
    Ok(answer)
}

pub fn scope(config: &TestConfig, account: &str) -> String {
    serde_json::to_string(&(config.cache_key(), account)).expect("string serialization")
}

fn text(value: &Value, field: &str) -> Result<String, SyncError> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or(SyncError::Protocol)
}
fn strings(value: &Value, field: &str) -> Result<Vec<String>, SyncError> {
    value[field]
        .as_array()
        .ok_or(SyncError::Protocol)?
        .iter()
        .map(|id| id.as_str().map(str::to_owned).ok_or(SyncError::Protocol))
        .collect()
}

async fn get_emails(
    remote: &impl Remote,
    account: &str,
    ids: &[String],
) -> Result<Value, SyncError> {
    let response = remote
        .call(
            "Email/get",
            json!({"accountId":account,"ids":ids,"properties":EMAIL_PROPERTIES,
        "fetchTextBodyValues":true,"fetchHTMLBodyValues":true,"maxBodyValueBytes":262144}),
        )
        .await?;
    let expected: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let list = response["list"].as_array().ok_or(SyncError::Protocol)?;
    let missing = strings(&response, "notFound")?;
    let mut received = BTreeSet::new();
    for object in list {
        let id = object["id"].as_str().ok_or(SyncError::Protocol)?;
        if !received.insert(id) {
            return Err(SyncError::Protocol);
        }
    }
    for id in &missing {
        if !received.insert(id.as_str()) {
            return Err(SyncError::Protocol);
        }
    }
    if received != expected {
        return Err(SyncError::Protocol);
    }
    Ok(response)
}

async fn snapshot(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
    expected: Option<&str>,
) -> Result<(), SyncError> {
    let baseline = text(&get_emails(remote, account, &[]).await?, "state")?;
    let mut position = 0_u64;
    let mut query_state: Option<String> = None;
    let mut objects = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let query = remote
            .call(
                "Email/query",
                json!({"accountId":account,"position":position,"limit":250,"calculateTotal":true}),
            )
            .await?;
        let state = text(&query, "queryState")?;
        if query_state.as_ref().is_some_and(|prior| prior != &state) {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        query_state = Some(state);
        let ids = strings(&query, "ids")?;
        if ids.is_empty() {
            break;
        }
        if ids.iter().any(|id| !seen.insert(id.clone())) {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        for chunk in ids.chunks(50) {
            let answer = get_emails(remote, account, chunk).await?;
            if text(&answer, "state")? != baseline || !strings(&answer, "notFound")?.is_empty() {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            let list = answer["list"].as_array().ok_or(SyncError::Protocol)?;
            if list.len() != chunk.len() {
                return Err(SyncError::Protocol);
            }
            objects.extend(list.iter().cloned());
        }
        position += ids.len() as u64;
        if query["total"]
            .as_u64()
            .is_some_and(|total| position >= total)
        {
            break;
        }
    }
    if text(&get_emails(remote, account, &[]).await?, "state")? != baseline {
        return Err(SyncError::ChangedDuringSnapshot);
    }
    store.apply_remote(
        scope,
        Collection::Email,
        expected,
        &baseline,
        &objects,
        &[],
        true,
    )?;
    Ok(())
}

async fn pull_emails(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    let Some(mut since) = store.metadata(scope)?.email_state else {
        return snapshot(store, remote, scope, account, None).await;
    };
    loop {
        let changes = match remote
            .call(
                "Email/changes",
                json!({"accountId":account,"sinceState":since,"maxChanges":250}),
            )
            .await
        {
            Err(SyncError::Method(kind)) if kind == "cannotCalculateChanges" => {
                return snapshot(store, remote, scope, account, Some(&since)).await
            }
            answer => answer?,
        };
        if text(&changes, "oldState")? != since {
            return Err(SyncError::Protocol);
        }
        let next = text(&changes, "newState")?;
        let more = changes["hasMoreChanges"]
            .as_bool()
            .ok_or(SyncError::Protocol)?;
        if more && next == since {
            return Err(SyncError::Protocol);
        }
        let ids: BTreeSet<String> = strings(&changes, "created")?
            .into_iter()
            .chain(strings(&changes, "updated")?)
            .collect();
        let ids: Vec<String> = ids.into_iter().collect();
        let mut destroyed: BTreeSet<String> = strings(&changes, "destroyed")?.into_iter().collect();
        let mut objects = Vec::new();
        for chunk in ids.chunks(50) {
            let answer = get_emails(remote, account, chunk).await?;
            // get may see newer values; keep the changes token so the next delta still observes them.
            destroyed.extend(strings(&answer, "notFound")?);
            let list = answer["list"].as_array().ok_or(SyncError::Protocol)?;
            objects.extend(list.iter().cloned());
        }
        objects.retain(|object| {
            !object["id"]
                .as_str()
                .is_some_and(|id| destroyed.contains(id))
        });
        store.apply_remote(
            scope,
            Collection::Email,
            Some(&since),
            &next,
            &objects,
            &destroyed.into_iter().collect::<Vec<_>>(),
            false,
        )?;
        since = next;
        if !more {
            return Ok(());
        }
    }
}

async fn push_keyword(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
    operation: &PendingKeyword,
) -> Result<(), SyncError> {
    let state = store
        .metadata(scope)?
        .email_state
        .ok_or(SyncError::Protocol)?;
    let patch = format!(
        "keywords/{}",
        operation.keyword.replace('~', "~0").replace('/', "~1")
    );
    let enabled = if operation.enabled {
        Value::Bool(true)
    } else {
        Value::Null
    };
    let update = match &operation.mailbox_ids {
        Some(ids) => json!({"mailboxIds":ids}),
        None => json!({patch:enabled}),
    };
    let response = remote
        .call(
            "Email/set",
            json!({"accountId":account,"ifInState":state,"update":{&operation.email_id:update}}),
        )
        .await?;
    if response["updated"].get(&operation.email_id).is_some() {
        store.acknowledge_keyword(scope, operation)?;
        return Ok(());
    }
    Err(SyncError::Method(
        response["notUpdated"][&operation.email_id]["type"]
            .as_str()
            .unwrap_or("unknown")
            .into(),
    ))
}

#[cfg(test)]
pub async fn synchronize(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    synchronize_with_policy(store, remote, scope, account, false).await
}

pub async fn synchronize_with_policy(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
    read_only: bool,
) -> Result<(), SyncError> {
    pull_mailboxes(store, remote, scope, account).await?;
    pull_emails(store, remote, scope, account).await?;
    if read_only {
        return Ok(());
    }
    flush_pending_keywords(store, remote, scope, account).await?;
    flush_pending_deletes(store, remote, scope, account).await
}

async fn flush_pending_deletes(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    let operations = store.local_mail(scope)?.pending_deletes;
    for operation in operations {
        if operation.blocked.is_some() {
            continue;
        }
        let mut finished = false;
        for _ in 0..3 {
            let state = store
                .metadata(scope)?
                .email_state
                .ok_or(SyncError::Protocol)?;
            match remote
                .call(
                    "Email/set",
                    json!({"accountId":account,"ifInState":state,"destroy":[operation.email_id]}),
                )
                .await
            {
                Ok(result)
                    if result["destroyed"]
                        .as_array()
                        .is_some_and(|ids| ids.contains(&json!(operation.email_id))) =>
                {
                    store.acknowledge_delete(scope, &operation)?;
                    crate::logging::event("info", "sync", "draft_delete_synced", json!({}));
                    finished = true;
                    break;
                }
                Ok(result) if result["notDestroyed"][&operation.email_id]["type"] == "notFound" => {
                    store.acknowledge_delete(scope, &operation)?;
                    crate::logging::event("info", "sync", "draft_already_deleted", json!({}));
                    finished = true;
                    break;
                }
                Ok(result) => {
                    let kind = result["notDestroyed"][&operation.email_id]["type"]
                        .as_str()
                        .unwrap_or("unknown");
                    if kind == "stateMismatch" {
                        pull_emails(store, remote, scope, account).await?;
                        continue;
                    }
                    crate::logging::event(
                        "error",
                        "sync",
                        "draft_delete_blocked",
                        json!({"code":kind}),
                    );
                    store.block_delete(scope, &operation, &kind)?;
                    break;
                }
                Err(SyncError::Method(kind)) if kind == "stateMismatch" => {
                    pull_emails(store, remote, scope, account).await?;
                }
                Err(SyncError::Method(kind)) if kind == "notFound" => {
                    store.acknowledge_delete(scope, &operation)?;
                    crate::logging::event("info", "sync", "draft_already_deleted", json!({}));
                    finished = true;
                    break;
                }
                Err(SyncError::Method(kind)) if kind == "forbidden" => {
                    crate::logging::event(
                        "error",
                        "sync",
                        "draft_delete_blocked",
                        json!({"code":kind}),
                    );
                    store.block_delete(scope, &operation, &kind)?;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if !finished && operation.blocked.is_none() {
            let current = store
                .local_mail(scope)?
                .pending_deletes
                .into_iter()
                .find(|item| item.sequence == operation.sequence);
            if current.is_some_and(|item| item.blocked.is_none()) {
                return Err(SyncError::Method("stateMismatch".into()));
            }
        }
    }
    Ok(())
}

pub async fn pull_mailboxes(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    // Mailbox/get (ids:null) returns the complete accessible collection and its state.
    let mailboxes = remote
        .call("Mailbox/get", json!({"accountId":account,"ids":null}))
        .await?;
    let expected = store.metadata(scope)?.mailbox_state;
    store.apply_remote(
        scope,
        Collection::Mailbox,
        expected.as_deref(),
        &text(&mailboxes, "state")?,
        mailboxes["list"].as_array().ok_or(SyncError::Protocol)?,
        &[],
        true,
    )?;
    Ok(())
}

async fn flush_pending_keywords(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    // Snapshot the queue. Edits made while awaiting the server remain for the next pass.
    for operation in store.local_mail(scope)?.pending {
        if operation.blocked.is_some() {
            continue;
        }
        match push_keyword(store, remote, scope, account, &operation).await {
            Err(SyncError::Method(kind))
                if [
                    "notFound",
                    "forbidden",
                    "accountReadOnly",
                    "invalidPatch",
                    "invalidProperties",
                ]
                .contains(&kind.as_str()) =>
            {
                // Permanent failures remain visible and durable without starving later edits.
                store.block_keyword(scope, &operation, &kind)?;
                continue;
            }
            result => result?,
        }
        pull_emails(store, remote, scope, account).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    #[test]
    fn submission_cleanup_requires_a_persisted_account_scoped_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.redb");
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "test".into(),
            password: String::new(),
            read_only: false,
        };
        let calls = vec![(
            "EmailSubmission/set".into(),
            json!({"accountId":"a","create":{"1":{"emailId":"e"}}}),
            "send".into(),
        )];
        {
            let store = Store::open(&path).unwrap();
            record_submission_receipts(&store, &config, &calls, &json!({"methodResponses":[["EmailSubmission/set",{"created":{"1":{"id":"submission"}}},"send"]]})).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let cleanup = |account: &str, id: &str| {
            vec![(
                "EmailSubmission/set".into(),
                json!({"accountId":account,"destroy":[id]}),
                "cleanup".into(),
            )]
        };
        assert!(validate_submission_cleanup(&store, &config, &cleanup("a", "submission")).is_ok());
        assert!(validate_submission_cleanup(&store, &config, &cleanup("b", "submission")).is_err());
        assert!(validate_submission_cleanup(&store, &config, &cleanup("a", "unknown")).is_err());
        assert!(validate_submission_cleanup(
            &store,
            &config,
            &[(
                "EmailSubmission/set".into(),
                json!({"accountId":"a","destroy":["submission"],"update":{"other":{}}}),
                "bad".into()
            )]
        )
        .is_err());
        let mut other_login = config;
        other_login.username = "other".into();
        assert!(
            validate_submission_cleanup(&store, &other_login, &cleanup("a", "submission")).is_err()
        );
        other_login.username = "test".into();
        other_login.read_only = true;
        tauri::async_runtime::block_on(async {
            assert!(matches!(
                composer_request(&store, &other_login, &cleanup("a", "submission")).await,
                Err(SyncError::Local(_))
            ));
        });
    }

    #[test]
    #[ignore = "Read-only live attachment download into a temporary redb database"]
    fn live_blob_read_only() {
        tauri::async_runtime::block_on(async {
            let mut config =
                TestConfig::load().unwrap_or_else(|_| panic!("Testzugang nicht verfuegbar"));
            config.read_only = true;
            let session: Value = serde_json::from_slice(
                &crate::session::session_bytes(&config)
                    .await
                    .unwrap_or_else(|_| panic!("Sitzung nicht erreichbar")),
            )
            .unwrap_or_else(|_| panic!("Ungueltige Sitzung"));
            let account = session["primaryAccounts"][MAIL]
                .as_str()
                .expect("Kein primaeres Mailkonto");
            let remote = Connection::discover(&config, account)
                .await
                .unwrap_or_else(|e| panic!("{}", e.message()));
            let query = remote
                .call(
                    "Email/query",
                    json!({"accountId":account,"filter":{"hasAttachment":true},"limit":10}),
                )
                .await
                .unwrap_or_else(|e| panic!("{}", e.message()));
            let result = remote.call("Email/get", json!({"accountId":account,"ids":query["ids"],"properties":["id","blobId","size","keywords","attachments","bodyStructure"]})).await.unwrap_or_else(|e| panic!("{}",e.message()));
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("attachment-live.redb");
            let store = Store::open(&path).unwrap();
            let rows = result["list"].as_array().expect("Nachrichtenliste fehlt");
            store
                .apply_remote("live", Collection::Email, None, "sample", rows, &[], true)
                .unwrap();
            let reference = crate::blobs::references(&store.local_mail("live").unwrap())
                .into_iter()
                .find(|r| r.attachment && r.size.is_some_and(|size| size <= 1024 * 1024));
            let Some(reference) = reference else {
                panic!("Kein kleiner Anhang in der begrenzten Stichprobe vorhanden");
            };
            let bytes = remote
                .download_blob(account, &reference)
                .await
                .unwrap_or_else(|e| panic!("{}", e.message()));
            store.cache_blob("live", &reference.id, &bytes).unwrap();
            drop(store);
            let reopened = Store::open(&path).unwrap();
            assert!(
                reopened
                    .blob_bytes("live", &reference.id)
                    .unwrap()
                    .as_deref()
                    == Some(bytes.as_slice())
            );
            assert!(reopened
                .cached_blob("live", &reference.id)
                .unwrap()
                .is_some());
            assert!(reopened
                .blob_bytes("other-account", &reference.id)
                .unwrap()
                .is_none());
            println!("Ein Anhang read-only geladen und nach redb-Neustart geprueft; keine Inhalte protokolliert.");
        });
    }

    #[test]
    fn blob_download_templates_encode_ids_and_reject_credential_leaks() {
        let origin = url::Url::parse("https://mail.example.test/jmap").unwrap();
        let template = "https://mail.example.test/download/{accountId}/{blobId}/{name}?type={type}";
        let url = blob_download_url(&origin, template, "shared/a", "blob?#/{other}").unwrap();
        assert!(url
            .as_str()
            .contains("shared%2Fa/blob%3F%23%2F%7Bother%7D/download"));
        assert_eq!(url.query(), Some("type=application%2Foctet-stream"));
        for bad in [
            template.replace("https:", "http:"),
            template.replace("mail.example.test", "other.test"),
            template.replace("mail.example.test", "user@mail.example.test"),
            format!("{template}#fragment"),
            template.replace("{blobId}", "missing"),
            format!("{template}&extra={{unknown}}"),
        ] {
            assert!(blob_download_url(&origin, &bad, "a", "b").is_err());
        }
        assert!(blob_download_url(&origin, template, "a", "..").is_err());
    }

    #[test]
    fn mailbox_move_conflict_retains_queue_and_success_updates_baseline() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store
                .queue_email_update("scope", "m1", &[], Some(json!({"trash":true})))
                .unwrap();
            let operation = store.local_mail("scope").unwrap().pending.remove(0);
            let rejected = Script::new(vec![(
                "Email/set",
                Err(SyncError::Method("stateMismatch".into())),
            )]);
            assert!(push_keyword(&store, &rejected, "scope", "a", &operation)
                .await
                .is_err());
            assert_eq!(store.local_mail("scope").unwrap().pending.len(), 1);
            let accepted = Script::new(vec![("Email/set", Ok(json!({"updated":{"m1":null}})))]);
            push_keyword(&store, &accepted, "scope", "a", &operation)
                .await
                .unwrap();
            let local = store.local_mail("scope").unwrap();
            assert!(local.pending.is_empty());
            assert_eq!(local.emails[0]["mailboxIds"], json!({"trash":true}));
        });
    }

    #[test]
    fn identity_and_submission_calls_include_the_submission_capability() {
        let identity = vec![("Identity/get".into(), json!({"accountId":"a"}), "0".into())];
        let mailboxes = vec![("Mailbox/get".into(), json!({"accountId":"a"}), "0".into())];
        assert_eq!(composer_using(&identity), vec![CORE, MAIL, SUBMISSION]);
        assert_eq!(composer_using(&mailboxes), vec![CORE, MAIL]);
        assert_eq!(
            composer_method_capability("EmailSubmission/set"),
            SUBMISSION
        );
    }

    #[test]
    fn only_definitively_failed_composer_copies_are_scheduled_for_cleanup() {
        let calls = vec![
            ("Email/set".into(), json!({"accountId":"a"}), "email".into()),
            (
                "EmailSubmission/set".into(),
                json!({"accountId":"a"}),
                "submission".into(),
            ),
        ];
        let refused = json!({"methodResponses":[
            ["Email/set",{"created":{"send-1":{"id":"server-copy"}}},"email"],
            ["EmailSubmission/set",{"notCreated":{"1":{"type":"forbidden"}}},"submission"]
        ]});
        assert_eq!(
            failed_composer_copy_targets(&refused, &calls),
            vec![("a".into(), vec!["server-copy".into()])]
        );
        let accepted = json!({"methodResponses":[
            ["Email/set",{"created":{"send-1":{"id":"sent-copy"}}},"email"],
            ["EmailSubmission/set",{"created":{"1":{"id":"submission-id"}}},"submission"],
            ["EmailSubmission/get",{"list":[{"deliveryStatus":{"to@example.test":{"delivered":"yes"}}}]},"deliveryStatus"]
        ]});
        assert!(failed_composer_copy_targets(&accepted, &calls).is_empty());
        let rejected_all = json!({"methodResponses":[
            ["Email/set",{"created":{"send-1":{"id":"rejected-copy"}}},"email"],
            ["EmailSubmission/set",{"created":{"1":{"id":"submission-id"}}},"submission"],
            ["EmailSubmission/get",{"list":[{"deliveryStatus":{"to@example.test":{"delivered":"no"}}}]},"deliveryStatus"]
        ]});
        assert_eq!(
            failed_composer_copy_targets(&rejected_all, &calls),
            vec![("a".into(), vec!["rejected-copy".into()])]
        );
    }

    #[test]
    #[ignore = "Live read-only check of composer identity/capability access"]
    fn live_composer_identity_read() {
        tauri::async_runtime::block_on(async {
            let config = TestConfig::load().unwrap();
            let session: Value =
                serde_json::from_slice(&crate::session::session_bytes(&config).await.unwrap())
                    .unwrap();
            let account = session["primaryAccounts"][MAIL]
                .as_str()
                .expect("Kein primäres Mailkonto vorhanden.");
            assert!(
                account_supports(&session, account, SUBMISSION),
                "Submission-Capability fehlt für das primäre Konto."
            );
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open(&directory.path().join("composer-read.redb")).unwrap();
            let response = composer_request(
                &store,
                &config,
                &[(
                    "Identity/get".into(),
                    json!({"accountId":account}),
                    "identity-check".into(),
                )],
            )
            .await
            .unwrap_or_else(|error| panic!("{}", error.message()));
            let count = response["methodResponses"]
                .as_array()
                .and_then(|rows| rows.first())
                .and_then(|row| row[1]["list"].as_array())
                .map_or(0, Vec::len);
            assert!(
                count > 0,
                "Der Server hat keine Absenderidentität zurückgegeben."
            );
            let connection = Connection::discover(&config, account).await.unwrap();
            let mailboxes = connection
                .call(
                    "Mailbox/get",
                    json!({"accountId":account,"properties":["role","myRights"]}),
                )
                .await
                .unwrap();
            let drafts = mailboxes["list"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|mailbox| mailbox["role"] == "drafts");
            let may_remove =
                drafts.is_some_and(|mailbox| mailbox["myRights"]["mayRemoveItems"] == true);
            println!("Composer-Identitäten: {count}; Konto schreibgeschützt: {}; Entfernen aus Entwürfen erlaubt: {may_remove} (keine Adressen oder Inhalte protokolliert).", session["accounts"][account]["isReadOnly"] == true);
        });
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct FlagRecovery {
        configuration: String,
        account: String,
        email: String,
        original: bool,
    }

    async fn restore_test_flag(
        remote: &impl Remote,
        record: &FlagRecovery,
    ) -> Result<(), SyncError> {
        for _ in 0..4 {
            let current=remote.call("Email/get",json!({"accountId":record.account,"ids":[record.email],"properties":["id","keywords"]})).await?;
            let row = current["list"]
                .as_array()
                .and_then(|rows| rows.first())
                .ok_or(SyncError::Protocol)?;
            if (row["keywords"]["$flagged"] == true) == record.original {
                return Ok(());
            }
            let flag = if record.original {
                Value::Bool(true)
            } else {
                Value::Null
            };
            let result=remote.call("Email/set",json!({"accountId":record.account,"ifInState":text(&current,"state")?,"update":{&record.email:{"keywords/$flagged":flag}}})).await;
            match result {
                Ok(response) if response["updated"].get(&record.email).is_some() => {}
                Err(SyncError::Method(kind)) if kind == "stateMismatch" => continue,
                Err(error) => return Err(error),
                _ => return Err(SyncError::Protocol),
            }
        }
        Err(SyncError::Method("stateMismatch".into()))
    }

    #[test]
    #[ignore = "Explicitly authorized reversible flag write; requires JMAP_WRITE_TEST=flag-roundtrip"]
    fn live_flag_write_roundtrip() {
        assert!(
            std::env::var("JMAP_WRITE_TEST").as_deref() == Ok("flag-roundtrip"),
            "Schreibtest muss ausdrücklich freigegeben werden."
        );
        tauri::async_runtime::block_on(async {
            let mut config = TestConfig::load().unwrap();
            config.read_only = false;
            let recovery_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../.cache/live-flag-recovery.json");
            if recovery_path.exists() {
                let previous: FlagRecovery =
                    serde_json::from_slice(&std::fs::read(&recovery_path).unwrap()).unwrap();
                assert!(
                    previous.configuration == config.cache_key(),
                    "Wiederherstellung gehört zu einer anderen Konfiguration."
                );
                let connection = Connection::discover(&config, &previous.account)
                    .await
                    .unwrap_or_else(|error| panic!("{}", error.message()));
                restore_test_flag(&connection,&previous).await.expect("Vorheriger Flag-Test konnte nicht wiederhergestellt werden; Recovery-Datei bleibt erhalten.");
                std::fs::remove_file(&recovery_path).unwrap();
            }
            let session: Value =
                serde_json::from_slice(&crate::session::session_bytes(&config).await.unwrap())
                    .unwrap();
            let account = session["primaryAccounts"][MAIL]
                .as_str()
                .expect("Kein primäres Mailkonto vorhanden.");
            let connection = Connection::discover(&config, account)
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let query=connection.call("Email/query",json!({"accountId":account,"limit":1,"filter":{"notKeyword":"$draft"},"sort":[{"property":"receivedAt","isAscending":false}]})).await.unwrap_or_else(|error|panic!("{}",error.message()));
            let id = query["ids"]
                .as_array()
                .and_then(|ids| ids.first())
                .and_then(Value::as_str)
                .expect("Keine Nachricht für den Flag-Test vorhanden.");
            let before = connection
                .call(
                    "Email/get",
                    json!({"accountId":account,"ids":[id],"properties":["id","keywords"]}),
                )
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let email = before["list"]
                .as_array()
                .and_then(|rows| rows.first())
                .expect("Nachricht für Flag-Test nicht verfügbar.");
            let record = FlagRecovery {
                configuration: config.cache_key(),
                account: account.into(),
                email: id.into(),
                original: email["keywords"]["$flagged"] == true,
            };
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("write-test.redb")).unwrap();
            store
                .apply_remote(
                    "write-test",
                    Collection::Email,
                    None,
                    before["state"].as_str().unwrap(),
                    &[email.clone()],
                    &[],
                    true,
                )
                .unwrap();
            store
                .queue_keyword("write-test", id, "$flagged", !record.original)
                .unwrap();
            let operation = store.local_mail("write-test").unwrap().pending.remove(0);
            std::fs::create_dir_all(recovery_path.parent().unwrap()).unwrap();
            std::fs::write(&recovery_path, serde_json::to_vec(&record).unwrap()).unwrap();
            let write_result =
                push_keyword(&store, &connection, "write-test", account, &operation).await;
            let verification = connection
                .call(
                    "Email/get",
                    json!({"accountId":account,"ids":[id],"properties":["id","keywords"]}),
                )
                .await;
            // Restore even if the write timed out: Stalwart may already have accepted it.
            restore_test_flag(&connection,&record).await.expect("Originalmarkierung konnte nicht wiederhergestellt werden; Recovery-Datei bleibt erhalten.");
            std::fs::remove_file(&recovery_path).unwrap();
            assert!(write_result.is_ok(),"Flag-Schreibtest wurde vom Server nicht bestätigt; Originalzustand wurde wiederhergestellt.");
            let verified = verification.expect(
                "Änderung konnte nicht gelesen werden; Originalzustand wurde wiederhergestellt.",
            );
            assert!(
                (verified["list"][0]["keywords"]["$flagged"] == true) != record.original,
                "Markierungsänderung nicht sichtbar; Originalzustand wurde wiederhergestellt."
            );
            assert!(store.local_mail("write-test").unwrap().pending.is_empty());
            println!("Markierung über persistente Warteschlange geändert, serverseitig gelesen und Originalzustand wiederhergestellt.");
        });
    }

    #[test]
    fn shared_capabilities_fall_back_only_when_not_explicitly_restricted() {
        let session = json!({"capabilities":{MAIL:{},CALENDARS:{}},"accounts":{
            "shared":{"isPersonal":false,"accountCapabilities":{}},
            "calendarOnly":{"isPersonal":false,"accountCapabilities":{CALENDARS:{}}},
            "personal":{"isPersonal":true,"accountCapabilities":{}}
        }});
        assert!(account_supports(&session, "shared", MAIL));
        assert!(account_supports(&session, "shared", CALENDARS));
        assert!(!account_supports(&session, "calendarOnly", MAIL));
        assert!(!account_supports(&session, "personal", MAIL));
        assert!(!account_supports(&session, "missing", MAIL));
    }

    #[test]
    fn read_only_transport_allows_only_audited_read_methods() {
        assert!(authorize_method(false, "Email/set").is_ok());
        for method in [
            "CalendarEvent/set",
            "Mailbox/set",
            "EmailSubmission/set",
            "Unknown/get",
        ] {
            assert!(authorize_method(false, method).is_err());
        }
        for method in [
            "Mailbox/get",
            "Email/get",
            "Email/query",
            "Email/changes",
            "Calendar/get",
            "CalendarEvent/get",
            "CalendarEvent/query",
            "CalendarEvent/changes",
            "ParticipantIdentity/get",
        ] {
            assert!(authorize_method(true, method).is_ok());
        }
        for method in [
            "Email/set",
            "Mailbox/set",
            "EmailSubmission/set",
            "Email/import",
            "Blob/upload",
            "Unknown/get",
            "Calendar/set",
            "CalendarEvent/set",
            "ParticipantIdentity/set",
        ] {
            assert!(authorize_method(true, method).is_err());
        }
    }

    #[test]
    fn read_only_sync_does_not_flush_preexisting_pending_changes() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store.queue_keyword("scope", "m1", "$seen", true).unwrap();
            let remote = Script::new(vec![
                ("Mailbox/get", Ok(get("mb1", vec![]))),
                (
                    "Email/changes",
                    Ok(changes("s1", "s1", vec![], vec![], false)),
                ),
            ]);
            synchronize_with_policy(&store, &remote, "scope", "a", true)
                .await
                .unwrap();
            assert_eq!(store.local_mail("scope").unwrap().pending.len(), 1);
            remote.exhausted();
        });
    }

    struct Script(Mutex<VecDeque<(&'static str, Result<Value, SyncError>)>>);
    impl Script {
        fn new(rows: Vec<(&'static str, Result<Value, SyncError>)>) -> Self {
            Self(Mutex::new(rows.into()))
        }
        fn exhausted(&self) {
            assert!(self.0.lock().unwrap().is_empty());
        }
    }
    impl Remote for Script {
        async fn call(&self, method: &str, args: Value) -> Result<Value, SyncError> {
            let (expected, result) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected request");
            assert_eq!(method, expected);
            assert_eq!(args["accountId"], "a");
            if method == "Email/set" {
                assert!(args["ifInState"].is_string());
                if args.get("update").is_some() {
                    if args["update"]["m1"].get("mailboxIds").is_some() {
                        assert_eq!(args["update"]["m1"], json!({"mailboxIds":{"trash":true}}));
                    } else {
                        assert_eq!(args["update"]["m1"]["keywords/$seen"], true);
                    }
                }
            }
            result
        }
    }
    fn email(id: &str) -> Value {
        json!({"id":id,"keywords":{},"subject":"Cached"})
    }
    fn get(state: &str, rows: Vec<Value>) -> Value {
        json!({"state":state,"list":rows,"notFound":[]})
    }
    fn changes(
        old: &str,
        new: &str,
        created: Vec<&str>,
        destroyed: Vec<&str>,
        more: bool,
    ) -> Value {
        json!({"oldState":old,"newState":new,"created":created,"updated":[],"destroyed":destroyed,"hasMoreChanges":more})
    }
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("sync.redb")).unwrap();
        (dir, store)
    }

    #[test]
    fn queued_server_draft_deletion_reaches_jmap_and_acknowledges_redb() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            store
                .apply_remote(
                    "scope",
                    Collection::Email,
                    None,
                    "s1",
                    &[json!({"id":"draft-1","keywords":{"$draft":true}})],
                    &[],
                    true,
                )
                .unwrap();
            store.queue_draft_delete("scope", "draft-1").unwrap();
            let remote = Script::new(vec![("Email/set", Ok(json!({"destroyed":["draft-1"]})))]);
            flush_pending_deletes(&store, &remote, "scope", "a")
                .await
                .unwrap();
            assert!(store
                .local_mail("scope")
                .unwrap()
                .pending_deletes
                .is_empty());
            assert!(store.local_mail("scope").unwrap().emails.is_empty());
            remote.exhausted();
        });
    }

    #[test]
    fn blocked_server_draft_deletion_remains_hidden_and_visible_in_status() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            store
                .apply_remote(
                    "scope",
                    Collection::Email,
                    None,
                    "s1",
                    &[json!({"id":"draft-1","keywords":{"$draft":true}})],
                    &[],
                    true,
                )
                .unwrap();
            let operation = store.queue_draft_delete("scope", "draft-1").unwrap();
            store
                .block_delete("scope", &operation, "forbidden")
                .unwrap();
            let mail = store.local_mail("scope").unwrap();
            assert!(mail.emails.is_empty());
            assert_eq!(mail.pending_deletes.len(), 1);
            assert_eq!(
                mail.pending_deletes[0].blocked.as_deref(),
                Some("forbidden")
            );
        });
    }
    fn seed(store: &Store) {
        store
            .apply_remote(
                "scope",
                Collection::Email,
                None,
                "s1",
                &[email("m1")],
                &[],
                true,
            )
            .unwrap();
    }

    #[test]
    fn initial_snapshot_is_committed_only_after_final_state_check() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            let remote = Script::new(vec![
                ("Email/get", Ok(get("s1", vec![]))),
                (
                    "Email/query",
                    Ok(json!({"queryState":"q1","ids":["m1"],"total":1})),
                ),
                ("Email/get", Ok(get("s1", vec![email("m1")]))),
                ("Email/get", Ok(get("s1", vec![]))),
            ]);
            pull_emails(&store, &remote, "scope", "a").await.unwrap();
            assert_eq!(
                store.metadata("scope").unwrap().email_state.as_deref(),
                Some("s1")
            );
            assert_eq!(store.local_mail("scope").unwrap().emails.len(), 1);
            remote.exhausted();
        });
    }

    #[test]
    fn snapshot_race_does_not_publish_partial_data() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            let remote = Script::new(vec![
                ("Email/get", Ok(get("s1", vec![]))),
                (
                    "Email/query",
                    Ok(json!({"queryState":"q1","ids":["m1"],"total":1})),
                ),
                ("Email/get", Ok(get("s2", vec![email("m1")]))),
            ]);
            assert!(matches!(
                pull_emails(&store, &remote, "scope", "a").await,
                Err(SyncError::ChangedDuringSnapshot)
            ));
            assert!(!store.local_mail("scope").unwrap().initialized);
            assert!(store.local_mail("scope").unwrap().emails.is_empty());
            remote.exhausted();
        });
    }

    #[test]
    fn paginated_changes_remove_destroyed_objects_and_advance_each_page() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            let remote = Script::new(vec![
                (
                    "Email/changes",
                    Ok(changes("s1", "s2", vec!["m2"], vec!["m1"], true)),
                ),
                ("Email/get", Ok(get("s3", vec![email("m2")]))),
                (
                    "Email/changes",
                    Ok(changes("s2", "s3", vec![], vec![], false)),
                ),
            ]);
            pull_emails(&store, &remote, "scope", "a").await.unwrap();
            let local = store.local_mail("scope").unwrap();
            assert_eq!(local.emails.len(), 1);
            assert_eq!(local.emails[0]["id"], "m2");
            assert_eq!(
                store.metadata("scope").unwrap().email_state.as_deref(),
                Some("s3")
            );
            remote.exhausted();
        });
    }

    #[test]
    fn network_interruption_does_not_advance_checkpoint_or_remove_queue() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store.queue_keyword("scope", "m1", "$seen", true).unwrap();
            let remote = Script::new(vec![
                (
                    "Email/changes",
                    Ok(changes("s1", "s2", vec!["m2"], vec!["m1"], false)),
                ),
                ("Email/get", Err(SyncError::Network)),
            ]);
            assert!(pull_emails(&store, &remote, "scope", "a").await.is_err());
            assert_eq!(
                store.metadata("scope").unwrap().email_state.as_deref(),
                Some("s1")
            );
            let local = store.local_mail("scope").unwrap();
            assert_eq!(local.emails[0]["keywords"]["$seen"], true);
            assert_eq!(local.pending.len(), 1);
        });
    }

    #[test]
    fn state_mismatch_retains_intent_success_acknowledges_only_confirmed_edit() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store.queue_keyword("scope", "m1", "$seen", true).unwrap();
            let operation = store.local_mail("scope").unwrap().pending.remove(0);
            let rejected = Script::new(vec![(
                "Email/set",
                Err(SyncError::Method("stateMismatch".into())),
            )]);
            assert!(push_keyword(&store, &rejected, "scope", "a", &operation)
                .await
                .is_err());
            assert_eq!(store.local_mail("scope").unwrap().pending.len(), 1);
            let accepted = Script::new(vec![("Email/set", Ok(json!({"updated":{"m1":null}})))]);
            push_keyword(&store, &accepted, "scope", "a", &operation)
                .await
                .unwrap();
            let local = store.local_mail("scope").unwrap();
            assert!(local.pending.is_empty());
            assert_eq!(local.emails[0]["keywords"]["$seen"], true);
        });
    }

    #[test]
    fn expired_checkpoint_replaces_baseline_without_discarding_local_intent() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store.queue_keyword("scope", "m1", "$seen", true).unwrap();
            let remote = Script::new(vec![
                (
                    "Email/changes",
                    Err(SyncError::Method("cannotCalculateChanges".into())),
                ),
                ("Email/get", Ok(get("s2", vec![]))),
                (
                    "Email/query",
                    Ok(json!({"queryState":"q2","ids":["m1"],"total":1})),
                ),
                ("Email/get", Ok(get("s2", vec![email("m1")]))),
                ("Email/get", Ok(get("s2", vec![]))),
            ]);
            pull_emails(&store, &remote, "scope", "a").await.unwrap();
            let local = store.local_mail("scope").unwrap();
            assert_eq!(local.pending.len(), 1);
            assert_eq!(local.emails[0]["keywords"]["$seen"], true);
            assert_eq!(
                store.metadata("scope").unwrap().email_state.as_deref(),
                Some("s2")
            );
            remote.exhausted();
        });
    }

    #[test]
    fn permanent_rejection_is_visible_and_not_replayed_on_every_sync() {
        tauri::async_runtime::block_on(async {
            let (_dir, store) = store();
            seed(&store);
            store.queue_keyword("scope", "m1", "$seen", true).unwrap();
            let remote = Script::new(vec![
                ("Mailbox/get", Ok(get("mb1", vec![]))),
                (
                    "Email/changes",
                    Ok(changes("s1", "s1", vec![], vec![], false)),
                ),
                ("Email/set", Err(SyncError::Method("notFound".into()))),
                ("Mailbox/get", Ok(get("mb1", vec![]))),
                (
                    "Email/changes",
                    Ok(changes("s1", "s1", vec![], vec![], false)),
                ),
            ]);
            synchronize(&store, &remote, "scope", "a").await.unwrap();
            synchronize(&store, &remote, "scope", "a").await.unwrap();
            assert_eq!(
                store.local_mail("scope").unwrap().pending[0]
                    .blocked
                    .as_deref(),
                Some("notFound")
            );
            remote.exhausted();
        });
    }
}
