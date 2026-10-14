//! Favourites and keyboard zapping: next / previous channel, channel numbers,
//! back to the previous channel, and the on-screen display (OSD) on the video.

use super::MainWindow;
use crate::channel_list::{ChannelRow, row_of};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::rc::Rc;
use std::time::Duration;
use tvmaxe_core::Kind;
use tvmaxe_core::i18n::{fill, tr};

/// How long typed digits wait for the next one before switching.
const DIGIT_WAIT: Duration = Duration::from_millis(1500);
/// How long the OSD stays on the video.
const OSD_TIME: Duration = Duration::from_millis(2500);

impl MainWindow {
    // ---------- Favourites ----------

    /// Reads the favourites and marks them in both lists.
    pub(super) fn apply_favourites(&self) {
        let subs = self.db.borrow().subscriptions().unwrap_or_default();
        for list in [&self.tv, &self.radio] {
            let favs = self.db.borrow().favourites(list.kind).unwrap_or_default();
            if list.set_favourites(&favs) {
                list.set_subscriptions(&subs);
            }
        }
        self.update_favourite_button();
    }

    /// Adds the channel to the favourites, or removes it.
    pub fn toggle_favourite(&self, row: &ChannelRow) {
        let Some(url) = row.channel.urls.first() else {
            return;
        };
        let on = !row.favourite.get();
        let saved = self
            .db
            .borrow()
            .set_favourite(row.channel.kind, url, &row.channel.name, on);
        if let Err(e) = saved {
            self.set_status(&fill(tr("Cannot save the favourites: %1"), &[&e]));
            return;
        }
        self.apply_favourites();
        self.set_status(&fill(
            if on {
                tr("“%1” was added to the favourites")
            } else {
                tr("“%1” was removed from the favourites")
            },
            &[&row.channel.name],
        ));
    }

    /// Ctrl+D / the ☆ button: the channel that is playing, or the selected one.
    pub fn toggle_current_favourite(&self) {
        let obj = self
            .current_item()
            .or_else(|| self.current_list().selected());
        match obj {
            Some(o) => self.toggle_favourite(&row_of(o.upcast_ref())),
            None => self.set_status(tr("Choose a channel first")),
        }
    }

    /// The list item of the channel that is playing, if it is still in a list.
    fn current_item(&self) -> Option<glib::BoxedAnyObject> {
        let cur = self.current.borrow();
        let c = cur.as_ref()?;
        let url = c.urls.first()?;
        self.list(c.kind).item_by_url(url)
    }

    /// The ☆ / ★ button follows the channel that is playing.
    pub(super) fn update_favourite_button(&self) {
        let fav = self
            .current_item()
            .is_some_and(|o| row_of(o.upcast_ref()).favourite.get());
        self.fav_btn.set_icon_name(if fav {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        });
        self.fav_btn.set_tooltip_text(Some(if fav {
            tr("Remove from favourites (Ctrl+D)")
        } else {
            tr("Add to favourites (Ctrl+D)")
        }));
        self.fav_btn.set_sensitive(self.current_item().is_some());
    }

    // ---------- Zapping ----------

    /// The list zapping works on: the visible tab.
    fn zap_list(&self) -> &Rc<crate::channel_list::ChannelList> {
        self.current_list()
    }

    /// Position of the playing (or else selected) channel among those shown.
    fn zap_position(&self) -> Option<u32> {
        let list = self.zap_list();
        let playing = self
            .current
            .borrow()
            .as_ref()
            .filter(|c| c.kind == list.kind)
            .and_then(|c| c.urls.first().cloned());
        playing
            .and_then(|url| list.visible_position(&url))
            .or_else(|| list.selected_position())
    }

    /// Next (`+1`) or previous (`-1`) channel in the list as it is shown
    /// (search, group and favourites filters included), wrapping around.
    pub fn zap(&self, delta: i32) {
        let list = self.zap_list();
        let n = list.visible_len();
        if n == 0 {
            return;
        }
        let pos = match self.zap_position() {
            Some(p) => (i64::from(p) + i64::from(delta)).rem_euclid(i64::from(n)) as u32,
            None if delta < 0 => n - 1,
            None => 0,
        };
        self.zap_to_position(pos);
    }

    /// Channel number `number` (1 = the first one shown).
    pub fn zap_to_number(&self, number: u32) {
        let list = self.zap_list();
        if number == 0 || number > list.visible_len() {
            self.show_osd(&fill(tr("No channel %1"), &[&number]));
            return;
        }
        self.zap_to_position(number - 1);
    }

    fn zap_to_position(&self, pos: u32) {
        let list = self.zap_list();
        let Some(obj) = list.visible_item(pos) else {
            return;
        };
        list.select_visible(pos);
        let row = row_of(obj.upcast_ref());
        self.show_osd(&format!("{}  {}", pos + 1, row.channel.name));
        self.play_row(&row);
    }

    /// Back to the channel watched before (Backspace).
    pub fn zap_back(&self) {
        let Some((kind, url)) = self.previous.borrow().clone() else {
            return;
        };
        let Some(obj) = self.list(kind).item_by_url(&url) else {
            return;
        };
        let page = u32::from(kind == Kind::Radio);
        if self.notebook.current_page() != Some(page) {
            self.notebook.set_current_page(Some(page));
        }
        let row = row_of(obj.upcast_ref());
        let pos = self.list(kind).visible_position(&url);
        if let Some(p) = pos {
            self.list(kind).select_visible(p);
        }
        let label = match pos {
            Some(p) => format!("{}  {}", p + 1, row.channel.name),
            None => row.channel.name.clone(),
        };
        self.show_osd(&label);
        self.play_row(&row);
    }

    /// A digit typed on the keyboard: channel numbers of several digits are
    /// collected until a short pause.
    fn zap_digit(self: &Rc<Self>, digit: u32) {
        let mut typed = self.typed_number.borrow_mut();
        if typed.len() >= 4 {
            typed.clear();
        }
        typed.push(char::from_digit(digit, 10).unwrap_or('0'));
        let text = typed.clone();
        drop(typed);
        self.show_osd(&format!("{text}–"));
        if let Some(id) = self.digit_timer.take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(DIGIT_WAIT, move || {
            let Some(w) = w.upgrade() else { return };
            w.digit_timer.take();
            let typed = std::mem::take(&mut *w.typed_number.borrow_mut());
            if let Ok(n) = typed.parse::<u32>() {
                w.zap_to_number(n);
            }
        });
        self.digit_timer.replace(Some(id));
    }

    /// Text over the video for a few seconds.
    pub fn show_osd(&self, text: &str) {
        self.osd.set_label(text);
        self.osd.set_visible(true);
        if let Some(id) = self.osd_timer.take() {
            id.remove();
        }
        let osd = self.osd.downgrade();
        let id = glib::timeout_add_local_once(OSD_TIME, move || {
            if let Some(osd) = osd.upgrade() {
                osd.set_visible(false);
            }
        });
        // The timer may fire later than a new message; the old id is dropped above.
        self.osd_timer.replace(Some(id));
    }

    pub fn osd_text(&self) -> Option<String> {
        self.osd.is_visible().then(|| self.osd.label().to_string())
    }

    /// Keys for zapping, handled before the focused widget (so Page Up/Down
    /// change the channel even while the channel list has the focus). Ignored
    /// while typing in a text field.
    pub(super) fn zap_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>());
        let mods = state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK);
        if typing || !mods.is_empty() {
            return false;
        }
        match key {
            gdk::Key::Page_Down | gdk::Key::KP_Page_Down => self.zap(1),
            gdk::Key::Page_Up | gdk::Key::KP_Page_Up => self.zap(-1),
            gdk::Key::BackSpace => self.zap_back(),
            gdk::Key::a => self.cycle_audio_track(),
            k => match k.to_unicode().and_then(|c| c.to_digit(10)) {
                Some(d) => self.zap_digit(d),
                None => return false,
            },
        }
        true
    }
}
