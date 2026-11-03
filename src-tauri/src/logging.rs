//! Small privacy-safe JSONL logger for the native application.
//! Call sites must pass error classes and operation names, never server bodies,
//! email addresses, recipients, credentials, or message content.
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub fn init(app_data: &Path) -> std::io::Result<PathBuf> {
    let directory = app_data.join("logs");
    fs::create_dir_all(&directory)?;
    let path = directory.join("herakles.jsonl");
    let _ = LOG_PATH.set(path.clone());
    event(
        "info",
        "app",
        "started",
        json!({"version": env!("CARGO_PKG_VERSION")}),
    );
    Ok(path)
}

pub fn event(level: &str, area: &str, name: &str, details: Value) {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    let Ok(_guard) = LOG_LOCK.lock() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let record = json!({
        "time": now.as_secs(),
        "level": level,
        "area": area,
        "event": name,
        "details": details,
    });
    let Ok(mut line) = serde_json::to_vec(&record) else {
        return;
    };
    line.push(b'\n');
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() + line.len() as u64 > MAX_BYTES) {
        rotate(path);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(&line);
    }
}

fn rotate(path: &Path) {
    for index in (1..=3).rev() {
        let source = if index == 1 {
            path.to_owned()
        } else {
            path.with_extension(format!("jsonl.{}", index - 1))
        };
        let target = path.with_extension(format!("jsonl.{index}"));
        if source.exists() {
            if index == 3 {
                let _ = fs::remove_file(&target);
            }
            let _ = fs::rename(source, target);
        }
    }
}
