// SPDX-License-Identifier: CC0-1.0

//! How this service RENDERS an instant it names in a message, end to end over HTTP.
//!
//! The rule every case here asserts: an instant a refusal of this service names is
//! rendered in the ISO 8601 `date-time` form the model documents for every one of its time
//! members — "This is specified using the ISO 8601 format. For example,
//! 2020-06-01T13:15:02.001Z represents 1 millisecond past June 1, 2020 1:15:02 PM UTC"
//! (`smithy_query(query="doc", shape="GetRecommendationsRequest", member="startTime")`) —
//! whose fractional-second field is a FIXED-WIDTH, zero-padded, 3-digit millisecond field
//! present on every instant. So a bound at an exact second is named `...:59.000Z`, one at
//! half a second `...:59.500Z` and one at 660 ms `...:59.660Z`: never a shorter field with
//! its trailing zeros dropped, and never no field at all.
//!
//! Rows vary what the rule generalises over: how many digits the REQUEST spelled the
//! fraction with (none, one, two, three, and more than the field holds), which millisecond
//! value it names (zero, each power of ten, values ending in one zero and in two, and the
//! largest the field can hold), where on the time axis the instant sits (before the epoch,
//! at it, at a present-day instant and far into the future) and which operation refuses —
//! including the bound a refusal COMPUTED rather than echoed, so the rule is about
//! rendering an instant and not about repeating request text.
//!
//! The emulator, the HTTP plumbing, the ranged-operation list and the refusal sentences
//! are the shared [`crate::profiling_group_lifecycle`], [`crate::interval_validation`] and
//! [`crate::frame_metric_data`] helpers, not a fork of them.

use crate::interval_validation::{call_bounds, create_group, operations};
use crate::profiling_group_lifecycle::Harness;

/// The group every row runs against, created through the API like any other.
const GROUP: &str = "message-timestamp-group";

/// One fractional-second field: the digits a request spells after the whole second (empty
/// for a bound at an exact second, the dot included otherwise) and the three digits the
/// documented form names that instant by.
#[derive(Debug, Clone, Copy)]
struct Fraction {
    label: &'static str,
    /// What the request carries after the seconds.
    sent: &'static str,
    /// The three digits a message's fractional-second field must carry.
    named: &'static str,
}

/// The table of fractional-second fields.
///
/// The widths cover every form the field can be ASKED for — absent, one digit, two, the
/// three the form carries, and finer than it — and the values cover a zero millisecond,
/// each power of ten, a value ending in one zero and one ending in two (the ones a
/// trailing-zero-stripping renderer shortens), and the largest value the field holds.
const FRACTIONS: [Fraction; 14] = [
    Fraction {
        label: "an exact second, no fraction sent",
        sent: "",
        named: "000",
    },
    Fraction {
        label: "a one-digit zero fraction",
        sent: ".0",
        named: "000",
    },
    Fraction {
        label: "a three-digit zero fraction",
        sent: ".000",
        named: "000",
    },
    Fraction {
        label: "the millisecond the model's own example names",
        sent: ".001",
        named: "001",
    },
    Fraction {
        label: "ten milliseconds spelled with two digits",
        sent: ".01",
        named: "010",
    },
    Fraction {
        label: "a hundred milliseconds spelled with one digit",
        sent: ".1",
        named: "100",
    },
    Fraction {
        label: "half a second spelled with one digit",
        sent: ".5",
        named: "500",
    },
    Fraction {
        label: "half a second spelled with two digits",
        sent: ".50",
        named: "500",
    },
    Fraction {
        label: "half a second spelled with three digits",
        sent: ".500",
        named: "500",
    },
    Fraction {
        label: "a millisecond value ending in one zero, spelled with two digits",
        sent: ".66",
        named: "660",
    },
    Fraction {
        label: "the same value spelled with all three digits",
        sent: ".660",
        named: "660",
    },
    Fraction {
        label: "a millisecond value ending in two zeros",
        sent: ".7",
        named: "700",
    },
    Fraction {
        label: "the largest value the field holds",
        sent: ".999",
        named: "999",
    },
    Fraction {
        label: "a fraction finer than the field, named at the millisecond it falls in",
        sent: ".123456",
        named: "123",
    },
];

/// Where on the time axis a pair of bounds is laid out, as the civil date-time text before
/// the fractional second. UTC has no offset to shift, so a civil part stands for the same
/// instant whichever day it names.
const CIVIL_BASES: [(&str, &str); 4] = [
    ("before the epoch", "1969-12-31T23:59:59"),
    ("at the epoch", "1970-01-01T00:00:00"),
    ("at a 2026 instant", "2026-10-25T15:59:59"),
    ("at a far-future instant", "2286-11-20T17:46:39"),
];

/// The bound a request carries: the civil part as the fraction was spelled.
fn sent(civil: &str, fraction: Fraction) -> String {
    format!("{civil}{}Z", fraction.sent)
}

/// The bound a message must name: the civil part with the field's three digits.
fn named(civil: &str, fraction: Fraction) -> String {
    format!("{civil}.{}Z", fraction.named)
}

/// The interval refusal's sentence, over two already-rendered bounds. The wording itself
/// is the one shared [`crate::interval_validation::expected_refusal`]; this suite is about
/// the two bounds inside it.
fn interval_refusal(start: &str, end: &str) -> String {
    crate::interval_validation::expected_refusal(start, end)
}

#[tokio::test]
async fn an_interval_refusal_names_each_bound_with_the_documented_millisecond_field() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;

    for (where_on_axis, civil) in CIVIL_BASES {
        for fraction in FRACTIONS {
            // A zero-width pair, so the refusal is the interval one and both bounds are
            // the instant this row is about.
            let bound = sent(civil, fraction);
            let expected = interval_refusal(&named(civil, fraction), &named(civil, fraction));
            for operation in operations() {
                let case = format!(
                    "{} {where_on_axis}, {}, sent {bound:?}",
                    operation.name, fraction.label
                );
                let answer = call_bounds(&harness, &operation, GROUP, &bound, &bound).await;
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
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}

#[tokio::test]
async fn a_refusal_naming_two_different_bounds_renders_each_one_at_the_field() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;

    // Reversed pairs whose two bounds carry DIFFERENT fractions, so neither bound's
    // rendering can be standing in for the other's: the start is the later instant.
    let civil_start = "2026-10-25T16:00:00";
    let civil_end = "2026-10-25T15:00:00";
    for start_fraction in FRACTIONS {
        for end_fraction in FRACTIONS {
            let start = sent(civil_start, start_fraction);
            let end = sent(civil_end, end_fraction);
            let expected = interval_refusal(
                &named(civil_start, start_fraction),
                &named(civil_end, end_fraction),
            );
            for operation in operations() {
                let case = format!(
                    "{} start {:?} ({}), end {:?} ({})",
                    operation.name, start, start_fraction.label, end, end_fraction.label
                );
                let answer = call_bounds(&harness, &operation, GROUP, &start, &end).await;
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}

#[tokio::test]
async fn a_refusal_names_a_bound_it_computed_with_the_same_field() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;

    // `BatchGetFrameMetricData` over a two-day period refuses it as longer than the one
    // day the `period` member documents, and that refusal names the END of the period —
    // an instant the service COMPUTED from the start and the length, which the request
    // never spelled. Two days on is the same civil time two days later, UTC having no
    // offset to shift.
    let civil_start = "2026-10-25T15:59:59";
    let civil_end = "2026-10-27T15:59:59";
    for fraction in FRACTIONS {
        let start = sent(civil_start, fraction);
        let case = format!("{}, sent {start:?}", fraction.label);
        let answer = crate::frame_metric_data::frame_metric_data(
            &harness,
            GROUP,
            &format!("?startTime={start}&period=P2D"),
            &[crate::frame_metric_data::frame_metric("A", &["RUNNABLE"])],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case}: expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected = format!(
            "the time period for the returned time series values must be at most PT24H long, \
             got {} to {}",
            named(civil_start, fraction),
            named(civil_end, fraction)
        );
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}
