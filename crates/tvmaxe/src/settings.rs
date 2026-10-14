//! Preferințele utilizatorului, salvate în tabelul `settings` din baza SQLite.

use crate::player::{PlaybackSettings, VideoQuality};
use tvmaxe_core::Database;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub playback: PlaybackSettings,
    /// Actualizează abonamentele la fiecare pornire (ca TV-Maxe).
    pub refresh_on_start: bool,
    /// Descarcă logo-urile canalelor din liste M3U.
    pub download_logos: bool,
    /// Pornește automat ultimul canal la deschiderea aplicației.
    pub autoplay_last: bool,
    /// Surse XMLTV adăugate de utilizator, pe lângă cele declarate de liste.
    pub extra_epg_urls: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            playback: PlaybackSettings::default(),
            refresh_on_start: true,
            download_logos: true,
            autoplay_last: false,
            extra_epg_urls: Vec::new(),
        }
    }
}

/// Limitele acceptate în Preferințe (și la citire, contra valorilor corupte).
pub const CACHE_SECS: (u32, u32) = (2, 120);
pub const TIMEOUT_SECS: (u32, u32) = (5, 120);

impl Settings {
    pub fn load(db: &Database) -> Self {
        let d = Self::default();
        let p = &d.playback;
        Self {
            playback: PlaybackSettings {
                hardware_decoding: db.setting("pref.hwdec").unwrap_or(p.hardware_decoding),
                cache_secs: db
                    .setting::<u32>("pref.cache_secs")
                    .unwrap_or(p.cache_secs)
                    .clamp(CACHE_SECS.0, CACHE_SECS.1),
                network_timeout_secs: db
                    .setting::<u32>("pref.network_timeout")
                    .unwrap_or(p.network_timeout_secs)
                    .clamp(TIMEOUT_SECS.0, TIMEOUT_SECS.1),
                user_agent: db
                    .setting::<String>("pref.user_agent")
                    .filter(|s| !s.trim().is_empty()),
                video_quality: db
                    .setting::<String>("pref.video_quality")
                    .and_then(|s| VideoQuality::parse(&s))
                    .unwrap_or(p.video_quality),
            },
            refresh_on_start: db
                .setting("pref.refresh_on_start")
                .unwrap_or(d.refresh_on_start),
            download_logos: db
                .setting("pref.download_logos")
                .unwrap_or(d.download_logos),
            autoplay_last: db.setting("pref.autoplay_last").unwrap_or(d.autoplay_last),
            extra_epg_urls: db
                .setting::<String>("pref.epg_urls")
                .map(|s| parse_url_lines(&s))
                .unwrap_or_default(),
        }
    }

    pub fn save(&self, db: &Database) -> tvmaxe_core::Result<()> {
        let p = &self.playback;
        db.set_setting("pref.hwdec", p.hardware_decoding)?;
        db.set_setting("pref.cache_secs", p.cache_secs)?;
        db.set_setting("pref.network_timeout", p.network_timeout_secs)?;
        db.set_setting("pref.user_agent", p.user_agent.as_deref().unwrap_or(""))?;
        db.set_setting("pref.video_quality", p.video_quality.as_str())?;
        db.set_setting("pref.refresh_on_start", self.refresh_on_start)?;
        db.set_setting("pref.download_logos", self.download_logos)?;
        db.set_setting("pref.autoplay_last", self.autoplay_last)?;
        db.set_setting("pref.epg_urls", self.extra_epg_urls.join("\n"))?;
        Ok(())
    }
}

/// Câte o adresă pe linie; liniile goale și duplicatele sunt ignorate.
pub fn parse_url_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !out.iter().any(|u| u == l) {
            out.push(l.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(Settings::load(&db), Settings::default());
    }

    #[test]
    fn roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let s = Settings {
            playback: PlaybackSettings {
                hardware_decoding: false,
                cache_secs: 30,
                network_timeout_secs: 40,
                user_agent: Some("VLC/3.0.21 LibVLC/3.0.21".into()),
                video_quality: VideoQuality::Fast,
            },
            refresh_on_start: false,
            download_logos: false,
            autoplay_last: true,
            extra_epg_urls: vec!["https://epg/a.xml.gz".into(), "/home/u/b.xml".into()],
        };
        s.save(&db).unwrap();
        assert_eq!(Settings::load(&db), s);
    }

    #[test]
    fn url_lines() {
        assert_eq!(parse_url_lines("  a \n\n b\na\n"), ["a", "b"]);
        assert!(parse_url_lines("\n  \n").is_empty());
    }

    #[test]
    fn empty_user_agent_means_default_and_values_are_clamped() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting("pref.user_agent", "  ").unwrap();
        db.set_setting("pref.cache_secs", 100000).unwrap();
        db.set_setting("pref.network_timeout", 0).unwrap();
        db.set_setting("pref.video_quality", "necunoscut").unwrap();
        let s = Settings::load(&db);
        assert_eq!(s.playback.user_agent, None);
        assert_eq!(s.playback.cache_secs, CACHE_SECS.1);
        assert_eq!(s.playback.network_timeout_secs, TIMEOUT_SECS.0);
        assert_eq!(s.playback.video_quality, VideoQuality::Auto);
    }
}
