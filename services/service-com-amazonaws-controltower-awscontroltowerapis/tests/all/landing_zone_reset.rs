// SPDX-License-Identifier: CC0-1.0

//! `ResetLandingZone`, end to end over HTTP.
//!
//! Drives `ResetLandingZone` together with `GetLandingZone` and
//! `GetLandingZoneOperation` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a reset starts an asynchronous `RESET` operation against the landing zone the
//!   request names, in either identifier form, and returns a fresh `operationIdentifier`
//!   that resolves through `GetLandingZoneOperation` — `IN_PROGRESS` with no `endTime`
//!   before the span, `SUCCEEDED` with a stable `endTime` after;
//! * the reset re-deploys the configuration the landing zone already carries and changes
//!   nothing in it: `version`, `manifest`, `remediationTypes`, `arn` and
//!   `latestAvailableVersion` read the same before, during and after the operation, while
//!   `status` is `PROCESSING` until the clock makes the operation due and `ACTIVE` after,
//!   with the landing zone `IN_SYNC` with its manifest;
//! * an identifier no landing zone matches is `ResourceNotFoundException` (404), and
//!   nothing is persisted;
//! * a reset against a landing zone with an operation already in flight is
//!   `ConflictException` (409), and nothing is persisted.
//!
//! The enum values come from
//! `smithy_query(query="shape", shape="LandingZoneOperationType,LandingZoneStatus,LandingZoneDriftStatus,RemediationType")`
//! on this crate's `smithy-model/`: `CREATE` | `DELETE` | `RESET` | `UPDATE`, `ACTIVE` |
//! `PROCESSING` | `FAILED`, `DRIFTED` | `IN_SYNC`, and the single `RemediationType` value
//! `INHERITANCE_DRIFT`.

use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, manifest_governing, past_the_settle_span, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// `GetLandingZone`'s `landingZone` member for `identifier`.
async fn get_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let answer = call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected get-landingzone to return 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("landingZone").clone()
}

/// `GetLandingZoneOperation`'s `operationDetails` member for `identifier`.
async fn get_operation(
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

/// Call `ResetLandingZone` for `identifier` and answer the whole response.
async fn reset(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/reset-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await
}

/// Create the account's one landing zone and wait for its `CREATE` operation to become
/// due, so a later reset is not refused for an operation in flight. Answers its ARN.
async fn create_settled_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    version: &str,
    manifest: &serde_json::Value,
    remediation_types: Option<&[&str]>,
) -> String {
    let mut body = serde_json::json!({ "version": version, "manifest": manifest });
    if let Some(types) = remediation_types {
        body["remediationTypes"] = serde_json::json!(types);
    }
    let created = call(router, "/create-landingzone", body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the create to return 200, got {} body {:?}",
        created.status, created.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// One row of the reset table.
struct ResetCase {
    case: &'static str,
    /// The configuration the landing zone is created with.
    version: &'static str,
    /// The regions the created manifest governs, so each row resets a DIFFERENT manifest
    /// (including the empty list, which `Manifest`'s `governedRegions` permits).
    governed_regions: &'static [&'static str],
    remediation_types: Option<&'static [&'static str]>,
    /// Address the landing zone by the bare resource id its ARN ends in rather than by
    /// the ARN itself; both forms resolve.
    by_bare_id: bool,
    /// How many resets this row performs in a row, each waiting for its own operation.
    resets: usize,
}

/// The reset table. Rows differ along what the rule generalises over: the deployed
/// `version` (the catalog's oldest, a step inside the 2.x line, across the major
/// boundary, and the catalog's newest — from
/// `state::CATALOG_LANDING_ZONE_VERSIONS`, the 2.0..3.3 line), the size and content of
/// the manifest being re-deployed (one region, several, none), whether
/// `remediationTypes` is deployed, which of the two identifier forms addresses the
/// landing zone, and the number of successive resets.
fn reset_cases() -> Vec<ResetCase> {
    vec![
        ResetCase {
            case: "the catalog's newest version, one governed region, by ARN",
            version: "3.3",
            governed_regions: &["us-west-2"],
            remediation_types: None,
            by_bare_id: false,
            resets: 1,
        },
        ResetCase {
            case: "the catalog's oldest version, by bare resource id",
            version: "2.0",
            governed_regions: &["eu-west-1"],
            remediation_types: None,
            by_bare_id: true,
            resets: 1,
        },
        ResetCase {
            case: "a version inside the 2.x line, three governed regions",
            version: "2.4",
            governed_regions: &["us-east-1", "us-east-2", "ap-south-1"],
            remediation_types: None,
            by_bare_id: false,
            resets: 2,
        },
        ResetCase {
            case: "across the major boundary, remediation types deployed",
            version: "3.0",
            governed_regions: &["ca-central-1", "eu-central-1"],
            remediation_types: Some(&["INHERITANCE_DRIFT"]),
            by_bare_id: true,
            resets: 1,
        },
        ResetCase {
            case: "a manifest governing no region, reset three times",
            version: "3.1",
            governed_regions: &[],
            remediation_types: None,
            by_bare_id: false,
            resets: 3,
        },
    ]
}

/// RULE: a reset starts an asynchronous `RESET` operation that re-deploys the
/// configuration the landing zone already carries — nothing in that configuration
/// changes, and the landing zone is `PROCESSING` until the clock makes the operation due.
#[tokio::test]
async fn a_reset_redeploys_the_deployed_configuration_without_changing_it() {
    for row in reset_cases() {
        let case = row.case;
        let router = emulator();
        let deployed_manifest = manifest_governing(row.governed_regions);
        let arn = create_settled_landing_zone(
            &router,
            case,
            row.version,
            &deployed_manifest,
            row.remediation_types,
        )
        .await;
        let identifier = if row.by_bare_id {
            arn.rsplit('/')
                .next()
                .expect("an ARN ends in its resource segment")
                .to_string()
        } else {
            arn.clone()
        };

        // The configuration as deployed, before any reset: every later read is compared
        // against this, so the assertion is "unchanged" rather than a copy of the row.
        let before = get_landing_zone(&router, case, &identifier).await;

        let mut identifiers: Vec<String> = Vec::new();
        for attempt in 0..row.resets {
            let case = &format!("{case} (reset {})", attempt + 1);
            let answer = reset(&router, &identifier).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected reset-landingzone to return 200, got {} body {:?}",
                answer.status, answer.body
            );
            let operation = answer.string("operationIdentifier");
            assert!(
                is_operation_identifier(&operation),
                "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
                 {operation:?}"
            );
            assert!(
                !identifiers.contains(&operation),
                "case {case:?}: expected a fresh operationIdentifier, got the earlier \
                 {operation:?}"
            );

            // While the RESET is in flight its record is IN_PROGRESS and the landing zone
            // is PROCESSING, still reporting the configuration that is deployed.
            let in_flight = get_operation(&router, case, &operation).await;
            assert_eq!(
                (
                    in_flight.get("operationType").and_then(|v| v.as_str()),
                    in_flight.get("status").and_then(|v| v.as_str()),
                    in_flight.get("operationIdentifier").and_then(|v| v.as_str()),
                ),
                (Some("RESET"), Some("IN_PROGRESS"), Some(operation.as_str())),
                "case {case:?}: expected its own IN_PROGRESS RESET record, got {in_flight:?}"
            );
            assert!(
                in_flight.get("endTime").is_none(),
                "case {case:?}: expected no endTime while the reset is in flight, got \
                 {in_flight:?}"
            );
            assert!(
                in_flight.get("startTime").is_some(),
                "case {case:?}: expected a startTime on the RESET record, got {in_flight:?}"
            );

            let processing = get_landing_zone(&router, case, &identifier).await;
            assert_eq!(
                processing.get("status").and_then(|v| v.as_str()),
                Some("PROCESSING"),
                "case {case:?}: expected PROCESSING while the reset is in flight, got \
                 {processing:?}"
            );
            for member in ["version", "manifest", "remediationTypes", "arn"] {
                assert_eq!(
                    processing.get(member),
                    before.get(member),
                    "case {case:?}: expected the reset to leave {member} as deployed \
                     ({:?}), got {:?}",
                    before.get(member),
                    processing.get(member)
                );
            }

            // The clock makes the operation due; nothing sleeps server-side, the wait is
            // the caller's.
            tokio::time::sleep(past_the_settle_span()).await;

            let settled = get_operation(&router, case, &operation).await;
            assert_eq!(
                settled.get("status").and_then(|v| v.as_str()),
                Some("SUCCEEDED"),
                "case {case:?}: expected SUCCEEDED after the span, got {settled:?}"
            );
            let end_time = settled.get("endTime").cloned();
            assert!(
                end_time.is_some(),
                "case {case:?}: expected an endTime after the span, got {settled:?}"
            );
            let reread = get_operation(&router, case, &operation).await;
            assert_eq!(
                reread.get("endTime").cloned(),
                end_time,
                "case {case:?}: expected a terminal record's endTime to be stable across \
                 reads, got {reread:?}"
            );

            let after = get_landing_zone(&router, case, &identifier).await;
            assert_eq!(
                after.get("status").and_then(|v| v.as_str()),
                Some("ACTIVE"),
                "case {case:?}: expected ACTIVE after the span, got {after:?}"
            );
            assert_eq!(
                after.get("driftStatus"),
                Some(&serde_json::json!({ "status": "IN_SYNC" })),
                "case {case:?}: expected the re-deployed landing zone to be IN_SYNC with \
                 its manifest, got {after:?}"
            );
            for member in [
                "version",
                "manifest",
                "remediationTypes",
                "arn",
                "latestAvailableVersion",
            ] {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {case:?}: expected the reset to leave {member} unchanged ({:?}), \
                     got {:?}",
                    before.get(member),
                    after.get(member)
                );
            }

            identifiers.push(operation);
        }

        // Every identifier this row's resets returned keeps resolving to its own record.
        for operation in &identifiers {
            let record = get_operation(&router, case, operation).await;
            assert_eq!(
                (
                    record.get("operationType").and_then(|v| v.as_str()),
                    record.get("status").and_then(|v| v.as_str()),
                    record.get("operationIdentifier").and_then(|v| v.as_str()),
                ),
                (Some("RESET"), Some("SUCCEEDED"), Some(operation.as_str())),
                "case {case:?}: expected {operation:?} to keep resolving to its own \
                 SUCCEEDED RESET record, got {record:?}"
            );
        }
    }
}

/// RULE: a reset whose `landingZoneIdentifier` matches no landing zone is
/// `ResourceNotFoundException` (404) and persists nothing.
#[tokio::test]
async fn a_reset_of_an_unknown_landing_zone_is_not_found() {
    // Rows span the forms a client can hold: an identifier of the right shape that was
    // never minted, a bare resource id, a plain name, an empty-ish free-form value, and
    // the ARN of a landing zone this account did hold and deleted (the row whose literal
    // is `None`).
    let cases: Vec<(&str, Option<&str>)> = vec![
        (
            "an ARN no call returned",
            Some(
                "arn:aws:controltower:us-west-2:123456789012:landingzone/\
                 11111111-2222-3333-4444-555555555555",
            ),
        ),
        (
            "a bare resource id no call returned",
            Some("11111111-2222-3333-4444-555555555555"),
        ),
        ("a plain name", Some("my-landing-zone")),
        ("a single character", Some("x")),
        ("the ARN of a landing zone that was deleted", None),
    ];

    for (case, literal_identifier) in cases {
        let router = emulator();
        let deployed = manifest_governing(&["us-west-2"]);
        let arn = create_settled_landing_zone(&router, case, "3.2", &deployed, None).await;

        let identifier = match literal_identifier {
            Some(literal) => literal.to_string(),
            None => {
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
                tokio::time::sleep(past_the_settle_span()).await;
                arn.clone()
            }
        };

        let refused = reset(&router, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: expected the refusal to start no operation, got {:?}",
            refused.body
        );

        // Nothing was persisted: the landing zone the account holds (if any) is untouched
        // and no operation record was minted for the refused call.
        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        let expected = if literal_identifier.is_some() {
            serde_json::json!([{ "arn": arn }])
        } else {
            serde_json::json!([])
        };
        assert_eq!(
            listed.get("landingZones"),
            &expected,
            "case {case:?}: expected the refused reset to persist nothing, leaving \
             {expected:?}, got {:?}",
            listed.get("landingZones")
        );
    }
}

/// RULE: a reset against a landing zone that already has an operation in flight is
/// `ConflictException` (409) and persists nothing — whatever started that operation.
#[tokio::test]
async fn a_reset_against_an_operation_in_flight_is_a_conflict() {
    // (case, what leaves an operation in flight when the reset is attempted)
    let cases = ["create", "update", "reset"];

    for case in cases {
        let router = emulator();
        let deployed = manifest_governing(&["us-west-2"]);
        let created = call(
            &router,
            "/create-landingzone",
            serde_json::json!({ "version": "3.0", "manifest": deployed }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to return 200, got {} body {:?}",
            created.status, created.body
        );
        let arn = created.string("arn");

        // The version that is deployed once the in-flight operation becomes due: the
        // update is the only one of the three that deploys a different one.
        let version_after_the_span = match case {
            "create" => "3.0",
            "update" => {
                tokio::time::sleep(past_the_settle_span()).await;
                let updated = call(
                    &router,
                    "/update-landingzone",
                    serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.1" }),
                )
                .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: expected the update to return 200, got {} body {:?}",
                    updated.status, updated.body
                );
                "3.1"
            }
            _ => {
                tokio::time::sleep(past_the_settle_span()).await;
                let first = reset(&router, &arn).await;
                assert_eq!(
                    first.status, 200,
                    "case {case:?}: expected the first reset to return 200, got {} body {:?}",
                    first.status, first.body
                );
                "3.0"
            }
        };

        let refused = reset(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: expected the refusal to start no operation, got {:?}",
            refused.body
        );

        // The refusal persisted nothing: once the in-flight operation is due, the landing
        // zone carries what THAT operation deployed, and a reset is accepted again.
        tokio::time::sleep(past_the_settle_span()).await;
        let detail = get_landing_zone(&router, case, &arn).await;
        assert_eq!(
            (
                detail.get("version").and_then(|v| v.as_str()),
                detail.get("status").and_then(|v| v.as_str()),
            ),
            (Some(version_after_the_span), Some("ACTIVE")),
            "case {case:?}: expected the refused reset to persist nothing, leaving \
             {version_after_the_span:?} deployed and ACTIVE, got {detail:?}"
        );
        let accepted = reset(&router, &arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected a reset to be accepted once nothing is in flight, got \
             {} body {:?}",
            accepted.status, accepted.body
        );
    }
}
