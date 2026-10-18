//! TV-Maxe lists (`.db`), compatible with TV-Maxe 0.x and TV-Maxe NG:
//!
//! ```sql
//! info(name, version, author, url, epgurl)
//! tv_channels(id, icon, name, streamurls, params, guide, audiochannels)
//! radio_channels(id, icon, name, streamurls, params)
//! ```
//!
//! `icon` is a base64-encoded PNG (TV-Maxe 0.x) or a raw blob (NG); `streamurls`
//! is a JSON array of URLs, tried in order.

use super::non_empty;
use crate::{Channel, ChannelList, Error, Kind, Logo, Result};
use base64::Engine;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, params};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn parse_tvmaxe_db_file(path: &Path) -> Result<ChannelList> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    read(&conn)
}

/// Downloaded lists arrive as bytes; SQLite needs a file.
pub(super) fn parse_tvmaxe_db_bytes(data: &[u8]) -> Result<ChannelList> {
    let temp = create_temp_db(data)?;
    parse_tvmaxe_db_file(&temp.path)
}

#[derive(Debug)]
struct TempDb {
    path: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn create_temp_db(data: &[u8]) -> Result<TempDb> {
    static N: AtomicU64 = AtomicU64::new(0);
    let first = N.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..128 {
        let path = std::env::temp_dir().join(format!(
            "tvmaxe-import-{}-{}.db",
            std::process::id(),
            first.wrapping_add(attempt)
        ));
        match create_temp_db_at(path, data) {
            Ok(temp) => return Ok(temp),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        crate::i18n::tr("could not create a unique temporary file for import"),
    )))
}

fn create_temp_db_at(path: PathBuf, data: &[u8]) -> Result<TempDb> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&path)?;
    let temp = TempDb { path };
    file.write_all(data)?;
    drop(file);
    Ok(temp)
}

fn read(conn: &Connection) -> Result<ChannelList> {
    let has_table = |t: &str| -> Result<bool> {
        Ok(conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1")?
            .exists([t])?)
    };
    if !has_table("tv_channels")? && !has_table("radio_channels")? {
        return Err(Error::UnknownFormat);
    }

    let mut list = ChannelList::default();
    if has_table("info")? {
        let rows = rows_as_maps(conn, "SELECT * FROM info LIMIT 1")?;
        if let Some(info) = rows.first() {
            list.name = text(info, "name").and_then(|s| non_empty(&s));
        }
    }
    for (kind, table) in [(Kind::Tv, "tv_channels"), (Kind::Radio, "radio_channels")] {
        if !has_table(table)? {
            continue;
        }
        for row in rows_as_maps(conn, &format!("SELECT * FROM {table}"))? {
            let Some(name) = text(&row, "name").and_then(|s| non_empty(&s)) else {
                continue;
            };
            let urls = text(&row, "streamurls")
                .map(|s| parse_urls(&s))
                .unwrap_or_default();
            if urls.is_empty() {
                continue;
            }
            list.channels.push(Channel {
                kind,
                name,
                urls,
                group: None,
                logo: decode_icon(row.iter().find(|(k, _)| k == "icon").map(|(_, v)| v)),
                http: Default::default(),
            });
        }
    }
    Ok(list)
}

type Row = Vec<(String, Value)>;

fn rows_as_maps(conn: &Connection, sql: &str) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let rows = stmt.query_map([], |r| {
        (0..names.len())
            .map(|i| Ok((names[i].clone(), r.get::<_, Value>(i)?)))
            .collect::<rusqlite::Result<Row>>()
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn text(row: &Row, col: &str) -> Option<String> {
    match row.iter().find(|(k, _)| k == col).map(|(_, v)| v)? {
        Value::Text(s) => Some(s.clone()),
        Value::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
        Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// `["url1","url2"]`, with a single plain URL as a fallback.
fn parse_urls(s: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<String>>(s) {
        Ok(v) => v.into_iter().filter_map(|u| non_empty(&u)).collect(),
        Err(_) => non_empty(s).into_iter().collect(),
    }
}

fn decode_icon(v: Option<&Value>) -> Logo {
    let bytes: &[u8] = match v {
        Some(Value::Blob(b)) => b,
        Some(Value::Text(t)) => t.as_bytes(),
        _ => return Logo::None,
    };
    if bytes.is_empty() {
        return Logo::None;
    }
    if is_image(bytes) {
        return Logo::Embedded(bytes.to_vec());
    }
    let cleaned: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    match base64::engine::general_purpose::STANDARD.decode(&cleaned) {
        Ok(img) if is_image(&img) => Logo::Embedded(img),
        _ => Logo::None,
    }
}

fn is_image(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG") || b.starts_with(b"\xff\xd8\xff") || b.starts_with(b"GIF8")
}

/// Writes a list in the TV-Maxe format (base64 icon, like TV-Maxe 0.x).
pub fn write_tvmaxe_db(path: &Path, list: &ChannelList) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let mut conn = Connection::open(path)?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE info (name text, version text, author text, url text, epgurl text);
         CREATE TABLE tv_channels (id text, icon blob, name text, streamurls text, params text, guide text, audiochannels text);
         CREATE TABLE radio_channels (id text, icon blob, name text, streamurls text, params text);",
    )?;
    // `epgurl` and `guide` stay in the file because the original TV-Maxe
    // expects those columns, but they are left empty.
    tx.execute(
        "INSERT INTO info VALUES (?1, '1.0', '', '', '')",
        params![list.name.as_deref().unwrap_or("Local")],
    )?;
    for (i, ch) in list.channels.iter().enumerate() {
        let icon = match &ch.logo {
            Logo::Embedded(b) => base64::engine::general_purpose::STANDARD.encode(b),
            _ => String::new(),
        };
        let urls = serde_json::to_string(&ch.urls).expect("Vec<String> → JSON");
        let id = format!("ch{i}");
        match ch.kind {
            Kind::Tv => tx.execute(
                "INSERT INTO tv_channels VALUES (?1, ?2, ?3, ?4, '{}', '', '[]')",
                params![id, icon, ch.name, urls],
            )?,
            Kind::Radio => tx.execute(
                "INSERT INTO radio_channels VALUES (?1, ?2, ?3, ?4, '{}')",
                params![id, icon, ch.name, urls],
            )?,
        };
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{Format, detect, parse};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

    /// Unique temporary path (tests run in parallel).
    fn tmp(name: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "tvmaxe-test-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A list built exactly like TV-Maxe 0.x: base64 icon as text, JSON streamurls.
    fn original_style_db() -> Vec<u8> {
        let p = tmp("orig.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE tv_channels (id, icon, name, streamurls, params, guide, audiochannels);
             CREATE TABLE radio_channels (id, icon, name, streamurls, params);
             CREATE TABLE info (name, version, author, url, epgurl);
             INSERT INTO info VALUES ('Romania', '1', 'x', '', 'http://epg/x.xml');",
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(PNG);
        c.execute(
            "INSERT INTO tv_channels VALUES ('tvr1', ?1, 'TVR 1', '[\"http://a/1\", \"http://b/1\"]', '{}', 'tvr1', '[]')",
            [b64],
        )
        .unwrap();
        c.execute(
            "INSERT INTO radio_channels VALUES ('r1', ?1, 'Radio România', '[\"http://r/1\"]', '{}')",
            [PNG],
        )
        .unwrap();
        drop(c);
        let data = std::fs::read(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        data
    }

    #[test]
    fn reads_original_tvmaxe_list() {
        let data = original_style_db();
        assert_eq!(detect(&data), Some(Format::TvMaxeDb));
        let l = parse(&data).unwrap();
        assert_eq!(l.name.as_deref(), Some("Romania"));
        assert_eq!(l.channels.len(), 2);

        let tvr = &l.channels[0];
        assert_eq!(tvr.kind, Kind::Tv);
        assert_eq!(tvr.urls, vec!["http://a/1", "http://b/1"]);
        assert_eq!(tvr.logo, Logo::Embedded(PNG.to_vec()), "icon base64");

        let radio = &l.channels[1];
        assert_eq!(radio.kind, Kind::Radio);
        assert_eq!(
            radio.logo,
            Logo::Embedded(PNG.to_vec()),
            "icon blob brut (NG)"
        );
    }

    #[test]
    fn export_roundtrip() {
        let mut l = parse(&original_style_db()).unwrap();
        l.name = Some("Export".into());
        let p = tmp("rt.db");
        write_tvmaxe_db(&p, &l).unwrap();
        let back = parse_tvmaxe_db_file(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn non_tvmaxe_sqlite_is_rejected() {
        let p = tmp("other.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch("CREATE TABLE x(a)")
            .unwrap();
        let data = std::fs::read(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(matches!(parse(&data), Err(Error::UnknownFormat)));
    }

    #[test]
    fn secure_temp_file_does_not_overwrite_an_existing_path() {
        let p = tmp("occupied.db");
        std::fs::write(&p, b"existing").unwrap();

        let err = create_temp_db_at(p.clone(), b"replacement").unwrap_err();
        assert!(matches!(
            err,
            Error::Io(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(std::fs::read(&p).unwrap(), b"existing");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn secure_temp_file_is_removed_on_drop() {
        let p = tmp("drop.db");
        let temp = create_temp_db_at(p.clone(), b"temporary").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"temporary");
        drop(temp);
        assert!(!p.exists());
    }
}
