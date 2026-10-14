//! Shared infrastructure for the GTK integration tests.
//!
//! GTK only works on the main thread and can be initialised only once per
//! process, so every test file has `harness = false` and its own `main`,
//! which runs the tests one after another in the GLib main loop.
//!
//! Without a display (neither `DISPLAY` nor `WAYLAND_DISPLAY`), the tests are
//! skipped, so `cargo test` works anywhere. In CI, `TVMAXE_REQUIRE_DISPLAY=1`
//! turns a missing display into an error.

#![allow(dead_code)] // each test file uses only part of this

use gtk::glib;
use gtk::prelude::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type TestResult = Result<(), String>;
pub type TestFn = fn() -> Pin<Box<dyn Future<Output = TestResult>>>;

/// GLib/GTK warnings and errors raised during a test.
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Warnings caused by the test environment, not by the application.
const IGNORED_WARNINGS: &[&str] = &[
    // No D-Bus session (Xvfb, headless Weston): accessibility and portals.
    "AT-SPI",
    "org.a11y",
    "org.freedesktop.portal",
    "Unable to acquire session bus",
    "Failed to open display", // tratat separat
];

/// Runs the tests and exits with the right code. `suite` appears in the messages.
pub fn run(suite: &str, tests: &[(&str, TestFn)]) {
    capture_warnings();
    // The tests compare interface texts: always English, whatever the system
    // language (the translations have their own test, `translations.rs`).
    assert!(tvmaxe_core::i18n::force_language(
        tvmaxe_core::i18n::Language::English
    ));
    if !has_display() {
        if std::env::var_os("TVMAXE_REQUIRE_DISPLAY").is_some() {
            eprintln!("{suite}: TVMAXE_REQUIRE_DISPLAY is set, but there is no display");
            std::process::exit(1);
        }
        println!("{suite}: no display (DISPLAY / WAYLAND_DISPLAY), tests skipped");
        return;
    }
    if let Err(e) = gtk::init() {
        eprintln!("{suite}: GTK does not start: {e}");
        std::process::exit(1);
    }
    let backend = gtk::gdk::Display::default()
        .map(|d| d.type_().name().to_string())
        .unwrap_or_default();
    println!("\n{suite}: {} tests ({backend})", tests.len());

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
            result = Err(format!("GTK warnings:\n    {}", warnings.join("\n    ")));
        }
        match result {
            Ok(()) => println!(
                "test {name} ... ok ({:.1}s)",
                started.elapsed().as_secs_f64()
            ),
            Err(e) => {
                println!("test {name} ... FAILED\n    {e}");
                failed.push(*name);
            }
        }
    }
    println!(
        "\n{suite}: {} passed, {} failed",
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

/// Records warnings and errors (e.g. "A window is shown after it has been
/// destroyed"): a test that produces them is considered failed.
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

/// Waits (without blocking the GTK loop) until `cond` becomes true.
pub async fn wait_until(what: &str, timeout: Duration, cond: impl Fn() -> bool) -> TestResult {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > timeout {
            return Err(format!("timed out ({}s): {what}", timeout.as_secs()));
        }
        glib::timeout_future(Duration::from_millis(50)).await;
    }
    Ok(())
}

pub async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// An `assert!` that returns an error instead of aborting the whole process.
#[macro_export]
macro_rules! check {
    ($cond:expr, $($msg:tt)+) => {
        let ok: bool = $cond;
        if !ok {
            return Err(format!($($msg)+));
        }
    };
}

/// mpv test video source (FFmpeg's lavfi filter): colour bars,
/// no files and no network.
pub const TEST_VIDEO: &str = "av://lavfi:smptebars=size=320x240:rate=25:duration=600";
/// 440 Hz tone, for radio.
pub const TEST_AUDIO: &str = "av://lavfi:sine=frequency=440:duration=600";
/// A port nothing listens on: connection refused immediately.
pub const DEAD_URL: &str = "http://127.0.0.1:9/mort.m3u8";

/// Unique temporary directory, deleted at the end.
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
