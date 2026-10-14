//! The application player, split in two layers:
//!
//! * [`Player`] — the facade used by the interface. It holds the logic that does
//!   not depend on the video engine: the channel's URL list and switching to the
//!   next one when one fails (like TV-Maxe), the active/stopped state, pause and
//!   the events sent to the window.
//! * [`Backend`] — the actual video engine. It only does simple operations
//!   (load a URL, stop, volume…) and reports what happens.
//!
//! Available backends (Cargo feature): `mpv` (default).
//!
//! ## Adding a new backend (e.g. libVLC 4)
//!
//! 1. A new `player/<name>/mod.rs` module with a type that implements [`Backend`].
//! 2. A new feature in `crates/tvmaxe/Cargo.toml`, with its optional dependencies.
//! 3. A branch in [`create_backend`].
//!
//! The rest of the application does not change: the window only talks to [`Player`].

#[cfg(feature = "mpv")]
mod mpv;
#[cfg(feature = "mpv")]
pub use mpv::recorder::{Recorder, RecorderState};

#[cfg(not(any(feature = "mpv")))]
compile_error!("Enable at least one video backend (feature `mpv`).");

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
pub use tvmaxe_core::StreamOptions;
use tvmaxe_core::i18n::tr;

// ---------- Backend contract ----------

/// What a backend reports. The facade decides what it means for the user.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// Opening the URL has started.
    Connecting,
    /// Playback started (or restarted after buffering).
    Playing,
    /// Waiting for data; buffer fill percentage.
    Buffering(i64),
    /// The stream ended (for live TV it means the broadcast dropped).
    Ended,
    /// The URL cannot be played; message for the user.
    Failed(String),
}

pub type BackendHandler = Box<dyn Fn(BackendEvent)>;

/// A video engine. All methods are called from the GTK main thread, and
/// events must be delivered on the main thread too.
pub trait Backend {
    /// The widget that shows the picture.
    fn widget(&self) -> gtk::Widget;
    /// Where the backend sends its events. Called once.
    fn set_event_handler(&self, handler: BackendHandler);
    /// Starts playing a URL, replacing whatever was playing. Error = it could not start at all.
    /// `http` = the headers the channel requires (referrer, User-Agent); empty fields
    /// mean the defaults.
    fn load(&self, url: &str, http: &StreamOptions) -> Result<(), String>;
    fn stop(&self);
    fn set_paused(&self, paused: bool);
    fn is_paused(&self) -> bool;
    /// Volume 0–100.
    fn set_volume(&self, percent: f64);
    /// Seconds since the current URL started.
    fn position(&self) -> Option<f64>;
    /// Applies the playback settings (from Preferences). Can be called at any time.
    fn apply_settings(&self, settings: &PlaybackSettings);
    /// The audio tracks of the current stream.
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        Vec::new()
    }
    /// Switches to audio track `id` (from [`AudioTrack::id`]).
    fn set_audio_track(&self, _id: i64) {}
}

/// An audio track of the stream (several languages, commentary…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub id: i64,
    /// ISO 639 code from the stream ("ron", "eng"…), if any.
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub channels: Option<i64>,
    pub selected: bool,
}

impl AudioTrack {
    /// "Romanian (AAC, 5.1)" or "Track 2 (MP2, stereo)".
    pub fn label(&self) -> String {
        let name = self
            .title
            .clone()
            .or_else(|| self.lang.as_deref().map(language_name))
            .unwrap_or_else(|| tvmaxe_core::i18n::fill(tr("Track %1"), &[&self.id]));
        let channels = self.channels.map(|c| match c {
            1 => tr("mono").to_string(),
            2 => tr("stereo").to_string(),
            6 => "5.1".to_string(),
            8 => "7.1".to_string(),
            n => tvmaxe_core::i18n::fill(tr("%1 channels"), &[&n]),
        });
        let details: Vec<String> = [self.codec.as_deref().map(str::to_uppercase), channels]
            .into_iter()
            .flatten()
            .collect();
        if details.is_empty() {
            name
        } else {
            format!("{name} ({})", details.join(", "))
        }
    }
}

/// Language name for the ISO 639 codes common in IPTV streams.
pub fn language_name(code: &str) -> String {
    let name = match code.to_ascii_lowercase().as_str() {
        "ro" | "ron" | "rum" | "mol" => tr("Romanian"),
        "en" | "eng" => tr("English"),
        "hu" | "hun" => tr("Hungarian"),
        "de" | "deu" | "ger" => tr("German"),
        "fr" | "fra" | "fre" => tr("French"),
        "es" | "spa" => tr("Spanish"),
        "it" | "ita" => tr("Italian"),
        "ru" | "rus" => tr("Russian"),
        "uk" | "ukr" => tr("Ukrainian"),
        "bg" | "bul" => tr("Bulgarian"),
        "sr" | "srp" => tr("Serbian"),
        "tr" | "tur" => tr("Turkish"),
        "pl" | "pol" => tr("Polish"),
        "el" | "ell" | "gre" => tr("Greek"),
        "pt" | "por" => tr("Portuguese"),
        "nl" | "nld" | "dut" => tr("Dutch"),
        "und" | "mis" | "mul" | "" => return tr("Unknown language").to_string(),
        _ => return code.to_uppercase(),
    };
    name.to_string()
}

/// Video scaling quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoQuality {
    /// Quality, but switches to "fast" by itself if the system cannot keep up.
    #[default]
    Auto,
    /// Always quality (may stutter on slow computers).
    Quality,
    /// Always fast (bilinear scaling, low load).
    Fast,
}

impl VideoQuality {
    pub const ALL: [VideoQuality; 3] = [
        VideoQuality::Auto,
        VideoQuality::Quality,
        VideoQuality::Fast,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VideoQuality::Auto => "auto",
            VideoQuality::Quality => "quality",
            VideoQuality::Fast => "fast",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Auto => tr("Automatic"),
            VideoQuality::Quality => tr("Best quality"),
            VideoQuality::Fast => tr("Fast (low-end computers)"),
        }
    }
}

/// Settings that concern the video engine.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSettings {
    pub hardware_decoding: bool,
    /// How much content is kept in the buffer before/while playing.
    pub cache_secs: u32,
    /// After how long without an answer a URL is considered dead.
    pub network_timeout_secs: u32,
    /// `None` = User-Agent implicit.
    pub user_agent: Option<String>,
    pub video_quality: VideoQuality,
    /// Preferred audio languages, e.g. "ro,en" (empty = the stream's default).
    pub audio_languages: String,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            hardware_decoding: true,
            cache_secs: 10,
            network_timeout_secs: 15,
            user_agent: None,
            video_quality: VideoQuality::Auto,
            audio_languages: String::new(),
        }
    }
}

/// The backend chosen at compile time.
fn create_backend() -> anyhow::Result<Rc<dyn Backend>> {
    #[cfg(feature = "mpv")]
    {
        Ok(mpv::MpvBackend::new()?)
    }
    #[cfg(not(any(feature = "mpv")))]
    unreachable!()
}

// ---------- Facade ----------

/// What the window sees.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Connecting,
    Playing,
    /// Buffer fill percentage.
    Buffering(i64),
    Paused(bool),
    Stopped,
    /// Every URL of the channel failed.
    Failed(String),
}

type Handler = Box<dyn Fn(PlayerEvent)>;

pub struct Player {
    backend: Rc<dyn Backend>,
    handler: RefCell<Option<Handler>>,
    urls: RefCell<Vec<String>>,
    http: RefCell<StreamOptions>,
    /// Index of the current URL in the channel's list.
    index: Cell<usize>,
    /// How many URLs were tried for the current start (including the current one).
    tried: Cell<usize>,
    active: Cell<bool>,
}

impl Player {
    pub fn new() -> anyhow::Result<Rc<Self>> {
        Ok(Self::with_backend(create_backend()?))
    }

    pub fn with_backend(backend: Rc<dyn Backend>) -> Rc<Self> {
        let player = Rc::new(Self {
            backend,
            handler: RefCell::default(),
            urls: RefCell::default(),
            http: RefCell::default(),
            index: Cell::new(0),
            tried: Cell::new(0),
            active: Cell::new(false),
        });
        let weak: Weak<Self> = Rc::downgrade(&player);
        player.backend.set_event_handler(Box::new(move |ev| {
            if let Some(p) = weak.upgrade() {
                p.on_backend_event(ev);
            }
        }));
        player
    }

    pub fn widget(&self) -> gtk::Widget {
        self.backend.widget()
    }

    pub fn connect_event(&self, f: impl Fn(PlayerEvent) + 'static) {
        *self.handler.borrow_mut() = Some(Box::new(f));
    }

    /// Starts a channel; the URLs are tried in order.
    pub fn play(&self, urls: Vec<String>, http: StreamOptions) {
        self.play_from(urls, http, 0);
    }

    /// Starts a channel from source `start` (chosen by the user). If it
    /// fails, the others are tried in order, then from the beginning
    /// (`start = 1` of 3 → 1, 2, 0).
    pub fn play_from(&self, urls: Vec<String>, http: StreamOptions, start: usize) {
        let start = if start < urls.len() { start } else { 0 };
        *self.urls.borrow_mut() = urls;
        *self.http.borrow_mut() = http;
        self.index.set(start);
        self.tried.set(1);
        self.load_current();
    }

    /// The source (URL index) that is playing or connecting now.
    pub fn source_index(&self) -> Option<usize> {
        self.active.get().then(|| self.index.get())
    }

    pub fn stop(&self) {
        self.active.set(false);
        self.backend.stop();
        self.emit(PlayerEvent::Stopped);
    }

    pub fn is_active(&self) -> bool {
        self.active.get()
    }

    pub fn toggle_pause(&self) {
        if !self.active.get() {
            return;
        }
        let paused = !self.backend.is_paused();
        self.backend.set_paused(paused);
        self.emit(PlayerEvent::Paused(paused));
    }

    pub fn is_paused(&self) -> bool {
        self.active.get() && self.backend.is_paused()
    }

    pub fn set_volume(&self, percent: f64) {
        self.backend.set_volume(percent);
    }

    pub fn apply_settings(&self, settings: &PlaybackSettings) {
        self.backend.apply_settings(settings);
    }

    /// The audio tracks of what is playing (empty when stopped).
    pub fn audio_tracks(&self) -> Vec<AudioTrack> {
        if self.active.get() {
            self.backend.audio_tracks()
        } else {
            Vec::new()
        }
    }

    pub fn set_audio_track(&self, id: i64) {
        if self.active.get() {
            self.backend.set_audio_track(id);
        }
    }

    /// Playback time since the channel started, in seconds.
    pub fn playback_time(&self) -> Option<f64> {
        if self.active.get() {
            self.backend.position()
        } else {
            None
        }
    }

    fn emit(&self, ev: PlayerEvent) {
        if let Some(f) = self.handler.borrow().as_ref() {
            f(ev);
        }
    }

    fn load_current(&self) {
        let url = self.urls.borrow().get(self.index.get()).cloned();
        let Some(url) = url else {
            self.active.set(false);
            self.emit(PlayerEvent::Failed(tr("The channel has no URL").into()));
            return;
        };
        self.active.set(true);
        let http = self.http.borrow().clone();
        if let Err(msg) = self.backend.load(&url, &http) {
            self.on_failure(msg);
        }
    }

    /// The current URL failed: the next one, or an error if there are none left.
    fn on_failure(&self, msg: String) {
        if !self.active.get() {
            return;
        }
        let n = self.urls.borrow().len();
        if self.tried.get() < n {
            self.tried.set(self.tried.get() + 1);
            self.index.set((self.index.get() + 1) % n);
            self.load_current();
        } else {
            self.active.set(false);
            self.emit(PlayerEvent::Failed(msg));
        }
    }

    fn on_backend_event(&self, ev: BackendEvent) {
        if !self.active.get() {
            return; // late events after Stop
        }
        match ev {
            BackendEvent::Connecting => self.emit(PlayerEvent::Connecting),
            BackendEvent::Playing => self.emit(PlayerEvent::Playing),
            BackendEvent::Buffering(p) => self.emit(PlayerEvent::Buffering(p)),
            BackendEvent::Ended => self.on_failure(tr("The broadcast has ended").into()),
            BackendEvent::Failed(msg) => self.on_failure(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake backend: records what it is asked; events are sent by hand.
    #[derive(Default)]
    struct Fake {
        loads: RefCell<Vec<String>>,
        handler: RefCell<Option<BackendHandler>>,
        paused: Cell<bool>,
        stops: Cell<u32>,
        /// URLs for which `load` fails immediately.
        refuse: Vec<&'static str>,
        settings: RefCell<Option<PlaybackSettings>>,
        last_http: RefCell<Option<StreamOptions>>,
    }

    impl Fake {
        fn fire(&self, ev: BackendEvent) {
            // The handler may call `load` again (switching to a fallback).
            let h = self.handler.borrow_mut().take().unwrap();
            h(ev);
            *self.handler.borrow_mut() = Some(h);
        }
    }

    impl Backend for Fake {
        fn widget(&self) -> gtk::Widget {
            unimplemented!("the tests need no UI")
        }
        fn set_event_handler(&self, handler: BackendHandler) {
            *self.handler.borrow_mut() = Some(handler);
        }
        fn load(&self, url: &str, http: &StreamOptions) -> Result<(), String> {
            self.loads.borrow_mut().push(url.into());
            self.last_http.replace(Some(http.clone()));
            self.paused.set(false);
            if self.refuse.contains(&url) {
                Err(format!("refused: {url}"))
            } else {
                Ok(())
            }
        }
        fn stop(&self) {
            self.stops.set(self.stops.get() + 1);
        }
        fn set_paused(&self, paused: bool) {
            self.paused.set(paused);
        }
        fn is_paused(&self) -> bool {
            self.paused.get()
        }
        fn set_volume(&self, _: f64) {}
        fn position(&self) -> Option<f64> {
            Some(12.0)
        }
        fn apply_settings(&self, s: &PlaybackSettings) {
            self.settings.replace(Some(s.clone()));
        }
    }

    fn setup(fake: Fake) -> (Rc<Fake>, Rc<Player>, Rc<RefCell<Vec<PlayerEvent>>>) {
        let fake = Rc::new(fake);
        let player = Player::with_backend(fake.clone());
        let events: Rc<RefCell<Vec<PlayerEvent>>> = Rc::default();
        let e = events.clone();
        player.connect_event(move |ev| e.borrow_mut().push(ev));
        (fake, player, events)
    }

    fn urls(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Like `Player::play`, without special headers.
    trait PlayUrls {
        fn play_urls(&self, urls: Vec<String>);
    }
    impl PlayUrls for Player {
        fn play_urls(&self, urls: Vec<String>) {
            self.play(urls, StreamOptions::default());
        }
    }

    #[test]
    fn http_options_reach_every_fallback_url() {
        let (fake, player, _) = setup(Fake::default());
        let http = StreamOptions {
            referrer: Some("https://ref/".into()),
            user_agent: Some("UA/1".into()),
        };
        player.play(urls(&["a", "b"]), http.clone());
        fake.fire(BackendEvent::Failed(String::new()));
        assert_eq!(*fake.loads.borrow(), ["a", "b"]);
        assert_eq!(fake.last_http.borrow().as_ref(), Some(&http));
    }

    #[test]
    fn falls_back_through_all_urls() {
        let (fake, player, events) = setup(Fake::default());
        player.play_urls(urls(&["a", "b", "c"]));
        fake.fire(BackendEvent::Failed("a dead".into()));
        fake.fire(BackendEvent::Ended);
        assert_eq!(*fake.loads.borrow(), ["a", "b", "c"]);
        assert!(player.is_active());
        assert!(events.borrow().is_empty(), "rezervele sunt transparente");

        fake.fire(BackendEvent::Failed("c dead".into()));
        assert!(!player.is_active());
        assert_eq!(*events.borrow(), [PlayerEvent::Failed("c dead".into())]);
    }

    #[test]
    fn load_error_also_falls_back() {
        let (fake, player, _) = setup(Fake {
            refuse: vec!["bad"],
            ..Default::default()
        });
        player.play_urls(urls(&["bad", "good"]));
        assert_eq!(*fake.loads.borrow(), ["bad", "good"]);
        assert!(player.is_active());
    }

    #[test]
    fn late_events_after_stop_are_ignored() {
        let (fake, player, events) = setup(Fake::default());
        player.play_urls(urls(&["a", "b"]));
        player.stop();
        fake.fire(BackendEvent::Failed("late".into()));
        fake.fire(BackendEvent::Playing);
        assert_eq!(*fake.loads.borrow(), ["a"], "no fallback after Stop");
        assert_eq!(*events.borrow(), [PlayerEvent::Stopped]);
        assert_eq!(player.playback_time(), None);
    }

    #[test]
    fn channel_without_urls() {
        let (fake, player, events) = setup(Fake::default());
        player.play_urls(Vec::new());
        assert!(fake.loads.borrow().is_empty());
        assert!(!player.is_active());
        assert!(matches!(events.borrow()[0], PlayerEvent::Failed(_)));
    }

    #[test]
    fn events_and_pause() {
        let (fake, player, events) = setup(Fake::default());
        player.toggle_pause(); // nothing to pause yet
        player.play_urls(urls(&["a"]));
        fake.fire(BackendEvent::Connecting);
        fake.fire(BackendEvent::Buffering(40));
        fake.fire(BackendEvent::Playing);
        player.toggle_pause();
        assert!(player.is_paused());
        player.toggle_pause();
        assert_eq!(
            *events.borrow(),
            [
                PlayerEvent::Connecting,
                PlayerEvent::Buffering(40),
                PlayerEvent::Playing,
                PlayerEvent::Paused(true),
                PlayerEvent::Paused(false),
            ]
        );
        assert_eq!(player.playback_time(), Some(12.0));
    }

    #[test]
    fn settings_reach_backend() {
        let (fake, player, _) = setup(Fake::default());
        let s = PlaybackSettings {
            hardware_decoding: false,
            video_quality: VideoQuality::Fast,
            ..Default::default()
        };
        player.apply_settings(&s);
        assert_eq!(fake.settings.borrow().as_ref(), Some(&s));
    }

    #[test]
    fn audio_track_labels() {
        let t = AudioTrack {
            id: 2,
            lang: Some("eng".into()),
            title: None,
            codec: Some("aac".into()),
            channels: Some(6),
            selected: true,
        };
        assert_eq!(t.label(), format!("{} (AAC, 5.1)", tr("English")));
        let t = AudioTrack {
            id: 3,
            lang: Some("xyz".into()),
            title: None,
            codec: None,
            channels: None,
            selected: false,
        };
        assert_eq!(t.label(), "XYZ");
        let t = AudioTrack {
            id: 4,
            lang: None,
            title: Some("Commentary".into()),
            codec: Some("mp2".into()),
            channels: Some(2),
            selected: false,
        };
        assert_eq!(t.label(), format!("Commentary (MP2, {})", tr("stereo")));
        assert_eq!(language_name("ron"), tr("Romanian"));
        assert_eq!(language_name("und"), tr("Unknown language"));
    }

    #[test]
    fn video_quality_roundtrip() {
        for q in VideoQuality::ALL {
            assert_eq!(VideoQuality::parse(q.as_str()), Some(q));
        }
        assert_eq!(VideoQuality::parse("altceva"), None);
    }

    #[test]
    fn chosen_source_then_the_others_in_order() {
        let (fake, player, events) = setup(Fake::default());
        player.play_from(urls(&["a", "b", "c"]), StreamOptions::default(), 1);
        assert_eq!(player.source_index(), Some(1));
        fake.fire(BackendEvent::Failed(String::new()));
        assert_eq!(player.source_index(), Some(2));
        fake.fire(BackendEvent::Failed(String::new()));
        assert_eq!(
            player.source_index(),
            Some(0),
            "wraps around to the first source"
        );
        fake.fire(BackendEvent::Failed("all".into()));
        assert_eq!(
            *fake.loads.borrow(),
            ["b", "c", "a"],
            "each source only once"
        );
        assert_eq!(player.source_index(), None);
        assert_eq!(*events.borrow(), [PlayerEvent::Failed("all".into())]);
    }

    #[test]
    fn source_out_of_range_starts_at_first() {
        let (fake, player, _) = setup(Fake::default());
        player.play_from(urls(&["a", "b"]), StreamOptions::default(), 7);
        assert_eq!(*fake.loads.borrow(), ["a"]);
        assert_eq!(player.source_index(), Some(0));
    }

    #[test]
    fn new_channel_restarts_fallback_list() {
        let (fake, player, _) = setup(Fake::default());
        player.play_urls(urls(&["a1", "a2"]));
        fake.fire(BackendEvent::Failed(String::new()));
        player.play_urls(urls(&["b1", "b2"]));
        fake.fire(BackendEvent::Failed(String::new()));
        assert_eq!(*fake.loads.borrow(), ["a1", "a2", "b1", "b2"]);
    }
}
