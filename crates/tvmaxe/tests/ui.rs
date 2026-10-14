//! Teste de integrare pentru interfață: fereastra principală, ferestrele
//! secundare (deschise și închise de mai multe ori, ca să nu revină bug-ul
//! „A window is shown after it has been destroyed”), abonament dintr-un M3U
//! local și redarea unui canal cu URL de rezervă.
//!
//! Orice avertisment GTK apărut în timpul unui test îl face să eșueze.
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
            ("fereastra_principala_porneste", || {
                Box::pin(main_window_starts())
            }),
            ("abonament_m3u_local_si_redare", || {
                Box::pin(local_m3u_and_play())
            }),
            // După testul de mai sus: „Ghid TV” are nevoie de un canal ales.
            ("ferestrele_secundare_se_redeschid", || {
                Box::pin(dialogs_reopen())
            }),
            ("lista_ascunsa_si_reafisata", || {
                Box::pin(toggle_channel_list())
            }),
            ("alegerea_sursei", || Box::pin(choose_source())),
            ("editarea_unui_canal_local", || {
                Box::pin(edit_local_channel())
            }),
            // Ultimul: închide fereastra principală.
            ("inchidere_in_timpul_redarii", || {
                Box::pin(close_while_playing())
            }),
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

/// Aplicația și fereastra principală, create o singură dată, cu baza de date
/// într-un director temporar (nu atinge datele utilizatorului).
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
        .map_err(|e| format!("aplicația nu se înregistrează: {e}"))?;
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

/// Ferestrele vizibile, în afară de cea principală.
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
    wait_until("fereastra principală afișată", TIMEOUT, || {
        win.is_mapped()
    })
    .await?;
    check!(
        win.title().as_deref() == Some("TV-Maxe"),
        "titlu: {:?}",
        win.title()
    );
    check!(
        f.main.channel_count(Kind::Tv) == 0,
        "baza nouă are canale TV"
    );
    check!(
        win.application().is_some_and(|a| a.menubar().is_some()),
        "meniul aplicației lipsește"
    );
    // Fără canal ales, „Ghid TV” explică în bara de stare în loc să deschidă ceva.
    WidgetExt::activate_action(&win, "win.guide", None).map_err(|e| e.to_string())?;
    glib::timeout_future(Duration::from_millis(200)).await;
    check!(
        secondary_windows(win.upcast_ref()).is_empty(),
        "Ghid TV s-a deschis fără canal"
    );
    check!(
        f.main.status_text().starts_with("Choose a channel"),
        "starea: {:?}",
        f.main.status_text()
    );
    Ok(())
}

/// Fiecare fereastră secundară: deschisă și închisă de 3 ori. A doua deschidere
/// trebuie să meargă (fereastră ascunsă refolosită sau una nouă), fără
/// avertismente GTK.
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
            let what = format!("{action}: fereastra apare (runda {round})");
            wait_until(&what, TIMEOUT, || {
                secondary_windows(&main).iter().any(|w| w.is_mapped())
            })
            .await?;
            let opened = secondary_windows(&main);
            check!(
                opened.len() == 1,
                "{action}: {} ferestre deschise în loc de 1",
                opened.len()
            );
            opened[0].close();
            let what = format!("{action}: fereastra se închide (runda {round})");
            wait_until(&what, TIMEOUT, || secondary_windows(&main).is_empty()).await?;
        }
    }
    Ok(())
}

/// Un M3U de pe disc ca abonament: canalul TV are întâi un URL mort, apoi unul
/// bun; grupul „Radio” trece canalul la tabul radio.
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
    wait_until("canalele din M3U în liste", TIMEOUT, || {
        f.main.channel_count(Kind::Tv) == 1 && f.main.channel_count(Kind::Radio) == 1
    })
    .await?;

    check!(f.main.play_channel_at(Kind::Tv, 0), "nu pot porni canalul");
    wait_until("status “Playing: Canal test”", TIMEOUT, || {
        f.main.status_text() == "Playing: Canal test"
    })
    .await
    .map_err(|e| format!("{e}; starea: {:?}", f.main.status_text()))?;

    f.main.player().stop();
    check!(
        f.main.status_text() == "Stopped",
        "starea după Stop: {:?}",
        f.main.status_text()
    );
    Ok(())
}

/// F9: lista de canale se ascunde și reapare.
async fn toggle_channel_list() -> TestResult {
    let f = fixture()?;
    let state = || {
        f.main
            .window
            .lookup_action("show-list")
            .and_then(|a| a.state())
            .and_then(|s| s.get::<bool>())
    };
    check!(state() == Some(true), "lista nu e vizibilă la pornire");
    for expected in [false, true] {
        WidgetExt::activate_action(&f.main.window, "win.show-list", None)
            .map_err(|e| e.to_string())?;
        glib::timeout_future(Duration::from_millis(100)).await;
        check!(state() == Some(expected), "show-list = {:?}", state());
    }
    Ok(())
}

/// Închiderea ferestrei în timpul redării distruge zona video cât timp mpv încă
/// desenează (aici era un use-after-free în callback-ul de redesenare din libmpv2).
async fn close_while_playing() -> TestResult {
    let f = fixture()?;
    check!(f.main.play_channel_at(Kind::Tv, 0), "nu pot porni canalul");
    wait_until("redare înainte de închidere", TIMEOUT, || {
        f.main.status_text() == "Playing: Canal test"
    })
    .await?;
    glib::timeout_future(Duration::from_millis(500)).await;
    f.main.window.close();
    let win = f.main.window.clone();
    wait_until("fereastra principală închisă", TIMEOUT, || {
        !win.is_visible()
    })
    .await?;
    // mpv mai rulează câteva cadre pe firul lui; un crash ar opri procesul aici.
    glib::timeout_future(Duration::from_millis(1500)).await;
    Ok(())
}

/// Click dreapta → Sursă: pornește direct sursa aleasă; dacă e moartă, trece
/// la următoarea (aici: sursa 1 moartă → 2).
async fn choose_source() -> TestResult {
    let f = fixture()?;
    let id = f.main.channel_id_at(Kind::Tv, 0).ok_or("niciun canal TV")?;
    for (chosen, expected) in [(1u32, 1usize), (0, 1)] {
        WidgetExt::activate_action(
            &f.main.window,
            "win.play-source",
            Some(&(id, chosen).to_variant()),
        )
        .map_err(|e| e.to_string())?;
        wait_until("redare de la sursa aleasă", TIMEOUT, || {
            f.main.status_text() == "Playing: Canal test"
                && f.main.player().source_index() == Some(expected)
        })
        .await
        .map_err(|e| {
            format!(
                "{e}; sursa {chosen}: stare {:?}, index {:?}",
                f.main.status_text(),
                f.main.player().source_index()
            )
        })?;
    }
    f.main.player().stop();
    Ok(())
}

/// Toți descendenții unui widget de tipul `T`.
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

/// „Editează canalul…” pe un canal din „Canalele mele”: formularul vine
/// completat, iar „Salvează” schimbă canalul (nu adaugă altul). Canalele din
/// abonamente nu se pot edita.
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
    wait_until("fereastra de editare", TIMEOUT, || {
        secondary_windows(&main).iter().any(|w| w.is_mapped())
    })
    .await?;
    let dialog = secondary_windows(&main).remove(0);
    check!(
        dialog.title().as_deref() == Some("Edit channel"),
        "titlu: {:?}",
        dialog.title()
    );
    let root: gtk::Widget = dialog.clone().upcast();
    let entry = descendants::<gtk::Entry>(&root)
        .into_iter()
        .next()
        .ok_or("lipsește câmpul Nume")?;
    check!(entry.text() == "Local vechi", "nume: {:?}", entry.text());
    let view = descendants::<gtk::TextView>(&root)
        .into_iter()
        .next()
        .ok_or("lipsește câmpul URL-uri")?;
    let b = view.buffer();
    let text = b.text(&b.start_iter(), &b.end_iter(), false);
    check!(text == "http://a/1\nhttp://a/2", "URL-uri: {text:?}");

    entry.set_text("Local nou");
    b.set_text("http://a/2\nhttp://a/3");
    let save = descendants::<gtk::Button>(&root)
        .into_iter()
        .find(|b| b.label().as_deref() == Some("Save"))
        .ok_or("lipsește butonul Save")?;
    save.emit_clicked();
    wait_until("fereastra se închide", TIMEOUT, || {
        secondary_windows(&main).is_empty()
    })
    .await?;

    check!(
        f.main.channel_count(Kind::Tv) == tv_before,
        "s-a adăugat un canal în plus"
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
        .ok_or("canalul a dispărut")?;
    check!(ch.name == "Local nou", "nume salvat: {:?}", ch.name);
    check!(
        ch.urls == ["http://a/2", "http://a/3"],
        "URL-uri salvate: {:?}",
        ch.urls
    );

    // Un canal dintr-un abonament nu deschide formularul.
    let sub_id = channels
        .iter()
        .find(|c| !c.is_local)
        .ok_or("niciun canal din abonament")?
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
        "s-a deschis editarea pentru un canal din abonament"
    );

    // Curățenie: testul următor închide fereastra în timpul redării canalului 0.
    let _ = f.main.db.borrow().delete_local_channel(id);
    f.main.reload();
    Ok(())
}
