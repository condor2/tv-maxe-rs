//! Teste de integrare pentru backend-ul libmpv, prin fațada `Player`:
//! pornire sub locale non-C, redare și randare reală în GtkGLArea, trecerea la
//! URL-ul de rezervă, antetele HTTP per canal, pauză și oprire.
//!
//! Au nevoie de ecran (X11 sau Wayland, merge și Xvfb / weston headless cu
//! OpenGL software). Sursele video sunt generate de mpv (lavfi), iar serverul
//! HTTP e local, deci nu e nevoie de internet.
//!
//! ```sh
//! xvfb-run -a cargo test -p tvmaxe --test player
//! cargo test -p tvmaxe --test player -- antete   # doar testele care conțin „antete”
//! ```

#[macro_use]
mod common;

use common::{DEAD_URL, TEST_VIDEO, TestResult, sleep, wait_until};
use gtk::prelude::*;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use tvmaxe::player::{PlaybackSettings, Player, PlayerEvent, StreamOptions};

const START_TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    common::run(
        "player (libmpv)",
        &[
            ("porneste_sub_locale_non_c", || {
                Box::pin(starts_under_non_c_locale())
            }),
            ("reda_si_deseneaza_video", || {
                Box::pin(plays_and_renders_video())
            }),
            ("trece_la_url_de_rezerva", || {
                Box::pin(falls_back_to_next_url())
            }),
            ("raporteaza_esec_cand_toate_cad", || {
                Box::pin(fails_when_all_urls_fail())
            }),
            ("antete_http_per_canal", || {
                Box::pin(sends_channel_http_headers())
            }),
            ("pauza_si_stop", || Box::pin(pause_and_stop())),
        ],
    );
}

/// O fereastră cu zona video și evenimentele player-ului înregistrate.
struct Rig {
    window: gtk::Window,
    player: Rc<Player>,
    events: Rc<RefCell<Vec<PlayerEvent>>>,
}

impl Rig {
    async fn new() -> Result<Self, String> {
        let player = Player::new().map_err(|e| format!("Player::new: {e:#}"))?;
        let events: Rc<RefCell<Vec<PlayerEvent>>> = Rc::default();
        let e = events.clone();
        player.connect_event(move |ev| e.borrow_mut().push(ev));
        let window = gtk::Window::builder()
            .title("tvmaxe test")
            .default_width(320)
            .default_height(240)
            .child(&player.widget())
            .build();
        window.present();
        let w = player.widget();
        wait_until("zona video afișată", START_TIMEOUT, || w.is_mapped()).await?;
        Ok(Self {
            window,
            player,
            events,
        })
    }

    fn has(&self, f: impl Fn(&PlayerEvent) -> bool) -> bool {
        self.events.borrow().iter().any(f)
    }

    async fn wait_playing(&self) -> TestResult {
        wait_until("PlayerEvent::Playing", START_TIMEOUT, || {
            self.has(|e| *e == PlayerEvent::Playing)
                || self.has(|e| matches!(e, PlayerEvent::Failed(_)))
        })
        .await?;
        if let Some(PlayerEvent::Failed(m)) = self
            .events
            .borrow()
            .iter()
            .find(|e| matches!(e, PlayerEvent::Failed(_)))
        {
            return Err(format!("redarea a eșuat: {m}"));
        }
        Ok(())
    }

    async fn wait_failed(&self) -> TestResult {
        wait_until("PlayerEvent::Failed", START_TIMEOUT, || {
            self.has(|e| matches!(e, PlayerEvent::Failed(_)))
        })
        .await
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.player.stop();
        self.window.destroy();
    }
}

fn urls(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Bug real: GTK aplică LC_NUMERIC al sistemului (ro_RO → „1,5”), iar libmpv
/// refuză să pornească („Non-C locale detected”).
async fn starts_under_non_c_locale() -> TestResult {
    let candidates = [
        c"ro_RO.UTF-8",
        c"ro_RO.utf8",
        c"de_DE.UTF-8",
        c"fr_FR.UTF-8",
        c"en_US.UTF-8",
    ];
    // SAFETY: firul principal; niciun alt fir nu citește locale-ul acum.
    let chosen = candidates
        .iter()
        .find(|l| unsafe { !libc::setlocale(libc::LC_NUMERIC, l.as_ptr()).is_null() });
    let Some(chosen) = chosen else {
        println!("    (niciun locale non-C instalat; verific doar pornirea)");
        return Player::new().map(|_| ()).map_err(|e| format!("{e:#}"));
    };
    let player = Player::new();
    // SAFETY: ca mai sus; pointerul întors e citit imediat.
    let now = unsafe {
        std::ffi::CStr::from_ptr(libc::setlocale(libc::LC_NUMERIC, std::ptr::null()))
            .to_string_lossy()
            .into_owned()
    };
    check!(
        player.is_ok(),
        "libmpv nu pornește sub LC_NUMERIC={chosen:?}: {:#}",
        player.err().unwrap()
    );
    check!(now == "C", "LC_NUMERIC a rămas {now:?}, trebuia „C”");
    Ok(())
}

/// Redare reală: evenimentul Playing, timpul avansează, iar în GtkGLArea apar
/// barele de culoare (prinde regresii de tipul „imagine neagră”).
async fn plays_and_renders_video() -> TestResult {
    let rig = Rig::new().await?;
    rig.player
        .play(urls(&[TEST_VIDEO]), StreamOptions::default());
    rig.wait_playing().await?;
    check!(rig.player.is_active(), "player-ul nu e activ după Playing");
    wait_until("timpul de redare avansează", START_TIMEOUT, || {
        rig.player.playback_time().is_some_and(|t| t > 0.5)
    })
    .await?;

    let area = rig.player.widget();
    let mut colors = 0;
    for _ in 0..40 {
        colors = distinct_colors(&area)?;
        if colors >= 5 {
            return Ok(());
        }
        sleep(250).await;
    }
    Err(format!(
        "zona video arată {colors} culori distincte, așteptam barele SMPTE (≥ 5)"
    ))
}

/// Captura zonei video (exact ce desenează GTK) și numărul de culori distincte
/// ne-negre, rotunjite grosier ca să nu conteze scalarea.
fn distinct_colors(widget: &gtk::Widget) -> Result<usize, String> {
    let (w, h) = (widget.width(), widget.height());
    if w == 0 || h == 0 {
        return Ok(0);
    }
    let parent = widget.parent().ok_or("zona video nu are părinte")?;
    let snapshot = gtk::Snapshot::new();
    parent.snapshot_child(widget, &snapshot);
    let Some(node) = snapshot.to_node() else {
        return Ok(0);
    };
    let renderer = widget
        .native()
        .and_then(|n| n.renderer())
        .ok_or("fereastra nu are renderer")?;
    let texture = renderer.render_texture(&node, None);
    // Depanare (și artefact în CI): TVMAXE_TEST_CAPTURE=/tmp/captura.png
    if let Some(path) = std::env::var_os("TVMAXE_TEST_CAPTURE") {
        let _ = texture.save_to_png(path);
    }
    let (tw, th) = (texture.width() as usize, texture.height() as usize);
    let mut buf = vec![0u8; tw * th * 4];
    texture.download(&mut buf, tw * 4);
    let mut seen = std::collections::HashSet::new();
    for px in buf.chunks_exact(4) {
        let (b, g, r) = (px[0], px[1], px[2]); // GDK_MEMORY_DEFAULT = B8G8R8A8 premultiplicat
        if u16::from(r) + u16::from(g) + u16::from(b) > 60 {
            seen.insert((r >> 6, g >> 6, b >> 6));
        }
    }
    Ok(seen.len())
}

async fn falls_back_to_next_url() -> TestResult {
    let rig = Rig::new().await?;
    rig.player
        .play(urls(&[DEAD_URL, TEST_VIDEO]), StreamOptions::default());
    rig.wait_playing().await?;
    check!(
        !rig.has(|e| matches!(e, PlayerEvent::Failed(_))),
        "URL-ul mort a ajuns la utilizator ca eroare: {:?}",
        rig.events.borrow()
    );
    Ok(())
}

async fn fails_when_all_urls_fail() -> TestResult {
    let rig = Rig::new().await?;
    rig.player.play(
        urls(&[DEAD_URL, "http://127.0.0.1:9/alt-mort.ts"]),
        StreamOptions::default(),
    );
    rig.wait_failed().await?;
    check!(!rig.player.is_active(), "player-ul a rămas activ după eșec");
    Ok(())
}

/// Server HTTP minimal: trimite antetele fiecărei cereri și răspunde 404.
fn header_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/live.m3u8", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&req).into_owned());
            let _ = s.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (url, rx)
}

async fn next_request(rx: &mpsc::Receiver<String>) -> Result<String, String> {
    let got: RefCell<Option<String>> = RefCell::default();
    wait_until("cererea HTTP a mpv", START_TIMEOUT, || {
        if got.borrow().is_none() {
            *got.borrow_mut() = rx.try_recv().ok();
        }
        got.borrow().is_some()
    })
    .await?;
    Ok(got.into_inner().unwrap())
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// `http-referrer` / `http-user-agent` din M3U ajung la server; un canal fără
/// ele nu trimite Referer deloc (nici gol) și folosește User-Agent-ul din Preferințe.
async fn sends_channel_http_headers() -> TestResult {
    let (url, rx) = header_server();
    let rig = Rig::new().await?;

    rig.player.play(
        urls(&[&url]),
        StreamOptions {
            referrer: Some("https://referer.example/".into()),
            user_agent: Some("UA-Canal/1.0".into()),
        },
    );
    let req = next_request(&rx).await?;
    check!(
        header(&req, "Referer") == Some("https://referer.example/"),
        "Referer greșit în:\n{req}"
    );
    check!(
        header(&req, "User-Agent") == Some("UA-Canal/1.0"),
        "User-Agent greșit în:\n{req}"
    );
    rig.wait_failed().await?;
    while rx.try_recv().is_ok() {}

    rig.player.apply_settings(&PlaybackSettings {
        user_agent: Some("UA-Preferinte/2.0".into()),
        ..Default::default()
    });
    rig.events.borrow_mut().clear();
    rig.player.play(urls(&[&url]), StreamOptions::default());
    let req = next_request(&rx).await?;
    check!(
        header(&req, "Referer").is_none(),
        "Referer trimis pentru un canal care nu-l cere:\n{req}"
    );
    check!(
        header(&req, "User-Agent") == Some("UA-Preferinte/2.0"),
        "User-Agent-ul din Preferințe nu e folosit:\n{req}"
    );
    rig.wait_failed().await
}

async fn pause_and_stop() -> TestResult {
    let rig = Rig::new().await?;
    rig.player
        .play(urls(&[TEST_VIDEO]), StreamOptions::default());
    rig.wait_playing().await?;

    rig.player.toggle_pause();
    check!(rig.player.is_paused(), "nu e pe pauză");
    sleep(300).await;
    let t1 = rig.player.playback_time().unwrap_or(0.0);
    sleep(700).await;
    let t2 = rig.player.playback_time().unwrap_or(0.0);
    check!(
        (t2 - t1).abs() < 0.2,
        "timpul avansează pe pauză: {t1} → {t2}"
    );

    rig.player.toggle_pause();
    check!(!rig.player.is_paused(), "pauza nu s-a anulat");
    rig.player.stop();
    check!(!rig.player.is_active(), "activ după Stop");
    check!(
        rig.events.borrow().last() == Some(&PlayerEvent::Stopped),
        "ultimul eveniment nu e Stopped: {:?}",
        rig.events.borrow()
    );
    check!(
        rig.player.playback_time().is_none(),
        "timp de redare după Stop"
    );
    Ok(())
}
