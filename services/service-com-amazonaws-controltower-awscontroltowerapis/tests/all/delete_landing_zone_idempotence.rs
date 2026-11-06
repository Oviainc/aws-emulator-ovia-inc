// SPDX-License-Identifier: CC0-1.0

//! What `@idempotent` means for a teardown this service starts asynchronously, end to end
//! over HTTP.
//!
//! Drives `DeleteLandingZone` together with `CreateLandingZone`, `UpdateLandingZone`,
//! `ResetLandingZone`, `GetLandingZone`, `GetLandingZoneOperation`,
//! `ListLandingZoneOperations`, `EnableBaseline` and `DisableBaseline` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a repeat of an `@idempotent` call, while the asynchronous operation that call started
//!   against the same resource is still in flight, IS that call's work: it answers 200
//!   with the SAME `operationIdentifier` and starts no second operation;
//! * an operation of another type still changing the resource is a different piece of
//!   work, so starting the teardown then is `ConflictException` (409) and persists
//!   nothing — and the teardown succeeds once the clock has made that operation due;
//! * the idempotent window is the resource's lifetime: once the teardown is due the
//!   resource is gone, so the call names no resource and is `ResourceNotFoundException`
//!   (404).
//!
//! `smithy_query(query="operation", operation="DeleteLandingZone,DisableBaseline")` on
//! this crate's `smithy-model/` reports `"idempotent": true` for exactly these two of the
//! service's operations, so both are driven here; the enum values come from
//! `smithy_query(query="shape", shape="LandingZoneOperationType,LandingZoneOperationStatus,RemediationType")`
//! — `CREATE` | `DELETE` | `RESET` | `UPDATE`, `SUCCEEDED` | `FAILED` | `IN_PROGRESS`, and
//! the single `RemediationType` value `INHERITANCE_DRIFT`.

use crate::baseline_operations::{disable, enable, enablements};
use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, manifest_governing, past_the_settle_span,
    set_up_emulator, simple_manifest, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// One landing zone configuration the idempotence rule is asserted over, with how many
/// times the row repeats the delete.
struct Row {
    case: &'static str,
    version: &'static str,
    manifest: serde_json::Value,
    remediation_types: Option<serde_json::Value>,
    tags: Option<serde_json::Value>,
    /// Repeats AFTER the first delete. Nothing about the rule depends on this count, so
    /// the rows differ along it.
    repeats: usize,
}

/// The rows differ along the deployed version (the catalog's oldest, newest and ones in
/// between), the manifest's shape (no governed region, one, several), whether the landing
/// zone carries a remediation type or tags, and how many times the delete is repeated.
/// Nothing about a repeated delete depends on any of them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the catalog's newest version, untagged, repeated once",
            version: "3.3",
            manifest: simple_manifest(),
            remediation_types: None,
            tags: None,
            repeats: 1,
        },
        Row {
            case: "the catalog's oldest version, no governed region, one tag, repeated twice",
            version: "2.0",
            manifest: manifest_governing(&[]),
            remediation_types: None,
            tags: Some(serde_json::json!({ "team": "platform" })),
            repeats: 2,
        },
        Row {
            case: "several governed regions and tags, repeated four times",
            version: "3.1",
            manifest: manifest_governing(&["us-west-2", "eu-west-1", "ap-south-1"]),
            remediation_types: None,
            tags: Some(serde_json::json!({ "team": "platform", "env": "test" })),
            repeats: 4,
        },
        Row {
            case: "a remediation type on the landing zone, repeated three times",
            version: "3.2",
            manifest: simple_manifest(),
            // The one value the model's `RemediationType` enum carries.
            remediation_types: Some(serde_json::json!(["INHERITANCE_DRIFT"])),
            tags: None,
            repeats: 3,
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
    if let Some(tags) = &row.tags {
        body["tags"] = tags.clone();
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

/// POST `/delete-landingzone` for `arn`.
async fn delete(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await
}

/// Every `DELETE` record the operation log holds, read through the listing's own
/// `filter.types` (`LandingZoneOperationTypes` is `@length` 1..1, so one type per call).
async fn delete_records(router: &MultiServiceRouter, case: &str) -> Vec<serde_json::Value> {
    let listed = call(
        router,
        "/list-landingzone-operations",
        serde_json::json!({ "filter": { "types": ["DELETE"] } }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected list-landingzone-operations to return 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
        .get("landingZoneOperations")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A record's member, as a `&str`.
fn member<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    record.get(name).and_then(serde_json::Value::as_str)
}

/// RULE: a repeat of the `@idempotent` `DeleteLandingZone`, while the decommissioning its
/// first call started is still in flight, is that same work: 200 with the identifier the
/// first call returned, and no second `DELETE` in the operation log.
#[tokio::test]
async fn a_repeated_delete_reports_the_decommissioning_already_in_flight() {
    for row in rows() {
        // One landing zone per account, so each row gets its own emulator.
        let router = emulator();
        let case = row.case;
        let arn = create_settled(&router, &row).await;

        let first = delete(&router, &arn).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected the first delete to return 200, got {} body {:?}",
            first.status, first.body
        );
        let identifier = first.string("operationIdentifier");
        assert!(
            is_operation_identifier(&identifier),
            "case {case:?}: expected a modeled operationIdentifier, got {identifier:?}"
        );

        for repeat in 1..=row.repeats {
            let again = delete(&router, &arn).await;
            assert_eq!(
                (again.status, again.string("operationIdentifier")),
                (200, identifier.clone()),
                "case {case:?} repeat {repeat}: expected 200 and {identifier:?}, got {} body \
                 {:?}",
                again.status,
                again.body
            );
        }

        // No repeat started a second teardown: the log holds exactly the one DELETE the
        // first call wrote, still in flight.
        let records = delete_records(&router, case).await;
        assert_eq!(
            records
                .iter()
                .map(|record| (
                    member(record, "operationIdentifier"),
                    member(record, "status")
                ))
                .collect::<Vec<_>>(),
            vec![(Some(identifier.as_str()), Some("IN_PROGRESS"))],
            "case {case:?}: expected exactly one IN_PROGRESS DELETE record, got {records:?}"
        );

        // The repeats changed nothing else either: the landing zone is still the resource
        // the first delete left PROCESSING.
        let during = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            (
                during.status,
                during
                    .get("landingZone")
                    .get("status")
                    .and_then(|value| value.as_str())
            ),
            (200, Some("PROCESSING")),
            "case {case:?}: expected the landing zone still PROCESSING, got {} body {:?}",
            during.status,
            during.body
        );

        // The idempotent window is the resource's lifetime: once the clock has made the
        // teardown due the landing zone is gone, so the call names no resource.
        tokio::time::sleep(past_the_settle_span()).await;
        let settled = call(
            &router,
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": identifier }),
        )
        .await;
        let details = settled.get("operationDetails").clone();
        assert_eq!(
            (
                member(&details, "operationType"),
                member(&details, "status")
            ),
            (Some("DELETE"), Some("SUCCEEDED")),
            "case {case:?}: expected the one DELETE to be SUCCEEDED once due, got {details:?}"
        );
        let gone = delete(&router, &arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException once the teardown is due, \
             got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
    }
}

/// One row of [`a_delete_while_another_operation_is_in_flight_is_a_conflict`]: the call
/// that leaves an operation of ANOTHER type in flight against the landing zone.
struct Blocker {
    case: &'static str,
    /// The path the blocking call is sent to, and the body it carries; `None` means the
    /// row's own `CREATE` is the operation still in flight, so no further call is made.
    path: Option<&'static str>,
    body: fn(&str) -> serde_json::Value,
    /// The `LandingZoneOperationType` the blocking call's operation carries.
    operation_type: &'static str,
}

/// RULE: an operation of another type still changing the landing zone is not this call's
/// work, so starting the decommissioning then is `ConflictException` (409) and persists
/// nothing — and the teardown succeeds once the clock has made that operation due.
#[tokio::test]
async fn a_delete_while_another_operation_is_in_flight_is_a_conflict() {
    let blockers = vec![
        Blocker {
            case: "the landing zone's own create",
            path: None,
            body: |_| serde_json::Value::Null,
            operation_type: "CREATE",
        },
        Blocker {
            case: "an update",
            path: Some("/update-landingzone"),
            body: |arn| {
                serde_json::json!({
                    "landingZoneIdentifier": arn,
                    "version": "3.3",
                    "manifest": simple_manifest(),
                })
            },
            operation_type: "UPDATE",
        },
        Blocker {
            case: "a reset",
            path: Some("/reset-landingzone"),
            body: |arn| serde_json::json!({ "landingZoneIdentifier": arn }),
            operation_type: "RESET",
        },
    ];

    for blocker in blockers {
        let case = blocker.case;
        let router = emulator();

        // The create is what every row starts from; only the CREATE row leaves it in
        // flight.
        let created = call(
            &router,
            "/create-landingzone",
            serde_json::json!({ "version": "3.2", "manifest": simple_manifest() }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to return 200, got {} body {:?}",
            created.status, created.body
        );
        let arn = created.string("arn");

        let blocking_identifier = match blocker.path {
            None => created.string("operationIdentifier"),
            Some(path) => {
                tokio::time::sleep(past_the_settle_span()).await;
                let blocking = call(&router, path, (blocker.body)(&arn)).await;
                assert_eq!(
                    blocking.status, 200,
                    "case {case:?}: expected {path} to return 200, got {} body {:?}",
                    blocking.status, blocking.body
                );
                blocking.string("operationIdentifier")
            }
        };

        let refused = delete(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException while a {} is in flight, got {:?} \
             body {:?}",
            blocker.operation_type,
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: expected the refusal to start no operation, got {:?}",
            refused.body
        );

        // The refusal wrote nothing: no DELETE was logged.
        let logged = delete_records(&router, case).await;
        assert!(
            logged.is_empty(),
            "case {case:?}: expected the refusal to log no DELETE, got {logged:?}"
        );

        // Once the blocking operation is due the landing zone is this call's to tear down.
        tokio::time::sleep(past_the_settle_span()).await;
        let accepted = delete(&router, &arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the delete to return 200 once the {} is due, got {} \
             body {:?}",
            blocker.operation_type, accepted.status, accepted.body
        );
        let identifier = accepted.string("operationIdentifier");
        assert_ne!(
            identifier, blocking_identifier,
            "case {case:?}: expected the teardown to mint its own identifier, got the {}'s \
             {blocking_identifier:?}",
            blocker.operation_type
        );

        // And from there the repeat is the same work again, whatever operation preceded it.
        let repeated = delete(&router, &arn).await;
        assert_eq!(
            (repeated.status, repeated.string("operationIdentifier")),
            (200, identifier.clone()),
            "case {case:?}: expected the repeat to report {identifier:?}, got {} body {:?}",
            repeated.status,
            repeated.body
        );
    }
}

/// RULE (the same rule on the service's other `@idempotent` operation): a repeat of
/// `DisableBaseline` while the teardown it started is in flight reports that same
/// identifier, exactly as the repeated `DeleteLandingZone` does. Both operations of the
/// service the model marks `@idempotent` are driven here, so the two cannot drift apart.
#[tokio::test]
async fn every_idempotent_teardown_reports_the_operation_in_flight() {
    let router = emulator();

    // (case, first identifier, the identifier the repeat reported).
    let mut answers: Vec<(&'static str, String, String)> = Vec::new();

    let landing_zone = Row {
        case: "DeleteLandingZone",
        version: "3.3",
        manifest: simple_manifest(),
        remediation_types: None,
        tags: None,
        repeats: 1,
    };
    let arn = create_settled(&router, &landing_zone).await;
    let deleted = delete(&router, &arn).await;
    assert_eq!(
        deleted.status, 200,
        "case {:?}: expected the teardown to return 200, got {} body {:?}",
        landing_zone.case, deleted.status, deleted.body
    );
    let repeated = delete(&router, &arn).await;
    assert_eq!(
        repeated.status, 200,
        "case {:?}: expected the repeat to return 200, got {} body {:?}",
        landing_zone.case, repeated.status, repeated.body
    );
    answers.push((
        landing_zone.case,
        deleted.string("operationIdentifier"),
        repeated.string("operationIdentifier"),
    ));

    let enablement = &enablements()[0];
    // Its own emulator, SET UP: the half above decommissions this account's landing zone,
    // and a decommissioning deletes the enablements deployed under it, so a shared store
    // would tear this enablement down in the middle of the case — and a baseline enables
    // only for an account Control Tower has been set up in.
    let baseline_router = set_up_emulator().await;
    let enabled = enable(&baseline_router, enablement).await;
    assert_eq!(
        enabled.status, 200,
        "case \"DisableBaseline\": expected the enable to return 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let enabled_baseline = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;
    let disabled = disable(&baseline_router, &enabled_baseline).await;
    assert_eq!(
        disabled.status, 200,
        "case \"DisableBaseline\": expected the teardown to return 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let disabled_again = disable(&baseline_router, &enabled_baseline).await;
    assert_eq!(
        disabled_again.status, 200,
        "case \"DisableBaseline\": expected the repeat to return 200, got {} body {:?}",
        disabled_again.status, disabled_again.body
    );
    answers.push((
        "DisableBaseline",
        disabled.string("operationIdentifier"),
        disabled_again.string("operationIdentifier"),
    ));

    for (case, first, repeat) in &answers {
        assert!(
            is_operation_identifier(first),
            "case {case:?}: expected a modeled operationIdentifier, got {first:?}"
        );
        assert_eq!(
            repeat, first,
            "case {case:?}: expected the repeat to report the operation already in flight \
             {first:?}, got {repeat:?}"
        );
    }
}
