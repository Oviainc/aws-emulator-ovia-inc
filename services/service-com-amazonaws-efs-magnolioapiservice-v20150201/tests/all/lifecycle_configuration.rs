// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS lifecycle-configuration slice:
//! PutLifecycleConfiguration and DescribeLifecycleConfiguration, driven over HTTP
//! through the same `MultiServiceRouter` the production entrypoint serves, on the
//! harness `file_system_lifecycle` owns.
//!
//! The rule under test is a round trip over a SET of configurations rather than one
//! recorded call: the tables below vary the number of policies (0..3), which transition
//! each names and the value it carries, and the day-based catalog is walked whole.

use serde_json::{json, Value};

use crate::file_system_lifecycle::{settle, Emulator, Reply};

/// Every value of the modelled `TransitionToIARules` enum
/// (`smithy_query(query="shape",shape="TransitionToIARules")`), lowest day count first.
/// A configuration naming any one of them round-trips unchanged.
const TRANSITION_TO_IA_VALUES: &[&str] = &[
    "AFTER_1_DAY",
    "AFTER_7_DAYS",
    "AFTER_14_DAYS",
    "AFTER_30_DAYS",
    "AFTER_60_DAYS",
    "AFTER_90_DAYS",
    "AFTER_180_DAYS",
    "AFTER_270_DAYS",
    "AFTER_365_DAYS",
];

/// Every value of the modelled `TransitionToArchiveRules` enum
/// (`smithy_query(query="shape",shape="TransitionToArchiveRules")`), lowest day count
/// first.
const TRANSITION_TO_ARCHIVE_VALUES: &[&str] = &[
    "AFTER_1_DAY",
    "AFTER_7_DAYS",
    "AFTER_14_DAYS",
    "AFTER_30_DAYS",
    "AFTER_60_DAYS",
    "AFTER_90_DAYS",
    "AFTER_180_DAYS",
    "AFTER_270_DAYS",
    "AFTER_365_DAYS",
];

/// The one value of the modelled `TransitionToPrimaryStorageClassRules` enum
/// (`smithy_query(query="shape",shape="TransitionToPrimaryStorageClassRules")`).
const TRANSITION_TO_PRIMARY_VALUE: &str = "AFTER_1_ACCESS";

fn lifecycle_path(file_system_id: &str) -> String {
    format!("/2015-02-01/file-systems/{file_system_id}/lifecycle-configuration")
}

/// `PUT /2015-02-01/file-systems/{FileSystemId}/lifecycle-configuration`.
async fn put(efs: &Emulator, file_system_id: &str, policies: &Value) -> Reply {
    efs.send(
        "PUT",
        &lifecycle_path(file_system_id),
        Some(json!({ "LifecyclePolicies": policies })),
    )
    .await
}

/// `GET /2015-02-01/file-systems/{FileSystemId}/lifecycle-configuration`.
async fn describe(efs: &Emulator, file_system_id: &str) -> Reply {
    efs.send("GET", &lifecycle_path(file_system_id), None).await
}

/// One `available` file system with the throughput mode the Archive storage class
/// requires ("Elastic throughput mode and the General Purpose performance mode", which
/// is the default `PerformanceMode`).
async fn elastic_file_system(efs: &Emulator, token: &str) -> String {
    let created = efs
        .create_file_system(json!({
            "CreationToken": token,
            "ThroughputMode": "elastic",
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "creating elastic file system {token}: expected 201, got {} {:?}",
        created.status,
        created.get("Message")
    );
    let id = created.string("FileSystemId");
    settle().await;
    id
}

/// Put `policies` and assert the answer, then read it back and assert the stored
/// configuration is exactly what was put — the round trip, for one case of a table.
async fn assert_round_trip(efs: &Emulator, file_system_id: &str, case: &str, policies: Value) {
    let stored = put(efs, file_system_id, &policies).await;
    assert_eq!(
        stored.status, 200,
        "case {case:?}: PutLifecycleConfiguration expected 200, got {} {:?}",
        stored.status,
        stored.get("Message")
    );
    assert_eq!(
        stored.get("LifecyclePolicies"),
        &policies,
        "case {case:?}: PutLifecycleConfiguration must answer with the policies it \
         stored, got {:?}",
        stored.get("LifecyclePolicies")
    );

    let read_back = describe(efs, file_system_id).await;
    assert_eq!(
        read_back.status, 200,
        "case {case:?}: DescribeLifecycleConfiguration expected 200, got {} {:?}",
        read_back.status,
        read_back.get("Message")
    );
    assert_eq!(
        read_back.get("LifecyclePolicies"),
        &policies,
        "case {case:?}: DescribeLifecycleConfiguration must report the stored \
         configuration, got {:?}",
        read_back.get("LifecyclePolicies")
    );
}

// ---------------------------------------------------------------------------
// The round trip, over the configurations the model allows.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_reports_every_put_configuration_shape() {
    let efs = Emulator::new();
    // Elastic throughput, so every transition — Archive included — is allowed here.
    let file_system_id = elastic_file_system(&efs, "lifecycle-shapes").await;

    // The table varies the member the rule generalises over: how many policies the
    // configuration holds (0 through the modelled maximum of 3), which transition each
    // one names, and the order they arrive in.
    let shapes: Vec<(&str, Value)> = vec![
        ("no policies", json!([])),
        (
            "one IA transition",
            json!([{ "TransitionToIA": "AFTER_7_DAYS" }]),
        ),
        (
            "one primary-storage transition",
            json!([{ "TransitionToPrimaryStorageClass": TRANSITION_TO_PRIMARY_VALUE }]),
        ),
        (
            "IA then archive",
            json!([
                { "TransitionToIA": "AFTER_1_DAY" },
                { "TransitionToArchive": "AFTER_90_DAYS" },
            ]),
        ),
        (
            "primary storage before IA",
            json!([
                { "TransitionToPrimaryStorageClass": TRANSITION_TO_PRIMARY_VALUE },
                { "TransitionToIA": "AFTER_365_DAYS" },
            ]),
        ),
        (
            "all three transitions",
            json!([
                { "TransitionToIA": "AFTER_14_DAYS" },
                { "TransitionToArchive": "AFTER_270_DAYS" },
                { "TransitionToPrimaryStorageClass": TRANSITION_TO_PRIMARY_VALUE },
            ]),
        ),
    ];

    for (case, policies) in shapes {
        assert_round_trip(&efs, &file_system_id, case, policies).await;
    }
}

#[tokio::test]
async fn describe_reports_every_published_transition_value() {
    let efs = Emulator::new();
    let bursting = efs.available_file_system("lifecycle-ia-catalog").await;
    let elastic = elastic_file_system(&efs, "lifecycle-archive-catalog").await;

    // The whole published `TransitionToIA` catalog, on a default (bursting) file system.
    for value in TRANSITION_TO_IA_VALUES {
        assert_round_trip(
            &efs,
            &bursting,
            value,
            json!([{ "TransitionToIA": value }]),
        )
        .await;
    }

    // The whole published `TransitionToArchive` catalog, each paired with the earliest
    // IA transition so the archive transition is always the later one.
    for value in TRANSITION_TO_ARCHIVE_VALUES.iter().skip(1) {
        assert_round_trip(
            &efs,
            &elastic,
            value,
            json!([
                { "TransitionToIA": "AFTER_1_DAY" },
                { "TransitionToArchive": value },
            ]),
        )
        .await;
    }

    // Each file system holds its own configuration: the last write to one is not the
    // configuration of the other.
    let on_bursting = describe(&efs, &bursting).await;
    assert_eq!(
        on_bursting.get("LifecyclePolicies"),
        &json!([{ "TransitionToIA": "AFTER_365_DAYS" }]),
        "a configuration belongs to the file system it was put on, got {:?}",
        on_bursting.get("LifecyclePolicies")
    );
}

#[tokio::test]
async fn describe_reports_an_empty_array_without_a_configuration() {
    let efs = Emulator::new();

    // "For a file system without a LifecycleConfiguration object, the call returns an
    // empty array in the response" — true of a fresh file system, and true again after
    // an empty put deletes the configuration it had.
    for (case, token) in [("fresh", "lifecycle-empty-1"), ("deleted", "lifecycle-empty-2")] {
        let file_system_id = efs.available_file_system(token).await;
        if case == "deleted" {
            assert_round_trip(
                &efs,
                &file_system_id,
                "configuration to delete",
                json!([{ "TransitionToIA": "AFTER_60_DAYS" }]),
            )
            .await;
            let deleted = put(&efs, &file_system_id, &json!([])).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: an empty LifecyclePolicies array deletes the \
                 configuration, got {} {:?}",
                deleted.status,
                deleted.get("Message")
            );
        }
        let read = describe(&efs, &file_system_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            read.status,
            read.get("Message")
        );
        assert_eq!(
            read.get("LifecyclePolicies"),
            &json!([]),
            "case {case:?}: expected an empty array, got {:?}",
            read.get("LifecyclePolicies")
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals, and that a refused put stores nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_malformed_configuration_is_refused_and_stores_nothing() {
    let efs = Emulator::new();
    let elastic = elastic_file_system(&efs, "lifecycle-refusals").await;
    let kept = json!([{ "TransitionToIA": "AFTER_30_DAYS" }]);
    assert_round_trip(&efs, &elastic, "the configuration to keep", kept.clone()).await;

    // Each row breaks one documented rule of a LifecycleConfiguration.
    let refused: Vec<(&str, Value)> = vec![
        ("no transition at all", json!([{}])),
        (
            "two transitions in one policy object",
            json!([{ "TransitionToIA": "AFTER_30_DAYS", "TransitionToArchive": "AFTER_60_DAYS" }]),
        ),
        (
            "the same transition twice",
            json!([
                { "TransitionToIA": "AFTER_7_DAYS" },
                { "TransitionToIA": "AFTER_30_DAYS" },
            ]),
        ),
        (
            "archive on the same day as IA",
            json!([
                { "TransitionToIA": "AFTER_30_DAYS" },
                { "TransitionToArchive": "AFTER_30_DAYS" },
            ]),
        ),
        (
            "archive earlier than IA",
            json!([
                { "TransitionToIA": "AFTER_30_DAYS" },
                { "TransitionToArchive": "AFTER_7_DAYS" },
            ]),
        ),
        (
            "archive without IA",
            json!([{ "TransitionToArchive": "AFTER_30_DAYS" }]),
        ),
    ];

    for (case, policies) in refused {
        let reply = put(&efs, &elastic, &policies).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status, reply.error_type
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequest"),
            "case {case:?}: expected a BadRequest, got {:?}",
            reply.error_type
        );
        let read = describe(&efs, &elastic).await;
        assert_eq!(
            read.get("LifecyclePolicies"),
            &kept,
            "case {case:?}: a refused put must leave the stored configuration alone, \
             got {:?}",
            read.get("LifecyclePolicies")
        );
    }
}

#[tokio::test]
async fn the_archive_transition_needs_elastic_general_purpose_throughput() {
    let efs = Emulator::new();
    let policies = json!([
        { "TransitionToIA": "AFTER_1_DAY" },
        { "TransitionToArchive": "AFTER_30_DAYS" },
    ]);

    // "The Archive storage class is available only for file systems that use the Elastic
    // throughput mode and the General Purpose performance mode."
    let unsupported: Vec<(&str, Value)> = vec![
        (
            "bursting throughput",
            json!({ "CreationToken": "archive-bursting", "ThroughputMode": "bursting" }),
        ),
        (
            "provisioned throughput",
            json!({
                "CreationToken": "archive-provisioned",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 64.0,
            }),
        ),
        (
            "maxIO performance",
            json!({
                "CreationToken": "archive-max-io",
                "ThroughputMode": "bursting",
                "PerformanceMode": "maxIO",
            }),
        ),
    ];

    for (case, request) in unsupported {
        let created = efs.create_file_system(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status,
            created.get("Message")
        );
        let file_system_id = created.string("FileSystemId");
        settle().await;

        let refused = put(&efs, &file_system_id, &policies).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an archive transition is not available here, expected 400, \
             got {} {:?}",
            refused.status,
            refused.get("Message")
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequest"),
            "case {case:?}: expected a BadRequest, got {:?}",
            refused.error_type
        );

        // The IA transition on its own is still allowed on the same file system.
        assert_round_trip(
            &efs,
            &file_system_id,
            case,
            json!([{ "TransitionToIA": "AFTER_1_DAY" }]),
        )
        .await;
    }

    // And on an elastic, general-purpose file system the same configuration is stored.
    let elastic = elastic_file_system(&efs, "archive-elastic").await;
    assert_round_trip(&efs, &elastic, "elastic throughput", policies).await;
}

#[tokio::test]
async fn an_unknown_file_system_is_not_found_by_either_operation() {
    let efs = Emulator::new();
    let known = efs.available_file_system("lifecycle-known").await;
    let policies = json!([{ "TransitionToIA": "AFTER_30_DAYS" }]);

    // "Returned if the specified FileSystemId value doesn't exist in the requester's
    // Amazon Web Services account" — for an id of the modelled form that names nothing,
    // whether it is read or written.
    for unknown in ["fs-00000000", "fs-0123456789abcdef"] {
        for (operation, reply) in [
            ("PutLifecycleConfiguration", put(&efs, unknown, &policies).await),
            ("DescribeLifecycleConfiguration", describe(&efs, unknown).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {unknown:?} on {operation}: expected 404, got {} {:?}",
                reply.status,
                reply.get("Message")
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("FileSystemNotFound"),
                "case {unknown:?} on {operation}: unexpected error type, got {:?}",
                reply.error_type
            );
        }
    }

    // The ARN form of the same member names the file system just as its id does. A
    // label value carries no literal `/` or `:`, so the ARN arrives percent-encoded,
    // exactly as an SDK encodes a label.
    let arn = format!(
        "arn%3Aaws%3Aelasticfilesystem%3Aus-east-1%3A000000000000%3Afile-system%2F{known}"
    );
    assert_round_trip(&efs, &arn, "file system ARN", policies).await;
    let by_id = describe(&efs, &known).await;
    assert_eq!(
        by_id.get("LifecyclePolicies"),
        &json!([{ "TransitionToIA": "AFTER_30_DAYS" }]),
        "a configuration put by ARN is readable by id, got {:?}",
        by_id.get("LifecyclePolicies")
    );
}

#[tokio::test]
async fn putting_on_a_file_system_that_is_still_creating_is_a_conflict() {
    let efs = Emulator::new();
    let created = efs
        .create_file_system(json!({ "CreationToken": "lifecycle-creating" }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status,
        created.get("Message")
    );
    let file_system_id = created.string("FileSystemId");
    assert_eq!(
        created.string("LifeCycleState"),
        "creating",
        "a new file system is reported while still creating, got {:?}",
        created.get("LifeCycleState")
    );

    // `IncorrectFileSystemLifeCycleState`: "Returned if the file system's lifecycle state
    // is not available."
    let refused = put(
        &efs,
        &file_system_id,
        &json!([{ "TransitionToIA": "AFTER_30_DAYS" }]),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "put while creating: expected 409, got {} {:?}",
        refused.status,
        refused.get("Message")
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "put while creating: unexpected error type, got {:?}",
        refused.error_type
    );

    // Reading is allowed in every lifecycle state, and reports no configuration.
    let read = describe(&efs, &file_system_id).await;
    assert_eq!(
        read.status, 200,
        "describe while creating: expected 200, got {} {:?}",
        read.status,
        read.get("Message")
    );
    assert_eq!(
        read.get("LifecyclePolicies"),
        &json!([]),
        "describe while creating: expected an empty array, got {:?}",
        read.get("LifecyclePolicies")
    );

    // Once the clock has made the transition due, the same put is stored.
    settle().await;
    assert_round_trip(
        &efs,
        &file_system_id,
        "after the creating span",
        json!([{ "TransitionToIA": "AFTER_30_DAYS" }]),
    )
    .await;
}
