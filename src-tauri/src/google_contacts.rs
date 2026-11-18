//! Explicit, one-way Google contact import. UI reads persisted previews only.
use crate::{
    config::TestConfig, contacts, desktop, journal, storage::Store, sync, writes, SyncGate,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tauri::State;

const FIELDS: [&str; 3] = ["name", "emails", "phones"];
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    pub subject: String,
    pub email: String,
    pub fetched_at: u64,
    pub cards: BTreeMap<String, Value>,
}
pub(crate) fn source_key(config: &TestConfig) -> String {
    format!("google-contacts:{}", config.cache_key())
}
pub(crate) fn read_source(store: &Store, config: &TestConfig) -> Result<Source, String> {
    store
        .get(&source_key(config))?
        .map(|raw| serde_json::from_str(&raw).map_err(|_| "Google-Kontaktablage ungültig.".into()))
        .transpose()
        .map(|s| s.unwrap_or_default())
}
pub(crate) fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub(crate) fn uid(subject: &str, resource: &str) -> String {
    format!(
        "urn:herakles:google:{}",
        digest(&json!([subject, resource]).to_string())
    )
}

/// Deliberately bounded first-stage projection. No photos, groups or auto-created Other Contacts.
pub(crate) fn project(person: &Value) -> Result<Value, String> {
    let resource = person["resourceName"]
        .as_str()
        .filter(|s| s.starts_with("people/") && s.len() < 512)
        .ok_or("Ungültige Google-Kontaktkennung.")?;
    if resource.len() < 8 || person["metadata"]["deleted"] == true {
        return Err("Ungültiger Google-Kontakt.".into());
    }
    let names = person["names"].as_array();
    let name = names.and_then(|n| {
        n.iter()
            .find(|n| n["metadata"]["primary"] == true)
            .or(n.first())
    });
    let full = name
        .and_then(|n| n["displayName"].as_str())
        .unwrap_or("")
        .trim();
    let mut card = json!({"name":{"full":full},"emails":{},"phones":{}});
    for (google, field, ty, property) in [
        ("emailAddresses", "emails", "EmailAddress", "address"),
        ("phoneNumbers", "phones", "Phone", "number"),
    ] {
        let mut values = BTreeMap::new();
        for item in person[google].as_array().into_iter().flatten() {
            let Some(value) = item["value"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 4096)
            else {
                continue;
            };
            let mut entry = json!({"@type":ty, property:value});
            if let Some(context) = item["type"]
                .as_str()
                .filter(|s| ["home", "work"].contains(s))
            {
                entry["contexts"] = json!({context:true});
            }
            if item["metadata"]["primary"] == true {
                entry["pref"] = json!(1);
            }
            values.insert(format!("g{}", &digest(value)[..24]), entry);
        }
        card[field] = json!(values);
    }
    if full.len() > 8192 {
        return Err("Google-Kontaktname ist zu lang.".into());
    }
    Ok(card)
}
fn managed(card: &Value) -> Value {
    let mut out = json!({});
    for field in FIELDS {
        out[field] = card
            .get(field)
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| {
                if field == "name" {
                    json!({"full":""})
                } else {
                    json!({})
                }
            });
    }
    out
}
fn identities(card: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for email in card["emails"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
    {
        if let Some(s) = email["address"].as_str() {
            ids.insert(format!("e:{}", s.trim().to_lowercase()));
        }
    }
    for phone in card["phones"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
    {
        if let Some(s) = phone["number"].as_str() {
            let digits: String = s.chars().filter(char::is_ascii_digit).collect();
            if digits.len() >= 7 {
                ids.insert(format!("p:{digits}"));
            }
        }
    }
    if let Some(name) = card["name"]["full"]
        .as_str()
        .map(|s| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
    {
        ids.insert(format!("n:{name}"));
    }
    ids
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub resource: String,
    pub title: String,
    pub status: String,
    pub reason: String,
    pub current: Value,
    pub incoming: Value,
    pub patch: Value,
    pub target_id: Option<String>,
    pub uid: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub revision: String,
    pub rows: Vec<Row>,
    pub read_only: bool,
}
fn target(
    store: &Store,
    config: &TestConfig,
    account: &str,
    book: &str,
) -> Result<contacts::Snapshot, String> {
    desktop::require_capability(store, config, account, sync::CONTACTS)?;
    let mut snapshot = contacts::read_local(store, &sync::scope(config, account))?;
    if snapshot.access_denied || snapshot.contact_state.is_none() {
        return Err("Kontakte zuerst mit Stalwart abgleichen.".into());
    }
    if !snapshot
        .books
        .get(book)
        .is_some_and(|b| b["myRights"]["mayRead"] != false && b["myRights"]["mayWrite"] == true)
    {
        return Err("Adressbuch ist nicht beschreibbar.".into());
    }
    snapshot.contacts.retain(|_, card| {
        card["addressBookIds"].as_object().is_some_and(|ids| {
            ids.iter().any(|(id, on)| {
                on == true
                    && snapshot
                        .books
                        .get(id)
                        .is_some_and(|b| b["myRights"]["mayRead"] != false)
            })
        })
    });
    Ok(snapshot)
}
pub(crate) fn validate_entry(
    store: &Store,
    config: &TestConfig,
    account: &str,
    entry: &journal::Entry,
) -> Result<(), String> {
    writes::writable(store, config, account, sync::CONTACTS)?;
    let snapshot = contacts::read(store, &sync::scope(config, account))?;
    if snapshot.access_denied {
        return Err("forbidden".into());
    }
    let local = entry.local.as_ref().ok_or("Ungültiger Kontaktauftrag.")?;
    let books = local["addressBookIds"]
        .as_object()
        .ok_or("Adressbuch fehlt.")?;
    if books.is_empty()
        || books.iter().any(|(id, on)| {
            on != true
                || !snapshot.books.get(id).is_some_and(|b| {
                    b["myRights"]["mayWrite"] == true && b["myRights"]["mayRead"] != false
                })
        })
    {
        return Err("forbidden".into());
    }
    if entry
        .calls
        .first()
        .is_some_and(|call| call.1["update"].is_object())
    {
        let current = snapshot.contacts.get(&entry.object_id).ok_or("notFound")?;
        if current["uid"] != local["uid"] {
            return Err("Kontaktkennung wurde geändert.".into());
        }
        let current_books = current["addressBookIds"]
            .as_object()
            .ok_or("Adressbuch fehlt.")?;
        if current_books.is_empty()
            || current_books.iter().any(|(id, on)| {
                on != true
                    || !snapshot.books.get(id).is_some_and(|b| {
                        b["myRights"]["mayWrite"] == true && b["myRights"]["mayRead"] != false
                    })
            })
        {
            return Err("forbidden".into());
        }
    }
    if entry.calls.len() != 1
        || entry.calls[0].0 != "ContactCard/set"
        || entry.calls[0].1["accountId"] != account
        || entry.calls[0].1.get("destroy").is_some()
    {
        return Err("Ungültiger Kontaktauftrag.".into());
    }
    Ok(())
}
pub(crate) fn preview(
    store: &Store,
    config: &TestConfig,
    account: &str,
    book: &str,
) -> Result<Preview, String> {
    let snapshot = target(store, config, account, book)?;
    let source = read_source(store, config)?;
    if source.subject.is_empty() {
        return Err("Google-Kontakte zuerst abrufen.".into());
    }
    let scope = sync::scope(config, account);
    let receipts = journal::import_receipts(store, &scope)?;
    let pending = journal::entries(store, &scope)?;
    let mut identities_on_server = BTreeSet::new();
    for card in snapshot.contacts.values() {
        identities_on_server.extend(identities(card));
    }
    let mut identities_in_source = BTreeMap::<String, usize>::new();
    for card in source.cards.values() {
        for identity in identities(card) {
            *identities_in_source.entry(identity).or_default() += 1;
        }
    }
    let mut uid_index = BTreeMap::<String, Vec<&Value>>::new();
    for card in snapshot.contacts.values() {
        if let Some(uid) = card["uid"].as_str() {
            uid_index.entry(uid.into()).or_default().push(card);
        }
    }
    let mut rows = Vec::new();
    for (resource, fields) in &source.cards {
        let uid = uid(&source.subject, resource);
        let matches = uid_index.get(&uid).cloned().unwrap_or_default();
        let mut row = Row {
            resource: resource.clone(),
            title: fields["name"]["full"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("Ohne Namen")
                .into(),
            status: "new".into(),
            reason: "Neuen Kontakt anlegen".into(),
            current: Value::Null,
            incoming: fields.clone(),
            patch: json!({}),
            target_id: None,
            uid: uid.clone(),
        };
        if identities(fields).is_empty() {
            row.status = "unsupported".into();
            row.reason =
                "Kontakt enthält keinen Namen, keine E-Mail-Adresse und keine Telefonnummer."
                    .into();
        } else if matches.len() > 1 {
            row.status = "conflict".into();
            row.reason = "Mehrere Kontakte mit gleicher Importkennung.".into();
        } else if let Some(current) = matches.first() {
            row.current = managed(current);
            row.target_id = current["id"].as_str().map(str::to_owned);
            let id = row.target_id.as_deref().unwrap_or("");
            if let Some(entry) = pending
                .iter()
                .find(|e| e.kind == "contact" && e.object_id == id)
            {
                let source_matches = receipts
                    .get(&uid)
                    .is_some_and(|base| canonical(base) == canonical(fields));
                if entry.status == "applied" {
                    row.status = "transferred".into();
                    row.reason =
                        "Auf dem Server bestätigt; die lokale Ansicht wird abgeglichen.".into();
                } else if ["ready", "sending"].contains(&entry.status.as_str()) {
                    row.status = "sending".into();
                    row.reason = "Die Übertragung wurde bereits freigegeben.".into();
                } else if entry.status == "held"
                    && current["addressBookIds"][book] == true
                    && source_matches
                {
                    row.status = "pending".into();
                    row.reason = "Kontakt ist lokal vorbereitet und kann übertragen werden.".into();
                } else {
                    row.status = "review".into();
                    row.reason = if !source_matches { "Google-Daten haben sich seit dem Vormerken geändert. Vorhandenen Auftrag zuerst prüfen." } else { "Die frühere Übertragung muss zuerst geprüft werden; sie wird nicht automatisch wiederholt." }.into();
                }
            } else if current["addressBookIds"][book] != true {
                row.status = "conflict".into();
                row.reason =
                    "Kontakt liegt in einem anderen Adressbuch; keine automatische Verschiebung."
                        .into();
            } else if let Some(base) = receipts.get(&uid) {
                let mut conflict = false;
                for field in FIELDS {
                    if canonical(&fields[field]) != canonical(&base[field])
                        && canonical(&row.current[field]) != canonical(&fields[field])
                    {
                        if canonical(&row.current[field]) != canonical(&base[field]) {
                            conflict = true;
                        } else {
                            row.patch[field] = fields[field].clone();
                        }
                    }
                }
                if conflict {
                    row.status = "conflict".into();
                    row.reason =
                        "Google und Stalwart haben dasselbe Feld unterschiedlich geändert.".into();
                } else if row.patch.as_object().is_some_and(|m| !m.is_empty()) {
                    row.status = "update".into();
                    row.reason =
                        "Geänderte Google-Felder übernehmen; andere Felder bleiben erhalten."
                            .into();
                } else {
                    row.status = "unchanged".into();
                    row.reason = "Keine zu übernehmenden Änderungen.".into();
                }
            } else {
                row.status = "conflict".into();
                row.reason = "Importbasis fehlt; Kontakt manuell prüfen.".into();
            }
        } else if receipts.contains_key(&uid) {
            row.status = "conflict".into();
            row.reason = "Früher importierter Kontakt fehlt. Keine automatische Neuanlage.".into();
        } else {
            let ids = identities(fields);
            let duplicate = !identities_on_server.is_disjoint(&ids);
            let source_duplicate = ids
                .iter()
                .any(|id| identities_in_source.get(id).copied().unwrap_or(0) > 1);
            if duplicate || source_duplicate {
                row.status = "duplicate".into();
                row.reason =
                    "Mögliche Dublette anhand Name, E-Mail oder Telefonnummer. Manuell prüfen."
                        .into();
            }
        }
        rows.push(row);
    }
    let revision = digest(
        &json!([
            config.cache_key(),
            account,
            book,
            source.subject,
            source.cards,
            snapshot.contact_state,
            snapshot.book_state,
            snapshot.contacts,
            snapshot.books,
            journal::revision(store, &scope)?
        ])
        .to_string(),
    );
    Ok(Preview {
        revision,
        rows,
        read_only: config.read_only,
    })
}
#[tauri::command]
pub async fn google_import_status(store: State<'_, Store>) -> Result<Value, String> {
    let config = TestConfig::load()?;
    let source = read_source(&store, &config)?;
    let session: Value = store
        .get(&format!("{}:desktop-session", config.cache_key()))?
        .map(|s| serde_json::from_str(&s).map_err(|_| "Lokale Sitzung ungültig."))
        .transpose()?
        .unwrap_or(json!({}));
    let mut books = Vec::new();
    for (account, value) in session["accounts"].as_object().into_iter().flatten() {
        if desktop::require_capability(&store, &config, account, sync::CONTACTS).is_err() {
            continue;
        }
        let snapshot = contacts::read(&store, &sync::scope(&config, account))?;
        if snapshot.access_denied || value["isReadOnly"] == true {
            continue;
        }
        for (id, book) in snapshot.books {
            if book["myRights"]["mayWrite"] == true && book["myRights"]["mayRead"] != false {
                books.push(json!({"accountId":account,"id":id,"name":book["name"],"accountName":value["name"]}));
            }
        }
    }
    Ok(
        json!({"email":source.email,"fetchedAt":source.fetched_at,"count":source.cards.len(),"connected":crate::google_auth::connected(&config)?,"oauthConfigured":crate::google_auth::oauth_configured(),"books":books,"readOnly":config.read_only}),
    )
}
#[tauri::command]
pub async fn google_import_preview(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    account_id: String,
    book_id: String,
) -> Result<Preview, String> {
    let _guard = gate.0.lock().await;
    preview(&store, &TestConfig::load()?, &account_id, &book_id)
}
#[tauri::command]
pub async fn google_import_apply(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    account_id: String,
    book_id: String,
    revision: String,
    resources: Vec<String>,
) -> Result<Value, String> {
    let _guard = gate.0.lock().await;
    let config = TestConfig::load()?;
    let count = apply(
        &store,
        &config,
        &account_id,
        &book_id,
        &revision,
        &resources,
    )?;
    let source = read_source(&store, &config)?;
    let uids: BTreeSet<_> = resources
        .iter()
        .map(|resource| uid(&source.subject, resource))
        .collect();
    let snapshot = contacts::read_local(&store, &sync::scope(&config, &account_id))?;
    let contacts: Vec<_> = snapshot
        .contacts
        .values()
        .filter(|card| card["uid"].as_str().is_some_and(|id| uids.contains(id)))
        .cloned()
        .collect();
    Ok(json!({"count":count,"contacts":contacts}))
}
pub(crate) fn apply(
    store: &Store,
    config: &TestConfig,
    account: &str,
    book: &str,
    revision: &str,
    resources: &[String],
) -> Result<usize, String> {
    writes::writable(store, config, account, sync::CONTACTS)?;
    let preview = preview(store, config, account, book)?;
    if preview.revision != revision {
        return Err("Vorschau ist veraltet. Bitte erneut prüfen.".into());
    }
    let selected: BTreeSet<_> = resources.iter().collect();
    if selected.is_empty() || selected.len() != resources.len() || selected.len() > 500 {
        return Err("Bitte 1 bis 500 unterschiedliche Kontakte auswählen.".into());
    }
    let rows: Vec<_> = preview
        .rows
        .into_iter()
        .filter(|r| selected.contains(&r.resource))
        .collect();
    if rows.len() != selected.len()
        || rows
            .iter()
            .any(|r| !["new", "update"].contains(&r.status.as_str()))
    {
        return Err("Auswahl enthält nicht übernehmbare Kontakte.".into());
    }
    // Each contact + import baseline is one atomic redb transaction. A partial batch remains visible.
    let scope = sync::scope(config, account);
    for row in &rows {
        let snapshot = target(store, config, account, book)?;
        let args = if row.status == "new" {
            let mut card = row.incoming.clone();
            card["uid"] = json!(row.uid);
            card["addressBookIds"] = json!({book:true});
            json!({"accountId":account,"create":{"google":card}})
        } else {
            json!({"accountId":account,"update":{row.target_id.as_deref().ok_or("Kontaktkennung fehlt.")?:row.patch}})
        };
        journal::queue_import_object(
            store,
            &scope,
            "contact",
            "ContactCard/set",
            args,
            &snapshot.contacts,
            snapshot.contact_state,
            true,
            Some((row.uid.clone(), row.incoming.clone())),
        )?;
    }
    Ok(rows.len())
}

/// The button itself is the explicit approval for precisely this Google/contact selection.
pub(crate) fn prepare_transfer(
    store: &Store,
    config: &TestConfig,
    account: &str,
    book: &str,
    revision: &str,
    resources: &[String],
) -> Result<Vec<u64>, String> {
    writes::writable(store, config, account, sync::CONTACTS)?;
    let plan = preview(store, config, account, book)?;
    if plan.revision != revision {
        return Err("Kontaktliste wurde geändert. Bitte erneut laden.".into());
    }
    let selection: BTreeSet<_> = resources.iter().collect();
    if selection.is_empty() || selection.len() != resources.len() || selection.len() > 500 {
        return Err("Bitte 1 bis 500 Kontakte auswählen.".into());
    }
    let rows: Vec<_> = plan
        .rows
        .iter()
        .filter(|r| selection.contains(&r.resource))
        .collect();
    if rows.len() != selection.len()
        || rows
            .iter()
            .any(|r| !["new", "update", "pending"].contains(&r.status.as_str()))
    {
        return Err("Auswahl enthält Kontakte, die zuerst geprüft werden müssen.".into());
    }
    let fresh: Vec<_> = rows
        .iter()
        .filter(|r| r.status != "pending")
        .map(|r| r.resource.clone())
        .collect();
    if !fresh.is_empty() {
        apply(store, config, account, book, revision, &fresh)?;
    }
    let scope = sync::scope(config, account);
    let entries = journal::entries(store, &scope)?;
    let mut ids = Vec::new();
    for row in rows {
        let matches: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.kind == "contact"
                    && e.status == "held"
                    && e.local
                        .as_ref()
                        .is_some_and(|c| c["uid"] == row.uid && c["addressBookIds"][book] == true)
            })
            .collect();
        if matches.len() != 1 {
            return Err("Kontaktauftrag wurde inzwischen geändert. Bitte erneut laden.".into());
        }
        validate_entry(store, config, account, matches[0])?;
        ids.push(matches[0].id);
    }
    Ok(ids)
}
#[tauri::command]
pub async fn google_import_transfer(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, crate::NetworkMode>,
    account_id: String,
    book_id: String,
    revision: String,
    resources: Vec<String>,
) -> Result<Value, String> {
    let _guard = gate.0.lock().await;
    let config = TestConfig::load()?;
    let ids = prepare_transfer(
        &store,
        &config,
        &account_id,
        &book_id,
        &revision,
        &resources,
    )?;
    let scope = sync::scope(&config, &account_id);
    let mut warning = None;
    let mut readback_pending = false;
    if network.enabled() {
        journal::approve(&store, &scope, &ids)?;
        if let Err(error) =
            writes::sync_contact_selection(&store, &config, &network, &account_id, &ids).await
        {
            warning = Some(error);
        }
        if network.enabled() {
            let mut reader = TestConfig::load()?;
            reader.read_only = true;
            let result = async {
                let remote =
                    sync::Connection::discover_for(&reader, &account_id, sync::CONTACTS).await?;
                contacts::synchronize(&store, &remote, &scope, &account_id).await
            }
            .await;
            if result.is_err() {
                readback_pending = true;
            }
        } else {
            readback_pending = true;
        }
    }
    let entries = journal::entries(&store, &scope)?;
    let remaining: Vec<_> = entries.iter().filter(|e| ids.contains(&e.id)).collect();
    let transferred =
        ids.len() - remaining.len() + remaining.iter().filter(|e| e.status == "applied").count();
    let review = remaining
        .iter()
        .filter(|e| ["uncertain", "blocked"].contains(&e.status.as_str()))
        .count();
    let waiting = remaining
        .iter()
        .filter(|e| ["held", "ready", "sending"].contains(&e.status.as_str()))
        .count();
    let source = read_source(&store, &config)?;
    let uids: BTreeSet<_> = resources
        .iter()
        .map(|resource| uid(&source.subject, resource))
        .collect();
    let snapshot = contacts::read_local(&store, &scope)?;
    let contacts: Vec<_> = snapshot
        .contacts
        .values()
        .filter(|v| v["uid"].as_str().is_some_and(|id| uids.contains(id)))
        .cloned()
        .collect();
    Ok(
        json!({"transferred":transferred,"waiting":waiting,"review":review,"readbackPending":readback_pending,"warning":warning,"contacts":contacts}),
    )
}

/// Do not hide an accepted write until the read-only sync has seen its exact UID
/// and all managed fields. Ignore wire type annotations added by the server.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "@type")
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        value => value.clone(),
    }
}
pub(crate) fn clear_confirmed(
    store: &Store,
    scope: &str,
    objects: &BTreeMap<String, Value>,
) -> Result<(), String> {
    for entry in journal::entries(store, scope)?
        .into_iter()
        .filter(|e| e.kind == "contact" && e.status == "applied")
    {
        let Some(local) = entry.local.as_ref() else {
            continue;
        };
        let matches: Vec<_> = objects
            .values()
            .filter(|v| v["uid"] == local["uid"])
            .collect();
        if matches.len() == 1
            && FIELDS.iter().all(|field| {
                if *field == "name" {
                    matches[0]["name"]["full"] == local["name"]["full"]
                } else {
                    canonical(&managed(matches[0])[field]) == canonical(&managed(local)[field])
                }
            })
        {
            journal::finish(store, scope, entry.id, None, None)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_config() -> TestConfig {
        TestConfig {
            url: url::Url::parse("https://example.test/jmap").unwrap(),
            username: "test".into(),
            password: "synthetic".into(),
            read_only: false,
        }
    }
    fn seed(store: &Store, config: &TestConfig) {
        store.put(&format!("{}:desktop-session",config.cache_key()),&json!({"accounts":{"a":{"accountCapabilities":{sync::CONTACTS:{}}},"b":{"accountCapabilities":{}}}}).to_string()).unwrap();
        store.put(&format!("contacts:{}",sync::scope(config,"a")),&json!({"books":{"book":{"id":"book","name":"Contacts","myRights":{"mayRead":true,"mayWrite":true}}},"contacts":{},"bookState":"b1","contactState":"c1","accessDenied":false}).to_string()).unwrap();
        let cards=BTreeMap::from([("people/1".into(),project(&json!({"resourceName":"people/1","names":[{"displayName":"Alice"}],"emailAddresses":[{"value":"alice@example.test"}]})).unwrap())]);
        store
            .put(
                &source_key(config),
                &serde_json::to_string(&Source {
                    subject: "google-user".into(),
                    email: "reader@example.test".into(),
                    fetched_at: 1,
                    cards,
                })
                .unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn direct_transfer_only_approves_and_sends_selected_contacts_and_holds_offline() {
        use std::sync::Mutex;
        struct ContactRemote(Mutex<usize>);
        impl sync::Remote for ContactRemote {
            async fn call(&self, method: &str, args: Value) -> Result<Value, sync::SyncError> {
                assert_eq!(method, "ContactCard/set");
                assert_eq!(args["accountId"], "a");
                assert_eq!(args["ifInState"], "c1");
                assert!(args["create"]["google"]["uid"]
                    .as_str()
                    .unwrap()
                    .starts_with("urn:herakles:google:"));
                *self.0.lock().unwrap() += 1;
                Ok(
                    json!({"accountId":"a","created":{"google":{"id":"server-contact"}},"newState":"c2"}),
                )
            }
        }
        tauri::async_runtime::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("data.redb")).unwrap();
            let config = test_config();
            seed(&store, &config);
            let scope = sync::scope(&config, "a");
            journal::queue_object(
                &store,
                &scope,
                "file",
                "FileNode/set",
                json!({"accountId":"a","create":{"unrelated":{"name":"Leave file ready"}}}),
                &BTreeMap::new(),
                None,
                false,
            )
            .unwrap();
            journal::queue_object(&store,&scope,"contact","ContactCard/set",json!({"accountId":"a","create":{"unrelated":{"uid":"urn:unrelated","name":{"full":"Unselected"},"addressBookIds":{"book":true}}}}),&BTreeMap::new(),Some("c1".into()),true).unwrap();
            journal::enqueue_send(&store,&scope,vec![("Email/set".into(),json!({"accountId":"a","create":{"mail":{"subject":"Unrelated synthetic mail"}}}),"create".into()),("EmailSubmission/set".into(),json!({"accountId":"a","create":{"send":{"emailId":"#mail"}}}),"submit".into())],false,None,None).unwrap();
            let before = journal::entries(&store, &scope).unwrap();
            let p = preview(&store, &config, "a", "book").unwrap();
            let ids = prepare_transfer(
                &store,
                &config,
                "a",
                "book",
                &p.revision,
                &["people/1".into()],
            )
            .unwrap();
            assert_eq!(ids.len(), 1);
            assert!(!before.iter().any(|e| ids.contains(&e.id)));
            let p = preview(&store, &config, "a", "book").unwrap();
            assert_eq!(p.rows[0].status, "pending");
            assert_eq!(
                prepare_transfer(
                    &store,
                    &config,
                    "a",
                    "book",
                    &p.revision,
                    &["people/1".into()]
                )
                .unwrap(),
                ids
            );
            let (network, _events) = crate::NetworkMode::new();
            let remote = ContactRemote(Mutex::new(0));
            journal::approve(&store, &scope, &ids).unwrap();
            assert_eq!(
                writes::transmit_contact_selection(&store, &config, &network, "a", &ids, &remote)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(
                journal::entries(&store, &scope)
                    .unwrap()
                    .iter()
                    .find(|e| e.id == ids[0])
                    .unwrap()
                    .status,
                "held"
            );
            network.set_enabled(true);
            journal::approve(&store, &scope, &ids).unwrap();
            assert_eq!(
                writes::transmit_contact_selection(&store, &config, &network, "a", &ids, &remote)
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                writes::transmit_contact_selection(&store, &config, &network, "a", &ids, &remote)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(*remote.0.lock().unwrap(), 1);
            let after = journal::entries(&store, &scope).unwrap();
            for entry in before {
                let preserved = after.iter().find(|e| e.id == entry.id).unwrap();
                assert_eq!(preserved.kind, entry.kind);
                assert_eq!(preserved.status, entry.status);
                assert_eq!(preserved.calls, entry.calls);
            }
            let plan = preview(&store, &config, "a", "book").unwrap();
            assert_eq!(plan.rows[0].status, "transferred");
            assert!(prepare_transfer(
                &store,
                &config,
                "a",
                "book",
                &plan.revision,
                &["people/1".into()]
            )
            .is_err());
        });
    }
    #[test]
    fn discarded_known_unsent_import_can_be_selected_again_but_uncertain_cannot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data.redb")).unwrap();
        let config = test_config();
        seed(&store, &config);
        let scope = sync::scope(&config, "a");
        let p = preview(&store, &config, "a", "book").unwrap();
        apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()],
        )
        .unwrap();
        let e = journal::entries(&store, &scope).unwrap().remove(0);
        journal::discard(&store, &scope, e.id, false).unwrap();
        assert!(journal::import_receipts(&store, &scope).unwrap().is_empty());
        let p = preview(&store, &config, "a", "book").unwrap();
        assert_eq!(p.rows[0].status, "new");
        apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()],
        )
        .unwrap();
        let e = journal::entries(&store, &scope).unwrap().remove(0);
        journal::finish(&store, &scope, e.id, Some("uncertain"), Some("Unknown")).unwrap();
        assert!(journal::discard(&store, &scope, e.id, false).is_err());
        journal::discard(&store, &scope, e.id, true).unwrap();
        assert_eq!(
            preview(&store, &config, "a", "book").unwrap().rows[0].status,
            "conflict"
        );
    }
    #[test]
    fn import_is_held_atomic_survives_restart_and_never_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.redb");
        let config = test_config();
        let scope = sync::scope(&config, "a");
        let store = Store::open(&path).unwrap();
        seed(&store, &config);
        let p = preview(&store, &config, "a", "book").unwrap();
        assert_eq!(p.rows[0].status, "new");
        assert!(apply(&store, &config, "a", "book", "stale", &["people/1".into()]).is_err());
        let mut ro = test_config();
        ro.read_only = true;
        assert!(apply(&store, &ro, "a", "book", &p.revision, &["people/1".into()]).is_err());
        assert!(preview(&store, &config, "b", "book").is_err());
        apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()],
        )
        .unwrap();
        let queued = journal::entries(&store, &scope).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].status, "held");
        assert_eq!(queued[0].kind, "contact");
        assert!(journal::entries(&store, &sync::scope(&config, "b"))
            .unwrap()
            .is_empty());
        drop(store);
        let store = Store::open(&path).unwrap();
        let p = preview(&store, &config, "a", "book").unwrap();
        assert_eq!(p.rows[0].status, "pending");
        assert_eq!(journal::import_receipts(&store, &scope).unwrap().len(), 1);
        assert!(apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()]
        )
        .is_err());
        let queued = journal::entries(&store, &scope).unwrap().remove(0);
        journal::approve(&store, &scope, &[queued.id]).unwrap();
        journal::claim(&store, &scope, queued.id).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            journal::entries(&store, &scope).unwrap()[0].status,
            "uncertain"
        );
        assert_eq!(
            preview(&store, &config, "a", "book").unwrap().rows[0].status,
            "review"
        );
    }
    #[test]
    fn three_way_updates_preserve_unrelated_fields_and_detect_conflicts_and_removals() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data.redb")).unwrap();
        let config = test_config();
        seed(&store, &config);
        let scope = sync::scope(&config, "a");
        let p = preview(&store, &config, "a", "book").unwrap();
        apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()],
        )
        .unwrap();
        let e = journal::entries(&store, &scope).unwrap().remove(0);
        let mut contact = e.local.clone().unwrap();
        contact["id"] = json!("server-id");
        contact["notes"] = json!({"local":{"note":"Keep me"}});
        let mut snapshot = contacts::read(&store, &scope).unwrap();
        snapshot
            .contacts
            .insert("server-id".into(), contact.clone());
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        journal::applied(&store, &scope, e.id, Some("c2")).unwrap();
        clear_confirmed(&store, &scope, &BTreeMap::new()).unwrap();
        assert_eq!(journal::entries(&store, &scope).unwrap().len(), 1);
        clear_confirmed(&store, &scope, &snapshot.contacts).unwrap();
        assert!(journal::entries(&store, &scope).unwrap().is_empty());
        let mut source = read_source(&store, &config).unwrap();
        source.cards.get_mut("people/1").unwrap()["name"]["full"] = json!("Alice New");
        store
            .put(
                &source_key(&config),
                &serde_json::to_string(&source).unwrap(),
            )
            .unwrap();
        let p = preview(&store, &config, "a", "book").unwrap();
        assert_eq!(p.rows[0].status, "update");
        assert!(p.rows[0].patch.get("notes").is_none());
        assert!(p.rows[0].patch.get("emails").is_none());
        snapshot.contacts.get_mut("server-id").unwrap()["name"]["full"] = json!("Local edit");
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        assert_eq!(
            preview(&store, &config, "a", "book").unwrap().rows[0].status,
            "conflict"
        );
        snapshot.contacts.clear();
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        assert_eq!(
            preview(&store, &config, "a", "book").unwrap().rows[0].status,
            "conflict"
        );
        source.cards.clear();
        store
            .put(
                &source_key(&config),
                &serde_json::to_string(&source).unwrap(),
            )
            .unwrap();
        assert!(preview(&store, &config, "a", "book")
            .unwrap()
            .rows
            .is_empty());
        assert!(journal::entries(&store, &scope).unwrap().is_empty());
    }
    #[test]
    fn duplicate_detection_and_rights_are_rechecked_before_queue_or_transport() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data.redb")).unwrap();
        let config = test_config();
        seed(&store, &config);
        let scope = sync::scope(&config, "a");
        let mut snapshot = contacts::read(&store, &scope).unwrap();
        snapshot.contacts.insert("existing".into(),json!({"id":"existing","name":{"full":"Someone else"},"emails":{"one":{"address":"ALICE@example.test"}},"addressBookIds":{"book":true}}));
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        let p = preview(&store, &config, "a", "book").unwrap();
        assert_eq!(p.rows[0].status, "duplicate");
        assert!(apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()]
        )
        .is_err());
        snapshot.contacts.clear();
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        let p = preview(&store, &config, "a", "book").unwrap();
        apply(
            &store,
            &config,
            "a",
            "book",
            &p.revision,
            &["people/1".into()],
        )
        .unwrap();
        let e = journal::entries(&store, &scope).unwrap().remove(0);
        assert!(validate_entry(&store, &config, "a", &e).is_ok());
        snapshot.books.get_mut("book").unwrap()["myRights"]["mayWrite"] = json!(false);
        store
            .put(
                &format!("contacts:{scope}"),
                &serde_json::to_string(&snapshot).unwrap(),
            )
            .unwrap();
        assert!(validate_entry(&store, &config, "a", &e).is_err());
        assert!(preview(&store, &config, "a", "book").is_err());
        assert_ne!(uid("first", "people/1"), uid("second", "people/1"));
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::sync::Mutex;
    #[test]
    fn contact_worker_requires_exact_ack_and_never_replays_uncertain_creates() {
        struct Reply {
            answer: Mutex<Option<Result<Value, sync::SyncError>>>,
            calls: Mutex<usize>,
        }
        impl sync::Remote for Reply {
            async fn call(&self, method: &str, args: Value) -> Result<Value, sync::SyncError> {
                assert_eq!(method, "ContactCard/set");
                assert!(args.get("destroy").is_none());
                *self.calls.lock().unwrap() += 1;
                self.answer
                    .lock()
                    .unwrap()
                    .take()
                    .expect("unexpected retry")
            }
        }
        tauri::async_runtime::block_on(async {
            for (answer, status) in [
                (
                    Ok(json!({"accountId":"a","created":{"google":{"id":"real"}},"newState":"c2"})),
                    "applied",
                ),
                (
                    Ok(json!({"created":{"other":{"id":"real"}},"newState":"c2"})),
                    "uncertain",
                ),
                (Ok(json!({"created":{"google":{"id":"real"}}})), "uncertain"),
                (
                    Ok(json!({"notCreated":{"google":{"type":"forbidden"}}})),
                    "blocked",
                ),
                (Err(sync::SyncError::Network), "uncertain"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let store = Store::open(&dir.path().join("data.redb")).unwrap();
                let fields = json!({"uid":"urn:synthetic:1","name":{"full":"Test"},"emails":{},"phones":{},"addressBookIds":{"book":true}});
                journal::queue_object(
                    &store,
                    "scope",
                    "contact",
                    "ContactCard/set",
                    json!({"accountId":"a","create":{"google":fields}}),
                    &BTreeMap::new(),
                    Some("c1".into()),
                    true,
                )
                .unwrap();
                let e = journal::entries(&store, "scope").unwrap().remove(0);
                journal::approve(&store, "scope", &[e.id]).unwrap();
                let remote = Reply {
                    answer: Mutex::new(Some(answer)),
                    calls: Mutex::new(0),
                };
                writes::transmit_object_change(
                    &store,
                    &remote,
                    "scope",
                    &e,
                    "ContactCard/set",
                    e.calls[0].1.clone(),
                )
                .await
                .unwrap();
                assert_eq!(journal::entries(&store, "scope").unwrap()[0].status, status);
                assert!(!writes::transmit_object_change(
                    &store,
                    &remote,
                    "scope",
                    &e,
                    "ContactCard/set",
                    e.calls[0].1.clone()
                )
                .await
                .unwrap());
                assert_eq!(*remote.calls.lock().unwrap(), 1);
            }
        });
    }
}
