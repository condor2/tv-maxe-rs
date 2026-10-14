//! Integration tests for the interface: the main window, the secondary windows
//! (opened and closed several times, so the "A window is shown after it has
//! been destroyed" bug does not come back), a subscription from a local M3U
//! and playing a channel with a fallback URL.
//!
//! Any GTK warning during a test makes it fail.
//!
//! ```sh
//! xvfb-run -a cargo test -p tvmaxe --test ui
//! ```

#[macro_use]
mod common;

use common::{DEAD_URL, TEST_AUDIO, TEST_VIDEO, TempDir, TestResult, wait_until};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tvmaxe::MainWindow;
use tvmaxe_core::Kind;

const TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    common::run(
        "ui",
        &[
            ("main_window_starts", || Box::pin(main_window_starts())),
            ("local_m3u_subscription_and_playback", || {
                Box::pin(local_m3u_and_play())
            }),
            // After the test above: "TV guide" needs a selected channel.
            ("dialogs_reopen_after_close", || Box::pin(dialogs_reopen())),
            ("channel_list_hide_and_show", || {
                Box::pin(toggle_channel_list())
            }),
            ("choose_source", || Box::pin(choose_source())),
            ("edit_local_channel", || Box::pin(edit_local_channel())),
            // Last: closes the main window.
            ("close_while_playing", || Box::pin(close_while_playing())),
        ],
    );
}

struct Fixture {
    _dir: TempDir,
    _app: gtk::Application,
    main: Rc<MainWindow>,
}

thread_local! {
    static FIXTURE: RefCell<Option<Rc<Fixture>>> = const { RefCell::new(None) };
}

/// The application and the main window, created once, with the database
/// in a temporary directory (the user's data is not touched).
fn fixture() -> Result<Rc<Fixture>, String> {
    if let Some(f) = FIXTURE.with_borrow(|f| f.clone()) {
        return Ok(f);
    }
    let dir = TempDir::new("ui");
    let app = gtk::Application::builder()
        .application_id("io.condor2.TvMaxe.Tests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(tvmaxe::setup_app);
    app.register(None::<&gio::Cancellable>)
        .map_err(|e| format!("the application does not register: {e}"))?;
    let main = MainWindow::new(&app, dir.0.join("tvmaxe.db")).map_err(|e| format!("{e:#}"))?;
    main.window.present();
    let f = Rc::new(Fixture {
        _dir: dir,
        _app: app,
        main,
    });
    FIXTURE.with_borrow_mut(|slot| *slot = Some(f.clone()));
    Ok(f)
}

/// The visible windows, except the main one.
fn secondary_windows(main: &gtk::Window) -> Vec<gtk::Window> {
    let list = gtk::Window::toplevels();
    (0..list.n_items())
        .filter_map(|i| list.item(i).and_downcast::<gtk::Window>())
        .filter(|w| w != main && w.is_visible())
        .collect()
}

async fn main_window_starts() -> TestResult {
    let f = fixture()?;
    let win = f.main.window.clone();
    wait_until("main window shown", TIMEOUT, || win.is_mapped()).await?;
    check!(
        win.title().as_deref() == Some("TV-Maxe"),
        "title: {:?}",
        win.title()
    );
    check!(
        f.main.channel_count(Kind::Tv) == 0,
        "the new database has TV channels"
    );
    check!(
        win.application().is_some_and(|a| a.menubar().is_some()),
        "the application menu is missing"
    );
    // Without a selected channel, "TV guide" explains in the status bar instead of opening.
    WidgetExt::activate_action(&win, "win.guide", None).map_err(|e| e.to_string())?;
    glib::timeout_future(Duration::from_millis(200)).await;
    check!(
        secondary_windows(win.upcast_ref()).is_empty(),
        "TV guide opened without a channel"
    );
    check!(
        f.main.status_text().starts_with("Choose a channel"),
        "status: {:?}",
        f.main.status_text()
    );
    Ok(())
}

/// Every secondary window: opened and closed 3 times. Opening it again must
/// work (hidden window reused or a new one), without GTK warnings.
async fn dialogs_reopen() -> TestResult {
    let f = fixture()?;
    let main: gtk::Window = f.main.window.clone().upcast();
    for action in [
        "subscriptions",
        "guide",
        "preferences",
        "add-channel",
        "about",
    ] {
        for round in 1..=3 {
            WidgetExt::activate_action(&f.main.window, &format!("win.{action}"), None)
                .map_err(|e| format!("win.{action}: {e}"))?;
            let what = format!("{action}: window appears (round {round})");
            wait_until(&what, TIMEOUT, || {
                secondary_windows(&main).iter().any(|w| w.is_mapped())
            })
            .await?;
            let opened = secondary_windows(&main);
            check!(
                opened.len() == 1,
                "{action}: {} windows open instead of 1",
                opened.len()
            );
            opened[0].close();
            let what = format!("{action}: window closes (round {round})");
            wait_until(&what, TIMEOUT, || secondary_windows(&main).is_empty()).await?;
        }
    }
    Ok(())
}

/// An M3U from disk as a subscription: the TV channel has a dead URL first, then a
/// good one; the "Radio" group moves the channel to the radio tab.
async fn local_m3u_and_play() -> TestResult {
    let f = fixture()?;
    let dir = TempDir::new("m3u");
    let path = dir.0.join("test.m3u");
    std::fs::write(
        &path,
        format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"test.tv\" group-title=\"Test\",Canal test\n\
             {DEAD_URL}\n{TEST_VIDEO}\n\
             #EXTINF:-1 group-title=\"Radio\",Radio test\n\
             {TEST_AUDIO}\n"
        ),
    )
    .unwrap();
    f.main.add_subscription(path.to_str().unwrap());
    wait_until("M3U channels in the lists", TIMEOUT, || {
        f.main.channel_count(Kind::Tv) == 1 && f.main.channel_count(Kind::Radio) == 1
    })
    .await?;

    check!(f.main.play_channel_at(Kind::Tv, 0), "nu pot porni canalul");
    wait_until("status \"Playing: Canal test\"", TIMEOUT, || {
        f.main.status_text() == "Playing: Canal test"
    })
    .await
    .map_err(|e| format!("{e}; status: {:?}", f.main.status_text()))?;

    f.main.player().stop();
    check!(
        f.main.status_text() == "Stopped",
        "status after Stop: {:?}",
        f.main.status_text()
    );
    Ok(())
}

/// F9: the channel list hides and shows again.
async fn toggle_channel_list() -> TestResult {
    let f = fixture()?;
    let state = || {
        f.main
            .window
            .lookup_action("show-list")
            .and_then(|a| a.state())
            .and_then(|s| s.get::<bool>())
    };
    check!(state() == Some(true), "the list is not visible at startup");
    for expected in [false, true] {
        WidgetExt::activate_action(&f.main.window, "win.show-list", None)
            .map_err(|e| e.to_string())?;
        glib::timeout_future(Duration::from_millis(100)).await;
        check!(state() == Some(expected), "show-list = {:?}", state());
    }
    Ok(())
}

/// Closing the window during playback destroys the video area while mpv is still
/// drawing (this was a use-after-free in libmpv2's redraw callback).
async fn close_while_playing() -> TestResult {
    let f = fixture()?;
    check!(f.main.play_channel_at(Kind::Tv, 0), "nu pot porni canalul");
    wait_until("playing before closing", TIMEOUT, || {
        f.main.status_text() == "Playing: Canal test"
    })
    .await?;
    glib::timeout_future(Duration::from_millis(500)).await;
    f.main.window.close();
    let win = f.main.window.clone();
    wait_until("main window closed", TIMEOUT, || !win.is_visible()).await?;
    // mpv still renders a few frames on its own thread; a crash would stop the process here.
    glib::timeout_future(Duration::from_millis(1500)).await;
    Ok(())
}

/// Right-click → Source: plays the chosen source directly; if it is dead, moves
/// on to the next one (here: source 1 dead → 2).
async fn choose_source() -> TestResult {
    let f = fixture()?;
    let id = f.main.channel_id_at(Kind::Tv, 0).ok_or("no TV channel")?;
    for (chosen, expected) in [(1u32, 1usize), (0, 1)] {
        WidgetExt::activate_action(
            &f.main.window,
            "win.play-source",
            Some(&(id, chosen).to_variant()),
        )
        .map_err(|e| e.to_string())?;
        wait_until("playing from the chosen source", TIMEOUT, || {
            f.main.status_text() == "Playing: Canal test"
                && f.main.player().source_index() == Some(expected)
        })
        .await
        .map_err(|e| {
            format!(
                "{e}; source {chosen}: status {:?}, index {:?}",
                f.main.status_text(),
                f.main.player().source_index()
            )
        })?;
    }
    f.main.player().stop();
    Ok(())
}

/// All descendants of a widget of type `T`.
fn descendants<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Vec<T> {
    let mut out = Vec::new();
    let mut child = root.first_child();
    while let Some(w) = child {
        if let Ok(t) = w.clone().downcast::<T>() {
            out.push(t);
        }
        out.extend(descendants::<T>(&w));
        child = w.next_sibling();
    }
    out
}

/// "Edit channel…" on a channel in "My channels": the form comes pre-filled,
/// and "Save" changes the channel (it does not add another one). Subscription
/// channels cannot be edited.
async fn edit_local_channel() -> TestResult {
    let f = fixture()?;
    let main: gtk::Window = f.main.window.clone().upcast();
    let id = f
        .main
        .db
        .borrow()
        .add_local_channel_urls(
            Kind::Tv,
            "Local vechi",
            &["http://a/1".into(), "http://a/2".into()],
        )
        .map_err(|e| e.to_string())?;
    f.main.reload();
    let tv_before = f.main.channel_count(Kind::Tv);

    WidgetExt::activate_action(&f.main.window, "win.edit-channel", Some(&id.to_variant()))
        .map_err(|e| e.to_string())?;
    wait_until("edit window", TIMEOUT, || {
        secondary_windows(&main).iter().any(|w| w.is_mapped())
    })
    .await?;
    let dialog = secondary_windows(&main).remove(0);
    check!(
        dialog.title().as_deref() == Some("Edit channel"),
        "title: {:?}",
        dialog.title()
    );
    let root: gtk::Widget = dialog.clone().upcast();
    let entry = descendants::<gtk::Entry>(&root)
        .into_iter()
        .next()
        .ok_or("the Name field is missing")?;
    check!(entry.text() == "Local vechi", "name: {:?}", entry.text());
    let view = descendants::<gtk::TextView>(&root)
        .into_iter()
        .next()
        .ok_or("the URLs field is missing")?;
    let b = view.buffer();
    let text = b.text(&b.start_iter(), &b.end_iter(), false);
    check!(text == "http://a/1\nhttp://a/2", "URLs: {text:?}");

    entry.set_text("Local nou");
    b.set_text("http://a/2\nhttp://a/3");
    let save = descendants::<gtk::Button>(&root)
        .into_iter()
        .find(|b| b.label().as_deref() == Some("Save"))
        .ok_or("the Save button is missing")?;
    save.emit_clicked();
    wait_until("window closes", TIMEOUT, || {
        secondary_windows(&main).is_empty()
    })
    .await?;

    check!(
        f.main.channel_count(Kind::Tv) == tv_before,
        "an extra channel was added"
    );
    let channels = f
        .main
        .db
        .borrow()
        .channels(Kind::Tv)
        .map_err(|e| e.to_string())?;
    let ch = channels
        .iter()
        .find(|c| c.id == id)
        .ok_or("the channel disappeared")?;
    check!(ch.name == "Local nou", "saved name: {:?}", ch.name);
    check!(
        ch.urls == ["http://a/2", "http://a/3"],
        "saved URLs: {:?}",
        ch.urls
    );

    // A subscription channel does not open the form.
    let sub_id = channels
        .iter()
        .find(|c| !c.is_local)
        .ok_or("no subscription channel")?
        .id;
    WidgetExt::activate_action(
        &f.main.window,
        "win.edit-channel",
        Some(&sub_id.to_variant()),
    )
    .map_err(|e| e.to_string())?;
    glib::timeout_future(Duration::from_millis(300)).await;
    check!(
        secondary_windows(&main).is_empty(),
        "the edit form opened for a subscription channel"
    );

    // Clean-up: the next test closes the window while channel 0 is playing.
    let _ = f.main.db.borrow().delete_local_channel(id);
    f.main.reload();
    Ok(())
}
