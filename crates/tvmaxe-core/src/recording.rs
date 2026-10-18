//! Recordings: the model, the scheduling rules and file naming.
//!
//! Everything here is pure (no clock, no I/O), so the scheduler's decisions
//! can be unit-tested. The application calls [`plan`] every second with the
//! current time and carries out the returned actions.

use crate::{Kind, StreamOptions};

const DAY: i64 = 24 * 3600;

/// How often a scheduled recording repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Once,
    Daily,
    Weekly,
}

impl Repeat {
    pub const ALL: [Repeat; 3] = [Repeat::Once, Repeat::Daily, Repeat::Weekly];

    pub fn as_str(self) -> &'static str {
        match self {
            Repeat::Once => "once",
            Repeat::Daily => "daily",
            Repeat::Weekly => "weekly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// Seconds between two occurrences (`None` = does not repeat).
    pub fn period(self) -> Option<i64> {
        match self {
            Repeat::Once => None,
            Repeat::Daily => Some(DAY),
            Repeat::Weekly => Some(7 * DAY),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waiting for its start time.
    Scheduled,
    /// Being written to disk now.
    Recording,
    /// Finished at its stop time (or stopped by the user).
    Done,
    /// Every URL failed; nothing (useful) was written.
    Failed,
    /// The start time passed while TV-Maxe was not running.
    Missed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Scheduled => "scheduled",
            Status::Recording => "recording",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Missed => "missed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Status::Scheduled,
            Status::Recording,
            Status::Done,
            Status::Failed,
            Status::Missed,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }
}

/// A recording job: what to record, when, and how it went.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub id: i64,
    /// A name chosen by the user (may be empty).
    pub title: String,
    pub channel: String,
    pub kind: Kind,
    /// The channel's URLs, copied when the recording is created, so it still
    /// works if the subscription changes in the meantime.
    pub urls: Vec<String>,
    pub http: StreamOptions,
    /// Unix seconds.
    pub start: i64,
    /// Unix seconds; `None` = until stopped by hand ("Record now").
    pub stop: Option<i64>,
    pub repeat: Repeat,
    pub status: Status,
    /// The files written so far (a reconnect starts a new part).
    pub files: Vec<String>,
    pub error: Option<String>,
}

impl Recording {
    /// A new, scheduled recording of a channel (`stop = None`: until stopped).
    pub fn new(
        channel: &str,
        kind: Kind,
        urls: &[String],
        http: &StreamOptions,
        start: i64,
        stop: Option<i64>,
    ) -> Self {
        Self {
            id: 0,
            title: String::new(),
            channel: channel.to_string(),
            kind,
            urls: urls.to_vec(),
            http: http.clone(),
            start,
            stop,
            repeat: Repeat::Once,
            status: Status::Scheduled,
            files: Vec::new(),
            error: None,
        }
    }

    /// The name shown in lists: the title, or the channel if there is none.
    pub fn display_title(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.channel
        } else {
            &self.title
        }
    }

    /// The next occurrence of a repeating recording that has not ended by
    /// `now` (occurrences missed in between are skipped).
    pub fn next_occurrence(&self, now: i64) -> Option<Recording> {
        let period = self.repeat.period()?;
        let stop = self.stop?;
        let mut next = self.clone();
        next.id = 0;
        next.status = Status::Scheduled;
        next.files.clear();
        next.error = None;
        // Smallest k ≥ 1 with stop + k·period > now.
        let k = ((now - stop).div_euclid(period) + 1).max(1);
        next.start = self.start + k * period;
        next.stop = Some(stop + k * period);
        Some(next)
    }
}

/// What the scheduler must do at a given moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start writing recording `id`.
    Start(i64),
    /// Recording `id` reached its stop time.
    Stop(i64),
    /// Recording `id` should have started but its time has passed.
    Miss(i64),
}

/// The scheduler's decisions for `now`. `running` = ids with an active
/// recorder in this process (a `Recording` row without one is left over from
/// a crash and is finished).
pub fn plan(recordings: &[Recording], running: &[i64], now: i64) -> Vec<Action> {
    let mut actions = Vec::new();
    for r in recordings {
        let ended = r.stop.is_some_and(|stop| stop <= now);
        match r.status {
            Status::Scheduled if ended => actions.push(Action::Miss(r.id)),
            Status::Scheduled if r.start <= now => actions.push(Action::Start(r.id)),
            Status::Recording if ended || !running.contains(&r.id) => {
                actions.push(Action::Stop(r.id))
            }
            _ => {}
        }
    }
    actions
}

/// A safe file name: "Channel 2026-10-14 20.30 - Title.ts". `when` is the
/// already formatted local start time.
pub fn file_name(channel: &str, title: &str, when: &str, part: u32, extension: &str) -> String {
    let mut name = clean(channel);
    if !when.is_empty() {
        name.push(' ');
        name.push_str(&clean(when));
    }
    let title = clean(title);
    if !title.is_empty() && title != clean(channel) {
        name.push_str(" - ");
        name.push_str(&title);
    }
    if name.trim().is_empty() {
        name = "recording".into();
    }
    // Keep the name well below the 255-byte limit of most file systems.
    let mut name: String = name.chars().take(150).collect();
    if part > 1 {
        name.push_str(&format!(" ({part})"));
    }
    format!("{}.{extension}", name.trim())
}

/// Removes characters that are invalid or awkward in file names.
fn clean(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    mapped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: i64, start: i64, stop: Option<i64>, status: Status) -> Recording {
        Recording {
            id,
            title: String::new(),
            channel: "TVR 1".into(),
            kind: Kind::Tv,
            urls: vec!["http://a".into()],
            http: StreamOptions::default(),
            start,
            stop,
            repeat: Repeat::Once,
            status,
            files: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn plan_starts_stops_and_misses() {
        let recs = [
            rec(1, 100, Some(200), Status::Scheduled), // due
            rec(2, 300, Some(400), Status::Scheduled), // later
            rec(3, 10, Some(50), Status::Scheduled),   // missed
            rec(4, 0, Some(140), Status::Recording),   // ends
            rec(5, 0, Some(900), Status::Recording),   // goes on
            rec(6, 0, None, Status::Recording),        // manual, goes on
            rec(7, 0, Some(900), Status::Recording),   // no recorder (crash)
            rec(8, 0, Some(20), Status::Done),
        ];
        assert_eq!(
            plan(&recs, &[4, 5, 6], 150),
            [
                Action::Start(1),
                Action::Miss(3),
                Action::Stop(4),
                Action::Stop(7)
            ]
        );
    }

    #[test]
    fn plan_boundaries() {
        let r = [rec(1, 100, Some(200), Status::Scheduled)];
        assert!(plan(&r, &[], 99).is_empty());
        assert_eq!(plan(&r, &[], 100), [Action::Start(1)]);
        assert_eq!(plan(&r, &[], 200), [Action::Miss(1)], "stop time reached");
    }

    #[test]
    fn next_occurrence_skips_missed_ones() {
        let mut r = rec(9, 1000, Some(2000), Status::Done);
        r.repeat = Repeat::Daily;
        r.files = vec!["a.ts".into()];
        let n = r.next_occurrence(2000).unwrap();
        assert_eq!((n.start, n.stop), (1000 + DAY, Some(2000 + DAY)));
        assert_eq!((n.id, n.status), (0, Status::Scheduled));
        assert!(n.files.is_empty());

        // Three days later: the next one is the fourth occurrence.
        let n = r.next_occurrence(2000 + 3 * DAY + 5).unwrap();
        assert_eq!(n.start, 1000 + 4 * DAY);

        r.repeat = Repeat::Weekly;
        assert_eq!(r.next_occurrence(0).unwrap().start, 1000 + 7 * DAY);
        r.repeat = Repeat::Once;
        assert!(r.next_occurrence(0).is_none());
    }

    #[test]
    fn file_names() {
        assert_eq!(
            file_name(
                "Pro TV",
                "Știrile: ediția / 19:00",
                "2026-10-14 20.30",
                1,
                "ts"
            ),
            "Pro TV 2026-10-14 20.30 - Știrile- ediția - 19-00.ts"
        );
        assert_eq!(
            file_name("TVR 1", "TVR 1", "2026-10-14 20.30", 2, "ts"),
            "TVR 1 2026-10-14 20.30 (2).ts",
            "the title is not repeated"
        );
        assert_eq!(file_name("..", "", "", 1, "ts"), "recording.ts");
        let long = file_name(&"x".repeat(400), "", "", 1, "ts");
        assert!(long.len() < 200);
    }

    #[test]
    fn enums_roundtrip() {
        for r in Repeat::ALL {
            assert_eq!(Repeat::parse(r.as_str()), Some(r));
        }
        for s in ["scheduled", "recording", "done", "failed", "missed"] {
            assert_eq!(Status::parse(s).unwrap().as_str(), s);
        }
    }
}
