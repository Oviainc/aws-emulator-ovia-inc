// SPDX-License-Identifier: CC0-1.0

//! The bounds a trace-query read declares on the window and the granularity it
//! will serve, and the refusals that follow from them.
//!
//! Each read that takes a `[StartTime, EndTime]` window publishes the widest
//! window it serves, and how far ahead of the clock its `EndTime` may reach;
//! a request outside either bound is refused rather than answered over part
//! of it, because a client that must cut a long range into slices has to
//! learn the bound from the service. The aggregated statistics read publishes
//! the aggregation PERIODS it serves the same way.
//!
//! One concern, one file: the bound of each read and the sentence it is refused
//! with are stated once here, so two readers cannot word one rule two ways and
//! the bound a message names is the bound that was checked.

use std::fmt::Write as _;

/// The widest window `GetTraceSummaries` serves, in whole hours.
pub const MAX_TRACE_SUMMARIES_WINDOW_HOURS: i64 = 24;

/// The widest window `GetServiceGraph` serves, in whole hours.
pub const MAX_SERVICE_GRAPH_WINDOW_HOURS: i64 = 6;

/// How many seconds one hour is — the unit the stored and submitted instants of
/// this crate are in ([`super::epoch_seconds`]), against which a bound stated in
/// hours is checked.
const SECONDS_PER_HOUR: f64 = 60.0 * 60.0;

/// Whether a `[start, end]` window is inside the `max_hours` a read serves.
///
/// `Err` carries the refusal's message, which the calling operation lifts into
/// its own modelled client-input error. The span is measured between the
/// submitted bounds themselves, so the same window is judged the same way
/// whichever instant of a trace the read applies it to. A window running
/// backwards is not this check's business — it is refused by the crate's window
/// ORDER check before this one — so a negative span passes here.
pub fn validate_window_span(start: f64, end: f64, max_hours: i64) -> Result<(), String> {
    #[allow(clippy::cast_precision_loss)]
    let max_seconds = max_hours as f64 * SECONDS_PER_HOUR;
    if end - start > max_seconds {
        return Err(window_too_long_message(max_hours));
    }
    Ok(())
}

/// The refusal for a window wider than the `max_hours` a read serves: fixed
/// text naming that bound, and nothing from the request.
#[must_use]
pub fn window_too_long_message(max_hours: i64) -> String {
    format!("Time range cannot be longer than {max_hours} hours")
}

/// How far ahead of the clock the `EndTime` of a trace-summaries query may
/// reach, in whole minutes.
///
/// A window may end a little past now — a client's clock runs slightly ahead,
/// and a dashboard asks for "up to now" — but a bound further ahead than that
/// names a time no recorded traffic can carry, so the read publishes how far it
/// will go.
pub const MAX_TRACE_SUMMARIES_FUTURE_MINUTES: i64 = 15;

/// How many seconds one minute is, for a bound stated in minutes.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// Whether `end` is inside the `max_minutes` past `now` a read will reach.
///
/// `Err` carries the refusal's message, which the calling operation lifts into
/// its own modelled client-input error.
pub fn validate_window_end_ahead(end: f64, now: f64, max_minutes: i64) -> Result<(), String> {
    #[allow(clippy::cast_precision_loss)]
    let allowance = max_minutes as f64 * SECONDS_PER_MINUTE;
    if end - now > allowance {
        return Err(window_too_far_ahead_message(max_minutes));
    }
    Ok(())
}

/// The refusal for an `EndTime` further ahead of the clock than a read will
/// reach: fixed text naming that allowance, and nothing from the request.
#[must_use]
pub fn window_too_far_ahead_message(max_minutes: i64) -> String {
    format!("End time cannot be later than {max_minutes} minutes in the future")
}

/// The aggregation periods `GetTimeSeriesServiceStatistics` serves, in seconds:
/// the one-minute and five-minute aggregations.
///
/// `Period` targets `NullableInteger` and carries no modelled range
/// (`smithy_query(query="operation",operation="GetTimeSeriesServiceStatistics")`),
/// so the set of periods the read serves is the service's own, and a `Period`
/// outside it is refused rather than aggregated at a granularity nothing
/// computes.
pub const SERVED_PERIOD_SECONDS: [i32; 2] = [60, 300];

/// Whether `period` is one of the aggregation periods a statistics read serves.
///
/// `Err` carries the refusal's message, which names the SET that is served —
/// what the caller has to choose from — and nothing from the request.
pub fn validate_period(period: i32) -> Result<(), String> {
    if SERVED_PERIOD_SECONDS.contains(&period) {
        return Ok(());
    }
    Err(unserved_period_message())
}

/// The refusal for a `Period` outside [`SERVED_PERIOD_SECONDS`]: fixed text
/// listing the served periods, rendered from that one statement of them.
#[must_use]
pub fn unserved_period_message() -> String {
    let mut served = String::new();
    for (at, period) in SERVED_PERIOD_SECONDS.iter().enumerate() {
        if at > 0 {
            served.push_str(", ");
        }
        // Writing into a `String` cannot fail, and the value is one of this
        // module's own constants.
        let _ = write!(served, "{period}");
    }
    format!("Time period must be one of [{served}]")
}
