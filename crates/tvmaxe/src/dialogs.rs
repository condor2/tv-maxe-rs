//! Secondary windows, split by purpose.

mod channel;
mod guide;
mod preferences;
mod recordings;
mod sleep;
mod subscriptions;

pub use channel::{about, add_channel, edit_channel, export_local};
pub use guide::GuideDialog;
pub use preferences::preferences;
pub use recordings::{RecordingsDialog, schedule_dialog};
pub use sleep::sleep_timer;
pub use subscriptions::SubscriptionsDialog;
