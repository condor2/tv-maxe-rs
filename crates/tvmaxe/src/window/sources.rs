//! Abonamente, canale și logo-uri.

use super::MainWindow;
use crate::channel_list::ChannelRow;
use gtk::{gdk, gio, glib};
use std::collections::HashMap;
use std::rc::Rc;
use tvmaxe_core::i18n::{fill, tr, trn};
use tvmaxe_core::net;

impl MainWindow {
    /// Recitește abonamentele și canalele din bază.
    pub fn reload(&self) {
        let db = self.db.borrow();
        let subs = match db.subscriptions() {
            Ok(s) => s,
            Err(e) => return self.set_status(&fill(tr("Error reading the database: %1"), &[&e])),
        };
        let names: HashMap<i64, String> = subs
            .iter()
            .map(|s| (s.id, s.display_name().to_string()))
            .collect();
        let idx = self.epg_index.borrow();
        for list in [&self.tv, &self.radio] {
            list.set_subscriptions(&subs);
            let rows = db
                .channels(list.kind)
                .unwrap_or_default()
                .into_iter()
                .map(|c| {
                    let sub = names.get(&c.subscription_id).cloned().unwrap_or_default();
                    let key = idx.resolve(c.guide_id.as_deref(), &c.name);
                    ChannelRow::new(c, sub, key)
                })
                .collect();
            list.set_channels(rows);
        }
        let active = subs
            .iter()
            .filter(|s| s.enabled && s.tv_count + s.radio_count > 0)
            .count();
        self.list_status.set_label(&fill(
            tr("%1, %2 in %3"),
            &[
                &trn("%n TV channel", "%n TV channels", self.tv.len().into()),
                &trn(
                    "%n radio station",
                    "%n radio stations",
                    self.radio.len().into(),
                ),
                &trn("%n list", "%n lists", active as u64),
            ],
        ));
        drop(idx);
        drop(db);
        self.refresh_epg_view();
        if let Some(d) = self.subs_dialog.borrow().clone().filter(|d| d.is_visible()) {
            d.refresh();
        }
        if self.current.borrow().is_none() && self.busy.get() == 0 {
            self.set_status(if subs.is_empty() {
                tr("Welcome! Add a channel list from TV-Maxe → Subscriptions…")
            } else {
                tr("Ready")
            });
        }
    }

    /// Actualizează abonamentele (toate cele active dacă `ids` e `None`),
    /// apoi descarcă logo-urile lipsă. Totul pe fire separate.
    pub fn refresh_subscriptions(self: &Rc<Self>, ids: Option<Vec<i64>>) {
        let ids: Vec<(i64, String)> = {
            let subs = self.db.borrow().subscriptions().unwrap_or_default();
            subs.into_iter()
                .filter(|s| {
                    !s.is_local() && ids.as_ref().map_or(s.enabled, |ids| ids.contains(&s.id))
                })
                .map(|s| (s.id, s.display_name().to_string()))
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.begin_busy();
            let total = ids.len();
            let mut failed = Vec::new();
            for (i, (id, name)) in ids.into_iter().enumerate() {
                // Progresul stă sub listă (ca în TV-Maxe), nu peste starea redării.
                this.list_status.set_label(&fill(
                    tr("Updating “%1” (%2/%3)…"),
                    &[&name, &(i + 1), &total],
                ));
                let path = this.db_path.clone();
                match gio::spawn_blocking(move || net::update_subscription(&path, id)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => failed.push(format!("{name}: {e}")),
                    Err(_) => failed.push(format!("{name}: {}", tr("internal error"))),
                }
                this.reload();
            }
            this.end_busy();
            if failed.is_empty() {
                if !this.player.is_active() {
                    this.set_status(tr("The lists have been updated"));
                }
            } else {
                this.set_status(&fill(tr("Could not update: %1"), &[&failed.join("; ")]));
            }
            this.refresh_epg(false);
        });
    }

    // ---------- Logo-uri la cerere ----------

    /// Un rând afișat are nevoie de logo. Cererile se deduplică; cele mai
    /// recente (rândurile tocmai derulate în vizor) se descarcă primele.
    pub(super) fn request_logo(self: &Rc<Self>, url: &str) {
        if !self.settings.borrow().download_logos
            || !self.logo_seen.borrow_mut().insert(url.to_string())
        {
            return;
        }
        self.logo_queue.borrow_mut().push(url.to_string());
        self.pump_logos();
    }

    fn pump_logos(self: &Rc<Self>) {
        const PARALLEL: usize = 4;
        while self.logo_inflight.get() < PARALLEL {
            let Some(url) = self.logo_queue.borrow_mut().pop() else {
                break;
            };
            self.logo_inflight.set(self.logo_inflight.get() + 1);
            let this = self.clone();
            glib::spawn_future_local(async move {
                let path = this.db_path.clone();
                let u = url.clone();
                let res = gio::spawn_blocking(move || net::fetch_logo(&path, &u)).await;
                this.logo_inflight.set(this.logo_inflight.get() - 1);
                if let Ok(Ok(Some(bytes))) = res
                    && let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))
                {
                    this.tv.set_logo(&url, &tex);
                    this.radio.set_logo(&url, &tex);
                }
                this.pump_logos();
            });
        }
    }

    pub fn add_subscription(self: &Rc<Self>, source: &str) {
        let source = source.trim();
        if source.is_empty() {
            return;
        }
        match self.db.borrow().add_subscription(source, source) {
            Ok(id) => {
                self.refresh_subscriptions(Some(vec![id]));
            }
            Err(e) => self.set_status(&fill(tr("Cannot add the list: %1"), &[&e])),
        }
        self.reload();
    }

    pub(super) fn delete_channel(&self, id: i64) {
        match self.db.borrow().delete_local_channel(id) {
            Ok(true) => self.set_status(tr("The channel was removed from “My channels”")),
            Ok(false) => {}
            Err(e) => self.set_status(&fill(tr("Cannot delete the channel: %1"), &[&e])),
        }
        self.reload();
    }

    /// Prima pornire: preia lista locală a vechiului TV-Maxe (`~/.tvmaxe/userlist.db`).
    pub(super) fn import_legacy_userlist(&self) {
        let db = self.db.borrow();
        if db.setting::<bool>("legacy_import_done").unwrap_or(false) {
            return;
        }
        let _ = db.set_setting("legacy_import_done", true);
        let path = glib::home_dir().join(".tvmaxe/userlist.db");
        if !path.exists() {
            return;
        }
        drop(db);
        let imported = tvmaxe_core::formats::parse_tvmaxe_db_file(&path).and_then(|list| {
            let mut db = self.db.borrow_mut();
            let id = db.local_list_id()?;
            db.replace_channels(id, &list)
        });
        if let Ok(n) = imported {
            self.set_status(&trn(
                "Imported %n channel from the old TV-Maxe",
                "Imported %n channels from the old TV-Maxe",
                n as u64,
            ));
        }
    }

    /// Selectează ultimul canal folosit. Întoarce `true` dacă l-a găsit.
    pub(super) fn restore_last_channel(&self) -> bool {
        let Some(url) = self.db.borrow().setting::<String>("last_channel") else {
            return false;
        };
        if self.current_list().select_by_url(&url) {
            return true;
        }
        for (page, list) in [(0, &self.tv), (1, &self.radio)] {
            if list.select_by_url(&url) {
                self.notebook.set_current_page(Some(page));
                return true;
            }
        }
        false
    }
}
