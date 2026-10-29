// SPDX-License-Identifier: CC0-1.0

//! How `Scan` WORDS its refusal of a parallel-scan `Segment`/`TotalSegments` pair.
//!
//! The rule under test: a `Scan` whose parallel-scan pair breaks the pairing rule is
//! refused as a 400 `ValidationException` whose message is the service's FIXED sentence
//! for that refusal — the same string for every request that breaks it, naming the two
//! members the rule is about and interpolating no value the request carried. A client
//! that matches the refusal matches one string, so the message may not carry the
//! `Segment` or `TotalSegments` the caller sent.
//!
//! The rows differ along what the rule generalises over: which half of the pairing rule
//! is broken (a member supplied alone, a `Segment` equal to `TotalSegments`, a `Segment`
//! past it), and the magnitude of the two values — the smallest pair, a middling pair
//! and each published bound. The bounds come from the Smithy model (`smithy_query`
//! `shape` for `ScanSegment`, range 0..999999, and `ScanTotalSegments`, range
//! 1..1000000), so the pair at the top of both ranges is a VALID parallel scan while the
//! same `Segment` with the top `TotalSegments`-1 is not.
//!
//! [`every_pair_in_a_small_square_is_served_or_refused_by_the_one_rule`] then sweeps the
//! whole square of small pairs, so the rule is pinned as an invariant — inside the range
//! the scan is served, outside it the one sentence is stated — and not only on the rows
//! written out by hand.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

use smithy_server_runtime::MultiServiceRouter;

/// The sentence a `Segment` that is not inside its `TotalSegments` is refused with,
/// whole. Fixed text: it names the two members and reports neither value.
const SEGMENT_NOT_BELOW_TOTAL_SEGMENTS: &str =
    "The Segment parameter is zero-based and must be less than parameter TotalSegments";

/// The sentence a `Segment` supplied with no `TotalSegments` is refused with — "If you
/// provide Segment, you must also provide TotalSegments" (`Scan$Segment`).
const TOTAL_SEGMENTS_ABSENT: &str = "The TotalSegments parameter is required but was not present \
                                     in the request when Segment parameter is present";

/// The sentence a `TotalSegments` supplied with no `Segment` is refused with — "If you
/// specify TotalSegments, you must also specify Segment" (`Scan$TotalSegments`).
const SEGMENT_ABSENT: &str = "The Segment parameter is required but was not present in the \
                              request when parameter TotalSegments is present";

/// Seed a single-key table with a few items, so a refused scan is refused over a table
/// that could have answered it and the refusal is the request's own, not the table's.
async fn seeded_table(router: &MultiServiceRouter, name: &str) {
    create_active_table(router, name, &[("Artist", "HASH", "S")]).await;
    for artist in ["Acme Band", "Beta Quartet", "Gamma Trio"] {
        ok(
            router,
            "PutItem",
            json!({"TableName": name, "Item": {"Artist": {"S": artist}}}),
        )
        .await;
    }
}

/// Drive one `Scan` and answer `Ok` with its parsed body when it was served, or `Err`
/// with the message it was refused with after asserting the refusal's status and code.
async fn scan(router: &MultiServiceRouter, case: &str, request: Value) -> Result<Value, String> {
    let (status, body) = call(router, "Scan", request).await;
    if status == 200 {
        return Ok(body);
    }
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: a parallel-scan pairing refusal is a 400 ValidationException, got {status} \
         with {body}"
    );
    Err(message(&body))
}

#[tokio::test]
async fn a_refused_parallel_scan_pair_states_only_its_fixed_sentence() {
    let router = emulator();
    seeded_table(&router, "SegmentRefusals").await;

    // Each row: the case, the request's parallel-scan members as
    // `(Segment, TotalSegments)` with `None` for a member the request omits, and the
    // WHOLE message the refusal must carry.
    let cases: [(&str, Option<i64>, Option<i64>, &str); 9] = [
        (
            "Segment supplied alone",
            Some(0),
            None,
            TOTAL_SEGMENTS_ABSENT,
        ),
        (
            "a Segment past the sequential total, supplied alone",
            Some(7),
            None,
            TOTAL_SEGMENTS_ABSENT,
        ),
        ("TotalSegments supplied alone", None, Some(4), SEGMENT_ABSENT),
        (
            "the smallest TotalSegments, supplied alone",
            None,
            Some(1),
            SEGMENT_ABSENT,
        ),
        (
            "Segment equal to the sequential TotalSegments",
            Some(1),
            Some(1),
            SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
        ),
        (
            "Segment equal to a middling TotalSegments",
            Some(8),
            Some(8),
            SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
        ),
        (
            "Segment past TotalSegments",
            Some(9),
            Some(4),
            SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
        ),
        (
            "the largest Segment the model allows, past a sequential TotalSegments",
            Some(999_999),
            Some(1),
            SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
        ),
        (
            // One inside the top of `ScanTotalSegments`: 999999 segments number 0..999998.
            "the largest Segment the model allows, equal to a TotalSegments one under its own \
             ceiling",
            Some(999_999),
            Some(999_999),
            SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
        ),
    ];

    for (case, segment, total, expected) in cases {
        let mut request = json!({"TableName": "SegmentRefusals"});
        if let Some(segment) = segment {
            request["Segment"] = json!(segment);
        }
        if let Some(total) = total {
            request["TotalSegments"] = json!(total);
        }
        let stated = match scan(&router, case, request).await {
            Ok(served) => panic!("case {case:?}: the pair must be refused, got {served}"),
            Err(stated) => stated,
        };
        assert_eq!(
            stated, expected,
            "case {case:?}: expected exactly the fixed sentence {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(|character: char| character.is_ascii_digit()),
            "case {case:?}: the refusal reports no value the request carried, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_parallel_scan_pair_inside_the_modelled_ranges_is_served() {
    let router = emulator();
    seeded_table(&router, "SegmentAdmissions").await;

    // Each row: the case and the `(Segment, TotalSegments)` pair, spanning the smallest
    // parallel scan, a middling one, and the top of both published ranges — where the
    // largest `Segment` IS inside the largest `TotalSegments`.
    let cases: [(&str, i64, i64); 5] = [
        ("the sequential scan", 0, 1),
        ("the first of four segments", 0, 4),
        ("the last of four segments", 3, 4),
        ("a segment of a large partition", 512, 1_000),
        ("the largest segment of the largest partition", 999_999, 1_000_000),
    ];

    for (case, segment, total) in cases {
        let request = json!({
            "TableName": "SegmentAdmissions",
            "Segment": segment,
            "TotalSegments": total,
        });
        match scan(&router, case, request).await {
            Ok(served) => assert!(
                served.get("Count").is_some() && served.get("ScannedCount").is_some(),
                "case {case:?}: a served scan reports its counts, got {served}"
            ),
            Err(stated) => {
                panic!("case {case:?}: Segment {segment} of {total} is inside range, got {stated:?}")
            }
        }
    }
}

#[tokio::test]
async fn every_pair_in_a_small_square_is_served_or_refused_by_the_one_rule() {
    let router = emulator();
    seeded_table(&router, "SegmentSquare").await;

    // The invariant over the whole square of small pairs: a `Segment` inside
    // `0..TotalSegments` is served, every other pair is refused with the one sentence.
    // `TotalSegments` starts at 1, the minimum its shape allows.
    for total in 1..=6_i64 {
        for segment in 0..=6_i64 {
            let case = format!("Segment {segment} of TotalSegments {total}");
            let request = json!({
                "TableName": "SegmentSquare",
                "Segment": segment,
                "TotalSegments": total,
            });
            let outcome = scan(&router, &case, request).await;
            match (segment < total, outcome) {
                (true, Ok(served)) => assert!(
                    served["ScannedCount"].as_u64().is_some(),
                    "case {case:?}: a served scan reports ScannedCount, got {served}"
                ),
                (true, Err(stated)) => {
                    panic!("case {case:?}: the pair is inside range, got refusal {stated:?}")
                }
                (false, Ok(served)) => {
                    panic!("case {case:?}: the pair is outside range, got {served}")
                }
                (false, Err(stated)) => assert_eq!(
                    stated, SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
                    "case {case:?}: expected exactly the fixed sentence, got {stated:?}"
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_segment_under_its_modelled_minimum_is_refused_by_the_constraint_not_the_pairing() {
    let router = emulator();
    seeded_table(&router, "SegmentBelowZero").await;

    // `ScanSegment` has range min 0 (`smithy_query` `shape ScanSegment`), so a negative
    // `Segment` breaks the member's own constraint rather than the pairing rule: the
    // refusal is still a 400 `ValidationException`, and it must not be the pairing
    // sentence, which speaks about a relation this request never reached.
    for segment in [-1_i64, -1_000] {
        let case = format!("Segment {segment}");
        let request = json!({
            "TableName": "SegmentBelowZero",
            "Segment": segment,
            "TotalSegments": 4,
        });
        let stated = match scan(&router, &case, request).await {
            Ok(served) => panic!("case {case:?}: a negative Segment is refused, got {served}"),
            Err(stated) => stated,
        };
        assert!(
            stated.contains("segment") || stated.contains("Segment"),
            "case {case:?}: the refusal names the member it is about, got {stated:?}"
        );
        assert_ne!(
            stated, SEGMENT_NOT_BELOW_TOTAL_SEGMENTS,
            "case {case:?}: the pairing sentence is not this refusal, got {stated:?}"
        );
    }
}
