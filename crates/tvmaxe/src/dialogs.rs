//! Secondary windows, split by purpose.

mod channel;
mod preferences;
mod recordings;
mod sleep;
mod subscriptions;
mod xtream;

pub use channel::{about, add_channel, edit_channel, export_local};
pub use preferences::preferences;
pub use recordings::{RecordingsDialog, schedule_dialog};
pub use sleep::sleep_timer;
pub use subscriptions::SubscriptionsDialog;
pub use xtream::xtream_account;
