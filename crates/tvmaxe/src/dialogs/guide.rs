//! The TV guide window.

use crate::window::MainWindow;
use gtk::glib;
use gtk::prelude::*;
use std::rc::{Rc, Weak};
use tvmaxe_core::i18n::{fill, tr};

// ---------- TV guide ----------

/// A channel's schedule by day, like the "TV programme" window of TV-Maxe.
pub struct GuideDialog {
    window: gtk::Window,
    logo: gtk::Image,
    title: gtk::Label,
    day_label: gtk::Label,
    list: gtk::ListBox,
    stack: gtk::Stack,
    empty_hint: gtk::Label,
    /// 0 = today, -1 = yesterday, 1 = tomorrow…
    day: std::cell::Cell<i32>,
    target: std::cell::RefCell<Option<crate::window::GuideTarget>>,
    main: Weak<MainWindow>,
}

impl GuideDialog {
    pub fn present(main: &Rc<MainWindow>) {
        if let Some(d) = main.guide_dialog.borrow().clone() {
            if let Some(t) = main.guide_target() {
                d.show_channel(t);
            }
            d.window.present();
            return;
        }
        let Some(target) = main.guide_target() else {
            main.set_status(tr("Choose a channel from the list to see its schedule"));
            return;
        };

        let logo = gtk::Image::builder().pixel_size(40).build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title.add_css_class("title-3");
        title.add_css_class("channel-title");
        let header = gtk::Box::builder().spacing(12).build();
        header.append(&logo);
        header.append(&title);

        let prev = gtk::Button::from_icon_name("go-previous-symbolic");
        prev.set_tooltip_text(Some(tr("Previous day")));
        let next = gtk::Button::from_icon_name("go-next-symbolic");
        next.set_tooltip_text(Some(tr("Next day")));
        let today = gtk::Button::with_label(tr("Today"));
        let day_label = gtk::Label::builder().hexpand(true).build();
        day_label.add_css_class("channel-title");
        let nav = gtk::Box::builder().spacing(6).build();
        nav.append(&prev);
        nav.append(&day_label);
        nav.append(&today);
        nav.append(&next);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .min_content_height(380)
            .build();
        let empty_hint = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .vexpand(true)
            .build();
        empty_hint.add_css_class("dim-label");
        let stack = gtk::Stack::new();
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty_hint, Some("empty"));

        let update = gtk::Button::with_label(tr("Update guide"));
        let close = gtk::Button::with_label(tr("Close"));
        let footer = gtk::Box::builder().spacing(6).build();
        footer.append(&update);
        footer.append(&gtk::Box::builder().hexpand(true).build());
        footer.append(&close);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();
        content.append(&header);
        content.append(&nav);
        content.append(&stack);
        content.append(&footer);

        let window = gtk::Window::builder()
            .title(tr("TV guide"))
            .transient_for(&main.window)
            .hide_on_close(true)
            .default_width(520)
            .default_height(600)
            .child(&content)
            .build();

        let this = Rc::new(Self {
            window,
            logo,
            title,
            day_label,
            list,
            stack,
            empty_hint,
            day: std::cell::Cell::new(0),
            target: std::cell::RefCell::new(None),
            main: Rc::downgrade(main),
        });

        for (btn, delta) in [(&prev, -1), (&next, 1), (&today, 0)] {
            let w = Rc::downgrade(&this);
            btn.connect_clicked(move |_| {
                if let Some(d) = w.upgrade() {
                    d.day.set(if delta == 0 {
                        0
                    } else {
                        (d.day.get() + delta).clamp(-1, 7)
                    });
                    d.refresh();
                }
            });
        }
        let m = Rc::downgrade(main);
        update.connect_clicked(move |_| {
            if let Some(m) = m.upgrade() {
                m.refresh_epg(true);
            }
        });
        let w = this.window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.close();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let w = this.window.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape
                && let Some(w) = w.upgrade()
            {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        this.window.add_controller(keys);

        *main.guide_dialog.borrow_mut() = Some(this.clone());
        this.show_channel(target);
        this.window.present();
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn show_channel(&self, target: crate::window::GuideTarget) {
        self.title.set_label(&target.name);
        match &target.texture {
            Some(t) => self.logo.set_paintable(Some(t)),
            None => self
                .logo
                .set_icon_name(Some(crate::channel_list::placeholder_icon(target.kind))),
        }
        *self.target.borrow_mut() = Some(target);
        self.day.set(0);
        self.refresh();
    }

    pub fn refresh(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let Some(target) = self.target.borrow().clone() else {
            return;
        };

        let (from, to, label) = day_bounds(self.day.get());
        self.day_label.set_label(&label);
        self.list.remove_all();

        let progs = target
            .epg_key
            .as_deref()
            .map(|k| main.epg_range(k, from, to))
            .unwrap_or_default();
        if progs.is_empty() {
            self.empty_hint.set_label(if !main.has_epg_sources() {
                tr("There is no TV guide source.\n\nM3U lists usually declare one themselves (url-tvg). \
                    You can add one in TV-Maxe → Preferences → TV guide.")
            } else if target.epg_key.is_none() {
                tr("The channel does not appear in the downloaded TV guide.")
            } else {
                tr("There is no schedule for this day.")
            });
            self.stack.set_visible_child_name("empty");
            return;
        }
        self.stack.set_visible_child_name("list");

        let now = tvmaxe_core::net::unix_now();
        let mut current_row = None;
        for p in &progs {
            let time = gtk::Label::builder()
                .label(crate::window::hhmm(p.start))
                .valign(gtk::Align::Start)
                .width_chars(5)
                .build();
            time.add_css_class("numeric");
            time.add_css_class("channel-title");
            let title = gtk::Label::builder()
                .label(&p.title)
                .xalign(0.0)
                .wrap(true)
                .build();
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .hexpand(true)
                .build();
            text.append(&title);
            if let Some(d) = &p.desc {
                let desc = gtk::Label::builder()
                    .label(d)
                    .xalign(0.0)
                    .wrap(true)
                    .lines(3)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .tooltip_text(d)
                    .build();
                desc.add_css_class("dim-label");
                desc.add_css_class("caption");
                text.append(&desc);
            }
            let bx = gtk::Box::builder()
                .spacing(12)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(8)
                .margin_end(8)
                .build();
            bx.append(&time);
            bx.append(&text);
            if p.stop > now && !target.urls.is_empty() {
                let rec = gtk::Button::from_icon_name("media-record-symbolic");
                rec.set_tooltip_text(Some(tr("Record this programme…")));
                rec.set_valign(gtk::Align::Center);
                rec.add_css_class("flat");
                let (m, t, prog) = (self.main.clone(), target.clone(), p.clone());
                rec.connect_clicked(move |_| {
                    if let Some(m) = m.upgrade() {
                        m.schedule_programme(&t, &prog);
                    }
                });
                bx.append(&rec);
            }
            let row = gtk::ListBoxRow::builder().child(&bx).build();
            if p.start <= now && now < p.stop {
                row.add_css_class("epg-current");
                title.set_label(&fill(tr("%1  ·  now"), &[&p.title]));
                current_row = Some(row.clone());
            } else if p.stop <= now {
                row.add_css_class("epg-past");
            }
            self.list.append(&row);
        }
        // Scrolls the current programme into view once the list has a size.
        if let Some(row) = current_row {
            glib::idle_add_local_once(move || {
                row.grab_focus();
            });
        }
    }
}

/// Start and end of the local day `offset` (0 = today) and its label.
fn day_bounds(offset: i32) -> (i64, i64, String) {
    let now = glib::DateTime::now_local().expect("local time");
    let day = now.add_days(offset).unwrap_or(now);
    let start = glib::DateTime::from_local(day.year(), day.month(), day.day_of_month(), 0, 0, 0.0)
        .unwrap_or(day);
    let end = start.add_days(1).unwrap_or_else(|_| start.clone());
    let name = match offset {
        -1 => tr("Yesterday").to_string(),
        0 => tr("Today").to_string(),
        1 => tr("Tomorrow").to_string(),
        _ => start
            .format("%A")
            .map(|s| capitalize(&s))
            .unwrap_or_default(),
    };
    let date = start
        .format("%e %B")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    (start.to_unix(), end.to_unix(), format!("{name}, {date}"))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}
