//! User preferences, stored in the `settings` table of the SQLite database.

use crate::player::{PlaybackSettings, VideoQuality};
use std::path::PathBuf;
use tvmaxe_core::Database;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub playback: PlaybackSettings,
    /// Update the subscriptions on every start (like TV-Maxe).
    pub refresh_on_start: bool,
    /// Download channel logos from M3U lists.
    pub download_logos: bool,
    /// Play the last channel automatically when the application opens.
    pub autoplay_last: bool,
    /// Where recordings are written (`None` = the default, see [`Settings::recordings_dir`]).
    pub recordings_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            playback: PlaybackSettings::default(),
            refresh_on_start: true,
            download_logos: true,
            autoplay_last: false,
            recordings_dir: None,
        }
    }
}

/// Limits accepted in Preferences (and when reading, against corrupt values).
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
                audio_languages: db
                    .setting::<String>("pref.audio_languages")
                    .unwrap_or_default(),
            },
            refresh_on_start: db
                .setting("pref.refresh_on_start")
                .unwrap_or(d.refresh_on_start),
            download_logos: db
                .setting("pref.download_logos")
                .unwrap_or(d.download_logos),
            autoplay_last: db.setting("pref.autoplay_last").unwrap_or(d.autoplay_last),
            recordings_dir: db
                .setting::<String>("pref.recordings_dir")
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from),
        }
    }

    /// The folder for recordings: the chosen one, or "TV-Maxe" in the user's
    /// Videos folder (in the home folder if there is none).
    pub fn recordings_dir(&self) -> PathBuf {
        self.recordings_dir
            .clone()
            .unwrap_or_else(default_recordings_dir)
    }

    pub fn save(&self, db: &Database) -> tvmaxe_core::Result<()> {
        let p = &self.playback;
        db.set_setting("pref.hwdec", p.hardware_decoding)?;
        db.set_setting("pref.cache_secs", p.cache_secs)?;
        db.set_setting("pref.network_timeout", p.network_timeout_secs)?;
        db.set_setting("pref.user_agent", p.user_agent.as_deref().unwrap_or(""))?;
        db.set_setting("pref.video_quality", p.video_quality.as_str())?;
        db.set_setting("pref.audio_languages", &p.audio_languages)?;
        db.set_setting("pref.refresh_on_start", self.refresh_on_start)?;
        db.set_setting("pref.download_logos", self.download_logos)?;
        db.set_setting("pref.autoplay_last", self.autoplay_last)?;
        db.set_setting(
            "pref.recordings_dir",
            self.recordings_dir
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )?;
        Ok(())
    }
}

pub fn default_recordings_dir() -> PathBuf {
    gtk::glib::user_special_dir(gtk::glib::UserDirectory::Videos)
        .unwrap_or_else(gtk::glib::home_dir)
        .join("TV-Maxe")
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
                audio_languages: "ro,en".into(),
            },
            refresh_on_start: false,
            download_logos: false,
            autoplay_last: true,
            recordings_dir: Some("/media/tv".into()),
        };
        s.save(&db).unwrap();
        assert_eq!(Settings::load(&db), s);
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
