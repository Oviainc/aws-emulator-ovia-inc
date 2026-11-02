// SPDX-License-Identifier: CC0-1.0

//! `CreateFramework` and the read that proves what it stored, driven as real
//! HTTP through the router.
//!
//! The rules under test:
//!
//! * an audit framework is a STORED resource: the create mints the framework
//!   ARN and records the controls, the description and the creation instant it
//!   was given, and a later `DescribeFramework` of that name reports them
//!   member for member;
//! * a framework is returned in a non-terminal deployment status, so the
//!   `CREATE_IN_PROGRESS` -> `COMPLETED` transition its clock owes is observable
//!   and `FrameworkStatus` follows it;
//! * a create that repeats an `IdempotencyToken` answers with the framework
//!   that token already made ("Retrying a successful request with the same
//!   idempotency token results in a success message with no action taken"), so
//!   no second framework appears;
//! * any other create naming a framework the account already holds is refused
//!   with `AlreadyExistsException`;
//! * a control list outside what AWS accepts — no controls at all, one control
//!   name used twice, a control scope with more than one tag pair, a scope
//!   resource-id list outside its bound — is refused with
//!   `InvalidParameterValueException` and stores nothing;
//! * a framework name the account does not hold is `ResourceNotFoundException`.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

use super::vault_and_backup_job::{call, error_code, router, ACCOUNT, REGION};

/// Longer than the framework deployment's settle span
/// (`crate::state::FRAMEWORK_SETTLE_AFTER`), so a read after this wait observes
/// the deployment the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`, for the reason given on
/// `vault_and_backup_job::PAST_SETTLE`: the workspace's `tokio` dependency does
/// not enable the `time` feature, and the handlers read the wall clock anyway.
pub(crate) const PAST_FRAMEWORK_SETTLE: Duration = Duration::from_millis(3_400);

/// The shortest `FrameworkName` the model admits (`smithy_query` operation
/// `CreateFramework`: `FrameworkName`, `@length` 1..256).
const MIN_FRAMEWORK_NAME_LENGTH: usize = 1;

/// The longest `FrameworkName` the model admits (same `@length`).
pub(crate) const MAX_FRAMEWORK_NAME_LENGTH: usize = 256;

/// The alphabet the `FrameworkName` pattern admits after the leading letter
/// (`^[a-zA-Z][_a-zA-Z0-9]*$`).
const NAME_TAIL_ALPHABET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";

/// `DeploymentStatus` of a framework whose controls are not deployed yet, and of
/// one whose deployment has settled — two of the statuses the model lists for
/// `Framework$DeploymentStatus`.
const DEPLOYMENT_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
pub(crate) const DEPLOYMENT_COMPLETED: &str = "COMPLETED";

/// `FrameworkStatus` values the model documents for
/// `DescribeFrameworkOutput$FrameworkStatus`: `ACTIVE` once recording is
/// validated for every resource the framework governs, `UNAVAILABLE` while
/// Backup cannot validate it.
pub(crate) const FRAMEWORK_STATUS_ACTIVE: &str = "ACTIVE";
pub(crate) const FRAMEWORK_STATUS_UNAVAILABLE: &str = "UNAVAILABLE";

/// `POST /audit/frameworks` — `CreateFramework`'s HTTP binding.
pub(crate) async fn create_framework(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/audit/frameworks",
        Some(&request.to_string()),
    )
    .await
}

/// `GET /audit/frameworks/{FrameworkName}` — `DescribeFramework`'s HTTP binding.
pub(crate) async fn describe_framework(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/audit/frameworks/{name}"), None).await
}

/// A string member of a JSON object, or the empty string.
pub(crate) fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A deterministic framework name of `length` characters that satisfies the
/// model's pattern: a leading letter, then characters drawn from the whole
/// admitted tail alphabet, so the length bound is exercised across it rather
/// than on one hand-picked spelling.
pub(crate) fn framework_name_of_length(length: usize) -> String {
    (0..length)
        .map(|index| {
            if index == 0 {
                'F'
            } else {
                char::from(NAME_TAIL_ALPHABET[(index * 7 + length) % NAME_TAIL_ALPHABET.len()])
            }
        })
        .collect()
}

/// The create requests the round-trip table walks: the rows differ in how many
/// controls the framework carries, in which of a control's optional members are
/// set, in how the control scope selects resources and in the length of the
/// name — which is what the "read back what was stored" rule generalises over.
///
/// The control names are the published Backup Audit Manager control identifiers
/// (`ControlName`: "The name of a control"), including ones this issue's
/// recording did not cite.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one control carrying only its required member",
            json!({
                "FrameworkName": "MinimalFramework",
                "FrameworkControls": [
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                ],
            }),
        ),
        (
            "a description, a token and a control with input parameters",
            json!({
                "FrameworkName": "ParameterisedFramework",
                "FrameworkDescription": "frequency and retention of every plan",
                "IdempotencyToken": "round-trip-token",
                "FrameworkControls": [{
                    "ControlName": "BACKUP_PLAN_MIN_FREQUENCY_AND_MIN_RETENTION_CHECK",
                    "ControlInputParameters": [
                        {"ParameterName": "requiredFrequencyUnit", "ParameterValue": "hours"},
                        {"ParameterName": "requiredRetentionDays", "ParameterValue": "35"},
                    ],
                }],
            }),
        ),
        (
            "a control scope naming resource types",
            json!({
                "FrameworkName": "ScopedByType",
                "FrameworkControls": [{
                    "ControlName": "BACKUP_RESOURCES_PROTECTED_BY_BACKUP_PLAN",
                    "ControlScope": {"ComplianceResourceTypes": ["EBS", "RDS", "DynamoDB"]},
                }],
            }),
        ),
        (
            "a control scope carrying the one tag pair the model admits",
            json!({
                "FrameworkName": "ScopedByTag",
                "FrameworkControls": [{
                    "ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED",
                    "ControlScope": {"Tags": {"tier": "gold"}},
                }],
            }),
        ),
        (
            "three controls, each scoped differently",
            json!({
                "FrameworkName": "MultiControlFramework",
                "FrameworkDescription": "one of each scope form",
                "FrameworkControls": [
                    {
                        "ControlName": "BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK",
                        "ControlInputParameters": [
                            {"ParameterName": "requiredRetentionDays", "ParameterValue": "1"},
                        ],
                        "ControlScope": {"ComplianceResourceTypes": ["EFS"]},
                    },
                    {
                        "ControlName": "BACKUP_RECOVERY_POINT_MANUAL_DELETION_DISABLED",
                        "ControlScope": {
                            "ComplianceResourceTypes": ["S3"],
                            "ComplianceResourceIds": ["arn:aws:s3:::audited-bucket"],
                        },
                    },
                    {"ControlName": "BACKUP_PLAN_MIN_FREQUENCY_AND_MIN_RETENTION_CHECK"},
                ],
            }),
        ),
        (
            "framework tags assigned to the framework",
            json!({
                "FrameworkName": "TaggedFramework",
                "FrameworkTags": {"owner": "backup-team", "env": "test"},
                "FrameworkControls": [
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                ],
            }),
        ),
        (
            "a name at the shortest length the model admits",
            json!({
                "FrameworkName": framework_name_of_length(MIN_FRAMEWORK_NAME_LENGTH),
                "FrameworkControls": [
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                ],
            }),
        ),
        (
            "a name at the longest length the model admits",
            json!({
                "FrameworkName": framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH),
                "FrameworkControls": [
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                ],
            }),
        ),
    ]
}

/// Every member a framework is created with is reported by a later read of it,
/// the ARN the create minted names that framework, and a framework whose
/// controls are still deploying reports the non-terminal status it is in.
#[tokio::test]
async fn a_created_framework_is_read_back_member_for_member() {
    for (case, request) in round_trip_cases() {
        // One router per row: each row's framework is the only one the account
        // holds, so a row cannot see what another row stored.
        let router = router();
        let (status, created) = create_framework(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {created:?}"
        );

        let name = text(&request, "FrameworkName");
        assert_eq!(
            created.get("FrameworkName"),
            request.get("FrameworkName"),
            "case {case:?}: expected the stored name back, got {created:?}"
        );
        let arn = text(&created, "FrameworkArn");
        let prefix = format!("arn:aws:backup:{REGION}:{ACCOUNT}:framework:{name}-");
        assert!(
            arn.starts_with(&prefix) && arn.len() > prefix.len(),
            "case {case:?}: expected a framework ARN starting {prefix:?} and a uniquifier, \
             got {arn:?}"
        );

        let (status, read) = describe_framework(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeFramework 200, got {status} {read:?}"
        );
        for member in ["FrameworkName", "FrameworkArn"] {
            assert_eq!(
                read.get(member),
                created.get(member),
                "case {case:?}: {member} read back differs from the created one, got {read:?}"
            );
        }
        for member in [
            "FrameworkDescription",
            "FrameworkControls",
            "IdempotencyToken",
        ] {
            assert_eq!(
                read.get(member),
                request.get(member),
                "case {case:?}: {member} must report what the create was given, got {read:?}"
            );
        }
        assert!(
            read["CreationTime"].is_number(),
            "case {case:?}: expected a CreationTime, got {read:?}"
        );
        assert_eq!(
            (
                text(&read, "DeploymentStatus").as_str(),
                text(&read, "FrameworkStatus").as_str(),
            ),
            (DEPLOYMENT_CREATE_IN_PROGRESS, FRAMEWORK_STATUS_UNAVAILABLE),
            "case {case:?}: a framework read straight after its create is still deploying, \
             got {read:?}"
        );

        // A read changes nothing a later reader sees: the second read reports
        // the same ARN and the same creation instant.
        let (status, again) = describe_framework(&router, &name).await;
        assert_eq!(
            (status, again.get("FrameworkArn"), again.get("CreationTime")),
            (200, read.get("FrameworkArn"), read.get("CreationTime")),
            "case {case:?}: two reads of one framework must agree, got {again:?}"
        );
    }
}

/// The deployment a create starts is owed by the clock: the framework reports
/// `CREATE_IN_PROGRESS` straight after the call and `COMPLETED` once the span
/// has passed, with `FrameworkStatus` following it, and nothing else about the
/// framework changes.
#[tokio::test]
async fn a_framework_deployment_settles_from_create_in_progress_to_completed() {
    let router = router();
    let request = json!({
        "FrameworkName": "SettlingFramework",
        "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
    });
    let (status, created) = create_framework(&router, &request).await;
    assert_eq!(
        status, 200,
        "expected CreateFramework 200, got {status} {created:?}"
    );

    let (status, deploying) = describe_framework(&router, "SettlingFramework").await;
    assert_eq!(
        (status, text(&deploying, "DeploymentStatus").as_str()),
        (200, DEPLOYMENT_CREATE_IN_PROGRESS),
        "expected the transient deployment status, got {status} {deploying:?}"
    );

    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    let (status, deployed) = describe_framework(&router, "SettlingFramework").await;
    assert_eq!(
        (
            status,
            text(&deployed, "DeploymentStatus").as_str(),
            text(&deployed, "FrameworkStatus").as_str(),
        ),
        (200, DEPLOYMENT_COMPLETED, FRAMEWORK_STATUS_ACTIVE),
        "expected a settled deployment, got {status} {deployed:?}"
    );
    for member in ["FrameworkName", "FrameworkArn", "CreationTime"] {
        assert_eq!(
            deployed.get(member),
            deploying.get(member),
            "settling must not change {member}, got {deployed:?}"
        );
    }
}

/// A repeated `IdempotencyToken` replays the framework that token made: the
/// same name and ARN, and no second framework for the name the retry named.
#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_first_framework() {
    let router = router();
    let first = json!({
        "FrameworkName": "TokenFramework",
        "IdempotencyToken": "token-1",
        "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
    });
    let (status, created) = create_framework(&router, &first).await;
    assert_eq!(
        status, 200,
        "expected the first CreateFramework 200, got {status} {created:?}"
    );

    // The same token with a DIFFERENT name: no action is taken, so the answer
    // is the framework the token already made.
    let retry = json!({
        "FrameworkName": "OtherFramework",
        "IdempotencyToken": "token-1",
        "FrameworkControls": [{"ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED"}],
    });
    let (status, replayed) = create_framework(&router, &retry).await;
    assert_eq!(
        (status, &replayed),
        (200, &created),
        "a replayed token must answer with the framework it made, got {status} {replayed:?}"
    );

    let (status, missing) = describe_framework(&router, "OtherFramework").await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (400, "ResourceNotFoundException"),
        "a replayed token must create no second framework, got {status} {missing:?}"
    );
}

/// A create that is not a token replay and names a framework the account
/// already holds is refused, and the refusal discloses nothing about the stored
/// framework.
#[tokio::test]
async fn a_second_create_of_one_name_is_already_exists() {
    let router = router();
    let request = json!({
        "FrameworkName": "DuplicateFramework",
        "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
    });
    let (status, created) = create_framework(&router, &request).await;
    assert_eq!(
        status, 200,
        "expected the first CreateFramework 200, got {status} {created:?}"
    );

    let (status, refused) = create_framework(&router, &request).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "AlreadyExistsException"),
        "expected the second create to be refused, got {status} {refused:?}"
    );
    assert!(
        !text(&refused, "Message").is_empty(),
        "the refusal states the condition, got {refused:?}"
    );
    for member in ["Arn", "CreatorRequestId"] {
        assert!(
            refused.get(member).is_none(),
            "the refusal must not disclose the stored framework's {member}, got {refused:?}"
        );
    }

    // The stored framework is untouched by the refused create.
    let (status, read) = describe_framework(&router, "DuplicateFramework").await;
    assert_eq!(
        (status, read.get("FrameworkArn")),
        (200, created.get("FrameworkArn")),
        "a refused create must change nothing, got {status} {read:?}"
    );
}

/// A create carrying a control list, a name or a scope AWS does not accept is
/// refused with the modelled exception for that condition and stores nothing.
///
/// The last element of each row is the name to probe afterwards, for a row whose
/// name is one the read path admits.
#[tokio::test]
async fn a_create_outside_what_aws_accepts_is_refused_and_stores_nothing() {
    let cases: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "no controls at all",
            json!({"FrameworkName": "EmptyControls", "FrameworkControls": []}),
            "InvalidParameterValueException",
            Some("EmptyControls"),
        ),
        (
            "one control name used twice",
            json!({
                "FrameworkName": "RepeatedControl",
                "FrameworkControls": [
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                    {"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"},
                ],
            }),
            "InvalidParameterValueException",
            Some("RepeatedControl"),
        ),
        (
            "a control scope carrying two tag pairs",
            json!({
                "FrameworkName": "TwoScopeTags",
                "FrameworkControls": [{
                    "ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED",
                    "ControlScope": {"Tags": {"tier": "gold", "owner": "backup-team"}},
                }],
            }),
            "InvalidParameterValueException",
            Some("TwoScopeTags"),
        ),
        (
            "a scope resource-id list one past its documented floor",
            json!({
                "FrameworkName": "EmptyScopeIds",
                "FrameworkControls": [{
                    "ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED",
                    "ControlScope": {"ComplianceResourceIds": []},
                }],
            }),
            "InvalidParameterValueException",
            Some("EmptyScopeIds"),
        ),
        (
            "a name one past the longest the model admits",
            json!({
                "FrameworkName": framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH + 1),
                "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
            }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "a name that does not start with a letter",
            json!({
                "FrameworkName": "9NotAName",
                "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
            }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "no FrameworkControls member at all",
            json!({"FrameworkName": "NoControlsMember"}),
            "MissingParameterValueException",
            Some("NoControlsMember"),
        ),
        (
            "no FrameworkName member at all",
            json!({
                "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
            }),
            "MissingParameterValueException",
            None,
        ),
    ];

    for (case, request, expected, probe) in cases {
        let router = router();
        let (status, refused) = create_framework(&router, &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {refused:?}"
        );

        if let Some(name) = probe {
            let (status, missing) = describe_framework(&router, name).await;
            assert_eq!(
                (status, error_code(&missing).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: a refused create must store nothing, got {status} {missing:?}"
            );
        }
    }
}

/// A framework name the account holds nothing under is refused by the read, and
/// a name it does hold is not.
#[tokio::test]
async fn a_read_of_an_unheld_name_is_resource_not_found() {
    let router = router();
    let (status, created) = create_framework(
        &router,
        &json!({
            "FrameworkName": "HeldFramework",
            "FrameworkControls": [{"ControlName": "BACKUP_RECOVERY_POINT_ENCRYPTED"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateFramework 200, got {status} {created:?}"
    );

    for name in [
        "NeverCreated",
        "heldframework",
        &framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH),
    ] {
        let (status, missing) = describe_framework(&router, name).await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (400, "ResourceNotFoundException"),
            "name {name:?}: expected the read to be refused, got {status} {missing:?}"
        );
        assert!(
            !text(&missing, "Message").is_empty(),
            "name {name:?}: the refusal states the condition, got {missing:?}"
        );
    }

    let (status, read) = describe_framework(&router, "HeldFramework").await;
    assert_eq!(
        (status, read.get("FrameworkArn")),
        (200, created.get("FrameworkArn")),
        "the name the account does hold reads back, got {status} {read:?}"
    );
}
