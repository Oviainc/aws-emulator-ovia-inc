// SPDX-License-Identifier: CC0-1.0

//! What MAGNITUDE a number a stored profile document records is read at, end to end over
//! HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a number a stored profile document states is read as
//! the number its own notation states, whatever its magnitude and whichever of Ion's
//! number types spells it — so a sample count larger than a machine integer holds is a
//! count like any other, counted in the frame metric it belongs to and added into the
//! aggregated profile the window's profiles combine into. Only a value that is no number,
//! or no number a count of samples can be (negative, or no finite number at all), is read
//! as no count.
//!
//! Rows vary what the rule generalises over: the magnitude of the count (inside a machine
//! integer, at its largest value, one past it, past an unsigned one, past any finite
//! number at all), the Ion number type and notation spelling it (integer, hexadecimal
//! integer, decimal with Ion's own `d` exponent, float), where the count sits (keyed under
//! a frame, nested below it, listed), how many thread states the metric asks about, and
//! whether the count is one a profile can record at all.
//!
//! Only Amazon Ion spells an integer of such a magnitude: a JSON text carries a magnitude
//! past a machine integer as a floating-point number, which is why every document here is
//! posted in Ion's binary encoding.
//!
//! The emulator, the HTTP plumbing and the response reducers are the shared
//! [`crate::profiling_group_lifecycle`] and [`crate::frame_metric_data`] harnesses, not a
//! fork of them.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ion_rs::v1_0;

use crate::frame_metric_data::{TOLERANCE, assert_value, datum, frame_metric, frame_metric_data};
use crate::interval_validation::create_group;
use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::{Answer, Harness, element_of};

/// The media types the model's `PostAgentProfile$contentType` and `GetProfile$accept`
/// docs name: "application/json — standard JSON format" and "application/x-amzn-ion — the
/// Amazon Ion data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// The instant every profile below is received at, in epoch seconds: a whole number of
/// hours past the epoch plus a minute, so it sits inside one hourly aggregation window
/// whose aggregation fell due long ago.
const RECEIPT_SECONDS: i64 = 1_600_002_000;

/// One hour in seconds — the resolution every series below is reported at.
const HOUR: i64 = 60 * 60;

/// The start of the one hourly window every case reports over, aligned to UTC.
fn window_start() -> i64 {
    RECEIPT_SECONDS - RECEIPT_SECONDS.rem_euclid(HOUR)
}

/// An instant as the ISO 8601 `date-time` form the modelled query members carry.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// A submittable profile document written in Ion's TEXT notation — the only notation that
/// spells an integer, a hexadecimal integer, a decimal and a float apart — carrying
/// `callgraph` and the `start` member `PostAgentProfile` requires of every profile it
/// accepts, and encoded as the binary Ion an agent posts.
fn ion_profile(callgraph: &str) -> Vec<u8> {
    let text = format!(
        "{{ start: {}, callgraph: {callgraph} }}",
        crate::profiling_group_lifecycle::PROFILE_START_MILLIS
    );
    element_of(text.as_bytes())
        .encode_as(v1_0::Binary)
        .expect("an encodable Ion document")
}

/// One row of the magnitude table: the call graph a stored profile records, written in Ion
/// text notation, the metric asked about it and the ratio the one step of the series must
/// report.
struct Case {
    label: &'static str,
    callgraph: &'static str,
    frame_name: &'static str,
    thread_states: &'static [&'static str],
    expected: f64,
}

/// The magnitudes and notations a count can be stated in.
///
/// The ratios are stated as the arithmetic the counts themselves state, so no row carries
/// a hand-computed number: `AggregatedRelativeTotalTime` is "Metric value aggregated for
/// all instances of a frame name in a profile relative to the root frame", so a row whose
/// named frame records one count `c` beside a sibling recording `c` reports `1/2`, and one
/// recording `2c` beside a sibling recording `c` reports `2/3`, whatever `c` is.
///
/// The magnitudes: 2^63 - 1 is the largest value a signed 64-bit machine integer holds,
/// 2^63 is one past it, and 2^64 is past an unsigned one — each spelled below as the
/// decimal digits Ion writes it with.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "counts a machine integer holds",
            callgraph: "{ A: { RUNNABLE: 4 }, B: { RUNNABLE: 1 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 4.0 / 5.0,
        },
        Case {
            label: "counts at the largest signed 64-bit integer",
            callgraph: "{ A: { RUNNABLE: 9223372036854775807 }, \
                          B: { RUNNABLE: 9223372036854775807 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 1.0 / 2.0,
        },
        Case {
            label: "counts one past the largest signed 64-bit integer",
            callgraph: "{ A: { RUNNABLE: 9223372036854775808 }, \
                          B: { RUNNABLE: 9223372036854775808 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 1.0 / 2.0,
        },
        Case {
            label: "a count past an unsigned 64-bit integer over a smaller one",
            callgraph: "{ A: { RUNNABLE: 18446744073709551616 }, \
                          B: { RUNNABLE: 9223372036854775808 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "the same magnitudes spelled as hexadecimal integers",
            callgraph: "{ A: { RUNNABLE: 0x10000000000000000 }, \
                          B: { RUNNABLE: 0x8000000000000000 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "the same magnitudes spelled as Ion decimals",
            callgraph: "{ A: { RUNNABLE: 1.8446744073709551616d19 }, \
                          B: { RUNNABLE: 9.223372036854775808d18 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "the same magnitudes spelled as floats",
            callgraph: "{ A: { RUNNABLE: 1.8446744073709551616e19 }, \
                          B: { RUNNABLE: 9.223372036854775808e18 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "a count past a machine integer beside a count a machine integer holds",
            callgraph: "{ A: { RUNNABLE: 18446744073709551616 }, B: { RUNNABLE: 1 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            // One sample beside 2^64 of them moves the ratio by less than the tolerance,
            // which is what makes the row an assertion that BOTH counts were read: a
            // dropped 2^64 would report the one sample's own share of nothing else.
            expected: 1.0,
        },
        Case {
            label: "a count past a machine integer nested below the named frame",
            callgraph: "{ A: { RUNNABLE: 9223372036854775808, \
                               B: { RUNNABLE: 9223372036854775808 } }, \
                          C: { RUNNABLE: 9223372036854775808 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "counts past a machine integer in frames listed rather than keyed",
            callgraph: "{ A: [{ RUNNABLE: 9223372036854775808 }, \
                              { RUNNABLE: 9223372036854775808 }], \
                          B: { RUNNABLE: 9223372036854775808 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "counts past a machine integer in two thread states",
            callgraph: "{ A: { RUNNABLE: 9223372036854775808, \
                               BLOCKED: 9223372036854775808 }, \
                          C: { RUNNABLE: 9223372036854775808, \
                               WAITING: 9223372036854775808 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE", "BLOCKED"],
            expected: 2.0 / 3.0,
        },
        Case {
            label: "a negative number past a machine integer is no sample count",
            callgraph: "{ A: { RUNNABLE: -18446744073709551616, B: { RUNNABLE: 2 } }, \
                          C: { RUNNABLE: 2 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 1.0 / 2.0,
        },
        Case {
            label: "a magnitude past any finite number is no sample count",
            callgraph: "{ A: { RUNNABLE: 1d400, B: { RUNNABLE: 2 } }, \
                          C: { RUNNABLE: 2 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            // No finite number states that magnitude, so it is no count a profile can
            // record and the frame's own ratio is the counts that remain.
            expected: 1.0 / 2.0,
        },
        Case {
            label: "a float magnitude past any finite number is no sample count",
            callgraph: "{ A: { RUNNABLE: 1e400, B: { RUNNABLE: 2 } }, \
                          C: { RUNNABLE: 2 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 1.0 / 2.0,
        },
        Case {
            label: "a count past a machine integer in a thread state the metric does not ask about",
            callgraph: "{ A: { WAITING: 18446744073709551616 }, B: { RUNNABLE: 2 } }",
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: 0.0,
        },
        Case {
            label: "a count past a machine integer under a frame name the profile does not hold",
            callgraph: "{ A: { RUNNABLE: 18446744073709551616 } }",
            frame_name: "NotInTheProfile",
            thread_states: &["RUNNABLE"],
            expected: 0.0,
        },
    ]
}

/// A group holding one SUBMITTED Ion profile recording `callgraph`, received inside the
/// window [`window_start`] names so the aggregation it is owed has fallen due.
async fn group_holding(harness: &Harness, name: &str, callgraph: &str, label: &str) {
    create_group(harness, name).await;
    let posted = harness
        .post_agent_profile(name, ION, None, ion_profile(callgraph))
        .await;
    assert_eq!(
        posted.status,
        204,
        "case {label:?}: posting the profile must succeed, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    place_receipts(harness, name, &[RECEIPT_SECONDS]);
}

/// The one-step series `name` reports for the metric on `frame_name` over
/// `thread_states`, having checked the datum names the metric the request asked for. A
/// position carrying no value — a step with no metric data — reads back as `None`.
async fn reported_series(
    harness: &Harness,
    name: &str,
    frame_name: &str,
    thread_states: &[&str],
    label: &str,
) -> Vec<Option<f64>> {
    let requested = frame_metric(frame_name, thread_states);
    let answer = frame_metric_data(
        harness,
        name,
        &format!(
            "?startTime={}&endTime={}&targetResolution=PT1H",
            iso(window_start()),
            iso(window_start() + HOUR)
        ),
        std::slice::from_ref(&requested),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {label:?}: expected HTTP 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let (metric, values) = datum(&answer, 0);
    assert_eq!(
        metric, requested,
        "case {label:?}: the datum must name the metric it was asked for, got {metric:?}"
    );
    values
}

/// A frame metric counts a sample count of any magnitude: the ratio is the counts the
/// document records, whichever Ion number type and notation state them.
#[tokio::test]
async fn a_sample_count_is_counted_at_the_magnitude_its_notation_states() {
    let harness = Harness::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("document-number-magnitude-{index}");
        group_holding(&harness, &name, case.callgraph, label).await;

        let values = reported_series(
            &harness,
            &name,
            case.frame_name,
            case.thread_states,
            label,
        )
        .await;
        assert_eq!(
            values.len(),
            1,
            "case {label:?}: expected one value per reported step, got {values:?}"
        );
        assert_value(&values, 0, Some(case.expected), label);
    }
}

/// The number at `path` in a served JSON document.
fn served_number(answer: &Answer, path: &[&str], label: &str) -> f64 {
    let mut value = answer.json();
    for step in path {
        value = value
            .get(step)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: no {step} in {value}"));
    }
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {label:?}: {value} is no number"))
}

/// Two counts of one frame in one window add up in the aggregated profile their profiles
/// combine into, at any magnitude — and the frame metric of that window counts both.
///
/// The window holds two profiles recording the same frame and thread state, so the
/// aggregated profile the window's profiles combine into records their sum: dropping
/// either count would report one of them, and the metric's ratio for a frame only one
/// profile records would move with it.
#[tokio::test]
async fn counts_of_one_frame_add_up_at_any_magnitude() {
    let harness = Harness::new();

    // Rows: the pair of counts the two profiles of the window record for frame `A`,
    // spanning a pair a machine integer holds, a pair past one, and a mixed pair — with
    // the sum each pair states.
    let rows: [(&str, &str, &str, f64); 3] = [
        ("counts a machine integer holds", "2", "3", 5.0),
        (
            "counts past an unsigned 64-bit integer",
            "18446744073709551616",
            "18446744073709551616",
            2.0 * 18_446_744_073_709_551_616.0,
        ),
        (
            "one count past a machine integer and one inside it",
            "18446744073709551616",
            "4",
            18_446_744_073_709_551_616.0 + 4.0,
        ),
    ];

    for (index, (label, first, second, sum)) in rows.into_iter().enumerate() {
        let name = format!("document-number-combination-{index}");
        create_group(&harness, &name).await;
        for (slot, count) in [first, second].into_iter().enumerate() {
            let posted = harness
                .post_agent_profile(
                    &name,
                    ION,
                    Some(&format!("token-{index}-{slot}")),
                    ion_profile(&format!("{{ A: {{ RUNNABLE: {count} }} }}")),
                )
                .await;
            assert_eq!(
                posted.status,
                204,
                "case {label:?}: posting a profile must succeed, got {} {}",
                posted.status,
                String::from_utf8_lossy(&posted.body)
            );
        }
        place_receipts(
            &harness,
            &name,
            &[RECEIPT_SECONDS, RECEIPT_SECONDS + 60],
        );

        // The aggregated profile of the window records the SUM of the two counts.
        let served = harness
            .get_profile_accepting(
                &name,
                &format!(
                    "?startTime={}&endTime={}",
                    iso(window_start()),
                    iso(window_start() + HOUR)
                ),
                Some(JSON),
            )
            .await;
        assert_eq!(
            served.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        let recorded = served_number(&served, &["callgraph", "A", "RUNNABLE"], label);
        assert!(
            (recorded - sum).abs() <= sum * TOLERANCE,
            "case {label:?}: expected the two counts added up to {sum}, got {recorded}"
        );

        // And the metric of that window is computed over both profiles: every count of
        // the window sits under `A`, so `A` accounts for all of them.
        let values = reported_series(&harness, &name, "A", &["RUNNABLE"], label).await;
        assert_eq!(
            values.len(),
            1,
            "case {label:?}: expected one value per reported step, got {values:?}"
        );
        assert_value(&values, 0, Some(1.0), label);
    }
}
