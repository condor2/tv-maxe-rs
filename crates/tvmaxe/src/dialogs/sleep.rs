//! The Sleep timer window (like TV-Maxe's "Sleep…").

use crate::sleep::{self, SleepAction};
use crate::window::MainWindow;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use std::time::Duration;
use tvmaxe_core::i18n::{fill, tr};

pub fn sleep_timer(main: &Rc<MainWindow>) {
    let db = main.db.borrow();
    let last_minutes: u32 = db
        .setting::<u32>("sleep_minutes")
        .unwrap_or(30)
        .clamp(sleep::MINUTES.0, sleep::MINUTES.1);
    let last_action = db
        .setting::<String>("sleep_action")
        .and_then(|s| SleepAction::parse(&s))
        .unwrap_or_default();
    drop(db);

    // Minutes, with one-click choices.
    let minutes = gtk::SpinButton::with_range(
        f64::from(sleep::MINUTES.0),
        f64::from(sleep::MINUTES.1),
        5.0,
    );
    minutes.set_value(f64::from(last_minutes));
    minutes.set_valign(gtk::Align::Center);
    minutes.set_activates_default(true);
    let after = gtk::Box::builder().spacing(6).build();
    after.append(&minutes);
    after.append(&gtk::Label::new(Some(tr("minutes"))));

    let presets = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(3)
        .max_children_per_line(6)
        .column_spacing(2)
        .row_spacing(2)
        .homogeneous(true)
        .build();
    for m in sleep::PRESETS {
        let b = gtk::Button::with_label(&fill(tr("%1 min"), &[&m]));
        b.add_css_class("flat");
        let spin = minutes.clone();
        b.connect_clicked(move |_| spin.set_value(f64::from(m)));
        presets.insert(&b, -1);
    }

    let labels: Vec<&str> = SleepAction::ALL.iter().map(|a| a.label()).collect();
    let action = gtk::DropDown::from_strings(&labels);
    action.set_selected(
        SleepAction::ALL
            .iter()
            .position(|a| *a == last_action)
            .unwrap_or(0) as u32,
    );
    action.set_halign(gtk::Align::Start);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(46)
        .build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    let running = gtk::Label::builder().xalign(0.0).build();
    running.add_css_class("channel-title");

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .build();
    let label = |t: &str| gtk::Label::builder().label(t).xalign(1.0).build();
    grid.attach(&label(tr("After:")), 0, 0, 1, 1);
    grid.attach(&after, 1, 0, 1, 1);
    grid.attach(&presets, 1, 1, 1, 1);
    grid.attach(&label(tr("Then:")), 0, 2, 1, 1);
    grid.attach(&action, 1, 2, 1, 1);

    let cancel_timer = gtk::Button::with_label(tr("Cancel timer"));
    cancel_timer.add_css_class("destructive-action");
    let close = gtk::Button::with_label(tr("Close"));
    let start = gtk::Button::with_label(tr("Start"));
    start.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(6).build();
    buttons.append(&cancel_timer);
    buttons.append(&gtk::Box::builder().hexpand(true).build());
    buttons.append(&close);
    buttons.append(&start);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(12)
        .margin_start(14)
        .margin_end(14)
        .build();
    content.append(&running);
    content.append(&grid);
    content.append(&hint);
    content.append(&buttons);

    let win = gtk::Window::builder()
        .title(tr("Sleep timer"))
        .transient_for(&main.window)
        .modal(true)
        .resizable(false)
        .default_width(420)
        .child(&content)
        .default_widget(&start)
        .build();

    // The running timer, refreshed every second while the window is open.
    let show_running = {
        let (m, running, cancel_timer, start) = (
            Rc::downgrade(main),
            running.clone(),
            cancel_timer.clone(),
            start.clone(),
        );
        move || {
            let Some(m) = m.upgrade() else { return };
            match (m.sleep_remaining(), m.sleep_action()) {
                (Some(left), Some(a)) => {
                    running.set_label(&fill(
                        tr("%1 in %2"),
                        &[&a.label(), &sleep::format_remaining(left)],
                    ));
                    running.set_visible(true);
                    cancel_timer.set_visible(true);
                    start.set_label(tr("Restart"));
                }
                _ => {
                    running.set_visible(false);
                    cancel_timer.set_visible(false);
                    start.set_label(tr("Start"));
                }
            }
        }
    };
    show_running();
    {
        let (show, w) = (show_running.clone(), win.downgrade());
        glib::timeout_add_local(Duration::from_secs(1), move || {
            if w.upgrade().is_none_or(|w| !w.is_visible()) {
                return glib::ControlFlow::Break;
            }
            show();
            glib::ControlFlow::Continue
        });
    }

    // Suspend / shut down: only when the system allows it.
    let check = {
        let (action, hint, start) = (action.clone(), hint.clone(), start.clone());
        move || {
            let a = SleepAction::ALL[action.selected() as usize];
            let (hint, start) = (hint.clone(), start.clone());
            hint.set_label(match a {
                SleepAction::StopPlayback => tr("The volume goes down during the last 30 seconds."),
                _ => tr(
                    "The volume goes down during the last 30 seconds. If a recording is running, \
                     TV-Maxe only stops playback, so the recording is not lost.",
                ),
            });
            start.set_sensitive(true);
            glib::spawn_future_local(async move {
                if !crate::window::sleep_action_available(a).await {
                    start.set_sensitive(false);
                    hint.set_label(tr(
                        "This system does not allow it (or logind is not running).",
                    ));
                }
            });
        }
    };
    check();
    action.connect_selected_notify(move |_| check());

    let m = Rc::downgrade(main);
    let show = show_running.clone();
    cancel_timer.connect_clicked(move |_| {
        if let Some(m) = m.upgrade() {
            m.cancel_sleep_timer();
        }
        show();
    });
    let w = win.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let m = Rc::downgrade(main);
    let w = win.downgrade();
    start.connect_clicked(move |_| {
        let (Some(m), Some(w)) = (m.upgrade(), w.upgrade()) else {
            return;
        };
        let mins = minutes.value_as_int().max(1) as u32;
        let a = SleepAction::ALL[action.selected() as usize];
        let db = m.db.borrow();
        let _ = db.set_setting("sleep_minutes", mins);
        let _ = db.set_setting("sleep_action", a.as_str());
        drop(db);
        m.start_sleep_timer(Duration::from_secs(u64::from(mins) * 60), a);
        w.close();
    });

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
    win.present();
}
