//! Baza de date a aplicației (`~/.local/share/tvmaxe/tvmaxe.db`).
//!
//! Ține abonamentele, canalele lor (copie locală, deci aplicația pornește
//! și fără internet), cache-ul de logo-uri și setările.
//! Fiecare fir de execuție își deschide propria conexiune (WAL).

use crate::epg::{Guide, Programme, Wanted, normalize_name};
use crate::{Channel, ChannelList, Error, Kind, Logo, Result, StreamOptions};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// URL-ul rezervat listei locale („My channels” / „Canalele mele”).
pub const LOCAL_URL: &str = "local:";

const SCHEMA_VERSION: i64 = 3;

/// Versiunea 2: ghidul TV (EPG).
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

/// Versiunea 3: antete HTTP per canal (referrer, User-Agent).
const SCHEMA_V3: &str = "
ALTER TABLE channels ADD COLUMN referrer TEXT;
ALTER TABLE channels ADD COLUMN user_agent TEXT;
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
    logo            BLOB                -- logo inclus în listă
);
CREATE INDEX channels_by_sub ON channels(subscription_id, kind, position);
CREATE TABLE logos (
    url        TEXT PRIMARY KEY,
    data       BLOB,                    -- NULL = descărcare eșuată, nu reîncerca
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

    /// Numele afișat: lista locală are nume tradus („My channels” / „Canalele
    /// mele”), indiferent de limba în care a fost creată baza.
    pub fn display_name(&self) -> &str {
        if self.is_local() {
            crate::i18n::tr("My channels")
        } else {
            &self.name
        }
    }
}

/// Un canal salvat, gata de afișat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChannel {
    pub id: i64,
    pub subscription_id: i64,
    pub kind: Kind,
    pub name: String,
    pub urls: Vec<String>,
    pub group: Option<String>,
    /// `tvg-id` (M3U) sau `guide` (TV-Maxe): cheia pentru ghidul TV.
    pub guide_id: Option<String>,
    /// Logo inclus în listă sau descărcat anterior.
    pub logo: Option<Vec<u8>>,
    /// De unde se poate descărca logo-ul, dacă nu e încă în bază.
    pub logo_url: Option<String>,
    /// Descărcarea logo-ului a eșuat deja (nu se reîncearcă).
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
        // Migrări incrementale: o bază veche primește doar ce îi lipsește.
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
        const { assert!(SCHEMA_VERSION == 3, "adaugă migrarea pentru noua versiune") };
        Ok(Self { conn })
    }

    // ---------- Abonamente ----------

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

    /// Adaugă (sau reactivează) un abonament. Întoarce id-ul lui.
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

    /// Înlocuiește canalele unui abonament cu conținutul proaspăt descărcat.
    pub fn replace_channels(&mut self, sub_id: i64, list: &ChannelList) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM channels WHERE subscription_id = ?1", [sub_id])?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO channels
                   (subscription_id, kind, position, name, urls, grp, guide_id, logo_url, logo,
                    referrer, user_agent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                    ch.guide_id,
                    logo_url,
                    logo,
                    ch.http.referrer,
                    ch.http.user_agent,
                ])?;
            }
        }
        // Numele implicit (= URL-ul) e înlocuit cu cel declarat în listă
        // sau, dacă lista nu are nume, cu numele fișierului.
        let url: String = tx.query_row(
            "SELECT url FROM subscriptions WHERE id = ?1",
            [sub_id],
            |r| r.get(0),
        )?;
        let name = list.name.clone().unwrap_or_else(|| name_from_source(&url));
        tx.execute(
            "UPDATE subscriptions
             SET name = CASE WHEN name = url THEN ?2 ELSE name END,
                 epg_url = ?3, updated_at = unixepoch(), last_error = NULL
             WHERE id = ?1",
            params![sub_id, name, list.epg_url],
        )?;
        tx.commit()?;
        Ok(list.channels.len())
    }

    // ---------- Lista locală ----------

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

    /// Modifică un canal din lista locală (nume, tip, URL-uri). Canalele din
    /// abonamente nu se modifică: s-ar pierde la următoarea actualizare.
    /// Întoarce `false` dacă `id` nu e un canal local.
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

    /// Șterge un canal — doar din lista locală; abonamentele se rescriu la actualizare.
    pub fn delete_local_channel(&self, id: i64) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM channels WHERE id = ?1
               AND subscription_id = (SELECT id FROM subscriptions WHERE url = ?2)",
            params![id, LOCAL_URL],
        )?;
        Ok(n > 0)
    }

    /// Conținutul unui abonament ca listă (pentru export TV-Maxe `.db`).
    pub fn export_list(&self, sub_id: i64) -> Result<ChannelList> {
        let (name, epg_url): (String, Option<String>) = self.conn.query_row(
            "SELECT name, epg_url FROM subscriptions WHERE id = ?1",
            [sub_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT c.kind, c.name, c.urls, c.grp, c.guide_id, c.logo_url, coalesce(c.logo, l.data),
                    c.referrer, c.user_agent
             FROM channels c LEFT JOIN logos l ON l.url = c.logo_url
             WHERE c.subscription_id = ?1 ORDER BY c.kind = 'radio', c.position",
        )?;
        let channels = stmt
            .query_map([sub_id], |r| {
                let logo_url: Option<String> = r.get(5)?;
                let logo: Option<Vec<u8>> = r.get(6)?;
                Ok(Channel {
                    kind: Kind::parse(&r.get::<_, String>(0)?).unwrap_or(Kind::Tv),
                    name: r.get(1)?,
                    urls: parse_urls(&r.get::<_, String>(2)?),
                    group: r.get(3)?,
                    guide_id: r.get(4)?,
                    logo: match (logo, logo_url) {
                        (Some(b), _) => Logo::Embedded(b),
                        (None, Some(u)) => Logo::Url(u),
                        _ => Logo::None,
                    },
                    http: StreamOptions {
                        referrer: r.get(7)?,
                        user_agent: r.get(8)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ChannelList {
            name: Some(name),
            epg_url,
            channels,
        })
    }

    // ---------- Canale ----------

    /// Canalele de un anumit tip din abonamentele active, în ordinea listelor.
    pub fn channels(&self, kind: Kind) -> Result<Vec<StoredChannel>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.subscription_id, c.name, c.urls, c.grp,
                    coalesce(c.logo, l.data), s.url = ?2, c.guide_id,
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
                guide_id: r.get(7)?,
                logo_url: r.get(8)?,
                logo_failed: r.get(9)?,
                http: StreamOptions {
                    referrer: r.get(10)?,
                    user_agent: r.get(11)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- Ghid TV (EPG) ----------

    /// Adresele EPG declarate de abonamentele active (`url-tvg`, `epgurl`).
    pub fn epg_source_urls(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT epg_url FROM subscriptions
             WHERE enabled AND epg_url IS NOT NULL AND trim(epg_url) != ''",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Ce canale există în listele active, ca parserul să păstreze doar ce trebuie.
    pub fn epg_wanted(&self) -> Result<Wanted> {
        let mut stmt = self.conn.prepare(
            "SELECT c.guide_id, c.name FROM channels c
             JOIN subscriptions s ON s.id = c.subscription_id WHERE s.enabled",
        )?;
        let mut w = Wanted::default();
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            if let Some(id) = id.filter(|i| !i.trim().is_empty()) {
                let id = id.trim().to_lowercase();
                // iptv-org: „DisneyChannel.ro@SD” — ghidul folosește „DisneyChannel.ro”.
                if let Some((base, _)) = id.split_once('@') {
                    w.ids.insert(base.to_string());
                }
                w.ids.insert(id);
            }
            w.names.insert(normalize_name(&name));
        }
        Ok(w)
    }

    pub fn epg_fetched_at(&self, url: &str) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT fetched_at FROM epg_sources WHERE url = ?1",
                [url],
                |r| r.get(0),
            )
            .ok()
            .flatten()
    }

    pub fn set_epg_error(&self, url: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO epg_sources (url, last_error) VALUES (?1, ?2)
             ON CONFLICT(url) DO UPDATE SET last_error = excluded.last_error",
            params![url, error],
        )?;
        Ok(())
    }

    /// Înlocuiește tot ce venea dintr-o sursă EPG și șterge emisiunile
    /// terminate de peste două zile (față de `now`).
    pub fn replace_epg(&mut self, url: &str, guide: &Guide, now: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM epg_channels WHERE source = ?1", [url])?;
        tx.execute("DELETE FROM epg_programmes WHERE source = ?1", [url])?;
        {
            let mut ch =
                tx.prepare("INSERT INTO epg_channels (source, id, name) VALUES (?1, ?2, ?3)")?;
            for c in &guide.channels {
                for n in &c.names {
                    ch.execute(params![url, c.id, n])?;
                }
            }
            let mut pr = tx.prepare(
                "INSERT INTO epg_programmes (source, channel, start, stop, title, descr)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for p in &guide.programmes {
                pr.execute(params![url, p.channel, p.start, p.stop, p.title, p.desc])?;
            }
        }
        tx.execute(
            "INSERT INTO epg_sources (url, fetched_at, last_error) VALUES (?1, unixepoch(), NULL)
             ON CONFLICT(url) DO UPDATE SET fetched_at = unixepoch(), last_error = NULL",
            [url],
        )?;
        tx.execute(
            "DELETE FROM epg_programmes WHERE stop < ?1",
            [now - 2 * 86400],
        )?;
        tx.commit()?;
        Ok(guide.programmes.len())
    }

    /// Indexul pentru potrivirea canal ↔ ghid.
    pub fn epg_index(&self) -> Result<EpgIndex> {
        let mut idx = EpgIndex::default();
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT channel FROM epg_programmes")?;
        for id in stmt.query_map([], |r| r.get::<_, String>(0))? {
            idx.ids.insert(id?);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT name, id FROM epg_channels")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (name, id) = row?;
            idx.by_name.entry(name).or_insert(id);
        }
        Ok(idx)
    }

    /// Emisiunea curentă pe fiecare canal din ghid.
    pub fn epg_current(&self, now: i64) -> Result<HashMap<String, Programme>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel, start, stop, title, descr FROM epg_programmes
             WHERE start <= ?1 AND stop > ?1",
        )?;
        let mut out = HashMap::new();
        for p in stmt.query_map([now], programme_row)? {
            let p = p?;
            out.entry(p.channel.clone()).or_insert(p);
        }
        Ok(out)
    }

    /// Emisiunile unui canal care se suprapun cu intervalul [from, to).
    pub fn epg_range(&self, channel: &str, from: i64, to: i64) -> Result<Vec<Programme>> {
        // GROUP BY start: aceeași emisiune poate veni din mai multe surse.
        let mut stmt = self.conn.prepare(
            "SELECT channel, start, max(stop), title, descr FROM epg_programmes
             WHERE channel = ?1 AND stop > ?2 AND start < ?3
             GROUP BY start ORDER BY start",
        )?;
        let rows = stmt.query_map(params![channel, from, to], programme_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// (acum, următoarea) pe un canal.
    pub fn epg_now_next(
        &self,
        channel: &str,
        now: i64,
    ) -> Result<(Option<Programme>, Option<Programme>)> {
        let progs = self.epg_range(channel, now, now + 2 * 86400)?;
        let mut it = progs.into_iter();
        let first = it.next();
        match first {
            Some(p) if p.start <= now => Ok((Some(p), it.next())),
            other => Ok((None, other)),
        }
    }

    // ---------- Logo-uri ----------

    /// URL-uri de logo încă necăutate, din abonamentele active.
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

    // ---------- Setări ----------

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

fn programme_row(r: &rusqlite::Row) -> rusqlite::Result<Programme> {
    Ok(Programme {
        channel: r.get(0)?,
        start: r.get(1)?,
        stop: r.get(2)?,
        title: r.get(3)?,
        desc: r.get(4)?,
    })
}

/// Leagă un canal din liste de id-ul lui din ghid: întâi după `tvg-id`,
/// apoi după numele normalizat („Pro TV HD” ↔ „PRO TV”).
#[derive(Debug, Default, Clone)]
pub struct EpgIndex {
    ids: HashSet<String>,
    by_name: HashMap<String, String>,
}

impl EpgIndex {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn resolve(&self, guide_id: Option<&str>, name: &str) -> Option<String> {
        if let Some(id) = guide_id.map(|g| g.trim().to_lowercase()) {
            if self.ids.contains(&id) {
                return Some(id);
            }
            if let Some((base, _)) = id.split_once('@')
                && self.ids.contains(base)
            {
                return Some(base.to_string());
            }
        }
        self.by_name.get(&normalize_name(name)).cloned()
    }
}

fn parse_urls(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// `https://x.ro/liste/Romania.db?v=2` → `Romania`
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

/// URL-urile unui canal: fără spații, fără linii goale, fără duplicate,
/// în ordinea dată. Cel puțin unul e obligatoriu.
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
            epg_url: None,
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
        assert_eq!(s.name, "Romania", "numele din listă înlocuiește URL-ul");
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

        // A doua actualizare nu dublează canalele.
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
        assert_eq!(n, 0, "canalele se șterg odată cu abonamentul");
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

        // Eșecurile sunt reținute ca să nu fie reîncercate la fiecare pornire.
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
            "ordinea dată, fără duplicate"
        );
        assert!(
            db.update_local_channel(id, Kind::Radio, "Nou", &["  ".into()])
                .is_err()
        );

        // Un canal dintr-un abonament nu se editează (s-ar pierde la actualizare).
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
    fn epg_store_and_lookup() {
        use crate::epg::parse_xmltv;
        let mut db = Database::open_in_memory().unwrap();
        let id = db.add_subscription("x", "http://x").unwrap();
        let mut tvr = Channel::new(Kind::Tv, "TVR 1", "http://a");
        tvr.guide_id = Some("TVR1.ro".into());
        let protv = Channel::new(Kind::Tv, "Pro TV", "http://b");
        let list = ChannelList {
            name: None,
            epg_url: Some("http://epg/ro.xml.gz".into()),
            channels: vec![tvr, protv],
        };
        db.replace_channels(id, &list).unwrap();
        assert_eq!(db.epg_source_urls().unwrap(), ["http://epg/ro.xml.gz"]);

        let wanted = db.epg_wanted().unwrap();
        assert!(wanted.ids.contains("tvr1.ro") && wanted.names.contains("protv"));

        let now = 10_000_000;
        let xml = format!(
            r#"<tv><channel id="ProTV.ro"><display-name>PRO TV HD</display-name></channel>
            <programme start="{a}" stop="{b}" channel="TVR1.ro"><title>Acum</title></programme>
            <programme start="{b}" stop="{c}" channel="TVR1.ro"><title>Urmează</title></programme>
            <programme start="{a}" stop="{c}" channel="ProTV.ro"><title>Știri</title></programme>
            <programme start="{a}" stop="{c}" channel="Altul"><title>Nu</title></programme></tv>"#,
            a = ts(now - 600),
            b = ts(now + 600),
            c = ts(now + 3600)
        );
        let guide = parse_xmltv(xml.as_bytes(), Some(&wanted), None).unwrap();
        assert_eq!(
            db.replace_epg("http://epg/ro.xml.gz", &guide, now).unwrap(),
            3
        );
        // Reîmprospătarea nu dublează.
        db.replace_epg("http://epg/ro.xml.gz", &guide, now).unwrap();
        assert!(db.epg_fetched_at("http://epg/ro.xml.gz").is_some());

        let idx = db.epg_index().unwrap();
        let chans = db.channels(Kind::Tv).unwrap();
        let tvr_key = idx
            .resolve(chans[0].guide_id.as_deref(), &chans[0].name)
            .unwrap();
        let pro_key = idx
            .resolve(chans[1].guide_id.as_deref(), &chans[1].name)
            .unwrap();
        assert_eq!(
            (tvr_key.as_str(), pro_key.as_str()),
            ("tvr1.ro", "protv.ro")
        );

        let (cur, next) = db.epg_now_next(&tvr_key, now).unwrap();
        assert_eq!(cur.unwrap().title, "Acum");
        assert_eq!(next.unwrap().title, "Urmează");
        let current = db.epg_current(now).unwrap();
        assert_eq!(current[&pro_key].title, "Știri");
        assert_eq!(
            db.epg_range(&tvr_key, now - 3600, now + 7200)
                .unwrap()
                .len(),
            2
        );
    }

    /// Unix → format XMLTV în UTC.
    fn ts(t: i64) -> String {
        let (days, rem) = (t.div_euclid(86400), t.rem_euclid(86400));
        // civil_from_days (Hinnant)
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
            rem / 3600,
            rem / 60 % 60,
            rem % 60
        )
    }

    #[test]
    fn http_options_and_feed_suffix() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db.add_subscription("x", "http://x").unwrap();
        let mut ch = Channel::new(Kind::Tv, "Disney Channel (Romania)", "http://d");
        ch.guide_id = Some("DisneyChannel.ro@SD".into());
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

        let w = db.epg_wanted().unwrap();
        assert!(w.ids.contains("disneychannel.ro") && w.ids.contains("disneychannel.ro@sd"));
        let guide = crate::epg::parse_xmltv(
            br#"<tv><programme start="20261014200000 +0000" stop="20261014210000 +0000" channel="DisneyChannel.ro"><title>Desene</title></programme></tv>"#,
            Some(&w),
            None,
        )
        .unwrap();
        db.replace_epg("g", &guide, 0).unwrap();
        let idx = db.epg_index().unwrap();
        assert_eq!(
            idx.resolve(s.guide_id.as_deref(), &s.name).as_deref(),
            Some("disneychannel.ro")
        );
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
        assert_eq!(db.subscriptions().unwrap()[0].name, "veche", "datele rămân");
        assert!(
            db.epg_source_urls().unwrap().is_empty(),
            "tabelele EPG există"
        );
        let v: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
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
