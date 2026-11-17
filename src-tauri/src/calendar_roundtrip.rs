//! Separately approved lifecycle probe: one synthetic event, no attendees or mail.
//! Never run as part of the default suite. A permanent receipt prevents repeats.
use crate::{
    calendar,
    config::TestConfig,
    storage::Store,
    sync::{Connection, Remote, CALENDARS},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn reserve(path: &Path) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Calendar probe already attempted or receipt unavailable".to_owned())?;
    file.write_all(b"{\"status\":\"reserved\"}")
        .and_then(|_| file.sync_all())
        .map_err(|_| "Calendar receipt could not be persisted".into())
}
async fn call(remote: &impl Remote, method: &str, args: Value) -> Result<Value, String> {
    remote.call(method, args).await.map_err(|_| {
        "Calendar probe request failed; inspect receipt before any further attempt".into()
    })
}
#[test]
fn calendar_probe_receipt_cannot_be_reused_or_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attempt.json");
    reserve(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(reserve(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
#[test]
#[ignore = "Requires separate explicit approval: one synthetic calendar event, one update, one deletion; no scheduling mail"]
fn live_calendar_lifecycle_roundtrip() {
    assert_eq!(
        std::env::var("JMAP_CALENDAR_TEST").ok().as_deref(),
        Some("one-synthetic-event"),
        "Explicit one-event gate required"
    );
    let profile = std::env::var("JMAP_TEST_PROFILE").expect("Explicit profile required");
    let login = std::env::var("JMAP_TEST_ACCOUNT").expect("Explicit login required");
    let mut config = TestConfig::existing_for_test(&profile, &login)
        .expect("Existing native credentials unavailable");
    config.read_only = true;
    tauri::async_runtime::block_on(async {
        let bytes = crate::session::session_bytes(&config)
            .await
            .expect("Read-only session discovery failed");
        let session: Value = serde_json::from_slice(&bytes).expect("Invalid session");
        let account = session["primaryAccounts"][CALENDARS]
            .as_str()
            .expect("Personal primary calendar unavailable");
        assert_eq!(session["accounts"][account]["isPersonal"], true);
        let reader = Connection::discover_for(&config, account, CALENDARS)
            .await
            .expect("Read-only calendar connection unavailable");
        let calendars = call(
            &reader,
            "Calendar/get",
            json!({"accountId":account,"ids":null}),
        )
        .await
        .unwrap();
        let defaults: Vec<_> = calendars["list"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["isDefault"] == true && row["myRights"]["mayWriteAll"] == true)
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "Exactly one writable personal default calendar required"
        );
        let calendar_id = defaults[0]["id"].as_str().unwrap();
        let receipt_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.cache/calendar-live-lifecycle-attempt.json");
        std::fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        reserve(&receipt_path).expect("Persistent receipt blocks repeating this run");
        // Only this deliberately gated probe connection is writable. No saved setting changes.
        config.read_only = false;
        let writer = Connection::discover_for(&config, account, CALENDARS)
            .await
            .expect("Probe writer unavailable; receipt retained");
        let uid = format!(
            "herakles-synthetic-calendar-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let created = call(&writer,"CalendarEvent/set",json!({"accountId":account,"sendSchedulingMessages":false,
            "create":{"probe":{"@type":"Event","uid":uid,"title":"Herakles synthetic calendar test","calendarIds":{calendar_id:true},"start":"2030-01-15T10:00:00","timeZone":"Etc/UTC","duration":"PT30M","sequence":0}}})).await.expect("Create status unknown; receipt retained, do not repeat");
        let id = created["created"]["probe"]["id"]
            .as_str()
            .expect("Create acknowledgement incomplete; do not repeat");
        let result: Result<(),String> = async {
            std::fs::write(&receipt_path,serde_json::to_vec(&json!({"status":"created","uid":uid,"eventId":id})).unwrap()).map_err(|_| "Receipt update failed")?;
            let before = call(&reader,"CalendarEvent/get",json!({"accountId":account,"ids":[id]})).await?;
            if before["list"][0]["uid"] != uid || before["list"][0]["start"] != "2030-01-15T10:00:00" { return Err("Create readback mismatch".into()); }
            let moved = call(&writer,"CalendarEvent/set",json!({"accountId":account,"ifInState":before["state"],"sendSchedulingMessages":false,"update":{id:{"start":"2030-01-15T11:00:00","sequence":1}}})).await?;
            if !moved["updated"].as_object().is_some_and(|rows| rows.contains_key(id)) { return Err("Update not acknowledged".into()); }
            let dir=tempfile::tempdir().map_err(|_| "Temporary database unavailable")?;
            let path=dir.path().join("calendar.redb");
            let store=Store::open(&path)?;
            calendar::synchronize(&store,&reader,"probe",account).await.map_err(|_| "Read-only replica sync failed")?;
            drop(store); let store=Store::open(&path)?;
            let snapshot=calendar::read(&store,"probe")?;
            if snapshot.events[id]["uid"] != uid || snapshot.events[id]["start"] != "2030-01-15T11:00:00" { return Err("Offline restart readback mismatch".into()); }
            Ok(())
        }.await;
        // Exactly one cleanup attempt, only for the ID acknowledged for this synthetic UID.
        let own_probe = call(
            &reader,
            "CalendarEvent/get",
            json!({"accountId":account,"ids":[id]}),
        )
        .await
        .ok()
        .is_some_and(|value| {
            value["list"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1 && rows[0]["uid"] == uid)
        });
        let cleanup = if own_probe {
            call(
                &writer,
                "CalendarEvent/set",
                json!({"accountId":account,"sendSchedulingMessages":false,"destroy":[id]}),
            )
            .await
        } else {
            Err("Probe identity could not be confirmed; cleanup requires review".into())
        };
        let cleaned = cleanup
            .as_ref()
            .ok()
            .and_then(|v| v["destroyed"].as_array())
            .is_some_and(|ids| ids.iter().any(|value| value == id));
        let absent = if cleaned {
            call(
                &reader,
                "CalendarEvent/get",
                json!({"accountId":account,"ids":[id]}),
            )
            .await
            .ok()
            .is_some_and(|v| {
                v["list"].as_array().is_some_and(Vec::is_empty)
                    && v["notFound"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|value| value == id))
            })
        } else {
            false
        };
        std::fs::write(&receipt_path,serde_json::to_vec(&json!({"status":if absent{"cleaned"}else{"cleanup-needs-review"},"eventId":id,"uid":uid,"roundtripVerified":result.is_ok(),"cleanupVerified":absent})).unwrap()).expect("Final receipt persistence failed");
        assert!(
            absent,
            "Cleanup not confirmed; inspect receipt, do not repeat"
        );
        result.expect("Synthetic calendar lifecycle failed; receipt retained");
    });
}

fn explicit_suite_config() -> TestConfig {
    let profile = std::env::var("JMAP_TEST_PROFILE").expect("Explicit profile required");
    let account = std::env::var("JMAP_TEST_ACCOUNT").expect("Explicit login required");
    TestConfig::existing_for_test(&profile, &account).expect("Native credentials unavailable")
}
fn suite_receipt(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|_| "Receipt unavailable")?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|_| "Receipt encoding failed")?)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Receipt persistence failed".into())
}
fn set_failure(response: &Value, action: &str, key: &str) -> String {
    let kind = response[action][key]["type"]
        .as_str()
        .unwrap_or("unconfirmed");
    let safe = if [
        "invalidProperties",
        "forbidden",
        "notFound",
        "overQuota",
        "tooLarge",
        "alreadyExists",
    ]
    .contains(&kind)
    {
        kind
    } else {
        "unconfirmed"
    };
    let fields: Vec<_> = response[action][key]["properties"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| {
            [
                "start",
                "timeZone",
                "duration",
                "sequence",
                "recurrenceRule",
                "recurrenceOverrides",
                "alerts",
                "privacy",
                "participants",
                "organizerCalendarAddress",
                "calendarIds",
                "showWithoutTime",
            ]
            .contains(name)
        })
        .collect();
    format!("{safe}; fields={}", fields.join(","))
}
#[test]
#[ignore = "Explicitly authorized five synthetic events and invitation only to mollument@gmail.com; permanent receipt, no automatic retry"]
fn live_calendar_five_event_suite() {
    assert_eq!(
        std::env::var("JMAP_CALENDAR_TEST").ok().as_deref(),
        Some("five-events-mollument")
    );
    let mut config = explicit_suite_config();
    config.read_only = true;
    tauri::async_runtime::block_on(async {
        let session: Value = serde_json::from_slice(
            &crate::session::session_bytes(&config)
                .await
                .expect("Read-only session unavailable"),
        )
        .unwrap();
        let account = session["primaryAccounts"][CALENDARS]
            .as_str()
            .expect("Primary calendar unavailable");
        assert_eq!(session["accounts"][account]["isPersonal"], true);
        let reader = Connection::discover_for(&config, account, CALENDARS)
            .await
            .expect("Read-only connection unavailable");
        let calendars = call(
            &reader,
            "Calendar/get",
            json!({"accountId":account,"ids":null}),
        )
        .await
        .unwrap();
        let defaults: Vec<_> = calendars["list"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["isDefault"] == true && c["myRights"]["mayWriteAll"] == true)
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "Exactly one writable personal default required"
        );
        let cal = defaults[0]["id"].as_str().unwrap();
        let identities = call(
            &reader,
            "ParticipantIdentity/get",
            json!({"accountId":account,"ids":null}),
        )
        .await
        .unwrap();
        let identity = identities["list"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|v| v["isDefault"] == true)
                    .or_else(|| rows.first())
            })
            .and_then(|v| v["calendarAddress"].as_str())
            .expect("Calendar scheduling identity unavailable");
        assert!(
            identity.starts_with("mailto:")
                && !identity.eq_ignore_ascii_case("mailto:mollument@gmail.com")
        );
        let receipt_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.cache/calendar-five-event-attempt.json");
        reserve(&receipt_path).expect("Existing receipt blocks repeat run");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut report = json!({"status":"reserved","createAttempts":0,"events":[],"invitationRequested":false,"cancelRequested":false});
        config.read_only = false;
        let writer = Connection::discover_for(&config, account, CALENDARS)
            .await
            .expect("Writer unavailable; receipt retained");
        let result:Result<(),String>=async {
            for number in 1..=5 {
                let uid=format!("herakles-five-{nonce}-{number}");
                let mut event=json!({"@type":"Event","uid":uid,"title":format!("Herakles TEST {number} - synthetic calendar validation"),"calendarIds":{cal:true},"start":"2026-11-24T10:00:00","duration":"PT30M","timeZone":"Europe/Berlin","sequence":0});
                match number {
                    2 => { event["start"]=json!("2026-11-25T00:00:00");event["duration"]=json!("P1D");event["timeZone"]=Value::Null;event["showWithoutTime"]=json!(true); },
                    3 => { event["recurrenceRule"]=json!({"@type":"RecurrenceRule","frequency":"weekly","count":3}); },
                    4 => { event["privacy"]=json!("private");event["freeBusyStatus"]=json!("free");event["useDefaultAlerts"]=json!(false);event["alerts"]=json!({"test":{"@type":"Alert","trigger":{"@type":"OffsetTrigger","offset":"-PT15M","relativeTo":"start"},"action":"display"}}); },
                    5 => { event["title"]=json!("Herakles TEST - Einladung zur Kalenderpruefung (wird danach abgesagt)");event["description"]=json!("Autorisierter synthetischer Funktionstest. Keine echte Besprechung. Der Termin wird nach der technischen Pruefung wieder abgesagt.");event["organizerCalendarAddress"]=json!(identity);event["participants"]=json!({"organizer":{"@type":"Participant","name":"Herakles Test Organizer","calendarAddress":identity,"roles":{"owner":true},"participationStatus":"accepted"},"guest":{"@type":"Participant","name":"Calendar Test Recipient","calendarAddress":"mailto:mollument@gmail.com","roles":{"attendee":true},"participationStatus":"needs-action","expectReply":true}}); },
                    _=>{}
                }
                report["createAttempts"]=json!(number);if number==5 {report["invitationRequested"]=json!(true);}
                suite_receipt(&receipt_path,&report)?;
                let response=call(&writer,"CalendarEvent/set",json!({"accountId":account,"sendSchedulingMessages":number==5,"create":{"probe":event}})).await?;
                let Some(id)=response["created"]["probe"]["id"].as_str() else {return Err(format!("Create {number}: {}",set_failure(&response,"notCreated","probe")));};
                report["events"].as_array_mut().unwrap().push(json!({"number":number,"uid":uid,"id":id,"verified":false,"cleanupVerified":false}));
                suite_receipt(&receipt_path,&report)?;
            }
            let dir=tempfile::tempdir().map_err(|_| "Temporary DB unavailable")?;let path=dir.path().join("suite.redb");
            let scope=crate::sync::scope(&config,account);
            {
                let store=Store::open(&path)?;
                store.put(&format!("{}:desktop-session",config.cache_key()),&session.to_string())?;
                calendar::synchronize(&store,&reader,&scope,account).await.map_err(|_| "Initial replica sync failed")?;
            }
            // These updates cross the same native validation/journal boundary as the UI.
            for number in [1u64,3,4] {
                let row=report["events"].as_array().unwrap().iter().find(|v| v["number"]==number).unwrap();let id=row["id"].as_str().unwrap();
                let patch=match number {1=>json!({"start":"2026-11-24T11:30:00","sequence":1}),3=>json!({"recurrenceOverrides/2026-12-01T10:00:00":{"excluded":true},"sequence":1}),_=>json!({"freeBusyStatus":"busy","sequence":1})};
                {
                    let store=Store::open(&path)?;let snapshot=calendar::read(&store,&scope)?;
                    let state=format!("local-calendar:{}:{}:{}",snapshot.event_state.as_deref().unwrap(),snapshot.calendar_state.as_deref().unwrap(),snapshot.local_revision);
                    crate::writes::local_object_set_inner(&store,&config,false,"CalendarEvent/set".into(),json!({"accountId":account,"ifInState":state,"sendSchedulingMessages":false,"update":{id:patch}}))?;
                }
                let store=Store::open(&path)?;let entry=crate::journal::entries(&store,&scope)?.remove(0);
                if entry.status!="held" {return Err("Offline update not held".into());}
                crate::journal::approve(&store,&scope,&[entry.id])?;
                let mut args=entry.calls[0].1.clone();args["ifInState"]=json!(entry.base_state);
                if !crate::writes::transmit_object_change(&store,&writer,&scope,&entry,"CalendarEvent/set",args).await? {return Err("Synthetic update not acknowledged; no retry".into());}
                calendar::synchronize(&store,&reader,&scope,account).await.map_err(|_| "Update replica sync failed")?;
            }
            let store=Store::open(&path)?;let snapshot=calendar::read(&store,&scope)?;
            for row in report["events"].as_array_mut().unwrap() {
                let id=row["id"].as_str().unwrap();let event=&snapshot.events[id];
                let valid=event["uid"]==row["uid"] && match row["number"].as_u64().unwrap() {
                    1=>event["start"]=="2026-11-24T11:30:00",2=>event["showWithoutTime"]==true && event["duration"]=="P1D",
                    3=>event["recurrenceOverrides"]["2026-12-01T10:00:00"]["excluded"]==true,
                    4=>event["privacy"]=="private" && event["freeBusyStatus"]=="busy" && event["alerts"].as_object().is_some_and(|a| !a.is_empty()),
                    5=>event["participants"].as_object().is_some_and(|p| p.values().any(|v| v["calendarAddress"]=="mailto:mollument@gmail.com")),_=>false};
                row["verified"]=json!(valid);
            }
            suite_receipt(&receipt_path,&report)?;
            if report["events"].as_array().unwrap().iter().any(|r|r["verified"]!=true) {return Err("Readback option validation failed".into());}
            Ok(())
        }.await;
        // Cleanup never targets unacknowledged or pre-existing events, and never retries.
        for index in 0..report["events"].as_array().unwrap().len() {
            let row = report["events"][index].clone();
            let id = row["id"].as_str().unwrap();
            let current = call(
                &reader,
                "CalendarEvent/get",
                json!({"accountId":account,"ids":[id],"properties":["id","uid"]}),
            )
            .await;
            let owned = current.ok().is_some_and(|v| {
                v["list"]
                    .as_array()
                    .is_some_and(|a| a.len() == 1 && a[0]["uid"] == row["uid"])
            });
            if !owned {
                continue;
            }
            let scheduling = row["number"] == 5;
            if scheduling {
                report["cancelRequested"] = json!(true);
            }
            report["events"][index]["cleanupAttempted"] = json!(true);
            suite_receipt(&receipt_path, &report).unwrap();
            let destroyed = call(
                &writer,
                "CalendarEvent/set",
                json!({"accountId":account,"sendSchedulingMessages":scheduling,"destroy":[id]}),
            )
            .await;
            let ack = destroyed.ok().is_some_and(|v| {
                v["destroyed"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == id))
            });
            let absent = if ack {
                call(
                    &reader,
                    "CalendarEvent/get",
                    json!({"accountId":account,"ids":[id],"properties":["id"]}),
                )
                .await
                .ok()
                .is_some_and(|v| {
                    v["notFound"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v == id))
                })
            } else {
                false
            };
            report["events"][index]["cleanupVerified"] = json!(absent);
            suite_receipt(&receipt_path, &report).unwrap();
        }
        let clean = report["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["cleanupVerified"] == true);
        report["status"] = json!(if result.is_ok() && clean {
            "passed-and-cleaned"
        } else {
            "needs-review"
        });
        report["result"] = json!(result.as_ref().err());
        suite_receipt(&receipt_path, &report).unwrap();
        eprintln!("Calendar suite: attempts={}, acknowledged={}, verified={}, cleaned={}, invitationRequested={}, cancellationRequested={}",report["createAttempts"],report["events"].as_array().unwrap().len(),report["events"].as_array().unwrap().iter().filter(|r|r["verified"]==true).count(),report["events"].as_array().unwrap().iter().filter(|r|r["cleanupVerified"]==true).count(),report["invitationRequested"],report["cancelRequested"]);
        assert!(clean, "Cleanup incomplete; inspect receipt, do not repeat");
        result.expect("Calendar suite failed; inspect sanitized receipt, do not repeat");
    });
}
#[test]
#[ignore = "Read-only diagnostic for authorized calendar suite; no writes or message contents"]
fn live_calendar_suite_readonly_diagnostic() {
    struct Diagnostic<'a>(&'a Connection);
    impl Remote for Diagnostic<'_> {
        async fn call(&self, method: &str, args: Value) -> Result<Value, crate::sync::SyncError> {
            assert!(!method.ends_with("/set"));
            let result = self.0.call(method, args.clone()).await;
            match &result {
                Ok(value) => {
                    let rows = value["list"].as_array();
                    eprintln!("Read diagnostic: method={}, requestedIds={}, requestedProperties={}, rows={}, missing={}, overridesPresent={}, overrideObjects={}, overrideNull={}",method,
                        args["ids"].as_array().map_or(0,Vec::len),args["properties"].as_array().map_or(0,Vec::len),rows.map_or(0,Vec::len),value["notFound"].as_array().map_or(0,Vec::len),
                        rows.map_or(0,|v|v.iter().filter(|r|r.get("recurrenceOverrides").is_some()).count()),rows.map_or(0,|v|v.iter().filter(|r|r["recurrenceOverrides"].is_object()).count()),rows.map_or(0,|v|v.iter().filter(|r|r.get("recurrenceOverrides").is_some_and(Value::is_null)).count()));
                }
                Err(_) => eprintln!("Read diagnostic: method={method}, requestFailed=true"),
            }
            result
        }
    }
    let config = explicit_suite_config();
    assert!(config.read_only);
    tauri::async_runtime::block_on(async {
        let session: Value =
            serde_json::from_slice(&crate::session::session_bytes(&config).await.unwrap()).unwrap();
        let account = session["primaryAccounts"][CALENDARS].as_str().unwrap();
        let remote = Connection::discover_for(&config, account, CALENDARS)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("diagnostic.redb")).unwrap();
        let result =
            calendar::synchronize(&store, &Diagnostic(&remote), "diagnostic", account).await;
        eprintln!(
            "Read diagnostic final: {}",
            result
                .as_ref()
                .err()
                .map(|e| e.message())
                .unwrap_or("ok".into())
        );
        assert!(result.is_ok(), "Read-only calendar sync failed");
    });
}

#[test]
#[ignore = "Authorized single TEST-marked event: offline create/move/series/all-day/delete and real reconnect worker, no mail"]
fn live_calendar_offline_options_roundtrip() {
    assert_eq!(
        std::env::var("JMAP_CALENDAR_TEST").ok().as_deref(),
        Some("one-test-offline-options")
    );
    let mut config = explicit_suite_config();
    config.read_only = true;
    tauri::async_runtime::block_on(async {
        let session: Value = serde_json::from_slice(
            &crate::session::session_bytes(&config)
                .await
                .expect("Read-only discovery unavailable"),
        )
        .unwrap();
        let account = session["primaryAccounts"][CALENDARS].as_str().unwrap();
        assert_eq!(session["accounts"][account]["isPersonal"], true);
        let reader = Connection::discover_for(&config, account, CALENDARS)
            .await
            .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("offline-options.redb");
        let scope = crate::sync::scope(&config, account);
        let calendar_id = {
            let store = Store::open(&path).unwrap();
            store
                .put(
                    &format!("{}:desktop-session", config.cache_key()),
                    &session.to_string(),
                )
                .unwrap();
            calendar::synchronize(&store, &reader, &scope, account)
                .await
                .expect("Read-only baseline unavailable");
            let snapshot = calendar::read(&store, &scope).unwrap();
            let defaults: Vec<_> = snapshot
                .calendars
                .iter()
                .filter(|c| c["isDefault"] == true && c["myRights"]["mayWriteAll"] == true)
                .collect();
            assert_eq!(defaults.len(), 1);
            defaults[0]["id"].as_str().unwrap().to_owned()
        };
        let receipt_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.cache/calendar-offline-options-attempt.json");
        reserve(&receipt_path).expect("Attempt receipt blocks repeated run");
        let uid = format!(
            "herakles-test-offline-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut report = json!({"status":"reserved","createAttempted":false,"deleteAttempted":false,"uid":uid,"phases":[],"cleanupVerified":false,"mailRequested":false});
        config.read_only = false;
        let (network, _) = crate::NetworkMode::new();
        let mut server_id: Option<String> = None;
        let result:Result<(),String>=async {
            for phase in 0..5 {
                network.set_enabled(false);
                let local_id;
                let baseline_ids=call(&reader,"CalendarEvent/query",json!({"accountId":account,"filter":{"uid":uid},"limit":2})).await?["ids"].clone();
                {
                    let store=Store::open(&path)?;
                    store.set_change_review(&scope,true,true)?;
                    let snapshot=calendar::read(&store,&scope)?;
                    let state=format!("local-calendar:{}:{}:{}",snapshot.event_state.as_deref().unwrap(),snapshot.calendar_state.as_deref().unwrap(),snapshot.local_revision);
                    let mut args=json!({"accountId":account,"ifInState":state,"sendSchedulingMessages":false});
                    if phase==0 {
                        args["create"]=json!({"probe":{"@type":"Event","uid":uid,"title":"TEST Herakles - Offline- und Kalenderoptionen","calendarIds":{calendar_id.clone():true},"start":"2030-01-15T10:00:00","duration":"PT30M","timeZone":"Etc/UTC","sequence":0}});
                    } else if phase==4 {args["destroy"]=json!([server_id.as_ref().unwrap()]);}
                    else {
                        let patch=match phase {
                            1=>json!({"start":"2030-01-15T11:00:00","sequence":1,"privacy":"private","freeBusyStatus":"busy","useDefaultAlerts":false,"alerts":{"test":{"@type":"Alert","trigger":{"@type":"OffsetTrigger","offset":"-PT15M","relativeTo":"start"},"action":"display"}}}),
                            2=>json!({"sequence":2,"recurrenceRule":{"@type":"RecurrenceRule","frequency":"weekly","count":3},"recurrenceOverrides":{"2030-01-22T11:00:00":{"excluded":true}}}),
                            _=>json!({"sequence":3,"recurrenceRule":null,"recurrenceOverrides":null,"start":"2030-01-16T00:00:00","duration":"P1D","timeZone":null,"showWithoutTime":true,"freeBusyStatus":"free"})
                        };
                        args["update"]=json!({server_id.as_ref().unwrap():patch});
                    }
                    let answer=crate::writes::local_object_set_inner(&store,&config,network.enabled(),"CalendarEvent/set".into(),args)?;
                    local_id=if phase==0 {answer["created"]["probe"]["id"].as_str().ok_or("Local create unavailable")?.to_owned()}else{server_id.as_ref().unwrap().clone()};
                    if crate::writes::sync_pending_changes_inner(&store,&config,&network).await.is_ok() {return Err("Offline worker was not blocked".into());}
                }
                // Reopen redb as on a process restart, with the same offline barrier.
                let store=Store::open(&path)?;
                let entries=crate::journal::entries(&store,&scope)?;
                if entries.len()!=1 || entries[0].status!="held" {return Err(format!("Phase {phase}: offline intention not durable"));}
                let local=calendar::read(&store,&scope)?;
                let visible=local.events.get(&local_id);
                let local_valid=match phase {
                    0=>visible.is_some_and(|e|e["uid"]==uid && e["title"].as_str().is_some_and(|t|t.starts_with("TEST "))),
                    1=>visible.is_some_and(|e|e["start"]=="2030-01-15T11:00:00" && e["privacy"]=="private"),
                    2=>visible.is_some_and(|e|e["recurrenceOverrides"]["2030-01-22T11:00:00"]["excluded"]==true),
                    3=>visible.is_some_and(|e|e["showWithoutTime"]==true && e["duration"]=="P1D"),
                    _=>visible.is_none()
                };
                if !local_valid {return Err(format!("Phase {phase}: offline view mismatch"));}
                let unchanged=call(&reader,"CalendarEvent/query",json!({"accountId":account,"filter":{"uid":uid},"limit":2})).await?["ids"]==baseline_ids;
                if !unchanged {return Err("Server membership changed while offline".into());}
                network.set_enabled(true);network.reconnect();
                // Reconnection alone must respect the product's explicit review barrier.
                if crate::writes::sync_pending_changes_inner(&store,&config,&network).await? {return Err("Reconnect bypassed review".into());}
                if crate::journal::entries(&store,&scope)?[0].status!="held" {return Err("Reconnect changed held intention".into());}
                crate::journal::approve(&store,&scope,&[entries[0].id])?;
                report[if phase==0 {"createAttempted"}else if phase==4 {"deleteAttempted"}else{"updateAttempted"}]=json!(true);
                suite_receipt(&receipt_path,&report)?;
                if !crate::writes::sync_pending_changes_inner(&store,&config,&network).await? {return Err(format!("Phase {phase}: worker did not acknowledge change"));}
                // The worker must never replay an already acknowledged operation.
                if crate::writes::sync_pending_changes_inner(&store,&config,&network).await? {return Err("Worker replayed confirmed change".into());}
                calendar::synchronize(&store,&reader,&scope,account).await.map_err(|_|format!("Phase {phase}: replica readback failed"))?;
                let query=call(&reader,"CalendarEvent/query",json!({"accountId":account,"filter":{"uid":uid},"limit":2})).await?;
                let ids=query["ids"].as_array().ok_or("Invalid readback query")?;
                if phase==4 {
                    if !ids.is_empty() {return Err("Deleted event remains on server".into());}
                    report["cleanupVerified"]=json!(true);
                } else {
                    if ids.len()!=1 {return Err("Readback did not find exactly one TEST event".into());}
                    let id=ids[0].as_str().ok_or("Invalid server id")?;server_id=Some(id.to_owned());report["eventId"]=json!(id);
                    let snapshot=calendar::read(&store,&scope)?;let event=&snapshot.events[id];
                    let valid=event["uid"]==uid && event["title"].as_str().is_some_and(|t|t.starts_with("TEST ")) && match phase {
                        0=>event["start"]=="2030-01-15T10:00:00",
                        1=>event["start"]=="2030-01-15T11:00:00" && event["privacy"]=="private" && event["alerts"].as_object().is_some_and(|v|!v.is_empty()),
                        2=>event["recurrenceOverrides"]["2030-01-22T11:00:00"]["excluded"]==true && event["recurrenceRule"]["count"]==3,
                        _=>event["showWithoutTime"]==true && ["P1D","PT24H"].contains(&event["duration"].as_str().unwrap_or("")) && event["freeBusyStatus"]=="free" && event["recurrenceRule"].is_null()
                    };
                    if !valid {return Err(format!("Phase {phase}: option readback mismatch"));}
                }
                if !crate::journal::entries(&store,&scope)?.is_empty() {return Err("Acknowledged journal entry not cleared by replica".into());}
                report["phases"].as_array_mut().unwrap().push(json!({"phase":phase,"offlineRestartVerified":true,"reviewRespected":true,"onlineReadbackVerified":true,"noReplayVerified":true}));
                suite_receipt(&receipt_path,&report)?;
            }
            Ok(())
        }.await;
        if report["cleanupVerified"] != true {
            let query = call(
                &reader,
                "CalendarEvent/query",
                json!({"accountId":account,"filter":{"uid":uid},"limit":2}),
            )
            .await;
            if let Some(ids) = query.as_ref().ok().and_then(|v| v["ids"].as_array()) {
                if ids.is_empty() {
                    report["cleanupVerified"] = json!(true);
                } else if ids.len() == 1 && report["deleteAttempted"] != true {
                    let id = ids[0].as_str().unwrap();
                    let current = call(
                        &reader,
                        "CalendarEvent/get",
                        json!({"accountId":account,"ids":[id],"properties":["id","uid","title"]}),
                    )
                    .await;
                    if current.ok().is_some_and(|v| {
                        v["list"].as_array().is_some_and(|r| {
                            r.len() == 1
                                && r[0]["uid"] == uid
                                && r[0]["title"]
                                    .as_str()
                                    .is_some_and(|t| t.starts_with("TEST "))
                        })
                    }) {
                        report["deleteAttempted"] = json!(true);
                        suite_receipt(&receipt_path, &report).unwrap();
                        let writer = Connection::discover_for(&config, account, CALENDARS)
                            .await
                            .unwrap();
                        let _=call(&writer,"CalendarEvent/set",json!({"accountId":account,"sendSchedulingMessages":false,"destroy":[id]})).await;
                        report["cleanupVerified"] = json!(call(
                            &reader,
                            "CalendarEvent/query",
                            json!({"accountId":account,"filter":{"uid":uid},"limit":2})
                        )
                        .await
                        .ok()
                        .is_some_and(|v| v["ids"].as_array().is_some_and(Vec::is_empty)));
                    }
                }
            }
        }
        report["status"] = json!(if result.is_ok() && report["cleanupVerified"] == true {
            "passed-and-cleaned"
        } else {
            "needs-review"
        });
        report["error"] = json!(result.as_ref().err());
        suite_receipt(&receipt_path, &report).unwrap();
        eprintln!(
            "Offline calendar options: verifiedPhases={}, cleanupVerified={}, mailRequested=false",
            report["phases"].as_array().unwrap().len(),
            report["cleanupVerified"]
        );
        assert_eq!(
            report["cleanupVerified"], true,
            "Cleanup not confirmed; receipt retained"
        );
        result.expect("Offline calendar options failed; inspect receipt, do not repeat");
    });
}
