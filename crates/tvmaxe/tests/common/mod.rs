//! Infrastructură comună pentru testele de integrare cu GTK.
//!
//! GTK merge doar pe firul principal și poate fi inițializat o singură dată
//! pe proces, așa că fiecare fișier de test are `harness = false` și propriul
//! `main`, care rulează testele unul după altul în bucla principală GLib.
//!
//! Fără ecran (nici `DISPLAY`, nici `WAYLAND_DISPLAY`), testele sunt sărite,
//! ca `cargo test` să meargă oriunde. În CI, `TVMAXE_REQUIRE_DISPLAY=1`
//! transformă lipsa ecranului în eroare.

#![allow(dead_code)] // fiecare fișier de test folosește doar o parte

use gtk::glib;
use gtk::prelude::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type TestResult = Result<(), String>;
pub type TestFn = fn() -> Pin<Box<dyn Future<Output = TestResult>>>;

/// Avertismentele și erorile GLib/GTK din timpul unui test.
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Avertismente care țin de mediul de test, nu de aplicație.
const IGNORED_WARNINGS: &[&str] = &[
    // Fără sesiune D-Bus (Xvfb, weston headless): accesibilitate și portaluri.
    "AT-SPI",
    "org.a11y",
    "org.freedesktop.portal",
    "Unable to acquire session bus",
    "Failed to open display", // tratat separat
];

/// Rulează testele și iese cu codul potrivit. `name` apare în mesaje.
pub fn run(suite: &str, tests: &[(&str, TestFn)]) {
    capture_warnings();
    // Testele compară texte din interfață: mereu în engleză, oricare ar fi
    // limba sistemului (traducerile au testul lor, `translations.rs`).
    assert!(tvmaxe_core::i18n::force_language(
        tvmaxe_core::i18n::Language::English
    ));
    if !has_display() {
        if std::env::var_os("TVMAXE_REQUIRE_DISPLAY").is_some() {
            eprintln!("{suite}: TVMAXE_REQUIRE_DISPLAY e setat, dar nu există ecran");
            std::process::exit(1);
        }
        println!("{suite}: fără ecran (DISPLAY / WAYLAND_DISPLAY), teste sărite");
        return;
    }
    if let Err(e) = gtk::init() {
        eprintln!("{suite}: GTK nu pornește: {e}");
        std::process::exit(1);
    }
    let backend = gtk::gdk::Display::default()
        .map(|d| d.type_().name().to_string())
        .unwrap_or_default();
    println!("\n{suite}: {} teste ({backend})", tests.len());

    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let ctx = glib::MainContext::default();
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        WARNINGS.lock().unwrap().clear();
        let started = Instant::now();
        let mut result = ctx.block_on(test());
        let warnings = std::mem::take(&mut *WARNINGS.lock().unwrap());
        if result.is_ok() && !warnings.is_empty() {
            result = Err(format!(
                "avertismente GTK:\n    {}",
                warnings.join("\n    ")
            ));
        }
        match result {
            Ok(()) => println!(
                "test {name} ... ok ({:.1}s)",
                started.elapsed().as_secs_f64()
            ),
            Err(e) => {
                println!("test {name} ... EȘUAT\n    {e}");
                failed.push(*name);
            }
        }
    }
    println!(
        "\n{suite}: {} trecute, {} eșuate",
        ran - failed.len(),
        failed.len()
    );
    if !failed.is_empty() {
        std::process::exit(1);
    }
}

fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

/// Reține avertismentele și erorile (ex. „A window is shown after it has been
/// destroyed”): un test care le produce e considerat eșuat.
fn capture_warnings() {
    glib::log_set_writer_func(|level, fields| {
        if matches!(
            level,
            glib::LogLevel::Warning | glib::LogLevel::Critical | glib::LogLevel::Error
        ) {
            let field = |k: &str| {
                fields
                    .iter()
                    .find(|f| f.key() == k)
                    .and_then(|f| f.value_str())
                    .unwrap_or("")
                    .to_string()
            };
            let msg = format!("{}: {}", field("GLIB_DOMAIN"), field("MESSAGE"));
            if !IGNORED_WARNINGS.iter().any(|i| msg.contains(i)) {
                WARNINGS.lock().unwrap().push(msg);
            }
        }
        glib::log_writer_default(level, fields)
    });
}

/// Așteaptă (fără să blocheze bucla GTK) până când `cond` devine adevărată.
pub async fn wait_until(what: &str, timeout: Duration, cond: impl Fn() -> bool) -> TestResult {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > timeout {
            return Err(format!("timp expirat ({}s): {what}", timeout.as_secs()));
        }
        glib::timeout_future(Duration::from_millis(50)).await;
    }
    Ok(())
}

pub async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// `assert!` care întoarce eroare în loc să oprească tot procesul.
#[macro_export]
macro_rules! check {
    ($cond:expr, $($msg:tt)+) => {
        let ok: bool = $cond;
        if !ok {
            return Err(format!($($msg)+));
        }
    };
}

/// Sursă video de test a mpv (filtrul lavfi din FFmpeg): bare de culoare,
/// fără fișiere și fără rețea.
pub const TEST_VIDEO: &str = "av://lavfi:smptebars=size=320x240:rate=25:duration=600";
/// Ton de 440 Hz, pentru radio.
pub const TEST_AUDIO: &str = "av://lavfi:sine=frequency=440:duration=600";
/// Port pe care nu ascultă nimic: conexiune refuzată imediat.
pub const DEAD_URL: &str = "http://127.0.0.1:9/mort.m3u8";

/// Director temporar unic, șters la final.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("tvmaxe-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
