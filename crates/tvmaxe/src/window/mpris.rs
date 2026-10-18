//! MPRIS glue: what the desktop sees of the window, and what it can ask for.

use super::MainWindow;
use crate::mpris::{Command, Mpris, State, Status};
use gtk::prelude::*;
use std::rc::Rc;

impl MainWindow {
    /// Makes the player visible to the desktop (media keys, sound applet,
    /// `playerctl`). Called once, after the window is built.
    pub fn start_mpris(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        let service = Mpris::start(move |cmd| {
            if let Some(w) = w.upgrade() {
                w.mpris_command(cmd);
            }
        });
        *self.mpris.borrow_mut() = Some(service);
        self.mpris_sync();
    }

    /// Sends the current state to the desktop; only changes are announced.
    pub(super) fn mpris_sync(&self) {
        let Ok(service) = self.mpris.try_borrow() else {
            return;
        };
        let Some(service) = service.as_ref() else {
            return;
        };
        let channel = self
            .current
            .borrow()
            .as_ref()
            .map(|c| (c.channel_id, c.name.clone()));
        let status = if !self.player.is_active() {
            Status::Stopped
        } else if self.player.is_paused() {
            Status::Paused
        } else {
            Status::Playing
        };
        service.update(State {
            status,
            programme: channel
                .as_ref()
                .and_then(|_| self.current_programme())
                .map(|(title, _)| title),
            channel,
            volume: self.volume.value() / 100.0,
            can_navigate: self.current_list().visible_len() > 0,
        });
    }

    fn mpris_command(&self, cmd: Command) {
        match cmd {
            Command::Raise => self.window.present(),
            // Goes through close-request: the recordings confirmation applies.
            Command::Quit => self.window.close(),
            Command::Next => self.zap(1),
            Command::Previous => self.zap(-1),
            Command::Play => {
                if !self.player.is_active() || self.player.is_paused() {
                    self.play_or_pause();
                }
            }
            Command::Pause => {
                if self.player.is_active() && !self.player.is_paused() {
                    self.player.toggle_pause();
                }
            }
            Command::PlayPause => self.play_or_pause(),
            Command::Stop => self.player.stop(),
            Command::SetVolume(v) => self.volume.set_value(v * 100.0),
        }
    }
}
