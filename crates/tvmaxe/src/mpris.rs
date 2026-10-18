//! MPRIS 2: lets the desktop control TV-Maxe (media keys, the sound applet
//! of GNOME / KDE / Xfce, `playerctl`) and shows the channel and the programme
//! on air.
//!
//! This module only speaks D-Bus. It knows nothing about the window: the
//! window feeds it a [`State`] and receives [`Command`]s.

use gtk::gio;
use gtk::glib::{self, Variant, prelude::ToVariant};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.tvmaxe";
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// `mpris:trackid` when nothing is loaded (defined by the specification).
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

const INTROSPECTION: &str = r#"
<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="CanSetFullscreen" type="b" access="read"/>
    <property name="Fullscreen" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="DesktopEntry" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek"><arg direction="in" name="Offset" type="x"/></method>
    <method name="SetPosition">
      <arg direction="in" name="TrackId" type="o"/>
      <arg direction="in" name="Position" type="x"/>
    </method>
    <method name="OpenUri"><arg direction="in" name="Uri" type="s"/></method>
    <signal name="Seeked"><arg name="Position" type="x"/></signal>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="Rate" type="d" access="read"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read"/>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
</node>"#;

/// What the desktop asks TV-Maxe to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Raise,
    Quit,
    Next,
    Previous,
    Play,
    Pause,
    PlayPause,
    Stop,
    /// Volume from 0.0 to 1.0.
    SetVolume(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
    }
}

/// What the desktop can see.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub status: Status,
    /// The channel (database id, name), if one was chosen.
    pub channel: Option<(i64, String)>,
    /// The programme on air on that channel, if the guide knows it.
    pub programme: Option<String>,
    /// From 0.0 to 1.0.
    pub volume: f64,
    /// There are channels to go to with Next / Previous.
    pub can_navigate: bool,
}

/// The names of the `Player` properties that differ between two states.
fn changed(old: &State, new: &State) -> Vec<&'static str> {
    let mut v = Vec::new();
    if old.status != new.status {
        v.push("PlaybackStatus");
    }
    if old.channel != new.channel || old.programme != new.programme {
        v.push("Metadata");
    }
    if old.volume != new.volume {
        v.push("Volume");
    }
    if old.can_navigate != new.can_navigate {
        v.extend(["CanGoNext", "CanGoPrevious"]);
    }
    v
}

fn object_path(path: &str) -> Variant {
    glib::variant::ObjectPath::try_from(path)
        .unwrap_or_else(|_| glib::variant::ObjectPath::try_from(NO_TRACK).expect("valid path"))
        .to_variant()
}

/// `Metadata`: the programme is the title and the channel the artist, as
/// players show a song; without a guide the channel is the title.
fn metadata(state: &State) -> HashMap<String, Variant> {
    let mut m = HashMap::new();
    let Some((id, name)) = &state.channel else {
        m.insert("mpris:trackid".to_string(), object_path(NO_TRACK));
        return m;
    };
    m.insert(
        "mpris:trackid".to_string(),
        object_path(&format!("/io/condor2/TvMaxe/channel/{}", id.unsigned_abs())),
    );
    match &state.programme {
        Some(programme) => {
            m.insert("xesam:title".to_string(), programme.to_variant());
            m.insert("xesam:artist".to_string(), vec![name.clone()].to_variant());
        }
        None => {
            m.insert("xesam:title".to_string(), name.to_variant());
        }
    }
    m
}

fn root_property(name: &str) -> Variant {
    match name {
        "Identity" => "TV-Maxe".to_variant(),
        "DesktopEntry" => "TvMaxe".to_variant(),
        "CanQuit" | "CanRaise" => true.to_variant(),
        "SupportedUriSchemes" | "SupportedMimeTypes" => Vec::<String>::new().to_variant(),
        // CanSetFullscreen, Fullscreen, HasTrackList
        _ => false.to_variant(),
    }
}

fn player_property(name: &str, s: &State) -> Variant {
    match name {
        "PlaybackStatus" => s.status.as_str().to_variant(),
        "Metadata" => metadata(s).to_variant(),
        "Volume" => s.volume.to_variant(),
        "Rate" | "MinimumRate" | "MaximumRate" => 1.0f64.to_variant(),
        // A live stream has no position to report.
        "Position" => 0i64.to_variant(),
        "CanGoNext" | "CanGoPrevious" => s.can_navigate.to_variant(),
        "CanPlay" | "CanPause" | "CanControl" => true.to_variant(),
        // CanSeek
        _ => false.to_variant(),
    }
}

fn command_for(interface: &str, method: &str) -> Option<Command> {
    match (interface, method) {
        (ROOT, "Raise") => Some(Command::Raise),
        (ROOT, "Quit") => Some(Command::Quit),
        (PLAYER, "Next") => Some(Command::Next),
        (PLAYER, "Previous") => Some(Command::Previous),
        (PLAYER, "Play") => Some(Command::Play),
        (PLAYER, "Pause") => Some(Command::Pause),
        (PLAYER, "PlayPause") => Some(Command::PlayPause),
        (PLAYER, "Stop") => Some(Command::Stop),
        // Seek, SetPosition and OpenUri do nothing: there is no seeking in a
        // live stream, and channels are not opened by URI.
        _ => None,
    }
}

struct Shared {
    state: RefCell<State>,
    connection: RefCell<Option<gio::DBusConnection>>,
    on_command: Box<dyn Fn(Command)>,
}

/// The service on the session bus. It stops when dropped.
pub struct Mpris {
    shared: Rc<Shared>,
    owner: Option<gio::OwnerId>,
}

impl Mpris {
    /// Takes the name `org.mpris.MediaPlayer2.tvmaxe` on the session bus.
    /// Without a session bus the service just stays idle.
    pub fn start(on_command: impl Fn(Command) + 'static) -> Self {
        let shared = Rc::new(Shared {
            state: RefCell::default(),
            connection: RefCell::new(None),
            on_command: Box::new(on_command),
        });
        let s = shared.clone();
        let owner = gio::bus_own_name(
            gio::BusType::Session,
            BUS_NAME,
            gio::BusNameOwnerFlags::DO_NOT_QUEUE,
            move |connection, _| {
                if let Err(e) = register(&connection, &s) {
                    eprintln!("MPRIS: {e}");
                    return;
                }
                *s.connection.borrow_mut() = Some(connection);
            },
            |_, _| {},
            |_, name| eprintln!("MPRIS: the name {name} is not available"),
        );
        Self {
            shared,
            owner: Some(owner),
        }
    }

    /// Publishes a new state; the properties that changed are announced.
    pub fn update(&self, new: State) {
        let names = {
            let mut old = self.shared.state.borrow_mut();
            let names = changed(&old, &new);
            *old = new.clone();
            names
        };
        let connection = self.shared.connection.borrow();
        let Some(connection) = connection.as_ref() else {
            return;
        };
        if names.is_empty() {
            return;
        }
        let props: HashMap<String, Variant> = names
            .iter()
            .map(|n| (n.to_string(), player_property(n, &new)))
            .collect();
        let payload = (PLAYER, props, Vec::<String>::new()).to_variant();
        if let Err(e) = connection.emit_signal(
            None,
            OBJECT_PATH,
            PROPERTIES,
            "PropertiesChanged",
            Some(&payload),
        ) {
            eprintln!("MPRIS: cannot announce the change: {e}");
        }
    }
}

impl Drop for Mpris {
    fn drop(&mut self) {
        if let Some(id) = self.owner.take() {
            gio::bus_unown_name(id);
        }
    }
}

fn register(connection: &gio::DBusConnection, shared: &Rc<Shared>) -> Result<(), glib::Error> {
    let node = gio::DBusNodeInfo::for_xml(INTROSPECTION)?;
    for name in [ROOT, PLAYER] {
        let info = node
            .lookup_interface(name)
            .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "missing interface"))?;
        let on_call = shared.clone();
        let on_get = shared.clone();
        let on_set = shared.clone();
        connection
            .register_object(OBJECT_PATH, &info)
            .method_call(move |_, _, _, interface, method, params, invocation| {
                let interface = interface.unwrap_or_default();
                if let Some(cmd) = command_for(interface, method) {
                    (on_call.on_command)(cmd);
                }
                let _ = params;
                invocation.return_value(None);
            })
            .property(move |_, _, _, interface, property| {
                if interface == ROOT {
                    root_property(property)
                } else {
                    player_property(property, &on_get.state.borrow())
                }
            })
            .set_property(move |_, _, _, interface, property, value| {
                match (interface, property, value.get::<f64>()) {
                    (PLAYER, "Volume", Some(v)) => {
                        (on_set.on_command)(Command::SetVolume(v.clamp(0.0, 1.0)));
                        true
                    }
                    _ => false,
                }
            })
            .build()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> State {
        State {
            status: Status::Playing,
            channel: Some((7, "Kanal D".into())),
            programme: Some("Știri".into()),
            volume: 0.5,
            can_navigate: true,
        }
    }

    #[test]
    fn interfaces_parse() {
        let node = gio::DBusNodeInfo::for_xml(INTROSPECTION).unwrap();
        assert!(node.lookup_interface(ROOT).is_some());
        assert!(node.lookup_interface(PLAYER).is_some());
    }

    #[test]
    fn only_changed_properties_are_announced() {
        let a = playing();
        assert!(changed(&a, &a).is_empty());
        let b = State {
            status: Status::Paused,
            ..a.clone()
        };
        assert_eq!(changed(&a, &b), ["PlaybackStatus"]);
        let c = State {
            programme: None,
            volume: 1.0,
            ..a.clone()
        };
        assert_eq!(changed(&a, &c), ["Metadata", "Volume"]);
    }

    #[test]
    fn metadata_shows_programme_and_channel() {
        let m = metadata(&playing());
        assert_eq!(m["xesam:title"].get::<String>().as_deref(), Some("Știri"));
        assert_eq!(
            m["xesam:artist"].get::<Vec<String>>(),
            Some(vec!["Kanal D".to_string()])
        );
        assert_eq!(
            m["mpris:trackid"].str(),
            Some("/io/condor2/TvMaxe/channel/7")
        );
    }

    #[test]
    fn metadata_without_guide_uses_the_channel_name() {
        let s = State {
            programme: None,
            ..playing()
        };
        let m = metadata(&s);
        assert_eq!(m["xesam:title"].get::<String>().as_deref(), Some("Kanal D"));
        assert!(!m.contains_key("xesam:artist"));
    }

    #[test]
    fn metadata_when_stopped_has_no_track() {
        let m = metadata(&State::default());
        assert_eq!(m.len(), 1);
        assert_eq!(m["mpris:trackid"].str(), Some(NO_TRACK));
    }

    #[test]
    fn methods_map_to_commands() {
        assert_eq!(command_for(PLAYER, "PlayPause"), Some(Command::PlayPause));
        assert_eq!(command_for(ROOT, "Quit"), Some(Command::Quit));
        assert_eq!(command_for(PLAYER, "Seek"), None);
        // The same name on the wrong interface is not a command.
        assert_eq!(command_for(ROOT, "Next"), None);
    }
}
