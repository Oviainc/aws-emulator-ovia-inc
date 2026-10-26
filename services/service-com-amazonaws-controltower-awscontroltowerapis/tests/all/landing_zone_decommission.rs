// SPDX-License-Identifier: CC0-1.0

//! The asynchronous decommissioning `DeleteLandingZone` starts, end to end over HTTP.
//!
//! Drives `DeleteLandingZone` together with `GetLandingZone`, `ListLandingZones`,
//! `ListTagsForResource`, `GetLandingZoneOperation` and the other mutating landing zone
//! calls through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a call that starts an asynchronous operation against a resource leaves that resource
//!   reportable until the operation it returned reaches a terminal status, so a landing
//!   zone whose `DELETE` is still `IN_PROGRESS` is still reported (`PROCESSING`), still
//!   listed and still tagged, and stops existing only once the clock has made that
//!   operation due;
//! * a mutating call against a resource an in-flight operation is already changing is
//!   `ConflictException` (409) and persists nothing — for every such call, including the
//!   `CreateLandingZone` whose account the decommissioning landing zone still occupies.
//!
//! The version rows come from this crate's landing zone version catalog and the enum
//! values from
//! `smithy_query(query="shape", shape="LandingZoneStatus,LandingZoneOperationStatus,LandingZoneOperationType,RemediationType")`
//! on this crate's `smithy-model/`: `ACTIVE` | `PROCESSING` | `FAILED`, `SUCCEEDED` |
//! `FAILED` | `IN_PROGRESS`, `CREATE` | `DELETE` | `RESET` | `UPDATE`, and the single
//! `RemediationType` value `INHERITANCE_DRIFT`.

use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, manifest_governing, past_the_settle_span,
    simple_manifest, Answer,
};
use crate::resource_tags::{read_tags, reported_tags, tags};

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

/// One landing zone configuration a row deploys before deleting it.
struct Row {
    case: &'static str,
    version: &'static str,
    manifest: serde_json::Value,
    remediation_types: Option<serde_json::Value>,
    tags: BTreeMap<String, String>,
}

/// The configurations the decommissioning rule is asserted over: they differ along the
/// deployed version (the catalog's oldest, newest and one in between), the manifest's
/// shape (no governed region, one, several) and whether the landing zone carries a
/// remediation type or tags. Nothing about a delete depends on any of them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the catalog's newest version, untagged",
            version: "3.3",
            manifest: simple_manifest(),
            remediation_types: None,
            tags: tags(&[]),
        },
        Row {
            case: "the catalog's oldest version, no governed region, one tag",
            version: "2.0",
            manifest: manifest_governing(&[]),
            remediation_types: None,
            tags: tags(&[("team", "platform")]),
        },
        Row {
            case: "several governed regions and several tags",
            version: "3.1",
            manifest: manifest_governing(&["us-west-2", "eu-west-1", "ap-south-1"]),
            remediation_types: None,
            tags: tags(&[("team", "platform"), ("env", "test")]),
        },
        Row {
            case: "a remediation type on the landing zone",
            version: "3.2",
            manifest: simple_manifest(),
            // The one value the model's `RemediationType` enum carries.
            remediation_types: Some(serde_json::json!(["INHERITANCE_DRIFT"])),
            tags: tags(&[("owner", "sre")]),
        },
    ]
}

/// Create the row's landing zone and wait for its `CREATE` to become due, answering with
/// the ARN the create reported.
async fn create_settled(router: &MultiServiceRouter, row: &Row) -> String {
    let mut body = serde_json::json!({ "version": row.version, "manifest": row.manifest });
    if let Some(remediation_types) = &row.remediation_types {
        body["remediationTypes"] = remediation_types.clone();
    }
    if !row.tags.is_empty() {
        body["tags"] = serde_json::Value::Object(
            row.tags
                .iter()
                .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
                .collect(),
        );
    }
    let created = call(router, "/create-landingzone", body).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the create to return 200, got {} body {:?}",
        row.case, created.status, created.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// `GetLandingZoneOperation`'s `operationDetails` for `identifier`.
async fn operation_details(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let answer = call(
        router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected get-landingzone-operation to return 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("operationDetails").clone()
}

/// RULE: the landing zone an in-flight `DELETE` is decommissioning is still a resource —
/// reported as `PROCESSING`, listed, and carrying its tags — and stops existing only once
/// the clock has made that operation due.
#[tokio::test]
async fn a_landing_zone_stays_a_resource_until_its_delete_is_due() {
    for row in rows() {
        // One landing zone per account, so each row gets its own emulator.
        let router = emulator();
        let case = row.case;
        let arn = create_settled(&router, &row).await;

        let before = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        let mut deployed = before.get("landingZone").clone();
        assert_eq!(
            deployed.get("status").and_then(|v| v.as_str()),
            Some("ACTIVE"),
            "case {case:?}: expected the settled landing zone to be ACTIVE, got {deployed:?}"
        );

        let deleted = call(
            &router,
            "/delete-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to return 200, got {} body {:?}",
            deleted.status, deleted.body
        );
        let delete_operation = deleted.string("operationIdentifier");
        assert!(
            is_operation_identifier(&delete_operation),
            "case {case:?}: expected a modeled operationIdentifier, got {delete_operation:?}"
        );

        // The operation the delete returned is in flight, so the decommissioning has not
        // finished.
        let in_flight = operation_details(&router, case, &delete_operation).await;
        assert_eq!(
            (
                in_flight.get("operationType").and_then(|v| v.as_str()),
                in_flight.get("status").and_then(|v| v.as_str()),
                in_flight.get("endTime").is_some(),
            ),
            (Some("DELETE"), Some("IN_PROGRESS"), false),
            "case {case:?}: expected an IN_PROGRESS DELETE with no endTime, got {in_flight:?}"
        );

        // The landing zone is still reported, PROCESSING, with everything else it was
        // deployed with unchanged.
        let during = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            during.status, 200,
            "case {case:?}: expected get-landingzone during the delete to return 200, got {} \
             body {:?}",
            during.status, during.body
        );
        let mut reported = during.get("landingZone").clone();
        assert_eq!(
            reported.get("status").and_then(|v| v.as_str()),
            Some("PROCESSING"),
            "case {case:?}: expected PROCESSING while the delete is in flight, got {reported:?}"
        );
        // Only the status moved: every other member reads what was deployed.
        for detail in [&mut deployed, &mut reported] {
            detail
                .as_object_mut()
                .expect("the landing zone detail is an object")
                .remove("status");
        }
        assert_eq!(
            reported, deployed,
            "case {case:?}: expected the in-flight delete to change nothing but the status, \
             got {reported:?}"
        );

        // It is still the one entry the list reports, and still carries its tags.
        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([{ "arn": arn }]),
            "case {case:?}: expected the landing zone still listed, got {:?}",
            listed.get("landingZones")
        );
        let tagged = read_tags(&router, &arn).await;
        assert_eq!(
            (tagged.status, reported_tags(&tagged)),
            (200, row.tags.clone()),
            "case {case:?}: expected the tags of a resource that still exists, got {} body {:?}",
            tagged.status,
            tagged.body
        );

        // Once the clock has made the DELETE due the decommissioning is complete: the
        // operation is terminal and the landing zone, and its tags, are gone.
        tokio::time::sleep(past_the_settle_span()).await;
        let settled = operation_details(&router, case, &delete_operation).await;
        assert_eq!(
            (
                settled.get("status").and_then(|v| v.as_str()),
                settled.get("endTime").is_some(),
            ),
            (Some("SUCCEEDED"), true),
            "case {case:?}: expected a SUCCEEDED DELETE with an endTime, got {settled:?}"
        );
        let gone = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException once the delete is due, \
             got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
        let emptied = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            emptied.get("landingZones"),
            &serde_json::json!([]),
            "case {case:?}: expected an empty list once the delete is due, got {:?}",
            emptied.get("landingZones")
        );
        let untagged = read_tags(&router, &arn).await;
        assert_eq!(
            (untagged.status, untagged.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the tags to go with the resource, got {:?} body {:?}",
            (untagged.status, untagged.error_code.as_deref()),
            untagged.body
        );
    }
}

/// One row of [`a_mutating_call_during_the_in_flight_delete_is_a_conflict`]: a mutating
/// call of this service's landing zone surface, with the body it sends during the window.
struct Collision {
    case: &'static str,
    path: &'static str,
    /// The request body, built from the landing zone's ARN (ignored by the create, which
    /// names no landing zone).
    body: fn(&str) -> serde_json::Value,
}

/// RULE: a mutating call against a landing zone an in-flight operation is already
/// changing is `ConflictException` (409) and persists nothing — including the create whose
/// account that landing zone still occupies.
#[tokio::test]
async fn a_mutating_call_during_the_in_flight_delete_is_a_conflict() {
    let cases = vec![
        Collision {
            case: "a second delete",
            path: "/delete-landingzone",
            body: |arn| serde_json::json!({ "landingZoneIdentifier": arn }),
        },
        Collision {
            case: "an update",
            path: "/update-landingzone",
            body: |arn| serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.3" }),
        },
        Collision {
            case: "a reset",
            path: "/reset-landingzone",
            body: |arn| serde_json::json!({ "landingZoneIdentifier": arn }),
        },
        Collision {
            case: "a create for the account the decommissioning landing zone still occupies",
            path: "/create-landingzone",
            body: |_| serde_json::json!({ "version": "3.3", "manifest": simple_manifest() }),
        },
    ];

    for Collision { case, path, body } in cases {
        let router = emulator();
        let row = Row {
            case,
            version: "3.2",
            manifest: simple_manifest(),
            remediation_types: None,
            tags: tags(&[]),
        };
        let arn = create_settled(&router, &row).await;
        let deleted = call(
            &router,
            "/delete-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to return 200, got {} body {:?}",
            deleted.status, deleted.body
        );

        let refused: Answer = call(&router, path, body(&arn)).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} on {path}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?} on {path}: expected the refusal to start no operation, got {:?}",
            refused.body
        );

        // Nothing was persisted: the account still holds exactly the landing zone being
        // decommissioned, still on the version it deployed.
        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([{ "arn": arn }]),
            "case {case:?}: expected the refused call to persist nothing, got {:?}",
            listed.get("landingZones")
        );
        let during = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            during
                .get("landingZone")
                .get("version")
                .and_then(|v| v.as_str()),
            Some("3.2"),
            "case {case:?}: expected the deployed version untouched, got {:?}",
            during.get("landingZone")
        );

        // And once the delete is due the window is over: the account is free again, so the
        // create that collided now succeeds and the calls that name the deleted landing
        // zone answer 404 instead.
        tokio::time::sleep(past_the_settle_span()).await;
        let after = call(&router, path, body(&arn)).await;
        let expected = if path == "/create-landingzone" {
            (200, None)
        } else {
            (404, Some("ResourceNotFoundException"))
        };
        assert_eq!(
            (after.status, after.error_code.as_deref()),
            expected,
            "case {case:?} on {path} after the delete is due: expected {expected:?}, got {:?} \
             body {:?}",
            (after.status, after.error_code.as_deref()),
            after.body
        );
    }
}
