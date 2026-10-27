// SPDX-License-Identifier: CC0-1.0

//! `CreateBackupPlan` and the read that proves what it stored, driven as real
//! HTTP through the router.
//!
//! The rules under test:
//!
//! * a backup plan is a STORED resource: the create mints the plan id, the plan
//!   ARN, a version id and the creation instant, and a later `GetBackupPlan` of
//!   that id reports the body the create was given, member for member, with a
//!   `RuleId` added to each rule;
//! * a create that repeats a `CreatorRequestId` answers with the plan that token
//!   already made ("If the request includes a CreatorRequestId that matches an
//!   existing backup plan, that plan is returned"), so no second plan appears;
//! * any other create naming a plan the account already holds is refused with
//!   `AlreadyExistsException`;
//! * a plan body outside what AWS accepts — no rules at all, a rule name used
//!   twice, a window outside its documented span, a destination that is not an
//!   ARN — is refused with `InvalidParameterValueException` and stores nothing;
//! * a plan id, or a `VersionId`, the account does not hold is
//!   `ResourceNotFoundException`.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call, error_code, router, ACCOUNT, REGION};

/// The least `StartWindowMinutes` accepts, as the model documents for
/// `BackupRuleInput`'s member ("If this value is included, it must be at least
/// 60 minutes to avoid errors").
const MIN_START_WINDOW_MINUTES: i64 = 60;

/// The most either backup window accepts, as the model documents for the same
/// member ("This parameter has a maximum value of 100 years (52,560,000
/// minutes)").
const MAX_WINDOW_MINUTES: i64 = 52_560_000;

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

/// A string member of a JSON object, or the empty string.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The plan bodies the round-trip table walks: the rows differ in how many rules
/// the plan carries and in which of a rule's optional members are set, which is
/// what the "read back what was stored" rule generalises over.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    let far_away_vault = format!("arn:aws:backup:us-east-1:{ACCOUNT}:backup-vault:far-away");
    let air_gapped = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:air-gapped");
    vec![
        (
            "one rule carrying only its required members",
            json!({
                "BackupPlanName": "minimal-plan",
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
            }),
        ),
        (
            "a schedule with both windows at their documented minimum",
            json!({
                "BackupPlanName": "windowed-plan",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "ScheduleExpression": "cron(0 5 * * ? *)",
                    "StartWindowMinutes": MIN_START_WINDOW_MINUTES,
                    "CompletionWindowMinutes": 0,
                }],
            }),
        ),
        (
            "both windows at their documented maximum",
            json!({
                "BackupPlanName": "wide-window-plan",
                "Rules": [{
                    "RuleName": "centennial",
                    "TargetBackupVaultName": "plan-vault",
                    "ScheduleExpression": "cron(0 5 1 * ? *)",
                    "StartWindowMinutes": MAX_WINDOW_MINUTES,
                    "CompletionWindowMinutes": MAX_WINDOW_MINUTES,
                }],
            }),
        ),
        (
            "three rules, each with its own schedule and recovery-point tags",
            json!({
                "BackupPlanName": "multi-rule-plan",
                "Rules": [
                    {
                        "RuleName": "hourly",
                        "TargetBackupVaultName": "plan-vault",
                        "ScheduleExpression": "cron(0 * * * ? *)",
                        "RecoveryPointTags": {"tier": "hot"},
                    },
                    {
                        "RuleName": "daily",
                        "TargetBackupVaultName": "plan-vault",
                        "ScheduleExpression": "cron(0 5 * * ? *)",
                        "StartWindowMinutes": 120,
                        "CompletionWindowMinutes": 240,
                    },
                    {
                        "RuleName": "weekly.archive",
                        "TargetBackupVaultName": "other-plan-vault",
                        "ScheduleExpression": "cron(0 5 ? * SUN *)",
                        "RecoveryPointTags": {"tier": "cold", "owner": "backup-team"},
                    },
                ],
            }),
        ),
        (
            "a lifecycle and a copy action into another region's vault",
            json!({
                "BackupPlanName": "lifecycle-plan",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "ScheduleExpression": "cron(0 5 * * ? *)",
                    "Lifecycle": {
                        "MoveToColdStorageAfterDays": 30,
                        "DeleteAfterDays": 365,
                        "OptInToArchiveForSupportedResources": true,
                    },
                    "CopyActions": [{
                        "DestinationBackupVaultArn": far_away_vault,
                        "Lifecycle": {"DeleteAfterDays": 120},
                    }],
                }],
            }),
        ),
        (
            "continuous backup, a named timezone, index actions and an air-gapped target",
            json!({
                "BackupPlanName": "continuous-plan",
                "Rules": [{
                    "RuleName": "continuous",
                    "TargetBackupVaultName": "plan-vault",
                    "TargetLogicallyAirGappedBackupVaultArn": air_gapped,
                    "ScheduleExpression": "cron(0 5 * * ? *)",
                    "ScheduleExpressionTimezone": "Europe/Berlin",
                    "EnableContinuousBackup": true,
                    "IndexActions": [{"ResourceTypes": ["EBS"]}],
                }],
            }),
        ),
        (
            "advanced backup settings and scan settings beside the rules",
            json!({
                "BackupPlanName": "advanced-plan",
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
                "AdvancedBackupSettings": [{
                    "ResourceType": "EC2",
                    "BackupOptions": {"WindowsVSS": "enabled"},
                }],
                "ScanSettings": [{
                    "MalwareScanner": "GUARDDUTY",
                    "ResourceTypes": ["EBS"],
                    "ScannerRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/scanner"),
                }],
            }),
        ),
    ]
}

/// Every member a plan is created with is reported by a later read of it, and
/// the identifiers the create minted address exactly that plan.
#[tokio::test]
async fn a_created_plan_is_read_back_member_for_member() {
    for (case, plan) in round_trip_cases() {
        // One router per row: each row's plan is the only plan the account holds,
        // so a row cannot see what another row stored.
        let router = router();
        let (status, created) = create_plan(&router, &json!({"BackupPlan": plan})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBackupPlan 200, got {status} {created:?}"
        );

        let plan_id = text(&created, "BackupPlanId");
        let version_id = text(&created, "VersionId");
        assert!(
            !plan_id.is_empty() && !version_id.is_empty(),
            "case {case:?}: the create must mint a plan id and a version id, got {created:?}"
        );
        assert_eq!(
            text(&created, "BackupPlanArn"),
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-plan:{plan_id}"),
            "case {case:?}: expected the plan ARN to name the minted id, got {created:?}"
        );
        assert!(
            created["CreationDate"].is_number(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        let (status, read) = get_plan(&router, &plan_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBackupPlan 200, got {status} {read:?}"
        );
        for member in ["BackupPlanId", "BackupPlanArn", "VersionId", "CreationDate"] {
            assert_eq!(
                read.get(member),
                created.get(member),
                "case {case:?}: {member} read back differs from the created one, got {read:?}"
            );
        }
        // Nothing has deleted the plan or run one of its rules.
        for member in ["DeletionDate", "LastExecutionDate"] {
            assert!(
                read.get(member).is_none(),
                "case {case:?}: {member} must be unset on a live plan, got {read:?}"
            );
        }

        assert_eq!(
            read["BackupPlan"]["BackupPlanName"], plan["BackupPlanName"],
            "case {case:?}: expected the stored plan name, got {read:?}"
        );
        let sent_rules = plan["Rules"].as_array().cloned().unwrap_or_default();
        let read_rules = read["BackupPlan"]["Rules"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            read_rules.len(),
            sent_rules.len(),
            "case {case:?}: expected {} rule(s) read back, got {read_rules:?}",
            sent_rules.len()
        );
        let mut rule_ids: Vec<String> = Vec::new();
        for (sent, got) in sent_rules.iter().zip(read_rules.iter()) {
            for (member, value) in sent.as_object().expect("a rule is a JSON object") {
                assert_eq!(
                    got.get(member),
                    Some(value),
                    "case {case:?}: rule member {member} read back differs, got {got:?}"
                );
            }
            let rule_id = text(got, "RuleId");
            assert!(
                !rule_id.is_empty(),
                "case {case:?}: every stored rule carries a RuleId, got {got:?}"
            );
            rule_ids.push(rule_id);
        }
        let mut distinct = rule_ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            rule_ids.len(),
            "case {case:?}: each rule needs its own RuleId, got {rule_ids:?}"
        );

        // `AdvancedBackupSettings` is "populated only if the advanced option is
        // set for the backup plan", and both the create and the read answer from
        // the same stored list.
        for answer in [&created, &read] {
            assert_eq!(
                answer.get("AdvancedBackupSettings"),
                plan.get("AdvancedBackupSettings"),
                "case {case:?}: AdvancedBackupSettings must report what the plan was given, got {answer:?}"
            );
        }
        assert_eq!(
            read["BackupPlan"].get("ScanSettings"),
            plan.get("ScanSettings"),
            "case {case:?}: ScanSettings must report what the plan was given, got {read:?}"
        );
        // No preview was asked for, so none is reported.
        assert!(
            read.get("ScheduledRunsPreview").is_none(),
            "case {case:?}: a read that asked for no preview must report none, got {read:?}"
        );
    }
}

/// `BackupPlanTags` and `CreatorRequestId` are stored with the plan: the request
/// id is reported by a later read, and tagging the plan does not change what the
/// read reports about its body.
#[tokio::test]
async fn the_creator_request_id_is_reported_by_a_later_read() {
    let router = router();
    let request = json!({
        "BackupPlan": {
            "BackupPlanName": "tagged-plan",
            "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
        },
        "BackupPlanTags": {"owner": "backup-team"},
        "CreatorRequestId": "creator-request-1",
    });
    let (status, created) = create_plan(&router, &request).await;
    assert_eq!(
        status, 200,
        "expected CreateBackupPlan 200, got {status} {created:?}"
    );
    let (status, read) = get_plan(&router, &text(&created, "BackupPlanId"), None).await;
    assert_eq!(
        (status, text(&read, "CreatorRequestId").as_str()),
        (200, "creator-request-1"),
        "case \"a plan created with a CreatorRequestId\": expected it read back, got {status} {read:?}"
    );
}

/// A repeated `CreatorRequestId` answers with the plan that token already made,
/// whatever body the repeat carries; a create that is not such a replay and
/// names a plan the account holds is refused.
#[tokio::test]
async fn a_repeated_creator_request_id_replays_the_plan_it_made() {
    let first = json!({
        "BackupPlan": {
            "BackupPlanName": "replayed-plan",
            "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
        },
        "CreatorRequestId": "token-1",
    });
    // The rows differ in how much of the ORIGINAL request the repeat keeps: the
    // token is what identifies the plan, so each row replays it.
    let repeats: Vec<(&str, Value)> = vec![
        ("the same request again", first.clone()),
        (
            "the same token with another plan name",
            json!({
                "BackupPlan": {
                    "BackupPlanName": "replayed-plan-renamed",
                    "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
                },
                "CreatorRequestId": "token-1",
            }),
        ),
        (
            "the same token with another rule set",
            json!({
                "BackupPlan": {
                    "BackupPlanName": "replayed-plan",
                    "Rules": [
                        {"RuleName": "daily", "TargetBackupVaultName": "plan-vault"},
                        {"RuleName": "weekly", "TargetBackupVaultName": "plan-vault"},
                    ],
                },
                "CreatorRequestId": "token-1",
            }),
        ),
    ];

    for (case, repeat) in repeats {
        let router = router();
        let (status, created) = create_plan(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupPlan should succeed, got {status} {created:?}"
        );

        let (status, replayed) = create_plan(&router, &repeat).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed token must answer 200, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed, created,
            "case {case:?}: a replay must answer with the plan the token made, got {replayed:?}"
        );
        // And the stored plan is still the one the FIRST call created.
        let (_, read) = get_plan(&router, &text(&created, "BackupPlanId"), None).await;
        assert_eq!(
            text(&read["BackupPlan"], "BackupPlanName"),
            "replayed-plan",
            "case {case:?}: a replay must not rewrite the stored plan, got {read:?}"
        );
        assert_eq!(
            read["BackupPlan"]["Rules"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a replay must not add a rule to the stored plan, got {read:?}"
        );
    }
}

/// A create naming a plan the account already holds is refused; a create naming
/// another plan is not, and gets identifiers of its own.
#[tokio::test]
async fn a_second_plan_of_the_same_name_is_refused() {
    let plan = |name: &str, token: Option<&str>| {
        let mut request = json!({
            "BackupPlan": {
                "BackupPlanName": name,
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
            },
        });
        if let Some(token) = token {
            request["CreatorRequestId"] = json!(token);
        }
        request
    };

    let router = router();
    let (status, created) = create_plan(&router, &plan("collide", None)).await;
    assert_eq!(
        status, 200,
        "setup: the first CreateBackupPlan should succeed, got {status} {created:?}"
    );

    // (case, the repeat's name, the repeat's CreatorRequestId, is it accepted?)
    let cases: Vec<(&str, &str, Option<&str>, bool)> = vec![
        ("the same name, no token at all", "collide", None, false),
        (
            "the same name with a token the account has not seen",
            "collide",
            Some("unseen-token"),
            false,
        ),
        ("a name the account does not hold", "collide-two", None, true),
        (
            "another name with a token of its own",
            "collide-three",
            Some("another-token"),
            true,
        ),
    ];
    for (case, name, token, accepted) in cases {
        let (status, answer) = create_plan(&router, &plan(name, token)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {answer:?}"
            );
            assert_ne!(
                text(&answer, "BackupPlanId"),
                text(&created, "BackupPlanId"),
                "case {case:?}: a new plan needs an id of its own, got {answer:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "AlreadyExistsException"),
                "case {case:?}: expected 400 AlreadyExistsException, got {status} {answer:?}"
            );
        }
    }
}

/// A plan body AWS does not accept is refused with the modelled error for the
/// member at fault, and stores nothing — the name is still free afterwards.
#[tokio::test]
async fn a_plan_body_outside_what_aws_accepts_is_refused() {
    let not_an_arn = "backup-vault:far-away";
    // (case, the BackupPlan body, the expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no rules at all",
            json!({"BackupPlanName": "refused", "Rules": []}),
            "InvalidParameterValueException",
        ),
        (
            "one rule name used twice",
            json!({
                "BackupPlanName": "refused",
                "Rules": [
                    {"RuleName": "daily", "TargetBackupVaultName": "plan-vault"},
                    {"RuleName": "daily", "TargetBackupVaultName": "other-plan-vault"},
                ],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a StartWindowMinutes one below the documented minimum",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "StartWindowMinutes": MIN_START_WINDOW_MINUTES - 1,
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a StartWindowMinutes one above the documented maximum",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "StartWindowMinutes": MAX_WINDOW_MINUTES + 1,
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a negative CompletionWindowMinutes",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "CompletionWindowMinutes": -1,
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a CompletionWindowMinutes one above the documented maximum",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "CompletionWindowMinutes": MAX_WINDOW_MINUTES + 1,
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a copy-action destination that is not an ARN",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "CopyActions": [{"DestinationBackupVaultArn": not_an_arn}],
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "an air-gapped target that is not an ARN",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{
                    "RuleName": "daily",
                    "TargetBackupVaultName": "plan-vault",
                    "TargetLogicallyAirGappedBackupVaultArn": not_an_arn,
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a TargetBackupVaultName the model's pattern refuses",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "has.dot"}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a rule with no RuleName",
            json!({
                "BackupPlanName": "refused",
                "Rules": [{"TargetBackupVaultName": "plan-vault"}],
            }),
            "MissingParameterValueException",
        ),
        (
            "a plan with no Rules member",
            json!({"BackupPlanName": "refused"}),
            "MissingParameterValueException",
        ),
    ];

    for (case, plan, expected) in cases {
        let router = router();
        let (status, refused) = create_plan(&router, &json!({"BackupPlan": plan})).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {refused:?}"
        );

        // A refused create stored nothing, so the name it named is still free.
        let accepted = json!({"BackupPlan": {
            "BackupPlanName": "refused",
            "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
        }});
        let (status, created) = create_plan(&router, &accepted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused create must store nothing, got {status} {created:?}"
        );
    }

    // `BackupPlan` itself is required.
    let router = router();
    let (status, refused) = create_plan(&router, &json!({"CreatorRequestId": "no-plan"})).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "MissingParameterValueException"),
        "case \"no BackupPlan at all\": expected 400 MissingParameterValueException, got {status} {refused:?}"
    );
}

/// A plan's rule list is sized by the request, so it is bounded: a plan at the
/// ceiling this emulator keeps is stored, and one rule past it is refused with
/// the modelled `LimitExceededException` rather than stored.
#[tokio::test]
async fn the_rule_list_of_one_plan_is_bounded() {
    // The ceiling is `crate::state::MAX_RULES_PER_PLAN`; the test walks up to it
    // rather than naming the number twice.
    let rules = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| {
                json!({
                    "RuleName": format!("rule-{index}"),
                    "TargetBackupVaultName": "plan-vault",
                })
            })
            .collect()
    };

    let router = router();
    let mut stored = 0_usize;
    let mut refused: Option<(u16, String)> = None;
    for count in 1..=2_000 {
        let request = json!({"BackupPlan": {
            "BackupPlanName": format!("bounded-{count}"),
            "Rules": rules(count),
        }});
        let (status, body) = create_plan(&router, &request).await;
        if status == 200 {
            stored = count;
            continue;
        }
        refused = Some((status, error_code(&body)));
        break;
    }
    let Some((status, code)) = refused else {
        panic!("expected a ceiling on the rules of one plan, but 2000 rules were accepted");
    };
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "case \"one rule past the ceiling\": expected 400 LimitExceededException after a plan of {stored} rules, got {status} {code}"
    );
    assert!(
        stored > 1,
        "case \"the ceiling\": a plan must accept more than one rule, got {stored}"
    );
}

/// A read of a plan id, or of a version, the account does not hold is
/// `ResourceNotFoundException`; the version the create minted is found.
#[tokio::test]
async fn an_unknown_plan_or_version_is_not_found() {
    let router = router();
    let request = json!({"BackupPlan": {
        "BackupPlanName": "version-plan",
        "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "plan-vault"}],
    }});
    let (status, created) = create_plan(&router, &request).await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupPlan should succeed, got {status} {created:?}"
    );
    let plan_id = text(&created, "BackupPlanId");
    let version_id = text(&created, "VersionId");

    // (case, the plan id read, the version read, is it found?)
    let cases: Vec<(&str, &str, Option<&str>, bool)> = vec![
        ("the stored plan, no version named", plan_id.as_str(), None, true),
        (
            "the stored plan at the version the create minted",
            plan_id.as_str(),
            Some(version_id.as_str()),
            true,
        ),
        (
            "the stored plan at a version it does not have",
            plan_id.as_str(),
            Some("00000000-0000-0000-0000-000000000000"),
            false,
        ),
        (
            "a plan id the account never held",
            "00000000-0000-0000-0000-000000000000",
            None,
            false,
        ),
    ];
    for (case, read_id, version, found) in cases {
        let (status, read) = get_plan(&router, read_id, version).await;
        if found {
            assert_eq!(
                (status, text(&read, "BackupPlanId").as_str()),
                (200, plan_id.as_str()),
                "case {case:?}: expected the stored plan, got {status} {read:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&read).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected 400 ResourceNotFoundException, got {status} {read:?}"
            );
        }
    }
}
