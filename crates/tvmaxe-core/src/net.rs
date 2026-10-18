//! Slow operations (network + database writes), meant to run on threads
//! separate from the UI. Each one opens its own database connection.

use crate::i18n::{fill, tr};
use crate::{ChannelList, Database, Error, Result, formats};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

const MAX_LIST: u64 = 64 * 1024 * 1024;
const MAX_LOGO: u64 = 2 * 1024 * 1024;

pub const DEFAULT_USER_AGENT: &str = concat!("TV-Maxe/", env!("CARGO_PKG_VERSION"));

static USER_AGENT: RwLock<Option<String>> = RwLock::new(None);

/// User-Agent for downloading lists and logos (`None` = default).
/// Some IPTV servers only accept certain values.
pub fn set_user_agent(ua: Option<String>) {
    *USER_AGENT.write().unwrap() = ua.filter(|s| !s.trim().is_empty());
}

fn agent() -> ureq::Agent {
    let ua = USER_AGENT
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(ua)
        .build()
        .into()
}

/// Reads a source: `http(s)://`, `file://` or a local path.
pub fn fetch(source: &str, limit: u64) -> Result<Vec<u8>> {
    let data = if source.starts_with("http://") || source.starts_with("https://") {
        let mut resp = agent()
            .get(source)
            .call()
            .map_err(|e| Error::Http(e.to_string()))?;
        resp.body_mut()
            .with_config()
            .limit(limit.saturating_add(1))
            .read_to_vec()
            .map_err(|e| Error::Http(e.to_string()))
    } else {
        let path = source.strip_prefix("file://").unwrap_or(source);
        let file = std::fs::File::open(path)?;
        let mut data = Vec::new();
        file.take(limit.saturating_add(1)).read_to_end(&mut data)?;
        Ok(data)
    }?;

    if u64::try_from(data.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::Invalid(fill(
            tr("the source exceeds the %1-byte limit"),
            &[&limit],
        )));
    }
    Ok(data)
}

/// Downloads and stores a subscription. The error is also recorded in the database.
pub fn update_subscription(db_path: &Path, id: i64) -> Result<usize> {
    let mut db = Database::open(db_path)?;
    let Some(url) = db.subscription_url(id)? else {
        return Ok(0);
    };
    let list = match formats::xtream::Account::from_source(&url) {
        Some(account) => fetch_xtream(&account),
        None => fetch(&url, MAX_LIST).and_then(|data| formats::parse(&data)),
    };
    match list {
        Ok(list) => db.replace_channels(id, &list),
        Err(e) => {
            db.set_subscription_error(id, &e.to_string())?;
            Err(e)
        }
    }
}

/// Asks an Xtream Codes panel for its account and live channels.
pub fn fetch_xtream(account: &formats::xtream::Account) -> Result<ChannelList> {
    let ask = |action: Option<&str>| -> Result<serde_json::Value> {
        let data = fetch(&account.api_url(action), MAX_LIST)?;
        serde_json::from_slice(&data).map_err(|_| {
            Error::Account(tr("the server did not answer like an Xtream Codes panel").into())
        })
    };
    let formats = formats::xtream::check_account(&ask(None)?)?;
    // Without categories the channels are still usable, just ungrouped.
    let categories = ask(Some("get_live_categories")).unwrap_or(serde_json::Value::Null);
    let streams = ask(Some("get_live_streams"))?;
    let list = formats::xtream::build_list(account, &formats, &categories, &streams);
    if list.channels.is_empty() {
        Err(Error::Empty)
    } else {
        Ok(list)
    }
}

/// Downloads the missing logos with a few threads in parallel.
/// Returns how many were downloaded successfully.
pub fn fetch_missing_logos(db_path: &Path, workers: usize) -> Result<usize> {
    let db = Database::open(db_path)?;
    let queue = Arc::new(Mutex::new(db.missing_logo_urls()?));
    let (tx, rx) = mpsc::channel::<(String, Option<Vec<u8>>)>();

    let handles: Vec<_> = (0..workers.max(1))
        .map(|_| {
            let queue = queue.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                loop {
                    let Some(url) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let data = fetch(&url, MAX_LOGO).ok().filter(|d| looks_like_image(d));
                    if tx.send((url, data)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(tx);

    // A single database writer.
    let mut ok = 0;
    for (url, data) in rx {
        ok += usize::from(data.is_some());
        db.store_logo(&url, data.as_deref())?;
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(ok)
}

/// Downloads a single logo and stores it (the failure too, so it is not retried).
/// Used for loading the visible rows on demand.
pub fn fetch_logo(db_path: &Path, url: &str) -> Result<Option<Vec<u8>>> {
    let data = fetch(url, MAX_LOGO).ok().filter(|d| looks_like_image(d));
    Database::open(db_path)?.store_logo(url, data.as_deref())?;
    Ok(data)
}

fn looks_like_image(d: &[u8]) -> bool {
    d.starts_with(b"\x89PNG")
        || d.starts_with(b"\xff\xd8\xff")
        || d.starts_with(b"GIF8")
        || (d.starts_with(b"RIFF") && d.get(8..12) == Some(b"WEBP"))
        || d.starts_with(b"<svg")
        || d.starts_with(b"<?xml")
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
