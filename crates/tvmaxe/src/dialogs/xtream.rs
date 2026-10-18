//! Adding an Xtream Codes account (server, user name and password).

use crate::window::MainWindow;
use gtk::prelude::*;
use std::rc::Rc;
use tvmaxe_core::formats::xtream::Account;
use tvmaxe_core::i18n::tr;

/// Opens the form over `parent`. On *Add* the account becomes a subscription.
pub fn xtream_account(main: &Rc<MainWindow>, parent: &gtk::Window) {
    let server = gtk::Entry::builder()
        .hexpand(true)
        .placeholder_text("http://server.example:8080")
        .input_purpose(gtk::InputPurpose::Url)
        .activates_default(true)
        .build();
    let username = gtk::Entry::builder()
        .hexpand(true)
        .activates_default(true)
        .build();
    let password = gtk::PasswordEntry::builder()
        .hexpand(true)
        .show_peek_icon(true)
        .activates_default(true)
        .build();

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .build();
    for (i, (label, w)) in [
        (tr("Server:"), server.upcast_ref::<gtk::Widget>()),
        (tr("User name:"), username.upcast_ref()),
        (tr("Password:"), password.upcast_ref()),
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

    let hint = gtk::Label::builder()
        .label(tr(
            "Enter the address, user name and password your provider gave you. \
             Only live channels are added. Only use accounts you have the right to use.",
        ))
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(48)
        .build();
    hint.add_css_class("dim-label");

    let cancel = gtk::Button::with_label(tr("Cancel"));
    let add = gtk::Button::with_label(tr("Add"));
    add.add_css_class("suggested-action");
    add.set_sensitive(false);
    let buttons = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&cancel);
    buttons.append(&add);

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
        .title(tr("Xtream Codes account"))
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .resizable(false)
        .child(&content)
        .default_widget(&add)
        .build();

    let account = {
        let (server, username, password) = (server.clone(), username.clone(), password.clone());
        move || Account::new(&server.text(), &username.text(), &password.text())
    };
    let validate = {
        let (account, add) = (account.clone(), add.clone());
        move || add.set_sensitive(account().is_some())
    };
    let v = validate.clone();
    server.connect_changed(move |_| v());
    let v = validate.clone();
    username.connect_changed(move |_| v());
    password.connect_changed(move |_| validate());

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
        if let Some(account) = account() {
            main.add_xtream(&account);
        }
        w.close();
    });
    win.present();
    server.grab_focus();
}
