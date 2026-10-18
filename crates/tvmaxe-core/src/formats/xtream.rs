//! Xtream Codes: the JSON API (`player_api.php`) that most IPTV providers
//! give their customers, with a server, a user name and a password instead of
//! a list file.
//!
//! A subscription is stored as its API address,
//! `http://server:port/player_api.php?username=…&password=…`, so it needs no
//! extra columns; [`Account::from_source`] recognises it again when the list
//! is updated. Only live channels are used (no films or series).

use super::{looks_like_stream_url, non_empty};
use crate::i18n::{fill, tr};
use crate::{Channel, ChannelList, Error, Kind, Logo, Result};
use serde_json::Value;
use std::collections::HashMap;

const API: &str = "player_api.php";
/// Endings a user may paste together with the server address.
const ENDINGS: &[&str] = &[
    "player_api.php",
    "get.php",
    "xmltv.php",
    "panel_api.php",
    "c",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `http://host:port` (or `https://…`), maybe with a path prefix; no
    /// trailing slash.
    pub server: String,
    pub username: String,
    pub password: String,
}

impl Account {
    /// An account from what a person types. The server may be written as
    /// `host:port`, `http://host:port/c/` or the address of any panel page.
    pub fn new(server: &str, username: &str, password: &str) -> Option<Self> {
        let (username, password) = (username.trim(), password.trim());
        if username.is_empty() || password.is_empty() {
            return None;
        }
        Some(Self {
            server: normalize_server(server)?,
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    /// The account behind a subscription address (`…/player_api.php?username=…&password=…`).
    pub fn from_source(source: &str) -> Option<Self> {
        let (path, query) = source.split_once('?')?;
        let server = path.strip_suffix(API)?.trim_end_matches('/');
        let get = |key: &str| {
            query.split('&').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                (k == key).then(|| pct_decode(v))
            })
        };
        Self::new(server, &get("username")?, &get("password")?)
    }

    /// What is stored as the subscription address.
    pub fn source_url(&self) -> String {
        self.api_url(None)
    }

    /// The API address, for the account data or for an `action`
    /// (`get_live_categories`, `get_live_streams`).
    pub fn api_url(&self, action: Option<&str>) -> String {
        let mut url = format!(
            "{}/{API}?username={}&password={}",
            self.server,
            pct_encode(&self.username),
            pct_encode(&self.password)
        );
        if let Some(action) = action {
            url.push_str("&action=");
            url.push_str(action);
        }
        url
    }

    /// `host:port`, for naming the subscription (no secrets in it).
    pub fn host(&self) -> String {
        let rest = self.server.split_once("://").map_or(&*self.server, |s| s.1);
        rest.split('/').next().unwrap_or(rest).to_string()
    }

    fn stream_url(&self, id: &str, extension: &str) -> String {
        format!(
            "{}/live/{}/{}/{id}.{extension}",
            self.server,
            pct_encode(&self.username),
            pct_encode(&self.password)
        )
    }
}

/// `scheme://host[:port][/prefix]`, with `http://` assumed, or `None` if
/// there is no host.
fn normalize_server(input: &str) -> Option<String> {
    let input = input.trim();
    let input = input.split(['?', '#']).next().unwrap_or(input);
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    let (scheme, rest) = with_scheme.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let mut rest = rest.trim_end_matches('/');
    while let Some((head, last)) = rest.rsplit_once('/') {
        if ENDINGS.contains(&last) {
            rest = head.trim_end_matches('/');
        } else {
            break;
        }
    }
    let host = rest.split('/').next().unwrap_or("");
    (!host.is_empty() && !host.contains(char::is_whitespace))
        .then(|| format!("{}://{rest}", scheme.to_ascii_lowercase()))
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16));
        match (bytes[i], hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b'+', ..) => {
                out.push(b' ');
                i += 1;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `s` with the password of an Xtream address hidden, for showing it.
pub fn mask_password(s: &str) -> String {
    let Some((head, query)) = s.split_once('?') else {
        return s.to_string();
    };
    let query: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some(("password", _)) => "password=••••".to_string(),
            _ => pair.to_string(),
        })
        .collect();
    format!("{head}?{}", query.join("&"))
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => non_empty(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn number(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Checks the answer of the panel to the account request and returns the
/// stream formats it allows, in the order we try them.
pub fn check_account(answer: &Value) -> Result<Vec<&'static str>> {
    let info = answer
        .get("user_info")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::Account(tr("the server is not an Xtream Codes panel").into()))?;
    if info.get("auth").and_then(number) == Some(0) {
        return Err(Error::Account(tr("wrong user name or password").into()));
    }
    if let Some(status) = info.get("status").and_then(text)
        && !status.eq_ignore_ascii_case("active")
    {
        return Err(Error::Account(fill(
            tr("the account is not active (%1)"),
            &[&status],
        )));
    }
    let allowed: Vec<String> = info
        .get("allowed_output_formats")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(text)
                .map(|s| s.to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    // MPEG-TS first (every panel has it), then HLS as the second source.
    let formats: Vec<&'static str> = ["ts", "m3u8"]
        .into_iter()
        .filter(|f| allowed.iter().any(|a| a == f))
        .collect();
    Ok(if formats.is_empty() {
        vec!["ts"]
    } else {
        formats
    })
}

/// The channels of the panel. `categories` and `streams` are the answers to
/// `get_live_categories` and `get_live_streams`; `formats` comes from
/// [`check_account`].
pub fn build_list(
    account: &Account,
    formats: &[&str],
    categories: &Value,
    streams: &Value,
) -> ChannelList {
    let names: HashMap<String, String> = categories
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((text(c.get("category_id")?)?, text(c.get("category_name")?)?)))
        .collect();
    let mut channels = Vec::new();
    for s in streams.as_array().into_iter().flatten() {
        let (Some(name), Some(id)) = (
            s.get("name").and_then(text),
            s.get("stream_id").and_then(text),
        ) else {
            continue;
        };
        if s.get("stream_type")
            .and_then(text)
            .is_some_and(|t| t != "live")
        {
            continue;
        }
        let group = s
            .get("category_id")
            .and_then(text)
            .and_then(|c| names.get(&c).cloned());
        let direct = s
            .get("direct_source")
            .and_then(text)
            .filter(|u| looks_like_stream_url(u));
        let urls = match direct {
            Some(u) => vec![u],
            None => formats.iter().map(|f| account.stream_url(&id, f)).collect(),
        };
        let kind = if group
            .as_deref()
            .is_some_and(|g| g.to_lowercase().contains("radio"))
        {
            Kind::Radio
        } else {
            Kind::Tv
        };
        channels.push(Channel {
            kind,
            name,
            urls,
            group,
            logo: s
                .get("stream_icon")
                .and_then(text)
                .filter(|u| u.starts_with("http"))
                .map_or(Logo::None, Logo::Url),
            http: Default::default(),
        });
    }
    ChannelList {
        name: Some(account.host()),
        channels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Account {
        Account::new("iptv.example:8080", "ana", "p@ss word").unwrap()
    }

    #[test]
    fn server_is_normalized() {
        for typed in [
            "iptv.example:8080",
            "http://iptv.example:8080/",
            "http://iptv.example:8080/c/",
            "http://iptv.example:8080/player_api.php?username=x&password=y",
            "  HTTP://iptv.example:8080/get.php  ",
        ] {
            assert_eq!(
                normalize_server(typed).as_deref(),
                Some("http://iptv.example:8080"),
                "{typed}"
            );
        }
        assert_eq!(
            normalize_server("https://tv.example/panel/c").as_deref(),
            Some("https://tv.example/panel")
        );
        assert_eq!(normalize_server("ftp://x"), None);
        assert_eq!(normalize_server("  "), None);
        assert_eq!(normalize_server("http://"), None);
    }

    #[test]
    fn needs_user_and_password() {
        assert!(Account::new("x.example", "", "p").is_none());
        assert!(Account::new("x.example", "u", " ").is_none());
    }

    #[test]
    fn source_url_round_trips_special_characters() {
        let a = account();
        let url = a.source_url();
        assert_eq!(
            url,
            "http://iptv.example:8080/player_api.php?username=ana&password=p%40ss%20word"
        );
        assert_eq!(Account::from_source(&url), Some(a));
        assert_eq!(Account::from_source("http://x.example/list.m3u"), None);
        assert_eq!(
            Account::from_source("http://x.example/get.php?username=a&password=b"),
            None,
            "get.php lists stay plain M3U"
        );
    }

    #[test]
    fn password_is_hidden_for_display() {
        assert_eq!(
            mask_password(&account().source_url()),
            "http://iptv.example:8080/player_api.php?username=ana&password=••••"
        );
        assert_eq!(
            mask_password("http://x.example/a.m3u"),
            "http://x.example/a.m3u"
        );
    }

    #[test]
    fn host_for_naming() {
        assert_eq!(account().host(), "iptv.example:8080");
        assert_eq!(
            Account::new("https://tv.example/panel", "u", "p")
                .unwrap()
                .host(),
            "tv.example"
        );
    }

    #[test]
    fn account_checks() {
        let ok = json!({"user_info": {"auth": 1, "status": "Active",
            "allowed_output_formats": ["m3u8", "ts"]}});
        assert_eq!(check_account(&ok).unwrap(), ["ts", "m3u8"]);
        let only_hls = json!({"user_info": {"auth": "1", "allowed_output_formats": ["m3u8"]}});
        assert_eq!(check_account(&only_hls).unwrap(), ["m3u8"]);
        let unknown = json!({"user_info": {"auth": 1}});
        assert_eq!(check_account(&unknown).unwrap(), ["ts"]);

        let wrong = json!({"user_info": {"auth": 0}});
        assert_eq!(
            check_account(&wrong).unwrap_err().to_string(),
            tr("wrong user name or password")
        );
        let expired = json!({"user_info": {"auth": 1, "status": "Expired"}});
        assert_eq!(
            check_account(&expired).unwrap_err().to_string(),
            fill(tr("the account is not active (%1)"), &[&"Expired"])
        );
        assert_eq!(
            check_account(&json!({"hello": 1})).unwrap_err().to_string(),
            tr("the server is not an Xtream Codes panel")
        );
    }

    #[test]
    fn channels_from_the_api() {
        let categories = json!([
            {"category_id": "1", "category_name": "Romania", "parent_id": 0},
            {"category_id": 2, "category_name": "Radio FM", "parent_id": 0}
        ]);
        let streams = json!([
            {"num": 1, "name": "Pro TV", "stream_type": "live", "stream_id": 101,
             "stream_icon": "http://logo.example/protv.png", "epg_channel_id": "protv.ro",
             "category_id": "1", "direct_source": ""},
            {"name": "Radio Uno", "stream_type": "live", "stream_id": "202", "stream_icon": "",
             "epg_channel_id": null, "category_id": 2},
            {"name": "Direct", "stream_type": "live", "stream_id": 303,
             "direct_source": "http://cdn.example/direct.m3u8"},
            {"name": "A film", "stream_type": "movie", "stream_id": 404},
            {"name": "", "stream_id": 505},
            {"stream_id": 606}
        ]);
        let list = build_list(&account(), &["ts", "m3u8"], &categories, &streams);
        assert_eq!(list.name.as_deref(), Some("iptv.example:8080"));
        assert_eq!(list.channels.len(), 3);

        let tv = &list.channels[0];
        assert_eq!((tv.name.as_str(), tv.kind), ("Pro TV", Kind::Tv));
        assert_eq!(
            tv.urls,
            [
                "http://iptv.example:8080/live/ana/p%40ss%20word/101.ts",
                "http://iptv.example:8080/live/ana/p%40ss%20word/101.m3u8"
            ]
        );
        assert_eq!(tv.group.as_deref(), Some("Romania"));
        assert_eq!(tv.logo, Logo::Url("http://logo.example/protv.png".into()));

        let radio = &list.channels[1];
        assert_eq!(radio.kind, Kind::Radio);
        assert_eq!(radio.logo, Logo::None);

        assert_eq!(list.channels[2].urls, ["http://cdn.example/direct.m3u8"]);
    }

    #[test]
    fn empty_answers_give_an_empty_list() {
        let list = build_list(&account(), &["ts"], &Value::Null, &json!([]));
        assert!(list.channels.is_empty());
        let list = build_list(&account(), &["ts"], &Value::Null, &json!({"error": 1}));
        assert!(list.channels.is_empty());
    }
}
