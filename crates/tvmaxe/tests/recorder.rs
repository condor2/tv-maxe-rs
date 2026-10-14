//! Tests for the recording engine (headless libmpv + `stream-record`).
//!
//! They need no display, but they need `ffmpeg` to generate a short H.264/AAC
//! MPEG-TS source (the lavfi test sources produce raw video, which cannot be
//! written to a `.ts` file). Without ffmpeg the tests are skipped, except in
//! CI (`CI` is set), where a missing ffmpeg is an error.

#![cfg(feature = "mpv")]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tvmaxe::player::{Recorder, RecorderState, StreamOptions};

const DEAD_URL: &str = "http://127.0.0.1:9/dead.ts";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("tvmaxe-rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(urls: &[&str], dir: &Path) -> Recorder {
    let base = dir.to_path_buf();
    Recorder::start(
        urls.iter().map(|s| s.to_string()).collect(),
        StreamOptions::default(),
        "TV-Maxe/test",
        5,
        move |part| base.join(format!("out-{part}.ts")),
    )
    .expect("recorder starts")
}

/// Polls until `cond` holds for the state, or panics after `secs` seconds.
fn wait_for(rec: &Recorder, secs: u64, what: &str, cond: impl Fn(&RecorderState) -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        let state = rec.poll();
        if cond(&state) {
            return;
        }
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}; state {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn is_mpeg_ts(path: &Path) -> bool {
    let data = std::fs::read(path).unwrap();
    data.len() > 10_000 && data[0] == 0x47 && data.len().is_multiple_of(188)
}

#[test]
fn records_to_disk() {
    let dir = TempDir::new("basic");
    let Some(src) = common::ts_source(&dir.0, 20) else {
        return;
    };
    let rec = start(&[src.to_str().unwrap()], &dir.0);
    wait_for(&rec, 15, "Recording", |s| *s == RecorderState::Recording);
    std::thread::sleep(Duration::from_secs(2));
    rec.poll();
    let files = rec.stop();
    assert_eq!(files, [dir.0.join("out-1.ts")]);
    assert!(is_mpeg_ts(&files[0]), "not a usable MPEG-TS file");
}

#[test]
fn falls_back_to_the_next_url() {
    let dir = TempDir::new("fallback");
    let Some(src) = common::ts_source(&dir.0, 20) else {
        return;
    };
    let rec = start(&[DEAD_URL, src.to_str().unwrap()], &dir.0);
    wait_for(&rec, 20, "Recording", |s| *s == RecorderState::Recording);
    std::thread::sleep(Duration::from_secs(1));
    let files = rec.stop();
    assert_eq!(
        files.len(),
        1,
        "the dead URL leaves no extra file: {files:?}"
    );
    assert!(is_mpeg_ts(&files[0]));
}

#[test]
fn fails_when_every_url_fails() {
    let dir = TempDir::new("fail");
    let rec = start(&[DEAD_URL, "http://127.0.0.1:9/other.ts"], &dir.0);
    wait_for(&rec, 20, "Failed", |s| {
        matches!(s, RecorderState::Failed(_))
    });
    assert!(rec.stop().is_empty());
    let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
    assert!(left.is_empty(), "empty files are removed: {left:?}");
}

/// A stream that ends in the middle of a recording is reopened, into a new part.
#[test]
fn reconnects_into_a_new_part() {
    let dir = TempDir::new("reconnect");
    let Some(src) = common::ts_source(&dir.0, 2) else {
        return;
    };
    let rec = start(&[src.to_str().unwrap()], &dir.0);
    wait_for(&rec, 15, "Recording", |s| *s == RecorderState::Recording);
    let until = Instant::now() + Duration::from_secs(30);
    while rec.files().len() < 2 {
        assert!(
            Instant::now() < until,
            "no second part; files {:?}",
            rec.files()
        );
        assert!(!matches!(rec.poll(), RecorderState::Failed(_)), "gave up");
        std::thread::sleep(Duration::from_millis(100));
    }
    let files = rec.stop();
    assert_eq!(files, [dir.0.join("out-1.ts"), dir.0.join("out-2.ts")]);
    assert!(files.iter().all(|f| is_mpeg_ts(f)));
}
