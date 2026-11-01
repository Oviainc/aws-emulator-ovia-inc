// SPDX-License-Identifier: CC0-1.0

//! WHEN a rotation's shifts begin and end, and WHO is on call during each.
//!
//! A rotation is not a stored list of shifts: it is a recurrence, a shift team in rotation
//! order and a time zone, and the shifts are COMPUTED from those three whenever somebody
//! asks for a range of them. This module is that computation, placed beside this service's
//! other recurrence and time-zone rules rather than inside one handler, because it is a fact
//! about this service's rotations rather than about one operation: the preview answers it for
//! a rotation the caller only describes, and `ListRotationShifts` — still a scaffold, with
//! its own issue — asks the same question of a [`super::RotationRecord`]'s own fields.
//!
//! The rules it implements, each from the modelled members it reads
//! (`smithy_query(query="closure", operation="ListPreviewRotationShifts")`):
//!
//! * A HAND-OFF is when one shift ends and the next begins. `RecurrenceSettings` says when
//!   they fall: `DailySettings` is a time of day, `WeeklySettings` a day of the week with a
//!   time, `MonthlySettings` a day of the month with a time, and `RecurrenceMultiplier` is
//!   how many of those periods one turn of the recurrence spans. The periods are counted
//!   from the rotation's own start — "the first shift is calculated from this date and time"
//!   — and every hand-off is read in the rotation's IANA zone, so a zone's daylight-saving
//!   jumps move the instant a local hand-off time falls on.
//! * A SHIFT is the span between two consecutive hand-offs, and the contacts on call during
//!   it are `NumberOfOnCalls` of the team, taken in the team's own order and advancing by
//!   that many at every hand-off, so the team is walked round and round in rotation order.
//! * `ShiftCoverages` narrows a shift to the parts of the week the rotation covers: a day of
//!   the week the map does not name is not covered at all, and a day it names is covered
//!   only inside its `CoverageTime` windows. A shift reaching outside them is reported as
//!   the covered pieces of itself.
//! * An override replaces who is on call over a window: the pieces of a shift the window
//!   covers are reported as `OVERRIDDEN`, carrying the override's own members, with the
//!   contacts they displaced in `ShiftDetails$OverriddenContactIds`; every other piece stays
//!   `REGULAR`.
//!
//! Calendar arithmetic is `chrono`'s and the zone database is `chrono-tz`'s — the crate this
//! service already resolves a `TimeZoneId` against — so no date, month length or
//! daylight-saving rule is worked out here.
//!
//! Nothing in this module reads the clock or the store: it is a function of the schedule it
//! is handed, which is what lets the preview compute shifts for a rotation that does not
//! exist. Its work is bounded by the range asked for and, whatever that range is, by ONE step
//! budget ([`MAX_COMPUTATION_STEPS`]) that every loop of the computation spends from — so no
//! ceiling here can be multiplied by the length of a list a request chose.

use ::chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Weekday};
use ::chrono_tz::Tz;

use crate::ops::types::{CoverageTime, HandOffTime, RecurrenceSettings, RotationShift, ShiftDetails};

/// The schedule a shift computation reads: the rotation's team, its recurrence, its zone and
/// the instant it goes into effect.
///
/// Borrowed rather than owned so a handler can hand over what it already has — the request's
/// members and recurrence for a preview, a [`super::RotationRecord`]'s for a stored rotation.
#[derive(Debug)]
pub struct ShiftSchedule<'a> {
    /// The shift team, in the order shifts hand off along it.
    pub members: &'a [String],
    /// When the hand-offs fall and how many of the team are on call at once.
    pub recurrence: &'a RecurrenceSettings,
    /// The zone the recurrence's local times are read in.
    pub time_zone: Tz,
    /// Epoch seconds of the instant the rotation goes into effect; the first hand-off is the
    /// first one the recurrence puts at or after it.
    pub rotation_start: i64,
}

/// A window over which somebody else is on call, as a shift computation reads it.
///
/// Epoch seconds, the start inclusive and the end exclusive, so two overrides that meet at
/// an instant do not both claim it. Either end may be absent — the model makes both optional —
/// and an absent one is open rather than an extreme instant, so no end of this window is ever
/// an operand of any arithmetic.
#[derive(Debug, Clone)]
pub struct ShiftOverride {
    /// Who is on call instead, which the model admits as empty (`length` 0..30).
    pub new_members: Vec<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl ShiftOverride {
    /// Whether this override covers the WHOLE of `[start, end)`, so every instant of that
    /// piece is somebody else's.
    fn covers(&self, start: i64, end: i64) -> bool {
        self.start.is_none_or(|from| from <= start) && self.end.is_none_or(|to| to >= end)
    }

    /// Whether this override covers any instant of `[start, end)` at all.
    fn reaches(&self, start: i64, end: i64) -> bool {
        self.end.is_none_or(|to| to > start) && self.start.is_none_or(|from| from < end)
    }
}

/// `ShiftType$REGULAR` — a shift served by the rotation's own team.
///
/// Spelled from [`crate::ops::types::SHIFT_TYPE_VALUES`].
pub const SHIFT_TYPE_REGULAR: &str = "REGULAR";

/// `ShiftType$OVERRIDDEN` — a shift, or the piece of one, an override took over.
pub const SHIFT_TYPE_OVERRIDDEN: &str = "OVERRIDDEN";

/// How many hand-offs one computation reads at most.
///
/// The model bounds a page at `MaxResults` 1024 and the requested range bounds the walk, so
/// this is the EMULATOR's ceiling on one computation rather than anything AWS publishes: it
/// is what stops a request naming a far-away `EndTime` from making the recurrence walked for
/// ever, and it is far above the hand-offs any page of shifts can report.
pub const MAX_HAND_OFFS: usize = 20_000;

/// How many shifts one computation reports at most, whatever it is asked for.
///
/// The EMULATOR's ceiling, and the reason [`shifts_in_range`] cannot be made to accumulate
/// more than this however large a `limit` it is handed: a caller's `MaxResults` is bounded by
/// the model, but a resume point is not, so the ceiling is applied to the computation itself
/// rather than only to what sizes it. A preview can therefore be paged to this many shifts of
/// one schedule, which is far past the calendar any caller draws.
pub const MAX_SHIFTS: usize = 20_000;

/// How much work one computation does at most, counted in every step it takes: a hand-off
/// candidate computed, a hand-off interval examined, a local day walked for coverage, a
/// coverage window read and a piece an override split.
///
/// Each of those factors has a ceiling of its own, but their PRODUCT is what a request pays
/// for, so it is this ONE budget — spent by every step wherever it is taken — that decides how
/// much a preview can cost. A step is cheap (a date addition and a zone resolution), so the
/// whole budget is a few milliseconds of the blocking thread the handler computes on.
const MAX_COMPUTATION_STEPS: usize = 100_000;

/// The work one computation may still do.
///
/// Threaded through every loop of the computation, rather than counted per loop, so no list a
/// request chose the length of can be multiplied by a ceiling: the walk stops at the step the
/// budget runs out, wherever that step is taken.
#[derive(Debug)]
struct Budget {
    left: usize,
}

impl Budget {
    fn new() -> Self {
        Self {
            left: MAX_COMPUTATION_STEPS,
        }
    }

    /// Charge one step, answering whether there is budget left to take another.
    fn spend(&mut self) -> bool {
        self.left = self.left.saturating_sub(1);
        self.left > 0
    }

    fn exhausted(&self) -> bool {
        self.left == 0
    }
}

/// How many turns of the recurrence one computation walks at most.
///
/// A turn usually yields a hand-off, so this bound is reached only by a recurrence that
/// yields none for many turns — a monthly day of the month the months it lands on do not all
/// have, say — and it keeps that walk finite too.
const MAX_RECURRENCE_PERIODS: u64 = 20_000;

/// How many local days of ONE shift are tested against `ShiftCoverages`.
///
/// A shift spans the gap between two hand-offs, which a monthly recurrence with a multiplier
/// makes months long; this bounds the per-shift day walk without bounding the schedule.
const MAX_COVERAGE_DAYS: u64 = 400;

/// How many instants ONE shift is split at by the overrides that reach it.
///
/// Two per override, and the model bounds the `Overrides` list not at all, so this is the
/// EMULATOR's ceiling on how finely one shift is cut — far more cuts than a preview calendar
/// draws, and it keeps the split of one shift bounded independently of the budget.
const MAX_OVERRIDE_EDGES: usize = 2_048;

/// The shifts `schedule` puts in `[from, to)`, at most `limit` of them, in order of start
/// time.
///
/// A shift is reported when it covers an instant of the range: a shift that merely ends where
/// the range begins, or begins where it ends, covers none of it. The times reported are the
/// shift's OWN, never clipped to the range, so a caller sees the shift it is in the middle of.
///
/// `limit` is what the CALLER's page needs; the computation serves the smaller of it and
/// [`MAX_SHIFTS`], and stops in any case once [`MAX_COMPUTATION_STEPS`] of work are done, so
/// neither the shifts accumulated nor the walk that finds them is sized by the request.
pub fn shifts_in_range(
    schedule: &ShiftSchedule<'_>,
    overrides: &[ShiftOverride],
    from: i64,
    to: i64,
    limit: usize,
) -> Vec<RotationShift> {
    let mut shifts: Vec<RotationShift> = Vec::new();
    let limit = limit.min(MAX_SHIFTS);
    if limit == 0 || to <= from {
        return shifts;
    }

    let mut budget = Budget::new();
    let hand_offs = hand_off_instants(schedule, to, &mut budget);
    let on_calls = on_call_count(schedule);

    for (index, boundaries) in hand_offs.windows(2).enumerate() {
        let &[start, end] = boundaries else { continue };
        if !budget.spend() {
            break;
        }
        if end <= from {
            continue;
        }
        if start >= to {
            break;
        }
        let team = on_call_members(schedule.members, index, on_calls);
        for (covered_start, covered_end) in covered_segments(schedule, start, end, &mut budget) {
            if covered_end <= from || covered_start >= to {
                continue;
            }
            for shift in overridden_pieces(covered_start, covered_end, &team, overrides, &mut budget)
            {
                shifts.push(shift);
                if shifts.len() >= limit || !budget.spend() {
                    return shifts;
                }
            }
        }
    }

    shifts
}

/// Every hand-off instant the recurrence puts at or after the rotation's start, up to the
/// first one past `until`, in increasing order and without repeats.
///
/// One past `until` is deliberate: it is what closes the last shift of the requested range,
/// which would otherwise have no end.
///
/// Every candidate instant the recurrence's settings lists yield spends a step of `budget`, so
/// the length of those lists cannot multiply the number of turns walked.
fn hand_off_instants(schedule: &ShiftSchedule<'_>, until: i64, budget: &mut Budget) -> Vec<i64> {
    let Some(anchor) = schedule
        .time_zone
        .timestamp_opt(schedule.rotation_start, 0)
        .single()
    else {
        return Vec::new();
    };
    let anchor_date = anchor.date_naive();
    // The model bounds `RecurrenceMultiplier` to 1..100, so this only guards a value the
    // boundary validator cannot have passed.
    let multiplier = u64::try_from(schedule.recurrence.recurrence_multiplier).unwrap_or(1).max(1);

    let mut instants: Vec<i64> = Vec::new();
    let mut period: u64 = 0;
    while period < MAX_RECURRENCE_PERIODS && instants.len() < MAX_HAND_OFFS && !budget.exhausted()
    {
        let Some(of_period) = period_hand_offs(schedule, anchor_date, period, multiplier, budget)
        else {
            // The calendar cannot represent this turn of the recurrence, so neither can any
            // later one.
            break;
        };
        let past_the_range = !of_period.is_empty() && of_period.iter().all(|at| *at > until);
        instants.extend(
            of_period
                .into_iter()
                .filter(|at| *at >= schedule.rotation_start),
        );
        if past_the_range {
            break;
        }
        period = period.saturating_add(1);
    }

    instants.sort_unstable();
    instants.dedup();
    instants.truncate(MAX_HAND_OFFS);
    instants
}

/// The hand-offs of ONE turn of the recurrence, `period` turns after the rotation's start, or
/// `None` when the calendar cannot represent that turn.
///
/// Each settings list is anchored on the rotation's own start: the daily one on its date, the
/// weekly one on the Monday of its week, the monthly one on the first of its month. A monthly
/// day of the month the turn's month does not have yields nothing for that turn, exactly as a
/// 31st does in a 30-day month.
///
/// Every entry read out of a settings list spends a step of `budget`, and a turn stops where
/// the budget does.
fn period_hand_offs(
    schedule: &ShiftSchedule<'_>,
    anchor_date: NaiveDate,
    period: u64,
    multiplier: u64,
    budget: &mut Budget,
) -> Option<Vec<i64>> {
    let zone = &schedule.time_zone;
    let recurrence = schedule.recurrence;
    let step = period.checked_mul(multiplier)?;
    let mut instants: Vec<i64> = Vec::new();
    let mut representable = false;

    if let Some(settings) = recurrence.daily_settings.as_ref() {
        if let Some(date) = anchor_date.checked_add_days(Days::new(step)) {
            representable = true;
            for hand_off in settings {
                if !budget.spend() {
                    return Some(instants);
                }
                instants.extend(hand_off_instant(zone, date, hand_off));
            }
        }
    }

    if let Some(settings) = recurrence.weekly_settings.as_ref() {
        let week_step = step.checked_mul(DAYS_PER_WEEK)?;
        let week_start = anchor_date
            .checked_sub_days(Days::new(u64::from(
                anchor_date.weekday().num_days_from_monday(),
            )))
            .and_then(|monday| monday.checked_add_days(Days::new(week_step)));
        if let Some(week_start) = week_start {
            representable = true;
            for setting in settings {
                if !budget.spend() {
                    return Some(instants);
                }
                let Some(weekday) = weekday_of(&setting.day_of_week) else {
                    continue;
                };
                let Some(date) = week_start
                    .checked_add_days(Days::new(u64::from(weekday.num_days_from_monday())))
                else {
                    continue;
                };
                instants.extend(hand_off_instant(zone, date, &setting.hand_off_time));
            }
        }
    }

    if let Some(settings) = recurrence.monthly_settings.as_ref() {
        let month_step = u32::try_from(step).ok()?;
        let month_start = NaiveDate::from_ymd_opt(anchor_date.year(), anchor_date.month(), 1)
            .and_then(|first| first.checked_add_months(Months::new(month_step)));
        if let Some(month_start) = month_start {
            representable = true;
            for setting in settings {
                if !budget.spend() {
                    return Some(instants);
                }
                let Some(day_of_month) = u32::try_from(setting.day_of_month).ok() else {
                    continue;
                };
                let Some(date) =
                    NaiveDate::from_ymd_opt(month_start.year(), month_start.month(), day_of_month)
                else {
                    // The turn's month has no such day, so this setting hands off in another.
                    continue;
                };
                instants.extend(hand_off_instant(zone, date, &setting.hand_off_time));
            }
        }
    }

    representable.then_some(instants)
}

/// Days in a week, the unit `RecurrenceMultiplier` multiplies for a weekly recurrence.
const DAYS_PER_WEEK: u64 = 7;

/// The instant a local `HandOffTime` on `date` falls on in `zone`.
fn hand_off_instant(zone: &Tz, date: NaiveDate, hand_off: &HandOffTime) -> Option<i64> {
    local_instant(zone, date.and_time(local_time(hand_off)?))
}

/// A `HandOffTime` as a local time of day.
///
/// The model bounds `HourOfDay` to 0..23 and `MinuteOfHour` to 0..59, so a value outside
/// either is one the boundary validator cannot have passed, and it yields no hand-off rather
/// than a panic.
fn local_time(hand_off: &HandOffTime) -> Option<NaiveTime> {
    NaiveTime::from_hms_opt(
        u32::try_from(hand_off.hour_of_day).ok()?,
        u32::try_from(hand_off.minute_of_hour).ok()?,
        0,
    )
}

/// The instant a local reading falls on in `zone`, as epoch seconds.
///
/// A local reading a daylight-saving jump BACK gives twice is read at the earlier of the two,
/// and one a jump FORWARD skips altogether — a local time that does not occur in that zone —
/// is read at the first instant the zone reaches after the jump.
fn local_instant(zone: &Tz, local: NaiveDateTime) -> Option<i64> {
    if let Some(at) = zone.from_local_datetime(&local).earliest() {
        return Some(at.timestamp());
    }
    let after_the_jump = local.checked_add_signed(TimeDelta::try_hours(1)?)?;
    zone.from_local_datetime(&after_the_jump)
        .earliest()
        .map(|at| at.timestamp())
}

/// A modelled `DayOfWeek` as a calendar weekday, or `None` when it names none.
///
/// The names are read by `chrono`'s own weekday parse rather than matched here, so this
/// service keeps no second table of the days of the week; the values the member admits are
/// [`crate::ops::types::DAY_OF_WEEK_VALUES`].
fn weekday_of(day_of_week: &str) -> Option<Weekday> {
    day_of_week.parse::<Weekday>().ok()
}

/// How many of the team are on call at once: the recurrence's `NumberOfOnCalls`, which cannot
/// exceed the team it is taken from.
///
/// The REFUSAL of a `NumberOfOnCalls` the team cannot meet belongs to the operation boundary
/// ([`crate::state::check_number_of_on_calls`], which every operation that installs or
/// previews a recurrence reads its request through); by the time a computation runs the value
/// has been admitted, so this is the guard that keeps the walk total rather than a second
/// reading of the rule.
fn on_call_count(schedule: &ShiftSchedule<'_>) -> usize {
    let requested = usize::try_from(schedule.recurrence.number_of_on_calls).unwrap_or(1);
    requested.clamp(1, schedule.members.len().max(1))
}

/// The contacts on call during the shift that follows hand-off number `shift_index`.
///
/// The team is walked in its own order, advancing by `on_calls` at every hand-off and wrapping
/// at the end, so a team of three handing off one at a time is on call in the order it was
/// given, over and over.
fn on_call_members(members: &[String], shift_index: usize, on_calls: usize) -> Vec<String> {
    if members.is_empty() || on_calls == 0 {
        return Vec::new();
    }
    let team_size = members.len();
    // `(i * k) mod n` without the multiplication overflowing for a far-away shift index:
    // reducing the index first leaves the same remainder.
    let first = (shift_index % team_size).saturating_mul(on_calls) % team_size;
    (0..on_calls)
        .filter_map(|offset| members.get((first.saturating_add(offset)) % team_size).cloned())
        .collect()
}

/// The pieces of `[start, end)` the recurrence's `ShiftCoverages` cover.
///
/// A recurrence that names no coverage at all covers the whole shift; one that does covers
/// only the windows it names, on the days of the week it names, so an uncovered day
/// contributes nothing.
///
/// `steps` is the computation's shared work budget: every local day this walk reads and every
/// coverage window it builds spends a step of it, so neither the days-per-shift ceiling nor the
/// length of a `CoverageTimes` list can be multiplied by the hand-off ceiling.
fn covered_segments(
    schedule: &ShiftSchedule<'_>,
    start: i64,
    end: i64,
    budget: &mut Budget,
) -> Vec<(i64, i64)> {
    let Some(coverages) = schedule
        .recurrence
        .shift_coverages
        .as_ref()
        .filter(|coverages| !coverages.is_empty())
    else {
        return vec![(start, end)];
    };

    let zone = &schedule.time_zone;
    let (Some(first_day), Some(last_day)) = (
        zone.timestamp_opt(start, 0).single().map(|at| at.date_naive()),
        zone.timestamp_opt(end, 0).single().map(|at| at.date_naive()),
    ) else {
        return Vec::new();
    };

    let mut windows: Vec<(i64, i64)> = Vec::new();
    // The walk starts the local day BEFORE the shift's first one: a coverage window of the
    // previous day that runs past midnight covers the beginning of this shift.
    let mut day = first_day.checked_sub_days(Days::new(1)).unwrap_or(first_day);
    for _ in 0..=MAX_COVERAGE_DAYS {
        if day > last_day {
            break;
        }
        if !budget.spend() {
            break;
        }
        for times in coverages
            .iter()
            .filter(|(key, _)| weekday_of(key) == Some(day.weekday()))
            .map(|(_, times)| times)
        {
            for time in times {
                if !budget.spend() {
                    break;
                }
                windows.extend(coverage_window(zone, day, time));
            }
        }
        let Some(next_day) = day.checked_add_days(Days::new(1)) else {
            break;
        };
        day = next_day;
    }

    merged(windows)
        .into_iter()
        .filter_map(|(window_start, window_end)| {
            let covered = (window_start.max(start), window_end.min(end));
            (covered.0 < covered.1).then_some(covered)
        })
        .collect()
}

/// One `CoverageTime` on `day`, as an instant window.
///
/// An absent `Start` covers from the start of the local day and an absent `End` to the end of
/// it. An `End` that does not follow its `Start` closes on the NEXT day, which is what an
/// overnight window says.
fn coverage_window(zone: &Tz, day: NaiveDate, time: &CoverageTime) -> Option<(i64, i64)> {
    let start_time = match time.start.as_ref() {
        Some(hand_off) => local_time(hand_off)?,
        None => NaiveTime::MIN,
    };
    let start = local_instant(zone, day.and_time(start_time))?;
    let next_day = day.checked_add_days(Days::new(1))?;
    let end = match time.end.as_ref() {
        Some(hand_off) => {
            let end_time = local_time(hand_off)?;
            if end_time > start_time {
                local_instant(zone, day.and_time(end_time))?
            } else {
                local_instant(zone, next_day.and_time(end_time))?
            }
        }
        None => local_instant(zone, next_day.and_time(NaiveTime::MIN))?,
    };
    (end > start).then_some((start, end))
}

/// `windows` in increasing order, with overlapping and touching ones joined, so two coverage
/// times that overlap report ONE covered span rather than two.
fn merged(mut windows: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    windows.sort_unstable();
    let mut joined: Vec<(i64, i64)> = Vec::new();
    for (start, end) in windows {
        match joined.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => joined.push((start, end)),
        }
    }
    joined
}

/// `[start, end)` split where the overrides begin and end, each piece reported as the shift it
/// is: `OVERRIDDEN` with the override's members and the team it displaced where an override
/// covers it, `REGULAR` with the team everywhere else.
///
/// With no override this is the one shift, which is why every shift goes through here.
///
/// Every override examined, every piece cut and every covering test spends a step of `budget`,
/// so the length of the `Overrides` list a request carries cannot be multiplied by the number
/// of shifts walked; the splits of one shift are capped by [`MAX_OVERRIDE_EDGES`] besides.
fn overridden_pieces(
    start: i64,
    end: i64,
    team: &[String],
    overrides: &[ShiftOverride],
    budget: &mut Budget,
) -> Vec<RotationShift> {
    // The overrides that reach this shift at all, kept so the pieces below are tested against
    // those rather than against the whole list again.
    let mut reaching: Vec<&ShiftOverride> = Vec::new();
    let mut edges: Vec<i64> = vec![start, end];
    for applies in overrides {
        if !budget.spend() {
            break;
        }
        if !applies.reaches(start, end) {
            continue;
        }
        reaching.push(applies);
        if edges.len() >= MAX_OVERRIDE_EDGES {
            continue;
        }
        if let Some(from) = applies.start.filter(|from| *from > start) {
            edges.push(from);
        }
        if let Some(to) = applies.end.filter(|to| *to < end) {
            edges.push(to);
        }
    }
    edges.sort_unstable();
    edges.dedup();

    let mut pieces: Vec<RotationShift> = Vec::new();
    for edge_pair in edges.windows(2) {
        let &[piece_start, piece_end] = edge_pair else {
            continue;
        };
        if !budget.spend() {
            break;
        }
        let mut covering: Option<&ShiftOverride> = None;
        for applies in &reaching {
            if !budget.spend() {
                break;
            }
            if applies.covers(piece_start, piece_end) {
                covering = Some(applies);
                break;
            }
        }
        pieces.push(match covering {
            Some(applies) => RotationShift {
                contact_ids: Some(applies.new_members.clone()),
                start_time: ::aws_smithy_types::DateTime::from_secs(piece_start),
                end_time: ::aws_smithy_types::DateTime::from_secs(piece_end),
                r#type: Some(SHIFT_TYPE_OVERRIDDEN.to_string()),
                shift_details: Some(ShiftDetails {
                    overridden_contact_ids: team.to_vec(),
                }),
            },
            None => RotationShift {
                contact_ids: Some(team.to_vec()),
                start_time: ::aws_smithy_types::DateTime::from_secs(piece_start),
                end_time: ::aws_smithy_types::DateTime::from_secs(piece_end),
                r#type: Some(SHIFT_TYPE_REGULAR.to_string()),
                shift_details: None,
            },
        });
    }
    pieces
}
