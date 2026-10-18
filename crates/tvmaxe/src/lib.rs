//! TV-Maxe: the GTK 4 application. The binary (`main.rs`) only calls [`run`]; the rest
//! is a library so the integration tests (`tests/`) can build the window and
//! the player.

mod channel_list;
mod dialogs;
mod mpris;
pub mod player;
mod settings;
pub mod sleep;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use tvmaxe_core::i18n::tr;
pub use window::MainWindow;

pub const APP_ID: &str = "io.condor2.TvMaxe";

thread_local! {
    static WINDOW: RefCell<Option<Rc<MainWindow>>> = const { RefCell::new(None) };
}

/// Starts the application (GTK main loop).
pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(setup_app);

    app.connect_activate(|app| {
        if let Some(w) = window(app) {
            w.window.present();
        }
    });

    // `tvmaxe list.db`, `tvmaxe list.m3u` or `tvmaxe https://…` → new subscription.
    app.connect_open(|app, files, _| {
        let Some(w) = window(app) else { return };
        for f in files {
            let src = match f.uri_scheme().as_deref() {
                Some("http" | "https") => f.uri().to_string(),
                _ => f
                    .path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            };
            w.add_subscription(&src);
        }
        w.window.present();
    });

    app.run()
}

/// Menu, application actions, shortcuts and style. Runs on `startup`;
/// public for the integration tests, which build the application themselves.
#[doc(hidden)]
pub fn setup_app(app: &gtk::Application) {
    app.set_menubar(Some(&menu_bar()));
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = a.upgrade() {
            // Goes through close-request, so the settings are saved.
            app.windows().iter().for_each(|w| w.close());
        }
    });
    app.add_action(&quit);
    for (action, keys) in [
        ("app.quit", &["<Ctrl>q"][..]),
        ("win.subscriptions", &["<Ctrl>b"]),
        ("win.refresh", &["<Ctrl>r", "F5"]),
        ("win.add-channel", &["<Ctrl>n"]),
        ("win.preferences", &["<Ctrl>comma"]),
        ("win.guide", &["<Ctrl>g"]),
        ("win.recordings", &["<Ctrl>j"]),
        ("win.record", &["<Ctrl><Shift>r"]),
        ("win.sleep-timer", &["<Ctrl>t"]),
        ("win.favourite-current", &["<Ctrl>d"]),
        ("win.search", &["<Ctrl>f"]),
        ("win.fullscreen", &["F11"]),
        ("win.show-list", &["F9"]),
    ] {
        app.set_accels_for_action(action, keys);
    }
    load_css();
}

fn window(app: &gtk::Application) -> Option<Rc<MainWindow>> {
    if let Some(w) = WINDOW.with_borrow(|w| w.clone()) {
        return Some(w);
    }
    let db_path = glib::user_data_dir().join("tvmaxe").join("tvmaxe.db");
    match MainWindow::new(app, db_path) {
        Ok(w) => {
            w.start_mpris();
            WINDOW.with_borrow_mut(|slot| *slot = Some(w.clone()));
            Some(w)
        }
        Err(e) => {
            eprintln!("TV-Maxe cannot start: {e:#}");
            show_startup_error(app, &format!("{e:#}"));
            None
        }
    }
}

/// Started from the menu, the application has no terminal: the error must be shown in a window.
fn show_startup_error(app: &gtk::Application, detail: &str) {
    let guard = app.hold();
    let app = app.clone();
    let dialog = gtk::AlertDialog::builder()
        .message(tr("TV-Maxe cannot start"))
        .detail(detail)
        .buttons([tr("Close")])
        .build();
    glib::spawn_future_local(async move {
        let _ = dialog.choose_future(None::<&gtk::Window>).await;
        drop(guard);
        app.quit();
    });
}

fn menu_bar() -> gio::Menu {
    let tvmaxe = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("Subscriptions…")), Some("win.subscriptions"));
    s1.append(Some(tr("Xtream Codes account…")), Some("win.add-xtream"));
    s1.append(Some(tr("Update lists")), Some("win.refresh"));
    let s2 = gio::Menu::new();
    s2.append(Some(tr("Add channel…")), Some("win.add-channel"));
    s2.append(Some(tr("Save local list…")), Some("win.export-local"));
    let rec = gio::Menu::new();
    rec.append(Some(tr("Record current channel")), Some("win.record"));
    rec.append(Some(tr("Recordings…")), Some("win.recordings"));
    rec.append(Some(tr("Sleep timer…")), Some("win.sleep-timer"));
    let s3 = gio::Menu::new();
    s3.append(Some(tr("Preferences…")), Some("win.preferences"));
    s3.append(Some(tr("Quit")), Some("app.quit"));
    tvmaxe.append_section(None, &s1);
    tvmaxe.append_section(None, &s2);
    tvmaxe.append_section(None, &rec);
    tvmaxe.append_section(None, &s3);

    let view = gio::Menu::new();
    view.append(Some(tr("Fullscreen")), Some("win.fullscreen"));
    view.append(Some(tr("Channel list")), Some("win.show-list"));
    view.append(Some(tr("Search channel")), Some("win.search"));
    view.append(Some(tr("TV guide")), Some("win.guide"));
    let zap = gio::Menu::new();
    // Handled by the window's key controller (they must not steal keys from
    // text fields), so the keys are only shown here.
    for (label, action, accel) in [
        (tr("Next channel"), "win.zap-next", "Page_Down"),
        (tr("Previous channel"), "win.zap-prev", "Page_Up"),
        (tr("Last channel watched"), "win.zap-back", "BackSpace"),
        (tr("Next audio track"), "win.cycle-audio", "a"),
    ] {
        let item = gio::MenuItem::new(Some(label), Some(action));
        item.set_attribute_value("accel", Some(&accel.to_variant()));
        zap.append_item(&item);
    }
    let fav = gio::Menu::new();
    fav.append(Some(tr("Show favourites")), Some("win.show-favourites"));
    fav.append(
        Some(tr("Add to / remove from favourites")),
        Some("win.favourite-current"),
    );
    view.append_section(None, &zap);
    view.append_section(None, &fav);

    let help = gio::Menu::new();
    help.append(Some(tr("About TV-Maxe")), Some("win.about"));

    let bar = gio::Menu::new();
    bar.append_submenu(Some("_TV-Maxe"), &tvmaxe);
    bar.append_submenu(Some(tr("_View")), &view);
    bar.append_submenu(Some(tr("_Help")), &help);
    bar
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".video-area { background: black; }
         .video-placeholder { background: black; }
         .video-placeholder label { color: #d0d0d0; font-size: 1.4em; }
         .video-placeholder image { color: #6a6a6a; }
         .channel-title { font-weight: bold; }
         .epg-progress progress, .epg-progress trough { min-height: 4px; }
         .epg-current { background: alpha(@theme_selected_bg_color, 0.18); border-radius: 6px; }
         .epg-past { opacity: 0.55; }
         .recording-indicator { color: #e01b24; font-weight: bold; }
         .recording-active { color: #e01b24; }
         .favourite-star { color: #e5a50a; }
         .zap-osd { font-size: 1.6em; font-weight: bold; padding: 6px 14px; border-radius: 8px; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
