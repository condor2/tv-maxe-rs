//! Player-ul aplicației, separat în două straturi:
//!
//! * [`Player`] — fațada folosită de interfață. Ține logica independentă de
//!   motorul video: lista de URL-uri a canalului și trecerea automată la
//!   următorul când unul cade (ca în TV-Maxe), starea activ/oprit, pauza și
//!   evenimentele către fereastră.
//! * [`Backend`] — motorul video propriu-zis. Face doar operații simple
//!   (încarcă un URL, oprește, volum…) și raportează ce se întâmplă.
//!
//! Backend-uri disponibile (feature Cargo): `mpv` (implicit).
//!
//! ## Adăugarea unui backend nou (de ex. libVLC 4)
//!
//! 1. Modul nou `player/<nume>/mod.rs` cu un tip care implementează [`Backend`].
//! 2. Feature nou în `crates/tvmaxe/Cargo.toml`, cu dependențele lui opționale.
//! 3. O ramură în [`create_backend`].
//!
//! Restul aplicației nu se schimbă: fereastra vorbește doar cu [`Player`].

#[cfg(feature = "mpv")]
mod mpv;

#[cfg(not(any(feature = "mpv")))]
compile_error!("Enable at least one video backend (feature `mpv`).");

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
pub use tvmaxe_core::StreamOptions;
use tvmaxe_core::i18n::tr;

// ---------- Contractul cu backend-ul ----------

/// Ce raportează un backend. Fațada decide ce înseamnă pentru utilizator.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// A început deschiderea URL-ului.
    Connecting,
    /// Redarea a pornit (sau a repornit după buffering).
    Playing,
    /// Așteaptă date; procentul de umplere a buffer-ului.
    Buffering(i64),
    /// Stream-ul s-a terminat (la TV live înseamnă că transmisia a căzut).
    Ended,
    /// URL-ul nu poate fi redat; mesaj pentru utilizator.
    Failed(String),
}

pub type BackendHandler = Box<dyn Fn(BackendEvent)>;

/// Un motor video. Toate metodele se apelează din firul principal GTK, iar
/// evenimentele trebuie livrate tot pe firul principal.
pub trait Backend {
    /// Widget-ul în care se afișează imaginea.
    fn widget(&self) -> gtk::Widget;
    /// Unde trimite backend-ul evenimentele. Apelat o singură dată.
    fn set_event_handler(&self, handler: BackendHandler);
    /// Începe redarea unui URL, înlocuind ce rula. Eroarea = nu a putut porni deloc.
    /// `http` = antetele cerute de canal (referrer, User-Agent); câmpurile goale
    /// înseamnă valorile implicite.
    fn load(&self, url: &str, http: &StreamOptions) -> Result<(), String>;
    fn stop(&self);
    fn set_paused(&self, paused: bool);
    fn is_paused(&self) -> bool;
    /// Volum 0–100.
    fn set_volume(&self, percent: f64);
    /// Secunde de la pornirea URL-ului curent.
    fn position(&self) -> Option<f64>;
    /// Aplică setările de redare (din Preferințe). Poate fi apelat oricând.
    fn apply_settings(&self, settings: &PlaybackSettings);
}

/// Calitatea scalării video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoQuality {
    /// Calitate, dar trece singur pe „rapid” dacă sistemul nu face față.
    #[default]
    Auto,
    /// Mereu calitate (poate sacada pe calculatoare slabe).
    Quality,
    /// Mereu rapid (scalare biliniară, consum mic).
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

/// Setările care privesc motorul video.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSettings {
    pub hardware_decoding: bool,
    /// Cât conținut se ține în buffer înainte/în timpul redării.
    pub cache_secs: u32,
    /// După cât timp fără răspuns un URL e considerat căzut.
    pub network_timeout_secs: u32,
    /// `None` = User-Agent implicit.
    pub user_agent: Option<String>,
    pub video_quality: VideoQuality,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            hardware_decoding: true,
            cache_secs: 10,
            network_timeout_secs: 15,
            user_agent: None,
            video_quality: VideoQuality::Auto,
        }
    }
}

/// Backend-ul ales la compilare.
fn create_backend() -> anyhow::Result<Rc<dyn Backend>> {
    #[cfg(feature = "mpv")]
    {
        Ok(mpv::MpvBackend::new()?)
    }
    #[cfg(not(any(feature = "mpv")))]
    unreachable!()
}

// ---------- Fațada ----------

/// Ce vede fereastra.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Connecting,
    Playing,
    /// Procent de umplere a buffer-ului.
    Buffering(i64),
    Paused(bool),
    Stopped,
    /// Toate URL-urile canalului au eșuat.
    Failed(String),
}

type Handler = Box<dyn Fn(PlayerEvent)>;

pub struct Player {
    backend: Rc<dyn Backend>,
    handler: RefCell<Option<Handler>>,
    urls: RefCell<Vec<String>>,
    http: RefCell<StreamOptions>,
    /// Indexul URL-ului curent în lista canalului.
    index: Cell<usize>,
    /// Câte URL-uri s-au încercat pentru pornirea curentă (inclusiv cel curent).
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

    /// Pornește un canal; URL-urile sunt încercate în ordine.
    pub fn play(&self, urls: Vec<String>, http: StreamOptions) {
        self.play_from(urls, http, 0);
    }

    /// Pornește un canal de la sursa `start` (aleasă de utilizator). Dacă
    /// aceasta cade, se încearcă celelalte în ordine, apoi de la început
    /// (`start = 1` din 3 → 1, 2, 0).
    pub fn play_from(&self, urls: Vec<String>, http: StreamOptions, start: usize) {
        let start = if start < urls.len() { start } else { 0 };
        *self.urls.borrow_mut() = urls;
        *self.http.borrow_mut() = http;
        self.index.set(start);
        self.tried.set(1);
        self.load_current();
    }

    /// Sursa (indexul URL-ului) care rulează sau se conectează acum.
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

    /// Timpul de redare de la pornirea canalului, în secunde.
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

    /// URL-ul curent a căzut: următorul, sau eroare dacă nu mai sunt.
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
            return; // evenimente întârziate după Stop
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

    /// Backend fals: reține ce i se cere; evenimentele sunt trimise manual.
    #[derive(Default)]
    struct Fake {
        loads: RefCell<Vec<String>>,
        handler: RefCell<Option<BackendHandler>>,
        paused: Cell<bool>,
        stops: Cell<u32>,
        /// URL-uri pentru care `load` eșuează imediat.
        refuse: Vec<&'static str>,
        settings: RefCell<Option<PlaybackSettings>>,
        last_http: RefCell<Option<StreamOptions>>,
    }

    impl Fake {
        fn fire(&self, ev: BackendEvent) {
            // Handler-ul poate apela din nou `load` (trecerea la rezervă).
            let h = self.handler.borrow_mut().take().unwrap();
            h(ev);
            *self.handler.borrow_mut() = Some(h);
        }
    }

    impl Backend for Fake {
        fn widget(&self) -> gtk::Widget {
            unimplemented!("testele nu au nevoie de UI")
        }
        fn set_event_handler(&self, handler: BackendHandler) {
            *self.handler.borrow_mut() = Some(handler);
        }
        fn load(&self, url: &str, http: &StreamOptions) -> Result<(), String> {
            self.loads.borrow_mut().push(url.into());
            self.last_http.replace(Some(http.clone()));
            self.paused.set(false);
            if self.refuse.contains(&url) {
                Err(format!("refuzat: {url}"))
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

    /// Ca `Player::play`, fără antete speciale.
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
        fake.fire(BackendEvent::Failed("a mort".into()));
        fake.fire(BackendEvent::Ended);
        assert_eq!(*fake.loads.borrow(), ["a", "b", "c"]);
        assert!(player.is_active());
        assert!(events.borrow().is_empty(), "rezervele sunt transparente");

        fake.fire(BackendEvent::Failed("c mort".into()));
        assert!(!player.is_active());
        assert_eq!(*events.borrow(), [PlayerEvent::Failed("c mort".into())]);
    }

    #[test]
    fn load_error_also_falls_back() {
        let (fake, player, _) = setup(Fake {
            refuse: vec!["rau"],
            ..Default::default()
        });
        player.play_urls(urls(&["rau", "bun"]));
        assert_eq!(*fake.loads.borrow(), ["rau", "bun"]);
        assert!(player.is_active());
    }

    #[test]
    fn late_events_after_stop_are_ignored() {
        let (fake, player, events) = setup(Fake::default());
        player.play_urls(urls(&["a", "b"]));
        player.stop();
        fake.fire(BackendEvent::Failed("întârziat".into()));
        fake.fire(BackendEvent::Playing);
        assert_eq!(*fake.loads.borrow(), ["a"], "nu trece la rezervă după Stop");
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
        player.toggle_pause(); // nimic de pus pe pauză încă
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
        assert_eq!(player.source_index(), Some(0), "revine la prima sursă");
        fake.fire(BackendEvent::Failed("toate".into()));
        assert_eq!(
            *fake.loads.borrow(),
            ["b", "c", "a"],
            "fiecare sursă o singură dată"
        );
        assert_eq!(player.source_index(), None);
        assert_eq!(*events.borrow(), [PlayerEvent::Failed("toate".into())]);
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
