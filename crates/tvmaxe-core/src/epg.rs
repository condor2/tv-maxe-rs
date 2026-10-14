//! TV guide in XMLTV format (the standard used by IPTV lists):
//!
//! ```xml
//! <tv>
//!   <channel id="TVR1.ro"><display-name>TVR 1</display-name></channel>
//!   <programme start="20261014200000 +0300" stop="20261014210000 +0300" channel="TVR1.ro">
//!     <title>Telejurnal</title><desc>…</desc>
//!   </programme>
//! </tv>
//! ```
//!
//! EPG files can be hundreds of MB (thousands of channels), so the parser keeps
//! only the channels that appear in the user's lists and only a few days.

use crate::i18n::{fill, tr};
use crate::{Error, Result};
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use std::collections::{HashMap, HashSet};
use std::io::Read;

/// Limit for XMLTV after decompression. Protects the application from gzip
/// archives that expand disproportionately in memory.
pub const MAX_DECOMPRESSED_EPG: u64 = 256 * 1024 * 1024;

/// A programme. `channel` is the XMLTV id, lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programme {
    pub channel: String,
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub desc: Option<String>,
}

impl Programme {
    /// How much of the programme has elapsed (0.0–1.0) at `now`.
    pub fn progress(&self, now: i64) -> f64 {
        let len = (self.stop - self.start).max(1) as f64;
        ((now - self.start) as f64 / len).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideChannel {
    /// Lowercased XMLTV id.
    pub id: String,
    /// Display names, normalised with [`normalize_name`].
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guide {
    pub channels: Vec<GuideChannel>,
    pub programmes: Vec<Programme>,
}

/// The channels we care about (from the user's lists).
#[derive(Debug, Clone, Default)]
pub struct Wanted {
    /// Lowercased `tvg-id`.
    pub ids: HashSet<String>,
    /// Nume normalizate.
    pub names: HashSet<String>,
}

/// A name comparable across sources: no diacritics, spaces, punctuation or
/// quality suffixes. "Pro TV HD" → `protv`, "TVR 1" → `tvr1`.
pub fn normalize_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ă' | 'â' | 'á' | 'à' | 'ä' => 'a',
            'î' | 'í' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            'é' | 'è' | 'ë' => 'e',
            'ö' | 'ó' | 'ő' => 'o',
            'ü' | 'ú' | 'ű' => 'u',
            other => other,
        })
        .filter(char::is_ascii_alphanumeric)
        .collect();
    loop {
        let before = out.len();
        for suffix in ["fhd", "uhd", "hd", "sd", "4k"] {
            if out.len() > suffix.len() + 1 && out.ends_with(suffix) {
                out.truncate(out.len() - suffix.len());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// `20261014203000 +0300` → Unix seconds. No time zone = UTC.
pub fn parse_xmltv_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let (digits, tz) = match s.split_once(' ') {
        Some((d, tz)) => (d, Some(tz.trim())),
        None => (s, None),
    };
    if digits.len() < 12 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = |a: usize, b: usize| digits.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d, h, mi) = (
        num(0, 4)?,
        num(4, 6)?,
        num(6, 8)?,
        num(8, 10)?,
        num(10, 12)?,
    );
    let sec = num(12, 14).unwrap_or(0);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let offset = match tz {
        Some(tz) if tz.len() == 5 && (tz.starts_with('+') || tz.starts_with('-')) => {
            let hh: i64 = tz[1..3].parse().ok()?;
            let mm: i64 = tz[3..5].parse().ok()?;
            let o = hh * 3600 + mm * 60;
            if tz.starts_with('-') { -o } else { o }
        }
        _ => 0,
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Decompresses `.xml.gz` (detected from the content).
pub fn decompress(data: Vec<u8>) -> Result<Vec<u8>> {
    decompress_limited(data, MAX_DECOMPRESSED_EPG)
}

fn decompress_limited(data: Vec<u8>, limit: u64) -> Result<Vec<u8>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let initial_capacity = data
            .len()
            .min(8 * 1024 * 1024)
            .min(usize::try_from(limit).unwrap_or(usize::MAX));
        let mut out = Vec::with_capacity(initial_capacity);
        flate2::read::MultiGzDecoder::new(&data[..])
            .take(limit.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| Error::Invalid(fill(tr("gzip archive: %1"), &[&e])))?;
        if u64::try_from(out.len()).unwrap_or(u64::MAX) > limit {
            return Err(Error::Invalid(fill(
                tr("the decompressed EPG exceeds the %1-byte limit"),
                &[&limit],
            )));
        }
        return Ok(out);
    }

    if u64::try_from(data.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::Invalid(fill(
            tr("the uncompressed EPG exceeds the %1-byte limit"),
            &[&limit],
        )));
    }
    Ok(data)
}

#[derive(PartialEq)]
enum Field {
    None,
    DisplayName,
    Title,
    Desc,
}

/// Parses XMLTV. `wanted` = only these channels (`None` = all),
/// `window` = only programmes overlapping the given interval.
pub fn parse_xmltv(
    data: &[u8],
    wanted: Option<&Wanted>,
    window: Option<(i64, i64)>,
) -> Result<Guide> {
    let text = String::from_utf8_lossy(data);
    // No trim_text: it would strip the spaces around entities ("A &amp; B" → "A&B").
    // The collected text is trimmed at the end.
    let mut reader = quick_xml::Reader::from_str(&text);

    let mut guide = Guide::default();
    let mut kept_ids: HashSet<String> = HashSet::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    let mut chan: Option<GuideChannel> = None;
    let mut prog: Option<Programme> = None;
    let mut field = Field::None;
    let mut buf = String::new();
    let mut root_seen = false;

    loop {
        let ev = match reader.read_event() {
            Ok(ev) => ev,
            // A file truncated at the end does not spoil what was already read.
            Err(e) if root_seen && !guide.programmes.is_empty() => {
                let _ = e;
                break;
            }
            Err(e) => return Err(Error::Invalid(format!("XMLTV: {e}"))),
        };
        match ev {
            Event::Start(e) => match e.name().as_ref() {
                "tv" => root_seen = true,
                "channel" => {
                    let id = attr(&e, "id").unwrap_or_default().to_lowercase();
                    chan = Some(GuideChannel {
                        id,
                        names: Vec::new(),
                    });
                }
                "display-name" if chan.is_some() => {
                    start_field(&mut field, &mut buf, Field::DisplayName)
                }
                "programme" => {
                    let channel = attr(&e, "channel").unwrap_or_default().to_lowercase();
                    let start = attr(&e, "start").and_then(|s| parse_xmltv_time(&s));
                    let stop = attr(&e, "stop")
                        .and_then(|s| parse_xmltv_time(&s))
                        .unwrap_or(0);
                    prog = start.map(|start| Programme {
                        channel,
                        start,
                        stop,
                        title: String::new(),
                        desc: None,
                    });
                }
                "title" if prog.as_ref().is_some_and(|p| p.title.is_empty()) => {
                    start_field(&mut field, &mut buf, Field::Title)
                }
                "desc" if prog.as_ref().is_some_and(|p| p.desc.is_none()) => {
                    start_field(&mut field, &mut buf, Field::Desc)
                }
                _ => {}
            },
            Event::Text(t) if field != Field::None => {
                buf.push_str(&t.xml_content(XmlVersion::Implicit1_0));
            }
            Event::CData(t) if field != Field::None => {
                buf.push_str(&t.into_inner());
            }
            Event::GeneralRef(r) if field != Field::None => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    buf.push(c);
                } else {
                    buf.push_str(match r.xml_content(XmlVersion::Implicit1_0).as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "display-name" if field == Field::DisplayName => {
                    if let Some(c) = chan.as_mut() {
                        let n = normalize_name(&buf);
                        if !n.is_empty() {
                            c.names.push(n);
                        }
                    }
                    field = Field::None;
                }
                "title" if field == Field::Title => {
                    if let Some(p) = prog.as_mut() {
                        p.title = buf.trim().to_string();
                    }
                    field = Field::None;
                }
                "desc" if field == Field::Desc => {
                    if let Some(p) = prog.as_mut() {
                        p.desc = Some(buf.trim().to_string()).filter(|d| !d.is_empty());
                    }
                    field = Field::None;
                }
                "channel" => {
                    if let Some(c) = chan.take()
                        && !c.id.is_empty()
                    {
                        seen_ids.insert(c.id.clone());
                        let keep = wanted.is_none_or(|w| {
                            w.ids.contains(&c.id) || c.names.iter().any(|n| w.names.contains(n))
                        });
                        if keep {
                            kept_ids.insert(c.id.clone());
                            guide.channels.push(c);
                        }
                    }
                }
                "programme" => {
                    if let Some(p) = prog.take() {
                        let keep_channel = kept_ids.contains(&p.channel)
                            || (!seen_ids.contains(&p.channel)
                                && wanted.is_none_or(|w| w.ids.contains(&p.channel)));
                        if keep_channel && !p.title.is_empty() && !p.channel.is_empty() {
                            guide.programmes.push(p);
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }

    if !root_seen {
        return Err(Error::Invalid(
            tr("not an XMLTV file (<tv> is missing)").into(),
        ));
    }
    fill_missing_stops(&mut guide.programmes);
    if let Some((from, to)) = window {
        guide.programmes.retain(|p| p.stop > from && p.start < to);
    }
    Ok(guide)
}

fn start_field(field: &mut Field, buf: &mut String, f: Field) {
    *field = f;
    buf.clear();
}

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Some sources omit `stop`: the programme lasts until the next one on the same channel.
fn fill_missing_stops(progs: &mut [Programme]) {
    progs.sort_by(|a, b| a.channel.cmp(&b.channel).then(a.start.cmp(&b.start)));
    let mut next_start: HashMap<&str, i64> = HashMap::new();
    // Walk backwards so we know when the next programme starts.
    let mut fixes = Vec::new();
    for (i, p) in progs.iter().enumerate().rev() {
        if p.stop <= p.start {
            let stop = next_start
                .get(p.channel.as_str())
                .copied()
                .unwrap_or(p.start + 3600);
            fixes.push((i, stop));
        }
        next_start.insert(&p.channel, p.start);
    }
    for (i, stop) in fixes {
        progs[i].stop = stop;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="test">
  <channel id="TVR1.ro"><display-name lang="ro">TVR 1</display-name></channel>
  <channel id="ProTV.ro"><display-name>Pro TV HD</display-name><display-name>PROTV</display-name></channel>
  <channel id="Altul.fr"><display-name>Altul</display-name></channel>
  <programme start="20261014200000 +0300" stop="20261014210000 +0300" channel="TVR1.ro">
    <title lang="ro">Telejurnal &amp; Sport</title>
    <title lang="en">News</title>
    <desc>Știri &#x219;i vreme</desc>
  </programme>
  <programme start="20261014210000 +0300" channel="TVR1.ro"><title>Film</title></programme>
  <programme start="20261014220000 +0300" stop="20261014230000 +0300" channel="TVR1.ro"><title><![CDATA[Noapte <bună>]]></title></programme>
  <programme start="20261014200000 +0300" stop="20261014203000 +0300" channel="ProTV.ro"><title>Știrile Pro TV</title></programme>
  <programme start="20261014200000 +0300" stop="20261014210000 +0300" channel="Altul.fr"><title>Ignorat</title></programme>
  <programme start="20261014200000 +0300" stop="20261014210000 +0300" channel="FaraCanal.ro"><title>Fără element channel</title></programme>
</tv>"#;

    #[test]
    fn time_parsing() {
        // 2026-10-14 17:00:00 UTC
        assert_eq!(
            parse_xmltv_time("20261014170000 +0000"),
            Some(1_791_997_200)
        );
        assert_eq!(
            parse_xmltv_time("20261014200000 +0300"),
            Some(1_791_997_200)
        );
        assert_eq!(
            parse_xmltv_time("20261014120000 -0500"),
            Some(1_791_997_200)
        );
        assert_eq!(parse_xmltv_time("202610141700"), Some(1_791_997_200));
        assert_eq!(parse_xmltv_time("19700101000000"), Some(0));
        assert_eq!(
            parse_xmltv_time("20240229120000 +0000"),
            Some(1_709_208_000),
            "an bisect"
        );
        assert_eq!(parse_xmltv_time("2026"), None);
        assert_eq!(parse_xmltv_time("20261332000000"), None);
    }

    #[test]
    fn names() {
        assert_eq!(normalize_name("Pro TV HD"), "protv");
        assert_eq!(normalize_name("TVR 1"), "tvr1");
        assert_eq!(normalize_name("Digi 24 FHD"), "digi24");
        assert_eq!(normalize_name("Antena 3 CNN"), "antena3cnn");
        assert_eq!(normalize_name("Știrile Țării"), "stiriletarii");
        assert_eq!(normalize_name("HD"), "hd", "does not empty the name");
    }

    #[test]
    fn parses_everything_without_filter() {
        let g = parse_xmltv(SAMPLE.as_bytes(), None, None).unwrap();
        assert_eq!(g.channels.len(), 3);
        assert_eq!(g.channels[1].names, vec!["protv", "protv"]);
        assert_eq!(g.programmes.len(), 6);

        let tvr: Vec<_> = g
            .programmes
            .iter()
            .filter(|p| p.channel == "tvr1.ro")
            .collect();
        assert_eq!(tvr[0].title, "Telejurnal & Sport", "first title + entities");
        assert_eq!(
            tvr[0].desc.as_deref(),
            Some("Știri și vreme"),
            "numeric reference"
        );
        assert_eq!(
            tvr[1].stop, tvr[2].start,
            "missing stop = start of the next one"
        );
        assert_eq!(tvr[2].title, "Noapte <bună>", "CDATA");
    }

    #[test]
    fn filters_by_wanted_ids_and_names() {
        let wanted = Wanted {
            ids: ["tvr1.ro".to_string(), "faracanal.ro".to_string()].into(),
            names: ["protv".to_string()].into(),
        };
        let g = parse_xmltv(SAMPLE.as_bytes(), Some(&wanted), None).unwrap();
        let ids: HashSet<&str> = g.programmes.iter().map(|p| p.channel.as_str()).collect();
        assert_eq!(ids, ["tvr1.ro", "protv.ro", "faracanal.ro"].into());
        assert!(g.channels.iter().all(|c| c.id != "altul.fr"));
    }

    #[test]
    fn time_window() {
        let at = |s| parse_xmltv_time(s).unwrap();
        let g = parse_xmltv(
            SAMPLE.as_bytes(),
            None,
            Some((at("20261014213000 +0300"), at("20261014223000 +0300"))),
        )
        .unwrap();
        let titles: Vec<&str> = g.programmes.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Film", "Noapte <bună>"]);
    }

    #[test]
    fn gzip_and_errors() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(SAMPLE.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(decompress(gz).unwrap(), SAMPLE.as_bytes());
        assert_eq!(decompress(b"plain".to_vec()).unwrap(), b"plain");

        assert!(parse_xmltv(b"#EXTM3U\nhttp://x", None, None).is_err());
        let truncated =
            &SAMPLE.as_bytes()[..SAMPLE.find("<programme start=\"20261014220000").unwrap() + 30];
        let g = parse_xmltv(truncated, None, None).unwrap();
        assert_eq!(
            g.programmes.len(),
            2,
            "a truncated file keeps what was read"
        );
    }

    #[test]
    fn rejects_epg_over_decompressed_limit() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let mut enc = GzEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&[b'x'; 4096]).unwrap();
        let gz = enc.finish().unwrap();

        let err = decompress_limited(gz, 1024).unwrap_err();
        assert!(
            matches!(&err, Error::Invalid(m) if m.contains("1024")),
            "{err}"
        );
    }

    #[test]
    fn rejects_plain_epg_over_decompressed_limit() {
        let err = decompress_limited(vec![b'x'; 1025], 1024).unwrap_err();
        assert!(
            matches!(&err, Error::Invalid(m) if m.contains("1024")),
            "{err}"
        );
    }

    #[test]
    fn progress() {
        let p = Programme {
            channel: "x".into(),
            start: 100,
            stop: 200,
            title: "t".into(),
            desc: None,
        };
        assert_eq!(p.progress(50), 0.0);
        assert_eq!(p.progress(150), 0.5);
        assert_eq!(p.progress(500), 1.0);
    }
}
