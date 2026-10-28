// SPDX-License-Identifier: CC0-1.0

//! What a detector-configuration write that names NO configuration does, end to end
//! over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! serialization and the handlers are exercised together.
//!
//! The rule under test, asserted over a SET of request forms rather than one body: a
//! write whose every configurable member is OPTIONAL writes the record only for the
//! members the request carried, so a request that carries none of them changes
//! nothing — and a later read reports the identical record, its last-updated
//! timestamp included. A request that does carry one writes it, and that write moves
//! the timestamp.
//!
//! The rows span the forms in which a request can name no configuration: no member at
//! all, a present-but-empty list, a list whose entries name no switch, the
//! present-but-empty deprecated view, and the two empty views together; and they span
//! the two detector-configuration operations whose members are all optional,
//! `UpdateDetector` at `/detector/{DetectorId}` and `UpdateMalwareScanSettings` at
//! `/detector/{DetectorId}/malware-scan-settings`
//! (`smithy_query(operation="UpdateDetector,UpdateMalwareScanSettings")`).
//!
//! The writing rows are the counterpart: the timestamp is pinned as moving for each
//! member a request CAN carry, whether or not the value it names is the one the
//! detector already holds — the same reading the `detector_configuration_timestamp`
//! suite pins for the auto-enablement a fresh detector already carries.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// One request: the case name, the path suffix under `/detector/{detectorId}` the
/// operation is bound at, and the body.
type Write = (&'static str, &'static str, fn() -> Value);

/// The accepted requests that name no configuration at all.
///
/// Members come from `smithy_query(operation="UpdateDetector")` — `Enable`,
/// `FindingPublishingFrequency`, `DataSources` and `Features`, none of them required
/// — and `smithy_query(operation="UpdateMalwareScanSettings")`, whose
/// `ScanResourceCriteria` and `EbsSnapshotPreservation` are optional too.
const EMPTY_WRITES: [Write; 7] = [
    ("no member at all", "", || json!({})),
    (
        "a present but empty features list",
        "",
        || json!({"features": []}),
    ),
    (
        // `DetectorFeatureConfiguration.Name` is optional
        // (`smithy_query(shape="DetectorFeatureConfiguration")`), so an entry naming
        // no feature is well formed and configures nothing.
        "a features entry that names no feature",
        "",
        || json!({"features": [{}]}),
    ),
    (
        "a features entry carrying only a status",
        "",
        || json!({"features": [{"status": "ENABLED"}]}),
    ),
    (
        // Every member of `DataSourceConfigurations` is optional
        // (`smithy_query(shape="DataSourceConfigurations")`).
        "a present but empty dataSources view",
        "",
        || json!({"dataSources": {}}),
    ),
    (
        "both views present and both empty",
        "",
        || json!({"dataSources": {}, "features": []}),
    ),
    (
        "no member at all on the malware scan settings",
        "/malware-scan-settings",
        || json!({}),
    ),
];

/// The accepted requests that DO name configuration, one per member each operation
/// can carry.
const CONFIGURING_WRITES: [Write; 7] = [
    ("a disabling status", "", || json!({"enable": false})),
    (
        // The status the detector already holds: the request still WRITES the status,
        // so it is still a write to the record.
        "the status the detector already holds",
        "",
        || json!({"enable": true}),
    ),
    (
        "a publishing frequency",
        "",
        || json!({"findingPublishingFrequency": "FIFTEEN_MINUTES"}),
    ),
    (
        "a named feature switch",
        "",
        || json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
    ),
    (
        "a data source switch through the deprecated view",
        "",
        || json!({"dataSources": {"s3Logs": {"enable": false}}}),
    ),
    (
        "a named feature beside an empty dataSources view",
        "",
        || {
            json!({
                "dataSources": {},
                "features": [{"name": "EKS_AUDIT_LOGS", "status": "ENABLED"}],
            })
        },
    ),
    (
        "a malware scan setting",
        "/malware-scan-settings",
        || json!({"ebsSnapshotPreservation": "RETENTION_WITH_FINDING"}),
    ),
];

/// `GetDetector`, asserting it succeeded: the whole reported record, which is what
/// makes "nothing changed" an assertion about the record and not only its timestamp.
async fn get_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-detector expected 200, got {status} {body:?}"
    );
    body
}

/// The `updatedAt` a `GetDetector` reports. An ISO-8601 instant with a fixed-width
/// subsecond field, so comparing the text compares the instants.
async fn updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let body = get_detector(router, detector_id, case).await;
    body["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {body:?}"))
        .to_string()
}

/// Send one row's request, asserting the modelled success.
async fn send(router: &Router, detector_id: &str, (case, suffix, body): &Write) {
    let (status, got) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}{suffix}"),
        Some(body()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the write expected 200, got {status} {got:?}"
    );
}

/// A request that names no configuration is accepted and changes nothing a later read
/// reports — the timestamp included.
#[tokio::test]
async fn a_write_naming_no_configuration_leaves_the_detector_identical() {
    for row in &EMPTY_WRITES {
        let case = row.0;
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let before = get_detector(&router, &detector_id, case).await;
        send(&router, &detector_id, row).await;
        let after = get_detector(&router, &detector_id, case).await;

        assert_eq!(
            after, before,
            "case {case:?}: a write naming no configuration must leave the detector \
             exactly as it was, expected {before:?}, got {after:?}"
        );
    }
}

/// Repeating such a request changes nothing either: the timestamp does not creep
/// forward one call at a time.
#[tokio::test]
async fn repeating_a_write_naming_no_configuration_still_changes_nothing() {
    for row in &EMPTY_WRITES {
        let case = row.0;
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let before = get_detector(&router, &detector_id, case).await;
        for attempt in 1..=3 {
            send(&router, &detector_id, row).await;
            let after = get_detector(&router, &detector_id, case).await;
            assert_eq!(
                after, before,
                "case {case:?}: write {attempt} naming no configuration must leave the \
                 detector exactly as it was, expected {before:?}, got {after:?}"
            );
        }
    }
}

/// A request that carries a configuration member writes it, and that write moves the
/// detector's `updatedAt` without touching `createdAt`.
#[tokio::test]
async fn a_write_carrying_configuration_moves_the_updated_at() {
    for row in &CONFIGURING_WRITES {
        let case = row.0;
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let created = get_detector(&router, &detector_id, case).await;
        let created_at = created["createdAt"].as_str().unwrap_or_default().to_string();
        let before = created["updatedAt"].as_str().unwrap_or_default().to_string();

        send(&router, &detector_id, row).await;

        let read = get_detector(&router, &detector_id, case).await;
        let after = read["updatedAt"].as_str().unwrap_or_default().to_string();
        assert!(
            after > before,
            "case {case:?}: a write carrying configuration must move updatedAt: \
             {before:?} -> {after:?}"
        );
        assert_eq!(
            read["createdAt"].as_str().unwrap_or_default(),
            created_at,
            "case {case:?}: a write must not move createdAt, got {read:?}"
        );
    }
}

/// A request naming no configuration between two that do reports the LAST real
/// write's instant, and reverts nothing that write stored.
#[tokio::test]
async fn a_write_naming_no_configuration_keeps_the_last_real_writes_instant() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let case = "the status write";
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"enable": false, "findingPublishingFrequency": "ONE_HOUR"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the write expected 200, got {status} {got:?}"
    );
    let after_real_write = get_detector(&router, &detector_id, case).await;
    assert_eq!(
        after_real_write["status"].as_str().unwrap_or_default(),
        "DISABLED",
        "case {case:?}: the write must be what a later read reports, got {after_real_write:?}"
    );

    for row in &EMPTY_WRITES {
        send(&router, &detector_id, row).await;
        let read = get_detector(&router, &detector_id, row.0).await;
        assert_eq!(
            read, after_real_write,
            "case {:?}: a write naming no configuration must neither revert the last \
             write nor move its instant, expected {after_real_write:?}, got {read:?}",
            row.0
        );
    }

    // The next request that does carry configuration still moves the timestamp on.
    let case = "the frequency write after the empty ones";
    let before = updated_at(&router, &detector_id, case).await;
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"findingPublishingFrequency": "FIFTEEN_MINUTES"})),
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
        "case {case:?}: a write carrying configuration must move updatedAt: \
         {before:?} -> {after:?}"
    );
    assert_eq!(
        read["findingPublishingFrequency"].as_str().unwrap_or_default(),
        "FIFTEEN_MINUTES",
        "case {case:?}: the write must be what a later read reports, got {read:?}"
    );
}
