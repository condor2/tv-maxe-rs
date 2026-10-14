//! Parser tolerant pentru M3U „extins”:
//!
//! ```text
//! #EXTM3U url-tvg="https://example.org/epg.xml"
//! #EXTINF:-1 tvg-id="TVR1.ro" tvg-logo="https://…/tvr1.png" group-title="Generaliste",TVR 1
//! https://example.org/tvr1/index.m3u8
//! ```
//!
//! Liniile neînțelese sunt ignorate, nu produc eroare.

use super::non_empty;
use crate::{Channel, ChannelList, Kind, Logo, StreamOptions};

pub fn parse_m3u(input: &str) -> ChannelList {
    let mut list = ChannelList::default();
    let mut pending: Option<Channel> = None;
    let mut pending_group: Option<String> = None;

    for raw in input.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTM3U") {
            let attrs = parse_attrs(rest);
            list.epg_url = attr(&attrs, "url-tvg")
                .or_else(|| attr(&attrs, "x-tvg-url"))
                .and_then(|s| non_empty(s.split(',').next().unwrap_or(s)));
            list.name = attr(&attrs, "name").and_then(non_empty);
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            push_pending(&mut list, &mut pending, &mut pending_group);
            pending = Some(parse_extinf(rest));
            pending_group = None;
        } else if let Some(g) = line.strip_prefix("#EXTGRP:") {
            pending_group = non_empty(g);
        } else if line.starts_with('#') {
            // #EXTVLCOPT:http-referrer=… / http-user-agent=… (stil VLC);
            // restul (#KODIPROP etc.) e ignorat.
            if let (Some(opt), Some(ch)) = (line.strip_prefix("#EXTVLCOPT:"), pending.as_mut())
                && let Some((k, v)) = opt.split_once('=')
            {
                match k.trim().to_ascii_lowercase().as_str() {
                    "http-referrer" | "http-referer" => ch.http.referrer = non_empty(v),
                    "http-user-agent" => ch.http.user_agent = non_empty(v),
                    _ => {}
                }
            }
        } else if let Some(ch) = pending.as_mut() {
            // Mai multe URL-uri după același #EXTINF sunt variante ale
            // aceluiași canal, încercate în ordinea din listă.
            if !ch.urls.iter().any(|url| url.as_str() == line) {
                ch.urls.push(line.to_string());
            }
        } else {
            let mut ch = Channel::new(Kind::Tv, name_from_url(line), line);
            ch.group = pending_group.take();
            if ch.group.as_deref().is_some_and(is_radio_group) {
                ch.kind = Kind::Radio;
            }
            list.channels.push(ch);
        }
    }
    push_pending(&mut list, &mut pending, &mut pending_group);
    list
}

fn push_pending(
    list: &mut ChannelList,
    pending: &mut Option<Channel>,
    pending_group: &mut Option<String>,
) {
    let Some(mut ch) = pending.take() else {
        return;
    };
    if ch.urls.is_empty() {
        pending_group.take();
        return;
    }
    if ch.group.is_none() {
        ch.group = pending_group.take();
    }
    if ch.name.is_empty() {
        ch.name = name_from_url(&ch.urls[0]);
    }
    if ch.group.as_deref().is_some_and(is_radio_group) {
        ch.kind = Kind::Radio;
    }
    list.channels.push(ch);
}

fn is_radio_group(g: &str) -> bool {
    g.to_lowercase().contains("radio")
}

fn parse_extinf(rest: &str) -> Channel {
    let (head, name) = split_outside_quotes(rest);
    let attrs = parse_attrs(head);
    let radio = attr(&attrs, "radio").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    Channel {
        kind: if radio { Kind::Radio } else { Kind::Tv },
        name: name.trim().to_string(),
        urls: Vec::new(),
        group: attr(&attrs, "group-title").and_then(non_empty),
        guide_id: attr(&attrs, "tvg-id").and_then(non_empty),
        logo: attr(&attrs, "tvg-logo")
            .and_then(non_empty)
            .map_or(Logo::None, Logo::Url),
        http: StreamOptions {
            referrer: attr(&attrs, "http-referrer")
                .or_else(|| attr(&attrs, "http-referer"))
                .and_then(non_empty),
            user_agent: attr(&attrs, "http-user-agent").and_then(non_empty),
        },
    }
}

/// Prima virgulă din afara ghilimelelor separă atributele de nume.
/// Numele poate conține la rândul lui virgule („Digi 24, HD”).
fn split_outside_quotes(s: &str) -> (&str, &str) {
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => return (&s[..i], &s[i + 1..]),
            _ => {}
        }
    }
    (s, "")
}

/// `cheie="valoare"` și `cheie=valoare`; durata (`-1`) e ignorată.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        let key = s[key_start..i].to_ascii_lowercase();
        i += 1;
        let value = if i < b.len() && b[i] == b'"' {
            i += 1;
            let start = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            let v = &s[start..i];
            i += 1;
            v
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            &s[start..i]
        };
        out.push((key, value.to_string()));
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn name_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"#EXTM3U url-tvg="https://epg.example/ro.xml"
#EXTINF:-1 tvg-id="TVR1.ro" tvg-logo="https://logo/tvr1.png" group-title="Generaliste",TVR 1
https://example.org/tvr1/index.m3u8

#EXTINF:-1 tvg-id="Digi24.ro" group-title="Știri, Actualitate",Digi 24, HD
#EXTVLCOPT:http-user-agent=Mozilla
https://example.org/digi24.m3u8
#EXTINF:-1,Europa FM
#EXTGRP:Radio
http://radio.example/stream
#EXTINF:-1 radio="true",Kiss FM
http://radio.example/kiss
"#;

    #[test]
    fn header_and_attributes() {
        let p = parse_m3u(SAMPLE);
        assert_eq!(p.epg_url.as_deref(), Some("https://epg.example/ro.xml"));
        assert_eq!(p.channels.len(), 4);
        let tvr = &p.channels[0];
        assert_eq!(tvr.name, "TVR 1");
        assert_eq!(tvr.kind, Kind::Tv);
        assert_eq!(tvr.guide_id.as_deref(), Some("TVR1.ro"));
        assert_eq!(tvr.logo, Logo::Url("https://logo/tvr1.png".into()));
        assert_eq!(tvr.urls, vec!["https://example.org/tvr1/index.m3u8"]);
    }

    #[test]
    fn commas_in_quotes_and_name() {
        let d = &parse_m3u(SAMPLE).channels[1];
        assert_eq!(d.group.as_deref(), Some("Știri, Actualitate"));
        assert_eq!(d.name, "Digi 24, HD");
    }

    #[test]
    fn radio_detection() {
        let p = parse_m3u(SAMPLE);
        assert_eq!(p.channels[2].kind, Kind::Radio);
        assert_eq!(p.channels[3].kind, Kind::Radio);
    }

    #[test]
    fn http_options() {
        let p = parse_m3u(SAMPLE);
        assert_eq!(
            p.channels[1].http.user_agent.as_deref(),
            Some("Mozilla"),
            "#EXTVLCOPT"
        );
        let iptv_org = r#"#EXTM3U x-tvg-url="https://g/guide.xml.gz"
#EXTINF:-1 tvg-id="DisneyChannel.ro@SD" http-referrer="https://cool-tv.net/" http-user-agent="Mozilla/5.0 (X11) VLC/3.0.18" group-title="Moldova",Disney Channel (Romania)
https://x/disney.m3u8"#;
        let p = parse_m3u(iptv_org);
        assert_eq!(p.epg_url.as_deref(), Some("https://g/guide.xml.gz"));
        let c = &p.channels[0];
        assert_eq!(c.name, "Disney Channel (Romania)");
        assert_eq!(c.http.referrer.as_deref(), Some("https://cool-tv.net/"));
        assert_eq!(
            c.http.user_agent.as_deref(),
            Some("Mozilla/5.0 (X11) VLC/3.0.18")
        );
        assert_eq!(c.guide_id.as_deref(), Some("DisneyChannel.ro@SD"));
    }

    #[test]
    fn bare_urls_and_bom() {
        let p = parse_m3u("\u{feff}http://a/b/live.ts\nhttp://c/d/\n");
        assert_eq!(p.channels.len(), 2);
        assert_eq!(p.channels[0].name, "live.ts");
        assert_eq!(p.channels[1].name, "d");
    }

    #[test]
    fn multiple_urls_after_one_extinf_are_fallbacks() {
        let p = parse_m3u(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"tvr1.ro\",TVR 1\n\
             https://primary.example/tvr1.m3u8\n\
             https://backup.example/tvr1.m3u8\n\
             https://backup.example/tvr1.m3u8\n\
             #EXTINF:-1,Digi 24\n\
             https://example.org/digi24.m3u8\n",
        );
        assert_eq!(p.channels.len(), 2);
        assert_eq!(
            p.channels[0].urls,
            [
                "https://primary.example/tvr1.m3u8",
                "https://backup.example/tvr1.m3u8"
            ]
        );
        assert_eq!(p.channels[1].urls, ["https://example.org/digi24.m3u8"]);
    }
}
