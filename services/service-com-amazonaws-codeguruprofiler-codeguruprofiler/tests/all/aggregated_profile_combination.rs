// SPDX-License-Identifier: CC0-1.0

//! What `GetProfile` serves for a range holding SEVERAL profiles, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts: the aggregated profile a request's range resolves to
//! is EVERY aggregated profile the group holds in that range combined into one document —
//! the service collects the profiles posted during an aggregation period into that
//! period's aggregated profile, and a range covering several of them is answered with
//! them combined as one aggregated profile — so frames of the same name combine, the
//! sample counts two profiles record for one frame add, a frame only one of them records
//! is in the answer too, and the combination covers from the earliest start its profiles
//! state. No profile of the range is left out of the answer.
//!
//! Rows vary what the rule generalises over: how many profiles the range holds (two,
//! three), whether their frames are disjoint, shared, nested or listed, how deep the
//! frames sit, whether the counts are whole or fractional, which starts the profiles
//! state, what else the documents state about themselves, whether the profiles share one
//! aggregation window or sit in several inside the range, which representation each one is
//! stored in, and what the request asks be done with the combination (`Accept`,
//! `maxDepth`).
//!
//! The emulator, the HTTP plumbing and the receipt placement are the shared
//! [`crate::profiling_group_lifecycle`] harness and [`crate::profile_times`] writers, not
//! a fork of either.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ion_rs::{Element, v1_0};

use crate::frame_metric_data::{TOLERANCE, assert_value, datum, frame_metric, frame_metric_data};
use crate::interval_validation::create_group;
use crate::profile_times::{place_profile, place_receipts};
use crate::profiling_group_lifecycle::{Harness, element_of};

/// The media types the model's `GetProfile$accept` doc names: "application/json —
/// standard JSON format" and "application/x-amzn-ion — the Amazon Ion data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// 2020-06-01T00:00:00Z: a whole number of days past the epoch, so an aggregation window
/// aligned to UTC starts here for each of the three periods, and long enough past that the
/// aggregation every placed profile is owed has fallen due.
const DAY_START: i64 = 1_590_969_600;

/// Five minutes in seconds — the narrowest aggregation period the model's
/// `AggregationPeriod` declares ("PT5M — 5 minutes"), so receipts this far apart sit in
/// different windows.
const FIVE_MINUTES: i64 = 5 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// An instant as the ISO 8601 `date-time` form the modelled query members carry.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// The expectation `document` states, in the one form a served document is compared in
/// ([`element_of`], the shared reader every suite in this target reads one through).
fn expected_element(document: &serde_json::Value) -> Element {
    element_of(&serde_json::to_vec(document).expect("a serializable document"))
}

/// `document` as the bytes an agent posts a profile as, in one of the two representations
/// this service serves.
fn posted_bytes(document: &serde_json::Value, representation: &str) -> Vec<u8> {
    let json = serde_json::to_vec(document).expect("a serializable document");
    if representation == ION {
        expected_element(document)
            .encode_as(v1_0::Binary)
            .expect("an encodable Ion document")
    } else {
        json
    }
}

/// One profile the group is posted: the document it states and the representation its
/// bytes are written in.
struct Posted {
    document: serde_json::Value,
    representation: &'static str,
}

/// One row: the profiles posted into one aggregation window, oldest first, and the
/// aggregated profile a request over that window must serve.
struct Row {
    label: &'static str,
    posted: Vec<Posted>,
    expected: serde_json::Value,
}

/// A profile stating its `start` and the call graph `callgraph`, which is the document
/// shape this service's profile readers walk (a frame is a container keyed by its name and
/// the numbers it carries are the samples observed for it).
fn profile(start: i64, callgraph: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "start": start, "callgraph": callgraph })
}

fn json_profile(start: i64, callgraph: serde_json::Value) -> Posted {
    Posted {
        document: profile(start, callgraph),
        representation: JSON,
    }
}

fn ion_profile(start: i64, callgraph: serde_json::Value) -> Posted {
    Posted {
        document: profile(start, callgraph),
        representation: ION,
    }
}

/// The table. Each row posts its profiles into ONE aggregation window and states the one
/// aggregated profile that window holds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "two profiles recording different frames",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 3 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 3 } }),
            ),
        },
        Row {
            label: "two profiles recording the same frame",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 4 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 5 } })),
        },
        Row {
            label: "three profiles, one frame shared by two of them",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 2 } })),
                json_profile(1_000, serde_json::json!({ "C": { "BLOCKED": 7 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 3 }, "C": { "BLOCKED": 7 } }),
            ),
        },
        Row {
            label: "frames nested under a shared caller",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } } }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "A": { "RUNNABLE": 4, "D": { "WAITING": 1 } } }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "A": { "RUNNABLE": 5, "B": { "RUNNABLE": 2 }, "D": { "WAITING": 1 } }
                }),
            ),
        },
        Row {
            label: "frames listed rather than keyed",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "frames": [{ "A": { "RUNNABLE": 1 } }] }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "frames": [{ "B": { "RUNNABLE": 2 } }] }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "frames": [{ "A": { "RUNNABLE": 1 } }, { "B": { "RUNNABLE": 2 } }]
                }),
            ),
        },
        Row {
            label: "fractional counts",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.5 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.25 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.75 } })),
        },
        Row {
            // The combination covers the periods of every profile it combines, so it
            // starts at the earliest of their starts, whichever profile was posted first.
            label: "the later profile states the earlier start",
            posted: vec![
                json_profile(2_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 2 } })),
        },
        Row {
            label: "profiles stored in different representations",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                ion_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 2 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 2 } }),
            ),
        },
        Row {
            // A member no document can state twice: the value the most recently received
            // profile reports is the one the combination carries, and the frames of both
            // are still in it.
            label: "a member the two profiles state different text for",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                Posted {
                    document: serde_json::json!({
                        "start": 1_000,
                        "agent": "second",
                        "callgraph": { "B": { "RUNNABLE": 1 } }
                    }),
                    representation: JSON,
                },
            ],
            expected: serde_json::json!({
                "start": 1_000,
                "callgraph": { "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 1 } },
                "agent": "second"
            }),
        },
        Row {
            // A frame of the call graph keyed with the name a profile states its period's
            // start under is a frame like any other: its counts combine, and neither
            // profile's frames under it are dropped.
            label: "a frame named like the period's start member",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "start": { "RUNNABLE": 1, "A": { "RUNNABLE": 2 } } }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "start": { "RUNNABLE": 4, "B": { "RUNNABLE": 8 } } }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "start": { "RUNNABLE": 5, "A": { "RUNNABLE": 2 }, "B": { "RUNNABLE": 8 } }
                }),
            ),
        },
        Row {
            // A number the DOCUMENT states is about the profile itself, not a count of
            // samples of a frame, so it does not add up: the period's start is the
            // earliest of the two, and any other such number is the one the most recently
            // received profile states.
            label: "numbers the documents state about themselves",
            posted: vec![
                Posted {
                    document: serde_json::json!({
                        "start": 2_000,
                        "end": 3_000,
                        "durationInMs": 500,
                        "callgraph": { "A": { "RUNNABLE": 1 } }
                    }),
                    representation: JSON,
                },
                Posted {
                    document: serde_json::json!({
                        "start": 1_000,
                        "end": 4_000,
                        "durationInMs": 700,
                        "callgraph": { "A": { "RUNNABLE": 2 } }
                    }),
                    representation: JSON,
                },
            ],
            expected: serde_json::json!({
                "start": 1_000,
                "end": 4_000,
                "durationInMs": 700,
                "callgraph": { "A": { "RUNNABLE": 3 } }
            }),
        },
        Row {
            label: "one profile, which is a combination of one",
            posted: vec![json_profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 } }),
            )],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
        },
    ]
}

/// Post `posted` to a freshly created group and place their receipts inside the one
/// five-minute window starting at [`DAY_START`], oldest first.
async fn group_holding(harness: &Harness, name: &str, posted: &[Posted], label: &str) {
    create_group(harness, name).await;
    for (index, profile) in posted.iter().enumerate() {
        let answer = harness
            .post_agent_profile(
                name,
                profile.representation,
                Some(&format!("ptok-{name}-{index}")),
                posted_bytes(&profile.document, profile.representation),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "case {label:?}: posting profile {index} must succeed, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let receipts: Vec<i64> = (0..posted.len())
        .map(|index| DAY_START + 10 + i64::try_from(index).unwrap_or(0))
        .collect();
    place_receipts(harness, name, &receipts);
}

/// The query naming the one five-minute window every row's receipts fall in.
fn window_query() -> String {
    format!(
        "?startTime={}&endTime={}",
        iso(DAY_START),
        iso(DAY_START + FIVE_MINUTES)
    )
}

#[tokio::test]
async fn an_aggregated_profile_holds_every_profile_of_the_requested_range() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let label = row.label;
        let name = format!("combined-{index}");
        group_holding(&harness, &name, &row.posted, label).await;

        let answer = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = element_of(&answer.body);
        let expected = expected_element(&row.expected);
        assert_eq!(
            served, expected,
            "case {label:?}: expected {expected}, got {served}"
        );

        // Serving the combination changed nothing: the next read of the same window is
        // the same aggregated profile.
        let again = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            element_of(&again.body),
            expected,
            "case {label:?}: a read must not change what the next caller is served, got {}",
            element_of(&again.body)
        );
    }
}

/// The combination is what the request's `accept` and `maxDepth` are applied to: it is
/// served in either representation the caller asks for, and bounded to the depth asked
/// for, exactly as a single stored profile is.
#[tokio::test]
async fn the_combination_is_what_accept_and_max_depth_are_applied_to() {
    let harness = Harness::new();
    let name = "combined-asked";
    group_holding(
        &harness,
        name,
        &[
            json_profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } } }),
            ),
            json_profile(1_000, serde_json::json!({ "C": { "RUNNABLE": 4 } })),
        ],
        "accept and maxDepth",
    )
    .await;
    let whole = serde_json::json!({
        "start": 1_000,
        "callgraph": { "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } }, "C": { "RUNNABLE": 4 } }
    });

    // Each representation the model names, and a range that allows either.
    for (label, accept, announced) in [
        ("JSON", Some(JSON), JSON),
        ("Ion", Some(ION), ION),
        // Both profiles are stored in JSON, so that is the representation a request
        // accepting anything leaves the combination in.
        ("anything", Some("*/*"), JSON),
        ("no Accept at all", None, JSON),
    ] {
        let answer = harness
            .get_profile_accepting(name, &window_query(), accept)
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(announced),
            "case {label:?}: expected {announced}, got {:?}",
            answer.content_type
        );
        let served = element_of(&answer.body);
        assert_eq!(
            served,
            expected_element(&whole),
            "case {label:?}: expected the combination, got {served}"
        );
    }

    // `maxDepth`: "if the maxDepth is set to 2, then the aggregated profile contains
    // representations of methods A and B" — the frames of the COMBINATION nested deeper
    // than it are the ones left out, so a depth bound still sees every profile's frames
    // above it.
    for (max_depth, expected) in [
        (1i64, serde_json::json!({ "start": 1_000, "callgraph": {} })),
        (
            2,
            serde_json::json!({ "start": 1_000, "callgraph": { "A": {}, "C": {} } }),
        ),
        (
            3,
            serde_json::json!({
                "start": 1_000,
                "callgraph": { "A": { "RUNNABLE": 1, "B": {} }, "C": { "RUNNABLE": 4 } }
            }),
        ),
        (4, whole.clone()),
        (10_000, whole.clone()),
    ] {
        let answer = harness
            .get_profile(name, &format!("{}&maxDepth={max_depth}", window_query()))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case depth {max_depth}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = element_of(&answer.body);
        assert_eq!(
            served,
            expected_element(&expected),
            "case depth {max_depth}: expected {expected}, got {served}"
        );
    }
}

/// A range covering several aggregation WINDOWS is answered with the aggregated profiles
/// of all of them combined — "CodeGuru Profiler makes a best effort to combine existing
/// aggregated profiles from the requested time range and return them as one aggregated
/// profile" — while a range covering one of those windows is answered with that window's
/// profiles alone.
#[tokio::test]
async fn profiles_of_several_windows_in_the_range_are_combined() {
    let harness = Harness::new();
    let name = "combined-windows";
    create_group(&harness, name).await;

    // One profile per window, three windows apart inside the same hour.
    let placed: [(i64, &str, i64); 3] = [
        (DAY_START + 10, "A", 1_000),
        (DAY_START + FIVE_MINUTES + 10, "B", 2_000),
        (DAY_START + 2 * FIVE_MINUTES + 10, "C", 3_000),
    ];
    for (index, (_, frame, start)) in placed.iter().enumerate() {
        let answer = harness
            .post_agent_profile(
                name,
                JSON,
                Some(&format!("ptok-windows-{index}")),
                posted_bytes(
                    &profile(*start, serde_json::json!({ *frame: { "RUNNABLE": 1 } })),
                    JSON,
                ),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "posting the profile of window {index} must succeed, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let receipts: Vec<i64> = placed.iter().map(|(at, _, _)| *at).collect();
    place_receipts(&harness, name, &receipts);

    // (the case, how many windows the range covers, the aggregated profile it serves)
    let cases: [(&str, i64, serde_json::Value); 3] = [
        (
            "one window",
            1,
            profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
        ),
        (
            "two windows",
            2,
            profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 1 } }),
            ),
        ),
        (
            "three windows",
            3,
            profile(
                1_000,
                serde_json::json!({
                    "A": { "RUNNABLE": 1 },
                    "B": { "RUNNABLE": 1 },
                    "C": { "RUNNABLE": 1 }
                }),
            ),
        ),
    ];
    for (label, windows, expected) in cases {
        let answer = harness
            .get_profile(
                name,
                &format!(
                    "?startTime={}&endTime={}",
                    iso(DAY_START),
                    iso(DAY_START + windows * FIVE_MINUTES)
                ),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = element_of(&answer.body);
        assert_eq!(
            served,
            expected_element(&expected),
            "case {label:?}: expected {expected}, got {served}"
        );
    }
}

/// A stored profile whose bytes are a document in neither representation this service
/// reads holds no frames it can see, so it contributes none to the combination — and
/// where it is the only profile of the range, that range is still served exactly as those
/// bytes are stored.
#[tokio::test]
async fn bytes_that_are_no_document_contribute_nothing_to_the_combination() {
    let harness = Harness::new();
    let opaque = b"{\"callgraph\":".to_vec();

    // (the case, the readable profiles beside the opaque bytes, what the range serves)
    let readable = profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } }));
    for (index, (label, readable_profiles, expected)) in [
        (
            "one readable profile beside it",
            vec![readable.clone()],
            Some(readable.clone()),
        ),
        (
            "two readable profiles beside it",
            vec![
                readable.clone(),
                profile(2_000, serde_json::json!({ "B": { "RUNNABLE": 2 } })),
            ],
            Some(profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 2 } }),
            )),
        ),
        ("no readable profile at all", Vec::new(), None),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("combined-opaque-{index}");
        create_group(&harness, &name).await;
        for (post, document) in readable_profiles.iter().enumerate() {
            let answer = harness
                .post_agent_profile(
                    &name,
                    JSON,
                    Some(&format!("ptok-opaque-{index}-{post}")),
                    posted_bytes(document, JSON),
                )
                .await;
            assert_eq!(
                answer.status,
                204,
                "case {label:?}: posting profile {post} must succeed, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        // Bytes that are no document cannot be SUBMITTED — `PostAgentProfile` refuses
        // data that is not an agent profile — so they are placed in the store through the
        // one shared writer, received last.
        place_receipts(
            &harness,
            &name,
            &(0..readable_profiles.len())
                .map(|post| DAY_START + 10 + i64::try_from(post).unwrap_or(0))
                .collect::<Vec<i64>>(),
        );
        place_profile(
            &harness,
            &name,
            JSON,
            opaque.clone(),
            DAY_START + 10 + i64::try_from(readable_profiles.len()).unwrap_or(0),
        );

        let answer = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        match expected {
            Some(expected) => assert_eq!(
                element_of(&answer.body),
                expected_element(&expected),
                "case {label:?}: expected {expected}, got {}",
                element_of(&answer.body)
            ),
            None => assert_eq!(
                answer.body, opaque,
                "case {label:?}: expected the stored bytes, got {:?}",
                String::from_utf8_lossy(&answer.body)
            ),
        }
    }
}

/// The two readers of a group's aggregated data agree about what a window holds: the
/// frame metric `BatchGetFrameMetricData` reports for a window is the ratio the aggregated
/// profile `GetProfile` serves for that same window records, so a frame that appears in
/// only one of the window's profiles is in both answers.
#[tokio::test]
async fn the_served_combination_records_the_counts_a_frame_metric_reports() {
    let harness = Harness::new();
    let name = "combined-metric";
    group_holding(
        &harness,
        name,
        &[
            json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
            json_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 3 } })),
        ],
        "frame metric",
    )
    .await;

    let served = element_of(&harness.get_profile(name, &window_query()).await.body);
    for (frame, expected) in [("A", 0.25), ("B", 0.75)] {
        // What the served document records for the frame, read the way the metric's own
        // definition reads it: the counts under that frame over the counts of the whole
        // profile.
        let under = counts_under(&served, frame);
        let whole = counts_under(&served, "callgraph");
        assert!(
            whole > 0.0,
            "frame {frame}: the served combination must record counts, got {served}"
        );
        assert!(
            (under / whole - expected).abs() < TOLERANCE,
            "frame {frame}: the served combination must record {expected}, got {} in \
             {served}",
            under / whole
        );

        let requested = frame_metric(frame, &["RUNNABLE"]);
        let answer = frame_metric_data(
            &harness,
            name,
            &format!(
                "?startTime={}&endTime={}&targetResolution=PT1H",
                iso(DAY_START - DAY_START.rem_euclid(HOUR)),
                iso(DAY_START - DAY_START.rem_euclid(HOUR) + HOUR)
            ),
            std::slice::from_ref(&requested),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "frame {frame}: the metric must answer 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            1,
            "frame {frame}: expected one value per step, got {values:?}"
        );
        assert_value(
            &values,
            0,
            Some(under / whole),
            &format!("frame {frame}"),
        );
    }
}

/// The `RUNNABLE` counts `document` records inside the frame named `frame`, summed over
/// every frame of that name — the counts half of the metric's own definition, computed
/// here from the SERVED document so the two answers are compared rather than restated.
fn counts_under(document: &Element, frame: &str) -> f64 {
    fn walk(node: &Element, frame: &str, inside: bool, total: &mut f64) {
        match node.value() {
            ion_rs::Value::Struct(fields) => {
                for (name, value) in fields.fields() {
                    let name = name.text().unwrap_or_default();
                    match value.value() {
                        ion_rs::Value::Int(count) if name == "RUNNABLE" => {
                            if inside {
                                *total += count.expect_i64().unwrap_or_default() as f64;
                            }
                        }
                        _ => walk(value, frame, inside || name == frame, total),
                    }
                }
            }
            ion_rs::Value::List(items) | ion_rs::Value::SExp(items) => {
                for item in items.elements() {
                    walk(item, frame, inside, total);
                }
            }
            _ => {}
        }
    }
    let mut total = 0.0;
    walk(document, frame, false, &mut total);
    total
}
