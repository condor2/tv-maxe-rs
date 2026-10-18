//! Recordings: the scheduler, "Record now" and stopping on quit.
//!
//! Every second the scheduler asks [`recording::plan`] what to do with the
//! recordings stored in the database and starts or stops headless recorders
//! accordingly. A recording only runs while TV-Maxe is open.

use super::MainWindow;
use crate::player::{Recorder, RecorderState};
use gtk::glib;
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use tvmaxe_core::i18n::{fill, tr, trn};
use tvmaxe_core::net::unix_now;
use tvmaxe_core::recording::{self, Action, Recording, Status};
use tvmaxe_core::{Kind, StreamOptions};

impl MainWindow {
    pub(super) fn start_recording_scheduler(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(w) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            w.recordings_tick();
            glib::ControlFlow::Continue
        });
        // Recordings left in "recording" by a crash are finished right away.
        self.recordings_tick();
    }

    /// One scheduler step: start, stop and check the recordings.
    pub fn recordings_tick(&self) {
        let now = unix_now();
        let recs = self.db.borrow().recordings().unwrap_or_default();
        let running: Vec<i64> = self.recorders.borrow().keys().copied().collect();
        let mut changed = false;
        for action in recording::plan(&recs, &running, now) {
            changed = true;
            let id = match action {
                Action::Start(id) | Action::Stop(id) | Action::Miss(id) => id,
            };
            let Some(rec) = recs.iter().find(|r| r.id == id) else {
                continue;
            };
            match action {
                Action::Start(_) => self.begin_recording(rec),
                Action::Stop(_) => self.finish_recording(rec.clone(), None),
                Action::Miss(_) => {
                    let mut r = rec.clone();
                    r.status = Status::Missed;
                    self.save_and_repeat(r, now);
                }
            }
        }

        // Check the running recorders: failures and new file parts.
        let mut failed = Vec::new();
        for (id, recorder) in self.recorders.borrow().iter() {
            if let RecorderState::Failed(msg) = recorder.poll() {
                failed.push((*id, msg));
                continue;
            }
            let files = path_strings(&recorder.files());
            if let Some(mut r) = recs.iter().find(|r| r.id == *id).cloned()
                && r.files != files
            {
                r.files = files;
                let _ = self.db.borrow().update_recording(&r);
                changed = true;
            }
        }
        for (id, msg) in failed {
            if let Ok(Some(r)) = self.db.borrow().recording(id) {
                self.finish_recording(r, Some(msg));
            }
            changed = true;
        }

        if changed {
            self.update_recording_indicator();
            if let Some(d) = self
                .recordings_dialog
                .borrow()
                .clone()
                .filter(|d| d.is_visible())
            {
                d.refresh();
            }
        }
    }

    fn begin_recording(&self, rec: &Recording) {
        let settings = self.settings();
        let dir = settings.recordings_dir();
        let when = unique_when(&dir, rec);
        let (channel, title) = (rec.channel.clone(), rec.title.clone());
        let user_agent = settings
            .playback
            .user_agent
            .clone()
            .unwrap_or_else(|| tvmaxe_core::net::DEFAULT_USER_AGENT.to_string());
        let started = Recorder::start(
            rec.urls.clone(),
            rec.http.clone(),
            &user_agent,
            settings.playback.network_timeout_secs,
            move |part| dir.join(recording::file_name(&channel, &title, &when, part, "ts")),
        );
        let mut r = rec.clone();
        match started {
            Ok(recorder) => {
                r.status = Status::Recording;
                r.error = None;
                self.recorders.borrow_mut().insert(r.id, recorder);
                self.set_status(&fill(tr("Recording started: %1"), &[&r.display_title()]));
                let _ = self.db.borrow().update_recording(&r);
            }
            Err(e) => {
                r.status = Status::Failed;
                r.error = Some(e);
                self.save_and_repeat(r, unix_now());
            }
        }
    }

    /// Stops a recording (at its stop time, by the user, or after a failure)
    /// and records the outcome.
    fn finish_recording(&self, mut rec: Recording, error: Option<String>) {
        let now = unix_now();
        let recorder = self.recorders.borrow_mut().remove(&rec.id);
        let files = match recorder {
            Some(r) => path_strings(&r.stop()),
            // No recorder: left over from a crash; keep what was written.
            None => rec
                .files
                .iter()
                .filter(|f| std::fs::metadata(f).is_ok_and(|m| m.len() > 0))
                .cloned()
                .collect(),
        };
        if rec.stop.is_none_or(|stop| stop > now) {
            rec.stop = Some(now.max(rec.start));
        }
        rec.files = files;
        if rec.files.is_empty() {
            rec.status = Status::Failed;
            rec.error = Some(error.unwrap_or_else(|| tr("Nothing was recorded").to_string()));
            self.set_status(&fill(
                tr("Recording failed: %1 (%2)"),
                &[&rec.display_title(), rec.error.as_ref().unwrap()],
            ));
        } else {
            rec.status = Status::Done;
            rec.error = error;
            self.set_status(&fill(tr("Recording finished: %1"), &[&rec.display_title()]));
        }
        self.save_and_repeat(rec, now);
    }

    /// Saves a finished (or missed) recording and, if it repeats, schedules
    /// the next occurrence.
    fn save_and_repeat(&self, rec: Recording, now: i64) {
        let db = self.db.borrow();
        let _ = db.update_recording(&rec);
        if let Some(next) = rec.next_occurrence(now) {
            let _ = db.add_recording(&next);
        }
    }

    pub fn active_recordings(&self) -> usize {
        self.recorders.borrow().len()
    }

    /// The red "● REC" label in the status bar and the Record button.
    fn update_recording_indicator(&self) {
        let titles: Vec<String> = {
            let ids: Vec<i64> = self.recorders.borrow().keys().copied().collect();
            let db = self.db.borrow();
            ids.iter()
                .filter_map(|id| db.recording(*id).ok().flatten())
                .map(|r| r.display_title().to_string())
                .collect()
        };
        let n = titles.len();
        self.rec_indicator.set_visible(n > 0);
        self.rec_indicator.set_label(&match n {
            1 => fill(tr("● REC %1"), &[&titles[0]]),
            _ => trn("● %n recording", "● %n recordings", n as u64),
        });
        self.rec_indicator
            .set_tooltip_text(Some(&titles.join("\n")));
        let current = self.current_manual_recording().is_some();
        if current {
            self.record_btn.add_css_class("recording-active");
            self.record_btn
                .set_tooltip_text(Some(tr("Stop recording (Ctrl+Shift+R)")));
        } else {
            self.record_btn.remove_css_class("recording-active");
            self.record_btn
                .set_tooltip_text(Some(tr("Record (Ctrl+Shift+R)")));
        }
    }

    /// The running "Record now" recording of the current channel, if any.
    fn current_manual_recording(&self) -> Option<i64> {
        let cur = self.current.borrow();
        let c = cur.as_ref()?;
        let ids: Vec<i64> = self.recorders.borrow().keys().copied().collect();
        let db = self.db.borrow();
        ids.into_iter().find(|id| {
            db.recording(*id)
                .ok()
                .flatten()
                .is_some_and(|r| r.stop.is_none() && r.channel == c.name && r.urls == c.urls)
        })
    }

    /// The Record button / Ctrl+Shift+R: starts recording the current channel
    /// until stopped, or stops that recording.
    pub fn record_now(&self) {
        if let Some(id) = self.current_manual_recording() {
            self.stop_recording(id);
            return;
        }
        let draft = {
            let cur = self.current.borrow();
            let Some(c) = cur.as_ref() else {
                self.set_status(tr("Choose a channel to record"));
                return;
            };
            Recording::new(&c.name, c.kind, &c.urls, &c.http, unix_now(), None)
        };
        self.start_draft(draft);
    }

    /// "Record now" for any channel (channel menu).
    pub fn record_channel_now(
        &self,
        name: &str,
        kind: Kind,
        urls: &[String],
        http: &StreamOptions,
    ) {
        self.start_draft(Recording::new(name, kind, urls, http, unix_now(), None));
    }

    fn start_draft(&self, rec: Recording) {
        if let Err(e) = self.db.borrow().add_recording(&rec) {
            self.set_status(&fill(tr("Cannot save the recording: %1"), &[&e]));
            return;
        }
        self.recordings_tick();
    }

    /// Stops a running recording now; a scheduled one is cancelled.
    pub fn stop_recording(&self, id: i64) {
        let Ok(Some(rec)) = self.db.borrow().recording(id) else {
            return;
        };
        match rec.status {
            Status::Recording => {
                self.finish_recording(rec, None);
                self.update_recording_indicator();
            }
            Status::Scheduled => {
                let _ = self.db.borrow().delete_recording(id);
            }
            _ => {}
        }
        if let Some(d) = self.recordings_dialog.borrow().clone() {
            d.refresh();
        }
    }

    /// On quit: every running recording is stopped and kept.
    pub(super) fn stop_all_recordings(&self) {
        let ids: Vec<i64> = self.recorders.borrow().keys().copied().collect();
        for id in ids {
            if let Ok(Some(r)) = self.db.borrow().recording(id) {
                self.finish_recording(r, None);
            }
        }
    }

    pub(super) fn confirm_quit_while_recording(self: &Rc<Self>) {
        let n = self.active_recordings() as u64;
        let dialog = gtk::AlertDialog::builder()
            .message(trn(
                "%n recording is in progress",
                "%n recordings are in progress",
                n,
            ))
            .detail(tr(
                "Quitting stops recording. What was recorded so far is kept.",
            ))
            .buttons([tr("Cancel"), tr("Stop and quit")])
            .cancel_button(0)
            .default_button(0)
            .build();
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&w.window)).await == Ok(1) {
                w.quit_confirmed.set(true);
                w.window.close();
            }
        });
    }
}

/// The local time the recording actually starts, for file names
/// ("2026-10-14 20.30"), made unique if a file with that name already exists
/// (two recordings of the channel in the same minute).
fn unique_when(dir: &std::path::Path, rec: &Recording) -> String {
    let base = glib::DateTime::from_unix_local(unix_now())
        .and_then(|d| d.format("%Y-%m-%d %H.%M"))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut when = base.clone();
    let mut n = 2;
    while dir
        .join(recording::file_name(
            &rec.channel,
            &rec.title,
            &when,
            1,
            "ts",
        ))
        .exists()
    {
        when = format!("{base}-{n}");
        n += 1;
    }
    when
}

fn path_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}
