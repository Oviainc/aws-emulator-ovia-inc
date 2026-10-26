// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when a saved filter
//! is added to it or taken off it.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of filter creates rather than one of them:
//! a call that changes WHICH saved sub-resources a detector holds changes the detector
//! record itself, so `GetDetector`'s `UpdatedAt` moves to the instant of that call —
//! while a call that leaves that set alone moves it nowhere, whether it changed one
//! sub-resource's own contents (an update), minted nothing (an idempotent replay, a
//! refusal) or changed nothing at all (a read). `detector_sub_resource_creates` asserts
//! the same rule over the detector's other saved sub-resource families; this suite
//! covers the filter family, whose create answers with the caller's `Name` instead of a
//! minted id and whose delete is addressed by that name.
//!
//! The rows differ along what the rule generalises over — which members the create
//! carries (`Action`, `Rank`, `Description`, `Tags`, `ClientToken`), the form of the
//! `FindingCriteria.Criterion` it saves, the length of the `Name` within its published
//! bound, and which of the two handler-enforced rules the refused row breaks. The
//! member catalogue comes from `smithy_query(operation="CreateFilter,UpdateFilter")`
//! and `smithy_query(shape="Condition,FilterAction,FilterRank")`: `FilterAction` is
//! ARCHIVE | NOOP, `FilterRank` has range 1..100 and `Name` length 3..64.
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these
//! assertions order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// One filter create, with the refusal and the update that bracket it.
struct Row {
    case: &'static str,
    /// The `Name` the create asks for, which is also the delete's URI label.
    name: &'static str,
    /// A create this service accepts.
    create: Value,
    /// A create this service refuses, so nothing is saved and the detector is
    /// unchanged by it.
    refused: Value,
    /// An `UpdateFilter` body: it changes the filter's own contents and so leaves the
    /// SET of sub-resources the detector holds exactly as it was.
    update: Value,
}

/// The rows, spanning the create members and criterion forms a saved filter can
/// carry. Each row's `name` stays within the modelled 3..64 bound and the documented
/// alphabet (period, underscore, dash and alphanumerics).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest permitted name, no optional member at all",
            name: "a.b",
            create: json!({
                "name": "a.b",
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 7}}},
            }),
            // A name carrying whitespace is outside the documented alphabet.
            refused: json!({
                "name": "refused name",
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 7}}},
            }),
            update: json!({"rank": 1}),
        },
        Row {
            case: "ARCHIVE at the bottom of the rank range, described and tagged",
            name: "archive-everything_low",
            create: json!({
                "name": "archive-everything_low",
                "action": "ARCHIVE",
                "rank": 1,
                "description": "suppress the noisy port probes",
                "tags": {"team": "secops", "env": "test"},
                "findingCriteria": {
                    "criterion": {
                        "type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]},
                    },
                },
            }),
            // A criterion key that names no field of a finding is refused.
            refused: json!({
                "name": "refused-criterion-key",
                "findingCriteria": {"criterion": {"notAFindingField": {"equals": ["x"]}}},
            }),
            update: json!({"description": "narrowed", "action": "NOOP"}),
        },
        Row {
            case: "NOOP at the top of the rank range, two comparisons on one field",
            name: "noop.rank.top-100",
            create: json!({
                "name": "noop.rank.top-100",
                "action": "NOOP",
                "rank": 100,
                "clientToken": "filter-token-rank-top",
                "findingCriteria": {
                    "criterion": {"severity": {"greaterThan": 2, "lessThanOrEqual": 8}},
                },
            }),
            // `Rank` is modelled range 1..100, so 0 is outside it and the boundary
            // walk refuses the request before the handler runs.
            refused: json!({
                "name": "refused-rank",
                "rank": 0,
                "findingCriteria": {"criterion": {"severity": {"greaterThan": 2}}},
            }),
            update: json!({"findingCriteria": {"criterion": {"severity": {"lessThan": 3}}}}),
        },
        Row {
            case: "a 64-character name, a timestamp criterion in epoch milliseconds",
            name: "n64.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            create: json!({
                "name": "n64.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "clientToken": "filter-token-long-name",
                "findingCriteria": {
                    "criterion": {"createdAt": {"greaterThanOrEqual": 1486685375000i64}},
                },
            }),
            // `Name` is modelled length 3..64, so a 65-character name is outside it.
            refused: json!({
                "name": "n65.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "findingCriteria": {"criterion": {"createdAt": {"lessThan": 1486685375000i64}}},
            }),
            update: json!({}),
        },
        Row {
            case: "a wildcard criterion over several members, no action or rank",
            name: "matches_resource-arn",
            create: json!({
                "name": "matches_resource-arn",
                "description": "",
                "findingCriteria": {
                    "criterion": {
                        "resource.instanceDetails.instanceId": {"matches": ["i-*"]},
                        "region": {"notEquals": ["us-west-2"]},
                    },
                },
            }),
            // `Action` is `FilterAction`: ARCHIVE | NOOP and nothing else.
            refused: json!({
                "name": "refused-action",
                "action": "DELETE",
                "findingCriteria": {"criterion": {"region": {"equals": ["us-east-1"]}}},
            }),
            update: json!({"rank": 50}),
        },
    ]
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// The detector's `UpdatedAt` as the string a client sees.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let got = read_detector(router, detector_id, case).await;
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The rule: adding a filter to a detector, and taking it off again, each move the
// detector's UpdatedAt; a call that leaves the set of sub-resources alone does not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adding_or_removing_a_filter_moves_the_detectors_updated_at() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass on another row's
        // write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let collection = format!("/detector/{detector_id}/filter");

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A refused create saves nothing, so the detector is unchanged by it.
        let (status, refusal) = call(&router, "POST", &collection, Some(row.refused.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refused create must answer 400, got {status} {refusal:?}"
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused create must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        // A read is not a change either.
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, updated_before,
            "case {case:?}: reading the detector must not move its updatedAt, expected \
             {updated_before:?}, got {after_read:?}"
        );

        // The accepted create: the detector now holds one more sub-resource, so its
        // UpdatedAt moves.
        let (status, created) = call(&router, "POST", &collection, Some(row.create.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );
        assert_eq!(
            created["name"].as_str(),
            Some(row.name),
            "case {case:?}: expected the saved filter's name back, got {created:?}"
        );

        let after_create = read_detector(&router, &detector_id, case).await;
        let updated_after_create = after_create["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {after_create:?}"))
            .to_string();
        assert!(
            updated_after_create > updated_before,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {updated_before:?}, got {updated_after_create:?}"
        );
        assert_eq!(
            after_create["createdAt"], created_at,
            "case {case:?}: adding a filter must not move the detector's createdAt, expected \
             {created_at:?}, got {:?}",
            after_create["createdAt"]
        );

        // Round trip: the name the create answered with is a filter a read resolves, so
        // the moved UpdatedAt records a change that really landed in the store.
        let (status, read_back) = call(
            &router,
            "GET",
            &format!("{collection}/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the saved filter to answer 200, got {status} \
             {read_back:?}"
        );
        assert_eq!(
            read_back["name"].as_str(),
            Some(row.name),
            "case {case:?}: expected the saved filter to read back under its name, got \
             {read_back:?}"
        );

        // A second create of the same name saves nothing: it is either the idempotent
        // replay of the token the row carried, or a refusal of the name already taken.
        // Either way the detector holds the same sub-resources, so nothing moves.
        let (status, replayed) = call(&router, "POST", &collection, Some(row.create.clone())).await;
        if row.create.get("clientToken").is_some() {
            assert_eq!(
                status, 200,
                "case {case:?}: a replayed clientToken must answer 200, got {status} {replayed:?}"
            );
            assert_eq!(
                replayed["name"].as_str(),
                Some(row.name),
                "case {case:?}: a replayed clientToken must return the first call's name, got \
                 {replayed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a repeated create of a name the detector holds must be refused, \
                 got {status} {replayed:?}"
            );
        }
        let after_replay = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_replay, updated_after_create,
            "case {case:?}: a create that saves nothing must not move the detector's updatedAt, \
             expected {updated_after_create:?}, got {after_replay:?}"
        );

        // An update changes the filter's own contents, not WHICH sub-resources the
        // detector holds, so the detector's own UpdatedAt stays where the create put it.
        let (status, updated) = call(
            &router,
            "POST",
            &format!("{collection}/{}", row.name),
            Some(row.update.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer 200, got {status} {updated:?}"
        );
        let after_update = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_update, updated_after_create,
            "case {case:?}: the detector holds the same sub-resources after an update, so its \
             updatedAt must stay at {updated_after_create:?}, got {after_update:?}"
        );

        // The delete is the other half of the same lifecycle: the detector holds one
        // fewer sub-resource, so its UpdatedAt moves again.
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("{collection}/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        let after_delete = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            after_delete > updated_after_create,
            "case {case:?}: the delete changed the detector too, so its updatedAt must move past \
             {updated_after_create:?}, got {after_delete:?}"
        );

        // A refused delete removes nothing, so it moves nothing.
        let (status, again) = call(
            &router,
            "DELETE",
            &format!("{collection}/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete of the same name must be refused, got {status} \
             {again:?}"
        );
        let after_second_delete = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_second_delete, after_delete,
            "case {case:?}: a refused delete must not move the detector's updatedAt, expected \
             {after_delete:?}, got {after_second_delete:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every change to the set moves the instant again, so the detector's UpdatedAt names
// the LAST such change and not the first.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_filter_added_to_one_detector_moves_the_instant_again() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let collection = format!("/detector/{detector_id}/filter");

    let mut previous = detector_updated_at(&router, &detector_id, "the fresh detector").await;
    let mut saved: Vec<&'static str> = Vec::new();

    for row in rows() {
        let case = row.case;
        let (status, created) = call(&router, "POST", &collection, Some(row.create.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each filter added to the detector must move its updatedAt past \
             {previous:?}, got {now:?}"
        );
        previous = now;
        saved.push(row.name);
    }

    // The deletes move it on in the same way, one per change.
    for name in saved {
        let case = name;
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("{collection}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each filter taken off the detector must move its updatedAt past \
             {previous:?}, got {now:?}"
        );
        previous = now;
    }
}
