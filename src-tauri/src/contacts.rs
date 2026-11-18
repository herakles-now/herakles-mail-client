//! Read-only, account-scoped contact snapshots. Only native sync fetches data.
use crate::{
    config::TestConfig,
    desktop,
    journal::Calls,
    storage::Store,
    sync::{self, Remote, SyncError},
    NetworkMode, SyncGate,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tauri::State;

const LIMIT: usize = 250;
const MAX_OBJECTS: usize = 100_000;
const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub books: BTreeMap<String, Value>,
    pub contacts: BTreeMap<String, Value>,
    pub book_state: Option<String>,
    pub contact_state: Option<String>,
    #[serde(default)]
    pub access_denied: bool,
}
pub fn read(store: &Store, scope: &str) -> Result<Snapshot, String> {
    store
        .get(&format!("contacts:{scope}"))?
        .map(|s| {
            serde_json::from_str(&s).map_err(|_| "Lokaler Kontaktbestand ist ungültig.".into())
        })
        .transpose()
        .map(|s| s.unwrap_or_default())
}
pub(crate) fn read_local(store: &Store, scope: &str) -> Result<Snapshot, String> {
    read(store, scope)
}
fn text(value: &Value, name: &str) -> Result<String, SyncError> {
    value[name]
        .as_str()
        .map(str::to_owned)
        .ok_or(SyncError::Protocol)
}
fn strings(value: &Value, name: &str) -> Result<Vec<String>, SyncError> {
    value[name]
        .as_array()
        .ok_or(SyncError::Protocol)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or(SyncError::Protocol))
        .collect()
}
async fn get(
    remote: &impl Remote,
    account: &str,
    kind: &str,
    ids: &[String],
) -> Result<Value, SyncError> {
    let mut args = json!({"accountId":account,"ids":ids});
    if kind == "AddressBook" {
        args["properties"] = json!([
            "id",
            "name",
            "description",
            "sortOrder",
            "isDefault",
            "isSubscribed",
            "myRights"
        ]);
    }
    let value = remote.call(&format!("{kind}/get"), args).await?;
    if value["accountId"] != account || !value["list"].is_array() || !value["notFound"].is_array() {
        return Err(SyncError::Protocol);
    }
    Ok(value)
}
async fn collect(
    remote: &impl Remote,
    account: &str,
    kind: &str,
) -> Result<(String, BTreeMap<String, Value>), SyncError> {
    let state = text(&get(remote, account, kind, &[]).await?, "state")?;
    let mut objects = BTreeMap::new();
    let mut position = 0usize;
    let mut query_state = None;
    let mut expected_total = None;
    let mut bytes = 0usize;
    loop {
        let query=remote.call(&format!("{kind}/query"),json!({"accountId":account,"position":position,"limit":LIMIT,"calculateTotal":true})).await?;
        if query["accountId"] != account || query["position"].as_u64() != Some(position as u64) {
            return Err(SyncError::Protocol);
        }
        let next = text(&query, "queryState")?;
        if query_state.as_ref().is_some_and(|state| state != &next) {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        query_state = Some(next);
        if let Some(total) = query["total"].as_u64() {
            if total > MAX_OBJECTS as u64 || expected_total.is_some_and(|n| n != total) {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            expected_total = Some(total);
        }
        let wanted = strings(&query, "ids")?;
        if wanted.len() > LIMIT || position.saturating_add(wanted.len()) > MAX_OBJECTS {
            return Err(SyncError::Protocol);
        }
        if wanted.is_empty() {
            if expected_total.is_some_and(|n| n != position as u64) {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            break;
        }
        for chunk in wanted.chunks(50) {
            let response = get(remote, account, kind, chunk).await?;
            if text(&response, "state")? != state || !strings(&response, "notFound")?.is_empty() {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            let mut received = BTreeSet::new();
            for object in response["list"].as_array().ok_or(SyncError::Protocol)? {
                bytes = bytes.saturating_add(
                    serde_json::to_vec(object)
                        .map_err(|_| SyncError::Protocol)?
                        .len(),
                );
                if bytes > MAX_SNAPSHOT_BYTES {
                    return Err(SyncError::Local(
                        "Kontaktbestand überschreitet das lokale Limit von 32 MiB.".into(),
                    ));
                }
                let id = text(object, "id")?;
                if !chunk.contains(&id)
                    || !received.insert(id.clone())
                    || objects.insert(id, object.clone()).is_some()
                {
                    return Err(SyncError::Protocol);
                }
            }
            if received.len() != chunk.len() {
                return Err(SyncError::ChangedDuringSnapshot);
            }
        }
        position += wanted.len();
        if expected_total.is_some_and(|n| position as u64 >= n) {
            if expected_total != Some(position as u64) {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            break;
        }
    }
    if text(&get(remote, account, kind, &[]).await?, "state")? != state {
        return Err(SyncError::ChangedDuringSnapshot);
    }
    Ok((state, objects))
}
pub async fn synchronize(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    let result = async {
        let (book_state, books) = collect(remote, account, "AddressBook").await?;
        let (contact_state, contacts) = collect(remote, account, "ContactCard").await?;
        // A book removed while fetching contacts must not leave an inconsistent
        // snapshot published with contacts from a different rights checkpoint.
        if text(&get(remote, account, "AddressBook", &[]).await?, "state")? != book_state {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        Ok(Snapshot {
            books,
            contacts,
            book_state: Some(book_state),
            contact_state: Some(contact_state),
            access_denied: false,
        })
    }
    .await;
    let snapshot = match result {
        Ok(snapshot) => snapshot,
        Err(SyncError::Method(kind))
            if [
                "forbidden",
                "accountNotFound",
                "accountNotSupportedByMethod",
                "unknownMethod",
            ]
            .contains(&kind.as_str()) =>
        {
            Snapshot {
                access_denied: true,
                ..Snapshot::default()
            }
        }
        Err(error) => return Err(error),
    };
    let encoded = serde_json::to_string(&snapshot).map_err(|_| SyncError::Protocol)?;
    if encoded.len() > MAX_SNAPSHOT_BYTES {
        return Err(SyncError::Local(
            "Kontaktbestand überschreitet das lokale Limit von 32 MiB.".into(),
        ));
    }
    // Full replacement explicitly removes remotely deleted contacts/books.
    store.put(&format!("contacts:{scope}"), &encoded)?;
    Ok(())
}
fn searchable(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(searchable).collect::<Vec<_>>().join(" "),
        Value::Object(map) => map.values().map(searchable).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}
fn matches(object: &Value, filter: &Value) -> Result<bool, &'static str> {
    if filter.is_null() {
        return Ok(true);
    }
    let filter = filter.as_object().ok_or("invalidArguments")?;
    if let Some(operator) = filter.get("operator") {
        if filter
            .keys()
            .any(|k| !["operator", "conditions"].contains(&k.as_str()))
        {
            return Err("unsupportedFilter");
        }
        let conditions = filter
            .get("conditions")
            .and_then(Value::as_array)
            .ok_or("invalidArguments")?;
        let values = conditions
            .iter()
            .map(|v| matches(object, v))
            .collect::<Result<Vec<_>, _>>()?;
        return match operator.as_str() {
            Some("AND") => Ok(values.iter().all(|v| *v)),
            Some("OR") => Ok(values.iter().any(|v| *v)),
            Some("NOT") => Ok(!values.iter().any(|v| *v)),
            _ => Err("invalidArguments"),
        };
    }
    let mut result = true;
    for (field, value) in filter {
        result &= match field.as_str() {
            "inAddressBook" => {
                object["addressBookIds"][value.as_str().ok_or("invalidArguments")?] == true
            }
            "kind" | "uid" => {
                object[field].as_str() == Some(value.as_str().ok_or("invalidArguments")?)
            }
            "hasMember" => object["members"][value.as_str().ok_or("invalidArguments")?] == true,
            "text" | "name" | "email" | "phone" | "organization" | "note" | "nickname" => {
                let query = value.as_str().ok_or("invalidArguments")?.to_lowercase();
                let haystack = if field == "text" {
                    [
                        "name",
                        "nicknames",
                        "emails",
                        "phones",
                        "organizations",
                        "notes",
                        "addresses",
                    ]
                    .iter()
                    .map(|k| searchable(&object[k]))
                    .collect::<Vec<_>>()
                    .join(" ")
                } else {
                    let key = match field.as_str() {
                        "email" => "emails",
                        "phone" => "phones",
                        "organization" => "organizations",
                        "note" => "notes",
                        "nickname" => "nicknames",
                        _ => "name",
                    };
                    searchable(&object[key])
                };
                query
                    .split_whitespace()
                    .all(|term| haystack.to_lowercase().contains(term))
            }
            _ => return Err("unsupportedFilter"),
        };
    }
    Ok(result)
}
pub(crate) fn answer(
    snapshot: &Snapshot,
    account: &str,
    method: &str,
    args: &Value,
) -> Result<Value, &'static str> {
    if method.ends_with("/set") || method.ends_with("/copy") {
        return Err("forbidden");
    }
    if snapshot.access_denied {
        return Err("accountNotSupportedByMethod");
    }
    let is_book = method.starts_with("AddressBook/");
    let source = if is_book {
        &snapshot.books
    } else {
        &snapshot.contacts
    };
    let state = if is_book {
        snapshot.book_state.as_deref()
    } else {
        snapshot.contact_state.as_deref()
    }
    .ok_or("serverUnavailable")?;
    let mut objects: Vec<Value> = source
        .values()
        .filter(|object| {
            if is_book {
                object["myRights"]["mayRead"] != false
            } else {
                object["addressBookIds"].as_object().is_some_and(|ids| {
                    ids.iter().any(|(id, enabled)| {
                        enabled == true
                            && snapshot
                                .books
                                .get(id)
                                .is_some_and(|book| book["myRights"]["mayRead"] != false)
                    })
                })
            }
        })
        .cloned()
        .collect();
    if is_book {
        for book in &mut objects {
            book["myRights"] =
                json!({"mayRead":true,"mayWrite":false,"mayShare":false,"mayDelete":false});
            book["shareWith"] = Value::Null;
        }
    } else {
        for contact in &mut objects {
            // Contact photos need their own native blob cache. Do not hand
            // auto-loading remote image URLs to the original avatar component.
            if let Some(object) = contact.as_object_mut() {
                object.remove("media");
            }
            if let Some(books) = contact["addressBookIds"].as_object_mut() {
                books.retain(|id, enabled| {
                    enabled == true
                        && snapshot
                            .books
                            .get(id)
                            .is_some_and(|book| book["myRights"]["mayRead"] != false)
                });
            }
        }
    }
    match method {
        "AddressBook/get" | "ContactCard/get" => {
            desktop::object_get(&objects, account, &format!("local-contacts:{state}"), args)
        }
        "AddressBook/query" | "ContactCard/query" => {
            let mut filtered = Vec::new();
            for object in objects {
                if matches(&object, &args["filter"])? {
                    filtered.push(object);
                }
            }
            let sorts = args.get("sort").and_then(Value::as_array);
            if args.get("sort").is_some_and(|v| !v.is_array()) {
                return Err("invalidArguments");
            }
            for sort in sorts.into_iter().flatten() {
                if !["name", "uid", "created", "updated"]
                    .contains(&sort["property"].as_str().unwrap_or(""))
                {
                    return Err("unsupportedSort");
                }
            }
            filtered.sort_by(|a, b| {
                for sort in sorts.into_iter().flatten() {
                    let field = sort["property"].as_str().unwrap_or("name");
                    let comparison = searchable(&a[field])
                        .to_lowercase()
                        .cmp(&searchable(&b[field]).to_lowercase());
                    if !comparison.is_eq() {
                        return if sort["isAscending"] == false {
                            comparison.reverse()
                        } else {
                            comparison
                        };
                    }
                }
                a["id"].as_str().cmp(&b["id"].as_str())
            });
            let mut position = args
                .get("position")
                .map(|v| v.as_u64().ok_or("invalidArguments"))
                .transpose()?
                .unwrap_or(0) as usize;
            if let Some(anchor) = args.get("anchor") {
                let anchor = anchor.as_str().ok_or("invalidArguments")?;
                let at = filtered
                    .iter()
                    .position(|v| v["id"] == anchor)
                    .ok_or("anchorNotFound")?;
                position = (at as i64
                    + args
                        .get("anchorOffset")
                        .map(|v| v.as_i64().ok_or("invalidArguments"))
                        .transpose()?
                        .unwrap_or(0))
                .max(0) as usize;
            }
            let limit = args
                .get("limit")
                .map(|v| v.as_u64().ok_or("invalidArguments"))
                .transpose()?
                .unwrap_or(250)
                .min(MAX_OBJECTS as u64) as usize;
            let ids = filtered
                .iter()
                .skip(position)
                .take(limit)
                .map(|v| v["id"].clone())
                .collect::<Vec<_>>();
            Ok(
                json!({"accountId":account,"queryState":format!("local-contacts:{state}"),"canCalculateChanges":false,"position":position,"total":filtered.len(),"ids":ids}),
            )
        }
        "ContactCard/changes" | "AddressBook/changes" => Err("cannotCalculateChanges"),
        _ => Err("unknownMethod"),
    }
}
pub fn answer_batch(
    store: &Store,
    config: &TestConfig,
    default_account: &str,
    calls: Calls,
) -> Result<Value, String> {
    let mut responses = vec![];
    let mut prior = HashMap::new();
    for (method, args, id) in calls {
        let result = (|| {
            let args = desktop::resolve_references(&args, &prior)?;
            let account = args
                .get("accountId")
                .map(|v| v.as_str().ok_or("invalidArguments"))
                .transpose()?
                .unwrap_or(default_account);
            desktop::require_capability(store, config, account, sync::CONTACTS)
                .map_err(|_| "accountNotSupportedByMethod")?;
            let scope = sync::scope(config, account);
            let mut snapshot = read_local(store, &scope).map_err(|_| "serverFail")?;
            if let Some(state) = &mut snapshot.contact_state {
                *state = format!(
                    "{}:{}",
                    state,
                    crate::journal::revision(store, &scope).map_err(|_| "serverFail")?
                );
            }
            answer(&snapshot, account, &method, &args)
        })();
        match result {
            Ok(value) => {
                prior.insert((id.clone(), method.clone()), value.clone());
                responses.push(json!([method, value, id]));
            }
            Err(kind) => responses.push(json!(["error",{"type":kind},id])),
        }
    }
    Ok(json!({"methodResponses":responses}))
}
#[tauri::command]
pub fn local_contacts(
    store: State<'_, Store>,
    account_id: String,
    method_calls: Calls,
) -> Result<Value, String> {
    answer_batch(&store, &TestConfig::load()?, &account_id, method_calls)
}
#[tauri::command]
pub async fn sync_contacts(
    store: State<'_, Store>,
    gate: State<'_, SyncGate>,
    network: State<'_, NetworkMode>,
    account_id: String,
) -> Result<(), String> {
    crate::require_network(&network)?;
    let _guard = gate
        .0
        .try_lock()
        .map_err(|_| "Ein Abgleich läuft bereits.")?;
    crate::require_network(&network)?;
    let mut config = TestConfig::load()?;
    config.read_only = true;
    desktop::require_capability(&store, &config, &account_id, sync::CONTACTS)?;
    let remote = sync::Connection::discover_for(&config, &account_id, sync::CONTACTS)
        .await
        .map_err(|e| e.message())?;
    synchronize(
        &store,
        &remote,
        &sync::scope(&config, &account_id),
        &account_id,
    )
    .await
    .map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    #[test]
    #[ignore = "Live read-only contact/address-book snapshot into a temporary database; no contents logged"]
    fn live_contacts_read_only() {
        tauri::async_runtime::block_on(async {
            let mut config = crate::upload_roundtrip::test_config()
                .expect("Nativer Profilzugang nicht verfügbar.");
            config.read_only = true;
            let session: Value = serde_json::from_slice(
                &crate::session::session_bytes(&config)
                    .await
                    .expect("Sitzungsabruf fehlgeschlagen."),
            )
            .expect("Sitzungsformat ungültig.");
            let account = session["primaryAccounts"][sync::CONTACTS]
                .as_str()
                .expect("Kein primäres Kontaktkonto vorhanden.");
            let remote = sync::Connection::discover_for(&config, account, sync::CONTACTS)
                .await
                .unwrap_or_else(|e| panic!("{}", e.message()));
            assert!(remote.call("ContactCard/set", json!({})).await.is_err());
            assert!(remote.call("AddressBook/set", json!({})).await.is_err());
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("contacts.redb");
            let scope = sync::scope(&config, account);
            let counts;
            {
                let store = Store::open(&path).unwrap();
                synchronize(&store, &remote, &scope, account)
                    .await
                    .unwrap_or_else(|e| panic!("{}", e.message()));
                let snapshot = read(&store, &scope).unwrap();
                assert!(!snapshot.access_denied, "Kontaktzugriff wurde abgelehnt.");
                assert!(snapshot.book_state.is_some() && snapshot.contact_state.is_some());
                counts = (snapshot.books.len(), snapshot.contacts.len());
            }
            let store = Store::open(&path).unwrap();
            let snapshot = read(&store, &scope).unwrap();
            assert_eq!((snapshot.books.len(), snapshot.contacts.len()), counts);
            synchronize(&store, &remote, &scope, account)
                .await
                .unwrap_or_else(|e| panic!("{}", e.message()));
            println!("Kontakt-Live-Test read-only: {} Adressbücher, {} Kontakte; redb-Neustart und Folgeabgleich erfolgreich. Keine Inhalte protokolliert.",counts.0,counts.1);
        });
    }
    struct Catalog {
        start: usize,
        count: usize,
        state: &'static str,
        fail: bool,
        race: bool,
        denied: bool,
        checkpoints: AtomicUsize,
        calls: Mutex<Vec<(String, Value)>>,
    }
    impl Catalog {
        fn new(start: usize, count: usize, state: &'static str) -> Self {
            Self {
                start,
                count,
                state,
                fail: false,
                race: false,
                denied: false,
                checkpoints: AtomicUsize::new(0),
                calls: Mutex::new(vec![]),
            }
        }
        fn objects(&self, book: bool) -> Vec<Value> {
            if book {
                vec![
                    json!({"id":"book","name":"Synthetic book","myRights":{"mayRead":true,"mayWrite":true}}),
                ]
            } else {
                (self.start..self.start+self.count).map(|i|json!({"id":format!("c{i:04}"),"addressBookIds":{"book":true},"name":{"full":format!("Person {i:04}")},"emails":{"e":{"address":format!("person{i:04}@example.test")}}})).collect()
            }
        }
    }
    impl Remote for Catalog {
        async fn call(&self, method: &str, args: Value) -> Result<Value, SyncError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), args.clone()));
            assert!([
                "AddressBook/get",
                "AddressBook/query",
                "ContactCard/get",
                "ContactCard/query"
            ]
            .contains(&method));
            if self.denied && method == "ContactCard/query" {
                return Err(SyncError::Method("forbidden".into()));
            }
            let book = method.starts_with("AddressBook/");
            let objects = self.objects(book);
            if method.ends_with("/query") {
                let position = args["position"].as_u64().unwrap() as usize;
                let ids = objects
                    .iter()
                    .skip(position)
                    .take(args["limit"].as_u64().unwrap() as usize)
                    .map(|v| v["id"].clone())
                    .collect::<Vec<_>>();
                return Ok(
                    json!({"accountId":args["accountId"],"position":position,"total":objects.len(),"queryState":self.state,"ids":ids}),
                );
            }
            let ids = args["ids"].as_array().unwrap();
            if self.fail && !book && !ids.is_empty() {
                return Err(SyncError::Network);
            }
            let state = if !book
                && ids.is_empty()
                && self.checkpoints.fetch_add(1, Ordering::SeqCst) > 0
                && self.race
            {
                "changed"
            } else {
                self.state
            };
            Ok(
                json!({"accountId":args["accountId"],"state":state,"list":objects.into_iter().filter(|v|ids.contains(&v["id"])).collect::<Vec<_>>(),"notFound":[]}),
            )
        }
    }
    #[tokio::test]
    async fn pagination_atomic_replacement_deletions_and_offline_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contacts.redb");
        {
            let store = Store::open(&path).unwrap();
            let remote = Catalog::new(0, 260, "s1");
            synchronize(&store, &remote, "a", "a").await.unwrap();
            assert_eq!(read(&store, "a").unwrap().contacts.len(), 260);
            assert!(read(&store, "b").unwrap().contacts.is_empty());
            let calls = remote.calls.lock().unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(m, _)| m == "ContactCard/query")
                    .map(|(_, a)| a["position"].as_u64().unwrap())
                    .collect::<Vec<_>>(),
                vec![0, 250]
            );
            assert!(calls
                .iter()
                .filter(|(m, _)| m.ends_with("/get"))
                .all(|(_, a)| a["ids"].as_array().unwrap().len() <= 50));
            drop(calls);
            let fail = Catalog {
                fail: true,
                ..Catalog::new(1, 2, "s2")
            };
            assert!(synchronize(&store, &fail, "a", "a").await.is_err());
            assert_eq!(read(&store, "a").unwrap().book_state.as_deref(), Some("s1"));
            let race = Catalog {
                race: true,
                ..Catalog::new(1, 2, "s2")
            };
            assert!(synchronize(&store, &race, "a", "a").await.is_err());
            assert_eq!(read(&store, "a").unwrap().contacts.len(), 260);
            synchronize(&store, &Catalog::new(1, 2, "s2"), "a", "a")
                .await
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let snapshot = read(&store, "a").unwrap();
        assert_eq!(snapshot.contacts.len(), 2);
        assert!(!snapshot.contacts.contains_key("c0000"));
        assert_eq!(snapshot.contact_state.as_deref(), Some("s2"));
        let denied = Catalog {
            denied: true,
            ..Catalog::new(0, 3, "s3")
        };
        synchronize(&store, &denied, "a", "a").await.unwrap();
        assert!(read(&store, "a").unwrap().access_denied);
        assert!(read(&store, "a").unwrap().contacts.is_empty());
        synchronize(&store, &Catalog::new(0, 1, "s4"), "a", "a")
            .await
            .unwrap();
        assert!(!read(&store, "a").unwrap().access_denied);
    }
    #[test]
    fn local_search_shared_batches_references_and_revocation_never_allow_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("contacts.redb")).unwrap();
        let config = TestConfig {
            url: url::Url::parse("https://mail.example.test/jmap").unwrap(),
            username: "synthetic".into(),
            password: String::new(),
            read_only: false,
        };
        let key = format!("{}:desktop-session", config.cache_key());
        store.put(&key,&json!({"capabilities":{sync::CONTACTS:{}},"accounts":{"a":{"isPersonal":true,"accountCapabilities":{sync::CONTACTS:{}}},"b":{"isPersonal":false,"accountCapabilities":{}}}}).to_string()).unwrap();
        for (account, name) in [("a", "Alice Offline"), ("b", "Bob Shared")] {
            let snapshot = Snapshot {
                books: BTreeMap::from([(
                    "book".into(),
                    json!({"id":"book","name":"Book","myRights":{"mayRead":true,"mayWrite":true}}),
                )]),
                contacts: BTreeMap::from([(
                    "same".into(),
                    json!({"id":"same","name":{"full":name},"addressBookIds":{"book":true,"hidden-book":true},"emails":{"e":{"address":"reader@example.test"}},"media":{"photo":{"uri":"https://external.example.test/photo.jpg"}}}),
                )]),
                book_state: Some("b1".into()),
                contact_state: Some("c1".into()),
                access_denied: false,
            };
            store
                .put(
                    &format!("contacts:{}", sync::scope(&config, account)),
                    &serde_json::to_string(&snapshot).unwrap(),
                )
                .unwrap();
        }
        let response=answer_batch(&store,&config,"a",vec![
            ("ContactCard/query".into(),json!({"accountId":"a","filter":{"text":"alice offline"},"limit":1}),"q".into()),
            ("ContactCard/get".into(),json!({"accountId":"a","#ids":{"resultOf":"q","name":"ContactCard/query","path":"/ids"}}),"get-a".into()),
            ("ContactCard/get".into(),json!({"accountId":"b","ids":["same"]}),"get-b".into()),
            ("AddressBook/get".into(),json!({"accountId":"a","ids":null}),"books".into()),
            ("ContactCard/set".into(),json!({"accountId":"a","destroy":["same"]}),"write".into()),
            ("ContactCard/query".into(),json!({"accountId":"a","filter":{"unknown":true}}),"bad".into())
        ]).unwrap();
        let rows = &response["methodResponses"];
        assert_eq!(rows[1][1]["list"][0]["name"]["full"], "Alice Offline");
        assert_eq!(rows[2][1]["list"][0]["name"]["full"], "Bob Shared");
        assert!(rows[1][1]["list"][0].get("media").is_none());
        assert!(rows[1][1]["list"][0]["addressBookIds"]
            .get("hidden-book")
            .is_none());
        assert_eq!(rows[3][1]["list"][0]["myRights"]["mayWrite"], false);
        assert_eq!(rows[4][1]["type"], "forbidden");
        assert_eq!(rows[5][1]["type"], "unsupportedFilter");
        store
            .put(
                &key,
                &json!({"accounts":{"a":{"accountCapabilities":{sync::CONTACTS:{}}}}}).to_string(),
            )
            .unwrap();
        let revoked = answer_batch(
            &store,
            &config,
            "a",
            vec![(
                "ContactCard/get".into(),
                json!({"accountId":"b","ids":["same"]}),
                "revoked".into(),
            )],
        )
        .unwrap();
        assert_eq!(
            revoked["methodResponses"][0][1]["type"],
            "accountNotSupportedByMethod"
        );
        assert_eq!(
            read(&store, &sync::scope(&config, "a"))
                .unwrap()
                .contacts
                .len(),
            1
        );
    }
}
