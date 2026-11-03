// SPDX-License-Identifier: CC0-1.0

//! The resource selections of a backup plan, and the deletion of the plan that
//! holds them, driven as real HTTP through the router.
//!
//! The rules under test:
//!
//! * a resource selection is a STORED resource of the plan it is assigned to:
//!   the create mints the `SelectionId` and records the creation instant, and a
//!   later `GetBackupSelection` of the (`BackupPlanId`, `SelectionId`) pair
//!   reports the body the create was given, member for member;
//! * a create that repeats a `CreatorRequestId` answers with the selection that
//!   token already made, so no second selection appears; any other create naming
//!   a selection the plan already holds is `AlreadyExistsException`, and one
//!   naming a plan the account does not hold is
//!   `InvalidParameterValueException`;
//! * a pair the account does not hold — a selection that was never created, one
//!   that has been deleted, or a plan id nothing answers to — is
//!   `ResourceNotFoundException`;
//! * the selections of a plan are listed in `SelectionId` order, one page of
//!   `MaxResults` of them at a time, and a plan the account does not hold lists
//!   nothing but `ResourceNotFoundException`;
//! * a plan rule created without a backup window carries the service's own
//!   defaults for both windows, and one created with either window keeps the
//!   value it was given;
//! * "a backup plan can only be deleted after all associated selections of
//!   resources have been deleted": a plan that still holds one is
//!   `InvalidRequestException`, and once the plan is deleted the delete reports
//!   the `DeletionDate` it recorded, no read of the plan reports its body any
//!   more and its name becomes free again.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call, error_code, router, ACCOUNT};

/// The backup window AWS Backup applies to a rule created without one, in
/// minutes: 8 hours for the start window and 7 days for the completion window
/// (`crate::state`'s `DEFAULT_START_WINDOW_MINUTES` /
/// `DEFAULT_COMPLETION_WINDOW_MINUTES`).
const DEFAULT_START_WINDOW_MINUTES: i64 = 480;
const DEFAULT_COMPLETION_WINDOW_MINUTES: i64 = 10_080;

/// `PUT /backup/plans` — `CreateBackupPlan`'s HTTP binding.
async fn create_plan(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "PUT", "/backup/plans", Some(&request.to_string())).await
}

/// `PUT /backup/plans/{BackupPlanId}/selections` — `CreateBackupSelection`'s.
async fn create_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup/plans/{plan_id}/selections"),
        Some(&request.to_string()),
    )
    .await
}

/// `GET /backup/plans/{BackupPlanId}/selections/{SelectionId}` —
/// `GetBackupSelection`'s.
async fn get_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    selection_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        None,
    )
    .await
}

/// `DELETE /backup/plans/{BackupPlanId}/selections/{SelectionId}` —
/// `DeleteBackupSelection`'s.
async fn delete_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    selection_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        None,
    )
    .await
}

/// `GET /backup/plans/{BackupPlanId}/selections` — `ListBackupSelections`'s HTTP
/// binding, with its optional page parameters.
async fn list_selections(
    router: &MultiServiceRouter,
    plan_id: &str,
    page: Option<(usize, Option<&str>)>,
) -> (u16, Value) {
    let path = match page {
        None => format!("/backup/plans/{plan_id}/selections"),
        Some((max_results, None)) => {
            format!("/backup/plans/{plan_id}/selections?maxResults={max_results}")
        }
        Some((max_results, Some(token))) => format!(
            "/backup/plans/{plan_id}/selections?maxResults={max_results}&nextToken={token}"
        ),
    };
    call(router, "GET", &path, None).await
}

/// `DELETE /backup/plans/{BackupPlanId}` — `DeleteBackupPlan`'s.
async fn delete_plan(router: &MultiServiceRouter, plan_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/backup/plans/{plan_id}"), None).await
}

/// `GET /backup/plans/{BackupPlanId}` — `GetBackupPlan`'s.
async fn get_plan(router: &MultiServiceRouter, plan_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/backup/plans/{plan_id}"), None).await
}

/// A string member of a JSON object, or the empty string.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A plan with one minimal rule, under `name`, for a row that only needs a plan
/// to hang selections off.
fn plan_body(name: &str) -> Value {
    json!({
        "BackupPlan": {
            "BackupPlanName": name,
            "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
        }
    })
}

/// Create a plan and answer the id it minted.
async fn plan_id_of(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = create_plan(router, &plan_body(name)).await;
    assert_eq!(
        status, 200,
        "expected CreateBackupPlan 200 for {name:?}, got {status} {created:?}"
    );
    let plan_id = text(&created, "BackupPlanId");
    assert!(
        !plan_id.is_empty(),
        "expected the create of {name:?} to mint a plan id, got {created:?}"
    );
    plan_id
}

/// An IAM role ARN of this emulator's account, as a selection's `IamRoleArn`.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role}")
}

/// The selection bodies the round-trip table walks: the rows differ in HOW the
/// selection picks its resources — the three published strategies (resource
/// ARNs, tag conditions under `ListOfTags`, the richer `Conditions` with its
/// four operators) and their combinations, which is what the "read back what was
/// stored" rule generalises over.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the two required members",
            json!({
                "SelectionName": "everything",
                "IamRoleArn": role_arn("AWSBackupDefaultServiceRole"),
            }),
        ),
        (
            "two resource ARNs picked by name",
            json!({
                "SelectionName": "by-arn",
                "IamRoleArn": role_arn("S3Access"),
                "Resources": [
                    "arn:aws:s3:::probe-bucket".to_string(),
                    format!("arn:aws:dynamodb:us-west-2:{ACCOUNT}:table/probe"),
                ],
            }),
        ),
        (
            "a tag condition under ListOfTags",
            json!({
                "SelectionName": "by-tag",
                "IamRoleArn": role_arn("S3Access"),
                "ListOfTags": [{
                    "ConditionType": "STRINGEQUALS",
                    "ConditionKey": "backup",
                    "ConditionValue": "daily",
                }],
            }),
        ),
        (
            "resources included and excluded by ARN",
            json!({
                "SelectionName": "with-exclusions",
                "IamRoleArn": role_arn("S3Access"),
                "Resources": ["arn:aws:s3:::probe-*"],
                "NotResources": ["arn:aws:s3:::probe-logs"],
            }),
        ),
        (
            "every Conditions operator at once",
            json!({
                "SelectionName": "by-conditions",
                "IamRoleArn": role_arn("S3Access"),
                "Conditions": {
                    "StringEquals": [
                        {"ConditionKey": "aws:ResourceTag/backup", "ConditionValue": "daily"},
                    ],
                    "StringNotEquals": [
                        {"ConditionKey": "aws:ResourceTag/tier", "ConditionValue": "scratch"},
                    ],
                    "StringLike": [
                        {"ConditionKey": "aws:ResourceTag/app", "ConditionValue": "prod*"},
                    ],
                    "StringNotLike": [
                        {"ConditionKey": "aws:ResourceTag/app", "ConditionValue": "*test"},
                    ],
                },
            }),
        ),
    ]
}

/// Every member a selection is created with is reported by a later read of it,
/// and the pair the create minted addresses exactly that selection.
#[tokio::test]
async fn a_created_selection_is_read_back_member_for_member() {
    for (case, selection) in round_trip_cases() {
        // One router per row: each row's selection is the only one the account
        // holds, so a row cannot see what another row stored.
        let router = router();
        let plan_id = plan_id_of(&router, "selection-plan").await;

        let (status, created) =
            create_selection(&router, &plan_id, &json!({"BackupSelection": selection})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupSelection 200, got {status} {created:?}"
        );
        let selection_id = text(&created, "SelectionId");
        assert!(
            !selection_id.is_empty(),
            "case {case:?}: the create must mint a selection id, got {created:?}"
        );
        assert_eq!(
            text(&created, "BackupPlanId"),
            plan_id,
            "case {case:?}: expected the create to report the plan it was made in, got \
             {created:?}"
        );
        assert!(
            created["CreationDate"].is_number(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        let (status, read) = get_selection(&router, &plan_id, &selection_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBackupSelection 200, got {status} {read:?}"
        );
        assert_eq!(
            text(&read, "SelectionId"),
            selection_id,
            "case {case:?}: the read must report the selection asked for, got {read:?}"
        );
        assert_eq!(
            text(&read, "BackupPlanId"),
            plan_id,
            "case {case:?}: the read must report the plan that holds it, got {read:?}"
        );
        assert_eq!(
            read.get("CreationDate"),
            created.get("CreationDate"),
            "case {case:?}: CreationDate read back differs from the created one, got {read:?}"
        );
        for (member, value) in selection.as_object().expect("a selection is an object") {
            assert_eq!(
                read["BackupSelection"].get(member),
                Some(value),
                "case {case:?}: selection member {member} read back differs, got {read:?}"
            );
        }
    }
}

/// A repeated `CreatorRequestId` replays the selection that token made; any other
/// create of a selection name the plan already holds is refused, and so is one
/// naming a plan the account does not hold.
#[tokio::test]
async fn a_second_create_replays_or_is_refused() {
    let router = router();
    let plan_id = plan_id_of(&router, "replay-plan").await;
    let body = |name: &str| {
        json!({
            "BackupSelection": {
                "SelectionName": name,
                "IamRoleArn": role_arn("S3Access"),
            },
            "CreatorRequestId": "creator-request-1",
        })
    };

    let (status, first) = create_selection(&router, &plan_id, &body("tagged")).await;
    assert_eq!(
        status, 200,
        "expected the first CreateBackupSelection to answer 200, got {status} {first:?}"
    );
    let (status, replayed) = create_selection(&router, &plan_id, &body("tagged")).await;
    assert_eq!(
        status, 200,
        "expected a repeated CreatorRequestId to answer 200, got {status} {replayed:?}"
    );
    assert_eq!(
        text(&replayed, "SelectionId"),
        text(&first, "SelectionId"),
        "expected the replay to report the first selection, got {replayed:?}"
    );

    // A create with no token naming the same selection is the collision.
    let (status, refused) = create_selection(
        &router,
        &plan_id,
        &json!({
            "BackupSelection": {
                "SelectionName": "tagged",
                "IamRoleArn": role_arn("S3Access"),
            }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a duplicate selection name to be refused 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "AlreadyExistsException",
        "expected AlreadyExistsException for a duplicate selection name, got {refused:?}"
    );

    // ...and a plan nothing created holds no selection to add to.
    let (status, refused) = create_selection(
        &router,
        "00000000-0000-0000-0000-000000000000",
        &json!({
            "BackupSelection": {
                "SelectionName": "orphan",
                "IamRoleArn": role_arn("S3Access"),
            }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a selection in an unknown plan to be refused 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterValueException",
        "expected InvalidParameterValueException for an unknown BackupPlanId, got {refused:?}"
    );
}

/// Every pair the account does not hold is the same refusal: a selection id
/// nothing created, one a delete removed, and a plan id nothing answers to.
#[tokio::test]
async fn a_pair_the_account_does_not_hold_is_not_found() {
    let router = router();
    let plan_id = plan_id_of(&router, "missing-plan").await;
    let (status, created) = create_selection(
        &router,
        &plan_id,
        &json!({
            "BackupSelection": {
                "SelectionName": "transient",
                "IamRoleArn": role_arn("S3Access"),
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateBackupSelection 200, got {status} {created:?}"
    );
    let selection_id = text(&created, "SelectionId");

    let (status, deleted) = delete_selection(&router, &plan_id, &selection_id).await;
    assert_eq!(
        status, 200,
        "expected DeleteBackupSelection 200, got {status} {deleted:?}"
    );

    let unknown = "11111111-2222-3333-4444-555555555555";
    for (case, plan, selection) in [
        ("a selection the delete removed", plan_id.as_str(), selection_id.as_str()),
        ("a selection id nothing created", plan_id.as_str(), unknown),
        ("a plan id nothing created", unknown, selection_id.as_str()),
    ] {
        let (status, read) = get_selection(&router, plan, selection).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetBackupSelection 400, got {status} {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {read:?}"
        );
        // The second delete of the same pair is the same condition.
        let (status, again) = delete_selection(&router, plan, selection).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DeleteBackupSelection 400, got {status} {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the delete, got {again:?}"
        );
    }
}

/// The windows a rule is stored with: the service's own defaults where the create
/// stated none, and the stated value where it did — row by row over the four
/// combinations, with the second rule of the last row proving the fill-in is per
/// RULE and not per plan.
#[tokio::test]
async fn a_rule_carries_the_windows_it_was_created_with_or_the_service_defaults() {
    for (case, rules, expected) in [
        (
            "a rule stating neither window",
            json!([{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}]),
            vec![(
                DEFAULT_START_WINDOW_MINUTES,
                DEFAULT_COMPLETION_WINDOW_MINUTES,
            )],
        ),
        (
            "a rule stating only its start window",
            json!([{
                "RuleName": "daily",
                "TargetBackupVaultName": "plan-vault",
                "StartWindowMinutes": 60,
            }]),
            vec![(60, DEFAULT_COMPLETION_WINDOW_MINUTES)],
        ),
        (
            "a rule stating only its completion window",
            json!([{
                "RuleName": "daily",
                "TargetBackupVaultName": "plan-vault",
                "CompletionWindowMinutes": 0,
            }]),
            vec![(DEFAULT_START_WINDOW_MINUTES, 0)],
        ),
        (
            "one rule stating both windows beside one stating neither",
            json!([
                {
                    "RuleName": "hourly",
                    "TargetBackupVaultName": "plan-vault",
                    "StartWindowMinutes": 120,
                    "CompletionWindowMinutes": 240,
                },
                {"RuleName": "weekly", "TargetBackupVaultName": "plan-vault"},
            ]),
            vec![
                (120, 240),
                (
                    DEFAULT_START_WINDOW_MINUTES,
                    DEFAULT_COMPLETION_WINDOW_MINUTES,
                ),
            ],
        ),
    ] {
        let router = router();
        let (status, created) = create_plan(
            &router,
            &json!({"BackupPlan": {"BackupPlanName": "windows", "Rules": rules}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupPlan 200, got {status} {created:?}"
        );
        let plan_id = text(&created, "BackupPlanId");
        let (status, read) = get_plan(&router, &plan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBackupPlan 200, got {status} {read:?}"
        );
        let read_rules = read["BackupPlan"]["Rules"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            read_rules.len(),
            expected.len(),
            "case {case:?}: expected {} rule(s) read back, got {read_rules:?}",
            expected.len()
        );
        for (rule, (start, completion)) in read_rules.iter().zip(expected) {
            assert_eq!(
                rule.get("StartWindowMinutes").and_then(Value::as_i64),
                Some(start),
                "case {case:?}: expected StartWindowMinutes {start}, got {rule:?}"
            );
            assert_eq!(
                rule.get("CompletionWindowMinutes").and_then(Value::as_i64),
                Some(completion),
                "case {case:?}: expected CompletionWindowMinutes {completion}, got {rule:?}"
            );
        }
    }
}

/// A plan is deletable only once its selections are: the delete is refused while
/// the plan holds one, the delete reports the `DeletionDate` it recorded, no
/// later read of the plan reports its body, and the name it held is free again.
#[tokio::test]
async fn a_plan_is_deleted_only_after_its_selections_are() {
    let router = router();
    let plan_id = plan_id_of(&router, "deletable-plan").await;
    let (status, created) = create_selection(
        &router,
        &plan_id,
        &json!({
            "BackupSelection": {
                "SelectionName": "assigned",
                "IamRoleArn": role_arn("S3Access"),
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateBackupSelection 200, got {status} {created:?}"
    );
    let selection_id = text(&created, "SelectionId");

    let (status, refused) = delete_plan(&router, &plan_id).await;
    assert_eq!(
        status, 400,
        "expected a plan holding a selection to refuse the delete, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "expected InvalidRequestException while a selection is assigned, got {refused:?}"
    );
    // The refusal changed nothing: the plan is still there.
    let (status, still) = get_plan(&router, &plan_id).await;
    assert_eq!(
        status, 200,
        "expected the refused delete to leave the plan readable, got {status} {still:?}"
    );
    assert!(
        still.get("DeletionDate").is_none(),
        "expected no DeletionDate on a plan nothing deleted, got {still:?}"
    );

    let (status, gone) = delete_selection(&router, &plan_id, &selection_id).await;
    assert_eq!(
        status, 200,
        "expected DeleteBackupSelection 200, got {status} {gone:?}"
    );
    let (status, deleted) = delete_plan(&router, &plan_id).await;
    assert_eq!(
        status, 200,
        "expected DeleteBackupPlan 200 once no selection is assigned, got {status} {deleted:?}"
    );
    assert_eq!(
        text(&deleted, "BackupPlanId"),
        plan_id,
        "expected the delete to report the plan it deleted, got {deleted:?}"
    );
    assert!(
        deleted["DeletionDate"].is_number(),
        "expected the delete to report a DeletionDate, got {deleted:?}"
    );

    // A deleted plan is no longer there to read: the version the account held
    // is gone, so the read is refused rather than reporting the body again
    // (`backup_plan_deletion.rs` spans that rule).
    let (status, read) = get_plan(&router, &plan_id).await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (400, "ResourceNotFoundException"),
        "expected a deleted plan to be unreadable, got {status} {read:?}"
    );

    // ...and the account no longer holds it: a second delete is not found, and
    // the name is free for another plan.
    let (status, again) = delete_plan(&router, &plan_id).await;
    assert_eq!(
        status, 400,
        "expected a second DeleteBackupPlan to be refused 400, got {status} {again:?}"
    );
    assert_eq!(
        error_code(&again),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException from a second delete, got {again:?}"
    );
    let (status, remade) = create_plan(&router, &plan_body("deletable-plan")).await;
    assert_eq!(
        status, 200,
        "expected the deleted plan's name to be free again, got {status} {remade:?}"
    );
    assert_ne!(
        text(&remade, "BackupPlanId"),
        plan_id,
        "expected a new plan id for the re-created plan, got {remade:?}"
    );
}

/// The selections of a plan are listed in `SelectionId` order, a page at a time,
/// and the pages walked with the cursor each one carries cover the set exactly
/// once — over page sizes smaller than, equal to and larger than the set.
#[tokio::test]
async fn the_selections_of_a_plan_are_listed_a_page_at_a_time() {
    let router = router();
    let plan_id = plan_id_of(&router, "listed-plan").await;
    let mut created: Vec<String> = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let (status, body) = create_selection(
            &router,
            &plan_id,
            &json!({
                "BackupSelection": {
                    "SelectionName": name,
                    "IamRoleArn": role_arn("S3Access"),
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "expected CreateBackupSelection 200 for {name:?}, got {status} {body:?}"
        );
        created.push(text(&body, "SelectionId"));
    }
    // The ids are minted in creation order, and the list is in id order.
    created.sort();

    // One unpaginated call reports the whole set, each member carrying the
    // metadata the model declares for one.
    let (status, listed) = list_selections(&router, &plan_id, None).await;
    assert_eq!(
        status, 200,
        "expected ListBackupSelections 200, got {status} {listed:?}"
    );
    let members = listed["BackupSelectionsList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        members.len(),
        created.len(),
        "expected every selection of the plan to be listed, got {listed:?}"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "expected no cursor past the last page, got {listed:?}"
    );
    for member in &members {
        assert_eq!(
            member.get("BackupPlanId").and_then(Value::as_str),
            Some(plan_id.as_str()),
            "expected each listed selection to name its plan, got {member:?}"
        );
        assert_eq!(
            member.get("IamRoleArn").and_then(Value::as_str),
            Some(role_arn("S3Access").as_str()),
            "expected each listed selection to report its role, got {member:?}"
        );
        assert!(
            member["CreationDate"].is_number(),
            "expected each listed selection to report its creation date, got {member:?}"
        );
    }
    let listed_ids: Vec<String> = members
        .iter()
        .map(|member| text(member, "SelectionId"))
        .collect();
    assert_eq!(
        listed_ids, created,
        "expected the listing in SelectionId order, got {listed_ids:?}"
    );

    // ...and the pages a cursor walks cover the same set exactly once, whatever
    // the page size.
    for page_size in [1_usize, 2, 3, 4] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, page) =
                list_selections(&router, &plan_id, Some((page_size, token.as_deref()))).await;
            assert_eq!(
                status, 200,
                "case {page_size}: expected ListBackupSelections 200, got {status} {page:?}"
            );
            let members = page["BackupSelectionsList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                members.len() <= page_size,
                "case {page_size}: expected at most {page_size} per page, got {page:?}"
            );
            walked.extend(members.iter().map(|member| text(member, "SelectionId")));
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {page_size}: the cursor walk must terminate, got {walked:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case {page_size}: expected the pages to cover the set once in id order, got \
             {walked:?}"
        );
    }

    // A plan the account does not hold lists nothing: the resource asked about is
    // not there.
    let (status, refused) =
        list_selections(&router, "99999999-8888-7777-6666-555555555555", None).await;
    assert_eq!(
        status, 400,
        "expected ListBackupSelections on an unknown plan to be refused 400, got {status} \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for an unknown plan, got {refused:?}"
    );
}
