// SPDX-License-Identifier: CC0-1.0

//! What moves a detector's `UpdatedAt`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! serialization and the handlers are exercised together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! an accepted write to a field of the detector's OWN record moves that record's
//! last-updated timestamp, whichever operation and whichever view of the field the
//! request used; a read, and a refused write, leave it where it was. `CreatedAt` is
//! not a last-updated timestamp, so nothing moves it after the create.
//!
//! The rows span the detector-own configuration fields and the request forms that
//! write them: the organization configuration (through either auto-enablement view,
//! through `features`, through the deprecated `dataSources` view), the Malware
//! Protection scan settings, and the detector's own status and publishing frequency.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::member_association::DEFAULT_REGION;
use crate::organization_configuration::describe_in;

/// The error class a rejection the HANDLER made is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The error class a rejection the request BOUNDARY made is named by: a value
/// outside a constraint the model itself declares carries the modelled shape.
const BOUNDARY_CLASS: &str = "BadRequestException";

/// One accepted write to the detector's own record: the case name, the path suffix
/// under `/detector/{detectorId}` the operation is bound at, and the request body.
type ConfigurationWrite = (&'static str, &'static str, fn() -> Value);

/// One refused write: a [`ConfigurationWrite`] plus the error class the refusal is
/// named by.
type RefusedWrite = (&'static str, &'static str, fn() -> Value, &'static str);

/// The accepted writes under test.
///
/// The suffixes are the operations' own URIs:
/// `UpdateOrganizationConfiguration` at `/detector/{DetectorId}/admin`,
/// `UpdateMalwareScanSettings` at `/detector/{DetectorId}/malware-scan-settings` and
/// `UpdateDetector` at `/detector/{DetectorId}`
/// (`smithy_query(query="digest")`).
const CONFIGURATION_WRITES: [ConfigurationWrite; 9] = [
    (
        "organization auto-enablement ALL",
        "/admin",
        || json!({"autoEnableOrganizationMembers": "ALL"}),
    ),
    (
        "organization auto-enablement NEW",
        "/admin",
        || json!({"autoEnableOrganizationMembers": "NEW"}),
    ),
    (
        // The value a fresh detector already holds: the request still WRITES the
        // auto-enablement, so it is still a write to the record.
        "organization auto-enablement NONE",
        "/admin",
        || json!({"autoEnableOrganizationMembers": "NONE"}),
    ),
    (
        "organization auto-enablement through the deprecated boolean",
        "/admin",
        || json!({"autoEnable": true}),
    ),
    (
        "organization feature switches",
        "/admin",
        || {
            json!({
                "autoEnableOrganizationMembers": "NEW",
                "features": [{"name": "S3_DATA_EVENTS", "autoEnable": "ALL"},
                             {"name": "RDS_LOGIN_EVENTS", "autoEnable": "NEW"}],
            })
        },
    ),
    (
        "organization switches through the deprecated dataSources view",
        "/admin",
        || {
            json!({
                "autoEnableOrganizationMembers": "NEW",
                "dataSources": {"s3Logs": {"autoEnable": true}},
            })
        },
    ),
    (
        "malware scan settings",
        "/malware-scan-settings",
        || json!({"ebsSnapshotPreservation": "RETENTION_WITH_FINDING"}),
    ),
    (
        "the detector's own status",
        "",
        || json!({"enable": false}),
    ),
    (
        "the detector's finding publishing frequency",
        "",
        || json!({"findingPublishingFrequency": "ONE_HOUR"}),
    ),
];

/// `GetDetector`, asserting it succeeded.
async fn get_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-detector expected 200, got {status} {body:?}"
    );
    body
}

/// The `UpdatedAt` a `GetDetector` reports. An ISO-8601 instant with a fixed-width
/// subsecond field, so comparing the text compares the instants.
async fn updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let body = get_detector(router, detector_id, case).await;
    body["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {body:?}"))
        .to_string()
}

/// A write to the detector's own record moves the `UpdatedAt` a later read reports,
/// whichever field and whichever view of it the request wrote.
#[tokio::test]
async fn an_accepted_configuration_write_moves_the_detectors_updated_at() {
    for (case, suffix, body) in CONFIGURATION_WRITES {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let created = get_detector(&router, &detector_id, case).await;
        let created_at = created["createdAt"].as_str().unwrap_or_default().to_string();
        let before = created["updatedAt"].as_str().unwrap_or_default().to_string();

        let (status, got) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}{suffix}"),
            Some(body()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the write expected 200, got {status} {got:?}"
        );

        let read = get_detector(&router, &detector_id, case).await;
        let after = read["updatedAt"].as_str().unwrap_or_default().to_string();
        assert!(
            after > before,
            "case {case:?}: a write must move the detector's updatedAt: {before:?} -> {after:?}"
        );
        assert_eq!(
            read["createdAt"].as_str().unwrap_or_default(),
            created_at,
            "case {case:?}: a write must not move createdAt, got {read:?}"
        );
    }
}

/// Each successive write moves the timestamp again, strictly: two writes in the same
/// wall-clock millisecond are still ordered, so a client can tell them apart.
#[tokio::test]
async fn successive_writes_move_it_strictly_forward() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut previous = updated_at(&router, &detector_id, "the create").await;
    for members in ["ALL", "NEW", "NONE", "ALL"] {
        let case = format!("organization auto-enablement {members}");
        let (status, got) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/admin"),
            Some(json!({"autoEnableOrganizationMembers": members})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the write expected 200, got {status} {got:?}"
        );

        // The write landed, which is what makes the moved timestamp a report of a
        // change and not a tick of its own.
        let configuration = describe_in(&router, DEFAULT_REGION, &detector_id).await;
        assert_eq!(
            configuration["autoEnableOrganizationMembers"]
                .as_str()
                .unwrap_or_default(),
            members,
            "case {case:?}: the write must be what a later read reports, got {configuration:?}"
        );

        let now = updated_at(&router, &detector_id, &case).await;
        assert!(
            now > previous,
            "case {case:?}: each write must move updatedAt forward: {previous:?} -> {now:?}"
        );
        previous = now;
    }
}

/// A refused write changes nothing, including the timestamp: the rows span a request
/// the operation's own rules refuse and a detector id the account does not hold.
#[tokio::test]
async fn a_refused_write_leaves_the_updated_at_alone() {
    // (the case, the path suffix, the body, the class the refusal is named by)
    let refusals: [RefusedWrite; 4] = [
        (
            "both auto-enablement views at once",
            "/admin",
            || json!({"autoEnableOrganizationMembers": "ALL", "autoEnable": true}),
            CLASS,
        ),
        (
            "neither auto-enablement view",
            "/admin",
            || json!({}),
            CLASS,
        ),
        (
            "both Runtime Monitoring features enabled together",
            "/admin",
            || {
                json!({
                    "autoEnableOrganizationMembers": "NEW",
                    "features": [{"name": "RUNTIME_MONITORING", "autoEnable": "ALL"},
                                 {"name": "EKS_RUNTIME_MONITORING", "autoEnable": "NEW"}],
                })
            },
            CLASS,
        ),
        (
            "an auto-enablement the enum does not carry",
            "/admin",
            || json!({"autoEnableOrganizationMembers": "EVERYTHING"}),
            BOUNDARY_CLASS,
        ),
    ];

    for (case, suffix, body, class) in refusals {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let before = updated_at(&router, &detector_id, case).await;

        let (status, got) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}{suffix}"),
            Some(body()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            class,
            "case {case:?}: expected the {class} class, got {got:?}"
        );

        let after = updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must not move updatedAt, got {after:?}"
        );
    }

    // A write aimed at a detector the account does not hold moves no OTHER
    // detector's timestamp either.
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let case = "an unheld detector id";
    let before = updated_at(&router, &detector_id, case).await;
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{ABSENT_DETECTOR_ID}/admin"),
        Some(json!({"autoEnableOrganizationMembers": "ALL"})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {got:?}"
    );
    let after = updated_at(&router, &detector_id, case).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused write must not move updatedAt, got {after:?}"
    );
}

/// A read reports; it moves nothing a later caller sees.
#[tokio::test]
async fn reads_leave_the_updated_at_alone() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let case = "after an organization write";
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/admin"),
        Some(json!({"autoEnableOrganizationMembers": "ALL"})),
    )
    .await;
    assert_eq!(status, 200, "setup write got {status} {got:?}");

    let before = updated_at(&router, &detector_id, case).await;
    for read in 1..=3 {
        let configuration = describe_in(&router, DEFAULT_REGION, &detector_id).await;
        assert_eq!(
            configuration["autoEnableOrganizationMembers"]
                .as_str()
                .unwrap_or_default(),
            "ALL",
            "read {read}: a read must report the stored configuration, got {configuration:?}"
        );
        let again = updated_at(&router, &detector_id, case).await;
        assert_eq!(
            again, before,
            "read {read}: a read must not move updatedAt, got {again:?}"
        );
    }
}
