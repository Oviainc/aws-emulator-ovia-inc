// SPDX-License-Identifier: CC0-1.0

//! `UpdateLandingZone`, end to end over HTTP.
//!
//! Drives `UpdateLandingZone` together with `GetLandingZone` and
//! `GetLandingZoneOperation` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * `UpdateLandingZone` starts an asynchronous `UPDATE` operation whose configuration is
//!   deployed only once the clock makes that operation due: until then the landing zone is
//!   `PROCESSING` and reports the configuration still DEPLOYED, and afterwards it reports
//!   the requested `version`, the requested `manifest` and the requested
//!   `remediationTypes`, with a member the request omitted carried forward unchanged;
//! * `latestAvailableVersion` is recomputed from the newly deployed version, so it is
//!   never older than what is deployed;
//! * an `operationIdentifier` the call returns is fresh and resolves through
//!   `GetLandingZoneOperation` to an `UPDATE` record — `IN_PROGRESS` before the span,
//!   `SUCCEEDED` with a stable `endTime` after;
//! * an identifier no landing zone matches is `ResourceNotFoundException` (404), and
//!   nothing is persisted;
//! * an update against a landing zone with an operation already in flight is
//!   `ConflictException` (409), and nothing is persisted.
//!
//! The enum values come from
//! `smithy_query(query="shape", shape="LandingZoneOperationType,LandingZoneStatus,RemediationType")`
//! on this crate's `smithy-model/`: `CREATE` | `DELETE` | `RESET` | `UPDATE`, `ACTIVE` |
//! `PROCESSING` | `FAILED`, and the single `RemediationType` value `INHERITANCE_DRIFT`.

use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, past_the_settle_span, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// The catalog's newest landing zone version, read from the service so a retune of the
/// constant fails here rather than silently changing what these rows expect.
const CATALOG: &str =
    service_com_amazonaws_controltower_awscontroltowerapis::state::CATALOG_LANDING_ZONE_VERSION;

/// A manifest document, parameterised so each row can deploy a DIFFERENT one.
fn manifest(governed_region: &str) -> serde_json::Value {
    serde_json::json!({
        "governedRegions": [governed_region],
        "organizationStructure": { "security": { "name": "Security" } }
    })
}

/// Create the account's one landing zone and wait for its `CREATE` operation to become
/// due, so a later update is not refused for an operation in flight. Answers its ARN.
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

/// Call `UpdateLandingZone`, asserting it was accepted, and answer the whole response so a
/// row can read the `operationIdentifier` out of it.
async fn update(
    router: &MultiServiceRouter,
    case: &str,
    body: serde_json::Value,
) -> Answer {
    let answer = call(router, "/update-landingzone", body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected update-landingzone to return 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
}

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    /// The configuration the landing zone is created with.
    created_version: &'static str,
    created_region: &'static str,
    created_remediation_types: Option<&'static [&'static str]>,
    /// What the update asks for. `None` means the member is omitted entirely.
    update_version: &'static str,
    update_region: Option<&'static str>,
    update_remediation_types: Option<&'static [&'static str]>,
    /// Address the landing zone by the bare resource id its ARN ends in rather than by
    /// the ARN itself; both forms resolve.
    by_bare_id: bool,
    /// `latestAvailableVersion` once the update is deployed: the numerically later of the
    /// deployed version and the catalog's newest.
    expected_latest_available: &'static str,
}

/// The update table. Rows differ along what the rule generalises over: the requested
/// `version` relative to the catalog (below, equal, above, a minor larger numerically but
/// smaller lexicographically, and both published length bounds of `LandingZoneVersion`,
/// len 3..10), whether the request carries a new `manifest` or omits it, whether it
/// carries `remediationTypes` (`RemediationType` has the single value
/// `INHERITANCE_DRIFT`, and the list's length is 1..1), adds them, omits them, and which
/// of the two identifier forms addresses the landing zone.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a newer version below the catalog, with a replaced manifest",
            created_version: "3.0",
            created_region: "us-west-2",
            created_remediation_types: None,
            update_version: "3.2",
            update_region: Some("eu-west-1"),
            update_remediation_types: None,
            by_bare_id: false,
            expected_latest_available: "3.3",
        },
        UpdateCase {
            case: "the catalog's own version, manifest left as deployed",
            created_version: "3.0",
            created_region: "us-west-2",
            created_remediation_types: None,
            update_version: "3.3",
            update_region: None,
            update_remediation_types: None,
            by_bare_id: false,
            expected_latest_available: "3.3",
        },
        UpdateCase {
            case: "a version above the catalog, which becomes its own latest available",
            created_version: "3.2",
            created_region: "ap-south-1",
            created_remediation_types: None,
            update_version: "4.0",
            update_region: Some("ap-south-1"),
            update_remediation_types: None,
            by_bare_id: false,
            expected_latest_available: "4.0",
        },
        UpdateCase {
            case: "a minor larger numerically but smaller lexicographically",
            created_version: "3.2",
            created_region: "us-east-2",
            created_remediation_types: None,
            update_version: "3.10",
            update_region: Some("us-east-1"),
            update_remediation_types: None,
            by_bare_id: true,
            expected_latest_available: "3.10",
        },
        UpdateCase {
            case: "the shortest allowed version, len 3",
            created_version: "3.2",
            created_region: "ca-central-1",
            created_remediation_types: None,
            update_version: "1.0",
            update_region: Some("ca-central-1"),
            update_remediation_types: None,
            by_bare_id: false,
            expected_latest_available: "3.3",
        },
        UpdateCase {
            case: "the longest allowed version, len 10",
            created_version: "3.2",
            created_region: "sa-east-1",
            created_remediation_types: None,
            update_version: "12345.6789",
            update_region: Some("sa-east-1"),
            update_remediation_types: None,
            by_bare_id: true,
            expected_latest_available: "12345.6789",
        },
        UpdateCase {
            case: "remediation types added by the update",
            created_version: "3.0",
            created_region: "eu-central-1",
            created_remediation_types: None,
            update_version: "3.1",
            update_region: Some("eu-central-1"),
            update_remediation_types: Some(&["INHERITANCE_DRIFT"]),
            by_bare_id: false,
            expected_latest_available: "3.3",
        },
        UpdateCase {
            case: "remediation types carried forward when the update omits them",
            created_version: "3.0",
            created_region: "eu-north-1",
            created_remediation_types: Some(&["INHERITANCE_DRIFT"]),
            update_version: "3.1",
            update_region: None,
            update_remediation_types: None,
            by_bare_id: true,
            expected_latest_available: "3.3",
        },
    ]
}

/// RULE: `UpdateLandingZone` starts an asynchronous operation; the configuration it asks
/// for is deployed only once the clock makes that operation due, and a member the request
/// omits is carried forward from what is deployed.
#[tokio::test]
async fn an_update_deploys_its_configuration_when_the_clock_makes_the_operation_due() {
    assert_eq!(
        CATALOG, "3.3",
        "the catalog's newest version moved to {CATALOG:?}; retune this table's \
         expected_latest_available"
    );

    for row in update_cases() {
        let case = row.case;
        let router = emulator();
        let deployed_manifest = manifest(row.created_region);
        let arn = create_settled_landing_zone(
            &router,
            case,
            row.created_version,
            &deployed_manifest,
            row.created_remediation_types,
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

        let mut body = serde_json::json!({
            "landingZoneIdentifier": identifier,
            "version": row.update_version,
        });
        let requested_manifest = row.update_region.map(manifest);
        if let Some(document) = &requested_manifest {
            body["manifest"] = document.clone();
        }
        if let Some(types) = row.update_remediation_types {
            body["remediationTypes"] = serde_json::json!(types);
        }

        let updated = update(&router, case, body).await;
        let operation = updated.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation),
            "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation:?}"
        );

        // While the UPDATE is in flight the operation is IN_PROGRESS and the landing zone
        // is PROCESSING, still reporting the configuration that is actually DEPLOYED.
        let in_flight = get_operation(&router, case, &operation).await;
        assert_eq!(
            in_flight.get("operationType").and_then(|v| v.as_str()),
            Some("UPDATE"),
            "case {case:?}: expected operationType UPDATE, got {in_flight:?}"
        );
        assert_eq!(
            in_flight.get("status").and_then(|v| v.as_str()),
            Some("IN_PROGRESS"),
            "case {case:?}: expected IN_PROGRESS while the update is in flight, got \
             {in_flight:?}"
        );
        assert!(
            in_flight.get("endTime").is_none(),
            "case {case:?}: expected no endTime while the update is in flight, got \
             {in_flight:?}"
        );

        let processing = get_landing_zone(&router, case, &identifier).await;
        assert_eq!(
            processing.get("status").and_then(|v| v.as_str()),
            Some("PROCESSING"),
            "case {case:?}: expected PROCESSING while the update is in flight, got \
             {processing:?}"
        );
        assert_eq!(
            processing.get("version").and_then(|v| v.as_str()),
            Some(row.created_version),
            "case {case:?}: expected the deployed version {:?} while the update is in \
             flight, got {processing:?}",
            row.created_version
        );
        assert_eq!(
            processing.get("manifest"),
            Some(&deployed_manifest),
            "case {case:?}: expected the deployed manifest while the update is in flight, \
             got {processing:?}"
        );

        // The clock makes the operation due; nothing sleeps server-side, the wait is the
        // caller's.
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

        let detail = get_landing_zone(&router, case, &identifier).await;
        assert_eq!(
            detail.get("status").and_then(|v| v.as_str()),
            Some("ACTIVE"),
            "case {case:?}: expected ACTIVE after the span, got {detail:?}"
        );
        assert_eq!(
            detail.get("version").and_then(|v| v.as_str()),
            Some(row.update_version),
            "case {case:?}: expected the requested version {:?} to be deployed, got \
             {detail:?}",
            row.update_version
        );
        assert_eq!(
            detail.get("latestAvailableVersion").and_then(|v| v.as_str()),
            Some(row.expected_latest_available),
            "case {case:?}: expected latestAvailableVersion {:?}, got {detail:?}",
            row.expected_latest_available
        );
        // An omitted manifest is not a change, so the deployed one is carried forward.
        let expected_manifest = requested_manifest.unwrap_or_else(|| deployed_manifest.clone());
        assert_eq!(
            detail.get("manifest"),
            Some(&expected_manifest),
            "case {case:?}: expected the manifest {expected_manifest:?} to be deployed, \
             got {detail:?}"
        );
        let expected_remediation_types = row
            .update_remediation_types
            .or(row.created_remediation_types)
            .map(|types| serde_json::json!(types));
        assert_eq!(
            detail.get("remediationTypes").cloned(),
            expected_remediation_types,
            "case {case:?}: expected remediationTypes {expected_remediation_types:?}, got \
             {detail:?}"
        );
        assert_eq!(
            detail.get("arn").and_then(|v| v.as_str()),
            Some(arn.as_str()),
            "case {case:?}: expected the update to leave the ARN alone, got {detail:?}"
        );
        assert_eq!(
            detail.get("driftStatus"),
            Some(&serde_json::json!({ "status": "IN_SYNC" })),
            "case {case:?}: expected the deployed landing zone to be IN_SYNC with its \
             manifest, got {detail:?}"
        );
    }
}

/// RULE: every `UpdateLandingZone` call mints a FRESH `operationIdentifier`, and every
/// identifier the account's calls returned keeps resolving to its own record.
#[tokio::test]
async fn each_update_mints_a_fresh_identifier_and_every_record_keeps_resolving() {
    let router = emulator();
    let deployed = manifest("us-west-2");
    let arn = create_settled_landing_zone(&router, "repeated updates", "3.0", &deployed, None).await;

    let mut identifiers: Vec<String> = Vec::new();
    // Three successive updates, each waiting for its own operation to become due before
    // the next starts: the version the Nth update asked for is what is deployed after it.
    for version in ["3.1", "3.2", "3.3"] {
        let answer = update(
            &router,
            version,
            serde_json::json!({ "landingZoneIdentifier": arn, "version": version }),
        )
        .await;
        let identifier = answer.string("operationIdentifier");
        assert!(
            !identifiers.contains(&identifier),
            "case {version:?}: expected a fresh operationIdentifier, got the earlier \
             {identifier:?}"
        );
        identifiers.push(identifier);
        tokio::time::sleep(past_the_settle_span()).await;
        let detail = get_landing_zone(&router, version, &arn).await;
        assert_eq!(
            detail.get("version").and_then(|v| v.as_str()),
            Some(version),
            "case {version:?}: expected the latest update's version to be deployed, got \
             {detail:?}"
        );
    }

    for identifier in &identifiers {
        let record = get_operation(&router, "every record resolves", identifier).await;
        assert_eq!(
            (
                record.get("operationType").and_then(|v| v.as_str()),
                record.get("status").and_then(|v| v.as_str()),
                record.get("operationIdentifier").and_then(|v| v.as_str()),
            ),
            (Some("UPDATE"), Some("SUCCEEDED"), Some(identifier.as_str())),
            "identifier {identifier:?}: expected its own SUCCEEDED UPDATE record, got \
             {record:?}"
        );
    }
}

/// RULE: an update whose `landingZoneIdentifier` matches no landing zone is
/// `ResourceNotFoundException` (404) and persists nothing.
#[tokio::test]
async fn an_update_of_an_unknown_landing_zone_is_not_found() {
    // Rows span the forms a client can hold: an identifier of the right shape that was
    // never minted, a bare resource id, a plain name, and the ARN of a landing zone this
    // account did hold and deleted (the row whose literal is `None`).
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
        ("the ARN of a landing zone that was deleted", None),
    ];

    for (case, literal_identifier) in cases {
        let router = emulator();
        let deployed = manifest("us-west-2");
        let arn = create_settled_landing_zone(&router, case, "3.2", &deployed, None).await;
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

        let identifier = literal_identifier.map_or_else(|| arn.clone(), str::to_string);
        let answer = call(
            &router,
            "/update-landingzone",
            serde_json::json!({ "landingZoneIdentifier": identifier, "version": "3.3" }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (answer.status, answer.error_code.as_deref()),
            answer.body
        );
        // Nothing was persisted: the account still holds no landing zone.
        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([]),
            "case {case:?}: expected the refused update to persist nothing, got {:?}",
            listed.get("landingZones")
        );
    }
}

/// RULE: an update against a landing zone that already has an operation in flight is
/// `ConflictException` (409) and persists nothing — whatever started that operation.
#[tokio::test]
async fn an_update_against_an_operation_in_flight_is_a_conflict() {
    // (case, whether the in-flight operation is the CREATE itself or an UPDATE, the
    // version that is deployed when the refused update is attempted)
    let cases = [
        ("while the CREATE is still in flight", false, "3.0"),
        ("while an earlier UPDATE is still in flight", true, "3.1"),
    ];

    for (case, start_an_update, version_after_the_span) in cases {
        let router = emulator();
        let deployed = manifest("us-west-2");
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

        if start_an_update {
            tokio::time::sleep(past_the_settle_span()).await;
            update(
                &router,
                case,
                serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.1" }),
            )
            .await;
        }

        let refused = call(
            &router,
            "/update-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.2" }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );

        // The refusal persisted nothing: once the in-flight operation is due, the
        // deployed version is the one that operation was deploying, not the refused one.
        tokio::time::sleep(past_the_settle_span()).await;
        let detail = get_landing_zone(&router, case, &arn).await;
        assert_eq!(
            detail.get("version").and_then(|v| v.as_str()),
            Some(version_after_the_span),
            "case {case:?}: expected the refused update to persist nothing, leaving \
             {version_after_the_span:?} deployed, got {detail:?}"
        );
    }
}
