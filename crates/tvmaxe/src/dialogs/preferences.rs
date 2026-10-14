//! Application preferences.

use crate::window::MainWindow;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use tvmaxe_core::i18n::tr;

// ---------- Preferences ----------

pub fn preferences(main: &Rc<MainWindow>) {
    use crate::player::VideoQuality;
    use crate::settings::{CACHE_SECS, Settings, TIMEOUT_SECS};

    let s = main.settings();

    // Every control saves and applies immediately (no "Apply" button).
    let update = {
        let main = Rc::downgrade(main);
        move |f: &dyn Fn(&mut Settings)| {
            if let Some(main) = main.upgrade() {
                let mut s = main.settings();
                f(&mut s);
                main.set_settings(s);
            }
        }
    };

    // --- Playback ---
    let hwdec = gtk::Switch::builder()
        .active(s.playback.hardware_decoding)
        .valign(gtk::Align::Center)
        .build();
    let u = update.clone();
    hwdec.connect_active_notify(move |w| u(&|s| s.playback.hardware_decoding = w.is_active()));

    let (cache_row, cache) = seconds_spin(CACHE_SECS, s.playback.cache_secs);
    let u = update.clone();
    cache.connect_value_changed(move |w| u(&|s| s.playback.cache_secs = w.value_as_int() as u32));

    let (timeout_row, timeout) = seconds_spin(TIMEOUT_SECS, s.playback.network_timeout_secs);
    let u = update.clone();
    timeout.connect_value_changed(move |w| {
        u(&|s| s.playback.network_timeout_secs = w.value_as_int() as u32)
    });

    let labels: Vec<&str> = VideoQuality::ALL.iter().map(|q| q.label()).collect();
    let quality = gtk::DropDown::from_strings(&labels);
    quality.set_valign(gtk::Align::Center);
    quality.set_selected(
        VideoQuality::ALL
            .iter()
            .position(|q| *q == s.playback.video_quality)
            .unwrap_or(0) as u32,
    );
    let u = update.clone();
    quality.connect_selected_notify(move |w| {
        let q = VideoQuality::ALL[w.selected() as usize];
        u(&|s| s.playback.video_quality = q)
    });

    let ua = gtk::Entry::builder()
        .text(s.playback.user_agent.as_deref().unwrap_or(""))
        .placeholder_text(tvmaxe_core::net::DEFAULT_USER_AGENT)
        .width_chars(28)
        .valign(gtk::Align::Center)
        .build();
    let u = update.clone();
    ua.connect_changed(move |w| {
        let t = w.text().trim().to_string();
        u(&|s| s.playback.user_agent = (!t.is_empty()).then(|| t.clone()))
    });

    // --- Lists and startup ---
    let refresh = gtk::Switch::builder()
        .active(s.refresh_on_start)
        .valign(gtk::Align::Center)
        .build();
    let u = update.clone();
    refresh.connect_active_notify(move |w| u(&|s| s.refresh_on_start = w.is_active()));

    let logos = gtk::Switch::builder()
        .active(s.download_logos)
        .valign(gtk::Align::Center)
        .build();
    let u = update.clone();
    logos.connect_active_notify(move |w| u(&|s| s.download_logos = w.is_active()));

    let autoplay = gtk::Switch::builder()
        .active(s.autoplay_last)
        .valign(gtk::Align::Center)
        .build();
    let u = update.clone();
    autoplay.connect_active_notify(move |w| u(&|s| s.autoplay_last = w.is_active()));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(14)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&section(
        tr("Playback"),
        &[
            (
                tr("Hardware decoding"),
                Some(tr(
                    "Uses the graphics card. Turn it off if the picture shows artefacts.",
                )),
                hwdec.upcast_ref(),
            ),
            (
                tr("Network buffer"),
                Some(tr("Larger = fewer interruptions, slightly slower start.")),
                cache_row.upcast_ref(),
            ),
            (
                tr("Connection timeout"),
                Some(tr("After this time the channel's next URL is tried.")),
                timeout_row.upcast_ref(),
            ),
            (
                tr("Video quality"),
                Some(tr(
                    "Switching back from “Fast” takes effect after a restart.",
                )),
                quality.upcast_ref(),
            ),
            (
                "User-Agent",
                Some(tr(
                    "Empty = default. Some IPTV lists require a specific value.",
                )),
                ua.upcast_ref(),
            ),
        ],
    ));
    content.append(&section(
        tr("Channel lists"),
        &[
            (
                tr("Update the lists at startup"),
                None,
                refresh.upcast_ref(),
            ),
            (tr("Download channel logos"), None, logos.upcast_ref()),
        ],
    ));
    let epg_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(6)
        .bottom_margin(6)
        .left_margin(6)
        .right_margin(6)
        .build();
    epg_view.buffer().set_text(&s.extra_epg_urls.join("\n"));
    let u = update.clone();
    epg_view.buffer().connect_changed(move |b| {
        let urls = crate::settings::parse_url_lines(&b.text(&b.start_iter(), &b.end_iter(), false));
        u(&|s| s.extra_epg_urls = urls.clone())
    });
    let epg_scroll = gtk::ScrolledWindow::builder()
        .child(&epg_view)
        .min_content_height(64)
        .has_frame(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let epg_hint = gtk::Label::builder()
        .label(tr(
            "XMLTV addresses (.xml or .xml.gz), one per line. M3U lists usually declare \
             their own guide (url-tvg), so this is only needed for extra sources.",
        ))
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(60)
        .build();
    epg_hint.add_css_class("dim-label");
    epg_hint.add_css_class("caption");
    let epg_heading = gtk::Label::builder()
        .label(tr("TV guide"))
        .xalign(0.0)
        .build();
    epg_heading.add_css_class("channel-title");
    let epg_section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    let epg_inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .build();
    epg_inner.append(
        &gtk::Label::builder()
            .label(tr("Extra EPG sources"))
            .xalign(0.0)
            .build(),
    );
    epg_inner.append(&epg_hint);
    epg_inner.append(&epg_scroll);
    epg_section.append(&epg_heading);
    epg_section.append(&epg_inner);
    content.append(&epg_section);

    content.append(&section(
        tr("Startup"),
        &[(
            tr("Play the last channel on startup"),
            None,
            autoplay.upcast_ref(),
        )],
    ));

    let defaults = gtk::Button::with_label(tr("Defaults"));
    let close = gtk::Button::with_label(tr("Close"));
    let spacer = gtk::Box::builder().hexpand(true).build();
    let footer = gtk::Box::builder().spacing(6).build();
    footer.append(&defaults);
    footer.append(&spacer);
    footer.append(&close);
    content.append(&footer);

    let win = gtk::Window::builder()
        .title(tr("Preferences"))
        .transient_for(&main.window)
        .modal(true)
        .resizable(false)
        .child(&content)
        .build();

    // Resetting moves the controls; their handlers save the default values.
    let d = Settings::default();
    defaults.connect_clicked(move |_| {
        hwdec.set_active(d.playback.hardware_decoding);
        cache.set_value(f64::from(d.playback.cache_secs));
        timeout.set_value(f64::from(d.playback.network_timeout_secs));
        quality.set_selected(0);
        ua.set_text("");
        epg_view.buffer().set_text("");
        refresh.set_active(d.refresh_on_start);
        logos.set_active(d.download_logos);
        autoplay.set_active(d.autoplay_last);
    });
    let w = win.downgrade();
    close.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
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
    let initial_epg = s.extra_epg_urls.clone();
    let m = Rc::downgrade(main);
    win.connect_close_request(move |_| {
        if let Some(m) = m.upgrade()
            && m.settings().extra_epg_urls != initial_epg
        {
            m.refresh_epg(false);
        }
        glib::Propagation::Proceed
    });
    win.present();
}

fn seconds_spin((min, max): (u32, u32), value: u32) -> (gtk::Box, gtk::SpinButton) {
    let spin = gtk::SpinButton::with_range(f64::from(min), f64::from(max), 1.0);
    spin.set_value(f64::from(value));
    spin.set_valign(gtk::Align::Center);
    let bx = gtk::Box::builder()
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    bx.append(&spin);
    bx.append(&gtk::Label::new(Some(tr("seconds"))));
    (bx, spin)
}

/// Section title + rows "label (with explanation) — control".
fn section(title: &str, rows: &[(&str, Option<&str>, &gtk::Widget)]) -> gtk::Box {
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("channel-title");
    let grid = gtk::Grid::builder()
        .row_spacing(12)
        .column_spacing(24)
        .margin_start(12)
        .build();
    for (i, (label, hint, widget)) in rows.iter().enumerate() {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(&gtk::Label::builder().label(*label).xalign(0.0).build());
        if let Some(h) = hint {
            let hint = gtk::Label::builder()
                .label(*h)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(44)
                .build();
            hint.add_css_class("dim-label");
            hint.add_css_class("caption");
            text.append(&hint);
        }
        grid.attach(&text, 0, i as i32, 1, 1);
        widget.set_halign(gtk::Align::End);
        grid.attach(*widget, 1, i as i32, 1, 1);
    }
    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    bx.append(&heading);
    bx.append(&grid);
    bx
}
