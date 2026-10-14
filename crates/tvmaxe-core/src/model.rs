/// Channel kind — TV-Maxe keeps TV and radio in separate lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Tv,
    Radio,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tv => "tv",
            Kind::Radio => "radio",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tv" => Some(Kind::Tv),
            "radio" => Some(Kind::Radio),
            _ => None,
        }
    }
}

/// A channel logo: embedded in the list (TV-Maxe .db) or to be downloaded (M3U).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Logo {
    #[default]
    None,
    Url(String),
    Embedded(Vec<u8>),
}

/// A channel as it comes from a list, before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub kind: Kind,
    pub name: String,
    /// URLs in order of preference; the next one is tried if the first fails.
    pub urls: Vec<String>,
    pub group: Option<String>,
    /// EPG identifier (`tvg-id` in M3U, `guide` in TV-Maxe).
    pub guide_id: Option<String>,
    pub logo: Logo,
    /// HTTP headers required by some servers (iptv-org declares them per channel).
    pub http: StreamOptions,
}

/// Per-channel network options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamOptions {
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
}

impl Channel {
    pub fn new(kind: Kind, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            urls: vec![url.into()],
            group: None,
            guide_id: None,
            logo: Logo::None,
            http: StreamOptions::default(),
        }
    }
}

/// A parsed channel list (one subscription).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelList {
    /// The name declared in the list (`info.name` in TV-Maxe).
    pub name: Option<String>,
    pub epg_url: Option<String>,
    pub channels: Vec<Channel>,
}
