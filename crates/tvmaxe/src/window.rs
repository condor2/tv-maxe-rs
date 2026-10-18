//! The main window, modelled on TV-Maxe:
//!
//! ```text
//! ┌ TV-Maxe  View  Help ────────────────────────────────────────┐
//! │┌TV channels│Radio┐    ┌─────────────────────────────────┐ │
//! ││Channel list: [▾]  │   │                                 │ │
//! ││[Search channel  ]  │   │             video               │ │
//! ││▣ TVR 1             │   │                                 │ │
//! ││▣ Digi 24           │   └─────────────────────────────────┘ │
//! ││…                   │    ▣ TVR 1   News · Romania           │
//! │└────────────────────┘    ▶ ■  00:12:31            ⛶  🔊     │
//! │ 123 TV channels in 3 lists                                   │
//! ├──────────────────────────────────────────────────────────────┤
//! │ Playing: TVR 1                                 [▬▬▬     ]   │
//! └──────────────────────────────────────────────────────────────┘
//! ```

mod epg;
mod mpris;
mod recordings;
mod sleep;
mod sources;
mod zapping;

pub use sleep::sleep_action_available;

use crate::channel_list::{ChannelList, ChannelRow, placeholder_icon, row_of};
use crate::dialogs;
use crate::player::{Player, PlayerEvent, StreamOptions};
use crate::settings::Settings;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use tvmaxe_core::i18n::{fill, tr};
use tvmaxe_core::{Database, EpgIndex, Kind, net};

struct Current {
    /// The channel's database id (for editing and choosing the source).
    channel_id: i64,
    name: String,
    kind: Kind,
    urls: Vec<String>,
    detail: String,
    texture: Option<gdk::Texture>,
    epg_key: Option<String>,
    http: StreamOptions,
}

/// The channel the "TV guide" window opens for.
#[derive(Clone)]
pub struct GuideTarget {
    pub name: String,
    pub kind: Kind,
    pub epg_key: Option<String>,
    pub texture: Option<gdk::Texture>,
    /// For recording a programme from the guide.
    pub urls: Vec<String>,
    pub http: StreamOptions,
}

pub struct MainWindow {
    pub window: gtk::ApplicationWindow,
    pub db: RefCell<Database>,
    pub db_path: PathBuf,
    player: Rc<Player>,

    paned: gtk::Paned,
    notebook: gtk::Notebook,
    tv: Rc<ChannelList>,
    radio: Rc<ChannelList>,
    list_status: gtk::Label,

    placeholder: gtk::Box,
    placeholder_image: gtk::Image,
    placeholder_label: gtk::Label,

    info_bar: gtk::Box,
    info_logo: gtk::Image,
    info_name: gtk::Label,
    info_detail: gtk::Label,
    epg_box: gtk::Box,
    epg_now: gtk::Label,
    epg_progress: gtk::ProgressBar,
    epg_next: gtk::Label,

    controls: gtk::Box,
    play_btn: gtk::Button,
    time_label: gtk::Label,
    volume: gtk::ScaleButton,

    bottom: gtk::Box,
    status_label: gtk::Label,
    progress: gtk::ProgressBar,

    current: RefCell<Option<Current>>,
    connecting: Cell<bool>,
    busy: Cell<u32>,
    list_visible: Cell<bool>,
    /// Reusable dialogs (hidden on close, not destroyed).
    pub subs_dialog: RefCell<Option<Rc<dialogs::SubscriptionsDialog>>>,
    settings: RefCell<Settings>,
    epg_index: RefCell<EpgIndex>,
    pub guide_dialog: RefCell<Option<Rc<dialogs::GuideDialog>>>,
    pub recordings_dialog: RefCell<Option<Rc<dialogs::RecordingsDialog>>>,
    /// Active recorders, by recording id.
    recorders: RefCell<std::collections::HashMap<i64, crate::player::Recorder>>,
    record_btn: gtk::Button,
    rec_indicator: gtk::Label,
    /// The user confirmed quitting while recordings were running.
    quit_confirmed: Cell<bool>,
    sleep: RefCell<Option<sleep::SleepTimer>>,
    /// The MPRIS service on the session bus, once started.
    mpris: RefCell<Option<crate::mpris::Mpris>>,
    sleep_indicator: gtk::Label,
    fav_btn: gtk::Button,
    audio_btn: gtk::MenuButton,
    /// On-screen display over the video (channel number and name, audio track…).
    osd: gtk::Label,
    osd_timer: RefCell<Option<glib::SourceId>>,
    /// The channel watched before the current one: (kind, first URL).
    previous: RefCell<Option<(Kind, String)>>,
    /// Digits typed for a channel number.
    typed_number: RefCell<String>,
    digit_timer: RefCell<Option<glib::SourceId>>,
    logo_queue: RefCell<Vec<String>>,
    logo_seen: RefCell<std::collections::HashSet<String>>,
    logo_inflight: Cell<usize>,
}

impl MainWindow {
    pub fn new(app: &gtk::Application, db_path: PathBuf) -> anyhow::Result<Rc<Self>> {
        let db = Database::open(&db_path).map_err(|e| {
            anyhow::anyhow!(
                "{}",
                fill(
                    tr("cannot open the database %1: %2"),
                    &[&db_path.display(), &e]
                )
            )
        })?;
        let player = Player::new()?;

        // ---------- Left: TV / Radio tabs ----------
        let tv = ChannelList::new(Kind::Tv);
        let radio = ChannelList::new(Kind::Radio);
        let notebook = gtk::Notebook::new();
        notebook.append_page(&tv.root, Some(&gtk::Label::new(Some(tr("TV channels")))));
        notebook.append_page(
            &radio.root,
            Some(&gtk::Label::new(Some(tr("Radio stations")))),
        );
        notebook.set_vexpand(true);

        let list_status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(8)
            .margin_bottom(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        list_status.add_css_class("dim-label");
        list_status.add_css_class("caption");

        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.append(&notebook);
        left.append(&list_status);

        // ---------- Right: video ----------
        let placeholder_image = gtk::Image::builder()
            .pixel_size(96)
            .icon_name("video-display-symbolic")
            .build();
        let placeholder_label = gtk::Label::builder()
            .label("TV-Maxe")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let placeholder_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        placeholder_content.append(&placeholder_image);
        placeholder_content.append(&placeholder_label);
        // Covers the whole video area (black background), so for radio or after Stop
        // the last frame of the previous channel does not stay visible. Clicks go
        // through to the video (double-click = fullscreen).
        let placeholder = gtk::Box::builder().can_target(false).build();
        placeholder.add_css_class("video-placeholder");
        placeholder.append(&placeholder_content);

        let video = gtk::Overlay::new();
        video.add_css_class("video-area");
        video.set_child(Some(&player.widget()));
        video.add_overlay(&placeholder);
        let osd = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(16)
            .margin_top(16)
            .visible(false)
            .can_target(false)
            .build();
        osd.add_css_class("osd");
        osd.add_css_class("zap-osd");
        video.add_overlay(&osd);
        video.set_vexpand(true);

        // Information about the current channel.
        let info_logo = gtk::Image::builder().pixel_size(32).build();
        let info_name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        info_name.add_css_class("channel-title");
        let info_detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        info_detail.add_css_class("dim-label");
        let info_text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        info_text.append(&info_name);
        info_text.append(&info_detail);
        // TV guide for the current channel: now (with progress) and the next programme.
        let epg_now = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let epg_progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .build();
        epg_progress.add_css_class("epg-progress");
        let epg_next = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        epg_next.add_css_class("dim-label");
        let epg_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .visible(false)
            .tooltip_text(tr("TV guide (Ctrl+G)"))
            .build();
        epg_box.append(&epg_now);
        epg_box.append(&epg_progress);
        epg_box.append(&epg_next);
        let epg_click = gtk::GestureClick::new();
        epg_click.connect_released(|g, _, _, _| {
            if let Some(w) = g.widget() {
                let _ = w.activate_action("win.guide", None);
            }
        });
        epg_box.add_controller(epg_click);

        let info_bar = gtk::Box::builder()
            .spacing(10)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .build();
        info_bar.append(&info_logo);
        info_bar.append(&info_text);
        info_bar.append(&epg_box);

        // Control bar: Play, Stop, time, fullscreen, volume.
        let play_btn = icon_button("media-playback-start-symbolic", tr("Play"));
        let stop_btn = icon_button("media-playback-stop-symbolic", tr("Stop"));
        let time_label = gtk::Label::new(Some("00:00:00"));
        time_label.add_css_class("numeric");
        let record_btn = icon_button("media-record-symbolic", tr("Record (Ctrl+Shift+R)"));
        record_btn.set_action_name(Some("win.record"));
        let fav_btn = icon_button("non-starred-symbolic", tr("Add to favourites (Ctrl+D)"));
        fav_btn.set_action_name(Some("win.favourite-current"));
        let audio_btn = gtk::MenuButton::builder()
            .icon_name("audio-x-generic-symbolic")
            .tooltip_text(tr("Audio track (A)"))
            .visible(false)
            .build();
        audio_btn.add_css_class("flat");
        let full_btn = icon_button("view-fullscreen-symbolic", tr("Fullscreen (F11)"));
        full_btn.set_action_name(Some("win.fullscreen"));
        let volume = gtk::ScaleButton::new(
            0.0,
            100.0,
            5.0,
            &[
                "audio-volume-muted-symbolic",
                "audio-volume-high-symbolic",
                "audio-volume-low-symbolic",
                "audio-volume-medium-symbolic",
            ],
        );
        let spacer = gtk::Box::builder().hexpand(true).build();
        let controls = gtk::Box::builder()
            .spacing(4)
            .margin_start(6)
            .margin_end(6)
            .margin_top(4)
            .margin_bottom(6)
            .build();
        for w in [
            play_btn.upcast_ref::<gtk::Widget>(),
            stop_btn.upcast_ref(),
            record_btn.upcast_ref(),
            fav_btn.upcast_ref(),
            time_label.upcast_ref(),
            spacer.upcast_ref(),
            audio_btn.upcast_ref(),
            full_btn.upcast_ref(),
            volume.upcast_ref(),
        ] {
            controls.append(w);
        }
        time_label.set_margin_start(8);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.append(&video);
        right.append(&info_bar);
        right.append(&controls);

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&left)
            .end_child(&right)
            .resize_start_child(false)
            .shrink_start_child(false)
            .vexpand(true)
            .build();

        // ---------- Status bar ----------
        let status_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .width_request(140)
            .visible(false)
            .build();
        let statusbar = gtk::Box::builder()
            .spacing(8)
            .margin_start(8)
            .margin_end(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        // Shown while something is being recorded; opens the Recordings window.
        let rec_indicator = gtk::Label::builder().visible(false).build();
        rec_indicator.add_css_class("recording-indicator");
        let rec_click = gtk::GestureClick::new();
        rec_click.connect_released(|g, _, _, _| {
            if let Some(w) = g.widget() {
                let _ = w.activate_action("win.recordings", None);
            }
        });
        rec_indicator.add_controller(rec_click);
        // Sleep timer countdown; opens the Sleep timer window.
        let sleep_indicator = gtk::Label::builder().visible(false).build();
        sleep_indicator.add_css_class("numeric");
        let sleep_click = gtk::GestureClick::new();
        sleep_click.connect_released(|g, _, _, _| {
            if let Some(w) = g.widget() {
                let _ = w.activate_action("win.sleep-timer", None);
            }
        });
        sleep_indicator.add_controller(sleep_click);
        statusbar.append(&status_label);
        statusbar.append(&sleep_indicator);
        statusbar.append(&rec_indicator);
        statusbar.append(&progress);
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        bottom.append(&statusbar);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&paned);
        content.append(&bottom);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("TV-Maxe")
            .icon_name("TvMaxe")
            .show_menubar(true)
            .child(&content)
            .build();

        // Saved state.
        window.set_default_size(
            db.setting("window_width").unwrap_or(1000),
            db.setting("window_height").unwrap_or(620),
        );
        if db.setting::<bool>("window_maximized").unwrap_or(false) {
            window.maximize();
        }
        paned.set_position(db.setting("paned_position").unwrap_or(300));
        notebook.set_current_page(Some(db.setting("tab").unwrap_or(0)));
        let vol = db.setting("volume").unwrap_or(100.0);
        volume.set_value(vol);
        player.set_volume(vol);
        let settings = Settings::load(&db);
        let epg_index = db.epg_index().unwrap_or_default();
        player.apply_settings(&settings.playback);
        net::set_user_agent(settings.playback.user_agent.clone());

        let this = Rc::new(Self {
            window,
            db: RefCell::new(db),
            db_path,
            player,
            paned,
            notebook,
            tv,
            radio,
            list_status,
            placeholder,
            placeholder_image,
            placeholder_label,
            info_bar,
            info_logo,
            info_name,
            info_detail,
            epg_box,
            epg_now,
            epg_progress,
            epg_next,
            controls,
            play_btn,
            time_label,
            volume,
            bottom,
            status_label,
            progress,
            current: RefCell::default(),
            connecting: Cell::new(false),
            busy: Cell::new(0),
            list_visible: Cell::new(true),
            subs_dialog: RefCell::default(),
            settings: RefCell::new(settings),
            epg_index: RefCell::new(epg_index),
            guide_dialog: RefCell::default(),
            recordings_dialog: RefCell::default(),
            recorders: RefCell::default(),
            record_btn,
            rec_indicator,
            quit_confirmed: Cell::new(false),
            sleep: RefCell::default(),
            mpris: RefCell::default(),
            sleep_indicator,
            fav_btn,
            audio_btn,
            osd,
            osd_timer: RefCell::default(),
            previous: RefCell::default(),
            typed_number: RefCell::default(),
            digit_timer: RefCell::default(),
            logo_queue: RefCell::default(),
            logo_seen: RefCell::default(),
            logo_inflight: Cell::new(0),
        });

        this.connect_signals(&stop_btn);
        this.install_actions();
        this.start_recording_scheduler();
        this.import_legacy_userlist();
        this.reload();
        let restored = this.restore_last_channel();
        this.update_now_playing();

        let s = this.settings();
        if restored && s.autoplay_last {
            // mpv needs the video area on screen (OpenGL context); the window is
            // not shown yet here, so playback starts on the first map.
            let done = Cell::new(false);
            let w = Rc::downgrade(&this);
            this.player.widget().connect_map(move |_| {
                if !done.replace(true)
                    && let Some(w) = w.upgrade()
                {
                    w.play_or_pause();
                }
            });
        }
        if s.refresh_on_start {
            this.refresh_subscriptions(None);
        } else {
            this.refresh_epg(false);
        }
        Ok(this)
    }

    fn connect_signals(self: &Rc<Self>, stop_btn: &gtk::Button) {
        let w = Rc::downgrade(self);
        let on_row = move |row: &ChannelRow| {
            if let Some(w) = w.upgrade() {
                w.play_row(row);
            }
        };
        self.tv.connect_activate(on_row.clone());
        self.radio.connect_activate(on_row);
        for list in [&self.tv, &self.radio] {
            let w = Rc::downgrade(self);
            list.connect_need_logo(move |url| {
                if let Some(w) = w.upgrade() {
                    w.request_logo(url);
                }
            });
            let w = Rc::downgrade(self);
            list.connect_context_menu(move |widget, x, y, row| {
                if let Some(w) = w.upgrade() {
                    w.show_channel_menu(widget, x, y, row);
                }
            });
        }

        let w = Rc::downgrade(self);
        self.play_btn.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.play_or_pause();
            }
        });
        let w = Rc::downgrade(self);
        stop_btn.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.player.stop();
            }
        });

        let w = Rc::downgrade(self);
        self.volume.connect_value_changed(move |_, v| {
            if let Some(w) = w.upgrade() {
                w.player.set_volume(v);
                w.mpris_sync();
            }
        });

        let w = Rc::downgrade(self);
        self.player.connect_event(move |ev| {
            if let Some(w) = w.upgrade() {
                w.on_player_event(ev);
            }
        });

        // Double-click on the video → fullscreen.
        let dbl = gtk::GestureClick::new();
        let win = self.window.downgrade();
        dbl.connect_pressed(move |_, n, _, _| {
            if n == 2
                && let Some(win) = win.upgrade()
            {
                let _ = WidgetExt::activate_action(&win, "win.fullscreen", None);
            }
        });
        self.player.widget().add_controller(dbl);

        // Esc leaves fullscreen. Capture phase: the key arrives here before the
        // focused widget (which is hidden in fullscreen).
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(w) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::Escape && w.window.is_fullscreen() {
                w.window.unfullscreen();
                return glib::Propagation::Stop;
            }
            // Zapping: Page Up/Down, digits, Backspace, A (audio track).
            if w.zap_key(key, state) {
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);

        let w = Rc::downgrade(self);
        self.window.connect_fullscreened_notify(move |win| {
            if let Some(w) = w.upgrade() {
                w.apply_fullscreen(win.is_fullscreen());
            }
        });

        // Playback clock and progress bar animation.
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(500), move || {
            let Some(w) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let t = w.player.playback_time().unwrap_or(0.0) as u64;
            w.time_label
                .set_label(&format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60));
            glib::ControlFlow::Continue
        });
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(30), move || {
            let Some(w) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            w.refresh_epg_view();
            glib::ControlFlow::Continue
        });
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(120), move || {
            let Some(w) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if w.connecting.get() || w.busy.get() > 0 {
                w.progress.set_visible(true);
                w.progress.pulse();
            }
            glib::ControlFlow::Continue
        });

        let w = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(w) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if w.active_recordings() > 0 && !w.quit_confirmed.get() {
                w.confirm_quit_while_recording();
                return glib::Propagation::Stop;
            }
            w.stop_all_recordings();
            w.save_state();
            w.player.stop();
            glib::Propagation::Proceed
        });
    }

    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: fn(&Rc<Self>)| {
            let a = gio::SimpleAction::new(name, None);
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&a);
        };
        add("subscriptions", dialogs::SubscriptionsDialog::present);
        add("refresh", |w| w.refresh_subscriptions(None));
        add("add-channel", dialogs::add_channel);
        add("add-xtream", |w| {
            dialogs::xtream_account(w, w.window.upcast_ref())
        });
        add("export-local", dialogs::export_local);
        add("search", |w| w.current_list().focus_search());
        add("about", |w| dialogs::about(&w.window));
        add("preferences", dialogs::preferences);
        add("guide", dialogs::GuideDialog::present);
        add("recordings", dialogs::RecordingsDialog::present);
        add("record", |w| w.record_now());
        add("sleep-timer", dialogs::sleep_timer);
        add("favourite-current", |w| w.toggle_current_favourite());
        add("zap-next", |w| w.zap(1));
        add("zap-prev", |w| w.zap(-1));
        add("zap-back", |w| w.zap_back());
        add("cycle-audio", |w| w.cycle_audio_track());
        add("show-favourites", |w| {
            if !w.current_list().show_favourites() {
                w.set_status(tr(
                    "No favourites yet: right-click a channel → Add to favourites",
                ));
            }
        });

        let fav = gio::SimpleAction::new("toggle-favourite", Some(glib::VariantTy::INT64));
        let w = Rc::downgrade(self);
        fav.connect_activate(move |_, p| {
            if let (Some(w), Some(id)) = (w.upgrade(), p.and_then(|p| p.get::<i64>()))
                && let Some(obj) = w.item_by_id(id)
            {
                w.toggle_favourite(&row_of(obj.upcast_ref()));
            }
        });
        self.window.add_action(&fav);

        // Radio items of the audio menu: the state is the selected track id.
        let audio = gio::SimpleAction::new_stateful(
            "audio-track",
            Some(glib::VariantTy::INT64),
            &0i64.to_variant(),
        );
        let w = Rc::downgrade(self);
        audio.connect_activate(move |a, p| {
            if let (Some(w), Some(id)) = (w.upgrade(), p.and_then(|p| p.get::<i64>())) {
                w.player.set_audio_track(id);
                a.set_state(&id.to_variant());
            }
        });
        self.window.add_action(&audio);
        let w = Rc::downgrade(self);
        self.audio_btn.set_create_popup_func(move |b| {
            if let Some(w) = w.upgrade() {
                b.set_menu_model(Some(&w.audio_menu()));
            }
        });

        let delete = gio::SimpleAction::new("delete-channel", Some(glib::VariantTy::INT64));
        let w = Rc::downgrade(self);
        delete.connect_activate(move |_, p| {
            if let (Some(w), Some(id)) = (w.upgrade(), p.and_then(|p| p.get::<i64>())) {
                w.delete_channel(id);
            }
        });
        self.window.add_action(&delete);

        let edit = gio::SimpleAction::new("edit-channel", Some(glib::VariantTy::INT64));
        let w = Rc::downgrade(self);
        edit.connect_activate(move |_, p| {
            if let (Some(w), Some(id)) = (w.upgrade(), p.and_then(|p| p.get::<i64>()))
                && let Some(obj) = w.item_by_id(id)
            {
                let channel = row_of(obj.upcast_ref()).channel.clone();
                dialogs::edit_channel(&w, &channel);
            }
        });
        self.window.add_action(&edit);

        for name in ["record-channel", "schedule-channel"] {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::INT64));
            let w = Rc::downgrade(self);
            let schedule = name == "schedule-channel";
            a.connect_activate(move |_, p| {
                let (Some(w), Some(id)) = (w.upgrade(), p.and_then(|p| p.get::<i64>())) else {
                    return;
                };
                let Some(obj) = w.item_by_id(id) else { return };
                let ch = row_of(obj.upcast_ref()).channel.clone();
                if schedule {
                    let now = tvmaxe_core::net::unix_now();
                    let start = now + 60 - now % 60;
                    let rec = tvmaxe_core::recording::Recording::new(
                        &ch.name,
                        ch.kind,
                        &ch.urls,
                        &ch.http,
                        start,
                        Some(start + 3600),
                    );
                    dialogs::schedule_dialog(&w, rec);
                } else {
                    w.record_channel_now(&ch.name, ch.kind, &ch.urls, &ch.http);
                }
            });
            self.window.add_action(&a);
        }

        // (channel id, source index): plays the channel from the chosen source.
        let source = gio::SimpleAction::new(
            "play-source",
            Some(&<(i64, u32) as glib::variant::StaticVariantType>::static_variant_type()),
        );
        let w = Rc::downgrade(self);
        source.connect_activate(move |_, p| {
            if let (Some(w), Some((id, index))) =
                (w.upgrade(), p.and_then(|p| p.get::<(i64, u32)>()))
                && let Some(obj) = w.item_by_id(id)
            {
                w.play_row_from(&row_of(obj.upcast_ref()), index as usize);
            }
        });
        self.window.add_action(&source);

        let fullscreen = gio::SimpleAction::new_stateful("fullscreen", None, &false.to_variant());
        let win = self.window.downgrade();
        fullscreen.connect_activate(move |_, _| {
            if let Some(win) = win.upgrade() {
                win.set_fullscreened(!win.is_fullscreen());
            }
        });
        self.window.add_action(&fullscreen);

        let show_list = gio::SimpleAction::new_stateful("show-list", None, &true.to_variant());
        let w = Rc::downgrade(self);
        show_list.connect_activate(move |a, _| {
            let Some(w) = w.upgrade() else { return };
            let visible = !w.list_visible.get();
            w.list_visible.set(visible);
            a.set_state(&visible.to_variant());
            w.paned.start_child().unwrap().set_visible(visible);
        });
        self.window.add_action(&show_list);
    }

    // ---------- Lists ----------

    pub fn current_kind(&self) -> Kind {
        self.current_list().kind
    }

    fn current_list(&self) -> &Rc<ChannelList> {
        if self.notebook.current_page() == Some(1) {
            &self.radio
        } else {
            &self.tv
        }
    }

    // ---------- Preferences ----------

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// Saves and applies the new preferences immediately.
    pub fn set_settings(self: &Rc<Self>, new: Settings) {
        let old = self.settings.replace(new.clone());
        if let Err(e) = new.save(&self.db.borrow()) {
            self.set_status(&fill(tr("Cannot save preferences: %1"), &[&e]));
        }
        if new.playback != old.playback {
            self.player.apply_settings(&new.playback);
            net::set_user_agent(new.playback.user_agent.clone());
        }
        if new.download_logos && !old.download_logos {
            self.logo_seen.borrow_mut().clear();
            self.tv.refresh_visible();
            self.radio.refresh_visible();
        }
    }

    // ---------- Playback ----------

    fn play_row(&self, row: &ChannelRow) {
        self.play_row_from(row, 0);
    }

    /// Plays a channel from source (URL) `source`.
    fn play_row_from(&self, row: &ChannelRow, source: usize) {
        let ch = &row.channel;
        // Remember the channel left behind, for Backspace.
        if let Some(c) = self.current.borrow().as_ref()
            && c.channel_id >= 0
            && c.urls.first() != ch.urls.first()
            && let Some(url) = c.urls.first()
        {
            *self.previous.borrow_mut() = Some((c.kind, url.clone()));
        }
        let detail = match &ch.group {
            Some(g) => format!("{g} · {}", row.subscription),
            None => row.subscription.clone(),
        };
        *self.current.borrow_mut() = Some(Current {
            channel_id: ch.id,
            name: ch.name.clone(),
            kind: ch.kind,
            urls: ch.urls.clone(),
            detail,
            texture: row.texture(),
            epg_key: row.epg_key.clone(),
            http: ch.http.clone(),
        });
        let _ = self
            .db
            .borrow()
            .set_setting("last_channel", ch.urls.first().cloned().unwrap_or_default());
        self.update_now_playing();
        self.update_epg_info();
        self.player
            .play_from(ch.urls.clone(), ch.http.clone(), source);
        if let (Some(d), Some(t)) = (
            self.guide_dialog
                .borrow()
                .clone()
                .filter(|d| d.is_visible()),
            self.guide_target(),
        ) {
            d.show_channel(t);
        }
    }

    fn play_or_pause(&self) {
        if self.player.is_active() {
            self.player.toggle_pause();
            return;
        }
        if let Some(obj) = self.current_list().selected() {
            self.play_row(&row_of(obj.upcast_ref()));
        } else if let Some((urls, http)) = self
            .current
            .borrow()
            .as_ref()
            .map(|c| (c.urls.clone(), c.http.clone()))
        {
            self.player.play(urls, http);
        } else {
            self.set_status(tr("Choose a channel from the list (double-click)"));
        }
    }

    fn on_player_event(&self, ev: PlayerEvent) {
        let name = self
            .current
            .borrow()
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        self.connecting.set(matches!(ev, PlayerEvent::Connecting));
        match &ev {
            PlayerEvent::Connecting => self.set_status(&fill(tr("Connecting to %1…"), &[&name])),
            PlayerEvent::Playing => {
                self.update_audio_button();
                self.progress.set_visible(false);
                self.set_status(&fill(tr("Playing: %1"), &[&name]));
            }
            PlayerEvent::Buffering(p) => {
                self.progress.set_visible(true);
                self.progress.set_fraction(*p as f64 / 100.0);
                self.set_status(&fill(tr("Buffering… %1%"), &[p]));
            }
            PlayerEvent::Paused(true) => self.set_status(&fill(tr("Paused: %1"), &[&name])),
            PlayerEvent::Paused(false) => self.set_status(&fill(tr("Playing: %1"), &[&name])),
            PlayerEvent::Stopped => self.set_status(tr("Stopped")),
            PlayerEvent::Failed(msg) => self.set_status(&format!("{name}: {msg}")),
        }
        if !self.connecting.get()
            && self.busy.get() == 0
            && !matches!(ev, PlayerEvent::Buffering(_))
        {
            self.progress.set_visible(false);
        }
        self.update_now_playing();
    }

    /// The info panel, the Play button and the picture in the video area.
    fn update_now_playing(&self) {
        self.update_favourite_button();
        let cur = self.current.borrow();
        let active = self.player.is_active();
        let paused = active && self.player.is_paused();
        self.play_btn.set_icon_name(if active && !paused {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.play_btn.set_tooltip_text(Some(if active && !paused {
            tr("Pause")
        } else {
            tr("Play")
        }));

        match cur.as_ref() {
            Some(c) => {
                self.info_name.set_label(&c.name);
                match self.player.source_index().filter(|_| c.urls.len() > 1) {
                    Some(i) => self.info_detail.set_label(&fill(
                        tr("%1 · source %2/%3"),
                        &[&c.detail, &(i + 1), &c.urls.len()],
                    )),
                    None => self.info_detail.set_label(&c.detail),
                }
                match &c.texture {
                    Some(t) => self.info_logo.set_paintable(Some(t)),
                    None => self.info_logo.set_icon_name(Some(placeholder_icon(c.kind))),
                }
                self.info_bar.set_visible(!self.window.is_fullscreen());
            }
            None => self.info_bar.set_visible(false),
        }

        // The picture in the video area: when nothing is playing, or for radio.
        let show = !active || cur.as_ref().is_some_and(|c| c.kind == Kind::Radio);
        self.placeholder.set_visible(show);
        if show {
            match cur.as_ref().filter(|_| active) {
                Some(c) => {
                    match &c.texture {
                        Some(t) => self.placeholder_image.set_paintable(Some(t)),
                        None => self
                            .placeholder_image
                            .set_icon_name(Some(placeholder_icon(c.kind))),
                    }
                    self.placeholder_label.set_label(&c.name);
                }
                None => {
                    self.placeholder_image
                        .set_icon_name(Some("video-display-symbolic"));
                    self.placeholder_label.set_label("TV-Maxe");
                }
            }
        }
        self.mpris_sync();
    }

    // ---------- Audio tracks ----------

    /// Shows the audio track button only when there is a choice.
    fn update_audio_button(&self) {
        let tracks = self.player.audio_tracks();
        self.audio_btn.set_visible(tracks.len() > 1);
        if let Some(t) = tracks.iter().find(|t| t.selected) {
            self.audio_btn
                .set_tooltip_text(Some(&fill(tr("Audio track: %1 (A)"), &[&t.label()])));
            if let Some(a) = self
                .window
                .lookup_action("audio-track")
                .and_downcast::<gio::SimpleAction>()
            {
                a.set_state(&t.id.to_variant());
            }
        }
    }

    fn audio_menu(&self) -> gio::Menu {
        self.update_audio_button();
        let menu = gio::Menu::new();
        for t in self.player.audio_tracks() {
            let item = gio::MenuItem::new(Some(&t.label().replace('_', "__")), None);
            item.set_action_and_target_value(Some("win.audio-track"), Some(&t.id.to_variant()));
            menu.append_item(&item);
        }
        menu
    }

    /// A key: the next audio track, with the choice shown over the video.
    pub fn cycle_audio_track(&self) {
        let tracks = self.player.audio_tracks();
        if tracks.len() < 2 {
            self.show_osd(tr("No other audio track"));
            return;
        }
        let cur = tracks.iter().position(|t| t.selected).unwrap_or(0);
        let next = &tracks[(cur + 1) % tracks.len()];
        self.player.set_audio_track(next.id);
        self.show_osd(&fill(tr("Audio: %1"), &[&next.label()]));
        if let Some(a) = self
            .window
            .lookup_action("audio-track")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_state(&next.id.to_variant());
        }
        self.audio_btn
            .set_tooltip_text(Some(&fill(tr("Audio track: %1 (A)"), &[&next.label()])));
    }

    // ---------- Recordings ----------

    /// Plays a finished recording; its parts follow one another.
    pub fn play_recording(&self, title: &str, files: &[String]) {
        *self.current.borrow_mut() = Some(Current {
            channel_id: -1,
            name: title.to_string(),
            kind: Kind::Tv,
            urls: files.to_vec(),
            detail: tr("Recording").to_string(),
            texture: None,
            epg_key: None,
            http: StreamOptions::default(),
        });
        self.update_now_playing();
        self.update_epg_info();
        self.player.play(files.to_vec(), StreamOptions::default());
    }

    /// The schedule form for a programme from the TV guide, with a few
    /// minutes of margin on both sides.
    pub fn schedule_programme(
        self: &Rc<Self>,
        target: &GuideTarget,
        p: &tvmaxe_core::epg::Programme,
    ) {
        let mut rec = tvmaxe_core::recording::Recording::new(
            &target.name,
            target.kind,
            &target.urls,
            &target.http,
            p.start - recordings::EARLY_SECS,
            Some(p.stop + recordings::LATE_SECS),
        );
        rec.title = p.title.clone();
        dialogs::schedule_dialog(self, rec);
    }

    // ---------- Channel menu (right-click) ----------

    fn item_by_id(&self, id: i64) -> Option<glib::BoxedAnyObject> {
        self.tv.item_by_id(id).or_else(|| self.radio.item_by_id(id))
    }

    /// Play, source choice (when there are several URLs) and, for
    /// "My channels", edit and delete.
    fn show_channel_menu(&self, anchor: &gtk::Widget, x: f64, y: f64, row: &ChannelRow) {
        let ch = &row.channel;
        let menu = gio::Menu::new();
        let play = gio::Menu::new();
        let item = gio::MenuItem::new(Some(tr("Play")), None);
        item.set_action_and_target_value(
            Some("win.play-source"),
            Some(&(ch.id, 0u32).to_variant()),
        );
        play.append_item(&item);
        menu.append_section(None, &play);

        if ch.urls.len() > 1 {
            // The source playing now for this channel, if any.
            let playing = self
                .current
                .borrow()
                .as_ref()
                .filter(|c| c.channel_id == ch.id)
                .and_then(|_| self.player.source_index());
            let sources = gio::Menu::new();
            for (i, url) in ch.urls.iter().enumerate() {
                let label = source_label(i, url, playing == Some(i));
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.play-source"),
                    Some(&(ch.id, i as u32).to_variant()),
                );
                sources.append_item(&item);
            }
            menu.append_section(Some(tr("Source")), &sources);
        }

        let fav = gio::Menu::new();
        let item = gio::MenuItem::new(
            Some(if row.favourite.get() {
                tr("Remove from favourites")
            } else {
                tr("Add to favourites")
            }),
            None,
        );
        item.set_action_and_target_value(Some("win.toggle-favourite"), Some(&ch.id.to_variant()));
        fav.append_item(&item);
        menu.append_section(None, &fav);

        let record = gio::Menu::new();
        for (label, action) in [
            (tr("Record now"), "win.record-channel"),
            (tr("Schedule recording…"), "win.schedule-channel"),
        ] {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&ch.id.to_variant()));
            record.append_item(&item);
        }
        menu.append_section(None, &record);

        if ch.is_local {
            let local = gio::Menu::new();
            for (label, action) in [
                (tr("Edit channel…"), "win.edit-channel"),
                (tr("Delete channel"), "win.delete-channel"),
            ] {
                let item = gio::MenuItem::new(Some(label), None);
                item.set_action_and_target_value(Some(action), Some(&ch.id.to_variant()));
                local.append_item(&item);
            }
            menu.append_section(None, &local);
        }

        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(anchor);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// A channel in "My channels" was edited: if it is the current one, the panel
    /// and "last channel" follow the new data (playback is not interrupted).
    pub(crate) fn channel_edited(&self, id: i64, name: &str, kind: Kind, urls: &[String]) {
        let mut cur = self.current.borrow_mut();
        let Some(c) = cur.as_mut().filter(|c| c.channel_id == id) else {
            return;
        };
        c.name = name.to_string();
        c.kind = kind;
        c.urls = urls.to_vec();
        let _ = self
            .db
            .borrow()
            .set_setting("last_channel", urls.first().cloned().unwrap_or_default());
        drop(cur);
        self.update_now_playing();
    }

    // ---------- Misc ----------

    pub fn set_status(&self, text: &str) {
        self.status_label.set_label(text);
        self.status_label.set_tooltip_text(Some(text));
    }

    fn begin_busy(&self) {
        self.busy.set(self.busy.get() + 1);
    }

    fn end_busy(&self) {
        self.busy.set(self.busy.get().saturating_sub(1));
        if self.busy.get() == 0 && !self.connecting.get() {
            self.progress.set_visible(false);
        }
    }

    fn apply_fullscreen(&self, fs: bool) {
        self.window.set_show_menubar(!fs);
        self.paned
            .start_child()
            .unwrap()
            .set_visible(!fs && self.list_visible.get());
        self.controls.set_visible(!fs);
        self.bottom.set_visible(!fs);
        self.info_bar
            .set_visible(!fs && self.current.borrow().is_some());
        if let Some(a) = self
            .window
            .lookup_action("fullscreen")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_state(&fs.to_variant());
        }
    }

    fn save_state(&self) {
        let db = self.db.borrow();
        let maximized = self.window.is_maximized();
        let _ = db.set_setting("window_maximized", maximized);
        if !maximized && !self.window.is_fullscreen() {
            let (w, h) = self.window.default_size();
            let _ = db.set_setting("window_width", w);
            let _ = db.set_setting("window_height", h);
        }
        let _ = db.set_setting("paned_position", self.paned.position());
        let _ = db.set_setting("tab", self.notebook.current_page().unwrap_or(0));
        let _ = db.set_setting("volume", self.volume.value());
    }
}

/// Access for the integration tests (`crates/tvmaxe/tests/ui.rs`).
#[doc(hidden)]
impl MainWindow {
    pub fn channel_count(&self, kind: Kind) -> u32 {
        self.list(kind).len()
    }

    /// Plays the channel at position `index`, like a double-click.
    pub fn play_channel_at(&self, kind: Kind, index: u32) -> bool {
        let Some(obj) = self.list(kind).item(index) else {
            return false;
        };
        self.play_row(&row_of(obj.upcast_ref()));
        true
    }

    pub fn channel_id_at(&self, kind: Kind, index: u32) -> Option<i64> {
        let obj = self.list(kind).item(index)?;
        Some(row_of(obj.upcast_ref()).channel.id)
    }

    pub fn visible_channel_count(&self, kind: Kind) -> u32 {
        self.list(kind).visible_len()
    }

    pub fn status_text(&self) -> String {
        self.status_label.label().to_string()
    }

    pub fn player(&self) -> &Rc<Player> {
        &self.player
    }

    fn list(&self, kind: Kind) -> &Rc<ChannelList> {
        match kind {
            Kind::Tv => &self.tv,
            Kind::Radio => &self.radio,
        }
    }
}

/// "2. backup.example/live/tvr1.m3u8"; ▶ marks the source that is playing.
fn source_label(index: usize, url: &str, playing: bool) -> String {
    let short = url.split_once("://").map_or(url, |(_, rest)| rest);
    let chars: Vec<char> = short.chars().collect();
    let short = if chars.len() > 48 {
        let head: String = chars[..30].iter().collect();
        let tail: String = chars[chars.len() - 15..].iter().collect();
        format!("{head}…{tail}")
    } else {
        short.to_string()
    };
    // In GTK menus "_" marks the access key; doubled it shows as is.
    let short = short.replace('_', "__");
    format!("{}{}. {short}", if playing { "▶ " } else { "" }, index + 1)
}

/// Local time, "20:30".
pub fn hhmm(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix)
        .and_then(|d| d.format("%H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.add_css_class("flat");
    b
}

#[cfg(test)]
mod tests {
    use super::source_label;

    #[test]
    fn source_labels() {
        assert_eq!(
            source_label(0, "https://backup.example/live/tv_1.m3u8", false),
            "1. backup.example/live/tv__1.m3u8",
            "no scheme; \"_\" doubled for GTK menus"
        );
        assert!(source_label(2, "rtmp://a/b", true).starts_with("▶ 3. "));
        let long = format!("http://example.org/{}", "x".repeat(80));
        let l = source_label(0, &long, false);
        assert!(l.chars().count() < 60 && l.contains('…'), "{l}");
    }
}
