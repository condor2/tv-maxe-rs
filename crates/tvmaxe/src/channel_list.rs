//! A tab on the left ("TV channels" / "Radio stations"), like in TV-Maxe:
//! "Channel list:" + subscription picker, search, list with logo and name.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tvmaxe_core::i18n::tr;
use tvmaxe_core::{Kind, StoredChannel, Subscription};

pub const LOGO_SIZE: i32 = 24;

/// A row of the list. The logo is decoded lazily, on first display.
pub struct ChannelRow {
    pub channel: StoredChannel,
    pub subscription: String,
    /// Marked as a favourite (★).
    pub favourite: Cell<bool>,
    texture: RefCell<Option<gdk::Texture>>,
}

impl ChannelRow {
    pub fn new(channel: StoredChannel, subscription: String) -> Self {
        Self {
            channel,
            subscription,
            favourite: Cell::new(false),
            texture: RefCell::default(),
        }
    }

    pub fn set_texture(&self, t: gdk::Texture) {
        *self.texture.borrow_mut() = Some(t);
    }

    /// Does the logo need downloading (not in the database and not failed before)?
    pub fn needs_logo(&self) -> Option<&str> {
        let c = &self.channel;
        (c.logo.is_none() && !c.logo_failed && self.texture.borrow().is_none())
            .then_some(c.logo_url.as_deref())
            .flatten()
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        if self.texture.borrow().is_none() {
            let bytes = self.channel.logo.as_ref()?;
            let tex = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)).ok()?;
            *self.texture.borrow_mut() = Some(tex);
        }
        self.texture.borrow().clone()
    }
}

/// Picker id of the "★ Favourites" entry (subscription ids are positive).
const FAVOURITES: i64 = -1;

pub fn row_of(obj: &glib::Object) -> std::cell::Ref<'_, ChannelRow> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .expect("BoxedAnyObject")
        .borrow::<ChannelRow>()
}

type Callback = Box<dyn Fn(&ChannelRow)>;
type LogoCallback = Box<dyn Fn(&str)>;
/// Right-click on a row: (widget, x, y, row).
type MenuCallback = Box<dyn Fn(&gtk::Widget, f64, f64, &ChannelRow)>;

pub struct ChannelList {
    pub kind: Kind,
    pub root: gtk::Box,
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    selection: gtk::SingleSelection,
    list_view: gtk::ListView,
    source: gtk::DropDown,
    search: gtk::SearchEntry,
    /// Subscription ids in the picker (index 0 = "All lists", then
    /// [`FAVOURITES`] when there are any).
    source_ids: RefCell<Vec<i64>>,
    stack: gtk::Stack,
    on_activate: RefCell<Option<Callback>>,
    /// Rows created by the factory (reused while scrolling).
    items: RefCell<Vec<glib::WeakRef<gtk::ListItem>>>,
    /// Group filter (`group-title`: country, category…); index 0 = all.
    group: gtk::DropDown,
    group_box: gtk::Box,
    group_names: RefCell<Vec<String>>,
    /// Logo request for a visible row (loading on demand).
    need_logo: RefCell<Option<LogoCallback>>,
    on_menu: RefCell<Option<MenuCallback>>,
}

impl ChannelList {
    pub fn new(kind: Kind) -> Rc<Self> {
        let source = gtk::DropDown::from_strings(&[tr("All lists")]);
        source.set_hexpand(true);
        let source_box = gtk::Box::builder().spacing(6).build();
        let source_label = gtk::Label::new(Some(tr("Channel list:")));
        source_label.set_width_chars(15);
        source_label.set_xalign(0.0);
        source_box.append(&source_label);
        source_box.append(&source);

        let group = gtk::DropDown::from_strings(&[tr("All groups")]);
        group.set_hexpand(true);
        group.set_enable_search(true);
        group.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<&gtk::Expression>,
            "string",
        )));
        let group_box = gtk::Box::builder().spacing(6).visible(false).build();
        let group_label = gtk::Label::new(Some(tr("Group:")));
        group_label.set_width_chars(15);
        group_label.set_xalign(0.0);
        group_box.append(&group_label);
        group_box.append(&group);

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search channel"))
            .build();

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filter = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let selection = gtk::SingleSelection::builder()
            .model(&filtered)
            .autoselect(false)
            .can_unselect(true)
            .build();

        let list_view = gtk::ListView::builder().model(&selection).build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list_view)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .has_frame(true)
            .build();

        let empty = gtk::Label::builder()
            .label(match kind {
                Kind::Tv => tr("No TV channels.\n\nAdd a list from\nTV-Maxe → Subscriptions…"),
                Kind::Radio => {
                    tr("No radio stations.\n\nAdd a list from\nTV-Maxe → Subscriptions…")
                }
            })
            .justify(gtk::Justification::Center)
            .vexpand(true)
            .build();
        empty.add_css_class("dim-label");

        let stack = gtk::Stack::new();
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty, Some("empty"));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        root.append(&source_box);
        root.append(&group_box);
        root.append(&search);
        root.append(&stack);

        let this = Rc::new(Self {
            kind,
            root,
            store,
            filter,
            selection,
            list_view,
            source,
            search,
            source_ids: RefCell::default(),
            stack,
            on_activate: RefCell::default(),
            items: RefCell::default(),
            group,
            group_box,
            group_names: RefCell::default(),
            need_logo: RefCell::default(),
            on_menu: RefCell::default(),
        });
        this.install_factory();
        this.install_filter();

        // Double-click / Enter on a channel → play.
        let weak = Rc::downgrade(&this);
        this.list_view.connect_activate(move |lv, pos| {
            let Some(this) = weak.upgrade() else { return };
            let Some(obj) = lv.model().and_then(|m| m.item(pos)) else {
                return;
            };
            if let Some(cb) = this.on_activate.borrow().as_ref() {
                cb(&row_of(&obj));
            }
        });

        this.update_empty();
        this
    }

    fn install_factory(self: &Rc<Self>) {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(this) = weak.upgrade() {
                this.items.borrow_mut().push(item.downgrade());
            }
            let image = gtk::Image::builder().pixel_size(LOGO_SIZE).build();
            let label = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let row = gtk::Box::builder()
                .spacing(8)
                .margin_top(3)
                .margin_bottom(3)
                .margin_start(4)
                .build();
            row.append(&image);
            row.append(&label);
            let star = gtk::Image::from_icon_name("starred-symbolic");
            star.add_css_class("favourite-star");
            star.set_margin_end(6);
            star.set_tooltip_text(Some(tr("Favourite")));
            row.append(&star);

            // Right-click → the channel menu (built by the window).
            let click = gtk::GestureClick::builder().button(3).build();
            let item_w = item.downgrade();
            let weak = weak.clone();
            click.connect_pressed(move |g, _, x, y| {
                let (Some(this), Some(item)) = (weak.upgrade(), item_w.upgrade()) else {
                    return;
                };
                let Some(obj) = item.item() else { return };
                this.selection.set_selected(item.position());
                if let (Some(cb), Some(w)) = (this.on_menu.borrow().as_ref(), g.widget()) {
                    cb(&w, x, y, &row_of(&obj));
                    g.set_state(gtk::EventSequenceState::Claimed);
                }
            });
            row.add_controller(click);
            item.set_child(Some(&row));
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            bind_row(item);
            if let (Some(this), Some(obj)) = (weak.upgrade(), item.item())
                && let Some(url) = row_of(&obj).needs_logo()
                && let Some(cb) = this.need_logo.borrow().as_ref()
            {
                cb(url);
            }
        });
        self.list_view.set_factory(Some(&factory));
    }

    /// Filtering: the chosen subscription + the search text (name or category).
    fn install_filter(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.filter.set_filter_func(move |obj| {
            let Some(this) = weak.upgrade() else {
                return true;
            };
            let row = row_of(obj);
            let sel = this.source.selected() as usize;
            match this.source_ids.borrow().get(sel).copied() {
                Some(FAVOURITES) if !row.favourite.get() => return false,
                Some(id) if id > 0 && id != row.channel.subscription_id => return false,
                _ => {}
            }
            let g = this.group.selected() as usize;
            if g > 0
                && row.channel.group.as_deref()
                    != this.group_names.borrow().get(g).map(String::as_str)
            {
                return false;
            }
            let q = fold(this.search.text().trim());
            q.is_empty()
                || fold(&row.channel.name).contains(&q)
                || row
                    .channel
                    .group
                    .as_deref()
                    .is_some_and(|g| fold(g).contains(&q))
        });
        let f = self.filter.clone();
        self.search
            .connect_search_changed(move |_| f.changed(gtk::FilterChange::Different));
        let weak = Rc::downgrade(self);
        self.source.connect_selected_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.rebuild_groups();
                this.filter.changed(gtk::FilterChange::Different);
            }
        });
        let f = self.filter.clone();
        self.group
            .connect_selected_notify(move |_| f.changed(gtk::FilterChange::Different));
    }

    /// The groups of the chosen list, sorted; hidden when pointless.
    fn rebuild_groups(&self) {
        let sel = self.source.selected() as usize;
        let sub = (sel > 0)
            .then(|| self.source_ids.borrow().get(sel).copied())
            .flatten();
        let mut groups = std::collections::BTreeSet::new();
        for i in 0..self.store.n_items() {
            let obj = self.store.item(i).unwrap();
            let row = row_of(&obj);
            if sub.is_none_or(|s| s == row.channel.subscription_id)
                && let Some(g) = &row.channel.group
            {
                groups.insert(g.clone());
            }
        }
        let current = self
            .group_names
            .borrow()
            .get(self.group.selected() as usize)
            .cloned();
        let mut names = vec![tr("All groups").to_string()];
        names.extend(groups);
        let keep = current
            .and_then(|c| names.iter().position(|n| *n == c))
            .unwrap_or(0);
        let strs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.group_box.set_visible(names.len() > 2);
        *self.group_names.borrow_mut() = names.clone();
        self.group.set_model(Some(&gtk::StringList::new(&strs)));
        self.group.set_selected(keep as u32);
    }

    pub fn connect_context_menu(&self, f: impl Fn(&gtk::Widget, f64, f64, &ChannelRow) + 'static) {
        *self.on_menu.borrow_mut() = Some(Box::new(f));
    }

    /// The row of the channel with this database id, if it is in this list.
    pub fn item_by_id(&self, id: i64) -> Option<glib::BoxedAnyObject> {
        (0..self.store.n_items())
            .filter_map(|i| self.store.item(i).and_downcast::<glib::BoxedAnyObject>())
            .find(|obj| row_of(obj.upcast_ref()).channel.id == id)
    }

    pub fn connect_need_logo(&self, f: impl Fn(&str) + 'static) {
        *self.need_logo.borrow_mut() = Some(Box::new(f));
    }

    /// A logo arrived: put it on every row that uses it.
    pub fn set_logo(&self, url: &str, texture: &gdk::Texture) {
        let mut changed = false;
        for i in 0..self.store.n_items() {
            let obj = self.store.item(i).unwrap();
            let row = row_of(&obj);
            if row.channel.logo_url.as_deref() == Some(url) {
                row.set_texture(texture.clone());
                changed = true;
            }
        }
        if changed {
            self.refresh_visible();
        }
    }

    /// Refills only the visible rows (the model stays untouched).
    pub fn refresh_visible(&self) {
        self.items.borrow_mut().retain(|w| w.upgrade().is_some());
        let items: Vec<gtk::ListItem> = self
            .items
            .borrow()
            .iter()
            .filter_map(|w| w.upgrade())
            .collect();
        for item in items {
            bind_row(&item);
            if let Some(obj) = item.item()
                && let Some(url) = row_of(&obj).needs_logo()
                && let Some(cb) = self.need_logo.borrow().as_ref()
            {
                cb(url);
            }
        }
    }

    pub fn connect_activate(&self, f: impl Fn(&ChannelRow) + 'static) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// Reloads the subscription picker, keeping the selection when possible.
    pub fn set_subscriptions(&self, subs: &[Subscription]) {
        let current = self
            .source_ids
            .borrow()
            .get(self.source.selected() as usize)
            .copied();
        let subs: Vec<&Subscription> = subs
            .iter()
            .filter(|s| {
                s.enabled
                    && if self.kind == Kind::Tv {
                        s.tv_count > 0
                    } else {
                        s.radio_count > 0
                    }
            })
            .collect();
        let mut names = vec![tr("All lists")];
        let mut ids = vec![0];
        if self.has_favourites() {
            names.push(tr("★ Favourites"));
            ids.push(FAVOURITES);
        }
        names.extend(subs.iter().map(|s| s.display_name()));
        ids.extend(subs.iter().map(|s| s.id));
        let keep = current
            .and_then(|c| ids.iter().position(|&i| i == c))
            .unwrap_or(0);
        *self.source_ids.borrow_mut() = ids;
        self.source.set_model(Some(&gtk::StringList::new(&names)));
        self.source.set_selected(keep as u32);
    }

    /// Replaces the channels, keeping the selected channel (by URL) if it still exists.
    pub fn set_channels(&self, rows: Vec<ChannelRow>) {
        let selected_url = self
            .selected()
            .and_then(|o| row_of(o.upcast_ref()).channel.urls.first().cloned());
        let items: Vec<glib::BoxedAnyObject> =
            rows.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.rebuild_groups();
        self.filter.changed(gtk::FilterChange::Different);
        self.update_empty();
        if let Some(url) = selected_url {
            self.select_by_url(&url);
        }
    }

    /// The row at position `i` in the full (unfiltered) list.
    pub fn item(&self, i: u32) -> Option<glib::BoxedAnyObject> {
        self.store.item(i).and_downcast()
    }

    pub fn len(&self) -> u32 {
        self.store.n_items()
    }

    fn has_favourites(&self) -> bool {
        (0..self.store.n_items()).any(|i| {
            self.store
                .item(i)
                .is_some_and(|o| row_of(&o).favourite.get())
        })
    }

    /// Marks the favourites (by first URL, or by name) and refreshes the rows
    /// and, if it is chosen, the favourites view. Returns whether the
    /// subscription picker must be rebuilt (favourites appeared or vanished).
    pub fn set_favourites(&self, favs: &[(String, String)]) -> bool {
        let had = self.has_favourites();
        for i in 0..self.store.n_items() {
            let obj = self.store.item(i).unwrap();
            let row = row_of(&obj);
            let url = row.channel.urls.first().map(String::as_str);
            let fav = favs
                .iter()
                .any(|(u, n)| Some(u.as_str()) == url || *n == row.channel.name);
            row.favourite.set(fav);
        }
        self.refresh_visible();
        if self.showing_favourites() {
            self.filter.changed(gtk::FilterChange::Different);
        }
        had != self.has_favourites()
    }

    /// Shows only the favourites (picker on "★ Favourites"); `false` if
    /// there are none.
    pub fn show_favourites(&self) -> bool {
        let pos = self
            .source_ids
            .borrow()
            .iter()
            .position(|&i| i == FAVOURITES);
        match pos {
            Some(p) => {
                self.source.set_selected(p as u32);
                true
            }
            None => false,
        }
    }

    fn showing_favourites(&self) -> bool {
        self.source_ids
            .borrow()
            .get(self.source.selected() as usize)
            .copied()
            == Some(FAVOURITES)
    }

    // ---------- Zapping: the channels as they are shown (filtered) ----------

    /// How many channels are shown now.
    pub fn visible_len(&self) -> u32 {
        self.selection.n_items()
    }

    /// The shown channel at `pos` (0-based).
    pub fn visible_item(&self, pos: u32) -> Option<glib::BoxedAnyObject> {
        self.selection.item(pos).and_downcast()
    }

    /// Position among the shown channels of the channel with this first URL.
    pub fn visible_position(&self, url: &str) -> Option<u32> {
        (0..self.selection.n_items()).find(|&i| {
            self.selection
                .item(i)
                .is_some_and(|o| row_of(&o).channel.urls.first().map(String::as_str) == Some(url))
        })
    }

    pub fn selected_position(&self) -> Option<u32> {
        let p = self.selection.selected();
        (p != gtk::INVALID_LIST_POSITION).then_some(p)
    }

    /// Selects the shown channel at `pos` and scrolls it into view (without
    /// moving the keyboard focus, so zapping from fullscreen stays there).
    pub fn select_visible(&self, pos: u32) {
        self.list_view
            .scroll_to(pos, gtk::ListScrollFlags::SELECT, None);
    }

    /// The channel with this first URL, anywhere in the list.
    pub fn item_by_url(&self, url: &str) -> Option<glib::BoxedAnyObject> {
        (0..self.store.n_items())
            .filter_map(|i| self.store.item(i).and_downcast::<glib::BoxedAnyObject>())
            .find(|o| {
                row_of(o.upcast_ref())
                    .channel
                    .urls
                    .first()
                    .map(String::as_str)
                    == Some(url)
            })
    }

    fn update_empty(&self) {
        self.stack
            .set_visible_child_name(if self.store.n_items() == 0 {
                "empty"
            } else {
                "list"
            });
    }

    pub fn selected(&self) -> Option<glib::BoxedAnyObject> {
        self.selection.selected_item().and_downcast()
    }

    /// Selects the channel with the given first URL and scrolls it into view.
    pub fn select_by_url(&self, url: &str) -> bool {
        let model = self.selection.model().unwrap();
        let found = (0..model.n_items()).find(|&i| {
            row_of(&model.item(i).unwrap())
                .channel
                .urls
                .first()
                .map(String::as_str)
                == Some(url)
        });
        if let Some(i) = found {
            self.list_view.scroll_to(
                i,
                gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
        found.is_some()
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }
}

/// Fills a row's widgets from its data.
fn bind_row(item: &gtk::ListItem) {
    let Some(obj) = item.item() else { return };
    let row = row_of(&obj);
    let bx = item.child().and_downcast::<gtk::Box>().unwrap();
    let image = bx.first_child().and_downcast::<gtk::Image>().unwrap();
    let label = image.next_sibling().and_downcast::<gtk::Label>().unwrap();
    if let Some(star) = label.next_sibling() {
        star.set_visible(row.favourite.get());
    }
    label.set_label(&row.channel.name);
    match row.texture() {
        Some(t) => image.set_paintable(Some(&t)),
        None => image.set_icon_name(Some(placeholder_icon(row.channel.kind))),
    }
    let tip = match &row.channel.group {
        Some(g) => format!("{}\n{g} · {}", row.channel.name, row.subscription),
        None => format!("{}\n{}", row.channel.name, row.subscription),
    };
    bx.set_tooltip_text(Some(&tip));
}

/// Text for searching: lowercase, without diacritics ("Știri" ↔ "stiri").
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ă' | 'â' => 'a',
            'î' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            other => other,
        })
        .collect()
}

pub fn placeholder_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Tv => "video-display-symbolic",
        Kind::Radio => "audio-x-generic-symbolic",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn search_ignores_diacritics() {
        assert!(super::fold("Știri Pro TV").contains(&super::fold("stiri")));
        assert!(super::fold("Antena 3 CNN").contains("antena"));
        assert_eq!(super::fold("ÎNȚELEPCIUNE"), "intelepciune");
    }
}
