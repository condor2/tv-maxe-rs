//! The Recordings window (like TV-Maxe's "Schedule manager") and the form for
//! scheduling a recording.

use crate::window::MainWindow;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::{Rc, Weak};
use tvmaxe_core::i18n::{fill, tr, trn};
use tvmaxe_core::net::unix_now;
use tvmaxe_core::recording::{Recording, Repeat, Status};

// ---------- Recordings window ----------

pub struct RecordingsDialog {
    window: gtk::Window,
    list: gtk::ListBox,
    folder: gtk::Label,
    main: Weak<MainWindow>,
}

impl RecordingsDialog {
    pub fn present(main: &Rc<MainWindow>) {
        if let Some(d) = main.recordings_dialog.borrow().clone() {
            d.refresh();
            d.window.present();
            return;
        }

        let folder = gtk::Label::builder().xalign(0.0).wrap(true).build();
        folder.add_css_class("dim-label");

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .min_content_height(260)
            .has_frame(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let schedule = gtk::Button::with_label(tr("Schedule…"));
        schedule.set_tooltip_text(Some(tr(
            "Schedule a recording of the channel that is playing or selected",
        )));
        let open = gtk::Button::with_label(tr("Open folder"));
        let close = gtk::Button::with_label(tr("Close"));
        let footer = gtk::Box::builder().spacing(6).build();
        footer.append(&schedule);
        footer.append(&open);
        footer.append(&gtk::Box::builder().hexpand(true).build());
        footer.append(&close);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&folder);
        content.append(&scrolled);
        content.append(&footer);

        let window = gtk::Window::builder()
            .title(tr("Recordings"))
            .transient_for(&main.window)
            // Hidden on close and reused, like Subscriptions.
            .hide_on_close(true)
            .default_width(640)
            .default_height(460)
            .child(&content)
            .build();

        let this = Rc::new(Self {
            window,
            list,
            folder,
            main: Rc::downgrade(main),
        });

        let m = Rc::downgrade(main);
        schedule.connect_clicked(move |_| {
            let Some(m) = m.upgrade() else { return };
            match m.channel_target() {
                Some(t) => {
                    let now = unix_now();
                    let start = now + 60 - now % 60;
                    let rec = tvmaxe_core::recording::Recording::new(
                        &t.name,
                        t.kind,
                        &t.urls,
                        &t.http,
                        start,
                        Some(start + 3600),
                    );
                    schedule_dialog(&m, rec);
                }
                None => m.set_status(tr("Choose a channel to record")),
            }
        });
        let m = Rc::downgrade(main);
        let w = this.window.downgrade();
        open.connect_clicked(move |_| {
            if let Some(m) = m.upgrade() {
                let dir = m.settings().recordings_dir();
                let _ = std::fs::create_dir_all(&dir);
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(dir)));
                launcher.launch(w.upgrade().as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });
        let w = this.window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.close();
            }
        });

        close_on_escape(&this.window);
        *main.recordings_dialog.borrow_mut() = Some(this.clone());
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
        self.folder.set_label(&fill(
            tr("Recordings are saved in %1. TV-Maxe must be running for scheduled recordings to start."),
            &[&main.settings().recordings_dir().display()],
        ));
        self.list.remove_all();
        let mut recs = main.db.borrow().recordings().unwrap_or_default();
        // Running first, then upcoming (soonest first), then the history (newest first).
        recs.sort_by_key(|r| match r.status {
            Status::Recording => (0, r.start),
            Status::Scheduled => (1, r.start),
            _ => (2, -r.start),
        });
        if recs.is_empty() {
            let empty = gtk::Label::builder()
                .label(tr(
                    "No recordings yet.\n\nRecord the channel you are watching with the ● button, \
                     or schedule one from the channel menu (right-click).",
                ))
                .justify(gtk::Justification::Center)
                .wrap(true)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(24)
                .margin_end(24)
                .build();
            empty.add_css_class("dim-label");
            self.list.append(&empty);
        }
        for r in recs {
            self.list.append(&self.row(&main, r));
        }
    }

    fn row(&self, main: &Rc<MainWindow>, r: Recording) -> gtk::Box {
        let icon = gtk::Image::from_icon_name(match r.status {
            Status::Recording => "media-record-symbolic",
            Status::Scheduled => "alarm-symbolic",
            Status::Done => "emblem-ok-symbolic",
            Status::Failed => "dialog-error-symbolic",
            Status::Missed => "dialog-warning-symbolic",
        });
        if r.status == Status::Recording {
            icon.add_css_class("recording-active");
        }
        icon.set_valign(gtk::Align::Center);

        let title = gtk::Label::builder()
            .label(r.display_title())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title.add_css_class("channel-title");
        let when = gtk::Label::builder()
            .label(describe_time(&r))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        when.add_css_class("dim-label");
        when.add_css_class("caption");
        let (status_text, is_error) = describe_status(&r);
        let status = gtk::Label::builder()
            .label(&status_text)
            .tooltip_text(&status_text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("caption");
        status.add_css_class(if is_error { "error" } else { "dim-label" });

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .spacing(2)
            .build();
        text.append(&title);
        text.append(&when);
        text.append(&status);

        let row = gtk::Box::builder()
            .spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(8)
            .build();
        row.append(&icon);
        row.append(&text);

        let button = |icon: &str, tip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.set_valign(gtk::Align::Center);
            b.add_css_class("flat");
            row.append(&b);
            b
        };
        let id = r.id;
        match r.status {
            Status::Recording => {
                let m = Rc::downgrade(main);
                button("media-playback-stop-symbolic", tr("Stop recording")).connect_clicked(
                    move |_| {
                        if let Some(m) = m.upgrade() {
                            m.stop_recording(id);
                        }
                    },
                );
            }
            Status::Scheduled => {
                let m = Rc::downgrade(main);
                let rec = r.clone();
                button("document-edit-symbolic", tr("Edit")).connect_clicked(move |_| {
                    if let Some(m) = m.upgrade() {
                        schedule_dialog(&m, rec.clone());
                    }
                });
                let m = Rc::downgrade(main);
                button("user-trash-symbolic", tr("Cancel recording")).connect_clicked(move |_| {
                    if let Some(m) = m.upgrade() {
                        m.stop_recording(id);
                    }
                });
            }
            _ => {
                if let Some(first) = r.files.first().cloned() {
                    let m = Rc::downgrade(main);
                    let (title, files) = (r.display_title().to_string(), r.files.clone());
                    button("media-playback-start-symbolic", tr("Play")).connect_clicked(
                        move |_| {
                            if let Some(m) = m.upgrade() {
                                m.play_recording(&title, &files);
                            }
                        },
                    );
                    let w = self.window.downgrade();
                    button("folder-open-symbolic", tr("Show in folder")).connect_clicked(
                        move |_| {
                            let launcher =
                                gtk::FileLauncher::new(Some(&gio::File::for_path(&first)));
                            launcher.open_containing_folder(
                                w.upgrade().as_ref(),
                                gio::Cancellable::NONE,
                                |_| {},
                            );
                        },
                    );
                }
                let m = Rc::downgrade(main);
                button(
                    "edit-clear-symbolic",
                    tr("Remove from the list (the files are kept)"),
                )
                .connect_clicked(move |_| {
                    if let Some(m) = m.upgrade() {
                        let _ = m.db.borrow().delete_recording(id);
                        if let Some(d) = m.recordings_dialog.borrow().clone() {
                            d.refresh();
                        }
                    }
                });
            }
        }
        row
    }
}

/// "TVR 1 · Wed 14 Oct, 20:30–21:00 · Every day".
fn describe_time(r: &Recording) -> String {
    let day = glib::DateTime::from_unix_local(r.start)
        .and_then(|d| d.format("%a %e %b"))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let time = match r.stop {
        Some(stop) => format!(
            "{}–{}",
            crate::window::hhmm(r.start),
            crate::window::hhmm(stop)
        ),
        None => fill(tr("from %1"), &[&crate::window::hhmm(r.start)]),
    };
    let mut parts = vec![r.channel.clone(), format!("{day}, {time}")];
    if r.repeat != Repeat::Once {
        parts.push(repeat_label(r.repeat).to_string());
    }
    parts.join(" · ")
}

/// Status line and whether it is an error.
fn describe_status(r: &Recording) -> (String, bool) {
    let size = || {
        let bytes: u64 = r
            .files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    };
    match r.status {
        Status::Recording => (fill(tr("Recording… %1"), &[&size()]), false),
        Status::Scheduled => (tr("Scheduled").to_string(), false),
        Status::Done => {
            let mut s = fill(
                tr("Done · %1 · %2"),
                &[&trn("%n file", "%n files", r.files.len() as u64), &size()],
            );
            if let Some(e) = &r.error {
                s = format!("{s} · {e}");
            }
            (s, false)
        }
        Status::Failed => (
            fill(
                tr("Failed: %1"),
                &[&r.error.as_deref().unwrap_or(tr("Nothing was recorded"))],
            ),
            true,
        ),
        Status::Missed => (tr("Missed: TV-Maxe was not running").to_string(), true),
    }
}

fn repeat_label(r: Repeat) -> &'static str {
    match r {
        Repeat::Once => tr("Once"),
        Repeat::Daily => tr("Every day"),
        Repeat::Weekly => tr("Every week"),
    }
}

// ---------- Schedule form ----------

/// The form for a new (`rec.id == 0`) or a scheduled recording.
pub fn schedule_dialog(main: &Rc<MainWindow>, rec: Recording) {
    let editing = rec.id != 0;
    let start = glib::DateTime::from_unix_local(rec.start)
        .unwrap_or_else(|_| glib::DateTime::now_local().unwrap());
    let stop = glib::DateTime::from_unix_local(rec.stop.unwrap_or(rec.start + 3600))
        .unwrap_or_else(|_| start.clone());

    let channel = gtk::Label::builder()
        .label(&rec.channel)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    channel.add_css_class("channel-title");
    let title = gtk::Entry::builder()
        .text(&rec.title)
        .placeholder_text(&rec.channel)
        .hexpand(true)
        .activates_default(true)
        .build();

    let calendar = gtk::Calendar::new();
    calendar.select_day(&start);
    let date_label = gtk::Label::new(None);
    let date_btn = gtk::MenuButton::builder()
        .child(&date_label)
        .popover(&gtk::Popover::builder().child(&calendar).build())
        .halign(gtk::Align::Start)
        .build();

    let (start_box, start_h, start_m) = time_spins(start.hour(), start.minute());
    let (stop_box, stop_h, stop_m) = time_spins(stop.hour(), stop.minute());

    let labels: Vec<&str> = Repeat::ALL.iter().map(|r| repeat_label(*r)).collect();
    let repeat = gtk::DropDown::from_strings(&labels);
    repeat.set_selected(
        Repeat::ALL
            .iter()
            .position(|r| *r == rec.repeat)
            .unwrap_or(0) as u32,
    );
    repeat.set_halign(gtk::Align::Start);

    let hint = gtk::Label::builder().xalign(0.0).wrap(true).build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .build();
    for (i, (label, w)) in [
        (tr("Channel:"), channel.upcast_ref::<gtk::Widget>()),
        (tr("Title:"), title.upcast_ref()),
        (tr("Date:"), date_btn.upcast_ref()),
        (tr("Start:"), start_box.upcast_ref()),
        (tr("End:"), stop_box.upcast_ref()),
        (tr("Repeat:"), repeat.upcast_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        grid.attach(
            &gtk::Label::builder().label(label).xalign(1.0).build(),
            0,
            i as i32,
            1,
            1,
        );
        grid.attach(w, 1, i as i32, 1, 1);
    }

    let cancel = gtk::Button::with_label(tr("Cancel"));
    let save = gtk::Button::with_label(if editing { tr("Save") } else { tr("Schedule") });
    save.add_css_class("suggested-action");
    let buttons = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&cancel);
    buttons.append(&save);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&grid);
    content.append(&hint);
    content.append(&buttons);

    let win = gtk::Window::builder()
        .title(if editing {
            tr("Edit recording")
        } else {
            tr("Schedule recording")
        })
        .transient_for(&main.window)
        .modal(true)
        .resizable(false)
        .default_width(380)
        .child(&content)
        .default_widget(&save)
        .build();

    // Start and stop (Unix) from the form; an end before the start is the next day.
    let times = {
        let (calendar, start_h, start_m, stop_h, stop_m) = (
            calendar.clone(),
            start_h.clone(),
            start_m.clone(),
            stop_h.clone(),
            stop_m.clone(),
        );
        move || -> Option<(i64, i64)> {
            let d = calendar.date();
            let at = |h: &gtk::SpinButton, m: &gtk::SpinButton| {
                glib::DateTime::from_local(
                    d.year(),
                    d.month(),
                    d.day_of_month(),
                    h.value_as_int(),
                    m.value_as_int(),
                    0.0,
                )
                .ok()
                .map(|t| t.to_unix())
            };
            let start = at(&start_h, &start_m)?;
            let mut stop = at(&stop_h, &stop_m)?;
            if stop <= start {
                stop += 24 * 3600;
            }
            Some((start, stop))
        }
    };

    let validate = {
        let (times, hint, save, date_label, calendar) = (
            times.clone(),
            hint.clone(),
            save.clone(),
            date_label.clone(),
            calendar.clone(),
        );
        move || {
            date_label.set_label(
                &calendar
                    .date()
                    .format("%A, %e %B %Y")
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            );
            let now = unix_now();
            let (ok, text) = match times() {
                None => (false, String::new()),
                Some((_, stop)) if stop <= now => {
                    (false, tr("This time has already passed.").to_string())
                }
                Some((start, stop)) => {
                    let mins = (stop - start) / 60;
                    let mut t = fill(
                        tr("Duration: %1"),
                        &[&format!("{}:{:02}", mins / 60, mins % 60)],
                    );
                    let next_day = glib::DateTime::from_unix_local(start)
                        .ok()
                        .zip(glib::DateTime::from_unix_local(stop).ok())
                        .is_some_and(|(a, b)| a.day_of_month() != b.day_of_month());
                    if next_day {
                        t = format!("{t} · {}", tr("ends the next day"));
                    }
                    if start <= now {
                        t = format!("{t} · {}", tr("starts right away"));
                    }
                    (true, t)
                }
            };
            save.set_sensitive(ok);
            hint.set_label(&text);
        }
    };
    validate();
    for spin in [&start_h, &start_m, &stop_h, &stop_m] {
        let v = validate.clone();
        spin.connect_value_changed(move |_| v());
    }
    {
        let v = validate.clone();
        let pop = date_btn.popover();
        calendar.connect_day_selected(move |_| {
            v();
            if let Some(p) = &pop {
                p.popdown();
            }
        });
    }

    close_on_escape(&win);
    let w = win.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let main_w = Rc::downgrade(main);
    let w = win.downgrade();
    save.connect_clicked(move |_| {
        let (Some(main), Some(w), Some((start, stop))) = (main_w.upgrade(), w.upgrade(), times())
        else {
            return;
        };
        let mut r = rec.clone();
        r.title = title.text().trim().to_string();
        r.start = start;
        r.stop = Some(stop);
        r.repeat = Repeat::ALL[repeat.selected() as usize];
        r.status = Status::Scheduled;
        let saved = if editing {
            main.db.borrow().update_recording(&r).map(|_| ())
        } else {
            main.db.borrow().add_recording(&r).map(|_| ())
        };
        match saved {
            Ok(()) => main.set_status(&fill(
                tr("Recording scheduled: %1, %2"),
                &[&r.display_title(), &describe_time(&r)],
            )),
            Err(e) => main.set_status(&fill(tr("Cannot save the recording: %1"), &[&e])),
        }
        main.recordings_tick();
        if let Some(d) = main.recordings_dialog.borrow().clone() {
            d.refresh();
        }
        w.close();
    });
    win.present();
}

fn close_on_escape(win: &gtk::Window) {
    let keys = gtk::EventControllerKey::new();
    let w = win.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape
            && let Some(w) = w.upgrade()
        {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(keys);
}

/// HH : MM spin buttons.
fn time_spins(h: i32, m: i32) -> (gtk::Box, gtk::SpinButton, gtk::SpinButton) {
    let spin = |max: f64, v: i32| {
        let s = gtk::SpinButton::with_range(0.0, max, 1.0);
        s.set_value(f64::from(v));
        s.set_wrap(true);
        s.set_orientation(gtk::Orientation::Vertical);
        // Two digits, like a clock.
        s.connect_output(|s| {
            s.set_text(&format!("{:02}", s.value_as_int()));
            glib::Propagation::Stop
        });
        s
    };
    let hours = spin(23.0, h);
    let minutes = spin(59.0, m);
    let bx = gtk::Box::builder()
        .spacing(4)
        .halign(gtk::Align::Start)
        .build();
    bx.append(&hours);
    bx.append(&gtk::Label::new(Some(":")));
    bx.append(&minutes);
    (bx, hours, minutes)
}
