// SPDX-License-Identifier: CC0-1.0

//! `Rank` is a POSITION in a detector's filter list, and a delete closes the gap.
//!
//! `Rank` "specifies the position of the filter in the list of current filters" and
//! "the order in which this filter is applied to the findings", over `FilterRank`'s
//! 1..100 range (`smithy_query(operation="GetFilter")`). The rule under test:
//!
//! > dropping a filter removes a place from that list, so every filter ranked BEHIND
//! > the one that went away stands one place earlier afterwards, keeping its order
//! > among the survivors; a filter ranked ahead of it does not move, a filter carrying
//! > no rank has no position to move, and a delete of a filter that carried no rank
//! > frees no place.
//!
//! The rows span which position is vacated (the head, the middle, the tail, a sparse
//! layout, the ends of the published range), how many filters survive, and filters
//! with no rank at all — asserted on every survivor after each delete, not only on the
//! filter the recorded call read.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// A filter to create, as `(name, rank)`: `None` is a create that names no rank, which
/// reports none.
type Saved = (&'static str, Option<i32>);

/// One case: why it is interesting, the filters to create, the name to delete, and the
/// rank each survivor must report afterwards.
struct Case {
    why: &'static str,
    saved: Vec<Saved>,
    delete: &'static str,
    expected: Vec<(&'static str, Option<i32>)>,
}

async fn create_filter(router: &Router, detector_id: &str, name: &str, rank: Option<i32>) -> Value {
    let mut body = json!({
        "name": name,
        "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
    });
    if let Some(rank) = rank {
        body["rank"] = json!(rank);
    }
    let (status, created) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating {name} at rank {rank:?} must be served, got {status} {created:?}"
    );
    created
}

async fn delete_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn get_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            why: "the head of a dense sequence of two is vacated",
            saved: vec![("first", Some(1)), ("second", Some(2))],
            delete: "first",
            expected: vec![("second", Some(1))],
        },
        Case {
            why: "the tail of a dense sequence of two is vacated",
            saved: vec![("first", Some(1)), ("second", Some(2))],
            delete: "second",
            expected: vec![("first", Some(1))],
        },
        Case {
            why: "the middle of a dense sequence of four is vacated",
            saved: vec![
                ("one", Some(1)),
                ("two", Some(2)),
                ("three", Some(3)),
                ("four", Some(4)),
            ],
            delete: "two",
            expected: vec![
                ("one", Some(1)),
                ("three", Some(2)),
                ("four", Some(3)),
            ],
        },
        Case {
            why: "a sparse layout keeps its order and loses the place that went away",
            saved: vec![("low", Some(3)), ("mid", Some(20)), ("high", Some(90))],
            delete: "mid",
            expected: vec![("low", Some(3)), ("high", Some(89))],
        },
        Case {
            why: "the published minimum rank is vacated",
            saved: vec![("minimum", Some(1)), ("next", Some(2)), ("last", Some(100))],
            delete: "minimum",
            expected: vec![("next", Some(1)), ("last", Some(99))],
        },
        Case {
            why: "the published maximum rank is vacated, so nothing sits behind it",
            saved: vec![("maximum", Some(100)), ("ahead", Some(1))],
            delete: "maximum",
            expected: vec![("ahead", Some(1))],
        },
        Case {
            why: "a filter carrying no rank is deleted, which frees no place",
            saved: vec![("unranked", None), ("ranked", Some(5))],
            delete: "unranked",
            expected: vec![("ranked", Some(5))],
        },
        Case {
            why: "a filter carrying no rank survives a delete with no position to move",
            saved: vec![("first", Some(1)), ("second", Some(2)), ("unranked", None)],
            delete: "first",
            expected: vec![("second", Some(1)), ("unranked", None)],
        },
        Case {
            why: "the only filter on the detector is deleted",
            saved: vec![("alone", Some(7))],
            delete: "alone",
            expected: vec![],
        },
        Case {
            why: "two filters share a position, so neither is behind the other",
            saved: vec![("shared", Some(4)), ("twin", Some(4)), ("behind", Some(6))],
            delete: "shared",
            expected: vec![("twin", Some(4)), ("behind", Some(5))],
        },
    ]
}

#[tokio::test]
async fn a_delete_closes_the_gap_it_leaves_in_the_rank_sequence() {
    for case in cases() {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        for (name, rank) in &case.saved {
            create_filter(&router, &detector_id, name, *rank).await;
        }

        let (status, deleted) = delete_filter(&router, &detector_id, case.delete).await;
        assert_eq!(
            status, 200,
            "case {}: expected HTTP 200 from the delete, got {status} {deleted:?}",
            case.why
        );

        for (name, rank) in &case.expected {
            let (status, read) = get_filter(&router, &detector_id, name).await;
            assert_eq!(
                status, 200,
                "case {}: expected {name} to survive the delete, got {status} {read:?}",
                case.why
            );
            let reported = read["rank"].as_i64().map(|rank| rank as i32);
            assert_eq!(
                reported, *rank,
                "case {}: expected {name} at rank {rank:?} after {} went away, got {read:?}",
                case.why, case.delete
            );
        }

        // The deleted name is gone, so nothing above is read off a resurrected record.
        let (status, read) = get_filter(&router, &detector_id, case.delete).await;
        assert_eq!(
            status, 400,
            "case {}: expected the deleted name to be refused, got {status} {read:?}",
            case.why
        );
    }
}

#[tokio::test]
async fn a_refused_delete_moves_no_rank() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    for (name, rank) in [("first", Some(1)), ("second", Some(2))] {
        create_filter(&router, &detector_id, name, rank).await;
    }

    // A name the detector does not hold, and a detector id nothing minted: neither
    // delete removes a place, so no surviving rank moves.
    for (case, path) in [
        (
            "a filter name the detector does not hold",
            format!("/detector/{detector_id}/filter/absent.filter"),
        ),
        (
            "a detector id nothing minted",
            format!(
                "/detector/{}/filter/second",
                crate::detector_lifecycle::ABSENT_DETECTOR_ID
            ),
        ),
    ] {
        let (status, body) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the delete to be refused, got {status} {body:?}"
        );
        for (name, rank) in [("first", 1), ("second", 2)] {
            let (status, read) = get_filter(&router, &detector_id, name).await;
            assert_eq!(
                status, 200,
                "case {case}: expected {name} to read back, got {status} {read:?}"
            );
            assert_eq!(
                read["rank"],
                json!(rank),
                "case {case}: a refused delete frees no place, so {name} stays at rank {rank}, got {read:?}"
            );
        }
    }
}
