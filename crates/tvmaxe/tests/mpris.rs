//! MPRIS integration tests: TV-Maxe on a private session bus, driven by a
//! second D-Bus connection, like the media keys or `playerctl` would.
//!
//! Needs `dbus-daemon` (and a display, like the other GTK tests). Without
//! `dbus-daemon` the tests are skipped, unless `TVMAXE_REQUIRE_DISPLAY` is set
//! (CI).
//!
//! ```sh
//! xvfb-run -a cargo test -p tvmaxe --test mpris
//! ```

#[macro_use]
mod common;

use common::{TEST_VIDEO, TempDir, TestResult, wait_until};
use gtk::gio;
use gtk::glib::{self, Variant, prelude::ToVariant};
use gtk::prelude::*;
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;
use tvmaxe::MainWindow;
use tvmaxe_core::Kind;

const TIMEOUT: Duration = Duration::from_secs(20);
const NAME: &str = "org.mpris.MediaPlayer2.tvmaxe";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// A session bus of its own, so the tests never touch the user's.
struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_bus() -> Option<Bus> {
    let spawned = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(_) if std::env::var_os("TVMAXE_REQUIRE_DISPLAY").is_some() => {
            panic!("dbus-daemon is required in CI")
        }
        Err(_) => return None,
    };
    let mut address = String::new();
    BufReader::new(child.stdout.take()?)
        .read_line(&mut address)
        .ok()?;
    Some(Bus {
        child,
        address: address.trim().to_string(),
    })
}

thread_local! {
    static FIXTURE: RefCell<Option<Rc<Fixture>>> = const { RefCell::new(None) };
}

struct Fixture {
    _dir: TempDir,
    _app: gtk::Application,
    _bus: Bus,
    main: Rc<MainWindow>,
    /// The client: another connection to the same bus.
    client: gio::DBusConnection,
}

fn main() {
    let Some(bus) = start_bus() else {
        println!("mpris: dbus-daemon not found, tests skipped");
        return;
    };
    // SAFETY: nothing else runs yet (GTK starts in `common::run`), so no other
    // thread reads the environment.
    unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address) };
    BUS.with_borrow_mut(|b| *b = Some(bus));
    common::run(
        "mpris",
        &[
            ("appears_on_the_bus", || Box::pin(appears_on_the_bus())),
            ("play_pause_stop_and_metadata", || {
                Box::pin(play_pause_stop_and_metadata())
            }),
            ("next_and_previous", || Box::pin(next_and_previous())),
            ("volume", || Box::pin(volume())),
            (
                "changes_are_announced",
                || Box::pin(changes_are_announced()),
            ),
        ],
    );
}

thread_local! {
    static BUS: RefCell<Option<Bus>> = const { RefCell::new(None) };
}

async fn fixture() -> Result<Rc<Fixture>, String> {
    if let Some(f) = FIXTURE.with_borrow(|f| f.clone()) {
        return Ok(f);
    }
    let bus = BUS.with_borrow_mut(|b| b.take()).ok_or("no bus")?;
    let address = bus.address.clone();
    // The session bus connection raises SIGTERM when the bus goes away
    // (GDBus' exit-on-close); the bus is stopped when the tests end.
    gio::bus_get_future(gio::BusType::Session)
        .await
        .map_err(|e| format!("no session bus: {e}"))?
        .set_exit_on_close(false);
    let dir = TempDir::new("mpris");
    let app = gtk::Application::builder()
        .application_id("io.condor2.TvMaxe.MprisTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(tvmaxe::setup_app);
    app.register(None::<&gio::Cancellable>)
        .map_err(|e| format!("the application does not register: {e}"))?;
    let main = MainWindow::new(&app, dir.0.join("tvmaxe.db")).map_err(|e| format!("{e:#}"))?;
    // mpv only starts once the video area exists, so the window is shown.
    main.window.present();
    for (i, name) in ["Radio Uno", "Radio Due", "Radio Tre"].iter().enumerate() {
        let url = TEST_VIDEO.replace("duration=600", &format!("duration={}", 600 + i));
        main.db
            .borrow()
            .add_local_channel(Kind::Tv, name, &url)
            .map_err(|e| e.to_string())?;
    }
    main.reload();
    main.start_mpris();
    let client = gio::DBusConnection::for_address_future(
        &address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
    )
    .await
    .map_err(|e| format!("the client cannot connect: {e}"))?;
    let f = Rc::new(Fixture {
        _dir: dir,
        _app: app,
        _bus: bus,
        main,
        client,
    });
    FIXTURE.with_borrow_mut(|slot| *slot = Some(f.clone()));
    Ok(f)
}

impl Fixture {
    async fn call(
        &self,
        iface: &str,
        method: &str,
        args: Option<Variant>,
    ) -> Result<Variant, String> {
        self.client
            .call_future(
                Some(NAME),
                PATH,
                iface,
                method,
                args.as_ref(),
                None,
                gio::DBusCallFlags::NONE,
                5000,
            )
            .await
            .map_err(|e| format!("{iface}.{method}: {e}"))
    }

    async fn get(&self, iface: &str, prop: &str) -> Result<Variant, String> {
        let r = self
            .call(
                "org.freedesktop.DBus.Properties",
                "Get",
                Some((iface, prop).to_variant()),
            )
            .await?;
        r.child_value(0)
            .get::<Variant>()
            .ok_or_else(|| format!("{prop}: not a variant"))
    }

    async fn status(&self) -> Result<String, String> {
        Ok(self
            .get(PLAYER, "PlaybackStatus")
            .await?
            .get::<String>()
            .unwrap_or_default())
    }

    /// `xesam:title` of the metadata ("" if there is none).
    async fn title(&self) -> Result<String, String> {
        let meta = self.get(PLAYER, "Metadata").await?;
        let dict: std::collections::HashMap<String, Variant> =
            meta.get().ok_or("Metadata is not a{sv}")?;
        Ok(dict
            .get("xesam:title")
            .and_then(|v| v.get::<String>())
            .unwrap_or_default())
    }

    /// Waits until `PlaybackStatus` is `want`.
    async fn wait_status(&self, want: &str) -> TestResult {
        let start = std::time::Instant::now();
        loop {
            if self.status().await? == want {
                return Ok(());
            }
            if start.elapsed() > TIMEOUT {
                return Err(format!("PlaybackStatus never became {want}"));
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
    }
}

async fn appears_on_the_bus() -> TestResult {
    let f = fixture().await?;
    // The name is taken asynchronously.
    let start = std::time::Instant::now();
    while f.get(ROOT, "Identity").await.is_err() {
        check!(start.elapsed() < TIMEOUT, "{NAME} never appeared");
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    check!(
        f.get(ROOT, "Identity").await?.get::<String>().as_deref() == Some("TV-Maxe"),
        "Identity"
    );
    check!(
        f.get(ROOT, "DesktopEntry")
            .await?
            .get::<String>()
            .as_deref()
            == Some("TvMaxe"),
        "DesktopEntry"
    );
    check!(
        f.get(ROOT, "CanQuit").await?.get::<bool>() == Some(true),
        "CanQuit"
    );
    check!(f.status().await? == "Stopped", "status at start");
    check!(
        f.get(PLAYER, "CanGoNext").await?.get::<bool>() == Some(true),
        "CanGoNext with three channels"
    );
    check!(
        f.get(PLAYER, "CanSeek").await?.get::<bool>() == Some(false),
        "CanSeek"
    );
    Ok(())
}

async fn play_pause_stop_and_metadata() -> TestResult {
    let f = fixture().await?;
    // Play with nothing chosen selects nothing and does not fail.
    f.main.play_channel_at(Kind::Tv, 0);
    f.wait_status("Playing").await?;
    let first = f.title().await?;
    check!(first.starts_with("Radio "), "title: {first:?}");

    f.call(PLAYER, "Pause", None).await?;
    f.wait_status("Paused").await?;
    f.call(PLAYER, "PlayPause", None).await?;
    f.wait_status("Playing").await?;
    f.call(PLAYER, "PlayPause", None).await?;
    f.wait_status("Paused").await?;
    f.call(PLAYER, "Play", None).await?;
    f.wait_status("Playing").await?;

    f.call(PLAYER, "Stop", None).await?;
    f.wait_status("Stopped").await?;
    // Play starts the channel again.
    f.call(PLAYER, "Play", None).await?;
    f.wait_status("Playing").await?;
    check!(f.title().await? == first, "the channel changed on Play");
    Ok(())
}

async fn next_and_previous() -> TestResult {
    let f = fixture().await?;
    f.main.play_channel_at(Kind::Tv, 0);
    f.wait_status("Playing").await?;
    let first = f.title().await?;
    f.call(PLAYER, "Next", None).await?;
    wait_until("another channel after Next", TIMEOUT, || {
        f.main.status_text().starts_with("Playing: ")
            && f.main.status_text() != format!("Playing: {first}")
    })
    .await?;
    f.wait_status("Playing").await?;
    let second = f.title().await?;
    check!(second != first, "Next stayed on {first:?}");
    f.call(PLAYER, "Previous", None).await?;
    wait_until("the first channel after Previous", TIMEOUT, || {
        f.main.status_text() == format!("Playing: {first}")
    })
    .await?;
    check!(f.title().await? == first, "Previous did not come back");
    Ok(())
}

async fn volume() -> TestResult {
    let f = fixture().await?;
    f.call(
        "org.freedesktop.DBus.Properties",
        "Set",
        Some((PLAYER, "Volume", 0.25f64.to_variant()).to_variant()),
    )
    .await?;
    let v = f.get(PLAYER, "Volume").await?.get::<f64>().unwrap_or(-1.0);
    check!((v - 0.25).abs() < 0.01, "Volume: {v}");
    f.call(
        "org.freedesktop.DBus.Properties",
        "Set",
        Some((PLAYER, "Volume", 7.0f64.to_variant()).to_variant()),
    )
    .await?;
    let v = f.get(PLAYER, "Volume").await?.get::<f64>().unwrap_or(-1.0);
    check!(
        (v - 1.0).abs() < 0.01,
        "Volume above 1.0 was not clamped: {v}"
    );
    Ok(())
}

async fn changes_are_announced() -> TestResult {
    let f = fixture().await?;
    f.main.play_channel_at(Kind::Tv, 0);
    f.wait_status("Playing").await?;
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let s = seen.clone();
    let _subscription = f.client.subscribe_to_signal(
        Some(NAME),
        Some("org.freedesktop.DBus.Properties"),
        Some("PropertiesChanged"),
        Some(PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let names: Vec<String> = signal
                .parameters
                .child_value(1)
                .iter()
                .filter_map(|e| e.child_value(0).get::<String>())
                .collect();
            s.borrow_mut().extend(names);
        },
    );
    // The bus installs the match rule when it reaches it; a call on the same
    // connection comes after it, so once it is answered no signal is missed.
    f.get(ROOT, "Identity").await?;
    // Pausing from the window (not through D-Bus) is announced too.
    f.main.player().toggle_pause();
    wait_until("PlaybackStatus announced", TIMEOUT, || {
        seen.borrow().iter().any(|n| n == "PlaybackStatus")
    })
    .await?;
    f.main.player().toggle_pause();
    seen.borrow_mut().clear();
    f.main.zap(1);
    wait_until("Metadata announced", TIMEOUT, || {
        seen.borrow().iter().any(|n| n == "Metadata")
    })
    .await?;
    Ok(())
}
