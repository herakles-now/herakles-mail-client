//! Native JMAP Server-Sent Events listener.
//! The stream carries only invalidation signals; actual data is fetched through
//! the existing redb synchronization path after a notification arrives.
use crate::{config::TestConfig, logging, session};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub fn start(app: AppHandle, mut network: tokio::sync::watch::Receiver<crate::NetworkState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            if !network.borrow().enabled && network.changed().await.is_err() {
                return;
            }
            tokio::select! {
                result = listen_once(&app) => {
                    if let Err(reason) = result {
                        logging::event(
                            "warn",
                            "sync",
                            "push_disconnected",
                            serde_json::json!({"reason":reason}),
                        );
                    }
                    if network.borrow().enabled {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                            changed = network.changed() => if changed.is_err() { return; },
                        }
                    }
                }
                changed = network.changed() => {
                    if changed.is_err() { return; }
                }
            }
        }
    });
}

async fn listen_once(app: &AppHandle) -> Result<(), String> {
    let config = TestConfig::load()?;
    let session: Value = serde_json::from_slice(&session::session_bytes(&config).await?)
        .map_err(|_| "JMAP-Sitzung ist ungültig.")?;
    let template = session["eventSourceUrl"]
        .as_str()
        .ok_or("JMAP-Push wird vom Server nicht angeboten.")?;
    let url = expand_event_source(template)?;
    if url.origin() != config.url.origin() {
        return Err("JMAP-Push-Adresse gehört nicht zum konfigurierten Server.".into());
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "JMAP-Push-Verbindung konnte nicht gestartet werden.")?;
    let response = client
        .get(url)
        .basic_auth(&config.username, Some(&config.password))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await
        .map_err(|_| "JMAP-Push-Server ist nicht erreichbar.")?;
    if !response.status().is_success() {
        return Err(format!(
            "JMAP-Push-Verbindung abgelehnt (HTTP {}).",
            response.status().as_u16()
        ));
    }
    logging::event("info", "sync", "push_connected", serde_json::json!({}));

    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut event_name = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "JMAP-Push-Verbindung wurde unterbrochen.")?;
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = pending.drain(..=newline).collect();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if event_name == "state" {
                    let _ = app.emit("jmap-push", ());
                }
                event_name.clear();
            } else if let Some(value) = line.strip_prefix("event:") {
                event_name = value.trim().to_string();
            }
        }
        if pending.len() > 64 * 1024 {
            return Err("JMAP-Push-Ereignis ist unerwartet groß.".into());
        }
    }
    Err("JMAP-Push-Verbindung wurde beendet.".into())
}

fn expand_event_source(template: &str) -> Result<url::Url, String> {
    let expanded = template
        .replace("{types}", "Email,Mailbox,CalendarEvent,Calendar")
        .replace("{closeafter}", "no")
        .replace("{ping}", "30");
    if expanded.contains('{') || expanded.contains('}') {
        return Err("JMAP-Push-Adresse enthält nicht unterstützte Parameter.".into());
    }
    let url = url::Url::parse(&expanded).map_err(|_| "JMAP-Push-Adresse ist ungültig.")?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("JMAP-Push-Adresse ist nicht sicher.".into());
    }
    Ok(url)
}
