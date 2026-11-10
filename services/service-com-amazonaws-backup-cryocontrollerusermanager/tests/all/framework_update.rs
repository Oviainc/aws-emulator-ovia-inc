// SPDX-License-Identifier: CC0-1.0

//! `UpdateFramework`: writing a stored audit framework, and the serialization
//! AWS Backup gives a framework's deployment.
//!
//! The rules under test:
//!
//! * an update writes the members it names onto the stored framework and leaves
//!   every member it does not name as it was, so a later `DescribeFramework`
//!   reports the merged resource — and the name, the ARN and the creation
//!   instant, which no update can change, still read back as the create minted
//!   them;
//! * a framework's deployment is serialized: one action on a framework runs at a
//!   time, so an update that arrives while a previous action on that framework
//!   is still in flight is refused 409 `ConflictException` and writes nothing,
//!   whether the action in flight is the create's deployment or another update's
//!   — which makes the outcome of N concurrent updates of one name exactly one
//!   winner and N-1 refusals, with the winner's value stored verbatim;
//! * an update leaves the framework in a non-terminal deployment status that its
//!   own clock settles to a terminal one, which is the window the loser of that
//!   race is refused in;
//! * retrying an update with the `IdempotencyToken` of the update that last
//!   succeeded "results in a success message with no action taken";
//! * a `FrameworkName` the account does not hold is 400
//!   `ResourceNotFoundException`, and a value outside what the model admits is
//!   400 `InvalidParameterValueException` — neither writes anything.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees. The request harness is the shared
//! one in `vault_and_backup_job.rs` and the framework helpers are the ones in
//! `framework.rs`, so there is ONE wire path for this service's suites.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::framework::{
    create_framework, describe_framework, framework_name_of_length, text, DEPLOYMENT_COMPLETED,
    FRAMEWORK_STATUS_ACTIVE, FRAMEWORK_STATUS_UNAVAILABLE, MAX_FRAMEWORK_NAME_LENGTH,
    PAST_FRAMEWORK_SETTLE,
};
use crate::vault_and_backup_job::{call, error_code, router};

/// `DeploymentStatus` of a framework whose update is not deployed yet — one of
/// the statuses the model lists for `Framework$DeploymentStatus`
/// (`CREATE_IN_PROGRESS | UPDATE_IN_PROGRESS | DELETE_IN_PROGRESS | COMPLETED |
/// FAILED`).
pub(crate) const DEPLOYMENT_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// The longest `FrameworkDescription` the model admits (`smithy_query`
/// operation `UpdateFramework`: `FrameworkDescription`, `@length` 0..1024).
const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// `PUT /audit/frameworks/{FrameworkName}` — `UpdateFramework`'s HTTP binding.
///
/// `pub(crate)` so a sibling suite in this test binary drives the same one wire
/// path rather than growing a second copy of it.
pub(crate) async fn update_framework(
    router: &MultiServiceRouter,
    name: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/audit/frameworks/{name}"),
        Some(&request.to_string()),
    )
    .await
}

/// A description of `length` characters that satisfies the model's `\S`
/// pattern — a non-whitespace run, so the length bound is what the row probes.
fn description_of_length(length: usize) -> String {
    "description-".chars().cycle().take(length).collect()
}

/// One control, named by a published Backup Audit Manager control identifier
/// (`ControlName`: "The name of a control").
fn control(name: &str) -> Value {
    json!({"ControlName": name})
}

/// The one control every row's framework is created with, so what an update
/// does to a control list is read against a known starting point.
pub(crate) fn created_controls() -> Value {
    json!([control("BACKUP_RECOVERY_POINT_ENCRYPTED")])
}

/// The framework every row's update is applied to: the same starting state,
/// named per row so one router can hold them all.
///
/// `pub(crate)` so a sibling suite in this test binary starts its frameworks from
/// the SAME state rather than carrying a second copy of this fixture.
pub(crate) fn create_request(name: &str) -> Value {
    json!({
        "FrameworkName": name,
        "FrameworkDescription": "as created",
        "FrameworkControls": created_controls(),
    })
}

/// The rows of the merge table: each is (case, the update body, the description
/// the framework must report afterwards, the controls it must report
/// afterwards). The rows differ in WHICH members the update names, in how many
/// controls it names, in how those controls are scoped and in the length of the
/// description — which is what "an update writes the members it names and
/// leaves the rest" generalises over.
fn merge_cases() -> Vec<(&'static str, Value, Value, Value)> {
    let scoped = json!([
        {
            "ControlName": "BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK",
            "ControlInputParameters": [
                {"ParameterName": "requiredRetentionDays", "ParameterValue": "35"},
            ],
            "ControlScope": {"ComplianceResourceTypes": ["EBS", "RDS"]},
        },
        {
            "ControlName": "BACKUP_RECOVERY_POINT_MANUAL_DELETION_DISABLED",
            "ControlScope": {"Tags": {"tier": "gold"}},
        },
        {"ControlName": "BACKUP_PLAN_MIN_FREQUENCY_AND_MIN_RETENTION_CHECK"},
    ]);
    vec![
        (
            "the description alone",
            json!({"FrameworkDescription": "updated description"}),
            json!("updated description"),
            created_controls(),
        ),
        (
            "the control list alone",
            json!({"FrameworkControls": [control("BACKUP_LAST_RECOVERY_POINT_CREATED")]}),
            json!("as created"),
            json!([control("BACKUP_LAST_RECOVERY_POINT_CREATED")]),
        ),
        (
            "both, with three differently scoped controls",
            json!({"FrameworkDescription": "re-scoped", "FrameworkControls": scoped.clone()}),
            json!("re-scoped"),
            scoped,
        ),
        (
            "neither: an update that names no member at all",
            json!({}),
            json!("as created"),
            created_controls(),
        ),
        (
            "a description of one character, the shortest the pattern admits",
            json!({"FrameworkDescription": "x"}),
            json!("x"),
            created_controls(),
        ),
        (
            "a description at the longest length the model admits",
            json!({"FrameworkDescription": description_of_length(MAX_DESCRIPTION_LENGTH)}),
            json!(description_of_length(MAX_DESCRIPTION_LENGTH)),
            created_controls(),
        ),
        (
            "an idempotency token beside the members it writes",
            json!({
                "FrameworkDescription": "written with a token",
                "IdempotencyToken": "merge-row-token",
            }),
            json!("written with a token"),
            created_controls(),
        ),
    ]
}

/// A framework name for row `index` of the merge table.
fn merge_row_name(index: usize) -> String {
    format!("MergeRow{index}")
}

/// An update writes the members it names, leaves the rest as they were, and
/// changes neither the name, the ARN nor the creation instant; the framework it
/// wrote re-enters a non-terminal deployment status and settles to a terminal
/// one on its own clock.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let router = router();
    let cases = merge_cases();

    // Every row's framework is created up front, so ONE wait settles every
    // create's deployment rather than one wait per row.
    let mut created = Vec::new();
    for (index, (case, _, _, _)) in cases.iter().enumerate() {
        let name = merge_row_name(index);
        let (status, body) = create_framework(&router, &create_request(&name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {body:?}"
        );
        created.push(body);
    }
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    for (index, (case, request, expected_description, expected_controls)) in
        cases.iter().enumerate()
    {
        let name = merge_row_name(index);
        let (status, updated) = update_framework(&router, &name, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateFramework 200, got {status} {updated:?}"
        );
        for member in ["FrameworkName", "FrameworkArn"] {
            assert_eq!(
                updated.get(member),
                created[index].get(member),
                "case {case:?}: an update must report the created {member}, got {updated:?}"
            );
        }
        assert!(
            updated["CreationTime"].is_number(),
            "case {case:?}: expected the framework's CreationTime, got {updated:?}"
        );

        let (status, read) = describe_framework(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeFramework 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("FrameworkDescription"),
            Some(expected_description),
            "case {case:?}: expected the merged description, got {read:?}"
        );
        assert_eq!(
            read.get("FrameworkControls"),
            Some(expected_controls),
            "case {case:?}: expected the merged control list, got {read:?}"
        );
        assert_eq!(
            read.get("FrameworkArn"),
            created[index].get("FrameworkArn"),
            "case {case:?}: an update must not change the ARN, got {read:?}"
        );
        assert_eq!(
            read.get("CreationTime"),
            updated.get("CreationTime"),
            "case {case:?}: an update must report the framework's own creation instant, \
             got {read:?} and {updated:?}"
        );
        assert_eq!(
            (
                text(&read, "DeploymentStatus").as_str(),
                text(&read, "FrameworkStatus").as_str(),
            ),
            (DEPLOYMENT_UPDATE_IN_PROGRESS, FRAMEWORK_STATUS_UNAVAILABLE),
            "case {case:?}: a framework read straight after an update is deploying it, \
             got {read:?}"
        );
    }

    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    for (index, (case, _, expected_description, expected_controls)) in cases.iter().enumerate() {
        let name = merge_row_name(index);
        let (status, read) = describe_framework(&router, &name).await;
        assert_eq!(
            (
                status,
                text(&read, "DeploymentStatus").as_str(),
                text(&read, "FrameworkStatus").as_str(),
            ),
            (200, DEPLOYMENT_COMPLETED, FRAMEWORK_STATUS_ACTIVE),
            "case {case:?}: an update's deployment is owed by the clock, got {status} {read:?}"
        );
        assert_eq!(
            (
                read.get("FrameworkDescription"),
                read.get("FrameworkControls"),
            ),
            (Some(expected_description), Some(expected_controls)),
            "case {case:?}: settling must not change what the update wrote, got {read:?}"
        );
    }
}

/// An update that arrives while a previous action on the framework is still in
/// flight is refused `ConflictException` and writes nothing; once the clock has
/// settled that action, the same update succeeds.
///
/// The rows are the two ways an action can be in flight: the deployment a create
/// starts, and the deployment another update starts.
#[tokio::test]
async fn an_update_during_a_previous_action_is_conflict() {
    for case in ["while the create is deploying", "while an update is deploying"] {
        let router = router();
        let (status, body) = create_framework(&router, &create_request("RacedFramework")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {body:?}"
        );

        // The row that races an UPDATE waits the create's deployment out and
        // then starts one of its own, so the action in flight below is that
        // update's.
        let expected_description = if case == "while an update is deploying" {
            std::thread::sleep(PAST_FRAMEWORK_SETTLE);
            let (status, body) = update_framework(
                &router,
                "RacedFramework",
                &json!({"FrameworkDescription": "the action in flight"}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first UpdateFramework 200, got {status} {body:?}"
            );
            "the action in flight"
        } else {
            "as created"
        };

        let (status, refused) = update_framework(
            &router,
            "RacedFramework",
            &json!({
                "FrameworkDescription": "the refused update",
                "FrameworkControls": [control("BACKUP_LAST_RECOVERY_POINT_CREATED")],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected the second action to be refused, got {status} {refused:?}"
        );
        assert!(
            !text(&refused, "Message").is_empty(),
            "case {case:?}: the refusal states the condition, got {refused:?}"
        );

        // The refused update wrote nothing: the framework still reports what
        // the action in flight left on it.
        let (status, read) = describe_framework(&router, "RacedFramework").await;
        assert_eq!(
            (
                status,
                text(&read, "FrameworkDescription").as_str(),
                read.get("FrameworkControls"),
            ),
            (200, expected_description, Some(&created_controls())),
            "case {case:?}: a refused update must change nothing, got {status} {read:?}"
        );

        // Once the action in flight is settled, the same update is accepted.
        std::thread::sleep(PAST_FRAMEWORK_SETTLE);
        let (status, accepted) = update_framework(
            &router,
            "RacedFramework",
            &json!({"FrameworkDescription": "the retried update"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry after settling to succeed, got {status} \
             {accepted:?}"
        );
        let (status, read) = describe_framework(&router, "RacedFramework").await;
        assert_eq!(
            (status, text(&read, "FrameworkDescription").as_str()),
            (200, "the retried update"),
            "case {case:?}: the accepted update must be what the framework reports, got \
             {status} {read:?}"
        );
    }
}

/// Concurrent updates of ONE framework have exactly one winner: one answers 200
/// and every other is refused `ConflictException`, and the framework reports the
/// winner's description verbatim — one of the submitted values, never a blend, a
/// blank or a third value.
///
/// The rows differ in how many callers race, which is what the serialization
/// rule generalises over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_of_one_framework_have_exactly_one_winner() {
    for racers in [2_usize, 3, 5] {
        let case = format!("{racers} concurrent updates");
        let router = Arc::new(router());
        let (status, body) = create_framework(&router, &create_request("ContendedFramework")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {body:?}"
        );
        // The create's own deployment is settled first, so what the racers
        // contend over is each other rather than the create.
        std::thread::sleep(PAST_FRAMEWORK_SETTLE);

        let mut pending = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let description = format!("racer-{racer}");
            pending.push(tokio::spawn(async move {
                let (status, body) = update_framework(
                    &router,
                    "ContendedFramework",
                    &json!({"FrameworkDescription": description.clone()}),
                )
                .await;
                (description, status, body)
            }));
        }

        let mut winners = Vec::new();
        let mut refusals = Vec::new();
        for task in pending {
            let (description, status, body) = task.await.expect("a racer task to finish");
            if status == 200 {
                winners.push(description);
            } else {
                refusals.push((status, error_code(&body), body));
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: exactly one concurrent update wins, got winners {winners:?} and \
             refusals {refusals:?}"
        );
        for (status, code, body) in &refusals {
            assert_eq!(
                (*status, code.as_str()),
                (409, "ConflictException"),
                "case {case:?}: a loser is refused the modelled conflict, got {status} {body:?}"
            );
        }

        // The framework settles to a terminal deployment status carrying the
        // winner's value, and no other.
        std::thread::sleep(PAST_FRAMEWORK_SETTLE);
        let (status, read) = describe_framework(&router, "ContendedFramework").await;
        assert_eq!(
            (
                status,
                text(&read, "FrameworkDescription").as_str(),
                text(&read, "DeploymentStatus").as_str(),
            ),
            (200, winners[0].as_str(), DEPLOYMENT_COMPLETED),
            "case {case:?}: the winner's value is what the framework reports, got {status} \
             {read:?}"
        );
    }
}

/// Retrying an update with the `IdempotencyToken` of the update that last
/// succeeded takes no action: the answer is a success, the framework still
/// reports what that update wrote, and the retry starts no second deployment.
#[tokio::test]
async fn a_repeated_idempotency_token_takes_no_action() {
    let router = router();
    let (status, body) = create_framework(&router, &create_request("TokenedFramework")).await;
    assert_eq!(
        status, 200,
        "expected CreateFramework 200, got {status} {body:?}"
    );
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    let first = json!({
        "FrameworkDescription": "written once",
        "IdempotencyToken": "update-token-1",
    });
    let (status, applied) = update_framework(&router, "TokenedFramework", &first).await;
    assert_eq!(
        status, 200,
        "expected the first UpdateFramework 200, got {status} {applied:?}"
    );

    // The retries: the same body, and the same token carrying a DIFFERENT
    // description. Both are the retry of a request that already succeeded, so
    // neither acts — and neither is refused the conflict the deployment still in
    // flight would otherwise answer.
    let retries = vec![
        ("the same body", first.clone()),
        (
            "the same token with another description",
            json!({
                "FrameworkDescription": "never applied",
                "IdempotencyToken": "update-token-1",
            }),
        ),
    ];
    for (case, retry) in retries {
        let (status, replayed) = update_framework(&router, "TokenedFramework", &retry).await;
        assert_eq!(
            (status, &replayed),
            (200, &applied),
            "case {case:?}: a replayed token answers with what the update already did, got \
             {status} {replayed:?}"
        );
        let (status, read) = describe_framework(&router, "TokenedFramework").await;
        assert_eq!(
            (status, text(&read, "FrameworkDescription").as_str()),
            (200, "written once"),
            "case {case:?}: a replayed token takes no action, got {status} {read:?}"
        );
    }

    // After the deployment the first update started has settled, the retry is
    // still no action: it neither rewrites the framework nor starts a second
    // deployment.
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);
    let (status, replayed) = update_framework(&router, "TokenedFramework", &first).await;
    assert_eq!(
        (status, &replayed),
        (200, &applied),
        "a replayed token answers the same once the deployment settled, got {status} {replayed:?}"
    );
    let (status, read) = describe_framework(&router, "TokenedFramework").await;
    assert_eq!(
        (
            status,
            text(&read, "FrameworkDescription").as_str(),
            text(&read, "DeploymentStatus").as_str(),
        ),
        (200, "written once", DEPLOYMENT_COMPLETED),
        "a replayed token must start no second deployment, got {status} {read:?}"
    );

    // A NEW token on the same framework is a new action, which does write.
    let (status, second) = update_framework(
        &router,
        "TokenedFramework",
        &json!({
            "FrameworkDescription": "written twice",
            "IdempotencyToken": "update-token-2",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a new token's update to be applied, got {status} {second:?}"
    );
    let (status, read) = describe_framework(&router, "TokenedFramework").await;
    assert_eq!(
        (status, text(&read, "FrameworkDescription").as_str()),
        (200, "written twice"),
        "a new token's update writes, got {status} {read:?}"
    );
}

/// An update of a `FrameworkName` the account holds nothing under is refused by
/// the same not-found refusal the read answers, and a name it does hold is not.
#[tokio::test]
async fn an_update_of_an_unheld_name_is_resource_not_found() {
    let router = router();
    let (status, created) = create_framework(&router, &create_request("HeldForUpdate")).await;
    assert_eq!(
        status, 200,
        "expected CreateFramework 200, got {status} {created:?}"
    );
    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    for name in [
        "NeverCreated",
        "heldforupdate",
        &framework_name_of_length(MAX_FRAMEWORK_NAME_LENGTH),
    ] {
        let (status, missing) = update_framework(
            &router,
            name,
            &json!({"FrameworkDescription": "nothing to write to"}),
        )
        .await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (400, "ResourceNotFoundException"),
            "name {name:?}: expected the update to be refused, got {status} {missing:?}"
        );
        assert!(
            !text(&missing, "Message").is_empty(),
            "name {name:?}: the refusal states the condition, got {missing:?}"
        );
    }

    let (status, updated) = update_framework(
        &router,
        "HeldForUpdate",
        &json!({"FrameworkDescription": "written"}),
    )
    .await;
    assert_eq!(
        (status, updated.get("FrameworkArn")),
        (200, created.get("FrameworkArn")),
        "the name the account does hold is updated, got {status} {updated:?}"
    );
}

/// An update carrying a value AWS does not accept is refused with the modelled
/// exception for that condition and writes nothing — not the member it carried,
/// and not a deployment.
#[tokio::test]
async fn an_update_outside_what_aws_accepts_is_refused_and_writes_nothing() {
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no controls at all",
            json!({"FrameworkControls": []}),
            "InvalidParameterValueException",
        ),
        (
            "one control name used twice",
            json!({
                "FrameworkControls": [
                    control("BACKUP_RECOVERY_POINT_ENCRYPTED"),
                    control("BACKUP_RECOVERY_POINT_ENCRYPTED"),
                ],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a control scope carrying two tag pairs",
            json!({
                "FrameworkControls": [{
                    "ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED",
                    "ControlScope": {"Tags": {"tier": "gold", "owner": "backup-team"}},
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a scope resource-id list one past its documented floor",
            json!({
                "FrameworkControls": [{
                    "ControlName": "BACKUP_LAST_RECOVERY_POINT_CREATED",
                    "ControlScope": {"ComplianceResourceIds": []},
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a description one past the longest the model admits",
            json!({"FrameworkDescription": description_of_length(MAX_DESCRIPTION_LENGTH + 1)}),
            "InvalidParameterValueException",
        ),
        (
            "a description of whitespace, which the model's pattern refuses",
            json!({"FrameworkDescription": "   "}),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, expected) in cases {
        let router = router();
        let (status, body) = create_framework(&router, &create_request("GuardedFramework")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFramework 200, got {status} {body:?}"
        );
        std::thread::sleep(PAST_FRAMEWORK_SETTLE);

        let (status, refused) = update_framework(&router, "GuardedFramework", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {refused:?}"
        );

        // Nothing was written: the framework still reports what the create
        // stored, and its deployment status is still the terminal one it had
        // before the refused call — so no deployment was started either.
        let (status, read) = describe_framework(&router, "GuardedFramework").await;
        assert_eq!(
            (
                status,
                text(&read, "FrameworkDescription").as_str(),
                read.get("FrameworkControls"),
                text(&read, "DeploymentStatus").as_str(),
            ),
            (
                200,
                "as created",
                Some(&created_controls()),
                DEPLOYMENT_COMPLETED,
            ),
            "case {case:?}: a refused update must write nothing, got {status} {read:?}"
        );
    }
}
