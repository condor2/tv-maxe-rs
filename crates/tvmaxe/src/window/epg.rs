//! Updating and showing the TV guide.

use super::{GuideTarget, MainWindow, hhmm};
use crate::channel_list::row_of;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;
use tvmaxe_core::epg::Programme;
use tvmaxe_core::i18n::{fill, tr};
use tvmaxe_core::net;

impl MainWindow {
    // ---------- TV guide ----------

    /// Downloads the EPG sources (from the lists + Preferences). Without `force`, only
    /// the ones older than 12 hours.
    pub fn refresh_epg(self: &Rc<Self>, force: bool) {
        let now = net::unix_now();
        let urls: Vec<String> = {
            let db = self.db.borrow();
            let mut urls = db.epg_source_urls().unwrap_or_default();
            for u in &self.settings.borrow().extra_epg_urls {
                if !urls.contains(u) {
                    urls.push(u.clone());
                }
            }
            urls.into_iter()
                .filter(|u| force || db.epg_fetched_at(u).is_none_or(|t| now - t > 12 * 3600))
                .collect()
        };
        if urls.is_empty() {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.begin_busy();
            let total = urls.len();
            let mut failed = Vec::new();
            for (i, url) in urls.into_iter().enumerate() {
                this.list_status.set_label(&fill(
                    tr("Downloading the TV guide (%1/%2)…"),
                    &[&(i + 1), &total],
                ));
                let path = this.db_path.clone();
                let u = url.clone();
                match gio::spawn_blocking(move || net::update_epg(&path, &u)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => failed.push(format!("{url}: {e}")),
                    Err(_) => failed.push(format!("{url}: {}", tr("internal error"))),
                }
            }
            this.end_busy();
            let idx = this.db.borrow().epg_index().unwrap_or_default();
            *this.epg_index.borrow_mut() = idx;
            this.reload();
            this.retarget_current_epg();
            if let Some(d) = this
                .guide_dialog
                .borrow()
                .clone()
                .filter(|d| d.is_visible())
            {
                d.refresh();
            }
            if !failed.is_empty() && !this.player.is_active() {
                this.set_status(&fill(tr("TV guide unavailable: %1"), &[&failed.join("; ")]));
            }
        });
    }

    /// After a guide update, the current channel may now have a key.
    fn retarget_current_epg(&self) {
        let found = {
            let cur = self.current.borrow();
            cur.as_ref().and_then(|c| {
                let url = c.urls.first()?;
                [&self.tv, &self.radio]
                    .iter()
                    .find_map(|l| l.epg_key_for_url(url))
            })
        };
        if let Some(c) = self.current.borrow_mut().as_mut() {
            c.epg_key = found;
        }
        self.update_epg_info();
    }

    /// "Now playing" in the lists + the panel under the video.
    pub fn refresh_epg_view(&self) {
        // The programme on air may have changed.
        self.mpris_sync();
        let current = self
            .db
            .borrow()
            .epg_current(net::unix_now())
            .unwrap_or_default();
        for list in [&self.tv, &self.radio] {
            list.update_now(|key| current.get(key).map(|p| p.title.clone()));
        }
        self.update_epg_info();
    }

    pub(super) fn update_epg_info(&self) {
        let key = self
            .current
            .borrow()
            .as_ref()
            .and_then(|c| c.epg_key.clone());
        let Some(key) = key else {
            self.epg_box.set_visible(false);
            return;
        };
        let now = net::unix_now();
        let (cur, next) = self
            .db
            .borrow()
            .epg_now_next(&key, now)
            .unwrap_or((None, None));
        if cur.is_none() && next.is_none() {
            self.epg_box.set_visible(false);
            return;
        }
        match &cur {
            Some(p) => {
                self.epg_now
                    .set_label(&format!("{}–{}  {}", hhmm(p.start), hhmm(p.stop), p.title));
                self.epg_now.set_tooltip_text(p.desc.as_deref());
                self.epg_progress.set_fraction(p.progress(now));
                self.epg_progress.set_visible(true);
            }
            None => {
                self.epg_now.set_label(tr("Now: no programme in the guide"));
                self.epg_now.set_tooltip_text(None);
                self.epg_progress.set_visible(false);
            }
        }
        match &next {
            Some(p) => self
                .epg_next
                .set_label(&fill(tr("Next %1  %2"), &[&hhmm(p.start), &p.title])),
            None => self.epg_next.set_label(""),
        }
        self.epg_box.set_visible(!self.window.is_fullscreen());
    }

    /// The channel for the TV guide: the one playing, otherwise the selected one.
    pub fn guide_target(&self) -> Option<GuideTarget> {
        if let Some(c) = self.current.borrow().as_ref() {
            return Some(GuideTarget {
                name: c.name.clone(),
                kind: c.kind,
                epg_key: c.epg_key.clone(),
                texture: c.texture.clone(),
                urls: c.urls.clone(),
                http: c.http.clone(),
            });
        }
        let obj = self.current_list().selected()?;
        let row = row_of(obj.upcast_ref());
        Some(GuideTarget {
            name: row.channel.name.clone(),
            kind: row.channel.kind,
            epg_key: row.epg_key.clone(),
            texture: row.texture(),
            urls: row.channel.urls.clone(),
            http: row.channel.http.clone(),
        })
    }

    /// The programme on air on the current channel: (title, end).
    pub fn current_programme(&self) -> Option<(String, i64)> {
        let key = self.current.borrow().as_ref()?.epg_key.clone()?;
        let now = net::unix_now();
        let (cur, _) = self.db.borrow().epg_now_next(&key, now).ok()?;
        cur.map(|p| (p.title, p.stop))
    }

    pub fn epg_range(&self, key: &str, from: i64, to: i64) -> Vec<Programme> {
        self.db
            .borrow()
            .epg_range(key, from, to)
            .unwrap_or_default()
    }

    /// Is at least one guide source configured?
    pub fn has_epg_sources(&self) -> bool {
        !self.settings.borrow().extra_epg_urls.is_empty()
            || !self
                .db
                .borrow()
                .epg_source_urls()
                .unwrap_or_default()
                .is_empty()
    }
}
