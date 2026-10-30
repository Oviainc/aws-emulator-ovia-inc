// SPDX-License-Identifier: CC0-1.0

//! What `GetProfile` serves for a request that names the window's LENGTH and neither of
//! its bounds, end to end over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a length names no window — it says how long the
//! aggregated profile wanted is, not where it sits — so such a request is served the
//! group's LATEST aggregated profile of that length: the profiles of the last window of
//! that length, aligned to UTC, that the group has anything aggregated in, combined, and
//! no profile of any earlier window. The length therefore decides which data is served,
//! and a group holding aggregated profiles in several windows of that length is answered
//! with the newest of them rather than with all of them combined.
//!
//! SOURCE: the operation's own doc — "If you specify only period, the latest aggregated
//! profile is returned" and "Aggregated profiles are available with aggregation periods of
//! 5 minutes, 1 hour, and 1 day, aligned to UTC"
//! (`smithy_query(query="doc", shape="GetProfile")`) — with `period` targeting the
//! free-form `Period` string ("To get the latest aggregated profile, specify only period",
//! `member="period"`), which is why a length that is no modelled `AggregationPeriod` value
//! is a row too.
//!
//! Rows vary what the rule generalises over: the length named (the three periods the model
//! declares, one between them, one shorter than a second, one spelled with every field,
//! and the 7-day maximum the operation allows), whether the length's windows are aligned
//! with the instant the receipts are placed around, how many windows the group holds
//! profiles in, how many profiles the latest of those windows holds, and how far apart the
//! windows sit. The window a row's receipts fall in is worked out from the length alone —
//! each row states the profiles its latest window holds — so the expectation is not the
//! handler's own arithmetic read back.
//!
//! The sibling rules for the same operation are [`crate::aggregated_profile_combination`]
//! (what a range holding several profiles serves), [`crate::get_profile_window_companion`]
//! (which members may be named) and [`crate::get_profile_window_length`] (how long a
//! window may be).
//!
//! The emulator, the HTTP plumbing and the receipt placement are the shared
//! [`crate::profiling_group_lifecycle`] harness and [`crate::profile_times`] writers, not
//! a fork of either.

use crate::interval_validation::create_group;
use crate::profile_times::place_receipts_at;
use crate::profiling_group_lifecycle::{Harness, element_of};

/// The media type every profile here is posted and served in, one of the two the model's
/// `GetProfile$accept` doc names.
const JSON: &str = "application/json";

/// 2020-06-01T00:00:00Z, the instant these cases place their receipts around: long past,
/// so the aggregation every placed profile is owed has fallen due and the only thing
/// deciding what is served is the length the request names.
const DAY_START_MILLIS: i64 = 1_590_969_600_000;

/// The `start` the first profile of a group states, in the milliseconds a profile document
/// states it in; each later profile states one millisecond more, so the combination's own
/// start names the EARLIEST profile it holds and a profile from an earlier window leaking
/// into the answer would show up in it.
const PROFILE_START_MILLIS: i64 = 1_600_000_000_000;

/// One receipt time, as milliseconds from [`DAY_START_MILLIS`] (negative is earlier).
type Offset = i64;

/// One group's stored profiles, the length a request names, and the profiles the latest
/// window of that length holds.
struct Row {
    label: &'static str,
    /// The receipt times of the group's profiles, oldest first.
    receipts: Vec<Offset>,
    /// The ISO 8601 length the request names as `period`, and nothing else.
    period: &'static str,
    /// Indices into `receipts` of the profiles the LAST window of that length holds — the
    /// aggregated profile the request must be served.
    expected: Vec<usize>,
}

/// One minute, one hour and one day in milliseconds, for readable receipt arithmetic.
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The table.
///
/// Each row's `expected` is worked out from the length and the epoch-aligned windows it
/// names: a 5-minute, 1-hour, 1-day and 1.5-second window all start at
/// [`DAY_START_MILLIS`] (a whole number of each past the epoch), while a 7-minute window
/// starts 6 minutes before it (the epoch is 3 788 022 whole 7-minute windows plus 6
/// minutes earlier), a `P1DT1H1M1S` window 42 035 seconds before it, and the 7-day window
/// holding it starts 4 days earlier.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "three five-minute windows, a five-minute length",
            receipts: vec![0, 10 * MINUTE, 20 * MINUTE],
            period: "PT5M",
            expected: vec![2],
        },
        Row {
            label: "three five-minute windows inside one hour, an hour-long length",
            receipts: vec![0, 10 * MINUTE, 20 * MINUTE],
            period: "PT1H",
            expected: vec![0, 1, 2],
        },
        Row {
            label: "two profiles in the latest five-minute window",
            receipts: vec![0, 10 * MINUTE, 20 * MINUTE, 20 * MINUTE + 30_000],
            period: "PT5M",
            expected: vec![2, 3],
        },
        Row {
            label: "every profile in one five-minute window",
            receipts: vec![0, 1_000, 2_000],
            period: "PT5M",
            expected: vec![0, 1, 2],
        },
        Row {
            label: "two hours apart, an hour-long length",
            receipts: vec![0, 2 * HOUR, 2 * HOUR + 10 * MINUTE],
            period: "PT1H",
            expected: vec![1, 2],
        },
        Row {
            label: "two days apart, a day-long length",
            receipts: vec![-DAY, 0, 20 * MINUTE],
            period: "P1D",
            expected: vec![1, 2],
        },
        Row {
            label: "two days apart, a five-minute length",
            receipts: vec![-DAY, 0, 20 * MINUTE],
            period: "PT5M",
            expected: vec![2],
        },
        // A length that is no modelled `AggregationPeriod` value, and whose windows are
        // NOT aligned with the instant the receipts sit around: the windows run
        // [-6min, +1min), [+1min, +8min), [+8min, +15min), [+15min, +22min), so the last
        // one holding anything holds the 20-minute receipt alone.
        Row {
            label: "a seven-minute length, whose windows the receipts straddle",
            receipts: vec![0, 10 * MINUTE, 20 * MINUTE],
            period: "PT7M",
            expected: vec![2],
        },
        // A length shorter than a second, which aligns at sub-second resolution: 1.5-second
        // windows run [0, 1.5s), [1.5s, 3s), so the first two receipts share a window and
        // the last one is alone in the next.
        Row {
            label: "a length shorter than a second",
            receipts: vec![0, 1_000, 2_000],
            period: "PT1.5S",
            expected: vec![2],
        },
        // The member doc's own example length, spelled with every field (P1DT1H1M1S =
        // 90 061 s): the window holding the receipts starts 42 035 s before them, so a
        // receipt 50 000 s earlier sits in the window before it.
        Row {
            label: "a length spelled with every field",
            receipts: vec![-50_000_000, 0, 20 * MINUTE],
            period: "P1DT1H1M1S",
            expected: vec![1, 2],
        },
        // The longest range the operation allows ("The maximum time range allowed is 7
        // days"): the 7-day window holding the receipts starts 4 days before them, so a
        // receipt 8 days earlier is in the window before it.
        Row {
            label: "the longest length the operation allows",
            receipts: vec![-8 * DAY, 0, 20 * MINUTE],
            period: "P7D",
            expected: vec![1, 2],
        },
    ]
}

/// The document the profile at `index` of a group states: its own `start`, and one frame
/// of its own carrying the samples observed for it, so which profiles an answer holds is
/// readable off the answer.
fn document(index: usize) -> serde_json::Value {
    let counted = i64::try_from(index).unwrap_or(0);
    serde_json::json!({
        "start": PROFILE_START_MILLIS + counted,
        "callgraph": { format!("F{index}"): { "RUNNABLE": counted + 1 } },
    })
}

/// The bytes the profile at `index` is posted as.
fn posted_bytes(index: usize) -> Vec<u8> {
    serde_json::to_vec(&document(index)).expect("a serializable document")
}

/// The aggregated profile the profiles `indices` were collected into: every frame of each,
/// in posting order, under the EARLIEST start they state (the combination covers the
/// periods of all of them).
fn expected_document(indices: &[usize]) -> serde_json::Value {
    let earliest = indices.iter().copied().min().unwrap_or(0);
    let mut callgraph = serde_json::Map::new();
    for index in indices {
        let counted = i64::try_from(*index).unwrap_or(0);
        callgraph.insert(
            format!("F{index}"),
            serde_json::json!({ "RUNNABLE": counted + 1 }),
        );
    }
    serde_json::json!({
        "start": PROFILE_START_MILLIS + i64::try_from(earliest).unwrap_or(0),
        "callgraph": serde_json::Value::Object(callgraph),
    })
}

/// `millis` from [`DAY_START_MILLIS`] as the (epoch seconds, sub-second nanoseconds) pair a
/// receipt is stored as.
fn receipt(millis: Offset) -> (i64, u32) {
    let total = DAY_START_MILLIS + millis;
    (
        total.div_euclid(1_000),
        u32::try_from(total.rem_euclid(1_000)).expect("a millisecond inside a second")
            * 1_000_000,
    )
}

/// A freshly created group holding one posted profile per receipt, placed at those
/// receipts.
async fn group_holding(harness: &Harness, name: &str, receipts: &[Offset], label: &str) {
    create_group(harness, name).await;
    for index in 0..receipts.len() {
        let answer = harness
            .post_agent_profile(
                name,
                JSON,
                Some(&format!("ptok-{name}-{index}")),
                posted_bytes(index),
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
    let placed: Vec<(i64, u32)> = receipts.iter().copied().map(receipt).collect();
    place_receipts_at(harness, name, &placed);
}

#[tokio::test]
async fn a_length_alone_serves_the_latest_aggregated_profile_of_that_length() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let name = format!("latest-{index}");
        let case = format!("{} (?period={})", row.label, row.period);
        group_holding(&harness, &name, &row.receipts, &case).await;

        let answer = harness
            .get_profile(&name, &format!("?period={}", row.period))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        let served = element_of(&answer.body);
        let expected = element_of(
            &serde_json::to_vec(&expected_document(&row.expected))
                .expect("a serializable document"),
        );
        assert_eq!(
            served, expected,
            "case {case}: expected the profiles {:?} of the latest window, got {served}",
            row.expected
        );

        // A window holding ONE profile is that profile, so its stored bytes are served as
        // they were uploaded.
        if let [only] = row.expected[..] {
            assert_eq!(
                answer.body,
                posted_bytes(only),
                "case {case}: expected profile {only}'s stored bytes, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }

        // No frame of a window the request did not ask about is in the answer — the
        // consequence the combination of every window would show.
        for left_out in 0..row.receipts.len() {
            if row.expected.contains(&left_out) {
                continue;
            }
            let frame = format!("F{left_out}");
            assert!(
                !String::from_utf8_lossy(&answer.body).contains(&frame),
                "case {case}: profile {left_out} sits in an earlier window and must not be \
                 in the answer, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

/// The length a request names decides WHICH data is served, not only whether the request
/// is within the maximum range: on one group holding profiles in three five-minute windows
/// inside one hour, a five-minute length is answered with the latest of them and an
/// hour-long length with all three — two different bodies for two different lengths.
///
/// Reading them changes nothing a later caller sees: each read is repeated and must agree
/// with itself.
#[tokio::test]
async fn the_length_named_decides_which_aggregated_profile_is_served() {
    let harness = Harness::new();
    let name = "latest-by-length";
    let receipts = [0, 10 * MINUTE, 20 * MINUTE];
    group_holding(&harness, name, &receipts, "by length").await;

    // (the length, the profiles the latest window of that length holds)
    let lengths: [(&str, &[usize]); 4] = [
        ("PT5M", &[2]),
        ("PT1H", &[0, 1, 2]),
        ("P1D", &[0, 1, 2]),
        ("PT1S", &[2]),
    ];
    let mut bodies: Vec<(&str, Vec<u8>)> = Vec::new();
    for (period, expected) in lengths {
        let case = format!("?period={period}");
        let answer = harness.get_profile(name, &case).await;
        assert_eq!(
            answer.status,
            200,
            "case {case}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = element_of(&answer.body);
        let wanted = element_of(
            &serde_json::to_vec(&expected_document(expected)).expect("a serializable document"),
        );
        assert_eq!(
            served, wanted,
            "case {case}: expected the profiles {expected:?}, got {served}"
        );

        let again = harness.get_profile(name, &case).await;
        assert_eq!(
            again.body, answer.body,
            "case {case}: two reads of one aggregated profile must agree, got {:?} then {:?}",
            String::from_utf8_lossy(&answer.body),
            String::from_utf8_lossy(&again.body)
        );
        bodies.push((period, answer.body));
    }

    // The two lengths whose latest window holds different profiles are answered
    // differently: the value of `period` is not ignored.
    let five_minutes = &bodies[0];
    let an_hour = &bodies[1];
    assert_ne!(
        five_minutes.1, an_hour.1,
        "case {:?} against {:?}: two lengths selecting different windows must not serve \
         the same body, got {:?}",
        five_minutes.0,
        an_hour.0,
        String::from_utf8_lossy(&five_minutes.1)
    );
}
