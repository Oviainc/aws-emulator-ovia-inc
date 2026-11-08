// SPDX-License-Identifier: CC0-1.0

//! What a plan READ answers once `DeleteBackupPlan` has recorded the plan's
//! deletion, driven as real HTTP through the router.
//!
//! The rule under test: a plan read answers from a version the account still
//! HOLDS, so once a plan's deletion is recorded the version it held is no longer
//! there to read and every read of it is refused with the modelled 400
//! `ResourceNotFoundException` for an unreadable version — exactly as a
//! `VersionId` naming a version the plan never had is refused. A plan nothing
//! has deleted keeps reading back, and a plan id the account never held keeps
//! answering its own refusal.
//!
//! The rows differ in how the read addresses the plan (no version, the version
//! the create minted, a version the plan never had) and in how many rules and
//! advanced settings the deleted plan carried — which is what the rule
//! generalises over; the behavioural instance (one single-rule plan read with no
//! version after its delete) is one row.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call, error_code, router, ACCOUNT};

/// The sentence AWS states for "the version of the plan this read asks for
/// cannot be read", which this service words once (`crate::state`'s
/// `MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE`), with that condition's own `Code`
/// (`ERROR_CODE_BACKUP_PLAN_VERSION_UNREADABLE`).
const VERSION_UNREADABLE: &str = "Failed reading Backup plan with provided version";
const VERSION_UNREADABLE_CODE: &str = "ERROR_2202";

/// The error shape every one of these refusals names (`smithy_query` operation
/// `GetBackupPlan`: `ResourceNotFoundException`, `httpError` 400).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A version id of the form this service mints but no plan of a fresh account
/// carries.
const UNHELD_VERSION: &str = "00000000-0000-0000-0000-000000000000";

/// `PUT /backup/plans` — `CreateBackupPlan`'s HTTP binding.
async fn create_plan(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "PUT", "/backup/plans", Some(&request.to_string())).await
}

/// `GET /backup/plans/{BackupPlanId}` — `GetBackupPlan`'s HTTP binding, with the
/// optional `versionId` query parameter.
async fn get_plan(
    router: &MultiServiceRouter,
    plan_id: &str,
    version_id: Option<&str>,
) -> (u16, Value) {
    let path = match version_id {
        Some(version) => format!("/backup/plans/{plan_id}?versionId={version}"),
        None => format!("/backup/plans/{plan_id}"),
    };
    call(router, "GET", &path, None).await
}

/// `DELETE /backup/plans/{BackupPlanId}` — `DeleteBackupPlan`'s HTTP binding.
async fn delete_plan(router: &MultiServiceRouter, plan_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/backup/plans/{plan_id}"), None).await
}

/// `GET /backup/plans/{BackupPlanId}/selections` — `ListBackupSelections`'s
/// binding, which reports the selections assigned to the plan rather than
/// resolving the plan itself.
async fn list_selections(router: &MultiServiceRouter, plan_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/backup/plans/{plan_id}/selections"),
        None,
    )
    .await
}

/// A string member of a JSON object, or the empty string.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The plan bodies the table walks: the rows differ in how many rules the plan
/// carries, in whether it carries advanced backup settings and a
/// `CreatorRequestId`, and in the schedule its rules keep — the deletion rule
/// holds whatever the deleted plan's body was.
fn deleted_plan_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one rule carrying only its required members",
            json!({"BackupPlan": {
                "BackupPlanName": "minimal-plan",
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
            }}),
        ),
        (
            "one scheduled rule, as the behavioural instance carries",
            json!({"BackupPlan": {
                "BackupPlanName": "scheduled-plan",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "ScheduleExpression": "cron(0 5 * * ? *)",
                    "StartWindowMinutes": 480,
                    "CompletionWindowMinutes": 10080,
                }],
            }}),
        ),
        (
            "three rules and a creator request id",
            json!({"BackupPlan": {
                "BackupPlanName": "multi-rule-plan",
                "Rules": [
                    {"RuleName": "hourly", "TargetBackupVaultName": "plan-vault"},
                    {"RuleName": "daily", "TargetBackupVaultName": "plan-vault"},
                    {"RuleName": "weekly", "TargetBackupVaultName": "other-vault"},
                ],
            }, "CreatorRequestId": "deleted-plan-token"}),
        ),
        (
            "a plan carrying advanced backup settings",
            json!({"BackupPlan": {
                "BackupPlanName": "advanced-plan",
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
                "AdvancedBackupSettings": [{
                    "ResourceType": "EC2",
                    "BackupOptions": {"WindowsVSS": "enabled"},
                }],
            }}),
        ),
    ]
}

/// Once a plan's deletion is recorded, no read reports its body: the read is
/// refused with the modelled 400 `ResourceNotFoundException` for a version that
/// cannot be read, whether it names the version the create minted or none at
/// all, and reading it again answers the same.
#[tokio::test]
async fn a_deleted_backup_plan_is_no_longer_readable() {
    for (case, request) in deleted_plan_cases() {
        // One router per row: each row's plan is the only plan the account
        // holds, so a row cannot see what another row stored.
        let router = router();
        let (status, created) = create_plan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupPlan 200, got {status} {created:?}"
        );
        let plan_id = text(&created, "BackupPlanId");
        let version_id = text(&created, "VersionId");

        // The control: nothing has deleted the plan, so it reads back.
        let (status, live) = get_plan(&router, &plan_id, None).await;
        assert_eq!(
            (status, text(&live, "BackupPlanId").as_str()),
            (200, plan_id.as_str()),
            "case {case:?}: a plan nothing deleted must read back, got {status} {live:?}"
        );

        let (status, deleted) = delete_plan(&router, &plan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteBackupPlan 200, got {status} {deleted:?}"
        );

        // Every way the read can address the deleted plan's version, twice over
        // — a read changes nothing a later reader sees.
        let addressed: Vec<(&str, Option<&str>)> = vec![
            ("no version named", None),
            ("the version the create minted", Some(version_id.as_str())),
            ("a version the plan never had", Some(UNHELD_VERSION)),
        ];
        for (addressing, version) in addressed {
            for attempt in 0..2 {
                let (status, refused) = get_plan(&router, &plan_id, version).await;
                assert_eq!(
                    (
                        status,
                        error_code(&refused).as_str(),
                        text(&refused, "Message").as_str(),
                        text(&refused, "Code").as_str(),
                    ),
                    (400, NOT_FOUND, VERSION_UNREADABLE, VERSION_UNREADABLE_CODE),
                    "case {case:?} / {addressing} / read {attempt}: expected the deleted plan's \
                     version to be unreadable, got {status} {refused:?}"
                );
                assert!(
                    refused.get("BackupPlan").is_none(),
                    "case {case:?} / {addressing} / read {attempt}: a refusal reports no plan \
                     body, got {refused:?}"
                );
            }
        }

        // The listing of its selections is NOT such a read: it reports the
        // selections assigned to the plan, and a deleted plan has none, so it
        // answers an empty page (`plan_and_selection_refusals.rs`).
        let (status, listed) = list_selections(&router, &plan_id).await;
        assert_eq!(
            (
                status,
                listed.get("BackupSelectionsList").and_then(Value::as_array)
            ),
            (200, Some(&Vec::new())),
            "case {case:?}: expected the deleted plan's selections to list empty, \
             got {status} {listed:?}"
        );
    }
}

/// The two conditions a plan read refuses are ONE: a plan read answers from the
/// plan at a version, so an id the account holds no plan under, a plan whose
/// deletion is recorded and a version the plan never had all answer the
/// unreadable-version refusal, with its own `Code`.
#[tokio::test]
async fn an_unreadable_version_and_an_unheld_plan_id_answer_their_own_refusals() {
    let router = router();
    let (status, created) = create_plan(
        &router,
        &json!({"BackupPlan": {
            "BackupPlanName": "version-plan",
            "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
        }}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupPlan should succeed, got {status} {created:?}"
    );
    let plan_id = text(&created, "BackupPlanId");
    let version_id = text(&created, "VersionId");

    // (case, the plan id read, the version read, the sentence expected)
    let cases: Vec<(&str, &str, Option<&str>, &str)> = vec![
        (
            "a live plan at a version it does not have",
            plan_id.as_str(),
            Some(UNHELD_VERSION),
            VERSION_UNREADABLE,
        ),
        (
            "a live plan at a version that is one character short",
            plan_id.as_str(),
            Some(&version_id[1..]),
            VERSION_UNREADABLE,
        ),
        (
            // A plan read asks for the plan AT a version — the one named, or
            // the latest — so an id nothing answers to is again the version it
            // could not read.
            "a plan id the account never held",
            UNHELD_VERSION,
            None,
            VERSION_UNREADABLE,
        ),
        (
            "a plan id the account never held, at a version",
            UNHELD_VERSION,
            Some(version_id.as_str()),
            VERSION_UNREADABLE,
        ),
    ];
    for (case, read_id, version, message) in cases {
        let (status, refused) = get_plan(&router, read_id, version).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                text(&refused, "Message").as_str(),
                text(&refused, "Code").as_str(),
            ),
            (400, NOT_FOUND, message, VERSION_UNREADABLE_CODE),
            "case {case:?}: expected 400 {NOT_FOUND} {message:?}, got {status} {refused:?}"
        );
    }

    // The control: the live plan still reads back at the version the create
    // minted and with no version named, so the refusals above are the stored
    // state's answer rather than a read that refuses everything.
    for version in [None, Some(version_id.as_str())] {
        let (status, read) = get_plan(&router, &plan_id, version).await;
        assert_eq!(
            (status, text(&read, "BackupPlanId").as_str()),
            (200, plan_id.as_str()),
            "the live plan must read back at version {version:?}, got {status} {read:?}"
        );
        assert!(
            read.get("DeletionDate").is_none(),
            "a plan nothing deleted reports no DeletionDate, got {read:?}"
        );
    }
    // The account id is what the harness signs with, so the ARN the create
    // minted names this account's plan — the resource the refusals above are
    // about.
    assert!(
        text(&created, "BackupPlanArn").contains(ACCOUNT),
        "setup: the created plan's ARN must name this account, got {created:?}"
    );
}
