//! Adding channels, exporting the local list and the About dialog.

use crate::window::MainWindow;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use tvmaxe_core::i18n::{fill, tr};
use tvmaxe_core::{Kind, StoredChannel};

// ---------- Add / edit channel ----------

pub fn add_channel(main: &Rc<MainWindow>) {
    channel_dialog(main, None);
}

/// Only for "My channels"; subscription channels are rewritten on update.
pub fn edit_channel(main: &Rc<MainWindow>, channel: &StoredChannel) {
    if channel.is_local {
        channel_dialog(main, Some(channel));
    }
}

/// The same form for adding (`existing = None`) and editing.
fn channel_dialog(main: &Rc<MainWindow>, existing: Option<&StoredChannel>) {
    let edit_id = existing.map(|c| c.id);
    let name = gtk::Entry::builder()
        .hexpand(true)
        .activates_default(true)
        .build();
    let urls = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(gtk::WrapMode::Char)
        .accepts_tab(false)
        .build();
    urls.set_tooltip_text(Some(tr("One URL per line, in order of preference")));
    let url_buffer = urls.buffer();
    let url_scroll = gtk::ScrolledWindow::builder()
        .child(&urls)
        .hexpand(true)
        .min_content_height(90)
        .has_frame(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let kind = gtk::DropDown::from_strings(&[tr("TV channel"), tr("Radio station")]);
    match existing {
        Some(c) => {
            name.set_text(&c.name);
            url_buffer.set_text(&c.urls.join("\n"));
            kind.set_selected(u32::from(c.kind == Kind::Radio));
        }
        None => kind.set_selected(u32::from(main.current_kind() == Kind::Radio)),
    }

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .build();
    for (i, (label, w)) in [
        (tr("Name:"), name.upcast_ref::<gtk::Widget>()),
        (tr("URLs:"), url_scroll.upcast_ref()),
        (tr("Type:"), kind.upcast_ref()),
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
    let add = gtk::Button::with_label(if edit_id.is_some() {
        tr("Save")
    } else {
        tr("Add")
    });
    add.add_css_class("suggested-action");
    add.set_sensitive(existing.is_some());
    let buttons = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&cancel);
    buttons.append(&add);

    let hint = gtk::Label::builder()
        .label(if edit_id.is_some() {
            tr("The first URL is the main one; the others are fallbacks, tried in order.")
        } else {
            tr(
                "The channel will appear in “My channels”. The first URL is the main one; the others are fallbacks.",
            )
        })
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("dim-label");

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
        .title(if edit_id.is_some() {
            tr("Edit channel")
        } else {
            tr("Add channel")
        })
        .transient_for(&main.window)
        .modal(true)
        .default_width(440)
        .resizable(false)
        .child(&content)
        .default_widget(&add)
        .build();

    let validate = {
        let (name, buffer, add) = (name.clone(), url_buffer.clone(), add.clone());
        move || {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            add.set_sensitive(
                !name.text().trim().is_empty() && !parse_url_lines(text.as_str()).is_empty(),
            )
        }
    };
    let v = validate.clone();
    name.connect_changed(move |_| v());
    url_buffer.connect_changed(move |_| validate());

    let w = win.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let main_w = Rc::downgrade(main);
    let w = win.downgrade();
    add.connect_clicked(move |_| {
        let (Some(main), Some(w)) = (main_w.upgrade(), w.upgrade()) else {
            return;
        };
        let k = if kind.selected() == 1 {
            Kind::Radio
        } else {
            Kind::Tv
        };
        let n = name.text().trim().to_string();
        let text = url_buffer.text(&url_buffer.start_iter(), &url_buffer.end_iter(), false);
        let urls = parse_url_lines(text.as_str());
        let result = match edit_id {
            Some(id) => main
                .db
                .borrow()
                .update_local_channel(id, k, &n, &urls)
                .map(|_| ()),
            None => main
                .db
                .borrow()
                .add_local_channel_urls(k, &n, &urls)
                .map(|_| ()),
        };
        match (result, edit_id) {
            (Ok(()), Some(id)) => {
                main.set_status(&fill(tr("“%1” was saved"), &[&n]));
                main.channel_edited(id, &n, k, &urls);
            }
            (Ok(()), None) => main.set_status(&fill(tr("“%1” was added to “My channels”"), &[&n])),
            (Err(e), Some(_)) => main.set_status(&fill(tr("Cannot save the channel: %1"), &[&e])),
            (Err(e), None) => main.set_status(&fill(tr("Cannot add the channel: %1"), &[&e])),
        }
        main.reload();
        w.close();
    });
    win.present();
}

fn parse_url_lines(text: &str) -> Vec<String> {
    let mut urls = Vec::<String>::new();
    for url in text.lines().map(str::trim).filter(|url| !url.is_empty()) {
        if !urls.iter().any(|existing| existing.as_str() == url) {
            urls.push(url.to_string());
        }
    }
    urls
}

// ---------- Local list export ----------

pub fn export_local(main: &Rc<MainWindow>) {
    let list = {
        let db = main.db.borrow();
        db.local_list_id().and_then(|id| db.export_list(id))
    };
    let list = match list {
        Ok(l) if !l.channels.is_empty() => l,
        Ok(_) => return main.set_status(tr("“My channels” is empty — add a channel first")),
        Err(e) => return main.set_status(&fill(tr("Cannot read the local list: %1"), &[&e])),
    };
    let main = main.clone();
    glib::spawn_future_local(async move {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save local list"))
            .initial_name(tr("my-channels.db"))
            .build();
        let Ok(file) = dialog.save_future(Some(&main.window)).await else {
            return;
        };
        let Some(path) = file.path() else { return };
        match tvmaxe_core::formats::write_tvmaxe_db(&path, &list) {
            Ok(()) => main.set_status(&fill(tr("The list was saved to %1"), &[&path.display()])),
            Err(e) => main.set_status(&fill(tr("Cannot save the list: %1"), &[&e])),
        }
    });
}

// ---------- About ----------

pub fn about(parent: &gtk::ApplicationWindow) {
    let about = gtk::AboutDialog::builder()
        .transient_for(parent)
        .modal(true)
        .program_name("TV-Maxe")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(tr(
            "Internet television and radio for Linux.\nA Rust rebuild of the TV-Maxe application.",
        ))
        .license_type(gtk::License::Gpl30)
        .logo_icon_name("video-display")
        .build();
    about.add_credit_section(tr("Original TV-Maxe"), &["Ovidiu Nițan"]);
    about.present();
}

#[cfg(test)]
mod tests {
    use super::parse_url_lines;

    #[test]
    fn url_lines_are_trimmed_and_deduplicated() {
        assert_eq!(
            parse_url_lines(
                " https://primary/stream \n\nhttps://backup/stream\nhttps://primary/stream"
            ),
            ["https://primary/stream", "https://backup/stream"]
        );
    }
}
