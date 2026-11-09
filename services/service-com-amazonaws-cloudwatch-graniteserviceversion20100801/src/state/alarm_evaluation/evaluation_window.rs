// SPDX-License-Identifier: CC0-1.0

//! The GRID of period windows ONE metric alarm's evaluation grades: where the boundaries of
//! those windows fall, and whether the window the clock is currently IN may be graded before it
//! closes.
//!
//! "The evaluation window that an alarm uses to select the range of metric data that it
//! evaluates each time it runs. This is a union type. Set exactly one of its members,
//! `SlidingWindow` or `WallClockWindow`. If you don't set `EvaluationWindow`, the alarm uses a
//! `SlidingWindow` by default." (`MetricAlarm$EvaluationWindow`) — so the configuration of an
//! alarm chooses which of two grids its evaluation reads the series over, and an alarm that
//! chooses neither is read over the rolling one:
//!
//! * a SLIDING window "advances each time the alarm is evaluated, forming a rolling time
//!   window" (`SlidingWindow`), so its range ends one evaluation cadence behind the clock
//!   ([`super::evaluation_lag_secs`]) and the period the clock is in is graded while it is
//!   still open — "the fastest detection";
//! * a WALL CLOCK window "aligns the evaluated range to fixed clock boundaries that match the
//!   alarm's period, such as the top of the hour, midnight, or the start of the calendar week,
//!   optionally in a specific time zone" (`WallClockWindow`), so its range begins at the
//!   boundary the alarm's own period repeats at and covers only periods that have CLOSED: the
//!   period the clock is in decides nothing until its own end has passed, which is why such an
//!   alarm reports no verdict over a partial period at all.
//!
//! The zone the boundaries are the clock's own in is the one the window names, and it shifts
//! them "for all periods, including periods of one hour or shorter"
//! (`WallClockWindow$Timezone`), read in each of the three forms that member documents — an
//! IANA name, a fixed UTC offset, and an offset prefixed with `UTC` — through the same IANA
//! database every other zone of this service is resolved against
//! ([`crate::state::alarm_mute_rules`]). Which clock boundary a period repeats at is the
//! period's own: the top of the minute, the five-minute mark, the top of the hour and midnight
//! are multiples of those periods measured on that clock, while a week-wide period begins where
//! a calendar week does ([`CALENDAR_WEEK_SHIFT_SECS`]).

use ::chrono::{DateTime, FixedOffset, Offset};
use ::chrono_tz::Tz;

use crate::ops::types::{EvaluationWindow, MetricAlarm};

/// The zone an aligned window's boundaries are the clock's own in when its configuration names
/// none: "If you don't specify a time zone, CloudWatch uses `UTC`."
/// (`WallClockWindow$Timezone`) — which is this store's own clock, so its boundaries need no
/// shift at all.
const UTC_SHIFT_SECS: f64 = 0.0;

/// The prefix an offset-prefixed zone identifier carries: "an offset-prefixed identifier (for
/// example, `UTC+05:30`)" (`WallClockWindow$Timezone`).
const OFFSET_PREFIX: &str = "UTC";

/// The step an offset-stated zone's offset has to be a multiple of: "The offset must be aligned
/// to a multiple of 5 minutes." (`WallClockWindow$Timezone`), in seconds.
const OFFSET_STEP_SECS: i32 = 5 * 60;

/// How wide a WEEK-long period is, the widest an aligned window may carry.
const WEEK_SECS: f64 = 604_800.0;

/// How far the start of a CALENDAR WEEK sits from the epoch's own multiples of a week.
///
/// An aligned range begins at "fixed clock boundaries that match the alarm's period, such as the
/// top of the hour, midnight, or the start of the calendar week" (`WallClockWindow`), so a
/// week-wide period begins where a calendar week does: at midnight on Sunday, the day a calendar
/// week starts on. The epoch itself fell on a Thursday (1 January 1970), so ITS multiples of a
/// week are Thursdays — four days past the Sunday the calendar week containing the epoch began
/// at, which is how far the grid has to move to lay its boundaries on calendar weeks.
///
/// Every shorter period an aligned window may carry divides a day, so for those the multiples of
/// the period already fall on the clock boundary that period repeats at — the top of the minute,
/// the five-minute mark, the top of the hour, midnight — and nothing is moved.
const CALENDAR_WEEK_SHIFT_SECS: f64 = 4.0 * 86_400.0;

/// The grid of period windows ONE alarm's evaluation grades its series over.
///
/// Two things decide which periods an evaluation covers, and both of them come from the alarm's
/// `EvaluationWindow`: where the boundaries between the periods fall, and whether the period
/// the clock is in may be graded before that period has closed. Everything else about the
/// evaluation is the same for either window — the statistic over each period, the m-of-n
/// predicate, the missing-data treatment and the explanation — so this is the whole of the
/// difference between them.
#[derive(Debug, Clone, Copy)]
pub struct EvaluationGrid {
    /// How far ahead of UTC the zone an aligned window is read in runs; nothing for a rolling
    /// window, whose boundaries are the ones a metric is aggregated into
    /// ([`Self::boundary_shift_secs`]).
    shift_secs: f64,
    /// Whether only a period that has CLOSED is graded.
    closed_only: bool,
}

impl EvaluationGrid {
    /// The grid `alarm`'s evaluation grades its periods on, as the clock stands at `now_secs`.
    ///
    /// An alarm whose `EvaluationWindow` is a wall clock window is graded on the boundaries of
    /// the zone that window names; every other alarm — one that names a sliding window, and one
    /// that names no evaluation window at all, which "uses a `SlidingWindow` by default"
    /// (`EvaluationWindow`) — is graded on the rolling grid.
    #[must_use]
    pub fn of(alarm: &MetricAlarm, now_secs: f64) -> Self {
        match alarm.evaluation_window.as_ref() {
            Some(EvaluationWindow::WallClockWindow(window)) => Self {
                shift_secs: zone_shift_secs(window.timezone.as_deref(), now_secs),
                closed_only: true,
            },
            _ => Self::rolling(),
        }
    }

    /// The grid an alarm with no aligned window is graded on: the rolling one, whose boundaries
    /// are the multiples of the period a metric is itself aggregated into and whose newest
    /// period is graded while it is still open.
    #[must_use]
    pub fn rolling() -> Self {
        Self {
            shift_secs: UTC_SHIFT_SECS,
            closed_only: false,
        }
    }

    /// How far this grid's boundaries are shifted from the epoch's own multiples of `period`.
    ///
    /// An ALIGNED grid's boundaries are the clock's own in the zone the window names, which moves
    /// them by how far ahead of UTC that zone runs, and the ones of a week-wide period are the
    /// starts of calendar weeks rather than the epoch's own Thursdays
    /// ([`CALENDAR_WEEK_SHIFT_SECS`]).
    ///
    /// A ROLLING grid is not aligned to the calendar at all: its boundaries are the ones a metric
    /// is itself aggregated into — "CloudWatch aggregates all data points with time stamps that
    /// fall within each one-hour period", the multiples of the period this store's metric readers
    /// fold a series at — so nothing moves them, whatever the width of the period.
    fn boundary_shift_secs(self, period: f64) -> f64 {
        if !self.closed_only {
            return UTC_SHIFT_SECS;
        }
        if period == WEEK_SECS {
            return self.shift_secs + CALENDAR_WEEK_SHIFT_SECS;
        }
        self.shift_secs
    }

    /// Which of this grid's periods `instant` falls in, as a count of whole periods.
    fn period_index(self, instant: f64, period: f64) -> f64 {
        ((instant + self.boundary_shift_secs(period)) / period).floor()
    }

    /// The instant the `index`th period of this grid begins at.
    ///
    /// The periods are the ones this store's readers fold a series into, so where one begins is
    /// the shared definition ([`crate::state::window_start`]) measured from the boundary this
    /// grid's zone puts at the epoch: one definition of a period's start serves the alarm
    /// evaluator and the metric readers.
    fn period_start(self, index: f64, period: f64) -> f64 {
        #[allow(clippy::cast_possible_truncation)]
        let index = index as i64;
        crate::state::window_start(-self.boundary_shift_secs(period), index, period)
    }

    /// The newest period of this grid an evaluation running at `now_secs` GRADES, as a count of
    /// whole periods.
    ///
    /// On the ROLLING grid that is the period the end of the evaluation's rolling range falls in
    /// ([`super::evaluation_lag_secs`]): the period the clock is in once that period has been
    /// open for a whole evaluation cadence, and the period before it until then.
    ///
    /// On an ALIGNED grid it is the newest period that has CLOSED — the one before the period
    /// the clock is in — because an aligned window "aligns the evaluated range to fixed clock
    /// boundaries that match the alarm's period" (`WallClockWindow`): the range starts at the
    /// boundary the clock has last passed and is not evaluated before the next boundary, so the
    /// verdict over a period is reported only once every datapoint of it could have been
    /// published.
    #[must_use]
    pub fn newest_graded_period(self, now_secs: f64, period: f64) -> f64 {
        if self.closed_only {
            self.period_index(now_secs, period) - 1.0
        } else {
            self.period_index(now_secs - super::evaluation_lag_secs(period), period)
        }
    }

    /// The instant the range an evaluation running at `now_secs` grades ENDS at, the range being
    /// half-open on it: the end of the newest period that evaluation grades.
    #[must_use]
    pub fn range_end(self, now_secs: f64, period: f64) -> f64 {
        self.period_start(self.newest_graded_period(now_secs, period) + 1.0, period)
    }

    /// The instant the `index`th period back from the newest one an evaluation running at
    /// `now_secs` grades BEGINS at, which is the time stamp a datapoint aggregated out of that
    /// period carries.
    #[must_use]
    pub fn window_start(self, now_secs: f64, index: usize, period: f64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let periods_back = index as f64;
        self.period_start(
            self.newest_graded_period(now_secs, period) - periods_back,
            period,
        )
    }

    /// Which of this grid's periods `instant` falls in, counted BACK from the newest period an
    /// evaluation running at `now_secs` grades; `None` for an instant in a period newer than
    /// that one, which a later evaluation grades and this one does not.
    #[must_use]
    pub fn periods_back(self, now_secs: f64, instant: f64, period: f64) -> Option<usize> {
        let periods_back =
            self.newest_graded_period(now_secs, period) - self.period_index(instant, period);
        if periods_back < 0.0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let periods_back = periods_back as usize;
        Some(periods_back)
    }

    /// How long the end of the range an evaluation running at `now_secs` grades LAGS that
    /// instant, for periods `period` seconds wide — the span the evaluation range has to reach
    /// back over on top of the look-back ([`super::evaluation_range_periods`]).
    ///
    /// On the rolling grid that is one evaluation cadence
    /// ([`super::evaluation_lag_secs`]). On an aligned grid it is however long the period the
    /// clock is in has been open, since nothing of that period is graded yet: nothing at the
    /// boundary, and almost a whole period just before the next one.
    #[must_use]
    pub fn lag_secs(self, now_secs: f64, period: f64) -> f64 {
        if self.closed_only {
            (now_secs - self.range_end(now_secs, period)).max(0.0)
        } else {
            super::evaluation_lag_secs(period)
        }
    }
}

/// The zone an ALIGNED window lays its boundaries on the clock of.
///
/// "You can specify an IANA time zone name (for example, `America/New_York`), a fixed UTC offset
/// (for example, `+05:30`), or an offset-prefixed identifier (for example, `UTC+05:30`). The
/// offset must be aligned to a multiple of 5 minutes. If you don't specify a time zone,
/// CloudWatch uses `UTC`." (`WallClockWindow$Timezone`) — the forms a window may name its zone
/// in, read here once: the write that configures an alarm refuses a value in none of them
/// ([`crate::ops::put_metric_alarm`]) and the evaluation lays the alarm's boundaries on the one
/// it named, so the two cannot disagree about what counts as a zone.
#[derive(Debug, Clone, Copy)]
pub enum AlignmentZone {
    /// The clock a window naming no zone is aligned on, which is this store's own.
    Utc,
    /// A zone named in the IANA database, whose offset that database decides — a zone observing
    /// daylight saving does not keep one offset all year.
    Named(Tz),
    /// A zone stated as a fixed offset ahead of UTC, in seconds.
    Offset(f64),
}

impl AlignmentZone {
    /// The zone `timezone` names, or `None` for a value in none of the documented forms — which
    /// is no zone at all, so no clock boundary can be resolved for it.
    #[must_use]
    pub fn of(timezone: Option<&str>) -> Option<Self> {
        let Some(named) = timezone.map(str::trim).filter(|named| !named.is_empty()) else {
            return Some(Self::Utc);
        };
        if let Ok(zone) = named.parse::<Tz>() {
            return Some(Self::Named(zone));
        }
        fixed_offset_secs(named).map(Self::Offset)
    }

    /// How far ahead of UTC this zone runs at `now_secs`.
    ///
    /// A named zone's offset is the one the IANA database gives for THAT instant rather than a
    /// fixed figure, so a zone that observes daylight saving aligns its boundaries to the local
    /// clock the evaluation is running under.
    fn shift_secs(self, now_secs: f64) -> f64 {
        match self {
            Self::Utc => UTC_SHIFT_SECS,
            Self::Offset(secs) => secs,
            Self::Named(zone) => named_zone_shift_secs(zone, now_secs),
        }
    }
}

/// How far ahead of UTC the zone an aligned window names runs, at `now_secs`.
///
/// A value in none of the documented forms is refused by the write that configures the alarm, so
/// an alarm this store holds names a zone its boundaries can be laid on; one stored with any
/// other value is aligned on this store's own clock rather than on boundaries nothing resolved.
fn zone_shift_secs(timezone: Option<&str>, now_secs: f64) -> f64 {
    AlignmentZone::of(timezone)
        .unwrap_or(AlignmentZone::Utc)
        .shift_secs(now_secs)
}

/// How far ahead of UTC an IANA zone runs at `now_secs`, which is what the database says for
/// that instant rather than a fixed figure: a zone observing daylight saving runs an hour
/// further ahead over the part of the year it is in.
fn named_zone_shift_secs(zone: Tz, now_secs: f64) -> f64 {
    if !now_secs.is_finite() {
        return UTC_SHIFT_SECS;
    }
    #[allow(clippy::cast_possible_truncation)]
    let Some(at) = DateTime::from_timestamp(now_secs.floor() as i64, 0) else {
        return UTC_SHIFT_SECS;
    };
    f64::from(at.with_timezone(&zone).offset().fix().local_minus_utc())
}

/// How far ahead of UTC a zone stated as an OFFSET runs: `+05:30`, or the same offset prefixed
/// with `UTC`.
///
/// `None` for a value that states no offset at all, and for one "The offset must be aligned to a
/// multiple of 5 minutes" (`WallClockWindow$Timezone`) rules out: an offset no clock boundary of
/// the alarm's period repeats at is not a zone an aligned range can begin in.
fn fixed_offset_secs(named: &str) -> Option<f64> {
    let offset = named.strip_prefix(OFFSET_PREFIX).unwrap_or(named);
    let secs = offset.parse::<FixedOffset>().ok()?.local_minus_utc();
    (secs % OFFSET_STEP_SECS == 0).then(|| f64::from(secs))
}
