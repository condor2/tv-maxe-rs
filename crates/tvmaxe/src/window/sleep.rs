//! The sleep timer: countdown in the status bar, volume fade, and the action
//! at the end (stop, quit, suspend or shut down).

use super::MainWindow;
use crate::sleep::{self, SleepAction};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tvmaxe_core::i18n::{fill, tr};

pub(super) struct SleepTimer {
    deadline: Instant,
    action: SleepAction,
    source: glib::SourceId,
    warned: bool,
}

impl MainWindow {
    /// Starts (or restarts) the sleep timer.
    pub fn start_sleep_timer(self: &Rc<Self>, after: Duration, action: SleepAction) {
        self.cancel_sleep_timer();
        let w = Rc::downgrade(self);
        let source = glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(w) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if w.sleep_tick() {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        *self.sleep.borrow_mut() = Some(SleepTimer {
            deadline: Instant::now() + after,
            action,
            source,
            warned: false,
        });
        let at = crate::window::hhmm(
            tvmaxe_core::net::unix_now() + i64::try_from(after.as_secs()).unwrap_or(0),
        );
        self.set_status(&fill(
            tr("Sleep timer: %1 at %2"),
            &[&action.label().to_lowercase(), &at],
        ));
        self.sleep_indicator.set_tooltip_text(Some(&fill(
            tr("Sleep timer: %1 at %2"),
            &[&action.label().to_lowercase(), &at],
        )));
        self.sleep_tick();
    }

    /// Cancels the timer and puts the volume back.
    pub fn cancel_sleep_timer(&self) {
        if let Some(t) = self.sleep.borrow_mut().take() {
            t.source.remove();
            self.player.set_volume(self.volume.value());
            self.set_status(tr("Sleep timer cancelled"));
        }
        self.sleep_indicator.set_visible(false);
    }

    /// Time left, if the timer is running.
    pub fn sleep_remaining(&self) -> Option<Duration> {
        self.sleep
            .borrow()
            .as_ref()
            .map(|t| t.deadline.saturating_duration_since(Instant::now()))
    }

    pub fn sleep_action(&self) -> Option<SleepAction> {
        self.sleep.borrow().as_ref().map(|t| t.action)
    }

    /// One step of the countdown; `false` when the timer has ended.
    fn sleep_tick(self: &Rc<Self>) -> bool {
        let Some(left) = self.sleep_remaining() else {
            return false;
        };
        self.sleep_indicator.set_visible(true);
        self.sleep_indicator
            .set_label(&format!("☾ {}", sleep::format_remaining(left)));

        // The volume goes down during the last seconds.
        if self.player.is_active() {
            self.player
                .set_volume(self.volume.value() * sleep::volume_factor(left));
        }
        if left <= Duration::from_secs(60) {
            let mut warn = false;
            if let Some(t) = self.sleep.borrow_mut().as_mut()
                && !t.warned
            {
                t.warned = true;
                warn = true;
            }
            if warn {
                self.set_status(tr("Sleep timer: one minute left"));
            }
        }
        if !left.is_zero() {
            return true;
        }

        let action = self.sleep.borrow_mut().take().map(|t| t.action);
        self.sleep_indicator.set_visible(false);
        if let Some(action) = action {
            self.sleep_finish(action);
        }
        false
    }

    fn sleep_finish(self: &Rc<Self>, action: SleepAction) {
        self.player.stop();
        self.player.set_volume(self.volume.value());
        if action.ends_recordings() && self.active_recordings() > 0 {
            // Never lose a recording to the sleep timer.
            self.set_status(tr(
                "Sleep timer: playback stopped; TV-Maxe stays open because a recording is running",
            ));
            return;
        }
        match action {
            SleepAction::StopPlayback => {
                self.set_status(tr("Sleep timer: playback stopped"));
            }
            SleepAction::Quit => {
                self.window.close();
            }
            SleepAction::Suspend | SleepAction::PowerOff => {
                let method = action.logind_method().unwrap_or_default();
                if action == SleepAction::PowerOff {
                    // Settings are saved before the session goes away.
                    self.save_state();
                }
                let w = Rc::downgrade(self);
                glib::spawn_future_local(async move {
                    if let Err(e) = logind(method, (false,).to_variant()).await
                        && let Some(w) = w.upgrade()
                    {
                        w.set_status(&fill(
                            tr("Sleep timer: %1 failed (%2)"),
                            &[&action.label(), &e],
                        ));
                    }
                });
            }
        }
    }
}

/// Calls a method of logind's Manager on the system bus.
async fn logind(method: &str, args: glib::Variant) -> Result<glib::Variant, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::System).await?;
    bus.call_future(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        10_000,
    )
    .await
}

/// Whether the system allows `action` ("yes" or "challenge" from logind).
/// Actions that do not need the system are always available.
pub async fn sleep_action_available(action: SleepAction) -> bool {
    let Some(check) = action.logind_check() else {
        return true;
    };
    match logind(check, ().to_variant()).await {
        Ok(v) => v
            .get::<(String,)>()
            .is_some_and(|(s,)| s == "yes" || s == "challenge"),
        Err(_) => false,
    }
}
