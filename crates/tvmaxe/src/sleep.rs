//! Sleep timer: what happens when it runs out, the volume fade and the
//! countdown text. The timer itself lives in the main window.

use std::time::Duration;
use tvmaxe_core::i18n::tr;

/// The last seconds before the end, during which the volume goes down.
pub const FADE: Duration = Duration::from_secs(30);

/// Durations offered as one-click choices, in minutes.
pub const PRESETS: [u32; 6] = [15, 30, 45, 60, 90, 120];

/// Accepted range for a custom duration, in minutes.
pub const MINUTES: (u32, u32) = (1, 600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SleepAction {
    #[default]
    StopPlayback,
    Quit,
    Suspend,
    PowerOff,
}

impl SleepAction {
    pub const ALL: [SleepAction; 4] = [
        SleepAction::StopPlayback,
        SleepAction::Quit,
        SleepAction::Suspend,
        SleepAction::PowerOff,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SleepAction::StopPlayback => "stop",
            SleepAction::Quit => "quit",
            SleepAction::Suspend => "suspend",
            SleepAction::PowerOff => "poweroff",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            SleepAction::StopPlayback => tr("Stop playback"),
            SleepAction::Quit => tr("Quit TV-Maxe"),
            SleepAction::Suspend => tr("Suspend the computer"),
            SleepAction::PowerOff => tr("Shut down the computer"),
        }
    }

    /// The logind method that checks whether the action is allowed, if the
    /// action needs the system.
    pub fn logind_check(self) -> Option<&'static str> {
        match self {
            SleepAction::Suspend => Some("CanSuspend"),
            SleepAction::PowerOff => Some("CanPowerOff"),
            _ => None,
        }
    }

    /// The logind method that carries out the action, if it needs the system.
    pub fn logind_method(self) -> Option<&'static str> {
        match self {
            SleepAction::Suspend => Some("Suspend"),
            SleepAction::PowerOff => Some("PowerOff"),
            _ => None,
        }
    }

    /// Whether the action would end a running recording.
    pub fn ends_recordings(self) -> bool {
        self != SleepAction::StopPlayback
    }
}

/// Volume multiplier (0.0–1.0) with `remaining` time left: 1.0 until the
/// fade starts, then down linearly to 0.
pub fn volume_factor(remaining: Duration) -> f64 {
    if remaining >= FADE {
        1.0
    } else {
        remaining.as_secs_f64() / FADE.as_secs_f64()
    }
}

/// "29:41", or "1:05:09" from one hour up. Rounded up, so the countdown
/// shows 0:00 only at the very end.
pub fn format_remaining(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Minutes until a programme ends, rounded up, within [`MINUTES`].
pub fn minutes_until(now: i64, stop: i64) -> Option<u32> {
    let secs = stop - now;
    if secs <= 0 {
        return None;
    }
    let m = u32::try_from((secs + 59) / 60).ok()?;
    Some(m.clamp(MINUTES.0, MINUTES.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade() {
        assert_eq!(volume_factor(Duration::from_secs(600)), 1.0);
        assert_eq!(volume_factor(FADE), 1.0);
        assert!((volume_factor(FADE / 2) - 0.5).abs() < 1e-9);
        assert_eq!(volume_factor(Duration::ZERO), 0.0);
    }

    #[test]
    fn countdown_text() {
        assert_eq!(format_remaining(Duration::from_secs(29 * 60 + 41)), "29:41");
        assert_eq!(format_remaining(Duration::from_millis(59_200)), "1:00");
        assert_eq!(format_remaining(Duration::ZERO), "0:00");
        assert_eq!(format_remaining(Duration::from_secs(3909)), "1:05:09");
    }

    #[test]
    fn until_end_of_programme() {
        assert_eq!(minutes_until(1000, 1000 + 61), Some(2));
        assert_eq!(minutes_until(1000, 1000 + 60), Some(1));
        assert_eq!(minutes_until(1000, 900), None);
        assert_eq!(minutes_until(0, 100 * 3600), Some(MINUTES.1));
    }

    #[test]
    fn actions() {
        for a in SleepAction::ALL {
            assert_eq!(SleepAction::parse(a.as_str()), Some(a));
        }
        assert!(!SleepAction::StopPlayback.ends_recordings());
        assert!(SleepAction::PowerOff.ends_recordings());
        assert_eq!(SleepAction::Quit.logind_method(), None);
    }
}
