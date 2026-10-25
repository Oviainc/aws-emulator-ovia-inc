// SPDX-License-Identifier: CC0-1.0

//! How EXACTLY the interval rule compares the two bounds a ranged request carries, end
//! to end over HTTP.
//!
//! The rule every case here asserts is the interval rule of
//! [`crate::interval_validation`] taken to the resolution a `Timestamp` actually has: a
//! pair of bounds names an interval when the start is strictly before the end at the FULL
//! precision the request spelled them in, so a pair separated by a single nanosecond is
//! an interval and is answered, while the same pair reversed — and a pair whose bounds are
//! equal down to the nanosecond — is refused with HTTP 400 `ValidationException` naming
//! both bounds.
//!
//! Why this is a case of its own rather than another row of the sibling table: those rows
//! are a millisecond or more apart, which an `f64` of epoch seconds can still order, so
//! they do not distinguish an exact comparison from one that rounds. These rows are at most
//! a microsecond apart — a difference an `f64` of epoch seconds cannot hold at a
//! present-day instant (its steps there are hundreds of nanoseconds) nor further out (its
//! steps grow with the instant) — so a comparison that went through one would read such a
//! pair as the same instant and refuse an interval it must answer, or accept a reversed
//! pair it must refuse.
//!
//! Rows vary what the rule generalises over: the operation called, how far apart the
//! bounds are (one, ten, a hundred and a thousand nanoseconds), their direction, and where
//! on the time axis they sit — at the epoch, at a present-day instant, and far enough into
//! the future that even a microsecond is below the resolution of an `f64` of epoch
//! seconds.
//!
//! The emulator, the HTTP plumbing, the operation list and the expected refusal are the
//! shared [`crate::profiling_group_lifecycle`] and [`crate::interval_validation`] helpers,
//! not a fork of them.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::interval_validation::{call_bounds, canonical, expected_refusal, operations};
use crate::profiling_group_lifecycle::Harness;

/// The group every row runs against, created through the API like any other.
const GROUP: &str = "interval-resolution-group";

/// Instants the pairs are laid out at, as (label, epoch seconds). The last is far enough
/// out that an `f64` of epoch seconds cannot resolve a microsecond there, so even the
/// widest row below would collapse under a rounding comparison.
const BASES: [(&str, i64); 3] = [
    ("at the epoch", 0),
    ("at a 2026 instant", 1_767_225_600),
    ("at a year-2286 instant", 9_999_999_999),
];

/// How far apart a pair's bounds are, in nanoseconds: every step from the smallest
/// difference a `Timestamp` can express up to a microsecond.
const DELTAS_NANOS: [u32; 4] = [1, 10, 100, 1_000];

/// `secs`.`nanos` in the ISO 8601 `date-time` form the modelled query members carry, with
/// the fraction spelled to the nanosecond — the precision a row needs the REQUEST to
/// carry, which is what the rule is compared at.
fn at(secs: i64, nanos: u32) -> String {
    if nanos == 0 {
        return whole_second(secs);
    }
    with_fraction(secs, &format!("{nanos:09}"))
}

/// `secs` with no sub-second part.
fn whole_second(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {secs}: {error}"))
}

/// `secs`.`micros` — the finest fraction this service RENDERS when it names an instant
/// in a message (what `Format::DateTime` emits), so a bound spelled this way reads back
/// from a refusal exactly as the request carried it.
fn at_micros(secs: i64, micros: u32) -> String {
    with_fraction(secs, &format!("{micros:06}"))
}

/// `secs` carrying `fraction` as its already-rendered sub-second digits.
fn with_fraction(secs: i64, fraction: &str) -> String {
    let whole = whole_second(secs);
    let without_zone = whole.strip_suffix('Z').unwrap_or(&whole);
    format!("{without_zone}.{fraction}Z")
}

/// One pair of bounds and whether the rule says it names an interval.
#[derive(Debug)]
struct Pair {
    label: String,
    start: String,
    end: String,
    is_interval: bool,
}

/// The table: for each instant on the axis and each sub-microsecond separation, the
/// increasing pair (an interval), the same pair reversed, and the pair whose bounds are
/// equal at that nanosecond (neither an interval).
fn pairs() -> Vec<Pair> {
    let mut rows = Vec::new();
    for (where_on_axis, secs) in BASES {
        for delta in DELTAS_NANOS {
            let earlier = at(secs, 0);
            let later = at(secs, delta);
            rows.push(Pair {
                label: format!("{where_on_axis}, increasing by {delta} ns"),
                start: earlier.clone(),
                end: later.clone(),
                is_interval: true,
            });
            rows.push(Pair {
                label: format!("{where_on_axis}, reversed by {delta} ns"),
                start: later.clone(),
                end: earlier,
                is_interval: false,
            });
            rows.push(Pair {
                label: format!("{where_on_axis}, equal at a {delta} ns fraction"),
                start: later.clone(),
                end: later,
                is_interval: false,
            });
        }
    }
    rows
}

#[tokio::test]
async fn the_interval_rule_compares_both_bounds_at_their_full_precision() {
    let harness = Harness::new();
    crate::interval_validation::create_group(&harness, GROUP).await;
    // One posted profile, so an accepted row reaches a group that holds data rather than
    // only an absence.
    assert_eq!(
        harness
            .post_agent_profile(GROUP, "application/json", None, b"{\"frames\":[]}".to_vec())
            .await
            .status,
        204,
        "the profile must be stored"
    );

    for operation in operations() {
        for row in pairs() {
            let case = format!("{} {:?}", operation.name, row.label);
            let answer = call_bounds(&harness, &operation, GROUP, &row.start, &row.end).await;

            if row.is_interval {
                // The pair IS an interval, so the answer is the operation's own: the
                // listing or analysis itself (200), or — for `GetProfile`, which reports
                // one aggregated profile covering the window — the modelled 404 when the
                // group retains none so narrow a window touches. Never a refusal of the
                // range.
                assert!(
                    matches!(answer.status, 200 | 404),
                    "case {case}: bounds {:?}..{:?} name an interval, so the answer must be \
                     the operation's own (200, or 404 when nothing covers the window), \
                     got {} {}",
                    row.start,
                    row.end,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                if answer.status != 200 {
                    assert_eq!(
                        answer.error_code(),
                        "ResourceNotFoundException",
                        "case {case}: an interval must never be answered with a refusal of \
                         the range, got {}",
                        String::from_utf8_lossy(&answer.body)
                    );
                }
                continue;
            }

            assert_eq!(
                answer.status,
                400,
                "case {case}: expected HTTP 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected = expected_refusal(&row.start, &row.end);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_names_the_bounds_the_request_carried() {
    let harness = Harness::new();
    crate::interval_validation::create_group(&harness, GROUP).await;

    // A bound whose sub-second part must survive into the message: the refusal states the
    // instants the request carried, not a whole-second rounding of them.
    for (_, secs) in BASES {
        let instant = at_micros(secs, 123_456);
        assert_eq!(
            canonical(&instant),
            instant,
            "case {instant:?}: the ISO 8601 form a row sends must be the canonical one"
        );
        for operation in operations() {
            let answer = call_bounds(&harness, &operation, GROUP, &instant, &instant).await;
            let expected = expected_refusal(&instant, &instant);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {} {instant:?}: expected {expected:?}, got {:?}",
                operation.name,
                answer.error_message()
            );
        }
    }
}
