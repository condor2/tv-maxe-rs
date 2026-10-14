//! Nucleul TV-Maxe, fără UI: modele, formate de liste, baza de date SQLite
//! operațiile de rețea (actualizare abonamente, descărcare logo-uri) și
//! traducerile interfeței (`i18n`).

pub mod db;
pub mod epg;
pub mod error;
pub mod formats;
pub mod i18n;
pub mod model;
pub mod net;

pub use db::{Database, EpgIndex, StoredChannel, Subscription};
pub use error::{Error, Result};
pub use model::{Channel, ChannelList, Kind, Logo, StreamOptions};
