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
use tvmaxe_core::formats::xtream::Account;
use tvmaxe_core::recording::{Recording, Repeat, Status};

const TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    common::run(
        "ui",
        &[
            ("main_window_starts", || Box::pin(main_window_starts())),
            ("local_m3u_subscription_and_playback", || {
                Box::pin(local_m3u_and_play())
            }),
            ("dialogs_reopen_after_close", || Box::pin(dialogs_reopen())),
            ("channel_list_hide_and_show", || {
                Box::pin(toggle_channel_list())
            }),
            ("choose_source", || Box::pin(choose_source())),
            ("edit_local_channel", || Box::pin(edit_local_channel())),
            ("recording", || Box::pin(recording())),
            ("schedule_form", || Box::pin(schedule_form())),
            ("sleep_timer", || Box::pin(sleep_timer())),
            ("favourites_and_zapping", || {
                Box::pin(favourites_and_zapping())
            }),
            ("xtream_account", || Box::pin(xtream_account())),
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
    Ok(())
}

/// Every secondary window: opened and closed 3 times. Opening it again must
/// work (hidden window reused or a new one), without GTK warnings.
async fn dialogs_reopen() -> TestResult {
    let f = fixture()?;
    let main: gtk::Window = f.main.window.clone().upcast();
    for action in [
        "subscriptions",
        "preferences",
        "add-channel",
        "about",
        "recordings",
        "sleep-timer",
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

fn recordings_of(f: &Fixture, title: &str) -> Vec<Recording> {
    f.main
        .db
        .borrow()
        .recordings()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.display_title() == title)
        .collect()
}

/// What the recordings look like right now, for the message of a failed wait.
fn recording_state(f: &Fixture) -> String {
    let rows: Vec<String> = f
        .main
        .db
        .borrow()
        .recordings()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let files: Vec<String> = r
                .files
                .iter()
                .map(|p| {
                    format!(
                        "{p} ({} bytes)",
                        std::fs::metadata(p).map_or(0, |m| m.len())
                    )
                })
                .collect();
            format!(
                "#{} {:?} status={:?} error={:?} files={files:?}",
                r.id,
                r.display_title(),
                r.status,
                r.error
            )
        })
        .collect();
    format!(
        "active recorders: {}; status bar: {:?}; recordings: {rows:?}",
        f.main.active_recordings(),
        f.main.status_text()
    )
}

/// Recording end to end: the ● button (start and stop), a scheduled
/// recording that starts and stops by itself, and a missed daily recording
/// that is moved to the next day.
async fn recording() -> TestResult {
    let f = fixture()?;
    let dir = TempDir::new("recordings");
    let Some(src) = common::ts_source(&dir.0, 30) else {
        return Ok(());
    };
    let out = dir.0.join("out");
    let mut s = f.main.settings();
    s.recordings_dir = Some(out.clone());
    f.main.set_settings(s);

    let id = f
        .main
        .db
        .borrow()
        .add_local_channel(Kind::Tv, "Rec test", src.to_str().unwrap())
        .map_err(|e| e.to_string())?;
    f.main.reload();

    // ● while playing: records the current channel until pressed again.
    WidgetExt::activate_action(
        &f.main.window,
        "win.play-source",
        Some(&(id, 0u32).to_variant()),
    )
    .map_err(|e| e.to_string())?;
    wait_until("playing the source", TIMEOUT, || {
        f.main.status_text() == "Playing: Rec test"
    })
    .await?;
    WidgetExt::activate_action(&f.main.window, "win.record", None).map_err(|e| e.to_string())?;
    wait_until("one active recording", TIMEOUT, || {
        f.main.active_recordings() == 1
    })
    .await
    .map_err(|e| format!("{e}; {}", recording_state(&f)))?;
    wait_until("the recording has a file with data", TIMEOUT, || {
        recordings_of(&f, "Rec test")
            .iter()
            .any(|r| r.status == Status::Recording && !r.files.is_empty())
    })
    .await
    .map_err(|e| format!("{e}; {}", recording_state(&f)))?;
    // The sleep timer never ends a running recording: "Quit" only stops playback.
    f.main
        .start_sleep_timer(Duration::from_secs(1), tvmaxe::sleep::SleepAction::Quit);
    wait_until("sleep timer ended", TIMEOUT, || {
        f.main.sleep_remaining().is_none()
    })
    .await?;
    check!(
        f.main.window.is_visible() && f.main.active_recordings() == 1,
        "the sleep timer quit during a recording"
    );
    check!(
        f.main.status_text().contains("stays open"),
        "status: {:?}",
        f.main.status_text()
    );
    // Back to the channel, so the ● button refers to it again.
    WidgetExt::activate_action(
        &f.main.window,
        "win.play-source",
        Some(&(id, 0u32).to_variant()),
    )
    .map_err(|e| e.to_string())?;
    glib::timeout_future(Duration::from_secs(1)).await;
    WidgetExt::activate_action(&f.main.window, "win.record", None).map_err(|e| e.to_string())?;
    check!(
        f.main.active_recordings() == 0,
        "still recording after the second press"
    );
    let done = recordings_of(&f, "Rec test");
    check!(
        done.len() == 1 && done[0].status == Status::Done && done[0].stop.is_some(),
        "manual recording: {done:?}"
    );
    let file = std::path::Path::new(&done[0].files[0]);
    check!(
        file.starts_with(&out)
            && file
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Rec test ")
            && std::fs::metadata(file).map(|m| m.len()).unwrap_or(0) > 10_000,
        "recorded file: {file:?}"
    );
    f.main.player().stop();

    // Scheduled: starts and stops on its own.
    let now = tvmaxe_core::net::unix_now();
    let mut r = Recording::new(
        "Rec test",
        Kind::Tv,
        &[src.to_string_lossy().into()],
        &Default::default(),
        now,
        Some(now + 3),
    );
    r.title = "Scheduled".into();
    f.main
        .db
        .borrow()
        .add_recording(&r)
        .map_err(|e| e.to_string())?;
    wait_until(
        "scheduled recording finished",
        Duration::from_secs(30),
        || {
            recordings_of(&f, "Scheduled")
                .iter()
                .any(|r| r.status == Status::Done)
        },
    )
    .await
    .map_err(|e| format!("{e}; {:?}", recordings_of(&f, "Scheduled")))?;
    let done = &recordings_of(&f, "Scheduled")[0];
    check!(
        done.files
            .iter()
            .all(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0)),
        "scheduled files: {:?}",
        done.files
    );

    // Missed while TV-Maxe was closed; a daily one moves to the next day.
    let mut r = Recording::new(
        "Rec test",
        Kind::Tv,
        &[src.to_string_lossy().into()],
        &Default::default(),
        now - 7200,
        Some(now - 3600),
    );
    r.title = "Daily".into();
    r.repeat = Repeat::Daily;
    f.main
        .db
        .borrow()
        .add_recording(&r)
        .map_err(|e| e.to_string())?;
    wait_until("missed + rescheduled", TIMEOUT, || {
        let v = recordings_of(&f, "Daily");
        v.iter().any(|r| r.status == Status::Missed)
            && v.iter()
                .any(|r| r.status == Status::Scheduled && r.start == now - 7200 + 86400)
    })
    .await
    .map_err(|e| format!("{e}; {:?}", recordings_of(&f, "Daily")))?;

    // Clean-up for the tests that follow.
    for r in f.main.db.borrow().recordings().unwrap_or_default() {
        let _ = f.main.db.borrow().delete_recording(r.id);
    }
    let _ = f.main.db.borrow().delete_local_channel(id);
    f.main.reload();
    Ok(())
}

/// Channel menu → "Schedule recording…": the form opens with the channel and
/// the next full hour, and "Schedule" stores it.
async fn schedule_form() -> TestResult {
    let f = fixture()?;
    let main: gtk::Window = f.main.window.clone().upcast();
    let id = f.main.channel_id_at(Kind::Tv, 0).ok_or("no TV channel")?;
    WidgetExt::activate_action(
        &f.main.window,
        "win.schedule-channel",
        Some(&id.to_variant()),
    )
    .map_err(|e| e.to_string())?;
    wait_until("schedule form", TIMEOUT, || {
        secondary_windows(&main).iter().any(|w| w.is_mapped())
    })
    .await?;
    let dialog = secondary_windows(&main).remove(0);
    check!(
        dialog.title().as_deref() == Some("Schedule recording"),
        "title: {:?}",
        dialog.title()
    );
    let root: gtk::Widget = dialog.clone().upcast();
    let entry = descendants::<gtk::Entry>(&root)
        .into_iter()
        .next()
        .ok_or("the Title field is missing")?;
    entry.set_text("From the form");
    let save = descendants::<gtk::Button>(&root)
        .into_iter()
        .find(|b| b.label().as_deref() == Some("Schedule"))
        .ok_or("the Schedule button is missing")?;
    check!(
        save.is_sensitive(),
        "Schedule is disabled for a future time"
    );
    save.emit_clicked();
    wait_until("form closes", TIMEOUT, || {
        secondary_windows(&main).is_empty()
    })
    .await?;
    let recs = recordings_of(&f, "From the form");
    check!(
        recs.len() == 1
            && recs[0].status == Status::Scheduled
            && recs[0].channel == "Canal test"
            && recs[0].stop.is_some_and(|s| s - recs[0].start == 3600),
        "scheduled: {recs:?}"
    );
    let _ = f.main.db.borrow().delete_recording(recs[0].id);
    Ok(())
}

/// The sleep timer stops playback at the end (volume put back), and can be
/// cancelled.
async fn sleep_timer() -> TestResult {
    use tvmaxe::sleep::SleepAction;
    let f = fixture()?;
    check!(
        f.main.play_channel_at(Kind::Tv, 0),
        "cannot play the channel"
    );
    wait_until("playing", TIMEOUT, || {
        f.main.status_text() == "Playing: Canal test"
    })
    .await?;

    f.main
        .start_sleep_timer(Duration::from_secs(2), SleepAction::StopPlayback);
    check!(
        f.main
            .sleep_remaining()
            .is_some_and(|d| d <= Duration::from_secs(2)),
        "timer not running: {:?}",
        f.main.sleep_remaining()
    );
    wait_until("playback stopped by the timer", TIMEOUT, || {
        !f.main.player().is_active()
    })
    .await?;
    check!(
        f.main.status_text() == "Sleep timer: playback stopped",
        "status: {:?}",
        f.main.status_text()
    );
    check!(f.main.sleep_remaining().is_none(), "timer still running");

    f.main
        .start_sleep_timer(Duration::from_secs(600), SleepAction::Quit);
    check!(
        f.main.sleep_action() == Some(SleepAction::Quit),
        "action not kept"
    );
    f.main.cancel_sleep_timer();
    check!(
        f.main.sleep_remaining().is_none(),
        "cancel did not stop the timer"
    );
    check!(
        f.main.status_text() == "Sleep timer cancelled",
        "status: {:?}",
        f.main.status_text()
    );
    Ok(())
}

/// Favourites (★ list) and zapping: next / previous with wrap-around,
/// channel numbers, back to the last channel, and the OSD.
async fn favourites_and_zapping() -> TestResult {
    let f = fixture()?;
    let mut ids = Vec::new();
    for (i, name) in ["Zap A", "Zap B", "Zap C"].iter().enumerate() {
        // Distinct first URLs (channels are identified by them).
        let url = TEST_VIDEO.replace("duration=600", &format!("duration={}", 600 + i));
        let id = f
            .main
            .db
            .borrow()
            .add_local_channel(Kind::Tv, name, &url)
            .map_err(|e| e.to_string())?;
        ids.push(id);
    }
    f.main.reload();
    let total = f.main.visible_channel_count(Kind::Tv);
    check!(total >= 4, "visible TV channels: {total}");

    let playing = |name: &str| f.main.status_text() == format!("Playing: {name}");
    // Find Zap A's position among the shown channels by zapping to its number.
    let mut pos_a = None;
    for n in 1..=total {
        f.main.zap_to_number(n);
        if f.main.osd_text().is_some_and(|t| t.ends_with("Zap A")) {
            pos_a = Some(n);
            break;
        }
    }
    let pos_a = pos_a.ok_or("Zap A not found by number")?;
    wait_until("Zap A playing", TIMEOUT, || playing("Zap A"))
        .await
        .map_err(|e| {
            format!(
                "{e}; status {:?}, OSD {:?}",
                f.main.status_text(),
                f.main.osd_text()
            )
        })?;
    check!(
        f.main.osd_text() == Some(format!("{pos_a}  Zap A")),
        "OSD: {:?}",
        f.main.osd_text()
    );

    f.main.zap(1);
    wait_until("Zap B after next", TIMEOUT, || playing("Zap B")).await?;
    f.main.zap(-1);
    wait_until("Zap A after previous", TIMEOUT, || playing("Zap A")).await?;
    f.main.zap_back();
    wait_until("Zap B after back", TIMEOUT, || playing("Zap B")).await?;
    f.main.zap_to_number(total + 5);
    check!(
        f.main.osd_text() == Some(format!("No channel {}", total + 5)),
        "OSD: {:?}",
        f.main.osd_text()
    );

    // Favourites: two channels, then the ★ view shows only them.
    for id in &ids[..2] {
        WidgetExt::activate_action(
            &f.main.window,
            "win.toggle-favourite",
            Some(&id.to_variant()),
        )
        .map_err(|e| e.to_string())?;
    }
    let favs = f.main.db.borrow().favourites(Kind::Tv).unwrap_or_default();
    check!(favs.len() == 2, "stored favourites: {favs:?}");
    WidgetExt::activate_action(&f.main.window, "win.show-favourites", None)
        .map_err(|e| e.to_string())?;
    check!(
        f.main.visible_channel_count(Kind::Tv) == 2,
        "favourites view shows {}",
        f.main.visible_channel_count(Kind::Tv)
    );
    // Zapping stays inside the favourites and wraps around.
    f.main.zap(1);
    f.main.zap(1);
    wait_until("zapping among favourites", TIMEOUT, || {
        playing("Zap A") || playing("Zap B")
    })
    .await?;
    // Favourites survive a list reload (rows are rebuilt).
    f.main.reload();
    check!(
        f.main.visible_channel_count(Kind::Tv) == 2,
        "after reload the favourites view shows {}",
        f.main.visible_channel_count(Kind::Tv)
    );

    // Clean-up: remove the favourites (the ★ entry disappears) and the channels.
    for id in &ids[..2] {
        WidgetExt::activate_action(
            &f.main.window,
            "win.toggle-favourite",
            Some(&id.to_variant()),
        )
        .map_err(|e| e.to_string())?;
    }
    check!(
        f.main
            .db
            .borrow()
            .favourites(Kind::Tv)
            .unwrap_or_default()
            .is_empty(),
        "favourites not removed"
    );
    check!(
        f.main.visible_channel_count(Kind::Tv) == total,
        "without favourites the list shows {} of {total}",
        f.main.visible_channel_count(Kind::Tv)
    );
    f.main.player().stop();
    for id in ids {
        let _ = f.main.db.borrow().delete_local_channel(id);
    }
    f.main.reload();
    Ok(())
}

/// A fake Xtream Codes panel with one user, "ana" / "s3cret".
fn xtream_panel() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let target = request.split(' ').nth(1).unwrap_or("");
            let body = if !target.contains("username=ana&password=s3cret") {
                r#"{"user_info":{"auth":0}}"#
            } else if target.contains("get_live_categories") {
                r#"[{"category_id":"1","category_name":"Romania"}]"#
            } else if target.contains("get_live_streams") {
                r#"[{"name":"XT Pro TV","stream_type":"live","stream_id":1,"category_id":"1"},
                    {"name":"XT Antena","stream_type":"live","stream_id":2,"category_id":"1"}]"#
            } else {
                r#"{"user_info":{"auth":1,"status":"Active"}}"#
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    base
}

/// "Xtream Codes account…": the form only enables *Add* when it is filled in,
/// the account becomes a subscription named after the server (no password in
/// the name), its channels appear, and a wrong password is reported.
async fn xtream_account() -> TestResult {
    let f = fixture()?;
    let main: gtk::Window = f.main.window.clone().upcast();
    let server = xtream_panel();
    let tv_before = f.main.channel_count(Kind::Tv);

    WidgetExt::activate_action(&f.main.window, "win.add-xtream", None)
        .map_err(|e| e.to_string())?;
    wait_until("Xtream form", TIMEOUT, || {
        secondary_windows(&main).iter().any(|w| w.is_mapped())
    })
    .await?;
    let dialog = secondary_windows(&main).remove(0);
    check!(
        dialog.title().as_deref() == Some("Xtream Codes account"),
        "title: {:?}",
        dialog.title()
    );
    let root: gtk::Widget = dialog.clone().upcast();
    let entries = descendants::<gtk::Entry>(&root);
    let (server_entry, user_entry) = match entries.as_slice() {
        [server, user, ..] => (server.clone(), user.clone()),
        _ => return Err("the Server / User name fields are missing".into()),
    };
    let password = descendants::<gtk::PasswordEntry>(&root)
        .into_iter()
        .next()
        .ok_or("the Password field is missing")?;
    let add = descendants::<gtk::Button>(&root)
        .into_iter()
        .find(|b| b.label().as_deref() == Some("Add"))
        .ok_or("the Add button is missing")?;

    check!(!add.is_sensitive(), "Add is enabled with an empty form");
    server_entry.set_text(&server);
    user_entry.set_text("ana");
    check!(!add.is_sensitive(), "Add is enabled without a password");
    password.set_text("s3cret");
    check!(add.is_sensitive(), "Add is disabled with a full form");
    add.emit_clicked();
    wait_until("the form closes", TIMEOUT, || {
        secondary_windows(&main).is_empty()
    })
    .await?;

    wait_until("the Xtream channels", TIMEOUT, || {
        f.main.channel_count(Kind::Tv) == tv_before + 2
    })
    .await?;
    let host = server.trim_start_matches("http://").to_string();
    let subs = f
        .main
        .db
        .borrow()
        .subscriptions()
        .map_err(|e| e.to_string())?;
    let sub = subs
        .iter()
        .find(|s| s.name == host)
        .ok_or_else(|| format!("no subscription named {host}"))?;
    check!(sub.last_error.is_none(), "error: {:?}", sub.last_error);
    check!(
        !sub.name.contains("s3cret") && !sub.display_url().contains("s3cret"),
        "the password is visible: {} / {}",
        sub.name,
        sub.display_url()
    );
    let id = sub.id;

    // A wrong password: the error is kept on the subscription.
    let wrong = Account::new(&server, "ana", "hunter2x").ok_or("account")?;
    f.main.add_xtream(&wrong);
    wait_until("the wrong password is reported", TIMEOUT, || {
        f.main
            .db
            .borrow()
            .subscriptions()
            .map(|subs| {
                subs.iter().any(|s| {
                    s.id != id
                        && s.name == host
                        && s.last_error.as_deref() == Some("wrong user name or password")
                })
            })
            .unwrap_or(false)
    })
    .await?;

    // Clean-up: the next tests count the channels.
    for s in f
        .main
        .db
        .borrow()
        .subscriptions()
        .map_err(|e| e.to_string())?
    {
        if s.name == host {
            let _ = f.main.db.borrow().remove_subscription(s.id);
        }
    }
    f.main.reload();
    check!(
        f.main.channel_count(Kind::Tv) == tv_before,
        "the Xtream channels stayed"
    );
    Ok(())
}
