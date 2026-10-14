/// Tipul canalului — TV-Maxe ține TV și radio în liste separate.
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

/// Logo-ul unui canal: inclus în listă (TV-Maxe .db) sau de descărcat (M3U).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Logo {
    #[default]
    None,
    Url(String),
    Embedded(Vec<u8>),
}

/// Un canal așa cum vine dintr-o listă, înainte de a fi salvat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub kind: Kind,
    pub name: String,
    /// URL-uri în ordinea preferinței; următorul e încercat dacă primul cade.
    pub urls: Vec<String>,
    pub group: Option<String>,
    /// Identificator EPG (`tvg-id` în M3U, `guide` în TV-Maxe).
    pub guide_id: Option<String>,
    pub logo: Logo,
    /// Antete HTTP cerute de unele servere (iptv-org le declară per canal).
    pub http: StreamOptions,
}

/// Opțiuni de rețea per canal.
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

/// O listă de canale parsată (un abonament).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelList {
    /// Numele declarat în listă (`info.name` la TV-Maxe).
    pub name: Option<String>,
    pub epg_url: Option<String>,
    pub channels: Vec<Channel>,
}
