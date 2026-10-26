// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an ISO 8601 DURATION a caller supplied as a string.
//!
//! # The rule
//!
//! A duration in the ISO 8601 format names a span of time: `PT5M`, `P1DT1H1M1S`,
//! `PT0.5S`. Several AWS APIs take one as a plain string member and then act on the span
//! it names — how long an action runs for, how long a time window a read selects over is
//! — so the reading is stated once here and every service that needs a span takes it from
//! the same place, rather than each deciding for itself how long `P1Y` is.
//!
//! The parse is the maintained `iso8601-duration` crate. What this module settles is which
//! durations name a span the emulator can act on:
//!
//! * a duration whose length depends on the CALENDAR — a year or a month designator —
//!   names no fixed span (a year is 365 or 366 days long, a month 28 to 31), so it names
//!   none here rather than being guessed at;
//! * a value that is not an ISO 8601 duration at all, one whose fields are not finite, and
//!   one whose span does not fit a [`Duration`] (the parser's fields are floats, so a
//!   caller may name 1e30 seconds) name none either.
//!
//! TOTAL over every string a caller can send, and it cannot panic: a span this cannot read
//! is `None`, and the service that asked words its own refusal — or takes its own default
//! — from that. A duration of ZERO length IS a span and is returned as one; whether a
//! zero-length span is usable is the asking service's rule, not this reading's.

use std::time::Duration;

/// The span the ISO 8601 duration `value` names, or `None` when it names none — the
/// module header states which durations are which.
#[must_use]
pub fn iso8601_span(value: &str) -> Option<Duration> {
    let parsed = ::iso8601_duration::Duration::parse(value).ok()?;
    if parsed.year != 0.0 || parsed.month != 0.0 {
        return None;
    }
    let seconds = f64::from(parsed.day) * 86_400.0
        + f64::from(parsed.hour) * 3_600.0
        + f64::from(parsed.minute) * 60.0
        + f64::from(parsed.second);
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    // `try_from_secs_f64` is the non-panicking construction: a span a `Duration` cannot
    // hold is read as no span, exactly like a value that is no duration at all.
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows span the duration grammar's fields and combinations, each with the span it
    /// names in milliseconds, and the values that name no span at all.
    #[test]
    fn a_duration_names_the_span_its_fields_add_up_to() {
        let spans: [(&str, u128); 10] = [
            ("PT5M", 300_000),
            ("PT1H", 3_600_000),
            ("P1D", 86_400_000),
            ("P1DT1H1M1S", 90_061_000),
            ("PT7M", 420_000),
            ("PT1.5S", 1_500),
            ("PT0S", 0),
            ("P7D", 604_800_000),
            ("PT12H30M", 45_000_000),
            ("P2DT3H4M5S", 183_845_000),
        ];
        for (value, expected) in spans {
            let actual = iso8601_span(value);
            assert_eq!(
                actual.map(|span| span.as_millis()),
                Some(expected),
                "case {value:?}: expected {expected} ms, got {actual:?}"
            );
        }

        let no_span = [
            "P1Y",
            "P1M",
            "P1Y2M",
            "P",
            "PT",
            "5m",
            "PT5",
            "notaduration",
            "",
        ];
        for value in no_span {
            let actual = iso8601_span(value);
            assert_eq!(
                actual, None,
                "case {value:?}: expected no span, got {actual:?}"
            );
        }
    }
}
