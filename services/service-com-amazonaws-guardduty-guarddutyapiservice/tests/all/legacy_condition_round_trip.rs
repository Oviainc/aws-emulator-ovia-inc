// SPDX-License-Identifier: CC0-1.0

//! Read-modify-write: what a read of a saved filter reports can be written straight
//! back, and the filter then reports the same thing again.
//!
//! The `Condition` members a read reports a comparison under are also members an
//! `UpdateFilter` request may carry, so the report is a request body: a client that
//! reads a filter, changes one member and writes the document back must not lose the
//! comparisons it did not touch, and must not see the criteria drift on a second pass.
//! The rule under test is that invariant — the reported criteria are a FIXED POINT of
//! the create/update round trip — over criteria written in either spelling.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

async fn send(router: &Router, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    call(router, method, path, body).await
}

/// Criteria spanning the spellings and types a condition can carry, as
/// `(why, criteria)`. Each is written in whichever spelling the row names; the read's
/// report of it is what the round trip then writes back.
fn criteria_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a deprecated equality list",
            json!({"criterion": {"severity": {"eq": ["8"]}}}),
        ),
        (
            "a current equality list",
            json!({"criterion": {"type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]}}}),
        ),
        (
            "a deprecated threshold",
            json!({"criterion": {"severity": {"gte": 4}}}),
        ),
        (
            "a current threshold past the 32-bit range",
            json!({"criterion": {"createdAt": {"greaterThanOrEqual": 1486685375000i64}}}),
        ),
        (
            "a pattern member, which has no second spelling",
            json!({"criterion": {"type": {"matches": ["Recon*"]}}}),
        ),
        (
            "several fields in both spellings at once",
            json!({"criterion": {
                "severity": {"gt": 2, "lessThanOrEqual": 8},
                "region": {"eq": ["us-east-1"]},
                "accountId": {"notEquals": ["111122223333"]},
            }}),
        ),
    ]
}

#[tokio::test]
async fn the_criteria_a_read_reports_can_be_written_straight_back() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (index, (case, criteria)) in criteria_rows().into_iter().enumerate() {
        let name = format!("round.trip-{index}");
        let (status, created) = send(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(json!({"name": name, "findingCriteria": criteria})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from the create, got {status} {created:?}"
        );

        let (status, first) = send(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the saved filter to read back, got {status} {first:?}"
        );
        let reported = first["findingCriteria"].clone();

        // What the read reported, written back unchanged — the modify step of a
        // read-modify-write that touched nothing.
        let (status, updated) = send(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter/{name}"),
            Some(json!({"findingCriteria": reported})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the reported criteria must be a body this service accepts, got {status} {updated:?}"
        );

        let (status, second) = send(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the updated filter to read back, got {status} {second:?}"
        );
        assert_eq!(
            second["findingCriteria"], reported,
            "case {case}: writing the reported criteria back must report the same criteria, got {second:?}"
        );
        // The comparison the create wrote is still there, under the spelling it was
        // written in: a round trip loses nothing.
        let sent = criteria["criterion"].as_object().expect("a criterion");
        for (field, condition) in sent {
            for (member, value) in condition.as_object().expect("a condition") {
                assert_eq!(
                    &second["findingCriteria"]["criterion"][field][member], value,
                    "case {case}: expected {field}.{member} {value} to survive the round trip, got {second:?}"
                );
            }
        }
    }
}
