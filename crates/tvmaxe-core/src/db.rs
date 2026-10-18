//! The application database (`~/.local/share/tvmaxe/tvmaxe.db`).
//!
//! Holds the subscriptions, their channels (a local copy, so the application
//! also starts without internet), the logo cache and the settings.
//! Every thread opens its own connection (WAL).

use crate::recording::{Recording, Repeat, Status};
use crate::{Channel, ChannelList, Error, Kind, Logo, Result, StreamOptions};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// The URL reserved for the local list ("My channels").
pub const LOCAL_URL: &str = "local:";

const SCHEMA_VERSION: i64 = 6;

/// Version 2: the TV guide (removed again in version 6; kept so that
/// every old database goes through the same steps).
const SCHEMA_V2: &str = "
CREATE TABLE epg_sources (
    url        TEXT PRIMARY KEY,
    fetched_at INTEGER,
    last_error TEXT
);
CREATE TABLE epg_channels (
    source TEXT NOT NULL,
    id     TEXT NOT NULL,        -- id XMLTV, litere mici
    name   TEXT NOT NULL         -- nume normalizat
);
CREATE INDEX epg_channels_by_source ON epg_channels(source);
CREATE TABLE epg_programmes (
    source  TEXT    NOT NULL,
    channel TEXT    NOT NULL,    -- id XMLTV, litere mici
    start   INTEGER NOT NULL,
    stop    INTEGER NOT NULL,
    title   TEXT    NOT NULL,
    descr   TEXT
);
CREATE INDEX epg_programmes_by_channel ON epg_programmes(channel, start);
CREATE INDEX epg_programmes_by_source ON epg_programmes(source);
";

/// Version 3: per-channel HTTP headers (referrer, User-Agent).
const SCHEMA_V3: &str = "
ALTER TABLE channels ADD COLUMN referrer TEXT;
ALTER TABLE channels ADD COLUMN user_agent TEXT;
";

/// Version 4: recordings (scheduled, running and finished).
const SCHEMA_V4: &str = "
CREATE TABLE recordings (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL DEFAULT '',
    channel    TEXT NOT NULL,
    kind       TEXT NOT NULL,
    urls       TEXT NOT NULL,              -- JSON array, in order of preference
    referrer   TEXT,
    user_agent TEXT,
    start      INTEGER NOT NULL,           -- Unix seconds
    stop       INTEGER,                    -- NULL = until stopped by hand
    repeat     TEXT NOT NULL DEFAULT 'once',
    status     TEXT NOT NULL DEFAULT 'scheduled',
    files      TEXT NOT NULL DEFAULT '[]', -- JSON array of written files
    error      TEXT
);
CREATE INDEX recordings_by_start ON recordings(start);
";

/// Version 5: favourite channels. Kept apart from `channels`, whose rows are
/// rewritten on every list update: a favourite is found again by its first URL
/// (or, if the URL changed, by its name).
const SCHEMA_V5: &str = "
CREATE TABLE favourites (
    kind     TEXT NOT NULL,
    url      TEXT NOT NULL,
    name     TEXT NOT NULL,
    added_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (kind, url)
);
";

/// Version 6: the TV guide is gone, so are its tables, columns and settings.
const SCHEMA_V6: &str = "
DROP TABLE IF EXISTS epg_programmes;
DROP TABLE IF EXISTS epg_channels;
DROP TABLE IF EXISTS epg_sources;
ALTER TABLE subscriptions DROP COLUMN epg_url;
ALTER TABLE channels DROP COLUMN guide_id;
DELETE FROM settings WHERE key LIKE '%epg%';
";

const SCHEMA: &str = "
CREATE TABLE subscriptions (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    url         TEXT    NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1,
    epg_url     TEXT,
    updated_at  INTEGER,
    last_error  TEXT
);
CREATE TABLE channels (
    id              INTEGER PRIMARY KEY,
    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    kind            TEXT    NOT NULL CHECK (kind IN ('tv', 'radio')),
    position        INTEGER NOT NULL,
    name            TEXT    NOT NULL,
    urls            TEXT    NOT NULL,   -- array JSON
    grp             TEXT,
    guide_id        TEXT,
    logo_url        TEXT,
    logo            BLOB                -- logo embedded in the list
);
CREATE INDEX channels_by_sub ON channels(subscription_id, kind, position);
CREATE TABLE logos (
    url        TEXT PRIMARY KEY,
    data       BLOB,                    -- NULL = download failed, do not retry
    fetched_at INTEGER NOT NULL
);
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub updated_at: Option<i64>,
    pub last_error: Option<String>,
    pub tv_count: i64,
    pub radio_count: i64,
}

impl Subscription {
    pub fn is_local(&self) -> bool {
        self.url == LOCAL_URL
    }

    /// The address for showing: the password of an Xtream account is hidden.
    pub fn display_url(&self) -> String {
        crate::formats::xtream::mask_password(&self.url)
    }

    /// Display name: the local list gets a translated name ("My channels"),
    /// whatever language the database was created in.
    pub fn display_name(&self) -> &str {
        if self.is_local() {
            crate::i18n::tr("My channels")
        } else {
            &self.name
        }
    }
}

/// A stored channel, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChannel {
    pub id: i64,
    pub subscription_id: i64,
    pub kind: Kind,
    pub name: String,
    pub urls: Vec<String>,
    pub group: Option<String>,
    /// Logo embedded in the list or downloaded earlier.
    pub logo: Option<Vec<u8>>,
    /// Where the logo can be downloaded from, if it is not in the database yet.
    pub logo_url: Option<String>,
    /// Downloading the logo already failed (not retried).
    pub logo_failed: bool,
    pub http: StreamOptions,
    pub is_local: bool,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // Incremental migrations: an old database only gets what it is missing.
        if version < 1 {
            conn.execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = 1; COMMIT;"))?;
        }
        if version < 2 {
            conn.execute_batch(&format!(
                "BEGIN; {SCHEMA_V2} PRAGMA user_version = 2; COMMIT;"
            ))?;
        }
        if version < 3 {
            conn.execute_batch(&format!(
                "BEGIN; {SCHEMA_V3} PRAGMA user_version = 3; COMMIT;"
            ))?;
        }
        if version < 4 {
            conn.execute_batch(&format!(
                "BEGIN; {SCHEMA_V4} PRAGMA user_version = 4; COMMIT;"
            ))?;
        }
        if version < 5 {
            conn.execute_batch(&format!(
                "BEGIN; {SCHEMA_V5} PRAGMA user_version = 5; COMMIT;"
            ))?;
        }
        if version < 6 {
            conn.execute_batch(&format!(
                "BEGIN; {SCHEMA_V6} PRAGMA user_version = 6; COMMIT;"
            ))?;
        }
        const { assert!(SCHEMA_VERSION == 6, "add the migration for the new version") };
        Ok(Self { conn })
    }

    // ---------- Subscriptions ----------

    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.name, s.url, s.enabled, s.updated_at, s.last_error,
                    (SELECT count(*) FROM channels c WHERE c.subscription_id = s.id AND c.kind = 'tv'),
                    (SELECT count(*) FROM channels c WHERE c.subscription_id = s.id AND c.kind = 'radio')
             FROM subscriptions s
             ORDER BY s.url = ?1, s.id",
        )?;
        let rows = stmt.query_map([LOCAL_URL], |r| {
            Ok(Subscription {
                id: r.get(0)?,
                name: r.get(1)?,
                url: r.get(2)?,
                enabled: r.get(3)?,
                updated_at: r.get(4)?,
                last_error: r.get(5)?,
                tv_count: r.get(6)?,
                radio_count: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn subscription_url(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT url FROM subscriptions WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Adds (or re-enables) a subscription. Returns its id.
    pub fn add_subscription(&self, name: &str, url: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "INSERT INTO subscriptions (name, url) VALUES (?1, ?2)
             ON CONFLICT(url) DO UPDATE SET enabled = 1
             RETURNING id",
            params![name, url],
            |r| r.get(0),
        )?)
    }

    pub fn set_subscription_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE subscriptions SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn remove_subscription(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM subscriptions WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_subscription_error(&self, id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE subscriptions SET last_error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    /// Replaces a subscription's channels with the freshly downloaded content.
    pub fn replace_channels(&mut self, sub_id: i64, list: &ChannelList) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM channels WHERE subscription_id = ?1", [sub_id])?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO channels
                   (subscription_id, kind, position, name, urls, grp, logo_url, logo,
                    referrer, user_agent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for (pos, ch) in list.channels.iter().enumerate() {
                let (logo_url, logo) = match &ch.logo {
                    Logo::None => (None, None),
                    Logo::Url(u) => (Some(u.as_str()), None),
                    Logo::Embedded(b) => (None, Some(b.as_slice())),
                };
                ins.execute(params![
                    sub_id,
                    ch.kind.as_str(),
                    pos as i64,
                    ch.name,
                    serde_json::to_string(&ch.urls).expect("Vec<String> → JSON"),
                    ch.group,
                    logo_url,
                    logo,
                    ch.http.referrer,
                    ch.http.user_agent,
                ])?;
            }
        }
        // The default name (= the URL) is replaced with the one declared in the list
        // or, if the list has no name, with the file name.
        let url: String = tx.query_row(
            "SELECT url FROM subscriptions WHERE id = ?1",
            [sub_id],
            |r| r.get(0),
        )?;
        let name = list.name.clone().unwrap_or_else(|| name_from_source(&url));
        tx.execute(
            "UPDATE subscriptions
             SET name = CASE WHEN name = url THEN ?2 ELSE name END,
                 updated_at = unixepoch(), last_error = NULL
             WHERE id = ?1",
            params![sub_id, name],
        )?;
        tx.commit()?;
        Ok(list.channels.len())
    }

    // ---------- Local list ----------

    pub fn local_list_id(&self) -> Result<i64> {
        self.add_subscription("My channels", LOCAL_URL)
    }

    pub fn add_local_channel(&self, kind: Kind, name: &str, url: &str) -> Result<i64> {
        self.add_local_channel_urls(kind, name, &[url.to_string()])
    }

    pub fn add_local_channel_urls(&self, kind: Kind, name: &str, urls: &[String]) -> Result<i64> {
        let clean = clean_urls(urls)?;
        let sub = self.local_list_id()?;
        Ok(self.conn.query_row(
            "INSERT INTO channels (subscription_id, kind, position, name, urls)
             VALUES (?1, ?2,
                     (SELECT coalesce(max(position), -1) + 1 FROM channels WHERE subscription_id = ?1),
                     ?3, ?4)
             RETURNING id",
            params![sub, kind.as_str(), name, serde_json::to_string(&clean).unwrap()],
            |r| r.get(0),
        )?)
    }

    /// Changes a channel in the local list (name, type, URLs). Subscription
    /// channels are not changed: the next update would overwrite them.
    /// Returns `false` if `id` is not a local channel.
    pub fn update_local_channel(
        &self,
        id: i64,
        kind: Kind,
        name: &str,
        urls: &[String],
    ) -> Result<bool> {
        let clean = clean_urls(urls)?;
        let n = self.conn.execute(
            "UPDATE channels SET kind = ?2, name = ?3, urls = ?4
             WHERE id = ?1
               AND subscription_id = (SELECT id FROM subscriptions WHERE url = ?5)",
            params![
                id,
                kind.as_str(),
                name,
                serde_json::to_string(&clean).unwrap(),
                LOCAL_URL
            ],
        )?;
        Ok(n > 0)
    }

    /// Deletes a channel — only from the local list; subscriptions are rewritten on update.
    pub fn delete_local_channel(&self, id: i64) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM channels WHERE id = ?1
               AND subscription_id = (SELECT id FROM subscriptions WHERE url = ?2)",
            params![id, LOCAL_URL],
        )?;
        Ok(n > 0)
    }

    /// A subscription's content as a list (for the TV-Maxe `.db` export).
    pub fn export_list(&self, sub_id: i64) -> Result<ChannelList> {
        let name: String = self.conn.query_row(
            "SELECT name FROM subscriptions WHERE id = ?1",
            [sub_id],
            |r| r.get(0),
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT c.kind, c.name, c.urls, c.grp, c.logo_url, coalesce(c.logo, l.data),
                    c.referrer, c.user_agent
             FROM channels c LEFT JOIN logos l ON l.url = c.logo_url
             WHERE c.subscription_id = ?1 ORDER BY c.kind = 'radio', c.position",
        )?;
        let channels = stmt
            .query_map([sub_id], |r| {
                let logo_url: Option<String> = r.get(4)?;
                let logo: Option<Vec<u8>> = r.get(5)?;
                Ok(Channel {
                    kind: Kind::parse(&r.get::<_, String>(0)?).unwrap_or(Kind::Tv),
                    name: r.get(1)?,
                    urls: parse_urls(&r.get::<_, String>(2)?),
                    group: r.get(3)?,
                    logo: match (logo, logo_url) {
                        (Some(b), _) => Logo::Embedded(b),
                        (None, Some(u)) => Logo::Url(u),
                        _ => Logo::None,
                    },
                    http: StreamOptions {
                        referrer: r.get(6)?,
                        user_agent: r.get(7)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ChannelList {
            name: Some(name),
            channels,
        })
    }

    // ---------- Channels ----------

    /// Channels of one kind from the enabled subscriptions, in list order.
    pub fn channels(&self, kind: Kind) -> Result<Vec<StoredChannel>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.subscription_id, c.name, c.urls, c.grp,
                    coalesce(c.logo, l.data), s.url = ?2,
                    c.logo_url, l.url IS NOT NULL AND l.data IS NULL, c.referrer, c.user_agent
             FROM channels c
             JOIN subscriptions s ON s.id = c.subscription_id
             LEFT JOIN logos l ON l.url = c.logo_url
             WHERE s.enabled AND c.kind = ?1
             ORDER BY s.url = ?2, s.id, c.position",
        )?;
        let rows = stmt.query_map(params![kind.as_str(), LOCAL_URL], |r| {
            Ok(StoredChannel {
                id: r.get(0)?,
                subscription_id: r.get(1)?,
                kind,
                name: r.get(2)?,
                urls: parse_urls(&r.get::<_, String>(3)?),
                group: r.get(4)?,
                logo: r.get(5)?,
                is_local: r.get(6)?,
                logo_url: r.get(7)?,
                logo_failed: r.get(8)?,
                http: StreamOptions {
                    referrer: r.get(9)?,
                    user_agent: r.get(10)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- Logos ----------

    /// Logo URLs not looked up yet, from the enabled subscriptions.
    pub fn missing_logo_urls(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT c.logo_url FROM channels c
             JOIN subscriptions s ON s.id = c.subscription_id
             WHERE s.enabled AND c.logo IS NULL AND c.logo_url IS NOT NULL
               AND c.logo_url NOT IN (SELECT url FROM logos)",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn store_logo(&self, url: &str, data: Option<&[u8]>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO logos (url, data, fetched_at) VALUES (?1, ?2, unixepoch())",
            params![url, data],
        )?;
        Ok(())
    }

    // ---------- Favourites ----------

    /// The favourites of one kind: (first URL, name).
    pub fn favourites(&self, kind: Kind) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT url, name FROM favourites WHERE kind = ?1 ORDER BY added_at, rowid")?;
        let rows = stmt.query_map([kind.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds or removes a favourite (identified by its first URL; removing also
    /// drops entries with the same name, which matched it by name).
    pub fn set_favourite(&self, kind: Kind, url: &str, name: &str, on: bool) -> Result<()> {
        if on {
            self.conn.execute(
                "INSERT INTO favourites (kind, url, name) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, url) DO UPDATE SET name = excluded.name",
                params![kind.as_str(), url, name],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM favourites WHERE kind = ?1 AND (url = ?2 OR name = ?3)",
                params![kind.as_str(), url, name],
            )?;
        }
        Ok(())
    }

    // ---------- Recordings ----------

    /// Stores a new recording and returns its id.
    pub fn add_recording(&self, r: &Recording) -> Result<i64> {
        Ok(self.conn.query_row(
            "INSERT INTO recordings
                 (title, channel, kind, urls, referrer, user_agent, start, stop, repeat, status, files, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             RETURNING id",
            params![
                r.title,
                r.channel,
                r.kind.as_str(),
                serde_json::to_string(&r.urls).unwrap(),
                r.http.referrer,
                r.http.user_agent,
                r.start,
                r.stop,
                r.repeat.as_str(),
                r.status.as_str(),
                serde_json::to_string(&r.files).unwrap(),
                r.error,
            ],
            |row| row.get(0),
        )?)
    }

    /// Saves every field of an existing recording. Returns `false` if it no
    /// longer exists.
    pub fn update_recording(&self, r: &Recording) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE recordings SET title = ?2, channel = ?3, kind = ?4, urls = ?5,
                 referrer = ?6, user_agent = ?7, start = ?8, stop = ?9, repeat = ?10,
                 status = ?11, files = ?12, error = ?13
             WHERE id = ?1",
            params![
                r.id,
                r.title,
                r.channel,
                r.kind.as_str(),
                serde_json::to_string(&r.urls).unwrap(),
                r.http.referrer,
                r.http.user_agent,
                r.start,
                r.stop,
                r.repeat.as_str(),
                r.status.as_str(),
                serde_json::to_string(&r.files).unwrap(),
                r.error,
            ],
        )?;
        Ok(n > 0)
    }

    pub fn delete_recording(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM recordings WHERE id = ?1", [id])?
            > 0)
    }

    pub fn recording(&self, id: i64) -> Result<Option<Recording>> {
        Ok(self.query_recordings("WHERE id = ?1", [id])?.pop())
    }

    /// Every recording, by start time.
    pub fn recordings(&self) -> Result<Vec<Recording>> {
        self.query_recordings("", [])
    }

    fn query_recordings(
        &self,
        filter: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<Recording>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, title, channel, kind, urls, referrer, user_agent, start, stop,
                    repeat, status, files, error
             FROM recordings {filter} ORDER BY start, id"
        ))?;
        let rows = stmt.query_map(args, |r| {
            let urls: String = r.get(4)?;
            let files: String = r.get(11)?;
            Ok(Recording {
                id: r.get(0)?,
                title: r.get(1)?,
                channel: r.get(2)?,
                kind: Kind::parse(&r.get::<_, String>(3)?).unwrap_or(Kind::Tv),
                urls: serde_json::from_str(&urls).unwrap_or_default(),
                http: StreamOptions {
                    referrer: r.get(5)?,
                    user_agent: r.get(6)?,
                },
                start: r.get(7)?,
                stop: r.get(8)?,
                repeat: Repeat::parse(&r.get::<_, String>(9)?).unwrap_or_default(),
                status: Status::parse(&r.get::<_, String>(10)?).unwrap_or(Status::Failed),
                files: serde_json::from_str(&files).unwrap_or_default(),
                error: r.get(12)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- Settings ----------

    pub fn setting<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| v.parse().ok())
    }

    pub fn set_setting(&self, key: &str, value: impl ToString) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }
}

fn parse_urls(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// `https://x.ro/lists/Romania.db?v=2` → `Romania`
pub fn name_from_source(source: &str) -> String {
    let path = source.split(['?', '#']).next().unwrap_or(source);
    let (host, path) = match path.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => ("", path),
    };
    let path = path.trim_end_matches('/');
    if path.is_empty() && !host.is_empty() {
        return host.to_string();
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    let stem = stem.replace('_', " ");
    if stem.trim().is_empty() {
        source.to_string()
    } else {
        stem
    }
}

/// A channel's URLs: trimmed, without empty lines or duplicates,
/// in the given order. At least one is required.
fn clean_urls(urls: &[String]) -> Result<Vec<&str>> {
    let mut clean = Vec::new();
    for url in urls.iter().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        if !clean.contains(&url) {
            clean.push(url);
        }
    }
    if clean.is_empty() {
        return Err(Error::Invalid(
            crate::i18n::tr("channel without URL").into(),
        ));
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChannelList {
        let mut tvr = Channel::new(Kind::Tv, "TVR 1", "http://a/1");
        tvr.urls.push("http://b/1".into());
        tvr.logo = Logo::Url("http://logo/tvr1.png".into());
        let mut radio = Channel::new(Kind::Radio, "Radio X", "http://r/x");
        radio.logo = Logo::Embedded(b"\x89PNGx".to_vec());
        ChannelList {
            name: Some("Romania".into()),
            channels: vec![tvr, radio],
        }
    }

    #[test]
    fn subscription_lifecycle() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db
            .add_subscription("http://x/list.db", "http://x/list.db")
            .unwrap();
        assert_eq!(
            db.add_subscription("alt nume", "http://x/list.db").unwrap(),
            id,
            "URL unic"
        );

        assert_eq!(db.replace_channels(id, &sample()).unwrap(), 2);
        let s = &db.subscriptions().unwrap()[0];
        assert_eq!(s.name, "Romania", "the list name replaces the URL");
        assert_eq!((s.tv_count, s.radio_count), (1, 1));
        assert!(s.updated_at.is_some());

        let tv = db.channels(Kind::Tv).unwrap();
        assert_eq!(tv.len(), 1);
        assert_eq!(tv[0].urls, vec!["http://a/1", "http://b/1"]);
        assert_eq!(tv[0].logo, None);
        assert_eq!(
            db.channels(Kind::Radio).unwrap()[0].logo.as_deref(),
            Some(&b"\x89PNGx"[..])
        );

        // A second update does not duplicate the channels.
        db.replace_channels(id, &sample()).unwrap();
        assert_eq!(db.channels(Kind::Tv).unwrap().len(), 1);

        db.set_subscription_enabled(id, false).unwrap();
        assert!(db.channels(Kind::Tv).unwrap().is_empty());

        db.remove_subscription(id).unwrap();
        assert!(db.subscriptions().unwrap().is_empty());
        let n: i64 = db
            .conn
            .query_row("SELECT count(*) FROM channels", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "channels are deleted with the subscription");
    }

    #[test]
    fn logo_cache() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db.add_subscription("x", "http://x").unwrap();
        db.replace_channels(id, &sample()).unwrap();
        assert_eq!(
            db.missing_logo_urls().unwrap(),
            vec!["http://logo/tvr1.png"]
        );

        db.store_logo("http://logo/tvr1.png", Some(b"img")).unwrap();
        assert!(db.missing_logo_urls().unwrap().is_empty());
        assert_eq!(
            db.channels(Kind::Tv).unwrap()[0].logo.as_deref(),
            Some(&b"img"[..])
        );

        // Failures are remembered so they are not retried on every start.
        db.store_logo("http://logo/tvr1.png", None).unwrap();
        assert!(db.missing_logo_urls().unwrap().is_empty());
    }

    #[test]
    fn local_list() {
        let db = Database::open_in_memory().unwrap();
        let urls = vec!["http://t/a".into(), "http://backup/a".into()];
        let a = db
            .add_local_channel_urls(Kind::Tv, "Test A", &urls)
            .unwrap();
        db.add_local_channel(Kind::Tv, "Test B", "http://t/b")
            .unwrap();
        let tv = db.channels(Kind::Tv).unwrap();
        assert_eq!(
            tv.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Test A", "Test B"]
        );
        assert_eq!(tv[0].urls, ["http://t/a", "http://backup/a"]);
        assert!(tv.iter().all(|c| c.is_local));
        assert!(db.delete_local_channel(a).unwrap());
        assert_eq!(db.channels(Kind::Tv).unwrap().len(), 1);
        assert_eq!(
            db.export_list(db.local_list_id().unwrap())
                .unwrap()
                .channels
                .len(),
            1
        );
    }

    #[test]
    fn favourites() {
        let db = Database::open_in_memory().unwrap();
        db.set_favourite(Kind::Tv, "http://a", "TVR 1", true)
            .unwrap();
        db.set_favourite(Kind::Tv, "http://b", "Pro TV", true)
            .unwrap();
        db.set_favourite(Kind::Radio, "http://r", "Radio", true)
            .unwrap();
        db.set_favourite(Kind::Tv, "http://a", "TVR 1 HD", true)
            .unwrap(); // renamed
        assert_eq!(
            db.favourites(Kind::Tv).unwrap(),
            [
                ("http://a".to_string(), "TVR 1 HD".to_string()),
                ("http://b".to_string(), "Pro TV".to_string())
            ]
        );
        // Removing a channel whose URL changed: matched by name.
        db.set_favourite(Kind::Tv, "http://new-b", "Pro TV", false)
            .unwrap();
        assert_eq!(db.favourites(Kind::Tv).unwrap().len(), 1);
        assert_eq!(db.favourites(Kind::Radio).unwrap().len(), 1);
    }

    #[test]
    fn recordings_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let mut r = Recording {
            id: 0,
            title: "Știri".into(),
            channel: "TVR 1".into(),
            kind: Kind::Tv,
            urls: vec!["http://a".into(), "http://b".into()],
            http: StreamOptions {
                referrer: Some("https://ref/".into()),
                user_agent: None,
            },
            start: 200,
            stop: Some(300),
            repeat: Repeat::Weekly,
            status: Status::Scheduled,
            files: Vec::new(),
            error: None,
        };
        r.id = db.add_recording(&r).unwrap();
        let mut manual = r.clone();
        manual.start = 100;
        manual.stop = None;
        manual.repeat = Repeat::Once;
        manual.id = db.add_recording(&manual).unwrap();

        let all = db.recordings().unwrap();
        assert_eq!(all, [manual.clone(), r.clone()], "ordered by start");

        r.status = Status::Done;
        r.files = vec!["/v/a.ts".into(), "/v/a (2).ts".into()];
        r.error = Some("x".into());
        assert!(db.update_recording(&r).unwrap());
        assert_eq!(db.recording(r.id).unwrap().unwrap(), r);
        assert!(db.delete_recording(r.id).unwrap());
        assert!(db.recording(r.id).unwrap().is_none());
        assert!(!db.update_recording(&r).unwrap());
    }

    #[test]
    fn edit_local_channel_only() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db
            .add_local_channel(Kind::Tv, "Vechi", "http://t/a")
            .unwrap();
        let urls: Vec<String> = vec![
            " http://t/b ".into(),
            "".into(),
            "http://t/a".into(),
            "http://t/b".into(),
        ];
        assert!(
            db.update_local_channel(id, Kind::Radio, "Nou", &urls)
                .unwrap()
        );
        assert!(
            db.channels(Kind::Tv).unwrap().is_empty(),
            "tipul s-a schimbat"
        );
        let radio = db.channels(Kind::Radio).unwrap();
        assert_eq!(radio[0].name, "Nou");
        assert_eq!(
            radio[0].urls,
            ["http://t/b", "http://t/a"],
            "given order, no duplicates"
        );
        assert!(
            db.update_local_channel(id, Kind::Radio, "Nou", &["  ".into()])
                .is_err()
        );

        // A subscription channel cannot be edited (the next update would overwrite it).
        let sub = db.add_subscription("Lista", "http://lista/x.m3u").unwrap();
        let list = ChannelList {
            channels: vec![Channel::new(Kind::Tv, "Din listă", "http://s/x")],
            ..Default::default()
        };
        db.replace_channels(sub, &list).unwrap();
        let other = db.channels(Kind::Tv).unwrap()[0].id;
        assert!(
            !db.update_local_channel(other, Kind::Tv, "X", &["http://y".into()])
                .unwrap()
        );
        assert_eq!(db.channels(Kind::Tv).unwrap()[0].name, "Din listă");
    }

    #[test]
    fn local_list_cannot_delete_subscription_channels() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db.add_subscription("x", "http://x").unwrap();
        db.replace_channels(id, &sample()).unwrap();
        let ch = db.channels(Kind::Tv).unwrap()[0].id;
        assert!(!db.delete_local_channel(ch).unwrap());
    }

    #[test]
    fn http_options_are_stored_and_exported() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db.add_subscription("x", "http://x").unwrap();
        let mut ch = Channel::new(Kind::Tv, "Disney Channel (Romania)", "http://d");
        ch.http.referrer = Some("https://ref/".into());
        ch.http.user_agent = Some("UA/1".into());
        ch.logo = Logo::Url("http://logo/d.png".into());
        let list = ChannelList {
            channels: vec![ch.clone()],
            ..Default::default()
        };
        db.replace_channels(id, &list).unwrap();
        let s = &db.channels(Kind::Tv).unwrap()[0];
        assert_eq!(s.http, ch.http);
        assert_eq!(s.logo_url.as_deref(), Some("http://logo/d.png"));
        assert!(!s.logo_failed);
        db.store_logo("http://logo/d.png", None).unwrap();
        assert!(db.channels(Kind::Tv).unwrap()[0].logo_failed);
        assert_eq!(db.export_list(id).unwrap().channels[0].http, ch.http);
    }

    #[test]
    fn migrates_v1_database() {
        let p = std::env::temp_dir().join(format!("tvmaxe-v1-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(&format!("{SCHEMA} PRAGMA user_version = 1;"))
                .unwrap();
            c.execute(
                "INSERT INTO subscriptions (name, url) VALUES ('veche', 'http://v')",
                [],
            )
            .unwrap();
        }
        let db = Database::open(&p).unwrap();
        assert_eq!(
            db.subscriptions().unwrap()[0].name,
            "veche",
            "the data stays"
        );
        let v: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        drop(db);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
    }

    #[test]
    fn migration_removes_the_old_tv_guide() {
        let p = std::env::temp_dir().join(format!("tvmaxe-v5-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(&format!(
                "{SCHEMA} {SCHEMA_V2} {SCHEMA_V3} {SCHEMA_V4} {SCHEMA_V5} PRAGMA user_version = 5;
                 INSERT INTO subscriptions (id, name, url, epg_url) VALUES (1, 'x', 'http://x', 'http://epg');
                 INSERT INTO channels (subscription_id, kind, position, name, urls, guide_id)
                     VALUES (1, 'tv', 0, 'TVR 1', '[\"http://a/1\"]', 'TVR1.ro');
                 INSERT INTO epg_programmes (source, channel, start, stop, title)
                     VALUES ('http://epg', 'tvr1.ro', 1, 2, 'Acum');
                 INSERT INTO settings VALUES ('pref.epg_urls', 'http://epg');
                 INSERT INTO settings VALUES ('volume', '70');"
            ))
            .unwrap();
        }
        let db = Database::open(&p).unwrap();
        let tv = db.channels(Kind::Tv).unwrap();
        assert_eq!(
            (tv.len(), tv[0].name.as_str()),
            (1, "TVR 1"),
            "channels stay"
        );
        assert_eq!(
            db.setting::<f64>("volume"),
            Some(70.0),
            "other settings stay"
        );
        assert_eq!(db.setting::<String>("pref.epg_urls"), None);
        let tables: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'epg%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "the guide tables are gone");
        drop(db);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
    }

    #[test]
    fn names_from_source() {
        assert_eq!(name_from_source("https://x.ro/l/Romania.db?v=2"), "Romania");
        assert_eq!(name_from_source("/home/u/United_Kingdom"), "United Kingdom");
        assert_eq!(name_from_source("https://x.ro/"), "x.ro");
    }

    #[test]
    fn settings() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.setting::<f64>("volume"), None);
        db.set_setting("volume", 75.5).unwrap();
        db.set_setting("volume", 80).unwrap();
        assert_eq!(db.setting::<f64>("volume"), Some(80.0));
    }
}
