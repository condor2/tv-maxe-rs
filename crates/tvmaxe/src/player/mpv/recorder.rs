//! Recording a channel with a separate, headless libmpv instance.
//!
//! The stream is written to disk as it arrives (`stream-record`, no
//! re-encoding), independently of what the main player shows. Like the
//! player, the recorder walks through the channel's URLs when one fails; if a
//! stream drops in the middle, it reconnects and continues in a new file part.

use super::{apply_http, describe_error};
use libmpv2::Mpv;
use libmpv2::events::Event;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tvmaxe_core::StreamOptions;
use tvmaxe_core::i18n::{fill, tr};

/// Wait before trying the URLs again after a stream dropped.
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// What a recorder is doing, as reported by [`Recorder::poll`].
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderState {
    /// Opening a URL (or waiting to reconnect).
    Connecting,
    /// Data is being written.
    Recording,
    /// No URL could be opened and nothing was recorded; message for the user.
    Failed(String),
}

pub struct Recorder {
    mpv: Mpv,
    urls: Vec<String>,
    http: StreamOptions,
    user_agent: String,
    /// File path for a part (1, 2, …).
    path_for: Box<dyn Fn(u32) -> PathBuf>,
    index: Cell<usize>,
    /// URLs tried since the last time data was flowing.
    tried: Cell<usize>,
    part: Cell<u32>,
    files: RefCell<Vec<PathBuf>>,
    state: RefCell<RecorderState>,
    /// Data arrived on the current part.
    receiving: Cell<bool>,
    /// Something was recorded at all (a later failure is then not fatal).
    recorded_any: Cell<bool>,
    retry_at: Cell<Option<Instant>>,
    last_error: RefCell<String>,
}

impl Recorder {
    /// Starts recording. `path_for(part)` gives the file of each part.
    pub fn start(
        urls: Vec<String>,
        http: StreamOptions,
        user_agent: &str,
        network_timeout_secs: u32,
        path_for: impl Fn(u32) -> PathBuf + 'static,
    ) -> Result<Self, String> {
        if urls.is_empty() {
            return Err(tr("The channel has no URL").into());
        }
        // See MpvBackend::new: libmpv refuses a non-C numeric locale.
        // SAFETY: called from the main thread.
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "null")?;
            init.set_property("ao", "null")?;
            init.set_property("ytdl", false)?;
            init.set_property("cache", "yes")?;
            init.set_property("network-timeout", i64::from(network_timeout_secs))?;
            init.set_property("keep-open", "no")?;
            init.set_property("idle", "yes")?;
            if let Ok(path) = std::env::var("TVMAXE_MPV_LOG") {
                init.set_property("log-file", format!("{path}.rec"))?;
            }
            Ok(())
        })
        .map_err(|e| fill(tr("cannot initialise libmpv (%1)"), &[&e]))?;

        let rec = Self {
            mpv,
            urls,
            http,
            user_agent: user_agent.to_string(),
            path_for: Box::new(path_for),
            index: Cell::new(0),
            tried: Cell::new(1),
            part: Cell::new(1),
            files: RefCell::default(),
            state: RefCell::new(RecorderState::Connecting),
            receiving: Cell::new(false),
            recorded_any: Cell::new(false),
            retry_at: Cell::new(None),
            last_error: RefCell::default(),
        };
        rec.load()?;
        Ok(rec)
    }

    fn load(&self) -> Result<(), String> {
        let path = (self.path_for)(self.part.get());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        apply_http(&self.mpv, &self.http, &self.user_agent);
        self.mpv
            .set_property("stream-record", path.to_string_lossy().as_ref())
            .map_err(|e| e.to_string())?;
        if !self.files.borrow().contains(&path) {
            self.files.borrow_mut().push(path);
        }
        self.receiving.set(false);
        let url = &self.urls[self.index.get()];
        self.mpv
            .command("loadfile", &[url, "replace"])
            .map_err(|e| fill(tr("Cannot start the stream: %1"), &[&e]))
    }

    /// Processes mpv's events (call it regularly from the main loop) and
    /// returns the current state.
    pub fn poll(&self) -> RecorderState {
        if let Some(at) = self.retry_at.get()
            && Instant::now() >= at
        {
            self.retry_at.set(None);
            self.next_part();
        }
        while let Some(ev) = self.mpv.wait_event(0.0) {
            match ev {
                Ok(Event::PlaybackRestart) => {
                    self.receiving.set(true);
                    self.recorded_any.set(true);
                    self.tried.set(1);
                    *self.state.borrow_mut() = RecorderState::Recording;
                }
                Ok(Event::EndFile(_)) => self.on_end(None),
                Err(e) => self.on_end(Some(describe_error(&e))),
                Ok(Event::Shutdown) => {
                    *self.state.borrow_mut() = RecorderState::Failed(tr("Recording stopped").into())
                }
                Ok(_) => {}
            }
        }
        self.state.borrow().clone()
    }

    /// The current URL ended or failed.
    fn on_end(&self, error: Option<String>) {
        if self.retry_at.get().is_some() {
            return; // already waiting to reconnect
        }
        if let Some(e) = &error {
            *self.last_error.borrow_mut() = e.clone();
        }
        if self.receiving.get() {
            // The stream dropped after data had been flowing: reconnect,
            // from the same URL, into a new part.
            self.tried.set(1);
            self.schedule_retry();
            return;
        }
        let n = self.urls.len();
        if self.tried.get() < n {
            self.tried.set(self.tried.get() + 1);
            self.index.set((self.index.get() + 1) % n);
            if let Err(e) = self.load() {
                *self.last_error.borrow_mut() = e;
                self.schedule_retry();
            }
        } else if self.recorded_any.get() {
            // Every URL failed this round, but something was recorded before:
            // keep trying until the stop time.
            self.tried.set(1);
            self.index.set((self.index.get() + 1) % n);
            self.schedule_retry();
        } else {
            let msg = match self.last_error.borrow().as_str() {
                "" => tr("The broadcast has ended").to_string(),
                e => e.to_string(),
            };
            *self.state.borrow_mut() = RecorderState::Failed(msg);
        }
    }

    fn schedule_retry(&self) {
        *self.state.borrow_mut() = RecorderState::Connecting;
        self.retry_at.set(Some(Instant::now() + RETRY_DELAY));
    }

    /// Continues in a new file, unless the current part is still empty.
    fn next_part(&self) {
        if self.current_part_has_data() {
            self.part.set(self.part.get() + 1);
        }
        if let Err(e) = self.load() {
            *self.last_error.borrow_mut() = e;
            self.schedule_retry();
        }
    }

    fn current_part_has_data(&self) -> bool {
        let path = (self.path_for)(self.part.get());
        std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
    }

    /// Files written so far (empty parts excluded).
    pub fn files(&self) -> Vec<PathBuf> {
        self.files
            .borrow()
            .iter()
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
            .cloned()
            .collect()
    }

    /// Stops recording, closes the file and returns the non-empty parts.
    /// Empty parts (a URL that never answered) are deleted.
    pub fn stop(self) -> Vec<PathBuf> {
        let _ = self.mpv.set_property("stream-record", "");
        let _ = self.mpv.command("stop", &[]);
        let all = self.files.borrow().clone();
        // Dropping the instance flushes and closes the output file.
        drop(self.mpv);
        let mut kept = Vec::new();
        for p in all {
            match std::fs::metadata(&p) {
                Ok(m) if m.len() > 0 => kept.push(p),
                Ok(_) => {
                    let _ = std::fs::remove_file(&p);
                }
                Err(_) => {}
            }
        }
        kept
    }
}
