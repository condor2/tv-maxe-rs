//! Supported list formats, detected from the content (not the extension):
//!
//! * **TV-Maxe `.db`** — SQLite database with the tables `info`, `tv_channels`, `radio_channels`
//! * **Old TV-Maxe** — text `|tvchannels||newchannel|name|-|url|-|logo…`
//! * **M3U / M3U8** — the usual IPTV format

mod m3u;
mod tvmaxe_db;
mod tvmaxe_legacy;

pub use m3u::parse_m3u;
pub use tvmaxe_db::{parse_tvmaxe_db_file, write_tvmaxe_db};
pub use tvmaxe_legacy::parse_tvmaxe_legacy;

use crate::{ChannelList, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    TvMaxeDb,
    TvMaxeLegacy,
    M3u,
}

const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

pub fn detect(data: &[u8]) -> Option<Format> {
    if data.starts_with(SQLITE_MAGIC) {
        return Some(Format::TvMaxeDb);
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(64 * 1024)]);
    if head.contains("|tvchannels|") || head.contains("|radiochannels|") {
        Some(Format::TvMaxeLegacy)
    } else if head
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
        || head.contains("#EXTINF")
        || head
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .is_some_and(looks_like_stream_url)
    {
        Some(Format::M3u)
    } else {
        None
    }
}

pub fn parse(data: &[u8]) -> Result<ChannelList> {
    let list = match detect(data).ok_or(Error::UnknownFormat)? {
        Format::TvMaxeDb => tvmaxe_db::parse_tvmaxe_db_bytes(data)?,
        Format::TvMaxeLegacy => parse_tvmaxe_legacy(&String::from_utf8_lossy(data)),
        Format::M3u => parse_m3u(&String::from_utf8_lossy(data)),
    };
    if list.channels.is_empty() {
        Err(Error::Empty)
    } else {
        Ok(list)
    }
}

fn looks_like_stream_url(s: &str) -> bool {
    [
        "http://", "https://", "rtmp://", "rtsp://", "udp://", "rtp://", "mms://", "mmsh://",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}
