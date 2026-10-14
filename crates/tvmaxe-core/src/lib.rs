//! TV-Maxe core, without UI: models, list formats, the SQLite database,
//! network operations (updating subscriptions, downloading logos) and
//! the interface translations (`i18n`).

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
