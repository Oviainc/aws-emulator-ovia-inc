// SPDX-License-Identifier: CC0-1.0

//! # Instance event windows
//!
//! [`ComAmazonawsEc2Amazonec2State::instance_event_windows`] holds the account's event
//! windows as [`InstanceEventWindowRecord`]s: the modelled [`InstanceEventWindow`] every
//! operation that reports one answers with, plus the instant the clock owes the window its
//! `creating` -> `active` move — the one member of the view that is a fact of the window's
//! age rather than of the request, so [`InstanceEventWindowRecord::view_at`] derives it on
//! read instead of letting two writers store two different words for it, exactly as
//! [`VolumeRecord`] does for a volume. Keyed by `InstanceEventWindowId` in a
//! [`std::collections::BTreeMap`], the same stable order the VPC, subnet and volume stores
//! are kept in, so a pagination token can name the id the next page starts at.
//!
//! What a window may say about WHEN it opens is a rule rather than a stored shape, and the
//! create and the modify of a window share it, so it lives here:
//! [`validate_event_window_cron_expression`] and [`validate_event_window_time_ranges`] are
//! the one reading of the constraints `CreateInstanceEventWindowRequest$CronExpression`
//! documents.
//!
//! A child module of [`crate::state`] rather than more lines in it: everything here belongs
//! to one resource and reaches the rest of the store through `use super::*`, so the state
//! module keeps only the field the windows live in and the writers that mint one.

use std::time::Instant;

use crate::ops::types::{
    InstanceEventWindow, InstanceEventWindowTimeRange, InstanceEventWindowTimeRangeRequest,
};

use super::*;

/// The id prefix an event window is minted with.
///
/// The model publishes no pattern for `InstanceEventWindow$InstanceEventWindowId` (it is a
/// plain `String` — `smithy_query(query="shape",shape="InstanceEventWindowId")`), so the id
/// takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with the `iew` prefix EC2 gives an event
/// window. It is minted by the store and never taken from a request.
pub const INSTANCE_EVENT_WINDOW_ID_PREFIX: &str = "iew";

/// The state an event window is created in: the `InstanceEventWindowState` member
/// `creating` (`crate::ops::types::INSTANCE_EVENT_WINDOW_STATE_VALUES`).
pub const INSTANCE_EVENT_WINDOW_STATE_CREATING: &str = "creating";

/// The state the clock moves a newly created event window to: the
/// `InstanceEventWindowState` member `active`.
pub const INSTANCE_EVENT_WINDOW_STATE_ACTIVE: &str = "active";

/// `instance-event-window`, the `ResourceType` member that names an event window — the one
/// resource `CreateInstanceEventWindow`'s `TagSpecifications` may tag.
pub const RESOURCE_TYPE_INSTANCE_EVENT_WINDOW: &str = "instance-event-window";

/// How many event windows this store holds: "You can create up to 200 event windows per
/// Amazon Web Services Region" (`CreateInstanceEventWindow`).
pub const MAX_INSTANCE_EVENT_WINDOWS: usize = 200;

/// EC2's code for a create that would take the account past
/// [`MAX_INSTANCE_EVENT_WINDOWS`], in the `<Resource>LimitExceeded` form every other EC2
/// quota code takes ([`VOLUME_LIMIT_EXCEEDED`], [`LAUNCH_TEMPLATE_LIMIT_EXCEEDED`]).
pub const INSTANCE_EVENT_WINDOW_LIMIT_EXCEEDED: &str = "InstanceEventWindowLimitExceeded";

/// EC2's code for a request that named an event window id the account holds no window for.
pub const INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND: &str =
    "InvalidInstanceEventWindowId.NotFound";

/// EC2's code for a request that named something that is not an event window id at all.
pub const INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED: &str =
    "InvalidInstanceEventWindowId.Malformed";

/// The days of the week in CALENDAR order, each as the model's `WeekDay` enum spells it
/// (`crate::ops::types::WEEK_DAY_VALUES`, which lists the same seven members
/// alphabetically) paired with the three-letter alternative a cron day-of-week value may
/// use: "you can specify either integers 0 through 6, or alternative single values SUN
/// through SAT" (`CreateInstanceEventWindowRequest$CronExpression`).
///
/// A day's POSITION here is its cron day-of-week integer, and the hour-of-the-week
/// arithmetic a time range's span is measured in counts from the same position, so one
/// ordering serves both.
pub const WEEK_DAYS_IN_ORDER: [(&str, &str); 7] = [
    ("sunday", "SUN"),
    ("monday", "MON"),
    ("tuesday", "TUE"),
    ("wednesday", "WED"),
    ("thursday", "THU"),
    ("friday", "FRI"),
    ("saturday", "SAT"),
];

/// Hours in a day, the unit a time range's week day and hour are combined into.
pub const HOURS_PER_DAY: i64 = 24;

/// Hours in a week: a time range may run from one week day to another, including across
/// the end of the week, so its span is measured modulo this.
pub const HOURS_PER_WEEK: i64 = 7 * HOURS_PER_DAY;

/// The smallest span one range of an event window may cover: "Each hour range must be >= 2
/// hours, for example, 0-2 or 20-23" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const MIN_EVENT_WINDOW_RANGE_HOURS: i64 = 2;

/// The smallest span an event window's ranges may cover between them: "The event window
/// must be >= 4 hours. The combined total time ranges in the event window must be >= 4
/// hours" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const MIN_EVENT_WINDOW_TOTAL_HOURS: i64 = 4;

/// How many time ranges one event window may carry.
///
/// A STORE-PROTECTION ceiling this emulator chose, not a modelled `@length` and not an
/// AWS-published limit: the model declares no length on
/// `InstanceEventWindowTimeRangeRequestSet`, so without this one request could hand the
/// store a list as long as the request body allows and keep it for the process's lifetime.
/// A window's ranges live in ONE week and each covers at least
/// [`MIN_EVENT_WINDOW_RANGE_HOURS`], so the ceiling is how many ranges of the smallest span
/// a week holds — past it a range can only repeat a time another already covers (ranges may
/// overlap, so that is an argument about usefulness, not about validity).
/// [`validate_event_window_time_ranges`] refuses a longer list before it allocates
/// anything, the way the describe operations refuse more ids than [`MAX_RESOURCE_IDS`].
pub const MAX_EVENT_WINDOW_TIME_RANGES: usize = (HOURS_PER_WEEK / MIN_EVENT_WINDOW_RANGE_HOURS)
    as usize;

/// How long a cron expression this service reads may be.
///
/// The same kind of store-protection ceiling as [`MAX_EVENT_WINDOW_TIME_RANGES`], and
/// refused BEFORE the expression is split into fields, so neither the work nor the
/// allocation of reading it is proportional to a length the caller chose freely. The
/// longest expression the documented grammar admits is far shorter: the hour field holds at
/// most [`HOURS_PER_DAY`] / [`MIN_EVENT_WINDOW_RANGE_HOURS`] ranges of five characters, the
/// day-of-the-week field at most seven three-character days, and the other three fields are
/// one character each.
pub const MAX_EVENT_WINDOW_CRON_BYTES: usize = 256;

/// How many fields a cron expression carries: minute, hour, day of the month, month and
/// day of the week — as the example `* 0-4,20-23 * * 1,5`
/// (`CreateInstanceEventWindowRequest$CronExpression`) has them.
pub const CRON_FIELDS: usize = 5;

/// One stored instance event window: the modelled [`InstanceEventWindow`] every operation
/// that reports one answers with, plus the instant the clock owes it its `creating` ->
/// `active` move.
///
/// The view holds no `State`: the state is a fact of the window's age, so
/// [`InstanceEventWindowRecord::view_at`] derives it rather than letting two writers store
/// two different words for it — the same shape [`VolumeRecord`] has.
#[derive(Debug, Clone)]
pub struct InstanceEventWindowRecord {
    /// The window as the model shapes it, with `State` left for `view_at` to fill.
    pub view: InstanceEventWindow,
    /// The instant from which this window's `creating` -> `active` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and
    /// never reported, so a wall-clock adjustment cannot move a window back to `creating`.
    pub active_at: Instant,
}

impl InstanceEventWindowRecord {
    /// This window as of `now`: the stored view with the `State` the clock has reached. A
    /// read observes the transition the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> InstanceEventWindow {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The window's `InstanceEventWindowState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.active_at {
            INSTANCE_EVENT_WINDOW_STATE_CREATING
        } else {
            INSTANCE_EVENT_WINDOW_STATE_ACTIVE
        }
    }
}

/// What one stored event window is created with, beside the id the store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for a subnet: the
/// operation that creates a window fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_instance_event_window`] turns it into the
/// modelled [`InstanceEventWindow`] the describe reports, so the id, the stored key and the
/// byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct InstanceEventWindowSpec {
    /// "The name of the event window" (`CreateInstanceEventWindowRequest$Name`), or `None`
    /// for a window created without one.
    pub name: Option<String>,
    /// The cron expression the window opens on, for a window defined by one.
    pub cron_expression: Option<String>,
    /// The time ranges the window opens in, for a window defined by them.
    pub time_ranges: Option<Vec<InstanceEventWindowTimeRange>>,
    /// The tags on the window.
    pub tags: Vec<Tag>,
    /// When the window's `creating` -> `active` move falls due.
    pub active_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another event window fits inside the store's ceiling
    /// ([`MAX_INSTANCE_EVENT_WINDOWS`]).
    ///
    /// A create must ask this BEFORE inserting and refuse with
    /// [`instance_event_window_limit_exceeded`] when it answers `false`, the way
    /// [`Self::has_vpc_capacity`] gates a VPC.
    pub fn has_instance_event_window_capacity(&self) -> bool {
        self.instance_event_windows.len() < MAX_INSTANCE_EVENT_WINDOWS
    }

    /// Store one newly created event window and answer the `InstanceEventWindowId` it was
    /// given, or `None` when the account already holds [`MAX_INSTANCE_EVENT_WINDOWS`] of
    /// them and NOTHING was stored.
    ///
    /// The id is minted here rather than taken from `spec`, the way [`Self::insert_subnet`]
    /// mints a `SubnetId`, and the caller-chosen bytes the record holds are charged against
    /// the store's byte budget here, so no path can store them uncharged. The count ceiling
    /// is asked HERE as well as by the create handler (which answers
    /// [`instance_event_window_limit_exceeded`] for it), so a writer that forgets the gate
    /// cannot grow the store either — the shape
    /// [`Self::insert_vpc_endpoint_connection`] has.
    pub fn insert_instance_event_window(
        &mut self,
        spec: InstanceEventWindowSpec,
    ) -> Option<String> {
        if !self.has_instance_event_window_capacity() {
            return None;
        }
        let instance_event_window_id = self.next_id(INSTANCE_EVENT_WINDOW_ID_PREFIX);
        let view = InstanceEventWindow {
            instance_event_window_id: Some(instance_event_window_id.clone()),
            name: spec.name,
            cron_expression: spec.cron_expression,
            time_ranges: spec.time_ranges,
            // "When you create the event window, targets (instance IDs, Dedicated Host IDs,
            // or tags) are not yet associated with it" (`CreateInstanceEventWindow`), so a
            // fresh window targets nothing; `AssociateInstanceEventWindow` is the operation
            // that gives it a target.
            association_target: None,
            // Derived from `active_at` on every read.
            state: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(instance_event_window_payload_bytes(&view));
        self.instance_event_windows.insert(
            instance_event_window_id.clone(),
            InstanceEventWindowRecord {
                view,
                active_at: spec.active_at,
            },
        );
        Some(instance_event_window_id)
    }
}

/// The caller-chosen bytes one stored event window holds — its tags, its name, the cron
/// expression the request named and the time ranges it named — plus the fixed cost of the
/// record itself ([`INSTANCE_EVENT_WINDOW_RECORD_BYTES`]), so no window is free. The id and
/// the state are derived by this module.
///
/// A time range is a fixed-size element, but the LIST of them is as long as the caller made
/// it, so each one is charged — the bytes of the two week-day words it stores plus the
/// fixed cost of the element itself ([`EVENT_WINDOW_TIME_RANGE_BYTES`]) — the way
/// [`vpc_endpoint_connection_payload_bytes`] charges each DNS entry and each load balancer
/// ARN of a connection. A count ceiling ([`MAX_EVENT_WINDOW_TIME_RANGES`]) bounds the list
/// too, and the two bounds are independent: neither is the other.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: a
/// writer charges exactly it, and a removal path must give back the same number.
pub fn instance_event_window_payload_bytes(window: &InstanceEventWindow) -> usize {
    let time_range_bytes: usize = window
        .time_ranges
        .iter()
        .flatten()
        .map(|range| {
            EVENT_WINDOW_TIME_RANGE_BYTES
                .saturating_add(range.start_week_day.as_deref().map_or(0, str::len))
                .saturating_add(range.end_week_day.as_deref().map_or(0, str::len))
        })
        .sum();
    INSTANCE_EVENT_WINDOW_RECORD_BYTES
        .saturating_add(tag_list_bytes(
            window.tags.as_deref().unwrap_or_default(),
        ))
        .saturating_add(window.name.as_deref().map_or(0, str::len))
        .saturating_add(window.cron_expression.as_deref().map_or(0, str::len))
        .saturating_add(time_range_bytes)
}

/// The fixed overhead one stored event window costs whatever it carries: the modelled
/// record itself, its minted id and the map entry holding it, as a CHOSEN constant in the
/// same spirit as [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] — the point is that an untagged
/// window is not free against [`MAX_STORED_BYTES`], not that it is byte-exact.
pub const INSTANCE_EVENT_WINDOW_RECORD_BYTES: usize = 512;

/// The fixed bytes one stored time range holds beside its two week-day words: the two hours
/// the model bounds to 0..=23, as the four-byte integers the record keeps them in.
pub const EVENT_WINDOW_TIME_RANGE_BYTES: usize = 2 * ::std::mem::size_of::<i32>();

/// Whether `id` is a well-formed event window id: [`INSTANCE_EVENT_WINDOW_ID_PREFIX`]
/// followed by 8 or 17 lowercase hexadecimal digits — the short and the long id form every
/// EC2 resource id takes, as the model's own `^iew-([0-9a-f]{8}|[0-9a-f]{17})$` style id
/// patterns read. The one definition of that form lives in
/// [`prefixed_id_is_well_formed`], which this names the prefix for.
pub fn instance_event_window_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(INSTANCE_EVENT_WINDOW_ID_PREFIX, id)
}

/// Refuse a request that named an event window id the account holds no such window for:
/// HTTP 400 `InvalidInstanceEventWindowId.NotFound`.
pub fn invalid_instance_event_window_id_not_found<E>(
    instance_event_window_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND,
        400,
        format!("The event window ID '{instance_event_window_id}' does not exist"),
    )
}

/// Refuse an event window id this service cannot resolve: a malformed one is answered as
/// malformed and a well-formed one as not found — the ordering [`unresolvable_host_id`] and
/// [`require_known_volume`] keep.
pub fn unresolvable_instance_event_window_id<E>(instance_event_window_id: &str) -> Scaffolded<E> {
    if instance_event_window_id_is_well_formed(instance_event_window_id) {
        return invalid_instance_event_window_id_not_found(instance_event_window_id);
    }
    Scaffolded::service_error(
        INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED,
        400,
        format!("Invalid id: '{instance_event_window_id}'"),
    )
}

/// Refuse a create that would take the store past [`MAX_INSTANCE_EVENT_WINDOWS`] or its
/// byte budget: HTTP 400 `InstanceEventWindowLimitExceeded`.
pub fn instance_event_window_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INSTANCE_EVENT_WINDOW_LIMIT_EXCEEDED, 400, message)
}

/// The calendar index of the `WeekDay` member `day` — `sunday` is 0 and `saturday` is 6 —
/// or `None` for a word that is not one of [`WEEK_DAYS_IN_ORDER`].
pub fn week_day_index(day: &str) -> Option<i64> {
    WEEK_DAYS_IN_ORDER
        .iter()
        .position(|(modelled, _)| *modelled == day)
        .and_then(|index| i64::try_from(index).ok())
}

/// How many hours a range running from the hour-of-the-week `start` to `end` covers.
///
/// Measured modulo [`HOURS_PER_WEEK`], because a time range may run across the end of the
/// week (`saturday` 20:00 to `sunday` 04:00 is eight hours, not minus 160). A range whose
/// start and end are the same hour covers none.
pub fn event_window_span_hours(start: i64, end: i64) -> i64 {
    (end - start).rem_euclid(HOURS_PER_WEEK)
}

/// Validate the `TimeRanges` of an event window, answering the modelled time ranges to
/// store.
///
/// How MANY ranges a window may carry is bounded first ([`MAX_EVENT_WINDOW_TIME_RANGES`]),
/// before this allocates anything, because the model declares no `@length` on the list.
/// The rules for each one are the ones `CreateInstanceEventWindowRequest$CronExpression` states for the
/// equivalent cron expression, and that "Define event windows for scheduled events" in the
/// Amazon EC2 User Guide — the guide `CreateInstanceEventWindow` points at — states for an
/// event window however it is defined: every range covers at least
/// [`MIN_EVENT_WINDOW_RANGE_HOURS`] and they cover at least
/// [`MIN_EVENT_WINDOW_TOTAL_HOURS`] between them. Each range names a week day and an hour
/// at both ends; the model bounds the hours (`Hour` is `@range(min: 0, max: 23)`), so an
/// out-of-range one is refused before a handler runs, and an unknown week day is refused
/// here.
///
/// Shared by the create and the modify of a window, which validate the same member the same
/// way.
pub fn validate_event_window_time_ranges<E>(
    ranges: &[InstanceEventWindowTimeRangeRequest],
) -> Result<Vec<InstanceEventWindowTimeRange>, Scaffolded<E>> {
    // Refused BEFORE anything is allocated or validated, so neither the work nor the
    // allocation this call makes is proportional to a number the caller chose freely.
    if ranges.len() > MAX_EVENT_WINDOW_TIME_RANGES {
        return Err(invalid_parameter_value(format!(
            "timeRanges carries {} time ranges, above the maximum of \
             {MAX_EVENT_WINDOW_TIME_RANGES}",
            ranges.len()
        )));
    }
    let mut stored: Vec<InstanceEventWindowTimeRange> = Vec::with_capacity(ranges.len());
    let mut total_hours: i64 = 0;
    for range in ranges {
        let (Some(start_week_day), Some(end_week_day)) = (
            range.start_week_day.as_deref(),
            range.end_week_day.as_deref(),
        ) else {
            return Err(missing_parameter(
                "A time range must name a startWeekDay and an endWeekDay",
            ));
        };
        let (Some(start_hour), Some(end_hour)) = (range.start_hour, range.end_hour) else {
            return Err(missing_parameter(
                "A time range must name a startHour and an endHour",
            ));
        };
        let (named_start_day, named_end_day) =
            (week_day_index(start_week_day), week_day_index(end_week_day));
        let (Some(start_day), Some(end_day)) = (named_start_day, named_end_day) else {
            let unknown = if named_start_day.is_none() {
                start_week_day
            } else {
                end_week_day
            };
            return Err(invalid_parameter_value(format!(
                "Invalid value '{unknown}' for a time range week day. \
                 A week day is one of {days}",
                days = WEEK_DAYS_IN_ORDER
                    .iter()
                    .map(|(modelled, _)| *modelled)
                    .collect::<Vec<_>>()
                    .join(", "),
            )));
        };
        let start = start_day * HOURS_PER_DAY + i64::from(start_hour);
        let end = end_day * HOURS_PER_DAY + i64::from(end_hour);
        let hours = event_window_span_hours(start, end);
        if hours < MIN_EVENT_WINDOW_RANGE_HOURS {
            return Err(invalid_parameter_value(format!(
                "The time range from {start_week_day} {start_hour} to {end_week_day} \
                 {end_hour} covers {hours} hours; each time range must cover at least \
                 {MIN_EVENT_WINDOW_RANGE_HOURS} hours"
            )));
        }
        total_hours = total_hours.saturating_add(hours);
        stored.push(InstanceEventWindowTimeRange {
            start_week_day: Some(start_week_day.to_string()),
            start_hour: Some(start_hour),
            end_week_day: Some(end_week_day.to_string()),
            end_hour: Some(end_hour),
        });
    }
    if total_hours < MIN_EVENT_WINDOW_TOTAL_HOURS {
        return Err(invalid_parameter_value(format!(
            "The time ranges cover {total_hours} hours between them; the combined total \
             must be at least {MIN_EVENT_WINDOW_TOTAL_HOURS} hours"
        )));
    }
    Ok(stored)
}

/// Validate the `CronExpression` of an event window.
///
/// The constraints are the ones the member documents: a length this service reads at all
/// ([`MAX_EVENT_WINDOW_CRON_BYTES`], refused first, before the expression is split), then
/// five cron fields, of which only the hour and the day of the week carry values — "the
/// minute, month, and year must be
/// specified by `*`" — the hour field being "one or a multiple range, for example, 0-4 or
/// 0-4,20-23" with every range covering at least [`MIN_EVENT_WINDOW_RANGE_HOURS`] and the
/// ranges covering at least [`MIN_EVENT_WINDOW_TOTAL_HOURS`] between them, and a day of the
/// week being "either integers 0 through 6, or alternative single values SUN through SAT"
/// (`CreateInstanceEventWindowRequest$CronExpression`).
///
/// Shared by the create and the modify of a window, which validate the same member the same
/// way.
pub fn validate_event_window_cron_expression<E>(expression: &str) -> Result<(), Scaffolded<E>> {
    // Refused BEFORE the expression is split, so the pointer vector below is never sized
    // from a length the caller chose freely.
    if expression.len() > MAX_EVENT_WINDOW_CRON_BYTES {
        return Err(invalid_parameter_value(format!(
            "cronExpression carries {} bytes, above the maximum of \
             {MAX_EVENT_WINDOW_CRON_BYTES}",
            expression.len()
        )));
    }
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{expression}' for cronExpression. A cron expression carries \
             {CRON_FIELDS} fields, not {}",
            fields.len()
        )));
    }
    // Only the hour (the second field) and the day of the week (the fifth) carry values;
    // the minute, the day of the month and the month must each be `*`.
    for (position, name) in [(0, "minute"), (2, "day of the month"), (3, "month")] {
        let field = fields.get(position).copied().unwrap_or_default();
        if field != CRON_WILDCARD {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{field}' for the {name} field of cronExpression. \
                 It must be '{CRON_WILDCARD}'"
            )));
        }
    }

    let hour_field = fields.get(1).copied().unwrap_or_default();
    let mut total_hours: i64 = 0;
    for element in hour_field.split(',') {
        // "The hour value must be one or a multiple range" — a bare hour names no range, so
        // it is refused along with anything that is not two hours around one `-`.
        let Some((start, end)) = element.split_once('-') else {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour value is one or more ranges, for example 0-4 or 0-4,20-23"
            )));
        };
        let (Ok(start), Ok(end)) = (start.parse::<i64>(), end.parse::<i64>()) else {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour range runs between two hours, for example 0-4"
            )));
        };
        if !(0..HOURS_PER_DAY).contains(&start) || !(0..HOURS_PER_DAY).contains(&end) {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour is from 0 to {last}",
                last = HOURS_PER_DAY - 1
            )));
        }
        // A cron hour range does not wrap within a day — the day-of-the-week field is what
        // carries a window across days — so a reversed range is refused as reversed rather
        // than reported as covering a negative number of hours.
        if end <= start {
            return Err(invalid_parameter_value(format!(
                "Invalid value '{element}' for the hour field of cronExpression. \
                 An hour range runs forwards, from its earlier hour to its later one"
            )));
        }
        let hours = end - start;
        if hours < MIN_EVENT_WINDOW_RANGE_HOURS {
            return Err(invalid_parameter_value(format!(
                "The hour range '{element}' of cronExpression covers {hours} hours; \
                 each hour range must cover at least {MIN_EVENT_WINDOW_RANGE_HOURS} hours"
            )));
        }
        total_hours = total_hours.saturating_add(hours);
    }
    if total_hours < MIN_EVENT_WINDOW_TOTAL_HOURS {
        return Err(invalid_parameter_value(format!(
            "The hour ranges of cronExpression cover {total_hours} hours between them; \
             the combined total must be at least {MIN_EVENT_WINDOW_TOTAL_HOURS} hours"
        )));
    }

    let day_field = fields.get(4).copied().unwrap_or_default();
    // The day-of-the-week field MAY be the wildcard — a window that opens in its hours on
    // every day of the week — while the hour field may not: "the hour value must be one or a
    // multiple range" names no wildcard, and a window open every hour of every day would
    // not be a window. The asymmetry is the member's, not an oversight here.
    if day_field != CRON_WILDCARD {
        for element in day_field.split(',') {
            if !cron_day_of_week_is_known(element) {
                return Err(invalid_parameter_value(format!(
                    "Invalid value '{element}' for the day of the week field of \
                     cronExpression. A day of the week is an integer from 0 to 6, or one \
                     of {days}",
                    days = WEEK_DAYS_IN_ORDER
                        .iter()
                        .map(|(_, alternative)| *alternative)
                        .collect::<Vec<_>>()
                        .join(", "),
                )));
            }
        }
    }
    Ok(())
}

/// The cron field that names every value of its position: "the minute, month, and year must
/// be specified by `*`" (`CreateInstanceEventWindowRequest$CronExpression`).
pub const CRON_WILDCARD: &str = "*";

/// Whether `value` names a day of the week a cron expression may carry: "either integers 0
/// through 6, or alternative single values SUN through SAT"
/// (`CreateInstanceEventWindowRequest$CronExpression`), the three-letter alternatives of
/// [`WEEK_DAYS_IN_ORDER`] in either case, cron naming them case-insensitively.
fn cron_day_of_week_is_known(value: &str) -> bool {
    if let Ok(day) = value.parse::<i64>() {
        let days = i64::try_from(WEEK_DAYS_IN_ORDER.len()).unwrap_or_default();
        return (0..days).contains(&day);
    }
    WEEK_DAYS_IN_ORDER
        .iter()
        .any(|(_, alternative)| alternative.eq_ignore_ascii_case(value))
}
