use crate::{
    storage::Store,
    sync::{Remote, SyncError},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const EVENT_PROPERTIES: &[&str] = &[
    "id",
    "@type",
    "uid",
    "calendarIds",
    "title",
    "description",
    "descriptionContentType",
    "created",
    "updated",
    "sequence",
    "start",
    "duration",
    "timeZone",
    "showWithoutTime",
    "utcStart",
    "utcEnd",
    "status",
    "freeBusyStatus",
    "privacy",
    "color",
    "keywords",
    "categories",
    "locale",
    "replyTo",
    "organizerCalendarAddress",
    "participants",
    "mayInviteSelf",
    "mayInviteOthers",
    "hideAttendees",
    "recurrenceId",
    "recurrenceIdTimeZone",
    "recurrenceRule",
    "excludedRecurrenceRule",
    "useDefaultAlerts",
    "alerts",
    "locations",
    "virtualLocations",
    "links",
    "relatedTo",
    "isDraft",
    "isOrigin",
    "baseEventId",
];
const CALENDAR_PROPERTIES: &[&str] = &[
    "id",
    "name",
    "description",
    "color",
    "sortOrder",
    "isSubscribed",
    "isVisible",
    "isDefault",
    "includeInAvailability",
    "defaultAlertsWithTime",
    "defaultAlertsWithoutTime",
    "timeZone",
    "shareWith",
    "myRights",
];

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(default)]
    pub access_denied: bool,
    pub calendars: Vec<Value>,
    pub events: BTreeMap<String, Value>,
    pub identities: Vec<Value>,
    pub event_state: Option<String>,
    pub calendar_state: Option<String>,
}

pub fn read(store: &Store, scope: &str) -> Result<Snapshot, String> {
    if store.get(&format!("calendar-denied:{scope}"))?.as_deref() == Some("true") {
        return Ok(Snapshot {
            access_denied: true,
            ..Snapshot::default()
        });
    }
    store
        .get(&format!("calendar:{scope}"))?
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|_| "Lokale Kalenderdaten sind ungültig.".into())
        })
        .transpose()
        .map(|v| v.unwrap_or_default())
}
fn text(value: &Value, field: &str) -> Result<String, SyncError> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or(SyncError::Protocol)
}
fn ids(value: &Value, field: &str) -> Result<Vec<String>, SyncError> {
    value[field]
        .as_array()
        .ok_or(SyncError::Protocol)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or(SyncError::Protocol))
        .collect()
}
async fn event_get(
    remote: &impl Remote,
    account: &str,
    wanted: &[String],
) -> Result<(String, Vec<Value>, Vec<String>), SyncError> {
    let base = remote
        .call(
            "CalendarEvent/get",
            json!({"accountId":account,"ids":wanted,"properties":EVENT_PROPERTIES}),
        )
        .await?;
    let state = text(&base, "state")?;
    let mut objects = base["list"].as_array().ok_or(SyncError::Protocol)?.clone();
    let missing = ids(&base, "notFound")?;
    let mut seen = BTreeSet::new();
    for object in &objects {
        if !seen.insert(text(object, "id")?) {
            return Err(SyncError::Protocol);
        }
    }
    for id in &missing {
        if !seen.insert(id.clone()) {
            return Err(SyncError::Protocol);
        }
    }
    if seen != wanted.iter().cloned().collect() {
        return Err(SyncError::Protocol);
    }
    if !wanted.is_empty() {
        // Stalwart refuses utcStart/utcEnd and recurrenceOverrides in the same get.
        let overrides = remote
            .call(
                "CalendarEvent/get",
                json!({"accountId":account,"ids":wanted,"properties":["id","recurrenceOverrides"]}),
            )
            .await?;
        if text(&overrides, "state")? != state {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        for row in overrides["list"].as_array().ok_or(SyncError::Protocol)? {
            if let Some(target) = objects.iter_mut().find(|object| object["id"] == row["id"]) {
                if let Some(value) = row.get("recurrenceOverrides") {
                    target["recurrenceOverrides"] = value.clone();
                }
            }
        }
    }
    Ok((state, objects, missing))
}

async fn full(
    remote: &impl Remote,
    account: &str,
    snapshot: &mut Snapshot,
) -> Result<(), SyncError> {
    let (baseline, _, _) = event_get(remote, account, &[]).await?;
    let mut position = 0;
    let mut query_state = None;
    let mut events = BTreeMap::new();
    loop {
        let query = remote
            .call(
                "CalendarEvent/query",
                json!({"accountId":account,"position":position,"limit":250,"calculateTotal":true}),
            )
            .await?;
        let state = text(&query, "queryState")?;
        if query_state.as_ref().is_some_and(|prior| prior != &state) {
            return Err(SyncError::ChangedDuringSnapshot);
        }
        query_state = Some(state);
        let wanted = ids(&query, "ids")?;
        if wanted.is_empty() {
            break;
        }
        for chunk in wanted.chunks(50) {
            let (state, objects, missing) = event_get(remote, account, chunk).await?;
            if state != baseline || !missing.is_empty() {
                return Err(SyncError::ChangedDuringSnapshot);
            }
            for object in objects {
                if events.insert(text(&object, "id")?, object).is_some() {
                    return Err(SyncError::ChangedDuringSnapshot);
                }
            }
        }
        position += wanted.len();
        if query["total"]
            .as_u64()
            .is_some_and(|total| position as u64 >= total)
        {
            break;
        }
    }
    if event_get(remote, account, &[]).await?.0 != baseline {
        return Err(SyncError::ChangedDuringSnapshot);
    }
    snapshot.events = events;
    snapshot.event_state = Some(baseline);
    Ok(())
}

pub async fn synchronize(
    store: &Store,
    remote: &impl Remote,
    scope: &str,
    account: &str,
) -> Result<(), SyncError> {
    let mut snapshot = read(store, scope)?;
    let calendars = match remote
        .call(
            "Calendar/get",
            json!({"accountId":account,"ids":null,"properties":CALENDAR_PROPERTIES}),
        )
        .await
    {
        Err(SyncError::Method(kind))
            if [
                "forbidden",
                "accountNotFound",
                "accountNotSupportedByMethod",
            ]
            .contains(&kind.as_str()) =>
        {
            store.put(&format!("calendar-denied:{scope}"), "true")?;
            return Ok(());
        }
        result => result?,
    };
    snapshot.calendars = calendars["list"]
        .as_array()
        .ok_or(SyncError::Protocol)?
        .clone();
    snapshot.calendar_state = Some(text(&calendars, "state")?);
    match remote
        .call(
            "ParticipantIdentity/get",
            json!({"accountId":account,"ids":null}),
        )
        .await
    {
        Ok(result) => {
            snapshot.identities = result["list"]
                .as_array()
                .ok_or(SyncError::Protocol)?
                .clone()
        }
        Err(SyncError::Method(kind))
            if ["unknownMethod", "forbidden", "accountNotSupportedByMethod"]
                .contains(&kind.as_str()) =>
        {
            snapshot.identities.clear();
        }
        Err(error) => return Err(error),
    }
    if snapshot.event_state.is_none() {
        full(remote, account, &mut snapshot).await?;
    } else {
        loop {
            let since = snapshot.event_state.as_ref().unwrap().clone();
            let changes = match remote
                .call(
                    "CalendarEvent/changes",
                    json!({"accountId":account,"sinceState":since,"maxChanges":250}),
                )
                .await
            {
                Err(SyncError::Method(kind))
                    if kind == "cannotCalculateChanges" || kind == "unknownMethod" =>
                {
                    full(remote, account, &mut snapshot).await?;
                    break;
                }
                result => result?,
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
            let wanted: BTreeSet<String> = ids(&changes, "created")?
                .into_iter()
                .chain(ids(&changes, "updated")?)
                .collect();
            let mut removed: BTreeSet<String> = ids(&changes, "destroyed")?.into_iter().collect();
            for chunk in wanted.into_iter().collect::<Vec<_>>().chunks(50) {
                let (_, objects, missing) = event_get(remote, account, chunk).await?;
                removed.extend(missing);
                for object in objects {
                    snapshot.events.insert(text(&object, "id")?, object);
                }
            }
            for id in removed {
                snapshot.events.remove(&id);
            }
            snapshot.event_state = Some(next);
            if !more {
                break;
            }
        }
    }
    // Entire calendar replica is published atomically; failures retain the previous snapshot.
    snapshot.access_denied = false;
    store.put(
        &format!("calendar:{scope}"),
        &serde_json::to_string(&snapshot).map_err(|_| SyncError::Protocol)?,
    )?;
    store.put(&format!("calendar-denied:{scope}"), "false")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::TestConfig,
        sync::{Connection, CALENDARS},
    };
    use std::{collections::VecDeque, sync::Mutex};

    struct Script(Mutex<VecDeque<(&'static str, Result<Value, SyncError>)>>);
    impl Remote for Script {
        async fn call(&self, method: &str, _: Value) -> Result<Value, SyncError> {
            assert!(!method.ends_with("/set"));
            let (expected, result) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("Unexpected request");
            assert_eq!(method, expected);
            result
        }
    }
    fn get(state: &str, rows: Vec<Value>) -> Value {
        json!({"state":state,"list":rows,"notFound":[]})
    }

    #[test]
    fn calendar_snapshot_survives_offline_restart_and_failed_delta() {
        tauri::async_runtime::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("calendar.redb");
            let store = Store::open(&path).unwrap();
            let remote = Script(Mutex::new(VecDeque::from(vec![
                (
                    "Calendar/get",
                    Ok(get("c1", vec![json!({"id":"c","name":"Calendar"})])),
                ),
                ("ParticipantIdentity/get", Ok(get("i1", vec![]))),
                ("CalendarEvent/get", Ok(get("e1", vec![]))),
                (
                    "CalendarEvent/query",
                    Ok(json!({"queryState":"q1","ids":["e"],"total":1})),
                ),
                (
                    "CalendarEvent/get",
                    Ok(get(
                        "e1",
                        vec![
                            json!({"id":"e","title":"Offline event","start":"2026-11-03T12:00:00","calendarIds":{"c":true}}),
                        ],
                    )),
                ),
                (
                    "CalendarEvent/get",
                    Ok(get(
                        "e1",
                        vec![json!({"id":"e","recurrenceOverrides":null})],
                    )),
                ),
                ("CalendarEvent/get", Ok(get("e1", vec![]))),
            ])));
            synchronize(&store, &remote, "a", "account").await.unwrap();
            drop(store);
            let store = Store::open(&path).unwrap();
            assert_eq!(read(&store, "a").unwrap().events.len(), 1);
            assert!(read(&store, "other-account").unwrap().events.is_empty());
            let failing = Script(Mutex::new(VecDeque::from(vec![
                ("Calendar/get", Ok(get("c2", vec![]))),
                ("ParticipantIdentity/get", Ok(get("i1", vec![]))),
                (
                    "CalendarEvent/changes",
                    Ok(
                        json!({"oldState":"e1","newState":"e2","created":["new"],"updated":[],"destroyed":["e"],"hasMoreChanges":false}),
                    ),
                ),
                ("CalendarEvent/get", Err(SyncError::Network)),
            ])));
            assert!(synchronize(&store, &failing, "a", "account").await.is_err());
            let saved = read(&store, "a").unwrap();
            assert_eq!(saved.event_state.as_deref(), Some("e1"));
            assert_eq!(saved.calendar_state.as_deref(), Some("c1"));
            assert!(saved.events.contains_key("e"));
        });
    }

    #[test]
    fn changed_calendar_snapshot_is_never_published() {
        tauri::async_runtime::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("calendar.redb")).unwrap();
            let remote = Script(Mutex::new(VecDeque::from(vec![
                ("Calendar/get", Ok(get("c1", vec![json!({"id":"c"})]))),
                ("ParticipantIdentity/get", Ok(get("i1", vec![]))),
                ("CalendarEvent/get", Ok(get("e1", vec![]))),
                (
                    "CalendarEvent/query",
                    Ok(json!({"queryState":"q1","ids":[],"total":0})),
                ),
                ("CalendarEvent/get", Ok(get("e2", vec![]))),
            ])));
            assert!(matches!(
                synchronize(&store, &remote, "scope", "account").await,
                Err(SyncError::ChangedDuringSnapshot)
            ));
            let snapshot = read(&store, "scope").unwrap();
            assert!(snapshot.calendar_state.is_none());
            assert!(snapshot.event_state.is_none());
        });
    }

    #[test]
    fn mail_only_shared_account_does_not_expose_old_calendar_cache() {
        tauri::async_runtime::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("calendar.redb")).unwrap();
            let mut old = Snapshot::default();
            old.event_state = Some("old".into());
            old.events.insert("event".into(), json!({"id":"event"}));
            store
                .put("calendar:shared", &serde_json::to_string(&old).unwrap())
                .unwrap();
            let remote = Script(Mutex::new(VecDeque::from(vec![(
                "Calendar/get",
                Err(SyncError::Method("accountNotSupportedByMethod".into())),
            )])));
            synchronize(&store, &remote, "shared", "account")
                .await
                .unwrap();
            let visible = read(&store, "shared").unwrap();
            assert!(visible.access_denied);
            assert!(visible.events.is_empty());
            assert!(visible.calendars.is_empty());
        });
    }

    #[test]
    #[ignore = "Requires local Stalwart; read-only calendar download into a temporary database"]
    fn live_calendar_read_only() {
        tauri::async_runtime::block_on(async {
            let mut config = TestConfig::load().expect("Lokale Konfiguration ungültig.");
            config.read_only = true;
            let session: Value =
                serde_json::from_slice(&crate::session::session_bytes(&config).await.unwrap())
                    .unwrap();
            let account = session["primaryAccounts"][CALENDARS]
                .as_str()
                .or_else(|| {
                    session["accounts"].as_object().and_then(|accounts| {
                        accounts
                            .iter()
                            .find(|(_, value)| {
                                value["accountCapabilities"].get(CALENDARS).is_some()
                            })
                            .map(|(id, _)| id.as_str())
                    })
                })
                .expect("Der Server meldet keine JMAP-Kalender für dieses Konto.");
            let connection = Connection::discover_for(&config, account, CALENDARS)
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("live-calendar.redb");
            let store = Store::open(&path).unwrap();
            synchronize(&store, &connection, "live", account)
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            let count = read(&store, "live").unwrap().events.len();
            synchronize(&store, &connection, "live", account)
                .await
                .unwrap_or_else(|error| panic!("{}", error.message()));
            drop(store);
            let store = Store::open(&path).unwrap();
            let saved = read(&store, "live").unwrap();
            assert!(saved.event_state.is_some());
            println!(
                "Kalender: {}; Termine beim Erstabgleich: {count}; Termine offline lesbar: {}",
                saved.calendars.len(),
                saved.events.len()
            );
        });
    }
}
