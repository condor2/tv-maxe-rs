//! Gestionarea abonamentelor de canale.

use crate::window::MainWindow;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::{Rc, Weak};
use tvmaxe_core::Subscription;
use tvmaxe_core::i18n::{fill, tr};

// ---------- Abonamente ----------

pub struct SubscriptionsDialog {
    window: gtk::Window,
    list: gtk::ListBox,
    main: Weak<MainWindow>,
}

impl SubscriptionsDialog {
    pub fn present(main: &Rc<MainWindow>) {
        if let Some(d) = main.subs_dialog.borrow().clone() {
            d.refresh();
            d.window.present();
            return;
        }

        let intro = gtk::Label::builder()
            .label(tr(
                "The channel lists you are subscribed to. TV-Maxe (.db) and M3U lists are accepted, \
                 from the internet or from disk. Only add lists you have the right to use.",
            ))
            .wrap(true)
            .xalign(0.0)
            .build();

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .min_content_height(240)
            .has_frame(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let entry = gtk::Entry::builder()
            .placeholder_text(tr("https://…/list.db  or  /path/list.m3u"))
            .hexpand(true)
            .build();
        let file_btn = gtk::Button::with_label(tr("File…"));
        let add_btn = gtk::Button::with_label(tr("Add"));
        add_btn.add_css_class("suggested-action");
        let add_row = gtk::Box::builder().spacing(6).build();
        add_row.append(&entry);
        add_row.append(&file_btn);
        add_row.append(&add_btn);

        let refresh_btn = gtk::Button::with_label(tr("Update all"));
        let close_btn = gtk::Button::with_label(tr("Close"));
        let footer = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::End)
            .build();
        footer.append(&refresh_btn);
        footer.append(&close_btn);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&intro);
        content.append(&scrolled);
        content.append(&add_row);
        content.append(&footer);

        let window = gtk::Window::builder()
            .title(tr("Subscriptions"))
            .transient_for(&main.window)
            .modal(true)
            // Se ascunde la închidere și e refolosită: o fereastră distrusă nu
            // mai poate fi reafișată (GTK o lasă într-o stare inconsistentă).
            .hide_on_close(true)
            .default_width(600)
            .default_height(440)
            .child(&content)
            .build();

        let this = Rc::new(Self {
            window,
            list,
            main: Rc::downgrade(main),
        });

        let add = {
            let main = Rc::downgrade(main);
            let entry = entry.clone();
            move || {
                if let Some(main) = main.upgrade() {
                    main.add_subscription(&entry.text());
                    entry.set_text("");
                }
            }
        };
        let add2 = add.clone();
        add_btn.connect_clicked(move |_| add2());
        entry.connect_activate(move |_| add());

        let w = this.window.downgrade();
        let main_w = Rc::downgrade(main);
        file_btn.connect_clicked(move |_| {
            let (Some(win), Some(main)) = (w.upgrade(), main_w.upgrade()) else {
                return;
            };
            glib::spawn_future_local(async move {
                let dialog = gtk::FileDialog::builder()
                    .title(tr("Choose a channel list"))
                    .filters(&list_filters())
                    .build();
                if let Ok(file) = dialog.open_future(Some(&win)).await
                    && let Some(path) = file.path()
                {
                    main.add_subscription(&path.to_string_lossy());
                }
            });
        });

        let main_w = Rc::downgrade(main);
        refresh_btn.connect_clicked(move |_| {
            if let Some(main) = main_w.upgrade() {
                main.refresh_subscriptions(None);
            }
        });
        let w = this.window.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.close();
            }
        });

        *main.subs_dialog.borrow_mut() = Some(this.clone());
        this.refresh();
        this.window.present();
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn refresh(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        self.list.remove_all();
        let subs = main.db.borrow().subscriptions().unwrap_or_default();
        if subs.is_empty() {
            let empty = gtk::Label::builder()
                .label(tr("No subscriptions yet."))
                .margin_top(24)
                .margin_bottom(24)
                .build();
            empty.add_css_class("dim-label");
            self.list.append(&empty);
        }
        for sub in subs {
            self.list.append(&self.row(&main, sub));
        }
    }

    fn row(&self, main: &Rc<MainWindow>, sub: Subscription) -> gtk::Box {
        let check = gtk::CheckButton::builder()
            .active(sub.enabled)
            .tooltip_text(tr("Show the channels from this list"))
            .valign(gtk::Align::Center)
            .build();
        let id = sub.id;
        let main_w = Rc::downgrade(main);
        check.connect_toggled(move |c| {
            let Some(main) = main_w.upgrade() else { return };
            let _ = main.db.borrow().set_subscription_enabled(id, c.is_active());
            // Reconstruirea rândurilor se face după ce se termină semnalul.
            glib::idle_add_local_once(move || main.reload());
        });

        let name = gtk::Label::builder()
            .label(sub.display_name())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("channel-title");
        let url = gtk::Label::builder()
            .label(if sub.is_local() {
                tr("Channels added by hand (TV-Maxe → Add channel…)")
            } else {
                &sub.url
            })
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        url.add_css_class("dim-label");
        url.add_css_class("caption");
        let counts = fill(tr("%1 TV · %2 radio"), &[&sub.tv_count, &sub.radio_count]);
        let (status_text, is_error) = match (&sub.last_error, sub.updated_at) {
            (Some(e), _) => (fill(tr("Error: %1"), &[e]), true),
            _ if sub.is_local() => (counts, false),
            (None, Some(t)) => (
                fill(tr("%1 · updated %2"), &[&counts, &format_time(t)]),
                false,
            ),
            (None, None) => (tr("Not updated yet").into(), false),
        };
        let status = gtk::Label::builder()
            .label(&status_text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(&status_text)
            .build();
        status.add_css_class("caption");
        status.add_css_class(if is_error { "error" } else { "dim-label" });

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .spacing(2)
            .build();
        text.append(&name);
        text.append(&url);
        text.append(&status);

        let row = gtk::Box::builder()
            .spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        row.append(&check);
        row.append(&text);

        if !sub.is_local() {
            let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
            refresh.set_tooltip_text(Some(tr("Update list")));
            refresh.set_valign(gtk::Align::Center);
            refresh.add_css_class("flat");
            let main_w = Rc::downgrade(main);
            refresh.connect_clicked(move |_| {
                if let Some(main) = main_w.upgrade() {
                    main.refresh_subscriptions(Some(vec![id]));
                }
            });

            let remove = gtk::Button::from_icon_name("user-trash-symbolic");
            remove.set_tooltip_text(Some(tr("Remove subscription")));
            remove.set_valign(gtk::Align::Center);
            remove.add_css_class("flat");
            let main_w = Rc::downgrade(main);
            let win = self.window.downgrade();
            let sub_name = sub.display_name().to_string();
            remove.connect_clicked(move |_| {
                let (Some(main), Some(win)) = (main_w.upgrade(), win.upgrade()) else {
                    return;
                };
                let sub_name = sub_name.clone();
                glib::spawn_future_local(async move {
                    let confirm = gtk::AlertDialog::builder()
                        .message(fill(tr("Remove the subscription “%1”?"), &[&sub_name]))
                        .detail(tr(
                            "The channels from this list will disappear from TV-Maxe.",
                        ))
                        .buttons([tr("Cancel"), tr("Remove")])
                        .cancel_button(0)
                        .default_button(0)
                        .build();
                    if confirm.choose_future(Some(&win)).await == Ok(1) {
                        let _ = main.db.borrow().remove_subscription(id);
                        main.reload();
                    }
                });
            });
            row.append(&refresh);
            row.append(&remove);
        }
        row
    }
}

fn format_time(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix)
        .and_then(|d| d.format("%x %H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn list_filters() -> gio::ListStore {
    let lists = gtk::FileFilter::new();
    lists.set_name(Some(tr("Channel lists (TV-Maxe, M3U)")));
    for s in ["db", "m3u", "m3u8"] {
        lists.add_suffix(s);
    }
    let all = gtk::FileFilter::new();
    all.set_name(Some(tr("All files")));
    all.add_pattern("*");
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&lists);
    store.append(&all);
    store
}
